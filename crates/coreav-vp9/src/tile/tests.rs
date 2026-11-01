#![allow(clippy::indexing_slicing)]

use super::*;

/// Reads `tree` from `data`, returning the leaf and the nodes whose
/// probabilities it asked for.
fn walk(data: &[u8], tree: &[i8]) -> (u8, Vec<usize>) {
    let mut d = BoolDecoder::new(data);
    let mut nodes = Vec::new();
    let leaf = read_tree(&mut d, tree, |node| {
        nodes.push(node);
        128
    });
    (leaf, nodes)
}

#[test]
fn a_tree_is_walked_from_its_root_by_the_bools_read() {
    let zeros = [0u8; 8];
    assert_eq!(walk(&zeros, &TOKEN_TREE), (ZERO_TOKEN, vec![0]));
    assert_eq!(walk(&zeros, &SEGMENT_TREE), (0, vec![0, 1, 3]));
    assert_eq!(walk(&zeros, &INTRA_MODE_TREE), (DC_PRED, vec![0]));

    let ones = [0xffu8; 8];
    assert_eq!(
        walk(&ones, &TOKEN_TREE),
        (DCT_VAL_CATEGORY6, vec![0, 1, 2, 5, 7, 9])
    );
    assert_eq!(walk(&ones, &SEGMENT_TREE), (7, vec![0, 2, 6]));
    assert_eq!(
        walk(&ones, &PARTITION_TREE),
        (PARTITION_SPLIT, vec![0, 1, 2])
    );
    assert_eq!(
        walk(&ones, &COLS_PARTITION_TREE),
        (PARTITION_SPLIT, vec![0])
    );
    assert_eq!(
        walk(&ones, &ROWS_PARTITION_TREE),
        (PARTITION_SPLIT, vec![0])
    );
}

#[test]
fn pareto_keeps_the_first_two_nodes_and_interpolates_the_rest() {
    for prob in 1..=255u8 {
        assert_eq!(pareto(0, prob), prob);
        assert_eq!(pareto(1, prob), prob);
        let x = usize::from(prob - 1) / 2;
        for node in 2..10 {
            let row = |r: usize| u16::from(PARETO_TABLE[r][node - 2]);
            let expected = if prob % 2 == 1 {
                row(x)
            } else {
                (row(x) + row(x + 1)) >> 1
            };
            assert_eq!(
                u16::from(pareto(node, prob)),
                expected,
                "prob {prob}, node {node}"
            );
            assert_ne!(pareto(node, prob), 0, "prob {prob}, node {node}");
        }
    }
}

#[test]
fn tile_offsets_split_the_superblocks_evenly_and_end_at_the_frame() {
    let offsets = |mis, log2| {
        (0..=1u32 << log2)
            .map(|t| tile_offset(t, mis, log2))
            .collect::<Vec<_>>()
    };
    assert_eq!(offsets(90, 0), [0, 90]);
    assert_eq!(offsets(160, 2), [0, 40, 80, 120, 160]);
    assert_eq!(offsets(135, 2), [0, 32, 64, 96, 135]);
    assert_eq!(offsets(9, 1), [0, 8, 9]);
    assert_eq!(offsets(1, 2), [0, 0, 0, 0, 1]);
}
