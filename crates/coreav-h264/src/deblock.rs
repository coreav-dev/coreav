//! The deblocking filter (clause 8.7) for frame pictures: boundary
//! strengths, thresholds, and the edges each macroblock is filtered on.
#![forbid(unsafe_code)]

use std::ops::Range;

use coreav_kernels_h264::{
    ChromaMbMut, EdgeMut, EdgeParams, Kernels, LumaMbMut, Motions, Reach, StrengthMb, StrengthSide,
    uniform_strengths,
};

use crate::error::Result;
use crate::mb::{MbInfo, MbKind};
use crate::picture::{BandData, Canvas, MbHead, Motion};
use crate::tables as t;

/// The filter control of one slice (clause 7.4.3).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SliceFilter {
    pub(crate) disable_idc: u32,
    pub(crate) offset_a: i32,
    pub(crate) offset_b: i32,
}

/// Motion stands in for a macroblock whose motion lies outside the
/// band's store, as `Motion::default()` did before; a well-formed picture
/// never reaches it.
const UNSTORED: MbHead = MbHead {
    m: Motion {
        mv: [[0; 2]; 2],
        ref_idx: [0; 2],
        ref_slot: [0; 2],
    },
    split: false,
};
const UNSTORED_BLOCKS: [Motion; 16] = [UNSTORED.m; 16];

/// The boundary strengths of a macroblock's luma edges
/// ([`Kernels::strengths`]): per direction (vertical 0, horizontal 1) and
/// edge 0 to 3, the four segments' bS in the bytes of a word, segment 0 in
/// the low byte (clause 8.7.2.1).
///
/// The macroblock and its usable neighbours go to the kernels as their
/// records give them. A macroblock that carries one motion is read as its
/// head ([`MbHead`]): its sixteen entries in the band's motion are stale
/// and never read.
#[inline(always)]
fn strengths<K: Kernels>(
    kernels: &K,
    band: &BandData,
    base: usize,
    mbs: &[MbInfo],
    q: (usize, &MbInfo),
    left: Option<usize>,
    top: Option<usize>,
) -> [[u32; 4]; 2] {
    // The band's motion and heads start at macroblock `base`: its first
    // stored row is the one above its own (`picture.rs`).
    let side = |a: usize, info: &MbInfo| {
        let at = a.wrapping_sub(base);
        let head = band.heads.get(at);
        debug_assert!(head.is_none_or(|h| h.split != info.uniform));
        let motion = if info.uniform {
            Motions::One(&head.unwrap_or(&UNSTORED).m)
        } else {
            let at = at.wrapping_mul(16);
            Motions::Each(
                band.motion
                    .get(at..at.wrapping_add(16))
                    .and_then(|m| <&[Motion; 16]>::try_from(m).ok())
                    .unwrap_or(&UNSTORED_BLOCKS),
            )
        };
        StrengthSide {
            motion,
            nz: info.nz,
            intra: info.kind.is_intra(),
        }
    };
    let info = |n: Option<usize>| n.map(|a| mbs.get(a).unwrap_or(&MbInfo::NONE));
    let (left_info, top_info) = (info(left), info(top));
    // Most macroblocks and their neighbours carry one motion each: their
    // derivation is inlined here, from the heads, and only the others go
    // to the kernels.
    let uniform = |m: Option<&MbInfo>| m.is_none_or(|m| m.uniform);
    if q.1.uniform && uniform(left_info) && uniform(top_info) {
        let one = |a: usize, info: &MbInfo| {
            let head = band.heads.get(a.wrapping_sub(base));
            debug_assert!(head.is_none_or(|h| h.split != info.uniform));
            StrengthSide {
                motion: Motions::One(&head.unwrap_or(&UNSTORED).m),
                nz: info.nz,
                intra: info.kind.is_intra(),
            }
        };
        return uniform_strengths(&StrengthMb {
            q: one(q.0, q.1),
            left: left.zip(left_info).map(|(a, m)| one(a, m)),
            top: top.zip(top_info).map(|(a, m)| one(a, m)),
            t8x8: q.1.t8x8,
        });
    }
    kernels.strengths(&StrengthMb {
        q: side(q.0, q.1),
        left: left.zip(left_info).map(|(a, m)| side(a, m)),
        top: top.zip(top_info).map(|(a, m)| side(a, m)),
        t8x8: q.1.t8x8,
    })
}

/// QPY of a macroblock for the filter: an I_PCM macroblock filters as if
/// its QPY were 0 (clause 8.7.2).
fn luma_qp(m: &MbInfo) -> i32 {
    if m.kind == MbKind::IPcm {
        0
    } else {
        i32::from(m.qp)
    }
}

/// QPC of a macroblock (clause 8.7.2), for a chroma QP offset as
/// [`shifted`] gives it.
#[inline(always)]
fn chroma_qp(m: &MbInfo, offset: usize) -> i32 {
    qpc(luma_qp(m), offset)
}

