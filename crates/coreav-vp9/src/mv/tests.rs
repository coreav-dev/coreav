#![allow(clippy::indexing_slicing)]

use super::*;
use crate::header::{Carried, Header};
use crate::probs::Contexts;

fn header() -> FrameHeader {
    let bytes = [
        0x82, 0x49, 0x83, 0x42, 0x40, 0x03, 0xf0, 0x02, 0xf5, 0x80, 0x07, 0x80, 0x00, 0x01, 0x00,
    ];
    let parsed = crate::header::parse(
        &bytes,
        &[None; 8],
        &Carried::default(),
        &Contexts::default(),
    )
    .unwrap();
    match parsed.header {
        Header::Frame { header, .. } => *header,
        _ => panic!("expected a frame header"),
    }
}

fn mode(frames: [i8; 2], vectors: [Mv; 2], prediction: u8) -> ModeInfo {
    ModeInfo {
        ref_frames: frames,
        sub_mvs: vectors.map(|mv| [mv; 4]),
        mode: prediction,
        size: 3,
        ..ModeInfo::default()
    }
}

fn search<'a>(modes: &'a [ModeInfo], header: &'a FrameHeader) -> Search<'a> {
    Search {
        modes,
        previous: None,
        mi_rows: 8,
        mi_cols: 8,
        col_start: 0,
        col_end: 8,
        header,
    }
}

#[test]
fn nearest_order_depends_on_the_current_block_shape() {
    let h = header();
    let mut modes = [ModeInfo::default(); 64];
    modes[2 * 8 + 3] = mode([1, -1], [[7, -13], [0, 0]], 10);
    modes[3 * 8 + 2] = mode([1, -1], [[-19, 31], [0, 0]], 11);
    let s = search(&modes, &h);
    assert_eq!(
        s.candidates(3, 3, 3, 1, None).vectors,
        [[7, -13], [-19, 31]]
    );
    assert_eq!(
        s.candidates(3, 3, 4, 1, None).vectors,
        [[-19, 31], [7, -13]]
    );
}

#[test]
fn duplicate_spatial_vectors_leave_room_for_later_neighbors() {
    let h = header();
    let mut modes = [ModeInfo::default(); 64];
    modes[2 * 8 + 3] = mode([2, 1], [[99, 33], [7, -13]], 10);
    modes[3 * 8 + 2] = mode([1, -1], [[7, -13], [0, 0]], 10);
    modes[2 * 8 + 2] = mode([1, -1], [[-23, 41], [0, 0]], 10);
    assert_eq!(
        search(&modes, &h).candidates(3, 3, 3, 1, None).vectors,
        [[7, -13], [-23, 41]]
    );
}

#[test]
fn context_counts_only_the_first_two_neighbors_even_when_the_list_is_full() {
    let h = header();
    let mut modes = [ModeInfo::default(); 64];
    modes[2 * 8 + 3] = mode([1, -1], [[7, -13], [0, 0]], 13);
    modes[3 * 8 + 2] = mode([1, -1], [[-23, 41], [0, 0]], 12);
    modes[2 * 8 + 2] = mode([1, -1], [[1, 2], [0, 0]], 0);
    assert_eq!(search(&modes, &h).candidates(3, 3, 3, 1, None).context, 3);
    modes[2 * 8 + 3].mode = 0;
    modes[3 * 8 + 2].mode = 9;
    assert_eq!(search(&modes, &h).candidates(3, 3, 3, 1, None).context, 6);
}

#[test]
fn temporal_matches_follow_every_spatial_match_and_precede_different_references() {
    let h = header();
    let mut modes = [ModeInfo::default(); 64];
    let mut previous = modes;
    modes[2 * 8 + 3] = mode([2, -1], [[51, -17], [0, 0]], 10);
    modes[8 + 3] = mode([1, -1], [[-9, 27], [0, 0]], 10);
    previous[3 * 8 + 3] = mode([3, 1], [[31, -47], [13, -29]], 13);
    let mut s = search(&modes, &h);
    s.previous = Some(&previous);
    let candidates = s.candidates(3, 3, 3, 1, None);
    assert_eq!(candidates.vectors, [[-9, 27], [13, -29]]);
    assert_eq!(candidates.context, 5);
}

