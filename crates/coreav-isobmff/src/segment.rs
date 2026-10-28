//! Segment index (`sidx`, ISO/IEC 14496-12 8.16.3), read by probe when it
//! comes before the first movie fragment.

use crate::bytes::Bytes;
use crate::{Error, FourCc};

/// One reference of a segment index: a subsegment of media, or another
/// segment index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentReference {
    /// Absolute byte offset of the referenced data.
    pub offset: u64,
    /// Size of the referenced data in bytes.
    pub size: u32,
    /// Duration of the subsegment in the index's timescale.
    pub duration: u32,
    /// The reference points to another segment index, not to media.
    pub is_index: bool,
    /// The subsegment starts with a stream access point.
    pub starts_with_sap: bool,
    /// `SAP_type`: 1 and 2 are closed GOPs, which present nothing before
    /// the subsegment's first decoded sample; 3 is an open GOP, whose
    /// leading samples need the subsegment before it; 0 says that the
    /// type is unknown (ISO/IEC 14496-12 8.16.3.3 and Annex I).
    pub sap_type: u8,
    /// `SAP_delta_time`: the access point's decode time less the
    /// subsegment's first decode time, in the index's timescale. 0 means
    /// the subsegment starts at the access point.
    pub sap_delta_time: u32,
}

/// A segment index box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentIndex {
    /// `reference_ID`: the track the index describes.
    pub reference_id: u32,
    /// Ticks per second of the durations.
    pub timescale: u32,
    /// Presentation time of the first referenced sample.
    pub earliest_presentation_time: u64,
    /// References in file order.
    pub references: Vec<SegmentReference>,
}

