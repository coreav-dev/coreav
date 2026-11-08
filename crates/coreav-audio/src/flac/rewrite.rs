use super::parse_header;
use crate::crc::crc8;

/// A CRC-8-validated frame header, with at most 16 retained bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    /// Samples per channel in this frame.
    pub block_size: u32,
    /// Frame number for fixed blocking, sample number for variable blocking.
    pub number: u64,
    /// Whether the coded number counts samples.
    pub variable: bool,
    bytes: [u8; 16],
    len: usize,
    number_end: usize,
}

impl FrameHeader {
    /// Parses only the bounded header at the start of a packet.
    pub fn parse(data: &[u8]) -> Option<Self> {
        let (header, len) = parse_header(data).ok()?;
        let mut bytes = [0; 16];
        bytes.get_mut(..len)?.copy_from_slice(data.get(..len)?);
        let first = *data.get(4)?;
        let number_len = if first & 0x80 == 0 {
            1
        } else {
            first.leading_ones() as usize
        };
        Some(Self {
            block_size: header.block_size,
            number: header.number,
            variable: header.variable,
            bytes,
            len,
            number_end: 4 + number_len,
        })
    }

    /// Header bytes including their CRC-8.
    pub fn bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).map_or(&[], |bytes| bytes)
    }

    /// Writes a variable-blocking header with a 36-bit sample number.
    pub fn renumber(&self, number: u64, out: &mut [u8; 16]) -> Option<usize> {
        if number >= 1 << 36 {
            return None;
        }
        out.get_mut(..4)?.copy_from_slice(self.bytes.get(..4)?);
        *out.get_mut(1)? |= 1;
        let count = match number {
            0..=0x7f => 1,
            0x80..=0x7ff => 2,
            0x800..=0xffff => 3,
            0x10000..=0x1fffff => 4,
            0x200000..=0x3ffffff => 5,
            0x4000000..=0x7fffffff => 6,
            _ => 7,
        };
        if count == 1 {
            *out.get_mut(4)? = number as u8;
        } else {
            *out.get_mut(4)? = (0xffu8 << (8 - count)) | (number >> (6 * (count - 1))) as u8;
            for index in 1..count {
                *out.get_mut(4 + index)? =
                    0x80 | ((number >> (6 * (count - index - 1))) as u8 & 0x3f);
            }
        }
        let tail = self.bytes.get(self.number_end..self.len.checked_sub(1)?)?;
        let at = 4 + count;
        out.get_mut(at..at + tail.len())?.copy_from_slice(tail);
        let end = at + tail.len();
        *out.get_mut(end)? = crc8(out.get(..end)?);
        Some(end + 1)
    }

    /// Packet size after replacing only its header and footer.
    pub fn rewritten_size(&self, size: u32, number: u64) -> Option<u32> {
        let mut bytes = [0; 16];
        let len = self.renumber(number, &mut bytes)?;
        let body = size.checked_sub(u32::try_from(self.len + 2).ok()?)?;
        body.checked_add(u32::try_from(len + 2).ok()?)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn coded_number_boundaries_preserve_uncommon_fields() {
        for size in [6, 7] {
            for rate in [12, 13, 14] {
                let mut bytes = vec![0xff, 0xf8, (size << 4) | rate, 0x18, 0];
                bytes.extend_from_slice(if size == 6 { &[31] } else { &[1, 255] });
                bytes.extend_from_slice(if rate == 12 { &[48] } else { &[0xbb, 0x80] });
                bytes.push(crc8(&bytes));
                let head = FrameHeader::parse(&bytes).unwrap();
                for number in [
                    0,
                    127,
                    128,
                    2047,
                    2048,
                    65535,
                    65536,
                    (1 << 21) - 1,
                    1 << 21,
                    (1 << 26) - 1,
                    1 << 26,
                    (1 << 31) - 1,
                    1 << 31,
                    (1 << 36) - 1,
                ] {
                    let mut out = [0; 16];
                    let len = head.renumber(number, &mut out).unwrap();
                    let rewritten = FrameHeader::parse(&out[..len]).unwrap();
                    assert!(rewritten.variable);
                    assert_eq!(rewritten.number, number);
                    assert_eq!(rewritten.block_size, head.block_size);
                    assert_eq!(
                        &out[rewritten.number_end..len - 1],
                        &bytes[head.number_end..bytes.len() - 1]
                    );
                    assert_eq!(
                        head.rewritten_size(100, number),
                        Some(100 + len as u32 - bytes.len() as u32)
                    );
                    for end in 0..len {
                        assert!(FrameHeader::parse(&out[..end]).is_none());
                    }
                    out[len - 1] ^= 1;
                    assert!(FrameHeader::parse(&out[..len]).is_none());
                }
                assert!(head.renumber(1 << 36, &mut [0; 16]).is_none());
                assert!(head.rewritten_size(bytes.len() as u32 + 1, 0).is_none());
            }
        }
    }
}
