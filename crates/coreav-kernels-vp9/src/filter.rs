//! The sample filtering process of the loop filter (clause 8.8.5): the
//! filter masks and the narrow and wide filters across one edge.

use crate::{BitDepth, Pixel, round2};

/// The largest filter an edge may take, `filterSize` (clause 8.8.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FilterSize {
    /// `TX_4X4`: the narrow filter only.
    Four,
    /// `TX_8X8`: up to the wide filter of 8 taps.
    Eight,
    /// `TX_16X16`: up to the wide filter of 16 taps.
    Sixteen,
}

impl FilterSize {
    /// The samples the filter reads on each side of the edge.
    fn reach(self) -> usize {
        match self {
            FilterSize::Four | FilterSize::Eight => 4,
            FilterSize::Sixteen => 8,
        }
    }
}

/// The strength of an edge's filter (clause 8.8.4): `limit`, `blimit` and
/// `thresh` of a filter level above 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Strength {
    limit: i32,
    blimit: i32,
    thresh: i32,
}

impl Strength {
    /// The strength of filter level `lvl` (1 to 63) at
    /// `loop_filter_sharpness` `sharpness` (0 to 7), or `None` for a level
    /// of 0, which filters nothing, or a value outside those ranges.
    pub fn new(lvl: u8, sharpness: u8) -> Option<Self> {
        if lvl == 0 || lvl > 63 || sharpness > 7 {
            return None;
        }
        let (lvl, sharpness) = (i32::from(lvl), i32::from(sharpness));
        let shift = match sharpness {
            0 => 0,
            1..=4 => 1,
            _ => 2,
        };
        let limit = if sharpness > 0 {
            (lvl >> shift).clamp(1, 9 - sharpness)
        } else {
            (lvl >> shift).max(1)
        };
        Some(Strength {
            limit,
            blimit: 2 * (lvl + 2) + limit,
            thresh: lvl >> 4,
        })
    }
}

/// The samples across one point of an edge: `q0` at `plane[at]`, the
/// `q` samples at `at + i * step` and the `p` samples at `at - (i + 1) *
/// step`, as many on each side as `size` reads.
#[derive(Debug)]
pub struct Edge<'a, P> {
    plane: &'a mut [P],
    at: usize,
    step: usize,
    size: FilterSize,
}

impl<'a, P: Pixel> Edge<'a, P> {
    /// The edge at `plane[at]` whose samples are `step` apart, or `None`
    /// when one the filter of `size` reads is outside `plane`.
    pub fn new(plane: &'a mut [P], at: usize, step: usize, size: FilterSize) -> Option<Self> {
        let reach = size.reach();
        if step == 0 || at.checked_sub(reach.checked_mul(step)?).is_none() {
            return None;
        }
        let last = at.checked_add((reach - 1).checked_mul(step)?)?;
        if last >= plane.len() {
            return None;
        }
        Some(Edge {
            plane,
            at,
            step,
            size,
        })
    }

    /// The sample `i` steps from the edge: `q(i)` for `i >= 0`, `p(-i - 1)`
    /// otherwise.
    fn get(&self, i: isize) -> i32 {
        self.index(i)
            .and_then(|k| self.plane.get(k))
            .map_or(0, |p| p.get())
    }

    fn set(&mut self, i: isize, v: i32, bit_depth: BitDepth) {
        if let Some(p) = self.index(i).and_then(|k| self.plane.get_mut(k)) {
            *p = P::clip(i64::from(v), bit_depth);
        }
    }

    fn index(&self, i: isize) -> Option<usize> {
        let offset = i.unsigned_abs().checked_mul(self.step)?;
        if i >= 0 {
            self.at.checked_add(offset)
        } else {
            self.at.checked_sub(offset)
        }
    }
}

