//! The inter prediction of all three planes of one block in one call
//! ([`crate::Kernels::predict`]): the reference picture and the picture
//! being decoded as whole planes, the block's position and motion vector,
//! and what each plane does with its prediction.
//!
//! One call checks every window and block once and then predicts the
//! three planes, where the decoder otherwise builds six views and makes
//! three calls a block (`docs/design/h264.md`, "Kernel round: the three
//! planes in one call").

use crate::views::{Block, BlockMut};
use crate::{Kernels, Weighting};

/// What [`crate::Kernels::predict`] does with one plane's prediction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaneOp {
    /// Writes it, as [`crate::Kernels::luma_mc`] and
    /// [`crate::Kernels::chroma_mc`] do.
    Put,
    /// Averages it into the block, as [`crate::Kernels::luma_mc_avg`] and
    /// [`crate::Kernels::chroma_mc_avg`] do.
    Avg,
    /// Weights it into the block, as [`crate::Kernels::luma_mc_weighted`]
    /// and [`crate::Kernels::chroma_mc_weighted`] do. A weighting outside
    /// [`Weighting::in_range`] is declined.
    Weigh(Weighting),
}

/// The largest width and height of a [`Planes`], and the largest block
/// position [`crate::Kernels::predict`] takes: four times the 8,192
/// samples of H.264's largest level (Table A-1), and small enough that
/// every offset the call computes fits 64 bits without a check.
pub const MAX_SIZE: usize = 1 << 15;

/// The largest origin of a plane of [`Planes`] and [`PlanesMut`]: with
/// it every offset [`crate::Kernels::predict`] computes still fits 64 bits
/// without a check.
pub const MAX_ORIGIN: usize = MAX_SIZE * MAX_SIZE;

/// A 4:2:0 reference picture: three planes without padding columns, the
/// chroma planes half as wide and half as tall as the luma plane. Each
/// plane's sample (0, 0) sits at an origin in its slice
/// ([`Planes::with_origin`]), with whatever the caller keeps before it
/// and after its last row; [`Planes::new`] puts it at 0. Or the rows of
/// the picture a band stores, each plane's from a first row on
/// ([`Planes::band`]).
#[derive(Clone, Copy, Debug)]
pub struct Planes<'a> {
    y: &'a [u8],
    cb: &'a [u8],
    cr: &'a [u8],
    stride: usize,
    rows: usize,
    /// Where the first row each slice holds sits in it: of the luma plane
    /// in `y`, and of each chroma plane in `cb` and `cr`.
    origin: [usize; 2],
    /// Where sample (0, 0) of the luma plane would sit in `y`, and of each
    /// chroma plane in `cb` and `cr`: the origin less the rows before the
    /// first held, which lies before the slice where the first row is not
    /// row 0.
    at: [i64; 2],
    /// The block positions whose windows need no test of their own.
    interior: Interior,
}

/// The integer luma positions of a block in a reference (xIntL and yIntL
/// of its first sample, Equations 8-228 and 8-229 before their clip) at
/// which every window [`place`] takes lies inside the picture and inside
/// its slices, whatever the block's size (4, 8 or 16 each way) and
/// fraction: x from 2 to `x_last` and y from `y_first` to `y_last`, an
/// empty range where the plane is too small for any. Computed once, when
/// the reference is built, so that a block inside it is placed with four
/// comparisons instead of a test of each window (`docs/design/h264.md`,
/// "Kernel round: a reference's interior, found once").
#[derive(Clone, Copy, Debug)]
struct Interior {
    x_last: i64,
    y_first: i64,
    y_last: i64,
}

impl Interior {
    /// No position: every block is tested window by window.
    #[cfg(test)]
    const NONE: Interior = Interior {
        x_last: -1,
        y_first: 2,
        y_last: -1,
    };

    /// The interior of a picture `stride` samples wide and `rows` tall
    /// whose slices hold every one of its rows. A luma window starts at
    /// most two samples before the block's position and ends at most
    /// `16 + 3` after it, so its last column is at most `x + 18` and its
    /// last row `y + 18`.
    ///
    /// Only for a whole picture ([`Planes::with_origin`]); never for a
    /// band ([`Planes::band`]), whose slices hold some of the rows only
    /// and whose interior is [`Interior::band`].
    const fn of(stride: usize, rows: usize) -> Interior {
        Interior {
            x_last: stride as i64 - 19,
            y_first: 2,
            y_last: rows as i64 - 19,
        }
    }

    /// The interior of a band of a picture `stride` by `rows` whose luma
    /// slice, `len[0]` samples, holds rows from `first[0]` on at
    /// `origin[0]`, and whose chroma slices, the shorter `len[1]` samples,
    /// hold chroma rows from `first[1]` on at `origin[1]`
    /// ([`Planes::band`]).
    ///
    /// Across, as [`Interior::of`]: for x from 2 to `stride - 19` every
    /// window's columns lie in 0 to `stride - 1` (the luma window's from
    /// `x - 2` to at most `x + 18`; the chroma window's from `xc` to at
    /// most `xc + 8` with `x - 2 <= 2 * xc <= x`, so from 0 to at most
    /// `stride / 2 - 2`), so a window lies in its slice where each of its
    /// rows does. A slice holds plane row `r` whole where
    /// `first <= r < first + n`, with `n = (len - origin) / s` rows of `s`
    /// samples after its origin (`at + r * s` and `at + (r + 1) * s`
    /// inside the slice, `at = origin - first * s`; the part of a row
    /// after them, and any rows past `rows`, are left out, which only
    /// narrows the interior). Down: the luma window's rows `y - 2` to at
    /// most `y + 18` lie there where
    /// `first[0] + 2 <= y <= first[0] + n[0] - 19`; the chroma window's
    /// rows `yc` to at most `yc + 8`, with `y - 2 <= 2 * yc <= y` (and
    /// `2 * yc` even), where `2 * first[1] + 1 <= y` and
    /// `y <= 2 * (first[1] + n[1]) - 17`; and the samples used lie in the
    /// picture where `2 <= y <= rows - 19`, as for a whole picture, the
    /// first of which `first[0] + 2 <= y` implies.
    fn band(
        stride: usize,
        rows: usize,
        first: [usize; 2],
        origin: [usize; 2],
        len: [usize; 2],
    ) -> Interior {
        let r = rows.min(MAX_SIZE) as i64;
        // Rows held after each origin, at most `rows`: none where a row is
        // 0 samples (which `Planes::band` declines).
        let held = |len: usize, origin: usize, s: usize| {
            len.saturating_sub(origin)
                .checked_div(s)
                .map_or(0, |n| n.min(rows.min(MAX_SIZE)) as i64)
        };
        let n = held(len[0], origin[0], stride);
        let cn = held(len[1], origin[1], stride / 2);
        let [f, cf] = first.map(|f| f.min(MAX_SIZE) as i64);
        Interior {
            x_last: stride.min(MAX_SIZE) as i64 - 19,
            y_first: (f + 2).max(2 * cf + 1),
            y_last: (r - 19).min(f + n - 19).min(2 * (cf + cn) - 17),
        }
    }

    /// Whether the block at (`xi`, `yi`) lies inside.
    #[inline(always)]
    const fn holds(&self, xi: i64, yi: i64) -> bool {
        2 <= xi && xi <= self.x_last && self.y_first <= yi && yi <= self.y_last
    }
}

