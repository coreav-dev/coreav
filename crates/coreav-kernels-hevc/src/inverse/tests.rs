#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

use super::*;
use crate::BlockSize;

#[test]
fn eight_bit_add_preserves_the_full_residual_domain_and_short_last_rows() {
    let residuals = [
        i32::MIN,
        i32::MIN + 255,
        -32768,
        -256,
        -255,
        -1,
        0,
        1,
        254,
        255,
        256,
        32767,
        i32::MAX - 255,
        i32::MAX,
    ];
    for n in [4, 8, 16, 32] {
        let size = BlockSize::new(n).unwrap();
        for p in 0..=255u16 {
            let mut prediction = std::vec![u16::MAX; (n - 1) * (n + 1) + n];
            let mut residual = std::vec![i32::MIN; (n - 1) * (n + 2) + n];
            let mut output = std::vec![u16::MAX; (n - 1) * (n + 3) + n];
            for offset in 0..residuals.len() {
                for y in 0..n {
                    for x in 0..n {
                        prediction[y * (n + 1) + x] = p;
                        residual[y * (n + 2) + x] =
                            residuals[(y * n + x + offset) % residuals.len()];
                    }
                }
                add_residual(
                    BitDepth::new(8).unwrap(),
                    Block::new(&prediction, n + 1, size).unwrap(),
                    Block::new(&residual, n + 2, size).unwrap(),
                    BlockMut::new(&mut output, n + 3, size).unwrap(),
                )
                .unwrap();
                for y in 0..n {
                    for x in 0..n {
                        assert_eq!(
                            output[y * (n + 3) + x],
                            (i64::from(p) + i64::from(residual[y * (n + 2) + x])).clamp(0, 255)
                                as u16
                        );
                    }
                    if y + 1 < n {
                        assert_eq!(&output[y * (n + 3) + n..(y + 1) * (n + 3)], &[u16::MAX; 3]);
                    }
                }
            }
        }
    }
}

#[test]
fn every_invalid_eight_bit_prediction_position_preserves_the_whole_output() {
    for n in [4, 8, 16, 32] {
        let size = BlockSize::new(n).unwrap();
        let residual = std::vec![0; n * n];
        for invalid in 0..n * n {
            let mut prediction = std::vec![255; n * n];
            prediction[invalid] = 256;
            let mut output = std::vec![1234; n * n];
            assert_eq!(
                add_residual(
                    BitDepth::new(8).unwrap(),
                    Block::new(&prediction, n, size).unwrap(),
                    Block::new(&residual, n, size).unwrap(),
                    BlockMut::new(&mut output, n, size).unwrap(),
                ),
                Err(Error::Sample)
            );
            assert!(output.iter().all(|&sample| sample == 1234));
        }
    }
}

#[test]
fn main10_add_preserves_the_full_residual_domain_and_short_last_rows() {
    let residuals = [
        i32::MIN,
        i32::MIN + 1023,
        -32768,
        -1024,
        -1023,
        -1,
        0,
        1,
        1022,
        1023,
        1024,
        32767,
        i32::MAX - 1023,
        i32::MAX,
    ];
    let predictions = (0..=1023u16)
        .step_by(7)
        .chain([1, 2, 511, 512, 1021, 1022, 1023]);
    for n in [4, 8, 16, 32] {
        let size = BlockSize::new(n).unwrap();
        for p in predictions.clone() {
            let mut prediction = std::vec![u16::MAX; (n - 1) * (n + 1) + n];
            let mut residual = std::vec![i32::MIN; (n - 1) * (n + 2) + n];
            let mut output = std::vec![u16::MAX; (n - 1) * (n + 3) + n];
            for offset in 0..residuals.len() {
                for y in 0..n {
                    for x in 0..n {
                        prediction[y * (n + 1) + x] = p;
                        residual[y * (n + 2) + x] =
                            residuals[(y * n + x + offset) % residuals.len()];
                    }
                }
                add_residual(
                    BitDepth::new(10).unwrap(),
                    Block::new(&prediction, n + 1, size).unwrap(),
                    Block::new(&residual, n + 2, size).unwrap(),
                    BlockMut::new(&mut output, n + 3, size).unwrap(),
                )
                .unwrap();
                for y in 0..n {
                    for x in 0..n {
                        assert_eq!(
                            output[y * (n + 3) + x],
                            (i64::from(p) + i64::from(residual[y * (n + 2) + x])).clamp(0, 1023)
                                as u16
                        );
                    }
                    if y + 1 < n {
                        assert_eq!(&output[y * (n + 3) + n..(y + 1) * (n + 3)], &[u16::MAX; 3]);
                    }
                }
            }
        }
    }
}

#[test]
fn every_invalid_main10_prediction_position_preserves_the_whole_output() {
    for n in [4, 8, 16, 32] {
        let size = BlockSize::new(n).unwrap();
        let residual = std::vec![0; n * n];
        for invalid in 0..n * n {
            let mut prediction = std::vec![1023; n * n];
            prediction[invalid] = 1024;
            let mut output = std::vec![1234; n * n];
            assert_eq!(
                add_residual(
                    BitDepth::new(10).unwrap(),
                    Block::new(&prediction, n, size).unwrap(),
                    Block::new(&residual, n, size).unwrap(),
                    BlockMut::new(&mut output, n, size).unwrap(),
                ),
                Err(Error::Sample)
            );
            assert!(output.iter().all(|&sample| sample == 1234));
        }
    }
}
