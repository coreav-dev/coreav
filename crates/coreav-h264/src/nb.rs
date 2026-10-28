//! The neighbour cache of the macroblock being decoded.
//!
//! Entropy decoding and intra mode prediction read a value of the 4x4
//! block to the left of and above each block (clauses 6.4.11.4, 8.3.1.1,
//! 9.2.1 and 9.3.3.1.1.7). Those blocks lie in the current macroblock or in
//! macroblock A or B. The cache holds them in one array per quantity: the
//! current macroblock's sixteen blocks, macroblock A's right column to
//! their left and macroblock B's bottom row above them, so the left and
//! upper neighbours of block (x, y) sit one and five places before it.
//! A read is one load whichever macroblock the block belongs to, and a
//! missing neighbour is a value in the array rather than a branch. Each
//! macroblock keeps only its edges in [`crate::mb::MbInfo`]
//! (`docs/design/h264.md`, "Round 22").
#![forbid(unsafe_code)]

/// Width of a luma cache: column 0 is macroblock A's right column, row 0
/// macroblock B's bottom row, and the corner is unused.
pub(crate) const W: usize = 5;

/// Width of a chroma cache, for one component's two by two 4x4 blocks.
pub(crate) const WC: usize = 3;

/// A neighbour that is not available, in the TotalCoeff and intra caches.
pub(crate) const NA: u8 = u8::MAX;

/// Index of the current macroblock's raster 4x4 block (`bx`, `by`) in a
/// luma cache.
#[inline]
pub(crate) const fn at(bx: usize, by: usize) -> usize {
    (by + 1) * W + bx + 1
}

/// Index of chroma 4x4 block `blk` (two by two, raster) in a chroma cache.
#[inline]
pub(crate) const fn at_c(blk: usize) -> usize {
    (blk / 2 + 1) * WC + blk % 2 + 1
}

/// What a macroblock leaves of one luma quantity for its neighbours: its
/// bottom row from left to right, then its right column from top to
/// bottom.
pub(crate) type Edge = [u8; 8];

/// Length of a motion vector difference cache: the [`W`] by [`W`] blocks
/// and room past them, so that a row of up to four blocks from any index
/// masked to 31 is inside the array and the compiler drops its bounds
/// check.
pub(crate) const MVD_LEN: usize = 40;

/// The mask of a block index into a motion vector difference cache.
const MVD_MASK: usize = 31;

/// The caches of the macroblock being decoded, each filled only when a
/// syntax element that reads it is about to be parsed.
#[derive(Clone, Copy)]
pub(crate) struct NbCache {
    /// TotalCoeff of each luma block for CAVLC, [`NA`] where the
    /// neighbouring macroblock is not available.
    pub(crate) tc: [u8; W * W],
    /// The same for the chroma AC blocks of each component.
    pub(crate) tc_c: [[u8; WC * WC]; 2],
    /// The absolute motion vector difference of each list, capped at 255,
    /// for CABAC: a block's horizontal component in the low byte and its
    /// vertical one in the high byte, so that one store writes both. 0
    /// where the neighbour is not available, skipped or intra, which
    /// contribute 0 to the context (clause 9.3.3.1.1.7).
    pub(crate) mvd: [[u16; MVD_LEN]; 2],
    /// Intra4x4PredMode or Intra8x8PredMode; 2 for a neighbour coded in
    /// another way, [`NA`] where the neighbour is not available for intra
    /// prediction (clause 8.3.1.1).
    pub(crate) intra: [u8; W * W],
}

impl NbCache {
    pub(crate) const NONE: NbCache = NbCache {
        tc: [0; W * W],
        tc_c: [[0; WC * WC]; 2],
        mvd: [[0; MVD_LEN]; 2],
        intra: [0; W * W],
    };
}

/// Starts a luma cache: `inner` in the current macroblock's blocks, the
/// right column of A in column 0 and the bottom row of B in row 0, each
/// `missing` where that macroblock is not there.
#[inline]
pub(crate) fn fill(
    cache: &mut [u8; W * W],
    a: Option<&Edge>,
    b: Option<&Edge>,
    missing: u8,
    inner: u8,
) {
    *cache = [inner; W * W];
    let above = b.map_or([missing; 4], |e| [e[0], e[1], e[2], e[3]]);
    let left = a.map_or([missing; 4], |e| [e[4], e[5], e[6], e[7]]);
    if let Some(row) = cache.get_mut(1..W) {
        row.copy_from_slice(&above);
    }
    for (y, v) in left.into_iter().enumerate() {
        if let Some(slot) = cache.get_mut((y + 1) * W) {
            *slot = v;
        }
    }
}