#[test]
fn temporal_vectors_are_optional_and_use_the_full_block_slot() {
    let h = header();
    let modes = [ModeInfo::default(); 64];
    let mut previous = modes;
    previous[0] = mode([1, -1], [[17, -35], [0, 0]], 13);
    previous[0].sub_mvs[0][2] = [55, 59];
    let mut s = search(&modes, &h);
    assert_eq!(s.candidates(0, 0, 0, 1, Some(0)).vectors, [[0, 0]; 2]);
    s.previous = Some(&previous);
    assert_eq!(
        s.candidates(0, 0, 0, 1, Some(0)).vectors,
        [[17, -35], [0, 0]]
    );
    assert_eq!(s.candidates(0, 0, 0, 1, Some(0)).context, 2);
}

#[test]
fn temporal_different_references_work_without_any_spatial_neighbor() {
    let mut h = header();
    h.ref_frame_sign_bias = [false, false, true, false];
    let modes = [ModeInfo::default(); 64];
    let mut previous = modes;
    previous[0] = mode([2, 3], [[11, -29], [-7, 23]], 10);
    let mut s = search(&modes, &h);
    s.previous = Some(&previous);
    assert_eq!(
        s.candidates(0, 0, 3, 1, None).vectors,
        [[-11, 29], [-7, 23]]
    );
}

#[test]
fn bias_negates_both_components_only_for_opposite_reference_direction() {
    let mut h = header();
    h.ref_frame_sign_bias = [false, false, true, false];
    let mut modes = [ModeInfo::default(); 64];
    modes[2 * 8 + 3] = mode([2, 3], [[11, -29], [-7, 23]], 10);
    assert_eq!(
        search(&modes, &h).candidates(3, 3, 3, 1, None).vectors,
        [[-11, 29], [-7, 23]]
    );
    h.ref_frame_sign_bias[1] = true;
    assert_eq!(
        search(&modes, &h).candidates(3, 3, 3, 1, None).vectors,
        [[11, -29], [7, -23]]
    );
}

#[test]
fn equal_compound_vectors_are_compared_before_sign_scaling() {
    let mut h = header();
    h.ref_frame_sign_bias = [false, false, true, false];
    let mut modes = [ModeInfo::default(); 64];
    modes[2 * 8 + 3] = mode([2, 3], [[11, -29]; 2], 10);
    assert_eq!(
        search(&modes, &h).candidates(3, 3, 3, 1, None).vectors,
        [[-11, 29], [0, 0]]
    );
}

#[test]
fn zero_is_a_real_candidate_and_absent_reference_slots_are_not() {
    let h = header();
    let mut modes = [ModeInfo::default(); 64];
    modes[2 * 8 + 3] = mode([1, -1], [[0, 0], [99, 33]], 12);
    modes[3 * 8 + 2] = mode([1, -1], [[-3, 37], [0, 0]], 10);
    assert_eq!(
        search(&modes, &h).candidates(3, 3, 3, 1, None).vectors,
        [[0, 0], [-3, 37]]
    );
    modes[2 * 8 + 3].ref_frames = [0, -1];
    modes[3 * 8 + 2].ref_frames = [0, -1];
    assert_eq!(
        search(&modes, &h).candidates(3, 3, 3, 1, None).vectors,
        [[0, 0]; 2]
    );
}

