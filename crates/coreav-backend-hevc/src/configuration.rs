use coreav_decode::{ConfigKind, Error, ErrorKind, StreamConfig};
use coreav_hevc::NalHeader;

use crate::{MAX_NALS, codec_error, invalid, limit};

#[derive(Clone, Copy)]
pub(super) enum Framing {
    AnnexB,
    Length(u8),
}

pub(super) struct Configuration<'a> {
    pub(super) framing: Framing,
    data: Option<&'a [u8]>,
}

impl<'a> Configuration<'a> {
    pub(super) fn parse(stream: &'a StreamConfig) -> Result<Self, Error> {
        let Some(config) = &stream.config else {
            return Ok(Self {
                framing: Framing::AnnexB,
                data: None,
            });
        };
        if !matches!(config.kind, ConfigKind::Hvcc | ConfigKind::MatroskaPrivate) {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "HEVC requires hvcC or Annex B",
            ));
        }
        let data = config.data.as_slice();
        if data.len() > coreav_hevc::Limits::default().max_nal_bytes {
            return Err(limit("HEVC configuration is too large"));
        }
        if data.len() < 23 || data.first() != Some(&1) {
            return Err(invalid("invalid hvcC header"));
        }
        let width = (data.get(21).copied().ok_or(invalid("truncated hvcC"))? & 3) + 1;
        if width == 3 {
            return Err(invalid("invalid hvcC NAL length size"));
        }
        let value = Self {
            framing: Framing::Length(width),
            data: Some(data),
        };
        value.feed(|_| Ok(()))?;
        Ok(value)
    }

    pub(super) fn feed(
        &self,
        mut feed: impl FnMut(&[u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let Some(data) = self.data else {
            return Ok(());
        };
        let arrays = data.get(22).copied().ok_or(invalid("truncated hvcC"))?;
        if arrays > 64 {
            return Err(limit("too many hvcC arrays"));
        }
        let mut remaining = data.get(23..).ok_or(invalid("truncated hvcC"))?;
        let mut total = 0usize;
        let mut seen = 0u64;
        for _ in 0..arrays {
            let (&header, tail) = remaining
                .split_first()
                .ok_or(invalid("truncated hvcC array"))?;
            if header & 0x40 != 0 {
                return Err(invalid("reserved hvcC array bit"));
            }
            let unit_type = header & 63;
            let bit = 1u64 << unit_type;
            if seen & bit != 0 {
                return Err(invalid("duplicate hvcC array"));
            }
            seen |= bit;
            remaining = tail;
            let count = length(&mut remaining)?;
            total = total
                .checked_add(count)
                .filter(|n| *n <= MAX_NALS)
                .ok_or(limit("too many hvcC NAL units"))?;
            for _ in 0..count {
                let length = length(&mut remaining)?;
                let nal = remaining
                    .get(..length)
                    .ok_or(invalid("truncated hvcC NAL"))?;
                if NalHeader::parse(nal).map_err(codec_error)?.unit_type != unit_type {
                    return Err(invalid("hvcC array NAL type mismatch"));
                }
                if !matches!(unit_type, 32..=34 | 39 | 40) {
                    return Err(invalid("hvcC array is not initialization data"));
                }
                feed(
                    coreav_hevc::packet_nal(nal, coreav_hevc::Limits::default().max_nal_bytes)
                        .map_err(codec_error)?
                        .0,
                )?;
                remaining = remaining
                    .get(length..)
                    .ok_or(invalid("truncated hvcC NAL"))?;
            }
        }
        if !remaining.is_empty() {
            return Err(invalid("trailing hvcC bytes"));
        }
        Ok(())
    }
}

fn length(remaining: &mut &[u8]) -> Result<usize, Error> {
    let bytes = remaining
        .first_chunk::<2>()
        .ok_or(invalid("truncated hvcC length"))?;
    let length = usize::from(u16::from_be_bytes(*bytes));
    *remaining = remaining.get(2..).ok_or(invalid("truncated hvcC length"))?;
    Ok(length)
}
