use crate::*;
use std::{vec, vec::Vec};

fn predict(
    bits: u8,
    n: usize,
    mode: u8,
    component: Component,
    strong_smoothing: bool,
    refs: &[Option<u16>],
) -> Result<Vec<u16>, Error> {
    let mut samples = vec![u16::MAX; n * n];
    predict_intra(
        IntraPrediction {
            depth: BitDepth::new(bits)?,
            component,
            mode: IntraMode::new(mode)?,
            strong_smoothing,
        },
        refs,
        BlockMut::new(&mut samples, n, BlockSize::new(n)?)?,
    )?;
    Ok(samples)
}

fn inverse(
    bits: u8,
    n: usize,
    transform: InverseTransform,
    qp: u8,
    levels: &[i16],
    factors: &[u8],
) -> Result<Vec<i32>, Error> {
    let mut samples = vec![i32::MAX; n * n];
    inverse_residual(
        Reconstruction {
            depth: BitDepth::new(bits)?,
            transform,
            qp,
        },
        Block::new(levels, n, BlockSize::new(n)?)?,
        factors,
        BlockMut::new(&mut samples, n, BlockSize::new(n)?)?,
    )?;
    Ok(samples)
}

#[test]
fn checked_domains_and_address_boundaries() {
    for bits in 0..=u8::MAX {
        assert_eq!(BitDepth::new(bits).is_ok(), bits == 8 || bits == 10);
    }
    for mode in 0..=u8::MAX {
        assert_eq!(IntraMode::new(mode).is_ok(), mode <= 34);
    }
    for n in 0..=64 {
        assert_eq!(BlockSize::new(n).is_ok(), [4, 8, 16, 32].contains(&n));
    }
    assert_eq!(BlockSize::new(usize::MAX), Err(Error::BlockSize));
    for n in [4, 8, 16, 32] {
        let mut data = vec![0; n * (n + 3) - 3];
        assert!(Block::new(&data, n + 3, BlockSize::new(n).unwrap()).is_ok());
        assert!(BlockMut::new(&mut data, n + 3, BlockSize::new(n).unwrap()).is_ok());
        data.pop();
        assert!(matches!(
            Block::new(&data, n + 3, BlockSize::new(n).unwrap()),
            Err(Error::Buffer)
        ));
        assert!(matches!(
            BlockMut::new(&mut data, n + 3, BlockSize::new(n).unwrap()),
            Err(Error::Buffer)
        ));
        assert!(matches!(
            Block::new(&data, n - 1, BlockSize::new(n).unwrap()),
            Err(Error::Buffer)
        ));
        assert!(matches!(
            BlockMut::new(&mut data, usize::MAX, BlockSize::new(n).unwrap()),
            Err(Error::Buffer)
        ));
    }
}

fn directional_expected(
    n: usize,
    mode: u8,
    top: &[u16],
    left: &[u16],
    corner: u16,
) -> Result<Vec<u16>, Error> {
    let angles = [
        32, 26, 21, 17, 13, 9, 5, 2, 0, -2, -5, -9, -13, -17, -21, -26, -32, -26, -21, -17, -13,
        -9, -5, -2, 0, 2, 5, 9, 13, 17, 21, 26, 32,
    ];
    let inverse_angles = [
        -4096, -1638, -910, -630, -482, -390, -315, -256, -315, -390, -482, -630, -910, -1638,
        -4096,
    ];
    let mut result = vec![];
    let dc = ((top
        .get(..n)
        .ok_or(Error::Buffer)?
        .iter()
        .map(|&v| u32::from(v))
        .sum::<u32>()
        + left
            .get(..n)
            .ok_or(Error::Buffer)?
            .iter()
            .map(|&v| u32::from(v))
            .sum::<u32>()
        + n as u32)
        / (2 * n) as u32) as u16;
    for y in 0..n {
        for x in 0..n {
            let value = if mode == 0 {
                (((n - 1 - x) as u32 * u32::from(*left.get(y).ok_or(Error::Buffer)?)
                    + (x + 1) as u32 * u32::from(*top.get(n).ok_or(Error::Buffer)?)
                    + (n - 1 - y) as u32 * u32::from(*top.get(x).ok_or(Error::Buffer)?)
                    + (y + 1) as u32 * u32::from(*left.get(n).ok_or(Error::Buffer)?)
                    + n as u32)
                    / (2 * n) as u32) as u16
            } else if mode == 1 {
                dc
            } else {
                let a = *angles.get(mode as usize - 2).ok_or(Error::Buffer)?;
                let (main, side, offset, step) = if mode >= 18 {
                    (top, left, x, y + 1)
                } else {
                    (left, top, y, x + 1)
                };
                let delta: i32 = step as i32 * a;
                let base = offset as i32 + delta.div_euclid(32) + 1;
                let rem = delta.rem_euclid(32);
                let sample = |k: i32| -> Result<i32, Error> {
                    Ok(if k == 0 {
                        i32::from(corner)
                    } else if k > 0 {
                        i32::from(*main.get(k as usize - 1).ok_or(Error::Buffer)?)
                    } else {
                        let side_index = (k * *inverse_angles
                            .get(mode as usize - 11)
                            .ok_or(Error::Buffer)?
                            + 128)
                            .div_euclid(256);
                        i32::from(*side.get(side_index as usize - 1).ok_or(Error::Buffer)?)
                    })
                };
                if rem == 0 {
                    sample(base)? as u16
                } else {
                    ((sample(base)? * (32 - rem) + sample(base + 1)? * rem + 16) / 32) as u16
                }
            };
            result.push(value);
        }
    }
    Ok(result)
}

