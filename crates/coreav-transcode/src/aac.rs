//! AAC-LC audio of a transcode (`docs/design/transcode.md`, "AAC audio";
//! policy record 0042, point 5): FFmpeg's built-in `aac` encoder, an
//! operator-provided program run as a separate process, encodes the
//! decoded samples, and [`read_output`] reads what it wrote back before a
//! byte of it is used.
//!
//! The encoder writes Matroska to its standard output: of the containers
//! FFmpeg writes to a pipe, it is the one that keeps the encoder's delay
//! (`CodecDelay`). ADTS has no field for it, and FFmpeg's fragmented MP4
//! leaves the edit list out.

use std::ffi::OsString;
use std::fmt;

use coreav_isobmff::Source;
use coreav_mkv::{Allowlist, Codec, Encoding, TrackKind};

/// Samples per channel in an AAC-LC frame (ISO/IEC 14496-3 4.5.2.1).
pub const FRAME_SAMPLES: u32 = 1024;

/// The lowest bit rate the encode takes, in bits a second.
pub const MIN_BITRATE: u32 = 16_000;

/// The highest bit rate the encode takes, in bits a second.
pub const MAX_BITRATE: u32 = 512_000;

/// The largest encoder delay taken, in samples: two frames.
pub const MAX_PRIMING: u32 = 2 * FRAME_SAMPLES;

/// The largest frame taken, in bytes: the largest that ADTS carries, its
/// 13-bit `aac_frame_length` less a 7-byte header (ISO/IEC 14496-3, Annex
/// 1.A). The decoder's buffer of 6,144 bits a channel (4.5.3.1) bounds
/// the average, not each frame: FFmpeg's encoder writes larger mono frames
/// at high rates.
pub const MAX_FRAME_BYTES: u32 = 8_184;

/// Bytes of Matroska an encoder writes for a frame beyond its bytes, at
/// most: its block's header and its share of the clusters and cues.
const FRAME_OVERHEAD: u64 = 64;

/// Bytes of Matroska before the first frame, at most: the header, the
/// segment's information, its track and its tags.
const HEADER_BYTES: u64 = 64 << 10;

/// The sampling frequencies the encode takes, with their index in ISO/IEC
/// 14496-3 1.6.3.4 (Table 1.18). Above 64 kHz an `mp4a` sample entry
/// cannot carry the rate (ISO/IEC 14496-12 12.2.3, 16.16 fixed point).
const SAMPLE_RATES: [(u32, u8); 10] = [
    (64_000, 2),
    (48_000, 3),
    (44_100, 4),
    (32_000, 5),
    (24_000, 6),
    (22_050, 7),
    (16_000, 8),
    (12_000, 9),
    (11_025, 10),
    (8_000, 11),
];

/// One audio stream encoded to AAC-LC ([`crate::AudioMode::Aac`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AacAudio {
    /// The source stream, by its position among all of the source's
    /// streams; `None` for the first audio stream that CoreAV decodes.
    pub stream: Option<usize>,
    /// Bits a second, from [`MIN_BITRATE`] to [`MAX_BITRATE`]; `None` for
    /// the encoder's own default.
    pub bitrate: Option<u32>,
}

/// Whether the encode takes `rate` Hz as it is: the sampling frequencies
/// of AAC from 8 to 64 kHz.
pub fn takes_rate(rate: u32) -> bool {
    SAMPLE_RATES.iter().any(|&(r, _)| r == rate)
}

/// The `AudioSpecificConfig` of AAC-LC at `rate` Hz with `channels` (1 or
/// 2) channels (ISO/IEC 14496-3 1.6.2.1 and 4.4.1): object type 2, the
/// rate's index, the channel configuration, 1,024-sample frames, no core
/// coder and no extension. `None` for another rate or channel count.
pub fn audio_specific_config(rate: u32, channels: u8) -> Option<[u8; 2]> {
    let &(_, index) = SAMPLE_RATES.iter().find(|&&(r, _)| r == rate)?;
    if !matches!(channels, 1 | 2) {
        return None;
    }
    Some([
        (2 << 3) | (index >> 1),
        ((index & 1) << 7) | (channels << 3),
    ])
}