#[test]
fn tile_columns_exclude_neighbors_but_rows_remain_frame_relative() {
    let h = header();
    let mut modes = [ModeInfo::default(); 64];
    modes[2 * 8 + 3] = mode([1, -1], [[7, -13], [0, 0]], 10);
    modes[3 * 8 + 2] = mode([1, -1], [[-19, 31], [0, 0]], 11);
    modes[2 * 8 + 4] = mode([1, -1], [[43, -53], [0, 0]], 10);
    modes[4 * 8 + 2] = mode([1, -1], [[61, 11], [0, 0]], 10);
    let mut s = search(&modes, &h);
    s.col_start = 3;
    s.col_end = 4;
    assert_eq!(s.candidates(3, 3, 6, 1, None).vectors, [[7, -13], [0, 0]]);
    s.col_start = 2;
    assert_eq!(
        s.candidates(3, 3, 7, 1, None).vectors,
        [[-19, 31], [7, -13]]
    );
    modes[3 * 8 + 2].ref_frames = [0, -1];
    let mut s = search(&modes, &h);
    s.col_end = 4;
    assert_eq!(s.candidates(3, 3, 6, 1, None).vectors, [[7, -13], [61, 11]]);
}

#[test]
fn first_two_neighbors_select_the_touching_subblocks_for_each_current_subblock() {
    let h = header();
    let mut modes = [ModeInfo::default(); 64];
    modes[2 * 8 + 3] = mode([1, -1], [[0, 0]; 2], 10);
    modes[3 * 8 + 2] = mode([1, -1], [[0, 0]; 2], 10);
    modes[2 * 8 + 3].sub_mvs[0] = [[1, 11], [2, 22], [3, 33], [4, 44]];
    modes[3 * 8 + 2].sub_mvs[0] = [[-1, 15], [-2, 25], [-3, 35], [-4, 45]];
    let s = search(&modes, &h);
    let expected = [
        [[3, 33], [-2, 25]],
        [[4, 44], [-2, 25]],
        [[3, 33], [-4, 45]],
        [[4, 44], [-4, 45]],
    ];
    for (block, vectors) in expected.into_iter().enumerate() {
        assert_eq!(s.candidates(3, 3, 0, 1, Some(block)).vectors, vectors);
    }
    assert_eq!(s.candidates(3, 3, 3, 1, None).vectors, [[4, 44], [-4, 45]]);
}

#[test]
fn farther_neighbors_and_different_reference_fallback_use_full_block_vectors() {
    let h = header();
    let mut modes = [ModeInfo::default(); 64];
    modes[2 * 8 + 2] = mode([1, -1], [[17, -35], [0, 0]], 10);
    modes[2 * 8 + 2].sub_mvs[0][2] = [43, 11];
    modes[2 * 8 + 3] = mode([2, -1], [[-23, 41], [0, 0]], 10);
    modes[2 * 8 + 3].sub_mvs[0][2] = [55, 59];
    assert_eq!(
        search(&modes, &h).candidates(3, 3, 0, 1, Some(0)).vectors,
        [[17, -35], [-23, 41]]
    );
}

#[test]
fn candidate_clamps_use_frame_edges_and_block_extents_not_tile_edges() {
    let h = header();
    let mut modes = [ModeInfo::default(); 64];
    modes[8 + 3] = mode([1, -1], [[i32::MIN, i32::MAX], [0, 0]], 10);
    modes[2 * 8 + 2] = mode([1, -1], [[i32::MAX, i32::MIN], [0, 0]], 10);
    let mut s = search(&modes, &h);
    s.col_start = 3;
    s.col_end = 5;
    assert_eq!(
        s.candidates(2, 3, 5, 1, None).vectors,
        [[-256, 320], [0, 0]]
    );
    s.col_start = 2;
    assert_eq!(
        s.candidates(2, 3, 5, 1, None).vectors,
        [[-256, 320], [448, -320]]
    );
    assert_eq!(
        s.candidates(2, 3, 4, 1, None).vectors,
        [[384, -320], [-256, 384]]
    );
}

#[test]
fn deduplication_happens_before_clamping() {
    let h = header();
    let mut modes = [ModeInfo::default(); 64];
    modes[2 * 8 + 3] = mode([1, -1], [[1000, -1000], [0, 0]], 10);
    modes[3 * 8 + 2] = mode([1, -1], [[2000, -2000], [0, 0]], 10);
    modes[2 * 8 + 2] = mode([1, -1], [[7, 11], [0, 0]], 10);
    assert_eq!(
        search(&modes, &h).candidates(3, 3, 3, 1, None).vectors,
        [[384, -320]; 2]
    );
}

