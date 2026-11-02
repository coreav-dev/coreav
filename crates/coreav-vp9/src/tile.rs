//! Tile data (clause 6.4) of intra frames: the tiles, the partition walk,
//! each block's mode info, and its tokens, read with the trees and
//! probabilities of clause 9.3.

use coreav_kernels_vp9::{
    BitDepth, BlockMut, Coefficients, Edges, IntraMode, TxSize, TxType, inverse_transform_add,
    predict_intra,
};

use crate::bool::BoolDecoder;
use crate::compressed::TxMode;
use crate::error::{Error, Result};
use crate::header::FrameHeader;
use crate::picture::{Planes, Sample};
use crate::probs::Probs;
use crate::tables::{
    AC_QLOOKUP, B_HEIGHT_LOG2, B_WIDTH_LOG2, CAT_PROBS, COEFBAND_4X4, COEFBAND_8X8PLUS,
    COL_SCAN_4X4, COL_SCAN_8X8, COL_SCAN_16X16, COLS_PARTITION_TREE, DC_QLOOKUP, DEFAULT_SCAN_4X4,
    DEFAULT_SCAN_8X8, DEFAULT_SCAN_16X16, DEFAULT_SCAN_32X32, ENERGY_CLASS, EXTRA_BITS,
    INTRA_MODE_TREE, KF_PARTITION_PROBS, KF_UV_MODE_PROBS, KF_Y_MODE_PROBS, MAX_TX_SIZE,
    MI_WIDTH_LOG2, MODE2TXFM_MAP, NUM_4X4_HIGH, NUM_4X4_WIDE, NUM_8X8_HIGH, NUM_8X8_WIDE,
    PARETO_TABLE, PARTITION_TREE, ROW_SCAN_4X4, ROW_SCAN_8X8, ROW_SCAN_16X16, ROWS_PARTITION_TREE,
    SEGMENT_TREE, SS_SIZE, SUBSIZE, TOKEN_TREE, TX_SIZE_8_TREE, TX_SIZE_16_TREE, TX_SIZE_32_TREE,
};

const BLOCK_8X8: u8 = 3;
const BLOCK_64X64: u8 = 12;
const BLOCK_INVALID: u8 = 14;
const PARTITION_HORZ: u8 = 1;
const PARTITION_VERT: u8 = 2;
const PARTITION_SPLIT: u8 = 3;
const DC_PRED: u8 = 0;
const SEG_LVL_ALT_Q: usize = 0;
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

/// What later blocks of a frame read of one 8x8 block (clause 6.4.4).
#[derive(Clone, Copy, Default)]
struct ModeInfo {
    sub_modes: [u8; 4],
    skip: bool,
    tx_size: u8,
}

/// A block's mode info as its residual reads it.
struct Block {
    row: u32,
    col: u32,
    size: u8,
    segment_id: u8,
    skip: bool,
    tx_size: u8,
    sub_modes: [u8; 4],
    uv_mode: u8,
    avail_u: bool,
    avail_l: bool,
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
    mi: Vec<ModeInfo>,
    above_partition: Vec<u8>,
    left_partition: [u8; 8],
    above_nonzero: [Vec<bool>; 3],
    left_nonzero: [[bool; 16]; 3],
    token_cache: Vec<u8>,
    /// `Tokens` of the transform block being read, in raster order.
    coefs: Vec<i64>,
}

/// Reads the tiles of an intra frame (`decode_tiles`, clause 6.4) from
/// `data`, the frame's bytes after its compressed header, with the
/// probabilities and transform mode of its compressed header. Returns
/// whether every tile's boolean decoder ended as clause 9.2.3 requires.
pub(crate) fn read<P: Sample>(
    data: &[u8],
    h: &FrameHeader,
    probs: &Probs,
    tx_mode: TxMode,
    frame: &mut Planes<P>,
) -> Result<bool> {
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
        mi: vec![ModeInfo::default(); mi_rows as usize * mi_cols as usize],
        above_partition: vec![0; sb_cols * 8],
        left_partition: [0; 8],
        above_nonzero: core::array::from_fn(|_| vec![false; sb_cols * 16]),
        left_nonzero: [[false; 16]; 3],
        token_cache: vec![0; 1024],
        coefs: vec![0; 1024],
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
            let mut d = BoolDecoder::new(tile);
            for r in (row_start..row_end).step_by(8) {
                t.left_partition = [0; 8];
                t.left_nonzero = [[false; 16]; 3];
                for c in (t.col_start..col_end).step_by(8) {
                    t.partition(&mut d, r, c, BLOCK_64X64)?;
                }
            }
            clean &= d.exit();
        }
    }
    Ok(clean)
}

