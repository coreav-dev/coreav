//! Validated views of the samples a kernel may read and write.
//!
//! A view is built once, from a plane, an offset and a geometry; building it
//! checks every length, stride and range with checked arithmetic. A kernel
//! therefore never validates the decoder's control code, and an
//! implementation that reads or writes within the view's own bounds is
//! correct by construction. This is what later lets the SIMD kernels
//! (`docs/design/h264.md`, "Kernels") carry their `unsafe` inside a
//! justified range.

use core::fmt;

/// A geometry no kernel can be called with.
///
/// The decoder turns this into its own error; it is a bug in the caller, not
/// a property of the bitstream, and the decoder's limits keep it from
/// happening on hostile input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    what: &'static str,
}

impl Geometry {
    const fn new(what: &'static str) -> Geometry {
        Geometry { what }
    }

    /// What was wrong with the geometry.
    pub const fn what(&self) -> &'static str {
        self.what
    }
}

impl fmt::Display for Geometry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "kernel geometry: {}", self.what)
    }
}

impl std::error::Error for Geometry {}

/// The last index a `w` by `h` block at `offset` with `stride` touches, plus
/// one, or `None` if the arithmetic overflows.
fn block_end(offset: usize, stride: usize, w: usize, h: usize) -> Option<usize> {
    h.checked_sub(1)?
        .checked_mul(stride)?
        .checked_add(w)?
        .checked_add(offset)
}

/// A read-only `w` by `h` block of samples inside a plane.
#[derive(Clone, Copy)]
pub struct Block<'a> {
    data: &'a [u8],
    stride: usize,
    w: usize,
    h: usize,
}

impl<'a> Block<'a> {
    /// The `w` by `h` block whose first sample is `plane[offset]`, with
    /// `stride` samples from one row to the next.
    pub fn new(
        plane: &'a [u8],
        offset: usize,
        stride: usize,
        w: usize,
        h: usize,
    ) -> Result<Block<'a>, Geometry> {
        if w == 0 || h == 0 || w > stride {
            return Err(Geometry::new("empty block, or a row wider than the stride"));
        }
        let end = block_end(offset, stride, w, h).ok_or(Geometry::new("block range overflows"))?;
        let data = plane
            .get(offset..end)
            .ok_or(Geometry::new("block reaches past the plane"))?;
        Ok(Block { data, stride, w, h })
    }

    /// Samples per row.
    pub const fn w(&self) -> usize {
        self.w
    }

    /// Rows.
    pub const fn h(&self) -> usize {
        self.h
    }

    /// Samples from one row to the next.
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// The whole window: `(h - 1) * stride + w` samples.
    pub const fn samples(&self) -> &'a [u8] {
        self.data
    }

    /// The `h` rows, each exactly `w` samples.
    pub fn rows(&self) -> impl Iterator<Item = &'a [u8]> + Clone {
        let w = self.w;
        self.data
            .chunks(self.stride)
            .take(self.h)
            .map(move |row| row.get(..w).unwrap_or(row))
    }

    /// Row `y`, exactly `w` samples, or `None` past the last row.
    pub fn row(&self, y: usize) -> Option<&'a [u8]> {
        let start = y.checked_mul(self.stride)?;
        if y >= self.h {
            return None;
        }
        let row = self.data.get(start..)?;
        row.get(..self.w)
    }

    /// The `h - skip` rows from row `skip` on, each exactly `w` samples.
    pub fn rows_from(&self, skip: usize) -> impl Iterator<Item = &'a [u8]> + Clone {
        let w = self.w;
        let start = skip.saturating_mul(self.stride).min(self.data.len());
        let h = self.h.saturating_sub(skip);
        self.data
            .get(start..)
            .unwrap_or_default()
            .chunks(self.stride)
            .take(h)
            .map(move |row| row.get(..w).unwrap_or(row))
    }
}

/// A writable `w` by `h` block of samples inside a plane.
pub struct BlockMut<'a> {
    data: &'a mut [u8],
    stride: usize,
    w: usize,
    h: usize,
}

impl<'a> BlockMut<'a> {
    /// The `w` by `h` block whose first sample is `plane[offset]`, with
    /// `stride` samples from one row to the next.
    pub fn new(
        plane: &'a mut [u8],
        offset: usize,
        stride: usize,
        w: usize,
        h: usize,
    ) -> Result<BlockMut<'a>, Geometry> {
        if w == 0 || h == 0 || w > stride {
            return Err(Geometry::new("empty block, or a row wider than the stride"));
        }
        let end = block_end(offset, stride, w, h).ok_or(Geometry::new("block range overflows"))?;
        let data = plane
            .get_mut(offset..end)
            .ok_or(Geometry::new("block reaches past the plane"))?;
        Ok(BlockMut { data, stride, w, h })
    }

    /// Samples per row.
    pub const fn w(&self) -> usize {
        self.w
    }

    /// Rows.
    pub const fn h(&self) -> usize {
        self.h
    }

    /// Samples from one row to the next.
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// The whole window: `(h - 1) * stride + w` samples.
    pub fn samples_mut(&mut self) -> &mut [u8] {
        self.data
    }

    /// The `h` rows, each exactly `w` samples.
    pub fn rows_mut(&mut self) -> impl Iterator<Item = &mut [u8]> {
        let (w, h, stride) = (self.w, self.h, self.stride);
        self.data.chunks_mut(stride).take(h).map(move |row| {
            let n = row.len().min(w);
            match row.get_mut(..n) {
                Some(row) => row,
                None => &mut [],
            }
        })
    }

    /// The same block, borrowed for a shorter life.
    pub fn reborrow(&mut self) -> BlockMut<'_> {
        BlockMut {
            data: self.data,
            stride: self.stride,
            w: self.w,
            h: self.h,
        }
    }
}

