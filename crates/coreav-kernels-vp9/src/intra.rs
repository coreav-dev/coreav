//! Intra prediction (clause 8.5.1) from the edges the decoder gathered.

use crate::{BlockMut, Pixel, round2};

/// The intra prediction modes (`intra_mode` and `uv_mode`, clause 7.4.5),
/// numbered as the specification numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntraMode {
    /// `DC_PRED` (0).
    Dc,
    /// `V_PRED` (1).
    V,
    /// `H_PRED` (2).
    H,
    /// `D45_PRED` (3).
    D45,
    /// `D135_PRED` (4).
    D135,
    /// `D117_PRED` (5).
    D117,
    /// `D153_PRED` (6).
    D153,
    /// `D207_PRED` (7).
    D207,
    /// `D63_PRED` (8).
    D63,
    /// `TM_PRED` (9).
    Tm,
}

impl IntraMode {
    /// The mode numbered `value`, or `None` past `TM_PRED`.
    pub fn from_value(value: u8) -> Option<Self> {
        Some(match value {
            0 => IntraMode::Dc,
            1 => IntraMode::V,
            2 => IntraMode::H,
            3 => IntraMode::D45,
            4 => IntraMode::D135,
            5 => IntraMode::D117,
            6 => IntraMode::D153,
            7 => IntraMode::D207,
            8 => IntraMode::D63,
            9 => IntraMode::Tm,
            _ => return None,
        })
    }
}

/// The edges a block of `size` samples is predicted from: `aboveRow[i]`
/// for i = -1 to `2 * size - 1`, at `above[i + 1]`, and `leftCol[i]` for
/// i = 0 to `size - 1`, as clause 8.5.1 sets them (the decoder fills them,
/// with the values that stand in for unavailable samples), and whether the
/// samples above and to the left are available, which `DC_PRED` reads.
#[derive(Clone, Debug)]
pub struct Edges<P> {
    /// `aboveRow[-1]` to `aboveRow[2 * size - 1]`.
    pub above: [P; 65],
    /// `leftCol[0]` to `leftCol[size - 1]`.
    pub left: [P; 32],
    /// `haveAbove`.
    pub have_above: bool,
    /// `haveLeft`.
    pub have_left: bool,
}

impl<P: Pixel> Default for Edges<P> {
    fn default() -> Self {
        Edges {
            above: [P::default(); 65],
            left: [P::default(); 32],
            have_above: false,
            have_left: false,
        }
    }
}

