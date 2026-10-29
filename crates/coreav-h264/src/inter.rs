//! Inter prediction: motion vector prediction (clause 8.4.1), the direct
//! modes (clause 8.4.1.2), sample interpolation (clause 8.4.2.2) and
//! weighted prediction (clause 8.4.2.3).
#![forbid(unsafe_code)]

use coreav_kernels_h264::{
    Block, BlockMut, Combine, Kernels, Ops, Part, PlaneOp, Planes, Weighting,
};

use crate::dpb::RefPic;
use crate::error::{Error, Result};
use crate::mb::{
    BAND_VIEWS, BandView, MAX_REFS, MbKind, SliceDecoder, part_xy, sub_part_xy, sub_shape,
};
use crate::params::Pps;
use crate::picture::{MbHead, MbMotion, Motion, NO_MOTION, NO_SLOT};
use crate::slice::SliceType;
use crate::tables::{self as t, PredMode};
use crate::tier::{Base, Tier};

mod p_cavlc;
pub(crate) use p_cavlc::{PPred, PShape};

/// Where [`SliceDecoder::predict_list`] puts a list's prediction.
///
/// Two words at most, since the hot path takes it by value: the weighted
/// operations stay with the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target<'o> {
    /// The list's scratch prediction, for weighted prediction to combine.
    Scratch,
    /// The picture: one list with the default weighting, or list 0 of two
    /// that are averaged.
    Picture,
    /// Averaged into the picture, which holds list 0's prediction: list 1
    /// of two with the default weighting (Equation 8-273).
    Average,
    /// The picture, each plane (luma, Cb, Cr) as its operation says:
    /// weighted prediction with the weighting done by the interpolation
    /// (`Kernels::luma_mc_weighted`), list 1 of two over list 0's
    /// prediction, or the only list.
    Weighted(&'o [Op; 3]),
}

/// What [`Target::Weighted`] does with one plane's prediction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    /// Writes it: the only list, with the default weights in this plane.
    Put,
    /// Averages it into the picture (Equation 8-273): list 1 of two, both
    /// with the default weights in this plane.
    Average,
    /// Weights it into the picture (Equations 8-286 to 8-288).
    Weigh(Weighting),
}

/// The operation for each plane of a block whose planes `combine` would
/// weight as `modes` say, for [`Target::Weighted`]: the second of two
/// lists, or the only one. `None` where a weighting lies outside what the
/// weighted kernels take ([`Weighting::in_range`]), which leaves the block
/// to the predictions in scratch and `combine`.
///
/// `T`, unused, gives each instance of the hot paths its own copy
/// ([`crate::tier`]).
fn fused_ops<T: Tier>(modes: [Combine; 3], used: [bool; 2]) -> Option<[Op; 3]> {
    let mut ops = [Op::Put; 3];
    for (op, mode) in ops.iter_mut().zip(modes) {
        *op = match (used, mode) {
            ([true, true], Combine::Average) => Op::Average,
            ([true, true], Combine::Bi { log_wd, w0, w1, o }) => {
                Op::Weigh(Weighting::Bi { log_wd, w0, w1, o })
            }
            ([true, false] | [false, true], Combine::Copy(_)) => Op::Put,
            ([true, false] | [false, true], Combine::Uni { log_wd, w, o, .. }) => {
                Op::Weigh(Weighting::Uni { log_wd, w, o })
            }
            _ => return None,
        };
        if let Op::Weigh(weighting) = *op {
            if !weighting.in_range() {
                return None;
            }
        }
    }
    Some(ops)
}

impl Op {
    /// The kernels' form of the operation.
    const fn plane_op(self) -> PlaneOp {
        match self {
            Op::Put => PlaneOp::Put,
            Op::Average => PlaneOp::Avg,
            Op::Weigh(weighting) => PlaneOp::Weigh(weighting),
        }
    }
}

impl Target<'_> {
    /// What the target does with the prediction of `plane` (0 luma, 1 Cb,
    /// 2 Cr): [`Op::Put`] for [`Target::Scratch`] too, into its buffer.
    #[inline(always)]
    fn op(self, plane: usize) -> Op {
        match self {
            Target::Average => Op::Average,
            Target::Weighted(ops) => ops.get(plane).copied().unwrap_or(Op::Put),
            Target::Picture | Target::Scratch => Op::Put,
        }
    }
}

/// One luma prediction weighted into the picture, out of line. The
/// block's path calls each of the unweighted kernels from one site: with a
/// call for each target, and the weighted kernels inline beside them, the
/// compiler stopped inlining the unweighted kernels' dispatch into
/// [`SliceDecoder::predict_whole`], which cost the unweighted blocks of
/// every stream more than the weighted ones saved.
#[inline(never)]
fn luma_weigh<K: Kernels>(
    kernels: &K,
    dst: BlockMut<'_>,
    src: Block<'_>,
    xf: u8,
    yf: u8,
    weighting: Weighting,
) {
    kernels.luma_mc_weighted(dst, src, xf, yf, weighting);
}

/// One chroma prediction weighted into the picture, out of line: see
/// [`luma_weigh`].
#[inline(never)]
fn chroma_weigh<K: Kernels>(
    kernels: &K,
    dst: BlockMut<'_>,
    src: Block<'_>,
    xf: u8,
    yf: u8,
    weighting: Weighting,
) {
    kernels.chroma_mc_weighted(dst, src, xf, yf, weighting);
}

/// Buffers the prediction of one macroblock needs, kept across macroblocks
/// so that no block allocates.
pub(crate) struct Scratch {
    /// The luma prediction of each list, 16 samples per row.
    luma: [[u8; 256]; 2],
    /// The chroma prediction of each list and component, 8 per row.
    chroma: [[[u8; 64]; 2]; 2],
    /// A window of a reference picture whose edges are replicated, a row
    /// of [`EDGE_STRIDE`] samples per window row ([`edge_window`]).
    edge: [[u8; EDGE_STRIDE]; 21],
    /// The spatial direct motion of the current macroblock, once derived.
    spatial: Option<SpatialDirect>,
}

impl Scratch {
    pub(crate) fn new() -> Box<Scratch> {
        Box::new(Scratch {
            luma: [[0; 256]; 2],
            chroma: [[[0; 64]; 2]; 2],
            edge: [[0; EDGE_STRIDE]; 21],
            spatial: None,
        })
    }
}

/// The reference indices and motion vectors that spatial direct prediction
/// gives every partition of a macroblock (clause 8.4.1.2.2).
#[derive(Clone, Copy)]
struct SpatialDirect {
    ref_idx: [i8; 2],
    mv: [[i16; 2]; 2],
    /// Both reference indices were below zero, so every vector is zero.
    zero: bool,
}

/// The reference index a block of [`MotionCtx`] holds where it is not
/// available (clause 6.4.11.7): outside the picture or the slice, or a
/// block of the current macroblock whose motion is not derived yet. An
/// available block that does not use a list holds -1 there, as an intra
/// one does; the clauses that tell the two apart test this value.
const UNAVAILABLE: i8 = -2;

/// The width of [`MotionCtx`]'s grid in 4x4 blocks.
const CTX_W: i32 = 6;

/// The blocks of [`MotionCtx`]'s grid, 30 of them used.
const CTX_CELLS: usize = 32;

/// The motion of the 4x4 blocks around and inside the macroblock being
/// predicted, per list: what motion vector prediction (clause 8.4.1.3) and
/// spatial direct prediction (clause 8.4.1.2.2) read.
///
/// A grid six blocks wide and five high: row 0 is the row above the
/// macroblock (D, then the four blocks of B, then C), column 0 the column
/// left of it (A), the macroblock is rows 1 to 4 and columns 1 to 4, and
/// column 5 below row 0 is the macroblock to the right, never available.
/// It is filled once a macroblock, from the motion its neighbours left in
/// the picture, with every block of the macroblock itself unavailable;
/// each partition writes its blocks as its motion is derived. So a
/// neighbour is one load at a position known from the partition's shape,
/// where deriving it (clause 6.4.12) took a branch per macroblock edge,
/// a test of whether the block was derived yet and a test for intra.
///
/// The reference indices of a list are one array, so that making every
/// block unavailable is one store a list; a vector is read only where its
/// reference index is at least 0, and is not cleared.
#[derive(Clone, Copy)]
pub(crate) struct MotionCtx {
    ref_idx: [[i8; CTX_CELLS]; 2],
    mv: [[[i16; 2]; CTX_CELLS]; 2],
}

impl MotionCtx {
    pub(crate) const EMPTY: MotionCtx = MotionCtx {
        ref_idx: [[UNAVAILABLE; CTX_CELLS]; 2],
        mv: [[[0; 2]; CTX_CELLS]; 2],
    };

    /// The reference index and vector of list `list` at block `c`: -1 and
    /// zero where the block does not use the list, and [`UNAVAILABLE`] and
    /// zero where it is not available (clause 8.4.1.3.2).
    #[inline]
    fn at(&self, list: usize, c: usize) -> (i8, [i16; 2]) {
        let r = self
            .ref_idx
            .get(list & 1)
            .and_then(|l| l.get(c))
            .copied()
            .unwrap_or(UNAVAILABLE);
        let mv = self
            .mv
            .get(list & 1)
            .and_then(|l| l.get(c))
            .copied()
            .unwrap_or([0, 0]);
        (r, if r < 0 { [0, 0] } else { mv })
    }

    /// Block `c` of the first `LISTS` lists, from a block of the
    /// picture's motion. A P slice fills list 0 alone: list 1 is never read
    /// there.
    #[inline(always)]
    fn set<const LISTS: usize>(&mut self, c: usize, m: &Motion) {
        for ((refs, mvs), (&r, &mv)) in self
            .ref_idx
            .iter_mut()
            .zip(self.mv.iter_mut())
            .zip(m.ref_idx.iter().zip(m.mv.iter()))
            .take(LISTS)
        {
            if let Some(slot) = refs.get_mut(c) {
                *slot = r;
            }
            if let Some(slot) = mvs.get_mut(c) {
                *slot = mv;
            }
        }
    }