/// Which samples an edge filter reaches on each side of the edge: 4 for luma
/// (p3..q3, clause 8.7.2.3), 2 for chroma (p1..q1, clause 8.7.2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Luma: p3..q3.
    Luma,
    /// Chroma: p1..q1.
    Chroma,
}

impl Reach {
    /// How many samples the filter reaches on each side of the edge.
    pub(crate) const fn samples(self) -> usize {
        match self {
            Reach::Luma => 4,
            Reach::Chroma => 2,
        }
    }
}

/// The samples one edge of one block boundary covers (clause 8.7.2).
///
/// `across` is the step from one sample to the next across the edge and
/// `along` the step from one filtered line to the next. A vertical edge has
/// `across == 1`; a horizontal edge has `along == 1`. The two directions may
/// not overlap, so every line is a distinct set of samples.
pub struct EdgeMut<'a> {
    /// The samples the filter may touch: exactly those for an edge made by
    /// [`EdgeMut::new`], the whole window for an edge of a [`LumaMbMut`],
    /// whose geometry puts every line of its edges inside it. The kernels
    /// check every access against this slice either way.
    data: &'a mut [u8],
    /// Index of the q0 sample of line 0 inside `data`.
    q0: usize,
    across: usize,
    along: usize,
    lines: usize,
    reach: Reach,
}

impl<'a> EdgeMut<'a> {
    /// The edge whose first q0 sample is `plane[q0]`.
    ///
    /// `lines` is a multiple of 4: the boundary strength and the threshold
    /// `tC0` are given per segment of `lines / 4` lines (Table 8-16).
    pub fn new(
        plane: &'a mut [u8],
        q0: usize,
        across: usize,
        along: usize,
        lines: usize,
        reach: Reach,
    ) -> Result<EdgeMut<'a>, Geometry> {
        let r = reach.samples();
        if lines == 0 || lines % 4 != 0 {
            return Err(Geometry::new("lines is not a positive multiple of 4"));
        }
        let orthogonal = (across == 1 && along >= 2 * r) || (along == 1 && across >= lines);
        if !orthogonal {
            return Err(Geometry::new("edge lines overlap"));
        }
        let before = r
            .checked_mul(across)
            .ok_or(Geometry::new("edge range overflows"))?;
        let start = q0
            .checked_sub(before)
            .ok_or(Geometry::new("edge reaches before the plane"))?;
        let end = lines
            .checked_sub(1)
            .and_then(|l| l.checked_mul(along))
            .and_then(|l| l.checked_add(q0))
            .and_then(|e| e.checked_add(r.checked_sub(1)?.checked_mul(across)?))
            .and_then(|e| e.checked_add(1))
            .ok_or(Geometry::new("edge range overflows"))?;
        let data = plane
            .get_mut(start..end)
            .ok_or(Geometry::new("edge reaches past the plane"))?;
        Ok(EdgeMut {
            data,
            q0: before,
            across,
            along,
            lines,
            reach,
        })
    }

    /// Filtered lines.
    pub const fn lines(&self) -> usize {
        self.lines
    }

    /// Lines per segment: each segment has its own boundary strength.
    pub const fn segment(&self) -> usize {
        self.lines / 4
    }

    /// How far the filter reaches on each side.
    pub const fn reach(&self) -> Reach {
        self.reach
    }

    /// Index of `q[j]` (`j >= 0`) or `p[-1 - j]` (`j < 0`) on line `k`, or
    /// `None` outside the view.
    fn at(&self, k: usize, j: i32) -> Option<usize> {
        let base = k.checked_mul(self.along)?.checked_add(self.q0)?;
        let step = self.across.checked_mul(j.unsigned_abs() as usize)?;
        if j < 0 {
            base.checked_sub(step)
        } else {
            base.checked_add(step)
        }
    }

    /// `q[j]` (`j >= 0`) or `p[-1 - j]` (`j < 0`) on line `k`.
    ///
    /// Returns 0 outside the view's slice, which the filter never asks for:
    /// `j` runs over `-reach..reach` and `k` over `0..lines`. For an edge of
    /// a [`LumaMbMut`] the slice is the whole window, so a `j` or `k` past
    /// those ranges may reach other samples of the window.
    pub fn get(&self, k: usize, j: i32) -> u8 {
        self.at(k, j)
            .and_then(|i| self.data.get(i))
            .copied()
            .unwrap_or(0)
    }

    /// Writes `q[j]` or `p[-1 - j]` on line `k`; does nothing outside the
    /// view's slice (the whole window for an edge of a [`LumaMbMut`], as for
    /// [`EdgeMut::get`]).
    pub fn set(&mut self, k: usize, j: i32, v: u8) {
        if let Some(p) = self.at(k, j).and_then(|i| self.data.get_mut(i)) {
            *p = v;
        }
    }

    /// The samples the filter may touch, and where the first q0 sample
    /// sits inside them. Used by the SIMD kernels, which work on whole
    /// lines rather than one sample at a time.
    #[cfg_attr(
        not(any(target_arch = "x86_64", target_arch = "aarch64")),
        expect(dead_code, reason = "only the vector kernels read whole lines")
    )]
    pub(crate) fn window(&mut self) -> (&mut [u8], usize) {
        (self.data, self.q0)
    }

    /// The step from one sample to the next across the edge.
    pub(crate) const fn across(&self) -> usize {
        self.across
    }

    /// The step from one filtered line to the next.
    #[cfg_attr(
        not(any(target_arch = "x86_64", target_arch = "aarch64")),
        expect(dead_code, reason = "only the vector kernels read whole lines")
    )]
    pub(crate) const fn along(&self) -> usize {
        self.along
    }

    /// The same edge, borrowed for a shorter life.
    pub fn reborrow(&mut self) -> EdgeMut<'_> {
        EdgeMut {
            data: self.data,
            q0: self.q0,
            across: self.across,
            along: self.along,
            lines: self.lines,
            reach: self.reach,
        }
    }
}

