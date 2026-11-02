#![allow(clippy::indexing_slicing)]

use super::*;
use crate::BitDepth;

/// A deterministic generator of test inputs.
fn inputs(seed: u64, len: usize, range: i64) -> Vec<i64> {
    let mut x = seed;
    (0..len)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((x >> 33) as i64 % (2 * range + 1)) - range
        })
        .collect()
}

fn run(input: &[i64], f: impl FnOnce(&mut Arrays)) -> Vec<i64> {
    let mut a = Arrays {
        t: [0; 32],
        s: [0; 32],
        peak: (0, 0),
    };
    a.t[..input.len()].copy_from_slice(input);
    f(&mut a);
    a.t[..input.len()].to_vec()
}

/// Each inverse DCT is the DCT-II's inverse, `x[k] = X[0] / sqrt(2) + sum
/// X[j] cos(pi (2k + 1) j / 2N)`, within the rounding of its stages.
#[test]
fn the_inverse_dct_is_the_cosine_sum() {
    for n in 2..=5u32 {
        let n0 = 1usize << n;
        for seed in 0..50 {
            let input = inputs(seed, n0, 1000);
            let out = run(&input, |a| {
                a.dct_permutation(n);
                a.dct(n);
            });
            for (k, &o) in out.iter().enumerate() {
                let exact: f64 = input
                    .iter()
                    .enumerate()
                    .map(|(j, &x)| {
                        let c = if j == 0 {
                            core::f64::consts::FRAC_1_SQRT_2
                        } else {
                            1.0
                        };
                        x as f64
                            * c
                            * (core::f64::consts::PI * (2 * k + 1) as f64 * j as f64
                                / (2 * n0) as f64)
                                .cos()
                    })
                    .sum();
                assert!(
                    (o as f64 - exact).abs() <= f64::from(n) * 1.5,
                    "n {n} seed {seed} k {k}: {o} vs {exact}"
                );
            }
        }
    }
}

/// The 8 and 16 point inverse ADSTs are the sine sum `x[k] = sum X[j]
/// sin(pi (2k + 1) (2j + 1) / 4N)`, within the rounding of their stages.
#[test]
fn the_inverse_adst_is_the_sine_sum() {
    for n in 3..=4u32 {
        let n0 = 1usize << n;
        for seed in 0..50 {
            let input = inputs(seed, n0, 1000);
            let out = run(&input, |a| if n == 3 { a.adst8() } else { a.adst16() });
            for (k, &o) in out.iter().enumerate() {
                let exact: f64 = input
                    .iter()
                    .enumerate()
                    .map(|(j, &x)| {
                        x as f64
                            * (core::f64::consts::PI * ((2 * k + 1) * (2 * j + 1)) as f64
                                / (4 * n0) as f64)
                                .sin()
                    })
                    .sum();
                assert!(
                    (o as f64 - exact).abs() <= 3.0,
                    "n {n} seed {seed} k {k}: {o} vs {exact}"
                );
            }
        }
    }
}

/// The 4 point inverse ADST is `x[k] = sum X[j] (2 sqrt(2) / 3) sin(pi (k + 1)
/// (2j + 1) / 9)`, the sine transform its `SINPI` constants sample.
#[test]
fn the_four_point_inverse_adst_is_its_sine_sum() {
    for seed in 0..50 {
        let input = inputs(seed, 4, 1000);
        let out = run(&input, |a| a.adst4());
        for (k, &o) in out.iter().enumerate() {
            let exact: f64 = input
                .iter()
                .enumerate()
                .map(|(j, &x)| {
                    x as f64
                        * (2.0 * core::f64::consts::SQRT_2 / 3.0)
                        * (core::f64::consts::PI * ((k + 1) * (2 * j + 1)) as f64 / 9.0).sin()
                })
                .sum();
            assert!(
                (o as f64 - exact).abs() <= 2.0,
                "seed {seed} k {k}: {o} vs {exact}"
            );
        }
    }
}