    /// `N` blocks of the first `LISTS` lists from block `c` on, all with
    /// motion `m`.
    #[inline(always)]
    fn set_row<const N: usize, const LISTS: usize>(&mut self, c: usize, m: &Motion) {
        for ((refs, mvs), (&r, &mv)) in self
            .ref_idx
            .iter_mut()
            .zip(self.mv.iter_mut())
            .zip(m.ref_idx.iter().zip(m.mv.iter()))
            .take(LISTS)
        {
            put_row::<N, _>(refs, c, &[r; N]);
            put_row::<N, _>(mvs, c, &[mv; N]);
        }
    }
}

/// Writes `row` into `dst` at `at`, if it fits.
#[inline(always)]
fn put_row<const N: usize, T: Copy>(dst: &mut [T], at: usize, row: &[T; N]) {
    if let Some(out) = dst
        .get_mut(at..at.wrapping_add(N))
        .and_then(|s| <&mut [T; N]>::try_from(s).ok())
    {
        *out = *row;
    }
}

/// Row `at` of a partition `bw` blocks wide in the picture's motion, and
/// the same row from block `c` on in the first `LISTS` lists of `ctx`.
#[inline(always)]
fn put_rows<const LISTS: usize>(
    motion: &mut [Motion],
    ctx: &mut MotionCtx,
    at: usize,
    c: usize,
    bw: usize,
    m: &Motion,
) {
    match bw {
        4 => {
            put_row::<4, _>(motion, at, &[*m; 4]);
            ctx.set_row::<4, LISTS>(c, m);
        }
        2 => {
            put_row::<2, _>(motion, at, &[*m; 2]);
            ctx.set_row::<2, LISTS>(c, m);
        }
        _ => {
            put_row::<1, _>(motion, at, &[*m]);
            ctx.set_row::<1, LISTS>(c, m);
        }
    }
}

/// The block of [`MotionCtx`] that holds the luma location (x, y) relative
/// to the macroblock, for x from -1 to 16 and y from -1 to 15.
#[inline]
fn cell(x: i32, y: i32) -> usize {
    ((((y + 4) >> 2) * CTX_W + ((x + 4) >> 2)) as usize) & (CTX_CELLS - 1)
}

/// What inter prediction looks up per block that depends on the slice
/// alone, built once when the slice starts.
pub(crate) struct InterTables {
    /// [`crate::picture::Picture::slot`] of each entry of each list, and
    /// how many entries each list has.
    slots: [[u8; MAX_REFS]; 2],
    count: [usize; 2],
    /// Whether the first entry of list 1 is a short-term reference, which
    /// colZeroFlag asks (clause 8.4.1.2.2).
    col_short_term: bool,
    /// Temporal direct prediction's DistScaleFactor (Equation 8-196) for
    /// each refIdxL0, or [`NO_SCALING`] where the clause takes mvCol as
    /// mvL0 and a zero mvL1.
    dist_scale: [i32; MAX_REFS],
    /// Explicit weighted prediction (weighted_pred_flag for P and SP
    /// slices, weighted_bipred_idc 1 for B slices), and implicit
    /// (weighted_bipred_idc 2 for B slices).
    explicit: bool,
    implicit: bool,
    /// The implicit weight w1 of each pair (refIdxL0, refIdxL1), at
    /// `refIdxL0 * MAX_REFS + refIdxL1` (clause 8.4.2.3.1; w0 is 64 - w1):
    /// worked out once a slice rather than once a block, since it divides.
    /// 32, the default, where a pair is not in the lists.
    implicit_w1: [i16; MAX_REFS * MAX_REFS],
}

/// [`InterTables::dist_scale`] where temporal direct prediction does not
/// scale the co-located vector.
const NO_SCALING: i32 = i32::MIN;

impl InterTables {
    pub(crate) fn new(
        refs: &[Vec<RefPic>; 2],
        slice_type: SliceType,
        pps: &Pps,
        current_poc: i32,
    ) -> InterTables {
        let mut slots = [[NO_SLOT; MAX_REFS]; 2];
        let mut count = [0; 2];
        for ((out, n), list) in slots.iter_mut().zip(count.iter_mut()).zip(refs.iter()) {
            for (slot, r) in out.iter_mut().zip(list.iter()) {
                *slot = r.picture.slot;
            }
            *n = list.len().min(MAX_REFS);
        }
        let pic1 = refs.get(1).and_then(|l| l.first());
        let mut dist_scale = [NO_SCALING; MAX_REFS];
        if let (Some(pic1), Some(list0)) = (pic1, refs.first()) {
            for (slot, pic0) in dist_scale.iter_mut().zip(list0.iter()) {
                if pic0.long_term || pic1.picture.poc() == pic0.picture.poc() {
                    continue;
                }
                // Equations 8-193 to 8-196.
                let tb = poc_distance(current_poc, pic0.picture.poc());
                let td = poc_distance(pic1.picture.poc(), pic0.picture.poc());
                let tx = (16384 + (td / 2).abs()) / td;
                *slot = ((tb * tx + 32) >> 6).clamp(-1024, 1023);
            }
        }
        let implicit = slice_type == SliceType::B && pps.weighted_bipred_idc == 2;
        let mut implicit_w1 = [32; MAX_REFS * MAX_REFS];
        if let (true, [list0, list1]) = (implicit, refs) {
            for (r0, p0) in list0.iter().enumerate().take(MAX_REFS) {
                for (r1, p1) in list1.iter().enumerate().take(MAX_REFS) {
                    if let Some(slot) = implicit_w1.get_mut(r0 * MAX_REFS + r1) {
                        *slot = implicit_w1_of(current_poc, p0, p1);
                    }
                }
            }
        }
        InterTables {
            slots,
            count,
            col_short_term: pic1.is_some_and(|r| !r.long_term),
            dist_scale,
            explicit: match slice_type {
                SliceType::P | SliceType::Sp => pps.weighted_pred,
                SliceType::B => pps.weighted_bipred_idc == 1,
                _ => false,
            },
            implicit,
            implicit_w1,
        }
    }
}

/// The implicit weight w1 of a block predicted from `p0` of list 0 and
/// `p1` of list 1 in a picture of order count `current_poc` (clause
/// 8.4.2.3.1): 32 where either is a long-term reference, the two are the
/// same distance away, or DistScaleFactor >> 2 lies outside -64 to 128.
fn implicit_w1_of(current_poc: i32, p0: &RefPic, p1: &RefPic) -> i16 {
    let tb = poc_distance(current_poc, p0.picture.poc());
    let td = poc_distance(p1.picture.poc(), p0.picture.poc());
    if td == 0 || p0.long_term || p1.long_term {
        return 32;
    }
    let tx = (16384 + (td / 2).abs()) / td;
    let factor = ((tb * tx + 32) >> 6).clamp(-1024, 1023);
    if (factor >> 2) < -64 || (factor >> 2) > 128 {
        return 32;
    }
    // -64 to 128: an i16 holds it.
    (factor >> 2) as i16
}

/// The median of three values (Equation 8-207).
fn median(a: i16, b: i16, c: i16) -> i16 {
    a.max(b).min(a.min(b).max(c))
}

/// MinPositive (Equation 8-208): the smaller of two indices where both are
/// at least 0, else the larger.
fn min_positive(x: i8, y: i8) -> i8 {
    if x >= 0 && y >= 0 { x.min(y) } else { x.max(y) }
}

/// The partition shapes whose prediction has a directional rule
/// (Equations 8-203 to 8-206).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Other,
    Part16x8(usize),
    Part8x16(usize),
}

impl<'a, K: Kernels> SliceDecoder<'a, K> {
    /// Fills [`SliceDecoder::ctx`] for the current macroblock: its
    /// neighbours' blocks from the motion they left in the picture, its own
    /// blocks and those right of it unavailable. An intra neighbour left a
    /// head of [`NO_MOTION`] there (`SliceDecoder::intra_head`), which
    /// reads as available with reference index -1, as clause 8.4.1.3.2 has
    /// it.
    ///
    /// Each neighbour's head is looked up once, and a neighbour with one
    /// motion sets its cells from it.
    ///
    /// `T`, unused, gives each instance of the hot paths its own copy
    /// ([`crate::tier`]).
    fn fill_ctx<const LISTS: usize, T: Tier>(&mut self) {
        let [a, b, c, d] = self.nbs;
        let base = self.motion_base;
        let band = &self.out;
        let ctx = &mut self.ctx;
        for refs in ctx.ref_idx.iter_mut().take(LISTS) {
            *refs = [UNAVAILABLE; CTX_CELLS];
        }
        let motion = |m: Option<usize>| m.and_then(|m| band.mb_motion(m.wrapping_sub(base)));
        if let Some(m) = motion(d) {
            ctx.set::<LISTS>(0, m.block(15));
        }
        match motion(b) {
            Some(MbMotion::One(m)) => ctx.set_row::<4, LISTS>(1, m),
            Some(m) => {
                for i in 0..4 {
                    ctx.set::<LISTS>(1 + i, m.block(12 + i));
                }
            }
            None => {}
        }
        if let Some(m) = motion(c) {
            ctx.set::<LISTS>(5, m.block(12));
        }
        match motion(a) {
            Some(MbMotion::One(m)) => {
                for r in 0..4 {
                    ctx.set::<LISTS>(6 * (r + 1), m);
                }
            }
            Some(m) => {
                for r in 0..4 {
                    ctx.set::<LISTS>(6 * (r + 1), m.block(4 * r + 3));
                }
            }
            None => {}
        }
    }