impl<'a> Planes<'a> {
    /// The picture whose luma plane `y` is `rows` rows of `stride` samples
    /// and whose chroma planes are `rows / 2` rows of `stride / 2`, or
    /// `None` where a plane is shorter than that, or `stride` or `rows` is
    /// odd, zero or above [`MAX_SIZE`]. Longer planes are fine; what
    /// follows is never part of a prediction.
    pub fn new(
        y: &'a [u8],
        cb: &'a [u8],
        cr: &'a [u8],
        stride: usize,
        rows: usize,
    ) -> Option<Planes<'a>> {
        Planes::with_origin(y, cb, cr, stride, rows, [0, 0])
    }

    /// [`Planes::new`] for planes whose sample (0, 0) sits at `origin[0]`
    /// in `y` and at `origin[1]` in `cb` and in `cr`, each plane's rows
    /// following it a stride apart: `None` where a slice does not hold its
    /// origin and the rows after it, or an origin is above [`MAX_ORIGIN`].
    ///
    /// What lies before the origin and after the last row is never part
    /// of a prediction, but it may be read: a window whose fraction leaves
    /// its samples outside the picture unused is taken where it lies in
    /// the slice ([`crate::Kernels::predict`]), so keeping some samples of
    /// slack there lets the windows at the picture's edges be taken.
    pub fn with_origin(
        y: &'a [u8],
        cb: &'a [u8],
        cr: &'a [u8],
        stride: usize,
        rows: usize,
        origin: [usize; 2],
    ) -> Option<Planes<'a>> {
        let [luma_origin, chroma_origin] = origin;
        let chroma = (stride / 2)
            .checked_mul(rows / 2)?
            .checked_add(chroma_origin)?;
        let fits = (2..=MAX_SIZE).contains(&stride)
            && (2..=MAX_SIZE).contains(&rows)
            && stride % 2 == 0
            && rows % 2 == 0
            && luma_origin <= MAX_ORIGIN
            && chroma_origin <= MAX_ORIGIN
            && y.len() >= stride.checked_mul(rows)?.checked_add(luma_origin)?
            && cb.len() >= chroma
            && cr.len() >= chroma;
        fits.then_some(Planes {
            y,
            cb,
            cr,
            stride,
            rows,
            origin,
            at: origin.map(|o| o as i64),
            interior: Interior::of(stride, rows),
        })
    }

    /// The rows of a picture `rows` luma rows of `stride` samples that one
    /// band of it stores: `y` holds its luma rows from row `first[0]` on,
    /// and `cb` and `cr` its chroma rows from row `first[1]` on, each
    /// plane's first row at its `origin` in its slice and as many rows as
    /// the slice holds. A plane whose first row is not row 0 starts at its
    /// slice's start (origin 0), so that a window above that row lies
    /// before the slice; one from row 0 may keep slack before it, as
    /// [`Planes::with_origin`]'s do. `None` where `stride` or `rows` is
    /// odd, zero or above [`MAX_SIZE`], an origin is above [`MAX_ORIGIN`]
    /// or past its slice, or is not 0 below row 0, or a first row is past
    /// the picture.
    ///
    /// [`crate::Kernels::predict`] tests the samples a prediction uses
    /// against the whole picture, `rows` rows, and declines a window that
    /// reaches rows the slices do not hold, as it declines one past a
    /// slice's end. The rows a window takes are read as the picture's:
    /// which rows of a band hold final samples is for the caller to
    /// know, and to choose the band by.
    pub fn band(
        y: &'a [u8],
        cb: &'a [u8],
        cr: &'a [u8],
        stride: usize,
        rows: usize,
        first: [usize; 2],
        origin: [usize; 2],
    ) -> Option<Planes<'a>> {
        let [luma_origin, chroma_origin] = origin;
        let [luma_first, chroma_first] = first;
        let fits = (2..=MAX_SIZE).contains(&stride)
            && (2..=MAX_SIZE).contains(&rows)
            && stride % 2 == 0
            && rows % 2 == 0
            && luma_first <= rows
            && chroma_first <= rows / 2
            && luma_origin <= MAX_ORIGIN.min(y.len())
            && chroma_origin <= MAX_ORIGIN.min(cb.len()).min(cr.len())
            && (luma_first == 0 || luma_origin == 0)
            && (chroma_first == 0 || chroma_origin == 0);
        if !fits {
            return None;
        }
        // Within +-2^31 with these bounds, as `place` requires.
        let at = [
            luma_origin as i64 - luma_first.checked_mul(stride)? as i64,
            chroma_origin as i64 - chroma_first.checked_mul(stride / 2)? as i64,
        ];
        Some(Planes {
            y,
            cb,
            cr,
            stride,
            rows,
            origin,
            at,
            interior: Interior::band(
                stride,
                rows,
                first,
                origin,
                [y.len(), cb.len().min(cr.len())],
            ),
        })
    }

    /// Luma samples per row, and from one row to the next.
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// Luma rows.
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// Where the first row held of the luma plane sits in [`Planes::y`],
    /// and of each chroma plane in [`Planes::cb`] and [`Planes::cr`]:
    /// sample (0, 0)'s, but for [`Planes::band`].
    pub const fn origin(&self) -> [usize; 2] {
        self.origin
    }

    /// Hints that a block's rows will be read ([`crate::prefetch`]): `h`
    /// luma rows from luma sample (`x`, `y`), and `h / 2` rows of each
    /// chroma plane from chroma sample (`x >> 1`, `y >> 1`), one hint a row
    /// at that column. A position outside the picture, or rows the slices
    /// do not hold, are not hinted; a hint changes no value.
    ///
    /// Out of line: its caller runs it for one block in many, and inlined,
    /// its three walks cost that caller registers on every block.
    #[inline(never)]
    pub fn request(&self, x: i64, y: i64, h: usize) {
        let stride = self.stride;
        let [luma, chroma] = self.at;
        hint_rows(self.y, luma, stride, [x, y], h);
        let half = stride / 2;
        hint_rows(self.cb, chroma, half, [x >> 1, y >> 1], h / 2);
        hint_rows(self.cr, chroma, half, [x >> 1, y >> 1], h / 2);
    }
}

/// [`Planes::request`] on one plane `stride` samples wide whose sample
/// (0, 0) would sit at `at` in `plane`.
#[inline(always)]
fn hint_rows(plane: &[u8], at: i64, stride: usize, [x, y]: [i64; 2], rows: usize) {
    let Ok(width) = i64::try_from(stride) else {
        return;
    };
    if !(0..width).contains(&x) || y < 0 {
        return;
    }
    let first = y
        .checked_mul(width)
        .and_then(|row| row.checked_add(at))
        .and_then(|row| row.checked_add(x))
        .and_then(|i| usize::try_from(i).ok());
    if let Some(first) = first {
        crate::prefetch::rows_for_read(plane, first, stride, rows);
    }
}

/// The picture being decoded, or the rows of it a band holds: three
/// planes of the same stride as the reference's (chroma's half), the luma
/// plane starting at the picture's luma row `first[0]` and the chroma
/// planes at its chroma row `first[1]`, each at an origin in its slice
/// ([`PlanesMut::with_origin`]).
#[derive(Debug)]
pub struct PlanesMut<'a> {
    y: &'a mut [u8],
    cb: &'a mut [u8],
    cr: &'a mut [u8],
    stride: usize,
    first: [usize; 2],
    origin: [usize; 2],
}

