//! Separable resampling: per-position weights, a vertical pass along whole
//! rows, then a horizontal pass (`docs/design/thumbnails.md`, "Filters").

use std::collections::VecDeque;

use crate::FrameError;

/// A resampling filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Filter {
    /// Triangle kernel, radius 1.
    Bilinear,
    /// Keys cubic with a = −0.6, radius 2. The default: the kernel of
    /// FFmpeg's default scaler, measured as a black box (its impulse
    /// response matches this kernel stretched by the scale factor).
    #[default]
    Bicubic,
    /// Box filter: when shrinking, each output pixel is the mean of the
    /// source area it covers; when enlarging, bilinear.
    Area,
}

impl Filter {
    fn radius(self) -> f64 {
        match self {
            Self::Bilinear | Self::Area => 1.0,
            Self::Bicubic => 2.0,
        }
    }

    fn kernel(self, x: f64) -> f64 {
        let x = x.abs();
        match self {
            Self::Bilinear | Self::Area => (1.0 - x).max(0.0),
            Self::Bicubic => {
                const A: f64 = -0.6;
                if x < 1.0 {
                    ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0
                } else if x < 2.0 {
                    ((A * x - 5.0 * A) * x + 8.0 * A) * x - 4.0 * A
                } else {
                    0.0
                }
            }
        }
    }
}

/// How [`taps`] treats source positions outside the axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    /// Every position outside reads the nearest edge sample.
    Clamp,
    /// As FFmpeg's default scaler, measured as a black box
    /// (`docs/design/thumbnails.md`, "Y'CbCr to Y'CbCr"): as `Clamp`,
    /// except that when the bicubic enlarges, a tap before the first
    /// sample and more than 1.5 samples from the output position is
    /// dropped before the weights are normalized instead of read from the
    /// edge.
    FfmpegCompatible,
}

/// The source samples and weights of one output position.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Taps {
    pub(crate) start: usize,
    pub(crate) weights: Vec<f32>,
}

/// Weights for `dst` output positions over `src` source samples. `pos(i)`
/// is the centre of output position `i` in source sample coordinates and
/// `step` the source samples per output position (the kernel is stretched
/// by it when it exceeds 1). Positions outside the source clamp to its
/// edges (see [`Edge`]). Each position's weights sum to 1.
pub(crate) fn taps(
    src: usize,
    dst: usize,
    step: f64,
    filter: Filter,
    edge: Edge,
    pos: impl Fn(usize) -> f64,
) -> Result<Vec<Taps>, FrameError> {
    if src == 0 || dst == 0 {
        return Err(FrameError::InvalidGeometry("empty resampling axis"));
    }
    let stretch = step.max(1.0);
    let support = filter.radius() * stretch;
    let last = src - 1;
    // How far before the first sample FFmpeg's scaler keeps taps.
    let reach = if edge == Edge::FfmpegCompatible && filter == Filter::Bicubic && step < 1.0 {
        1.5
    } else {
        f64::INFINITY
    };
    let mut out = Vec::new();
    out.try_reserve_exact(dst)
        .map_err(|_| FrameError::OutOfMemory)?;
    for i in 0..dst {
        let x = pos(i);
        let lo = clamp_index((x - support).ceil(), last);
        let hi = clamp_index((x + support).floor(), last);
        let (first, end) = ((x - support).ceil(), (x + support).floor());
        let mut weights = vec![0f32; hi.saturating_sub(lo) + 1];
        let mut sum = 0f64;
        let mut j = first;
        while j <= end {
            let w = if j < 0.0 && x - j > reach {
                0.0
            } else if filter == Filter::Area && step > 1.0 {
                // Overlap of the output pixel's area with source pixel j.
                let (a0, a1) = (x - step / 2.0, x + step / 2.0);
                ((a1).min(j + 0.5) - (a0).max(j - 0.5)).max(0.0) / step
            } else {
                filter.kernel((j - x) / stretch)
            };
            if w != 0.0 {
                let index = clamp_index(j, last).saturating_sub(lo);
                if let Some(slot) = weights.get_mut(index) {
                    // Weights are small; f32 keeps the passes fast.
                    #[allow(clippy::cast_possible_truncation)]
                    let w32 = w as f32;
                    *slot += w32;
                    sum += w;
                }
            }
            j += 1.0;
        }
        if sum.abs() < 1e-9 {
            // No overlap (cannot happen for valid positions): nearest.
            let nearest = clamp_index(x.round(), last);
            out.push(Taps {
                start: nearest,
                weights: vec![1.0],
            });
            continue;
        }
        #[allow(clippy::cast_possible_truncation)]
        let norm = (1.0 / sum) as f32;
        for w in &mut weights {
            *w *= norm;
        }
        // Drop zero weights at both ends (Catmull-Rom is 0 at whole
        // offsets), so an identity resampling reads one sample.
        let lead = weights.iter().take_while(|w| **w == 0.0).count();
        let trail = weights.iter().rev().take_while(|w| **w == 0.0).count();
        let keep = weights.len().saturating_sub(lead + trail).max(1);
        let trimmed: Vec<f32> = weights.into_iter().skip(lead).take(keep).collect();
        out.push(Taps {
            start: lo + lead,
            weights: trimmed,
        });
    }
    Ok(out)
}

