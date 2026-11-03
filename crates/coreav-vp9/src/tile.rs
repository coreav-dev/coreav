//! Tile data (clause 6.4): the tiles, the partition walk,
//! each block's mode info, and its tokens, read with the trees and
//! probabilities of clause 9.3.

use crate::memory::Memory;
use coreav_kernels_vp9::{
    BitDepth, BlockMut, Coefficients, Edges, InterpFilter, IntraMode, PredictionMut,
    ReferencePlane, Sampling, TxSize, TxType, inverse_transform_add, predict_inter, predict_intra,
};

use crate::adapt::Counts;
use crate::bool::BoolDecoder;
use crate::compressed::{ReferenceMode, TxMode};
use crate::error::{Error, Result};
use crate::header::{FrameHeader, InterpolationFilter};
use crate::mv::{self, Search};
use crate::picture::{Picture, Planes, Sample};
use crate::probs::Probs;
use crate::tables::{
    AC_QLOOKUP, B_HEIGHT_LOG2, B_WIDTH_LOG2, CAT_PROBS, COEFBAND_4X4, COEFBAND_8X8PLUS,
    COL_SCAN_4X4, COL_SCAN_8X8, COL_SCAN_16X16, COLS_PARTITION_TREE, DC_QLOOKUP, DEFAULT_SCAN_4X4,
    DEFAULT_SCAN_8X8, DEFAULT_SCAN_16X16, DEFAULT_SCAN_32X32, ENERGY_CLASS, EXTRA_BITS,
    INTER_MODE_TREE, INTERP_FILTER_TREE, INTRA_MODE_TREE, KF_PARTITION_PROBS, KF_UV_MODE_PROBS,
    KF_Y_MODE_PROBS, MAX_TX_SIZE, MI_WIDTH_LOG2, MODE2TXFM_MAP, MV_CLASS_TREE, MV_FR_TREE,
    MV_JOINT_TREE, NUM_4X4_HIGH, NUM_4X4_WIDE, NUM_8X8_HIGH, NUM_8X8_WIDE, PARETO_TABLE,
    PARTITION_TREE, ROW_SCAN_4X4, ROW_SCAN_8X8, ROW_SCAN_16X16, ROWS_PARTITION_TREE, SEGMENT_TREE,
    SIZE_GROUP, SS_SIZE, SUBSIZE, TOKEN_TREE, TX_SIZE_8_TREE, TX_SIZE_16_TREE, TX_SIZE_32_TREE,
};

const BLOCK_8X8: u8 = 3;
const BLOCK_64X64: u8 = 12;
const BLOCK_INVALID: u8 = 14;
const PARTITION_HORZ: u8 = 1;
const PARTITION_VERT: u8 = 2;
const PARTITION_SPLIT: u8 = 3;
const DC_PRED: u8 = 0;
const SEG_LVL_ALT_Q: usize = 0;
const SEG_LVL_REF_FRAME: usize = 2;
const SEG_LVL_SKIP: usize = 3;
const DCT_DCT: u8 = 0;
const ADST_DCT: u8 = 1;
const DCT_ADST: u8 = 2;
const ZERO_TOKEN: u8 = 0;
const DCT_VAL_CATEGORY6: u8 = 10;

/// The tree decoding process (clause 9.3.3), with `prob` giving the
/// probability of each node. An inner node of a tree (clause 9.3.1) holds
/// the index of its two children, a leaf its negated value.
fn read_tree(d: &mut BoolDecoder<'_>, tree: &[i8], mut prob: impl FnMut(usize) -> u8) -> u8 {
    let mut n = 0i8;
    loop {
        let node = n.unsigned_abs() as usize;
        let bit = d.read(prob(node >> 1));
        n = tree.get(node + usize::from(bit)).copied().unwrap_or(0);
        if n <= 0 {
            return n.unsigned_abs();
        }
    }
}

fn counted_bool(d: &mut BoolDecoder<'_>, probability: u8, counts: &mut [u32; 2]) -> bool {
    let bit = d.read(probability);
    if let Some(count) = counts.get_mut(usize::from(bit)) {
        *count = count.saturating_add(1);
    }
    bit
}

fn counted_tree(
    d: &mut BoolDecoder<'_>,
    tree: &[i8],
    probabilities: &[u8],
    counts: &mut [[u32; 2]],
) -> u8 {
    let mut n = 0usize;
    loop {
        let mut unused = [0; 2];
        let bit = counted_bool(
            d,
            get(probabilities, n >> 1),
            counts.get_mut(n >> 1).unwrap_or(&mut unused),
        );
        let child = get(tree, n + usize::from(bit));
        if child <= 0 {
            return child.unsigned_abs();
        }
        n = child as usize;
    }
}

fn observe(tree: &[i8], node: usize, symbol: u8, counts: &mut [[u32; 2]]) -> bool {
    for bit in 0..2 {
        let child = get(tree, node + bit);
        let found = if child <= 0 {
            child.unsigned_abs() == symbol
        } else {
            observe(tree, child as usize, symbol, counts)
        };
        if found {
            if let Some(count) = counts.get_mut(node >> 1).and_then(|c| c.get_mut(bit)) {
                *count = count.saturating_add(1);
            }
            return true;
        }
    }
    false
}

fn read_mv(
    d: &mut BoolDecoder<'_>,
    probs: &Probs,
    counts: &mut Counts,
    best: [i32; 2],
    allow_hp: bool,
) -> [i32; 2] {
    let hp = allow_hp && mv::use_hp(best);
    let joint = counted_tree(d, &MV_JOINT_TREE, &probs.mv_joint, &mut counts.mv_joint);
    let mut result = best;
    for comp in 0..2 {
        if joint != 3 && joint != if comp == 0 { 2 } else { 1 } {
            continue;
        }
        let sign = counted_bool(
            d,
            get(&probs.mv_sign, comp),
            counts.mv_sign.get_mut(comp).unwrap_or(&mut [0; 2]),
        );
        let class = counted_tree(
            d,
            &MV_CLASS_TREE,
            &get(&probs.mv_class, comp),
            counts.mv_class.get_mut(comp).unwrap_or(&mut [[0; 2]; 10]),
        );
        let magnitude = if class == 0 {
            let bit = usize::from(counted_bool(
                d,
                get(&probs.mv_class0_bit, comp),
                counts.mv_class0_bit.get_mut(comp).unwrap_or(&mut [0; 2]),
            ));
            let frac = counted_tree(
                d,
                &MV_FR_TREE,
                &get(&get(&probs.mv_class0_fr, comp), bit),
                counts
                    .mv_class0_fr
                    .get_mut(comp)
                    .and_then(|r| r.get_mut(bit))
                    .unwrap_or(&mut [[0; 2]; 3]),
            );
            let high = if hp {
                counted_bool(
                    d,
                    get(&probs.mv_class0_hp, comp),
                    counts.mv_class0_hp.get_mut(comp).unwrap_or(&mut [0; 2]),
                )
            } else {
                if let Some(c) = counts.mv_class0_hp.get_mut(comp).and_then(|c| c.get_mut(1)) {
                    *c = c.saturating_add(1);
                }
                true
            };
            ((bit as i32) << 3) + (i32::from(frac) << 1) + i32::from(high) + 1
        } else {
            let mut bits = 0;
            for i in 0..usize::from(class) {
                let bit = counted_bool(
                    d,
                    get(&get(&probs.mv_bits, comp), i),
                    counts
                        .mv_bits
                        .get_mut(comp)
                        .and_then(|r| r.get_mut(i))
                        .unwrap_or(&mut [0; 2]),
                );
                bits |= i32::from(bit) << i;
            }
            let frac = counted_tree(
                d,
                &MV_FR_TREE,
                &get(&probs.mv_fr, comp),
                counts.mv_fr.get_mut(comp).unwrap_or(&mut [[0; 2]; 3]),
            );
            let high = if hp {
                counted_bool(
                    d,
                    get(&probs.mv_hp, comp),
                    counts.mv_hp.get_mut(comp).unwrap_or(&mut [0; 2]),
                )
            } else {
                if let Some(c) = counts.mv_hp.get_mut(comp).and_then(|c| c.get_mut(1)) {
                    *c = c.saturating_add(1);
                }
                true
            };
            (2 << (class + 2)) + (bits << 3) + (i32::from(frac) << 1) + i32::from(high) + 1
        };
        if let Some(value) = result.get_mut(comp) {
            *value += if sign { -magnitude } else { magnitude };
        }
    }
    result
}