/// QPC of a QPY (clause 8.7.2), for a chroma QP offset as [`shifted`]
/// gives it.
#[inline(always)]
fn qpc(qp: i32, offset: usize) -> i32 {
    let index = (qp as usize).wrapping_add(offset) & 0xff;
    QPC_SHIFTED.get(index).copied().map_or(0, i32::from)
}

/// The lowest QPY at which an edge of a macroblock can filter a sample,
/// for a slice's filter offsets and the picture's chroma QP offsets
/// ([`shifted`]); 52 where no QPY can.
///
/// A macroblock whose QPY and whose neighbours' (left and top, where
/// filtered across) are all below it filters nothing, whatever bS:
/// - every edge's thresholds come from the average of two of those QPs,
///   which is at most their maximum, or for chroma from the average of
///   their QPC, which Table 8-15 makes at most the maximum's QPC;
/// - α' and β' do not decrease with indexA and indexB (Table 8-16), so an
///   edge's thresholds filter only if the maximum's do;
/// - with α' or β' 0, no sample passes filterSamplesFlag (Equation 8-468,
///   [`Thresholds::new`]).
///
/// Four lookups: the lowest average QP whose thresholds filter, the larger
/// of [`FIRST_ALPHA`]'s and [`FIRST_BETA`]'s, and for each chroma plane the
/// lowest QPY whose QPC reaches that average ([`FIRST_QPC`]).
#[inline(always)]
fn first_live_qp(offsets: Offsets, chroma_offsets: [usize; 2]) -> i32 {
    let first = |table: &[u8; 256], i: usize| table.get(i & 0xff).copied().map_or(52, i32::from);
    let average = first(&FIRST_ALPHA, offsets.a).max(first(&FIRST_BETA, offsets.b));
    // The lowest index of `QPC_SHIFTED` whose QPC is at least `average`,
    // less the plane's shifted offset: QPY + offset is that index.
    let reach = first(&FIRST_QPC, average as usize);
    chroma_offsets
        .iter()
        .map(|&c| (reach - c as i32).max(0))
        .fold(average, i32::min)
        .min(52)
}

/// By a shifted FilterOffsetA ([`shifted`]), the lowest average QP (0 to
/// 51) whose α' is not 0, or 52.
static FIRST_ALPHA: [u8; 256] = first_nonzero(&t::ALPHA);

/// By a shifted FilterOffsetB, the lowest average QP whose β' is not 0, or
/// 52.
static FIRST_BETA: [u8; 256] = first_nonzero(&t::BETA);

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn first_nonzero(table: &[u8; 52]) -> [u8; 256] {
    let mut out = [52u8; 256];
    let mut shift = 0;
    while shift < 256 {
        let mut average = 0;
        while average < 52 {
            if table[clipped_row(average + shift)] != 0 {
                out[shift] = average as u8;
                break;
            }
            average += 1;
        }
        shift += 1;
    }
    out
}

/// By a QP of 0 to 52, the lowest index of [`QPC_SHIFTED`] whose QPC is at
/// least that QP, or 255 where none is (a QP above 39, the largest QPC).
static FIRST_QPC: [u8; 256] = first_qpc();

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn first_qpc() -> [u8; 256] {
    let mut out = [255u8; 256];
    let mut qp = 0;
    while qp < 256 {
        let mut i = 0;
        while i < 256 {
            if t::QPC[clipped_row(i)] as usize >= qp {
                out[qp] = i as u8;
                break;
            }
            i += 1;
        }
        qp += 1;
    }
    out
}

/// An offset added to a QP before Equations 8-461 and 8-462 (FilterOffsetA
/// or FilterOffsetB) or Equation 8-313 (a chroma QP offset) clip the sum
/// to 0 to 51, as the tables below take it: clamped to −51 to 51, then
/// plus 51.
///
/// The tables hold the clip. A QP of 0 to 51 plus an offset of −51 to 51
/// plus 51 is 0 to 153, and entry `i` holds the row of `i − 51` clipped to
/// 0 to 51. Clamping the offset first changes nothing: with a QP of 0 to
/// 51, any offset at or below −51 clips to row 0 and any at or above 51 to
/// row 51. Every QP the filter reads is 0 to 51 (QPY of an 8-bit picture,
/// the average of two, or QPC); the tables have 256 entries so that an
/// index masked to a byte needs no bounds check.
fn shifted(offset: i32) -> usize {
    (offset.clamp(-51, 51) + 51) as usize
}

/// Row `i − 51` of a table of 52 rows, clipped to 0 to 51.
const fn clipped_row(i: usize) -> usize {
    if i < 51 {
        0
    } else if i - 51 > 51 {
        51
    } else {
        i - 51
    }
}

/// Table 8-15's QPC by qPI + 51 ([`shifted`]).
static QPC_SHIFTED: [u8; 256] = qpc_shifted();

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn qpc_shifted() -> [u8; 256] {
    let mut out = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        out[i] = t::QPC[clipped_row(i)];
        i += 1;
    }
    out
}