fn clamp_index(x: f64, last: usize) -> usize {
    if x <= 0.0 {
        0
    } else {
        // `last` fits an f64 exactly for every size a picture can have.
        #[allow(clippy::cast_precision_loss)]
        let max = last as f64;
        if x >= max {
            last
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let i = x as usize;
            i
        }
    }
}

/// How the samples of one channel are read from a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Channel {
    /// Bytes per element (one pixel of the plane).
    pub(crate) element: usize,
    /// Byte offset of the channel's sample in an element.
    pub(crate) offset: usize,
    /// 1 or 2 bytes per sample (little-endian).
    pub(crate) sample: usize,
    /// Right shift of a 16-bit sample whose value is in the high bits.
    pub(crate) shift: u32,
}

impl Channel {
    /// Writes the channel's samples of `row` to `out` as `f32`.
    pub(crate) fn load(&self, row: &[u8], out: &mut [f32]) {
        match (self.element, self.sample) {
            (1, 1) => {
                for (o, s) in out.iter_mut().zip(row) {
                    *o = f32::from(*s);
                }
            }
            (2, 2) if self.shift == 0 => {
                for (o, s) in out.iter_mut().zip(row.chunks_exact(2)) {
                    *o = f32::from(le16(s));
                }
            }
            _ => {
                for (o, e) in out.iter_mut().zip(row.chunks_exact(self.element.max(1))) {
                    let s = e.get(self.offset..).unwrap_or(&[]);
                    *o = if self.sample == 1 {
                        f32::from(s.first().copied().unwrap_or(0))
                    } else {
                        f32::from(le16(s) >> self.shift)
                    };
                }
            }
        }
    }
}

#[inline]
fn le16(s: &[u8]) -> u16 {
    match s {
        [a, b, ..] => u16::from_le_bytes([*a, *b]),
        _ => 0,
    }
}

/// A plane to resample: its rows, size in elements and how to read one
/// channel.
pub(crate) struct Source<'a> {
    pub(crate) rows: Vec<&'a [u8]>,
    pub(crate) width: usize,
    pub(crate) channel: Channel,
}

/// Work (source samples times vertical taps) above which [`resample`]
/// splits the output rows between threads.
const PARALLEL_WORK: usize = 1_000_000;
/// Most threads [`resample`] uses.
const MAX_THREADS: usize = 8;

/// The order of the two passes of [`resample`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Order {
    /// Vertical along whole source rows, then horizontal: the fastest.
    VerticalFirst,
    /// Horizontal, then vertical, with the horizontal results cut at `max`
    /// in between, with no lower cut (negative overshoots pass through), as
    /// FFmpeg's scaler keeps them
    /// (`docs/design/thumbnails.md`, "Y'CbCr to Y'CbCr").
    HorizontalFirst {
        /// The largest intermediate value.
        max: f32,
    },
}