/// `pareto(node, prob)` (clause 9.3.2): a token's probability past its
/// second node.
fn pareto(node: usize, prob: u8) -> u8 {
    if node < 2 {
        return prob;
    }
    let x = usize::from(prob.saturating_sub(1) / 2);
    let at = |row: usize| {
        PARETO_TABLE
            .get(row)
            .and_then(|r| r.get(node - 2))
            .copied()
            .map_or(0, u16::from)
    };
    if prob & 1 == 1 {
        at(x) as u8
    } else {
        ((at(x) + at(x + 1)) >> 1) as u8
    }
}

fn get<T: Copy + Default>(table: &[T], i: usize) -> T {
    table.get(i).copied().unwrap_or_default()
}

/// `get_tile_offset` (clause 6.4.1).
fn tile_offset(tile: u32, mis: u32, log2: u8) -> u32 {
    let sbs = (mis + 7) >> 3;
    (((tile * sbs) >> log2) << 3).min(mis)
}

/// What later blocks of a frame, and its loop filter, read of one 8x8
/// block (clause 6.4.4).
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct ModeInfo {
    pub(crate) sub_modes: [u8; 4],
    pub(crate) skip: bool,
    pub(crate) tx_size: u8,
    /// `MiSize`.
    pub(crate) size: u8,
    pub(crate) segment_id: u8,
    pub(crate) ref_frames: [i8; 2],
    pub(crate) sub_mvs: [[[i32; 2]; 4]; 2],
    pub(crate) mode: u8,
    pub(crate) filter: u8,
}

/// `get_plane_block_size` (clause 6.4.23): the size `size` covers in a
/// plane subsampled by `(sub_x, sub_y)`.
fn plane_block_size(size: u8, (sub_x, sub_y): (bool, bool)) -> Result<u8> {
    let s = get(
        &get(&get(&SS_SIZE, size.into()), sub_x.into()),
        sub_y.into(),
    );
    if s >= BLOCK_INVALID {
        Err(Error::Invalid(
            "a block too narrow or too flat for its chroma",
        ))
    } else {
        Ok(s)
    }
}

/// `get_uv_tx_size` (clause 6.4.22): the transform size of a block's
/// chroma planes, subsampled by `sub`.
pub(crate) fn uv_tx_size(size: u8, tx_size: u8, sub: (bool, bool)) -> Result<u8> {
    if size < BLOCK_8X8 {
        return Ok(0);
    }
    Ok(tx_size.min(get(&MAX_TX_SIZE, plane_block_size(size, sub)?.into())))
}

/// A block's mode info as its residual reads it.
struct Block {
    row: u32,
    col: u32,
    info: ModeInfo,
    uv_mode: u8,
    avail_u: bool,
    avail_l: bool,
}

pub(crate) struct TileContext<'a> {
    pub(crate) references: &'a [Option<&'a Picture>],
    pub(crate) previous: Option<&'a [ModeInfo]>,
    pub(crate) previous_segments: &'a [u8],
    pub(crate) reference_mode: ReferenceMode,
}

pub(crate) struct DecodedTiles {
    pub(crate) clean: bool,
    pub(crate) modes: Vec<ModeInfo>,
    pub(crate) counts: Counts,
}

/// The state of one frame's tiles: the mode info of its 8x8 blocks and the
/// above and left contexts of clause 7.4.
struct Tiles<'a, P> {
    h: &'a FrameHeader,
    frame: &'a mut Planes<P>,
    probs: &'a Probs,
    tx_mode: TxMode,
    mi_rows: u32,
    mi_cols: u32,
    col_start: u32,
    col_end: u32,
    context: TileContext<'a>,
    counts: Counts,
    mi: Vec<ModeInfo>,
    above_partition: Vec<u8>,
    left_partition: [u8; 8],
    above_seg_pred: Vec<bool>,
    left_seg_pred: [bool; 8],
    above_nonzero: [Vec<bool>; 3],
    left_nonzero: [[bool; 16]; 3],
    token_cache: Vec<u8>,
    /// `Tokens` of the transform block being read, in raster order.
    coefs: Vec<i64>,
}

