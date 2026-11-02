//! External libmp3lame encoding and checked Layer III output. The process
//! writes Matroska (RFC 9559); file metadata follows ID3v2.4 and the MP3
//! Info Tag specification linked from `docs/design/transcode.md`.

use std::ffi::OsString;
use std::fmt;

use coreav_audio::mp3::{Header, Version};
use coreav_isobmff::Source;
use coreav_mkv::{Allowlist, Codec, Encoding, TrackKind};

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

/// Raw float input and one MP3 track on Matroska standard output.
pub fn encode_args(rate: u32, channels: u8, mode: Mode) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
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
    args.extend(["-f", "matroska", "pipe:1"].map(OsString::from));
    args
}

/// A conservative bound for frames, Matroska headers and block overhead.
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
    encoder: String,
    software: String,
    delay: u32,
    padding: u32,
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

/// Reads only one uncompressed MP3 track, validating each frame and the
/// exact sample total after CodecDelay and final DiscardPadding.
pub fn read_output<S: Source>(
    mut source: S,
    expected: &Expected,
    limits: &coreav_mkv::Limits,
) -> Result<Output, ReadError> {
    if !takes_rate(expected.sample_rate)
        || !matches!(expected.channels, 1 | 2)
        || expected.samples <= u64::from(expected.guard)
    {
        return Err(refused("invalid submitted PCM settings"));
    }
    let mkv = |error: coreav_mkv::Error| refused(format!("its Matroska: {error}"));
    let segment =
        coreav_mkv::probe_with_metadata(&mut source, limits, &Allowlist::all()).map_err(mkv)?;
    let [track] = segment.tracks.as_slice() else {
        return Err(refused("not one track"));
    };
    if segment.timestamp_scale == 0 || segment.timestamp_scale > 1_000_000 {
        return Err(refused(
            "encoder timestamps are coarser than one millisecond",
        ));
    }
    if !segment.dropped.is_empty()
        || !segment.warnings.is_empty()
        || track.kind != TrackKind::Audio
        || track.codec != Codec::Mp3
        || track.encoding != Encoding::None
        || track.codec_private.as_ref().is_some_and(|v| !v.is_empty())
    {
        return Err(refused("not one uncompressed MP3 track"));
    }
    let audio = track
        .audio
        .as_ref()
        .ok_or_else(|| refused("no audio settings"))?;
    if audio.sampling_frequency.to_bits() != f64::from(expected.sample_rate).to_bits()
        || audio.channels != u64::from(expected.channels)
    {
        return Err(refused("its rate or channels are not those submitted"));
    }
    let to_samples = |ns: u64| {
        u32::try_from(
            (u128::from(ns) * u128::from(expected.sample_rate) + 500_000_000) / 1_000_000_000,
        )
        .map_err(|_| refused("sample trim overflow"))
    };
    let priming = to_samples(track.codec_delay)?;
    let delay = priming
        .checked_sub(DECODER_DELAY)
        .filter(|&n| n <= MAX_TRIM)
        .ok_or_else(|| refused("an unrepresentable encoder delay"))?;
    let software = segment
        .muxing_app
        .as_deref()
        .filter(|s| version(s, "Lavf"))
        .ok_or_else(|| refused("no bounded encoder software identity"))?
        .to_owned();
    let encoder = segment
        .tags
        .iter()
        .filter(|tag| track.uid.is_some_and(|uid| tag.track_uids.contains(&uid)))
        .flat_map(|tag| &tag.simple_tags)
        .filter(|tag| tag.name == "ENCODER")
        .filter_map(|tag| tag.string.as_deref())
        .find_map(|s| s.strip_suffix(" libmp3lame").filter(|s| version(s, "Lavc")))
        .ok_or_else(|| refused("no libmp3lame encoder identity"))?
        .to_owned();
    let most = expected
        .samples
        .saturating_add(u64::from(DECODER_DELAY + 2 * MAX_TRIM))
        .div_ceil(576);
    let mut frames = Vec::new();
    let mut total = 0u64;
    let mut padding = None;
    let mut first = None;
    let mut first_rate = None;
    let mut frame_samples = 0;
    let mut music_crc = 0;
    let mut bytes = Vec::new();
    let mut packets = segment.packets(&mut source, limits).with_audit();
    while let Some(packet) = packets.next() {
        let packet = packet.map_err(mkv)?;
        if packet.track != 0
            || packet.block_additions.is_some()
            || packet.codec_state
            || padding.is_some()
            || u64::try_from(frames.len()).unwrap_or(u64::MAX) >= most
        {
            return Err(refused("frames after the final padding or too many frames"));
        }
        let size = u32::try_from(packet.stored_size)
            .ok()
            .filter(|n| packet.stripped == 0 && (4..=MAX_FRAME_BYTES).contains(n))
            .ok_or_else(|| refused("invalid frame size"))?;
        bytes.resize(
            usize::try_from(size).map_err(|_| refused("frame size overflow"))?,
            0,
        );
        packets
            .source_mut()
            .read_exact_at(packet.offset, &mut bytes)
            .map_err(|e| refused(e.to_string()))?;
        let header = Header::parse(&bytes).map_err(|e| refused(e.to_string()))?;
        let expected_ns = i128::from(total) * 1_000_000_000 / i128::from(expected.sample_rate)
            - i128::from(track.codec_delay);
        let delta = packet
            .pts_ns(&segment)
            .and_then(|pts| pts.checked_sub(expected_ns))
            .ok_or_else(|| refused("an unknown or overflowing frame timestamp"))?;
        if delta.unsigned_abs() > u128::from(segment.timestamp_scale) {
            return Err(refused(
                "a frame timestamp does not match the sample sequence",
            ));
        }
        let frame_ns = u128::try_from(header.samples())
            .map_err(|_| refused("frame duration overflow"))?
            * 1_000_000_000
            / u128::from(expected.sample_rate);
        let trim_ns = u128::try_from(packet.discard_padding.unwrap_or(0))
            .map_err(|_| refused("negative discard padding"))?;
        if trim_ns > frame_ns + 1 {
            return Err(refused("discard padding exceeds its frame"));
        }
        let presented_ns = frame_ns.saturating_sub(trim_ns);
        if packet.block_duration.is_some_and(|n| {
            (u128::from(n) * u128::from(segment.timestamp_scale)).abs_diff(presented_ns)
                > u128::from(segment.timestamp_scale)
        }) || track
            .default_duration
            .is_some_and(|n| u128::from(n).abs_diff(frame_ns) > 1)
        {
            return Err(refused(format!(
                "a frame duration does not match its header: block {:?} ticks, default {:?} ns, frame {frame_ns} ns",
                packet.block_duration, track.default_duration
            )));
        }
        music_crc = crc(music_crc, &bytes);
        if header.sample_rate != expected.sample_rate
            || header.channels() != usize::from(expected.channels)
            || header.frame_bytes() != Some(bytes.len())
        {
            return Err(refused(
                "a frame header does not match its size, rate or channels",
            ));
        }
        if !expected.variable && first_rate.is_some_and(|rate| rate != header.bit_rate) {
            return Err(refused(
                "different frame bit rates in a constant-rate encode",
            ));
        }
        first_rate.get_or_insert(header.bit_rate);
        let word = <[u8; 4]>::try_from(bytes.get(..4).unwrap_or_default())
            .map_err(|_| refused("no frame header"))?;
        first.get_or_insert(u32::from_be_bytes(word));
        frame_samples =
            u32::try_from(header.samples()).map_err(|_| refused("frame sample overflow"))?;
        total = total
            .checked_add(u64::from(frame_samples))
            .ok_or_else(|| refused("sample count overflow"))?;
        if let Some(ns) = packet.discard_padding.filter(|&n| n != 0) {
            let ns = u64::try_from(ns).map_err(|_| refused("negative discard padding"))?;
            padding = Some(to_samples(ns)?);
        }
        frames.push(Frame {
            offset: packet.offset,
            size,
        });
    }
    if !packets.warnings().is_empty() {
        return Err(refused("invalid packet listing"));
    }
    let discarded = padding.unwrap_or(0);
    if total.checked_sub(u64::from(priming) + u64::from(discarded)) != Some(expected.samples) {
        return Err(refused(
            "the decoded sample total does not match the submitted PCM",
        ));
    }
    let padding = discarded
        .checked_add(DECODER_DELAY)
        .and_then(|n| n.checked_add(u32::from(expected.guard)))
        .filter(|&n| n <= MAX_TRIM)
        .ok_or_else(|| refused("unrepresentable final padding"))?;
    Ok(Output {
        frames,
        frame_samples,
        priming,
        info: Info {
            header: first.ok_or_else(|| refused("no frames"))?,
            encoder,
            software,
            delay,
            padding,
            variable: expected.variable,
            music_crc,
        },
    })
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn element(id: &[u8], bytes: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.push(1);
        out.extend_from_slice(&u64::try_from(bytes.len()).unwrap().to_be_bytes()[1..]);
        out.extend_from_slice(bytes);
        out
    }

    fn uint(id: &[u8], n: u64) -> Vec<u8> {
        element(id, &n.to_be_bytes())
    }

    pub(crate) fn fixture(
        codec: &str,
        delay: u64,
        frames: &[Vec<u8>],
        padding: &[Option<i64>],
    ) -> Vec<u8> {
        fixture_extra(codec, delay, frames, padding, &[])
    }

    fn fixture_extra(
        codec: &str,
        delay: u64,
        frames: &[Vec<u8>],
        padding: &[Option<i64>],
        extra: &[u8],
    ) -> Vec<u8> {
        let header = [
            uint(&[0x42, 0x86], 1),
            uint(&[0x42, 0xf7], 1),
            uint(&[0x42, 0xf2], 4),
            uint(&[0x42, 0xf3], 8),
            element(&[0x42, 0x82], b"matroska"),
            uint(&[0x42, 0x87], 4),
            uint(&[0x42, 0x85], 2),
        ]
        .concat();
        let audio = [element(&[0xb5], &48000f64.to_be_bytes()), uint(&[0x9f], 1)].concat();
        let track = [
            uint(&[0xd7], 1),
            uint(&[0x73, 0xc5], 1),
            uint(&[0x83], 2),
            element(&[0x86], codec.as_bytes()),
            uint(&[0x56, 0xaa], delay),
            element(&[0xe1], &audio),
        ]
        .concat();
        let info = [
            uint(&[0x2a, 0xd7, 0xb1], 1_000_000),
            element(&[0x4d, 0x80], b"Lavf1.2.3"),
            element(&[0x57, 0x41], b"Lavf1.2.3"),
        ]
        .concat();
        let tag = [
            element(&[0x63, 0xc0], &uint(&[0x63, 0xc5], 1)),
            element(
                &[0x67, 0xc8],
                &[
                    element(&[0x45, 0xa3], b"ENCODER"),
                    element(&[0x44, 0x87], b"Lavc1.2.3 libmp3lame"),
                ]
                .concat(),
            ),
        ]
        .concat();
        let mut cluster = uint(&[0xe7], 0);
        for (index, frame) in frames.iter().enumerate() {
            let mut block = vec![0x81];
            block.extend_from_slice(&i16::try_from(index * 24).unwrap().to_be_bytes());
            block.push(0);
            block.extend_from_slice(frame);
            let mut group = element(&[0xa1], &block);
            if index == 0 {
                group.extend_from_slice(extra);
            }
            if let Some(padding) = padding.get(index).copied().flatten() {
                group.extend(element(&[0x75, 0xa2], &padding.to_be_bytes()));
            }
            cluster.extend(element(&[0xa0], &group));
        }
        let segment = [
            element(&[0x15, 0x49, 0xa9, 0x66], &info),
            element(&[0x16, 0x54, 0xae, 0x6b], &element(&[0xae], &track)),
            element(&[0x12, 0x54, 0xc3, 0x67], &element(&[0x73, 0x73], &tag)),
            element(&[0x1f, 0x43, 0xb6, 0x75], &cluster),
        ]
        .concat();
        [
            element(&[0x1a, 0x45, 0xdf, 0xa3], &header),
            element(&[0x18, 0x53, 0x80, 0x67], &segment),
        ]
        .concat()
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
    fn complete_frames_and_exact_sample_accounting_are_required() {
        let frames = [frame(), frame()];
        let file = fixture("A_MPEG/L3", 23_020_833, &frames, &[None, Some(979_167)]);
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
            fixture("A_AAC", 23_020_833, &frames, &[None, Some(979_167)]),
            fixture("A_MPEG/L3", 0, &frames, &[None, Some(979_167)]),
            fixture("A_MPEG/L3", 23_020_833, &frames, &[Some(979_167), None]),
            fixture("A_MPEG/L3", 23_020_833, &frames, &[None, Some(-1)]),
            fixture(
                "A_MPEG/L3",
                23_020_833,
                &[vec![0; 192], frame()],
                &[None, Some(979_167)],
            ),
            fixture(
                "A_MPEG/L3",
                23_020_833,
                &[vec![0xff, 0xfb, 0x54, 0xc4], frame()],
                &[None, Some(979_167)],
            ),
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
        ] {
            assert!(read(&file, changed).is_err());
        }
        for end in 0..file.len() {
            assert!(read(&file[..end], expected()).is_err(), "{end}");
        }
    }

    #[test]
    fn full_output_structure_auxiliary_data_and_timestamps_are_checked() {
        let frames = [frame(), frame()];
        let padding = [None, Some(979_167)];
        let file = fixture("A_MPEG/L3", 23_020_833, &frames, &padding);
        for tail in [vec![0xff; 10], file.clone()] {
            let mut appended = file.clone();
            appended.extend(tail);
            assert!(read(&appended, expected()).is_err());
        }
        for extra in [
            element(&[0xa4], b"state"),
            element(
                &[0x75, 0xa1],
                &element(&[0xa6], &element(&[0xa5], b"addition")),
            ),
            uint(&[0x9b], 1000),
        ] {
            let bytes = fixture_extra("A_MPEG/L3", 23_020_833, &frames, &padding, &extra);
            assert!(read(&bytes, expected()).is_err());
        }
        let mut moved = file;
        let frame_offsets: Vec<_> = moved
            .windows(4)
            .enumerate()
            .filter_map(|(i, bytes)| (bytes == [0xff, 0xfb, 0x54, 0xc4]).then_some(i))
            .collect();
        let second = frame_offsets[1];
        moved[second - 3..second - 1].copy_from_slice(&(-30000i16).to_be_bytes());
        assert!(read(&moved, expected()).is_err());
    }

    #[test]
    fn the_writer_describes_audio_frames_without_counting_its_information_frame() {
        let file = fixture(
            "A_MPEG/L3",
            23_020_833,
            &[frame(), frame()],
            &[None, Some(979_167)],
        );
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

pub(crate) fn info_frame(info: &Info, sizes: &[u32]) -> Result<Vec<u8>, ReadError> {
    let original = Header::parse(&info.header.to_be_bytes()).map_err(|e| refused(e.to_string()))?;
    let side = match (original.version, original.channels()) {
        (Version::Mpeg1, 1) => 17,
        (Version::Mpeg1, _) => 32,
        (_, 1) => 9,
        _ => 17,
    };
    let needed = (4 + side + 120 + 36).max(190);
    let base = 0xffe0_0000 | (info.header & 0x001e_0cc0) | (1 << 16);
    let mut selected = None;
    for index in 1..15u32 {
        let word = base | (index << 12);
        let header = Header::parse(&word.to_be_bytes()).map_err(|e| refused(e.to_string()))?;
        if header.frame_bytes().is_some_and(|size| size >= needed)
            && header.bit_rate >= original.bit_rate
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
    let mut offset = u64::try_from(size).unwrap_or(u64::MAX);
    let mut frame = 0usize;
    for percent in 0..100usize {
        let target = sizes.len().saturating_mul(percent).div_ceil(100);
        while frame < target {
            offset += u64::from(
                *sizes
                    .get(frame)
                    .ok_or_else(|| refused("seek table frame overflow"))?,
            );
            frame += 1;
        }
        out.push(if percent == 0 {
            0
        } else {
            u8::try_from((offset * 256 / u64::from(total)).min(255)).unwrap_or(255)
        });
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
    out.resize(size, 0);
    let checksum = crc(
        0,
        out.get(..190)
            .ok_or_else(|| refused("short information-frame CRC span"))?,
    );
    out.get_mut(checksum_at..checksum_at + 2)
        .ok_or_else(|| refused("information-frame CRC offset"))?
        .copy_from_slice(&checksum.to_be_bytes());
    Ok(out)
}
