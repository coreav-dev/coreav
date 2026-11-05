#![allow(clippy::indexing_slicing)]

use super::*;

#[test]
fn an_edge_whose_samples_leave_the_plane_is_refused() {
    let mut plane = vec![0u8; 16];
    assert!(Edge::new(&mut plane, 8, 1, FilterSize::Sixteen).is_some());
    assert!(Edge::new(&mut plane, 7, 1, FilterSize::Sixteen).is_none());
    assert!(Edge::new(&mut plane, 9, 1, FilterSize::Sixteen).is_none());
    assert!(Edge::new(&mut plane, 4, 1, FilterSize::Four).is_some());
    assert!(Edge::new(&mut plane, 3, 1, FilterSize::Eight).is_none());
    assert!(Edge::new(&mut plane, 8, 0, FilterSize::Four).is_none());
    assert!(Edge::new(&mut plane, usize::MAX, 1, FilterSize::Four).is_none());
}

/// Clause 8.8.4's limits at the ends of the level and sharpness ranges.
#[test]
fn strengths_follow_the_level_and_sharpness() {
    assert_eq!(Strength::new(0, 0), None);
    assert_eq!(Strength::new(64, 0), None);
    assert_eq!(Strength::new(1, 8), None);
    assert_eq!(
        Strength::new(63, 0),
        Some(Strength {
            limit: 63,
            blimit: 2 * 65 + 63,
            thresh: 3
        })
    );
    assert_eq!(
        Strength::new(63, 7),
        Some(Strength {
            limit: 2,
            blimit: 2 * 65 + 2,
            thresh: 3
        })
    );
    assert_eq!(
        Strength::new(10, 3),
        Some(Strength {
            limit: 5,
            blimit: 2 * 12 + 5,
            thresh: 0
        })
    );
}

fn filtered(samples: [u8; 16], size: FilterSize, lvl: u8) -> [u8; 16] {
    let mut plane = samples;
    let edge = Edge::new(&mut plane, 8, 1, size).unwrap();
    filter_edge(edge, Strength::new(lvl, 0).unwrap(), BitDepth::Eight);
    plane
}

/// A step of 2 between two flat sides is smoothed by the widest filter the
/// size allows, and only the samples that filter writes change.
#[test]
fn a_small_step_between_flat_sides_is_smoothed() {
    let step = [
        100, 100, 100, 100, 100, 100, 100, 100, 102, 102, 102, 102, 102, 102, 102, 102,
    ];
    let four = filtered(step, FilterSize::Four, 32);
    assert_eq!(&four[..6], &step[..6]);
    assert_eq!(&four[10..], &step[10..]);
    assert_ne!(four, step);
    let eight = filtered(step, FilterSize::Eight, 32);
    assert_eq!(&eight[..5], &step[..5]);
    assert_eq!(&eight[11..], &step[11..]);
    let sixteen = filtered(step, FilterSize::Sixteen, 32);
    assert_eq!(sixteen[0], 100);
    assert_eq!(sixteen[15], 102);
    assert!(sixteen.windows(2).all(|w| w[0] <= w[1]));
    assert_ne!(sixteen, eight);
}

/// An edge whose step is larger than `blimit` is a real edge and is left
/// alone.
#[test]
fn a_large_step_is_not_filtered() {
    let step = [
        0, 0, 0, 0, 0, 0, 0, 0, 200, 200, 200, 200, 200, 200, 200, 200,
    ];
    for size in [FilterSize::Four, FilterSize::Eight, FilterSize::Sixteen] {
        assert_eq!(filtered(step, size, 10), step);
    }
}

/// Every depth's extremes filter without overflow and stay in range.
#[test]
fn extremes_stay_in_range() {
    for (depth, max) in [
        (BitDepth::Eight, 255u16),
        (BitDepth::Ten, 1023),
        (BitDepth::Twelve, 4095),
    ] {
        for pattern in 0..64u32 {
            let mut plane: Vec<u16> = (0..16)
                .map(|i| if pattern >> (i % 6) & 1 == 1 { max } else { 0 })
                .collect();
            for size in [FilterSize::Four, FilterSize::Eight, FilterSize::Sixteen] {
                let edge = Edge::new(&mut plane, 8, 1, size).unwrap();
                filter_edge(edge, Strength::new(63, 0).unwrap(), depth);
                assert!(plane.iter().all(|&s| s <= max));
            }
        }
    }
}