/// Parses a `sidx` payload. `payload_offset` is the absolute offset of the
/// payload and `box_end` the offset just past the box (the anchor of
/// `first_offset`); every referenced range must end within `file_len`.
pub(crate) fn parse_sidx(
    payload: &[u8],
    payload_offset: u64,
    box_end: u64,
    file_len: u64,
) -> Result<SegmentIndex, Error> {
    let invalid = |reason| Error::InvalidBox {
        offset: payload_offset,
        kind: FourCc(*b"sidx"),
        reason,
    };
    let mut b = Bytes::new(payload, payload_offset);
    let (version, _) = b.version_flags()?;
    let reference_id = b.u32()?;
    let timescale = b.u32()?;
    if timescale == 0 {
        return Err(invalid("zero timescale"));
    }
    let (earliest_presentation_time, first_offset) = if version == 0 {
        (u64::from(b.u32()?), u64::from(b.u32()?))
    } else {
        (b.u64()?, b.u64()?)
    };
    b.skip(2)?;
    let count = b.u16()?;
    if usize::from(count).saturating_mul(12) > b.remaining() {
        return Err(invalid("reference count exceeds the box"));
    }
    let mut offset = box_end.checked_add(first_offset).ok_or(Error::Overflow {
        offset: payload_offset,
    })?;
    let mut references = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let word = b.u32()?;
        let duration = b.u32()?;
        let sap = b.u32()?;
        let size = word & 0x7fff_ffff;
        let end = offset.checked_add(u64::from(size)).ok_or(Error::Overflow {
            offset: payload_offset,
        })?;
        if end > file_len {
            return Err(Error::Truncated { offset });
        }
        references.push(SegmentReference {
            offset,
            size,
            duration,
            is_index: word >> 31 == 1,
            starts_with_sap: sap >> 31 == 1,
            sap_type: u8::try_from((sap >> 28) & 0x7).unwrap_or(0),
            sap_delta_time: sap & 0x0fff_ffff,
        });
        offset = end;
    }
    Ok(SegmentIndex {
        reference_id,
        timescale,
        earliest_presentation_time,
        references,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sidx(count: u16, refs: &[(u32, u32)]) -> Vec<u8> {
        let mut p = vec![0, 0, 0, 0];
        p.extend_from_slice(&1u32.to_be_bytes());
        p.extend_from_slice(&1000u32.to_be_bytes());
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&8u32.to_be_bytes());
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(&count.to_be_bytes());
        for (size, duration) in refs {
            p.extend_from_slice(&size.to_be_bytes());
            p.extend_from_slice(&duration.to_be_bytes());
            p.extend_from_slice(&0x9000_0000u32.to_be_bytes());
        }
        p
    }

    #[test]
    fn references_are_absolute_and_bounded() {
        let payload = sidx(2, &[(100, 40), (50, 40)]);
        let index = parse_sidx(&payload, 100, 200, 1000).unwrap();
        assert_eq!(index.references.len(), 2);
        assert_eq!(index.references[0].offset, 208);
        assert_eq!(index.references[1].offset, 308);
        assert!(index.references[0].starts_with_sap);
        assert!(parse_sidx(&payload, 100, 200, 300).is_err());
        assert!(parse_sidx(&sidx(9, &[(1, 1)]), 0, 0, 1000).is_err());
        for end in 0..payload.len() {
            let _ = parse_sidx(&payload[..end], 0, 0, 1000);
        }
    }

    fn sidx_v1(first_offset: u64, count: u16, refs: &[(u32, u32)]) -> Vec<u8> {
        let mut p = vec![1, 0, 0, 0];
        p.extend_from_slice(&7u32.to_be_bytes());
        p.extend_from_slice(&90_000u32.to_be_bytes());
        p.extend_from_slice(&(u64::from(u32::MAX) + 5).to_be_bytes());
        p.extend_from_slice(&first_offset.to_be_bytes());
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(&count.to_be_bytes());
        for (word, duration) in refs {
            p.extend_from_slice(&word.to_be_bytes());
            p.extend_from_slice(&duration.to_be_bytes());
            p.extend_from_slice(&0x1000_0000u32.to_be_bytes());
        }
        p
    }

    #[test]
    fn version_1_has_64_bit_times_and_offsets() {
        let payload = sidx_v1(10, 2, &[(0x8000_0020, 3), (40, 4)]);
        let index = parse_sidx(&payload, 0, 100, 1000).unwrap();
        assert_eq!(index.reference_id, 7);
        assert_eq!(index.timescale, 90_000);
        assert_eq!(index.earliest_presentation_time, u64::from(u32::MAX) + 5);
        let [first, second] = index.references[..] else {
            panic!("{index:?}");
        };
        // The top bit of the first word is the reference type, not the size.
        assert!(first.is_index && !second.is_index);
        assert_eq!((first.offset, first.size), (110, 0x20));
        assert_eq!((second.offset, second.size), (142, 40));
        assert!(!first.starts_with_sap);
        for end in 0..payload.len() {
            assert!(parse_sidx(&payload[..end], 0, 100, 1000).is_err());
        }
    }

    #[test]
    fn hostile_offsets_and_counts_are_errors() {
        // first_offset past u64: overflow, not wrap.
        let payload = sidx_v1(u64::MAX, 1, &[(1, 1)]);
        assert!(matches!(
            parse_sidx(&payload, 0, 100, u64::MAX),
            Err(Error::Overflow { .. })
        ));
        // A reference ending past the object.
        let payload = sidx_v1(0, 1, &[(0x7fff_ffff, 1)]);
        assert!(matches!(
            parse_sidx(&payload, 0, 100, 1 << 30),
            Err(Error::Truncated { .. })
        ));
        // The largest count with no reference bytes: rejected before the
        // reference vector is allocated.
        let payload = sidx_v1(0, u16::MAX, &[]);
        assert!(matches!(
            parse_sidx(&payload, 0, 100, 1000),
            Err(Error::InvalidBox { .. })
        ));
        // Zero timescale.
        let mut payload = sidx_v1(0, 0, &[]);
        payload[8..12].copy_from_slice(&[0; 4]);
        assert!(parse_sidx(&payload, 0, 100, 1000).is_err());
    }
}
