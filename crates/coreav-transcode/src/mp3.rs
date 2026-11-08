//! External libmp3lame encoding and checked Layer III output. The process
//! writes MP3; file metadata follows ID3v2.4 and the MP3
//! Info Tag specification linked from `docs/design/transcode.md`.

use std::ffi::OsString;
use std::fmt;
use std::ops::Range;

use coreav_audio::mp3::{Header, Version};
use coreav_isobmff::Source;

/// The decoder's filterbank delay plus its one-sample alignment.
pub const DECODER_DELAY: u32 = 529;
/// Largest frame at 320 kbit/s and 32 kHz, including padding.
pub const MAX_FRAME_BYTES: u32 = 1441;
/// Largest delay or padding value in the 12-bit information-tag fields.
pub const MAX_TRIM: u32 = 4095;

/// Rate control for libmp3lame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Constant bit rate in bits per second, or the encoder's default.
    Constant(Option<u32>),
    /// Variable bit rate quality from 0 (highest) to 9.
    Quality(f32),
}

/// One source audio stream encoded to MP3 by the operator's FFmpeg.
#[derive(Clone, Debug, PartialEq)]
pub struct Mp3Audio {
    /// The source stream index, or the first audio stream CoreAV decodes.
    pub stream: Option<usize>,
    /// The encoder's rate control.
    pub mode: Mode,
}

/// Whether MP3 represents this sampling frequency without conversion.
pub fn takes_rate(rate: u32) -> bool {
    matches!(
        rate,
        8000 | 11025 | 12000 | 16000 | 22050 | 24000 | 32000 | 44100 | 48000
    )
}

/// Raw float input and Layer III frames on MP3 standard output.
pub fn encode_args(rate: u32, channels: u8, mode: Mode) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-nostdin",
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "info",
        "-f",
        "f32le",
        "-ar",
        &rate.to_string(),
        "-ac",
        &channels.to_string(),
        "-i",
        "pipe:0",
        "-map",
        "0:a:0",
        "-c:a",
        "libmp3lame",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    match mode {
        Mode::Constant(Some(rate)) => args.extend(["-b:a".into(), rate.to_string().into()]),
        Mode::Quality(quality) => args.extend(["-q:a".into(), quality.to_string().into()]),
        Mode::Constant(None) => {}
    }
    args.extend(["-f", "mp3", "pipe:1"].map(OsString::from));
    args
}

/// A conservative bound for frames and the encoder's ID3 header.
pub fn most_bytes(samples: u64) -> u64 {
    samples
        .saturating_add(u64::from(DECODER_DELAY + 2 * MAX_TRIM))
        .div_ceil(576)
        .saturating_add(1)
        .saturating_mul(u64::from(MAX_FRAME_BYTES) + 64)
        .saturating_add(64 << 10)
}

/// The PCM submitted to the external encoder.
#[derive(Clone, Copy, Debug)]
pub struct Expected {
    /// Samples per second.
    pub sample_rate: u32,
    /// Mono or stereo.
    pub channels: u8,
    /// Samples per channel submitted, including the explicit EOF guard.
    pub samples: u64,
    /// Trailing zero samples supplied only to finalize a complete frame.
    pub guard: u16,
    /// Whether quality, rather than a constant rate, was requested.
    pub variable: bool,
}

/// Checked frames retained for a bounded per-frame container writer.
pub struct Packets {
    /// Back-to-back Layer III frame bytes, without container metadata.
    pub data: Vec<u8>,
    /// Complete frame ranges in decode order.
    pub frames: Vec<Range<usize>>,
    /// Decoded samples per frame.
    pub frame_samples: u32,
    /// Mean compressed bytes per second over the complete frames.
    pub bytes_per_second: u32,
}

/// One complete Layer III frame in the encoder's output.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// Source offset of the frame header.
    pub offset: u64,
    /// Bytes including the header.
    pub size: u32,
}

