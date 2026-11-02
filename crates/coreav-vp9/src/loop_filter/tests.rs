#![allow(clippy::indexing_slicing)]

use super::*;
use crate::header::SEG_LVL_MAX;

fn filter(level: u8, delta_enabled: bool) -> LoopFilter {
    LoopFilter {
        level,
        sharpness: 0,
        delta_enabled,
        ref_deltas: [1, 0, -1, -1],
        mode_deltas: [0, 2],
    }
}

fn segments(enabled: bool, abs: bool, alt_l: Option<i16>) -> Segmentation {
    let mut features = [[None; SEG_LVL_MAX]; MAX_SEGMENTS];
    features[3][SEG_LVL_ALT_L] = alt_l;
    Segmentation {
        enabled,
        update_map: false,
        tree_probs: [255; 7],
        pred_probs: [255; 3],
        temporal_update: false,
        abs_or_delta_update: abs,
        features,
    }
}

/// Without deltas, every block of a segment takes its level.
#[test]
fn without_deltas_a_segment_takes_one_level() {
    let lvl = levels(&filter(40, false), &segments(true, true, Some(10)));
    assert_eq!(lvl[0], [[40; 2]; 4]);
    assert_eq!(lvl[3], [[10; 2]; 4]);
    let lvl = levels(&filter(40, false), &segments(false, true, Some(10)));
    assert_eq!(lvl[3], [[40; 2]; 4]);
}

/// The reference and mode deltas are scaled by `loop_filter_level >> 5`;
/// intra blocks take no mode delta.
#[test]
fn deltas_scale_with_the_frame_level() {
    let lvl = levels(&filter(40, true), &segments(false, false, None));
    assert_eq!(lvl[0][0][0], 42);
    assert_eq!(lvl[0][1], [40, 44]);
    assert_eq!(lvl[0][2], [38, 42]);
    let lvl = levels(&filter(20, true), &segments(false, false, None));
    assert_eq!(lvl[0][0][0], 21);
    assert_eq!(lvl[0][3], [19, 21]);
}

/// A segment's level is clipped before the deltas, and the sum after.
#[test]
fn levels_are_clipped_to_the_filter_range() {
    let lvl = levels(&filter(40, true), &segments(true, false, Some(-50)));
    assert_eq!(lvl[3][0][0], 2);
    assert_eq!(lvl[3][2][0], 0);
    let lvl = levels(&filter(62, true), &segments(true, false, Some(30)));
    assert_eq!(lvl[3][0][0], 63);
    assert_eq!(lvl[3][3][0], 61);
}
