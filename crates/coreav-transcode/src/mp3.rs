//! External MP3 encoding of raw samples, through the shared bounded process.

use std::ffi::OsString;
use std::ops::Range;
use std::path::Path;
use std::time::Instant;

use coreav_audio::mp3::Header;
use coreav_decode::{Error, ErrorKind};
use coreav_worker::{ExecLauncher, HugePages};

use crate::encoder::{EncoderLimits, Process, Sink, command};

/// Most retained compressed bytes from one encoding request.
pub const MAX_BYTES: usize = 16 << 20;

/// Complete MP3 frames, without the pipe's optional ID3 header.
pub struct Encoded {
    /// The encoder's complete output.
    pub data: Vec<u8>,
    /// Byte ranges of complete frames, in decode order.
    pub frames: Vec<Range<usize>>,
    /// Decoded samples per compressed frame.
    pub frame_samples: u32,
    /// Mean compressed bytes per second.
    pub bytes_per_second: u32,
}

fn bad(reason: &str) -> Error {
    Error::new(ErrorKind::Backend, reason)
}

/// The registered encoder-only invocation. No input container or media path
/// is passed to FFmpeg.
pub fn encode_args(rate: u32, channels: u16, bitrate: Option<u32>) -> Vec<OsString> {
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
    .map(OsString::from)
    .to_vec();
    if let Some(bitrate) = bitrate {
        args.extend([OsString::from("-b:a"), OsString::from(bitrate.to_string())]);
    }
    args.extend(["-f", "mp3", "pipe:1"].map(OsString::from));
    args
}

struct Bytes(Vec<u8>);