/// What may follow the `AudioSpecificConfig` of AAC-LC: the backward
/// compatible signalling that the stream has no SBR (ISO/IEC 14496-3
/// 1.6.2.1 and 1.6.6.2: `syncExtensionType` 0x2B7, extension object type
/// 5, `sbrPresentFlag` 0, then byte alignment), so that a decoder of
/// HE-AAC does not look for it.
pub const NO_SBR: [u8; 3] = [0x56, 0xE5, 0x00];

/// The encoder's command line, after the program: interleaved 32-bit
/// float samples at `rate` Hz with `channels` channels on standard input,
/// AAC-LC by the `aac` encoder at `bitrate` bits a second (the encoder's
/// default when `None`), Matroska on standard output.
pub fn encode_args(rate: u32, channels: u8, bitrate: Option<u32>) -> Vec<OsString> {
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
        "aac",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    if let Some(bitrate) = bitrate {
        args.push("-b:a".into());
        args.push(bitrate.to_string().into());
    }
    args.extend(["-f", "matroska", "pipe:1"].map(OsString::from));
    args
}

/// The command line that asks the program about its `aac` encoder.
pub fn probe_args() -> Vec<OsString> {
    ["-hide_banner", "-loglevel", "error", "-h", "encoder=aac"]
        .map(OsString::from)
        .to_vec()
}

/// Whether the program's answer to [`probe_args`] describes the `aac`
/// encoder; a program without it says it does not know the codec.
pub fn has_encoder(answer: &[u8]) -> bool {
    answer.starts_with(b"Encoder aac [")
}

/// The most output that `samples` samples per channel of `channels`
/// make: as many frames as cover the largest delay and the samples, and
/// one more, each of the largest size, with their Matroska.
pub fn most_bytes(samples: u64) -> u64 {
    let frames = u64::from(MAX_PRIMING)
        .saturating_add(samples)
        .div_ceil(u64::from(FRAME_SAMPLES))
        .saturating_add(1);
    let frame = u64::from(MAX_FRAME_BYTES) + FRAME_OVERHEAD;
    frames.saturating_mul(frame).saturating_add(HEADER_BYTES)
}

/// What the encoder was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected {
    /// The samples' rate in Hz.
    pub sample_rate: u32,
    /// The samples' channels.
    pub channels: u8,
    /// Samples per channel sent.
    pub samples: u64,
}

/// One AAC frame in the encoder's output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Offset of its bytes in the output.
    pub offset: u64,
    /// Its size in bytes.
    pub size: u32,
}

/// The encoder's output, read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// The stream's `AudioSpecificConfig`: that of AAC-LC at the rate and
    /// channels asked, followed by [`NO_SBR`] or by nothing.
    pub config: Vec<u8>,
    /// The encoder's delay in samples: not presented.
    pub priming: u32,
    /// The frames, 1,024 samples each, in order.
    pub frames: Vec<Frame>,
}

/// Why the encoder's output was refused.
#[derive(Debug)]
pub struct ReadError(String);

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the AAC encoder's output: {}", self.0)
    }
}

impl std::error::Error for ReadError {}

fn refused(message: impl Into<String>) -> ReadError {
    ReadError(message.into())
}

