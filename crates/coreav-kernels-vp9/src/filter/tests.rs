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
