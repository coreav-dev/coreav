#![allow(clippy::indexing_slicing)]

use super::*;

#[test]
fn every_sign_bias_layout_places_the_fixed_reference_on_its_own_list() {
    for bits in 0..8 {
        let biases = [false, bits & 1 != 0, bits & 2 != 0, bits & 4 != 0];
        if biases[1] == biases[2] && biases[2] == biases[3] {
            continue;
        }
        let layout = References::new(biases);
        for choice in [false, true] {
            let pair = layout.pair(choice);
            assert_eq!(pair[usize::from(layout.fixed_second)], layout.fixed);
            assert_eq!(
                pair[usize::from(!layout.fixed_second)],
                layout.variable[usize::from(choice)]
            );
            assert_ne!(biases[pair[0] as usize], biases[pair[1] as usize]);
        }
    }
}

#[test]
fn reference_mode_contexts_include_compound_and_intra_neighbors() {
    let r = References::new([false, false, false, true]);
    let states = [
        None,
        Some([0, -1]),
        Some([1, -1]),
        Some([3, -1]),
        Some([1, 3]),
    ];
    let expected = [
        [1, 0, 0, 1, 3],
        [0, 0, 0, 1, 3],
        [0, 0, 0, 1, 2],
        [1, 1, 1, 0, 3],
        [3, 3, 2, 3, 4],
    ];
    for (a, &above) in states.iter().enumerate() {
        for (l, &left) in states.iter().enumerate() {
            assert_eq!(r.mode_context(above, left), expected[a][l], "{a},{l}");
        }
    }
}

#[test]
fn single_reference_contexts_use_both_references_of_compound_neighbors() {
    let cases = [
        (None, None, [2, 2]),
        (None, Some([1, 3]), [2, 0]),
        (None, Some([2, 3]), [1, 3]),
        (Some([0, -1]), Some([2, 3]), [1, 3]),
        (Some([1, -1]), Some([1, 3]), [4, 1]),
        (Some([1, -1]), Some([2, 3]), [3, 3]),
        (Some([2, -1]), Some([1, 3]), [1, 3]),
        (Some([2, -1]), Some([2, 3]), [0, 4]),
        (Some([3, -1]), Some([1, 3]), [1, 0]),
        (Some([3, -1]), Some([2, 3]), [0, 1]),
        (Some([1, 3]), Some([2, 3]), [2, 2]),
        (Some([2, 3]), Some([2, 3]), [1, 3]),
    ];
    for (a, l, expected) in cases {
        assert_eq!(single_contexts(a, l), expected, "{a:?},{l:?}");
        assert_eq!(single_contexts(l, a), expected, "{l:?},{a:?}");
    }
}

#[test]
fn compound_reference_contexts_follow_the_variable_reference_in_either_list() {
    for bits in 1..7 {
        let r = References::new([false, bits & 1 != 0, bits & 2 != 0, bits & 4 != 0]);
        let fixed = Some([r.fixed, -1]);
        let first = Some([r.variable[0], -1]);
        let second = Some([r.variable[1], -1]);
        let c0 = Some(r.pair(false));
        let c1 = Some(r.pair(true));
        let cases = [
            (None, None, 2),
            (None, c0, 4),
            (None, c1, 0),
            (Some([0, -1]), c0, 3),
            (Some([0, -1]), c1, 1),
            (fixed, first, 4),
            (fixed, fixed, 3),
            (fixed, second, 1),
            (first, first, 3),
            (first, second, 1),
            (second, second, 0),
            (first, c0, 4),
            (first, c1, 1),
            (second, c0, 2),
            (second, c1, 0),
            (c0, c0, 4),
            (c0, c1, 2),
            (c1, c1, 0),
        ];
        for (a, l, expected) in cases {
            assert_eq!(r.compound_context(a, l), expected, "{bits}:{a:?},{l:?}");
            assert_eq!(r.compound_context(l, a), expected, "{bits}:{l:?},{a:?}");
        }
    }
}