/// A block whose only coefficient is its DC changes every sample by the
/// same amount, at every size.
#[test]
fn a_dc_only_block_adds_a_constant() {
    for tx_size in [
        TxSize::Tx4x4,
        TxSize::Tx8x8,
        TxSize::Tx16x16,
        TxSize::Tx32x32,
    ] {
        let size = tx_size.samples();
        for dc in [-4096i64, -64, 64, 1000] {
            let mut plane = vec![128u8; size * size];
            let mut values = vec![0i64; size * size];
            values[0] = dc;
            let coefs = Coefficients::new(&mut values, BitDepth::Eight).unwrap();
            let block = BlockMut::new(&mut plane, 0, size, tx_size).unwrap();
            inverse_transform_add(block, coefs, TxType::DctDct, false);
            assert!(plane.iter().all(|&p| p == plane[0]), "size {size} dc {dc}");
        }
    }
}

#[test]
fn a_block_that_does_not_fit_is_refused() {
    let mut plane = vec![0u8; 63];
    assert!(BlockMut::new(&mut plane, 0, 8, TxSize::Tx8x8).is_none());
    assert!(BlockMut::new(&mut plane, 0, 4, TxSize::Tx8x8).is_none());
    assert!(BlockMut::new(&mut plane, 0, 9, TxSize::Tx4x4).is_some());
    assert!(BlockMut::new(&mut plane, 0, 20, TxSize::Tx4x4).is_none());
    assert!(BlockMut::new(&mut plane, usize::MAX, 8, TxSize::Tx8x8).is_none());
}

/// A 32x32 block takes the DCT whatever its type says, and a lossless
/// block larger than 4x4 is not given the WHT: every combination the types
/// allow is defined.
#[test]
fn every_size_type_and_mode_is_defined() {
    let types = [
        TxType::DctDct,
        TxType::AdstDct,
        TxType::DctAdst,
        TxType::AdstAdst,
    ];
    let sizes = [
        TxSize::Tx4x4,
        TxSize::Tx8x8,
        TxSize::Tx16x16,
        TxSize::Tx32x32,
    ];
    for tx_size in sizes {
        let size = tx_size.samples();
        let coefs = inputs(size as u64, size * size, 64);
        let out = |tx_type, lossless| {
            let mut plane = vec![128u8; size * size];
            let mut c = coefs.clone();
            let c = Coefficients::new(&mut c, BitDepth::Eight).unwrap();
            let block = BlockMut::new(&mut plane, 0, size, tx_size).unwrap();
            inverse_transform_add(block, c, tx_type, lossless);
            plane
        };
        for tx_type in types {
            if tx_size == TxSize::Tx32x32 {
                assert_eq!(out(tx_type, false), out(TxType::DctDct, false));
            }
            if tx_size != TxSize::Tx4x4 {
                assert_eq!(out(tx_type, true), out(tx_type, false));
            }
        }
    }
}

const TYPES: [TxType; 4] = [
    TxType::DctDct,
    TxType::AdstDct,
    TxType::DctAdst,
    TxType::AdstAdst,
];
const SIZES: [TxSize; 4] = [
    TxSize::Tx4x4,
    TxSize::Tx8x8,
    TxSize::Tx16x16,
    TxSize::Tx32x32,
];
const DEPTHS: [BitDepth; 3] = [BitDepth::Eight, BitDepth::Ten, BitDepth::Twelve];