    /// mvpLX of the partition at (x, y) of width `w` (clause 8.4.1.3).
    fn mv_predictor(
        &self,
        x: i32,
        y: i32,
        w: i32,
        list: usize,
        ref_idx: i8,
        shape: Shape,
    ) -> [i16; 2] {
        let ctx = &self.ctx;
        let a = ctx.at(list, cell(x - 1, y));
        let mut b = ctx.at(list, cell(x, y - 1));
        let mut c = ctx.at(list, cell(x + w, y - 1));
        if c.0 == UNAVAILABLE {
            // D takes C's place where C is not available (clause 8.4.1.3.2).
            c = ctx.at(list, cell(x - 1, y - 1));
        }
        // The directional rules of a 16x8 or 8x16 partition.
        match shape {
            Shape::Part16x8(0) if b.0 == ref_idx => return b.1,
            Shape::Part16x8(1) if a.0 == ref_idx => return a.1,
            Shape::Part8x16(0) if a.0 == ref_idx => return a.1,
            Shape::Part8x16(1) if c.0 == ref_idx => return c.1,
            _ => {}
        }
        if b.0 == UNAVAILABLE && c.0 == UNAVAILABLE && a.0 != UNAVAILABLE {
            b = a;
            c = a;
        }
        let matching =
            usize::from(a.0 == ref_idx) + usize::from(b.0 == ref_idx) + usize::from(c.0 == ref_idx);
        if matching == 1 {
            if a.0 == ref_idx {
                return a.1;
            }
            if b.0 == ref_idx {
                return b.1;
            }
            return c.1;
        }
        [
            median(a.1[0], b.1[0], c.1[0]),
            median(a.1[1], b.1[1], c.1[1]),
        ]
    }

    /// `m` as the picture keeps it: the slot of each picture it refers to,
    /// and a zero vector for a list it does not use.
    fn resolve(&self, mut m: Motion) -> Result<Motion> {
        let tables = &self.tables;
        for (list, ((r, mv), slot)) in m
            .ref_idx
            .iter()
            .zip(m.mv.iter_mut())
            .zip(m.ref_slot.iter_mut())
            .enumerate()
        {
            *slot = if let Ok(i) = usize::try_from(*r) {
                if i >= tables.count.get(list).copied().unwrap_or(0) {
                    return Err(Error::Invalid("a reference index past its list"));
                }
                tables
                    .slots
                    .get(list)
                    .and_then(|l| l.get(i))
                    .copied()
                    .unwrap_or(NO_SLOT)
            } else {
                *mv = [0, 0];
                NO_SLOT
            };
        }
        Ok(m)
    }

    /// Stores one motion for the whole macroblock, into the picture: a
    /// macroblock with one partition, which no later partition reads in
    /// [`SliceDecoder::ctx`]. Its head alone, 16 bytes where its blocks
    /// are 192: they are not written, and no reader takes them
    /// ([`MbHead`]). `motion_first` stays `None`, which tells `finish_mb`
    /// that the head is written.
    #[inline(always)]
    fn store_mb(&mut self, m: Motion) -> Result<()> {
        let m = self.resolve(m)?;
        let at = self.addr.wrapping_sub(self.motion_base);
        if let Some(head) = self.out.heads.get_mut(at) {
            *head = MbHead { m, split: false };
        }
        if cfg!(debug_assertions) {
            self.derived = 0xffff;
        }
        Ok(())
    }

    /// Stores the motion of the 4x4 blocks of a partition: into the
    /// picture, for the deblocking filter and later pictures, and into
    /// [`SliceDecoder::ctx`], for the partitions after it. `finish_mb`
    /// writes the macroblock's head once its partitions are stored.
    fn store_motion(
        &mut self,
        bx: usize,
        by: usize,
        bw: usize,
        bh: usize,
        m: Motion,
    ) -> Result<()> {
        let m = self.resolve(m)?;
        // Whether the macroblock ends up with one motion throughout, which
        // lets the deblocking filter retire its internal edges in one test
        // (`MbInfo::uniform`) and its head stand for its blocks. Partitions
        // that store the same motion keep it uniform, however many of them
        // there are.
        match self.motion_first {
            None => self.motion_first = Some(m),
            Some(first) if first != m => self.cur.uniform = false,
            Some(_) => {}
        }
        // One row of the partition at a time: a fill of `bw` blocks in the
        // picture and in each list of the context.
        let base = self.motion_at(self.addr);
        let motion = &mut self.out.motion;
        if cfg!(debug_assertions) {
            for y in by..(by + bh).min(4) {
                self.derived |= ((1u16 << bw) - 1) << (y * 4 + bx);
            }
        }
        let rows = (by..(by + bh).min(4)).map(|y| (base + y * 4 + bx, (y + 1) * 6 + bx + 1));
        // Rows of a fixed width: a store of the whole row, where a fill of
        // a width known only at run time is a loop.
        if self.hdr.slice_type.is_b() {
            for (at, c) in rows {
                put_rows::<2>(motion, &mut self.ctx, at, c, bw, &m);
            }
        } else {
            for (at, c) in rows {
                put_rows::<1>(motion, &mut self.ctx, at, c, bw, &m);
            }
        }
        Ok(())
    }

    /// Derives the motion of the current macroblock and predicts it
    /// (clause 8.4).
    pub(crate) fn inter_pred<T: Tier>(&mut self, tier: T) -> Result<()> {
        self.scratch.spatial = None;
        if self.mb.kind == MbKind::PSkip {
            // Fills the context only where it needs it.
            return self.predict_p_skip(tier);
        }
        if self.hdr.slice_type.is_b() {
            self.fill_ctx::<2, T>();
        } else {
            self.fill_ctx::<1, T>();
        }
        match self.mb.kind {
            MbKind::BSkip | MbKind::B(0) if self.sps.direct_8x8_inference => {
                self.predict_direct(tier)
            }
            MbKind::BSkip | MbKind::B(0) => {
                for p in 0..4 {
                    self.direct_8x8(tier, p, false)?;
                }
                self.predict_merged(tier)
            }
            MbKind::P(3) | MbKind::P(4) | MbKind::B(22) => self.predict_sub_partitions(tier),
            MbKind::P(_) | MbKind::B(_) => self.predict_partitions(tier),
            _ => Err(Error::Invalid("not an inter macroblock")),
        }
    }

    /// P_Skip (clause 8.4.1.1). Its vector is zero where neighbour A or B
    /// is unavailable or holds a zero vector to reference 0, which on still
    /// content is most of the time, so A and B are read from the picture
    /// first and the context is filled only for the predictor.
    fn predict_p_skip<T: Tier>(&mut self, tier: T) -> Result<()> {
        let base = self.motion_base;
        let band = &self.out;
        let [a, b, ..] = self.nbs;
        let at = |m: Option<usize>, block: usize| {
            m.and_then(|m| band.mb_motion(m.wrapping_sub(base)))
                .map(|m| m.block(block))
        };
        let still = |m: &Motion| m.ref_idx[0] == 0 && m.mv[0] == [0, 0];
        let zero = match (at(a, 3), at(b, 12)) {
            (Some(a), Some(b)) => still(a) || still(b),
            _ => true,
        };
        let mv = if zero {
            [0, 0]
        } else {
            self.fill_ctx::<1, T>();
            self.mv_predictor(0, 0, 16, 0, 0, Shape::Other)
        };
        let m = Motion {
            mv: [mv, [0, 0]],
            ref_idx: [0, -1],
            ref_slot: [NO_SLOT; 2],
        };
        self.store_mb(m)?;
        self.motion_compensate(tier, part(0, 0, 16, 16)?, m)
    }

    /// A macroblock with one or two whole-macroblock partitions.
    fn predict_partitions<T: Tier>(&mut self, tier: T) -> Result<()> {
        let row = match self.mb.kind {
            MbKind::P(ty) => t::P_MB_TYPES.get(usize::from(ty)).copied(),
            MbKind::B(ty) => t::B_MB_TYPES.get(usize::from(ty)).copied(),
            _ => None,
        };
        let (parts, m0, m1, w, h) = row.ok_or(Error::Invalid("mb_type"))?;
        let modes = [m0, m1];
        for p in 0..usize::from(parts) {
            let (x, y) = part_xy(p, w, h);
            let shape = match (w, h) {
                (16, 8) => Shape::Part16x8(p),
                (8, 16) => Shape::Part8x16(p),
                _ => Shape::Other,
            };
            let mut m = NO_MOTION;
            for list in 0..2 {
                let mode = modes.get(p.min(1)).copied().unwrap_or(PredMode::Na);
                if !uses(mode, list) {
                    continue;
                }
                let ref_idx = self
                    .mb
                    .ref_idx
                    .get(list)
                    .and_then(|l| l.get(p))
                    .copied()
                    .unwrap_or(0);
                let mvp = self.mv_predictor(x as i32, y as i32, i32::from(w), list, ref_idx, shape);
                let mvd = self
                    .mb
                    .mvd
                    .get(list)
                    .and_then(|l| l.get(p * 4))
                    .copied()
                    .unwrap_or([0; 2]);
                if let Some(slot) = m.mv.get_mut(list) {
                    *slot = add_mvd(mvp, mvd)?;
                }
                if let Some(slot) = m.ref_idx.get_mut(list) {
                    *slot = ref_idx;
                }
            }
            if parts == 1 {
                self.store_mb(m)?;
            } else {
                self.store_motion(x / 4, y / 4, usize::from(w) / 4, usize::from(h) / 4, m)?;
            }
            self.motion_compensate(tier, part(x, y, usize::from(w), usize::from(h))?, m)?;
        }
        Ok(())
    }