/// Reads the tiles of a frame (`decode_tiles`, clause 6.4) from
/// `data`, the frame's bytes after its compressed header, with the
/// probabilities and transform mode of its compressed header. Returns
/// whether every tile's boolean decoder ended as clause 9.2.3 requires, and
/// the mode info of each 8x8 block in raster order.
pub(crate) fn read<P: Sample>(
    data: &[u8],
    h: &FrameHeader,
    probs: &Probs,
    tx_mode: TxMode,
    frame: &mut Planes<P>,
    context: TileContext<'_>,
    memory: &mut Memory,
) -> Result<DecodedTiles> {
    let mi_cols = h.mi_cols();
    let mi_rows = h.mi_rows();
    let sb_cols = (mi_cols as usize).div_ceil(8);
    let mut t = Tiles {
        h,
        frame,
        probs,
        tx_mode,
        mi_rows,
        mi_cols,
        col_start: 0,
        col_end: mi_cols,
        context,
        counts: Counts::default(),
        mi: memory.filled(
            (mi_rows as usize)
                .checked_mul(mi_cols as usize)
                .ok_or(Error::Limit(crate::LimitKind::MemoryBytes))?,
            ModeInfo::default(),
        )?,
        above_partition: memory.filled(sb_cols * 8, 0)?,
        left_partition: [0; 8],
        above_seg_pred: memory.filled(mi_cols as usize, false)?,
        left_seg_pred: [false; 8],
        above_nonzero: [
            memory.filled(sb_cols * 16, false)?,
            memory.filled(sb_cols * 16, false)?,
            memory.filled(sb_cols * 16, false)?,
        ],
        left_nonzero: [[false; 16]; 3],
        token_cache: memory.filled(1024, 0)?,
        coefs: memory.filled(1024, 0)?,
    };
    let tile_cols = 1u32 << h.tiles.cols_log2;
    let tile_rows = 1u32 << h.tiles.rows_log2;
    let mut rest = data;
    let mut clean = true;
    for tile_row in 0..tile_rows {
        for tile_col in 0..tile_cols {
            let last = tile_row == tile_rows - 1 && tile_col == tile_cols - 1;
            let tile = if last {
                core::mem::take(&mut rest)
            } else {
                let (size, after) = rest
                    .split_first_chunk::<4>()
                    .ok_or(Error::Truncated("a tile size"))?;
                let size = u32::from_be_bytes(*size) as usize;
                if size > after.len() {
                    return Err(Error::Truncated("a tile"));
                }
                let (tile, after) = after.split_at(size);
                rest = after;
                tile
            };
            let row_start = tile_offset(tile_row, mi_rows, h.tiles.rows_log2);
            let row_end = tile_offset(tile_row + 1, mi_rows, h.tiles.rows_log2);
            t.col_start = tile_offset(tile_col, mi_cols, h.tiles.cols_log2);
            let col_end = tile_offset(tile_col + 1, mi_cols, h.tiles.cols_log2);
            t.col_end = col_end;
            let mut d = BoolDecoder::new(tile);
            for r in (row_start..row_end).step_by(8) {
                t.left_partition = [0; 8];
                t.left_seg_pred = [false; 8];
                t.left_nonzero = [[false; 16]; 3];
                for c in (t.col_start..col_end).step_by(8) {
                    t.partition(&mut d, r, c, BLOCK_64X64)?;
                }
            }
            clean &= d.exit();
        }
    }
    Ok(DecodedTiles {
        clean,
        modes: t.mi,
        counts: t.counts,
    })
}

