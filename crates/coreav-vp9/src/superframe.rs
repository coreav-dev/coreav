//! Superframes: several frames in one packet, with an index at its end
//! (Annex B).

use crate::error::{Error, Result};

/// Splits a packet into the frames it carries (B.4): the frames of its
/// superframe index, or the whole packet when it has none.
///
/// Each size in the index is `SzBytes` bytes, least significant byte first.
/// Annex B writes the element as `f(SzBytes)` without naming the byte
/// order. The published test vectors settle it: read most significant byte
/// first, the superframes of 50 of them, whose sizes take more than one
/// byte, do not split.
pub fn split(packet: &[u8]) -> Result<Vec<&[u8]>> {
    let Some(&last) = packet.last() else {
        return Err(Error::Truncated("a packet"));
    };
    if last & 0xE0 != 0xC0 {
        return Ok(vec![packet]);
    }
    let size_bytes = usize::from((last >> 3) & 3) + 1;
    let count = usize::from(last & 7) + 1;
    let index_size = 2 + count * size_bytes;
    let Some(start) = packet.len().checked_sub(index_size) else {
        return Ok(vec![packet]);
    };
    let Some((&first, index)) = packet.get(start..).and_then(<[u8]>::split_first) else {
        return Ok(vec![packet]);
    };
    if first != last {
        return Ok(vec![packet]);
    }
    let mut frames = Vec::with_capacity(count);
    let mut rest = packet.get(..start).unwrap_or_default();
    for size in index.chunks_exact(size_bytes).take(count) {
        let size = size
            .iter()
            .rev()
            .fold(0usize, |n, &b| (n << 8) | usize::from(b));
        if size == 0 {
            return Err(Error::Invalid("a superframe frame of 0 bytes"));
        }
        let (frame, after) = rest
            .split_at_checked(size)
            .ok_or(Error::Truncated("the frames of a superframe"))?;
        frames.push(frame);
        rest = after;
    }
    if !rest.is_empty() {
        return Err(Error::Invalid(
            "bytes between a superframe's frames and its index",
        ));
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packet_without_an_index_is_one_frame() {
        let packet = [0x82, 0x49, 0x83, 0x42, 0x00];
        assert_eq!(split(&packet), Ok(vec![&packet[..]]));
    }

    #[test]
    fn an_index_splits_the_frames_it_names() {
        let mut packet = vec![1, 2, 3, 4, 5];
        packet.extend_from_slice(&[0xC9, 3, 0, 2, 0, 0xC9]);
        assert_eq!(split(&packet), Ok(vec![&[1, 2, 3][..], &[4, 5][..]]));
    }

    #[test]
    fn sizes_are_least_significant_byte_first() {
        let mut packet = vec![7; 0x102];
        packet.extend_from_slice(&[0xC8, 0x02, 0x01, 0xC8]);
        let sizes = split(&packet).map(|f| f.iter().map(|f| f.len()).collect::<Vec<_>>());
        assert_eq!(sizes, Ok(vec![0x102]));
    }

    #[test]
    fn a_marker_without_its_twin_is_part_of_the_frame() {
        let packet = [0x82, 0x00, 0x00, 0xC0];
        assert_eq!(split(&packet), Ok(vec![&packet[..]]));
    }

    #[test]
    fn sizes_that_do_not_fill_the_packet_are_refused() {
        let mut packet = vec![1, 2, 3, 4, 5];
        packet.extend_from_slice(&[0xC9, 3, 0, 1, 0, 0xC9]);
        assert_eq!(
            split(&packet),
            Err(Error::Invalid(
                "bytes between a superframe's frames and its index"
            ))
        );
        let mut packet = vec![1, 2, 3];
        packet.extend_from_slice(&[0xC9, 3, 0, 1, 0, 0xC9]);
        assert_eq!(
            split(&packet),
            Err(Error::Truncated("the frames of a superframe"))
        );
    }
}