    /// A macroblock whose 8x8 blocks have their own partitions.
    fn predict_sub_partitions<T: Tier>(&mut self, tier: T) -> Result<()> {
        let is_b = matches!(self.mb.kind, MbKind::B(_));
        for p in 0..4 {
            let ty = self.mb.sub.get(p).copied().unwrap_or(0);
            if is_b && ty == 0 {
                self.direct_8x8(tier, p, true)?;
                continue;
            }
            let (n, mode, w, h) = if is_b {
                sub_shape(true, ty)
            } else {
                let (n, _, w, h) = sub_shape(false, ty);
                (n, PredMode::L0, w, h)
            };
            for sp in 0..usize::from(n) {
                let (sx, sy) = sub_part_xy(sp, w, h);
                let x = (p % 2) * 8 + sx;
                let y = (p / 2) * 8 + sy;
                let mut m = NO_MOTION;
                for list in 0..2 {
                    if !uses(mode, list) {
                        continue;
                    }
                    let ref_idx = self
                        .mb
                        .ref_idx
                        .get(list)
                        .and_then(|l| l.get(p))
                        .copied()
                        .unwrap_or(0);
                    let mvp = self.mv_predictor(
                        x as i32,
                        y as i32,
                        i32::from(w),
                        list,
                        ref_idx,
                        Shape::Other,
                    );
                    let mvd = self
                        .mb
                        .mvd
                        .get(list)
                        .and_then(|l| l.get(p * 4 + sp))
                        .copied()
                        .unwrap_or([0; 2]);
                    if let Some(slot) = m.mv.get_mut(list) {
                        *slot = add_mvd(mvp, mvd)?;
                    }
                    if let Some(slot) = m.ref_idx.get_mut(list) {
                        *slot = ref_idx;
                    }
                }
                self.store_motion(x / 4, y / 4, usize::from(w) / 4, usize::from(h) / 4, m)?;
                self.motion_compensate(tier, part(x, y, usize::from(w), usize::from(h))?, m)?;
            }
        }
        Ok(())
    }

    /// B_Skip and B_Direct_16x16 with direct_8x8_inference_flag (clause
    /// 8.4.1.2): the motion of each 8x8 quadrant from the co-located
    /// macroblock's corner block, stored and predicted for the whole
    /// macroblock at once where the four agree, which they mostly do.
    fn predict_direct<T: Tier>(&mut self, tier: T) -> Result<()> {
        // The corner 4x4 block each quadrant takes its co-located motion
        // from (direct_8x8_inference_flag).
        const CORNERS: [(usize, usize); 4] = [(0, 0), (3, 0), (0, 3), (3, 3)];
        let spatial = self
            .hdr
            .direct_spatial_mv_pred
            .then(|| self.spatial_direct());
        let col = self.colocated_mb()?;
        let mut q = [NO_MOTION; 4];
        for (slot, &(cx, cy)) in q.iter_mut().zip(CORNERS.iter()) {
            let (mv_col, ref_col, slot_col) = colocated(col.block(cy * 4 + cx));
            *slot = match &spatial {
                Some(direct) => self.spatial_motion(direct, mv_col, ref_col),
                None => self.temporal_direct_motion(mv_col, ref_col, slot_col)?,
            };
        }
        let [q0, q1, q2, q3] = q;
        if q0 == q1 && q0 == q2 && q0 == q3 {
            self.store_mb(q0)?;
            return self.motion_compensate(tier, part(0, 0, 16, 16)?, q0);
        }
        for (p, m) in q.into_iter().enumerate() {
            self.store_motion((p % 2) * 2, (p / 2) * 2, 2, 2, m)?;
        }
        self.predict_quadrants(tier, q)
    }

    /// Predicts a direct macroblock whose 8x8 quadrants each carry one
    /// motion (direct_8x8_inference_flag), with quadrants of equal motion
    /// predicted together.
    ///
    /// Prediction is per sample (clauses 8.4.2.2 and 8.4.2.3), so this
    /// gives the same samples as predicting each quadrant on its own.
    fn predict_quadrants<T: Tier>(&mut self, tier: T, q: [Motion; 4]) -> Result<()> {
        let [q0, q1, q2, q3] = q;
        if q0 == q1 && q0 == q2 && q0 == q3 {
            return self.motion_compensate(tier, part(0, 0, 16, 16)?, q0);
        }
        if q0 == q1 && q2 == q3 {
            self.motion_compensate(tier, part(0, 0, 16, 8)?, q0)?;
            return self.motion_compensate(tier, part(0, 8, 16, 8)?, q2);
        }
        if q0 == q2 && q1 == q3 {
            self.motion_compensate(tier, part(0, 0, 8, 16)?, q0)?;
            return self.motion_compensate(tier, part(8, 0, 8, 16)?, q1);
        }
        for (i, m) in q.into_iter().enumerate() {
            self.motion_compensate(tier, part((i % 2) * 8, (i / 2) * 8, 8, 8)?, m)?;
        }
        Ok(())
    }

    /// Predicts the whole macroblock from the motion already stored, with
    /// blocks of equal motion predicted together: a direct macroblock
    /// whose 4x4 blocks each have their own motion.
    fn predict_merged<T: Tier>(&mut self, tier: T) -> Result<()> {
        let base = self.motion_at(self.addr);
        let motion: [Motion; 16] = self
            .out
            .motion
            .get(base..base + 16)
            .and_then(|m| <[Motion; 16]>::try_from(m).ok())
            .ok_or(Error::Geometry("the motion field of a macroblock"))?;
        let at = |i: usize| motion.get(i).copied().unwrap_or(NO_MOTION);
        // Whether the four 4x4 blocks of quadrant `q` share one motion.
        let uniform = |q: usize| {
            let o = (q / 2) * 8 + (q % 2) * 2;
            at(o) == at(o + 1) && at(o) == at(o + 4) && at(o) == at(o + 5)
        };
        if (0..4).all(uniform) {
            return self.predict_quadrants(tier, [at(0), at(2), at(8), at(10)]);
        }
        for q in 0..4 {
            let (x, y) = ((q % 2) * 8, (q / 2) * 8);
            let o = (q / 2) * 8 + (q % 2) * 2;
            if uniform(q) {
                self.motion_compensate(tier, part(x, y, 8, 8)?, at(o))?;
            } else {
                for k in 0..4 {
                    let m = at(o + (k / 2) * 4 + k % 2);
                    self.motion_compensate(tier, part(x + (k % 2) * 4, y + (k / 2) * 4, 4, 4)?, m)?;
                }
            }
        }
        Ok(())
    }