/// The luma samples the deblocking filter reads and writes for one
/// macroblock: its block of 16x16 samples with the four columns to its
/// left and the four rows above it, a window of twenty rows of twenty
/// samples whose rows do not overlap.
///
/// The window holds every sample of the macroblock's eight luma edges, so
/// it is checked once a macroblock and each edge is a view inside it
/// ([`LumaMbMut::vertical`], [`LumaMbMut::horizontal`]) made without a
/// check of its own, where [`EdgeMut::new`] checks each edge's range
/// (`docs/design/h264.md`, "Deblocking round: the kernels' setup in the
/// form the tables give").
pub struct LumaMbMut<'a> {
    /// From the sample four rows above and four columns left of the
    /// block's first sample, to the block's last sample.
    data: &'a mut [u8],
    stride: usize,
}

impl<'a> LumaMbMut<'a> {
    /// Rows of the window above the block, and columns left of it.
    const MARGIN: usize = 4;

    /// The window of the block whose first sample is `plane[at]`, with
    /// rows `stride` apart. The stride is at least twenty, so that the
    /// rows of the window do not overlap.
    pub fn new(plane: &'a mut [u8], at: usize, stride: usize) -> Result<LumaMbMut<'a>, Geometry> {
        if stride < 20 {
            return Err(Geometry::new("luma window rows overlap"));
        }
        let start = stride
            .checked_mul(Self::MARGIN)
            .and_then(|v| v.checked_add(Self::MARGIN))
            .and_then(|v| at.checked_sub(v))
            .ok_or(Geometry::new("luma window reaches before the plane"))?;
        let end = stride
            .checked_mul(15)
            .and_then(|v| v.checked_add(at))
            .and_then(|v| v.checked_add(16))
            .ok_or(Geometry::new("luma window overflows"))?;
        let data = plane
            .get_mut(start..end)
            .ok_or(Geometry::new("luma window reaches past the plane"))?;
        Ok(LumaMbMut { data, stride })
    }

    /// Vertical edge `e` of the macroblock (0 to 3, at column 4e); `None`
    /// for any other.
    ///
    /// Its sixteen lines are the window's rows 4 to 19, each reaching four
    /// samples either side of column 4 + 4e: columns 4e to 4e + 7 of
    /// twenty. A line is one sample across, `stride` (at least twenty)
    /// along, so the lines do not overlap.
    #[inline(always)]
    pub fn vertical(&mut self, e: usize) -> Option<EdgeMut<'_>> {
        let first = Self::MARGIN * self.stride + Self::MARGIN;
        (e < 4).then(|| EdgeMut {
            data: self.data,
            q0: first + 4 * e,
            across: 1,
            along: self.stride,
            lines: 16,
            reach: Reach::Luma,
        })
    }

    /// Horizontal edge `e` of the macroblock (0 to 3, at row 4e); `None`
    /// for any other.
    ///
    /// Its sixteen lines are the window's columns 4 to 19, each reaching
    /// four rows either side of row 4 + 4e: rows 4e to 4e + 7 of twenty. A
    /// line is `stride` (at least twenty) across and one sample along, so
    /// the lines do not overlap.
    #[inline(always)]
    pub fn horizontal(&mut self, e: usize) -> Option<EdgeMut<'_>> {
        let s = self.stride;
        let first = Self::MARGIN * s + Self::MARGIN;
        (e < 4).then(|| EdgeMut {
            data: self.data,
            q0: first + 4 * e * s,
            across: s,
            along: 1,
            lines: 16,
            reach: Reach::Luma,
        })
    }
}

