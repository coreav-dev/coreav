//! Decoded pictures: the sample planes, stored in horizontal bands, and the
//! cropping rectangle.
//!
//! A picture is shared between the thread that decodes it and the threads
//! that decode the pictures predicted from it (`docs/design/h264.md`,
//! "Threading (step 7): our design"). Its samples and motion are kept in
//! bands of whole macroblock rows, each its own allocation; the writer owns
//! the bands it has not published ([`Canvas`]) and moves each into the
//! shared [`Picture`] once nothing will write it again, after which any
//! thread may read it. With one band and no aprons this is the plane of a
//! single-threaded decoder.
//!
//! The bands at the picture's edges also keep slack rows, [`LUMA_SLACK`]
//! luma and [`CHROMA_SLACK`] chroma rows of the picture's stride that
//! nothing writes or shows: before the planes of a band whose stored rows
//! start at row 0, and after the last band's ([`Geometry::luma_slack`]).
//! Motion compensation takes a window at the picture's top or bottom edge
//! where it lies in the buffer when the samples its fraction uses are
//! inside the picture (`coreav_kernels_h264::Kernels::predict`), and the
//! slack is what keeps such a window inside the band it is read from. A
//! picture of one band keeps both; the other bands keep none, so their
//! rows and aprons are as threading cuts them. Sample (x, row) of
//! a plane of band `k` sits at [`Geometry::luma_at`] (or
//! [`Geometry::chroma_at`]) plus x: the plane's origin, past the slack
//! rows before it, plus its rows from the band's first stored row on, a
//! stride apart.
#![forbid(unsafe_code)]

use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};

use crate::error::{Error, LimitKind, Result};
use crate::limits::Limits;

/// Luma rows above a macroblock's top edge that its deblocking writes.
const FILTERED_LUMA: usize = 3;
/// Chroma rows above the edge that its deblocking writes.
const FILTERED_CHROMA: usize = 1;

/// Luma rows above its own that a band keeps (its top apron). A reader
/// takes a row from the band that owns the row [`FILTERED_LUMA`] rows
/// below it, so that the rows the next band's filter changes are read
/// from that band's apron, and a luma motion-compensation window, at most
/// 21 rows, is read from the band holding its last row: 20 rows above the
/// first row a band is read for, and the 3 its filter changes.
pub(crate) const TOP_LUMA: usize = 20 + FILTERED_LUMA;
/// Chroma rows above its own that a band keeps. Motion compensation reads
/// a block's chroma from the band its luma window is read from, which the
/// luma window's last row chooses: that row may be one of the
/// [`FILTERED_LUMA`] rows above the band, so the block's integer luma row
/// is at most 21 rows above the band's first ([`TOP_LUMA`] less the two
/// rows a luma window starts above it), and its chroma window starts at
/// most 11 chroma rows above the band's first. That also holds a chroma
/// window of 9 rows whose last row is the one the filter changes above
/// the edge.
pub(crate) const TOP_CHROMA: usize = (TOP_LUMA - 2).div_ceil(2);
/// Of the top apron, the luma rows the writer reads before the band's
/// first row is filtered: intra prediction reads the row above. The
/// deblocking filter, which reads four rows across the edge, runs after
/// [`Canvas::refresh_top`] has copied the whole apron.
const ENTRY_LUMA: usize = 1;
/// Chroma rows the writer reads above a band before its first row is
/// filtered: intra prediction reads one.
const ENTRY_CHROMA: usize = 1;

/// The luma and chroma rows of band `k`'s top apron that
/// [`Canvas::refresh_top`] writes first, the entry copy having left them:
/// from the band's first stored row on, in its buffers. None for the
/// first band, which has no apron.
pub(crate) fn rows_refreshed_first(g: &Geometry, k: usize) -> [usize; 2] {
    if k == 0 {
        return [0, 0];
    }
    let (l, c) = (g.luma(k), g.chroma(k));
    [
        (l.start - l.first).saturating_sub(ENTRY_LUMA),
        (c.start - c.first).saturating_sub(ENTRY_CHROMA),
    ]
}

/// Luma rows the bands at a picture's edges keep before its luma plane
/// and after it ([`Geometry::luma_slack`]). A luma window, `w + 5` by `h + 5` from two
/// samples left of and two rows above the block's integer position, is
/// taken as it lies when the samples its fraction uses are inside the
/// picture; with yFracL 0 its block rows are, so it starts at most two
/// rows and two columns before the picture's first sample, and ends at
/// most three rows and three columns after its last: two rows and two
/// samples before, three rows and three samples after, which four rows
/// each way hold.
pub(crate) const LUMA_SLACK: usize = 4;
/// Chroma rows kept before and after each chroma plane: a chroma window,
/// `w / 2 + 1` by `h / 2 + 1` from the block's integer position, ends at
/// most one row and one sample past the plane's last sample when its last
/// row and column are unused, which two rows hold; and as many before.
pub(crate) const CHROMA_SLACK: usize = 2;

/// The rows one band stores of one plane, in rows of that plane (or in
/// macroblock rows, for the motion).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    /// The first row stored: the first of the top apron.
    pub(crate) first: usize,
    /// The band's own rows, `start..end`, the last it stores.
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Span {
    /// Rows stored.
    pub(crate) const fn rows(&self) -> usize {
        self.end.saturating_sub(self.first)
    }
}

/// How a picture is cut into bands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub(crate) mb_width: usize,
    pub(crate) mb_height: usize,
    /// Macroblock rows per band; `mb_height` for a single band.
    band_mbs: usize,
}

impl Geometry {
    /// `mb_width` by `mb_height` macroblocks in bands of `band_mbs`
    /// macroblock rows (the last band may be shorter).
    ///
    /// A band's top apron may reach into several bands above it.
    pub(crate) fn new(mb_width: usize, mb_height: usize, band_mbs: usize) -> Geometry {
        let whole = mb_height.max(1);
        Geometry {
            mb_width,
            mb_height,
            band_mbs: band_mbs.clamp(1, whole),
        }
    }

    /// Macroblock rows per band.
    pub(crate) const fn band_mbs(&self) -> usize {
        self.band_mbs
    }

    /// The number of bands.
    pub(crate) const fn bands(&self) -> usize {
        self.mb_height.div_ceil(self.band_mbs)
    }

    /// The slack rows band `k` keeps before its luma plane and after it:
    /// [`LUMA_SLACK`] before where its stored rows start at the picture's
    /// first row, and after where it is the last band; none elsewhere. A
    /// picture of one band keeps both, as its references are read whole;
    /// in one of more, the bands holding row 0 and the last band keep
    /// theirs, so that the windows at the picture's top and bottom edges
    /// lie in the band they are read from as they do in a whole picture
    /// (`docs/design/h264.md`, "Threading round: slack rows at the
    /// picture's edges in every layout").
    #[inline]
    pub(crate) const fn luma_slack(&self, k: usize) -> [usize; 2] {
        [
            if self.luma_first(k) == 0 {
                LUMA_SLACK
            } else {
                0
            },
            if k + 1 >= self.bands() { LUMA_SLACK } else { 0 },
        ]
    }

    /// The slack rows band `k` keeps before each chroma plane and after
    /// it, as [`Geometry::luma_slack`]: [`CHROMA_SLACK`] each.
    #[inline]
    pub(crate) const fn chroma_slack(&self, k: usize) -> [usize; 2] {
        [
            if self.chroma_first(k) == 0 {
                CHROMA_SLACK
            } else {
                0
            },
            if k + 1 >= self.bands() {
                CHROMA_SLACK
            } else {
                0
            },
        ]
    }

    /// Where band `k`'s luma buffer holds sample (0, first stored row):
    /// past the slack rows before it. The stride is the coded width
    /// either way.
    ///
    /// Only the slack before, without [`Geometry::bands`]: the deblocking
    /// filter asks for it every macroblock row, and the division there
    /// cannot be dropped.
    #[inline]
    pub(crate) const fn luma_origin(&self, k: usize) -> usize {
        if self.luma_first(k) == 0 {
            LUMA_SLACK * self.mb_width * 16
        } else {
            0
        }
    }

    /// Where band `k`'s chroma buffers hold sample (0, first stored row).
    #[inline]
    pub(crate) const fn chroma_origin(&self, k: usize) -> usize {
        if self.chroma_first(k) == 0 {
            CHROMA_SLACK * self.mb_width * 8
        } else {
            0
        }
    }

    /// Where sample (0, `row`) of the luma plane sits in band `k`'s
    /// buffer, if the band stores the row's position (its rows start at
    /// [`Geometry::luma_first`]).
    #[inline]
    pub(crate) fn luma_at(&self, k: usize, row: usize) -> Option<usize> {
        row.checked_sub(self.luma_first(k))?
            .checked_mul(self.mb_width * 16)?
            .checked_add(self.luma_origin(k))
    }