/// The sample filtering process (clause 8.8.5) at one point of an edge.
pub fn filter_edge<P: Pixel>(mut edge: Edge<'_, P>, strength: Strength, bit_depth: BitDepth) {
    let shift = bit_depth.bits() - 8;
    let p = |e: &Edge<'_, P>, i: usize| e.get(-(i as isize) - 1);
    let q = |e: &Edge<'_, P>, i: usize| e.get(i as isize);
    let (p0, p1, p2, p3) = (p(&edge, 0), p(&edge, 1), p(&edge, 2), p(&edge, 3));
    let (q0, q1, q2, q3) = (q(&edge, 0), q(&edge, 1), q(&edge, 2), q(&edge, 3));
    let thresh = strength.thresh << shift;
    let hev = (p1 - p0).abs() > thresh || (q1 - q0).abs() > thresh;
    let limit = strength.limit << shift;
    let blimit = strength.blimit << shift;
    let outside = [p3 - p2, p2 - p1, p1 - p0, q1 - q0, q2 - q1, q3 - q2]
        .iter()
        .any(|d| d.abs() > limit)
        || (p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > blimit;
    if outside {
        return;
    }
    let flat_threshold = 1 << shift;
    let flat = |e: &Edge<'_, P>, range: core::ops::RangeInclusive<usize>| {
        range.into_iter().all(|i| {
            (p(e, i) - p0).abs() <= flat_threshold && (q(e, i) - q0).abs() <= flat_threshold
        })
    };
    let size = edge.size;
    if size == FilterSize::Four || !flat(&edge, 1..=3) {
        narrow(&mut edge, hev, bit_depth);
    } else if size == FilterSize::Eight || !flat(&edge, 4..=7) {
        wide(&mut edge, 3, bit_depth);
    } else {
        wide(&mut edge, 4, bit_depth);
    }
}

/// The narrow filter process (clause 8.8.5.2).
fn narrow<P: Pixel>(edge: &mut Edge<'_, P>, hev: bool, bit_depth: BitDepth) {
    let half = 1i32 << (bit_depth.bits() - 1);
    let offset = 0x80 << (bit_depth.bits() - 8);
    let clamp = |v: i32| v.clamp(-half, half - 1);
    let ps1 = edge.get(-2) - offset;
    let ps0 = edge.get(-1) - offset;
    let qs0 = edge.get(0) - offset;
    let qs1 = edge.get(1) - offset;
    let filter = if hev { clamp(ps1 - qs1) } else { 0 };
    let filter = clamp(filter + 3 * (qs0 - ps0));
    let filter1 = clamp(filter + 4) >> 3;
    let filter2 = clamp(filter + 3) >> 3;
    edge.set(0, clamp(qs0 - filter1) + offset, bit_depth);
    edge.set(-1, clamp(ps0 + filter2) + offset, bit_depth);
    if !hev {
        let filter = round2(i64::from(filter1), 1) as i32;
        edge.set(1, clamp(qs1 - filter) + offset, bit_depth);
        edge.set(-2, clamp(ps1 + filter) + offset, bit_depth);
    }
}

/// The wide filter process (clause 8.8.5.3) of `1 << log2_size` taps.
fn wide<P: Pixel>(edge: &mut Edge<'_, P>, log2_size: u32, bit_depth: BitDepth) {
    let n = (1isize << (log2_size - 1)) - 1;
    let mut original = [0i32; 16];
    for (sample, i) in original.iter_mut().zip(-(n + 1)..=n) {
        *sample = edge.get(i);
    }
    let sample = |i: isize| {
        original
            .get((i.clamp(-(n + 1), n) + n + 1) as usize)
            .copied()
            .unwrap_or(0)
    };
    let mut sum: i32 = (-n..=n).map(|j| sample(-n + j)).sum();
    let mut filtered = [0i32; 16];
    for (k, i) in (-n..n).enumerate() {
        if let Some(f) = filtered.get_mut(k) {
            *f = round2(i64::from(sum + sample(i)), log2_size) as i32;
        }
        sum += sample(i + n + 1) - sample(i - n);
    }
    for (k, i) in (-n..n).enumerate() {
        edge.set(i, filtered.get(k).copied().unwrap_or(0), bit_depth);
    }
}

#[cfg(test)]
mod tests;