/// Checked MP3 framing and gapless information.
#[derive(Debug)]
pub struct Output {
    /// Complete frames in encoding order.
    pub frames: Vec<Frame>,
    /// Samples per channel per frame.
    pub frame_samples: u32,
    /// Encoder and decoder delay in samples.
    pub priming: u32,
    pub(crate) info: Info,
}

#[derive(Debug)]
pub(crate) struct Info {
    header: u32,
    pub(crate) encoder: String,
    software: String,
    delay: u32,
    padding: u32,
    bit_rate: Option<u32>,
    variable: bool,
    music_crc: u16,
}

/// An encoder output that does not describe the submitted PCM.
#[derive(Debug)]
pub struct ReadError(String);

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the MP3 encoder's output: {}", self.0)
    }
}

impl std::error::Error for ReadError {}

fn refused(message: impl Into<String>) -> ReadError {
    ReadError(message.into())
}

fn version(text: &str, prefix: &str) -> bool {
    text.strip_prefix(prefix).is_some_and(|rest| {
        !rest.is_empty()
            && text.len() <= 64
            && rest.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    })
}

pub(crate) fn encoder_identity(stderr: &str) -> Result<String, ReadError> {
    let mut identities = stderr.lines().filter_map(|line| {
        let (key, value) = line.trim().split_once(':')?;
        if key.trim() != "encoder" {
            return None;
        }
        value.trim().strip_suffix(" libmp3lame")
    });
    let identity = identities
        .next()
        .filter(|text| version(text, "Lavc"))
        .ok_or_else(|| refused("no bounded libmp3lame encoder identity"))?;
    if identities.next().is_some() {
        return Err(refused("multiple encoder identities"));
    }
    Ok(identity.to_owned())
}