    /// Where sample (0, `row`) of a chroma plane sits in band `k`'s buffer.
    #[inline]
    pub(crate) fn chroma_at(&self, k: usize, row: usize) -> Option<usize> {
        row.checked_sub(self.chroma_first(k))?
            .checked_mul(self.mb_width * 8)?
            .checked_add(self.chroma_origin(k))
    }

    /// The samples band `k` stores of the luma plane and of each chroma
    /// plane: its rows ([`Geometry::luma`], [`Geometry::chroma`]) and the
    /// slack rows before and after them, a stride each.
    fn plane_samples(&self, k: usize) -> (usize, usize) {
        let width = self.mb_width * 16;
        let [before, after] = self.luma_slack(k);
        let luma = (self.luma(k).rows() + before + after) * width;
        let [before, after] = self.chroma_slack(k);
        let chroma = (self.chroma(k).rows() + before + after) * (width / 2);
        (luma, chroma)
    }

    /// The band that owns macroblock row `row`. Motion compensation asks
    /// this of every block it predicts, so the division is avoided where
    /// it can be: there is one band, or bands of a power of two rows.
    #[inline]
    pub(crate) const fn band_of_mb_row(&self, row: usize) -> usize {
        if self.band_mbs >= self.mb_height {
            0
        } else if self.band_mbs.is_power_of_two() {
            row >> self.band_mbs.trailing_zeros()
        } else {
            row / self.band_mbs
        }
    }

    /// The band that owns luma row `row`.
    #[inline]
    pub(crate) const fn band_of_luma_row(&self, row: usize) -> usize {
        self.band_of_mb_row(row / 16)
    }

    /// The band that owns chroma row `row`.
    #[inline]
    pub(crate) const fn band_of_chroma_row(&self, row: usize) -> usize {
        self.band_of_mb_row(row / 8)
    }

    /// The band a reader takes luma row `row` from, and a motion
    /// compensation window whose last row is `row`: the band that owns
    /// the row [`FILTERED_LUMA`] rows below it, whose top apron holds the
    /// rows above (rule 2). The last band holds the rows below it.
    #[inline]
    pub(crate) const fn band_reading_luma_row(&self, row: usize) -> usize {
        let mb_row = (row + FILTERED_LUMA) / 16;
        let last = self.mb_height.saturating_sub(1);
        self.band_of_mb_row(if mb_row < last { mb_row } else { last })
    }

    /// The band a block's luma window is read from, the window `h + 5`
    /// rows from two above the block's integer row `yi` in the reference:
    /// the band a reader takes its last row from, clamped into the picture
    /// ([`Geometry::band_reading_luma_row`]), whose top apron holds the
    /// rest. Motion compensation reads every plane of a block it predicts
    /// in one call from this band (`docs/design/h264.md`, "Round 17").
    #[inline]
    pub(crate) fn band_reading_luma_window(&self, yi: isize, h: usize) -> usize {
        let bottom = (self.mb_height * 16).saturating_sub(1) as isize;
        let last = yi.saturating_add(h as isize + 2).min(bottom).max(0);
        self.band_reading_luma_row(last as usize)
    }

    /// [`Geometry::band_reading_luma_window`] of a picture `mb_height`
    /// macroblock rows high cut into bands of `1 << shift` rows, fewer than
    /// `mb_height`: the same rule, with no test of how the picture is cut.
    #[inline(always)]
    pub(crate) fn band_reading_luma_window_shift(
        mb_height: usize,
        shift: u8,
        yi: isize,
        h: usize,
    ) -> usize {
        let bottom = (mb_height * 16).saturating_sub(1) as isize;
        let last = yi.saturating_add(h as isize + 2).clamp(0, bottom) as usize;
        ((last + FILTERED_LUMA) / 16).min(mb_height.saturating_sub(1)) >> shift
    }

    /// The band a reader takes chroma row `row` from (as
    /// [`Geometry::band_reading_luma_row`]).
    #[inline]
    pub(crate) const fn band_reading_chroma_row(&self, row: usize) -> usize {
        let mb_row = (row + FILTERED_CHROMA) / 8;
        let last = self.mb_height.saturating_sub(1);
        self.band_of_mb_row(if mb_row < last { mb_row } else { last })
    }

    /// The first luma row band `k` stores: [`Geometry::luma`]'s `first`,
    /// which motion compensation asks for every block it predicts.
    #[inline]
    pub(crate) const fn luma_first(&self, k: usize) -> usize {
        if k == 0 {
            0
        } else {
            (k * self.band_mbs * 16).saturating_sub(TOP_LUMA)
        }
    }

    /// The first chroma row band `k` stores.
    #[inline]
    pub(crate) const fn chroma_first(&self, k: usize) -> usize {
        if k == 0 {
            0
        } else {
            (k * self.band_mbs * 8).saturating_sub(TOP_CHROMA)
        }
    }

    /// The first macroblock row of motion band `k` stores.
    #[inline]
    pub(crate) const fn motion_first(&self, k: usize) -> usize {
        if k == 0 {
            0
        } else {
            (k * self.band_mbs).saturating_sub(1)
        }
    }

    /// The rows of band `k` in a plane of `unit` rows per macroblock row,
    /// with `top` rows of apron above (none for the first band).
    fn span(&self, k: usize, unit: usize, top: usize) -> Span {
        let start_mb = k.saturating_mul(self.band_mbs).min(self.mb_height);
        let end_mb = (start_mb + self.band_mbs).min(self.mb_height);
        let (start, end) = (start_mb * unit, end_mb * unit);
        Span {
            first: if k > 0 {
                start.saturating_sub(top)
            } else {
                start
            },
            start,
            end,
        }
    }

    /// The luma rows band `k` stores.
    pub(crate) fn luma(&self, k: usize) -> Span {
        self.span(k, 16, TOP_LUMA)
    }

    /// The chroma rows band `k` stores.
    pub(crate) fn chroma(&self, k: usize) -> Span {
        self.span(k, 8, TOP_CHROMA)
    }

    /// The macroblock rows of motion band `k` stores: its own, and the one
    /// above, whose motion vector prediction and deblocking read it.
    pub(crate) fn motion(&self, k: usize) -> Span {
        self.span(k, 1, 1)
    }

    /// Bytes a picture of this geometry stores: every band's samples and
    /// motion, its aprons and slack rows included ([`BandData::fit`]).
    pub(crate) fn stored_bytes(&self) -> u64 {
        let motion_row = (self.mb_width as u64).saturating_mul(MOTION_BYTES_PER_MB as u64);
        (0..self.bands())
            .map(|k| {
                let (luma, chroma) = self.plane_samples(k);
                let chroma = (chroma as u64).saturating_mul(2);
                let motion = (self.motion(k).rows() as u64).saturating_mul(motion_row);
                (luma as u64).saturating_add(chroma).saturating_add(motion)
            })
            .fold(0u64, u64::saturating_add)
    }

    /// The band height a decoder with `threads` threads uses for a picture
    /// `mb_height` macroblock rows high: one band with one thread, else
    /// about a sixteenth of the picture, rounded down to a power of two
    /// macroblock rows and kept between 1 and 8, except that a picture of
    /// 32 to 63 rows takes bands of 4 rather than 2 (`docs/design/h264.md`,
    /// rule 4).
    pub(crate) fn band_mbs_for(mb_height: usize, threads: usize) -> usize {
        if threads <= 1 {
            return mb_height.max(1);
        }
        let sixteenth = mb_height / 16;
        let power = if sixteenth == 0 {
            1
        } else {
            1usize << sixteenth.ilog2()
        };
        match power.clamp(1, 8) {
            // Bands of 2 rows copy 23 rows of apron for 32 of their own;
            // bands of 4 were faster at eight threads on these pictures
            // (`docs/measurements/h264-rounds/band-height/`).
            2 => 4,
            rows => rows,
        }
    }
}

/// One plane of a decoded picture, cropped to what the stream displays.
#[derive(Clone, Copy, Debug)]
pub struct PlaneRef<'a> {
    picture: &'a Picture,
    plane: Plane,
    /// The first column and row shown.
    left: usize,
    top: usize,
    stride: usize,
    width: usize,
    height: usize,
}

/// Which plane a [`PlaneRef`] shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plane {
    Y,
    Cb,
    Cr,
}

impl<'a> PlaneRef<'a> {
    /// Samples per row.
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Rows.
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Samples from one row to the next in the decoder's storage, which is
    /// at least the width: the decoder keeps whole macroblocks.
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// The rows, each exactly `width` samples.
    pub fn rows(&self) -> impl Iterator<Item = &'a [u8]> + Clone {
        let (picture, plane, left, width) = (self.picture, self.plane, self.left, self.width);
        (self.top..self.top + self.height).map(move |row| {
            let samples = picture
                .row(plane, row)
                .and_then(|r| r.get(left..left + width));
            // A picture reaches a caller only once every band is
            // published, and the view lies inside the coded picture.
            debug_assert!(samples.is_some(), "row {row} of a {plane:?} plane");
            samples.unwrap_or_default()
        })
    }
}

