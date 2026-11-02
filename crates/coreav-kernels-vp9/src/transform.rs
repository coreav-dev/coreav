//! The inverse transforms (clause 8.7) and the add of their output to the
//! prediction (step 4 of clause 8.6.2).

use crate::tables::{COS64_LOOKUP, SINPI_1_9, SINPI_2_9, SINPI_3_9, SINPI_4_9};
use crate::{BlockMut, Pixel, TxSize, round2};

/// `TxType`: the 1D transform of the columns, then of the rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxType {
    /// `DCT_DCT`.
    DctDct,
    /// `ADST_DCT`: an ADST of the columns, a DCT of the rows.
    AdstDct,
    /// `DCT_ADST`: a DCT of the columns, an ADST of the rows.
    DctAdst,
    /// `ADST_ADST`.
    AdstAdst,
}

/// The array `T` of clause 8.7.1 and the higher-precision `S` of the
/// ADSTs, of up to 32 values.
struct Arrays {
    t: [i64; 32],
    s: [i64; 32],
}

impl Arrays {
    fn t(&self, i: usize) -> i64 {
        self.t.get(i).copied().unwrap_or(0)
    }

    fn set_t(&mut self, i: usize, v: i64) {
        if let Some(t) = self.t.get_mut(i) {
            *t = v;
        }
    }

    fn s(&self, i: usize) -> i64 {
        self.s.get(i).copied().unwrap_or(0)
    }

    fn set_s(&mut self, i: usize, v: i64) {
        if let Some(s) = self.s.get_mut(i) {
            *s = v;
        }
    }

    /// `B(a, b, angle, flip)`.
    fn b(&mut self, a: usize, b: usize, angle: i32, flip: bool) {
        let (ta, tb) = (self.t(a), self.t(b));
        let x = ta * cos64(angle) - tb * sin64(angle);
        let y = ta * sin64(angle) + tb * cos64(angle);
        let (x, y) = (round2(x, 14), round2(y, 14));
        let (x, y) = if flip { (y, x) } else { (x, y) };
        self.set_t(a, x);
        self.set_t(b, y);
    }

    /// `H(a, b, flip)`.
    fn h(&mut self, a: usize, b: usize, flip: bool) {
        let (a, b) = if flip { (b, a) } else { (a, b) };
        let (x, y) = (self.t(a), self.t(b));
        self.set_t(a, x + y);
        self.set_t(b, x - y);
    }

    /// `SB(a, b, angle, flip)`.
    fn sb(&mut self, a: usize, b: usize, angle: i32, flip: bool) {
        let (ta, tb) = (self.t(a), self.t(b));
        let x = ta * cos64(angle) - tb * sin64(angle);
        let y = ta * sin64(angle) + tb * cos64(angle);
        let (x, y) = if flip { (y, x) } else { (x, y) };
        self.set_s(a, x);
        self.set_s(b, y);
    }

    /// `SH(a, b)`.
    fn sh(&mut self, a: usize, b: usize) {
        let (sa, sb) = (self.s(a), self.s(b));
        self.set_t(a, round2(sa + sb, 14));
        self.set_t(b, round2(sa - sb, 14));
    }

    /// The inverse DCT array permutation process (clause 8.7.1.2).
    fn dct_permutation(&mut self, n: u32) {
        let copy = self.t;
        for i in 0..1usize << n {
            self.set_t(i, copy.get(brev(n, i)).copied().unwrap_or(0));
        }
    }

    /// The inverse DCT process (clause 8.7.1.3) of the permuted array.
    fn dct(&mut self, n: u32) {
        let n_ = n as usize;
        let n0 = 1usize << n;
        let n1 = n0 >> 1;
        let n2 = n0 >> 2;
        let n3 = n0 >> 3;
        if n == 2 {
            self.b(0, 1, 16, true);
        } else {
            self.dct(n - 1);
        }
        for i in 0..n2 {
            self.b(n1 + i, n0 - 1 - i, 32 - brev(5, n1 + i) as i32, false);
        }
        if n >= 3 {
            for i in 0..n3 {
                for j in 0..2 {
                    self.h(n1 + 4 * i + 2 * j, n1 + 1 + 4 * i + 2 * j, j == 1);
                }
            }
        }
        if n == 5 {
            for i in 0..2 {
                for j in 0..2 {
                    let angle = 28 - 16 * i as i32 + 56 * j as i32;
                    self.b(
                        n0 - n_ + 3 - n2 * j - 4 * i,
                        n1 + n_ - 4 + n2 * j + 4 * i,
                        angle,
                        true,
                    );
                }
            }
            for i in 0..2 {
                for j in 0..4 {
                    self.h(n1 + n3 * j + i, n1 + n2 - 5 + n3 * j - i, j & 1 == 1);
                }
            }
        }
        if n >= 4 {
            for i in 0..=usize::from(n == 5) {
                for j in 0..2 {
                    self.b(
                        n0 - n_ + 2 - i - n2 * j,
                        n1 + n_ - 3 + i + n2 * j,
                        24 + 48 * j as i32,
                        true,
                    );
                }
            }
            for i in 0..=2 * n_ - 7 {
                for j in 0..2 {
                    self.h(n1 + n2 * j + i, n1 + n2 - 1 + n2 * j - i, j & 1 == 1);
                }
            }
        }
        if n >= 3 {
            for i in 0..n3 {
                self.b(n0 - n3 - 1 - i, n1 + n3 + i, 16, true);
            }
        }
        for i in 0..n1 {
            self.h(i, n0 - 1 - i, false);
        }
    }