#[test]
fn hp_threshold_checks_both_signed_components_without_abs_overflow() {
    for mv in [[63, -63], [-63, 63], [0, 0]] {
        assert!(use_hp(mv));
    }
    for mv in [
        [64, 0],
        [0, -64],
        [-64, 63],
        [63, 64],
        [i32::MIN, 0],
        [0, i32::MAX],
    ] {
        assert!(!use_hp(mv));
    }
}

#[test]
fn best_rounds_odd_components_toward_zero_unless_both_are_hp_eligible() {
    let mut h = header();
    let modes = [ModeInfo::default(); 64];
    assert_eq!(
        search(&modes, &h).best(3, 3, 3, [[17, -35], [-1, 1]]),
        [[16, -34], [0, 0]]
    );
    h.allow_high_precision_mv = true;
    assert_eq!(
        search(&modes, &h).best(3, 3, 3, [[17, -35], [-1, 1]]),
        [[17, -35], [-1, 1]]
    );
    assert_eq!(
        search(&modes, &h).best(3, 3, 3, [[63, -65], [-67, 61]]),
        [[62, -64], [-66, 60]]
    );
}

#[test]
fn best_uses_the_interpolation_border_and_asymmetric_block_extents() {
    let h = header();
    let modes = [ModeInfo::default(); 64];
    assert_eq!(
        search(&modes, &h).best(2, 3, 5, [[i32::MIN, i32::MAX], [i32::MAX, i32::MIN]]),
        [[-1376, 1440], [1568, -1440]]
    );
    assert_eq!(
        search(&modes, &h).best(2, 3, 4, [[i32::MIN, i32::MAX], [i32::MAX, i32::MIN]]),
        [[-1376, 1504], [1504, -1440]]
    );
}

#[test]
fn malformed_dimensions_and_short_mode_arrays_do_not_overflow_or_panic() {
    let h = header();
    let mut s = search(&[], &h);
    for (rows, cols) in [(0, 0), (1, 1), (u32::MAX, u32::MAX)] {
        s.mi_rows = rows;
        s.mi_cols = cols;
        s.col_end = cols;
        for (row, col) in [(0, 0), (u32::MAX - 1, u32::MAX - 1), (u32::MAX, u32::MAX)] {
            for size in [0, 12, 255] {
                s.candidates(row, col, size, 1, Some(usize::MAX));
                s.best(row, col, size, [[i32::MIN, i32::MAX]; 2]);
            }
        }
    }
    s.previous = Some(&[]);
    s.candidates(u32::MAX - 1, u32::MAX - 1, 3, 1, None);
}

#[test]
fn append_preserves_raw_for_block_zero_and_prefers_earlier_subblocks() {
    let raw = [[17, -35], [-23, 41]];
    let earlier = [[7, 11], [-13, 19], [29, -31], [55, 59]];
    assert_eq!(append_sub8x8(raw, earlier, 0), raw);
    assert_eq!(append_sub8x8(raw, earlier, 1), [[7, 11], [17, -35]]);
    assert_eq!(append_sub8x8(raw, earlier, 2), [[7, 11], [17, -35]]);
    assert_eq!(append_sub8x8(raw, earlier, 3), [[29, -31], [-13, 19]]);
}

#[test]
fn append_skips_duplicates_in_order_and_zero_fills_an_exhausted_list() {
    let mv = [7, 11];
    let next = [-13, 19];
    assert_eq!(append_sub8x8([mv, next], [mv; 4], 1), [mv, next]);
    assert_eq!(append_sub8x8([mv, next], [mv; 4], 3), [mv, next]);
    assert_eq!(append_sub8x8([mv; 2], [mv; 4], 3), [mv, [0, 0]]);
    assert_eq!(
        append_sub8x8([mv; 2], [next, mv, mv, [55, 59]], 3),
        [mv, next]
    );
}