/// Table 8-16's α' and Table 8-17's t'C0 by indexA + 51 ([`shifted`]): α'
/// in the low byte, then t'C0 for bS 1, 2 and 3 in the next three.
static BY_INDEX_A: [u32; 256] = by_index_a();

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn by_index_a() -> [u32; 256] {
    let mut out = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let row = clipped_row(i);
        let [t1, t2, t3] = t::TC0[row];
        out[i] = u32::from_le_bytes([t::ALPHA[row], t1, t2, t3]);
        i += 1;
    }
    out
}

/// Table 8-16's β' by indexB + 51 ([`shifted`]).
static BY_INDEX_B: [u8; 256] = by_index_b();

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn by_index_b() -> [u8; 256] {
    let mut out = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        out[i] = t::BETA[clipped_row(i)];
        i += 1;
    }
    out
}

/// A slice's FilterOffsetA and FilterOffsetB as the tables take them
/// ([`shifted`]).
#[derive(Clone, Copy)]
struct Offsets {
    a: usize,
    b: usize,
}

impl Offsets {
    fn new(f: &SliceFilter) -> Offsets {
        Offsets {
            a: shifted(f.offset_a),
            b: shifted(f.offset_b),
        }
    }
}

/// The thresholds of the edges between two macroblocks, by their QPs
/// (clause 8.7.2.2, Tables 8-16 and 8-17), in the kernels' form: an
/// edge's [`EdgeParams`] are these with its bS beside them.
#[derive(Clone, Copy)]
struct Thresholds {
    alpha: u8,
    beta: u8,
    /// t'C0 by bS, entry `b` for bS = `b`: Table 8-17's row for indexA at
    /// bS 1 to 3 (each at most 25), and 0 at bS 0 and 4 to 7, where it is
    /// not used.
    tc0: [u8; 8],
}

impl Thresholds {
    /// The thresholds of an edge that filters nothing.
    const NONE: Thresholds = Thresholds {
        alpha: 0,
        beta: 0,
        tc0: [0; 8],
    };

    /// The thresholds of an edge between QPs `qp_p` and `qp_q`. Where α'
    /// or β' is 0, `alpha` is 0 ([`Thresholds::filters`]): no sample of
    /// such an edge is filtered, since filterSamplesFlag needs
    /// |p0 − q0| < α and |p1 − p0| < β (Equation 8-468), whatever bS, and
    /// α = 0 alone says so to the kernels.
    ///
    /// Two lookups, without a clip, a bounds check or a branch: the tables
    /// are indexed by the average QP plus the shifted offsets
    /// ([`shifted`]).
    #[cfg(test)]
    fn new(qp_p: i32, qp_q: i32, offsets: Offsets) -> Thresholds {
        Thresholds::of_average(((qp_p + qp_q + 1) >> 1) as usize, offsets)
    }

    /// [`Thresholds::new`] by the average of the two QPs.
    #[inline(always)]
    fn of_average(average: usize, offsets: Offsets) -> Thresholds {
        let by_a = BY_INDEX_A
            .get(average.wrapping_add(offsets.a) & 0xff)
            .copied()
            .unwrap_or(0);
        let beta = BY_INDEX_B
            .get(average.wrapping_add(offsets.b) & 0xff)
            .copied()
            .unwrap_or(0);
        let alpha = (by_a & 0xff) as u8 * u8::from(beta != 0);
        Thresholds {
            alpha,
            beta,
            // t'C0 for bS 1 to 3 to bytes 1 to 3; bytes 0 and 4 to 7 are 0.
            tc0: u64::from(by_a & !0xff).to_le_bytes(),
        }
    }

    /// Whether any sample of an edge with these thresholds can be filtered.
    #[inline(always)]
    fn filters(&self) -> bool {
        self.alpha != 0
    }

    /// The kernel's parameters for an edge whose segments' bS are the
    /// bytes of `bs` (segment 0 in the low byte): these thresholds as they
    /// are, the kernel picking each segment's t'C0 by its bS.
    #[inline(always)]
    fn params(&self, bs: u32) -> EdgeParams {
        EdgeParams {
            alpha: self.alpha,
            beta: self.beta,
            tc0: self.tc0,
            bs: bs.to_le_bytes(),
        }
    }
}

/// Calls `filter` with the direction (0 vertical, 1 horizontal), the
/// index and the kernel's parameters of each luma edge of a macroblock
/// that can filter a sample, in the order clause 8.7 filters them.
/// `neighbours` are the left and the top macroblock where the edges to
/// them are filtered.
///
/// An edge without a neighbour, or an odd edge of a macroblock with the
/// 8x8 transform, has bS = 0 in `bs` and is passed over with the edges
/// whose segments are all 0.
#[inline(always)]
fn luma_edges(
    bs: &[[u32; 4]; 2],
    neighbours: [Option<&MbInfo>; 2],
    qp_q: i32,
    table: &ThresholdTable,
    mut filter: impl FnMut(usize, usize, &EdgeParams) -> Result<()>,
) -> Result<()> {
    let inside = table.get(qp_q, qp_q);
    for ((dir, edges), neighbour) in bs.iter().enumerate().zip(neighbours) {
        for (e, &strength) in edges.iter().enumerate() {
            if strength == 0 {
                continue;
            }
            let thresholds = if e == 0 {
                table.get(neighbour.map_or(qp_q, luma_qp), qp_q)
            } else {
                inside
            };
            if thresholds.filters() {
                filter(dir, e, &thresholds.params(strength))?;
            }
        }
    }
    Ok(())
}