impl<'a> PlanesMut<'a> {
    /// The planes, as they are, with their first rows at the start of
    /// each slice: [`crate::Kernels::predict`] checks each block it writes
    /// against them, so building them checks nothing.
    pub fn new(
        y: &'a mut [u8],
        cb: &'a mut [u8],
        cr: &'a mut [u8],
        stride: usize,
        first: [usize; 2],
    ) -> PlanesMut<'a> {
        PlanesMut::with_origin(y, cb, cr, stride, first, [0, 0])
    }

    /// [`PlanesMut::new`] for planes whose first luma row starts at
    /// `origin[0]` in `y` and whose first chroma rows start at `origin[1]`
    /// in `cb` and `cr`. An origin above [`MAX_ORIGIN`] makes every block
    /// declined.
    pub fn with_origin(
        y: &'a mut [u8],
        cb: &'a mut [u8],
        cr: &'a mut [u8],
        stride: usize,
        first: [usize; 2],
        origin: [usize; 2],
    ) -> PlanesMut<'a> {
        PlanesMut {
            y,
            cb,
            cr,
            stride,
            first,
            origin,
        }
    }
}

/// Where one call reads and writes, every range checked: the result of
/// [`place`].
#[derive(Clone, Copy, Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub(crate) struct Placement {
    /// Luma samples from one row to the next, in the reference and in the
    /// picture; chroma's are half.
    pub(crate) stride: usize,
    /// The first sample of the luma window, `(w + 5)` by `(h + 5)`, in the
    /// reference's luma slice.
    pub(crate) luma_src: usize,
    /// The first sample of the chroma windows, `(w / 2 + 1)` by
    /// `(h / 2 + 1)`, in each chroma slice of the reference.
    pub(crate) chroma_src: usize,
    /// The first sample of the luma block in the picture's luma slice.
    pub(crate) luma_dst: usize,
    /// The first sample of the chroma blocks in each chroma slice.
    pub(crate) chroma_dst: usize,
    /// xFracL and yFracL, 0 to 3.
    pub(crate) xf: u8,
    pub(crate) yf: u8,
    /// xFracC and yFracC, 0 to 7.
    pub(crate) cxf: u8,
    pub(crate) cyf: u8,
}

/// Where a `w` by `h` block at luma position (`px`, `py`) of the picture,
/// predicted with motion vector `mv` (quarter luma samples) from `src`,
/// reads and writes, or `None` where the call declines it: a size other
/// than 4, 8 or 16 each way; a weighting outside [`Weighting::in_range`];
/// a sample the prediction uses outside the reference picture, where the
/// specification replicates the picture's edge samples (Equations 8-228
/// and 8-229 and their chroma forms); a window row wider than the stride
/// (a plane narrower than `w + 5`); a window that does not lie in its
/// slice; a block outside the picture's planes; planes of different
/// strides; or a position beyond [`MAX_SIZE`].
///
/// A window reaches past the samples its fraction uses (the kernels'
/// contract, [`crate::Kernels::luma_mc`] and [`crate::Kernels::chroma_mc`]):
/// the luma window's two columns before the block and three after are
/// used only where xFracL is not 0, and its rows likewise with yFracL;
/// the chroma window's last column only where xFracC is not 0, and its
/// last row with yFracC. So the test of the picture is of the samples
/// used, and the window may reach past the picture where they do not:
/// into the row before or after, or into what the slice holds before the
/// origin or after the last row, which the prediction does not depend
/// on.
///
/// Where it returns a placement, every row of each region lies inside its
/// slice: each window, from its first sample to its last, is checked
/// against its slice's length here, and so is each block's last row. This
/// is the check the vector kernels' reads and writes rest on.
#[allow(
    clippy::too_many_arguments,
    reason = "the call's arguments, checked in one place"
)]
#[inline(always)]
pub(crate) fn place(
    dst: &PlanesMut<'_>,
    src: &Planes<'_>,
    px: usize,
    py: usize,
    w: usize,
    h: usize,
    mv: [i16; 2],
    ops: [PlaneOp; 3],
) -> Option<Placement> {
    let sizes = matches!(w, 4 | 8 | 16) && matches!(h, 4 | 8 | 16);
    let weights = ops.iter().all(|op| match op {
        PlaneOp::Weigh(weighting) => weighting.in_range(),
        PlaneOp::Put | PlaneOp::Avg => true,
    });
    let stride = src.stride;
    // `Planes::with_origin` and `Planes::band` held `stride` and `rows` to
    // `MAX_SIZE`, the reference's origins to `MAX_ORIGIN` and its first
    // rows to `rows`, so that where sample (0, 0) would sit is within
    // +-2^31, and these hold the rest, so
    // that every value below lies within +-2^32 and none of the
    // arithmetic can overflow; a slice's length is at most `isize::MAX`,
    // which an `i64` holds.
    let [first, chroma_first] = dst.first;
    let [origin, chroma_origin] = dst.origin;
    let bounded = px <= MAX_SIZE
        && py <= MAX_SIZE
        && first <= py
        && chroma_first <= py / 2
        && origin <= MAX_ORIGIN
        && chroma_origin <= MAX_ORIGIN;
    if !sizes || !weights || !bounded || dst.stride != stride {
        return None;
    }
    let s = stride as i64;
    let (x0, y0, wi, hi) = (px as i64, py as i64, w as i64, h as i64);
    let [mx, my] = mv.map(i64::from);
    let (xf, yf, cxf, cyf) = (mx & 3, my & 3, mx & 7, my & 7);
    // The block's integer position in the reference (Equations 8-228 and
    // 8-229), and its chroma position: the block's chroma position and
    // the vector's whole chroma samples (Equations 8-229 and 8-230 with
    // Table 8-9's vertical offset of 0 in a frame).
    let (xi, yi) = (x0 + (mx >> 2), y0 + (my >> 2));
    let (cx, cy) = ((x0 >> 1) + (mx >> 3), (y0 >> 1) + (my >> 3));
    let cs = s >> 1;
    // Each window's first sample in its slice: the luma window's from two
    // before the block, and the chroma windows'. Of a band
    // (`Planes::band`), a window that starts above the first row held
    // starts before the slice.
    let [luma_at, chroma_at] = src.at;
    let luma_src = luma_at + (yi - 2) * s + xi - 2;
    let chroma_src = chroma_at + cy * cs + cx;
    // A block inside the reference's interior needs no test of its
    // windows (`Interior`); any other is tested window by window.
    if !src.interior.holds(xi, yi) {
        let windows = Windows {
            size: (wi, hi),
            at: (xi, yi),
            fraction: (xf, yf, cxf, cyf),
            chroma: (cx, cy),
            first: (luma_src, chroma_src),
        };
        if !windows.fit(src) {
            return None;
        }
    }
    // The blocks written: the block's rows from the planes' first, each
    // block's last row ending inside its plane.
    let (cw, ch) = (wi >> 1, hi >> 1);
    let (row, crow) = (y0 - first as i64, (y0 >> 1) - chroma_first as i64);
    let luma_dst = origin as i64 + row * s + x0;
    let chroma_dst = chroma_origin as i64 + crow * cs + (x0 >> 1);
    let written = x0 + wi <= s
        && luma_dst + (hi - 1) * s + wi <= dst.y.len() as i64
        && chroma_dst + (ch - 1) * cs + cw <= dst.cb.len().min(dst.cr.len()) as i64;
    if !written {
        return None;
    }
    // Every value is non-negative here, so the conversions keep them.
    Some(Placement {
        stride,
        luma_src: luma_src as usize,
        chroma_src: chroma_src as usize,
        luma_dst: luma_dst as usize,
        chroma_dst: chroma_dst as usize,
        xf: xf as u8,
        yf: yf as u8,
        cxf: cxf as u8,
        cyf: cyf as u8,
    })
}