    /// The motion of the co-located macroblock (clause 8.4.1.2.1), its
    /// head looked up once.
    fn colocated_mb(&self) -> Result<MbMotion<'_>> {
        let other_size = Error::Invalid("a co-located picture of another size");
        match self.colocated_whole {
            // Found when the slice started: one published band of this
            // picture's size.
            Some(band) => band.mb_motion(self.addr).ok_or(other_size),
            None => self.colocated_banded(),
        }
    }

    /// The motion of the co-located macroblock in a picture cut into
    /// bands, once its band is published.
    fn colocated_banded(&self) -> Result<MbMotion<'_>> {
        let col = &self
            .refs
            .get(1)
            .and_then(|l| l.first())
            .ok_or(Error::Invalid("direct prediction without a list 1"))?
            .picture;
        let other_size = Error::Invalid("a co-located picture of another size");
        if col.mb_width != self.mb_w || col.mb_height != self.mb_h {
            return Err(other_size);
        }
        // The co-located macroblock's band, once its writer has published
        // it (`docs/design/h264.md`, "Threading (step 7)").
        let g = col.geometry();
        let k = g.band_of_mb_row(self.mby);
        let first = g.motion_first(k) * self.mb_w;
        col.band(k)
            .and_then(|b| b.mb_motion(self.addr.wrapping_sub(first)))
            .ok_or(other_size)
    }

    /// Direct prediction of the 8x8 quadrant `p` (clause 8.4.1.2), for
    /// B_Direct_8x8, and B_Skip and B_Direct_16x16 without
    /// direct_8x8_inference_flag.
    fn direct_8x8<T: Tier>(&mut self, tier: T, p: usize, compensate: bool) -> Result<()> {
        let (x0, y0) = ((p % 2) * 2, (p / 2) * 2);
        let inference = self.sps.direct_8x8_inference;
        // With direct_8x8_inference_flag the whole 8x8 takes the motion of
        // the macroblock's outer corner; otherwise each 4x4 takes its own.
        let blocks: &[(usize, usize)] = if inference {
            &[(0, 0)]
        } else {
            &[(0, 0), (1, 0), (0, 1), (1, 1)]
        };
        // The co-located motion of each block, from one lookup of the
        // co-located macroblock's head.
        let mut cols = [([0, 0], -1, NO_SLOT); 4];
        let col = self.colocated_mb()?;
        for (out, &(dx, dy)) in cols.iter_mut().zip(blocks) {
            let (bx, by) = (x0 + dx, y0 + dy);
            let (cx, cy) = if inference {
                (
                    if p % 2 == 0 { 0 } else { 3 },
                    if p / 2 == 0 { 0 } else { 3 },
                )
            } else {
                (bx, by)
            };
            *out = colocated(col.block(cy * 4 + cx));
        }
        for (&(mv_col, ref_col, slot_col), &(dx, dy)) in cols.iter().zip(blocks) {
            let (bx, by) = (x0 + dx, y0 + dy);
            let m = if self.hdr.direct_spatial_mv_pred {
                self.spatial_direct_motion(mv_col, ref_col)
            } else {
                self.temporal_direct_motion(mv_col, ref_col, slot_col)?
            };
            let size = if inference { 2 } else { 1 };
            self.store_motion(bx, by, size, size, m)?;
            if compensate {
                self.motion_compensate(tier, part(bx * 4, by * 4, size * 4, size * 4)?, m)?;
            }
        }
        Ok(())
    }

    /// Spatial direct prediction of one block (clause 8.4.1.2.2), with the
    /// macroblock's direct motion derived once a macroblock.
    fn spatial_direct_motion(&mut self, mv_col: [i16; 2], ref_col: i8) -> Motion {
        let direct = match self.scratch.spatial {
            Some(s) => s,
            None => {
                let s = self.spatial_direct();
                self.scratch.spatial = Some(s);
                s
            }
        };
        self.spatial_motion(&direct, mv_col, ref_col)
    }

    /// Spatial direct prediction of one block (clause 8.4.1.2.2) from the
    /// macroblock's direct motion `direct` and the block's co-located
    /// motion.
    fn spatial_motion(&self, direct: &SpatialDirect, mv_col: [i16; 2], ref_col: i8) -> Motion {
        // colZeroFlag (clause 8.4.1.2.2).
        let col_zero = self.tables.col_short_term
            && ref_col == 0
            && (-1..=1).contains(&mv_col[0])
            && (-1..=1).contains(&mv_col[1]);
        let mut m = NO_MOTION;
        for list in 0..2 {
            let ref_idx = direct.ref_idx.get(list).copied().unwrap_or(-1);
            if let Some(slot) = m.ref_idx.get_mut(list) {
                *slot = ref_idx;
            }
            let mv = if direct.zero || ref_idx < 0 || (ref_idx == 0 && col_zero) {
                [0, 0]
            } else {
                direct.mv.get(list).copied().unwrap_or([0, 0])
            };
            if let Some(slot) = m.mv.get_mut(list) {
                *slot = mv;
            }
        }
        m
    }

    /// Temporal direct prediction of one block (clause 8.4.1.2.3).
    fn temporal_direct_motion(
        &self,
        mv_col: [i16; 2],
        ref_col: i8,
        slot_col: u8,
    ) -> Result<Motion> {
        let tables = &self.tables;
        let [n0, n1] = tables.count;
        let ref0 = if ref_col < 0 {
            0
        } else {
            // RefPicList0 shall hold refPicCol; where a reference was lost,
            // index 0 conceals it.
            tables
                .slots
                .first()
                .and_then(|slots| slots.get(..n0))
                .and_then(|slots| slots.iter().position(|&slot| slot == slot_col))
                .unwrap_or(0)
        };
        if ref0 >= n0 {
            return Err(Error::Invalid("temporal direct refIdxL0"));
        }
        if n1 == 0 {
            return Err(Error::Invalid("direct prediction without a list 1"));
        }
        let factor = tables.dist_scale.get(ref0).copied().unwrap_or(NO_SCALING);
        let (mv0, mv1) = if factor == NO_SCALING {
            (mv_col, [0, 0])
        } else {
            // Equations 8-191 and 8-192.
            let scale = |c: i16| -> i32 { (factor * i32::from(c) + 128) >> 8 };
            let l0 = [scale(mv_col[0]), scale(mv_col[1])];
            let l1 = [l0[0] - i32::from(mv_col[0]), l0[1] - i32::from(mv_col[1])];
            (to_mv(l0)?, to_mv(l1)?)
        };
        Ok(Motion {
            mv: [mv0, mv1],
            ref_idx: [ref0 as i8, 0],
            ref_slot: [NO_SLOT; 2],
        })
    }

    /// The reference indices and vectors spatial direct prediction gives
    /// the whole macroblock (clause 8.4.1.2.2).
    fn spatial_direct(&self) -> SpatialDirect {
        let mut ref_idx = [-1i8; 2];
        for (list, out) in ref_idx.iter_mut().enumerate() {
            let a = self.ctx.at(list, cell(-1, 0)).0;
            let b = self.ctx.at(list, cell(0, -1)).0;
            let mut c = self.ctx.at(list, cell(16, -1)).0;
            if c == UNAVAILABLE {
                c = self.ctx.at(list, cell(-1, -1)).0;
            }
            // An unavailable neighbour's index is -1 in the clause.
            *out = min_positive(a, min_positive(b, c)).max(-1);
        }
        if ref_idx[0] < 0 && ref_idx[1] < 0 {
            return SpatialDirect {
                ref_idx: [0, 0],
                mv: [[0; 2]; 2],
                zero: true,
            };
        }
        let mut mv = [[0i16; 2]; 2];
        for (list, out) in mv.iter_mut().enumerate() {
            let r = ref_idx.get(list).copied().unwrap_or(-1);
            if r >= 0 {
                *out = self.mv_predictor(0, 0, 16, list, r, Shape::Other);
            }
        }
        SpatialDirect {
            ref_idx,
            mv,
            zero: false,
        }
    }

    /// The implicit weights of a bi-predicted block (clause 8.4.2.3.1).
    fn implicit_weights(&self, r0: usize, r1: usize) -> (i32, i32) {
        let w1 = (r0 < MAX_REFS && r1 < MAX_REFS)
            .then(|| self.tables.implicit_w1.get(r0 * MAX_REFS + r1))
            .flatten()
            .map_or(32, |&w| i32::from(w));
        (64 - w1, w1)
    }

    /// Predicts the block `part` of the current macroblock from its motion
    /// `m`, into the picture.
    pub(crate) fn motion_compensate<T: Tier>(
        &mut self,
        tier: T,
        part: Part,
        m: Motion,
    ) -> Result<()> {
        let used = [m.ref_idx[0] >= 0, m.ref_idx[1] >= 0];
        let (w, h) = (part.w(), part.h());
        let (px, py) = (self.mbx * 16 + part.x(), self.mby * 16 + part.y());
        // A slice without weighted prediction, most slices: one list goes
        // straight into the picture, and the second is averaged into it
        // (Equation 8-273), as below, without the weights' tests.
        if !self.tables.explicit && !self.tables.implicit {
            match used {
                [true, true] => {
                    self.predict_list(tier, 0, &m, Target::Picture, part)?;
                    return self.predict_list(tier, 1, &m, Target::Average, part);
                }
                [true, false] => {
                    return self.predict_list(tier, 0, &m, Target::Picture, part);
                }
                [false, true] => {
                    return self.predict_list(tier, 1, &m, Target::Picture, part);
                }
                [false, false] => {}
            }
        }
        let explicit = self.tables.explicit;
        // Implicit weights apply to a block predicted from both lists.
        let implicit = (self.tables.implicit && used[0] && used[1]).then(|| {
            let r0 = usize::try_from(m.ref_idx[0]).unwrap_or(0);
            let r1 = usize::try_from(m.ref_idx[1]).unwrap_or(0);
            self.implicit_weights(r0, r1)
        });
        // Whether list `list`'s reference has the default explicit weights
        // in all three planes (`PredWeightTable::default_weights`).
        let default_weights = |list: usize| {
            let r = m.ref_idx.get(list).copied().unwrap_or(-1);
            let mask = self.default_weights.get(list).copied().unwrap_or(0);
            (0..32).contains(&r) && (mask >> r) & 1 != 0
        };
        if used[0] && used[1] {
            // Both lists with the default weighting: Equation 8-273, or
            // Equation 8-276 with w0 = w1 = 2^logWD and o0 = o1 = 0 (the
            // explicit defaults, or the implicit 32 and 32, logWD 5), which
            // is the same rounded average. List 0 is predicted into the
            // picture and list 1 averaged into it.
            let average = match implicit {
                Some(weights) => weights == (32, 32),
                None => !explicit || (default_weights(0) && default_weights(1)),
            };
            if average {
                self.predict_list(tier, 0, &m, Target::Picture, part)?;
                return self.predict_list(tier, 1, &m, Target::Average, part);
            }
        }
        // One list with the default weighting, implied or explicit: the
        // prediction samples are the interpolated samples, so they go
        // straight into the picture.
        let one = usize::from(used[1]);
        let straight =
            implicit.is_none() && used[0] != used[1] && (!explicit || default_weights(one));
        if straight {
            return self.predict_list(tier, one, &m, Target::Picture, part);
        }
        // Weighted: the kernels weight each plane as they interpolate it,
        // list 1 of two over list 0's prediction in the picture.
        let modes = self.combine_modes::<T>(&m, used, explicit, implicit);
        if let Some(ops) = fused_ops::<T>(modes, used) {
            if used[0] && used[1] {
                self.predict_list(tier, 0, &m, Target::Picture, part)?;
                return self.predict_list(tier, 1, &m, Target::Weighted(&ops), part);
            }
            return self.predict_list(tier, one, &m, Target::Weighted(&ops), part);
        }
        for list in 0..2 {
            if used.get(list).copied().unwrap_or(false) {
                self.predict_list(tier, list, &m, Target::Scratch, part)?;
            }
        }
        // Where the block goes, in its band.
        let (off, chroma_off) = self.block_offsets(px, py);
        self.combine_predictions::<T>(modes, off, chroma_off, w, h)
    }

    /// Where the block at luma sample (`px`, `py`) of the picture being
    /// decoded starts in its band's luma buffer, and in its chroma buffers:
    /// past the planes' origins.
    #[inline(always)]
    fn block_offsets(&self, px: usize, py: usize) -> (usize, usize) {
        let stride = self.stride;
        let off = self.luma_origin + (py - self.luma_first) * stride + px;
        let chroma_off = self.chroma_origin + (py / 2 - self.chroma_first) * (stride / 2) + px / 2;
        (off, chroma_off)
    }

    /// The prediction of the block (`px`, `py`, `w`, `h`) from list `list`
    /// of its motion `m`, to `target`.
    ///
    /// With one thread, almost every block reads a reference that is one
    /// published band of this picture's size ([`SliceDecoder::whole`]);
    /// with more, the band of a reference its window is read from. Either
    /// way, a block into the picture whose window lies in what the band
    /// holds takes [`Self::predict_fast`].
    #[inline(always)]
    fn predict_list<T: Tier>(
        &mut self,
        tier: T,
        list: usize,
        m: &Motion,
        target: Target<'_>,
        part: Part,
    ) -> Result<()> {
        tier.predict_fast(self, list, m, target, part)
    }

    /// [`Self::predict_list`] for a block into the picture. A block from a
    /// whole reference, or from the band of a banded one its window is
    /// read from ([`Self::reference_band`]), is predicted in all three
    /// planes by one call, [`Kernels::predict_part`], which checks that the
    /// samples each window's fraction uses lie inside the reference, that
    /// every window lies in its buffer (a reference of one band keeps
    /// slack rows before and after each plane, which the windows at its
    /// top and bottom edges reach into; a band holds its top apron and its
    /// own rows), and that the block's macroblock is one the band's planes
    /// hold whole ([`SliceDecoder::macroblocks`], found when the band was
    /// taken, and each slice's length against the one found then), and
    /// declines the block, writing nothing, where one does not. Every
    /// other block, and every declined one, takes
    /// [`Self::predict_list_any`], out of line, so that what it keeps live
    /// does not cost the common block registers.
    ///
    /// Out of line on purpose: inlined into [`Self::motion_compensate`], the
    /// compiler prepares the work of both lists before it knows a block
    /// uses one, which most blocks do. One call site of
    /// [`Kernels::predict_part`] for both kinds of reference, so that its
    /// dispatch stays inlined here.
    #[inline(never)]
    pub(crate) fn predict_fast(
        &mut self,
        list: usize,
        m: &Motion,
        target: Target<'_>,
        part: Part,
    ) -> Result<()> {
        self.predict_fast_body(Base, list, m, target, part)
    }

    /// [`Self::predict_fast`]'s body, which each instance of the hot paths
    /// compiles ([`Tier`]).
    #[inline(always)]
    pub(crate) fn predict_fast_body<T: Tier>(
        &mut self,
        tier: T,
        list: usize,
        m: &Motion,
        target: Target<'_>,
        part: Part,
    ) -> Result<()> {
        let (px, py) = (self.mbx * 16 + part.x(), self.mby * 16 + part.y());
        let (w, h) = (part.w(), part.h());
        let idx = usize::try_from(m.ref_idx.get(list).copied().unwrap_or(-1)).unwrap_or(usize::MAX);
        let ops = match target {
            Target::Picture => Ops::PUT,
            Target::Average => Ops::AVG,
            Target::Weighted(ops) => match Ops::new(ops.map(Op::plane_op)) {
                Some(ops) => ops,
                None => return self.predict_list_any(list, m, target, px, py, w, h),
            },
            Target::Scratch => return self.predict_list_any(list, m, target, px, py, w, h),
        };
        let mv = m.mv.get(list).copied().unwrap_or([0, 0]);
        let whole = self
            .whole
            .get(list)
            .and_then(|l| l.get(idx))
            .and_then(Option::as_ref);
        let band;
        let src = match whole {
            Some(planes) => planes,
            None => match self.reference_band(list, idx, py, h, mv) {
                Some(planes) => {
                    band = planes;
                    &band
                }
                None => return self.predict_list_any(list, m, target, px, py, w, h),
            },
        };
        // The reference's lines the next four macroblocks read if they move
        // as this block does, one cache line ahead: asked for by the first
        // macroblock of each four, at its left edge (`docs/design/h264.md`,
        // "Motion round: the reference's lines requested ahead, cheaply").
        if self.hints != 0 && px % 64 == 0 {
            let x = px as i64 + i64::from(mv[0] >> 2) + 64;
            let y = py as i64 + i64::from(mv[1] >> 2);
            src.request(x, y, h);
        }
        let pic = &mut self.out;
        let dst = [
            pic.y.as_mut_slice(),
            pic.cb.as_mut_slice(),
            pic.cr.as_mut_slice(),
        ];
        let mb = [self.mbx, self.mby];
        if tier.predict_kernel(self.kernels, dst, &self.macroblocks, mb, part, src, mv, ops) {
            return Ok(());
        }
        self.predict_list_any(list, m, target, px, py, w, h)
    }

    /// The band of reference `idx` of list `list` that the luma window of
    /// a block at luma row `py`, `h` rows high, with vector `mv`, is read
    /// from, as [`Self::predict_list_any`] reads it: the band a reader
    /// takes the window's last row from, clamped into the picture, whose
    /// top apron holds the rest of the window (rule 2), waited for until
    /// it is published. `None` for an index past its list or a reference
    /// of another size, which [`Self::predict_list_any`] reports.
    ///
    /// The chroma windows are read from the same band, which holds their
    /// rows final: a chroma window ends at chroma row `(yi + h) / 2` at
    /// most (`yi` the block's integer row in the reference), and the luma
    /// window's last row, `yi + h + 2`, is at most the band's end less
    /// four unless the band is the picture's last, so the chroma window
    /// ends at least two rows above the band's last chroma row, the only
    /// one the next band's filter changes (`docs/design/h264.md`, "Round
    /// 17"); a chroma window above the band's top apron lies outside what
    /// it holds, which [`Kernels::predict_part`] declines.
    ///
    /// A band's view is built at the first block of the slice that reads
    /// the band and kept in [`SliceDecoder::band_views`] for the blocks
    /// after it.
    #[inline(never)]
    fn reference_band(
        &mut self,
        list: usize,
        idx: usize,
        py: usize,
        h: usize,
        mv: [i16; 2],
    ) -> Option<Planes<'a>> {
        let refs: &'a [Vec<RefPic>; 2] = self.refs;
        let reference = &refs.get(list)?.get(idx)?.picture;
        let g = reference.geometry();
        let k = g.band_reading_luma_window(py as isize + isize::from(mv[1] >> 2), h);
        let slot = self
            .band_views
            .get_mut(list)?
            .get_mut((4 * idx + k % 4) % BAND_VIEWS)?;
        if let Some(view) = slot
            && view.idx == idx
            && view.band == k
        {
            return Some(view.planes);
        }
        let height = self.mb_h * 16;
        if reference.coded_width() != self.stride || reference.coded_height() != height {
            return None;
        }
        let data = reference.band(k)?;
        let first = [g.luma_first(k), g.chroma_first(k)];
        let origin = [g.luma_origin(k), g.chroma_origin(k)];
        let planes = Planes::band(
            &data.y,
            &data.cb,
            &data.cr,
            self.stride,
            height,
            first,
            origin,
        )?;
        *slot = Some(BandView {
            idx,
            band: k,
            planes,
        });
        Some(planes)
    }

    /// [`Self::predict_list`] for any block: one [`Self::predict_fast`]
    /// declines, whose window reaches outside the picture or outside the
    /// band it is read from, or a weighted one that needs the scratch
    /// buffers. A banded reference is looked up by band and waited for,
    /// per plane.
    ///
    /// Not marked cold: it still takes a few percent of the blocks of
    /// some streams.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn predict_list_any(
        &mut self,
        list: usize,
        m: &Motion,
        target: Target<'_>,
        px: usize,
        py: usize,
        w: usize,
        h: usize,
    ) -> Result<()> {
        let stride = self.stride;
        let chroma_stride = stride / 2;
        let height = self.mb_h * 16;
        let (off, chroma_off) = self.block_offsets(px, py);
        let idx = usize::try_from(m.ref_idx.get(list).copied().unwrap_or(-1)).unwrap_or(usize::MAX);
        // A reference that is one published band of this picture's
        // size, which every reference is with one thread, was found
        // when the slice started (`SliceDecoder::whole`); any other is
        // looked up block by block, and waited for.
        let whole = self
            .whole
            .get(list)
            .and_then(|l| l.get(idx))
            .copied()
            .flatten();
        let reference = match whole {
            Some(_) => None,
            None => {
                let entry = self
                    .refs
                    .get(list)
                    .and_then(|l| l.get(idx))
                    .ok_or(Error::Invalid("a reference index past its list"))?;
                let reference = &entry.picture;
                if reference.coded_width() != stride || reference.coded_height() != height {
                    return Err(Error::Invalid("a reference picture of another size"));
                }
                Some(reference)
            }
        };
        let mv = m.mv.get(list).copied().unwrap_or([0, 0]);
        let (xf, yf) = ((mv[0] & 3) as u8, (mv[1] & 3) as u8);
        let xi = px as isize + isize::from(mv[0] >> 2);
        let yi = py as isize + isize::from(mv[1] >> 2);
        let scratch = &mut *self.scratch;
        // The window is read from the band a reader takes its last row
        // from, clamped into the picture as Equation 8-235 clamps it: that
        // band's top apron holds the rest of the window. Each plane is
        // taken from its origin, past the slack rows a band holding the
        // picture's first row keeps before it.
        let (luma, first) = match (whole, reference) {
            (Some(planes), _) => (past(planes.y(), planes.origin()[0])?, 0),
            (None, Some(reference)) => {
                let g = reference.geometry();
                let k = g.band_reading_luma_window(yi, h);
                let data = reference
                    .band(k)
                    .ok_or(Error::Geometry("a reference band"))?;
                (past(&data.y, g.luma_origin(k))?, g.luma_first(k))
            }
            (None, None) => return Err(Error::Geometry("a reference band")),
        };
        let source = window(
            luma,
            first,
            stride,
            height,
            xi - 2,
            yi - 2,
            w + 5,
            h + 5,
            &mut scratch.edge,
        )?;
        let dst = if target == Target::Scratch {
            let plane = scratch.luma.get_mut(list).ok_or(Error::Geometry("list"))?;
            BlockMut::new(plane, 0, 16, w, h)?
        } else {
            BlockMut::new(&mut self.out.y, off, stride, w, h)?
        };
        match target.op(0) {
            Op::Put => self.kernels.luma_mc(dst, source, xf, yf),
            Op::Average => self.kernels.luma_mc_avg(dst, source, xf, yf),
            Op::Weigh(weighting) => luma_weigh(self.kernels, dst, source, xf, yf, weighting),
        }
        // Chroma of a 4:2:0 frame: the vector in eighths of a chroma
        // sample is the luma vector (clause 8.4.1.4).
        let (cxf, cyf) = ((mv[0] & 7) as u8, (mv[1] & 7) as u8);
        let cxi = (px / 2) as isize + isize::from(mv[0] >> 3);
        let cyi = (py / 2) as isize + isize::from(mv[1] >> 3);
        let (cw, ch) = (w / 2, h / 2);
        let (chroma, first) = match (whole, reference) {
            (Some(planes), _) => {
                let origin = planes.origin()[1];
                ([past(planes.cb(), origin)?, past(planes.cr(), origin)?], 0)
            }
            (None, Some(reference)) => {
                let g = reference.geometry();
                let k = g.band_reading_chroma_row(clamp_row(cyi + ch as isize, height / 2));
                let data = reference
                    .band(k)
                    .ok_or(Error::Geometry("a reference band"))?;
                let origin = g.chroma_origin(k);
                let planes = [past(&data.cb, origin)?, past(&data.cr, origin)?];
                (planes, g.chroma_first(k))
            }
            (None, None) => return Err(Error::Geometry("a reference band")),
        };
        for (c, plane) in chroma.into_iter().enumerate() {
            let source = window(
                plane,
                first,
                chroma_stride,
                height / 2,
                cxi,
                cyi,
                cw + 1,
                ch + 1,
                &mut scratch.edge,
            )?;
            let dst = if target == Target::Scratch {
                let buffer = scratch
                    .chroma
                    .get_mut(list)
                    .and_then(|l| l.get_mut(c))
                    .ok_or(Error::Geometry("list"))?;
                BlockMut::new(buffer, 0, 8, cw, ch)?
            } else {
                let pic = &mut self.out;
                let out = if c == 0 { &mut pic.cb } else { &mut pic.cr };
                BlockMut::new(out, chroma_off, chroma_stride, cw, ch)?
            };
            match target.op(1 + c) {
                Op::Put => self.kernels.chroma_mc(dst, source, cxf, cyf),
                Op::Average => self.kernels.chroma_mc_avg(dst, source, cxf, cyf),
                Op::Weigh(weighting) => {
                    chroma_weigh(self.kernels, dst, source, cxf, cyf, weighting)
                }
            }
        }
        Ok(())
    }

    /// How weighted sample prediction (clause 8.4.2.3) combines the two
    /// lists' predictions of a block in each plane (luma, Cb, Cr).
    /// `implicit` holds the implicit weights (w0, w1) where they apply.
    ///
    /// `T`, unused, gives each instance of the hot paths its own copy
    /// ([`crate::tier`]).
    fn combine_modes<T: Tier>(
        &self,
        m: &Motion,
        used: [bool; 2],
        explicit: bool,
        implicit: Option<(i32, i32)>,
    ) -> [Combine; 3] {
        // (logWD, w0, w1, o0, o1) of one plane: 0 is luma, 1 is Cb, 2 is Cr.
        let weights = |plane: usize| -> Option<(i32, i32, i32, i32, i32)> {
            if let Some((w0, w1)) = implicit {
                return Some((5, w0, w1, 0, 0));
            }
            if !explicit {
                return None;
            }
            let table = self.hdr.pred_weights.as_ref()?;
            let of = |list: usize| -> (i32, i32) {
                let Ok(r) = usize::try_from(m.ref_idx.get(list).copied().unwrap_or(-1)) else {
                    return (0, 0);
                };
                if plane == 0 {
                    table.luma_weight(list, r)
                } else {
                    table.chroma_weight(list, r, plane - 1)
                }
            };
            let (w0, o0) = of(0);
            let (w1, o1) = of(1);
            let denom = if plane == 0 {
                table.luma_log2_denom
            } else {
                table.chroma_log2_denom
            };
            Some((denom as i32, w0, w1, o0, o1))
        };
        let mode = |weights: Option<(i32, i32, i32, i32, i32)>| -> Combine {
            match (used[0], used[1], weights) {
                (true, true, None) => Combine::Average,
                (true, false, None) => Combine::Copy(0),
                (false, true, None) => Combine::Copy(1),
                (true, true, Some((log_wd, w0, w1, o0, o1))) => Combine::Bi {
                    log_wd,
                    w0,
                    w1,
                    o: (o0 + o1 + 1) >> 1,
                },
                (first, _, Some((log_wd, w0, w1, o0, o1))) => {
                    let (list, w, o) = if first { (0, w0, o0) } else { (1, w1, o1) };
                    Combine::Uni { list, log_wd, w, o }
                }
                (false, false, _) => Combine::Copy(0),
            }
        };
        [mode(weights(0)), mode(weights(1)), mode(weights(2))]
    }

    /// Weighted sample prediction (clause 8.4.2.3) from the predictions in
    /// scratch: turns the two lists' predictions into the block's samples,
    /// each plane as `modes` says.
    ///
    /// `T`, unused, gives each instance of the hot paths its own copy
    /// ([`crate::tier`]).
    fn combine_predictions<T: Tier>(
        &mut self,
        modes: [Combine; 3],
        off: usize,
        chroma_off: usize,
        w: usize,
        h: usize,
    ) -> Result<()> {
        let [luma_mode, cb_mode, cr_mode] = modes;
        let chroma_modes = [cb_mode, cr_mode];
        let stride = self.stride;
        let chroma_stride = stride / 2;
        let scratch = &*self.scratch;
        let pic = &mut self.out;
        let p0 = Block::new(scratch.luma.first().map_or(&[], |p| p), 0, 16, w, h)?;
        let p1 = Block::new(scratch.luma.get(1).map_or(&[], |p| p), 0, 16, w, h)?;
        let dst = BlockMut::new(&mut pic.y, off, stride, w, h)?;
        self.kernels.combine(dst, p0, p1, luma_mode);
        let (cw, ch) = (w / 2, h / 2);
        for c in 0..2 {
            let empty: &[u8] = &[];
            let p0 = Block::new(
                scratch
                    .chroma
                    .first()
                    .and_then(|l| l.get(c))
                    .map_or(empty, |p| p),
                0,
                8,
                cw,
                ch,
            )?;
            let p1 = Block::new(
                scratch
                    .chroma
                    .get(1)
                    .and_then(|l| l.get(c))
                    .map_or(empty, |p| p),
                0,
                8,
                cw,
                ch,
            )?;
            let plane = if c == 0 { &mut pic.cb } else { &mut pic.cr };
            let dst = BlockMut::new(plane, chroma_off, chroma_stride, cw, ch)?;
            let mode = chroma_modes.get(c).copied().unwrap_or(Combine::Copy(0));
            self.kernels.combine(dst, p0, p1, mode);
        }
        Ok(())
    }
}

