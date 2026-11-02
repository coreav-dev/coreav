#![allow(clippy::indexing_slicing)]

use super::*;

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
            let mut coefs = vec![0i64; size * size];
            coefs[0] = dc;
            let block = BlockMut::new(&mut plane, 0, size, tx_size).unwrap();
            inverse_transform_add(block, &mut coefs, TxType::DctDct, false, 8);
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
            let block = BlockMut::new(&mut plane, 0, size, tx_size).unwrap();
            inverse_transform_add(block, &mut c, tx_type, lossless, 8);
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