/// The luma edges of a macroblock whose window does not fit in the plane,
/// each checked on its own (`deblock_row`). Out of line and cold: it runs
/// on the first macroblock of a picture of one band, the first row of the
/// first band of a picture of several, and a picture one macroblock wide,
/// and inlined it made the filter's loop a quarter larger.
#[cold]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn luma_by_edges<K: Kernels>(
    kernels: &K,
    y: &mut [u8],
    at: usize,
    stride: usize,
    bs: &[[u32; 4]; 2],
    neighbours: [Option<&MbInfo>; 2],
    qp_q: i32,
    table: &ThresholdTable,
) -> Result<()> {
    luma_edges(bs, neighbours, qp_q, table, |dir, e, params| {
        if dir == 0 {
            let edge = EdgeMut::new(y, at + e * 4, 1, stride, 16, Reach::Luma)?;
            kernels.filter_edge(edge, params);
        } else {
            let edge = EdgeMut::new(y, at + e * 4 * stride, stride, 1, 16, Reach::Luma)?;
            kernels.filter_edge(edge, params);
        }
        Ok(())
    })
}

/// The chroma edges of a macroblock whose windows do not fit in the
/// planes, an edge of both planes at a time (`deblock_row`); cold, as
/// [`luma_by_edges`].
#[cold]
#[inline(never)]
fn chroma_by_edges<K: Kernels>(
    kernels: &K,
    cb: &mut [u8],
    cr: &mut [u8],
    at: usize,
    stride: usize,
    slots: &[[EdgeParams; 2]; 4],
) -> Result<()> {
    for (slot, params) in slots.iter().enumerate() {
        if params.iter().all(|p| p.bs == [0; 4]) {
            continue;
        }
        let (q0, across, along) = match slot {
            0 => (at, 1, stride),
            1 => (at + 4, 1, stride),
            2 => (at, stride, 1),
            _ => (at + 4 * stride, stride, 1),
        };
        let cb = EdgeMut::new(cb, q0, across, along, 8, Reach::Chroma)?;
        let cr = EdgeMut::new(cr, q0, across, along, 8, Reach::Chroma)?;
        kernels.filter_chroma_pair(cb, cr, params);
    }
    Ok(())
}

/// The thresholds of every edge of the slices that share one pair of
/// filter offsets, by the average of the edge's two QPs: what
/// [`Thresholds::new`] gives, built once while consecutive macroblocks
/// keep the offsets, so that an edge takes one lookup.
///
/// 64 entries, so that an average masked to six bits needs no bounds
/// check; every average the filter forms is 0 to 51 (two QPY of an 8-bit
/// picture, or two QPC), and entries 52 to 63 hold what
/// [`Thresholds::new`] gives for those averages.
pub(crate) struct ThresholdTable {
    /// FilterOffsetA and FilterOffsetB as the slice header gives them.
    key: (i32, i32),
    by_average: [Thresholds; 64],
}

impl ThresholdTable {
    /// A table that every pair of offsets rebuilds ([`ThresholdTable::set`]),
    /// on the heap, where a decoder keeps it.
    pub(crate) fn boxed() -> Box<ThresholdTable> {
        Box::new(ThresholdTable::new())
    }

    /// A table that every pair of offsets rebuilds ([`ThresholdTable::set`]).
    fn new() -> ThresholdTable {
        ThresholdTable {
            key: (i32::MIN, i32::MIN),
            by_average: [Thresholds::NONE; 64],
        }
    }

    /// Holds the thresholds of the offsets of `f`, building them where the
    /// table holds another pair.
    #[inline(always)]
    fn set(&mut self, f: &SliceFilter) {
        let key = (f.offset_a, f.offset_b);
        if key != self.key {
            self.build(f);
        }
    }

    #[inline(never)]
    fn build(&mut self, f: &SliceFilter) {
        let offsets = Offsets::new(f);
        for (average, t) in self.by_average.iter_mut().enumerate() {
            *t = Thresholds::of_average(average, offsets);
        }
        self.key = (f.offset_a, f.offset_b);
    }

    /// [`Thresholds::new`] of `qp_p` and `qp_q`, for 0 to 51 each.
    #[inline(always)]
    fn get(&self, qp_p: i32, qp_q: i32) -> Thresholds {
        let average = ((qp_p + qp_q + 1) >> 1) as usize & 63;
        self.by_average
            .get(average)
            .copied()
            .unwrap_or(Thresholds::NONE)
    }
}