/// Resamples one channel to `dst_w` × `dst_h` `f32` samples (row-major),
/// with the given horizontal and vertical taps, in the given order. Large
/// pictures are split into bands of output rows, one thread each.
pub(crate) fn resample(
    src: &Source<'_>,
    htaps: &[Taps],
    vtaps: &[Taps],
    order: Order,
) -> Result<Vec<f32>, FrameError> {
    let (dst_w, dst_h) = (htaps.len(), vtaps.len());
    let total = dst_w
        .checked_mul(dst_h)
        .ok_or(FrameError::InvalidGeometry("output size overflows"))?;
    let mut out = Vec::new();
    out.try_reserve_exact(total)
        .map_err(|_| FrameError::OutOfMemory)?;
    out.resize(total, 0f32);
    if total == 0 {
        return Ok(out);
    }
    let taps = vtaps.first().map_or(1, |t| t.weights.len());
    let work = match order {
        Order::VerticalFirst => src.width.saturating_mul(taps).saturating_mul(dst_h),
        // Every source row filtered to the output width.
        Order::HorizontalFirst { .. } => {
            let htaps_len = htaps.first().map_or(1, |t| t.weights.len());
            src.rows
                .len()
                .saturating_mul(dst_w)
                .saturating_mul(htaps_len)
        }
    };
    let threads = if work < PARALLEL_WORK {
        1
    } else {
        std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .clamp(1, MAX_THREADS)
            .min(dst_h)
    };
    let band = dst_h.div_ceil(threads);
    if threads == 1 {
        return rows_in_order(src, htaps, vtaps, order, &mut out).map(|()| out);
    }
    // `None` for a band whose thread could not start.
    let results: Vec<Option<Result<(), FrameError>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = out
            .chunks_mut(band * dst_w)
            .zip(vtaps.chunks(band))
            .map(|(rows, vts)| {
                std::thread::Builder::new()
                    .spawn_scoped(scope, move || rows_in_order(src, htaps, vts, order, rows))
                    .ok()
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.map(|h| {
                    h.join()
                        .unwrap_or(Err(FrameError::Unsupported("resampling thread failed")))
                })
            })
            .collect()
    });
    // Bands whose thread could not start run on this one.
    for ((rows, vts), result) in out
        .chunks_mut(band * dst_w)
        .zip(vtaps.chunks(band))
        .zip(results)
    {
        match result {
            Some(result) => result?,
            None => rows_in_order(src, htaps, vts, order, rows)?,
        }
    }
    Ok(out)
}

fn rows_in_order(
    src: &Source<'_>,
    htaps: &[Taps],
    vtaps: &[Taps],
    order: Order,
    out: &mut [f32],
) -> Result<(), FrameError> {
    match order {
        Order::VerticalFirst => resample_rows(src, htaps, vtaps, out),
        Order::HorizontalFirst { max } => horizontal_first(src, htaps, vtaps, max, out),
    }
}

/// A zeroed `f32` buffer, or [`FrameError::OutOfMemory`].
fn floats(len: usize) -> Result<Vec<f32>, FrameError> {
    let mut v = Vec::new();
    v.try_reserve_exact(len)
        .map_err(|_| FrameError::OutOfMemory)?;
    v.resize(len, 0f32);
    Ok(v)
}

/// Resamples the output rows of `vtaps` into `out` (`vtaps.len()` rows of
/// `htaps.len()` samples), pushing rather than pulling: each source row
/// the band's taps reach is read once, resampled horizontally into one
/// scratch row and cut at `max`, then added, weighted, into every output
/// row whose taps cover it. Memory beyond `out` is one source row, one
/// output row and a list as long as the band's vertical taps, whatever
/// the scale factors.
fn horizontal_first(
    src: &Source<'_>,
    htaps: &[Taps],
    vtaps: &[Taps],
    max: f32,
    out: &mut [f32],
) -> Result<(), FrameError> {
    let dst_w = htaps.len();
    // Every (source row, output row, weight) of the band, by source row:
    // as many entries as the vertical taps hold.
    let count = vtaps.iter().map(|t| t.weights.len()).sum();
    let mut pushes: Vec<(usize, usize, f32)> = Vec::new();
    pushes
        .try_reserve_exact(count)
        .map_err(|_| FrameError::OutOfMemory)?;
    for (r, vt) in vtaps.iter().enumerate() {
        for (k, w) in vt.weights.iter().enumerate() {
            pushes.push((vt.start + k, r, *w));
        }
    }
    pushes.sort_by_key(|&(y, r, _)| (y, r));
    let mut raw = floats(src.width)?;
    let mut row = floats(dst_w)?;
    out.iter_mut().for_each(|o| *o = 0.0);
    let mut filtered = None;
    for (y, r, w) in pushes {
        if filtered != Some(y) {
            if let Some(bytes) = src.rows.get(y) {
                src.channel.load(bytes, &mut raw);
            }
            for (o, ht) in row.iter_mut().zip(htaps) {
                let window = raw
                    .get(ht.start..ht.start + ht.weights.len())
                    .ok_or(FrameError::InvalidGeometry("taps outside the row"))?;
                *o = dot(window, &ht.weights).min(max);
            }
            filtered = Some(y);
        }
        let line = out
            .get_mut(r * dst_w..(r + 1) * dst_w)
            .ok_or(FrameError::InvalidGeometry("short output band"))?;
        for (o, s) in line.iter_mut().zip(&row) {
            *o += w * s;
        }
    }
    Ok(())
}