/// One block's windows in a reference, for the test of a block outside
/// the reference's interior ([`Windows::fit`]).
struct Windows {
    /// The block's width and height.
    size: (i64, i64),
    /// Its integer luma position in the reference.
    at: (i64, i64),
    /// xFracL, yFracL, xFracC and yFracC.
    fraction: (i64, i64, i64, i64),
    /// Its integer chroma position.
    chroma: (i64, i64),
    /// The first sample of the luma window and of the chroma windows, in
    /// their slices.
    first: (i64, i64),
}

impl Windows {
    /// Whether every window lies where [`place`] requires, tested window
    /// by window: the samples its fraction uses inside the picture
    /// (Equations 8-228 and 8-229 and their chroma forms replicate the
    /// edge for any outside, which the call declines), the whole window
    /// inside its slice, and a window row within a stride.
    ///
    /// Out of line and cold: most blocks lie inside the interior, and this
    /// test kept inline would crowd the registers of every call.
    #[cold]
    #[inline(never)]
    fn fit(&self, src: &Planes<'_>) -> bool {
        let Windows {
            size: (wi, hi),
            at: (xi, yi),
            fraction: (xf, yf, cxf, cyf),
            chroma: (cx, cy),
            first: (luma_src, chroma_src),
        } = *self;
        let (s, r) = (src.stride as i64, src.rows as i64);
        // A window row fits within a stride, as `Block::new` requires of
        // the views every form builds: the plane is at least `w + 5`
        // samples wide (and its chroma `w / 2 + 1`). Without it the forms
        // would differ on which blocks of a plane narrower than that they
        // take.
        let fits = wi + 5 <= s;
        // The luma samples used beyond the block: two before and three
        // after, across where xFracL is not 0 and down where yFracL is
        // not 0 (Equations 8-239 to 8-241).
        let (before_x, after_x) = if xf != 0 { (2, 3) } else { (0, 0) };
        let (before_y, after_y) = if yf != 0 { (2, 3) } else { (0, 0) };
        // The chroma window of a 4:2:0 picture, `cw + 1` by `ch + 1`
        // samples, its last column used where xFracC is not 0 and its
        // last row where yFracC is not 0 (Equation 8-266).
        let (cs, cw, ch) = (s >> 1, wi >> 1, hi >> 1);
        let used = xi - before_x >= 0
            && yi - before_y >= 0
            && xi + wi + after_x <= s
            && yi + hi + after_y <= r
            && cx >= 0
            && cy >= 0
            && cx + cw + i64::from(cxf != 0) <= cs
            && cy + ch + i64::from(cyf != 0) <= r >> 1;
        // Each window, from its first sample to one past its last, in its
        // slice: the luma window `h + 5` rows of `w + 5`, and the chroma
        // windows `ch + 1` rows of `cw + 1`.
        let held = luma_src >= 0
            && luma_src + (hi + 4) * s + wi + 5 <= src.y.len() as i64
            && chroma_src >= 0
            && chroma_src + ch * cs + cw < src.cb.len().min(src.cr.len()) as i64;
        fits && used && held
    }
}

/// [`crate::Kernels::predict`] by parts: the views of each plane, then
/// its interpolation kernel, as the decoder calls them one plane at a
/// time. The trait's reference semantics, and what the kernels without a
/// fused form use.
#[allow(clippy::too_many_arguments, reason = "the trait method's arguments")]
pub(crate) fn predict_by_parts<K: Kernels + ?Sized>(
    kernels: &K,
    dst: PlanesMut<'_>,
    src: &Planes<'_>,
    px: usize,
    py: usize,
    w: usize,
    h: usize,
    mv: [i16; 2],
    ops: [PlaneOp; 3],
) -> bool {
    let Some(p) = place(&dst, src, px, py, w, h, mv, ops) else {
        return false;
    };
    let (cstride, cw, ch) = (p.stride / 2, w / 2, h / 2);
    let PlanesMut { y, cb, cr, .. } = dst;
    // `place` checked every one of these ranges, so none fails; one that
    // did would decline the block before anything is written.
    let views = (
        Block::new(src.y, p.luma_src, p.stride, w + 5, h + 5),
        Block::new(src.cb, p.chroma_src, cstride, cw + 1, ch + 1),
        Block::new(src.cr, p.chroma_src, cstride, cw + 1, ch + 1),
        BlockMut::new(y, p.luma_dst, p.stride, w, h),
        BlockMut::new(cb, p.chroma_dst, cstride, cw, ch),
        BlockMut::new(cr, p.chroma_dst, cstride, cw, ch),
    );
    let (Ok(sy), Ok(scb), Ok(scr), Ok(dy), Ok(dcb), Ok(dcr)) = views else {
        return false;
    };
    let [luma, for_cb, for_cr] = ops;
    match luma {
        PlaneOp::Put => kernels.luma_mc(dy, sy, p.xf, p.yf),
        PlaneOp::Avg => kernels.luma_mc_avg(dy, sy, p.xf, p.yf),
        PlaneOp::Weigh(weighting) => kernels.luma_mc_weighted(dy, sy, p.xf, p.yf, weighting),
    }
    for (op, dst, src) in [(for_cb, dcb, scb), (for_cr, dcr, scr)] {
        match op {
            PlaneOp::Put => kernels.chroma_mc(dst, src, p.cxf, p.cyf),
            PlaneOp::Avg => kernels.chroma_mc_avg(dst, src, p.cxf, p.cyf),
            PlaneOp::Weigh(weighting) => {
                kernels.chroma_mc_weighted(dst, src, p.cxf, p.cyf, weighting);
            }
        }
    }
    true
}

impl<'a> Planes<'a> {
    /// The luma plane.
    pub const fn y(&self) -> &'a [u8] {
        self.y
    }

    /// The Cb plane.
    pub const fn cb(&self) -> &'a [u8] {
        self.cb
    }

    /// The Cr plane.
    pub const fn cr(&self) -> &'a [u8] {
        self.cr
    }
}

