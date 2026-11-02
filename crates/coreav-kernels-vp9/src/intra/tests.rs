#![allow(clippy::indexing_slicing)]

use super::*;
use crate::TxSize;

/// DC prediction with neither edge gives the middle of the range at every
/// depth VP9 has; a depth outside them cannot be named.
#[test]
fn dc_prediction_without_edges_is_the_middle_of_the_range() {
    for (depth, bits) in [
        (BitDepth::Eight, 8),
        (BitDepth::Ten, 10),
        (BitDepth::Twelve, 12),
    ] {
        let mut plane = vec![0u16; 16];
        let block = BlockMut::new(&mut plane, 0, 4, TxSize::Tx4x4).unwrap();
        predict_intra(block, IntraMode::Dc, &Edges::default(), depth);
        assert!(plane.iter().all(|&p| p == 1 << (bits - 1)), "{depth:?}");
    }
    assert_eq!(BitDepth::from_bits(0), None);
}
