//! The macroblock layer: slice data (clause 7.3.4), the macroblock syntax
//! (clause 7.3.5) with either entropy coder, and the state a macroblock's
//! neighbours need.
//!
//! Reconstruction lives in `recon.rs`, inter prediction in `inter.rs` and
//! the syntax elements themselves in `syntax.rs`.
#![forbid(unsafe_code)]

use coreav_kernels_h264::{Kernels, Macroblocks, Planes, PlanesMut};

use crate::bits::stop_bit;
use crate::cabac::Cabac;
use crate::cavlc::Window;
use crate::deblock::{SliceFilter, ThresholdTable, deblock_rows};
use crate::dpb::RefPic;
use crate::error::{Error, Result, require};
use crate::inter::{InterTables, MotionCtx, Scratch};
use crate::nb::{self, Edge, NbCache};
use crate::params::{LevelScale, Pps, Sps};
use crate::picture::{
    BandData, Canvas, Geometry, MbHead, Motion, NO_HEAD, NO_MOTION, NO_SLOT, rows_refreshed_first,
};
use crate::recon::{CoeffScale, CoeffTables, LevelScales};
use crate::residual::{Blk, MAX_COEFFS};
use crate::slice::{SliceHeader, SliceType};
use crate::syntax::{CabacSyntax, CavlcSyntax, Syntax};
use crate::tables as t;
use crate::tier::Tier;
use coreav_kernels_h264::prefetch::{Ownership, rows_for_write};

/// What a macroblock is coded as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MbKind {
    /// Not decoded yet.
    #[default]
    None,
    I4x4,
    I8x8,
    I16x16,
    IPcm,
    /// A P macroblock type of Table 7-13.
    P(u8),
    /// A B macroblock type of Table 7-14.
    B(u8),
    PSkip,
    BSkip,
}

impl MbKind {
    pub(crate) fn is_intra(self) -> bool {
        matches!(
            self,
            MbKind::I4x4 | MbKind::I8x8 | MbKind::I16x16 | MbKind::IPcm
        )
    }

    pub(crate) fn is_skip(self) -> bool {
        matches!(self, MbKind::PSkip | MbKind::BSkip)
    }

    pub(crate) fn is_inter(self) -> bool {
        matches!(
            self,
            MbKind::P(_) | MbKind::B(_) | MbKind::PSkip | MbKind::BSkip
        )
    }
}

/// coded_block_flag bits of [`NbInfo::cbf`] that are not luma 4x4 blocks.
pub(crate) const CBF_LUMA_DC: u32 = 1 << 16;
pub(crate) const CBF_CB_DC: u32 = 1 << 17;
pub(crate) const CBF_CR_DC: u32 = 1 << 18;
/// The first of the four Cb AC bits.
pub(crate) const CBF_CB_AC0: u32 = 19;
/// The first of the four Cr AC bits.
pub(crate) const CBF_CR_AC0: u32 = 23;

/// What a decoded macroblock leaves for whatever reads it beyond its
/// neighbours A and B: the availability of later macroblocks, the
/// deblocking filter and the contexts that ask a neighbour's type. One
/// record a macroblock of the picture (`PictureWork::mbs`); what only
/// macroblocks A and B read is in [`NbInfo`], and the motion in the band's
/// heads ([`crate::picture::MbHead`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct MbInfo {
    /// 1 + the index of the slice the macroblock belongs to; 0 while it is
    /// not decoded.
    pub(crate) slice: u32,
    pub(crate) kind: MbKind,
    /// QPY.
    pub(crate) qp: i8,
    pub(crate) t8x8: bool,
    /// The luma 4x4 blocks, in raster order, whose transform block has a
    /// non-zero coefficient (clause 8.7.2.1).
    pub(crate) nz: u16,
    /// Whether every 4x4 block of the macroblock holds the same motion:
    /// its head is not split ([`crate::picture::MbHead::split`]).
    ///
    /// The deblocking filter uses it to retire all of a macroblock's
    /// internal edges at once: where the motion is uniform and no block
    /// holds a coefficient, every internal boundary has bS = 0
    /// (clause 8.7.2.1). An intra macroblock leaves it true, which costs
    /// nothing because intra boundaries take bS from their own rule.
    pub(crate) uniform: bool,
}

impl MbInfo {
    /// A macroblock that has not been decoded.
    pub(crate) const NONE: MbInfo = MbInfo {
        slice: 0,
        kind: MbKind::None,
        qp: 0,
        t8x8: false,
        nz: 0,
        uniform: true,
    };
}

impl Default for MbInfo {
    fn default() -> MbInfo {
        MbInfo::NONE
    }
}

// Twelve bytes: the record array is written once a macroblock and its
// lines were last touched a picture ago.
const _: () = assert!(core::mem::size_of::<MbInfo>() <= 12);

/// The motion of a P_Skip macroblock with a zero vector, as its
/// prediction hands it to motion compensation (`predict_p_skip`).
const STILL: Motion = Motion {
    mv: [[0, 0]; 2],
    ref_idx: [0, -1],
    ref_slot: [NO_SLOT; 2],
};

/// A macroblock that is not there, for neighbour lookups that fall outside
/// the picture or the slice.
static NO_MB: MbInfo = MbInfo::NONE;

/// What a decoded macroblock leaves for the macroblocks to its right (A)
/// and below (B) alone, kept in a ring of a little more than a macroblock
/// row ([`SliceDecoder::ring`]).
///
/// Each field is what the macroblock's syntax gives, and zero where its
/// type codes none of it, so that a reader asks no more of the type: a
/// skipped or intra macroblock leaves no `ref_gt0` bit, only an intra one
/// other than I_PCM a `chroma_mode`, a skipped one no `cbp` and an I_PCM
/// one all of it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NbInfo {
    /// What entropy decoding of a neighbour in the same slice reads of the
    /// edge blocks ([`Edge`]). With CAVLC, TotalCoeff: the luma edge, then
    /// the Cb and Cr edges ([`nb::edge_c`]). With CABAC, the absolute
    /// motion vector differences of list `l`, both components of each
    /// block capped at 255: the bottom row at `2 * l` ([`nb::mvd_row`]) and
    /// the right column at `2 * l + 1` ([`nb::mvd_col`]). One array serves
    /// both because a neighbour that is available is in the macroblock's
    /// own slice (clause 6.4.9), which has one entropy coding mode; a
    /// macroblock that codes none of these leaves zeros, which is what a
    /// skipped or intra macroblock gives.
    pub(crate) entropy: [Edge; 4],
    /// Intra4x4PredMode, or Intra8x8PredMode repeated, of the edge blocks
    /// ([`Edge`]); read only when the macroblock is I4x4 or I8x8, the only
    /// ones that set it.
    pub(crate) intra_edge: Edge,
    /// coded_block_flag for CABAC: bits 0 to 15 are the luma 4x4 blocks in
    /// raster order, then luma DC, Cb DC, Cr DC, four Cb AC and four Cr AC.
    pub(crate) cbf: u32,
    /// CodedBlockPatternLuma in bits 0 to 3, CodedBlockPatternChroma in
    /// bits 4 and 5.
    pub(crate) cbp: u8,
    /// intra_chroma_pred_mode of an intra macroblock other than I_PCM.
    pub(crate) chroma_mode: u8,
    /// Per list and 8x8 partition: whether a partition that uses the list
    /// has a reference index above 0, in bit `4 * list + partition`.
    pub(crate) ref_gt0: u8,
}

impl NbInfo {
    /// What a macroblock leaves before its syntax is parsed: a skipped
    /// macroblock's entry.
    pub(crate) const NONE: NbInfo = NbInfo {
        entropy: [[0; 8]; 4],
        intra_edge: [0; 8],
        cbf: 0,
        cbp: 0,
        chroma_mode: 0,
        ref_gt0: 0,
    };
}

// Three 16-byte moves to store, once a macroblock.
const _: () = assert!(core::mem::size_of::<NbInfo>() <= 48);

/// An entry for a lookup that falls outside the ring, which an available
/// neighbour never does.
static NO_NB: NbInfo = NbInfo::NONE;

/// The entries of the ring of [`NbInfo`] for a picture `mb_w` macroblocks
/// wide: a power of two above `mb_w`, so that macroblocks A (`addr - 1`)
/// and B (`addr - mb_w`) are among the last entries written, and an
/// address's entry is `addr & (entries - 1)`.
pub(crate) fn ring_entries(mb_w: usize) -> usize {
    (mb_w + 1).next_power_of_two()
}

/// The (x, y) of luma4x4BlkIdx in 4x4 block units (clause 6.4.3): the
/// blocks run over four 8x8 quadrants, and over four 4x4 blocks inside
/// each.
pub(crate) const BLK_XY: [(usize, usize); 16] = [
    (0, 0),
    (1, 0),
    (0, 1),
    (1, 1),
    (2, 0),
    (3, 0),
    (2, 1),
    (3, 1),
    (0, 2),
    (1, 2),
    (0, 3),
    (1, 3),
    (2, 2),
    (3, 2),
    (2, 3),
    (3, 3),
];

/// The raster position of luma4x4BlkIdx `blk`.
#[inline]
pub(crate) fn raster(blk: usize) -> usize {
    let (x, y) = BLK_XY.get(blk & 15).copied().unwrap_or((0, 0));
    y * 4 + x
}

/// The raster bits of the four 4x4 blocks of 8x8 quadrant `b8`.
#[inline]
pub(crate) fn quadrant(b8: usize) -> u16 {
    0x0033 << ((b8 & 1) * 2 + (b8 & 2) * 4)
}

/// The (x, y) in 4x4 block units of luma4x4BlkIdx `blk`.
#[inline]
pub(crate) fn blk_xy(blk: usize) -> (usize, usize) {
    BLK_XY.get(blk & 15).copied().unwrap_or((0, 0))
}

/// Raster position by scanning position, for each way a block's levels
/// are laid out: whole 4x4 blocks, the AC levels of a 4x4 block (which
/// start at scanning position 1), chroma DC, whole 8x8 blocks, and the four
/// interleaved 4x4 lists CAVLC codes an 8x8 block as (clause 7.3.5.3.1).
/// Each is padded to [`MAX_COEFFS`] so that a scanning position below it is
/// always in range.
pub(crate) const SCAN_4X4: [u8; MAX_COEFFS] = scan(&t::ZIGZAG_4X4, 0, 1);
pub(crate) const SCAN_4X4_AC: [u8; MAX_COEFFS] = scan(&t::ZIGZAG_4X4, 1, 1);
pub(crate) const SCAN_CHROMA_DC: [u8; MAX_COEFFS] = scan(&[0, 1, 2, 3], 0, 1);
pub(crate) const SCAN_8X8_CAVLC: [[u8; MAX_COEFFS]; 4] = [
    scan(&t::ZIGZAG_8X8, 0, 4),
    scan(&t::ZIGZAG_8X8, 1, 4),
    scan(&t::ZIGZAG_8X8, 2, 4),
    scan(&t::ZIGZAG_8X8, 3, 4),
];

/// `order[first + step * k]` for each `k` that stays inside `order`, then
/// zeros.
#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn scan(order: &[u8], first: usize, step: usize) -> [u8; MAX_COEFFS] {
    let mut out = [0u8; MAX_COEFFS];
    let mut k = 0;
    while first + step * k < order.len() {
        out[k] = order[first + step * k];
        k += 1;
    }
    out
}

