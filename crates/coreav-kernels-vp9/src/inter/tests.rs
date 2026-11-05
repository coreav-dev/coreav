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

fn literal<P: Pixel>(
    src: ReferencePlane<'_, P>,
    x: i64,
    y: i64,
    filter: InterpFilter,
    depth: BitDepth,
) -> P {
    let table = &SUBPEL_FILTERS[filter as usize];
    let mut sum_y = 0;
    for (t, &wy) in table[(y & 15) as usize].iter().enumerate() {
        let row = ((y >> 4) + t as i64 - 3).clamp(0, src.height as i64 - 1) as usize;
        let mut sum_x = 0;
        for (s, &wx) in table[(x & 15) as usize].iter().enumerate() {
            let col = ((x >> 4) + s as i64 - 3).clamp(0, src.width as i64 - 1) as usize;
            sum_x += i64::from(src.samples[row * src.stride + col].get()) * i64::from(wx);
        }
        let horizontal = P::clip((sum_x + 64) >> 7, depth);
        sum_y += i64::from(horizontal.get()) * i64::from(wy);
    }
    P::clip((sum_y + 64) >> 7, depth)
}

fn compare_prediction<P: Pixel>(
    source: &[P],
    depth: BitDepth,
    sampling: Sampling,
    width: u8,
    height: u8,
    filter: InterpFilter,
    compound: bool,
) {
    let src = ReferencePlane::new(source, 83, 79, 75).unwrap();
    let stride = usize::from(width) + 5;
    let initial = source[0];
    let mut output = vec![initial; 3 + (usize::from(height) - 1) * stride + usize::from(width)];
    let mut expected = output.clone();
    predict_inter(
        PredictionMut::new(&mut output, 3, stride, width, height).unwrap(),
        src,
        sampling,
        filter,
        depth,
        compound,
    );
    for r in 0..usize::from(height) {
        for c in 0..usize::from(width) {
            let at = 3 + r * stride + c;
            let p = literal(
                src,
                i64::from(sampling.x) + c as i64 * i64::from(sampling.step_x),
                i64::from(sampling.y) + r as i64 * i64::from(sampling.step_y),
                filter,
                depth,
            );
            expected[at] = if compound {
                P::clip(
                    (i64::from(initial.get()) + i64::from(p.get()) + 1) >> 1,
                    depth,
                )
            } else {
                p
            };
        }
    }
    assert_eq!(
        output, expected,
        "{depth:?} {sampling:?} {width}/{height} {filter:?} {compound}"
    );
}