/// The motion of one 4x4 luma block, kept with the picture it belongs to:
/// the next pictures' direct prediction reads it (clause 8.4.1.2) and the
/// deblocking filter derives boundary strengths from it (clause 8.7.2.1).
/// The kernels' type, which their strength derivation reads
/// ([`coreav_kernels_h264::Kernels::strengths`]): a reference index of -1
/// where a list is unused (and for both lists of an intra block), and a
/// slot ([`Picture::slot`]) for the picture each list refers to,
/// [`NO_SLOT`] where the list is unused.
pub(crate) use coreav_kernels_h264::Motion;

// Twelve bytes, so that a macroblock's sixteen are three cache lines
// (`docs/design/h264.md`, "Motion round: a block's motion in 12 bytes,
// on the current decoder").
const _: () = assert!(size_of::<Motion>() == 12);

/// How many slots ([`Picture::slot`]) the decoder hands out: 0 to 254.
pub(crate) const SLOTS: usize = 255;

/// [`Motion::ref_slot`] of a list a block does not use; never a picture's
/// slot.
pub(crate) const NO_SLOT: u8 = 255;

/// The bytes of motion a picture keeps for each of its macroblocks, one
/// record for each 4x4 luma block and its head ([`MbHead`]); with threads,
/// a band keeps the macroblock row above its own as well. A memory budget
/// counts it beside the samples.
pub const MOTION_BYTES_PER_MB: usize = 16 * size_of::<Motion>() + size_of::<MbHead>();

/// A block with no motion: intra, or not yet derived.
pub(crate) const NO_MOTION: Motion = Motion {
    mv: [[0; 2]; 2],
    ref_idx: [-1; 2],
    ref_slot: [NO_SLOT; 2],
};

/// The motion of one macroblock as its band keeps it: one motion that
/// stands for all sixteen blocks, or a mark that the blocks are stored one
/// by one (`docs/design/h264.md`, "MB layer round 2: the layout without
/// the added instructions").
///
/// A macroblock with one motion (a skipped one, one partition, direct
/// with equal quadrants) and an intra one ([`NO_MOTION`]) write the head
/// alone, 16 bytes where the blocks are 192; their sixteen entries in
/// [`BandData::motion`] are left as an earlier picture wrote them and
/// never read. Sixteen bytes and aligned to them, so that four heads share
/// a cache line and none straddles two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, align(16))]
pub(crate) struct MbHead {
    /// The macroblock's motion where `split` is false; where it is true,
    /// the motion of its first stored partition, which no reader takes.
    pub(crate) m: Motion,
    /// Whether the blocks carry more than one motion, each stored in
    /// [`BandData::motion`].
    pub(crate) split: bool,
}

const _: () = assert!(size_of::<MbHead>() == 16);

/// The head of a macroblock without motion: intra, or covered by no slice.
pub(crate) const NO_HEAD: MbHead = MbHead {
    m: NO_MOTION,
    split: false,
};

/// The motion of one macroblock, resolved from its head once: what the
/// readers of a neighbour's or a co-located macroblock's blocks look up.
#[derive(Clone, Copy, Debug)]
pub(crate) enum MbMotion<'a> {
    /// One motion for every block.
    One(&'a Motion),
    /// Each block its own, in raster order.
    Split(&'a [Motion; 16]),
}

impl<'a> MbMotion<'a> {
    /// The motion of raster block `i`.
    #[inline(always)]
    pub(crate) fn block(self, i: usize) -> &'a Motion {
        match self {
            MbMotion::One(m) => m,
            MbMotion::Split(blocks) => blocks.get(i & 15).unwrap_or(&NO_MOTION),
        }
    }
}

/// The samples and motion of one band: its own rows and its aprons
/// ([`Geometry::luma`], [`Geometry::chroma`], [`Geometry::motion`]), each
/// plane with the picture's stride and the slack rows the band keeps
/// before and after it; sample (x, row) is at
/// [`Geometry::luma_at`] (or [`Geometry::chroma_at`]) plus x.
#[derive(Debug, Default)]
pub(crate) struct BandData {
    pub(crate) y: Vec<u8>,
    pub(crate) cb: Vec<u8>,
    pub(crate) cr: Vec<u8>,
    /// The motion of every 4x4 luma block of the stored macroblock rows,
    /// indexed by `(mb_address - first stored address) * 16 + 4 * y + x`;
    /// read only for a macroblock whose head is split.
    pub(crate) motion: Vec<Motion>,
    /// The head of every stored macroblock, indexed by `mb_address - first
    /// stored address`: every macroblock a reader reaches has one written
    /// for the picture, by its decoding or by what fills a macroblock no
    /// slice covers.
    pub(crate) heads: Vec<MbHead>,
}

impl BandData {
    /// Sizes the buffers for band `k`, keeping what they hold: a picture is
    /// reused without being cleared (`docs/design/h264.md`, "Reusing a
    /// picture without clearing it"). A new plane is zeroed memory from the
    /// allocator, untouched until decoding writes it (the first-touch
    /// round); [`Picture::black`] fills its chroma with 128 itself.
    fn fit(&mut self, g: &Geometry, k: usize) {
        let (luma, chroma) = g.plane_samples(k);
        for (plane, len) in [
            (&mut self.y, luma),
            (&mut self.cb, chroma),
            (&mut self.cr, chroma),
        ] {
            if plane.is_empty() {
                *plane = vec![0; len];
            } else {
                plane.resize(len, 0);
            }
        }
        let mbs = g.motion(k).rows() * g.mb_width;
        self.motion.resize(mbs * 16, NO_MOTION);
        self.heads.resize(mbs, NO_HEAD);
    }

    /// The motion of the macroblock `i` places after the band's first
    /// stored one: one lookup of its head, and of its blocks only where it
    /// is split.
    #[inline(always)]
    pub(crate) fn mb_motion(&self, i: usize) -> Option<MbMotion<'_>> {
        let head = self.heads.get(i)?;
        if !head.split {
            return Some(MbMotion::One(&head.m));
        }
        let at = i.wrapping_mul(16);
        self.motion
            .get(at..at.wrapping_add(16))
            .and_then(|m| <&[Motion; 16]>::try_from(m).ok())
            .map(MbMotion::Split)
    }

    /// Fills one macroblock with the grey a decoder shows where a picture
    /// has no slice — luma 0, chroma 128, as [`Picture::black`] leaves a
    /// new picture — and clears its motion: the deblocking filter and a
    /// later picture's direct prediction read motion the stream never
    /// wrote.
    ///
    /// Macroblock (`mbx`, `mby`) is in this band, band `k` of `g`.
    ///
    /// A row outside the band is skipped rather than reported: the caller
    /// derives `mbx` and `mby` from the picture's own size, so no row of
    /// an in-range macroblock is out of range.
    pub(crate) fn clear_macroblock(&mut self, g: &Geometry, k: usize, mbx: usize, mby: usize) {
        for row in 0..16 {
            let Some(at) = g.luma_at(k, mby * 16 + row).map(|at| at + mbx * 16) else {
                continue;
            };
            if let Some(line) = self.y.get_mut(at..at + 16) {
                line.fill(0);
            }
        }
        for row in 0..8 {
            let Some(at) = g.chroma_at(k, mby * 8 + row).map(|at| at + mbx * 8) else {
                continue;
            };
            if let Some(line) = self.cb.get_mut(at..at + 8) {
                line.fill(128);
            }
            if let Some(line) = self.cr.get_mut(at..at + 8) {
                line.fill(128);
            }
        }
        let at = mby.saturating_sub(g.motion_first(k)) * g.mb_width + mbx;
        if let Some(head) = self.heads.get_mut(at) {
            *head = NO_HEAD;
        }
    }

    /// The buffer of one plane.
    fn plane(&self, plane: Plane) -> &[u8] {
        match plane {
            Plane::Y => &self.y,
            Plane::Cb => &self.cb,
            Plane::Cr => &self.cr,
        }
    }
}

/// A decoded picture: 8-bit 4:2:0 luma and chroma planes.
///
/// The planes hold whole macroblocks, with slack rows before and after
/// them at the picture's edges (`Geometry::luma_slack`); `width` and
/// `height` are the
/// cropped size the stream displays (clause 7.4.2.1.1), and [`Picture::luma`]
/// and [`Picture::chroma`] give views that already skip the cropped samples.
#[derive(Debug)]
pub struct Picture {
    geometry: Geometry,
    /// The bands, each set once its writer has published it.
    bands: Box<[OnceLock<BandData>]>,
    /// What a reader waiting for a band sleeps on.
    lock: Mutex<()>,
    published: Condvar,
    /// Coded size: a whole number of macroblocks.
    coded_width: usize,
    coded_height: usize,
    pub(crate) mb_width: usize,
    pub(crate) mb_height: usize,
    /// Cropped samples: left, right, top, bottom, in luma samples.
    crop: [usize; 4],
    poc: i32,
    /// The slot of this picture inside its decoder, 0 to 254, which no
    /// other picture of the decoded picture buffer holds while it is there,
    /// so that a block's [`Motion::ref_slot`] tells which picture it
    /// referred to (`Decoder::picture_slot`).
    pub(crate) slot: u8,
    /// What the caller attached to the NAL units this picture was decoded
    /// from ([`crate::Decoder::set_mark`]).
    pub(crate) mark: u64,
}

