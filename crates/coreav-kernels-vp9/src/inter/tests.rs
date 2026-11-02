#![allow(clippy::indexing_slicing)]

use super::*;

const FILTERS: [InterpFilter; 4] = [
    InterpFilter::EightTap,
    InterpFilter::EightTapSmooth,
    InterpFilter::EightTapSharp,
    InterpFilter::Bilinear,
];

#[test]
fn invalid_views_and_steps_are_refused() {
    let mut samples = [0u8; 128];
    for (stride, width, height) in [
        (0, 1, 1),
        (4, 0, 4),
        (4, 4, 0),
        (3, 4, 4),
        (4, 4, 33),
        (usize::MAX, 2, 3),
        (1, u32::MAX, 1),
    ] {
        assert!(ReferencePlane::new(&samples, stride, width, height).is_none());
    }
    assert!(ReferencePlane::new(&samples, 8, 4, 16).is_some());
    for (offset, stride, width, height) in [
        (0, 8, 0, 8),
        (0, 8, 8, 0),
        (0, 128, 65, 1),
        (0, 1, 1, 65),
        (0, 7, 8, 8),
        (1, 8, 8, 16),
        (usize::MAX, 4, 4, 4),
        (0, usize::MAX, 4, 4),
    ] {
        assert!(PredictionMut::new(&mut samples, offset, stride, width, height).is_none());
    }
    assert!(PredictionMut::new(&mut samples, 0, 8, 8, 16).is_some());
    for (x, y) in [(0, 1), (1, 0), (81, 1), (1, 81), (255, 255)] {
        assert!(Sampling::new(0, 0, x, y).is_none());
    }
    assert!(Sampling::new(i32::MIN, i32::MAX, 1, 80).is_some());
}

#[test]
fn integer_positions_copy_visible_samples_and_leave_padding_alone() {
    let source: Vec<u8> = (0..=255).collect();
    for filter in FILTERS {
        let src = ReferencePlane::new(&source, 16, 12, 16).unwrap();
        let mut result = [199; 128];
        predict_inter(
            PredictionMut::new(&mut result, 3, 16, 8, 4).unwrap(),
            src,
            Sampling::new(32, 48, 16, 16).unwrap(),
            filter,
            BitDepth::Eight,
            false,
        );
        for (at, sample) in result.iter().enumerate() {
            let (row, col) = (at / 16, at % 16);
            let expected = if row < 4 && (3..11).contains(&col) {
                source[(row + 3) * 16 + col - 1]
            } else {
                199
            };
            assert_eq!(*sample, expected, "{filter:?} at {at}");
        }
    }
}

#[test]
fn bilinear_rounds_each_pass_and_compounds_the_clipped_predictions() {
    let source = [0u16, 255, 0, 255];
    let src = ReferencePlane::new(&source, 2, 2, 2).unwrap();
    let mut output = [9u16];
    predict_inter(
        PredictionMut::new(&mut output, 0, 1, 1, 1).unwrap(),
        src,
        Sampling::new(8, 8, 16, 16).unwrap(),
        InterpFilter::Bilinear,
        BitDepth::Eight,
        true,
    );
    assert_eq!(output, [69]);
    let source = [0u16, 1, 1, 2];
    let src = ReferencePlane::new(&source, 2, 2, 2).unwrap();
    predict_inter(
        PredictionMut::new(&mut output, 0, 1, 1, 1).unwrap(),
        src,
        Sampling::new(8, 8, 16, 16).unwrap(),
        InterpFilter::Bilinear,
        BitDepth::Eight,
        false,
    );
    assert_eq!(output, [2]);
}

#[test]
fn every_phase_preserves_a_constant_at_each_depth() {
    for depth in [BitDepth::Eight, BitDepth::Ten, BitDepth::Twelve] {
        let value = (1u16 << depth.bits()) - 1;
        for filter in FILTERS {
            for x in 0..16 {
                for y in 0..16 {
                    let source = [value];
                    let src = ReferencePlane::new(&source, 1, 1, 1).unwrap();
                    let mut output = [0u16; 16];
                    predict_inter(
                        PredictionMut::new(&mut output, 0, 4, 4, 4).unwrap(),
                        src,
                        Sampling::new(x - 20, y - 20, 13, 27).unwrap(),
                        filter,
                        depth,
                        false,
                    );
                    assert_eq!(output, [value; 16]);
                }
            }
        }
    }
}