#[test]
fn every_mode_size_and_depth_against_prediction_equations() {
    for bits in [8, 10] {
        for n in [4, 8, 16, 32] {
            let max = BitDepth::new(bits).unwrap().max_sample();
            let top: Vec<_> = (0..2 * n)
                .map(|i| ((i * 47 + 17) % (max as usize + 1)) as u16)
                .collect();
            let left: Vec<_> = (0..2 * n)
                .map(|i| ((i * 71 + 211) % (max as usize + 1)) as u16)
                .collect();
            let corner = max / 3;
            let refs: Vec<_> = left
                .iter()
                .rev()
                .copied()
                .chain([corner])
                .chain(top.iter().copied())
                .map(Some)
                .collect();
            for mode in 0..=34 {
                let expected = directional_expected(n, mode, &top, &left, corner).unwrap();
                assert_eq!(
                    predict(bits, n, mode, Component::Chroma, true, &refs).unwrap(),
                    expected,
                    "depth {bits} size {n} mode {mode}"
                );
                let threshold = match n {
                    8 => 7,
                    16 => 1,
                    _ => 0,
                };
                let filter = n != 4
                    && mode != 1
                    && (mode as i32 - 26).abs().min((mode as i32 - 10).abs()) > threshold;
                let (filtered_top, filtered_left, filtered_corner) = if filter {
                    let mut filtered = refs.iter().map(|v| v.unwrap()).collect::<Vec<_>>();
                    for i in 1..4 * n {
                        filtered[i] = ((u32::from(refs[i - 1].unwrap())
                            + 2 * u32::from(refs[i].unwrap())
                            + u32::from(refs[i + 1].unwrap())
                            + 2)
                            / 4) as u16;
                    }
                    (
                        filtered[2 * n + 1..].to_vec(),
                        filtered[..2 * n].iter().rev().copied().collect(),
                        filtered[2 * n],
                    )
                } else {
                    (top.clone(), left.clone(), corner)
                };
                let mut expected =
                    directional_expected(n, mode, &filtered_top, &filtered_left, filtered_corner)
                        .unwrap();
                let dc = directional_expected(n, 1, &top, &left, corner).unwrap()[0];
                for y in 0..n {
                    for x in 0..n {
                        let v = match mode {
                            1 if n < 32 && x == 0 && y == 0 => {
                                (i32::from(left[0]) + 2 * i32::from(dc) + i32::from(top[0]) + 2) / 4
                            }
                            1 if n < 32 && y == 0 => {
                                (i32::from(top[x]) + 3 * i32::from(dc) + 2) / 4
                            }
                            1 if n < 32 && x == 0 => {
                                (i32::from(left[y]) + 3 * i32::from(dc) + 2) / 4
                            }
                            26 if n < 32 && x == 0 => {
                                i32::from(top[x])
                                    + (i32::from(left[y]) - i32::from(corner)).div_euclid(2)
                            }
                            10 if n < 32 && y == 0 => {
                                i32::from(left[y])
                                    + (i32::from(top[x]) - i32::from(corner)).div_euclid(2)
                            }
                            _ => i32::from(expected[y * n + x]),
                        };
                        expected[y * n + x] = v.clamp(0, i32::from(max)) as u16;
                    }
                }
                assert_eq!(
                    predict(bits, n, mode, Component::Luma, false, &refs).unwrap(),
                    expected,
                    "depth {bits} size {n} mode {mode}"
                );
            }
        }
    }
}