impl Picture {
    /// An all-black picture of `mb_width` by `mb_height` macroblocks, or
    /// [`LimitKind::PictureSize`] if that is more than `limits` allows.
    pub fn black(mb_width: usize, mb_height: usize, limits: &Limits) -> Result<Picture> {
        let geometry = Geometry::new(mb_width, mb_height, mb_height);
        let picture = Picture::new(geometry, limits)?;
        let mut data = BandData::default();
        data.fit(&geometry, 0);
        data.cb.fill(128);
        data.cr.fill(128);
        if let Some(cell) = picture.bands.first() {
            // A new cell: it cannot be set already.
            let _ = cell.set(data);
        }
        Ok(picture)
    }

    /// A picture of `geometry` whose bands are not published yet, or
    /// [`LimitKind::PictureSize`] if it is larger than `limits` allows.
    pub(crate) fn new(geometry: Geometry, limits: &Limits) -> Result<Picture> {
        let (mb_width, mb_height) = (geometry.mb_width, geometry.mb_height);
        let mbs = mb_width
            .checked_mul(mb_height)
            .filter(|&mbs| mbs > 0 && mbs <= limits.max_mbs)
            .ok_or(Error::Limit(LimitKind::PictureSize))?;
        let (coded_width, coded_height) = (
            mb_width
                .checked_mul(16)
                .ok_or(Error::Limit(LimitKind::PictureSize))?,
            mb_height
                .checked_mul(16)
                .ok_or(Error::Limit(LimitKind::PictureSize))?,
        );
        mbs.checked_mul(256)
            .ok_or(Error::Limit(LimitKind::PictureSize))?;
        Ok(Picture {
            geometry,
            bands: (0..geometry.bands()).map(|_| OnceLock::new()).collect(),
            lock: Mutex::new(()),
            published: Condvar::new(),
            coded_width,
            coded_height,
            mb_width,
            mb_height,
            crop: [0; 4],
            poc: 0,
            slot: 0,
            mark: 0,
        })
    }

    /// The picture order count, cropping rectangle, slot and mark the
    /// decoder derived for this picture, set before it is shared: a shared
    /// picture's metadata does not change.
    pub(crate) fn with_output(
        mut self,
        poc: i32,
        crop: [usize; 4],
        slot: u8,
        mark: u64,
    ) -> Picture {
        self.poc = poc;
        self.crop = crop;
        self.slot = slot;
        self.mark = mark;
        self
    }

    /// How the picture is cut into bands.
    pub(crate) const fn geometry(&self) -> &Geometry {
        &self.geometry
    }

    /// Band `k`, waiting until its writer has published it.
    ///
    /// The wait ends: a band is published once its writer is done with
    /// it, and a writer that stops early publishes what it has
    /// ([`Canvas`]'s `Drop`).
    #[inline]
    pub(crate) fn band(&self, k: usize) -> Option<&BandData> {
        let cell = self.bands.get(k)?;
        match cell.get() {
            Some(data) => Some(data),
            None => self.wait_for(cell),
        }
    }

    /// Waits until `cell` is set.
    #[cold]
    fn wait_for<'a>(&'a self, cell: &'a OnceLock<BandData>) -> Option<&'a BandData> {
        let mut guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(data) = cell.get() {
                return Some(data);
            }
            guard = self
                .published
                .wait(guard)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The picture's one band, if it is cut into one and it is published:
    /// every picture of a single-threaded decoder once it is decoded.
    pub(crate) fn whole(&self) -> Option<&BandData> {
        match &*self.bands {
            [only] => only.get(),
            _ => None,
        }
    }

    /// Band `k` if it is published, without waiting.
    fn published_band(&self, k: usize) -> Option<&BandData> {
        self.bands.get(k)?.get()
    }

    /// Moves band `k` into the picture and wakes the readers waiting for it.
    fn publish(&self, k: usize, data: BandData) {
        if let Some(cell) = self.bands.get(k) {
            // A band is published once ([`Canvas::publish_final`] counts
            // them), so the cell is empty.
            let _ = cell.set(data);
        }
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.published.notify_all();
    }

    /// The band buffers, for a picture nothing else holds any more.
    pub(crate) fn into_bands(self) -> Vec<BandData> {
        self.bands
            .into_vec()
            .into_iter()
            .map(|cell| cell.into_inner().unwrap_or_default())
            .collect()
    }

    /// One row of a plane, `stride` samples, if its band is published.
    fn row(&self, plane: Plane, row: usize) -> Option<&[u8]> {
        self.row_of(plane, row, false)
    }

    /// One row of a plane, `stride` samples; with `wait`, once its band is
    /// published.
    fn row_of(&self, plane: Plane, row: usize, wait: bool) -> Option<&[u8]> {
        let g = &self.geometry;
        let (k, stride) = match plane {
            Plane::Y => (g.band_reading_luma_row(row), self.coded_width),
            Plane::Cb | Plane::Cr => (g.band_reading_chroma_row(row), self.coded_width / 2),
        };
        let at = if plane == Plane::Y {
            g.luma_at(k, row)
        } else {
            g.chroma_at(k, row)
        }?;
        let band = if wait {
            self.band(k)
        } else {
            self.published_band(k)
        };
        band?.plane(plane).get(at..at.checked_add(stride)?)
    }

    /// The coded width in luma samples: `mb_width * 16`, which is also the
    /// stride of the luma plane.
    pub(crate) const fn coded_width(&self) -> usize {
        self.coded_width
    }

    /// The coded height in luma samples.
    pub(crate) const fn coded_height(&self) -> usize {
        self.coded_height
    }

    /// Displayed width in luma samples.
    pub fn width(&self) -> usize {
        self.coded_width
            .saturating_sub(self.crop[0])
            .saturating_sub(self.crop[1])
    }

    /// Displayed height in luma samples.
    pub fn height(&self) -> usize {
        self.coded_height
            .saturating_sub(self.crop[2])
            .saturating_sub(self.crop[3])
    }

    /// The picture order count, which gives the output order (clause 8.2.1).
    ///
    /// In [`crate::Mode::KeyframesOnly`] it counts from the keyframe the
    /// decoder restarted at, not from the start of the stream: the
    /// pictures of that mode come in decode order, and
    /// [`Picture::mark`] is what identifies them.
    pub const fn poc(&self) -> i32 {
        self.poc
    }

    /// What the caller had attached when this picture's first slice
    /// arrived ([`crate::Decoder::set_mark`]).
    ///
    /// The decoder returns pictures in output order, which is not the
    /// order they were decoded in; a caller that has to tell which packet
    /// a picture came from — to give a frame its timestamp, say — marks
    /// the packets and reads the mark back here.
    pub const fn mark(&self) -> u64 {
        self.mark
    }

    /// The luma plane, cropped.
    pub fn luma(&self) -> PlaneRef<'_> {
        PlaneRef {
            picture: self,
            plane: Plane::Y,
            left: self.crop[0],
            top: self.crop[2],
            stride: self.coded_width,
            width: self.width(),
            height: self.height(),
        }
    }

    /// The Cb and Cr planes, cropped: half the width and half the height of
    /// the luma plane, rounded up.
    pub fn chroma(&self) -> (PlaneRef<'_>, PlaneRef<'_>) {
        let plane = |plane| PlaneRef {
            picture: self,
            plane,
            left: self.crop[0] / 2,
            top: self.crop[2] / 2,
            stride: self.coded_width / 2,
            width: self.width().div_ceil(2),
            height: self.height().div_ceil(2),
        };
        (plane(Plane::Cb), plane(Plane::Cr))
    }

    /// The motion of every 4x4 block, in macroblock order, for tests.
    #[cfg(test)]
    pub(crate) fn all_motion(&self) -> Vec<Motion> {
        let g = self.geometry;
        let mut out = Vec::new();
        for k in 0..g.bands() {
            let span = g.motion(k);
            let skip = (span.start - span.first) * g.mb_width * 16;
            if let Some(data) = self.published_band(k) {
                for i in skip / 16..data.heads.len() {
                    match data.mb_motion(i) {
                        Some(MbMotion::One(m)) => out.extend([*m; 16]),
                        Some(MbMotion::Split(blocks)) => out.extend(blocks.iter().copied()),
                        None => {}
                    }
                }
            }
        }
        out
    }
}