impl Sink for Bytes {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.0.len().saturating_add(bytes.len()) > MAX_BYTES {
            return Err(Error::new(
                ErrorKind::Limit,
                "MP3 output exceeds the 16 MiB buffer limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }
}

/// Encodes bounded interleaved f32le samples, kills stalled children at
/// `deadline`, then checks every frame before returning any payload.
pub fn encode(
    samples: &[u8],
    rate: u32,
    channels: u16,
    bitrate: Option<u32>,
    program: &Path,
    launcher: &ExecLauncher,
    deadline: Instant,
) -> Result<Encoded, Error> {
    if !matches!(channels, 1 | 2)
        || samples.is_empty()
        || samples.len() > 64 << 20
        || samples.len() % (usize::from(channels) * 4) != 0
        || !matches!(
            rate,
            8000 | 11025 | 12000 | 16000 | 22050 | 24000 | 32000 | 44100 | 48000
        )
        || bitrate.is_some_and(|b| !(8000..=320000).contains(&b))
    {
        return Err(bad(
            "MP3 needs bounded mono or stereo f32le samples at a supported sample rate and bitrate",
        ));
    }
    let args = encode_args(rate, channels, bitrate);
    let command = command(
        program,
        &args,
        HugePages::Kept,
        Some(launcher),
        &EncoderLimits::new(2 << 30),
        deadline,
    );
    let mut process = Process::spawn(
        "the MP3 encoder",
        "samples",
        command,
        program,
        deadline,
        Bytes(Vec::new()),
    )?;
    for bytes in samples.chunks(32 << 10) {
        process.enqueue(bytes.to_vec())?;
    }
    let Bytes(data) = process.finish()?;
    read_output(
        data,
        rate,
        channels,
        samples.len() / (usize::from(channels) * 4),
    )
}

fn read_output(data: Vec<u8>, rate: u32, channels: u16, samples: usize) -> Result<Encoded, Error> {
    let mut offset = 0usize;
    if data.starts_with(b"ID3") {
        let header = data
            .get(..10)
            .ok_or_else(|| bad("truncated MP3 ID3 header"))?;
        if !matches!(header.get(3), Some(3 | 4)) || header.get(5).is_none_or(|b| b & 0x10 != 0) {
            return Err(bad("unsupported MP3 ID3 header"));
        }
        let mut length = 0usize;
        for byte in header
            .get(6..10)
            .ok_or_else(|| bad("MP3 ID3 size missing"))?
        {
            if byte & 0x80 != 0 {
                return Err(bad("invalid MP3 ID3 syncsafe size"));
            }
            length = (length << 7) | usize::from(*byte);
        }
        offset = length
            .checked_add(10)
            .filter(|n| *n <= data.len() && *n <= 1 << 20)
            .ok_or_else(|| bad("MP3 ID3 tag exceeds its bounds"))?;
    }
    let mut frames = Vec::new();
    let mut frame_samples = None;
    let mut total_bytes = 0u64;
    while offset < data.len() {
        let bytes = data
            .get(offset..)
            .ok_or_else(|| bad("MP3 frame is missing"))?;
        let header = Header::parse(bytes).map_err(|_| bad("invalid MP3 encoder frame"))?;
        let duration = header.samples() as u32;
        if header.sample_rate != rate
            || header.channels() != usize::from(channels)
            || frame_samples.is_some_and(|s| s != duration)
        {
            return Err(bad(
                "MP3 encoder changed the sample rate, channels or frame clock",
            ));
        }
        frame_samples = Some(duration);
        let length = header
            .frame_bytes()
            .ok_or_else(|| bad("free-format MP3 encoder output is unsupported"))?;
        let end = offset
            .checked_add(length)
            .filter(|n| *n <= data.len())
            .ok_or_else(|| bad("truncated MP3 encoder frame"))?;
        frames.push(offset..end);
        if frames.len() > 1_000_000 {
            return Err(bad("MP3 encoder frame count exceeds its limit"));
        }
        total_bytes += length as u64;
        offset = end;
    }
    let frame_samples = frame_samples.ok_or_else(|| bad("MP3 encoder produced no frames"))?;
    let decoded = frames.len() as u64 * u64::from(frame_samples);
    if decoded < samples as u64 || decoded > samples as u64 + 3 * u64::from(frame_samples) {
        return Err(bad(
            "MP3 encoder frame count differs from its input samples",
        ));
    }
    let bytes_per_second = u32::try_from(total_bytes * u64::from(rate) / decoded)
        .map_err(|_| bad("MP3 byte rate overflows"))?;
    Ok(Encoded {
        data,
        frames,
        frame_samples,
        bytes_per_second,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn frame() -> Vec<u8> {
        let mut data = vec![0xff, 0xfb, 0x90, 0x64];
        data.resize(Header::parse(&data).unwrap().frame_bytes().unwrap(), 0);
        data
    }

    #[test]
    fn complete_frames_have_a_sample_clock_and_tags_are_not_frames() {
        let mut data = b"ID3\x04\0\0\0\0\0\0".to_vec();
        data.extend(frame());
        data.extend(frame());
        let output = read_output(data, 44100, 2, 1152).unwrap();
        assert_eq!(output.frame_samples, 1152);
        assert_eq!(output.frames.len(), 2);
        assert_eq!(output.frames[0], 10..427);
        assert_eq!(output.bytes_per_second, 834 * 44100 / 2304);
    }

    #[test]
    fn malformed_tags_truncated_frames_and_incomplete_output_are_rejected() {
        for data in [
            b"ID3".to_vec(),
            b"ID3\x04\0\0\xff\0\0\0".to_vec(),
            b"ID3\x04\0\0\0\0\0\x01".to_vec(),
            vec![0xff, 0xfb, 0x90, 0x64],
            b"not mp3".to_vec(),
            Vec::new(),
        ] {
            assert!(read_output(data, 44100, 2, 1152).is_err());
        }
        assert!(read_output(frame(), 48000, 2, 1152).is_err());
        assert!(read_output(frame(), 44100, 1, 1152).is_err());
        assert!(read_output(frame(), 44100, 2, 10000).is_err());
        let mut excessive = Vec::new();
        for _ in 0..5 {
            excessive.extend(frame());
        }
        assert!(read_output(excessive, 44100, 2, 1).is_err());
    }

    #[test]
    fn the_only_input_is_raw_samples_and_the_only_encoder_is_mp3() {
        let args = encode_args(44100, 2, Some(128000));
        let text: Vec<_> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            text,
            [
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "f32le",
                "-ar",
                "44100",
                "-ac",
                "2",
                "-i",
                "pipe:0",
                "-map",
                "0:a:0",
                "-c:a",
                "libmp3lame",
                "-b:a",
                "128000",
                "-f",
                "mp3",
                "pipe:1"
            ]
        );
    }
}