#[test]
fn reference_substitution_follows_bottom_left_scan() {
    for bits in [8, 10] {
        for n in [4, 8, 16, 32] {
            for mode in 0..=34 {
                assert_eq!(
                    predict(bits, n, mode, Component::Luma, true, &vec![None; 4 * n + 1]).unwrap(),
                    vec![1 << (bits - 1); n * n]
                );
            }
            for available in 0..4 * n + 1 {
                let mut refs = vec![None; 4 * n + 1];
                refs[available] = Some(37);
                assert_eq!(
                    predict(bits, n, 18, Component::Chroma, false, &refs).unwrap(),
                    vec![37; n * n]
                );
            }
            let mut refs = vec![None; 4 * n + 1];
            refs[2 * n + 2] = Some(29);
            refs[3 * n] = Some(93);
            refs[4 * n] = Some(187);
            let mut substituted = refs.clone();
            let mut previous = 29;
            for v in &mut substituted {
                previous = v.unwrap_or(previous);
                *v = Some(previous);
            }
            for mode in 0..=34 {
                assert_eq!(
                    predict(bits, n, mode, Component::Luma, true, &refs).unwrap(),
                    predict(bits, n, mode, Component::Luma, true, &substituted).unwrap()
                );
            }
        }
    }
}

#[test]
fn strong_smoothing_is_luma_32_and_strict_threshold_only() {
    let n = 32;
    for bits in [8, 10] {
        let corner = 40;
        let mut left = vec![100; 64];
        let mut top = vec![120; 64];
        left[31] = 72;
        left[63] = 104;
        top[31] = 88;
        top[63] = 136;
        let refs: Vec<_> = left
            .iter()
            .rev()
            .copied()
            .chain([corner])
            .chain(top.iter().copied())
            .map(Some)
            .collect();
        let smooth_left: Vec<_> = (1..=64)
            .map(|i| ((64 - i) * corner + i * 104 + 32) / 64)
            .collect();
        let smooth_top: Vec<_> = (1..=64)
            .map(|i| ((64 - i) * corner + i * 136 + 32) / 64)
            .collect();
        for mode in [0, 2, 18, 34] {
            let expected =
                directional_expected(n, mode, &smooth_top, &smooth_left, corner).unwrap();
            assert_eq!(
                predict(bits, n, mode, Component::Luma, true, &refs).unwrap(),
                expected
            );
            assert_ne!(
                predict(bits, n, mode, Component::Luma, false, &refs).unwrap(),
                expected
            );
            assert_eq!(
                predict(bits, n, mode, Component::Chroma, true, &refs).unwrap(),
                directional_expected(n, mode, &top, &left, corner).unwrap()
            );
        }
        let mut refs = refs;
        let threshold = 1 << (bits - 5);
        refs[4 * n] = Some(136 + threshold);
        assert_eq!(
            predict(bits, n, 0, Component::Luma, true, &refs).unwrap(),
            predict(bits, n, 0, Component::Luma, false, &refs).unwrap()
        );
        refs[4 * n] = Some(136 + threshold - 1);
        assert_ne!(
            predict(bits, n, 0, Component::Luma, true, &refs).unwrap(),
            predict(bits, n, 0, Component::Luma, false, &refs).unwrap()
        );
    }
}

fn dequant(level: i16, factor: u8, bits: u8, n: usize, qp: u8) -> Result<i128, Error> {
    let scales = [40_i128, 45, 51, 57, 64, 72];
    let divisor = 1_i128 << (bits as u32 + n.ilog2() - 5);
    let product = i128::from(level)
        * i128::from(factor)
        * scales.get(qp as usize % 6).ok_or(Error::Qp)?
        * (1_i128 << (qp / 6));
    Ok(((product + divisor / 2).div_euclid(divisor)).clamp(-32768, 32767))
}