/// The chroma samples the deblocking filter reads and writes for one
/// macroblock of a 4:2:0 picture (`Kernels::filter_chroma_mb`): its block
/// of 8x8 samples with the two columns to its left and the two rows above
/// it, a window of ten rows of ten samples whose rows do not overlap.
///
/// The window holds every sample of the macroblock's four chroma edges
/// (vertical edges 0 and 4, horizontal edges 0 and 4), so each edge is a
/// view inside it ([`ChromaMbMut::edge`]), and a kernel that checks the
/// window once may reach any of them.
pub struct ChromaMbMut<'a> {
    /// From the sample two rows above and two columns left of the block's
    /// first sample, to the block's last sample.
    data: &'a mut [u8],
    stride: usize,
}

impl<'a> ChromaMbMut<'a> {
    /// Rows of the window above the block, and columns left of it.
    const MARGIN: usize = 2;

    /// The window of the block whose first sample is `plane[at]`, with
    /// rows `stride` apart. The stride is at least ten, so that the rows of
    /// the window do not overlap.
    pub fn new(plane: &'a mut [u8], at: usize, stride: usize) -> Result<ChromaMbMut<'a>, Geometry> {
        if stride < 10 {
            return Err(Geometry::new("chroma window rows overlap"));
        }
        let start = stride
            .checked_mul(Self::MARGIN)
            .and_then(|v| v.checked_add(Self::MARGIN))
            .and_then(|v| at.checked_sub(v))
            .ok_or(Geometry::new("chroma window reaches before the plane"))?;
        let end = stride
            .checked_mul(7)
            .and_then(|v| v.checked_add(at))
            .and_then(|v| v.checked_add(8))
            .ok_or(Geometry::new("chroma window overflows"))?;
        let data = plane
            .get_mut(start..end)
            .ok_or(Geometry::new("chroma window reaches past the plane"))?;
        Ok(ChromaMbMut { data, stride })
    }

    /// The windows of the same block of both chroma planes, whose first
    /// sample is `cb[at]` and `cr[at]`: [`ChromaMbMut::new`] on each, with
    /// the range worked out once for both.
    pub fn pair(
        cb: &'a mut [u8],
        cr: &'a mut [u8],
        at: usize,
        stride: usize,
    ) -> Option<(ChromaMbMut<'a>, ChromaMbMut<'a>)> {
        if stride < 10 {
            return None;
        }
        let start = at.checked_sub(
            stride
                .checked_mul(Self::MARGIN)?
                .checked_add(Self::MARGIN)?,
        )?;
        let end = stride.checked_mul(7)?.checked_add(at)?.checked_add(8)?;
        let cb = cb.get_mut(start..end)?;
        let cr = cr.get_mut(start..end)?;
        Some((
            ChromaMbMut { data: cb, stride },
            ChromaMbMut { data: cr, stride },
        ))
    }

    /// The distance between rows.
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// Edge `slot` of the macroblock: vertical edges 0 and 4 for slots 0
    /// and 1, horizontal edges 0 and 4 for slots 2 and 3; `None` for any
    /// other slot.
    pub fn edge(&mut self, slot: usize) -> Option<EdgeMut<'_>> {
        let s = self.stride;
        // The block's first sample inside the window.
        let first = Self::MARGIN * s + Self::MARGIN;
        let (q0, across, along) = match slot {
            0 => (first, 1, s),
            1 => (first + 4, 1, s),
            2 => (first, s, 1),
            3 => (first + 4 * s, s, 1),
            _ => return None,
        };
        EdgeMut::new(self.data, q0, across, along, 8, Reach::Chroma).ok()
    }

    /// The window, its first row the one two above the block, starting two
    /// columns left of it. Used by the SIMD kernel, which reads whole rows.
    #[cfg_attr(
        not(target_arch = "x86_64"),
        expect(dead_code, reason = "only the x86 kernel reads the window")
    )]
    pub(crate) fn window(&mut self) -> &mut [u8] {
        self.data
    }
}

/// Which neighbouring samples of a block an intra prediction may read
/// (clause 6.4.11.4, with constrained_intra_pred_flag applied by the
/// caller).
///
/// For a macroblock-level view these are the samples of macroblocks A
/// (`left`), B (`top`), C (`top_right`) and D (`top_left`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IntraAvail {
    /// p[-1, y]: the column left of the block.
    pub left: bool,
    /// p[x, -1] for x below n: the row above the block.
    pub top: bool,
    /// p[x, -1] for x from n to 2n - 1: the row above and right.
    pub top_right: bool,
    /// p[-1, -1].
    pub top_left: bool,
}

impl IntraAvail {
    const LEFT: u8 = 1;
    const TOP: u8 = 2;
    const TOP_RIGHT: u8 = 4;
    const TOP_LEFT: u8 = 8;