/// Reads the encoder-only MP3 pipe, checking its complete framing and
/// libmp3lame's frame-aligned sample accounting before publishing metadata.
pub fn read_output<S: Source>(
    mut source: S,
    expected: &Expected,
    limits: &coreav_mkv::Limits,
) -> Result<Output, ReadError> {
    let frame_samples = if expected.sample_rate >= 32_000 {
        1152
    } else {
        576
    };
    if !takes_rate(expected.sample_rate)
        || !matches!(expected.channels, 1 | 2)
        || expected.samples <= u64::from(expected.guard)
        || u32::from(expected.guard) >= frame_samples
        || expected.samples % u64::from(frame_samples) != 0
    {
        return Err(refused("invalid submitted PCM settings"));
    }
    let length = source.len().map_err(|e| refused(e.to_string()))?;
    if length > most_bytes(expected.samples) {
        return Err(refused("output exceeds its submitted PCM bound"));
    }
    let mut header = [0u8; 10];
    source
        .read_exact_at(0, &mut header)
        .map_err(|e| refused(e.to_string()))?;
    if header.get(..6) != Some(b"ID3\x04\0\0") {
        return Err(refused("not an unflagged ID3v2.4 header"));
    }
    let size = read_syncsafe(header.get(6..10).unwrap_or_default())?;
    if !(10..=256).contains(&size) || u64::from(size) + 10 > length {
        return Err(refused("invalid encoder metadata size"));
    }
    let mut metadata = vec![0; usize::try_from(size).map_err(|_| refused("ID3 size overflow"))?];
    source
        .read_exact_at(10, &mut metadata)
        .map_err(|e| refused(e.to_string()))?;
    if metadata.get(..4) != Some(b"TSSE") || metadata.get(8..11) != Some(&[0, 0, 3]) {
        return Err(refused("not the encoder software text frame"));
    }
    let text_size = usize::try_from(read_syncsafe(metadata.get(4..8).unwrap_or_default())?)
        .map_err(|_| refused("text size overflow"))?;
    let end = 10usize
        .checked_add(text_size)
        .ok_or_else(|| refused("text size overflow"))?;
    let software = metadata
        .get(11..end)
        .and_then(|bytes| bytes.strip_suffix(&[0]))
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .filter(|text| version(text, "Lavf"))
        .ok_or_else(|| refused("no bounded encoder software identity"))?
        .to_owned();
    if !metadata
        .get(end..)
        .is_some_and(|bytes| bytes.iter().all(|&b| b == 0))
    {
        return Err(refused("unexpected encoder metadata"));
    }
    let count = expected
        .samples
        .checked_add(1152)
        .map(|n| n / u64::from(frame_samples))
        .ok_or_else(|| refused("sample count overflow"))?;
    if count > limits.max_index_bytes / std::mem::size_of::<Frame>() as u64 {
        return Err(refused("frame index exceeds its limit"));
    }
    let mut frames = Vec::new();
    let mut offset = u64::from(size) + 10;
    let mut first = None;
    let mut first_rate = None;
    let mut variable = false;
    let mut music_crc = 0;
    let mut bytes = Vec::new();
    while offset < length {
        if frames.len() as u64 >= count {
            return Err(refused("frames after the submitted PCM"));
        }
        let mut word = [0u8; 4];
        source
            .read_exact_at(offset, &mut word)
            .map_err(|e| refused(e.to_string()))?;
        let header = Header::parse(&word).map_err(|e| refused(e.to_string()))?;
        let size = header
            .frame_bytes()
            .filter(|&n| (4..=MAX_FRAME_BYTES as usize).contains(&n))
            .ok_or_else(|| refused("invalid frame size"))?;
        if header.sample_rate != expected.sample_rate
            || header.channels() != usize::from(expected.channels)
            || header.samples() as u32 != frame_samples
            || size as u64 > length - offset
        {
            return Err(refused("a frame does not match its size, rate or channels"));
        }
        variable |= first_rate.is_some_and(|rate| rate != header.bit_rate);
        if !expected.variable && variable {
            return Err(refused(
                "different frame bit rates in a constant-rate encode",
            ));
        }
        first_rate.get_or_insert(header.bit_rate);
        first.get_or_insert(u32::from_be_bytes(word));
        bytes.resize(size, 0);
        source
            .read_exact_at(offset, &mut bytes)
            .map_err(|e| refused(e.to_string()))?;
        let side = match (header.version, header.channels()) {
            (Version::Mpeg1, 1) => 17,
            (Version::Mpeg1, _) => 32,
            (_, 1) => 9,
            _ => 17,
        };
        if frames.is_empty()
            && bytes
                .get(4 + side..8 + side)
                .is_some_and(|tag| matches!(tag, b"Xing" | b"Info"))
        {
            return Err(refused(
                "an unexpected information frame in the encoder pipe",
            ));
        }
        music_crc = crc(music_crc, &bytes);
        frames
            .try_reserve(1)
            .map_err(|_| refused("frame index allocation failed"))?;
        frames.push(Frame {
            offset,
            size: size as u32,
        });
        offset += size as u64;
    }
    if frames.len() as u64 != count {
        return Err(refused(
            "the decoded sample total does not match the submitted PCM",
        ));
    }
    Ok(Output {
        frames,
        frame_samples,
        priming: 576 + DECODER_DELAY,
        info: Info {
            header: first.ok_or_else(|| refused("no frames"))?,
            encoder: software.clone(),
            software,
            delay: 576,
            padding: 576 + u32::from(expected.guard),
            bit_rate: if expected.variable { None } else { first_rate },
            variable,
            music_crc,
        },
    })
}

fn read_syncsafe(bytes: &[u8]) -> Result<u32, ReadError> {
    if bytes.len() != 4 || bytes.iter().any(|&b| b & 128 != 0) {
        return Err(refused("invalid ID3 syncsafe size"));
    }
    Ok(bytes.iter().fold(0, |n, &b| (n << 7) | u32::from(b)))
}
fn syncsafe(n: usize) -> Result<[u8; 4], ReadError> {
    let n = u32::try_from(n)
        .ok()
        .filter(|&n| n < 1 << 28)
        .ok_or_else(|| refused("ID3 size overflow"))?;
    Ok([
        ((n >> 21) & 127) as u8,
        ((n >> 14) & 127) as u8,
        ((n >> 7) & 127) as u8,
        (n & 127) as u8,
    ])
}