/// Clip3(-128, 127, DiffPicOrderCnt(a, b)) (Equations 8-197 and 8-198),
/// in 64 bits: a stream may carry picture order counts across the whole
/// 32-bit range, whose difference a 32-bit subtraction would overflow.
fn poc_distance(a: i32, b: i32) -> i32 {
    (i64::from(a) - i64::from(b)).clamp(-128, 127) as i32
}

/// Whether a prediction mode uses `list`.
fn uses(mode: PredMode, list: usize) -> bool {
    matches!(
        (mode, list),
        (PredMode::Bi, _) | (PredMode::L0, 0) | (PredMode::L1, 1)
    )
}

/// The co-located motion of a block (clause 8.4.1.2.1) from its stored
/// motion `m`: the vector, the reference index and the slot of the picture
/// it refers to, of list 0 where it uses list 0, else of list 1; an intra
/// block gives a zero vector and -1.
#[inline(always)]
fn colocated(m: &Motion) -> ([i16; 2], i8, u8) {
    for list in 0..2 {
        let ref_idx = m.ref_idx.get(list).copied().unwrap_or(-1);
        if ref_idx >= 0 {
            return (
                m.mv.get(list).copied().unwrap_or([0, 0]),
                ref_idx,
                m.ref_slot.get(list).copied().unwrap_or(NO_SLOT),
            );
        }
    }
    ([0, 0], -1, NO_SLOT)
}