/// The edge of the current macroblock in a luma cache.
#[inline]
pub(crate) fn edge(cache: &[u8; W * W]) -> Edge {
    let v = |i: usize| cache.get(i).copied().unwrap_or(0);
    [
        v(at(0, 3)),
        v(at(1, 3)),
        v(at(2, 3)),
        v(at(3, 3)),
        v(at(3, 0)),
        v(at(3, 1)),
        v(at(3, 2)),
        v(at(3, 3)),
    ]
}

/// Starts a chroma cache from the edges of A and B, which hold, for each
/// component, the bottom row and then the right column (two blocks each).
#[inline]
pub(crate) fn fill_c(cache: &mut [u8; WC * WC], a: Option<[u8; 4]>, b: Option<[u8; 4]>) {
    *cache = [0; WC * WC];
    let above = b.map_or([NA; 2], |e| [e[0], e[1]]);
    let left = a.map_or([NA; 2], |e| [e[2], e[3]]);
    if let Some(row) = cache.get_mut(1..WC) {
        row.copy_from_slice(&above);
    }
    for (y, v) in left.into_iter().enumerate() {
        if let Some(slot) = cache.get_mut((y + 1) * WC) {
            *slot = v;
        }
    }
}

/// The edge of the current macroblock in a chroma cache: blocks 2 and 3,
/// then 1 and 3.
#[inline]
pub(crate) fn edge_c(cache: &[u8; WC * WC]) -> [u8; 4] {
    let v = |blk: usize| cache.get(at_c(blk)).copied().unwrap_or(0);
    [v(2), v(3), v(1), v(3)]
}

/// Four blocks of a motion vector difference cache, as a macroblock keeps
/// them in an [`Edge`]: each block's two bytes, horizontal first.
#[inline]
fn mvd_unpack(e: &Edge) -> [u16; 4] {
    let v = u64::from_le_bytes(*e);
    [
        v as u16,
        (v >> 16) as u16,
        (v >> 32) as u16,
        (v >> 48) as u16,
    ]
}

#[inline]
fn mvd_pack(b: [u16; 4]) -> Edge {
    (u64::from(b[0]) | u64::from(b[1]) << 16 | u64::from(b[2]) << 32 | u64::from(b[3]) << 48)
        .to_le_bytes()
}

/// Starts a motion vector difference cache: 0 in the current macroblock's
/// blocks, the right column of A (`a`, from [`mvd_col`]) in column 0 and
/// the bottom row of B (`b`, from [`mvd_row`]) in row 0, each 0 where that
/// macroblock is not there.
#[inline]
pub(crate) fn fill_mvd(cache: &mut [u16; MVD_LEN], a: Option<&Edge>, b: Option<&Edge>) {
    *cache = [0; MVD_LEN];
    if let Some(row) = cache.get_mut(1..W) {
        row.copy_from_slice(&b.map_or([0; 4], mvd_unpack));
    }
    for (y, v) in a.map_or([0; 4], mvd_unpack).into_iter().enumerate() {
        if let Some(slot) = cache.get_mut((y + 1) * W) {
            *slot = v;
        }
    }
}

/// The bottom row of the current macroblock in a motion vector difference
/// cache, for [`fill_mvd`]'s `b`.
#[inline]
pub(crate) fn mvd_row(cache: &[u16; MVD_LEN]) -> Edge {
    let v = |bx: usize| cache.get(at(bx, 3)).copied().unwrap_or(0);
    mvd_pack([v(0), v(1), v(2), v(3)])
}

/// The right column of the current macroblock in a motion vector
/// difference cache, for [`fill_mvd`]'s `a`.
#[inline]
pub(crate) fn mvd_col(cache: &[u16; MVD_LEN]) -> Edge {
    let v = |by: usize| cache.get(at(3, by)).copied().unwrap_or(0);
    mvd_pack([v(0), v(1), v(2), v(3)])
}

/// Writes `v` into `N` blocks of a row of a motion vector difference
/// cache, from the block at `i`.
#[inline]
pub(crate) fn put_mvd<const N: usize>(cache: &mut [u16; MVD_LEN], i: usize, v: u16) {
    let i = i & MVD_MASK;
    if let Some(row) = cache.get_mut(i..i + N) {
        row.copy_from_slice(&[v; N]);
    }
}

/// The left and upper neighbours of the block at `i` in a motion vector
/// difference cache.
#[inline]
pub(crate) fn mvd_pair(cache: &[u16; MVD_LEN], i: usize) -> (u16, u16) {
    let at = |j: usize| cache.get(j & MVD_MASK).copied().unwrap_or(0);
    (at(i.wrapping_sub(1)), at(i.wrapping_sub(W)))
}