    /// The inverse ADST input array permutation process (clause 8.7.1.4).
    fn adst_input_permutation(&mut self, n: u32) {
        let copy = self.t;
        let n0 = 1usize << n;
        for i in 0..n0 / 2 {
            self.set_t(2 * i, copy.get(n0 - 1 - 2 * i).copied().unwrap_or(0));
            self.set_t(2 * i + 1, copy.get(2 * i).copied().unwrap_or(0));
        }
    }

    /// The inverse ADST output array permutation process (clause 8.7.1.5).
    fn adst_output_permutation(&mut self, n: u32) {
        let copy = self.t;
        let at = |i: usize| copy.get(i).copied().unwrap_or(0);
        if n == 4 {
            for a in 0..2 {
                for b in 0..2 {
                    for c in 0..2 {
                        for d in 0..2 {
                            self.set_t(
                                8 * a + 4 * b + 2 * c + d,
                                at(8 * (d ^ c) + 4 * (c ^ b) + 2 * (b ^ a) + a),
                            );
                        }
                    }
                }
            }
        } else {
            for a in 0..2 {
                for b in 0..2 {
                    for c in 0..2 {
                        self.set_t(4 * a + 2 * b + c, at(4 * (c ^ b) + 2 * (b ^ a) + a));
                    }
                }
            }
        }
    }

    /// The inverse ADST4 process (clause 8.7.1.6).
    fn adst4(&mut self) {
        let (t0, t1, t2, t3) = (self.t(0), self.t(1), self.t(2), self.t(3));
        let s0 = SINPI_1_9 * t0;
        let s1 = SINPI_2_9 * t0;
        let s2 = SINPI_3_9 * t1;
        let s3 = SINPI_4_9 * t2;
        let s4 = SINPI_1_9 * t2;
        let s5 = SINPI_2_9 * t3;
        let s6 = SINPI_4_9 * t3;
        let v = t0 - t2 + t3;
        let s7 = SINPI_3_9 * v;
        let x0 = s0 + s3 + s5;
        let x1 = s1 - s4 - s6;
        let x2 = s7;
        let x3 = s2;
        self.set_t(0, round2(x0 + x3, 14));
        self.set_t(1, round2(x1 + x3, 14));
        self.set_t(2, round2(x2, 14));
        self.set_t(3, round2(x0 + x1 - x3, 14));
    }

    /// The inverse ADST8 process (clause 8.7.1.7).
    fn adst8(&mut self) {
        self.adst_input_permutation(3);
        for i in 0..4 {
            self.sb(2 * i, 1 + 2 * i, 30 - 8 * i as i32, true);
        }
        for i in 0..4 {
            self.sh(i, 4 + i);
        }
        for i in 0..2 {
            self.sb(4 + 3 * i, 5 + i, 24 - 16 * i as i32, true);
        }
        for i in 0..2 {
            self.sh(4 + i, 6 + i);
        }
        for i in 0..2 {
            self.h(i, 2 + i, false);
        }
        for i in 0..2 {
            self.b(2 + 4 * i, 3 + 4 * i, 16, true);
        }
        self.adst_output_permutation(3);
        for i in 0..4 {
            self.set_t(1 + 2 * i, -self.t(1 + 2 * i));
        }
    }

    /// The inverse ADST16 process (clause 8.7.1.8).
    fn adst16(&mut self) {
        self.adst_input_permutation(4);
        for i in 0..8 {
            self.sb(2 * i, 1 + 2 * i, 31 - 4 * i as i32, true);
        }
        for i in 0..8 {
            self.sh(i, 8 + i);
        }
        for i in 0..4 {
            self.sb(8 + 2 * i, 9 + 2 * i, 28 - 16 * i as i32, true);
        }
        for i in 0..4 {
            self.sh(8 + i, 12 + i);
        }
        for i in 0..4 {
            self.h(i, 4 + i, false);
        }
        for i in 0..2 {
            for j in 0..2 {
                self.sb(4 + 8 * i + 3 * j, 5 + 8 * i + j, 24 - 16 * j as i32, true);
            }
        }
        for i in 0..2 {
            for j in 0..2 {
                self.sh(4 + 8 * j + i, 6 + 8 * j + i);
            }
        }
        for i in 0..2 {
            for j in 0..2 {
                self.h(8 * j + i, 2 + 8 * j + i, false);
            }
        }
        for i in 0..2 {
            for j in 0..2 {
                self.b(
                    2 + 4 * j + 8 * i,
                    3 + 4 * j + 8 * i,
                    48 + 64 * (i ^ j) as i32,
                    false,
                );
            }
        }
        self.adst_output_permutation(4);
        for i in 0..2 {
            for j in 0..2 {
                self.set_t(1 + 12 * j + 2 * i, -self.t(1 + 12 * j + 2 * i));
            }
        }
    }