impl<P: Sample> Tiles<'_, P> {
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
        let probs = get(&KF_PARTITION_PROBS, ctx);
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

    /// `decode_block` (clause 6.4.4) of an intra frame.
    fn block(&mut self, d: &mut BoolDecoder<'_>, r: u32, c: u32, size: u8) -> Result<()> {
        if size >= BLOCK_INVALID {
            return Err(Error::Invalid("a partition of an invalid block size"));
        }
        let avail_u = r > 0;
        let avail_l = c > self.col_start;
        let above = avail_u.then(|| self.mi_at(r - 1, c));
        let left = avail_l.then(|| self.mi_at(r, c - 1));
        let seg = &self.h.segmentation;
        let segment_id = if seg.enabled && seg.update_map {
            read_tree(d, &SEGMENT_TREE, |node| get(&seg.tree_probs, node))
        } else {
            0
        };
        let skip_active = seg.enabled
            && seg
                .features
                .get(usize::from(segment_id))
                .and_then(|f| f.get(SEG_LVL_SKIP))
                .is_some_and(Option::is_some);
        let skip = skip_active || {
            let ctx = usize::from(above.is_some_and(|m| m.skip))
                + usize::from(left.is_some_and(|m| m.skip));
            d.read(get(&self.probs.skip, ctx))
        };
        let max_tx = get(&MAX_TX_SIZE, size.into());
        let tx_size = if self.tx_mode == TxMode::Select && size >= BLOCK_8X8 {
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
            read_tree(d, tree, |node| get(&probs, node))
        } else {
            max_tx.min(self.tx_mode.largest_tx_size() as u8)
        };
        let mut sub_modes = [DC_PRED; 4];
        let y_probs = |above_mode: u8, left_mode: u8| {
            get(&get(&KF_Y_MODE_PROBS, above_mode.into()), left_mode.into())
        };
        let y_mode;
        if size >= BLOCK_8X8 {
            let above_mode = above.map_or(DC_PRED, |m| m.sub_modes[2]);
            let left_mode = left.map_or(DC_PRED, |m| m.sub_modes[1]);
            let probs = y_probs(above_mode, left_mode);
            y_mode = read_tree(d, &INTRA_MODE_TREE, |node| get(&probs, node));
            sub_modes = [y_mode; 4];
        } else {
            let w = usize::from(get(&NUM_4X4_WIDE, size.into()));
            let h = usize::from(get(&NUM_4X4_HIGH, size.into()));
            let mut mode = DC_PRED;
            for idy in (0..2).step_by(h.max(1)) {
                for idx in (0..2).step_by(w.max(1)) {
                    let above_mode = if idy > 0 {
                        get(&sub_modes, idx)
                    } else {
                        above.map_or(DC_PRED, |m| get(&m.sub_modes, 2 + idx))
                    };
                    let left_mode = if idx > 0 {
                        get(&sub_modes, idy * 2)
                    } else {
                        left.map_or(DC_PRED, |m| get(&m.sub_modes, 1 + idy * 2))
                    };
                    let probs = y_probs(above_mode, left_mode);
                    mode = read_tree(d, &INTRA_MODE_TREE, |node| get(&probs, node));
                    for y2 in 0..h {
                        for x2 in 0..w {
                            if let Some(m) = sub_modes.get_mut((idy + y2) * 2 + idx + x2) {
                                *m = mode;
                            }
                        }
                    }
                }
            }
            y_mode = mode;
        }
        let uv_probs = get(&KF_UV_MODE_PROBS, y_mode.into());
        let uv_mode = read_tree(d, &INTRA_MODE_TREE, |node| get(&uv_probs, node));
        let block = Block {
            row: r,
            col: c,
            size,
            segment_id,
            skip,
            tx_size,
            sub_modes,
            uv_mode,
            avail_u,
            avail_l,
        };
        self.residual(d, &block)?;
        let info = ModeInfo {
            sub_modes,
            skip,
            tx_size,
        };
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

    /// `residual` (clause 6.4.21) of an intra block: the tokens of each
    /// transform block, and the nonzero contexts.
    fn residual(&mut self, d: &mut BoolDecoder<'_>, b: &Block) -> Result<()> {
        let bsize = b.size.max(BLOCK_8X8);
        let color = self.h.color;
        for plane in 0..3 {
            let (sub_x, sub_y) = if plane > 0 {
                (color.subsampling_x, color.subsampling_y)
            } else {
                (false, false)
            };
            let plane_size = |size: u8| {
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
            };
            let tx_size = if plane == 0 {
                b.tx_size
            } else if b.size < BLOCK_8X8 {
                0
            } else {
                b.tx_size.min(get(&MAX_TX_SIZE, plane_size(b.size)?.into()))
            };
            let size = plane_size(bsize)?;
            let step = 1u32 << tx_size;
            let w = u32::from(get(&NUM_4X4_WIDE, size.into()));
            let h = u32::from(get(&NUM_4X4_HIGH, size.into()));
            let base_x = (b.col * 8) >> u32::from(sub_x);
            let base_y = (b.row * 8) >> u32::from(sub_y);
            let max_x = (self.mi_cols * 8) >> u32::from(sub_x);
            let max_y = (self.mi_rows * 8) >> u32::from(sub_y);
            let mut block_idx = 0;
            for y in (0..h).step_by(step as usize) {
                for x in (0..w).step_by(step as usize) {
                    let start_x = base_x + 4 * x;
                    let start_y = base_y + 4 * y;
                    let mut nonzero = false;
                    if start_x < max_x && start_y < max_y {
                        let mode = if plane > 0 {
                            b.uv_mode
                        } else if b.size >= BLOCK_8X8 {
                            b.sub_modes[3]
                        } else {
                            get(&b.sub_modes, block_idx)
                        };
                        let have = (b.avail_l || x > 0, b.avail_u || y > 0, x + step < w);
                        self.predict(
                            plane,
                            (start_x, start_y),
                            tx_size,
                            mode,
                            have,
                            (max_x, max_y),
                        );
                        if !b.skip {
                            let tx_type = self.tx_type(plane, tx_size, b, block_idx);
                            nonzero = self.tokens(d, plane, start_x, start_y, tx_size, tx_type)?;
                            if nonzero {
                                self.reconstruct(
                                    plane,
                                    start_x,
                                    start_y,
                                    tx_size,
                                    tx_type,
                                    b.segment_id,
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
    /// clause's requirement, which the transforms rely on, and is refused.
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
        let coefs = Coefficients::new(coefs, bit_depth).ok_or(Error::Invalid(
            "a dequantized coefficient outside 8 + BitDepth bits (clause 8.6.2)",
        ))?;
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
        if plane > 0 || tx_size == 3 {
            return DCT_DCT;
        }
        let mode = if tx_size == 0 {
            if self.h.quantization.lossless() {
                return DCT_DCT;
            }
            if b.size < BLOCK_8X8 {
                get(&b.sub_modes, block_idx)
            } else {
                b.sub_modes[3]
            }
        } else {
            b.sub_modes[3]
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
        start_x: u32,
        start_y: u32,
        tx_size: u8,
        tx_type: u8,
    ) -> Result<bool> {
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
        let probs = get(&probs, 0);
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
            if check_eob && !d.read(get(&p, 0)) {
                break;
            }
            let token = read_tree(d, &TOKEN_TREE, |node| {
                pareto(node, get(&p, 2.min(1 + node)))
            });
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