/// Reads the encoder's output back (policy record 0042, point 3): one
/// Matroska audio track of AAC with the `AudioSpecificConfig` of AAC-LC at
/// the rate and channels asked (followed by [`NO_SBR`] or by nothing),
/// stored as it is; a delay of at most
/// [`MAX_PRIMING`] samples (`CodecDelay`, rounded to the nearest sample);
/// frames of 1 to [`MAX_FRAME_BYTES`] bytes; and, for the samples sent, as many
/// frames as cover the delay and the samples, or one more. The frames are
/// counted while they are read, so an output with more is refused before
/// they are all listed.
pub fn read_output<S: Source>(
    mut source: S,
    expected: &Expected,
    limits: &coreav_mkv::Limits,
) -> Result<Output, ReadError> {
    let config =
        audio_specific_config(expected.sample_rate, expected.channels).ok_or_else(|| {
            refused(format!(
                "AAC-LC has no configuration for {} Hz and {} channels",
                expected.sample_rate, expected.channels
            ))
        })?;
    let matroska = |e: coreav_mkv::Error| refused(format!("its Matroska: {e}"));
    let segment = coreav_mkv::probe(&mut source, limits, &Allowlist::all()).map_err(matroska)?;
    let [track] = segment.tracks.as_slice() else {
        return Err(refused("not one track"));
    };
    if !segment.dropped.is_empty() {
        return Err(refused("not one track"));
    }
    if track.kind != TrackKind::Audio || track.codec != Codec::Aac {
        return Err(refused("not an AAC track"));
    }
    let private = track.codec_private.as_deref().unwrap_or_default();
    if private != config && private != [&config[..], &NO_SBR[..]].concat() {
        return Err(refused(
            "its AudioSpecificConfig is not that of AAC-LC at the rate and channels asked",
        ));
    }
    if track.encoding != Encoding::None {
        return Err(refused("its frames are not stored as they are"));
    }
    let audio = track
        .audio
        .as_ref()
        .ok_or_else(|| refused("no audio settings"))?;
    if audio.sampling_frequency.to_bits() != f64::from(expected.sample_rate).to_bits()
        || audio.channels != u64::from(expected.channels)
    {
        return Err(refused("its rate or channels are not those asked"));
    }
    let priming = u32::try_from(
        (u128::from(track.codec_delay) * u128::from(expected.sample_rate) + 500_000_000)
            / 1_000_000_000,
    )
    .ok()
    .filter(|&p| p <= MAX_PRIMING)
    .ok_or_else(|| refused("a delay of more than two frames"))?;
    let needed =
        (u64::from(priming).saturating_add(expected.samples)).div_ceil(u64::from(FRAME_SAMPLES));
    let most = needed.saturating_add(1);
    let mut frames = Vec::new();
    for packet in segment.packets(&mut source, limits) {
        let packet = packet.map_err(matroska)?;
        if u64::try_from(frames.len()).unwrap_or(u64::MAX) >= most {
            return Err(refused(format!(
                "more frames than {} samples make",
                expected.samples
            )));
        }
        let size = u32::try_from(packet.stored_size)
            .ok()
            .filter(|&s| packet.stripped == 0 && (1..=MAX_FRAME_BYTES).contains(&s))
            .ok_or_else(|| refused(format!("a frame of {} bytes", packet.stored_size)))?;
        frames.push(Frame {
            offset: packet.offset,
            size,
        });
    }
    let count = u64::try_from(frames.len()).unwrap_or(u64::MAX);
    if count < needed {
        return Err(refused(format!(
            "{count} frames for {} samples and a delay of {priming}",
            expected.samples
        )));
    }
    Ok(Output {
        config: private.to_vec(),
        priming,
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_mkv::SliceSource;

    fn element(id: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.push(0x01);
        out.extend_from_slice(&u64::try_from(payload.len()).unwrap().to_be_bytes()[1..]);
        out.extend_from_slice(payload);
        out
    }

    fn uint(id: &[u8], value: u64) -> Vec<u8> {
        element(id, &value.to_be_bytes())
    }

    /// A Matroska file of one audio track, `codec` with `private`, a
    /// delay of `delay_ns`, at `rate` Hz with `channels` channels, and
    /// one block a frame of each size in `frames`.
    fn matroska(
        codec: &str,
        private: &[u8],
        delay_ns: u64,
        rate: u32,
        channels: u8,
        frames: &[usize],
    ) -> Vec<u8> {
        let header = [
            uint(&[0x42, 0x86], 1),
            uint(&[0x42, 0xF7], 1),
            uint(&[0x42, 0xF2], 4),
            uint(&[0x42, 0xF3], 8),
            element(&[0x42, 0x82], b"matroska"),
            uint(&[0x42, 0x87], 4),
            uint(&[0x42, 0x85], 2),
        ]
        .concat();
        let audio = [
            element(&[0xB5], &f64::from(rate).to_be_bytes()),
            uint(&[0x9F], u64::from(channels)),
        ]
        .concat();
        let track = [
            uint(&[0xD7], 1),
            uint(&[0x73, 0xC5], 1),
            uint(&[0x83], 2),
            element(&[0x86], codec.as_bytes()),
            element(&[0x63, 0xA2], private),
            uint(&[0x56, 0xAA], delay_ns),
            element(&[0xE1], &audio),
        ]
        .concat();
        let mut cluster = uint(&[0xE7], 0);
        for (i, &size) in frames.iter().enumerate() {
            let mut block = vec![0x81];
            block.extend_from_slice(&i16::try_from(i * 21).unwrap().to_be_bytes());
            block.push(0x80);
            block.extend(std::iter::repeat_n(0x21u8, size));
            cluster.extend(element(&[0xA3], &block));
        }
        let segment = [
            element(
                &[0x15, 0x49, 0xA9, 0x66],
                &uint(&[0x2A, 0xD7, 0xB1], 1_000_000),
            ),
            element(&[0x16, 0x54, 0xAE, 0x6B], &element(&[0xAE], &track)),
            element(&[0x1F, 0x43, 0xB6, 0x75], &cluster),
        ]
        .concat();
        [
            element(&[0x1A, 0x45, 0xDF, 0xA3], &header),
            element(&[0x18, 0x53, 0x80, 0x67], &segment),
        ]
        .concat()
    }

    const STEREO_48K: [u8; 2] = [0x11, 0x90];

    /// 1,024 samples at 48 kHz, in nanoseconds, as FFmpeg writes it.
    const DELAY: u64 = 21_333_333;

    fn read(file: &[u8], samples: u64) -> Result<Output, ReadError> {
        read_output(
            SliceSource::new(file),
            &Expected {
                sample_rate: 48_000,
                channels: 2,
                samples,
            },
            &coreav_mkv::Limits::default(),
        )
    }

    #[test]
    fn audio_specific_configs() {
        assert_eq!(audio_specific_config(48_000, 2), Some(STEREO_48K));
        assert_eq!(audio_specific_config(44_100, 2), Some([0x12, 0x10]));
        assert_eq!(audio_specific_config(44_100, 1), Some([0x12, 0x08]));
        assert_eq!(audio_specific_config(8_000, 1), Some([0x15, 0x88]));
        assert_eq!(audio_specific_config(96_000, 2), None);
        assert_eq!(audio_specific_config(48_000, 6), None);
        assert!(takes_rate(22_050));
        assert!(!takes_rate(88_200));
        assert!(!takes_rate(37_800));
    }

    #[test]
    fn the_command_lines() {
        let args = |b| {
            encode_args(44_100, 1, b)
                .into_iter()
                .map(|a| a.into_string().unwrap())
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(
            args(None),
            "-nostdin -hide_banner -loglevel error -f f32le -ar 44100 -ac 1 -i pipe:0 \
             -map 0:a:0 -c:a aac -f matroska pipe:1"
        );
        assert!(args(Some(64_000)).contains(" -c:a aac -b:a 64000 -f matroska "));
        assert!(has_encoder(b"Encoder aac [AAC (Advanced Audio Coding)]:\n"));
        assert!(!has_encoder(b"Codec 'aac' is not recognized by FFmpeg.\n"));
        assert!(!has_encoder(b""));
    }

    #[test]
    fn the_output_is_bounded_by_the_samples() {
        // The largest delay, the samples and one frame more, of 8,184
        // bytes and 64 of Matroska each, and the header.
        assert_eq!(most_bytes(0), 3 * 8_248 + 65_536);
        assert_eq!(most_bytes(1), 4 * 8_248 + 65_536);
        assert_eq!(most_bytes(u64::MAX), u64::MAX);
    }

    #[test]
    fn an_encoders_output_is_read() {
        let file = matroska("A_AAC", &STEREO_48K, DELAY, 48_000, 2, &[300, 1536, 7]);
        let output = read(&file, 2048).unwrap();
        assert_eq!(output.config, STEREO_48K);
        // With the signalling that there is no SBR, as FFmpeg writes it.
        let explicit = [&STEREO_48K[..], &NO_SBR[..]].concat();
        let file = matroska("A_AAC", &explicit, DELAY, 48_000, 2, &[300, 1536, 7]);
        assert_eq!(read(&file, 2048).unwrap().config, explicit);
        let file = matroska("A_AAC", &STEREO_48K, DELAY, 48_000, 2, &[300, 1536, 7]);
        assert_eq!(output.priming, 1024);
        let sizes: Vec<u32> = output.frames.iter().map(|f| f.size).collect();
        assert_eq!(sizes, [300, 1536, 7]);
        for f in &output.frames {
            let at = usize::try_from(f.offset).unwrap();
            assert!(
                file[at..at + usize::try_from(f.size).unwrap()]
                    .iter()
                    .all(|&b| b == 0x21)
            );
        }
        // One frame more than the delay and the samples need is taken.
        assert!(read(&file, 2047).is_ok());
        assert!(read(&file, 1025).is_ok());
    }

    #[test]
    fn invalid_output_is_refused() {
        let frames = [100; 3];
        for (file, samples, says) in [
            (b"not matroska".to_vec(), 2048, "its Matroska"),
            (
                matroska("A_AAC", &STEREO_48K, DELAY, 48_000, 2, &frames),
                2049,
                "3 frames for 2049",
            ),
            (
                matroska("A_AAC", &STEREO_48K, DELAY, 48_000, 2, &[100; 4]),
                1024,
                "more frames than 1024",
            ),
            (
                matroska("A_OPUS", &STEREO_48K, DELAY, 48_000, 2, &frames),
                2048,
                "not an AAC track",
            ),
            (
                matroska("A_AAC", &[0x12, 0x10], DELAY, 48_000, 2, &frames),
                2048,
                "AudioSpecificConfig",
            ),
            (
                matroska("A_AAC", &[0x11, 0x90, 0], DELAY, 48_000, 2, &frames),
                2048,
                "AudioSpecificConfig",
            ),
            (
                matroska("A_AAC", &STEREO_48K, DELAY, 44_100, 2, &frames),
                2048,
                "rate or channels",
            ),
            (
                matroska("A_AAC", &STEREO_48K, DELAY, 48_000, 1, &frames),
                2048,
                "rate or channels",
            ),
            (
                matroska("A_AAC", &STEREO_48K, 64_000_000, 48_000, 2, &frames),
                2048,
                "more than two frames",
            ),
            (
                matroska("A_AAC", &STEREO_48K, DELAY, 48_000, 2, &[100, 0, 100]),
                2048,
                "of 0 bytes",
            ),
            (
                matroska("A_AAC", &STEREO_48K, DELAY, 48_000, 2, &[100, 8_185, 100]),
                2048,
                "of 8185 bytes",
            ),
        ] {
            let e = read(&file, samples).unwrap_err().to_string();
            assert!(e.contains(says), "{says}: {e}");
            assert!(e.starts_with("the AAC encoder's output: "), "{e}");
        }
    }
}