fn crc(mut value: u16, bytes: &[u8]) -> u16 {
    for &byte in bytes {
        value ^= u16::from(byte);
        for _ in 0..8 {
            value = if value & 1 != 0 {
                (value >> 1) ^ 0xa001
            } else {
                value >> 1
            };
        }
    }
    value
}

pub(crate) fn metadata(info: &Info, tags: &[(String, String)]) -> Result<Vec<u8>, ReadError> {
    let size = tags
        .iter()
        .try_fold(info.software.len() + 32, |sum, (key, value)| {
            if key.contains('\0') || value.contains('\0') {
                return Err(refused("NUL in an ID3 text field"));
            }
            sum.checked_add(key.len())
                .and_then(|n| n.checked_add(value.len()))
                .and_then(|n| n.checked_add(24))
                .filter(|&n| n <= 16 << 20)
                .ok_or_else(|| refused("ID3 metadata exceeds 16 MiB"))
        })?;
    let mut frames = Vec::with_capacity(size);
    for (key, value) in tags
        .iter()
        .filter(|(key, _)| !key.eq_ignore_ascii_case("encoder"))
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .chain(std::iter::once(("encoder", info.software.as_str())))
    {
        let id = match key.to_ascii_lowercase().as_str() {
            "title" => b"TIT2",
            "artist" => b"TPE1",
            "album" => b"TALB",
            "album_artist" => b"TPE2",
            "track" => b"TRCK",
            "disc" => b"TPOS",
            "genre" => b"TCON",
            "date" => b"TDRC",
            "copyright" => b"TCOP",
            "composer" => b"TCOM",
            "encoder" => b"TSSE",
            _ => b"TXXX",
        };
        let mut data = vec![3];
        if id == b"TXXX" {
            data.extend_from_slice(key.as_bytes());
            data.push(0);
        }
        data.extend_from_slice(value.as_bytes());
        data.push(0);
        frames.extend_from_slice(id);
        frames.extend_from_slice(&syncsafe(data.len())?);
        frames.extend_from_slice(&[0, 0]);
        frames.extend_from_slice(&data);
    }
    frames.extend_from_slice(&[0; 10]);
    let mut out = b"ID3\x04\0\0".to_vec();
    out.extend_from_slice(&syncsafe(frames.len())?);
    out.extend_from_slice(&frames);
    Ok(out)
}