/// Every value a sample type holds filters without overflow at every depth,
/// even outside the depth's range: the sealed sample types bound the
/// arithmetic, not the stream.
#[test]
fn every_value_a_sample_holds_filters_without_overflow() {
    for depth in [BitDepth::Eight, BitDepth::Ten, BitDepth::Twelve] {
        for (lvl, sharpness) in [(1, 0), (63, 0), (63, 7)] {
            let strength = Strength::new(lvl, sharpness).unwrap();
            for pattern in 0..256u32 {
                let wide: Vec<u16> = (0..16)
                    .map(|i| {
                        if pattern >> (i % 8) & 1 == 1 {
                            u16::MAX
                        } else {
                            0
                        }
                    })
                    .collect();
                let narrow: Vec<u8> = wide.iter().map(|&s| (s >> 8) as u8).collect();
                for size in [FilterSize::Four, FilterSize::Eight, FilterSize::Sixteen] {
                    let mut plane = wide.clone();
                    filter_edge(Edge::new(&mut plane, 8, 1, size).unwrap(), strength, depth);
                    let mut plane = narrow.clone();
                    filter_edge(Edge::new(&mut plane, 8, 1, size).unwrap(), strength, depth);
                }
            }
            let mut flat = vec![u16::MAX; 16];
            filter_edge(
                Edge::new(&mut flat, 8, 1, FilterSize::Sixteen).unwrap(),
                strength,
                depth,
            );
        }
    }
}

fn literal_wide<P: Pixel>(edge: &mut Edge<'_, P>, log2_size: u32, depth: BitDepth) {
    let n = (1isize << (log2_size - 1)) - 1;
    let mut result = [0i32; 16];
    for (k, i) in (-n..n).enumerate() {
        let mut sum = edge.get(i);
        for j in -n..=n {
            sum += edge.get((i + j).clamp(-(n + 1), n));
        }
        result[k] = ((i64::from(sum) + (1 << (log2_size - 1))) >> log2_size) as i32;
    }
    for (k, i) in (-n..n).enumerate() {
        edge.set(i, result[k], depth);
    }
}

#[test]
fn sliding_wide_sums_equal_the_literal_formula_in_both_orientations() {
    fn check<P: Pixel>(samples: Vec<P>, step: usize, size: FilterSize, log2: u32, depth: BitDepth) {
        let mut actual = samples.clone();
        let mut expected = samples;
        wide(
            &mut Edge::new(&mut actual, 8 * step + 1, step, size).unwrap(),
            log2,
            depth,
        );
        literal_wide(
            &mut Edge::new(&mut expected, 8 * step + 1, step, size).unwrap(),
            log2,
            depth,
        );
        assert_eq!(actual, expected, "{step} {size:?} {depth:?}");
    }
    let mut seed = 0x7c33_1e29u32;
    for depth in [BitDepth::Eight, BitDepth::Ten, BitDepth::Twelve] {
        for step in [1, 19] {
            for (size, log2) in [(FilterSize::Eight, 3), (FilterSize::Sixteen, 4)] {
                for pattern in 0..256 {
                    let samples: Vec<u16> = (0..16 * step + 2)
                        .map(|i| {
                            seed ^= seed << 13;
                            seed ^= seed >> 17;
                            seed ^= seed << 5;
                            match pattern % 8 {
                                0 => 0,
                                1 => u16::MAX,
                                2 => {
                                    if i & 1 == 0 {
                                        0
                                    } else {
                                        u16::MAX
                                    }
                                }
                                3 => (1 << depth.bits()) - 1,
                                4 => 100 + (i % 3) as u16,
                                _ => seed as u16,
                            }
                        })
                        .collect();
                    check(
                        samples.iter().map(|&v| v as u8).collect(),
                        step,
                        size,
                        log2,
                        depth,
                    );
                    check(samples, step, size, log2, depth);
                }
            }
        }
    }
}

#[test]
fn every_strength_and_size_preserves_filter_results_across_orientations() {
    fn check<P: Pixel>(depth: BitDepth) {
        let background = P::clip(183, depth);
        for level in 1..=63 {
            for sharpness in 0..=7 {
                for size in [FilterSize::Four, FilterSize::Eight, FilterSize::Sixteen] {
                    for pattern in 0..4 {
                        let input: [P; 16] = core::array::from_fn(|i| {
                            let value = match pattern {
                                0 => 100 + if i < 8 { 0 } else { 2 },
                                1 => 100 + i as i64 % 4,
                                2 => {
                                    if i & 1 == 0 {
                                        0
                                    } else {
                                        (1 << depth.bits()) - 1
                                    }
                                }
                                _ => (1 << depth.bits()) - 1,
                            };
                            P::clip(value, depth)
                        });
                        let mut row = input;
                        let mut column = vec![background; 16 * 19];
                        for (i, &sample) in input.iter().enumerate() {
                            column[i * 19 + 1] = sample;
                        }
                        let strength = Strength::new(level, sharpness).unwrap();
                        filter_edge(Edge::new(&mut row, 8, 1, size).unwrap(), strength, depth);
                        filter_edge(
                            Edge::new(&mut column, 8 * 19 + 1, 19, size).unwrap(),
                            strength,
                            depth,
                        );
                        for (i, &sample) in column.iter().enumerate() {
                            let expected = if i % 19 == 1 { row[i / 19] } else { background };
                            assert_eq!(sample, expected, "{level}/{sharpness} {size:?} {depth:?}");
                        }
                    }
                }
            }
        }
    }
    for depth in [BitDepth::Eight, BitDepth::Ten, BitDepth::Twelve] {
        check::<u8>(depth);
        check::<u16>(depth);
    }
}