/// The upper-left luma sample of macroblock partition `p` of a partition
/// shape `w` by `h` (clause 6.4.2.1).
pub(crate) fn part_xy(p: usize, w: u8, h: u8) -> (usize, usize) {
    if w == 16 {
        (0, p * usize::from(h))
    } else if h == 16 {
        (p * usize::from(w), 0)
    } else {
        ((p % 2) * 8, (p / 2) * 8)
    }
}

/// The upper-left luma sample of sub-macroblock partition `sp` inside its
/// 8x8 block (clause 6.4.2.2).
pub(crate) fn sub_part_xy(sp: usize, w: u8, h: u8) -> (usize, usize) {
    match (w, h) {
        (8, 8) => (0, 0),
        (8, 4) => (0, sp * 4),
        (4, 8) => (sp * 4, 0),
        _ => ((sp % 2) * 4, (sp / 2) * 4),
    }
}

/// The syntax of the macroblock being decoded, and the coefficients it
/// carries.
pub(crate) struct MbData {
    pub(crate) kind: MbKind,
    pub(crate) i16_mode: u8,
    pub(crate) t8x8: bool,
    /// Intra4x4PredMode by luma4x4BlkIdx, or Intra8x8PredMode by
    /// luma8x8BlkIdx.
    pub(crate) modes: [u8; 16],
    pub(crate) chroma_mode: u8,
    pub(crate) sub: [u8; 4],
    pub(crate) ref_idx: [[i8; 4]; 2],
    /// Per list, per `mbPartIdx * 4 + subMbPartIdx`, per component.
    pub(crate) mvd: [[[i32; 2]; 16]; 2],
    pub(crate) cbp: u8,
    /// Luma coefficients of each 4x4 block, in raster order inside the
    /// block.
    pub(crate) coeff: [[i32; 16]; 16],
    /// Luma coefficients of each 8x8 block.
    pub(crate) coeff8: [[i32; 64]; 4],
    /// The Intra_16x16 DC block.
    pub(crate) dc: [i32; 16],
    /// The chroma DC block of each component.
    pub(crate) chroma_dc: [[i32; 4]; 2],
    /// The chroma AC blocks of each component.
    pub(crate) chroma_ac: [[[i32; 16]; 4]; 2],
    /// Which blocks hold a non-zero coefficient: bits 0 to 15 are the 4x4
    /// luma blocks by luma4x4BlkIdx, bits 16 to 19 the 8x8 blocks.
    pub(crate) luma_nz: u32,
    pub(crate) chroma_ac_nz: [u8; 2],
    pub(crate) dc_nz: bool,
    pub(crate) chroma_dc_nz: [bool; 2],
}

impl MbData {
    pub(crate) fn new() -> Box<MbData> {
        Box::new(MbData {
            kind: MbKind::None,
            i16_mode: 0,
            t8x8: false,
            modes: [0; 16],
            chroma_mode: 0,
            sub: [0; 4],
            ref_idx: [[0; 4]; 2],
            mvd: [[[0; 2]; 16]; 2],
            cbp: 0,
            coeff: [[0; 16]; 16],
            coeff8: [[0; 64]; 4],
            dc: [0; 16],
            chroma_dc: [[0; 4]; 2],
            chroma_ac: [[[0; 16]; 4]; 2],
            luma_nz: 0,
            chroma_ac_nz: [0; 2],
            dc_nz: false,
            chroma_dc_nz: [false; 2],
        })
    }

    /// Clears what the last macroblock left, touching only the coefficient
    /// blocks that were written: each loop runs over the set bits of its
    /// mask, so a macroblock after one without levels clears nothing.
    /// `t` is the instance of the hot paths it runs in.
    fn reset<T: Tier>(&mut self, t: T) {
        t.enter(
            #[inline(always)]
            || self.reset_body(),
        );
    }

    ///
    /// The fields the macroblock's syntax always writes before anything
    /// reads them are left as they are: `kind` and `cbp` (every coded
    /// macroblock sets them first), `sub` (all four, by every macroblock
    /// type that reads it), and `ref_idx` and `mvd`, which prediction reads
    /// for exactly the list and partition pairs whose syntax was parsed. A
    /// macroblock that fails ends its slice, whose `MbData` is not kept.
    /// `t8x8` is read by every macroblock and set by few.
    #[inline(always)]
    fn reset_body(&mut self) {
        self.t8x8 = false;
        let mut luma = self.luma_nz;
        while luma != 0 {
            let b = luma.trailing_zeros() as usize;
            luma &= luma - 1;
            if b < 16 {
                if let Some(block) = self.coeff.get_mut(b) {
                    *block = [0; 16];
                }
            } else if let Some(block) = self.coeff8.get_mut(b - 16) {
                *block = [0; 64];
            }
        }
        if self.dc_nz {
            self.dc = [0; 16];
        }
        for (c, nz) in self.chroma_dc_nz.iter().enumerate() {
            if !*nz {
                continue;
            }
            if let Some(block) = self.chroma_dc.get_mut(c) {
                *block = [0; 4];
            }
        }
        for (blocks, &nz) in self.chroma_ac.iter_mut().zip(self.chroma_ac_nz.iter()) {
            let mut nz = nz;
            while nz != 0 {
                let b = nz.trailing_zeros() as usize;
                nz &= nz - 1;
                if let Some(block) = blocks.get_mut(b) {
                    *block = [0; 16];
                }
            }
        }
        self.luma_nz = 0;
        self.chroma_ac_nz = [0; 2];
        self.dc_nz = false;
        self.chroma_dc_nz = [false; 2];
    }
}

/// Entries of a reference picture list, at most (clause 7.4.3).
pub(crate) const MAX_REFS: usize = 32;

/// Slots per list of [`SliceDecoder::band_views`].
pub(crate) const BAND_VIEWS: usize = 16;

/// A band of a reference as [`Kernels::predict`] reads it, kept in
/// [`SliceDecoder::band_views`]: band `band` of reference `idx` of its
/// list. A band does not change once it is published, and a slice's
/// reference lists do not change while it is decoded, so the view stays
/// right for the rest of the slice.
#[derive(Clone, Copy)]
pub(crate) struct BandView<'a> {
    pub(crate) idx: usize,
    pub(crate) band: usize,
    pub(crate) planes: Planes<'a>,
}