#[test]
fn skip_quantization_all_qps_factors_signed_extremes_and_rounding() {
    for bits in [8, 10] {
        for qp in 0..=51 + 6 * (bits - 8) {
            for factor in [1, 16, 255] {
                let levels = [
                    i16::MIN,
                    i16::MAX,
                    -1,
                    1,
                    -32,
                    32,
                    -1023,
                    1023,
                    -7,
                    7,
                    -65,
                    65,
                    -511,
                    511,
                    0,
                    3,
                ];
                let result =
                    inverse(bits, 4, InverseTransform::Skip, qp, &levels, &[factor; 16]).unwrap();
                let divisor = 1_i128 << (20 - bits);
                let expected: Vec<_> = levels
                    .iter()
                    .map(|&v| {
                        ((dequant(v, factor, bits, 4, qp).unwrap() * 128 + divisor / 2)
                            .div_euclid(divisor)) as i32
                    })
                    .collect();
                assert_eq!(result, expected, "bits {bits} qp {qp} factor {factor}");
            }
        }
    }
}

fn first_and_last_basis(n: usize) -> (Vec<i128>, Vec<i128>) {
    match n {
        4 => (vec![83, 36, -36, -83], vec![36, -83, 83, -36]),
        8 => (
            vec![89, 75, 50, 18, -18, -50, -75, -89],
            vec![18, -50, 75, -89, 89, -75, 50, -18],
        ),
        16 => (
            vec![
                90, 87, 80, 70, 57, 43, 25, 9, -9, -25, -43, -57, -70, -80, -87, -90,
            ],
            vec![
                9, -25, 43, -57, 70, -80, 87, -90, 90, -87, 80, -70, 57, -43, 25, -9,
            ],
        ),
        _ => (
            vec![
                90, 90, 88, 85, 82, 78, 73, 67, 61, 54, 46, 38, 31, 22, 13, 4, -4, -13, -22, -31,
                -38, -46, -54, -61, -67, -73, -78, -82, -85, -88, -90, -90,
            ],
            vec![
                4, -13, 22, -31, 38, -46, 54, -61, 67, -73, 78, -82, 85, -88, 90, -90, 90, -90, 88,
                -85, 82, -78, 73, -67, 61, -54, 46, -38, 31, -22, 13, -4,
            ],
        ),
    }
}

#[test]
fn dct_non_dc_impulses_both_axes_all_sizes_depths_and_signs() {
    for bits in [8, 10] {
        for n in [4, 8, 16, 32] {
            let (first, last) = first_and_last_basis(n);
            for (fx, fy, bx, by) in [(1, n - 1, &first, &last), (n - 1, 1, &last, &first)] {
                for (level, qp, factor) in [
                    (1024, 0, 16),
                    (-1024, 0, 16),
                    (i16::MAX, 51 + 6 * (bits - 8), 255),
                    (i16::MIN, 51 + 6 * (bits - 8), 255),
                ] {
                    let mut levels = vec![0; n * n];
                    levels[fy * n + fx] = level;
                    let result = inverse(
                        bits,
                        n,
                        InverseTransform::Dct,
                        qp,
                        &levels,
                        &vec![factor; n * n],
                    )
                    .unwrap();
                    let d = dequant(level, factor, bits, n, qp).unwrap();
                    let divisor = 1_i128 << (20 - bits);
                    for y in 0..n {
                        let g = ((by[y] * d + 64).div_euclid(128)).clamp(-32768, 32767);
                        for x in 0..n {
                            assert_eq!(
                                result[y * n + x],
                                ((bx[x] * g + divisor / 2).div_euclid(divisor)) as i32,
                                "bits {bits} n {n} impulse ({fx},{fy}) sign {level} at ({x},{y})"
                            );
                        }
                    }
                    assert!(result.iter().any(|&v| v != 0));
                }
            }
        }
    }
}

#[test]
fn dst_mixed_non_dc_coefficients_follow_exact_integer_matrix() {
    let basis: [[i128; 4]; 4] = [
        [29, 55, 74, 84],
        [74, 74, 0, -74],
        [84, -29, -74, 55],
        [55, -84, 74, -29],
    ];
    for bits in [8, 10] {
        for qp in [0, 17, 51 + 6 * (bits - 8)] {
            let levels = [
                i16::MAX,
                -170,
                89,
                i16::MIN,
                -97,
                223,
                1,
                -1,
                7,
                -13,
                19,
                -23,
                31,
                -43,
                53,
                -61,
            ];
            let factors = [
                16, 29, 37, 255, 17, 23, 1, 71, 7, 41, 19, 128, 3, 99, 31, 13,
            ];
            let result = inverse(
                bits,
                4,
                InverseTransform::IntraLumaDst4,
                qp,
                &levels,
                &factors,
            )
            .unwrap();
            let mut intermediate = [[0_i128; 4]; 4];
            for y in 0..4 {
                for x in 0..4 {
                    let sum: i128 = (0..4)
                        .map(|j| {
                            basis[j][y]
                                * dequant(levels[j * 4 + x], factors[j * 4 + x], bits, 4, qp)
                                    .unwrap()
                        })
                        .sum();
                    intermediate[y][x] = ((sum + 64).div_euclid(128)).clamp(-32768, 32767);
                }
            }
            let divisor = 1_i128 << (20 - bits);
            for y in 0..4 {
                for x in 0..4 {
                    let sum: i128 = (0..4).map(|j| basis[j][x] * intermediate[y][j]).sum();
                    assert_eq!(
                        result[y * 4 + x],
                        ((sum + divisor / 2).div_euclid(divisor)) as i32
                    );
                }
            }
        }
    }
}