    /// The flags as one byte, a bit each.
    const fn bits(self) -> u8 {
        (self.left as u8 * Self::LEFT)
            | (self.top as u8 * Self::TOP)
            | (self.top_right as u8 * Self::TOP_RIGHT)
            | (self.top_left as u8 * Self::TOP_LEFT)
    }

    /// The flags of [`IntraAvail::bits`].
    pub(crate) const fn from_bits(bits: u8) -> IntraAvail {
        IntraAvail {
            left: bits & Self::LEFT != 0,
            top: bits & Self::TOP != 0,
            top_right: bits & Self::TOP_RIGHT != 0,
            top_left: bits & Self::TOP_LEFT != 0,
        }
    }
}

/// For an `n` by `n` intra block whose first sample is `plane[offset]`,
/// with the neighbours `avail` names: how far before the block's first
/// sample its view starts, and the range of the plane the view covers,
/// from the first neighbour to the block's last sample. The block must be
/// at least one sample and at most `stride` wide.
fn intra_range(
    offset: usize,
    stride: usize,
    n: usize,
    avail: IntraAvail,
) -> Result<(usize, core::ops::Range<usize>), Geometry> {
    if n == 0 || n > stride {
        return Err(Geometry::new(
            "empty intra block, or one wider than the stride",
        ));
    }
    let end = block_end(offset, stride, n, n).ok_or(Geometry::new("block range overflows"))?;
    // The row above ends inside the block's own range: with a stride of at
    // least n, `offset - stride + 2n` is at most the end of the block's last
    // row. So only where the view starts needs a check.
    let before = if avail.top_left {
        stride.checked_add(1)
    } else if avail.top || avail.top_right {
        Some(stride)
    } else if avail.left {
        Some(1)
    } else {
        Some(0)
    }
    .ok_or(Geometry::new("block range overflows"))?;
    let start = offset
        .checked_sub(before)
        .ok_or(Geometry::new("intra neighbours reach before the plane"))?;
    Ok((before, start..end))
}

/// A writable `n` by `n` block and the neighbouring samples an intra
/// prediction of it reads (clauses 8.3.1.2, 8.3.2.2, 8.3.3 and 8.3.4).
///
/// Building it checks, besides the block itself, each neighbour whose flag
/// in [`IntraAvail`] is set: `n` samples of the row above (`2n` with
/// `top_right`), the column left and the sample above and left. The view
/// starts at the first neighbour its flags name and ends at the block's
/// last sample, so it never reaches a sample before a neighbour the caller
/// did not say exists. A kernel uses no neighbour whose flag is clear; a
/// vector kernel may read one that lies inside the view (the row above
/// right, inside the view whenever the row above is) and discard it.
pub struct IntraMut<'a> {
    /// From the first neighbour the view may read to the block's last
    /// sample.
    data: &'a mut [u8],
    /// Index of the block's first sample inside `data`.
    origin: usize,
    stride: usize,
    n: usize,
    avail: IntraAvail,
}

impl<'a> IntraMut<'a> {
    /// The `n` by `n` block whose first sample is `plane[offset]`, with
    /// `stride` samples from one row to the next, and the neighbours
    /// `avail` names.
    pub fn new(
        plane: &'a mut [u8],
        offset: usize,
        stride: usize,
        n: usize,
        avail: IntraAvail,
    ) -> Result<IntraMut<'a>, Geometry> {
        let (before, range) = intra_range(offset, stride, n, avail)?;
        let data = plane
            .get_mut(range)
            .ok_or(Geometry::new("block reaches past the plane"))?;
        Ok(IntraMut {
            data,
            origin: before,
            stride,
            n,
            avail,
        })
    }

    /// Samples per row and rows.
    pub const fn n(&self) -> usize {
        self.n
    }

    /// Samples from one row to the next.
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// Which neighbours the view covers.
    pub const fn avail(&self) -> IntraAvail {
        self.avail
    }

    /// The row above: `2n` samples with `top_right` and `top`, `n` with
    /// `top` alone, none without `top`. With a stride below `2n` the
    /// samples above and right are the start of the block's own first row;
    /// a prediction reads its neighbours before it writes the block.
    pub fn top(&self) -> &[u8] {
        if !self.avail.top {
            return &[];
        }
        let len = if self.avail.top_right {
            2 * self.n
        } else {
            self.n
        };
        let start = self.origin.wrapping_sub(self.stride);
        self.data.get(start..start + len).unwrap_or_default()
    }

    /// p[-1, y], or 0 without `left` or past the last row.
    pub fn left(&self, y: usize) -> u8 {
        if !self.avail.left || y >= self.n {
            return 0;
        }
        (y * self.stride + self.origin)
            .checked_sub(1)
            .and_then(|i| self.data.get(i))
            .copied()
            .unwrap_or(0)
    }

    /// p[-1, -1], or 0 without `top_left`.
    pub fn top_left(&self) -> u8 {
        if !self.avail.top_left {
            return 0;
        }
        self.data.first().copied().unwrap_or(0)
    }

    /// The block itself.
    pub fn block_mut(&mut self) -> Option<BlockMut<'_>> {
        BlockMut::new(self.data, self.origin, self.stride, self.n, self.n).ok()
    }

    /// The `m` by `m` block at column `x` and row `y` of this one, with the
    /// neighbours `avail` names, or `None` where they are not inside this
    /// view.
    pub fn sub(&mut self, x: usize, y: usize, m: usize, avail: IntraAvail) -> Option<IntraMut<'_>> {
        if x.checked_add(m)? > self.n || y.checked_add(m)? > self.n {
            return None;
        }
        let offset = y
            .checked_mul(self.stride)?
            .checked_add(x)?
            .checked_add(self.origin)?;
        IntraMut::new(self.data, offset, self.stride, m, avail).ok()
    }

    /// The samples of the view, and where the block's first sample sits
    /// inside them. Used by the SIMD kernels, which read and write whole
    /// rows: `data[origin - stride - 1]` is p[-1, -1] where `top_left` is
    /// set, and the view starts at the first neighbour it covers.
    #[cfg_attr(
        not(target_arch = "x86_64"),
        expect(dead_code, reason = "only the x86 kernels read whole rows")
    )]
    pub(crate) fn window(&mut self) -> (&mut [u8], usize) {
        (self.data, self.origin)
    }
}

