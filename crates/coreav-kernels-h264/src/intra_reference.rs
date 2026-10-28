//! A frozen oracle, compiled only for tests: the decoder's intra
//! prediction as it was before the rework that sums the chroma DC once and
//! reads neighbours from slices, which the reference in `intra.rs` (and so
//! every implementation of the kernels) is compared with byte for byte.
//!
//! It is not a copy to keep in step with `intra.rs`. It stays as it is when
//! `intra.rs` changes; a difference between the two is what the test
//! exists to find, and is fixed in `intra.rs` unless the specification
//! shows this file wrong. It came here from `coreav-h264` with the
//! prediction; only the lines that named that crate's error and
//! availability types changed.

use crate::IntraAvail as Avail;

/// The decoder's error, as far as this file uses it.
#[derive(Debug)]
pub(super) enum Error {
    Geometry(#[expect(dead_code, reason = "read only through `Debug`")] &'static str),
}

pub(super) type Result<T> = core::result::Result<T, Error>;

/// Clip1Y / Clip1C for 8-bit samples.
#[inline]
fn clip1(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// The neighbouring samples of a block: p[x, -1] for x up to 2n, p[-1, y]
/// for y up to n, and p[-1, -1].
struct Neighbours {
    top: [i32; 16],
    left: [i32; 16],
    top_left: i32,
    avail: Avail,
}

impl Neighbours {
    /// p(x, y) of the prediction equations, with x and y at least -1.
    #[inline]
    fn at(&self, x: i32, y: i32) -> i32 {
        if y < 0 {
            if x < 0 {
                self.top_left
            } else {
                self.top.get(x as usize).copied().unwrap_or(0)
            }
        } else {
            self.left.get(y as usize).copied().unwrap_or(0)
        }
    }

    /// The sum of the `n` samples above the block.
    fn top_sum(&self, n: usize) -> i32 {
        self.top.iter().take(n).sum()
    }

    /// The sum of the `n` samples left of the block.
    fn left_sum(&self, n: usize) -> i32 {
        self.left.iter().take(n).sum()
    }
}

/// Reads the neighbouring samples of an `n` by `n` block whose first sample
/// is `plane[off]`.
///
/// The `n` samples above the block are followed by `n` more where the block
/// may predict from them (`top_right`); where it may not, the last sample
/// above the block is repeated, as clause 8.3.1.2.3 and its neighbours do.
fn neighbours(plane: &[u8], stride: usize, off: usize, n: usize, avail: Avail) -> Neighbours {
    let mut nb = Neighbours {
        top: [0; 16],
        left: [0; 16],
        top_left: 0,
        avail,
    };
    let sample = |i: usize| plane.get(i).copied().map_or(0, i32::from);
    let above = if avail.top {
        off.checked_sub(stride)
    } else {
        None
    };
    if let Some(above) = above {
        let reach = if avail.top_right { 2 * n } else { n };
        for (x, slot) in nb.top.iter_mut().enumerate().take(2 * n) {
            *slot = if x < reach {
                sample(above + x)
            } else {
                sample(above + n - 1)
            };
        }
    }
    if avail.left {
        for (y, slot) in nb.left.iter_mut().enumerate().take(n) {
            *slot = off
                .checked_add(y * stride)
                .and_then(|i| i.checked_sub(1))
                .map_or(0, sample);
        }
    }
    if avail.top_left {
        nb.top_left = off
            .checked_sub(stride)
            .and_then(|i| i.checked_sub(1))
            .map_or(0, sample);
    }
    nb
}

/// Writes an `n` by `n` prediction, taken row by row from `block`, into the
/// plane at `off`.
fn write_block(plane: &mut [u8], stride: usize, off: usize, n: usize, block: &[u8]) -> Result<()> {
    for (y, row) in block.chunks_exact(n).take(n).enumerate() {
        let start = off
            .checked_add(y * stride)
            .ok_or(Error::Geometry("an intra block past the plane"))?;
        let out = plane
            .get_mut(start..start + n)
            .ok_or(Error::Geometry("an intra block past the plane"))?;
        out.copy_from_slice(row);
    }
    Ok(())
}

/// The DC prediction of an `n` by `n` block (Equations 8-47 to 8-50 and
/// their 8x8 and 16x16 counterparts).
fn dc(nb: &Neighbours, n: usize) -> i32 {
    let shift = n.trailing_zeros();
    match (nb.avail.top, nb.avail.left) {
        (true, true) => (nb.top_sum(n) + nb.left_sum(n) + n as i32) >> (shift + 1),
        (true, false) => (nb.top_sum(n) + (n as i32 >> 1)) >> shift,
        (false, true) => (nb.left_sum(n) + (n as i32 >> 1)) >> shift,
        (false, false) => 128,
    }
}

/// Intra_4x4 prediction of the block at `off` (clause 8.3.1.2).
pub(crate) fn pred4x4(
    plane: &mut [u8],
    stride: usize,
    off: usize,
    mode: u8,
    avail: Avail,
) -> Result<()> {
    let nb = neighbours(plane, stride, off, 4, avail);
    let p = |x: i32, y: i32| nb.at(x, y);
    let mut block = [0u8; 16];
    for (i, out) in block.iter_mut().enumerate() {
        let (x, y) = ((i % 4) as i32, (i / 4) as i32);
        let v = match mode {
            // Vertical and horizontal.
            0 => p(x, -1),
            1 => p(-1, y),
            2 => dc(&nb, 4),
            // Diagonal down-left (Equations 8-51 and 8-52).
            3 => {
                if x == 3 && y == 3 {
                    (p(6, -1) + 3 * p(7, -1) + 2) >> 2
                } else {
                    (p(x + y, -1) + 2 * p(x + y + 1, -1) + p(x + y + 2, -1) + 2) >> 2
                }
            }
            // Diagonal down-right (Equations 8-53 to 8-55).
            4 => match x.cmp(&y) {
                std::cmp::Ordering::Greater => {
                    (p(x - y - 2, -1) + 2 * p(x - y - 1, -1) + p(x - y, -1) + 2) >> 2
                }
                std::cmp::Ordering::Less => {
                    (p(-1, y - x - 2) + 2 * p(-1, y - x - 1) + p(-1, y - x) + 2) >> 2
                }
                std::cmp::Ordering::Equal => (p(0, -1) + 2 * p(-1, -1) + p(-1, 0) + 2) >> 2,
            },
            // Vertical-right (Equations 8-56 to 8-58).
            5 => {
                let z = 2 * x - y;
                if z >= 0 && z % 2 == 0 {
                    (p(x - (y >> 1) - 1, -1) + p(x - (y >> 1), -1) + 1) >> 1
                } else if z >= 0 {
                    (p(x - (y >> 1) - 2, -1)
                        + 2 * p(x - (y >> 1) - 1, -1)
                        + p(x - (y >> 1), -1)
                        + 2)
                        >> 2
                } else if z == -1 {
                    (p(-1, 0) + 2 * p(-1, -1) + p(0, -1) + 2) >> 2
                } else {
                    (p(-1, y - 1) + 2 * p(-1, y - 2) + p(-1, y - 3) + 2) >> 2
                }
            }
            // Horizontal-down (Equations 8-59 to 8-61).
            6 => {
                let z = 2 * y - x;
                if z >= 0 && z % 2 == 0 {
                    (p(-1, y - (x >> 1) - 1) + p(-1, y - (x >> 1)) + 1) >> 1
                } else if z >= 0 {
                    (p(-1, y - (x >> 1) - 2)
                        + 2 * p(-1, y - (x >> 1) - 1)
                        + p(-1, y - (x >> 1))
                        + 2)
                        >> 2
                } else if z == -1 {
                    (p(-1, 0) + 2 * p(-1, -1) + p(0, -1) + 2) >> 2
                } else {
                    (p(x - 1, -1) + 2 * p(x - 2, -1) + p(x - 3, -1) + 2) >> 2
                }
            }
            // Vertical-left (Equations 8-62 and 8-63).
            7 => {
                if y % 2 == 0 {
                    (p(x + (y >> 1), -1) + p(x + (y >> 1) + 1, -1) + 1) >> 1
                } else {
                    (p(x + (y >> 1), -1)
                        + 2 * p(x + (y >> 1) + 1, -1)
                        + p(x + (y >> 1) + 2, -1)
                        + 2)
                        >> 2
                }
            }
            // Horizontal-up (Equations 8-64 to 8-66).
            _ => {
                let z = x + 2 * y;
                if z <= 4 && z % 2 == 0 {
                    (p(-1, y + (x >> 1)) + p(-1, y + (x >> 1) + 1) + 1) >> 1
                } else if z < 5 {
                    (p(-1, y + (x >> 1))
                        + 2 * p(-1, y + (x >> 1) + 1)
                        + p(-1, y + (x >> 1) + 2)
                        + 2)
                        >> 2
                } else if z == 5 {
                    (p(-1, 2) + 3 * p(-1, 3) + 2) >> 2
                } else {
                    p(-1, 3)
                }
            }
        };
        *out = v as u8;
    }
    write_block(plane, stride, off, 4, &block)
}

/// Filters the neighbouring samples of an Intra_8x8 block
/// (clause 8.3.2.2.1).
fn filter8x8(nb: &Neighbours) -> Neighbours {
    let mut out = Neighbours {
        top: [0; 16],
        left: [0; 16],
        top_left: nb.top_left,
        avail: nb.avail,
    };
    let t = |x: i32| nb.at(x, -1);
    let l = |y: i32| nb.at(-1, y);
    if nb.avail.top {
        for (x, slot) in out.top.iter_mut().enumerate() {
            let x = x as i32;
            *slot = if x == 0 {
                if nb.avail.top_left {
                    (nb.top_left + 2 * t(0) + t(1) + 2) >> 2
                } else {
                    (3 * t(0) + t(1) + 2) >> 2
                }
            } else if x == 15 {
                (t(14) + 3 * t(15) + 2) >> 2
            } else {
                (t(x - 1) + 2 * t(x) + t(x + 1) + 2) >> 2
            };
        }
    }
    if nb.avail.top_left {
        out.top_left = match (nb.avail.top, nb.avail.left) {
            (true, true) => (t(0) + 2 * nb.top_left + l(0) + 2) >> 2,
            (true, false) => (3 * nb.top_left + t(0) + 2) >> 2,
            (false, true) => (3 * nb.top_left + l(0) + 2) >> 2,
            (false, false) => nb.top_left,
        };
    }
    if nb.avail.left {
        for (y, slot) in out.left.iter_mut().enumerate().take(8) {
            let y = y as i32;
            *slot = if y == 0 {
                if nb.avail.top_left {
                    (nb.top_left + 2 * l(0) + l(1) + 2) >> 2
                } else {
                    (3 * l(0) + l(1) + 2) >> 2
                }
            } else if y == 7 {
                (l(6) + 3 * l(7) + 2) >> 2
            } else {
                (l(y - 1) + 2 * l(y) + l(y + 1) + 2) >> 2
            };
        }
    }
    out
}

/// Intra_8x8 prediction of the block at `off` (clause 8.3.2.2).
pub(crate) fn pred8x8(
    plane: &mut [u8],
    stride: usize,
    off: usize,
    mode: u8,
    avail: Avail,
) -> Result<()> {
    let nb = filter8x8(&neighbours(plane, stride, off, 8, avail));
    let p = |x: i32, y: i32| nb.at(x, y);
    let mut block = [0u8; 64];
    for (i, out) in block.iter_mut().enumerate() {
        let (x, y) = ((i % 8) as i32, (i / 8) as i32);
        let v = match mode {
            0 => p(x, -1),
            1 => p(-1, y),
            2 => dc(&nb, 8),
            // Diagonal down-left (Equations 8-80 and 8-81).
            3 => {
                if x == 7 && y == 7 {
                    (p(14, -1) + 3 * p(15, -1) + 2) >> 2
                } else {
                    (p(x + y, -1) + 2 * p(x + y + 1, -1) + p(x + y + 2, -1) + 2) >> 2
                }
            }
            // Diagonal down-right (Equations 8-82 to 8-84).
            4 => match x.cmp(&y) {
                std::cmp::Ordering::Greater => {
                    (p(x - y - 2, -1) + 2 * p(x - y - 1, -1) + p(x - y, -1) + 2) >> 2
                }
                std::cmp::Ordering::Less => {
                    (p(-1, y - x - 2) + 2 * p(-1, y - x - 1) + p(-1, y - x) + 2) >> 2
                }
                std::cmp::Ordering::Equal => (p(0, -1) + 2 * p(-1, -1) + p(-1, 0) + 2) >> 2,
            },
            // Vertical-right (Equations 8-85 to 8-87).
            5 => {
                let z = 2 * x - y;
                if z >= 0 && z % 2 == 0 {
                    (p(x - (y >> 1) - 1, -1) + p(x - (y >> 1), -1) + 1) >> 1
                } else if z >= 0 {
                    (p(x - (y >> 1) - 2, -1)
                        + 2 * p(x - (y >> 1) - 1, -1)
                        + p(x - (y >> 1), -1)
                        + 2)
                        >> 2
                } else if z == -1 {
                    (p(-1, 0) + 2 * p(-1, -1) + p(0, -1) + 2) >> 2
                } else {
                    (p(-1, y - 2 * x - 1) + 2 * p(-1, y - 2 * x - 2) + p(-1, y - 2 * x - 3) + 2)
                        >> 2
                }
            }
            // Horizontal-down (Equations 8-88 to 8-90).
            6 => {
                let z = 2 * y - x;
                if z >= 0 && z % 2 == 0 {
                    (p(-1, y - (x >> 1) - 1) + p(-1, y - (x >> 1)) + 1) >> 1
                } else if z >= 0 {
                    (p(-1, y - (x >> 1) - 2)
                        + 2 * p(-1, y - (x >> 1) - 1)
                        + p(-1, y - (x >> 1))
                        + 2)
                        >> 2
                } else if z == -1 {
                    (p(-1, 0) + 2 * p(-1, -1) + p(0, -1) + 2) >> 2
                } else {
                    (p(x - 2 * y - 1, -1) + 2 * p(x - 2 * y - 2, -1) + p(x - 2 * y - 3, -1) + 2)
                        >> 2
                }
            }
            // Vertical-left (Equations 8-91 and 8-92).
            7 => {
                if y % 2 == 0 {
                    (p(x + (y >> 1), -1) + p(x + (y >> 1) + 1, -1) + 1) >> 1
                } else {
                    (p(x + (y >> 1), -1)
                        + 2 * p(x + (y >> 1) + 1, -1)
                        + p(x + (y >> 1) + 2, -1)
                        + 2)
                        >> 2
                }
            }
            // Horizontal-up (Equations 8-93 to 8-95).
            _ => {
                let z = x + 2 * y;
                if z <= 12 && z % 2 == 0 {
                    (p(-1, y + (x >> 1)) + p(-1, y + (x >> 1) + 1) + 1) >> 1
                } else if z < 13 {
                    (p(-1, y + (x >> 1))
                        + 2 * p(-1, y + (x >> 1) + 1)
                        + p(-1, y + (x >> 1) + 2)
                        + 2)
                        >> 2
                } else if z == 13 {
                    (p(-1, 6) + 3 * p(-1, 7) + 2) >> 2
                } else {
                    p(-1, 7)
                }
            }
        };
        *out = v as u8;
    }
    write_block(plane, stride, off, 8, &block)
}

/// The plane prediction mode of Intra_16x16 and of chroma (Equations 8-105
/// to 8-109 and 8-141 to 8-145), over an `n` by `n` block.
fn plane_prediction(nb: &Neighbours, n: usize, block: &mut [u8]) {
    let half = n as i32 / 2;
    let taps = n as i32 / 2;
    let (mut h, mut v) = (0i32, 0i32);
    for i in 0..taps {
        h += (i + 1) * (nb.at(half + i, -1) - nb.at(half - 2 - i, -1));
        v += (i + 1) * (nb.at(-1, half + i) - nb.at(-1, half - 2 - i));
    }
    let a = 16 * (nb.at(-1, n as i32 - 1) + nb.at(n as i32 - 1, -1));
    // The multiplier is (5 * H + 32) >> 6 for 16x16 and (34 * H + 32) >> 6
    // for an 8x8 chroma block.
    let scale = if n == 16 { 5 } else { 34 };
    let b = (scale * h + 32) >> 6;
    let c = (scale * v + 32) >> 6;
    for (i, out) in block.iter_mut().enumerate().take(n * n) {
        let (x, y) = ((i % n) as i32, (i / n) as i32);
        *out = clip1((a + b * (x - half + 1) + c * (y - half + 1) + 16) >> 5);
    }
}

/// Intra_16x16 prediction (clause 8.3.3).
pub(crate) fn pred16x16(
    plane: &mut [u8],
    stride: usize,
    off: usize,
    mode: u8,
    avail: Avail,
) -> Result<()> {
    let nb = neighbours(plane, stride, off, 16, avail);
    let mut block = [0u8; 256];
    match mode {
        0 => {
            for (i, out) in block.iter_mut().enumerate() {
                *out = nb.at((i % 16) as i32, -1) as u8;
            }
        }
        1 => {
            for (i, out) in block.iter_mut().enumerate() {
                *out = nb.at(-1, (i / 16) as i32) as u8;
            }
        }
        2 => block.fill(dc(&nb, 16) as u8),
        _ => plane_prediction(&nb, 16, &mut block),
    }
    write_block(plane, stride, off, 16, &block)
}

/// Intra chroma prediction of one 8x8 block of a 4:2:0 picture
/// (clause 8.3.4).
pub(crate) fn pred_chroma(
    plane: &mut [u8],
    stride: usize,
    off: usize,
    mode: u8,
    avail: Avail,
) -> Result<()> {
    let nb = neighbours(plane, stride, off, 8, avail);
    let mut block = [0u8; 64];
    match mode {
        // DC, over the four 4x4 blocks with their own rules
        // (clause 8.3.4.1).
        0 => {
            for (i, out) in block.iter_mut().enumerate() {
                let (x, y) = (i % 8, i / 8);
                let (bx, by) = (x / 4, y / 4);
                let top: i32 = nb.top.iter().skip(bx * 4).take(4).sum();
                let left: i32 = nb.left.iter().skip(by * 4).take(4).sum();
                // Blocks on the diagonal average both sides; the others
                // prefer the side they touch.
                let prefer_top = bx > by;
                let v = match (nb.avail.top, nb.avail.left) {
                    (true, true) if bx == by => (top + left + 4) >> 3,
                    (true, true) if prefer_top => (top + 2) >> 2,
                    (true, true) => (left + 2) >> 2,
                    (true, false) => (top + 2) >> 2,
                    (false, true) => (left + 2) >> 2,
                    (false, false) => 128,
                };
                *out = v as u8;
            }
        }
        1 => {
            for (i, out) in block.iter_mut().enumerate() {
                *out = nb.at(-1, (i / 8) as i32) as u8;
            }
        }
        2 => {
            for (i, out) in block.iter_mut().enumerate() {
                *out = nb.at((i % 8) as i32, -1) as u8;
            }
        }
        _ => plane_prediction(&nb, 8, &mut block),
    }
    write_block(plane, stride, off, 8, &block)
}
