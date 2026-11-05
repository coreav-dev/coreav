use md5::{Digest, Md5};

use crate::{Error, PictureHash, Result};

pub(crate) fn verify(
    expected: &PictureHash,
    planes: [&[u16]; 3],
    width: usize,
    height: usize,
    bit_depth: u32,
) -> Result<()> {
    if !matches!(bit_depth, 8 | 10) {
        return Err(Error::InvalidData("picture_hash_bit_depth"));
    }
    if width == 0
        || height == 0
        || width & 1 != 0
        || height & 1 != 0
        || u32::try_from(width).is_err()
        || u32::try_from(height).is_err()
    {
        return Err(Error::InvalidData("picture_hash_dimensions"));
    }
    let luma_len = width
        .checked_mul(height)
        .ok_or(Error::Limit("picture_hash_samples"))?;
    let chroma_len = (width / 2)
        .checked_mul(height / 2)
        .ok_or(Error::Limit("picture_hash_samples"))?;
    let max_sample = (1u16 << bit_depth) - 1;
    for (plane, length) in planes.iter().zip([luma_len, chroma_len, chroma_len]) {
        if plane.len() != length {
            return Err(Error::InvalidData("picture_hash_plane_length"));
        }
        if plane.iter().any(|sample| *sample > max_sample) {
            return Err(Error::InvalidData("picture_hash_sample"));
        }
    }
    let bytes_per_sample = if bit_depth == 8 { 1 } else { 2 };
    match expected {
        PictureHash::Md5(digests) => {
            for (plane, digest) in planes.into_iter().zip(digests) {
                if md5(plane, bytes_per_sample)? != *digest {
                    return Err(Error::InvalidData("picture_hash_mismatch"));
                }
            }
        }
        PictureHash::Crc(digests) => {
            for (plane, digest) in planes.into_iter().zip(digests) {
                if crc(plane, bytes_per_sample) != *digest {
                    return Err(Error::InvalidData("picture_hash_mismatch"));
                }
            }
        }
        PictureHash::Checksum(digests) => {
            for ((plane, comp_width), digest) in planes
                .into_iter()
                .zip([width, width / 2, width / 2])
                .zip(digests)
            {
                if checksum(plane, comp_width, bytes_per_sample) != *digest {
                    return Err(Error::InvalidData("picture_hash_mismatch"));
                }
            }
        }
    }
    Ok(())
}

fn md5(plane: &[u16], bytes_per_sample: usize) -> Result<[u8; 16]> {
    let mut state = Md5::new();
    let mut packed = [0u8; 1024];
    for samples in plane.chunks(packed.len() / bytes_per_sample) {
        for (dest, sample) in packed.chunks_exact_mut(bytes_per_sample).zip(samples) {
            dest.copy_from_slice(
                sample
                    .to_le_bytes()
                    .get(..bytes_per_sample)
                    .ok_or(Error::InvalidData("picture_hash_packing"))?,
            );
        }
        state.update(
            packed
                .get(..samples.len() * bytes_per_sample)
                .ok_or(Error::InvalidData("picture_hash_packing"))?,
        );
    }
    Ok(state.finalize().into())
}

fn crc(plane: &[u16], bytes_per_sample: usize) -> [u8; 2] {
    let mut value = 0xffffu16;
    for byte in plane
        .iter()
        .flat_map(|sample| sample.to_le_bytes().into_iter().take(bytes_per_sample))
        .chain([0, 0])
    {
        for shift in (0..8).rev() {
            let msb = value & 0x8000 != 0;
            value = (value << 1) | u16::from((byte >> shift) & 1);
            if msb {
                value ^= 0x1021;
            }
        }
    }
    value.to_be_bytes()
}

fn checksum(plane: &[u16], width: usize, bytes_per_sample: usize) -> [u8; 4] {
    let mut sum = 0u32;
    for (y, row) in plane.chunks_exact(width).enumerate() {
        for (x, sample) in row.iter().enumerate() {
            let mask = ((x & 255) ^ (y & 255) ^ (x >> 8) ^ (y >> 8)) as u32;
            for byte in sample.to_le_bytes().into_iter().take(bytes_per_sample) {
                sum = sum.wrapping_add(u32::from(byte) ^ mask);
            }
        }
    }
    sum.to_be_bytes()
}

