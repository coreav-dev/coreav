//! What the macroblock layer tells the two entropy coders about a residual
//! block: its kind, and the most coefficients a block can hold.
#![forbid(unsafe_code)]

/// A residual block's ctxBlockCat (Table 9-42) together with which block of
/// the macroblock it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Blk {
    /// The Intra_16x16 DC block (ctxBlockCat 0).
    LumaDc,
    /// The AC coefficients of luma block `luma4x4BlkIdx` of an Intra_16x16
    /// macroblock (ctxBlockCat 1).
    LumaAc(usize),
    /// A whole 4x4 luma block (ctxBlockCat 2).
    Luma4x4(usize),
    /// The chroma DC block of component `c` (ctxBlockCat 3).
    ChromaDc(usize),
    /// The AC coefficients of chroma block `b` of component `c`
    /// (ctxBlockCat 4).
    ChromaAc(usize, usize),
    /// A whole 8x8 luma block (ctxBlockCat 5).
    Luma8x8(usize),
}

impl Blk {
    /// ctxBlockCat (Table 9-42).
    pub(crate) fn category(self) -> usize {
        match self {
            Blk::LumaDc => 0,
            Blk::LumaAc(_) => 1,
            Blk::Luma4x4(_) => 2,
            Blk::ChromaDc(_) => 3,
            Blk::ChromaAc(..) => 4,
            Blk::Luma8x8(_) => 5,
        }
    }
}

/// Coefficients in scanning order, at most 64 of them.
pub(crate) const MAX_COEFFS: usize = 64;
