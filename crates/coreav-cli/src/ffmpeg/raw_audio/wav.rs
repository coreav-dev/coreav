use super::{Checked, DecodeError, Decoded, DecoderPlan, Pass, Plan, conversion};
use crate::error::Failure;
use crate::ffprobe::audio::wav::{self, Pcm, PcmEncoding};
use coreav_audio::{Error, Limits, SampleFormat, Sink};
use coreav_isobmff::{FileSource, Source};
use std::io::Write;
use std::path::Path;

#[cfg(test)]
#[path = "wav_tests.rs"]
mod tests;

pub(super) fn write(
    command: &super::Command,
    plan: &Plan,
    before: &Pass,
    force: bool,
) -> Result<serde_json::Value, super::Stop> {
    let bytes = before
        .converted
        .as_ref()
        .and_then(|summary| {
            summary
                .output_frames
                .checked_mul(u64::from(summary.output.channels) * 2)
        })
        .ok_or_else(|| super::Refusal::not_native("WAV output size overflows"))?;
    let header =
        output_header(plan.output_rate, plan.channels.output, bytes).map_err(super::prestart)?;
    Ok(super::output::to_file(
        Path::new(&command.output),
        force,
        |out| {
            out.write_all(&header)
                .map_err(|error| super::audio_failure(Error::Output(error)))?;
            let mut payload = Payload { out, bytes: 0 };
            let after =
                super::decode(command, plan, &mut payload).map_err(DecodeError::terminal)?;
            if before != &after || payload.bytes != bytes {
                return Err(super::audio_failure(Error::Invalid(
                    "WAVE audio changed between preflight and execution",
                )));
            }
            Ok(serde_json::Value::Null)
        },
    )?)
}

fn output_header(rate: u32, channels: u16, bytes: u64) -> Result<Vec<u8>, Error> {
    let encoder = crate::ffmpeg::MUXER_ENCODER.as_bytes();
    let mut info = b"LIST\x1a\0\0\0INFOISFT\x0d\0\0\0".to_vec();
    info.extend_from_slice(encoder);
    info.extend_from_slice(&[0, 0]);
    let with_info = bytes
        .checked_add(info.len() as u64)
        .filter(|size| coreav_audio::wav::fits(*size))
        .ok_or(Error::Unsupported(
            "PCM output larger than the WAV size fields",
        ))?;
    let mask = if channels == 1 { 4 } else { 3 };
    let mut header =
        coreav_audio::wav::header(SampleFormat::S16, channels, rate, mask, Some(bytes));
    let size = u32::try_from(with_info + header.len() as u64 - 8)
        .map_err(|_| Error::Unsupported("PCM output larger than the WAV size fields"))?;
    header
        .get_mut(4..8)
        .ok_or(Error::Invalid("WAV RIFF header"))?
        .copy_from_slice(&size.to_le_bytes());
    let data = header.split_off(header.len().saturating_sub(8));
    header.extend_from_slice(&info);
    header.extend_from_slice(&data);
    Ok(header)
}

struct Payload<'a> {
    out: &'a mut dyn Write,
    bytes: u64,
}

impl Sink for Payload<'_> {
    fn start(&mut self, _: &coreav_audio::OutputFormat) -> Result<(), Error> {
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.out.write_all(bytes).map_err(Error::Output)?;
        self.bytes = self
            .bytes
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Invalid("WAV output size overflows"))?;
        Ok(())
    }
}

pub(super) fn decode(
    command: &super::Command,
    plan: &Plan,
    sink: &mut dyn Sink,
) -> Result<Pass, DecodeError> {
    let mut source = FileSource::open(Path::new(&command.input))
        .map_err(|error| DecodeError::Wave(Failure::library(error)))?;
    let size = source
        .len()
        .map_err(|error| DecodeError::Wave(Failure::library(error)))?;
    let (_, pcm) = wav::probe_pcm(
        &mut source,
        size,
        &command.input,
        &coreav_isobmff::Limits::default(),
    )
    .map_err(DecodeError::Wave)?;
    if plan.decoder != DecoderPlan::Pcm(pcm.encoding)
        || plan.eligibility != super::DecoderEligibilityKind::Exact
        || pcm.rate != plan.input_rate
        || pcm.channels != plan.channels.input
        || pcm.valid_bits != pcm.encoding.bytes() * 8
        || pcm.bytes != pcm.declared_bytes
        || pcm.bytes == 0 && pcm.offset != size
        || !matches!((pcm.channels, pcm.mask), (1, 0 | 4) | (2, 0 | 3))
    {
        return Err(DecodeError::Audio(Error::Unsupported(
            "an unregistered WAVE PCM representation or data extent",
        )));
    }
    let limits = Limits::default();
    let mut checked = Checked { sink, plan };
    let mut stream = conversion(plan, &limits, &mut checked).map_err(DecodeError::Audio)?;
    feed(&mut source, pcm, plan.options.format, |bytes| {
        stream.write(bytes)
    })?;
    let converted = stream.finish().map_err(DecodeError::Audio)?;
    Ok(Pass {
        decoded: Decoded::Pcm(pcm),
        converted: Some(converted),
    })
}

