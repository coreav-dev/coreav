//! Sans-IO block parsing: the Block and SimpleBlock header and lacing
//! (RFC 9559 10.1 to 10.3).

use crate::Error;
use crate::ebml::{Parsed, read_vint, vint_len};

/// Most frames one block can hold: the lace count is one octet plus one.
pub const MAX_LACED_FRAMES: usize = 256;

/// The header of a Block or SimpleBlock and the frames it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockHeader {
    /// The track number.
    pub track: u64,
    /// The timestamp relative to the Cluster, in Track Ticks.
    pub timestamp: i16,
    /// The flags octet as stored.
    pub flags: u8,
    /// Each frame as (offset from the start of the block data, size).
    pub frames: Vec<(u64, u64)>,
}

impl BlockHeader {
    /// The SimpleBlock keyframe flag.
    pub fn keyframe(&self) -> bool {
        self.flags & 0x80 != 0
    }

    /// The invisible flag.
    pub fn invisible(&self) -> bool {
        self.flags & 0x08 != 0
    }

    /// The SimpleBlock discardable flag.
    pub fn discardable(&self) -> bool {
        self.flags & 0x01 != 0
    }
}

/// Parses the header and lacing of a block whose data is `block_size`
/// bytes and starts at absolute `offset`; `bytes` holds the first bytes of
/// that data. Asks for more bytes, never more than `block_size`, until the
/// lacing is read.
pub fn parse_block(
    bytes: &[u8],
    block_size: u64,
    offset: u64,
) -> Result<Parsed<BlockHeader>, Error> {
    let invalid = |reason| Error::InvalidElement {
        offset,
        id: 0,
        reason,
    };
    let need = |count: usize| {
        if count as u64 > block_size {
            Err(invalid("block shorter than its header"))
        } else {
            Ok(Parsed::NeedMore(count))
        }
    };
    let Some(&first) = bytes.first() else {
        return need(1);
    };
    let track_len = vint_len(first).ok_or(invalid("invalid track number"))?;
    let Some((track, _, _)) = read_vint(bytes) else {
        return need(track_len);
    };
    let header_len = track_len + 3;
    let Some(&[t0, t1, flags]) = bytes.get(track_len..header_len) else {
        return need(header_len);
    };
    let timestamp = i16::from_be_bytes([t0, t1]);
    let lacing = (flags >> 1) & 3;
    let block_size_usize = usize::try_from(block_size).unwrap_or(usize::MAX);
    if lacing == 0 {
        let size = block_size
            .checked_sub(header_len as u64)
            .ok_or(invalid("block shorter than its header"))?;
        return Ok(Parsed::Done(BlockHeader {
            track,
            timestamp,
            flags,
            frames: vec![(header_len as u64, size)],
        }));
    }
    let Some(&count_minus_one) = bytes.get(header_len) else {
        return need(header_len + 1);
    };
    let count = usize::from(count_minus_one) + 1;
    let mut at = header_len + 1;
    let mut sizes = Vec::with_capacity(count);
    match lacing {
        // Xiph lacing (10.3.2): each size but the last as a run of 255s
        // ended by an octet below 255.
        1 => {
            for _ in 1..count {
                let mut size = 0u64;
                loop {
                    let Some(&octet) = bytes.get(at) else {
                        return need(at + 1);
                    };
                    at += 1;
                    size += u64::from(octet);
                    if octet != 255 {
                        break;
                    }
                    if at >= block_size_usize {
                        return Err(invalid("lace sizes run past the block"));
                    }
                }
                sizes.push(size);
            }
        }
        // EBML lacing (10.3.3): the first size as a VINT, then signed
        // differences.
        3 => {
            let mut previous = 0i128;
            for i in 1..count {
                let Some(&lead) = bytes.get(at) else {
                    return need(at + 1);
                };
                let len = vint_len(lead).ok_or(invalid("invalid lace size"))?;
                let Some((raw, _, _)) = bytes.get(at..).and_then(read_vint) else {
                    return need(at + len);
                };
                at += len;
                let value = if i == 1 {
                    i128::from(raw)
                } else {
                    let bias = (1i128 << (7 * len - 1)) - 1;
                    previous + (i128::from(raw) - bias)
                };
                let size = u64::try_from(value).map_err(|_| invalid("negative lace size"))?;
                previous = value;
                sizes.push(size);
            }
        }
        // Fixed-size lacing (10.3.4).
        _ => {
            let data = block_size
                .checked_sub(at as u64)
                .ok_or(invalid("block shorter than its header"))?;
            let each = data / count as u64;
            if each * count as u64 != data {
                return Err(invalid("fixed-size lace does not divide the block"));
            }
            sizes.resize(count - 1, each);
        }
    }
    let laced: u64 = sizes
        .iter()
        .try_fold(0u64, |sum, s| sum.checked_add(*s))
        .ok_or(invalid("lace sizes overflow"))?;
    let last = block_size
        .checked_sub(at as u64)
        .and_then(|rest| rest.checked_sub(laced))
        .ok_or(invalid("lace sizes exceed the block"))?;
    sizes.push(last);
    let mut frames = Vec::with_capacity(count);
    let mut position = at as u64;
    for size in sizes {
        frames.push((position, size));
        position += size;
    }
    Ok(Parsed::Done(BlockHeader {
        track,
        timestamp,
        flags,
        frames,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(bytes: &[u8], size: u64) -> BlockHeader {
        match parse_block(bytes, size, 0).unwrap() {
            Parsed::Done(h) => h,
            Parsed::NeedMore(n) => panic!("need {n}"),
        }
    }

    #[test]
    fn rfc9559_lacing_examples() {
        // Table 36, Xiph: 800, 500 and 1000 octets in a 2311-octet block
        // (block octets 0 to 3 are the header).
        let xiph = [
            0x81, 0x00, 0x00, 0x02, 0x02, 0xFF, 0xFF, 0xFF, 0x23, 0xFF, 0xF5,
        ];
        let h = done(&xiph, 2311);
        assert_eq!(h.frames, vec![(11, 800), (811, 500), (1311, 1000)]);
        // Table 38, EBML: 800, 500 and 1000 octets. The table puts the
        // sizes at octets 5 to 8 but the first frame at octet 8 and the
        // block at 2308 octets; with the sizes where it shows them the
        // frames start at octet 9 and the block is 2309 octets.
        let ebml = [0x81, 0x00, 0x00, 0x06, 0x02, 0x43, 0x20, 0x5E, 0xD3];
        let h = done(&ebml, 2309);
        assert_eq!(h.frames, vec![(9, 800), (809, 500), (1309, 1000)]);
        // Table 39, fixed: three 800-octet frames in 2405.
        let fixed = [0x81, 0x00, 0x00, 0x04, 0x02];
        let h = done(&fixed, 2405);
        assert_eq!(h.frames, vec![(5, 800), (805, 800), (1605, 800)]);
        // No lacing.
        let h = done(&[0x82, 0xFF, 0xD8, 0x80], 804);
        assert_eq!((h.track, h.timestamp, h.keyframe()), (2, -40, true));
        assert_eq!(h.frames, vec![(4, 800)]);
    }

    #[test]
    fn asks_for_more_and_rejects_bad_laces() {
        assert_eq!(
            parse_block(&[0x81, 0], 100, 0).unwrap(),
            Parsed::NeedMore(4)
        );
        assert_eq!(
            parse_block(&[0x81, 0, 0, 0x02, 0x02, 0xFF], 2311, 0).unwrap(),
            Parsed::NeedMore(7)
        );
        // Header longer than the block.
        assert!(parse_block(&[0x81, 0], 2, 0).is_err());
        // Lace sizes larger than the block.
        assert!(parse_block(&[0x81, 0, 0, 0x02, 0x01, 0xFE], 100, 0).is_err());
        // Fixed lacing that does not divide.
        assert!(parse_block(&[0x81, 0, 0, 0x04, 0x02], 2406, 0).is_err());
        // EBML lacing with a negative size.
        assert!(parse_block(&[0x81, 0, 0, 0x06, 0x02, 0x81, 0x80], 100, 0).is_err());
        // A zero first octet is not a track number.
        assert!(parse_block(&[0x00], 10, 0).is_err());
    }
}