/// Predicts `dst`, of 4, 8, 16 or 32 samples square, with `mode` from
/// `edges` (clause 8.5.1).
pub fn predict_intra<P: Pixel>(
    mut dst: BlockMut<'_, P>,
    mode: IntraMode,
    edges: &Edges<P>,
    bit_depth: u8,
) {
    let size = dst.size();
    let log2 = size.trailing_zeros();
    let above = |i: isize| -> i64 {
        usize::try_from(i + 1)
            .ok()
            .and_then(|i| edges.above.get(i))
            .map_or(0, |p| p.get())
    };
    let left = |i: usize| edges.left.get(i).map_or(0, |p| p.get());
    let mut pred = [[0i64; 32]; 32];
    let s = size as isize;
    match mode {
        IntraMode::V => {
            for i in 0..size {
                for j in 0..size {
                    put(&mut pred, i, j, above(j as isize));
                }
            }
        }
        IntraMode::H => {
            for i in 0..size {
                for j in 0..size {
                    put(&mut pred, i, j, left(i));
                }
            }
        }
        IntraMode::D207 => {
            for j in 0..size {
                put(&mut pred, size - 1, j, left(size - 1));
            }
            for i in 0..size - 1 {
                put(&mut pred, i, 0, round2(left(i) + left(i + 1), 1));
            }
            for i in 0..size.saturating_sub(2) {
                put(
                    &mut pred,
                    i,
                    1,
                    round2(left(i) + 2 * left(i + 1) + left(i + 2), 2),
                );
            }
            put(
                &mut pred,
                size - 2,
                1,
                round2(left(size - 2) + 3 * left(size - 1), 2),
            );
            for i in (0..size - 1).rev() {
                for j in 2..size {
                    let v = pred
                        .get(i + 1)
                        .and_then(|r| r.get(j - 2))
                        .copied()
                        .unwrap_or(0);
                    put(&mut pred, i, j, v);
                }
            }
        }
        IntraMode::D45 => {
            for i in 0..size {
                for j in 0..size {
                    let k = (i + j) as isize;
                    let v = if k + 2 < 2 * s {
                        round2(above(k) + above(k + 1) * 2 + above(k + 2), 2)
                    } else {
                        above(2 * s - 1)
                    };
                    put(&mut pred, i, j, v);
                }
            }
        }
        IntraMode::D63 => {
            for i in 0..size {
                for j in 0..size {
                    let k = (i / 2 + j) as isize;
                    let v = if i & 1 == 1 {
                        round2(above(k) + above(k + 1) * 2 + above(k + 2), 2)
                    } else {
                        round2(above(k) + above(k + 1), 1)
                    };
                    put(&mut pred, i, j, v);
                }
            }
        }
        IntraMode::D117 => {
            for j in 0..size {
                let j = j as isize;
                put(&mut pred, 0, j as usize, round2(above(j - 1) + above(j), 1));
            }
            put(
                &mut pred,
                1,
                0,
                round2(left(0) + 2 * above(-1) + above(0), 2),
            );
            for j in 1..size {
                let j = j as isize;
                put(
                    &mut pred,
                    1,
                    j as usize,
                    round2(above(j - 2) + 2 * above(j - 1) + above(j), 2),
                );
            }
            put(
                &mut pred,
                2,
                0,
                round2(above(-1) + 2 * left(0) + left(1), 2),
            );
            for i in 3..size {
                put(
                    &mut pred,
                    i,
                    0,
                    round2(left(i - 3) + 2 * left(i - 2) + left(i - 1), 2),
                );
            }
            for i in 2..size {
                for j in 1..size {
                    let v = pred
                        .get(i - 2)
                        .and_then(|r| r.get(j - 1))
                        .copied()
                        .unwrap_or(0);
                    put(&mut pred, i, j, v);
                }
            }
        }
        IntraMode::D135 => {
            put(
                &mut pred,
                0,
                0,
                round2(left(0) + 2 * above(-1) + above(0), 2),
            );
            for j in 1..size {
                let j = j as isize;
                put(
                    &mut pred,
                    0,
                    j as usize,
                    round2(above(j - 2) + 2 * above(j - 1) + above(j), 2),
                );
            }
            put(
                &mut pred,
                1,
                0,
                round2(above(-1) + 2 * left(0) + left(1), 2),
            );
            for i in 2..size {
                put(
                    &mut pred,
                    i,
                    0,
                    round2(left(i - 2) + 2 * left(i - 1) + left(i), 2),
                );
            }
            for i in 1..size {
                for j in 1..size {
                    let v = pred
                        .get(i - 1)
                        .and_then(|r| r.get(j - 1))
                        .copied()
                        .unwrap_or(0);
                    put(&mut pred, i, j, v);
                }
            }
        }
        IntraMode::D153 => {
            put(&mut pred, 0, 0, round2(left(0) + above(-1), 1));
            for i in 1..size {
                put(&mut pred, i, 0, round2(left(i - 1) + left(i), 1));
            }
            put(
                &mut pred,
                0,
                1,
                round2(left(0) + 2 * above(-1) + above(0), 2),
            );
            put(
                &mut pred,
                1,
                1,
                round2(above(-1) + 2 * left(0) + left(1), 2),
            );
            for i in 2..size {
                put(
                    &mut pred,
                    i,
                    1,
                    round2(left(i - 2) + 2 * left(i - 1) + left(i), 2),
                );
            }
            for j in 2..size {
                let j = j as isize;
                put(
                    &mut pred,
                    0,
                    j as usize,
                    round2(above(j - 3) + 2 * above(j - 2) + above(j - 1), 2),
                );
            }
            for i in 1..size {
                for j in 2..size {
                    let v = pred
                        .get(i - 1)
                        .and_then(|r| r.get(j - 2))
                        .copied()
                        .unwrap_or(0);
                    put(&mut pred, i, j, v);
                }
            }
        }
        IntraMode::Tm => {
            for i in 0..size {
                for j in 0..size {
                    let v = above(j as isize) + left(i) - above(-1);
                    put(&mut pred, i, j, P::clip(v, bit_depth).get());
                }
            }
        }
        IntraMode::Dc => {
            let above_sum: i64 = (0..s).map(above).sum();
            let left_sum: i64 = (0..size).map(left).sum();
            let avg = match (edges.have_left, edges.have_above) {
                (true, true) => (above_sum + left_sum + s as i64) >> (log2 + 1),
                (true, false) => (left_sum + (1 << (log2 - 1))) >> log2,
                (false, true) => (above_sum + (1 << (log2 - 1))) >> log2,
                (false, false) => 1 << (bit_depth - 1),
            };
            for i in 0..size {
                for j in 0..size {
                    put(&mut pred, i, j, avg);
                }
            }
        }
    }
    for i in 0..size {
        let row = pred.get(i).copied().unwrap_or([0; 32]);
        for (p, v) in dst.row_mut(i).iter_mut().zip(row) {
            *p = P::clip(v, bit_depth);
        }
    }
}

fn put(pred: &mut [[i64; 32]; 32], i: usize, j: usize, v: i64) {
    if let Some(p) = pred.get_mut(i).and_then(|r| r.get_mut(j)) {
        *p = v;
    }
}