impl<P: Sample> Tiles<'_, P> {
    fn segment_id(&mut self, d: &mut BoolDecoder<'_>, row: u32, col: u32, size: u8) -> u8 {
        let seg = &self.h.segmentation;
        if !seg.enabled {
            return 0;
        }
        if self.h.is_intra() {
            return if seg.update_map {
                read_tree(d, &SEGMENT_TREE, |node| get(&seg.tree_probs, node))
            } else {
                0
            };
        }
        let width = u32::from(get(&NUM_8X8_WIDE, size.into()));
        let height = u32::from(get(&NUM_8X8_HIGH, size.into()));
        let mut predicted = 7;
        for r in row..(row + height).min(self.mi_rows) {
            for c in col..(col + width).min(self.mi_cols) {
                predicted = predicted.min(get(
                    self.context.previous_segments,
                    r as usize * self.mi_cols as usize + c as usize,
                ));
            }
        }
        if !seg.update_map {
            return predicted;
        }
        let use_prediction = if seg.temporal_update {
            let ctx = usize::from(get(&self.above_seg_pred, col as usize))
                + usize::from(get(&self.left_seg_pred, (row & 7) as usize));
            let value = d.read(get(&seg.pred_probs, ctx));
            for c in col..(col + width).min(self.mi_cols) {
                if let Some(v) = self.above_seg_pred.get_mut(c as usize) {
                    *v = value;
                }
            }
            for r in row..(row + height).min(self.mi_rows) {
                if let Some(v) = self.left_seg_pred.get_mut((r & 7) as usize) {
                    *v = value;
                }
            }
            value
        } else {
            false
        };
        if use_prediction {
            predicted
        } else {
            read_tree(d, &SEGMENT_TREE, |node| get(&seg.tree_probs, node))
        }
    }

    fn mi_at(&self, row: u32, col: u32) -> ModeInfo {
        get(
            &self.mi,
            row as usize * self.mi_cols as usize + col as usize,
        )
    }

    /// `decode_partition` (clause 6.4.3).
    fn partition(&mut self, d: &mut BoolDecoder<'_>, r: u32, c: u32, bsize: u8) -> Result<()> {
        if r >= self.mi_rows || c >= self.mi_cols {
            return Ok(());
        }
        let num8x8 = u32::from(get(&NUM_8X8_WIDE, bsize.into()));
        let half = num8x8 >> 1;
        let has_rows = r + half < self.mi_rows;
        let has_cols = c + half < self.mi_cols;
        let bsl = get(&MI_WIDTH_LOG2, bsize.into());
        let boffset = get(&MI_WIDTH_LOG2, BLOCK_64X64.into()) - bsl;
        let (mut above, mut left) = (0u8, 0u8);
        for i in 0..num8x8 {
            above |= get(&self.above_partition, (c + i) as usize);
            left |= get(&self.left_partition, ((r + i) & 7) as usize);
        }
        let ctx = usize::from(bsl) * 4
            + usize::from(left & (1 << boffset) != 0) * 2
            + usize::from(above & (1 << boffset) != 0);
        let probs = if self.h.is_intra() {
            get(&KF_PARTITION_PROBS, ctx)
        } else {
            get(&self.probs.partition, ctx)
        };
        let partition = match (has_rows, has_cols) {
            (true, true) => read_tree(d, &PARTITION_TREE, |node| get(&probs, node)),
            (false, true) => read_tree(d, &COLS_PARTITION_TREE, |_| get(&probs, 1)),
            (true, false) => read_tree(d, &ROWS_PARTITION_TREE, |_| get(&probs, 2)),
            (false, false) => PARTITION_SPLIT,
        };
        let subsize = get(&SUBSIZE, partition.into())
            .get(usize::from(bsize))
            .copied()
            .unwrap_or(BLOCK_INVALID);
        if let Some(counts) = self.counts.partition.get_mut(ctx) {
            observe(&PARTITION_TREE, 0, partition, counts);
        }
        if subsize < BLOCK_8X8 || partition == 0 {
            self.block(d, r, c, subsize)?;
        } else if partition == PARTITION_HORZ {
            self.block(d, r, c, subsize)?;
            if has_rows {
                self.block(d, r + half, c, subsize)?;
            }
        } else if partition == PARTITION_VERT {
            self.block(d, r, c, subsize)?;
            if has_cols {
                self.block(d, r, c + half, subsize)?;
            }
        } else {
            self.partition(d, r, c, subsize)?;
            self.partition(d, r, c + half, subsize)?;
            self.partition(d, r + half, c, subsize)?;
            self.partition(d, r + half, c + half, subsize)?;
        }
        if bsize == BLOCK_8X8 || partition != PARTITION_SPLIT {
            let above = 15 >> get(&B_WIDTH_LOG2, subsize.into());
            let left = 15 >> get(&B_HEIGHT_LOG2, subsize.into());
            for i in 0..num8x8 {
                if let Some(a) = self.above_partition.get_mut((c + i) as usize) {
                    *a = above;
                }
                if let Some(l) = self.left_partition.get_mut(((r + i) & 7) as usize) {
                    *l = left;
                }
            }
        }
        Ok(())
    }

    /// `decode_block` (clause 6.4.4).
    fn block(&mut self, d: &mut BoolDecoder<'_>, r: u32, c: u32, size: u8) -> Result<()> {
        if size >= BLOCK_INVALID {
            return Err(Error::Invalid("a partition of an invalid block size"));
        }
        let avail_u = r > 0;
        let avail_l = c > self.col_start;
        let above = avail_u.then(|| self.mi_at(r - 1, c));
        let left = avail_l.then(|| self.mi_at(r, c - 1));
        let segment_id = self.segment_id(d, r, c, size);
        let seg = &self.h.segmentation;
        let forced_ref = seg
            .features
            .get(usize::from(segment_id))
            .and_then(|f| f.get(SEG_LVL_REF_FRAME))
            .copied()
            .flatten()
            .filter(|_| seg.enabled);
        let skip_active = seg.enabled
            && seg
                .features
                .get(usize::from(segment_id))
                .and_then(|f| f.get(SEG_LVL_SKIP))
                .is_some_and(Option::is_some);
        let skip = skip_active || {
            let ctx = usize::from(above.is_some_and(|m| m.skip))
                + usize::from(left.is_some_and(|m| m.skip));
            counted_bool(
                d,
                get(&self.probs.skip, ctx),
                self.counts.skip.get_mut(ctx).unwrap_or(&mut [0; 2]),
            )
        };
        let is_inter = !self.h.is_intra()
            && forced_ref.map_or_else(
                || {
                    let ai = above.is_some_and(|m| m.ref_frames[0] <= 0);
                    let li = left.is_some_and(|m| m.ref_frames[0] <= 0);
                    let ctx = if avail_u && avail_l {
                        if ai && li { 3 } else { usize::from(ai || li) }
                    } else if avail_u || avail_l {
                        2 * usize::from(if avail_u { ai } else { li })
                    } else {
                        0
                    };
                    counted_bool(
                        d,
                        get(&self.probs.is_inter, ctx),
                        self.counts.is_inter.get_mut(ctx).unwrap_or(&mut [0; 2]),
                    )
                },
                |reference| reference != 0,
            );
        let max_tx = get(&MAX_TX_SIZE, size.into());
        let tx_size = if self.tx_mode == TxMode::Select && size >= BLOCK_8X8 && (!skip || !is_inter)
        {
            let mut a = above.filter(|m| !m.skip).map_or(max_tx, |m| m.tx_size);
            let mut l = left.filter(|m| !m.skip).map_or(max_tx, |m| m.tx_size);
            if !avail_l {
                l = a;
            }
            if !avail_u {
                a = l;
            }
            let ctx = usize::from(a + l > max_tx);
            let probs = get(&get(&self.probs.tx, max_tx.into()), ctx);
            let tree: &[i8] = match max_tx {
                3 => &TX_SIZE_32_TREE,
                2 => &TX_SIZE_16_TREE,
                _ => &TX_SIZE_8_TREE,
            };
            let mut unused = [[0; 2]; 3];
            let counts = self
                .counts
                .tx
                .get_mut(usize::from(max_tx))
                .and_then(|c| c.get_mut(ctx))
                .unwrap_or(&mut unused);
            counted_tree(d, tree, &probs, counts)
        } else {
            max_tx.min(self.tx_mode.largest_tx_size() as u8)
        };
        let mut info = ModeInfo {
            skip,
            tx_size,
            size,
            segment_id,
            ref_frames: [0, -1],
            filter: 3,
            ..ModeInfo::default()
        };
        let uv_mode = if is_inter {
            self.inter_mode(d, r, c, &mut info, above, left);
            0
        } else {
            self.intra_modes(d, &mut info, above, left)
        };
        let block = Block {
            row: r,
            col: c,
            info,
            uv_mode,
            avail_u,
            avail_l,
        };
        let any_tokens = self.residual(d, &block)?;
        if is_inter && size >= BLOCK_8X8 && !any_tokens {
            info.skip = true;
        }
        let rows = u32::from(get(&NUM_8X8_HIGH, size.into()));
        let cols = u32::from(get(&NUM_8X8_WIDE, size.into()));
        for y in r..(r + rows).min(self.mi_rows) {
            for x in c..(c + cols).min(self.mi_cols) {
                if let Some(m) = self
                    .mi
                    .get_mut(y as usize * self.mi_cols as usize + x as usize)
                {
                    *m = info;
                }
            }
        }
        Ok(())
    }

    fn intra_modes(
        &mut self,
        d: &mut BoolDecoder<'_>,
        info: &mut ModeInfo,
        above: Option<ModeInfo>,
        left: Option<ModeInfo>,
    ) -> u8 {
        let (w, h) = if info.size >= BLOCK_8X8 {
            (2, 2)
        } else {
            (
                usize::from(get(&NUM_4X4_WIDE, info.size.into())),
                usize::from(get(&NUM_4X4_HIGH, info.size.into())),
            )
        };
        let group = usize::from(get(&SIZE_GROUP, info.size.into()));
        for idy in (0..2).step_by(h) {
            for idx in (0..2).step_by(w) {
                let mode = if self.h.is_intra() {
                    let above_mode = if idy > 0 {
                        get(&info.sub_modes, idx)
                    } else {
                        above.map_or(DC_PRED, |m| get(&m.sub_modes, 2 + idx))
                    };
                    let left_mode = if idx > 0 {
                        get(&info.sub_modes, idy * 2)
                    } else {
                        left.map_or(DC_PRED, |m| get(&m.sub_modes, 1 + idy * 2))
                    };
                    let probs = get(&get(&KF_Y_MODE_PROBS, above_mode.into()), left_mode.into());
                    read_tree(d, &INTRA_MODE_TREE, |node| get(&probs, node))
                } else {
                    counted_tree(
                        d,
                        &INTRA_MODE_TREE,
                        &get(&self.probs.y_mode, group),
                        self.counts
                            .y_mode
                            .get_mut(group)
                            .unwrap_or(&mut [[0; 2]; 9]),
                    )
                };
                info.mode = mode;
                for y in 0..h {
                    for x in 0..w {
                        if let Some(m) = info.sub_modes.get_mut((idy + y) * 2 + idx + x) {
                            *m = mode;
                        }
                    }
                }
            }
        }
        if self.h.is_intra() {
            let probs = get(&KF_UV_MODE_PROBS, info.mode.into());
            read_tree(d, &INTRA_MODE_TREE, |node| get(&probs, node))
        } else {
            counted_tree(
                d,
                &INTRA_MODE_TREE,
                &get(&self.probs.uv_mode, info.mode.into()),
                self.counts
                    .uv_mode
                    .get_mut(usize::from(info.mode))
                    .unwrap_or(&mut [[0; 2]; 9]),
            )
        }
    }

    fn inter_mode(
        &mut self,
        d: &mut BoolDecoder<'_>,
        row: u32,
        col: u32,
        info: &mut ModeInfo,
        above: Option<ModeInfo>,
        left: Option<ModeInfo>,
    ) {
        let layout = crate::refs::References::new(self.h.ref_frame_sign_bias);
        let above_refs = above.map(|m| m.ref_frames);
        let left_refs = left.map(|m| m.ref_frames);
        let forced_ref = self
            .h
            .segmentation
            .features
            .get(usize::from(info.segment_id))
            .and_then(|f| f.get(SEG_LVL_REF_FRAME))
            .copied()
            .flatten()
            .filter(|_| self.h.segmentation.enabled);
        let compound = forced_ref.is_none()
            && match self.context.reference_mode {
                ReferenceMode::Compound => true,
                ReferenceMode::Single => false,
                ReferenceMode::Select => {
                    let ctx = layout.mode_context(above_refs, left_refs);
                    counted_bool(
                        d,
                        get(&self.probs.comp_mode, ctx),
                        self.counts.comp_mode.get_mut(ctx).unwrap_or(&mut [0; 2]),
                    )
                }
            };
        info.ref_frames = if let Some(reference) = forced_ref {
            [reference as i8, -1]
        } else if compound {
            let ctx = layout.compound_context(above_refs, left_refs);
            let choice = counted_bool(
                d,
                get(&self.probs.comp_ref, ctx),
                self.counts.comp_ref.get_mut(ctx).unwrap_or(&mut [0; 2]),
            );
            layout.pair(choice)
        } else {
            let [first, second] = crate::refs::single_contexts(above_refs, left_refs);
            let mut read = |ctx, node| {
                counted_bool(
                    d,
                    get(&get(&self.probs.single_ref, ctx), node),
                    self.counts
                        .single_ref
                        .get_mut(ctx)
                        .and_then(|p| p.get_mut(node))
                        .unwrap_or(&mut [0; 2]),
                )
            };
            let reference = if !read(first, 0) {
                1
            } else if read(second, 1) {
                3
            } else {
                2
            };
            [reference, -1]
        };
        let search = Search {
            modes: &self.mi,
            previous: self.context.previous,
            mi_rows: self.mi_rows,
            mi_cols: self.mi_cols,
            col_start: self.col_start,
            col_end: self.col_end,
            header: self.h,
        };
        let candidates = search.candidates(row, col, info.size, info.ref_frames[0], None);
        let mut best = [
            search.best(row, col, info.size, candidates.vectors),
            [[0; 2]; 2],
        ];
        if compound {
            let second = search.candidates(row, col, info.size, info.ref_frames[1], None);
            best[1] = search.best(row, col, info.size, second.vectors);
        }
        let force_zero = self.h.segmentation.enabled
            && self
                .h
                .segmentation
                .features
                .get(usize::from(info.segment_id))
                .and_then(|f| f.get(SEG_LVL_SKIP))
                .is_some_and(Option::is_some);
        if force_zero {
            info.mode = 12;
        } else if info.size >= BLOCK_8X8 {
            info.mode = 10
                + counted_tree(
                    d,
                    &INTER_MODE_TREE,
                    &get(&self.probs.inter_mode, candidates.context),
                    self.counts
                        .inter_mode
                        .get_mut(candidates.context)
                        .unwrap_or(&mut [[0; 2]; 3]),
                );
        }
        info.filter = match self.h.interpolation_filter {
            InterpolationFilter::Switchable => {
                let a = above
                    .filter(|m| m.ref_frames[0] > 0)
                    .map_or(3, |m| m.filter);
                let l = left.filter(|m| m.ref_frames[0] > 0).map_or(3, |m| m.filter);
                let ctx = usize::from(if a == l {
                    a
                } else if a == 3 {
                    l
                } else if l == 3 {
                    a
                } else {
                    3
                });
                counted_tree(
                    d,
                    &INTERP_FILTER_TREE,
                    &get(&self.probs.interp_filter, ctx),
                    self.counts
                        .interp_filter
                        .get_mut(ctx)
                        .unwrap_or(&mut [[0; 2]; 2]),
                )
            }
            InterpolationFilter::EightTap => 0,
            InterpolationFilter::EightTapSmooth => 1,
            InterpolationFilter::EightTapSharp => 2,
            InterpolationFilter::Bilinear => 3,
        };
        let (w, h) = if info.size >= BLOCK_8X8 {
            (2, 2)
        } else {
            (
                usize::from(get(&NUM_4X4_WIDE, info.size.into())),
                usize::from(get(&NUM_4X4_HIGH, info.size.into())),
            )
        };
        for y in (0..2).step_by(h) {
            for x in (0..2).step_by(w) {
                let block = y * 2 + x;
                if info.size < BLOCK_8X8 {
                    info.mode = 10
                        + counted_tree(
                            d,
                            &INTER_MODE_TREE,
                            &get(&self.probs.inter_mode, candidates.context),
                            self.counts
                                .inter_mode
                                .get_mut(candidates.context)
                                .unwrap_or(&mut [[0; 2]; 3]),
                        );
                }
                for (list, &reference_best) in
                    best.iter().enumerate().take(1 + usize::from(compound))
                {
                    let mut selected = reference_best;
                    if info.size < BLOCK_8X8 && (info.mode == 10 || info.mode == 11) {
                        let raw = search
                            .candidates(
                                row,
                                col,
                                info.size,
                                get(&info.ref_frames, list),
                                Some(block),
                            )
                            .vectors;
                        selected = mv::append_sub8x8(raw, get(&info.sub_mvs, list), block);
                    }
                    let vector = match info.mode {
                        10 => selected[0],
                        11 => selected[1],
                        12 => [0; 2],
                        _ => read_mv(
                            d,
                            self.probs,
                            &mut self.counts,
                            reference_best[0],
                            self.h.allow_high_precision_mv,
                        ),
                    };
                    for dy in 0..h {
                        for dx in 0..w {
                            if let Some(m) = info
                                .sub_mvs
                                .get_mut(list)
                                .and_then(|v| v.get_mut((y + dy) * 2 + x + dx))
                            {
                                *m = vector;
                            }
                        }
                    }
                }
            }
        }
    }

    /// `residual` (clause 6.4.21): prediction, the tokens of each
    /// transform block, and the nonzero contexts.
    fn residual(&mut self, d: &mut BoolDecoder<'_>, b: &Block) -> Result<bool> {
        let bsize = b.info.size.max(BLOCK_8X8);
        let color = self.h.color;
        let is_inter = b.info.ref_frames[0] > 0;
        let mut any_tokens = false;
        for plane in 0..3 {
            let (sub_x, sub_y) = if plane > 0 {
                (color.subsampling_x, color.subsampling_y)
            } else {
                (false, false)
            };
            let tx_size = if plane == 0 {
                b.info.tx_size
            } else {
                uv_tx_size(b.info.size, b.info.tx_size, (sub_x, sub_y))?
            };
            let size = plane_block_size(bsize, (sub_x, sub_y))?;
            let step = 1u32 << tx_size;
            let w = u32::from(get(&NUM_4X4_WIDE, size.into()));
            let h = u32::from(get(&NUM_4X4_HIGH, size.into()));
            let base_x = (b.col * 8) >> u32::from(sub_x);
            let base_y = (b.row * 8) >> u32::from(sub_y);
            let max_x = (self.mi_cols * 8) >> u32::from(sub_x);
            let max_y = (self.mi_rows * 8) >> u32::from(sub_y);
            if is_inter {
                if b.info.size < BLOCK_8X8 {
                    for y in 0..h {
                        for x in 0..w {
                            self.inter_prediction(
                                b,
                                plane,
                                (base_x + 4 * x, base_y + 4 * y, 4, 4),
                                (y * w + x) as usize,
                            )?;
                        }
                    }
                } else {
                    self.inter_prediction(b, plane, (base_x, base_y, w * 4, h * 4), 0)?;
                }
            }
            let mut block_idx = 0;
            for y in (0..h).step_by(step as usize) {
                for x in (0..w).step_by(step as usize) {
                    let start_x = base_x + 4 * x;
                    let start_y = base_y + 4 * y;
                    let mut nonzero = false;
                    if start_x < max_x && start_y < max_y {
                        let mode = if plane > 0 {
                            b.uv_mode
                        } else if b.info.size >= BLOCK_8X8 {
                            b.info.sub_modes[3]
                        } else {
                            get(&b.info.sub_modes, block_idx)
                        };
                        let have = (b.avail_l || x > 0, b.avail_u || y > 0, x + step < w);
                        if !is_inter {
                            self.predict(
                                plane,
                                (start_x, start_y),
                                tx_size,
                                mode,
                                have,
                                (max_x, max_y),
                            );
                        }
                        if !b.info.skip {
                            let tx_type = self.tx_type(plane, tx_size, b, block_idx);
                            nonzero = self.tokens(
                                d,
                                plane,
                                (start_x, start_y),
                                tx_size,
                                tx_type,
                                is_inter,
                            )?;
                            any_tokens |= nonzero;
                            if nonzero {
                                self.reconstruct(
                                    plane,
                                    start_x,
                                    start_y,
                                    tx_size,
                                    tx_type,
                                    b.info.segment_id,
                                )?;
                            }
                        }
                    }
                    for i in 0..step {
                        if let Some(a) = self
                            .above_nonzero
                            .get_mut(plane)
                            .and_then(|a| a.get_mut(((start_x >> 2) + i) as usize))
                        {
                            *a = nonzero;
                        }
                        if let Some(l) = self
                            .left_nonzero
                            .get_mut(plane)
                            .and_then(|l| l.get_mut((((start_y >> 2) + i) & 15) as usize))
                        {
                            *l = nonzero;
                        }
                    }
                    block_idx += 1;
                }
            }
        }
        Ok(any_tokens)
    }

    fn inter_prediction(
        &mut self,
        b: &Block,
        plane: usize,
        region: (u32, u32, u32, u32),
        block: usize,
    ) -> Result<()> {
        for list in 0..1 + usize::from(b.info.ref_frames[1] > 0) {
            self.reference_prediction(b, plane, region, block, list)?;
        }
        Ok(())
    }

    fn reference_prediction(
        &mut self,
        b: &Block,
        plane: usize,
        region: (u32, u32, u32, u32),
        block: usize,
        list: usize,
    ) -> Result<()> {
        let (x, y, w, h) = region;
        let (sx, sy) = if plane == 0 {
            (0, 0)
        } else {
            (
                u32::from(self.h.color.subsampling_x),
                u32::from(self.h.color.subsampling_y),
            )
        };
        let vectors = get(&b.info.sub_mvs, list);
        let average = |sum: i32, divisor: i32| {
            (sum + if sum < 0 { -(divisor / 2) } else { divisor / 2 }) / divisor
        };
        let mut vector = get(&vectors, block);
        if plane > 0 && b.info.size < BLOCK_8X8 {
            for comp in 0..2 {
                let at = |i| get(&get(&vectors, i), comp);
                let value = match (sx, sy) {
                    (0, 1) => average(at(block) + at(block + 2), 2),
                    (1, 0) => average(at(block) + at(block + 1), 2),
                    (1, 1) => average(at(0) + at(1) + at(2) + at(3), 4),
                    _ => at(block),
                };
                if let Some(v) = vector.get_mut(comp) {
                    *v = value;
                }
            }
        }
        let bw = i64::from(get(&NUM_8X8_WIDE, b.info.size.into()));
        let bh = i64::from(get(&NUM_8X8_HIGH, b.info.size.into()));
        let clamp = |mv: i32, at: u32, extent: u32, blocks: i64, sub: u32| {
            let before = -(i64::from(at) * 128) >> sub;
            let after = ((i64::from(extent) - blocks - i64::from(at)) * 128) >> sub;
            let extra = (4 + ((blocks * 8) >> sub)) << 4;
            ((i64::from(mv) * 2) >> sub).clamp(before - extra, after + extra - 16)
        };
        let row_mv = clamp(vector[0], b.row, self.mi_rows, bh, sy);
        let col_mv = clamp(vector[1], b.col, self.mi_cols, bw, sx);
        let slot = get(
            &self.h.ref_frame_idx,
            usize::try_from(get(&b.info.ref_frames, list) - 1).unwrap_or(0),
        );
        let reference = self
            .context
            .references
            .get(usize::from(slot))
            .and_then(|p| *p)
            .ok_or(Error::Invalid("an unavailable reference picture"))?;
        let fw = i64::from(self.h.width);
        let fh = i64::from(self.h.height);
        let rw = i64::from(reference.width);
        let rh = i64::from(reference.height);
        if rw > 2 * fw || rh > 2 * fh || fw > 16 * rw || fh > 16 * rh {
            return Err(Error::Invalid("reference scaling dimensions"));
        }
        let scale_x = (rw << 14) / fw;
        let scale_y = (rh << 14) / fh;
        let scaled = |pos: u32, sub: u32, scale: i64, mv: i64| {
            let base = (i64::from(pos) * scale) >> 14;
            let frac = ((16 * (i64::from(pos) << sub) * scale) >> 14) & 15;
            i32::try_from((base << 4) + ((mv * scale) >> 14) + frac).ok()
        };
        let sampling = scaled(x, sx, scale_x, col_mv)
            .zip(scaled(y, sy, scale_y, row_mv))
            .and_then(|(x, y)| {
                Sampling::new(
                    x,
                    y,
                    ((16 * scale_x) >> 14) as u8,
                    ((16 * scale_y) >> 14) as u8,
                )
            })
            .ok_or(Error::Invalid("inter prediction sampling"))?;
        let width = (reference.width + sx) >> sx;
        let height = (reference.height + sy) >> sy;
        let source = reference
            .planes
            .get(plane)
            .and_then(P::plane)
            .and_then(|s| ReferencePlane::new(s, width as usize, width, height))
            .ok_or(Error::Invalid("a reference plane"))?;
        let bit_depth =
            BitDepth::from_bits(self.h.color.bit_depth).ok_or(Error::Invalid("BitDepth"))?;
        let dst = self
            .frame
            .planes
            .get_mut(plane)
            .ok_or(Error::Invalid("a prediction plane"))?;
        let block = PredictionMut::new(
            &mut dst.samples,
            y as usize * dst.stride + x as usize,
            dst.stride,
            w as u8,
            h as u8,
        )
        .ok_or(Error::Invalid("an inter prediction region"))?;
        let filter = match b.info.filter {
            0 => InterpFilter::EightTap,
            1 => InterpFilter::EightTapSmooth,
            2 => InterpFilter::EightTapSharp,
            _ => InterpFilter::Bilinear,
        };
        predict_inter(block, source, sampling, filter, bit_depth, list != 0);
        Ok(())
    }

    /// The intra prediction process (clause 8.5.1) of the transform block at
    /// `(x, y)` of `plane`: gathers its edges from the frame and predicts
    /// it. `have` is (`haveLeft`, `haveAbove`, `notOnRight`), `end` the
    /// plane's width and height in whole 8x8 blocks (`maxX + 1`, `maxY + 1`).
    fn predict(
        &mut self,
        plane: usize,
        (x, y): (u32, u32),
        tx_size: u8,
        mode: u8,
        have: (bool, bool, bool),
        end: (u32, u32),
    ) {
        let (have_left, have_above, not_on_right) = have;
        let Some(bit_depth) = BitDepth::from_bits(self.h.color.bit_depth) else {
            return;
        };
        let size = 4usize << tx_size;
        let base = 1i64 << (bit_depth.bits() - 1);
        let Some(p) = self.frame.planes.get_mut(plane) else {
            return;
        };
        let stride = p.stride;
        let (x, y) = (x as usize, y as usize);
        let (max_x, max_y) = (
            end.0.saturating_sub(1) as usize,
            end.1.saturating_sub(1) as usize,
        );
        let at = |row: usize, col: usize| {
            p.samples
                .get(row * stride + col)
                .copied()
                .unwrap_or_default()
        };
        let fill = |v: i64| P::clip(v, bit_depth);
        let mut edges = Edges::<P> {
            have_above,
            have_left,
            ..Edges::default()
        };
        for i in 0..2 * size {
            let v = if !have_above {
                fill(base - 1)
            } else if i < size || (not_on_right && tx_size == 0) {
                at(y - 1, max_x.min(x + i))
            } else {
                edges.above.get(size).copied().unwrap_or_default()
            };
            if let Some(a) = edges.above.get_mut(i + 1) {
                *a = v;
            }
        }
        edges.above[0] = match (have_above, have_left) {
            (true, true) => at(y - 1, max_x.min(x - 1)),
            (true, false) => fill(base + 1),
            _ => fill(base - 1),
        };
        for i in 0..size {
            let v = if have_left {
                at(max_y.min(y + i), x - 1)
            } else {
                fill(base + 1)
            };
            if let Some(l) = edges.left.get_mut(i) {
                *l = v;
            }
        }
        let (Some(mode), Some(dst)) = (
            IntraMode::from_value(mode),
            TxSize::from_value(tx_size)
                .and_then(|t| BlockMut::new(&mut p.samples, y * stride + x, stride, t)),
        ) else {
            return;
        };
        predict_intra(dst, mode, &edges, bit_depth);
    }

    /// The reconstruct process (clause 8.6.2): dequantizes the transform
    /// block just read and adds its inverse transform at (`x`, `y`). A
    /// dequantized coefficient outside `8 + BitDepth` bits breaks the
    /// clause's requirement, which the transforms rely on, and is clamped
    /// into that range (`docs/design/vp9.md`, "Specification and the clean
    /// room").
    fn reconstruct(
        &mut self,
        plane: usize,
        x: u32,
        y: u32,
        tx_size: u8,
        tx_type: u8,
        segment_id: u8,
    ) -> Result<()> {
        let h = self.h;
        let q = &h.quantization;
        let seg = &h.segmentation;
        let qindex = match seg
            .features
            .get(usize::from(segment_id))
            .and_then(|f| f.get(SEG_LVL_ALT_Q))
            .copied()
            .flatten()
        {
            Some(data) if seg.enabled => {
                let data = if seg.abs_or_delta_update {
                    i32::from(data)
                } else {
                    i32::from(q.base_q_idx) + i32::from(data)
                };
                data.clamp(0, 255)
            }
            _ => i32::from(q.base_q_idx),
        };
        let bit_depth = BitDepth::from_bits(h.color.bit_depth).ok_or(Error::Invalid("BitDepth"))?;
        let depth = match bit_depth {
            BitDepth::Eight => 0,
            BitDepth::Ten => 1,
            BitDepth::Twelve => 2,
        };
        let lookup = |table: &[[u16; 256]; 3], b: i32| {
            i64::from(
                table
                    .get(depth)
                    .and_then(|t| t.get(b.clamp(0, 255) as usize))
                    .copied()
                    .unwrap_or(0),
            )
        };
        let (dc_delta, ac_delta) = if plane == 0 {
            (q.delta_q_y_dc, 0)
        } else {
            (q.delta_q_uv_dc, q.delta_q_uv_ac)
        };
        let dc = lookup(&DC_QLOOKUP, qindex + i32::from(dc_delta));
        let ac = lookup(&AC_QLOOKUP, qindex + i32::from(ac_delta));
        let denom = if tx_size == 3 { 2 } else { 1 };
        let n0 = 4usize << tx_size;
        let Some(coefs) = self.coefs.get_mut(..n0 * n0) else {
            return Ok(());
        };
        for (i, c) in coefs.iter_mut().enumerate() {
            *c = *c * if i == 0 { dc } else { ac } / denom;
        }
        let tx_type = match tx_type {
            ADST_DCT => TxType::AdstDct,
            DCT_ADST => TxType::DctAdst,
            DCT_DCT => TxType::DctDct,
            _ => TxType::AdstAdst,
        };
        let coefs = Coefficients::clamped(coefs, bit_depth);
        let Some(p) = self.frame.planes.get_mut(plane) else {
            return Ok(());
        };
        let stride = p.stride;
        if let Some(dst) = TxSize::from_value(tx_size).and_then(|t| {
            BlockMut::new(&mut p.samples, y as usize * stride + x as usize, stride, t)
        }) {
            inverse_transform_add(dst, coefs, tx_type, q.lossless());
        }
        Ok(())
    }

    /// `TxType` of `get_scan` (clause 6.4.25) for an intra block.
    fn tx_type(&self, plane: usize, tx_size: u8, b: &Block, block_idx: usize) -> u8 {
        if b.info.ref_frames[0] > 0 || plane > 0 || tx_size == 3 {
            return DCT_DCT;
        }
        let mode = if tx_size == 0 {
            if self.h.quantization.lossless() {
                return DCT_DCT;
            }
            if b.info.size < BLOCK_8X8 {
                get(&b.info.sub_modes, block_idx)
            } else {
                b.info.sub_modes[3]
            }
        } else {
            b.info.sub_modes[3]
        };
        get(&MODE2TXFM_MAP, mode.into())
    }

    /// `tokens` (clause 6.4.24): reads one transform block's tokens and
    /// returns whether it has any (`nonzero`). The coefficients' values are
    /// read and dropped until reconstruction uses them.
    fn tokens(
        &mut self,
        d: &mut BoolDecoder<'_>,
        plane: usize,
        start: (u32, u32),
        tx_size: u8,
        tx_type: u8,
        is_inter: bool,
    ) -> Result<bool> {
        let (start_x, start_y) = start;
        let seg_eob = 16usize << (tx_size << 1);
        let scan: &dyn Fn(usize) -> usize = match (tx_size, tx_type) {
            (0, ADST_DCT) => &|c| get(&ROW_SCAN_4X4, c).into(),
            (0, DCT_ADST) => &|c| get(&COL_SCAN_4X4, c).into(),
            (0, _) => &|c| get(&DEFAULT_SCAN_4X4, c).into(),
            (1, ADST_DCT) => &|c| get(&ROW_SCAN_8X8, c).into(),
            (1, DCT_ADST) => &|c| get(&COL_SCAN_8X8, c).into(),
            (1, _) => &|c| get(&DEFAULT_SCAN_8X8, c).into(),
            (2, ADST_DCT) => &|c| get(&ROW_SCAN_16X16, c).into(),
            (2, DCT_ADST) => &|c| get(&COL_SCAN_16X16, c).into(),
            (2, _) => &|c| get(&DEFAULT_SCAN_16X16, c).into(),
            _ => &|c| get(&DEFAULT_SCAN_32X32, c).into(),
        };
        let (sub_x, sub_y) = if plane > 0 {
            (self.h.color.subsampling_x, self.h.color.subsampling_y)
        } else {
            (false, false)
        };
        let max_x = (2 * self.mi_cols) >> u32::from(sub_x);
        let max_y = (2 * self.mi_rows) >> u32::from(sub_y);
        let (x4, y4) = (start_x >> 2, start_y >> 2);
        let (mut above, mut left) = (false, false);
        for i in 0..1u32 << tx_size {
            if x4 + i < max_x {
                above |= self
                    .above_nonzero
                    .get(plane)
                    .is_some_and(|a| get(a, (x4 + i) as usize));
            }
            if y4 + i < max_y {
                left |= self
                    .left_nonzero
                    .get(plane)
                    .is_some_and(|l| get(l, ((y4 + i) & 15) as usize));
            }
        }
        let first_ctx = usize::from(above) + usize::from(left);
        let n = 4usize << tx_size;
        let probs = get(
            &get(&self.probs.coef, tx_size.into()),
            usize::from(plane > 0),
        );
        let probs = get(&probs, usize::from(is_inter));
        let bit_depth = u32::from(self.h.color.bit_depth);
        self.coefs.iter_mut().take(n * n).for_each(|c| *c = 0);
        let mut check_eob = true;
        let mut c = 0;
        while c < seg_eob {
            let pos = scan(c);
            let band = if tx_size == 0 {
                get(&COEFBAND_4X4, c)
            } else {
                get(&COEFBAND_8X8PLUS, c)
            };
            let ctx = if c == 0 {
                first_ctx
            } else {
                let (i, j) = (pos / n, pos % n);
                let (a, b) = if i > 0 && j > 0 {
                    let up = (i - 1) * n + j;
                    let left = i * n + j - 1;
                    match tx_type {
                        DCT_ADST => (up, up),
                        ADST_DCT => (left, left),
                        _ => (up, left),
                    }
                } else if i > 0 {
                    ((i - 1) * n + j, (i - 1) * n + j)
                } else {
                    (j.saturating_sub(1), j.saturating_sub(1))
                };
                (1 + usize::from(get(&self.token_cache, a))
                    + usize::from(get(&self.token_cache, b)))
                    >> 1
            };
            let p = get(&get(&probs, band.into()), ctx);
            let mut unused = [[0; 2]; 3];
            let counts = self
                .counts
                .coef
                .get_mut(usize::from(tx_size))
                .and_then(|p| p.get_mut(usize::from(plane > 0)))
                .and_then(|p| p.get_mut(usize::from(is_inter)))
                .and_then(|p| p.get_mut(usize::from(band)))
                .and_then(|p| p.get_mut(ctx))
                .unwrap_or(&mut unused);
            if check_eob && !counted_bool(d, get(&p, 0), &mut counts[0]) {
                break;
            }
            let token = read_tree(d, &TOKEN_TREE, |node| {
                pareto(node, get(&p, 2.min(1 + node)))
            });
            if let Some(c) = counts
                .get_mut(1)
                .and_then(|c| c.get_mut(usize::from(token != 0)))
            {
                *c = c.saturating_add(1);
            }
            if token != 0 {
                if let Some(c) = counts
                    .get_mut(2)
                    .and_then(|c| c.get_mut(usize::from(token >= 2)))
                {
                    *c = c.saturating_add(1);
                }
            }
            if let Some(t) = self.token_cache.get_mut(pos) {
                *t = get(&ENERGY_CLASS, token.into());
            }
            if token == ZERO_TOKEN {
                check_eob = false;
            } else {
                check_eob = true;
                let [cat, extra, base] = get(&EXTRA_BITS, token.into());
                let mut coef = i64::from(base);
                if token == DCT_VAL_CATEGORY6 {
                    for e in 0..bit_depth - 8 {
                        coef += i64::from(d.read(255)) << (5 + bit_depth - e);
                    }
                }
                let cat_probs = get(&CAT_PROBS, cat.into());
                for e in 0..extra {
                    coef += i64::from(d.read(get(&cat_probs, e.into()))) << (extra - 1 - e);
                }
                if d.read(128) {
                    coef = -coef;
                }
                if let Some(t) = self.coefs.get_mut(pos) {
                    *t = coef;
                }
            }
            c += 1;
        }
        Ok(c > 0)
    }
}

#[cfg(test)]
mod tests;