/// Resamples the output rows of `vtaps` into `out` (`vtaps.len()` rows of
/// `htaps.len()` samples): a vertical pass along whole source rows, then a
/// horizontal one.
fn resample_rows(
    src: &Source<'_>,
    htaps: &[Taps],
    vtaps: &[Taps],
    out: &mut [f32],
) -> Result<(), FrameError> {
    let mut acc = vec![0f32; src.width];
    // Source rows converted to f32, for the rows the current output row's
    // taps cover; each source row is converted once per band, as the
    // window only moves down.
    let mut cache: VecDeque<(usize, Vec<f32>)> = VecDeque::new();
    let mut spare: Vec<Vec<f32>> = Vec::new();
    for (vt, line) in vtaps.iter().zip(out.chunks_exact_mut(htaps.len().max(1))) {
        while cache.front().is_some_and(|(y, _)| *y < vt.start) {
            if let Some((_, row)) = cache.pop_front() {
                spare.push(row);
            }
        }
        acc.iter_mut().for_each(|a| *a = 0.0);
        for (k, w) in vt.weights.iter().enumerate() {
            let y = vt.start + k;
            let cached = cache.iter().position(|(cy, _)| *cy == y);
            let index = match cached {
                Some(index) => index,
                None => {
                    let mut row = spare.pop().unwrap_or_else(|| vec![0f32; src.width]);
                    if let Some(bytes) = src.rows.get(y) {
                        src.channel.load(bytes, &mut row);
                    }
                    cache.push_back((y, row));
                    cache.len() - 1
                }
            };
            if let Some((_, row)) = cache.get(index) {
                for (a, s) in acc.iter_mut().zip(row) {
                    *a += w * s;
                }
            }
        }
        for (o, ht) in line.iter_mut().zip(htaps) {
            let window = acc
                .get(ht.start..ht.start + ht.weights.len())
                .ok_or(FrameError::InvalidGeometry("taps outside the row"))?;
            *o = dot(window, &ht.weights);
        }
    }
    Ok(())
}