#[test]
fn sparse_and_dense_random_blocks_match_literal_matrix_products() {
    let literal = |bits: u8, n: usize, transform, qp, levels: &[i16], factors: &[u8]| {
        let weight = |frequency: usize, position: usize| -> i128 {
            if transform == InverseTransform::IntraLumaDst4 {
                i128::from(crate::tables::DST_BASIS[frequency][position])
            } else {
                i128::from(crate::tables::DCT_BASIS[frequency * (32 / n)][position])
            }
        };
        let scaled: Vec<i128> = levels
            .iter()
            .zip(factors)
            .map(|(&level, &factor)| dequant(level, factor, bits, n, qp).unwrap())
            .collect();
        let mut intermediate = vec![0_i128; n * n];
        for x in 0..n {
            for y in 0..n {
                let sum: i128 = (0..n).map(|j| weight(j, y) * scaled[j * n + x]).sum();
                intermediate[y * n + x] = ((sum + 64) >> 7).clamp(-32768, 32767);
            }
        }
        let shift = 20 - u32::from(bits);
        let mut residual = vec![0; n * n];
        for y in 0..n {
            for x in 0..n {
                let sum: i128 = (0..n).map(|j| weight(j, x) * intermediate[y * n + j]).sum();
                residual[y * n + x] = ((sum + (1 << (shift - 1))) >> shift) as i32;
            }
        }
        residual
    };
    let mut seed = 0x6d2b_79f5_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for bits in [8, 10] {
        for (n, transform) in [
            (4, InverseTransform::IntraLumaDst4),
            (4, InverseTransform::Dct),
            (8, InverseTransform::Dct),
            (16, InverseTransform::Dct),
            (32, InverseTransform::Dct),
        ] {
            for round in 0..48 {
                let columns = 1 + next() as usize % n;
                let rows = 1 + next() as usize % n;
                let density = 1 + next() % 4;
                let qp = (next() % u32::from(52 + 6 * (bits - 8))) as u8;
                let mut levels = vec![0_i16; n * n];
                for y in 0..rows {
                    for x in 0..columns {
                        if next() % density == 0 {
                            let magnitude = if round % 8 == 0 { 0xffff } else { 0x3ff };
                            levels[y * n + x] = (next() & magnitude) as u16 as i16;
                        }
                    }
                }
                let factors: Vec<u8> = (0..n * n).map(|_| 1 + (next() % 255) as u8).collect();
                assert_eq!(
                    inverse(bits, n, transform, qp, &levels, &factors).unwrap(),
                    literal(bits, n, transform, qp, &levels, &factors),
                    "bits {bits} n {n} {transform:?} round {round}"
                );
            }
        }
    }
}

#[test]
fn intermediate_clipping_happens_before_horizontal_transform() {
    for bits in [8, 10] {
        for n in [4, 8, 16, 32] {
            let mut levels = vec![i16::MAX; n * n];
            for (i, v) in levels.iter_mut().enumerate() {
                if i % 3 == 0 {
                    *v = i16::MIN;
                }
            }
            let result = inverse(
                bits,
                n,
                InverseTransform::Dct,
                51 + 6 * (bits - 8),
                &levels,
                &vec![255; n * n],
            )
            .unwrap();
            assert!(result.iter().all(|v| v.abs() <= (n as i32*90*32768 + (1 << (19-bits))) >> (20-bits)));
            let mut saturated = vec![0_i16; n * n];
            for y in 0..n {
                saturated[y * n] = i16::MAX;
            }
            let clipped = inverse(
                bits,
                n,
                InverseTransform::Dct,
                51 + 6 * (bits - 8),
                &saturated,
                &vec![255; n * n],
            )
            .unwrap();
            let divisor = 1_i128 << (20 - bits);
            let expected_first = (32767 * 64 + divisor / 2).div_euclid(divisor) as i32;
            assert_eq!(clipped[0], expected_first);
        }
    }
}