impl<'a> PlanesMut<'a> {
    /// The three planes, for the vector kernels.
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn into_planes(self) -> [&'a mut [u8]; 3] {
        [self.y, self.cb, self.cr]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every block size `place` takes.
    const SIZES: [(usize, usize); 9] = [
        (4, 4),
        (4, 8),
        (4, 16),
        (8, 4),
        (8, 8),
        (8, 16),
        (16, 4),
        (16, 8),
        (16, 16),
    ];

    /// The operations a block is tried with: in range for 13 of 16, the
    /// rest a weighting outside its range on one plane.
    fn ops(k: usize) -> [PlaneOp; 3] {
        use PlaneOp::{Avg, Put, Weigh};
        let uni = |log_wd, w, o| Weigh(Weighting::Uni { log_wd, w, o });
        let bi = |log_wd, w0, w1, o| Weigh(Weighting::Bi { log_wd, w0, w1, o });
        match k % 16 {
            0 | 5 | 10 => [Put, Put, Put],
            1 | 6 | 11 => [Avg, Avg, Avg],
            2 | 7 => [uni(5, 40, -3), Put, Avg],
            3 | 8 => [Put, bi(6, -128, 128, 127), uni(0, 1, 0)],
            4 | 9 | 12 => [Avg, Put, bi(7, 64, 0, -128)],
            13 => [uni(8, 1, 0), Put, Put],
            14 => [Put, bi(2, 129, 0, 0), Put],
            _ => [Put, Put, uni(-1, 1, 0)],
        }
    }

    /// A reference's three slices: `before` samples ahead of each plane's
    /// origin (luma, chroma) and `after` past its last row (luma, Cb, Cr).
    struct Reference {
        y: Vec<u8>,
        cb: Vec<u8>,
        cr: Vec<u8>,
        stride: usize,
        rows: usize,
        origin: [usize; 2],
    }

    impl Reference {
        fn new(stride: usize, rows: usize, before: [usize; 2], after: [usize; 3]) -> Reference {
            let chroma = stride / 2 * (rows / 2) + before[1];
            Reference {
                y: vec![0; before[0] + stride * rows + after[0]],
                cb: vec![0; chroma + after[1]],
                cr: vec![0; chroma + after[2]],
                stride,
                rows,
                origin: before,
            }
        }

        /// The slices of the layouts every test takes: exactly as long as
        /// `Planes::with_origin` requires at origin 0; a few rows and
        /// samples before the origins, nothing after; slack each side, the
        /// chroma slices of different lengths; and a few samples before and
        /// after.
        fn layouts(stride: usize, rows: usize) -> Vec<Reference> {
            let cs = stride / 2;
            vec![
                Reference::new(stride, rows, [0, 0], [0, 0, 0]),
                Reference::new(stride, rows, [2 * stride + 3, cs + 1], [0, 0, 0]),
                Reference::new(
                    stride,
                    rows,
                    [4 * stride + 2, 2 * cs + 1],
                    [stride + 7, cs, cs + 3],
                ),
                Reference::new(stride, rows, [1, 0], [3, 1, 0]),
            ]
        }

        fn planes(&self) -> Planes<'_> {
            Planes::with_origin(
                &self.y,
                &self.cb,
                &self.cr,
                self.stride,
                self.rows,
                self.origin,
            )
            .expect("planes")
        }
    }