/// Filters the macroblock rows `rows` of a picture, in macroblock address
/// order (clause 8.7), and publishes the bands that are then final.
///
/// The decoder runs this one row behind decoding: the filter of a row
/// touches only that row and the three sample rows above it, and only the
/// next row's intra prediction reads the row's unfiltered samples, so
/// filtering a row once the next one is decoded gives the same picture as
/// filtering at the end of the picture. This holds because macroblocks
/// are decoded in increasing address order (`docs/design/h264.md`, "Bit-exact
/// whatever the thread count"). The last row of a band is filtered as soon
/// as decoding enters the next band, whose top apron has taken the
/// unfiltered rows its intra prediction reads: the band above is then
/// published a row sooner.
///
/// Each row is filtered in its own band. The first row of a band reads
/// four rows above its top edge and writes three, which are the band
/// above's: its top apron is refreshed first and the filter writes the
/// three rows there, where every reader takes them from (`picture.rs`),
/// so what is read is the result of filtering one plane.
///
/// A macroblock no slice covered is filled with grey here, when its row is
/// filtered: the filter neither filters nor reads it, so this gives the
/// picture filling it at the end would.
pub(crate) fn deblock_rows<K: Kernels>(
    kernels: &K,
    canvas: &mut Canvas,
    mbs: &[MbInfo],
    filters: &[SliceFilter],
    chroma_qp_offset: [i32; 2],
    table: &mut ThresholdTable,
    rows: Range<usize>,
) -> Result<()> {
    let g = canvas.geometry();
    for row in rows.start..rows.end.min(g.mb_height) {
        let k = g.band_of_mb_row(row);
        let first_of_band = k > 0 && g.motion_first(k) + 1 == row;
        if first_of_band {
            canvas.refresh_top(k)?;
        }
        deblock_row(kernels, canvas, mbs, filters, chroma_qp_offset, table, row)?;
        canvas.publish_final(row + 1)?;
    }
    Ok(())
}