/// The left (`a`) and upper (`b`) neighbours of the block at `i` in a
/// cache `width` wide.
#[inline]
pub(crate) fn pair(cache: &[u8], i: usize, width: usize) -> (u8, u8) {
    let a = cache.get(i.wrapping_sub(1)).copied().unwrap_or(NA);
    let b = cache.get(i.wrapping_sub(width)).copied().unwrap_or(NA);
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The left and upper neighbours of every block are the blocks of
    /// clause 6.4.11.4: inside the macroblock, or A's right column and B's
    /// bottom row.
    #[test]
    fn neighbours_are_the_adjacent_blocks() {
        let a: Edge = [0, 0, 0, 0, 40, 41, 42, 43];
        let b: Edge = [50, 51, 52, 53, 0, 0, 0, 0];
        let mut cache = [0u8; W * W];
        fill(&mut cache, Some(&a), Some(&b), NA, 0);
        for by in 0..4 {
            for bx in 0..4 {
                cache[at(bx, by)] = (by * 4 + bx) as u8;
            }
        }
        for by in 0..4 {
            for bx in 0..4 {
                let (l, u) = pair(&cache, at(bx, by), W);
                let want_l = if bx > 0 { by * 4 + bx - 1 } else { 40 + by };
                let want_u = if by > 0 { (by - 1) * 4 + bx } else { 50 + bx };
                assert_eq!((usize::from(l), usize::from(u)), (want_l, want_u));
            }
        }
        assert_eq!(edge(&cache), [12, 13, 14, 15, 3, 7, 11, 15]);
    }

    /// Missing neighbours read as the value given for them.
    #[test]
    fn missing_neighbours_read_as_missing() {
        let mut cache = [0u8; W * W];
        fill(&mut cache, None, None, NA, 7);
        assert_eq!(pair(&cache, at(0, 0), W), (NA, NA));
        assert_eq!(pair(&cache, at(1, 1), W), (7, 7));
        let mut c = [0u8; WC * WC];
        fill_c(&mut c, Some([1, 2, 3, 4]), None);
        assert_eq!(pair(&c, at_c(0), WC), (3, NA));
        assert_eq!(pair(&c, at_c(2), WC), (4, 0));
        assert_eq!(pair(&c, at_c(1), WC), (0, NA));
    }

    #[test]
    fn chroma_edge_is_bottom_then_right() {
        let mut c = [0u8; WC * WC];
        for blk in 0..4 {
            c[at_c(blk)] = 10 + blk as u8;
        }
        assert_eq!(edge_c(&c), [12, 13, 11, 13]);
    }

    /// A macroblock's motion vector difference edges, kept and read back
    /// by its right and lower neighbours, give the blocks of clause
    /// 6.4.11.7: A's right column to the left of column 0, B's bottom row
    /// above row 0, and the blocks a partition's row covers.
    #[test]
    fn mvd_edges_and_neighbours() {
        let both = |h: usize, v: usize| (h | v << 8) as u16;
        let mut prev = [0u16; MVD_LEN];
        for by in 0..4 {
            for bx in 0..4 {
                put_mvd::<1>(&mut prev, at(bx, by), both(by * 4 + bx, by * 4 + bx + 16));
            }
        }
        let (row, col) = (mvd_row(&prev), mvd_col(&prev));
        let mut cache = [7u16; MVD_LEN];
        fill_mvd(&mut cache, Some(&col), Some(&row));
        for bx in 0..4 {
            // The block above row 0 is B's block (bx, 3).
            assert_eq!(mvd_pair(&cache, at(bx, 0)).1, both(12 + bx, 28 + bx));
        }
        for by in 0..4 {
            // The block left of column 0 is A's block (3, by).
            assert_eq!(mvd_pair(&cache, at(0, by)).0, both(by * 4 + 3, by * 4 + 19));
        }
        put_mvd::<4>(&mut cache, at(0, 2), 0x0503);
        put_mvd::<2>(&mut cache, at(2, 3), 0x0204);
        assert_eq!(mvd_pair(&cache, at(1, 3)), (0, 0x0503));
        assert_eq!(mvd_pair(&cache, at(3, 3)), (0x0204, 0x0503));
        assert_eq!(mvd_row(&cache), mvd_pack([0, 0, 0x0204, 0x0204]));
        let mut none = [7u16; MVD_LEN];
        fill_mvd(&mut none, None, None);
        assert_eq!(none, [0; MVD_LEN]);
    }
}