/// Both chroma blocks of a macroblock of a 4:2:0 picture, 8 by 8 at the
/// same offset of each plane, and the neighbours their intra prediction
/// reads (clause 8.3.4): an [`IntraMut`] of each plane, built with one
/// check of the geometry the two share.
///
/// The flags are kept as one byte: a kernel that reads them reads what
/// the constructor stored, in one load the size of that store.
pub struct ChromaIntraMut<'a> {
    /// Each plane's samples, from the first neighbour the view may read to
    /// the block's last sample: the same range of both planes.
    planes: [&'a mut [u8]; 2],
    /// Index of the blocks' first sample inside each plane's samples.
    origin: usize,
    stride: usize,
    /// [`IntraAvail::bits`].
    avail: u8,
}

impl<'a> ChromaIntraMut<'a> {
    /// The 8x8 blocks whose first sample is `cb[offset]` and `cr[offset]`,
    /// with `stride` samples from one row to the next, and the neighbours
    /// `avail` names, as [`IntraMut::new`] builds each.
    pub fn new(
        cb: &'a mut [u8],
        cr: &'a mut [u8],
        offset: usize,
        stride: usize,
        avail: IntraAvail,
    ) -> Result<ChromaIntraMut<'a>, Geometry> {
        let (origin, range) = intra_range(offset, stride, 8, avail)?;
        let past = Geometry::new("block reaches past the plane");
        let cb = cb.get_mut(range.clone()).ok_or(past)?;
        let cr = cr.get_mut(range).ok_or(past)?;
        Ok(ChromaIntraMut {
            planes: [cb, cr],
            origin,
            stride,
            avail: avail.bits(),
        })
    }

    /// Samples from one row to the next.
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// Which neighbours the view covers.
    pub const fn avail(&self) -> IntraAvail {
        IntraAvail::from_bits(self.avail)
    }

    /// The view of each plane, Cb then Cr.
    pub fn into_parts(self) -> [IntraMut<'a>; 2] {
        let (origin, stride, avail) = (self.origin, self.stride, self.avail());
        self.planes.map(|data| IntraMut {
            data,
            origin,
            stride,
            n: 8,
            avail,
        })
    }

    /// Each plane's samples, where the blocks' first sample sits inside
    /// them, and the flags as [`IntraAvail::bits`]. Used by the SIMD
    /// kernel, as [`IntraMut::window`] is.
    #[cfg_attr(
        not(target_arch = "x86_64"),
        expect(dead_code, reason = "only the x86 kernels read whole rows")
    )]
    pub(crate) fn windows(&mut self) -> ([&mut [u8]; 2], usize, u8) {
        let [cb, cr] = &mut self.planes;
        ([&mut **cb, &mut **cr], self.origin, self.avail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_checks_its_range() {
        let plane = [0u8; 64];
        assert!(Block::new(&plane, 0, 8, 8, 8).is_ok());
        // One row too many.
        assert!(Block::new(&plane, 0, 8, 8, 9).is_err());
        // One sample too far right.
        assert!(Block::new(&plane, 1, 8, 8, 8).is_err());
        // A row wider than the stride would alias the next row.
        assert!(Block::new(&plane, 0, 4, 8, 2).is_err());
        assert!(Block::new(&plane, 0, usize::MAX, 4, 4).is_err());
    }

    #[test]
    fn block_rows_are_exactly_w_samples() {
        let plane: Vec<u8> = (0..64).collect();
        let b = Block::new(&plane, 9, 8, 4, 3).expect("block");
        let rows: Vec<&[u8]> = b.rows().collect();
        assert_eq!(
            rows,
            [&[9, 10, 11, 12], &[17, 18, 19, 20], &[25, 26, 27, 28]]
        );
        assert_eq!(b.row(2), Some(&[25u8, 26, 27, 28][..]));
        assert_eq!(b.row(3), None);
        assert_eq!(b.rows_from(2).count(), 1);
    }

    #[test]
    fn edge_rejects_overlapping_and_out_of_range_geometry() {
        let mut plane = [0u8; 256];
        // A vertical luma edge in a 16-sample-wide plane.
        assert!(EdgeMut::new(&mut plane, 8, 1, 16, 16, Reach::Luma).is_ok());
        // A horizontal luma edge: lines along the row.
        assert!(EdgeMut::new(&mut plane, 4 * 16, 16, 1, 16, Reach::Luma).is_ok());
        // Lines that step by 4 across an edge that reaches 4 samples overlap.
        assert!(EdgeMut::new(&mut plane, 8, 1, 4, 16, Reach::Luma).is_err());
        // p3 would be before the plane.
        assert!(EdgeMut::new(&mut plane, 2, 1, 16, 16, Reach::Luma).is_err());
        // q3 of the last line would be past the plane.
        assert!(EdgeMut::new(&mut plane, 15 * 16 + 8, 1, 16, 16, Reach::Luma).is_err());
        // Not a multiple of 4 lines.
        assert!(EdgeMut::new(&mut plane, 8, 1, 16, 6, Reach::Luma).is_err());
    }

    #[test]
    fn edge_addresses_p_and_q() {
        let mut plane: Vec<u8> = (0..=255).collect();
        let mut e = EdgeMut::new(&mut plane, 4 * 16 + 8, 1, 16, 4, Reach::Chroma).expect("edge");
        assert_eq!(e.get(0, 0), 4 * 16 + 8);
        assert_eq!(e.get(0, -1), 4 * 16 + 7);
        assert_eq!(e.get(1, 1), 5 * 16 + 9);
        assert_eq!(e.get(3, -2), 7 * 16 + 6);
        e.set(3, -2, 42);
        assert_eq!(e.get(3, -2), 42);
        assert_eq!(plane.get(7 * 16 + 6), Some(&42));
    }

    const ALL: IntraAvail = IntraAvail {
        left: true,
        top: true,
        top_right: true,
        top_left: true,
    };

    #[test]
    fn intra_view_reads_its_neighbours() {
        let mut plane: Vec<u8> = (0..=255).collect();
        // A 4x4 block at (4, 1) of a 16-wide plane.
        let v = IntraMut::new(&mut plane, 16 + 4, 16, 4, ALL).expect("view");
        assert_eq!(v.top(), &[4, 5, 6, 7, 8, 9, 10, 11]);
        assert_eq!(v.top_left(), 3);
        assert_eq!(v.left(0), 19);
        assert_eq!(v.left(3), 3 * 16 + 19);
        assert_eq!(v.left(4), 0);
        let only_top = IntraAvail {
            top: true,
            ..IntraAvail::default()
        };
        let v = IntraMut::new(&mut plane, 16 + 4, 16, 4, only_top).expect("view");
        assert_eq!(v.top(), &[4, 5, 6, 7]);
        assert_eq!((v.top_left(), v.left(0)), (0, 0));
    }

    #[test]
    fn intra_view_checks_the_neighbours_it_names() {
        let mut plane = [0u8; 256];
        // The first row has nothing above it, the first column nothing left.
        assert!(IntraMut::new(&mut plane, 4, 16, 4, ALL).is_err());
        let left = IntraAvail {
            left: true,
            ..IntraAvail::default()
        };
        assert!(IntraMut::new(&mut plane, 0, 16, 4, left).is_err());
        assert!(IntraMut::new(&mut plane, 0, 16, 4, IntraAvail::default()).is_ok());
        assert!(IntraMut::new(&mut plane, 17, 16, 4, ALL).is_ok());
        // Past the plane's end, and wider than the stride.
        assert!(IntraMut::new(&mut plane, 13 * 16, 16, 4, ALL).is_err());
        assert!(IntraMut::new(&mut plane, 17, 4, 8, ALL).is_err());
        // A sub-block keeps to the view it came from.
        let mut v = IntraMut::new(&mut plane, 17, 16, 8, ALL).expect("view");
        assert!(v.sub(4, 4, 4, ALL).is_some());
        assert!(v.sub(0, 0, 4, ALL).is_some());
        assert!(v.sub(8, 0, 4, ALL).is_none());
    }

    /// Every sample `get` reads on every line of `edge`, with its index.
    fn samples(edge: &EdgeMut<'_>) -> Vec<u8> {
        let r = edge.reach().samples() as i32;
        (0..edge.lines())
            .flat_map(|k| (-r..r).map(move |j| (k, j)))
            .map(|(k, j)| edge.get(k, j))
            .collect()
    }

    #[test]
    fn luma_window_edges_are_the_checked_edges() {
        for stride in [20, 21, 37, 64] {
            let plane: Vec<u8> = (0..stride * 40).map(|i| (i * 7 + i / 251) as u8).collect();
            let at = 9 * stride + 5;
            let mut window_plane = plane.clone();
            let mut w = LumaMbMut::new(&mut window_plane, at, stride).expect("window");
            let mut checked = plane.clone();
            for e in 0..4 {
                let v = w.vertical(e).expect("edge");
                let c = EdgeMut::new(&mut checked, at + 4 * e, 1, stride, 16, Reach::Luma)
                    .expect("edge");
                assert_eq!(samples(&v), samples(&c), "stride {stride} vertical {e}");
                let h = w.horizontal(e).expect("edge");
                let c = EdgeMut::new(
                    &mut checked,
                    at + 4 * e * stride,
                    stride,
                    1,
                    16,
                    Reach::Luma,
                )
                .expect("edge");
                assert_eq!(samples(&h), samples(&c), "stride {stride} horizontal {e}");
            }
            assert!(w.vertical(4).is_none() && w.horizontal(4).is_none());
        }
    }

    #[test]
    fn luma_window_checks_its_range() {
        let mut plane = vec![0u8; 20 * 20];
        // The window is the whole plane.
        assert!(LumaMbMut::new(&mut plane, 4 * 20 + 4, 20).is_ok());
        // One column or one row short before the block, or one after it.
        assert!(LumaMbMut::new(&mut plane, 4 * 20 + 3, 20).is_err());
        assert!(LumaMbMut::new(&mut plane, 3 * 20 + 4, 20).is_err());
        assert!(LumaMbMut::new(&mut plane[..399], 4 * 20 + 4, 20).is_err());
        // Rows narrower than the window would overlap.
        assert!(LumaMbMut::new(&mut plane, 4 * 19 + 4, 19).is_err());
        assert!(LumaMbMut::new(&mut plane, usize::MAX, 20).is_err());
        assert!(LumaMbMut::new(&mut plane, 100, usize::MAX / 8).is_err());
    }

    #[test]
    fn chroma_pair_is_both_windows() {
        for (len, at, stride) in [
            (100, 22, 10),
            (100, 21, 10),
            (100, 92, 10),
            (99, 92, 10),
            (100, 22, 9),
            (4000, 333, 37),
            (4000, usize::MAX, 37),
            (4000, 333, usize::MAX / 4),
        ] {
            let (mut cb, mut cr) = (vec![1u8; len], vec![2u8; len]);
            let (mut cb2, mut cr2) = (cb.clone(), cr.clone());
            let pair = ChromaMbMut::pair(&mut cb, &mut cr, at, stride);
            let each = ChromaMbMut::new(&mut cb2, at, stride)
                .ok()
                .zip(ChromaMbMut::new(&mut cr2, at, stride).ok());
            match (pair, each) {
                (Some((mut b, mut r)), Some((mut b2, mut r2))) => {
                    for slot in 0..4 {
                        let one = |w: &mut ChromaMbMut<'_>| samples(&w.edge(slot).expect("edge"));
                        assert_eq!(one(&mut b), one(&mut b2), "{len} {at} {stride} {slot}");
                        assert_eq!(one(&mut r), one(&mut r2), "{len} {at} {stride} {slot}");
                    }
                    assert_eq!((b.stride(), r.stride()), (stride, stride));
                }
                (None, None) => {}
                _ => panic!("{len} {at} {stride}"),
            }
        }
        // Planes of different lengths: each is checked.
        let (mut cb, mut cr) = (vec![0u8; 100], vec![0u8; 99]);
        assert!(ChromaMbMut::pair(&mut cb, &mut cr, 92, 10).is_none());
    }

    #[test]
    fn chroma_intra_view_checks_both_planes() {
        let (mut cb, mut cr) = ([0u8; 160], [0u8; 160]);
        // The 8x8 blocks at (1, 1) of 16-wide planes end at 8 * 16 + 9.
        let v = ChromaIntraMut::new(&mut cb, &mut cr, 17, 16, ALL).expect("view");
        assert_eq!((v.stride(), v.avail()), (16, ALL));
        let [b, r] = v.into_parts();
        assert_eq!((b.n(), r.n(), b.avail(), r.avail()), (8, 8, ALL, ALL));
        // Each flag survives the byte the view keeps.
        for bits in 0..16 {
            let a = IntraAvail::from_bits(bits);
            assert_eq!(a.bits(), bits);
            let v = ChromaIntraMut::new(&mut cb, &mut cr, 17, 16, a).expect("view");
            assert_eq!(v.avail(), a);
        }
        // Either plane too short, the first row with nothing above, a
        // stride narrower than the block.
        assert!(ChromaIntraMut::new(&mut cb, &mut cr[..136], 17, 16, ALL).is_err());
        assert!(ChromaIntraMut::new(&mut cb[..136], &mut cr, 17, 16, ALL).is_err());
        assert!(ChromaIntraMut::new(&mut cb, &mut cr[..137], 17, 16, ALL).is_ok());
        assert!(ChromaIntraMut::new(&mut cb, &mut cr, 1, 16, ALL).is_err());
        assert!(ChromaIntraMut::new(&mut cb, &mut cr, 9, 7, ALL).is_err());
    }
}