    /// The same reference with an empty interior, so that `place` tests
    /// every block window by window, as it did before the interior.
    fn without_interior<'a>(src: &Planes<'a>) -> Planes<'a> {
        Planes {
            interior: Interior::NONE,
            ..*src
        }
    }

    /// A picture `place` writes to: its stride, first rows, origins and
    /// slices.
    struct Picture {
        y: Vec<u8>,
        cb: Vec<u8>,
        cr: Vec<u8>,
        stride: usize,
        first: [usize; 2],
        origin: [usize; 2],
    }

    impl Picture {
        fn new(stride: usize, first: [usize; 2], origin: [usize; 2], len: [usize; 3]) -> Picture {
            Picture {
                y: vec![0; len[0]],
                cb: vec![0; len[1]],
                cr: vec![0; len[2]],
                stride,
                first,
                origin,
            }
        }

        /// Pictures for a reference `stride` by `rows`: the whole picture
        /// as long as it must be; with slack before and after; a band from
        /// halfway down; a band whose chroma starts a row late; slices a
        /// sample short; a luma slice of half the rows; and a different
        /// stride, which declines every block.
        fn targets(stride: usize, rows: usize) -> Vec<Picture> {
            let (s, cs) = (stride, stride / 2);
            let (luma, chroma) = (s * rows, cs * (rows / 2));
            let band = (rows / 2) & !1;
            let (band_luma, band_chroma) = (s * (rows - band), cs * (rows / 2 - band / 2));
            vec![
                Picture::new(s, [0, 0], [0, 0], [luma, chroma, chroma]),
                Picture::new(
                    s,
                    [0, 0],
                    [2 * s + 3, cs + 1],
                    [2 * s + 3 + luma + 5, cs + 1 + chroma, cs + 1 + chroma + 2],
                ),
                Picture::new(
                    s,
                    [band, band / 2],
                    [s, cs],
                    [s + band_luma, cs + band_chroma, cs + band_chroma],
                ),
                Picture::new(
                    s,
                    [band, band / 2 + 1],
                    [0, 0],
                    [band_luma, band_chroma, band_chroma],
                ),
                Picture::new(
                    s,
                    [0, 0],
                    [0, 0],
                    [luma - 1, chroma, chroma.saturating_sub(1)],
                ),
                Picture::new(s, [0, 0], [0, 0], [s * (rows / 2), chroma, chroma]),
                Picture::new(s + 2, [0, 0], [0, 0], [luma, chroma, chroma]),
            ]
        }
    }

    /// How many blocks the interior placed, and how many the window test
    /// placed and declined.
    #[derive(Default)]
    struct Tally {
        inside: usize,
        tested: usize,
        declined: usize,
    }

    /// `place` with the reference's interior and with the window test
    /// alone, which must agree on every input.
    #[allow(clippy::too_many_arguments, reason = "place's arguments")]
    fn check(
        tally: &mut Tally,
        picture: &mut Picture,
        src: &Planes<'_>,
        px: usize,
        py: usize,
        w: usize,
        h: usize,
        mv: [i16; 2],
        ops: [PlaneOp; 3],
    ) {
        let dst = PlanesMut::with_origin(
            &mut picture.y,
            &mut picture.cb,
            &mut picture.cr,
            picture.stride,
            picture.first,
            picture.origin,
        );
        let got = place(&dst, src, px, py, w, h, mv, ops);
        let want = place(&dst, &without_interior(src), px, py, w, h, mv, ops);
        assert_eq!(
            got,
            want,
            "{w}x{h} at ({px}, {py}), {mv:?}, {ops:?}, reference {}x{} at {:?} in {} {} {}, \
             picture {} first {:?} at {:?} in {} {} {}",
            src.stride,
            src.rows,
            src.origin,
            src.y.len(),
            src.cb.len(),
            src.cr.len(),
            dst.stride,
            dst.first,
            dst.origin,
            dst.y.len(),
            dst.cb.len(),
            dst.cr.len(),
        );
        let (xi, yi) = (
            px as i64 + i64::from(mv[0] >> 2),
            py as i64 + i64::from(mv[1] >> 2),
        );
        match got {
            Some(_) if src.interior.holds(xi, yi) => tally.inside += 1,
            Some(_) => tally.tested += 1,
            None => tally.declined += 1,
        }
    }

    /// Positions and vector components along one axis of a plane `n`
    /// samples long: for each position, integer target position (the
    /// block's position plus the vector's whole samples) and fraction
    /// (`mv & 7`, which also gives `mv & 3`), the component that reaches
    /// it, where the position's parity allows one. `full` takes every
    /// target from 5 samples before the plane to 23 inside it at the start
    /// and from 23 inside to 5 past it at the end, a few as far as 21
    /// samples outside, every fraction, and positions at the start, the
    /// middle and just past the end, of both parities; otherwise a few
    /// targets around each edge of the interior, a fraction of each kind,
    /// and positions 0 and 1.
    fn axis(n: usize, full: bool) -> Vec<(usize, i16)> {
        let n = n as i64;
        let (positions, mut targets, fractions): (Vec<i64>, Vec<i64>, Vec<i64>) = if full {
            (
                vec![0, 1, n / 2, n / 2 + 1, n, n + 1],
                (-5..=23)
                    .chain(n - 23..=n + 5)
                    .chain([-21, -20, -13, -8, n / 2, n + 8, n + 13, n + 20, n + 21])
                    .collect(),
                (0..8).collect(),
            )
        } else {
            (
                vec![0, 1],
                vec![
                    -2,
                    0,
                    1,
                    2,
                    3,
                    n / 2,
                    n - 20,
                    n - 19,
                    n - 18,
                    n - 17,
                    n - 4,
                    n - 3,
                    n,
                ],
                vec![0, 1, 4, 5],
            )
        };
        targets.sort_unstable();
        targets.dedup();
        let mut out = Vec::new();
        for &p in &positions {
            for &t in &targets {
                for &f in &fractions {
                    // m = 8q + f moves the block by 2q + (f >> 2) samples.
                    let d = t - p - (f >> 2);
                    if d % 2 == 0 {
                        let m = i16::try_from(4 * d + f).expect("vector");
                        assert_eq!(p + i64::from(m >> 2), t);
                        out.push((p as usize, m));
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    #[test]
    fn place_with_the_interior_is_place_with_the_window_test_near_the_edges() {
        // Exhaustive near the edges: every target and fraction along one
        // axis against a few of each kind along the other, every size; the
        // reference layouts, pictures and operations in turn.
        let mut tally = Tally::default();
        let mut k = 0usize;
        for (stride, rows) in [
            (2, 2),
            (4, 4),
            (18, 18),
            (20, 20),
            (20, 22),
            (22, 20),
            (22, 22),
            (24, 24),
            (22, 40),
            (40, 24),
            (64, 64),
        ] {
            let (full_x, short_x) = (axis(stride, true), axis(stride, false));
            let (full_y, short_y) = (axis(rows, true), axis(rows, false));
            let pairs: Vec<((usize, i16), (usize, i16))> = full_x
                .iter()
                .flat_map(|&x| short_y.iter().map(move |&y| (x, y)))
                .chain(
                    short_x
                        .iter()
                        .flat_map(|&x| full_y.iter().map(move |&y| (x, y))),
                )
                .collect();
            let mut pictures = Picture::targets(stride, rows);
            let layouts = Reference::layouts(stride, rows);
            let references: Vec<Planes<'_>> = layouts.iter().map(Reference::planes).collect();
            for &((px, mx), (py, my)) in &pairs {
                for (w, h) in SIZES {
                    k += 1;
                    let picture = k / 7 % pictures.len();
                    check(
                        &mut tally,
                        &mut pictures[picture],
                        &references[k / 5 % references.len()],
                        px,
                        py,
                        w,
                        h,
                        [mx, my],
                        ops(k),
                    );
                }
            }
        }
        assert!(tally.inside > 10_000, "{} inside", tally.inside);
        assert!(tally.tested > 10_000, "{} tested", tally.tested);
        assert!(tally.declined > 10_000, "{} declined", tally.declined);
    }

    /// A xorshift generator, for inputs that are reproducible.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        /// A value from 0 to `n - 1`.
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        /// A position and vector component along an axis `n` long: any
        /// vector of up to `n + 24` samples each way (`mode` 0), one whose
        /// target lies within 23 samples of an edge (1), or one whose
        /// target lies in the plane or up to 3 samples outside it (2).
        fn component(&mut self, n: usize, mode: u64) -> (usize, i16) {
            let p = self.below(n as u64 + 8) as i64;
            let n = n as i64;
            let t = match mode {
                0 => {
                    let reach = 4 * (n + 24);
                    let m = self.below(2 * reach as u64 + 1) as i64 - reach;
                    return (p as usize, i16::try_from(m).expect("vector"));
                }
                1 => {
                    let edge = if self.below(2) == 0 { 0 } else { n };
                    edge + self.below(47) as i64 - 23
                }
                _ => self.below(n as u64 + 7) as i64 - 3,
            };
            self.aim(p, t)
        }

        /// The vector component from position `p` to target `t`, with any
        /// fraction, the target moved by one where the parity leaves no
        /// vector.
        fn aim(&mut self, p: i64, t: i64) -> (usize, i16) {
            let f = self.below(8) as i64;
            let d = t - p - (f >> 2);
            let d = d - d.rem_euclid(2);
            (p as usize, i16::try_from(4 * d + f).expect("vector"))
        }
    }

    #[test]
    fn place_with_the_interior_is_place_with_the_window_test_on_random_blocks() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut tally = Tally::default();
        for _ in 0..10_000 {
            let stride = 2 * (1 + rng.below(40) as usize);
            let rows = 2 * (1 + rng.below(40) as usize);
            let cs = stride / 2;
            let before = [
                rng.below(4) as usize * stride + rng.below(6) as usize,
                rng.below(3) as usize * cs + rng.below(3) as usize,
            ];
            let mut after = || {
                if rng.below(2) == 0 {
                    0
                } else {
                    rng.below(2 * stride as u64 + 9) as usize
                }
            };
            let after = [after(), after(), after()];
            let reference = Reference::new(stride, rows, before, after);
            let src = reference.planes();
            let mut pictures = Picture::targets(stride, rows);
            for _ in 0..100 {
                let mode = rng.below(3);
                let (px, mx) = rng.component(stride, mode);
                let (py, my) = rng.component(rows, mode);
                let (w, h) = SIZES[rng.below(9) as usize];
                let picture = rng.below(pictures.len() as u64) as usize;
                let ops = ops(rng.below(16) as usize);
                check(
                    &mut tally,
                    &mut pictures[picture],
                    &src,
                    px,
                    py,
                    w,
                    h,
                    [mx, my],
                    ops,
                );
            }
        }
        assert!(tally.inside > 10_000, "{} inside", tally.inside);
        assert!(tally.tested > 5_000, "{} tested", tally.tested);
        assert!(tally.declined > 10_000, "{} declined", tally.declined);
    }

    /// The windows of a `w` by `h` block at (`px`, `py`) with vector `mv`,
    /// as `place` builds them.
    fn windows(
        src: &Planes<'_>,
        px: usize,
        py: usize,
        w: usize,
        h: usize,
        mv: [i16; 2],
    ) -> Windows {
        let s = src.stride as i64;
        let (x0, y0) = (px as i64, py as i64);
        let [mx, my] = mv.map(i64::from);
        let (xi, yi) = (x0 + (mx >> 2), y0 + (my >> 2));
        let (cx, cy) = ((x0 >> 1) + (mx >> 3), (y0 >> 1) + (my >> 3));
        let [luma_at, chroma_at] = src.at;
        Windows {
            size: (w as i64, h as i64),
            at: (xi, yi),
            fraction: (mx & 3, my & 3, mx & 7, my & 7),
            chroma: (cx, cy),
            first: (
                luma_at + (yi - 2) * s + xi - 2,
                chroma_at + cy * (s >> 1) + cx,
            ),
        }
    }

    /// The positions and vector components that put a block's integer
    /// position at `t` with fraction `f`: the vector of less than two
    /// samples, and one 7 or 8 samples back from a position further on.
    fn reaching(t: i64, f: i64) -> [(usize, i16); 2] {
        let near = t - (f >> 2);
        let far = near + 7 + (near & 1);
        [near, far].map(|p| {
            let m = i16::try_from(4 * (t - p - (f >> 2)) + f).expect("vector");
            (usize::try_from(p).expect("position"), m)
        })
    }

    #[test]
    fn every_window_of_a_block_inside_the_interior_fits() {
        for (stride, rows) in [
            (22, 22),
            (22, 24),
            (24, 22),
            (24, 24),
            (26, 40),
            (40, 26),
            (48, 48),
        ] {
            for reference in Reference::layouts(stride, rows) {
                let src = reference.planes();
                let mut inside = 0;
                for yi in -4..rows as i64 + 4 {
                    for xi in -4..stride as i64 + 4 {
                        if !src.interior.holds(xi, yi) {
                            continue;
                        }
                        inside += 1;
                        for (w, h) in SIZES {
                            for fx in 0..8 {
                                for fy in 0..8 {
                                    for (px, mx) in reaching(xi, fx) {
                                        for (py, my) in reaching(yi, fy) {
                                            let windows = windows(&src, px, py, w, h, [mx, my]);
                                            assert_eq!(windows.at, (xi, yi));
                                            assert!(
                                                windows.fit(&src),
                                                "{w}x{h} at ({px}, {py}), [{mx}, {my}], \
                                                 reference {stride}x{rows} at {:?}",
                                                src.origin
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                assert!(inside > 0, "{stride}x{rows}");
            }
        }
    }

    #[test]
    fn a_plane_narrower_or_shorter_than_21_samples_has_no_interior() {
        // Through `Planes::with_origin`, every even size it takes.
        for stride in (2..=64).step_by(2) {
            for rows in (2..=64).step_by(2) {
                if stride >= 21 && rows >= 21 {
                    continue;
                }
                let reference = Reference::new(stride, rows, [0, 0], [0, 0, 0]);
                let interior = reference.planes().interior;
                for yi in -40..100 {
                    for xi in -40..100 {
                        assert!(!interior.holds(xi, yi), "({xi}, {yi}) in {stride}x{rows}");
                    }
                }
            }
        }
        // And the interior's own form, odd sizes and 0 included.
        for small in 0..21 {
            for other in 0..=64 {
                for interior in [Interior::of(small, other), Interior::of(other, small)] {
                    for yi in -40..100 {
                        for xi in -40..100 {
                            assert!(!interior.holds(xi, yi), "({xi}, {yi}) {small} {other}");
                        }
                    }
                }
            }
        }
    }

    /// The rows of a reference one band holds (`Planes::band`): its luma
    /// slice `count[0]` rows from row `first[0]` on at `origin[0]`, and its
    /// chroma slices `count[1]` chroma rows from `first[1]` on at
    /// `origin[1]`, with `extra` samples after the last (luma, Cb, Cr).
    struct Band {
        y: Vec<u8>,
        cb: Vec<u8>,
        cr: Vec<u8>,
        stride: usize,
        rows: usize,
        first: [usize; 2],
        origin: [usize; 2],
    }

    impl Band {
        fn new(
            stride: usize,
            rows: usize,
            first: [usize; 2],
            origin: [usize; 2],
            count: [usize; 2],
            extra: [usize; 3],
        ) -> Band {
            let chroma = origin[1] + count[1] * (stride / 2);
            Band {
                y: vec![0; origin[0] + count[0] * stride + extra[0]],
                cb: vec![0; chroma + extra[1]],
                cr: vec![0; chroma + extra[2]],
                stride,
                rows,
                first,
                origin,
            }
        }

        /// Bands of a picture `stride` by `rows`: from row 0 and from rows
        /// near it, mid-picture and near the end, of both parities; chroma
        /// from half the luma row and from one row either side of it (as
        /// another apron would make it); holding exactly the rows to the
        /// picture's end, a band of 24 luma rows and 12 or 11 chroma rows,
        /// or rows past the end; with nothing after the last row or part
        /// of a row; and slack before an origin at row 0 for every other.
        fn layouts(stride: usize, rows: usize) -> Vec<Band> {
            let (s, cs) = (stride, stride / 2);
            let mut firsts = vec![
                0,
                1,
                2,
                3,
                6,
                9,
                (rows / 2) & !1,
                (rows / 2) | 1,
                rows.saturating_sub(22),
                rows.saturating_sub(3),
            ];
            firsts.retain(|&f| f <= rows);
            firsts.sort_unstable();
            firsts.dedup();
            let mut out = Vec::new();
            for f in firsts {
                let mut chroma_firsts = vec![f / 2, f / 2 + 1, (f / 2).saturating_sub(1)];
                chroma_firsts.retain(|&cf| cf <= rows / 2);
                chroma_firsts.dedup();
                for cf in chroma_firsts {
                    let (n, cn) = (rows - f, rows / 2 - cf);
                    let counts = [[n, cn], [24.min(n), 12.min(cn)], [24, 11], [n + 2, cn + 1]];
                    for (i, count) in counts.into_iter().enumerate() {
                        for extra in [[0, 0, 0], [s - 1, cs - 1, 1]] {
                            let slack = (i + out.len()) % 2 == 1;
                            let origin = [
                                if f == 0 && slack { 2 * s + 3 } else { 0 },
                                if cf == 0 && slack { cs + 1 } else { 0 },
                            ];
                            out.push(Band::new(s, rows, [f, cf], origin, count, extra));
                        }
                    }
                }
            }
            out
        }

        fn planes(&self) -> Planes<'_> {
            Planes::band(
                &self.y,
                &self.cb,
                &self.cr,
                self.stride,
                self.rows,
                self.first,
                self.origin,
            )
            .expect("band")
        }

        /// The luma and chroma rows (as luma rows) at which the rows held
        /// start and end, where a band's interior has its edges.
        fn edges(&self) -> [i64; 4] {
            let (s, cs) = (self.stride, self.stride / 2);
            let n = (self.y.len() - self.origin[0]) / s;
            let cn = (self.cb.len().min(self.cr.len()) - self.origin[1]) / cs;
            let [f, cf] = self.first;
            [f, f + n, 2 * cf, 2 * (cf + cn)].map(|e| e as i64)
        }
    }

    #[test]
    fn place_with_a_band_interior_is_place_with_the_window_test_near_its_edges() {
        // Every target row of the picture and a few past it, every
        // fraction, positions of both parities; a quarter of a few targets
        // across, of each kind; every band layout; the sizes, pictures and
        // operations in turn.
        let mut tally = Tally::default();
        let mut k = 0usize;
        for (stride, rows) in [(20, 40), (22, 48), (24, 30), (40, 64), (26, 22)] {
            let n = rows as i64;
            let mut down = Vec::new();
            for p in [0, 1] {
                for t in -5..=n + 5 {
                    for f in 0..8 {
                        let d = t - p - (f >> 2);
                        if d % 2 == 0 {
                            down.push((p as usize, i16::try_from(4 * d + f).expect("vector")));
                        }
                    }
                }
            }
            let across = axis(stride, false);
            let mut pictures = Picture::targets(stride, rows);
            for band in Band::layouts(stride, rows) {
                let src = band.planes();
                for (j, &(py, my)) in down.iter().enumerate() {
                    for (i, &(px, mx)) in across.iter().enumerate() {
                        if (i + j) % 4 != 0 {
                            continue;
                        }
                        k += 1;
                        let (w, h) = SIZES[k % SIZES.len()];
                        // Mostly the whole picture, which takes most
                        // blocks, so that the reference decides.
                        let picture = if k % 4 == 0 {
                            k / 4 % pictures.len()
                        } else {
                            0
                        };
                        check(
                            &mut tally,
                            &mut pictures[picture],
                            &src,
                            px,
                            py,
                            w,
                            h,
                            [mx, my],
                            ops(k / 9),
                        );
                    }
                }
            }
        }
        assert!(tally.inside > 100_000, "{} inside", tally.inside);
        assert!(tally.tested > 100_000, "{} tested", tally.tested);
        assert!(tally.declined > 100_000, "{} declined", tally.declined);
    }

    #[test]
    fn place_with_a_band_interior_is_place_with_the_window_test_on_random_blocks() {
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        let mut tally = Tally::default();
        for _ in 0..10_000 {
            let stride = 2 * (1 + rng.below(40) as usize);
            let rows = 2 * (1 + rng.below(40) as usize);
            let cs = stride / 2;
            // A first row near the top in half the bands, anywhere in the
            // rest.
            let top = if rng.below(2) == 0 { rows.min(8) } else { rows };
            let f = rng.below(top as u64 + 1) as usize;
            let cf = match rng.below(4) {
                0 => f / 2,
                1 => (f / 2 + 1).min(rows / 2),
                2 => (f / 2).saturating_sub(1),
                _ => rng.below(rows as u64 / 2 + 1) as usize,
            };
            // The rows to the picture's end and up to two more in three
            // bands of four, or fewer.
            let mut count = |n: usize| {
                if rng.below(4) != 0 {
                    n + rng.below(3) as usize
                } else {
                    rng.below(n as u64 + 1) as usize
                }
            };
            let count = [count(rows - f), count(rows / 2 - cf)];
            let mut extra = |n: usize| {
                if rng.below(2) == 0 {
                    0
                } else {
                    rng.below(n as u64) as usize
                }
            };
            let extra = [extra(stride), extra(cs), extra(cs)];
            let origin = [
                if f == 0 {
                    rng.below(3) as usize * stride + rng.below(4) as usize
                } else {
                    0
                },
                if cf == 0 {
                    rng.below(2) as usize * cs + rng.below(3) as usize
                } else {
                    0
                },
            ];
            let band = Band::new(stride, rows, [f, cf], origin, count, extra);
            let src = band.planes();
            let edges = band.edges();
            let mut pictures = Picture::targets(stride, rows);
            for _ in 0..100 {
                let mode = rng.below(4);
                let (px, mx) = rng.component(stride, mode.min(2));
                let (py, my) = if mode == 3 {
                    // A target within 23 rows of an edge of the rows held.
                    let edge = edges[rng.below(4) as usize];
                    let p = rng.below(rows as u64 + 8) as i64;
                    let t = edge + rng.below(47) as i64 - 23;
                    rng.aim(p, t)
                } else {
                    rng.component(rows, mode)
                };
                let (w, h) = SIZES[rng.below(9) as usize];
                let picture = if rng.below(2) == 0 {
                    0
                } else {
                    rng.below(pictures.len() as u64) as usize
                };
                let ops = ops(rng.below(16) as usize);
                check(
                    &mut tally,
                    &mut pictures[picture],
                    &src,
                    px,
                    py,
                    w,
                    h,
                    [mx, my],
                    ops,
                );
            }
        }
        assert!(tally.inside > 5_000, "{} inside", tally.inside);
        assert!(tally.tested > 5_000, "{} tested", tally.tested);
        assert!(tally.declined > 10_000, "{} declined", tally.declined);
    }

    #[test]
    fn every_window_of_a_block_inside_a_band_interior_fits() {
        let mut inside = 0;
        for (stride, rows) in [(22, 48), (24, 40), (26, 64), (40, 30)] {
            for band in Band::layouts(stride, rows) {
                let src = band.planes();
                // Every row; across, the interior's first and last columns
                // and those either side (its columns are a whole picture's,
                // which the test above takes all of).
                let across = [1, 2, stride as i64 - 19, stride as i64 - 18];
                for yi in -4..rows as i64 + 4 {
                    for xi in across {
                        if !src.interior.holds(xi, yi) {
                            continue;
                        }
                        inside += 1;
                        for (w, h) in SIZES {
                            for fx in 0..8 {
                                for fy in 0..8 {
                                    for (px, mx) in reaching(xi, fx) {
                                        for (py, my) in reaching(yi, fy) {
                                            let windows = windows(&src, px, py, w, h, [mx, my]);
                                            assert_eq!(windows.at, (xi, yi));
                                            assert!(
                                                windows.fit(&src),
                                                "{w}x{h} at ({px}, {py}), [{mx}, {my}], \
                                                 band {stride}x{rows} first {:?} at {:?} \
                                                 in {} {} {}",
                                                band.first,
                                                band.origin,
                                                band.y.len(),
                                                band.cb.len(),
                                                band.cr.len(),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(inside > 1_000, "{inside} inside");
    }

    #[test]
    fn a_band_interior_ends_where_a_window_first_leaves_the_rows_held() {
        // Of bands without part of a row after their last, so that each
        // bound is the last position at which every window fits: one past
        // each edge of a non-empty interior, some window does not.
        let mut edges = 0;
        for (stride, rows) in [(22, 48), (24, 40), (26, 64), (40, 30)] {
            for band in Band::layouts(stride, rows) {
                if band.y.len() % stride != 0
                    || band.cb.len().min(band.cr.len()) % (stride / 2) != 0
                {
                    continue;
                }
                let src = band.planes();
                let Interior {
                    x_last,
                    y_first,
                    y_last,
                } = src.interior;
                if x_last < 2 || y_last < y_first {
                    continue;
                }
                let some_window_fails = |xi: i64, yi: i64| {
                    SIZES.iter().any(|&(w, h)| {
                        (0..64).any(|f| {
                            reaching(xi, f % 8).iter().any(|&(px, mx)| {
                                reaching(yi, f / 8).iter().any(|&(py, my)| {
                                    !windows(&src, px, py, w, h, [mx, my]).fit(&src)
                                })
                            })
                        })
                    })
                };
                for (xi, yi) in [
                    (2, y_first - 1),
                    (x_last, y_first - 1),
                    (2, y_last + 1),
                    (x_last, y_last + 1),
                    (1, y_first),
                    (x_last + 1, y_last),
                ] {
                    edges += 1;
                    assert!(
                        some_window_fails(xi, yi),
                        "({xi}, {yi}) band {stride}x{rows} first {:?} at {:?} in {} {} {}",
                        band.first,
                        band.origin,
                        band.y.len(),
                        band.cb.len(),
                        band.cr.len(),
                    );
                }
            }
        }
        assert!(edges > 100, "{edges} edges");
    }

    #[test]
    fn requests_anywhere_change_nothing() {
        let far = [
            i64::MIN,
            -1_000_000,
            -33,
            -1,
            0,
            1,
            17,
            63,
            64,
            1_000_000,
            i64::MAX,
        ];
        let mut checked = 0;
        for (stride, rows) in [(32, 32), (48, 64), (64, 48)] {
            let references = Reference::layouts(stride, rows);
            let bands = Band::layouts(stride, rows);
            let before: Vec<_> = references.iter().map(|r| r.y.clone()).collect();
            let all = references
                .iter()
                .map(Reference::planes)
                .chain(bands.iter().map(Band::planes));
            for planes in all {
                let near = (-3..=stride as i64 + 3).step_by(5);
                for x in near.clone().chain(far) {
                    for y in near.clone().chain(far) {
                        for h in [0, 1, 4, 16, 200, usize::MAX] {
                            planes.request(x, y, h);
                            checked += 1;
                        }
                    }
                }
            }
            for (r, b) in references.iter().zip(&before) {
                assert_eq!(&r.y, b);
            }
        }
        assert!(checked > 10_000, "{checked} requests");
    }
}