#[test]
fn bypass_skip_transform_and_clip_add_remain_distinct() {
    for bits in [8, 10] {
        for n in [4, 8, 16, 32] {
            let levels: Vec<_> = (0..n * n)
                .map(|i| if i % 2 == 0 { i16::MIN } else { i16::MAX })
                .collect();
            let bypass = inverse(bits, n, InverseTransform::Bypass, 255, &levels, &[]).unwrap();
            assert_eq!(
                bypass,
                levels.iter().map(|&v| i32::from(v)).collect::<Vec<_>>()
            );
            let prediction = vec![BitDepth::new(bits).unwrap().max_sample() / 2; n * n];
            let mut output = vec![0; n * n];
            add_residual(
                BitDepth::new(bits).unwrap(),
                Block::new(&prediction, n, BlockSize::new(n).unwrap()).unwrap(),
                Block::new(&bypass, n, BlockSize::new(n).unwrap()).unwrap(),
                BlockMut::new(&mut output, n, BlockSize::new(n).unwrap()).unwrap(),
            )
            .unwrap();
            for (i, &value) in output.iter().enumerate() {
                assert_eq!(
                    value,
                    if i % 2 == 0 {
                        0
                    } else {
                        BitDepth::new(bits).unwrap().max_sample()
                    }
                );
            }
            if n == 4 {
                let skip = inverse(bits, n, InverseTransform::Skip, 0, &levels, &[16; 16]).unwrap();
                let transform =
                    inverse(bits, n, InverseTransform::Dct, 0, &levels, &[16; 16]).unwrap();
                assert_ne!(skip, bypass);
                assert_ne!(skip, transform);
                assert_ne!(transform, bypass);
            }
        }
    }
}

#[test]
fn independent_strides_leave_padding_and_extra_references_untouched() {
    for bits in [8, 10] {
        for n in [4, 8, 16, 32] {
            let refs = vec![Some(31); 4 * n + 4];
            let mut out = vec![u16::MAX; (n + 2) * n];
            predict_intra(
                IntraPrediction {
                    depth: BitDepth::new(bits).unwrap(),
                    component: Component::Luma,
                    mode: IntraMode::new(0).unwrap(),
                    strong_smoothing: true,
                },
                &refs,
                BlockMut::new(&mut out, n + 2, BlockSize::new(n).unwrap()).unwrap(),
            )
            .unwrap();
            for y in 0..n {
                assert_eq!(&out[y * (n + 2)..y * (n + 2) + n], &vec![31; n]);
                assert_eq!(&out[y * (n + 2) + n..(y + 1) * (n + 2)], &[u16::MAX; 2]);
            }
            let mut input = vec![i16::MIN; (n + 1) * n];
            let mut packed = vec![0; n * n];
            for y in 0..n {
                for x in 0..n {
                    let v = (x as i16 - y as i16) * 91;
                    input[y * (n + 1) + x] = v;
                    packed[y * n + x] = v;
                }
            }
            let mut residual = vec![i32::MIN; (n + 3) * n];
            inverse_residual(
                Reconstruction {
                    depth: BitDepth::new(bits).unwrap(),
                    transform: InverseTransform::Dct,
                    qp: 17,
                },
                Block::new(&input, n + 1, BlockSize::new(n).unwrap()).unwrap(),
                &vec![16; n * n],
                BlockMut::new(&mut residual, n + 3, BlockSize::new(n).unwrap()).unwrap(),
            )
            .unwrap();
            let expected = inverse(
                bits,
                n,
                InverseTransform::Dct,
                17,
                &packed,
                &vec![16; n * n],
            )
            .unwrap();
            let mut added = vec![u16::MAX; (n + 4) * n];
            add_residual(
                BitDepth::new(bits).unwrap(),
                Block::new(&out, n + 2, BlockSize::new(n).unwrap()).unwrap(),
                Block::new(&residual, n + 3, BlockSize::new(n).unwrap()).unwrap(),
                BlockMut::new(&mut added, n + 4, BlockSize::new(n).unwrap()).unwrap(),
            )
            .unwrap();
            for y in 0..n {
                for x in 0..n {
                    assert_eq!(residual[y * (n + 3) + x], expected[y * n + x]);
                    assert_eq!(
                        added[y * (n + 4) + x],
                        (31_i64 + i64::from(expected[y * n + x]))
                            .clamp(0, i64::from(BitDepth::new(bits).unwrap().max_sample()))
                            as u16
                    );
                }
                assert!(
                    residual[y * (n + 3) + n..(y + 1) * (n + 3)]
                        .iter()
                        .all(|&v| v == i32::MIN)
                );
                assert!(
                    added[y * (n + 4) + n..(y + 1) * (n + 4)]
                        .iter()
                        .all(|&v| v == u16::MAX)
                );
            }
        }
    }
}

