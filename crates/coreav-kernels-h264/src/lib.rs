//! Pixel kernels of CoreAV's H.264 decoder: intra prediction, inverse
//! transforms, motion compensation, prediction combining and the deblocking
//! filter.
//!
//! The kernels are pure arithmetic over samples. They hold no decoder state,
//! read no bitstream and allocate nothing: the decoder hands each one a
//! [`Block`], [`BlockMut`], [`IntraMut`] or [`EdgeMut`] view that already covers exactly
//! the samples the kernel may touch, and building that view checked every
//! length, stride and range (`views`). Later implementations of the same
//! trait use SIMD and carry `unsafe` inside those checked bounds
//! (`docs/design/h264.md`, "Kernels").
//!
//! [`Scalar`] is the reference: safe Rust written from the equations of Rec.
//! ITU-T H.264 (08/2024), and what every other implementation is compared
//! against, bit for bit. [`Auto`] picks the fastest implementation the CPU
//! supports at run time, and `Auto::scalar()` forces the reference.
//!
//! ```
//! use coreav_kernels_h264::{Auto, BlockMut, Kernels};
//!
//! let mut plane = vec![100u8; 16 * 16];
//! let block = BlockMut::new(&mut plane, 0, 16, 4, 4).expect("in range");
//! // A block whose only coefficient is a DC of 64 raises it by one.
//! Auto::detect().idct4x4_dc_add(block, 64);
//! assert_eq!(plane.first(), Some(&101));
//! ```

mod intra;
mod predict;
pub mod prefetch;
mod scalar;
mod simd;
mod strengths;
mod views;

pub use predict::{MAX_ORIGIN, MAX_SIZE, Macroblocks, Ops, Part, PlaneOp, Planes, PlanesMut};
pub use scalar::Scalar;
pub use simd::Simd;
pub use strengths::{Motion, Motions, StrengthMb, StrengthSide, uniform_strengths};
pub use views::{
    Block, BlockMut, ChromaIntraMut, ChromaMbMut, EdgeMut, Geometry, IntraAvail, IntraMut,
    LumaMbMut, Reach,
};

/// The thresholds one edge of a block boundary is filtered with
/// (clause 8.7.2), per segment of `EdgeMut::segment()` lines.
///
/// Fourteen bytes in the form the tables give them: t'C0 is held by
/// boundary strength, Table 8-17's row for the edge's indexA, and each
/// segment takes the entry of its bS ([`EdgeParams::tc0_at`]). The
/// thresholds of an edge are thereby a copy of its QPs' row with the bS
/// beside it, and a vector kernel spreads t'C0 over its lines with one byte
/// shuffle by the bS (`docs/design/h264.md`, "Deblocking round: the
/// kernels' setup in the form the tables give").
///
/// The fields lie in the order declared, fourteen bytes without padding
/// (`repr(C)`; a compile-time assertion below), so that a vector kernel
/// reads the records of a call as bytes (`docs/design/h264.md`, "Kernel
/// round: chroma deblocking on bytes").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct EdgeParams {
    /// α' of Table 8-16 (Equation 8-462 at 8 bits).
    pub alpha: u8,
    /// β' of Table 8-16 (Equation 8-463 at 8 bits).
    pub beta: u8,
    /// t'C0 of Table 8-17 by boundary strength: entry `b` for bS = `b`
    /// (entries 1 to 3 are the table's; the others are not used by
    /// in-range strengths). A segment's bS selects its entry modulo 8.
    pub tc0: [u8; 8],
    /// The boundary strength bS of each segment (clause 8.7.2.1). A segment
    /// with bS = 0 is not filtered.
    pub bs: [u8; 4],
}

// Fourteen bytes, aligned to one: no padding anywhere in an array of them.
const _: () =
    assert!(core::mem::size_of::<EdgeParams>() == 14 && core::mem::align_of::<EdgeParams>() == 1);

impl EdgeParams {
    /// The t'C0 of segment `segment`: the entry of its bS (modulo 8), or 0
    /// past the fourth segment.
    pub fn tc0_at(&self, segment: usize) -> u8 {
        self.bs
            .get(segment)
            .and_then(|&b| self.tc0.get(usize::from(b & 7)))
            .copied()
            .unwrap_or(0)
    }
}

/// How the motion-compensated predictions of a block become its prediction
/// samples (clause 8.4.2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combine {
    /// One list, default weighting: the prediction is copied.
    Copy(usize),
    /// Both lists, default weighting (Equation 8-284).
    Average,
    /// One list, explicit weighting (Equations 8-286 and 8-287).
    Uni {
        /// The list the prediction comes from.
        list: usize,
        /// luma_log2_weight_denom or chroma_log2_weight_denom.
        log_wd: i32,
        /// The list's weight.
        w: i32,
        /// The list's offset.
        o: i32,
    },
    /// Both lists, explicit or implicit weighting (Equation 8-288).
    Bi {
        /// luma_log2_weight_denom or chroma_log2_weight_denom.
        log_wd: i32,
        /// The weight of list 0.
        w0: i32,
        /// The weight of list 1.
        w1: i32,
        /// The rounded mean of the two lists' offsets.
        o: i32,
    },
}

/// How [`Kernels::luma_mc_weighted`] and [`Kernels::chroma_mc_weighted`]
/// weight the prediction they interpolate (clause 8.4.2.3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weighting {
    /// One list, explicit weighting (Equations 8-286 and 8-287): the
    /// prediction, weighted, is written to the block.
    Uni {
        /// luma_log2_weight_denom or chroma_log2_weight_denom.
        log_wd: i32,
        /// The list's weight.
        w: i32,
        /// The list's offset.
        o: i32,
    },
    /// Both lists, explicit or implicit weighting (Equation 8-288): the
    /// block holds list 0's prediction, unweighted, and the prediction
    /// interpolated is list 1's.
    Bi {
        /// luma_log2_weight_denom or chroma_log2_weight_denom.
        log_wd: i32,
        /// The weight of list 0.
        w0: i32,
        /// The weight of list 1.
        w1: i32,
        /// The rounded mean of the two lists' offsets.
        o: i32,
    },
}

impl Weighting {
    /// Whether the weighting lies in the ranges the specification gives:
    /// `log_wd` 0 to 7 and offsets -128 to 127 (clause 7.4.3.2), weights
    /// -128 to 128, which also takes in the implicit weights of clause
    /// 8.4.2.3.1 (64 - w1 with w1 -64 to 128). The weighted kernels leave
    /// the block alone for any other.
    pub const fn in_range(self) -> bool {
        let (log_wd, w0, w1, o) = match self {
            Weighting::Uni { log_wd, w, o } => (log_wd, w, 0, o),
            Weighting::Bi { log_wd, w0, w1, o } => (log_wd, w0, w1, o),
        };
        0 <= log_wd
            && log_wd <= 7
            && -128 <= w0
            && w0 <= 128
            && -128 <= w1
            && w1 <= 128
            && -128 <= o
            && o <= 127
    }

    /// The same weighting as [`Kernels::combine`] computes it, with list 0
    /// the block's samples and list 1 the prediction.
    const fn combine(self) -> Combine {
        match self {
            Weighting::Uni { log_wd, w, o } => Combine::Uni {
                list: 1,
                log_wd,
                w,
                o,
            },
            Weighting::Bi { log_wd, w0, w1, o } => Combine::Bi { log_wd, w0, w1, o },
        }
    }
}