/// The rows of `rows` in runs of consecutive rows that `band` gives the
/// same band, with that band: at most a few, as a band's top apron lies
/// in the bands just above it.
fn runs(
    rows: Range<usize>,
    band: impl Fn(usize) -> usize,
) -> impl Iterator<Item = (usize, Range<usize>)> {
    let mut next = rows.start;
    std::iter::from_fn(move || {
        let start = next;
        if start >= rows.end {
            return None;
        }
        let j = band(start);
        next = (start + 1..rows.end)
            .find(|&row| band(row) != j)
            .unwrap_or(rows.end);
        Some((j, start..next))
    })
}

/// Copies `len` elements from `src` at `from` to `dst` at `to`.
fn copy<T: Copy>(src: &[T], from: usize, dst: &mut [T], to: usize, len: usize) -> Result<()> {
    let geometry = Error::Geometry("an apron outside its band");
    let src = src.get(from..from + len).ok_or(geometry)?;
    dst.get_mut(to..to + len)
        .ok_or(geometry)?
        .copy_from_slice(src);
    Ok(())
}

/// The writer's side of a picture being decoded: the bands it has not
/// published yet, which it alone may write.
///
/// Bands are published in order, each once its own rows are filtered
/// (`docs/design/h264.md`, rule 5). Dropping a canvas
/// publishes whatever it still holds, so that a reader waiting for a band
/// is never left waiting by a writer that stopped.
pub(crate) struct Canvas {
    picture: Arc<Picture>,
    bands: Vec<BandData>,
    /// Bands sized for the picture so far: the first `fitted`. A band is
    /// sized when the canvas is first asked for it, or for a band after it,
    /// so on the thread that decodes it rather than the one that made the
    /// canvas ([`Canvas::fit_through`]).
    fitted: usize,
    /// Bands published so far: the first `published`.
    published: usize,
    /// The last band decoding has entered, whose top apron holds the row
    /// above it as decoded.
    entered: Option<usize>,
    /// Filtered macroblock rows after which the next band to publish is
    /// final.
    next_ready: usize,
}

impl Canvas {
    /// The canvas of `picture`, none of whose bands is published, writing
    /// into the buffers `bands` (a reused picture's, or none), which it
    /// sizes as it is first asked for them.
    pub(crate) fn new(picture: Arc<Picture>, mut bands: Vec<BandData>) -> Canvas {
        let g = picture.geometry;
        bands.resize_with(g.bands(), BandData::default);
        Canvas {
            picture,
            bands,
            fitted: 0,
            published: 0,
            entered: None,
            next_ready: Self::ready_at(&g, 0),
        }
    }

    /// How the picture is cut into bands.
    pub(crate) fn geometry(&self) -> Geometry {
        self.picture.geometry
    }

    /// Sizes every band up to `k` that is not sized yet, in order, so that
    /// a band and every band above it are sized before any of them is
    /// written, copied from or published. A new picture's bands are then
    /// first touched by the thread that decodes them, a band at a time,
    /// rather than all at once by the thread that dispatches the picture
    /// (`docs/design/h264.md`, "Threading round: a new picture's first
    /// touch on the thread that decodes it").
    #[inline]
    fn fit_through(&mut self, k: usize) {
        if k >= self.fitted {
            self.fit_bands(k);
        }
    }

    /// [`Canvas::fit_through`] where a band is not sized yet.
    #[inline(never)]
    fn fit_bands(&mut self, k: usize) {
        let g = self.geometry();
        let end = k.saturating_add(1).min(self.bands.len());
        for (j, band) in self
            .bands
            .iter_mut()
            .enumerate()
            .take(end)
            .skip(self.fitted)
        {
            band.fit(&g, j);
        }
        self.fitted = self.fitted.max(end);
    }

    /// Band `k`, which must not be published yet.
    #[inline]
    pub(crate) fn band_mut(&mut self, k: usize) -> Result<&mut BandData> {
        self.fit_through(k);
        self.bands
            .get_mut(k)
            .ok_or(Error::Geometry("a band outside the picture"))
    }