/// Filters macroblock row `row`, which lies in one band.
fn deblock_row<K: Kernels>(
    kernels: &K,
    canvas: &mut Canvas,
    mbs: &[MbInfo],
    filters: &[SliceFilter],
    chroma_qp_offset: [i32; 2],
    table: &mut ThresholdTable,
    row: usize,
) -> Result<()> {
    let g = canvas.geometry();
    let mb_w = g.mb_width;
    let stride = mb_w * 16;
    let chroma_stride = stride / 2;
    let k = g.band_of_mb_row(row);
    // Where the row's first macroblock starts in the band's planes, one
    // offset a row: offsets are the band's, whose first stored rows are its
    // top apron, and in a band holding the picture's first row its slack
    // rows come before them.
    let luma_row = g.luma_origin(k) + (row * 16 - g.luma_first(k)) * stride;
    let chroma_row = g.chroma_origin(k) + (row * 8 - g.chroma_first(k)) * chroma_stride;
    let motion_base = g.motion_first(k) * mb_w;
    let chroma_offsets = [0, 1].map(|c| shifted(chroma_qp_offset.get(c).copied().unwrap_or(0)));
    let pic = canvas.band_mut(k)?;
    // The slice whose `first_live_qp` is held, and that QP.
    let mut live = (0, 0);
    for addr in row * mb_w..(row + 1) * mb_w {
        let q = mbs.get(addr).unwrap_or(&MbInfo::NONE);
        if q.slice == 0 {
            // No slice covered it: the picture is reused without being
            // cleared, so it takes the grey `Picture::black` leaves.
            pic.clear_macroblock(&g, k, addr % mb_w, row);
            continue;
        }
        let f = filters
            .get(q.slice as usize - 1)
            .copied()
            .unwrap_or_default();
        if f.disable_idc == 1 {
            continue;
        }
        let (mbx, mby) = (addr % mb_w, row);
        // A neighbour in another slice is filtered across only where
        // disable_deblocking_filter_idc is 0.
        let usable = |a: usize| {
            mbs.get(a)
                .is_some_and(|m| m.slice != 0 && (f.disable_idc != 2 || m.slice == q.slice))
        };
        let left = (mbx > 0).then(|| addr - 1).filter(|&a| usable(a));
        let top = (mby > 0).then(|| addr - mb_w).filter(|&a| usable(a));
        // A macroblock coded at a QP so low that no edge's thresholds filter
        // a sample, nor its neighbours' across edge 0, needs neither its
        // strengths nor its edges: a picture coded at high quality has
        // most of its macroblocks so (`docs/design/h264.md`, "Deblocking
        // round: macroblocks below the thresholds").
        if live.0 != q.slice {
            live = (q.slice, first_live_qp(Offsets::new(&f), chroma_offsets));
        }
        // The QPs as coded: QPY is 0 to 51 (8-bit pictures only), so an
        // I_PCM macroblock's is at least the 0 the filter takes for it
        // ([`luma_qp`]) and the test stays safe; the macroblock's own QP
        // decides it alone on most content.
        let below = |m: &MbInfo| i32::from(m.qp) < live.1;
        let neighbour_below = |a: Option<usize>| a.is_none_or(|a| mbs.get(a).is_some_and(below));
        if below(q) && neighbour_below(left) && neighbour_below(top) {
            continue;
        }
        let bs = strengths(kernels, pic, motion_base, mbs, (addr, q), left, top);
        // Nothing to filter anywhere in this macroblock. Worth one test:
        // a macroblock with one motion and no coefficient next to similar
        // neighbours gives all zeros, the common case on low-motion
        // content, and without this test the loops
        // below still walk eight luma and eight chroma edge slots and
        // derive three quantisation parameters that nothing reads.
        if bs.iter().flatten().fold(0, |any, &w| any | w) == 0 {
            continue;
        }
        table.set(&f);
        let qp_q = luma_qp(q);
        let neighbours = [left, top].map(|n| n.and_then(|n| mbs.get(n)));
        let at = luma_row + mbx * 16;
        // The window of twenty rows of twenty samples around the block holds
        // all eight of its edges, so it is checked once and each edge is a
        // view inside it; where the plane holds less before the block, as
        // for chroma below, each edge is checked on its own. One call per
        // direction, so that the kernel's choice of direction folds where
        // the edge is built.
        if let Ok(mut window) = LumaMbMut::new(&mut pic.y, at, stride) {
            luma_edges(&bs, neighbours, qp_q, table, |dir, e, params| {
                if dir == 0 {
                    if let Some(edge) = window.vertical(e) {
                        kernels.filter_edge(edge, params);
                    }
                } else if let Some(edge) = window.horizontal(e) {
                    kernels.filter_edge(edge, params);
                }
                Ok(())
            })?;
        } else {
            luma_by_edges(
                kernels, &mut pic.y, at, stride, &bs, neighbours, qp_q, table,
            )?;
        }
        // Chroma of a 4:2:0 picture: its two edges per direction take the
        // boundary strengths of luma edges 0 and 2. The four edges of both
        // planes go to the kernels in one call (`Kernels::filter_chroma_mb`),
        // vertical edges 0 and 4 in slots 0 and 1, horizontal in 2 and 3;
        // an edge that filters nothing keeps bS 0 in its slot, and a plane
        // whose thresholds filter nothing takes an alpha of 0, which filters
        // no line. The thresholds of the internal edges, each plane's own
        // QPC on both sides, are derived once a macroblock.
        let qp_qs = chroma_offsets.map(|offset| chroma_qp(q, offset));
        let chroma_inside = qp_qs.map(|qp| table.get(qp, qp));
        let mut slots = [[EdgeParams::default(); 2]; 4];
        let mut any = false;
        for ((dir, edges), p) in bs.iter().enumerate().zip(neighbours) {
            for (k, luma_edge) in [0usize, 2].into_iter().enumerate() {
                let strength = edges.get(luma_edge).copied().unwrap_or(0);
                if strength == 0 {
                    continue;
                }
                let mut thresholds = chroma_inside;
                if luma_edge == 0 {
                    for (t, (&offset, &qp_q)) in
                        thresholds.iter_mut().zip(chroma_offsets.iter().zip(&qp_qs))
                    {
                        let qp_p = p.map_or(qp_q, |m| chroma_qp(m, offset));
                        *t = table.get(qp_p, qp_q);
                    }
                }
                if !thresholds.iter().any(Thresholds::filters) {
                    continue;
                }
                if let Some(slot) = slots.get_mut(2 * dir + k) {
                    *slot = thresholds.map(|t| t.params(strength));
                    any = true;
                }
            }
        }
        if !any {
            continue;
        }
        let at = chroma_row + mbx * 8;
        // The window of ten rows of ten samples around the block, two rows
        // above it and two columns left of it, lies inside each plane but
        // where the plane holds less before the block: the first
        // macroblock of a band holding the picture's first row (its slack
        // rows start at the left edge), and a picture one macroblock wide
        // (`docs/design/h264.md`, "Kernel round: a macroblock's chroma
        // edges in one call"). There the edges go one at a time.
        if let Some((cb, cr)) = ChromaMbMut::pair(&mut pic.cb, &mut pic.cr, at, chroma_stride) {
            kernels.filter_chroma_mb(cb, cr, &slots);
            continue;
        }
        chroma_by_edges(kernels, &mut pic.cb, &mut pic.cr, at, chroma_stride, &slots)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picture::{NO_HEAD, NO_MOTION, NO_SLOT};
    use coreav_kernels_h264::{Auto, Scalar};

    struct Rng(u64);

    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    /// A motion from a small set, so that equal, close and distant motion
    /// and every count of lists all occur.
    fn motion(rng: &mut Rng) -> Motion {
        let list = |rng: &mut Rng, used: bool| {
            if used {
                let r = rng.next(2) as i8;
                let mv = [rng.next(3) as i16 * 3, rng.next(3) as i16 * 3];
                (r, r as u8 + 10 * (rng.next(2) as u8), mv)
            } else {
                (-1, NO_SLOT, [0, 0])
            }
        };
        let lists = rng.next(3);
        let a = list(rng, lists != 1);
        let b = list(rng, lists != 0);
        Motion {
            mv: [a.2, b.2],
            ref_idx: [a.0, b.0],
            ref_slot: [a.1, b.1],
        }
    }

    /// What the kernels are handed, against every block's motion handed
    /// to the reference: the motion stored as the decoder stores it, a
    /// head for each macroblock, and the sixteen entries of one that
    /// carries one motion (or none) filled with other motion, so that a
    /// read of them fails the test. The band starts at a macroblock other
    /// than 0, the split macroblocks carry the partitions of clause 7.4.5
    /// as well as motion per block, and a neighbour outside the band's
    /// store reads as [`UNSTORED`].
    #[test]
    fn strengths_read_heads_and_blocks_as_stored() {
        let kernels = [Auto::detect(), Auto::scalar()];
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..200_000 {
            // The band's macroblocks 0 (left), 1 (top) and 2 (the current
            // one) at addresses `base` on; 3 and 4 are a left and a top
            // neighbour past the band's store.
            let base = rng.next(3) as usize;
            let mut infos = [MbInfo::NONE; 5];
            let mut stored = vec![NO_MOTION; 48];
            let mut heads = [NO_HEAD; 3];
            for (k, m) in infos.iter_mut().enumerate() {
                m.slice = 1;
                m.kind = match rng.next(6) {
                    0 => MbKind::I4x4,
                    1 => MbKind::PSkip,
                    2 => MbKind::B(1),
                    _ => MbKind::P(0),
                };
                m.nz = rng.next(1 << 16) as u16 & rng.next(1 << 16) as u16;
                m.uniform = m.kind.is_intra() || k > 2 && rng.next(2) == 0;
            }
            infos[2].t8x8 = rng.next(4) == 0;
            for (m, blocks) in infos.iter_mut().zip(stored.chunks_mut(16)) {
                if m.kind.is_intra() {
                    continue;
                }
                let first = motion(&mut rng);
                blocks.fill(first);
                if rng.next(2) == 0 {
                    // Motion per block, per 8x8 quadrant, per 16x8 or per
                    // 8x16 half; block `i` at column `i & 3`, row `i >> 2`.
                    let shape = rng.next(4);
                    let part = |i: usize| match shape {
                        0 => i,
                        1 => (i >> 3) * 2 + ((i & 3) >> 1),
                        2 => i >> 3,
                        _ => (i & 3) >> 1,
                    };
                    let mut parts = [first; 16];
                    for p in parts.iter_mut().skip(1) {
                        if rng.next(3) != 0 {
                            *p = motion(&mut rng);
                        }
                    }
                    for (i, b) in blocks.iter_mut().enumerate() {
                        *b = parts[part(i)];
                    }
                }
                m.uniform = blocks.iter().all(|b| b == &first);
            }
            // What the reference reads: every block's motion.
            let mut truth = stored.clone();
            truth.extend_from_slice(&UNSTORED_BLOCKS);
            truth.extend_from_slice(&UNSTORED_BLOCKS);
            for ((m, head), blocks) in infos
                .iter()
                .zip(heads.iter_mut())
                .zip(stored.chunks_mut(16))
            {
                if m.kind.is_intra() {
                    continue;
                }
                *head = MbHead {
                    m: blocks[0],
                    split: !m.uniform,
                };
                if m.uniform {
                    for b in blocks.iter_mut() {
                        *b = motion(&mut rng);
                    }
                }
            }
            let left = (rng.next(4) != 0).then(|| if rng.next(8) == 0 { 3 } else { 0 });
            let top = (rng.next(4) != 0).then(|| if rng.next(8) == 0 { 4 } else { 1 });
            let band = BandData {
                motion: stored,
                heads: heads.to_vec(),
                ..BandData::default()
            };
            let side = |k: usize| StrengthSide {
                motion: Motions::Each(
                    <&[Motion; 16]>::try_from(&truth[k * 16..k * 16 + 16]).unwrap(),
                ),
                nz: infos[k].nz,
                intra: infos[k].kind.is_intra(),
            };
            let want = Scalar.strengths(&StrengthMb {
                q: side(2),
                left: left.map(side),
                top: top.map(side),
                t8x8: infos[2].t8x8,
            });
            let mut mbs = vec![MbInfo::NONE; base];
            mbs.extend_from_slice(&infos);
            let at = |k: usize| k + base;
            for k in &kernels {
                let got = strengths(
                    k,
                    &band,
                    base,
                    &mbs,
                    (at(2), &infos[2]),
                    left.map(at),
                    top.map(at),
                );
                assert_eq!(
                    got,
                    want,
                    "{k:?} {base} {:?} {left:?} {top:?}",
                    infos.map(|m| (m.kind, m.nz, m.uniform))
                );
            }
        }
    }

    /// The thresholds as clauses 8.7.2 and 8.7.2.2 derive them, with the
    /// clips of Equations 8-313, 8-461 and 8-462 written out: the
    /// reference for the shifted tables.
    fn thresholds_by_the_clause(qp_p: i32, qp_q: i32, a: i32, b: i32) -> Option<(u8, u8, [u8; 3])> {
        let average = (qp_p + qp_q + 1) >> 1;
        let index_a = (average + a).clamp(0, 51) as usize;
        let index_b = (average + b).clamp(0, 51) as usize;
        let (alpha, beta) = (t::ALPHA[index_a], t::BETA[index_b]);
        (alpha != 0 && beta != 0).then(|| (alpha, beta, t::TC0[index_a]))
    }

    #[test]
    fn the_shifted_tables_match_the_clause() {
        // Offsets past the −12 to 12 the slice header and the PPS accept
        // too, so that the tables do not depend on that check.
        for a in -70..=70 {
            for b in [-70, -52, -51, -50, -12, -1, 0, 1, 12, 50, 51, 52, 70, a] {
                let offsets = Offsets::new(&SliceFilter {
                    disable_idc: 0,
                    offset_a: a,
                    offset_b: b,
                });
                for qp_p in 0..=51 {
                    for qp_q in 0..=51 {
                        let t = Thresholds::new(qp_p, qp_q, offsets);
                        let got = t.filters().then(|| {
                            let [t0, t1, t2, t3, t4, t5, t6, t7] = t.tc0;
                            assert_eq!([t0, t4, t5, t6, t7], [0; 5]);
                            (t.alpha, t.beta, [t1, t2, t3])
                        });
                        assert_eq!(
                            got,
                            thresholds_by_the_clause(qp_p, qp_q, a, b),
                            "{qp_p} {qp_q} {a} {b}"
                        );
                    }
                }
            }
        }
        for offset in -70..=70 {
            for qp in 0..=51 {
                for kind in [MbKind::I16x16, MbKind::IPcm] {
                    let m = MbInfo {
                        qp: qp as i8,
                        kind,
                        ..MbInfo::NONE
                    };
                    let qpi = (if kind == MbKind::IPcm { 0 } else { qp } + offset).clamp(0, 51);
                    let want = i32::from(t::QPC[qpi as usize]);
                    assert_eq!(
                        chroma_qp(&m, shifted(offset)),
                        want,
                        "{qp} {offset} {kind:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_threshold_table_matches_the_derivation() {
        let mut table = ThresholdTable::new();
        // Offsets in the order slices could bring them, a pair repeated
        // after another so that the table is rebuilt for it.
        for (a, b) in [
            (0, 0),
            (-12, 12),
            (12, -12),
            (0, 0),
            (-51, 51),
            (70, -70),
            (0, 1),
        ] {
            let f = SliceFilter {
                disable_idc: 0,
                offset_a: a,
                offset_b: b,
            };
            table.set(&f);
            let offsets = Offsets::new(&f);
            for qp_p in 0..=51 {
                for qp_q in 0..=51 {
                    let (got, want) = (table.get(qp_p, qp_q), Thresholds::new(qp_p, qp_q, offsets));
                    assert_eq!(
                        (got.alpha, got.beta, got.tc0),
                        (want.alpha, want.beta, want.tc0),
                        "{qp_p} {qp_q} {a} {b}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_edge_below_the_thresholds_tables_is_not_filtered() {
        let f = Offsets::new(&SliceFilter::default());
        assert!(!Thresholds::new(15, 15, f).filters());
        let t = Thresholds::new(40, 40, f);
        let p = t.params(u32::from_le_bytes([0, 1, 3, 4]));
        assert_eq!([0, 1, 2, 3].map(|s| p.tc0_at(s)), [0, 4, 7, 0]);
        assert_eq!((p.alpha, p.beta), (80, 13));
    }

    /// The offsets the tests of `first_live_qp` take: −12 to 12, which a
    /// slice header and a picture parameter set can carry, and the ends of
    /// the tables' range.
    fn offsets_tried(step: usize) -> Vec<usize> {
        let mut v: Vec<usize> = (-12..=12).step_by(step).map(shifted).collect();
        v.extend([shifted(-51), shifted(51)]);
        v
    }

    #[test]
    fn the_first_live_qp_is_the_lowest_whose_thresholds_filter() {
        for &a in &offsets_tried(1) {
            for &b in &offsets_tried(1) {
                let offsets = Offsets { a, b };
                let live = |qp: i32| Thresholds::new(qp, qp, offsets).filters();
                for &c0 in &offsets_tried(1) {
                    for &c1 in &offsets_tried(1) {
                        let searched = (0..52)
                            .find(|&qp| live(qp) || [c0, c1].iter().any(|&c| live(qpc(qp, c))))
                            .unwrap_or(52);
                        assert_eq!(
                            first_live_qp(offsets, [c0, c1]),
                            searched,
                            "{a} {b} {c0} {c1}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn no_edge_below_the_first_live_qp_filters() {
        for &a in &offsets_tried(3) {
            for &b in &offsets_tried(3) {
                let offsets = Offsets { a, b };
                for &c0 in &offsets_tried(3) {
                    for &c1 in &offsets_tried(3) {
                        let first = first_live_qp(offsets, [c0, c1]);
                        for p in 0..first {
                            for q in 0..first {
                                assert!(!Thresholds::new(p, q, offsets).filters());
                                for c in [c0, c1] {
                                    let t = Thresholds::new(qpc(p, c), qpc(q, c), offsets);
                                    assert!(!t.filters(), "{a} {b} {c} {p} {q}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