fn feed<S: Source>(
    source: &mut S,
    pcm: Pcm,
    format: SampleFormat,
    mut write: impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<(), DecodeError> {
    let width = usize::from(pcm.encoding.bytes());
    let channels = usize::from(pcm.channels);
    let packed_width = match format {
        SampleFormat::S16 => 2,
        SampleFormat::F32 => 4,
        SampleFormat::S24 | SampleFormat::S32 => {
            return Err(DecodeError::Audio(Error::Unsupported(
                "WAVE PCM stage packing",
            )));
        }
    };
    let frames = 65_536 / (width.max(packed_width) * channels);
    let capacity = frames * width * channels;
    let mut input = [0u8; 65_536];
    let mut packed = Vec::new();
    packed.try_reserve_exact(65_536).map_err(|_| {
        DecodeError::Audio(Error::LimitExceeded(coreav_audio::LimitKind::PacketBytes))
    })?;
    let mut offset = pcm.offset;
    let mut remaining = pcm.bytes;
    while remaining != 0 {
        let count = usize::try_from(remaining.min(capacity as u64))
            .map_err(|_| DecodeError::Audio(Error::Invalid("WAVE read length")))?;
        let input = input
            .get_mut(..count)
            .ok_or(DecodeError::Audio(Error::Invalid("WAVE read buffer")))?;
        source
            .read_exact_at(offset, input)
            .map_err(|error| DecodeError::Wave(Failure::library(error)))?;
        packed.clear();
        for sample in input.chunks_exact(width) {
            pack(pcm.encoding, format, sample, &mut packed).map_err(DecodeError::Audio)?;
        }
        write(&packed).map_err(DecodeError::Audio)?;
        offset = offset
            .checked_add(count as u64)
            .ok_or(DecodeError::Audio(Error::Invalid("WAVE data position")))?;
        remaining -= count as u64;
    }
    Ok(())
}

fn pack(
    encoding: PcmEncoding,
    format: SampleFormat,
    sample: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), Error> {
    if encoding == PcmEncoding::F32 && format == SampleFormat::F32 {
        let [a, b, c, d] = sample else {
            return Err(Error::Invalid("WAVE F32 sample width"));
        };
        if !f32::from_le_bytes([*a, *b, *c, *d]).is_finite() {
            return Err(Error::Unsupported("nonfinite WAVE PCM samples"));
        }
        out.extend_from_slice(sample);
        return Ok(());
    }
    let (value, bits) = match (encoding, sample) {
        (PcmEncoding::U8, [a]) => (i32::from(*a) - 128, 8),
        (PcmEncoding::S16, [a, b]) => (i32::from(i16::from_le_bytes([*a, *b])), 16),
        (PcmEncoding::S24, [a, b, c]) => (
            i32::from_le_bytes([*a, *b, *c, if c & 0x80 == 0 { 0 } else { 255 }]),
            24,
        ),
        (PcmEncoding::S32, [a, b, c, d]) => (i32::from_le_bytes([*a, *b, *c, *d]), 32),
        _ => return Err(Error::Unsupported("an unregistered WAVE PCM packing")),
    };
    match format {
        SampleFormat::S24 | SampleFormat::S32 => {
            return Err(Error::Unsupported("WAVE PCM stage packing"));
        }
        SampleFormat::S16 => {
            let value = if bits == 8 {
                value << 8
            } else {
                value >> (bits - 16)
            };
            let value = i16::try_from(value).map_err(|_| Error::Invalid("WAVE S16 packing"))?;
            out.extend_from_slice(&value.to_le_bytes());
        }
        SampleFormat::F32 => {
            let scale = match bits {
                8 => 1.0 / 128.0,
                16 => 1.0 / 32_768.0,
                24 => 1.0 / 8_388_608.0,
                _ => 1.0 / 2_147_483_648.0,
            };
            #[allow(clippy::cast_precision_loss)]
            let value = value as f32 * scale;
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(())
}
