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

fn segment_tiles(
    previous: &[u8],
    configure: impl FnOnce(&mut FrameHeader),
    run: impl FnOnce(&mut Tiles<'_, u8>),
) {
    let packet = [
        0x82, 0x49, 0x83, 0x42, 0x40, 0x03, 0xf0, 0x02, 0xf5, 0x80, 0x07, 0x80, 0x00, 0x01, 0x00,
    ];
    let parsed = crate::header::parse(
        &packet,
        &[None; 8],
        &crate::header::Carried::default(),
        &crate::probs::Contexts::default(),
    )
    .unwrap();
    let crate::Header::Frame { header, .. } = parsed.header else {
        panic!()
    };
    let mut header = header;
    header.key_frame = false;
    header.width = 128;
    header.height = 128;
    header.segmentation.enabled = true;
    configure(&mut header);
    let probs = crate::probs::Contexts::default().load(0);
    let mut frame = Planes::<u8>::new(16, 16, true, true, &mut Memory::new(u64::MAX)).unwrap();
    let mut t = Tiles {
        h: &header,
        frame: &mut frame,
        probs: &probs,
        tx_mode: TxMode::Only4x4,
        mi_rows: 16,
        mi_cols: 16,
        col_start: 0,
        col_end: 16,
        context: TileContext {
            references: &[],
            previous: None,
            previous_segments: previous,
            reference_mode: ReferenceMode::Single,
        },
        counts: Counts::default(),
        mi: vec![ModeInfo::default(); 256],
        above_partition: vec![0; 16],
        left_partition: [0; 8],
        above_seg_pred: vec![false; 16],
        left_seg_pred: [false; 8],
        above_nonzero: core::array::from_fn(|_| vec![false; 32]),
        left_nonzero: [[false; 16]; 3],
        token_cache: vec![0; 1024],
        coefs: vec![0; 1024],
    };
    run(&mut t);
}

#[test]
fn predicted_segments_use_the_smallest_id_in_the_clipped_block() {
    let mut map = vec![6; 256];
    map[7 * 16 + 7] = 2;
    map[15 * 16 + 15] = 3;
    segment_tiles(
        &map,
        |_| {},
        |t| {
            let mut d = BoolDecoder::new(&[0]);
            assert_eq!(t.segment_id(&mut d, 0, 0, BLOCK_64X64), 2);
            assert_eq!(t.segment_id(&mut d, 8, 8, BLOCK_64X64), 3);
            assert_eq!(t.segment_id(&mut d, 15, 15, BLOCK_64X64), 3);
            assert_eq!(t.segment_id(&mut d, 1, 1, BLOCK_8X8), 6);
            t.context.previous_segments = &[];
            assert_eq!(t.segment_id(&mut d, 0, 0, BLOCK_64X64), 0);
        },
    );
}

#[test]
fn temporal_segment_flags_cover_the_whole_block_and_select_the_next_context() {
    let map = vec![5; 256];
    segment_tiles(
        &map,
        |h| {
            h.segmentation.update_map = true;
            h.segmentation.temporal_update = true;
            h.segmentation.pred_probs = [0, 255, 255];
        },
        |t| {
            let mut d = BoolDecoder::new(&[0x7f, 0xff, 0xff, 0xff]);
            assert_eq!(t.segment_id(&mut d, 0, 0, BLOCK_64X64), 5);
            assert!(t.above_seg_pred[..8].iter().all(|&v| v));
            assert!(t.left_seg_pred.iter().all(|&v| v));
            assert!(t.above_seg_pred[8..].iter().all(|&v| !v));
            let mut zero = BoolDecoder::new(&[0]);
            assert_eq!(t.segment_id(&mut zero, 0, 8, BLOCK_64X64), 0);
            assert!(t.left_seg_pred.iter().all(|&v| !v));
            assert!(t.above_seg_pred[8..].iter().all(|&v| !v));
        },
    );
}

#[test]
fn non_temporal_map_updates_read_new_ids_instead_of_previous_ids() {
    let map = vec![5; 256];
    segment_tiles(
        &map,
        |h| h.segmentation.update_map = true,
        |t| {
            assert_eq!(
                t.segment_id(&mut BoolDecoder::new(&[0]), 0, 0, BLOCK_64X64),
                0
            );
        },
    );
    segment_tiles(
        &map,
        |h| h.key_frame = true,
        |t| {
            assert_eq!(
                t.segment_id(&mut BoolDecoder::new(&[0]), 0, 0, BLOCK_64X64),
                0
            );
        },
    );
}