pub(crate) fn info_frame(info: &Info, sizes: &[u32]) -> Result<Vec<u8>, ReadError> {
    let original = Header::parse(&info.header.to_be_bytes()).map_err(|e| refused(e.to_string()))?;
    let side = match (original.version, original.channels()) {
        (Version::Mpeg1, 1) => 17,
        (Version::Mpeg1, _) => 32,
        (_, 1) => 9,
        _ => 17,
    };
    let needed = 4 + side + 120 + 36;
    let channel_mode = if original.channels() == 1 { 3 << 6 } else { 0 };
    let base = 0xffe0_0000 | (info.header & 0x001e_0c00) | (1 << 16) | channel_mode;
    let mut selected = None;
    for index in 1..15u32 {
        let word = base | (index << 12);
        let header = Header::parse(&word.to_be_bytes()).map_err(|e| refused(e.to_string()))?;
        if header.frame_bytes().is_some_and(|size| size >= needed)
            && info.bit_rate.is_none_or(|rate| header.bit_rate >= rate)
        {
            selected = Some((word, header.frame_bytes().unwrap_or(0)));
            break;
        }
    }
    let (word, size) = selected.ok_or_else(|| refused("no information frame fits"))?;
    let frame_count =
        u32::try_from(sizes.len()).map_err(|_| refused("MP3 frame count overflow"))?;
    let total = sizes.iter().try_fold(
        u32::try_from(size).map_err(|_| refused("information frame size overflow"))?,
        |n, size| {
            n.checked_add(*size)
                .ok_or_else(|| refused("MP3 byte count overflow"))
        },
    )?;
    let mut out = word.to_be_bytes().to_vec();
    out.resize(4 + side, 0);
    out.extend_from_slice(if info.variable { b"Xing" } else { b"Info" });
    out.extend_from_slice(&15u32.to_be_bytes());
    out.extend_from_slice(&frame_count.to_be_bytes());
    out.extend_from_slice(&total.to_be_bytes());
    let mut stride = 1usize;
    while sizes.len() / stride >= 400 {
        stride *= 2;
    }
    let seek_points = sizes.len() / stride;
    let mut offset = u64::try_from(size).unwrap_or(u64::MAX);
    let mut frame = 0usize;
    out.push(0);
    for percent in 1..100usize {
        let target = (seek_points * percent / 100 + 1) * stride;
        while frame < target {
            offset += u64::from(
                *sizes
                    .get(frame)
                    .ok_or_else(|| refused("seek table frame overflow"))?,
            );
            frame += 1;
        }
        out.push(u8::try_from((offset * 256 / u64::from(total)).min(255)).unwrap_or(255));
    }
    out.extend_from_slice(&0u32.to_be_bytes());
    let encoder = info.encoder.as_bytes();
    out.extend_from_slice(encoder.get(..encoder.len().min(9)).unwrap_or_default());
    out.resize(4 + side + 120 + 9, 0);
    out.extend_from_slice(&[0; 12]);
    out.extend_from_slice(&[
        ((info.delay >> 4) & 255) as u8,
        (((info.delay & 15) << 4) | (info.padding >> 8)) as u8,
        (info.padding & 255) as u8,
    ]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&info.music_crc.to_be_bytes());
    let checksum_at = out.len();
    out.extend_from_slice(&[0, 0]);
    out.resize(size.max(190), 0);
    let checksum = crc(
        0,
        out.get(..190)
            .ok_or_else(|| refused("short information-frame CRC span"))?,
    );
    out.get_mut(checksum_at..checksum_at + 2)
        .ok_or_else(|| refused("information-frame CRC offset"))?
        .copy_from_slice(&checksum.to_be_bytes());
    out.truncate(size);
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn fixture(frames: &[Vec<u8>]) -> Vec<u8> {
        let info = Info {
            header: 0,
            encoder: "Lavc1.2.3".into(),
            software: "Lavf1.2.3".into(),
            delay: 576,
            padding: 576,
            bit_rate: None,
            variable: false,
            music_crc: 0,
        };
        let mut out = metadata(&info, &[]).unwrap();
        for frame in frames {
            out.extend_from_slice(frame);
        }
        out
    }

    fn frame() -> Vec<u8> {
        let mut frame = vec![0xff, 0xfb, 0x54, 0xc4];
        frame.resize(192, 0);
        frame
    }

    fn expected() -> Expected {
        Expected {
            sample_rate: 48000,
            channels: 1,
            samples: 1152,
            guard: 0,
            variable: false,
        }
    }

    fn read(bytes: &[u8], expected: Expected) -> Result<Output, ReadError> {
        read_output(
            coreav_mkv::SliceSource::new(bytes),
            &expected,
            &coreav_mkv::Limits::default(),
        )
    }

    #[test]
    fn encoder_identity_requires_one_declared_libmp3lame_version() {
        let line = "      encoder         : Lavc63.1.102 libmp3lame\n";
        assert_eq!(encoder_identity(line).unwrap(), "Lavc63.1.102");
        for text in [
            String::new(),
            line.repeat(2),
            line.replace("Lavc", "Lavf"),
            line.replace("libmp3lame", "aac"),
            line.replace("63.1.102", "invalid"),
            line.replace("63.1.102", &"1".repeat(65)),
        ] {
            assert!(encoder_identity(&text).is_err());
        }
    }

    #[test]
    fn complete_frames_and_exact_sample_accounting_are_required() {
        let file = fixture(&[frame(), frame()]);
        let output = read(&file, expected()).unwrap();
        assert_eq!(output.frames.len(), 2);
        assert_eq!(output.priming, 1105);
        assert_eq!(output.info.delay, 576);
        assert_eq!(output.info.padding, 576);
        let guarded = read(
            &file,
            Expected {
                guard: 1151,
                ..expected()
            },
        )
        .unwrap();
        assert_eq!(guarded.info.padding, 1727);
        for bytes in [
            fixture(&[frame()]),
            fixture(&[frame(), frame(), frame()]),
            fixture(&[vec![0; 192], frame()]),
            fixture(&[vec![0xff, 0xfb, 0x54, 0xc4], frame()]),
        ] {
            assert!(read(&bytes, expected()).is_err());
        }
        for changed in [
            Expected {
                samples: 1151,
                ..expected()
            },
            Expected {
                channels: 2,
                ..expected()
            },
            Expected {
                sample_rate: 44100,
                ..expected()
            },
            Expected {
                guard: 1152,
                ..expected()
            },
            Expected {
                samples: u64::MAX,
                ..expected()
            },
        ] {
            assert!(read(&file, changed).is_err());
        }
        for end in 0..file.len() {
            assert!(read(&file[..end], expected()).is_err(), "{end}");
        }
    }

    #[test]
    fn full_output_structure_auxiliary_data_and_timestamps_are_checked() {
        let file = fixture(&[frame(), frame()]);
        for tail in [vec![0xff; 10], file.clone(), vec![0; 128]] {
            let mut appended = file.clone();
            appended.extend(tail);
            assert!(read(&appended, expected()).is_err());
        }
        for (at, value) in [
            (3, 3),
            (5, 0x10),
            (6, 128),
            (9, 255),
            (10, b'X'),
            (11, b'I'),
            (18, 1),
            (20, 1),
            (21, b'X'),
        ] {
            let mut changed = file.clone();
            changed[at] = value;
            assert!(read(&changed, expected()).is_err(), "{at}");
        }
        let mut info_frame = frame();
        info_frame[21..25].copy_from_slice(b"Info");
        assert!(read(&fixture(&[info_frame, frame()]), expected()).is_err());
        let mut changed = frame();
        changed[3] = 0;
        assert!(read(&fixture(&[frame(), changed]), expected()).is_err());
        let mut changed = frame();
        changed[2] = 0x64;
        changed.resize(240, 0);
        let bytes = fixture(&[frame(), changed]);
        assert!(read(&bytes, expected()).is_err());
        assert!(
            read(
                &bytes,
                Expected {
                    variable: true,
                    ..expected()
                }
            )
            .is_ok()
        );
        let limits = coreav_mkv::Limits {
            max_index_bytes: 0,
            ..Default::default()
        };
        assert!(read_output(coreav_mkv::SliceSource::new(&file), &expected(), &limits).is_err());
    }

    #[test]
    fn the_writer_describes_audio_frames_without_counting_its_information_frame() {
        let file = fixture(&[frame(), frame()]);
        let output = read(&file, expected()).unwrap();
        let info = info_frame(&output.info, &[192, 192]).unwrap();
        assert_eq!(&info[21..25], b"Info");
        assert_eq!(u32::from_be_bytes(info[29..33].try_into().unwrap()), 2);
        assert_eq!(u32::from_be_bytes(info[33..37].try_into().unwrap()), 576);
        assert_eq!(info.len(), 192);
        assert!(info[37..137].windows(2).all(|p| p[0] <= p[1]));
        let mut zeroed = info.clone();
        zeroed[175..177].fill(0);
        assert_eq!(
            u16::from_be_bytes(info[175..177].try_into().unwrap()),
            crc(0, &zeroed[..190])
        );
        let id3 = metadata(
            &output.info,
            &[
                ("title".into(), "A tone".into()),
                ("custom".into(), "value".into()),
            ],
        )
        .unwrap();
        assert_eq!(&id3[..6], b"ID3\x04\0\0");
        assert!(id3.windows(4).any(|s| s == b"TIT2"));
        assert!(id3.windows(4).any(|s| s == b"TXXX"));
        assert!(metadata(&output.info, &[("title".into(), "bad\0text".into())]).is_err());
        assert!(info_frame(&output.info, &[u32::MAX]).is_err());
    }
}