#[test]
fn invalid_kernel_inputs_do_not_partially_write_outputs() {
    let n = 4;
    for bits in [8, 10] {
        let config = IntraPrediction {
            depth: BitDepth::new(bits).unwrap(),
            component: Component::Luma,
            mode: IntraMode::new(18).unwrap(),
            strong_smoothing: true,
        };
        let mut output = [123; 16];
        assert_eq!(
            predict_intra(
                config,
                &[None; 16],
                BlockMut::new(&mut output, n, BlockSize::new(n).unwrap()).unwrap()
            ),
            Err(Error::Buffer)
        );
        let mut refs = [None; 17];
        refs[16] = Some(BitDepth::new(bits).unwrap().max_sample() + 1);
        assert_eq!(
            predict_intra(
                config,
                &refs,
                BlockMut::new(&mut output, n, BlockSize::new(n).unwrap()).unwrap()
            ),
            Err(Error::Sample)
        );
        assert_eq!(output, [123; 16]);
        let mut residual = [123; 16];
        let config = Reconstruction {
            depth: BitDepth::new(bits).unwrap(),
            transform: InverseTransform::Dct,
            qp: 52 + 6 * (bits - 8),
        };
        assert_eq!(
            inverse_residual(
                config,
                Block::new(&[0; 16], n, BlockSize::new(n).unwrap()).unwrap(),
                &[16; 16],
                BlockMut::new(&mut residual, n, BlockSize::new(n).unwrap()).unwrap()
            ),
            Err(Error::Qp)
        );
        let config = Reconstruction { qp: 0, ..config };
        assert_eq!(
            inverse_residual(
                config,
                Block::new(&[0; 16], n, BlockSize::new(n).unwrap()).unwrap(),
                &[16; 15],
                BlockMut::new(&mut residual, n, BlockSize::new(n).unwrap()).unwrap()
            ),
            Err(Error::Buffer)
        );
        assert_eq!(
            inverse_residual(
                config,
                Block::new(&[0; 16], n, BlockSize::new(n).unwrap()).unwrap(),
                &[0; 16],
                BlockMut::new(&mut residual, n, BlockSize::new(n).unwrap()).unwrap()
            ),
            Err(Error::Scaling)
        );
        assert_eq!(
            inverse_residual(
                config,
                Block::new(&[0; 64], 8, BlockSize::new(8).unwrap()).unwrap(),
                &[16; 64],
                BlockMut::new(&mut residual, n, BlockSize::new(n).unwrap()).unwrap()
            ),
            Err(Error::BlockSize)
        );
        assert_eq!(residual, [123; 16]);
        let mut prediction = [0; 16];
        prediction[15] = BitDepth::new(bits).unwrap().max_sample() + 1;
        assert_eq!(
            add_residual(
                BitDepth::new(bits).unwrap(),
                Block::new(&prediction, n, BlockSize::new(n).unwrap()).unwrap(),
                Block::new(&[0; 16], n, BlockSize::new(n).unwrap()).unwrap(),
                BlockMut::new(&mut output, n, BlockSize::new(n).unwrap()).unwrap()
            ),
            Err(Error::Sample)
        );
        assert_eq!(output, [123; 16]);
        for transform in [InverseTransform::Skip, InverseTransform::IntraLumaDst4] {
            let mut large = [123; 64];
            assert_eq!(
                inverse_residual(
                    Reconstruction {
                        transform,
                        ..config
                    },
                    Block::new(&[0; 64], 8, BlockSize::new(8).unwrap()).unwrap(),
                    &[16; 64],
                    BlockMut::new(&mut large, 8, BlockSize::new(8).unwrap()).unwrap()
                ),
                Err(Error::Transform)
            );
            assert_eq!(large, [123; 64]);
        }
    }
}