/// mvLX = mvpLX + mvdLX (Equation 8-174).
fn add_mvd(mvp: [i16; 2], mvd: [i32; 2]) -> Result<[i16; 2]> {
    to_mv([i32::from(mvp[0]) + mvd[0], i32::from(mvp[1]) + mvd[1]])
}

/// A motion vector that fits the range of clause 7.4.5.1.
fn to_mv(v: [i32; 2]) -> Result<[i16; 2]> {
    let one = |x: i32| i16::try_from(x).map_err(|_| Error::Invalid("a motion vector out of range"));
    Ok([one(v[0])?, one(v[1])?])
}

/// Row `y` of a plane `rows` high, clamped into the plane as Equations
/// 8-235 and 8-243 clamp it.
#[inline]
fn clamp_row(y: isize, rows: usize) -> usize {
    y.clamp(0, rows.saturating_sub(1) as isize) as usize
}

/// A plane's samples from its origin on: past the slack rows a picture
/// of one band keeps before each plane (`Geometry::luma_origin`).
#[inline]
fn past(plane: &[u8], origin: usize) -> Result<&[u8]> {
    plane
        .get(origin..)
        .ok_or(Error::Geometry("a reference plane shorter than its origin"))
}

/// The `w` by `h` window at (x, y) of a plane `pw` by `ph` as a block,
/// with sample positions outside the picture taken from its edges
/// (Equations 8-233 to 8-236).
///
/// `plane` holds the rows of the plane from row `first` on: the band a
/// reader takes the window's last row from, clamped, whose top apron
/// holds the rest of the window. The block is the band's own samples where the
/// window is inside the picture, which is almost every window, and a copy
/// in `edge` where it is not ([`edge_window`]). A band that does not hold a
/// row of the window is [`Error::Geometry`], a bug in the decoder, not a
/// property of the stream.
///
/// Inlined, so the common case costs one range test and one block view;
/// as a call, three per predicted block, it cost more instructions than
/// the luma interpolation on streams of still macroblocks
/// (`docs/design/h264.md`, "Round 6").
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn window<'b>(
    plane: &'b [u8],
    first: usize,
    pw: usize,
    ph: usize,
    x: isize,
    y: isize,
    w: usize,
    h: usize,
    edge: &'b mut [[u8; EDGE_STRIDE]],
) -> Result<Block<'b>> {
    if x >= 0
        && y >= 0
        && (x as usize).saturating_add(w) <= pw
        && (y as usize).saturating_add(h) <= ph
    {
        let row = (y as usize)
            .checked_sub(first)
            .ok_or(Error::Geometry("a reference window outside its band"))?;
        let start = row.wrapping_mul(pw).wrapping_add(x as usize);
        return Ok(Block::new(plane, start, pw, w, h)?);
    }
    let (samples, stride) = edge_window(plane, first, pw, ph, x, y, w, h, edge)?;
    Ok(Block::new(samples, 0, stride, w, h)?)
}

