#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use super::*;
use crate::header::{self, Carried, Header};
use crate::probs::Contexts;

fn frame_header() -> FrameHeader {
    let frame = [
        0x82, 0x49, 0x83, 0x42, 0x40, 0x03, 0xf0, 0x02, 0xf5, 0x80, 0x07, 0x80, 0x00, 0x01, 0x00,
    ];
    let parsed = header::parse(
        &frame,
        &[None; 8],
        &Carried::default(),
        &Contexts::default(),
    )
    .unwrap();
    let Header::Frame { mut header, .. } = parsed.header else {
        panic!("a coded frame");
    };
    header.key_frame = false;
    header.intra_only = false;
    header.error_resilient = false;
    header.frame_parallel = false;
    header.interpolation_filter = InterpolationFilter::Switchable;
    header.allow_high_precision_mv = true;
    header
}

fn probabilities(p: u8) -> Probs {
    Probs {
        tx: [[[p; 3]; 2]; 4],
        coef: [[[[[[p; 3]; 6]; 6]; 2]; 2]; 4],
        skip: [p; 3],
        inter_mode: [[p; 3]; 7],
        interp_filter: [[p; 2]; 4],
        is_inter: [p; 4],
        comp_mode: [p; 5],
        single_ref: [[p; 2]; 5],
        comp_ref: [p; 5],
        y_mode: [[p; 9]; 4],
        uv_mode: [[p; 9]; 10],
        partition: [[p; 3]; 16],
        mv_joint: [p; 3],
        mv_sign: [p; 2],
        mv_class: [[p; 10]; 2],
        mv_class0_bit: [p; 2],
        mv_bits: [[p; 10]; 2],
        mv_class0_fr: [[[p; 3]; 2]; 2],
        mv_fr: [[p; 3]; 2],
        mv_class0_hp: [p; 2],
        mv_hp: [p; 2],
    }
}

fn counts(c: [u32; 2]) -> Counts {
    Counts {
        tx: [[[c; 3]; 2]; 4],
        coef: [[[[[[c; 3]; 6]; 6]; 2]; 2]; 4],
        skip: [c; 3],
        inter_mode: [[c; 3]; 7],
        interp_filter: [[c; 2]; 4],
        is_inter: [c; 4],
        comp_mode: [c; 5],
        single_ref: [[c; 2]; 5],
        comp_ref: [c; 5],
        y_mode: [[c; 9]; 4],
        uv_mode: [[c; 9]; 10],
        partition: [[c; 3]; 16],
        mv_joint: [c; 3],
        mv_sign: [c; 2],
        mv_class: [[c; 10]; 2],
        mv_class0_bit: [c; 2],
        mv_bits: [[c; 10]; 2],
        mv_class0_fr: [[[c; 3]; 2]; 2],
        mv_fr: [[c; 3]; 2],
        mv_class0_hp: [c; 2],
        mv_hp: [c; 2],
    }
}

#[test]
fn default_counts_are_zero_and_inter_reloads_the_saved_tables() {
    let saved = probabilities(73);
    let updated = probabilities(211);
    let c = Counts::default();
    assert_eq!(c, counts([0, 0]));
    for previous_key in [false, true] {
        assert_eq!(
            refresh(
                &saved,
                &updated,
                &c,
                &frame_header(),
                TxMode::Select,
                previous_key
            ),
            saved
        );
    }
}

#[test]
fn intra_reloads_all_except_transform_and_skip_tables() {
    let saved = probabilities(128);
    let updated = probabilities(211);
    let mut h = frame_header();
    for (key_frame, intra_only) in [(true, false), (false, true)] {
        h.key_frame = key_frame;
        h.intra_only = intra_only;
        let mut expected = saved.clone();
        expected.tx = updated.tx;
        expected.skip = updated.skip;
        assert_eq!(
            refresh(
                &saved,
                &updated,
                &Counts::default(),
                &h,
                TxMode::Select,
                false
            ),
            expected
        );
        let actual = refresh(&saved, &updated, &counts([24, 0]), &h, TxMode::Select, true);
        expected.coef = actual.coef;
        assert_eq!(actual, expected);
        assert_eq!(actual.coef[0][0][0][0][0], [184; 3]);
    }
}

