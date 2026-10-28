//! Sans-IO EBML primitives (RFC 8794): variable-size integers, element
//! headers and the element value types.

use crate::Error;

/// Longest element header Matroska allows: a 4-octet ID (RFC 9559 4.3,
/// `EBMLMaxIDLength` 4) and an 8-octet data size.
pub const MAX_HEADER_LEN: usize = 12;

/// Longest element ID, in octets (RFC 9559 4.3).
const MAX_ID_LEN: usize = 4;

/// A parsed element header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElementHeader {
    /// The element ID with its marker bits, as written (for example
    /// `0x1A45DFA3` for the EBML header).
    pub id: u32,
    /// Absolute offset of the first ID octet.
    pub offset: u64,
    /// Length of the ID and data size, in octets (2 to 12).
    pub header_len: u8,
    /// Element data size; `None` for an unknown-sized element (RFC 8794
    /// 6.2).
    pub size: Option<u64>,
}

impl ElementHeader {
    /// Absolute offset of the first data octet.
    pub fn data_offset(&self) -> u64 {
        self.offset.saturating_add(u64::from(self.header_len))
    }

    /// Absolute offset just past the element; `None` for an unknown size.
    /// Saturates instead of overflowing; [`parse_element_header`] never
    /// returns a header whose end overflows.
    pub fn end(&self) -> Option<u64> {
        self.size
            .map(|size| self.data_offset().saturating_add(size))
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

/// Total length of a variable-size integer from its first octet: the number
/// of leading zero bits plus one (RFC 8794 4.1). `None` for a zero octet,
/// which would announce more than 8 octets.
pub(crate) fn vint_len(first: u8) -> Option<usize> {
    if first == 0 {
        None
    } else {
        Some(first.leading_zeros() as usize + 1)
    }
}

/// Reads a variable-size integer of `len` octets from `bytes`, which holds
/// at least that many. Returns `(value, all VINT_DATA bits set)`, the value
/// without the marker bit when `keep_marker` is false.
fn vint_value(bytes: &[u8], len: usize, keep_marker: bool) -> Option<(u64, bool)> {
    let octets = bytes.get(..len)?;
    let mut value = 0u64;
    for &octet in octets {
        value = (value << 8) | u64::from(octet);
    }
    let data_bits = 7 * u32::try_from(len).ok()?;
    let data_mask = (1u64 << data_bits) - 1;
    let all_ones = value & data_mask == data_mask;
    Some((
        if keep_marker {
            value
        } else {
            value & data_mask
        },
        all_ones,
    ))
}

/// Reads a variable-size integer from the start of `bytes`, as used for
/// block track numbers and EBML lace sizes (RFC 9559 10.1 and 10.3.3).
/// Returns `(value, length, all VINT_DATA bits set)`; `None` when the first
/// octet is zero or `bytes` is too short.
pub(crate) fn read_vint(bytes: &[u8]) -> Option<(u64, usize, bool)> {
    let len = vint_len(*bytes.first()?)?;
    let (value, all_ones) = vint_value(bytes, len, false)?;
    Some((value, len, all_ones))
}

/// Parses one element header from `bytes`, which start at absolute
/// `offset`. `max_size_len` is the document's `EBMLMaxSizeLength` (1 to
/// 8).
///
/// The ID may be 1 to 4 octets and must not have all its data bits set (RFC
/// 8794 5; RFC 9559 4.2 makes `0x80` legal). A data size with all data bits
/// set is unknown (RFC 8794 6.2).
pub fn parse_element_header(
    bytes: &[u8],
    offset: u64,
    max_size_len: u8,
) -> Result<Parsed<ElementHeader>, Error> {
    let invalid = |id, reason| Error::InvalidElement { offset, id, reason };
    let Some(&first) = bytes.first() else {
        return Ok(Parsed::NeedMore(1));
    };
    let id_len = match vint_len(first) {
        Some(len) if len <= MAX_ID_LEN => len,
        _ => return Err(invalid(0, "element ID longer than 4 octets")),
    };
    let Some((id, all_ones)) = vint_value(bytes, id_len, true) else {
        return Ok(Parsed::NeedMore(id_len));
    };
    let id = u32::try_from(id).map_err(|_| invalid(0, "element ID longer than 4 octets"))?;
    if all_ones {
        return Err(invalid(id, "reserved element ID"));
    }
    let Some(&size_first) = bytes.get(id_len) else {
        return Ok(Parsed::NeedMore(id_len + 1));
    };
    let size_len = match vint_len(size_first) {
        Some(len) if len <= usize::from(max_size_len) => len,
        _ => return Err(invalid(id, "data size longer than EBMLMaxSizeLength")),
    };
    let header_len = id_len + size_len;
    let Some((size, unknown)) = bytes
        .get(id_len..)
        .and_then(|rest| vint_value(rest, size_len, false))
    else {
        return Ok(Parsed::NeedMore(header_len));
    };
    let header = ElementHeader {
        id,
        offset,
        header_len: u8::try_from(header_len).map_err(|_| Error::Overflow { offset })?,
        size: (!unknown).then_some(size),
    };
    if let Some(size) = header.size {
        header
            .data_offset()
            .checked_add(size)
            .ok_or(Error::Overflow { offset })?;
    }
    Ok(Parsed::Done(header))
}

/// An unsigned integer element value (RFC 8794 7.2): 0 to 8 octets,
/// big-endian; an empty element is 0.
pub(crate) fn uint(data: &[u8], header: &ElementHeader) -> Result<u64, Error> {
    if data.len() > 8 {
        return Err(bad_length(header));
    }
    Ok(data
        .iter()
        .fold(0u64, |value, &octet| (value << 8) | u64::from(octet)))
}

/// A signed integer element value (RFC 8794 7.1): two's complement, 0 to 8
/// octets.
pub(crate) fn int(data: &[u8], header: &ElementHeader) -> Result<i64, Error> {
    let unsigned = uint(data, header)?;
    let bits = data.len() * 8;
    if bits == 0 || bits == 64 {
        return Ok(unsigned as i64);
    }
    let shift = 64 - bits;
    Ok(((unsigned << shift) as i64) >> shift)
}

/// A float element value (RFC 8794 7.3): 0, 4 or 8 octets, IEEE 754.
pub(crate) fn float(data: &[u8], header: &ElementHeader) -> Result<f64, Error> {
    match *data {
        [] => Ok(0.0),
        [a, b, c, d] => Ok(f64::from(f32::from_be_bytes([a, b, c, d]))),
        [a, b, c, d, e, f, g, h] => Ok(f64::from_be_bytes([a, b, c, d, e, f, g, h])),
        _ => Err(bad_length(header)),
    }
}

/// A String or UTF-8 element value (RFC 8794 7.4, 7.5 and 13): the octets
/// up to the first null octet. Invalid UTF-8 is replaced, never rejected:
/// strings are metadata, not structure.
pub(crate) fn string(data: &[u8]) -> String {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    String::from_utf8_lossy(data.get(..end).unwrap_or_default()).into_owned()
}

fn bad_length(header: &ElementHeader) -> Error {
    Error::InvalidElement {
        offset: header.offset,
        id: header.id,
        reason: "invalid length for the element type",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(bytes: &[u8]) -> ElementHeader {
        match parse_element_header(bytes, 0, 8).unwrap() {
            Parsed::Done(header) => header,
            Parsed::NeedMore(n) => panic!("need {n}"),
        }
    }

    #[test]
    fn rfc8794_vint_examples() {
        // Table 2: the integer 2 at lengths 1 to 4.
        for bytes in [
            &[0x82][..],
            &[0x40, 0x02],
            &[0x20, 0, 0x02],
            &[0x10, 0, 0, 0x02],
        ] {
            assert_eq!(read_vint(bytes), Some((2, bytes.len(), false)));
        }
        // Table 7: 127 at one octet is reserved (unknown); at two it is 127.
        assert_eq!(read_vint(&[0xFF]), Some((127, 1, true)));
        assert_eq!(read_vint(&[0x40, 0x7F]), Some((127, 2, false)));
        assert_eq!(read_vint(&[0]), None);
        assert_eq!(read_vint(&[0x40]), None);
    }

    #[test]
    fn element_headers() {
        // EBML header ID, 1-octet size 0x23.
        let h = header(&[0x1A, 0x45, 0xDF, 0xA3, 0xA3]);
        assert_eq!(h.id, 0x1A45_DFA3);
        assert_eq!(h.header_len, 5);
        assert_eq!(h.size, Some(0x23));
        // Unknown-sized Segment.
        let h = header(&[
            0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ]);
        assert_eq!(h.size, None);
        assert_eq!(h.header_len, 12);
        // RFC 9559 4.2: 0x80 is a legal one-octet ID.
        assert_eq!(header(&[0x80, 0x81]).id, 0x80);
        assert_eq!(
            parse_element_header(&[0x1A, 0x45], 0, 8).unwrap(),
            Parsed::NeedMore(4)
        );
        assert_eq!(
            parse_element_header(&[0xEC, 0x40], 0, 8).unwrap(),
            Parsed::NeedMore(3)
        );
        assert!(parse_element_header(&[0xFF, 0x80], 0, 8).is_err());
        assert!(parse_element_header(&[0x08, 0, 0, 0, 0, 0x80], 0, 8).is_err());
        assert!(parse_element_header(&[0xEC, 0x40, 0x01], 0, 1).is_err());
        // A size whose end overflows u64.
        let huge = [0xEC, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE];
        assert!(parse_element_header(&huge, u64::MAX - 4, 8).is_err());
    }

    #[test]
    fn values() {
        let h = header(&[0x80, 0x80]);
        assert_eq!(uint(&[], &h).unwrap(), 0);
        assert_eq!(uint(&[0x0F, 0x42, 0x40], &h).unwrap(), 1_000_000);
        assert!(uint(&[0; 9], &h).is_err());
        assert_eq!(int(&[0xFF, 0xD8], &h).unwrap(), -40);
        assert_eq!(int(&[0x7F], &h).unwrap(), 127);
        assert_eq!(int(&[0xFF; 8], &h).unwrap(), -1);
        assert_eq!(float(&2008f64.to_be_bytes(), &h).unwrap(), 2008.0);
        assert_eq!(float(&48000f32.to_be_bytes(), &h).unwrap(), 48000.0);
        assert!(float(&[0; 3], &h).is_err());
        assert_eq!(string(b"webm\0\0"), "webm");
        assert_eq!(string(b"eb\0l"), "eb");
    }
}