/// The stride of [`edge_window`]'s copy. A window is at most 21 samples
/// wide (a 16-wide luma block and its five interpolation taps), so a row
/// of the copy is built with moves of this fixed size.
pub(crate) const EDGE_STRIDE: usize = 32;

/// The window of [`window`] that reaches outside the picture, copied into
/// `edge` a row at a time: the part inside the picture, and the edge
/// columns repeated left and right of it (Equations 8-233 to 8-236).
/// Returns the copy and its stride.
///
/// Every row takes its columns from the same positions, so a row is one
/// 32-sample load where the window is inside the picture across, and
/// otherwise 32 samples of the row's end beside 32 copies of its edge
/// sample, read back from where the window starts. Moves of a fixed size
/// stay inline, where a copy of a length known only at run time is a
/// call to `memcpy` and two to `memset` a row. A plane narrower than 32
/// samples takes [`narrow_window`].
#[allow(clippy::too_many_arguments)]
#[cold]
#[inline(never)]
fn edge_window<'b>(
    plane: &'b [u8],
    first: usize,
    pw: usize,
    ph: usize,
    x: isize,
    y: isize,
    w: usize,
    h: usize,
    edge: &'b mut [[u8; EDGE_STRIDE]],
) -> Result<(&'b [u8], usize)> {
    if pw < EDGE_STRIDE || w > EDGE_STRIDE {
        return narrow_window(plane, first, pw, ph, x, y, w, h, edge.as_flattened_mut());
    }
    let outside = Error::Geometry("a reference window outside its band");
    // Columns left of the picture repeat its first; a window starting 32 or
    // more to its left is the first column throughout, and one starting at
    // or past its right edge the last.
    let x = x.clamp(-(EDGE_STRIDE as isize), pw as isize);
    let inside = x >= 0 && x as usize + EDGE_STRIDE <= pw;
    // Where the 32 samples read back start, in `both` below.
    let (end, start) = if x < 0 {
        (0, (EDGE_STRIDE as isize + x) as usize)
    } else {
        let end = pw - EDGE_STRIDE;
        (end, (x as usize).saturating_sub(end))
    };
    let rows = edge.get_mut(..h).ok_or(outside)?;
    for (r, out) in rows.iter_mut().enumerate() {
        let row_y = (y + r as isize).clamp(0, ph.saturating_sub(1) as isize) as usize;
        let at = row_y.wrapping_sub(first).wrapping_mul(pw);
        let row = plane.get(at..at.wrapping_add(pw)).ok_or(outside)?;
        let take = |from: usize| -> Option<&[u8; EDGE_STRIDE]> {
            row.get(from..from + EDGE_STRIDE)?.try_into().ok()
        };
        if inside {
            *out = *take(x as usize).ok_or(outside)?;
            continue;
        }
        let samples = *take(end).ok_or(outside)?;
        let both = if x < 0 {
            [[row.first().copied().unwrap_or(0); EDGE_STRIDE], samples]
        } else {
            [samples, [row.last().copied().unwrap_or(0); EDGE_STRIDE]]
        };
        let from: &[u8; EDGE_STRIDE] = both
            .as_flattened()
            .get(start..start + EDGE_STRIDE)
            .and_then(|s| s.try_into().ok())
            .ok_or(outside)?;
        *out = *from;
    }
    Ok((edge.as_flattened(), EDGE_STRIDE))
}

/// [`edge_window`] for a plane narrower than [`EDGE_STRIDE`]: each row is
/// copied in three parts, at the window's own width.
#[allow(clippy::too_many_arguments)]
#[cold]
#[inline(never)]
fn narrow_window<'b>(
    plane: &'b [u8],
    first: usize,
    pw: usize,
    ph: usize,
    x: isize,
    y: isize,
    w: usize,
    h: usize,
    edge: &'b mut [u8],
) -> Result<(&'b [u8], usize)> {
    let outside = Error::Geometry("a reference window outside its band");
    // Per row: the part inside the picture is copied and the samples left
    // and right of it repeat its edge columns.
    let x0 = x.clamp(0, pw as isize) as usize;
    let x1 = (x + w as isize).clamp(0, pw as isize) as usize;
    for (r, out) in edge.chunks_exact_mut(w.max(1)).take(h).enumerate() {
        let row_y = (y + r as isize).clamp(0, ph.saturating_sub(1) as isize) as usize;
        let at = row_y.wrapping_sub(first).wrapping_mul(pw);
        let row = plane.get(at..at.wrapping_add(pw)).ok_or(outside)?;
        let left = (x0 as isize - x).clamp(0, w as isize) as usize;
        let middle = x1.saturating_sub(x0).min(w - left);
        let first = row.first().copied().unwrap_or(0);
        let last = row.last().copied().unwrap_or(0);
        if let Some(part) = out.get_mut(..left) {
            part.fill(first);
        }
        if let (Some(part), Some(from)) =
            (out.get_mut(left..left + middle), row.get(x0..x0 + middle))
        {
            part.copy_from_slice(from);
        }
        if let Some(part) = out.get_mut(left + middle..) {
            part.fill(last);
        }
    }
    Ok((edge.get(..w * h).unwrap_or(edge), w))
}

/// The `w` by `h` block at (`x`, `y`) of a macroblock, checked
/// ([`Part::new`]) where it is made: the callers' sizes are mostly
/// constants, for which the check folds away.
#[inline(always)]
pub(crate) fn part(x: usize, y: usize, w: usize, h: usize) -> Result<Part> {
    Part::new(x, y, w, h).ok_or(Error::Invalid("a partition's size"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reference_index_of_full_lists_keeps_its_picture() {
        // Lists of 32 entries each, the most a slice can name (ref_idx 0 to
        // 31: num_ref_idx_lX_active_minus1 up to 31 for a field, clause
        // 7.4.3; a frame takes 16, and the decoder refuses fields), each
        // entry a picture of its own slot, a long-term one among them: the
        // slot table that `resolve` and `temporal_direct_motion` read keeps
        // every entry's slot, and a block's twelve-byte motion holds any of
        // those indices with its slot.
        let limits = crate::limits::Limits::default();
        let picture = |slot: u8| {
            std::sync::Arc::new(
                crate::picture::Picture::black(1, 1, &limits)
                    .expect("a picture")
                    .with_output(i32::from(slot), [0; 4], slot, 0),
            )
        };
        let list = |base: u8| -> Vec<RefPic> {
            (0..32u8)
                .map(|i| RefPic {
                    picture: picture(base + i),
                    long_term: i == 31,
                })
                .collect()
        };
        let refs = [list(0), list(200)];
        let pps = Pps {
            sps_id: 0,
            cabac: true,
            bottom_field_pic_order_in_frame_present: false,
            num_slice_groups: 1,
            slice_group_map_type: 0,
            num_ref_idx_default_active: [32, 32],
            weighted_pred: false,
            weighted_bipred_idc: 0,
            pic_init_qp: 26,
            chroma_qp_index_offset: [0, 0],
            deblocking_filter_control_present: false,
            constrained_intra_pred: false,
            redundant_pic_cnt_present: false,
            transform_8x8_mode: false,
            scaling4x4: [[16; 16]; 6],
            scaling8x8: [[16; 64]; 6],
        };
        let tables = InterTables::new(&refs, SliceType::B, &pps, 1000);
        assert_eq!(tables.count, [32, 32]);
        for (list, base) in [(0usize, 0u8), (1, 200)] {
            for i in 0..32u8 {
                let slot = tables.slots[list][usize::from(i)];
                assert_eq!(slot, base + i);
                let m = Motion {
                    mv: [[-2048, 8191], [2047, -8192]],
                    ref_idx: [i as i8; 2],
                    ref_slot: [slot; 2],
                };
                assert_eq!((m.ref_idx[list], m.ref_slot[list]), (i as i8, base + i));
            }
        }
        assert_eq!(size_of::<Motion>(), 12);
    }

    /// Sample (i, j) of the window at (x, y), by Equations 8-233 to 8-236
    /// read directly: each coordinate clamped into the plane.
    fn clamped(plane: &[u8], pw: usize, ph: usize, x: isize, y: isize) -> u8 {
        let cx = x.clamp(0, pw as isize - 1) as usize;
        let cy = y.clamp(0, ph as isize - 1) as usize;
        plane[cy * pw + cx]
    }

    #[test]
    fn edge_windows_repeat_the_picture_edges() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _ in 0..20_000 {
            // Planes from narrower than the copy's stride to wider.
            let pw = 8 + next(80) as usize;
            let ph = 8 + next(40) as usize;
            let plane: Vec<u8> = (0..pw * ph).map(|_| next(256) as u8).collect();
            let (w, h) = if next(2) == 0 {
                (
                    4 * (1 + next(4) as usize) + 5,
                    4 * (1 + next(4) as usize) + 5,
                )
            } else {
                (
                    2 * (1 + next(4) as usize) + 1,
                    2 * (1 + next(4) as usize) + 1,
                )
            };
            let x = next(pw as u64 + 80) as isize - 40;
            let y = next(ph as u64 + 80) as isize - 40;
            let mut edge = [[0u8; EDGE_STRIDE]; 21];
            let block = window(&plane, 0, pw, ph, x, y, w, h, &mut edge).expect("window");
            for j in 0..h {
                for i in 0..w {
                    assert_eq!(
                        block.row(j).expect("row")[i],
                        clamped(&plane, pw, ph, x + i as isize, y + j as isize),
                        "{pw}x{ph} window {w}x{h} at ({x}, {y}), sample ({i}, {j})"
                    );
                }
            }
        }
    }
}