/// Each reference of `refs` that is one published band of a picture
/// `mb_w` by `mb_h` macroblocks, as the planes [`Kernels::predict`] reads,
/// for [`SliceDecoder::whole`], and the band of the first of list 1.
pub(crate) fn whole_references(
    refs: &[Vec<RefPic>; 2],
    mb_w: usize,
    mb_h: usize,
) -> ([[Option<Planes<'_>>; MAX_REFS]; 2], Option<&BandData>) {
    fn whole(r: &RefPic, mb_w: usize, mb_h: usize) -> Option<&BandData> {
        let p = &r.picture;
        if p.mb_width == mb_w && p.mb_height == mb_h {
            p.whole()
        } else {
            None
        }
    }
    let mut out = [[None; MAX_REFS]; 2];
    for (slots, list) in out.iter_mut().zip(refs.iter()) {
        for (slot, r) in slots.iter_mut().zip(list.iter()) {
            // Each plane past the slack rows its picture keeps before it
            // (`Geometry::luma_origin`), which the windows at the picture's
            // edges may reach into.
            *slot = whole(r, mb_w, mb_h).and_then(|data| {
                let g = r.picture.geometry();
                let origin = [g.luma_origin(0), g.chroma_origin(0)];
                Planes::with_origin(&data.y, &data.cb, &data.cr, mb_w * 16, mb_h * 16, origin)
            });
        }
    }
    let colocated = refs
        .get(1)
        .and_then(|l| l.first())
        .and_then(|r| whole(r, mb_w, mb_h));
    (out, colocated)
}

/// Decoding of one slice into the picture it belongs to.
pub(crate) struct SliceDecoder<'a, K: Kernels> {
    pub(crate) kernels: &'a K,
    pub(crate) sps: &'a Sps,
    pub(crate) pps: &'a Pps,
    pub(crate) hdr: &'a SliceHeader,
    pub(crate) scale: &'a LevelScale,
    /// 1 + the index of this slice in its picture.
    pub(crate) slice_id: u32,
    pub(crate) mb_w: usize,
    pub(crate) mb_h: usize,
    /// The bands of the picture this slice belongs to that are not
    /// published yet (`picture.rs`).
    pub(crate) canvas: &'a mut Canvas,
    /// The band of the macroblock being decoded, taken out of `canvas`
    /// while its rows are decoded ([`SliceDecoder::store_band`]), and
    /// which band that is.
    pub(crate) out: BandData,
    pub(crate) held: Option<usize>,
    pub(crate) geometry: Geometry,
    /// The band of the macroblock being decoded, `usize::MAX` before the
    /// first, and where its stored rows start: the luma and chroma row and
    /// the macroblock address of its first stored motion. Every offset
    /// into the band's planes subtracts these.
    pub(crate) band: usize,
    pub(crate) luma_first: usize,
    pub(crate) chroma_first: usize,
    pub(crate) motion_base: usize,
    /// The luma stride, `mb_w * 16`.
    pub(crate) stride: usize,
    /// Where the band's first stored luma and chroma rows start in its
    /// buffers, past the slack rows a band holding the picture's first row
    /// keeps before its planes ([`Geometry::luma_origin`],
    /// [`Geometry::chroma_origin`]), set with [`SliceDecoder::luma_first`]:
    /// sample (x, row) of the luma plane is at `luma_origin + (row -
    /// luma_first) * stride + x`. Every offset into the band's planes
    /// adds these.
    pub(crate) luma_origin: usize,
    pub(crate) chroma_origin: usize,
    /// The macroblocks the band being decoded holds whole, found when it
    /// is taken ([`SliceDecoder::enter_row`]): where inter prediction
    /// writes ([`Kernels::predict_part`]).
    pub(crate) macroblocks: Macroblocks,
    pub(crate) mbs: &'a mut [MbInfo],
    /// What macroblocks A and B read of the macroblocks before the current
    /// one, at `addr & ring_mask` ([`ring_entries`]). An entry is read only
    /// where its macroblock is available, so in this slice and decoded by
    /// this decoder, whose `finish_mb` wrote it; what an earlier slice or
    /// picture left in the rest is never read, and the ring is not
    /// cleared.
    pub(crate) ring: Vec<NbInfo>,
    pub(crate) ring_mask: usize,
    /// The deblocking control of every slice of the picture so far, how
    /// many macroblock rows have been filtered, and the thresholds of the
    /// last filter offsets (`deblock.rs`).
    pub(crate) filters: &'a [SliceFilter],
    pub(crate) deblocked: &'a mut usize,
    pub(crate) thresholds: &'a mut ThresholdTable,
    pub(crate) refs: &'a [Vec<RefPic>; 2],
    /// Each reference that is one published band of this picture's size —
    /// every reference, with one thread — resolved once when the slice
    /// starts, so that motion compensation reads it without a band lookup
    /// per block; `None` for the others, and past the lists' ends.
    pub(crate) whole: [[Option<Planes<'a>>; MAX_REFS]; 2],
    /// The views of the bands of banded references that this slice's
    /// blocks have read, kept for the slice so that a block reading a
    /// band an earlier block read takes the view as it was built rather
    /// than building it again (`docs/design/h264.md`, "Threading round:
    /// band views kept for the slice"). Per list, reference `idx`'s band
    /// `k` goes to slot `(4 * idx + k % 4) % BAND_VIEWS`, which keeps
    /// four consecutive bands of each of the first four references.
    pub(crate) band_views: [[Option<BandView<'a>>; BAND_VIEWS]; 2],
    /// The band of the first picture of list 1, where it is whole, whose
    /// motion direct prediction reads.
    pub(crate) colocated_whole: Option<&'a BandData>,
    /// Per list, bit `r`: whether reference `r` has the default explicit
    /// weights ([`crate::slice::PredWeightTable::default_weights`]); all
    /// set where the slice carries no weight table.
    pub(crate) default_weights: [u32; 2],
    /// QPY of the previous macroblock in decoding order (QPY,PRED).
    pub(crate) qp: i32,
    pub(crate) prev_qp_delta_nonzero: bool,
    pub(crate) addr: usize,
    pub(crate) mbx: usize,
    pub(crate) mby: usize,
    pub(crate) mb: Box<MbData>,
    /// What the macroblock being decoded leaves for later macroblocks;
    /// written into `mbs` and the ring when it is finished.
    pub(crate) cur: MbInfo,
    pub(crate) cur_nb: NbInfo,
    /// The neighbour caches of the macroblock being decoded.
    pub(crate) nb: NbCache,
    /// The addresses of macroblocks A, B, C and D, when they are available.
    pub(crate) nbs: [Option<usize>; 4],
    /// The motion around and inside the macroblock being predicted
    /// (`inter.rs`), filled when its prediction starts.
    pub(crate) ctx: MotionCtx,
    /// The raster 4x4 blocks of the current macroblock whose motion is
    /// stored; kept in debug builds only, to check that an inter
    /// macroblock stores all sixteen.
    pub(crate) derived: u16,
    /// What inter prediction looks up per block that depends on the slice
    /// alone.
    pub(crate) tables: InterTables,
    /// The motion the first partition of this macroblock stored, which
    /// its head records once it is finished, with whether every partition
    /// stored the same (`cur.uniform`). `None` for a macroblock that stored
    /// one motion at once (`store_mb`) and for an intra one, which write
    /// their heads themselves.
    pub(crate) motion_first: Option<Motion>,
    pub(crate) scratch: Box<Scratch>,
    /// What macroblocks A and B give coded_block_flag's context for the
    /// macroblock being decoded ([`SliceDecoder::neighbour_cbf`]), taken
    /// once when its residual starts.
    pub(crate) cbf_nb: [u32; 2],
    /// The scalings of every QP for the slice's scaling lists.
    pub(crate) coeff_tables: &'a CoeffTables,
    /// The scalings the entropy decoder applies to the levels it stores,
    /// those of the current macroblock's QP.
    pub(crate) level_scales: LevelScales<'a>,
    /// The hints macroblocks issue ([`hints`]), a set of [`HINT_AHEAD`],
    /// [`HINT_OWN`] and [`HINT_APRON`]; any set but the empty one holds
    /// [`HINT_AHEAD`]. One byte, where it was one flag: the hints of a
    /// banded picture added no field, so the decoder's layout stays as it
    /// was, and a macroblock that asks for nothing makes one check.
    pub(crate) hints: u8,
}

/// The lines macroblocks write and read are asked for a cache line ahead
/// (`SliceDecoder::request_destination`, and the reference's in
/// `inter.rs`).
pub(crate) const HINT_AHEAD: u8 = 1;
/// The picture's lines are asked to be owned (`prefetchw`), on a CPU that
/// can.
pub(crate) const HINT_OWN: u8 = 2;
/// The macroblock row is a band's first, whose top apron's lines are asked
/// to be owned ([`SliceDecoder::request_apron`]), on a CPU that can: set
/// and cleared by [`SliceDecoder::enter_row`], only with [`HINT_OWN`].
pub(crate) const HINT_APRON: u8 = 4;

impl<K: Kernels> SliceDecoder<'_, K> {
    /// The record of neighbouring macroblock `addr`, or an undecoded
    /// macroblock if it is outside the picture. Every caller asks for a
    /// neighbour, never the current macroblock, whose record is `cur`.
    #[inline]
    pub(crate) fn info(&self, addr: usize) -> &MbInfo {
        self.mbs.get(addr).unwrap_or(&NO_MB)
    }

    /// What available neighbouring macroblock `addr` (A or B) left in the
    /// ring.
    #[inline]
    pub(crate) fn neighbour(&self, addr: usize) -> &NbInfo {
        self.ring.get(addr & self.ring_mask).unwrap_or(&NO_NB)
    }

    /// [`SliceDecoder::neighbour`], or the current macroblock's entry so
    /// far where `addr` is the current macroblock.
    #[inline]
    pub(crate) fn nb_info(&self, addr: usize) -> &NbInfo {
        if addr == self.addr {
            return &self.cur_nb;
        }
        self.neighbour(addr)
    }

    /// Whether macroblock `addr` belongs to this slice and is decoded
    /// (clause 6.4.9).
    #[inline]
    fn available(&self, addr: usize) -> bool {
        self.mbs.get(addr).is_some_and(|m| m.slice == self.slice_id)
    }

    /// Macroblock A, the one to the left.
    #[inline]
    pub(crate) fn mb_a(&self) -> Option<usize> {
        self.nbs[0]
    }

    /// Macroblock B, the one above.
    #[inline]
    pub(crate) fn mb_b(&self) -> Option<usize> {
        self.nbs[1]
    }

    /// The neighbouring macroblocks of the current one (clause 6.4.9),
    /// derived once when it starts.
    fn derive_neighbours(&self) -> [Option<usize>; 4] {
        let (addr, w) = (self.addr, self.mb_w);
        let left = self.mbx > 0;
        let up = self.mby > 0;
        [
            (left && self.available(addr.wrapping_sub(1))).then(|| addr - 1),
            (up && self.available(addr.wrapping_sub(w))).then(|| addr - w),
            (up && self.mbx + 1 < w && self.available(addr.wrapping_sub(w) + 1))
                .then(|| addr - w + 1),
            (up && left && self.available(addr.wrapping_sub(w + 1))).then(|| addr - w - 1),
        ]
    }

    /// [`SliceDecoder::derive_neighbours`] of a macroblock of a slice that
    /// started at `first` and whose macroblocks are decoded one after the
    /// other in raster order, from `first` to the current one: a neighbour
    /// is available where it is in the picture and not before `first`.
    /// Exact because a slice is one run of macroblocks in raster order:
    /// slice groups and arbitrary slice order are rejected (`decoder.rs`),
    /// and the slice loop decodes each address after the one before it.
    #[inline(always)]
    fn derive_neighbours_in_order(&self, first: usize) -> [Option<usize>; 4] {
        let (addr, w) = (self.addr, self.mb_w);
        let left = self.mbx > 0;
        let up = self.mby > 0;
        let nbs = [
            (left && addr > first).then(|| addr - 1),
            (up && addr.wrapping_sub(w) >= first).then(|| addr - w),
            (up && self.mbx + 1 < w && addr.wrapping_sub(w) + 1 >= first).then(|| addr - w + 1),
            (up && left && addr.wrapping_sub(w + 1) >= first).then(|| addr - w - 1),
        ];
        debug_assert_eq!(nbs, self.derive_neighbours(), "neighbours in slice order");
        nbs
    }

    /// The 4x4 luma block left of (`above` false) or above (`above` true)
    /// the raster block (bx, by): its macroblock and its raster index
    /// (clauses 6.4.11.4 and 6.4.12).
    #[inline]
    pub(crate) fn neighbour_4x4(
        &self,
        bx: usize,
        by: usize,
        above: bool,
    ) -> Option<(usize, usize)> {
        if above {
            if by > 0 {
                Some((self.addr, (by - 1) * 4 + bx))
            } else {
                self.mb_b().map(|m| (m, 12 + bx))
            }
        } else if bx > 0 {
            Some((self.addr, by * 4 + bx - 1))
        } else {
            self.mb_a().map(|m| (m, by * 4 + 3))
        }
    }

    /// The coded_block_flag bits ([`NbInfo::cbf`]) macroblocks A and B
    /// give the context of the current macroblock's blocks
    /// (clause 9.3.3.1.1.9), in that order.
    ///
    /// The clause asks of a neighbouring block whether its macroblock is
    /// available, I_PCM, skipped, or without coefficients in the block's
    /// 8x8 quadrant or chroma, before it asks for the block's flag. For a
    /// decoded macroblock every one of those questions is already answered
    /// by its `cbf`: the bits of a quadrant or of chroma are set only where
    /// coded_block_pattern coded them, a skipped macroblock sets none and an
    /// I_PCM one sets all. What is left is a macroblock that is not
    /// available, which counts as coded when the current macroblock is
    /// intra.
    fn neighbour_cbf(&self) -> [u32; 2] {
        let missing = if self.cur.kind.is_intra() {
            u32::MAX
        } else {
            0
        };
        let of = |m: Option<usize>| m.map_or(missing, |m| self.neighbour(m).cbf);
        [of(self.mb_a()), of(self.mb_b())]
    }

    /// ctxIdxInc of coded_block_flag for block `kind` of the current
    /// macroblock (clause 9.3.3.1.1.9): condTermFlagA + 2 * condTermFlagB,
    /// from the blocks of this macroblock decoded so far and from
    /// [`SliceDecoder::cbf_nb`].
    #[inline]
    pub(crate) fn coded_block_flag_inc(&self, kind: Blk) -> u32 {
        let own = self.cur_nb.cbf;
        let [left, top] = self.cbf_nb;
        // The bit of the block to the left and of the block above, each
        // either in this macroblock or in the neighbour's edge row or
        // column.
        let (a, b) = match kind {
            Blk::LumaDc => (left >> 16, top >> 16),
            Blk::LumaAc(blk) | Blk::Luma4x4(blk) => {
                let r = raster(blk) as u32;
                let (x, y) = (r & 3, r >> 2);
                (
                    if x > 0 {
                        own >> (r - 1)
                    } else {
                        left >> (r + 3)
                    },
                    if y > 0 {
                        own >> (r - 4)
                    } else {
                        top >> (r + 12)
                    },
                )
            }
            Blk::ChromaDc(c) => {
                let bit = if c == 0 { 17 } else { 18 };
                (left >> bit, top >> bit)
            }
            Blk::ChromaAc(c, b) => {
                let base = if c == 0 { CBF_CB_AC0 } else { CBF_CR_AC0 };
                let b = b as u32 & 3;
                let (x, y) = (b & 1, b >> 1);
                (
                    if x > 0 {
                        own >> (base + b - 1)
                    } else {
                        left >> (base + b + 1)
                    },
                    if y > 0 {
                        own >> (base + b - 2)
                    } else {
                        top >> (base + b + 2)
                    },
                )
            }
            Blk::Luma8x8(_) => (0, 0),
        };
        (a & 1) + 2 * (b & 1)
    }

    /// Where the motion of macroblock `addr` starts in the current band's
    /// motion: `addr` is the current macroblock or a neighbour above it,
    /// which the band's top apron holds.
    #[inline]
    pub(crate) fn motion_at(&self, addr: usize) -> usize {
        addr.wrapping_sub(self.motion_base).wrapping_mul(16)
    }

    /// Whether macroblock `addr` may be predicted from by an intra
    /// macroblock (constrained_intra_pred_flag, clause 7.4.2.2).
    pub(crate) fn intra_allowed(&self, addr: usize) -> bool {
        !(self.pps.constrained_intra_pred && self.info(addr).kind.is_inter())
    }

    /// Starts macroblock `addr`: filters the rows that are now two behind,
    /// derives the neighbours and clears the per-macroblock state.
    fn start_mb<T: Tier>(&mut self, tier: T, addr: usize) -> Result<()> {
        tier.enter(
            #[inline(always)]
            || self.start_mb_body::<false>(addr, 0),
        )
    }

    /// [`SliceDecoder::start_mb`] in place. `IN_ORDER`: the slice's
    /// macroblocks are decoded one after the other from `first`, and the
    /// neighbours are found without reading their records
    /// ([`SliceDecoder::derive_neighbours_in_order`]).
    #[inline(always)]
    fn start_mb_body<const IN_ORDER: bool>(&mut self, addr: usize, first: usize) -> Result<()> {
        // The next macroblock in raster order is found without dividing,
        // which is every macroblock but the first of a slice.
        let (col, row) = if addr == self.addr.wrapping_add(1) && self.addr != usize::MAX {
            if self.mbx + 1 == self.mb_w {
                (0, self.mby + 1)
            } else {
                (self.mbx + 1, self.mby)
            }
        } else {
            (addr % self.mb_w, addr / self.mb_w)
        };
        // Bands and deblocking change only from one macroblock row to the
        // next, and at the first macroblock of a slice, where `mby` starts
        // as `usize::MAX`.
        if row != self.mby {
            self.enter_row(row)?;
        }
        self.addr = addr;
        self.mbx = col;
        self.mby = row;
        self.motion_first = None;
        self.derived = 0;
        self.nbs = if IN_ORDER {
            self.derive_neighbours_in_order(first)
        } else {
            self.derive_neighbours()
        };
        self.issue_hints();
        // `uniform` starts true: `store_motion` clears it.
        self.cur = MbInfo {
            slice: self.slice_id,
            qp: self.qp as i8,
            ..MbInfo::NONE
        };
        // `cur_nb` is started by `decode_mb`: a skipped macroblock leaves
        // `NbInfo::NONE` without building it (`finish_mb`).
        // `MbData` is cleared by `decode_mb`, not here: a skipped
        // macroblock, the most common kind in a P or B slice, reads none
        // of it, so what the last coded macroblock left can wait for the
        // next one.
        Ok(())
    }

    /// Asks for the picture's lines that the macroblocks one cache line
    /// ahead will write (`docs/design/h264.md`, "Round 12"): a line holds
    /// four macroblocks of luma and eight of chroma, so the first
    /// macroblock of each four asks for the luma rows of the next four, and
    /// the first of each eight for the chroma rows of the next eight.
    #[inline(always)]
    fn request_destination(&self, hint: impl Fn(&[u8], usize, usize, usize)) {
        if self.mbx % 4 != 0 {
            return;
        }
        let x = (self.mbx + 4) * 16;
        if x < self.stride {
            let row = (self.mby * 16).wrapping_sub(self.luma_first);
            let at = self.luma_origin + row * self.stride + x;
            hint(&self.out.y, at, self.stride, 16);
        }
        let chroma_stride = self.stride / 2;
        let x = (self.mbx + 8) * 8;
        if self.mbx % 8 == 0 && x < chroma_stride {
            let row = (self.mby * 8).wrapping_sub(self.chroma_first);
            let first = self.chroma_origin + row * chroma_stride + x;
            hint(&self.out.cb, first, chroma_stride, 8);
            hint(&self.out.cr, first, chroma_stride, 8);
        }
    }

    /// The hints a macroblock issues as it starts ([`hints`]): its
    /// destination rows ahead, or a banded picture's every fourth
    /// macroblock.
    #[inline(always)]
    fn issue_hints(&self) {
        if self.hints != 0 {
            if self.hints == HINT_AHEAD {
                self.request_destination(rows_for_write);
            } else if self.mbx % 4 == 0 {
                self.request_banded();
            }
        }
    }

    /// Every hint of a banded picture, out of line so that the one-band
    /// decode's code stays as it was, and asked for only by the first
    /// macroblock of each four, as each of its hints is: with
    /// [`HINT_OWN`], [`Self::request_destination`]'s lines are asked to be
    /// owned, and with [`HINT_APRON`] the band's top apron's too.
    #[inline(never)]
    fn request_banded(&self) {
        let Some(own) = Ownership::detect().filter(|_| self.hints & HINT_OWN != 0) else {
            self.request_destination(rows_for_write);
            return;
        };
        self.request_destination(|plane, first, stride, rows| {
            own.rows(plane, first, stride, rows);
        });
        if self.hints & HINT_APRON != 0 {
            self.request_apron(own);
        }
    }

    /// Asks to own the lines of the band's top apron that the refresh
    /// before its first row is filtered will write (`docs/design/h264.md`,
    /// "Threading round: the aprons' rows owned ahead"): the band's buffer
    /// was last written for an older picture, and its lines were read since
    /// by other cores, whose caches hold them shared. Spread over the
    /// band's first macroblock row, each first macroblock of four (the only
    /// ones [`Self::request_banded`] runs for) asks for its line of every
    /// such luma row, and each first of eight for its line of every such
    /// chroma row, so that the whole apron is asked for once by the row's
    /// end.
    #[inline(always)]
    fn request_apron(&self, own: Ownership) {
        let [luma, chroma] = rows_refreshed_first(&self.geometry, self.band);
        own.rows(
            &self.out.y,
            self.luma_origin + self.mbx * 16,
            self.stride,
            luma,
        );
        if self.mbx % 8 == 0 {
            let (first, chroma_stride) = (self.chroma_origin + self.mbx * 8, self.stride / 2);
            own.rows(&self.out.cb, first, chroma_stride, chroma);
            own.rows(&self.out.cr, first, chroma_stride, chroma);
        }
    }

    /// Starts macroblock row `row`: filters the rows that are now two
    /// behind, or every row above when `row` starts a band, and holds the
    /// row's band, entering it if it is new.
    fn enter_row(&mut self, row: usize) -> Result<()> {
        let band = self.geometry.band_of_mb_row(row);
        // Deblocking runs one row behind decoding, and up to the row above
        // where a band starts, whose top apron holds the unfiltered rows
        // this row predicts from: see `deblock.rs`.
        let first_of_band = band > 0 && self.geometry.motion_first(band) + 1 == row;
        let filter_to = if first_of_band {
            row
        } else {
            row.saturating_sub(1)
        };
        self.hints = if first_of_band && self.hints & HINT_OWN != 0 {
            self.hints | HINT_APRON
        } else {
            self.hints & !HINT_APRON
        };
        let deblock = filter_to > *self.deblocked;
        if band == self.band && !deblock && self.held.is_some() {
            return Ok(());
        }
        self.store_band();
        if band != self.band {
            // The first macroblock of this slice, or of a band: entering a
            // band copies the row above it into its top apron, before the
            // deblocking below can filter that row.
            self.canvas.enter(band)?;
        }
        if deblock {
            deblock_rows(
                self.kernels,
                self.canvas,
                self.mbs,
                self.filters,
                self.pps.chroma_qp_index_offset,
                self.thresholds,
                *self.deblocked..filter_to,
            )?;
            *self.deblocked = filter_to;
        }
        self.out = self.canvas.take_band(band);
        self.held = Some(band);
        if band != self.band {
            self.band = band;
            self.luma_first = self.geometry.luma_first(band);
            self.chroma_first = self.geometry.chroma_first(band);
            self.luma_origin = self.geometry.luma_origin(band);
            self.chroma_origin = self.geometry.chroma_origin(band);
            self.motion_base = self.geometry.motion_first(band) * self.mb_w;
        }
        let out = &mut self.out;
        self.macroblocks = PlanesMut::with_origin(
            &mut out.y,
            &mut out.cb,
            &mut out.cr,
            self.stride,
            [self.luma_first, self.chroma_first],
            [self.luma_origin, self.chroma_origin],
        )
        .macroblocks();
        Ok(())
    }

    /// Puts the band being decoded back into the canvas: before the
    /// deblocking filter or a band's top apron needs it, and when the slice
    /// ends, whether or not it failed.
    pub(crate) fn store_band(&mut self) {
        if let Some(k) = self.held.take() {
            self.canvas.put_band(k, std::mem::take(&mut self.out));
        }
    }

    /// Stores what the finished macroblock leaves for later macroblocks:
    /// its record, its ring entry and, for one built by partitions, its
    /// head, each once. `SKIPPED`: a skipped macroblock, whose ring entry
    /// is [`NbInfo::NONE`] and is not built in `cur_nb`.
    #[inline(always)]
    fn finish_mb<const SKIPPED: bool>(&mut self) {
        // Built by partitions (`store_motion`), which wrote its sixteen
        // blocks: the head stands for them where they all agree. A
        // macroblock with one motion (`store_mb`) and an intra one
        // (`decode_mb`, `decode_pcm`) wrote their heads.
        if let Some(m) = self.motion_first {
            let at = self.addr.wrapping_sub(self.motion_base);
            if let Some(head) = self.out.heads.get_mut(at) {
                *head = MbHead {
                    m,
                    split: !self.cur.uniform,
                };
            }
        }
        // Two things rest on an inter macroblock having written all sixteen
        // of its blocks: `MbInfo::uniform`, which the deblocking filter
        // trusts without re-reading the motion, and the picture's motion not
        // being cleared before it is decoded into. `derived` records
        // exactly which blocks were written, so the assumption is checked
        // here rather than assumed. Debug only: in release this is the
        // invariant, not a branch on the hot path.
        debug_assert!(
            self.cur.kind.is_intra() || self.derived == 0xffff,
            "an inter macroblock left some of its motion underived"
        );
        debug_assert!(
            self.out
                .heads
                .get(self.addr.wrapping_sub(self.motion_base))
                .is_some_and(|h| h.split != self.cur.uniform
                    && (h.m == NO_MOTION || !self.cur.kind.is_intra())),
            "a head that disagrees with its record"
        );
        if let Some(slot) = self.mbs.get_mut(self.addr) {
            *slot = self.cur;
        }
        if let Some(slot) = self.ring.get_mut(self.addr & self.ring_mask) {
            *slot = if SKIPPED { NbInfo::NONE } else { self.cur_nb };
        }
    }

    /// Writes the head of an intra macroblock, without motion: the picture
    /// it is decoded into is reused without its motion being cleared, and
    /// motion vector prediction and a later picture's direct prediction
    /// read it (`inter.rs`); the deblocking filter passes over it.
    fn intra_head(&mut self) {
        let at = self.addr.wrapping_sub(self.motion_base);
        if let Some(head) = self.out.heads.get_mut(at) {
            *head = NO_HEAD;
        }
    }

    /// slice_data() (clause 7.3.4), in the instance of the hot paths
    /// `tier` names ([`crate::tier`]).
    pub(crate) fn decode_slice_data<T: Tier>(&mut self, tier: T, rbsp: &[u8]) -> Result<()> {
        let total = self.mb_w * self.mb_h;
        let addr = self.hdr.first_mb as usize;
        require(addr < total, "first_mb_in_slice")?;
        if self.pps.cabac {
            self.decode_cabac(tier, rbsp, addr, total)
        } else {
            self.decode_cavlc(tier, rbsp, addr, total)
        }
    }

    /// slice_data() with CABAC. Out of line, as the compiler leaves it
    /// with one instance: inlined into [`Self::decode_slice_data`], it
    /// takes the CABAC engine's `terminate` and the residual's parse out
    /// of line instead.
    #[inline(never)]
    fn decode_cabac<T: Tier>(
        &mut self,
        tier: T,
        rbsp: &[u8],
        addr: usize,
        total: usize,
    ) -> Result<()> {
        tier.enter(
            #[inline(always)]
            || self.decode_cabac_body(tier, rbsp, addr, total),
        )
    }

    #[inline(always)]
    fn decode_cabac_body<T: Tier>(
        &mut self,
        tier: T,
        rbsp: &[u8],
        mut addr: usize,
        total: usize,
    ) -> Result<()> {
        let st = self.hdr.slice_type;
        let slot = if st.is_intra() {
            0
        } else {
            1 + self.hdr.cabac_init_idc as usize
        };
        let mut c = Cabac::new_in(
            tier,
            rbsp,
            self.hdr.data_bit_offset,
            slot,
            self.hdr.slice_qp,
        )?;
        loop {
            require(
                self.mbs.get(addr).is_some_and(|m| m.slice == 0),
                "a macroblock decoded twice",
            )?;
            self.start_mb(tier, addr)?;
            // mb_skip_flag (clause 9.3.3.1.1.1).
            let skip = if st.is_intra() {
                false
            } else {
                let a = self.mb_a().is_some_and(|m| !self.info(m).kind.is_skip());
                let b = self.mb_b().is_some_and(|m| !self.info(m).kind.is_skip());
                let base = if st.is_b() { 24 } else { 11 };
                c.decision(tier, base + usize::from(a) + usize::from(b)) == 1
            };
            c.check()?;
            if skip {
                self.decode_skip(tier)?;
                self.finish_mb::<true>();
            } else {
                let mut syntax = CabacSyntax { c: &mut c, t: tier };
                self.decode_mb(&mut syntax, tier)?;
                self.finish_mb::<false>();
            }
            let end = c.terminate();
            c.check()?;
            if end == 1 {
                return Ok(());
            }
            addr += 1;
            require(addr < total, "a slice that runs past the picture")?;
        }
    }

    /// slice_data() with CAVLC. Out of line, as [`Self::decode_cabac`] is,
    /// so that [`Self::decode_slice_data`] only calls it: the CABAC path's
    /// code does not change with the CAVLC loop's.
    #[inline(never)]
    fn decode_cavlc<T: Tier>(
        &mut self,
        tier: T,
        rbsp: &[u8],
        addr: usize,
        total: usize,
    ) -> Result<()> {
        tier.enter(
            #[inline(always)]
            || self.decode_cavlc_body(tier, rbsp, addr, total),
        )
    }

    /// The loop of slice_data() with CAVLC: one window reads every syntax
    /// element of the slice ([`CavlcSyntax`]), and each macroblock starts
    /// in place, its neighbours found from the slice's first address.
    #[inline(always)]
    fn decode_cavlc_body<T: Tier>(
        &mut self,
        tier: T,
        rbsp: &[u8],
        mut addr: usize,
        total: usize,
    ) -> Result<()> {
        let st = self.hdr.slice_type;
        let first = addr;
        // more_rbsp_data() (clause 7.2): data remains while the position is
        // below the rbsp_stop_one_bit, and none where no bit is set.
        let stop = stop_bit(rbsp).unwrap_or(0);
        let mut s = CavlcSyntax::new(Window::at(rbsp, self.hdr.data_bit_offset)?, tier);
        let mut more = true;
        while more {
            if !st.is_intra() {
                let run = s.w.ue()? as usize;
                // Checked before the run is decoded: a run read past the
                // end of the data decodes no macroblock.
                s.w.check()?;
                require(run <= total, "mb_skip_run")?;
                if run > 0 {
                    self.decode_skip_run(tier, &mut addr, run, total)?;
                    more = s.w.pos() < stop;
                    if !more {
                        return Ok(());
                    }
                }
            }
            require(
                addr < total && self.mbs.get(addr).is_some_and(|m| m.slice == 0),
                "a slice that runs past the picture",
            )?;
            self.start_mb_body::<true>(addr, first)?;
            if let Err(e) = self.decode_mb_body(&mut s, tier) {
                return Err(s.w.past_end_or(e));
            }
            self.finish_mb::<false>();
            more = s.w.pos() < stop;
            addr += 1;
        }
        Ok(())
    }

    /// The `run` skipped macroblocks of an mb_skip_run of a CAVLC slice,
    /// from `*addr` on (clause 7.3.4).
    ///
    /// In a P slice, a P_Skip macroblock whose neighbour A is unavailable
    /// or has reference index 0 and a zero vector has a zero vector
    /// (clause 8.4.1.1). Inside a run, A is the macroblock before, or
    /// unavailable at the start of a row, so after the first one with a
    /// zero vector every macroblock of the run has one: those take
    /// [`SliceDecoder::decode_still_skip`], which finds neither its
    /// neighbours nor its motion. The first of a run, and one after a
    /// non-zero vector, are decoded as any skipped macroblock, through the
    /// out-of-line [`SliceDecoder::start_mb`] the CABAC loop calls too.
    #[inline(always)]
    fn decode_skip_run<T: Tier>(
        &mut self,
        tier: T,
        addr: &mut usize,
        run: usize,
        total: usize,
    ) -> Result<()> {
        let p = !self.hdr.slice_type.is_b();
        // The slot of reference 0 of list 0 in the head of the macroblock
        // before, where it is a P_Skip with a zero vector.
        let mut still = None;
        let mut a = *addr;
        for _ in 0..run {
            require(
                a < total && self.mbs.get(a).is_some_and(|m| m.slice == 0),
                "mb_skip_run past the picture",
            )?;
            if let Some(slot) = still {
                self.decode_still_skip(tier, a, slot)?;
            } else {
                self.start_mb(tier, a)?;
                self.decode_skip(tier)?;
                self.finish_mb::<true>();
                if p {
                    still = self.still_slot();
                }
            }
            a += 1;
        }
        *addr = a;
        Ok(())
    }

    /// The head of a P_Skip macroblock with a zero vector to the picture
    /// in slot `slot`, as [`SliceDecoder::store_mb`] writes it.
    #[inline(always)]
    fn still_head(slot: u8) -> MbHead {
        MbHead {
            m: Motion {
                ref_slot: [slot, NO_SLOT],
                ..STILL
            },
            split: false,
        }
    }

    /// The slot of the picture the current macroblock, a P_Skip one,
    /// refers to, where its vector is zero: its head is
    /// [`Self::still_head`] of it.
    #[inline(always)]
    fn still_slot(&self) -> Option<u8> {
        let head = self
            .out
            .heads
            .get(self.addr.wrapping_sub(self.motion_base))?;
        let slot = head.m.ref_slot[0];
        (*head == Self::still_head(slot)).then_some(slot)
    }

    /// A P_Skip macroblock at `addr`, the one after a P_Skip macroblock of
    /// the same mb_skip_run with a zero vector: its vector is zero too
    /// ([`SliceDecoder::decode_skip_run`]), and its head is the one that
    /// macroblock wrote, [`Self::still_head`] of `slot`.
    ///
    /// What [`SliceDecoder::start_mb`], [`SliceDecoder::decode_skip`] and
    /// [`SliceDecoder::finish_mb`] do for it, without what the macroblock
    /// before already left as this one needs it: the macroblock state
    /// (`cur`, `mb.kind`, `motion_first`, the QP flags), which a skipped
    /// macroblock does not change from one to the next of a run, and the
    /// neighbours, which only the derivation of a vector reads. The
    /// output is the full path's, bit for bit: the same head, record, ring
    /// entry and motion compensation.
    #[inline(always)]
    fn decode_still_skip<T: Tier>(&mut self, tier: T, addr: usize, slot: u8) -> Result<()> {
        debug_assert!(addr == self.addr + 1 && self.mb.kind == MbKind::PSkip);
        debug_assert!(
            self.cur.kind == MbKind::PSkip
                && self.cur.slice == self.slice_id
                && i32::from(self.cur.qp) == self.qp
                && self.cur.uniform
                && self.cur.nz == 0
                && !self.cur.t8x8
                && self.motion_first.is_none()
                && !self.prev_qp_delta_nonzero
        );
        let (col, row) = if self.mbx + 1 == self.mb_w {
            (0, self.mby + 1)
        } else {
            (self.mbx + 1, self.mby)
        };
        if row != self.mby {
            self.enter_row(row)?;
        }
        self.addr = addr;
        self.mbx = col;
        self.mby = row;
        self.issue_hints();
        if let Some(head) = self.out.heads.get_mut(addr.wrapping_sub(self.motion_base)) {
            *head = Self::still_head(slot);
        }
        if cfg!(debug_assertions) {
            self.derived = 0xffff;
        }
        self.motion_compensate(tier, crate::inter::part(0, 0, 16, 16)?, STILL)?;
        // `finish_mb::<true>` of a macroblock whose head is written.
        debug_assert!(
            self.motion_first.is_none() && self.derived == 0xffff,
            "a still macroblock as finish_mb finishes it"
        );
        if let Some(record) = self.mbs.get_mut(addr) {
            *record = self.cur;
        }
        if let Some(entry) = self.ring.get_mut(addr & self.ring_mask) {
            *entry = NbInfo::NONE;
        }
        Ok(())
    }

    /// A skipped macroblock (clause 7.3.4): no residual, and the motion is
    /// inferred.
    fn decode_skip<T: Tier>(&mut self, tier: T) -> Result<()> {
        let kind = if self.hdr.slice_type.is_b() {
            MbKind::BSkip
        } else {
            MbKind::PSkip
        };
        self.mb.kind = kind;
        self.cur.kind = kind;
        self.prev_qp_delta_nonzero = false;
        self.inter_pred(tier)
    }

    /// macroblock_layer() (clause 7.3.5).
    fn decode_mb<S: Syntax, T: Tier>(&mut self, s: &mut S, tier: T) -> Result<()> {
        self.decode_mb_body(s, tier)
    }

    /// [`Self::decode_mb`], inlined: into the CAVLC slice loop, whose
    /// syntax reader holds its window by value, which a call taking the
    /// reader by reference would put in memory.
    #[inline(always)]
    fn decode_mb_body<S: Syntax, T: Tier>(&mut self, s: &mut S, tier: T) -> Result<()> {
        self.mb.reset(tier);
        self.cur_nb = NbInfo::NONE;
        let st = self.hdr.slice_type;
        let raw = s.mb_type(self)?;
        // The intra mb_type of Table 7-11, where this is an intra
        // macroblock: the P and B tables put theirs after their own types.
        let intra_type: Option<u32> = match st {
            SliceType::I => Some(raw),
            SliceType::P | SliceType::Sp => raw.checked_sub(5),
            SliceType::B => raw.checked_sub(23),
            SliceType::Si => return Err(Error::Unsupported("SI slices")),
        };
        if let Some(it) = intra_type {
            require(it <= 25, "mb_type")?;
            if it == 25 {
                return if s.is_cabac() {
                    self.decode_pcm(s)
                } else {
                    self.decode_pcm_cavlc(s)
                };
            }
            self.start_intra::<S, T>(s, it)?;
        } else {
            self.mb.kind = if st.is_b() {
                MbKind::B(raw as u8)
            } else {
                MbKind::P(raw as u8)
            };
            self.cur.kind = self.mb.kind;
            self.parse_inter_pred::<S, T>(s)?;
        }
        let kind = self.mb.kind;
        if kind == MbKind::I16x16 {
            self.cur_nb.cbp = self.mb.cbp;
        } else {
            let cbp = s.cbp(self, kind.is_intra())?;
            self.mb.cbp = cbp;
            self.cur_nb.cbp = cbp;
            if cbp & 15 != 0
                && self.pps.transform_8x8_mode
                && !kind.is_intra()
                && self.no_sub_8x8_partitions()
            {
                let t8x8 = s.transform8x8(self)?;
                self.mb.t8x8 = t8x8;
                self.cur.t8x8 = t8x8;
            }
        }
        if self.mb.cbp != 0 || kind == MbKind::I16x16 {
            let delta = s.qp_delta(self)?;
            require((-26..=25).contains(&delta), "mb_qp_delta")?;
            self.qp = (self.qp + delta + 52) % 52;
            self.prev_qp_delta_nonzero = delta != 0;
            self.cur.qp = self.qp as i8;
            self.parse_residual(s, tier)?;
        } else {
            self.prev_qp_delta_nonzero = false;
        }
        s.check()?;
        if kind.is_intra() {
            self.intra_head();
            self.reconstruct_intra()
        } else {
            self.inter_pred(tier)?;
            self.reconstruct_inter()
        }
    }

    /// mb_pred() of an intra macroblock (clause 7.3.5.1), and the mb_type
    /// fields that go with it.
    ///
    /// `T`, unused, gives each instance of the hot paths its own copy
    /// ([`crate::tier`]).
    fn start_intra<S: Syntax, T: Tier>(&mut self, s: &mut S, intra_type: u32) -> Result<()> {
        if intra_type == 0 {
            if self.pps.transform_8x8_mode && s.transform8x8(self)? {
                self.mb.t8x8 = true;
                self.mb.kind = MbKind::I8x8;
            } else {
                self.mb.kind = MbKind::I4x4;
            }
        } else {
            self.mb.kind = MbKind::I16x16;
            self.mb.i16_mode = ((intra_type - 1) % 4) as u8;
            let chroma = ((intra_type - 1) / 4) % 3;
            let luma = if intra_type >= 13 { 15 } else { 0 };
            self.mb.cbp = (luma | (chroma << 4)) as u8;
        }
        self.cur.kind = self.mb.kind;
        self.cur.t8x8 = self.mb.t8x8;
        match self.mb.kind {
            MbKind::I4x4 => {
                self.fill_intra_cache();
                for blk in 0..16 {
                    let previous = s.prev_intra_flag()?;
                    let rem = if previous { 0 } else { s.rem_intra_mode()? };
                    let mode = self.derive_4x4_mode(blk, previous, rem);
                    if let Some(slot) = self.mb.modes.get_mut(blk) {
                        *slot = mode;
                    }
                    let (bx, by) = blk_xy(blk);
                    if let Some(slot) = self.nb.intra.get_mut(nb::at(bx, by)) {
                        *slot = mode;
                    }
                }
                self.cur_nb.intra_edge = nb::edge(&self.nb.intra);
            }
            MbKind::I8x8 => {
                self.fill_intra_cache();
                for b8 in 0..4 {
                    let previous = s.prev_intra_flag()?;
                    let rem = if previous { 0 } else { s.rem_intra_mode()? };
                    let mode = self.derive_8x8_mode(b8, previous, rem);
                    if let Some(slot) = self.mb.modes.get_mut(b8) {
                        *slot = mode;
                    }
                    let (bx, by) = ((b8 % 2) * 2, (b8 / 2) * 2);
                    for i in [nb::at(bx, by), nb::at(bx, by + 1)] {
                        if let Some(row) = self.nb.intra.get_mut(i..i + 2) {
                            row.fill(mode);
                        }
                    }
                }
                self.cur_nb.intra_edge = nb::edge(&self.nb.intra);
            }
            _ => {}
        }
        let chroma_mode = s.chroma_pred_mode(self)?;
        require(chroma_mode <= 3, "intra_chroma_pred_mode")?;
        self.mb.chroma_mode = chroma_mode;
        self.cur_nb.chroma_mode = chroma_mode;
        Ok(())
    }

    /// noSubMbPartSizeLessThan8x8Flag, which decides whether
    /// transform_size_8x8_flag is coded (clause 7.3.5).
    fn no_sub_8x8_partitions(&self) -> bool {
        match self.mb.kind {
            MbKind::P(3) | MbKind::P(4) => self.mb.sub.iter().all(|&t| t == 0),
            MbKind::B(22) => self.mb.sub.iter().all(|&ty| {
                if ty == 0 {
                    self.sps.direct_8x8_inference
                } else {
                    t::B_SUB_MB_TYPES
                        .get(usize::from(ty))
                        .is_some_and(|e| e.0 == 1)
                }
            }),
            MbKind::B(0) => self.sps.direct_8x8_inference,
            _ => true,
        }
    }

    /// An I_PCM macroblock (clause 7.3.5): its samples are written into the
    /// picture unchanged.
    fn decode_pcm<S: Syntax>(&mut self, s: &mut S) -> Result<()> {
        self.mb.kind = MbKind::IPcm;
        self.cur.kind = MbKind::IPcm;
        let mut samples = [0u8; 384];
        s.pcm(&mut samples)?;
        self.put_pcm_body(&samples)
    }

    /// [`Self::decode_pcm`] in the CAVLC slice loop: the reader's part in
    /// place, so that the reader is not taken by reference across a call
    /// ([`CavlcSyntax`]), and the samples' out of line.
    #[inline(always)]
    fn decode_pcm_cavlc<S: Syntax>(&mut self, s: &mut S) -> Result<()> {
        self.mb.kind = MbKind::IPcm;
        self.cur.kind = MbKind::IPcm;
        let mut samples = [0u8; 384];
        s.pcm(&mut samples)?;
        self.put_pcm(&samples)
    }

    /// The samples of an I_PCM macroblock into the picture, and what it
    /// leaves for later macroblocks.
    #[inline(never)]
    fn put_pcm(&mut self, samples: &[u8; 384]) -> Result<()> {
        self.put_pcm_body(samples)
    }

    #[inline(always)]
    fn put_pcm_body(&mut self, samples: &[u8; 384]) -> Result<()> {
        let stride = self.stride;
        let chroma_stride = stride / 2;
        let (x0, y0) = (self.mbx * 16, self.mby * 16 - self.luma_first);
        let cy0 = self.mby * 8 - self.chroma_first;
        let (luma_origin, chroma_origin) = (self.luma_origin, self.chroma_origin);
        let pic = &mut self.out;
        let luma = samples.get(..256).unwrap_or_default();
        for (y, row) in luma.chunks_exact(16).enumerate() {
            let start = luma_origin + (y0 + y) * stride + x0;
            let out = pic
                .y
                .get_mut(start..start + 16)
                .ok_or(Error::Geometry("an I_PCM macroblock past the picture"))?;
            out.copy_from_slice(row);
        }
        for (c, plane) in [256usize, 320].into_iter().enumerate() {
            let block = samples.get(plane..plane + 64).unwrap_or_default();
            for (y, row) in block.chunks_exact(8).enumerate() {
                let start = chroma_origin + (cy0 + y) * chroma_stride + x0 / 2;
                let dst = if c == 0 { &mut pic.cb } else { &mut pic.cr };
                let out = dst
                    .get_mut(start..start + 8)
                    .ok_or(Error::Geometry("an I_PCM macroblock past the picture"))?;
                out.copy_from_slice(row);
            }
        }
        // An I_PCM macroblock predicts as if every block had coefficients,
        // and its QPY is 0 for the deblocking filter (clause 8.7.2).
        if !self.pps.cabac {
            self.cur_nb.entropy = [[16; 8], [16; 8], [0; 8], [0; 8]];
        }
        self.cur_nb.cbf = 0x07ff_ffff;
        self.cur_nb.cbp = 0x2f;
        self.intra_head();
        self.cur.nz = 0xffff;
        self.cur.qp = self.qp as i8;
        self.prev_qp_delta_nonzero = false;
        Ok(())
    }

    /// predIntra4x4PredMode or predIntra8x8PredMode of the block whose top
    /// left 4x4 block is (`bx`, `by`) (clauses 8.3.1.1 and 8.3.2.1).
    #[inline]
    fn predicted_intra_mode(&self, bx: usize, by: usize) -> u8 {
        match nb::pair(&self.nb.intra, nb::at(bx, by), nb::W) {
            (a, b) if a == nb::NA || b == nb::NA => 2,
            (a, b) => a.min(b),
        }
    }

    /// Intra4x4PredMode of block `blk` (clause 8.3.1.1).
    fn derive_4x4_mode(&self, blk: usize, previous: bool, rem: u8) -> u8 {
        let (bx, by) = blk_xy(blk);
        decode_intra_mode(self.predicted_intra_mode(bx, by), previous, rem)
    }

    /// Intra8x8PredMode of block `b8` (clause 8.3.2.1).
    fn derive_8x8_mode(&self, b8: usize, previous: bool, rem: u8) -> u8 {
        let (bx, by) = ((b8 % 2) * 2, (b8 / 2) * 2);
        // An Intra_4x4 neighbour in A or B gives the mode of block
        // luma8x8BlkIdxN * 4 + 1 (A) or + 2 (B) (Equation 8-72), which for
        // these blocks is the adjacent one, as for an Intra_8x8 neighbour.
        decode_intra_mode(self.predicted_intra_mode(bx, by), previous, rem)
    }

    /// mb_pred() and sub_mb_pred() of an inter macroblock (clauses 7.3.5.1
    /// and 7.3.5.2): the reference indices and the motion vector
    /// differences.
    ///
    /// `T`, unused, gives each instance of the hot paths its own copy
    /// ([`crate::tier`]).
    fn parse_inter_pred<S: Syntax, T: Tier>(&mut self, s: &mut S) -> Result<()> {
        let kind = self.mb.kind;
        if kind == MbKind::B(0) {
            return Ok(());
        }
        let cabac = self.pps.cabac;
        if cabac {
            self.fill_mvd_cache();
        }
        match kind {
            MbKind::P(3) | MbKind::P(4) | MbKind::B(22) => self.parse_sub_mb_pred::<S, T>(s)?,
            _ => self.parse_mb_pred::<S, T>(s)?,
        }
        if cabac {
            let lists = self.mvd_lists();
            for (list, cache) in self.nb.mvd.iter().enumerate().take(lists) {
                if let Some(slot) = self.cur_nb.entropy.get_mut(2 * list..2 * list + 2) {
                    slot.copy_from_slice(&[nb::mvd_row(cache), nb::mvd_col(cache)]);
                }
            }
        }
        Ok(())
    }

    /// The lists whose motion vector differences a macroblock of the slice
    /// can code: list 1 only in a B slice. A P slice's macroblocks leave
    /// list 1's edges at 0, and no macroblock of another slice reads them.
    #[inline]
    fn mvd_lists(&self) -> usize {
        if self.hdr.slice_type.is_b() { 2 } else { 1 }
    }

    /// Starts the motion vector difference caches of the slice's lists
    /// from the edges of macroblocks A and B; the current macroblock's
    /// blocks start at 0, which its direct sub-macroblocks keep.
    fn fill_mvd_cache(&mut self) {
        let (a, b) = (self.mb_a(), self.mb_b());
        let lists = self.mvd_lists();
        let (ring, mask) = (&self.ring, self.ring_mask);
        for (list, cache) in self.nb.mvd.iter_mut().enumerate().take(lists) {
            // A's right column and B's bottom row (`nb::mvd_col`,
            // `nb::mvd_row`).
            let edge = |m: Option<usize>, half: usize| {
                m.and_then(|m| ring.get(m & mask))
                    .and_then(|i| i.entropy.get(2 * list + half))
            };
            nb::fill_mvd(cache, edge(a, 1), edge(b, 0));
        }
    }

    /// Starts the intra mode cache: a neighbour's modes if it is coded
    /// Intra_4x4 or Intra_8x8, 2 if it is coded in another way and
    /// [`nb::NA`] if it is not available for intra prediction
    /// (clause 8.3.1.1).
    fn fill_intra_cache(&mut self) {
        let edge = |m: Option<usize>| -> Option<Edge> {
            let m = m.filter(|&m| self.intra_allowed(m))?;
            Some(match self.info(m).kind {
                MbKind::I4x4 | MbKind::I8x8 => self.neighbour(m).intra_edge,
                _ => [2; 8],
            })
        };
        let (a, b) = (edge(self.mb_a()), edge(self.mb_b()));
        nb::fill(&mut self.nb.intra, a.as_ref(), b.as_ref(), nb::NA, 2);
    }

    /// Starts the TotalCoeff caches from the edges of macroblocks A and B.
    fn fill_total_coeff_cache(&mut self) {
        let entropy = |m: Option<usize>| m.map(|m| self.neighbour(m).entropy);
        let (a, b) = (entropy(self.mb_a()), entropy(self.mb_b()));
        nb::fill(
            &mut self.nb.tc,
            a.as_ref().map(|e| &e[0]),
            b.as_ref().map(|e| &e[0]),
            nb::NA,
            0,
        );
        for (c, cache) in self.nb.tc_c.iter_mut().enumerate() {
            let half = |e: &[Edge; 4]| {
                let v = e[1];
                if c == 0 {
                    [v[0], v[1], v[2], v[3]]
                } else {
                    [v[4], v[5], v[6], v[7]]
                }
            };
            nb::fill_c(cache, a.as_ref().map(half), b.as_ref().map(half));
        }
    }

    /// mb_pred() of a macroblock with whole-macroblock partitions.
    ///
    /// `T`, unused, gives each instance of the hot paths its own copy
    /// ([`crate::tier`]).
    fn parse_mb_pred<S: Syntax, T: Tier>(&mut self, s: &mut S) -> Result<()> {
        let (parts, modes, w, h) = self.partition_shape()?;
        let active = self.hdr.num_ref_idx_active;
        for list in 0..2 {
            for p in 0..parts {
                if !uses_list(modes, p, list) {
                    continue;
                }
                let (px, py) = part_xy(p, w, h);
                let n = active.get(list).copied().unwrap_or(0);
                let ri = if n > 1 {
                    s.ref_idx(self, list, px / 4, py / 4)?
                } else {
                    0
                };
                require(ri < n, "ref_idx past the reference list")?;
                if let Some(slot) = self.mb.ref_idx.get_mut(list).and_then(|l| l.get_mut(p)) {
                    *slot = ri as i8;
                }
                // The CABAC context of a later ref_idx asks which 8x8
                // blocks this partition covers.
                for b8 in 0..4 {
                    let (x8, y8) = ((b8 % 2) * 8, (b8 / 2) * 8);
                    let inside = x8 >= px
                        && x8 < px + usize::from(w)
                        && y8 >= py
                        && y8 < py + usize::from(h);
                    if !inside {
                        continue;
                    }
                    set_ref_gt0(&mut self.cur_nb.ref_gt0, list, b8, ri > 0);
                }
            }
        }
        for list in 0..2 {
            for p in 0..parts {
                if !uses_list(modes, p, list) {
                    continue;
                }
                let (px, py) = part_xy(p, w, h);
                let mvd = self.parse_mvd::<S, T>(s, list, px / 4, py / 4)?;
                if let Some(slot) = self.mb.mvd.get_mut(list).and_then(|l| l.get_mut(p * 4)) {
                    *slot = mvd;
                }
                self.store_mvd(
                    list,
                    px / 4,
                    py / 4,
                    usize::from(w) / 4,
                    usize::from(h) / 4,
                    mvd,
                );
            }
        }
        Ok(())
    }

    /// sub_mb_pred() (clause 7.3.5.2).
    ///
    /// `T`, unused, gives each instance of the hot paths its own copy
    /// ([`crate::tier`]).
    fn parse_sub_mb_pred<S: Syntax, T: Tier>(&mut self, s: &mut S) -> Result<()> {
        let kind = self.mb.kind;
        let is_b = matches!(kind, MbKind::B(_));
        for p in 0..4 {
            let ty = s.sub_mb_type(self)?;
            require(if is_b { ty <= 12 } else { ty <= 3 }, "sub_mb_type")?;
            if let Some(slot) = self.mb.sub.get_mut(p) {
                *slot = ty;
            }
        }
        let active = self.hdr.num_ref_idx_active;
        for list in 0..2 {
            for p in 0..4 {
                let ty = self.mb.sub.get(p).copied().unwrap_or(0);
                if !sub_uses_list(is_b, ty, list) {
                    continue;
                }
                let n = active.get(list).copied().unwrap_or(0);
                let ri = if n > 1 && kind != MbKind::P(4) {
                    s.ref_idx(self, list, (p % 2) * 2, (p / 2) * 2)?
                } else {
                    0
                };
                require(ri < n, "ref_idx past the reference list")?;
                if let Some(slot) = self.mb.ref_idx.get_mut(list).and_then(|l| l.get_mut(p)) {
                    *slot = ri as i8;
                }
                set_ref_gt0(&mut self.cur_nb.ref_gt0, list, p, ri > 0);
            }
        }
        for list in 0..2 {
            for p in 0..4 {
                let ty = self.mb.sub.get(p).copied().unwrap_or(0);
                if !sub_uses_list(is_b, ty, list) {
                    continue;
                }
                let (n, _, w, h) = sub_shape(is_b, ty);
                for sp in 0..usize::from(n) {
                    let (sx, sy) = sub_part_xy(sp, w, h);
                    let bx = (p % 2) * 2 + sx / 4;
                    let by = (p / 2) * 2 + sy / 4;
                    let mvd = self.parse_mvd::<S, T>(s, list, bx, by)?;
                    if let Some(slot) = self
                        .mb
                        .mvd
                        .get_mut(list)
                        .and_then(|l| l.get_mut(p * 4 + sp))
                    {
                        *slot = mvd;
                    }
                    self.store_mvd(list, bx, by, usize::from(w) / 4, usize::from(h) / 4, mvd);
                }
            }
        }
        Ok(())
    }

    /// Both components of one motion vector difference.
    ///
    /// `T`, unused, gives each instance of the hot paths its own copy
    /// ([`crate::tier`]).
    #[inline(always)]
    fn parse_mvd<S: Syntax, T: Tier>(
        &mut self,
        s: &mut S,
        list: usize,
        bx: usize,
        by: usize,
    ) -> Result<[i32; 2]> {
        let mvd = s.mvd(self, list, bx, by)?;
        // -8192 to 8191.75 luma samples in quarter samples
        // (clause 7.4.5.1).
        for v in mvd {
            require((-32768..=32767).contains(&v), "mvd out of range")?;
        }
        Ok(mvd)
    }

    /// The partition shape of the current macroblock (Tables 7-13, 7-14).
    fn partition_shape(&self) -> Result<(usize, [t::PredMode; 2], u8, u8)> {
        let row = match self.mb.kind {
            MbKind::P(ty) => t::P_MB_TYPES.get(usize::from(ty)).copied(),
            MbKind::B(ty) => t::B_MB_TYPES.get(usize::from(ty)).copied(),
            _ => None,
        };
        let (parts, m0, m1, w, h) = row.ok_or(Error::Invalid("mb_type"))?;
        Ok((usize::from(parts), [m0, m1], w, h))
    }

    /// Remembers the absolute motion vector difference of the blocks a
    /// partition covers, for the CABAC context of its neighbours.
    fn store_mvd(
        &mut self,
        list: usize,
        bx: usize,
        by: usize,
        bw: usize,
        bh: usize,
        mvd: [i32; 2],
    ) {
        if !self.pps.cabac {
            return;
        }
        let cap = |c: i32| u16::from(c.unsigned_abs().min(255) as u8);
        let v = cap(mvd[0]) | cap(mvd[1]) << 8;
        let Some(cache) = self.nb.mvd.get_mut(list) else {
            return;
        };
        for y in by..by + bh {
            // A width the compiler knows, which a call to `memset` would
            // cost more than.
            match bw {
                4 => nb::put_mvd::<4>(cache, nb::at(bx, y), v),
                2 => nb::put_mvd::<2>(cache, nb::at(bx, y), v),
                _ => nb::put_mvd::<1>(cache, nb::at(bx, y), v),
            }
        }
    }

    /// residual() (clause 7.3.5.3): every residual block of the macroblock,
    /// each written into its place in [`MbData`]. With CAVLC, the
    /// TotalCoeff caches start from the neighbours' edges and the
    /// macroblock's edges are kept for its own neighbours.
    ///
    /// The CAVLC instance runs in `tier`: it stays out of line, where the
    /// CABAC one is inlined into its loop, which runs in `tier` already.
    fn parse_residual<S: Syntax, T: Tier>(&mut self, s: &mut S, tier: T) -> Result<()> {
        s.residual(self, tier)
    }

    /// [`Self::parse_residual`] with CAVLC ([`Syntax::residual`]), out of
    /// line in `tier`: the window `w` is moved in and comes back in
    /// `back`, so that the slice loop's window is not taken by reference.
    /// A method of the decoder, returning a `Result<()>`: the compiler
    /// orders the instances by symbol name, and this one's falls where the
    /// CAVLC residual's instance was, between those of the CABAC path.
    #[inline(always)]
    pub(crate) fn residual_cavlc<'w, T: Tier>(
        &mut self,
        tier: T,
        w: Window<'w>,
        back: &mut Window<'w>,
    ) -> Result<()> {
        tier.enter(
            #[inline(always)]
            || {
                let mut s = CavlcSyntax::new(w, tier);
                let read = self.parse_residual_cavlc(&mut s);
                *back = s.w;
                read
            },
        )
    }

    /// [`Self::parse_residual`] with CAVLC, inside its instance
    /// ([`Self::residual_cavlc`]).
    #[inline(always)]
    fn parse_residual_cavlc<S: Syntax>(&mut self, s: &mut S) -> Result<()> {
        self.fill_total_coeff_cache();
        self.parse_residual_blocks(s)?;
        let [cb, cr] = self.nb.tc_c.map(|c| nb::edge_c(&c));
        self.cur_nb.entropy[0] = nb::edge(&self.nb.tc);
        self.cur_nb.entropy[1] = [cb[0], cb[1], cb[2], cb[3], cr[0], cr[1], cr[2], cr[3]];
        Ok(())
    }

    /// The blocks of residual().
    #[inline(always)]
    pub(crate) fn parse_residual_blocks<S: Syntax>(&mut self, s: &mut S) -> Result<()> {
        let kind = self.mb.kind;
        let cbp = self.mb.cbp;
        if s.is_cabac() {
            self.cbf_nb = self.neighbour_cbf();
        }
        self.prepare_level_scales(cbp >> 4 == 2);
        if kind == MbKind::I16x16 {
            let context = s.block_context(self, Blk::LumaDc);
            let n = s.block(
                Blk::LumaDc,
                context,
                &mut self.mb.dc,
                &SCAN_4X4,
                16,
                &CoeffScale::IDENTITY,
            )?;
            if n > 0 {
                self.cur_nb.cbf |= CBF_LUMA_DC;
                self.mb.dc_nz = true;
            }
        }
        if self.mb.t8x8 {
            for b8 in 0..4 {
                if cbp & (1 << b8) == 0 {
                    continue;
                }
                if s.is_cabac() {
                    self.parse_8x8_block(s, b8)?;
                    continue;
                }
                for i4 in 0..4 {
                    self.parse_4x4_of_8x8(s, b8, i4)?;
                }
                if self.mb.luma_nz & (1 << (16 + b8)) != 0 {
                    self.cur.nz |= quadrant(b8);
                }
            }
        } else {
            self.parse_4x4_blocks(s, cbp)?;
        }
        let chroma = cbp >> 4;
        if chroma != 0 {
            for c in 0..2 {
                let context = s.block_context(self, Blk::ChromaDc(c));
                let Some(block) = self.mb.chroma_dc.get_mut(c) else {
                    continue;
                };
                let n = s.block(
                    Blk::ChromaDc(c),
                    context,
                    block,
                    &SCAN_CHROMA_DC,
                    4,
                    &CoeffScale::IDENTITY,
                )?;
                if n > 0 {
                    self.cur_nb.cbf |= if c == 0 { CBF_CB_DC } else { CBF_CR_DC };
                    if let Some(slot) = self.mb.chroma_dc_nz.get_mut(c) {
                        *slot = true;
                    }
                }
            }
        }
        if chroma == 2 {
            for c in 0..2 {
                for b in 0..4 {
                    let kind = Blk::ChromaAc(c, b);
                    let context = s.block_context(self, kind);
                    let Some(block) = self.mb.chroma_ac.get_mut(c).and_then(|p| p.get_mut(b))
                    else {
                        continue;
                    };
                    let scale = self
                        .level_scales
                        .chroma
                        .get(c)
                        .copied()
                        .unwrap_or(&CoeffScale::IDENTITY);
                    let n = s.block(kind, context, block, &SCAN_4X4_AC, 15, scale)?;
                    if let Some(slot) = self
                        .nb
                        .tc_c
                        .get_mut(c)
                        .and_then(|cache| cache.get_mut(nb::at_c(b)))
                    {
                        *slot = n;
                    }
                    if n == 0 {
                        continue;
                    }
                    let base = if c == 0 { CBF_CB_AC0 } else { CBF_CR_AC0 };
                    self.cur_nb.cbf |= 1 << (base + b as u32);
                    if let Some(slot) = self.mb.chroma_ac_nz.get_mut(c) {
                        *slot |= 1 << b;
                    }
                }
            }
        }
        Ok(())
    }

    /// One 8x8 luma block, as CABAC codes it (clause 7.3.5.3.1).
    fn parse_8x8_block<S: Syntax>(&mut self, s: &mut S, b8: usize) -> Result<()> {
        let kind = Blk::Luma8x8(b8);
        let context = s.block_context(self, kind);
        let Some(block) = self.mb.coeff8.get_mut(b8) else {
            return Ok(());
        };
        let scale = self.level_scales.luma8;
        let n = s.block(kind, context, block, &t::ZIGZAG_8X8, 64, scale)?;
        // coded_block_flag is not coded for an 8x8 block of a 4:2:0
        // picture, and is inferred to be 1.
        self.cur_nb.cbf |= u32::from(quadrant(b8));
        if n == 0 {
            return Ok(());
        }
        self.mb.luma_nz |= 1 << (16 + b8);
        self.cur.nz |= quadrant(b8);
        Ok(())
    }

    /// The 4x4 luma blocks of the 8x8 quadrants `cbp` codes, in a
    /// macroblock without the 8x8 transform (clause 7.3.5.3). What is the
    /// same for every block, the scan, maxNumCoeff and the scaling, is taken
    /// once.
    #[inline(always)]
    fn parse_4x4_blocks<S: Syntax>(&mut self, s: &mut S, cbp: u8) -> Result<()> {
        let intra16 = self.mb.kind == MbKind::I16x16;
        let (scan, max) = if intra16 {
            (&SCAN_4X4_AC, 15)
        } else {
            (&SCAN_4X4, 16)
        };
        let scale = self.level_scales.luma4;
        for b8 in 0..4 {
            if cbp & (1 << b8) == 0 {
                continue;
            }
            for i4 in 0..4 {
                let blk = b8 * 4 + i4;
                let r = raster(blk);
                let kind = if intra16 {
                    Blk::LumaAc(blk)
                } else {
                    Blk::Luma4x4(blk)
                };
                let context = s.block_context(self, kind);
                let Some(block) = self.mb.coeff.get_mut(blk) else {
                    continue;
                };
                let n = s.block(kind, context, block, scan, max, scale)?;
                if let Some(slot) = self.nb.tc.get_mut(nb::at(r % 4, r / 4)) {
                    *slot = n;
                }
                if n != 0 {
                    self.cur_nb.cbf |= 1 << r;
                    self.mb.luma_nz |= 1 << blk;
                }
            }
        }
        // A 4x4 block's coded_block_flag bit, in raster order, is set above
        // where the block holds levels, and no other luma bit of
        // `cur_nb.cbf` is set yet: the bits are `nz`.
        self.cur.nz = self.cur_nb.cbf as u16;
        Ok(())
    }

    /// One of the four interleaved 4x4 lists in which CAVLC codes an 8x8
    /// luma block (clause 7.3.5.3.1).
    fn parse_4x4_of_8x8<S: Syntax>(&mut self, s: &mut S, b8: usize, i4: usize) -> Result<()> {
        let blk = b8 * 4 + i4;
        let r = raster(blk);
        let kind = Blk::Luma4x4(blk);
        let context = s.block_context(self, kind);
        let scan = SCAN_8X8_CAVLC.get(i4).unwrap_or(&SCAN_4X4);
        let Some(block) = self.mb.coeff8.get_mut(b8) else {
            return Ok(());
        };
        let scale = self.level_scales.luma8;
        let n = s.block(kind, context, block, scan, 16, scale)?;
        if let Some(slot) = self.nb.tc.get_mut(nb::at(r % 4, r / 4)) {
            *slot = n;
        }
        if n == 0 {
            return Ok(());
        }
        self.cur_nb.cbf |= 1 << r;
        self.mb.luma_nz |= 1 << (16 + b8);
        Ok(())
    }
}

/// Intra4x4PredMode or Intra8x8PredMode from its predictor and the coded
/// remainder (Equation 8-71).
fn decode_intra_mode(predicted: u8, previous: bool, rem: u8) -> u8 {
    if previous {
        predicted
    } else if rem < predicted {
        rem
    } else {
        rem + 1
    }
}

/// Whether macroblock partition `p` predicts from `list` (Tables 7-13 and
/// 7-14).
fn uses_list(modes: [t::PredMode; 2], p: usize, list: usize) -> bool {
    let mode = modes.get(p.min(1)).copied().unwrap_or(t::PredMode::Na);
    matches!(
        (mode, list),
        (t::PredMode::Bi, _) | (t::PredMode::L0, 0) | (t::PredMode::L1, 1)
    )
}

/// (NumSubMbPart, SubMbPredMode, SubMbPartWidth, SubMbPartHeight) of a
/// sub-macroblock type (Tables 7-17 and 7-18).
pub(crate) fn sub_shape(is_b: bool, ty: u8) -> (u8, t::PredMode, u8, u8) {
    let row = if is_b {
        t::B_SUB_MB_TYPES.get(usize::from(ty)).copied()
    } else {
        t::P_SUB_MB_TYPES.get(usize::from(ty)).copied()
    };
    row.unwrap_or((0, t::PredMode::Na, 0, 0))
}

/// Sets or clears the bit of [`NbInfo::ref_gt0`] for `list` and 8x8
/// partition `b8`.
fn set_ref_gt0(bits: &mut u8, list: usize, b8: usize, gt0: bool) {
    let bit = 1u8 << ((4 * list + b8) & 7);
    if gt0 {
        *bits |= bit;
    } else {
        *bits &= !bit;
    }
}

/// Whether a sub-macroblock partition predicts from `list`.
fn sub_uses_list(is_b: bool, ty: u8, list: usize) -> bool {
    let mode = if is_b {
        sub_shape(true, ty).1
    } else {
        t::PredMode::L0
    };
    matches!(
        (mode, list),
        (t::PredMode::Bi, _) | (t::PredMode::L0, 0) | (t::PredMode::L1, 1)
    )
}

/// Whether a picture of `mbs` macroblocks asks for its memory ahead
/// (`docs/design/h264.md`, "Round 12"): where its samples, 384 bytes a
/// macroblock, are more than 1 MiB, the smallest second-level cache of
/// the x86-64 cores the speed bars name (1 MiB on the 2.50 GHz class,
/// 1.25 MiB on the bar machine). A picture that fits there stays in that
/// cache from one macroblock row to the next, with the part of its
/// reference a row reads, and the hints would cost without saving a miss.
pub(crate) fn ahead(mbs: usize) -> bool {
    mbs.saturating_mul(384) > 1 << 20
}

/// The hints of a picture of `mbs` macroblocks decoded in `bands` bands
/// (`docs/design/h264.md`, "Threading round: a banded picture's lines
/// asked for ahead"). In one band (every picture at one thread, and a
/// picture no other reads at any count), those of [`ahead`]. In more, the
/// picture is decoded beside others on other cores, which wrote its
/// references and last read its buffers: its lines are in this core's
/// caches only if asked for, whatever its size, so every banded picture
/// asks, and asks to own its own lines.
pub(crate) fn hints(mbs: usize, bands: usize) -> u8 {
    if bands > 1 {
        BANDED_HINTS
    } else if ahead(mbs) {
        HINT_AHEAD
    } else {
        0
    }
}

/// The hints of a banded picture ([`hints`]).
const BANDED_HINTS: u8 = HINT_AHEAD | HINT_OWN;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_banded_picture_asks_whatever_its_size() {
        // 720x480 (1,350 macroblocks, 0.49 MiB) and 1920x1088 (8,160,
        // 2.99 MiB): one band follows the size rule, more bands ask for
        // every hint.
        assert_eq!(hints(1350, 1), 0);
        assert_eq!(hints(8160, 1), HINT_AHEAD);
        assert_eq!(hints(1350, 2), BANDED_HINTS);
        assert_eq!(hints(8160, 9), BANDED_HINTS);
        assert_ne!(BANDED_HINTS & HINT_AHEAD, 0);
    }

    #[test]
    fn a_quadrant_holds_the_raster_bits_of_its_four_blocks() {
        for b8 in 0..4 {
            let bits = (0..4).fold(0u16, |m, k| m | 1 << raster(b8 * 4 + k));
            assert_eq!(quadrant(b8), bits, "quadrant {b8}");
        }
    }

    /// The record a macroblock leaves in the picture is 12 bytes, and its
    /// ring entry 48.
    #[test]
    fn the_records_keep_their_sizes() {
        assert_eq!(std::mem::size_of::<MbInfo>(), 12);
        assert_eq!(std::mem::size_of::<NbInfo>(), 48);
    }

    /// Macroblocks A and B of every macroblock of a row are among the last
    /// entries of the ring written before it: no entry between theirs and
    /// the current one's shares their place.
    #[test]
    fn the_ring_keeps_a_and_b() {
        for mb_w in 1..300 {
            let n = ring_entries(mb_w);
            assert!(n.is_power_of_two() && n > mb_w && n <= 2 * mb_w);
            let mask = n - 1;
            for addr in mb_w + 1..4 * mb_w + 2 {
                for nb in [addr - 1, addr - mb_w] {
                    for later in nb + 1..=addr {
                        assert_ne!(later & mask, nb & mask, "{mb_w} {addr} {nb} {later}");
                    }
                }
            }
        }
    }
}