#[test]
fn error_resilience_and_parallel_decoding_preserve_header_updates() {
    let saved = probabilities(73);
    let updated = probabilities(211);
    for intra in [false, true] {
        for (error_resilient, frame_parallel) in [(true, false), (false, true), (true, true)] {
            let mut h = frame_header();
            h.key_frame = intra;
            h.error_resilient = error_resilient;
            h.frame_parallel = frame_parallel;
            assert_eq!(
                refresh(&saved, &updated, &counts([24, 0]), &h, TxMode::Select, true),
                updated
            );
        }
    }
}

#[test]
fn coefficient_adaptation_uses_the_previous_frame_type() {
    let saved = probabilities(128);
    let updated = probabilities(211);
    let h = frame_header();
    for (previous_key, expected) in [(false, 184), (true, 192)] {
        let actual = refresh(
            &saved,
            &updated,
            &counts([24, 0]),
            &h,
            TxMode::Select,
            previous_key,
        );
        for by_plane in &actual.coef {
            for by_ref in by_plane {
                for by_band in by_ref {
                    for (band, by_ctx) in by_band.iter().enumerate() {
                        for (ctx, branches) in by_ctx.iter().enumerate() {
                            assert_eq!(
                                *branches,
                                [if band == 0 && ctx >= 3 { 128 } else { expected }; 3]
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn every_noncoefficient_group_uses_its_branch_counts() {
    let actual = refresh(
        &probabilities(128),
        &probabilities(211),
        &counts([20, 0]),
        &frame_header(),
        TxMode::Select,
        false,
    );
    let mut expected = probabilities(192);
    expected.coef = actual.coef;
    expected.tx[0] = [[128; 3]; 2];
    expected.tx[1] = [[192, 128, 128]; 2];
    expected.tx[2] = [[192, 192, 128]; 2];
    assert_eq!(actual, expected);
}

#[test]
fn inapplicable_groups_reload_without_adapting() {
    let saved = probabilities(128);
    let updated = probabilities(211);
    for filter in [
        InterpolationFilter::EightTap,
        InterpolationFilter::EightTapSmooth,
        InterpolationFilter::EightTapSharp,
        InterpolationFilter::Bilinear,
    ] {
        for tx_mode in [
            TxMode::Only4x4,
            TxMode::Allow8x8,
            TxMode::Allow16x16,
            TxMode::Allow32x32,
        ] {
            let mut h = frame_header();
            h.interpolation_filter = filter;
            h.allow_high_precision_mv = false;
            let actual = refresh(&saved, &updated, &counts([24, 0]), &h, tx_mode, false);
            assert_eq!(actual.interp_filter, saved.interp_filter);
            assert_eq!(actual.tx, saved.tx);
            assert_eq!(actual.mv_hp, saved.mv_hp);
            assert_eq!(actual.mv_class0_hp, saved.mv_class0_hp);
            assert_eq!(actual.skip, [192; 3]);
        }
    }
}

#[test]
fn merge_rounds_clips_and_saturates_without_overflow() {
    for (counts, sat, factor, expected) in [
        ([0, 0], 20, 128, 128),
        ([1, 0], 20, 128, 131),
        ([0, 1], 20, 128, 125),
        ([10, 0], 20, 128, 160),
        ([20, 0], 20, 128, 192),
        ([u32::MAX, 0], 20, 128, 192),
        ([0, u32::MAX], 20, 128, 65),
        ([u32::MAX, u32::MAX], 20, 128, 128),
        ([1, 2], 20, 128, 125),
        ([12, 0], 24, 112, 156),
        ([24, 0], 24, 112, 184),
        ([u32::MAX, 0], 24, 112, 184),
    ] {
        let mut p = 128;
        p.merge(&counts, sat, factor);
        assert_eq!(p, expected, "{counts:?}, {sat}, {factor}");
    }
}

#[test]
fn a_branch_changes_only_its_own_probability() {
    let saved = probabilities(128);
    let mut c = Counts::default();
    c.coef[3][1][1][5][5][2] = [0, 24];
    c.mv_class0_fr[1][1][2] = [20, 0];
    let actual = refresh(&saved, &saved, &c, &frame_header(), TxMode::Select, false);
    let mut expected = saved.clone();
    expected.coef[3][1][1][5][5][2] = 72;
    expected.mv_class0_fr[1][1][2] = 192;
    assert_eq!(actual, expected);
}