/// The dot product of two equal-length slices, in four lanes so that the
/// compiler can vectorize it.
#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0f32; 4];
    let (ca, cb) = (a.chunks_exact(4), b.chunks_exact(4));
    let (ra, rb) = (ca.remainder(), cb.remainder());
    for (x, y) in ca.zip(cb) {
        for ((l, p), q) in lanes.iter_mut().zip(x).zip(y) {
            *l += p * q;
        }
    }
    let tail: f32 = ra.iter().zip(rb).map(|(p, q)| p * q).sum();
    lanes.iter().sum::<f32>() + tail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_reads_one_sample() {
        for filter in [Filter::Bilinear, Filter::Bicubic, Filter::Area] {
            let t = taps(10, 10, 1.0, filter, Edge::Clamp, |i| i as f64).expect("taps");
            for (i, tap) in t.iter().enumerate() {
                assert_eq!(tap.start, i, "{filter:?}");
                assert_eq!(tap.weights, vec![1.0], "{filter:?}");
            }
        }
    }

    #[test]
    fn weights_sum_to_one_and_stay_inside() {
        for (src, dst) in [(1920, 320), (7, 1000), (1, 5), (5, 1), (16384, 3)] {
            let step = src as f64 / dst as f64;
            for filter in [Filter::Bilinear, Filter::Bicubic, Filter::Area] {
                let t = taps(src, dst, step, filter, Edge::Clamp, |i| {
                    (i as f64 + 0.5) * step - 0.5
                })
                .expect("taps");
                for tap in &t {
                    let sum: f32 = tap.weights.iter().sum();
                    assert!((sum - 1.0).abs() < 1e-4, "{src}->{dst} {filter:?} {sum}");
                    assert!(tap.start + tap.weights.len() <= src);
                }
            }
        }
    }

    #[test]
    fn area_is_the_mean() {
        let t = taps(4, 2, 2.0, Filter::Area, Edge::Clamp, |i| {
            (i as f64 + 0.5) * 2.0 - 0.5
        })
        .expect("taps");
        assert_eq!(t[0].start, 0);
        assert_eq!(t[0].weights, vec![0.5, 0.5]);
        assert_eq!(t[1].start, 2);
    }

    #[test]
    fn resample_constant_plane() {
        let row = vec![200u8; 8];
        let rows: Vec<&[u8]> = (0..8).map(|_| row.as_slice()).collect();
        let src = Source {
            rows,
            width: 8,
            channel: Channel {
                element: 1,
                offset: 0,
                sample: 1,
                shift: 0,
            },
        };
        let h = taps(8, 3, 8.0 / 3.0, Filter::Bicubic, Edge::Clamp, |i| {
            (i as f64 + 0.5) * 8.0 / 3.0 - 0.5
        })
        .expect("taps");
        let v = taps(8, 5, 8.0 / 5.0, Filter::Bicubic, Edge::Clamp, |i| {
            (i as f64 + 0.5) * 8.0 / 5.0 - 0.5
        })
        .expect("taps");
        let out = resample(&src, &h, &v, Order::VerticalFirst).expect("resample");
        assert_eq!(out.len(), 15);
        assert!(out.iter().all(|v| (v - 200.0).abs() < 1e-3));
    }

    fn grey(rows: &[Vec<u8>]) -> Source<'_> {
        Source {
            rows: rows.iter().map(Vec::as_slice).collect(),
            width: rows[0].len(),
            channel: Channel {
                element: 1,
                offset: 0,
                sample: 1,
                shift: 0,
            },
        }
    }

    fn centred(src: usize, dst: usize) -> Vec<Taps> {
        let step = src as f64 / dst as f64;
        taps(src, dst, step, Filter::Bicubic, Edge::Clamp, |i| {
            (i as f64 + 0.5) * step - 0.5
        })
        .expect("taps")
    }

    #[test]
    fn pass_order_only_matters_through_the_clip() {
        let rows: Vec<Vec<u8>> = (0..9)
            .map(|y| (0..11).map(|x| ((x * 37 + y * 91) % 256) as u8).collect())
            .collect();
        let src = grey(&rows);
        for (w, h) in [(23, 5), (4, 17), (11, 9)] {
            let (ht, vt) = (centred(11, w), centred(9, h));
            let a = resample(&src, &ht, &vt, Order::VerticalFirst).expect("resample");
            let b =
                resample(&src, &ht, &vt, Order::HorizontalFirst { max: 1e9 }).expect("resample");
            assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-2));
        }
    }

    #[test]
    fn horizontal_first_clips_the_overshoot() {
        // A bright bar on the lower rows: enlarging it overshoots above 255
        // horizontally; the clip cuts that before the rows are blended.
        let rows: Vec<Vec<u8>> = (0..8)
            .map(|y| {
                (0..8)
                    .map(|x| if y >= 4 && x >= 4 { 255 } else { 0 })
                    .collect()
            })
            .collect();
        let src = grey(&rows);
        let (ht, vt) = (centred(8, 16), centred(8, 16));
        let free = resample(&src, &ht, &vt, Order::HorizontalFirst { max: 1e9 }).expect("resample");
        let cut =
            resample(&src, &ht, &vt, Order::HorizontalFirst { max: 256.0 }).expect("resample");
        assert!(free.iter().zip(&cut).any(|(f, c)| f - c > 1.0));
        // Rows whose taps do not reach the bar do not change.
        assert_eq!(free[..16 * 5], cut[..16 * 5]);
    }

    /// A column of 16384 samples shrunk to one row while its width grows
    /// to 16384 (to 256 in a debug build, where the 2^28 multiply-adds of
    /// the full shape take a minute): each source row is filtered once
    /// into one scratch row and pushed into the output, so memory stays at
    /// a few rows. Pulling rows into a cache held the whole vertical
    /// window instead, about a gigabyte at the full shape.
    #[test]
    fn horizontal_first_memory_does_not_grow_with_the_window() {
        let width = if cfg!(debug_assertions) { 256 } else { 16384 };
        let rows: Vec<Vec<u8>> = (0..16384).map(|y| vec![(y % 251) as u8]).collect();
        let src = grey(&rows);
        let ht = centred(1, width);
        let vt = centred(16384, 1);
        assert_eq!(vt[0].weights.len(), 16384);
        let a = resample(&src, &ht, &vt, Order::VerticalFirst).expect("resample");
        let b = resample(&src, &ht, &vt, Order::HorizontalFirst { max: 256.0 }).expect("resample");
        assert_eq!(b.len(), width);
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-2));
    }
}