    /// Takes band `k` out to be written, leaving it empty until
    /// [`Canvas::put_band`] returns it: the decoding of a band's rows
    /// writes it without going through the canvas.
    pub(crate) fn take_band(&mut self, k: usize) -> BandData {
        self.fit_through(k);
        self.bands
            .get_mut(k)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Returns band `k`, taken by [`Canvas::take_band`].
    pub(crate) fn put_band(&mut self, k: usize, data: BandData) {
        if let Some(band) = self.bands.get_mut(k) {
            *band = data;
        }
    }

    /// Decoding enters band `k`: its top apron takes the rows above it as
    /// decoded, unfiltered, with their motion (rule 2), which is what the
    /// band's first macroblock row predicts from. Only the first entry
    /// copies; macroblocks are decoded in increasing order, so the rows
    /// above change after this only by the filter, whose result the apron
    /// takes before the band's first row is filtered
    /// ([`Canvas::refresh_top`]).
    pub(crate) fn enter(&mut self, k: usize) -> Result<()> {
        if self.entered.is_some_and(|e| k <= e) {
            return Ok(());
        }
        self.entered = Some(k);
        self.copy_top(k, ENTRY_LUMA, ENTRY_CHROMA, true)
    }

    /// Before the first row of band `k` is filtered: its whole top apron
    /// takes the rows above as filtered, which are final except the rows
    /// this filter changes across the band's top edge. Those it changes in
    /// the apron, where readers take them from; the band above keeps them
    /// as they were, and is read only above them.
    pub(crate) fn refresh_top(&mut self, k: usize) -> Result<()> {
        self.copy_top(k, TOP_LUMA, TOP_CHROMA, false)
    }

    /// Copies the last `luma` luma rows and `chroma` chroma rows above band
    /// `k` into its top apron, and with `motion` the macroblock row of
    /// motion above it. A row the filter of band `k`'s first row changes
    /// comes from the band that owns it; any other from the band a reader
    /// takes it from, which holds it final. Either is published already
    /// or still the canvas's.
    fn copy_top(&mut self, k: usize, luma: usize, chroma: usize, motion: bool) -> Result<()> {
        let g = self.geometry();
        if k == 0 {
            return Ok(());
        }
        self.fit_through(k);
        let missing = Error::Geometry("an apron outside its band");
        let width = g.mb_width * 16;
        let half = width / 2;
        let published = self.published;
        let picture = &*self.picture;
        let (above, rest) = self
            .bands
            .split_at_mut_checked(k)
            .ok_or(Error::Geometry("a band outside the picture"))?;
        let own = rest
            .first_mut()
            .ok_or(Error::Geometry("a band outside the picture"))?;
        let source = |j: usize| {
            if j < published {
                picture.published_band(j)
            } else {
                above.get(j)
            }
            .ok_or(missing)
        };
        // Rows from one band lie one after another in its buffer and in
        // this one, a stride each, so each run of them is one copy.
        let l = g.luma(k);
        let rows = l.start - luma.min(l.start - l.first)..l.start;
        for (j, run) in runs(rows, |row| {
            if row + FILTERED_LUMA >= l.start {
                g.band_of_luma_row(row)
            } else {
                g.band_reading_luma_row(row)
            }
        }) {
            let from = source(j)?;
            let at = g.luma_at(j, run.start).ok_or(missing)?;
            let to = g.luma_at(k, run.start).ok_or(missing)?;
            copy(&from.y, at, &mut own.y, to, run.len() * width)?;
        }
        let c = g.chroma(k);
        let rows = c.start - chroma.min(c.start - c.first)..c.start;
        for (j, run) in runs(rows, |row| {
            if row + FILTERED_CHROMA >= c.start {
                g.band_of_chroma_row(row)
            } else {
                g.band_reading_chroma_row(row)
            }
        }) {
            let from = source(j)?;
            let at = g.chroma_at(j, run.start).ok_or(missing)?;
            let to = g.chroma_at(k, run.start).ok_or(missing)?;
            copy(&from.cb, at, &mut own.cb, to, run.len() * half)?;
            copy(&from.cr, at, &mut own.cr, to, run.len() * half)?;
        }
        if motion {
            let j = k - 1;
            let (ma, m) = (g.motion(j), g.motion(k));
            let from = source(j)?;
            let (at, rows) = (
                (m.first - ma.first) * g.mb_width,
                (m.start - m.first) * g.mb_width,
            );
            copy(&from.heads, at, &mut own.heads, 0, rows)?;
            copy(&from.motion, at * 16, &mut own.motion, 0, rows * 16)?;
        }
        Ok(())
    }

    /// Publishes every band that is final once the first `filtered`
    /// macroblock rows are deblocked: a band whose own rows are all
    /// filtered (rule 5). The filter of its next row changes its bottom
    /// three luma rows and its bottom chroma row, but a reader takes those
    /// from the band below's top apron; its top apron was refreshed before
    /// its first row was filtered.
    #[inline]
    pub(crate) fn publish_final(&mut self, filtered: usize) -> Result<()> {
        // The deblocking calls this after every row; most rows publish
        // nothing.
        if filtered < self.next_ready {
            return Ok(());
        }
        self.publish_ready(filtered);
        Ok(())
    }

    /// The number of filtered macroblock rows after which band `k` is
    /// final (the condition of [`Canvas::publish_final`]).
    fn ready_at(g: &Geometry, k: usize) -> usize {
        if k >= g.bands() {
            return usize::MAX;
        }
        g.motion(k).end
    }

    /// [`Canvas::publish_final`] once a band may be final.
    fn publish_ready(&mut self, filtered: usize) {
        let g = self.geometry();
        while self.published < self.bands.len() {
            let k = self.published;
            if g.motion(k).end > filtered {
                break;
            }
            self.fit_through(k);
            let data = self
                .bands
                .get_mut(k)
                .map(std::mem::take)
                .unwrap_or_default();
            self.picture.publish(k, data);
            self.published += 1;
            self.next_ready = Self::ready_at(&g, self.published);
        }
    }

    /// Publishes every band that is left: the picture is finished.
    pub(crate) fn publish_all(&mut self) -> Result<()> {
        self.publish_final(usize::MAX)
    }

    /// Fills the whole picture as [`Picture::black`] leaves one, motion
    /// included: a picture no slice covers (clause 8.2.5.2).
    pub(crate) fn fill_black(&mut self) {
        self.fit_through(usize::MAX);
        for band in &mut self.bands {
            band.y.fill(0);
            band.cb.fill(128);
            band.cr.fill(128);
            band.heads.fill(NO_HEAD);
        }
    }

    /// Copies the samples of `other`, band by band as it publishes them,
    /// and clears the motion; false if it is of another size, or a band of
    /// it could not be read.
    ///
    /// `other` may be cut into bands differently — a decoder whose thread
    /// count was lowered starts cutting its pictures differently — and is
    /// then copied row by row.
    pub(crate) fn conceal_from(&mut self, other: &Picture) -> bool {
        self.fit_through(usize::MAX);
        let g = self.geometry();
        if other.mb_width != g.mb_width || other.mb_height != g.mb_height {
            return false;
        }
        if other.geometry != g {
            // Row by row, from each plane's origin: the slack rows are
            // left as they are.
            let width = g.mb_width * 16;
            for (k, band) in self.bands.iter_mut().enumerate() {
                for (plane, span, stride, origin) in [
                    (Plane::Y, g.luma(k), width, g.luma_origin(k)),
                    (Plane::Cb, g.chroma(k), width / 2, g.chroma_origin(k)),
                    (Plane::Cr, g.chroma(k), width / 2, g.chroma_origin(k)),
                ] {
                    let dst = match plane {
                        Plane::Y => &mut band.y,
                        Plane::Cb => &mut band.cb,
                        Plane::Cr => &mut band.cr,
                    };
                    let rows = dst.get_mut(origin..).unwrap_or_default();
                    for (row, out) in (span.first..span.end).zip(rows.chunks_exact_mut(stride)) {
                        let Some(src) = other.row_of(plane, row, true) else {
                            return false;
                        };
                        out.copy_from_slice(src);
                    }
                }
                band.heads.fill(NO_HEAD);
            }
            return true;
        }
        for (k, band) in self.bands.iter_mut().enumerate() {
            let Some(from) = other.band(k) else {
                return false;
            };
            for (dst, src) in [
                (&mut band.y, &from.y),
                (&mut band.cb, &from.cb),
                (&mut band.cr, &from.cr),
            ] {
                if let Some(dst) = dst.get_mut(..src.len()) {
                    dst.copy_from_slice(src);
                }
            }
            band.heads.fill(NO_HEAD);
        }
        true
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // A writer that stopped before finishing — a worker whose decoder
        // was dropped — still publishes, so that no reader waits for ever.
        let _ = self.publish_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_cover_the_rows_in_order_each_from_its_band() {
        for (rows, step) in [(0..0, 1), (5..6, 1), (0..23, 16), (3..40, 7), (9..10, 1)] {
            let band = |row: usize| row / step;
            let mut next = rows.start;
            for (j, run) in runs(rows.clone(), band) {
                assert_eq!(run.start, next);
                assert!(!run.is_empty());
                assert!(run.clone().all(|row| band(row) == j));
                next = run.end;
            }
            assert_eq!(next, rows.end.max(rows.start));
        }
        // Consecutive runs come from different bands.
        let found: Vec<_> = runs(2..40, |row| (row + 3) / 16).collect();
        assert_eq!(found, [(0, 2..13), (1, 13..29), (2, 29..40)]);
    }

    /// The band a window is read from by the band height's logarithm is
    /// the one the reading rule gives.
    #[test]
    fn band_reading_by_shift_follows_the_reading_rule() {
        for (mb_h, shift) in [
            (23, 0),
            (30, 1),
            (45, 2),
            (68, 2),
            (135, 3),
            (12, 3),
        ] {
            let g = Geometry::new(4, mb_h, 1 << shift);
            for h in [4, 8, 16] {
                for yi in -80..(mb_h as isize + 8) * 16 {
                    assert_eq!(
                        Geometry::band_reading_luma_window_shift(mb_h, shift, yi, h),
                        g.band_reading_luma_window(yi, h),
                        "{mb_h} {shift} {yi} {h}"
                    );
                }
            }
        }
    }

    #[test]
    fn band_heights_follow_the_picture_s_height() {
        // One thread: one band. Eight: a sixteenth, a power of two from 1
        // to 8, with 4 in place of 2.
        assert_eq!(Geometry::band_mbs_for(45, 1), 45);
        assert_eq!(Geometry::band_mbs_for(0, 1), 1);
        for (mb_height, rows) in [
            (1, 1),
            (31, 1),
            (32, 4),
            (45, 4),
            (63, 4),
            (64, 4),
            (127, 4),
            (128, 8),
            (500, 8),
        ] {
            assert_eq!(Geometry::band_mbs_for(mb_height, 8), rows, "{mb_height}");
        }
    }

    /// A head that is not split stands for its sixteen blocks, whatever
    /// they hold; a split one sends its readers to them.
    #[test]
    fn a_head_stands_for_its_blocks_unless_split() {
        let g = Geometry::new(2, 1, 1);
        let mut band = BandData::default();
        band.fit(&g, 0);
        assert_eq!((band.heads.len(), band.motion.len()), (2, 32));
        assert!(band.heads.iter().all(|h| *h == NO_HEAD));
        let a = Motion {
            mv: [[4, -4], [0, 0]],
            ref_idx: [0, -1],
            ref_slot: [3, NO_SLOT],
        };
        let b = Motion {
            mv: [[8, 0], [0, 0]],
            ref_idx: [1, -1],
            ref_slot: [5, NO_SLOT],
        };
        // Entries an earlier picture left, under both heads.
        band.motion.fill(b);
        band.motion[16 + 5] = a;
        band.heads[0] = MbHead { m: a, split: false };
        band.heads[1] = MbHead { m: a, split: true };
        let one = band.mb_motion(0).expect("a head");
        let split = band.mb_motion(1).expect("a head");
        for i in 0..16 {
            assert_eq!(*one.block(i), a);
            assert_eq!(*split.block(i), if i == 5 { a } else { b });
        }
        assert!(band.mb_motion(2).is_none());
        // A macroblock no slice covers takes a head without motion.
        band.clear_macroblock(&g, 0, 1, 0);
        assert_eq!(band.heads[1], NO_HEAD);
    }

    #[test]
    fn a_new_picture_is_black() {
        let p = Picture::black(2, 2, &Limits::default()).expect("picture");
        assert_eq!(p.width(), 32);
        assert_eq!(p.height(), 32);
        assert!(p.luma().rows().all(|row| row.iter().all(|&s| s == 0)));
        let (cb, cr) = p.chroma();
        assert_eq!(cb.width(), 16);
        assert_eq!(cb.rows().count(), 16);
        assert!(cr.rows().all(|row| row.iter().all(|&s| s == 128)));
    }

    #[test]
    fn cropping_moves_the_view() {
        let mut p = Picture::black(2, 2, &Limits::default()).expect("picture");
        // A 1920x1080 stream crops 8 rows at the bottom; here, 2 columns on
        // the left and 6 rows at the bottom of a 32x32 picture.
        p.crop = [2, 0, 0, 6];
        let at = p.geometry.luma_origin(0) + 2;
        if let Some(s) = p.bands[0].get_mut().and_then(|b| b.y.get_mut(at)) {
            *s = 7;
        }
        assert_eq!(p.width(), 30);
        assert_eq!(p.height(), 26);
        assert_eq!(p.luma().rows().count(), 26);
        assert_eq!(p.luma().rows().next().and_then(|r| r.first()), Some(&7));
        let (cb, _) = p.chroma();
        assert_eq!(cb.width(), 15);
        assert_eq!(cb.height(), 13);
    }

    /// A canvas sizes a band when it is first asked for it, with every
    /// band above it, and publishing what is left sizes the rest: a new
    /// picture's planes are first touched by whoever decodes it.
    #[test]
    fn a_canvas_sizes_its_bands_as_it_reaches_them() {
        let g = Geometry::new(2, 9, 3);
        assert_eq!(g.bands(), 3);
        let sized = |band: &BandData, k: usize| {
            let (luma, chroma) = g.plane_samples(k);
            let mbs = g.motion(k).rows() * 2;
            (
                band.y.len(),
                band.cb.len(),
                band.cr.len(),
                band.heads.len(),
                band.motion.len(),
            ) == (luma, chroma, chroma, mbs, mbs * 16)
        };
        let picture = Arc::new(Picture::new(g, &Limits::default()).expect("picture"));
        let mut canvas = Canvas::new(Arc::clone(&picture), Vec::new());
        assert!(canvas.bands.iter().all(|band| band.y.is_empty()));
        let band = canvas.band_mut(1).expect("band 1");
        assert!(sized(band, 1));
        // A new plane comes zeroed, chroma included.
        assert!(band.cb.iter().chain(&band.cr).all(|&s| s == 0));
        assert!(sized(&canvas.bands[0], 0));
        assert!(canvas.bands[2].y.is_empty());
        drop(canvas);
        for k in 0..3 {
            assert!(sized(picture.published_band(k).expect("published"), k));
        }
        // A reused band keeps what it holds.
        let fresh = || Arc::new(Picture::new(g, &Limits::default()).expect("picture"));
        let mut canvas = Canvas::new(fresh(), Vec::new());
        canvas.band_mut(0).expect("band 0").cb[0] = 7;
        let kept = canvas.take_band(0);
        let mut canvas = Canvas::new(fresh(), vec![kept]);
        assert_eq!(canvas.band_mut(0).expect("band 0").cb[0], 7);
    }

    #[test]
    fn a_banded_picture_reads_back_row_by_row_without_its_aprons() {
        // 2 by 9 macroblocks in bands of 1, 2 and 3 rows, each band but the
        // first with a top apron reaching into one or two bands above.
        // Every stored row of a plane holds its own row number, aprons
        // included, so a view that read a wrong row would show it.
        for (height, bands) in [(1, 9), (2, 5), (3, 3)] {
            let g = Geometry::new(2, 9, height);
            assert_eq!(g.bands(), bands);
            let picture = Arc::new(Picture::new(g, &Limits::default()).expect("picture"));
            let mut canvas = Canvas::new(Arc::clone(&picture), Vec::new());
            for k in 0..g.bands() {
                let (l, c) = (g.luma(k), g.chroma(k));
                assert_eq!(
                    l.first,
                    l.start.saturating_sub(if k == 0 { 0 } else { TOP_LUMA })
                );
                assert_eq!(
                    c.first,
                    c.start.saturating_sub(if k == 0 { 0 } else { TOP_CHROMA })
                );
                let last = k + 1 == g.bands();
                let band = canvas.band_mut(k).expect("band");
                // From each plane's origin, past the slack rows of a band
                // holding row 0; the slack after the last band is left.
                let luma = band.y[g.luma_origin(k)..].chunks_mut(32).take(l.rows());
                for (i, row) in luma.enumerate() {
                    // A band's own bottom rows that the next band's filter
                    // changes are stale: a reader takes them from the next
                    // band's apron, and must never see these.
                    let stale = !last && l.first + i + FILTERED_LUMA >= l.end;
                    row.fill(if stale { 0xee } else { (l.first + i) as u8 });
                }
                let chroma = band.cb[g.chroma_origin(k)..].chunks_mut(16).take(c.rows());
                for (i, row) in chroma.enumerate() {
                    let stale = !last && c.first + i + FILTERED_CHROMA >= c.end;
                    row.fill(if stale { 0xee } else { (c.first + i) as u8 });
                }
            }
            canvas.publish_all().expect("published");
            drop(canvas);
            let mut p = Arc::try_unwrap(picture).expect("one holder");
            p.crop = [2, 0, 4, 2];
            let rows: Vec<u8> = p.luma().rows().map(|r| r[0]).collect();
            let want: Vec<u8> = (4..142).map(|r| r as u8).collect();
            assert_eq!(rows, want);
            assert!(p.luma().rows().all(|r| r.len() == 30));
            let (cb, _) = p.chroma();
            let rows: Vec<u8> = cb.rows().map(|r| r[0]).collect();
            let want: Vec<u8> = (2..71).map(|r| r as u8).collect();
            assert_eq!(rows, want);
        }
    }

    #[test]
    fn a_top_apron_takes_each_row_from_the_band_that_holds_it() {
        // 1 by 4 macroblocks in bands of one row: band 2's apron of 23 luma
        // rows reaches into bands 0 and 1. Every band's own rows and apron
        // hold their row numbers, except a band's own bottom three rows,
        // which hold the value before the next band's filter (0xe0 + row
        // in the band); band 0 is published, band 1 is not.
        let g = Geometry::new(1, 4, 1);
        let picture = Arc::new(Picture::new(g, &Limits::default()).expect("picture"));
        let mut canvas = Canvas::new(Arc::clone(&picture), Vec::new());
        for k in 0..g.bands() {
            let l = g.luma(k);
            let band = canvas.band_mut(k).expect("band");
            let luma = band.y[g.luma_origin(k)..].chunks_mut(16).take(l.rows());
            for (i, row) in luma.enumerate() {
                let r = l.first + i;
                row.fill(if r + FILTERED_LUMA >= l.end {
                    0xe0 + (r - l.start) as u8
                } else {
                    r as u8
                });
            }
        }
        canvas.publish_final(1).expect("band 0 published");
        canvas.refresh_top(2).expect("refreshed");
        let l = g.luma(2);
        let band = canvas.band_mut(2).expect("band 2");
        let got: Vec<u8> = band
            .y
            .chunks(16)
            .take(l.start - l.first)
            .map(|r| r[0])
            .collect();
        // Rows 9 to 28 are final where a reader takes them: band 0's own
        // rows 9 to 12, band 1's apron (13 to 15) and own rows (16 to 28).
        // Rows 29 to 31 are band 1's own, as they stand before band 2's
        // first row is filtered.
        let mut want: Vec<u8> = (9..29).collect();
        want.extend([0xed, 0xee, 0xef]);
        assert_eq!(l.first, 9);
        assert_eq!(got, want);
    }

    #[test]
    fn a_window_lies_in_the_band_it_is_read_from() {
        // Every luma window of 21 rows and chroma window of 9, clamped
        // into the picture, lies in the band a reader takes its last row
        // from.
        for height in 1..=4 {
            let g = Geometry::new(1, 11, height);
            type Reading = fn(&Geometry, usize) -> usize;
            for (rows, size, apron, filtered, band) in [
                (
                    176,
                    21,
                    TOP_LUMA,
                    FILTERED_LUMA,
                    Geometry::band_reading_luma_row as Reading,
                ),
                (
                    88,
                    9,
                    TOP_CHROMA,
                    FILTERED_CHROMA,
                    Geometry::band_reading_chroma_row,
                ),
            ] {
                for first in -30isize..rows as isize + 30 {
                    let clamp = |y: isize| y.clamp(0, rows as isize - 1) as usize;
                    let (top, bottom) = (clamp(first), clamp(first + size as isize - 1));
                    let k = band(&g, bottom);
                    let span = if rows == 176 { g.luma(k) } else { g.chroma(k) };
                    assert!(
                        span.first <= top && bottom < span.end,
                        "{height} {first} {k}"
                    );
                    assert!(k == 0 || span.start - span.first <= apron);
                    // Not the rows the next band's filter changes, which
                    // only its apron holds final.
                    assert!(k + 1 == g.bands() || bottom + filtered < span.end);
                }
            }
        }
    }

    #[test]
    fn a_block_s_windows_are_final_in_the_band_of_its_luma_window() {
        // Motion compensation reads a block's luma and chroma windows from
        // the band of its luma window (`band_reading_luma_window`). A luma
        // window inside the picture lies in that band, above the rows the
        // next band's filter changes; and every chroma window the band
        // holds, from its first stored chroma row to its end, lies above
        // the row the next band's filter changes. For every block height,
        // row, and vertical vector of up to 150 rows each way (a longer one
        // puts both windows outside the picture), at band heights 1 to 8.
        for height in [1, 2, 3, 4, 8] {
            let g = Geometry::new(1, 23, height);
            let rows: usize = 23 * 16;
            for h in [4usize, 8, 16] {
                for py in (0..rows - h + 1).step_by(4) {
                    for my in -600i32..600 {
                        let yi = py as i32 + (my >> 2);
                        let k = g.band_reading_luma_window(yi as isize, h);
                        let last_band = k + 1 == g.bands();
                        let what = format!("{height} {h} {py} {my} {k}");
                        let (top, bottom) = (yi - 2, yi + h as i32 + 2);
                        let luma = g.luma(k);
                        if top >= 0 && bottom < rows as i32 {
                            assert!(top as usize >= luma.first, "{what}");
                            assert!(
                                if last_band {
                                    (bottom as usize) < luma.end
                                } else {
                                    bottom as usize + FILTERED_LUMA < luma.end
                                },
                                "{what}"
                            );
                        }
                        let span = g.chroma(k);
                        let cy = (py / 2) as i32 + (my >> 3);
                        let bottom = cy + (h / 2) as i32;
                        if cy < span.first as i32 || bottom >= span.end as i32 {
                            // Not held: declined.
                            continue;
                        }
                        assert!(
                            last_band || bottom as usize + FILTERED_CHROMA < span.end,
                            "{what}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_picture_of_one_band_keeps_its_slack_rows_out_of_view() {
        // 3 by 2 macroblocks: in one band, each plane keeps 4 luma or 2
        // chroma rows of slack before and after it at the coded width.
        // Every sample of the picture is written with its own value and
        // the slack with another; the view, and a picture concealed from
        // it in either layout, show the samples.
        let one = Geometry::new(3, 2, 2);
        let banded = Geometry::new(3, 2, 1);
        assert_eq!((one.luma_origin(0), one.chroma_origin(0)), (4 * 48, 2 * 24));
        assert_eq!((one.luma_slack(0), one.chroma_slack(0)), ([4, 4], [2, 2]));
        assert_eq!(one.luma_at(0, 5), Some(4 * 48 + 5 * 48));
        assert_eq!(one.chroma_at(0, 3), Some(2 * 24 + 3 * 24));
        let value = |x: usize, y: usize, c: usize| (x * 7 + y * 13 + c * 5) as u8;
        let picture = Arc::new(Picture::new(one, &Limits::default()).expect("picture"));
        let mut canvas = Canvas::new(Arc::clone(&picture), Vec::new());
        let band = canvas.band_mut(0).expect("band");
        assert_eq!(band.y.len(), 48 * (32 + 8));
        assert_eq!(band.cb.len(), 24 * (16 + 4));
        for (c, plane) in [&mut band.y, &mut band.cb, &mut band.cr]
            .into_iter()
            .enumerate()
        {
            plane.fill(0xee);
            let (width, rows) = if c == 0 { (48, 32) } else { (24, 16) };
            for y in 0..rows {
                let at = if c == 0 {
                    one.luma_at(0, y)
                } else {
                    one.chroma_at(0, y)
                }
                .expect("row");
                for x in 0..width {
                    plane[at + x] = value(x, y, c);
                }
            }
        }
        canvas.publish_all().expect("published");
        drop(canvas);
        let shows = |p: &Picture| {
            let (cb, cr) = p.chroma();
            for (c, view) in [p.luma(), cb, cr].into_iter().enumerate() {
                let rows: Vec<Vec<u8>> = view.rows().map(<[u8]>::to_vec).collect();
                let (width, height) = if c == 0 { (48, 32) } else { (24, 16) };
                let want: Vec<Vec<u8>> = (0..height)
                    .map(|y| (0..width).map(|x| value(x, y, c)).collect())
                    .collect();
                assert_eq!(rows, want, "plane {c}");
            }
        };
        shows(&picture);
        for g in [banded, one] {
            let copy = Arc::new(Picture::new(g, &Limits::default()).expect("picture"));
            let mut canvas = Canvas::new(Arc::clone(&copy), Vec::new());
            assert!(canvas.conceal_from(&picture));
            canvas.publish_all().expect("published");
            drop(canvas);
            shows(&copy);
            let back = Arc::new(Picture::new(one, &Limits::default()).expect("picture"));
            let mut canvas = Canvas::new(Arc::clone(&back), Vec::new());
            assert!(canvas.conceal_from(&copy));
            canvas.publish_all().expect("published");
            drop(canvas);
            shows(&back);
        }
    }

    #[test]
    fn bands_at_the_pictures_edges_keep_slack_rows() {
        // 3 by 4 macroblocks in bands of one row: the bands whose stored
        // rows start at row 0 (the first, and the second, whose apron
        // reaches it) keep slack before their planes, the last band after
        // them, and no other band any.
        let g = Geometry::new(3, 4, 1);
        assert_eq!(
            [0, 1, 2, 3].map(|k| g.luma_first(k)),
            [0, 0, 32 - TOP_LUMA, 48 - TOP_LUMA]
        );
        assert_eq!(
            [0, 1, 2, 3].map(|k| g.luma_slack(k)),
            [[4, 0], [4, 0], [0, 0], [0, 4]]
        );
        assert_eq!(
            [0, 1, 2, 3].map(|k| g.chroma_slack(k)),
            [[2, 0], [2, 0], [0, 0], [0, 2]]
        );
        assert_eq!(
            [0, 1, 2, 3].map(|k| g.luma_origin(k)),
            [4 * 48, 4 * 48, 0, 0]
        );
        assert_eq!(
            [0, 1, 2, 3].map(|k| g.chroma_origin(k)),
            [2 * 24, 2 * 24, 0, 0]
        );
        assert_eq!(g.luma_at(1, 3), Some(4 * 48 + 3 * 48));
        assert_eq!(g.luma_at(3, 48), Some((48 - (48 - TOP_LUMA)) * 48));
        let (luma, chroma) = g.plane_samples(3);
        assert_eq!(
            (luma, chroma),
            ((16 + TOP_LUMA + 4) * 48, (8 + TOP_CHROMA + 2) * 24)
        );
    }

    #[test]
    fn the_windows_at_a_banded_pictures_edges_are_taken_from_their_band() {
        // A block at the picture's first row, or reaching its last, whose
        // window passes the edge while every sample its fraction uses lies
        // inside: the kernel takes it from the band it is read from, as it
        // does from a picture of one band, and predicts the samples.
        use coreav_kernels_h264::{Kernels, PlaneOp, Planes, PlanesMut, Scalar};
        let g = Geometry::new(3, 4, 1);
        let value = |x: usize, y: usize, c: usize| (x * 7 + y * 13 + c * 5) as u8;
        let picture = Arc::new(Picture::new(g, &Limits::default()).expect("picture"));
        let mut canvas = Canvas::new(Arc::clone(&picture), Vec::new());
        for k in 0..g.bands() {
            let band = canvas.band_mut(k).expect("band");
            let (luma, chroma) = (g.luma(k), g.chroma(k));
            for (c, plane) in [&mut band.y, &mut band.cb, &mut band.cr]
                .into_iter()
                .enumerate()
            {
                plane.fill(0xee);
                let (width, span) = if c == 0 { (48, luma) } else { (24, chroma) };
                for y in span.first..span.end {
                    let at = if c == 0 {
                        g.luma_at(k, y)
                    } else {
                        g.chroma_at(k, y)
                    }
                    .expect("row");
                    for x in 0..width {
                        plane[at + x] = value(x, y, c);
                    }
                }
            }
        }
        canvas.publish_all().expect("published");
        drop(canvas);
        // (px, py, vector): at the top with no vertical fraction, at the
        // top-left corner with none across either, and at the bottom.
        for (px, py, mv) in [
            (16, 0, [2, 0]),
            (0, 0, [0, 0]),
            (32, 48, [0, 0]),
            (16, 48, [1, -4]),
        ] {
            let k = g.band_reading_luma_window(py as isize + (mv[1] >> 2) as isize, 16);
            let data = picture.band(k).expect("published");
            let src = Planes::band(
                &data.y,
                &data.cb,
                &data.cr,
                48,
                64,
                [g.luma_first(k), g.chroma_first(k)],
                [g.luma_origin(k), g.chroma_origin(k)],
            )
            .expect("band view");
            let (mut y, mut cb, mut cr) = (vec![0; 48 * 64], vec![0; 24 * 32], vec![0; 24 * 32]);
            let dst = PlanesMut::new(&mut y, &mut cb, &mut cr, 48, [0, 0]);
            let taken = Scalar.predict(dst, &src, px, py, 16, 16, mv, [PlaneOp::Put; 3]);
            assert!(taken, "block at ({px}, {py}) from band {k}");
            if mv == [0, 0] {
                for r in 0..16 {
                    let row: Vec<u8> = (0..16).map(|x| value(px + x, py + r, 0)).collect();
                    assert_eq!(y[(py + r) * 48 + px..][..16], row[..], "row {r}");
                }
            }
        }
    }

    #[test]
    fn a_picture_larger_than_the_limit_fails() {
        let limits = Limits {
            max_mbs: 8,
            ..Limits::default()
        };
        assert_eq!(
            Picture::black(4, 4, &limits).err(),
            Some(Error::Limit(LimitKind::PictureSize))
        );
        assert_eq!(
            Picture::black(0, 4, &limits).err(),
            Some(Error::Limit(LimitKind::PictureSize))
        );
    }
}