    /// The inverse Walsh-Hadamard transform process (clause 8.7.1.10).
    fn wht(&mut self, shift: u32) {
        let mut a = self.t(0) >> shift;
        let mut c = self.t(1) >> shift;
        let mut d = self.t(2) >> shift;
        let mut b = self.t(3) >> shift;
        a += c;
        d -= b;
        let e = (a - d) >> 1;
        b = e - b;
        c = e - c;
        a -= b;
        d += c;
        self.set_t(0, a);
        self.set_t(1, b);
        self.set_t(2, c);
        self.set_t(3, d);
    }

    /// One 1D transform of `T`: the WHT when `lossless`, else a DCT or an
    /// ADST (clause 8.7.2).
    fn transform(&mut self, n: u32, adst: bool, lossless: bool, wht_shift: u32) {
        if lossless {
            self.wht(wht_shift);
        } else if !adst {
            self.dct_permutation(n);
            self.dct(n);
        } else {
            match n {
                2 => self.adst4(),
                3 => self.adst8(),
                _ => self.adst16(),
            }
        }
    }
}

/// `cos64(angle)` (clause 8.7.1.1).
fn cos64(angle: i32) -> i64 {
    let angle2 = (angle & 127) as usize;
    let at = |i: usize| i64::from(COS64_LOOKUP.get(i).copied().unwrap_or(0));
    match angle2 {
        0..=32 => at(angle2),
        33..=64 => -at(64 - angle2),
        65..=96 => -at(angle2 - 64),
        _ => at(128 - angle2),
    }
}

/// `sin64(angle)` (clause 8.7.1.1).
fn sin64(angle: i32) -> i64 {
    cos64(angle - 32)
}

/// `brev(numBits, x)` (clause 8.7.1.1).
fn brev(num_bits: u32, x: usize) -> usize {
    (0..num_bits).fold(0, |t, i| t | ((x >> i) & 1) << (num_bits - 1 - i))
}

/// The 2D inverse transform (clause 8.7.2) of `coefs`, the dequantized
/// coefficients of a block of `dst.size()` by `dst.size()` in raster
/// order (`Dequant`), added to the prediction in `dst` with `Clip1`
/// (step 4 of clause 8.6.2). `coefs` holds the transform's output
/// afterwards. A 32x32 block takes the DCT whatever `tx_type` says, since
/// `get_scan` (clause 6.4.25) gives it `DCT_DCT` only; the WHT of a
/// `lossless` frame applies to 4x4 blocks, its only size (`tx_mode` is
/// `ONLY_4X4`), and a larger block takes the DCT or ADST instead.
pub fn inverse_transform_add<P: Pixel>(
    mut dst: BlockMut<'_, P>,
    coefs: &mut [i64],
    tx_type: TxType,
    lossless: bool,
    bit_depth: u8,
) {
    let tx_size = dst.tx_size();
    let n0 = tx_size.samples();
    let n = tx_size.log2();
    let lossless = lossless && tx_size == TxSize::Tx4x4;
    let adst = tx_size != TxSize::Tx32x32;
    let row_adst = adst && matches!(tx_type, TxType::DctAdst | TxType::AdstAdst);
    let col_adst = adst && matches!(tx_type, TxType::AdstDct | TxType::AdstAdst);
    let mut a = Arrays {
        t: [0; 32],
        s: [0; 32],
    };
    for row in coefs.chunks_mut(n0).take(n0) {
        a.t = [0; 32];
        for (t, c) in a.t.iter_mut().zip(row.iter()) {
            *t = *c;
        }
        a.transform(n, row_adst, lossless, 2);
        for (c, t) in row.iter_mut().zip(a.t.iter()) {
            *c = *t;
        }
    }
    for j in 0..n0 {
        a.t = [0; 32];
        for i in 0..n0 {
            a.set_t(i, coefs.get(i * n0 + j).copied().unwrap_or(0));
        }
        a.transform(n, col_adst, lossless, 0);
        for i in 0..n0 {
            let v = if lossless {
                a.t(i)
            } else {
                round2(a.t(i), 6.min(n + 2))
            };
            if let Some(c) = coefs.get_mut(i * n0 + j) {
                *c = v;
            }
        }
    }
    for (i, row) in coefs.chunks(n0).take(n0).enumerate() {
        for (p, c) in dst.row_mut(i).iter_mut().zip(row) {
            *p = P::clip(p.get() + c, bit_depth);
        }
    }
}

#[cfg(test)]
mod tests;
