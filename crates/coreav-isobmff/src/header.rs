//! Sans-IO box header parsing (ISO/IEC 14496-12, 4.2).

use crate::bytes::{be_u32_at, be_u64_at};
use crate::{Error, FourCc};

/// Longest possible box header: 32-bit size, type, 64-bit size and a
/// 16-byte `uuid` extended type.
pub const MAX_HEADER_LEN: usize = 32;

/// A parsed box header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxHeader {
    /// The box type.
    pub kind: FourCc,
    /// Absolute offset of the first header byte.
    pub offset: u64,
    /// Header length in bytes (8, 16, 24 or 32).
    pub header_len: u8,
    /// Total box size including the header.
    pub size: u64,
}

impl BoxHeader {
    /// Absolute offset of the first payload byte.
    pub fn payload_offset(&self) -> u64 {
        self.offset.saturating_add(u64::from(self.header_len))
    }

    /// Payload length in bytes.
    pub fn payload_len(&self) -> u64 {
        self.size.saturating_sub(u64::from(self.header_len))
    }

    /// Absolute offset just past the box.
    pub fn end(&self) -> u64 {
        self.offset.saturating_add(self.size)
    }
}

/// Result of a sans-IO parse step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parsed<T> {
    /// The value.
    Done(T),
    /// At least this many bytes (counted from the start of the input) are
    /// needed before the value can be parsed.
    NeedMore(usize),
}

/// Parses one box header from `bytes`, which start at absolute `offset`.
///
/// `room` is the number of bytes from `offset` to the end of the enclosing
/// box or file. A box that claims more than `room` is reported as
/// [`Error::Truncated`]; a size smaller than its own header is invalid.
pub fn parse_box_header(bytes: &[u8], offset: u64, room: u64) -> Result<Parsed<BoxHeader>, Error> {
    let Some(size32) = be_u32_at(bytes, 0) else {
        return need(8, room, offset);
    };
    let Some(kind) = be_u32_at(bytes, 4) else {
        return need(8, room, offset);
    };
    let kind = FourCc(kind.to_be_bytes());
    let mut header_len: u8 = 8;
    let size = match size32 {
        0 => room,
        1 => {
            let Some(large) = be_u64_at(bytes, 8) else {
                return need(16, room, offset);
            };
            header_len = 16;
            large
        }
        other => u64::from(other),
    };
    if kind == FourCc(*b"uuid") {
        if bytes.len() < usize::from(header_len) + 16 {
            return need(usize::from(header_len) + 16, room, offset);
        }
        header_len = header_len.saturating_add(16);
    }
    if size < u64::from(header_len) {
        return Err(Error::InvalidBox {
            offset,
            kind,
            reason: "size smaller than header",
        });
    }
    if size > room {
        return Err(Error::Truncated { offset });
    }
    Ok(Parsed::Done(BoxHeader {
        kind,
        offset,
        header_len,
        size,
    }))
}

fn need(count: usize, room: u64, offset: u64) -> Result<Parsed<BoxHeader>, Error> {
    if (count as u64) > room {
        Err(Error::Truncated { offset })
    } else {
        Ok(Parsed::NeedMore(count))
    }
}

/// Reads and parses the box header at `at` from a source; `end` bounds the
/// enclosing box or file.
pub(crate) fn read_header<S: crate::Source>(
    source: &mut S,
    at: u64,
    end: u64,
) -> Result<BoxHeader, Error> {
    let room = end.checked_sub(at).ok_or(Error::Truncated { offset: at })?;
    let mut buf = [0u8; MAX_HEADER_LEN];
    let n = usize::try_from(room.min(MAX_HEADER_LEN as u64)).unwrap_or(MAX_HEADER_LEN);
    let slice = buf.get_mut(..n).ok_or(Error::Truncated { offset: at })?;
    source.read_exact_at(at, slice)?;
    match parse_box_header(slice, at, room)? {
        Parsed::Done(header) => Ok(header),
        Parsed::NeedMore(_) => Err(Error::Truncated { offset: at }),
    }
}

/// Reads a box payload of at most `max` bytes, allocating fallibly.
pub(crate) fn read_payload<S: crate::Source>(
    source: &mut S,
    header: &BoxHeader,
    max: u64,
    limit: crate::LimitKind,
) -> Result<Vec<u8>, Error> {
    let offset = header.offset;
    if header.payload_len() > max {
        return Err(Error::LimitExceeded { limit, offset });
    }
    let size = usize::try_from(header.payload_len())
        .map_err(|_| Error::LimitExceeded { limit, offset })?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(size)
        .map_err(|_| Error::LimitExceeded { limit, offset })?;
    // Read into the reserved capacity, without zeroing it first.
    source.read_append(header.payload_offset(), size, &mut payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_and_large_sizes() {
        let compact = [0, 0, 0, 16, b'f', b't', b'y', b'p'];
        let Ok(Parsed::Done(h)) = parse_box_header(&compact, 4, 100) else {
            panic!("compact header");
        };
        assert_eq!(
            (h.kind, h.size, h.header_len, h.end()),
            (FourCc(*b"ftyp"), 16, 8, 20)
        );

        let mut large = vec![0, 0, 0, 1, b'm', b'd', b'a', b't'];
        large.extend_from_slice(&40u64.to_be_bytes());
        let Ok(Parsed::Done(h)) = parse_box_header(&large, 0, 40) else {
            panic!("large header");
        };
        assert_eq!((h.size, h.header_len), (40, 16));
        assert_eq!(
            parse_box_header(&large[..8], 0, 40),
            Ok(Parsed::NeedMore(16))
        );
    }

    #[test]
    fn hostile_sizes() {
        let tiny = [0, 0, 0, 7, b'f', b'r', b'e', b'e'];
        assert!(matches!(
            parse_box_header(&tiny, 0, 100),
            Err(Error::InvalidBox { .. })
        ));
        let huge = [0xff, 0xff, 0xff, 0xff, b'f', b'r', b'e', b'e'];
        assert_eq!(
            parse_box_header(&huge, 9, 100),
            Err(Error::Truncated { offset: 9 })
        );
        let mut large = vec![0, 0, 0, 1, b'm', b'd', b'a', b't'];
        large.extend_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(
            parse_box_header(&large, 0, 100),
            Err(Error::Truncated { offset: 0 })
        );
        let to_end = [0, 0, 0, 0, b'm', b'd', b'a', b't'];
        let Ok(Parsed::Done(h)) = parse_box_header(&to_end, 0, 77) else {
            panic!("size zero");
        };
        assert_eq!(h.size, 77);
        assert_eq!(
            parse_box_header(&[0, 0], 0, 4),
            Err(Error::Truncated { offset: 0 })
        );
    }
}