#[test]
fn unscaled_all_phase_pairs_equal_literal_predictions_for_both_sample_types() {
    let patterns: Vec<Vec<u16>> = (0..5)
        .map(|pattern| {
            (0..74 * 83 + 79)
                .map(|i| match pattern {
                    0 => {
                        if i % 17 == 0 {
                            u16::MAX
                        } else {
                            0
                        }
                    }
                    1 => {
                        if i & 1 == 0 {
                            0
                        } else {
                            u16::MAX
                        }
                    }
                    2 => (i * 71) as u16,
                    3 => u16::MAX,
                    _ => 0,
                })
                .collect()
        })
        .collect();
    let byte_patterns: Vec<Vec<u8>> = patterns
        .iter()
        .map(|s| s.iter().map(|&v| v as u8).collect())
        .collect();
    for filter in FILTERS {
        for phase_x in 0..16 {
            for phase_y in 0..16 {
                let case = phase_x + 16 * phase_y + filter as usize * 256;
                let width = ((phase_x + 4 * phase_y + filter as usize * 7) % 64 + 1) as u8;
                let height = ((phase_y + 4 * phase_x + filter as usize * 11) % 64 + 1) as u8;
                let x = if case & 1 == 0 {
                    5
                } else {
                    79 - i32::from(width) - if phase_x == 0 { 0 } else { 4 }
                };
                let y = if case & 1 == 0 {
                    5
                } else {
                    75 - i32::from(height) - if phase_y == 0 { 0 } else { 4 }
                };
                let sampling =
                    Sampling::new(x * 16 + phase_x as i32, y * 16 + phase_y as i32, 16, 16)
                        .unwrap();
                for depth in [BitDepth::Eight, BitDepth::Ten, BitDepth::Twelve] {
                    for compound in [false, true] {
                        compare_prediction(
                            &patterns[case % 5],
                            depth,
                            sampling,
                            width,
                            height,
                            filter,
                            compound,
                        );
                        compare_prediction(
                            &byte_patterns[case % 5],
                            depth,
                            sampling,
                            width,
                            height,
                            filter,
                            compound,
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn edges_extreme_origins_and_every_scale_step_equal_literal_predictions() {
    let source: Vec<u16> = (0..74 * 83 + 79)
        .map(|i| (i * 119 + 32767) as u16)
        .collect();
    let bytes: Vec<u8> = source.iter().map(|&v| v as u8).collect();
    for (x, y) in [
        (-33, -35),
        (-17, 16),
        (16, -17),
        (0, 0),
        (78 * 16 + 15, 0),
        (0, 74 * 16 + 15),
        (78 * 16 + 15, 74 * 16 + 15),
        (i32::MIN, i32::MAX),
        (i32::MAX, i32::MIN),
    ] {
        for step in 1..=80 {
            let sampling = Sampling::new(x, y, step, 81 - step).unwrap();
            for filter in FILTERS {
                for depth in [BitDepth::Eight, BitDepth::Ten, BitDepth::Twelve] {
                    compare_prediction(&source, depth, sampling, 9, 3, filter, true);
                    compare_prediction(&bytes, depth, sampling, 9, 3, filter, false);
                }
            }
        }
        let sampling = Sampling::new(x, y, 16, 16).unwrap();
        for filter in FILTERS {
            for depth in [BitDepth::Eight, BitDepth::Ten, BitDepth::Twelve] {
                compare_prediction(&source, depth, sampling, 17, 9, filter, true);
                compare_prediction(&bytes, depth, sampling, 17, 9, filter, false);
            }
        }
    }
}

#[test]
fn interpolation_tables_fit_the_signed_accumulation_contract() {
    let mut l1 = 0;
    let mut minimum = 0;
    let mut maximum = 0;
    for weights in SUBPEL_FILTERS.iter().flatten() {
        assert_eq!(weights.iter().map(|&w| i32::from(w)).sum::<i32>(), 128);
        l1 = l1.max(weights.iter().map(|&w| i32::from(w).abs()).sum::<i32>());
        minimum = minimum.min(
            weights
                .iter()
                .filter(|&&w| w < 0)
                .map(|&w| i32::from(w) * 255)
                .sum::<i32>(),
        );
        maximum = maximum.max(
            weights
                .iter()
                .filter(|&&w| w > 0)
                .map(|&w| i32::from(w) * 255)
                .sum::<i32>(),
        );
    }
    assert_eq!((l1, minimum, maximum), (236, -13770, 46410));
}

#[cfg(all(target_arch = "x86_64", not(feature = "safe")))]
#[test]
fn byte_kernel_admission_is_read_only_and_scalar_refusal_preserves_output() {
    let source = vec![201u8; 74 * 83 + 79];
    let high: Vec<u16> = source.iter().map(|&p| u16::from(p)).collect();
    let src = ReferencePlane::new(&source, 83, 79, 75).unwrap();
    let high_src = ReferencePlane::new(&high, 83, 79, 75).unwrap();
    if std::env::var_os("COREAV_REQUIRE_NO_AVX2").is_some() {
        assert!(crate::simd::Avx2::detect().is_none());
    }
    for width in [1, 7, 8, 9, 15, 16, 17, 63, 64] {
        for depth in [BitDepth::Eight, BitDepth::Ten, BitDepth::Twelve] {
            for sampling in [
                Sampling::new(81, 81, 16, 16).unwrap(),
                Sampling::new(80, 81, 16, 16).unwrap(),
                Sampling::new(-1, 81, 16, 16).unwrap(),
                Sampling::new(81, 81, 17, 16).unwrap(),
            ] {
                let mut output = vec![199u8; usize::from(width) * 9];
                let dst = PredictionMut::new(&mut output, 0, usize::from(width), width, 9).unwrap();
                let interior = Interior::new(&dst, src, sampling, depth);
                let eligible = width >= 8
                    && depth == BitDepth::Eight
                    && sampling.x == 81
                    && sampling.step_x == 16;
                assert_eq!(
                    interior.and_then(|p| p.horizontal).is_some(),
                    eligible && crate::simd::Avx2::detect().is_some()
                );
                assert!(dst.samples.iter().all(|&p| p == 199));
                let mut output = vec![199u16; usize::from(width) * 9];
                let dst = PredictionMut::new(&mut output, 0, usize::from(width), width, 9).unwrap();
                assert!(
                    Interior::new(&dst, high_src, sampling, depth)
                        .and_then(|p| p.horizontal)
                        .is_none()
                );
                assert!(dst.samples.iter().all(|&p| p == 199));
            }
        }
    }
    for filter in FILTERS {
        let sampling = Sampling::new(81, 81, 16, 16).unwrap();
        let mut scalar = vec![199u8; 17 * 9];
        let mut automatic = scalar.clone();
        let dst = PredictionMut::new(&mut scalar, 0, 17, 17, 9).unwrap();
        let mut interior = Interior::new(&dst, src, sampling, BitDepth::Eight).unwrap();
        interior.horizontal = None;
        unscaled(
            dst,
            src,
            sampling,
            filter,
            BitDepth::Eight,
            true,
            Some(interior),
        );
        predict_inter(
            PredictionMut::new(&mut automatic, 0, 17, 17, 9).unwrap(),
            src,
            sampling,
            filter,
            BitDepth::Eight,
            true,
        );
        assert_eq!(scalar, automatic);
    }
}