#[test]
fn each_filter_has_its_specified_impulse_response() {
    let mut source = [0u8; 13 * 13];
    source[6 * 13 + 6] = 128;
    let src = ReferencePlane::new(&source, 13, 13, 13).unwrap();
    for (filter, phase_one, phase_eight) in [
        (InterpFilter::EightTap, 126, 78),
        (InterpFilter::EightTapSmooth, 64, 55),
        (InterpFilter::EightTapSharp, 127, 80),
        (InterpFilter::Bilinear, 120, 64),
    ] {
        for (phase, expected) in [(1, phase_one), (8, phase_eight)] {
            for (x, y) in [(96 + phase, 96), (96, 96 + phase)] {
                let mut result = [0];
                predict_inter(
                    PredictionMut::new(&mut result, 0, 1, 1, 1).unwrap(),
                    src,
                    Sampling::new(x, y, 16, 16).unwrap(),
                    filter,
                    BitDepth::Eight,
                    false,
                );
                assert_eq!(result, [expected], "{filter:?}, phase {phase}");
            }
        }
    }
}

fn direct(
    src: &[u16],
    dimensions: (usize, usize, usize),
    position: (i64, i64),
    filter: InterpFilter,
    depth: BitDepth,
) -> u16 {
    let (width, height, stride) = dimensions;
    let (x, y) = position;
    let taps = &SUBPEL_FILTERS[filter as usize];
    let max = (1i64 << depth.bits()) - 1;
    let mut vertical = 0;
    for (dy, &wy) in taps[(y & 15) as usize].iter().enumerate() {
        let row = ((y >> 4) + dy as i64 - 3).clamp(0, height as i64 - 1) as usize;
        let mut horizontal = 0;
        for (dx, &wx) in taps[(x & 15) as usize].iter().enumerate() {
            let col = ((x >> 4) + dx as i64 - 3).clamp(0, width as i64 - 1) as usize;
            horizontal += i64::from(src[row * stride + col]) * i64::from(wx);
        }
        vertical += ((horizontal + 64) >> 7).clamp(0, max) * i64::from(wy);
    }
    ((vertical + 64) >> 7).clamp(0, max) as u16
}

#[test]
fn scaled_blocks_equal_independent_per_sample_convolutions() {
    let source: Vec<u16> = (0..19 * 17)
        .map(|i| {
            if i % 5 < 2 {
                u16::MAX
            } else {
                (i * 71 % 4096) as u16
            }
        })
        .collect();
    for depth in [BitDepth::Eight, BitDepth::Ten, BitDepth::Twelve] {
        let src = ReferencePlane::new(&source, 19, 13, 17).unwrap();
        for filter in FILTERS {
            for phase in 0..16 {
                for (sx, sy) in [(1, 1), (16, 16), (13, 27), (32, 32), (80, 80)] {
                    let x = phase - 21;
                    let y = 23 - phase;
                    let mut result = [0u16; 32];
                    predict_inter(
                        PredictionMut::new(&mut result, 0, 8, 8, 4).unwrap(),
                        src,
                        Sampling::new(x, y, sx, sy).unwrap(),
                        filter,
                        depth,
                        false,
                    );
                    for row in 0..4 {
                        for col in 0..8 {
                            assert_eq!(
                                result[row * 8 + col],
                                direct(
                                    &source,
                                    (13, 17, 19),
                                    (
                                        i64::from(x) + col as i64 * i64::from(sx),
                                        i64::from(y) + row as i64 * i64::from(sy),
                                    ),
                                    filter,
                                    depth
                                ),
                                "{filter:?} {depth:?} phase {phase} step {sx}/{sy} row {row} col {col}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn coordinate_extremes_and_largest_blocks_stay_in_range() {
    for depth in [BitDepth::Eight, BitDepth::Ten, BitDepth::Twelve] {
        for filter in FILTERS {
            for (x, y) in [(i32::MIN, i32::MAX), (i32::MAX, i32::MIN)] {
                let source = [u16::MAX];
                let src = ReferencePlane::new(&source, 1, 1, 1).unwrap();
                let mut output = [u16::MAX; 64 * 64];
                predict_inter(
                    PredictionMut::new(&mut output, 0, 64, 64, 64).unwrap(),
                    src,
                    Sampling::new(x, y, 80, 80).unwrap(),
                    filter,
                    depth,
                    true,
                );
                assert!(output.iter().all(|&v| v < 1 << depth.bits()));
            }
        }
    }
}