/// The kernels. Every implementation gives the same samples as [`Scalar`].
///
/// A kernel never fails: its views carry the ranges it may touch. An
/// implementation that is handed a block of the wrong shape, or views whose
/// sizes do not fit each other — neither of which the decoder does — leaves
/// the destination unchanged.
///
/// # What the values may be
///
/// The implementations agree with each other for every input in the ranges
/// the specification gives, and these are the ranges the decoder keeps to:
///
/// - **Coefficients** (`idct4x4_add`, `idct4x4_dc_add`, `idct8x8_add`,
///   `intra4x4_mb`, `intra8x8_mb`, `chroma_residual_mb`, `intra_chroma_mb`):
///   the scaled values of clauses 8.5.10 to 8.5.13, which are -2^15 to
///   2^15 - 1 for 8-bit samples. The decoder clips them there.
/// - **Intra modes**: Intra4x4PredMode and Intra8x8PredMode 0 to 8,
///   Intra16x16PredMode and intra_chroma_pred_mode 0 to 3 (clauses 8.3.1.1,
///   8.3.2.1, Table 7-11 and clause 7.4.5.1). A mode above them predicts DC
///   in every implementation.
/// - **Weights and offsets** ([`Combine`]): `log_wd` 0 to 7, and weights
///   and offsets -128 to 127 (clause 7.4.3.2). [`Weighting`] also takes
///   the implicit weight of 128 and leaves the block alone outside
///   [`Weighting::in_range`].
/// - **Thresholds** ([`EdgeParams`]): `alpha` and `beta` as Table 8-16
///   gives them (0 to 255 and 0 to 18), `tc0` as Table 8-17 does (0 to 25),
///   and `bs` 0 to 4 (clause 8.7.2.1).
///
/// Outside them an implementation still returns, and still touches only
/// what its views cover, but two implementations may differ.
pub trait Kernels: Send + Sync + 'static {
    /// Adds the inverse 4x4 transform of the scaled coefficients `coeffs`
    /// (row-major) to a 4x4 block, with clipping (clause 8.5.12.2).
    fn idct4x4_add(&self, dst: BlockMut<'_>, coeffs: &[i32; 16]);

    /// The same where only the DC coefficient is non-zero.
    fn idct4x4_dc_add(&self, dst: BlockMut<'_>, dc: i32);

    /// Adds the inverse 8x8 transform of `coeffs` to an 8x8 block
    /// (clause 8.5.13.2).
    fn idct8x8_add(&self, dst: BlockMut<'_>, coeffs: &[i32; 64]);

    /// [`Kernels::idct4x4_add`] for the four 4x4 blocks of an 8x8 block
    /// `dst`: block `k` of `coeffs` is added at column `(k % 2) * 4` and row
    /// `(k / 2) * 4`, which is the order of luma4x4BlkIdx inside an 8x8
    /// quadrant and of the AC blocks of a 4:2:0 chroma component, where bit
    /// `k` of `nz` is set. A block whose bit is clear is left unchanged, and
    /// so is a `dst` of another size.
    ///
    /// One call for up to four blocks: a vector transform pays for a call
    /// that cannot be inlined only over several blocks.
    fn idct4x4_add_quad(&self, dst: BlockMut<'_>, coeffs: &[[i32; 16]; 4], nz: u8) {
        idct4x4_add_quad_by_blocks(self, dst, coeffs, nz);
    }

    /// Intra_4x4 prediction and residual of a whole macroblock
    /// (clauses 8.3.1 and 8.5.12): for each luma4x4BlkIdx `k` in turn, the
    /// 4x4 block at its position in `dst` (a 16x16 view) is predicted in
    /// mode `modes[k]`, and where bit `k` of `nz` is set the inverse
    /// transform of `coeffs[k]` is added to it, as
    /// [`Kernels::idct4x4_add`] does, before the next block reads it.
    ///
    /// The view's [`IntraAvail`] says which of the macroblocks A, B, C and
    /// D the macroblock may predict from; each block's own neighbours
    /// follow from those and its position (clause 6.4.11.4). A `dst` of
    /// another size is left unchanged.
    fn intra4x4_mb(&self, dst: IntraMut<'_>, modes: &[u8; 16], coeffs: &[[i32; 16]; 16], nz: u16) {
        intra::intra4x4_mb(self, dst, modes, coeffs, nz);
    }

    /// The same for an Intra_8x8 macroblock (clauses 8.3.2 and 8.5.13):
    /// luma8x8BlkIdx `k` predicted in mode `modes[k]`, from neighbours
    /// filtered as clause 8.3.2.2.1 does, and `coeffs[k]` added as
    /// [`Kernels::idct8x8_add`] does where bit `k` of `nz` is set.
    fn intra8x8_mb(&self, dst: IntraMut<'_>, modes: &[u8; 4], coeffs: &[[i32; 64]; 4], nz: u8) {
        intra::intra8x8_mb(self, dst, modes, coeffs, nz);
    }

    /// Intra_16x16 prediction of a 16x16 view (clause 8.3.3), in
    /// Intra16x16PredMode `mode`. A view of another size is left unchanged.
    fn intra16x16(&self, mut dst: IntraMut<'_>, mode: u8) {
        intra::pred16x16(&mut dst, mode);
    }

    /// Intra prediction of one 8x8 chroma block of a 4:2:0 picture
    /// (clause 8.3.4), in intra_chroma_pred_mode `mode`. A view of another
    /// size is left unchanged.
    fn intra_chroma(&self, mut dst: IntraMut<'_>, mode: u8) {
        intra::pred_chroma(&mut dst, mode);
    }

    /// The residual of the chroma of a whole macroblock of a 4:2:0
    /// picture, both components in one call (clauses 8.5.11 and 8.5.12):
    /// for Cb (`cb`, `dc[0]`, `ac[0]` and bits 0 to 3 of `ac_nz`), then Cr
    /// (`cr`, `dc[1]`, `ac[1]`, bits 4 to 7), block `k` of the 8x8 block
    /// gets the inverse transform of `ac[c][k]` with its first coefficient
    /// (the DC, position 0) replaced by `dc[c][k]`, added as
    /// [`Kernels::idct4x4_add_quad`] adds it; where the block's bit of
    /// `ac_nz` is clear, its other coefficients are taken as 0 whatever
    /// `ac[c][k]` holds, and a block with neither is left unchanged. A
    /// block of another size than 8x8 is left unchanged.
    ///
    /// A block with a DC and no other coefficient, the most common kind of
    /// chroma residual, transforms to `dc` in every sample before
    /// rounding, so a kernel told which blocks have more needs no
    /// transform for it.
    fn chroma_residual_mb(
        &self,
        cb: BlockMut<'_>,
        cr: BlockMut<'_>,
        dc: &[[i32; 4]; 2],
        ac: &[[[i32; 16]; 4]; 2],
        ac_nz: u8,
    ) {
        chroma_residual_mb_by_parts(self, [cb, cr], dc, ac, ac_nz);
    }

    /// Intra prediction and residual of the chroma of a whole macroblock:
    /// each component's 8x8 block (Cb, then Cr) predicted in
    /// intra_chroma_pred_mode `mode`, as [`Kernels::intra_chroma`] does,
    /// and its residual then added as [`Kernels::chroma_residual_mb`] adds
    /// it.
    fn intra_chroma_mb(
        &self,
        dst: ChromaIntraMut<'_>,
        mode: u8,
        dc: &[[i32; 4]; 2],
        ac: &[[[i32; 16]; 4]; 2],
        ac_nz: u8,
    ) {
        intra_chroma_mb_by_parts(self, dst, mode, dc, ac, ac_nz);
    }

    /// Luma quarter-sample interpolation (clause 8.4.2.2.1).
    ///
    /// `src` is the `(w + 5)` by `(h + 5)` window that starts two samples
    /// left of and two rows above the block's integer position; `dst` is the
    /// `w` by `h` block. `xf` and `yf` are xFracL and yFracL (0 to 3).
    ///
    /// # The samples used
    ///
    /// The prediction depends only on the window samples its fraction
    /// uses (Equations 8-243 to 8-261), and every implementation gives the
    /// same prediction whatever the window's other samples hold:
    ///
    /// - with `xf` 0, no sample of the two columns before the block's and
    ///   the three after it (the window's columns 0, 1 and `w + 2` to
    ///   `w + 4`);
    /// - with `yf` 0, no sample of the two rows above the block's and the
    ///   three below it (rows 0, 1 and `h + 2` to `h + 4`);
    /// - with both 0, the block's own samples alone.
    ///
    /// With both non-zero, any sample of the window may be used. A caller
    /// may hand a window that reaches past the reference picture where
    /// those samples are unused ([`Kernels::predict`] does).
    fn luma_mc(&self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8);

    /// Chroma eighth-sample interpolation (clause 8.4.2.2.2).
    ///
    /// `src` is the `(w + 1)` by `(h + 1)` window at the block's integer
    /// position. `xf` and `yf` are xFracC and yFracC (0 to 7).
    ///
    /// # The samples used
    ///
    /// The prediction depends only on the window samples whose weight in
    /// Equation 8-266 is not 0: the window's last column is unused where
    /// `xf` is 0, and its last row where `yf` is 0. Every implementation
    /// gives the same prediction whatever the unused samples hold.
    fn chroma_mc(&self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8);

    /// [`Kernels::luma_mc`]'s prediction averaged into `dst`: each sample of
    /// `dst` becomes `(dst + pred + 1) >> 1`. With `dst` holding the other
    /// list's prediction, that is the bi-prediction of Equation 8-284 with
    /// default weights, in one pass and without a block for the second
    /// prediction. A source of another shape than `luma_mc` takes, or a
    /// block wider or taller than 16 samples, leaves `dst` unchanged. It
    /// uses the window samples `luma_mc` uses and no other.
    fn luma_mc_avg(&self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        luma_mc_avg_by_parts(self, dst, src, xf, yf);
    }

    /// [`Kernels::chroma_mc`]'s prediction averaged into `dst`, as
    /// [`Kernels::luma_mc_avg`] does for luma. A source of another shape than
    /// `chroma_mc` takes, or a block wider or taller than 16 samples (no
    /// H.264 chroma block is), leaves `dst` unchanged. It uses the window
    /// samples `chroma_mc` uses and no other.
    fn chroma_mc_avg(&self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        chroma_mc_avg_by_parts(self, dst, src, xf, yf);
    }

    /// [`Kernels::luma_mc`]'s prediction, weighted into `dst` as
    /// `weighting` says (clause 8.4.2.3.2): for [`Weighting::Uni`] `dst`
    /// becomes the weighted prediction, and for [`Weighting::Bi`] `dst`
    /// holds list 0's prediction and becomes Equation 8-288 of it and of
    /// this one, list 1's. That is [`Kernels::combine`] in the same pass
    /// as the interpolation, without a block for each list's prediction.
    /// A source of another shape than `luma_mc` takes, a block wider or
    /// taller than 16 samples, or a weighting outside
    /// [`Weighting::in_range`] leaves `dst` unchanged. It uses the window
    /// samples `luma_mc` uses and no other.
    fn luma_mc_weighted(
        &self,
        dst: BlockMut<'_>,
        src: Block<'_>,
        xf: u8,
        yf: u8,
        weighting: Weighting,
    ) {
        luma_mc_weighted_by_parts(self, dst, src, xf, yf, weighting);
    }

    /// [`Kernels::chroma_mc`]'s prediction weighted into `dst`, as
    /// [`Kernels::luma_mc_weighted`] does for luma. It uses the window
    /// samples `chroma_mc` uses and no other.
    fn chroma_mc_weighted(
        &self,
        dst: BlockMut<'_>,
        src: Block<'_>,
        xf: u8,
        yf: u8,
        weighting: Weighting,
    ) {
        chroma_mc_weighted_by_parts(self, dst, src, xf, yf, weighting);
    }

    /// The inter prediction of a block in all three planes of a 4:2:0
    /// picture, from one list: the `w` by `h` luma block at (`px`, `py`)
    /// of `dst` and its two chroma blocks, predicted from `src` with the
    /// motion vector `mv` in quarter luma samples (clause 8.4.2.2), and
    /// each plane's prediction (luma, Cb, Cr) written, averaged or
    /// weighted into its block as `ops` says, which is
    /// [`Kernels::luma_mc`], [`Kernels::luma_mc_avg`] or
    /// [`Kernels::luma_mc_weighted`] and their chroma forms on the windows
    /// and blocks the vector selects.
    ///
    /// Returns `false`, having written nothing, where it declines the
    /// block: a size other than 4, 8 or 16 each way, a weighting outside
    /// [`Weighting::in_range`], any sample the prediction uses outside the
    /// reference picture (where the specification replicates its edges),
    /// a window row wider than the stride (a plane narrower than `w + 5`),
    /// any window that does not lie in its slice of `src`, or any block
    /// outside `dst`'s planes. The caller then predicts the block another
    /// way.
    ///
    /// The samples used are those of [`Kernels::luma_mc`] and
    /// [`Kernels::chroma_mc`]: a window whose fraction leaves its apron
    /// unused across or down may reach past the picture's edge that way,
    /// into the next or previous row or into what `src`'s slices hold
    /// before each plane's origin and after its last row
    /// ([`Planes::with_origin`]), and the prediction does not depend on
    /// what it finds there. The whole window must still lie in the slice.
    ///
    /// One call checks the six ranges once and predicts the three planes,
    /// where the plane-by-plane form builds six views and makes three
    /// calls.
    #[allow(
        clippy::too_many_arguments,
        reason = "a block's position, size, vector and operations"
    )]
    fn predict(
        &self,
        dst: PlanesMut<'_>,
        src: &Planes<'_>,
        px: usize,
        py: usize,
        w: usize,
        h: usize,
        mv: [i16; 2],
        ops: [PlaneOp; 3],
    ) -> bool {
        predict::predict_by_parts(self, dst, src, px, py, w, h, mv, ops)
    }

    /// [`Kernels::predict`], for a caller compiled for x86-64-v3 (with
    /// `#[target_feature]`): the same result, but the dispatching kernels
    /// inline their vector kernel into such a caller, where
    /// [`Kernels::predict`] calls it out of line. A caller compiled for
    /// the baseline gets the same result from either, and calls
    /// [`Kernels::predict`].
    #[allow(
        clippy::too_many_arguments,
        reason = "a block's position, size, vector and operations"
    )]
    #[inline(always)]
    fn predict_inlined(
        &self,
        dst: PlanesMut<'_>,
        src: &Planes<'_>,
        px: usize,
        py: usize,
        w: usize,
        h: usize,
        mv: [i16; 2],
        ops: [PlaneOp; 3],
    ) -> bool {
        self.predict(dst, src, px, py, w, h, mv, ops)
    }

    /// [`Kernels::predict`] of the block `part` of macroblock `mb` (its
    /// column and row) of the picture whose planes `dst` are, and of
    /// which `mbs` was found ([`PlanesMut::macroblocks`]): the luma block
    /// at (`16 * mb[0] + part.x()`, `16 * mb[1] + part.y()`). The same
    /// result as [`Kernels::predict`] on those planes where `mbs` holds
    /// the macroblock; it also declines the block, writing nothing, where
    /// `mbs` does not hold the macroblock, a slice of `dst` is shorter
    /// than `mbs` holds it to (the luma slice than the luma slice it was
    /// found from, each chroma slice than the shorter of those), or
    /// `src`'s stride is not the planes'.
    ///
    /// The macroblocks a picture's planes hold are found once, so a block
    /// is checked by comparing its macroblock with them and each slice's
    /// length with the one found, where [`Kernels::predict`] checks the
    /// block's own position and offsets.
    #[allow(
        clippy::too_many_arguments,
        reason = "a block's macroblock, part, vector and operations"
    )]
    fn predict_part(
        &self,
        dst: [&mut [u8]; 3],
        mbs: &Macroblocks,
        mb: [usize; 2],
        part: Part,
        src: &Planes<'_>,
        mv: [i16; 2],
        ops: Ops,
    ) -> bool {
        predict::predict_part_by_parts(self, dst, mbs, mb, part, src, mv, ops)
    }

    /// [`Kernels::predict_part`], for a caller compiled for x86-64-v3, as
    /// [`Kernels::predict_inlined`] is [`Kernels::predict`].
    #[allow(
        clippy::too_many_arguments,
        reason = "a block's macroblock, part, vector and operations"
    )]
    #[inline(always)]
    fn predict_part_inlined(
        &self,
        dst: [&mut [u8]; 3],
        mbs: &Macroblocks,
        mb: [usize; 2],
        part: Part,
        src: &Planes<'_>,
        mv: [i16; 2],
        ops: Ops,
    ) -> bool {
        self.predict_part(dst, mbs, mb, part, src, mv, ops)
    }

    /// Writes the prediction of a block from the list 0 and list 1
    /// predictions `p0` and `p1` (clause 8.4.2.3).
    fn combine(&self, dst: BlockMut<'_>, p0: Block<'_>, p1: Block<'_>, mode: Combine);

    /// Filters one edge of one block boundary (clauses 8.7.2.3 and 8.7.2.4).
    fn filter_edge(&self, edge: EdgeMut<'_>, params: &EdgeParams);

    /// Filters the same edge of both chroma planes: [`Kernels::filter_edge`]
    /// on `cb` with `params[0]` and on `cr` with `params[1]`. The planes'
    /// thresholds may differ (chroma_qp_index_offset and
    /// second_chroma_qp_index_offset); a plane whose edge is not filtered
    /// takes an α of 0.
    ///
    /// One call for both planes: an edge of a 4:2:0 chroma plane is eight
    /// lines of two samples a side, too little work to pay for a call and a
    /// vector on its own.
    fn filter_chroma_pair(&self, cb: EdgeMut<'_>, cr: EdgeMut<'_>, params: &[EdgeParams; 2]) {
        let [for_cb, for_cr] = params;
        self.filter_edge(cb, for_cb);
        self.filter_edge(cr, for_cr);
    }

    /// Filters the chroma edges of one macroblock of a 4:2:0 picture, both
    /// planes: [`Kernels::filter_chroma_pair`] on vertical edges 0 and 4
    /// with `params[0]` and `params[1]`, then on horizontal edges 0 and 4
    /// with `params[2]` and `params[3]` ([`ChromaMbMut::edge`]), in that
    /// order. An edge whose segments all have bS 0 in both planes is not
    /// filtered, as it would filter nothing.
    ///
    /// One call for the macroblock: a macroblock that filters any chroma
    /// edge filters two to four of them on most streams, and each call to
    /// the paired kernel pays for its views and thresholds again.
    fn filter_chroma_mb(
        &self,
        cb: ChromaMbMut<'_>,
        cr: ChromaMbMut<'_>,
        params: &[[EdgeParams; 2]; 4],
    ) {
        chroma_mb_by_edges(self, cb, cr, params);
    }

    /// The boundary strengths of a macroblock's luma edges in a frame
    /// picture (clause 8.7.2.1): per direction (vertical 0, horizontal 1)
    /// and edge 0 to 3, the four segments' bS in the bytes of a word,
    /// segment 0 in the low byte.
    ///
    /// An edge that is not filtered holds zeros: edge 0 of a direction
    /// without its neighbour, and edges 1 and 3 of a macroblock with the
    /// 8x8 transform.
    fn strengths(&self, mb: &StrengthMb<'_>) -> [[u32; 4]; 2] {
        strengths::strengths(mb)
    }
}

/// [`Kernels::filter_chroma_mb`] an edge at a time, through
/// [`Kernels::filter_chroma_pair`]: the reference, and the fallback of the
/// vector kernel.
pub(crate) fn chroma_mb_by_edges<K: Kernels + ?Sized>(
    kernels: &K,
    mut cb: ChromaMbMut<'_>,
    mut cr: ChromaMbMut<'_>,
    params: &[[EdgeParams; 2]; 4],
) {
    for (slot, pair) in params.iter().enumerate() {
        if pair.iter().all(|p| p.bs == [0; 4]) {
            continue;
        }
        if let (Some(b), Some(r)) = (cb.edge(slot), cr.edge(slot)) {
            kernels.filter_chroma_pair(b, r, pair);
        }
    }
}

/// [`Kernels::intra_chroma_mb`] through [`Kernels::intra_chroma`] and
/// [`chroma_residual_by_parts`], a component at a time: the trait's
/// reference semantics, and the fallback of the vector kernel.
pub(crate) fn intra_chroma_mb_by_parts<K: Kernels + ?Sized>(
    kernels: &K,
    dst: ChromaIntraMut<'_>,
    mode: u8,
    dc: &[[i32; 4]; 2],
    ac: &[[[i32; 16]; 4]; 2],
    ac_nz: u8,
) {
    for (c, ((mut view, dc), ac)) in dst.into_parts().into_iter().zip(dc).zip(ac).enumerate() {
        let avail = view.avail();
        if let Some(v) = view.sub(0, 0, 8, avail) {
            kernels.intra_chroma(v, mode);
        }
        if let Some(block) = view.block_mut() {
            chroma_residual_by_parts(kernels, block, dc, ac, (ac_nz >> (4 * c)) & 0xf);
        }
    }
}

/// [`Kernels::chroma_residual_mb`] a component at a time through
/// [`chroma_residual_by_parts`]: the trait's reference semantics, and the
/// fallback of the vector kernel.
pub(crate) fn chroma_residual_mb_by_parts<K: Kernels + ?Sized>(
    kernels: &K,
    views: [BlockMut<'_>; 2],
    dc: &[[i32; 4]; 2],
    ac: &[[[i32; 16]; 4]; 2],
    ac_nz: u8,
) {
    for (c, ((view, dc), ac)) in views.into_iter().zip(dc).zip(ac).enumerate() {
        chroma_residual_by_parts(kernels, view, dc, ac, (ac_nz >> (4 * c)) & 0xf);
    }
}

/// One component of [`Kernels::chroma_residual_mb`] through
/// [`Kernels::idct4x4_add_quad`], with its four bits of `ac_nz`: the trait's
/// reference semantics, and the fallback of the vector kernel.
pub(crate) fn chroma_residual_by_parts<K: Kernels + ?Sized>(
    kernels: &K,
    dst: BlockMut<'_>,
    dc: &[i32; 4],
    ac: &[[i32; 16]; 4],
    ac_nz: u8,
) {
    if dst.w() != 8 || dst.h() != 8 {
        return;
    }
    let mut blocks = [[0; 16]; 4];
    let mut nz = 0;
    for (k, ((block, from), &value)) in blocks.iter_mut().zip(ac).zip(dc).enumerate() {
        if ac_nz & (1 << k) != 0 {
            *block = *from;
        }
        block[0] = value;
        if ac_nz & (1 << k) != 0 || value != 0 {
            nz |= 1 << k;
        }
    }
    if nz != 0 {
        kernels.idct4x4_add_quad(dst, &blocks, nz);
    }
}

/// The largest block the averaging kernels predict.
const AVG_MAX: usize = 16;

/// [`Kernels::luma_mc_avg`] in two steps: the prediction of `luma_mc` into
/// a block of its own, then averaged into `dst`. The trait's reference
/// semantics, and the kernels without a fused form use it. It relies on
/// `luma_mc` writing every sample of a block whose source has the right
/// shape, as every implementation does; one that declined more would
/// average zeros in.
pub(crate) fn luma_mc_avg_by_parts<K: Kernels + ?Sized>(
    kernels: &K,
    mut dst: BlockMut<'_>,
    src: Block<'_>,
    xf: u8,
    yf: u8,
) {
    let (w, h) = (dst.w(), dst.h());
    if src.w() != w + 5 || src.h() != h + 5 {
        return;
    }
    let mut pred = [0u8; AVG_MAX * AVG_MAX];
    let Ok(block) = BlockMut::new(&mut pred, 0, AVG_MAX, w, h) else {
        return;
    };
    kernels.luma_mc(block, src, xf, yf);
    average_into(&mut dst, &pred);
}

/// [`Kernels::idct4x4_add_quad`] one block at a time through
/// [`Kernels::idct4x4_add`]: the trait's reference semantics.
pub(crate) fn idct4x4_add_quad_by_blocks<K: Kernels + ?Sized>(
    kernels: &K,
    mut dst: BlockMut<'_>,
    coeffs: &[[i32; 16]; 4],
    nz: u8,
) {
    if dst.w() != 8 || dst.h() != 8 {
        return;
    }
    let stride = dst.stride();
    for (k, block) in coeffs.iter().enumerate() {
        if nz & (1 << k) == 0 {
            continue;
        }
        let offset = (k / 2) * 4 * stride + (k % 2) * 4;
        let Ok(view) = BlockMut::new(dst.samples_mut(), offset, stride, 4, 4) else {
            return;
        };
        kernels.idct4x4_add(view, block);
    }
}

/// [`Kernels::chroma_mc_avg`] in two steps, as [`luma_mc_avg_by_parts`].
pub(crate) fn chroma_mc_avg_by_parts<K: Kernels + ?Sized>(
    kernels: &K,
    mut dst: BlockMut<'_>,
    src: Block<'_>,
    xf: u8,
    yf: u8,
) {
    let (w, h) = (dst.w(), dst.h());
    if src.w() != w + 1 || src.h() != h + 1 {
        return;
    }
    let mut pred = [0u8; AVG_MAX * AVG_MAX];
    let Ok(block) = BlockMut::new(&mut pred, 0, AVG_MAX, w, h) else {
        return;
    };
    kernels.chroma_mc(block, src, xf, yf);
    average_into(&mut dst, &pred);
}

/// [`Kernels::luma_mc_weighted`] in two steps: the prediction of
/// `luma_mc` into a block of its own, then [`Kernels::combine`] of it and
/// of a copy of `dst`. The trait's reference semantics, and the kernels
/// without a fused form use it.
pub(crate) fn luma_mc_weighted_by_parts<K: Kernels + ?Sized>(
    kernels: &K,
    dst: BlockMut<'_>,
    src: Block<'_>,
    xf: u8,
    yf: u8,
    weighting: Weighting,
) {
    if src.w() != dst.w() + 5 || src.h() != dst.h() + 5 {
        return;
    }
    weighted_by_parts(kernels, dst, weighting, |block| {
        kernels.luma_mc(block, src, xf, yf);
    });
}

/// [`Kernels::chroma_mc_weighted`] in two steps, as
/// [`luma_mc_weighted_by_parts`].
pub(crate) fn chroma_mc_weighted_by_parts<K: Kernels + ?Sized>(
    kernels: &K,
    dst: BlockMut<'_>,
    src: Block<'_>,
    xf: u8,
    yf: u8,
    weighting: Weighting,
) {
    if src.w() != dst.w() + 1 || src.h() != dst.h() + 1 {
        return;
    }
    weighted_by_parts(kernels, dst, weighting, |block| {
        kernels.chroma_mc(block, src, xf, yf);
    });
}

/// Predicts with `predict` into a block of its own and weights it into
/// `dst` through [`Kernels::combine`], list 0 being a copy of `dst`.
fn weighted_by_parts<K: Kernels + ?Sized>(
    kernels: &K,
    mut dst: BlockMut<'_>,
    weighting: Weighting,
    predict: impl FnOnce(BlockMut<'_>),
) {
    let (w, h) = (dst.w(), dst.h());
    if w > AVG_MAX || h > AVG_MAX || !weighting.in_range() {
        return;
    }
    let mut pred = [0u8; AVG_MAX * AVG_MAX];
    let Ok(block) = BlockMut::new(&mut pred, 0, AVG_MAX, w, h) else {
        return;
    };
    predict(block);
    let mut list0 = [0u8; AVG_MAX * AVG_MAX];
    if matches!(weighting, Weighting::Bi { .. }) {
        for (to, from) in list0.chunks_exact_mut(AVG_MAX).zip(dst.rows_mut()) {
            let n = from.len().min(to.len());
            if let (Some(t), Some(f)) = (to.get_mut(..n), from.get(..n)) {
                t.copy_from_slice(f);
            }
        }
    }
    let (Ok(p0), Ok(p1)) = (
        Block::new(&list0, 0, AVG_MAX, w, h),
        Block::new(&pred, 0, AVG_MAX, w, h),
    ) else {
        return;
    };
    kernels.combine(dst, p0, p1, weighting.combine());
}

/// `dst = (dst + pred + 1) >> 1`, with `pred`'s rows `AVG_MAX` apart.
fn average_into(dst: &mut BlockMut<'_>, pred: &[u8; AVG_MAX * AVG_MAX]) {
    for (out, row) in dst.rows_mut().zip(pred.chunks_exact(AVG_MAX)) {
        for (o, p) in out.iter_mut().zip(row) {
            *o = ((u16::from(*o) + u16::from(*p) + 1) >> 1) as u8;
        }
    }
}

/// Averages the first `N` samples of each row of `src` into `dst`: the
/// full-sample case of [`Kernels::chroma_mc_avg`], with `N` known at
/// compile time as in [`copy_rows`].
fn average_rows<const N: usize>(dst: &mut BlockMut<'_>, src: &Block<'_>) {
    for_rows::<N>(dst, src, |o, u| {
        for (o, p) in o.iter_mut().zip(u) {
            *o = ((u16::from(*o) + u16::from(*p) + 1) >> 1) as u8;
        }
    });
}

/// Copies the first `N` samples of each row of `src` into `dst`, with `N`
/// known at compile time so that a row becomes a move rather than a call.
fn copy_rows<const N: usize>(dst: &mut BlockMut<'_>, src: &Block<'_>) {
    for_rows::<N>(dst, src, |o, u| *o = *u);
}

/// Calls `f` with the first `N` samples of each of `dst`'s rows and of the
/// row of `src` at the same height. Rows are reached by offset, each as an
/// array: one bounds check a row, where the views' row iterators take
/// several, which on a row of eight samples cost more than the copy. The
/// views' constructors checked that every row lies in its slice, so no
/// check fails; one that did would leave its row alone.
#[inline(always)]
fn for_rows<const N: usize>(
    dst: &mut BlockMut<'_>,
    src: &Block<'_>,
    f: impl Fn(&mut [u8; N], &[u8; N]),
) {
    let (h, from_stride, to_stride) = (dst.h(), src.stride(), dst.stride());
    let from = src.samples();
    let to = dst.samples_mut();
    let (mut s, mut d) = (0, 0);
    for _ in 0..h {
        let up = from.get(s..).and_then(|rest| rest.first_chunk::<N>());
        let out = to.get_mut(d..).and_then(|rest| rest.first_chunk_mut::<N>());
        if let (Some(o), Some(u)) = (out, up) {
            f(o, u);
        }
        // Wrapping: past the last row the offsets are never used.
        s = s.wrapping_add(from_stride);
        d = d.wrapping_add(to_stride);
    }
}

/// [`Scalar`]'s deblocking filter, kept out of line for [`Auto`]'s
/// inlined dispatch.
#[inline(never)]
fn scalar_filter_edge(edge: EdgeMut<'_>, params: &EdgeParams) {
    Scalar.filter_edge(edge, params);
}

/// The fastest kernels this CPU supports.
///
/// [`Auto::detect`] looks for the vector kernels of [`Simd`] once, and each
/// call then goes to whichever implementation is faster for its shape
/// (`docs/measurements/h264-kernels.md`). [`Auto::scalar`] keeps every call
/// in the reference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Auto {
    /// Detected once, when the `Auto` is made: holding a [`Simd`] is the
    /// proof that the CPU supports the kernels it dispatches to.
    simd: Option<Simd>,
}

impl Auto {
    /// The fastest kernels the CPU this process runs on supports.
    pub fn detect() -> Auto {
        Auto {
            simd: Simd::detect(),
        }
    }

    /// The reference kernels, whatever the CPU supports.
    pub const fn scalar() -> Auto {
        Auto { simd: None }
    }

    /// The same kernels without the AVX-512 tier ([`Simd::without_avx512`]):
    /// what a CPU with AVX2 alone runs.
    pub const fn without_avx512(self) -> Auto {
        match self.simd {
            Some(simd) => Auto {
                simd: Some(simd.without_avx512()),
            },
            None => self,
        }
    }

    /// Whether [`Auto::detect`] found SIMD kernels for this CPU.
    pub const fn uses_simd(&self) -> bool {
        self.simd.is_some()
    }

    /// [`Kernels::filter_chroma_mb`] where no vector kernel is available.
    #[cold]
    #[inline(never)]
    fn chroma_mb_without_simd(
        &self,
        cb: ChromaMbMut<'_>,
        cr: ChromaMbMut<'_>,
        params: &[[EdgeParams; 2]; 4],
    ) {
        chroma_mb_by_edges(self, cb, cr, params);
    }
}

impl Kernels for Auto {
    fn idct4x4_add(&self, dst: BlockMut<'_>, coeffs: &[i32; 16]) {
        // There is no vector 4x4 transform; `Simd` hands it back to the
        // reference, and calling it here saves the hop.
        Scalar.idct4x4_add(dst, coeffs);
    }

    fn idct4x4_dc_add(&self, dst: BlockMut<'_>, dc: i32) {
        Scalar.idct4x4_dc_add(dst, dc);
    }

    fn idct8x8_add(&self, dst: BlockMut<'_>, coeffs: &[i32; 64]) {
        match self.simd {
            Some(simd) => simd.idct8x8_add(dst, coeffs),
            None => Scalar.idct8x8_add(dst, coeffs),
        }
    }

    fn idct4x4_add_quad(&self, dst: BlockMut<'_>, coeffs: &[[i32; 16]; 4], nz: u8) {
        match self.simd {
            Some(simd) => simd.idct4x4_add_quad(dst, coeffs, nz),
            None => idct4x4_add_quad_by_blocks(&Scalar, dst, coeffs, nz),
        }
    }

    fn intra4x4_mb(&self, dst: IntraMut<'_>, modes: &[u8; 16], coeffs: &[[i32; 16]; 16], nz: u16) {
        match self.simd {
            Some(simd) => simd.intra4x4_mb(dst, modes, coeffs, nz),
            None => Scalar.intra4x4_mb(dst, modes, coeffs, nz),
        }
    }

    fn intra8x8_mb(&self, dst: IntraMut<'_>, modes: &[u8; 4], coeffs: &[[i32; 64]; 4], nz: u8) {
        match self.simd {
            Some(simd) => simd.intra8x8_mb(dst, modes, coeffs, nz),
            None => Scalar.intra8x8_mb(dst, modes, coeffs, nz),
        }
    }

    fn intra16x16(&self, dst: IntraMut<'_>, mode: u8) {
        match self.simd {
            Some(simd) => simd.intra16x16(dst, mode),
            None => Scalar.intra16x16(dst, mode),
        }
    }

    fn intra_chroma(&self, dst: IntraMut<'_>, mode: u8) {
        match self.simd {
            Some(simd) => simd.intra_chroma(dst, mode),
            None => Scalar.intra_chroma(dst, mode),
        }
    }

    fn chroma_residual_mb(
        &self,
        cb: BlockMut<'_>,
        cr: BlockMut<'_>,
        dc: &[[i32; 4]; 2],
        ac: &[[[i32; 16]; 4]; 2],
        ac_nz: u8,
    ) {
        match self.simd {
            Some(simd) => simd.chroma_residual_mb(cb, cr, dc, ac, ac_nz),
            None => Scalar.chroma_residual_mb(cb, cr, dc, ac, ac_nz),
        }
    }

    fn intra_chroma_mb(
        &self,
        dst: ChromaIntraMut<'_>,
        mode: u8,
        dc: &[[i32; 4]; 2],
        ac: &[[[i32; 16]; 4]; 2],
        ac_nz: u8,
    ) {
        match self.simd {
            Some(simd) => simd.intra_chroma_mb(dst, mode, dc, ac, ac_nz),
            None => Scalar.intra_chroma_mb(dst, mode, dc, ac, ac_nz),
        }
    }

    fn luma_mc(&self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        // No full-sample shortcut here, unlike `chroma_mc`: both kernels
        // already predict a full-sample block by copying it, and taking
        // that copy at this level measured worse (see
        // `docs/measurements/h264-decoder-speed.md`).
        match self.simd {
            Some(simd) if Simd::prefers_luma_mc(dst.w(), xf, yf) => {
                simd.luma_mc(dst, src, xf, yf);
            }
            _ => Scalar.luma_mc(dst, src, xf, yf),
        }
    }

    fn chroma_mc(&self, mut dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        // At a full sample position equation 8-266 weights the upper-left
        // sample by 64 and the other three by 0, so the prediction is a
        // copy of the source. Most chroma blocks of a typical stream land
        // here, and the general path would run the whole 4-tap filter to
        // reach the same bytes.
        // `Scalar::chroma_mc` writes nothing unless the source is exactly
        // the (w + 1) by (h + 1) window the trait asks for, so the copy
        // has to decline the same shapes it would: a kernel that is
        // chosen by dispatch must not be distinguishable from the
        // reference by what it writes (bar E1).
        let (w, h) = (dst.w(), dst.h());
        let exact = src.w() == w + 1 && src.h() == h + 1;
        if exact && xf & 7 == 0 && yf & 7 == 0 {
            // Chroma blocks are 2, 4 or 8 samples wide. Copying a row of a
            // width the compiler knows becomes one move; a width it only
            // learns at run time becomes a call to `memcpy` per row, which
            // for eight bytes costs more than the copy.
            match w {
                2 => copy_rows::<2>(&mut dst, &src),
                4 => copy_rows::<4>(&mut dst, &src),
                8 => copy_rows::<8>(&mut dst, &src),
                _ => {
                    for (out, up) in dst.rows_mut().zip(src.rows()) {
                        let n = out.len().min(up.len());
                        if let (Some(o), Some(u)) = (out.get_mut(..n), up.get(..n)) {
                            o.copy_from_slice(u);
                        }
                    }
                }
            }
            return;
        }
        match self.simd {
            Some(simd) if Simd::prefers_chroma_mc(dst.w()) => simd.chroma_mc(dst, src, xf, yf),
            _ => Scalar.chroma_mc(dst, src, xf, yf),
        }
    }

    fn luma_mc_avg(&self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        // The rule of `luma_mc`, measured for the plain prediction. It is
        // provisional for the averaging form until the round that has the
        // decoder use it measures that form.
        match self.simd {
            Some(simd) if Simd::prefers_luma_mc(dst.w(), xf, yf) => {
                simd.luma_mc_avg(dst, src, xf, yf);
            }
            _ => Scalar.luma_mc_avg(dst, src, xf, yf),
        }
    }

    fn chroma_mc_avg(&self, mut dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        // As `chroma_mc`, with each predicted sample averaged into `dst` as
        // it is computed: the shapes, the full-sample rows and the vector
        // rule are those of `chroma_mc`, and a block larger than the
        // trait's limit is left alone, as the reference leaves it.
        let (w, h) = (dst.w(), dst.h());
        if src.w() != w + 1 || src.h() != h + 1 || w > AVG_MAX || h > AVG_MAX {
            return;
        }
        if xf & 7 == 0 && yf & 7 == 0 {
            match w {
                2 => return average_rows::<2>(&mut dst, &src),
                4 => return average_rows::<4>(&mut dst, &src),
                8 => return average_rows::<8>(&mut dst, &src),
                _ => {}
            }
        }
        if let Some(simd) = self.simd {
            if Simd::prefers_chroma_mc(w) {
                return simd.chroma_mc_avg(dst, src, xf, yf);
            }
        }
        scalar::chroma_mc_with(&mut dst, &src, xf, yf, |o, v| {
            *o = ((u16::from(*o) + u16::from(v) + 1) >> 1) as u8;
        });
    }

    fn luma_mc_weighted(
        &self,
        dst: BlockMut<'_>,
        src: Block<'_>,
        xf: u8,
        yf: u8,
        weighting: Weighting,
    ) {
        // The rule of `luma_mc`. Elsewhere the prediction and the
        // weighting are each dispatched on their own.
        match self.simd {
            Some(simd) if Simd::prefers_luma_mc(dst.w(), xf, yf) => {
                simd.luma_mc_weighted(dst, src, xf, yf, weighting);
            }
            _ => luma_mc_weighted_by_parts(self, dst, src, xf, yf, weighting),
        }
    }

    fn chroma_mc_weighted(
        &self,
        dst: BlockMut<'_>,
        src: Block<'_>,
        xf: u8,
        yf: u8,
        weighting: Weighting,
    ) {
        match self.simd {
            Some(simd) if Simd::prefers_chroma_mc(dst.w()) => {
                simd.chroma_mc_weighted(dst, src, xf, yf, weighting);
            }
            _ => chroma_mc_weighted_by_parts(self, dst, src, xf, yf, weighting),
        }
    }

    fn predict(
        &self,
        dst: PlanesMut<'_>,
        src: &Planes<'_>,
        px: usize,
        py: usize,
        w: usize,
        h: usize,
        mv: [i16; 2],
        ops: [PlaneOp; 3],
    ) -> bool {
        match self.simd {
            Some(simd) => simd.predict(dst, src, px, py, w, h, mv, ops),
            None => predict::predict_by_parts(&Scalar, dst, src, px, py, w, h, mv, ops),
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[inline(always)]
    fn predict_inlined(
        &self,
        dst: PlanesMut<'_>,
        src: &Planes<'_>,
        px: usize,
        py: usize,
        w: usize,
        h: usize,
        mv: [i16; 2],
        ops: [PlaneOp; 3],
    ) -> bool {
        match self.simd {
            Some(simd) => simd.predict_inlined(dst, src, px, py, w, h, mv, ops),
            None => predict::predict_by_parts(&Scalar, dst, src, px, py, w, h, mv, ops),
        }
    }

    fn predict_part(
        &self,
        dst: [&mut [u8]; 3],
        mbs: &Macroblocks,
        mb: [usize; 2],
        part: Part,
        src: &Planes<'_>,
        mv: [i16; 2],
        ops: Ops,
    ) -> bool {
        match self.simd {
            Some(simd) => simd.predict_part(dst, mbs, mb, part, src, mv, ops),
            None => predict::predict_part_by_parts(&Scalar, dst, mbs, mb, part, src, mv, ops),
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[inline(always)]
    fn predict_part_inlined(
        &self,
        dst: [&mut [u8]; 3],
        mbs: &Macroblocks,
        mb: [usize; 2],
        part: Part,
        src: &Planes<'_>,
        mv: [i16; 2],
        ops: Ops,
    ) -> bool {
        match self.simd {
            Some(simd) => simd.predict_part_inlined(dst, mbs, mb, part, src, mv, ops),
            None => predict::predict_part_by_parts(&Scalar, dst, mbs, mb, part, src, mv, ops),
        }
    }

    fn combine(&self, dst: BlockMut<'_>, p0: Block<'_>, p1: Block<'_>, mode: Combine) {
        match self.simd {
            Some(simd) if Simd::prefers_combine(mode) => simd.combine(dst, p0, p1, mode),
            _ => Scalar.combine(dst, p0, p1, mode),
        }
    }

    // Inlined, so that where the decoder builds an edge its plane and
    // length are constants and the vector kernel's choice of function
    // folds away (`simd::x86::filter_edge`). The reference stays out of
    // line, so that it does not make every call site large.
    #[inline(always)]
    fn filter_edge(&self, edge: EdgeMut<'_>, params: &EdgeParams) {
        match self.simd {
            Some(simd) if Simd::prefers_filter_edge(edge.reach(), edge.across()) => {
                simd.filter_edge(edge, params);
            }
            _ => scalar_filter_edge(edge, params),
        }
    }

    fn filter_chroma_pair(&self, cb: EdgeMut<'_>, cr: EdgeMut<'_>, params: &[EdgeParams; 2]) {
        // Only x86 has a paired kernel; elsewhere each plane goes through
        // `filter_edge`, so that its measured choice per edge still holds.
        #[cfg(target_arch = "x86_64")]
        if let Some(simd) = self.simd {
            return simd.filter_chroma_pair(cb, cr, params);
        }
        let [for_cb, for_cr] = params;
        self.filter_edge(cb, for_cb);
        self.filter_edge(cr, for_cr);
    }

    fn filter_chroma_mb(
        &self,
        cb: ChromaMbMut<'_>,
        cr: ChromaMbMut<'_>,
        params: &[[EdgeParams; 2]; 4],
    ) {
        // As for the pair: only x86 has a kernel for the macroblock. The
        // edge-by-edge path is out of line, so that this call, made for
        // every macroblock, is a test and a jump to the kernel.
        #[cfg(target_arch = "x86_64")]
        if let Some(simd) = self.simd {
            return simd.filter_chroma_mb(cb, cr, params);
        }
        self.chroma_mb_without_simd(cb, cr, params);
    }

    #[inline(always)]
    fn strengths(&self, mb: &StrengthMb<'_>) -> [[u32; 4]; 2] {
        // Only the AVX-512 tier has a vector kernel; the reference is
        // inlined into the caller elsewhere.
        match self.simd {
            Some(simd) if simd.has_avx512() && !mb.all_one() => simd.strengths(mb),
            _ => strengths::strengths(mb),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plane of `w` by `h` samples with a reproducible pattern.
    fn plane(w: usize, h: usize, seed: u32) -> Vec<u8> {
        let mut s = seed | 1;
        (0..w * h)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn a_dispatched_chroma_kernel_writes_what_the_reference_writes() {
        // The copy path must decline every shape the reference declines.
        // `Scalar::chroma_mc` writes nothing unless the source is exactly
        // (w + 1) by (h + 1); an earlier version of the copy accepted any
        // source at least w wide, so `Auto` wrote a block where `Scalar`
        // wrote none. The decoder never built such a call, which is why
        // only a test over the trait's own shapes could find it.
        for (w, h) in [(2usize, 2usize), (4, 4), (8, 8), (8, 4), (2, 4)] {
            for (sw, sh) in [
                (w, h),
                (w + 1, h + 1),
                (w + 5, h + 5),
                (w + 1, h),
                (w, h + 1),
            ] {
                let src = plane(sw.max(1), sh.max(1), 0x1234_5678);
                let mut fast = vec![7u8; w * h];
                let mut general = vec![7u8; w * h];
                let source = || Block::new(&src, 0, sw, sw, sh).expect("source");
                Auto::detect().chroma_mc(
                    BlockMut::new(&mut fast, 0, w, w, h).expect("block"),
                    source(),
                    0,
                    0,
                );
                Scalar.chroma_mc(
                    BlockMut::new(&mut general, 0, w, w, h).expect("block"),
                    source(),
                    0,
                    0,
                );
                assert_eq!(fast, general, "dst {w}x{h} from src {sw}x{sh}");
            }
        }
    }

    #[test]
    fn a_one_row_chroma_block_of_any_stride_matches_the_reference() {
        // A view of one row may have any stride, up to usize::MAX. The
        // vector kernel once advanced its offsets past the last row with
        // arithmetic that overflowed there, a panic with overflow checks
        // on. Both forms, every width, against the reference.
        for (w, src_w) in [(2usize, 3usize), (4, 5), (8, 9)] {
            let src = plane(src_w, 2, 0x7a3c_19e5);
            for (xf, yf) in [(0u8, 0u8), (3, 5), (7, 0), (0, 7)] {
                for avg in [false, true] {
                    let run = |kernels: &dyn Kernels| {
                        let mut out = plane(w, 1, 0x0bad_cafe);
                        let block = BlockMut::new(&mut out, 0, usize::MAX, w, 1).expect("block");
                        let source = Block::new(&src, 0, src_w, src_w, 2).expect("source");
                        if avg {
                            kernels.chroma_mc_avg(block, source, xf, yf);
                        } else {
                            kernels.chroma_mc(block, source, xf, yf);
                        }
                        out
                    };
                    let want = run(&Scalar);
                    assert_eq!(run(&Auto::detect()), want, "{w} ({xf}, {yf}) avg {avg}");
                    if let Some(simd) = Simd::detect() {
                        assert_eq!(run(&simd), want, "simd {w} ({xf}, {yf}) avg {avg}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_full_sample_chroma_block_is_the_source() {
        // Auto takes a copy path at xFracC = yFracC = 0; it has to agree
        // with the general form of equation 8-266, which the scalar kernel
        // computes, for every block size chroma uses.
        for (w, h) in [(2usize, 2usize), (4, 4), (8, 8), (8, 4), (2, 4)] {
            let src = plane(w + 1, h + 1, 0x51ed_3c77);
            let mut fast = vec![0u8; w * h];
            let mut general = vec![0u8; w * h];
            let source = || Block::new(&src, 0, w + 1, w + 1, h + 1).expect("source");
            Auto::detect().chroma_mc(
                BlockMut::new(&mut fast, 0, w, w, h).expect("block"),
                source(),
                0,
                0,
            );
            Scalar.chroma_mc(
                BlockMut::new(&mut general, 0, w, w, h).expect("block"),
                source(),
                0,
                0,
            );
            assert_eq!(fast, general, "{w}x{h}");
        }
    }

    #[test]
    fn a_full_sample_luma_block_is_the_source() {
        // Auto copies at xFracL = yFracL = 0; it has to agree with the
        // general form the scalar kernel computes, at every luma size.
        for (w, h) in [(4usize, 4usize), (8, 8), (16, 16), (8, 16), (16, 8)] {
            let src = plane(w + 5, h + 5, 0x2c81_9f05);
            let mut fast = vec![0u8; w * h];
            let mut general = vec![0u8; w * h];
            let source = || Block::new(&src, 0, w + 5, w + 5, h + 5).expect("source");
            Auto::detect().luma_mc(
                BlockMut::new(&mut fast, 0, w, w, h).expect("block"),
                source(),
                0,
                0,
            );
            Scalar.luma_mc(
                BlockMut::new(&mut general, 0, w, w, h).expect("block"),
                source(),
                0,
                0,
            );
            assert_eq!(fast, general, "{w}x{h}");
        }
    }

    #[test]
    fn dc_transform_equals_the_full_transform() {
        for dc in [-2048, -64, 0, 1, 63, 64, 4096] {
            let mut full = vec![128u8; 64];
            let mut only_dc = vec![128u8; 64];
            let mut coeffs = [0i32; 16];
            if let Some(c) = coeffs.first_mut() {
                *c = dc;
            }
            Scalar.idct4x4_add(
                BlockMut::new(&mut full, 0, 8, 4, 4).expect("block"),
                &coeffs,
            );
            Scalar.idct4x4_dc_add(BlockMut::new(&mut only_dc, 0, 8, 4, 4).expect("block"), dc);
            assert_eq!(full, only_dc, "dc {dc}");
        }
    }

    #[test]
    fn transforms_clip_and_leave_the_rest_of_the_plane_alone() {
        let mut p = vec![250u8; 16 * 16];
        let coeffs = [1000i32; 64];
        Scalar.idct8x8_add(BlockMut::new(&mut p, 17, 16, 8, 8).expect("block"), &coeffs);
        assert_eq!(p.first(), Some(&250));
        assert_eq!(p.get(17), Some(&255));
        // Outside the 8x8 block at offset 17 nothing changed.
        assert_eq!(p.get(16 * 9 + 1), Some(&250));
    }

    #[test]
    fn full_sample_interpolation_copies_the_block() {
        let src = plane(21, 21, 7);
        let mut dst = vec![0u8; 16 * 16];
        Scalar.luma_mc(
            BlockMut::new(&mut dst, 0, 16, 16, 16).expect("block"),
            Block::new(&src, 0, 21, 21, 21).expect("block"),
            0,
            0,
        );
        for (y, row) in dst.chunks_exact(16).enumerate() {
            let want = src.get((y + 2) * 21 + 2..(y + 2) * 21 + 18).expect("row");
            assert_eq!(row, want, "row {y}");
        }
    }

    #[test]
    fn half_sample_interpolation_follows_the_equations() {
        // A single row of the source, so b is the horizontal filter alone.
        let mut src = vec![0u8; 21 * 6];
        for (i, s) in src.iter_mut().enumerate() {
            *s = ((i % 21) * 10) as u8;
        }
        let mut dst = vec![0u8; 4];
        Scalar.luma_mc(
            BlockMut::new(&mut dst, 0, 4, 4, 1).expect("block"),
            Block::new(&src, 0, 21, 9, 6).expect("block"),
            2,
            0,
        );
        // Row 2 holds 0, 10, 20, ...; the filter of a straight line is the
        // midpoint, so b at x is 10 * x + 5.
        assert_eq!(dst, vec![25, 35, 45, 55]);
    }

    #[test]
    fn chroma_interpolation_interpolates_bilinearly() {
        let src = [0u8, 8, 0, 8, 0, 8, 0, 8, 0];
        let mut dst = [0u8; 4];
        Scalar.chroma_mc(
            BlockMut::new(&mut dst, 0, 2, 2, 2).expect("block"),
            Block::new(&src, 0, 3, 3, 3).expect("block"),
            4,
            0,
        );
        assert_eq!(dst, [4, 4, 4, 4]);
    }

    #[test]
    fn combine_averages_and_weights() {
        let a = vec![100u8; 16];
        let b = vec![200u8; 16];
        let mut dst = vec![0u8; 16];
        Scalar.combine(
            BlockMut::new(&mut dst, 0, 4, 4, 4).expect("block"),
            Block::new(&a, 0, 4, 4, 4).expect("block"),
            Block::new(&b, 0, 4, 4, 4).expect("block"),
            Combine::Average,
        );
        assert!(dst.iter().all(|&v| v == 150));
        Scalar.combine(
            BlockMut::new(&mut dst, 0, 4, 4, 4).expect("block"),
            Block::new(&a, 0, 4, 4, 4).expect("block"),
            Block::new(&b, 0, 4, 4, 4).expect("block"),
            Combine::Uni {
                list: 1,
                log_wd: 1,
                w: 1,
                o: 3,
            },
        );
        assert!(dst.iter().all(|&v| v == 103));
    }

    #[test]
    fn an_edge_with_strength_zero_is_left_alone() {
        let before = plane(16, 16, 3);
        let mut after = before.clone();
        let edge = EdgeMut::new(&mut after, 4 * 16 + 8, 16, 1, 16, Reach::Luma).expect("edge");
        Scalar.filter_edge(
            edge,
            &EdgeParams {
                alpha: 255,
                beta: 18,
                tc0: [25; 8],
                bs: [0; 4],
            },
        );
        assert_eq!(before, after);
    }

    #[test]
    fn a_flat_block_boundary_stays_flat() {
        // Samples on both sides are equal, so every filter output equals the
        // input, whatever the strength.
        for bs in 1..=4u8 {
            let mut p = vec![128u8; 16 * 16];
            let edge = EdgeMut::new(&mut p, 8, 1, 16, 16, Reach::Luma).expect("edge");
            Scalar.filter_edge(
                edge,
                &EdgeParams {
                    alpha: 255,
                    beta: 18,
                    tc0: [25; 8],
                    bs: [bs; 4],
                },
            );
            assert!(p.iter().all(|&v| v == 128), "bs {bs}");
        }
    }

    #[test]
    fn a_strong_edge_is_smoothed_within_the_threshold() {
        // A step from 100 to 140 across the edge, bS = 1: p0 and q0 move
        // toward each other by at most tC.
        let mut p = vec![0u8; 16 * 16];
        for (x, s) in p.iter_mut().enumerate() {
            *s = if x % 16 < 8 { 100 } else { 140 };
        }
        let edge = EdgeMut::new(&mut p, 8, 1, 16, 16, Reach::Luma).expect("edge");
        Scalar.filter_edge(
            edge,
            &EdgeParams {
                alpha: 255,
                beta: 18,
                tc0: [4; 8],
                bs: [1; 4],
            },
        );
        // ap and aq are 0, so tC = tC0 + 2 = 6 and delta is clipped to 6.
        assert_eq!(p.get(7), Some(&106));
        assert_eq!(p.get(8), Some(&134));
        // p1 and q1 move by the clipped second-order term.
        assert_eq!(p.get(6), Some(&104));
        assert_eq!(p.get(9), Some(&136));
        // p3 and q3 are untouched by a bS below 4.
        assert_eq!(p.get(4), Some(&100));
        assert_eq!(p.get(11), Some(&140));
    }
}