/// A coefficient outside `8 + BitDepth` bits, or a depth VP9 does not
/// have, cannot be given to a kernel.
#[test]
fn values_outside_the_domains_are_refused() {
    for bits in [0, 1, 7, 9, 11, 13, 16, 255] {
        assert_eq!(BitDepth::from_bits(bits), None, "{bits} bits");
    }
    for depth in DEPTHS {
        let limit = 1i64 << (7 + depth.bits());
        for v in [i64::MAX, i64::MIN, limit, -limit - 1] {
            let mut values = [v, 0, 0, 0];
            assert!(
                Coefficients::new(&mut values, depth).is_none(),
                "{v} at {depth:?}"
            );
        }
        for v in [limit - 1, -limit] {
            let mut values = [v, 0, 0, 0];
            assert!(
                Coefficients::new(&mut values, depth).is_some(),
                "{v} at {depth:?}"
            );
        }
        let mut values = [i64::MAX, i64::MIN, limit, -limit - 1, 5];
        Coefficients::clamped(&mut values, depth);
        assert_eq!(values, [limit - 1, -limit, limit - 1, -limit, 5]);
    }
}

/// Upper bounds on the magnitudes, relative to its inputs', of the values
/// a 1D transform of `1 << n` inputs writes to `T` and to `S`. Each such
/// value is a linear function of the inputs plus the rounding of the steps
/// before it, so the sum over inputs of its response to each alone bounds
/// it. With impulses of `2^24`, the rounding is below the `+ 1`.
fn gain(n: u32, adst: bool, lossless: bool, shift: u32) -> (i64, i64) {
    let impulse = 1i64 << 24;
    let (t, s) = (0..1usize << n)
        .map(|i| {
            let mut a = Arrays {
                t: [0; 32],
                s: [0; 32],
                peak: (impulse, 0),
            };
            a.t[i] = impulse;
            a.transform(n, adst, lossless, shift);
            a.peak
        })
        .fold((0, 0), |(t, s), (pt, ps)| (t + pt, s + ps));
    (t / impulse + 1, s / impulse + 1)
}

/// No value the 2D transform computes from coefficients in the domain at
/// 12 bits, the widest, can overflow: the rows' outputs are bounded by
/// their gain, the columns' values by theirs, a sum of two `S` values by
/// twice their bound, and every value no step stores (the products of `B`
/// and the four-point ADST's sums of products) by `2^17` times the `T`
/// values it is computed from.
#[test]
fn no_coefficient_in_the_domain_can_overflow() {
    let input = 1i64 << 19;
    for tx_size in SIZES {
        let n = tx_size.log2();
        for (adst, lossless) in [(false, false), (true, false), (false, true)] {
            if (adst && n == 5) || (lossless && n != 2) {
                continue;
            }
            let (row_t, row_s) = gain(n, adst, lossless, 2);
            let rows = i128::from(row_t * input);
            for col_adst in [false, true] {
                if col_adst && (n == 5 || lossless) {
                    continue;
                }
                let (col_t, col_s) = gain(n, col_adst, lossless, 0);
                let largest = [
                    (1 << 17) * rows,
                    2 * i128::from(row_s * input),
                    (1 << 17) * i128::from(col_t) * rows,
                    2 * i128::from(col_s) * rows,
                ]
                .into_iter()
                .max()
                .unwrap_or(0);
                assert!(largest < 1 << 62, "{tx_size:?}: {largest}");
            }
        }
    }
}

/// The extremes of the domain, at every depth, size, type and mode, run
/// without overflow (these tests run with overflow checks).
#[test]
fn the_extremes_of_the_domain_transform() {
    for depth in DEPTHS {
        let limit = 1i64 << (7 + depth.bits());
        for tx_size in SIZES {
            let size = tx_size.samples();
            for seed in 0..8u64 {
                let signs = inputs(seed, size * size, 1);
                for tx_type in TYPES {
                    for lossless in [false, true] {
                        let mut values: Vec<i64> = signs
                            .iter()
                            .map(|&s| if s < 0 { -limit } else { limit - 1 })
                            .collect();
                        let coefs = Coefficients::new(&mut values, depth).unwrap();
                        let mut plane = vec![0u16; size * size];
                        let block = BlockMut::new(&mut plane, 0, size, tx_size).unwrap();
                        inverse_transform_add(block, coefs, tx_type, lossless);
                    }
                }
            }
        }
    }
}