#[test]
fn literal_non_dc_four_by_four_vectors() {
    let mut levels = [0; 16];
    levels[9] = 64;
    assert_eq!(
        inverse(8, 4, InverseTransform::Dct, 0, &levels, &[16; 16]).unwrap(),
        [
            13, 6, -6, -13, -13, -6, 6, 13, -13, -6, 6, 13, 13, 6, -6, -13
        ]
    );
    assert_eq!(
        inverse(8, 4, InverseTransform::IntraLumaDst4, 0, &levels, &[16; 16]).unwrap(),
        [
            15, 15, 0, -15, -5, -5, 0, 5, -13, -13, 0, 13, 10, 10, 0, -10
        ]
    );
    assert_eq!(
        inverse(8, 4, InverseTransform::Skip, 0, &levels, &[16; 16]).unwrap(),
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 40, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn expanded_default_and_custom_scaling_matrices_use_raster_coordinates() {
    let default_intra_scan = [
        16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 16, 17, 16, 17, 18, 17, 18, 18, 17, 18, 21, 19,
        20, 21, 20, 19, 21, 24, 22, 22, 24, 24, 22, 22, 24, 25, 25, 27, 30, 27, 25, 25, 29, 31, 35,
        35, 31, 29, 36, 41, 44, 41, 36, 47, 54, 54, 47, 65, 70, 65, 88, 88, 115,
    ];
    let mut default_raster = [0; 64];
    let mut index = 0;
    for diagonal in 0..15 {
        for x in 0..8 {
            if diagonal >= x && diagonal - x < 8 {
                default_raster[(diagonal - x) * 8 + x] = default_intra_scan[index];
                index += 1;
            }
        }
    }
    assert_eq!(index, 64);
    assert_eq!(default_raster[63], 115);
    for bits in [8, 10] {
        for n in [4, 8, 16, 32] {
            let step = n / 8;
            let mut factors = vec![16; n * n];
            if n >= 8 {
                for y in 0..n {
                    for x in 0..n {
                        factors[y * n + x] = default_raster[(y / step) * 8 + x / step];
                    }
                }
                factors[0] = 16;
            }
            let (first, last) = first_and_last_basis(n);
            for custom in [false, true] {
                if custom {
                    for (i, v) in factors.iter_mut().enumerate() {
                        *v = ((i * 73 + 19) % 255 + 1) as u8;
                    }
                    factors[0] = 251;
                }
                let mut levels = vec![0; n * n];
                levels[0] = -271;
                levels[(n - 1) * n + 1] = 919;
                let result =
                    inverse(bits, n, InverseTransform::Dct, 21, &levels, &factors).unwrap();
                let dc = dequant(levels[0], factors[0], bits, n, 21).unwrap();
                let ac = dequant(
                    levels[(n - 1) * n + 1],
                    factors[(n - 1) * n + 1],
                    bits,
                    n,
                    21,
                )
                .unwrap();
                let divisor = 1_i128 << (20 - bits);
                for y in 0..n {
                    let dc_stage = ((64 * dc + 64).div_euclid(128)).clamp(-32768, 32767);
                    let ac_stage = ((last[y] * ac + 64).div_euclid(128)).clamp(-32768, 32767);
                    for x in 0..n {
                        assert_eq!(
                            result[y * n + x],
                            ((64 * dc_stage + first[x] * ac_stage + divisor / 2)
                                .div_euclid(divisor)) as i32
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn sample_and_residual_extremes_never_wrap() {
    for bits in [8, 10] {
        let max = BitDepth::new(bits).unwrap().max_sample();
        for n in [4, 8, 16, 32] {
            let refs: Vec<_> = (0..4 * n + 1)
                .map(|i| Some(if i % 2 == 0 { 0 } else { max }))
                .collect();
            for mode in 0..=34 {
                for component in [Component::Luma, Component::Chroma] {
                    let samples = predict(bits, n, mode, component, true, &refs).unwrap();
                    assert!(samples.iter().all(|&v| v <= max));
                }
            }
            let prediction: Vec<_> = (0..n * n)
                .map(|i| if i % 2 == 0 { max } else { 0 })
                .collect();
            let residual: Vec<_> = (0..n * n)
                .map(|i| if i % 2 == 0 { i32::MAX } else { i32::MIN })
                .collect();
            let mut output = vec![123; n * n];
            add_residual(
                BitDepth::new(bits).unwrap(),
                Block::new(&prediction, n, BlockSize::new(n).unwrap()).unwrap(),
                Block::new(&residual, n, BlockSize::new(n).unwrap()).unwrap(),
                BlockMut::new(&mut output, n, BlockSize::new(n).unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(output, prediction);
        }
    }
}
