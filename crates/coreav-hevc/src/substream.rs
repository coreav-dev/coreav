use core::ops::Range;

use crate::{Error, NalHeader, Result, nal};

pub(crate) fn ranges(
    nal: &[u8],
    rbsp_header_bytes: usize,
    subset_lengths: &[u64],
    max_substreams: usize,
) -> Result<Vec<Range<usize>>> {
    let count = subset_lengths
        .len()
        .checked_add(1)
        .ok_or(Error::Limit("max_substreams"))?;
    if count > max_substreams {
        return Err(Error::Limit("max_substreams"));
    }
    NalHeader::parse(nal)?;
    let payload = nal
        .get(2..)
        .ok_or(Error::InvalidData("truncated_nal_header"))?;
    if rbsp_header_bytes >= payload.len() {
        return Err(Error::InvalidData("slice_data"));
    }
    let mut total = 0u64;
    for &length in subset_lengths {
        if length == 0 {
            return Err(Error::InvalidData("empty_substream"));
        }
        total = total
            .checked_add(length)
            .ok_or(Error::InvalidData("entry_point_offsets"))?;
    }
    let total = usize::try_from(total).map_err(|_| Error::InvalidData("entry_point_offsets"))?;
    if total >= payload.len() {
        return Err(Error::InvalidData("entry_point_offsets"));
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(count)
        .map_err(|_| Error::Limit("substream_allocation"))?;
    let mut lengths = subset_lengths.iter().copied();
    let mut remaining = lengths.next();
    let mut rbsp_pos = 0;
    let mut start = rbsp_header_bytes;
    let mut in_data = rbsp_header_bytes == 0;
    for byte in nal::Unescape::new(payload) {
        if byte?.is_some() {
            rbsp_pos += 1;
        }
        if in_data {
            if let Some(left) = remaining.as_mut() {
                *left -= 1;
                if *left == 0 {
                    if start == rbsp_pos {
                        return Err(Error::InvalidData("empty_substream"));
                    }
                    result.push(start..rbsp_pos);
                    start = rbsp_pos;
                    remaining = lengths.next();
                }
            }
        } else if rbsp_pos == rbsp_header_bytes {
            in_data = true;
        }
    }
    if !in_data || remaining.is_some() {
        return Err(Error::InvalidData("entry_point_offsets"));
    }
    if start == rbsp_pos {
        return Err(Error::InvalidData("empty_substream"));
    }
    result.push(start..rbsp_pos);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::ranges;
    use crate::{Error, Limits, nal};

    #[test]
    fn no_entry_points_returns_the_remaining_rbsp() {
        assert_eq!(
            ranges(&[2, 1, 0x80, 5, 6], 1, &[], 1),
            Ok(core::iter::once(1..3).collect())
        );
        assert_eq!(
            ranges(&[2, 1, 5, 6], 0, &[], 1),
            Ok(core::iter::once(0..2).collect())
        );
    }

    #[test]
    fn multiple_lengths_are_relative_to_slice_data_not_the_nal() {
        assert_eq!(
            ranges(&[2, 1, 0x80, 5, 6, 7, 8, 9, 10], 1, &[2, 1, 2], 4),
            Ok(vec![1..3, 3..4, 4..6, 6..7])
        );
    }

    #[test]
    fn header_and_data_escape_bytes_do_not_shift_rbsp_ranges() {
        let bytes = [2, 1, 0, 0, 3, 1, 0x80, 0, 0, 3, 2, 5, 6];
        let mapped = ranges(&bytes, 4, &[4, 1], 3).unwrap();
        assert_eq!(mapped, [4..7, 7..8, 8..9]);
        let rbsp = nal::rbsp(&bytes, &Limits::default()).unwrap();
        assert_eq!(&rbsp[mapped[0].clone()], [0, 0, 2]);
        assert_eq!(&rbsp[mapped[1].clone()], [5]);
        assert_eq!(&rbsp[mapped[2].clone()], [6]);
    }

    #[test]
    fn subset_boundaries_before_and_after_escape_bytes() {
        let bytes = [2, 1, 0x80, 0, 0, 3, 1, 5];
        assert_eq!(ranges(&bytes, 1, &[2], 2), Ok(vec![1..3, 3..5]));
        assert_eq!(ranges(&bytes, 1, &[3], 2), Ok(vec![1..3, 3..5]));
        assert_eq!(ranges(&bytes, 1, &[4], 2), Ok(vec![1..4, 4..5]));
    }

    #[test]
    fn escape_state_crosses_subset_boundaries() {
        assert_eq!(
            ranges(&[2, 1, 0x80, 0, 0, 3, 0, 0, 3, 1, 5], 1, &[1, 4], 3),
            Ok(vec![1..2, 2..5, 5..7])
        );
    }

    #[test]
    fn escape_at_header_boundary_counts_toward_slice_data() {
        assert_eq!(
            ranges(&[2, 1, 0, 0, 3, 1, 5], 2, &[2], 2),
            Ok(vec![2..3, 3..4])
        );
        assert_eq!(
            ranges(&[2, 1, 0, 0, 3, 1, 5, 6], 3, &[1], 2),
            Ok(vec![3..4, 4..5])
        );
    }

    #[test]
    fn zero_length_and_escape_only_subsets_are_empty() {
        assert_eq!(
            ranges(&[2, 1, 0x80, 5, 6], 1, &[0], 2),
            Err(Error::InvalidData("empty_substream"))
        );
        assert_eq!(
            ranges(&[2, 1, 0x80, 0, 0, 3, 1, 5], 1, &[2, 1], 3),
            Err(Error::InvalidData("empty_substream"))
        );
    }

    #[test]
    fn final_subset_must_be_nonempty() {
        assert_eq!(
            ranges(&[2, 1, 0x80, 5, 6], 1, &[2], 2),
            Err(Error::InvalidData("empty_substream"))
        );
    }

    #[test]
    fn missing_payload_and_header_beyond_unescaped_end_are_rejected() {
        for (bytes, header) in [
            (&[2, 1][..], 0),
            (&[2, 1, 0x80][..], 1),
            (&[2, 1, 0x80][..], usize::MAX),
            (&[2, 1, 0, 0, 3, 1][..], 3),
            (&[2, 1, 0, 0, 3, 1][..], 4),
        ] {
            assert!(matches!(
                ranges(bytes, header, &[], 1),
                Err(Error::InvalidData(_))
            ));
        }
    }

    #[test]
    fn oversized_and_overflowing_lengths_are_rejected() {
        for lengths in [&[4][..], &[u64::MAX][..], &[u64::MAX, 1][..]] {
            assert_eq!(
                ranges(&[2, 1, 0x80, 5, 6], 1, lengths, 3),
                Err(Error::InvalidData("entry_point_offsets"))
            );
        }
        assert_eq!(
            ranges(&[2, 1, 0, 0, 3, 1, 5], 3, &[2], 2),
            Err(Error::InvalidData("entry_point_offsets"))
        );
    }

    #[test]
    fn malformed_escaping_agrees_with_the_rbsp_owner() {
        for bytes in [
            &[2, 1, 0, 0, 3, 4][..],
            &[2, 1, 0, 0, 0][..],
            &[2, 1, 0, 0, 1][..],
            &[2, 1, 0, 0, 2][..],
            &[2, 1, 0x80, 5, 0, 0, 3, 4][..],
        ] {
            let expected = nal::rbsp(bytes, &Limits::default()).unwrap_err();
            assert_eq!(ranges(bytes, 1, &[], 1), Err(expected));
        }
    }

    #[test]
    fn terminal_escape_byte_agrees_with_the_rbsp_owner() {
        let bytes = [2, 1, 0x80, 5, 0, 0, 3];
        let rbsp = nal::rbsp(&bytes, &Limits::default()).unwrap();
        assert_eq!(rbsp, [0x80, 5, 0, 0]);
        assert_eq!(
            ranges(&bytes, 1, &[], 1),
            Ok(core::iter::once(1..4).collect())
        );
        let mapped = ranges(&bytes, 1, &[1], 2).unwrap();
        assert_eq!(mapped, [1..2, 2..4]);
        assert_eq!(&rbsp[mapped[0].clone()], [5]);
        assert_eq!(&rbsp[mapped[1].clone()], [0, 0]);
        assert_eq!(
            ranges(&bytes, 1, &[3], 2),
            Err(Error::InvalidData("empty_substream"))
        );
    }

    #[test]
    fn an_unescaped_three_is_data_without_two_preceding_zeros() {
        assert_eq!(
            ranges(&[2, 1, 0x80, 3, 0, 3, 5], 1, &[2], 2),
            Ok(vec![1..3, 3..5])
        );
    }

    #[test]
    fn truncated_and_invalid_nal_headers_are_rejected() {
        for bytes in [&[][..], &[2][..], &[0x80, 1, 5][..], &[2, 0, 5][..]] {
            assert!(matches!(
                ranges(bytes, 0, &[], 1),
                Err(Error::InvalidData(_))
            ));
        }
    }

    #[test]
    fn substream_count_is_bounded_before_input_validation_or_allocation() {
        assert_eq!(ranges(&[], 0, &[], 0), Err(Error::Limit("max_substreams")));
        assert_eq!(
            ranges(&[], 0, &[0, 0], 2),
            Err(Error::Limit("max_substreams"))
        );
        assert_eq!(
            ranges(&[2, 1, 5, 6, 7], 0, &[1, 1], 3),
            Ok(vec![0..1, 1..2, 2..3])
        );
    }

    #[test]
    fn range_storage_is_bounded_by_subset_count_not_payload_size() {
        let short = ranges(&[2, 1, 5, 6, 7], 0, &[1, 1], 3).unwrap();
        let long = ranges(&[2, 1, 5, 6, 7, 8, 9, 10, 11, 12], 0, &[1, 1], 3).unwrap();
        assert_eq!(short.len(), 3);
        assert_eq!(long.len(), 3);
        assert_eq!(short.capacity(), 3);
        assert_eq!(long.capacity(), 3);
    }
}