#[cfg(test)]
mod tests {
    use super::{crc, verify};
    use crate::{Error, PictureHash, Result};

    fn digest(hex: &str) -> Result<[u8; 16]> {
        let mut digest = [0; 16];
        if hex.len() != digest.len() * 2 {
            return Err(Error::InvalidData("test_digest_length"));
        }
        for (dest, pair) in digest.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
            let pair =
                core::str::from_utf8(pair).map_err(|_| Error::InvalidData("test_digest_hex"))?;
            *dest =
                u8::from_str_radix(pair, 16).map_err(|_| Error::InvalidData("test_digest_hex"))?;
        }
        Ok(digest)
    }

    fn at_mut<T>(values: &mut [T], index: usize) -> Result<&mut T> {
        values
            .get_mut(index)
            .ok_or(Error::InvalidData("test_index"))
    }

    fn md5_8() -> Result<PictureHash> {
        Ok(PictureHash::Md5([
            digest("e2fc714c4727ee9395f324cd2e7f331f")?,
            digest("0cc175b9c0f1b6a831c399e269772661")?,
            digest("92eb5ffee6ae2fec3ad71c777531578f")?,
        ]))
    }

    fn md5_10() -> Result<PictureHash> {
        Ok(PictureHash::Md5([
            digest("f7f36e75e243fd71c130fd1573f8349f")?,
            digest("706acf3204a3d094942c7bc49eba2aa8")?,
            digest("c76b9ba810dc1cfe94bbedf4f93a778f")?,
        ]))
    }

    #[test]
    fn md5_matches_known_ascii_byte_digests_without_u16_padding() -> Result<()> {
        assert_eq!(
            verify(&md5_8()?, [&[97, 98, 99, 100], &[97], &[98]], 2, 2, 8),
            Ok(())
        );
        Ok(())
    }

    #[test]
    fn md5_hashes_every_ten_bit_high_byte_in_little_endian_order() -> Result<()> {
        let planes: [&[u16]; 3] = [&[0x312, 0x256, 0x178, 0xab], &[0x301], &[0x2ff]];
        assert_eq!(verify(&md5_10()?, planes, 2, 2, 10), Ok(()));
        let changed: [&[u16]; 3] = [&[0x212, 0x256, 0x178, 0xab], &[0x301], &[0x2ff]];
        assert_eq!(
            verify(&md5_10()?, changed, 2, 2, 10),
            Err(Error::InvalidData("picture_hash_mismatch"))
        );
        let big_endian = PictureHash::Md5([
            digest("c95fdb1d42b65ec91d44fa34dc88d4fd")?,
            digest("20f9aa7c189af6e641a43669bebf1c63")?,
            digest("4b5e88f48e15d06ac21d251668349254")?,
        ]);
        assert_eq!(
            verify(&big_endian, planes, 2, 2, 10),
            Err(Error::InvalidData("picture_hash_mismatch"))
        );
        Ok(())
    }

    #[test]
    fn fixed_md5_scratch_handles_full_chunks_and_a_partial_final_chunk() -> Result<()> {
        for (depth, sample, y_hex, c_hex) in [
            (
                8,
                0xab,
                "15b3d6e6dd0b16d376379753e7453148",
                "ce1d711b30a86fc05e9741d4d5053f46",
            ),
            (
                10,
                0x301,
                "f7fcbf1d9718b4f5e55b9533e07d0720",
                "6e4489f9ec888d5d5121a23d8a8a6546",
            ),
        ] {
            let y = [sample; 1088];
            let c = [sample; 272];
            let expected = PictureHash::Md5([digest(y_hex)?, digest(c_hex)?, digest(c_hex)?]);
            assert_eq!(verify(&expected, [&y, &c, &c], 34, 32, depth), Ok(()));
        }
        Ok(())
    }

    fn polynomial_crc(bytes: &[u8]) -> [u8; 2] {
        let mut dividend = 0xffffu128;
        for &byte in bytes {
            dividend = (dividend << 8) | u128::from(byte);
        }
        dividend <<= 16;
        while dividend >= 1 << 16 {
            dividend ^= 0x11021u128 << (111 - dividend.leading_zeros());
        }
        (dividend as u16).to_be_bytes()
    }

    #[test]
    fn crc_matches_polynomial_division_with_initial_ffff_and_zero_augmentation() {
        let cases: [&[u8]; 5] = [
            &[0],
            &[255],
            &[0, 0, 0, 0],
            &[97, 98, 99, 100],
            &[1, 3, 255, 2],
        ];
        for bytes in cases {
            let samples: Vec<u16> = bytes.iter().copied().map(u16::from).collect();
            assert_eq!(crc(&samples, 1), polynomial_crc(bytes));
        }
        assert_eq!(crc(&[0], 1), [0xcc, 0x9c]);
        assert_eq!(crc(&[0, 0, 0, 0], 1), [0x0e, 0x10]);
        assert_eq!(
            crc(&[0x312, 0x256, 0x178, 0xab], 2),
            polynomial_crc(&[0x12, 3, 0x56, 2, 0x78, 1, 0xab, 0])
        );
    }

    #[test]
    fn crc_checks_all_three_components_and_retains_ten_bit_high_bytes() {
        let expected8 = PictureHash::Crc([[0xa6, 0x26], [0xb0, 0x1b], [0x80, 0x78]]);
        assert_eq!(
            verify(&expected8, [&[97, 98, 99, 100], &[97], &[98]], 2, 2, 8),
            Ok(())
        );
        let expected10 = PictureHash::Crc([[0x03, 0x5d], [0x87, 0x92], [0xa7, 0x7d]]);
        assert_eq!(
            verify(
                &expected10,
                [&[0x312, 0x256, 0x178, 0xab], &[0x301], &[0x2ff]],
                2,
                2,
                10
            ),
            Ok(())
        );
        assert!(
            verify(
                &expected10,
                [&[0x212, 0x256, 0x178, 0xab], &[0x301], &[0x2ff]],
                2,
                2,
                10
            )
            .is_err()
        );
    }

    #[test]
    fn checksum_applies_the_coordinate_mask_to_each_sample_byte() {
        let expected8 =
            PictureHash::Checksum([6u32.to_be_bytes(), 7u32.to_be_bytes(), 9u32.to_be_bytes()]);
        assert_eq!(
            verify(&expected8, [&[0, 1, 2, 3], &[7], &[9]], 2, 2, 8),
            Ok(())
        );
        let expected10 = PictureHash::Checksum([
            12u32.to_be_bytes(),
            1u32.to_be_bytes(),
            258u32.to_be_bytes(),
        ]);
        assert_eq!(
            verify(
                &expected10,
                [&[0x100, 0x201, 0x302, 3], &[0x100], &[0x3ff]],
                2,
                2,
                10
            ),
            Ok(())
        );
    }

    #[test]
    fn checksum_uses_high_coordinate_bits_and_component_local_coordinates() -> Result<()> {
        let mut y = [0u16; 516];
        *at_mut(&mut y, 256)? = 1;
        let chroma = [0u16; 129];
        let expected = PictureHash::Checksum([
            65281u32.to_be_bytes(),
            8256u32.to_be_bytes(),
            8256u32.to_be_bytes(),
        ]);
        assert_eq!(verify(&expected, [&y, &chroma, &chroma], 258, 2, 8), Ok(()));
        *at_mut(&mut y, 256)? = 0;
        *at_mut(&mut y, 512)? = 1;
        assert_eq!(verify(&expected, [&y, &chroma, &chroma], 2, 258, 8), Ok(()));
        Ok(())
    }

    #[test]
    fn each_hash_rejects_changed_pixels_expected_bytes_and_swapped_chroma() -> Result<()> {
        for expected in [
            md5_8()?,
            PictureHash::Crc([[0xa6, 0x26], [0xb0, 0x1b], [0x80, 0x78]]),
            PictureHash::Checksum([
                394u32.to_be_bytes(),
                97u32.to_be_bytes(),
                98u32.to_be_bytes(),
            ]),
        ] {
            let planes: [&[u16]; 3] = [&[97, 98, 99, 100], &[97], &[98]];
            assert_eq!(verify(&expected, planes, 2, 2, 8), Ok(()));
            assert!(verify(&expected, [&[98, 98, 99, 100], &[97], &[98]], 2, 2, 8).is_err());
            let [y, cb, cr] = planes;
            assert!(verify(&expected, [y, cr, cb], 2, 2, 8).is_err());
            for component in 0..3 {
                let mut changed = expected;
                match &mut changed {
                    PictureHash::Md5(values) => *at_mut(at_mut(values, component)?, 15)? ^= 1,
                    PictureHash::Crc(values) => *at_mut(at_mut(values, component)?, 1)? ^= 1,
                    PictureHash::Checksum(values) => *at_mut(at_mut(values, component)?, 3)? ^= 1,
                }
                assert_eq!(
                    verify(&changed, planes, 2, 2, 8),
                    Err(Error::InvalidData("picture_hash_mismatch"))
                );
            }
        }
        Ok(())
    }

    #[test]
    fn crc_and_checksum_expect_bitstream_big_endian_digest_values() {
        for expected in [
            PictureHash::Crc([[0x26, 0xa6], [0x1b, 0xb0], [0x78, 0x80]]),
            PictureHash::Checksum([
                394u32.to_le_bytes(),
                97u32.to_le_bytes(),
                98u32.to_le_bytes(),
            ]),
        ] {
            assert_eq!(
                verify(&expected, [&[97, 98, 99, 100], &[97], &[98]], 2, 2, 8),
                Err(Error::InvalidData("picture_hash_mismatch"))
            );
        }
    }

    #[test]
    fn dimensions_depth_and_exact_plane_lengths_are_validated() -> Result<()> {
        let planes: [&[u16]; 3] = [&[97, 98, 99, 100], &[97], &[98]];
        for (width, height) in [(0, 2), (2, 0), (1, 2), (2, 1), (usize::MAX, 2)] {
            assert_eq!(
                verify(&md5_8()?, planes, width, height, 8),
                Err(Error::InvalidData("picture_hash_dimensions"))
            );
        }
        for depth in [0, 7, 9, 11, 16, u32::MAX] {
            assert_eq!(
                verify(&md5_8()?, planes, 2, 2, depth),
                Err(Error::InvalidData("picture_hash_bit_depth"))
            );
        }
        let bad_planes: [[&[u16]; 3]; 4] = [
            [&[97, 98, 99], &[97], &[98]],
            [&[97, 98, 99, 100, 101], &[97], &[98]],
            [&[97, 98, 99, 100], &[], &[98]],
            [&[97, 98, 99, 100], &[97], &[98, 99]],
        ];
        for bad in bad_planes {
            assert_eq!(
                verify(&md5_8()?, bad, 2, 2, 8),
                Err(Error::InvalidData("picture_hash_plane_length"))
            );
        }
        assert!(
            verify(
                &md5_8()?,
                planes,
                u32::MAX as usize - 1,
                u32::MAX as usize - 1,
                8
            )
            .is_err()
        );
        assert!(verify(&md5_8()?, planes, usize::MAX - 1, usize::MAX - 1, 8).is_err());
        Ok(())
    }

    #[test]
    fn out_of_range_samples_in_any_component_fail_before_hash_comparison() -> Result<()> {
        for depth in [8, 10] {
            for component in 0..3 {
                let y = [0; 4];
                let c = [0];
                let bad_y = [0, 0, 0, 1u16 << depth];
                let bad_c = [1u16 << depth];
                let mut planes: [&[u16]; 3] = [&y, &c, &c];
                *at_mut(&mut planes, component)? = if component == 0 { &bad_y } else { &bad_c };
                assert_eq!(
                    verify(&md5_8()?, planes, 2, 2, depth),
                    Err(Error::InvalidData("picture_hash_sample"))
                );
            }
        }
        Ok(())
    }
}
