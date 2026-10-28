//! WAV output: the RIFF `WAVE` header (`fmt ` and `data` chunks) for
//! interleaved little-endian PCM.
//!
//! The header carries `WAVE_FORMAT_EXTENSIBLE` with the channel mask when
//! there are more than two channels or more than 16 bits per sample, as
//! Microsoft's WAVE format notes ask, and the plain `WAVE_FORMAT_PCM` (1)
//! or `WAVE_FORMAT_IEEE_FLOAT` (3) otherwise. The sizes are written as
//! `0xFFFFFFFF` while the length is unknown (a pipe), the convention
//! streaming writers use, and patched once it is known; a length past the
//! 32-bit fields keeps `0xFFFFFFFF` ([`crate::Warning::WavTooLarge`]).

use crate::decode::{OutputFormat, Sink};
use crate::{Error, SampleFormat};
use std::io::Write;

/// The header for `channels` channels of `format` at `sample_rate` with
/// the speaker positions `mask` (`dwChannelMask`), for `data_bytes` bytes
/// of samples, or an unknown length: 44 bytes for the plain formats, 68
/// for the extensible one. Its length depends only on the format and the
/// channel count, so it can be rewritten in place once the length is known.
pub fn header(
    format: SampleFormat,
    channels: u16,
    sample_rate: u32,
    mask: u32,
    data_bytes: Option<u64>,
) -> Vec<u8> {
    let bits = format.bits();
    let block_align = channels.saturating_mul(bits / 8);
    let byte_rate = sample_rate.saturating_mul(u32::from(block_align));
    let extensible = channels > 2 || bits > 16;
    let tag: u16 = match (extensible, format) {
        (true, _) => 0xFFFE,
        (false, SampleFormat::F32) => 3,
        (false, _) => 1,
    };
    let header_bytes: u64 = if extensible { 68 } else { 44 };
    let riff_size = data_bytes
        .and_then(|d| d.checked_add(header_bytes - 8))
        .and_then(|d| u32::try_from(d).ok())
        .unwrap_or(u32::MAX);
    let data_size = data_bytes
        .and_then(|d| u32::try_from(d).ok())
        .filter(|&d| d != u32::MAX)
        .unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(68);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    if extensible {
        out.extend_from_slice(&40u32.to_le_bytes());
    } else {
        out.extend_from_slice(&16u32.to_le_bytes());
    }
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    if extensible {
        out.extend_from_slice(&22u16.to_le_bytes());
        // wValidBitsPerSample, dwChannelMask, then the subformat GUID:
        // KSDATAFORMAT_SUBTYPE_PCM or _IEEE_FLOAT.
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(&mask.to_le_bytes());
        let code: u16 = if format == SampleFormat::F32 { 3 } else { 1 };
        out.extend_from_slice(&code.to_le_bytes());
        out.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
        ]);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_size.to_le_bytes());
    out
}

/// Whether `data_bytes` fits the header's 32-bit sizes.
pub fn fits(data_bytes: u64) -> bool {
    data_bytes < u64::from(u32::MAX) - 60
}

/// A [`Sink`] that writes a WAV file ([`crate::wav`]) to `out`.
///
/// The header's sizes are `0xFFFFFFFF` until the length is known;
/// [`WavSink::header`] gives the final header, for a caller that can seek
/// back and write it at the start of the output.
pub struct WavSink<W: Write> {
    out: W,
    format: Option<OutputFormat>,
    data_bytes: u64,
}

impl<W: Write> WavSink<W> {
    /// A sink writing to `out`.
    pub fn new(out: W) -> Self {
        Self {
            out,
            format: None,
            data_bytes: 0,
        }
    }

    /// Bytes of samples written.
    pub fn data_bytes(&self) -> u64 {
        self.data_bytes
    }

    /// The header with the sizes of what was written, and whether they fit
    /// its 32-bit fields; `None` before [`Sink::start`].
    pub fn header(&self) -> Option<(Vec<u8>, bool)> {
        let format = self.format?;
        let fits = fits(self.data_bytes);
        let header = header(
            format.format,
            format.channels,
            format.sample_rate,
            format.channel_mask,
            fits.then_some(self.data_bytes),
        );
        Some((header, fits))
    }

    /// The writer.
    pub fn into_inner(self) -> W {
        self.out
    }
}

impl<W: Write> Sink for WavSink<W> {
    fn start(&mut self, format: &OutputFormat) -> Result<(), Error> {
        self.format = Some(*format);
        let header = header(
            format.format,
            format.channels,
            format.sample_rate,
            format.channel_mask,
            None,
        );
        self.out.write_all(&header).map_err(Error::Output)
    }

    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        self.data_bytes = self.data_bytes.saturating_add(samples.len() as u64);
        self.out.write_all(samples).map_err(Error::Output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_extensible_headers() {
        let plain = header(SampleFormat::S16, 2, 44_100, 3, Some(4));
        assert_eq!(plain.len(), 44);
        assert_eq!(&plain[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(plain[4..8].try_into().unwrap()), 40);
        assert_eq!(u16::from_le_bytes([plain[20], plain[21]]), 1);
        assert_eq!(&plain[36..40], b"data");
        assert_eq!(u32::from_le_bytes(plain[40..44].try_into().unwrap()), 4);
        let wide = header(SampleFormat::S24, 6, 48_000, 0x3F, None);
        assert_eq!(wide.len(), 68);
        assert_eq!(u16::from_le_bytes([wide[20], wide[21]]), 0xFFFE);
        assert_eq!(u32::from_le_bytes(wide[4..8].try_into().unwrap()), u32::MAX);
        assert_eq!(
            u32::from_le_bytes(wide[64..68].try_into().unwrap()),
            u32::MAX
        );
    }
}
