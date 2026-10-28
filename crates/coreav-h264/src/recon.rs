//! Reconstruction: intra prediction into the picture, scaling of the
//! coefficients (clause 8.5.9 and 8.5.12.1) and the inverse transforms
//! (clauses 8.5.10 to 8.5.13).
#![forbid(unsafe_code)]

use coreav_kernels_h264::{BlockMut, ChromaIntraMut, IntraAvail, IntraMut, Kernels};

use crate::error::Result;
use crate::mb::{MbKind, SliceDecoder, raster};
use crate::params::{LevelScale, Pps};
use crate::tables as t;

/// The range a scaled coefficient has to be in (clauses 8.5.10, 8.5.11.2,
/// 8.5.12.1 and 8.5.13.1): -2^(7 + bitDepth) to 2^(7 + bitDepth) - 1, which
/// is -32768 to 32767 for 8-bit samples.
///
/// A conforming stream never leaves it. Values are clipped rather than
/// rejected so that one bad block does not end a picture, and so that every
/// kernel — including a later one that works in 16-bit lanes — is handed
/// coefficients it can transform without overflow.
const COEFF_MIN: i64 = -32768;
const COEFF_MAX: i64 = 32767;

/// `(coeff * scale) << shift`, clipped to the range above.
#[inline(always)]
fn scale_up(coeff: i32, scale: i32, shift: u32) -> i32 {
    let v = (i64::from(coeff) * i64::from(scale)) << shift.min(16);
    v.clamp(COEFF_MIN, COEFF_MAX) as i32
}

/// `(coeff * scale + round) >> shift`, clipped to the range above.
#[inline(always)]
fn scale_down(coeff: i32, scale: i32, round: i32, shift: u32) -> i32 {
    let v = (i64::from(coeff) * i64::from(scale) + i64::from(round)) >> shift.min(16);
    v.clamp(COEFF_MIN, COEFF_MAX) as i32
}

/// The scaling of one kind of residual block at one QP (clauses 8.5.12.1
/// and 8.5.13.1), which the entropy decoder applies to each level as it
/// stores it ([`crate::syntax::Syntax::block`]).
///
/// Scaling as levels are stored costs one multiplication per non-zero
/// level; a pass over the block afterwards costs one per position, 16 or
/// 64, most of them zero. Every QP takes one form, with the shift `r` of
/// the block size (4 for a 4x4 block, 6 for an 8x8 one):
/// `(level * (LevelScale << (qP / 6)) + 2^(r - 1)) >> r`. Below
/// `qP / 6 = r` the clause rounds and shifts right by `r - qP / 6`; the
/// form multiplies numerator and denominator by `2^(qP / 6)`, which leaves
/// the quotient and so its floor unchanged. From `qP / 6 = r` up the
/// clause shifts left by `qP / 6 - r`; the form's product is that value
/// times `2^r`, and the `2^(r - 1)` it adds is below `2^r`, so the floor
/// is the same. The products are exact in 64 bits (a parser's levels are
/// below 2^16 and a factor below 2^22), the round and the shift are
/// constants of the block size, and values are clipped to the range
/// above, as before. The DC levels of an Intra16x16 or chroma DC block
/// have scaling of their own after their transform, and take
/// [`CoeffScale::IDENTITY`], which only clips them to that range. A
/// conforming stream's DC levels are inside it already: clauses 8.5.10
/// and 8.5.11.2 bound the values their transforms take, so only a
/// non-conforming stream sees the clip.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CoeffScale<const N: usize> {
    /// Per raster position: LevelScale4x4 or LevelScale8x8 for the QP,
    /// shifted left by `qP / 6`.
    factor: [i32; N],
}

impl<const N: usize> CoeffScale<N> {
    /// The shift of the block size: 6 for an 8x8 block, 4 otherwise.
    const SHIFT: u32 = if N == 64 { 6 } else { 4 };

    /// Levels stored as they are decoded: `(level * 2^r + 2^(r - 1)) >> r`
    /// is `level`.
    pub(crate) const IDENTITY: CoeffScale<N> = CoeffScale {
        factor: [1 << Self::SHIFT; N],
    };

    /// The scaling of a block whose scaling list for `qP % 6` is `scales`,
    /// at `qp` (Equations 8-336 and 8-337, and 8-356 and 8-357).
    pub(crate) fn new(scales: &[i32; N], qp: i32) -> CoeffScale<N> {
        let q = (qp.clamp(0, 51) / 6) as u32;
        CoeffScale {
            factor: scales.map(|f| f.saturating_mul(1 << q)),
        }
    }

    /// The coefficient at raster position `at` for `level`.
    #[inline(always)]
    pub(crate) fn apply(&self, at: usize, level: i32) -> i32 {
        let f = self.factor.get(at).copied().unwrap_or(0);
        let v = (i64::from(level) * i64::from(f) + (1 << (Self::SHIFT - 1))) >> Self::SHIFT;
        v.clamp(COEFF_MIN, COEFF_MAX) as i32
    }
}

/// QPs 0 to 51, one table entry each.
const QPS: usize = 52;

/// The scalings of every QP for one picture parameter set's scaling lists
/// and chroma QP offsets, built when a slice's lists differ from the last
/// slice's, so that a macroblock only picks its entries
/// ([`SliceDecoder::prepare_level_scales`]): QPs change from one macroblock
/// to the next in many streams, and building a scaling costs a copy of 16
/// or 64 factors. Indexed by intra (0) or inter (1), then QP_Y; the chroma
/// entries by component as well, and by QP_Y rather than QP'C, so that
/// QP'C is derived only when the tables are built.
pub(crate) struct CoeffTables {
    luma4: [[CoeffScale<16>; QPS]; 2],
    luma8: [[CoeffScale<64>; QPS]; 2],
    chroma: [[[CoeffScale<16>; QPS]; 2]; 2],
    /// What the tables were built from.
    key: Option<TablesKey>,
}

/// The parts of a picture parameter set the scalings depend on.
#[derive(Clone, Copy, PartialEq, Eq)]
struct TablesKey {
    scaling4x4: [[u8; 16]; 6],
    scaling8x8: [[u8; 64]; 2],
    chroma_qp_index_offset: [i32; 2],
}

impl CoeffTables {
    /// Tables built for no lists yet, boxed: they hold about 47 KB.
    pub(crate) fn new() -> Box<CoeffTables> {
        Box::new(CoeffTables {
            luma4: [[CoeffScale::IDENTITY; QPS]; 2],
            luma8: [[CoeffScale::IDENTITY; QPS]; 2],
            chroma: [[[CoeffScale::IDENTITY; QPS]; 2]; 2],
            key: None,
        })
    }

    /// Makes the tables those of `pps`, whose level scales are `scale`;
    /// they are built again only if the lists or offsets differ from those
    /// they were built from.
    pub(crate) fn fit(&mut self, pps: &Pps, scale: &LevelScale) {
        let [l0, l1, ..] = pps.scaling8x8;
        let key = TablesKey {
            scaling4x4: pps.scaling4x4,
            scaling8x8: [l0, l1],
            chroma_qp_index_offset: pps.chroma_qp_index_offset,
        };
        if self.key == Some(key) {
            return;
        }
        let pick4 = |list: usize, qp: i32| {
            scale
                .l4
                .get(list)
                .and_then(|l| l.get((qp % 6) as usize))
                .map_or(CoeffScale::IDENTITY, |s| CoeffScale::new(s, qp))
        };
        for inter in 0..2 {
            for qp in 0..QPS as i32 {
                let at = qp as usize;
                if let Some(e) = self.luma4.get_mut(inter).and_then(|t| t.get_mut(at)) {
                    *e = pick4(3 * inter, qp);
                }
                if let Some(e) = self.luma8.get_mut(inter).and_then(|t| t.get_mut(at)) {
                    *e = scale
                        .l8
                        .get(inter)
                        .and_then(|l| l.get((qp % 6) as usize))
                        .map_or(CoeffScale::IDENTITY, |s| CoeffScale::new(s, qp));
                }
                for c in 0..2 {
                    let qpc = qp_c(pps, c, qp);
                    if let Some(e) = self
                        .chroma
                        .get_mut(inter)
                        .and_then(|t| t.get_mut(c))
                        .and_then(|t| t.get_mut(at))
                    {
                        *e = pick4(1 + 3 * inter + c, qpc);
                    }
                }
            }
        }
        self.key = Some(key);
    }
}

/// The scalings of the current macroblock's blocks, entries of
/// [`CoeffTables`].
#[derive(Clone, Copy)]
pub(crate) struct LevelScales<'a> {
    pub(crate) luma4: &'a CoeffScale<16>,
    pub(crate) luma8: &'a CoeffScale<64>,
    /// Cb then Cr.
    pub(crate) chroma: [&'a CoeffScale<16>; 2],
}

impl LevelScales<'_> {
    /// Before the first macroblock picks its entries.
    pub(crate) const NONE: LevelScales<'static> = LevelScales {
        luma4: &CoeffScale::IDENTITY,
        luma8: &CoeffScale::IDENTITY,
        chroma: [&CoeffScale::IDENTITY; 2],
    };
}

/// QP'C of chroma component `c` at QP_Y `qp` (clause 8.5.8).
fn qp_c(pps: &Pps, c: usize, qp: i32) -> i32 {
    let offset = pps.chroma_qp_index_offset.get(c).copied().unwrap_or(0);
    let index = (qp + offset).clamp(0, 51);
    i32::from(t::QPC.get(index as usize).copied().unwrap_or(0))
}

impl<'a, K: Kernels> SliceDecoder<'a, K> {
    /// Points [`SliceDecoder::level_scales`] at the entries of the current
    /// macroblock's QP for the blocks its residual codes.
    #[inline(always)]
    pub(crate) fn prepare_level_scales(&mut self, chroma: bool) {
        let inter = usize::from(!self.mb.kind.is_intra());
        let qp = self.qp as usize;
        let tables: &'a CoeffTables = self.coeff_tables;
        if self.mb.t8x8 {
            self.level_scales.luma8 = tables
                .luma8
                .get(inter)
                .and_then(|t| t.get(qp))
                .unwrap_or(&CoeffScale::IDENTITY);
        } else {
            self.level_scales.luma4 = tables
                .luma4
                .get(inter)
                .and_then(|t| t.get(qp))
                .unwrap_or(&CoeffScale::IDENTITY);
        }
        if chroma {
            let row = tables.chroma.get(inter);
            for (c, slot) in self.level_scales.chroma.iter_mut().enumerate() {
                *slot = row
                    .and_then(|t| t.get(c))
                    .and_then(|t| t.get(qp))
                    .unwrap_or(&CoeffScale::IDENTITY);
            }
        }
    }

    /// QP'C of chroma component `c` (clause 8.5.8).
    fn chroma_qp(&self, c: usize) -> i32 {
        qp_c(self.pps, c, self.qp)
    }

    /// The offset in the luma plane of the sample (bx, by) of the current
    /// macroblock.
    fn luma_off(&self, bx: usize, by: usize) -> usize {
        self.luma_origin + (self.mby * 16 + by - self.luma_first) * self.stride + self.mbx * 16 + bx
    }

    /// The offset in a chroma plane of the sample (bx, by) of the current
    /// macroblock.
    fn chroma_off(&self, bx: usize, by: usize) -> usize {
        let row = self.mby * 8 + by - self.chroma_first;
        self.chroma_origin + row * (self.stride / 2) + self.mbx * 8 + bx
    }

    /// Whether an intra block may predict from macroblocks A, B, C and D
    /// (clause 6.4.11.4 with constrained_intra_pred_flag), in that order:
    /// each available and, under constrained intra prediction, intra.
    fn intra_neighbours(&self) -> [bool; 4] {
        self.nbs.map(|m| m.is_some_and(|m| self.intra_allowed(m)))
    }

    /// The samples an intra prediction of the whole macroblock may predict
    /// from: those of macroblocks A, B, C and D. The kernels derive each
    /// Intra_4x4 and Intra_8x8 block's own from them.
    fn mb_avail(nb: [bool; 4]) -> IntraAvail {
        let [a, b, c, d] = nb;
        IntraAvail {
            left: a,
            top: b,
            top_right: c,
            top_left: d,
        }
    }

    /// Intra prediction and residual of the current macroblock
    /// (clauses 8.3 and 8.5).
    pub(crate) fn reconstruct_intra(&mut self) -> Result<()> {
        let stride = self.stride;
        let qp = self.qp;
        match self.mb.kind {
            MbKind::I4x4 => {
                let avail = Self::mb_avail(self.intra_neighbours());
                let off = self.luma_off(0, 0);
                let dst = IntraMut::new(&mut self.out.y, off, stride, 16, avail)?;
                let nz = (self.mb.luma_nz & 0xffff) as u16;
                self.kernels
                    .intra4x4_mb(dst, &self.mb.modes, &self.mb.coeff, nz);
            }
            MbKind::I8x8 => {
                let avail = Self::mb_avail(self.intra_neighbours());
                let off = self.luma_off(0, 0);
                let dst = IntraMut::new(&mut self.out.y, off, stride, 16, avail)?;
                let modes = [0, 1, 2, 3].map(|b8| self.mb.modes.get(b8).copied().unwrap_or(2));
                let nz = ((self.mb.luma_nz >> 16) & 0xf) as u8;
                self.kernels.intra8x8_mb(dst, &modes, &self.mb.coeff8, nz);
            }
            MbKind::I16x16 => {
                let avail = Self::mb_avail(self.intra_neighbours());
                let off = self.luma_off(0, 0);
                let i16_mode = self.mb.i16_mode;
                let dst = IntraMut::new(&mut self.out.y, off, stride, 16, avail)?;
                self.kernels.intra16x16(dst, i16_mode);
                if self.mb.dc_nz {
                    // Each block's DC goes into its coefficients, which an
                    // AC block leaves at 0 (it codes from position 1), and
                    // a block with a DC is transformed whether or not it
                    // has AC levels; `luma_nz` records it, so that
                    // `MbData::reset` clears the block again.
                    let dc = self.luma_dc(qp);
                    for (blk, block) in self.mb.coeff.iter_mut().enumerate() {
                        let value = dc.get(raster(blk)).copied().unwrap_or(0);
                        block[0] = value;
                        if value != 0 {
                            self.mb.luma_nz |= 1 << blk;
                        }
                    }
                }
                self.residual_4x4_luma()?;
            }
            // I_PCM wrote its samples already; nothing else is intra.
            _ => return Ok(()),
        }
        // A chroma block predicts from no macroblock C.
        let avail = IntraAvail {
            top_right: false,
            ..Self::mb_avail(self.intra_neighbours())
        };
        self.reconstruct_chroma(Some((self.mb.chroma_mode, avail)))
    }

    /// The Intra_16x16 DC block: the inverse transform of clause 8.5.10 and
    /// its scaling.
    fn luma_dc(&self, qp: i32) -> [i32; 16] {
        let c = &self.mb.dc;
        // The 4x4 inverse Hadamard transform (Equation 8-320), rows then
        // columns.
        let mut rows = [[0i32; 4]; 4];
        for (out, inp) in rows.iter_mut().zip(c.chunks_exact(4)) {
            let [a, b, cc, d] = <[i32; 4]>::try_from(inp).unwrap_or_default();
            *out = [
                a + b + cc + d,
                a + b - cc - d,
                a - b - cc + d,
                a - b + cc - d,
            ];
        }
        let [r0, r1, r2, r3] = rows;
        let column = |j: usize| {
            let pick = |r: [i32; 4]| r.get(j).copied().unwrap_or(0);
            let (a, b, c, d) = (pick(r0), pick(r1), pick(r2), pick(r3));
            [a + b + c + d, a + b - c - d, a - b - c + d, a - b + c - d]
        };
        let scale = self
            .scale
            .l4
            .first()
            .and_then(|l| l.get((qp % 6) as usize))
            .and_then(|s| s.first())
            .copied()
            .unwrap_or(0);
        let mut out = [0i32; 16];
        for j in 0..4 {
            let col = column(j);
            for (i, &f) in col.iter().enumerate() {
                let v = if qp >= 36 {
                    scale_up(f, scale, (qp / 6 - 6).max(0) as u32)
                } else {
                    let round = 1 << (5 - qp / 6).clamp(0, 30);
                    scale_down(f, scale, round, (6 - qp / 6).max(0) as u32)
                };
                if let Some(slot) = out.get_mut(i * 4 + j) {
                    *slot = v;
                }
            }
        }
        out
    }

    /// The residual of an inter macroblock (clause 8.5).
    pub(crate) fn reconstruct_inter(&mut self) -> Result<()> {
        let stride = self.stride;
        if self.mb.t8x8 {
            for b8 in 0..4 {
                if self.mb.luma_nz & (1 << (16 + b8)) == 0 {
                    continue;
                }
                let Some(coeff) = self.mb.coeff8.get(b8) else {
                    continue;
                };
                let off = self.luma_off((b8 % 2) * 8, (b8 / 2) * 8);
                let block = BlockMut::new(&mut self.out.y, off, stride, 8, 8)?;
                self.kernels.idct8x8_add(block, coeff);
            }
        } else {
            self.residual_4x4_luma()?;
        }
        self.reconstruct_chroma(None)
    }

    /// The 4x4 luma residual blocks of the current macroblock whose bit is
    /// set in `luma_nz`, an 8x8 quadrant a call (luma4x4BlkIdx runs over the
    /// quadrants, four blocks each).
    fn residual_4x4_luma(&mut self) -> Result<()> {
        let stride = self.stride;
        for q in 0..4 {
            let nz = ((self.mb.luma_nz >> (4 * q)) & 0xf) as u8;
            if nz == 0 {
                continue;
            }
            let Some(coeffs) = self
                .mb
                .coeff
                .get(4 * q..4 * q + 4)
                .and_then(|c| <&[[i32; 16]; 4]>::try_from(c).ok())
            else {
                continue;
            };
            let off = self.luma_off((q % 2) * 8, (q / 2) * 8);
            let block = BlockMut::new(&mut self.out.y, off, stride, 8, 8)?;
            self.kernels.idct4x4_add_quad(block, coeffs, nz);
        }
        Ok(())
    }

    /// The chroma of the current macroblock: its intra prediction where
    /// `intra` gives intra_chroma_pred_mode and the neighbours it may
    /// predict from (clause 8.3.4), then its residual (clauses 8.5.11 and
    /// 8.5.12), both components in one kernel call.
    fn reconstruct_chroma(&mut self, intra: Option<(u8, IntraAvail)>) -> Result<()> {
        let dc_nz = self.mb.chroma_dc_nz;
        let [cb_nz, cr_nz] = self.mb.chroma_ac_nz;
        let ac_nz = cb_nz | cr_nz << 4;
        if intra.is_none() && ac_nz == 0 && dc_nz == [false; 2] {
            return Ok(());
        }
        // The 2x2 chroma DC transform and its scaling; the kernels take each
        // value as its block's first coefficient, which leaves the AC
        // blocks as the entropy decoder wrote them (from position 1), so
        // `MbData::reset` clears what `chroma_ac_nz` records.
        let list = if intra.is_some() { 1 } else { 4 };
        let mut dc = [[0i32; 4]; 2];
        for (c, (out, &coded)) in dc.iter_mut().zip(&dc_nz).enumerate() {
            if coded {
                *out = self.chroma_dc(c, list + c);
            }
        }
        let stride = self.stride / 2;
        let off = self.chroma_off(0, 0);
        let pic = &mut self.out;
        let ac = &self.mb.chroma_ac;
        match intra {
            Some((mode, avail)) => {
                let view = ChromaIntraMut::new(&mut pic.cb, &mut pic.cr, off, stride, avail)?;
                self.kernels.intra_chroma_mb(view, mode, &dc, ac, ac_nz);
            }
            None => {
                let cb = BlockMut::new(&mut pic.cb, off, stride, 8, 8)?;
                let cr = BlockMut::new(&mut pic.cr, off, stride, 8, 8)?;
                self.kernels.chroma_residual_mb(cb, cr, &dc, ac, ac_nz);
            }
        }
        Ok(())
    }

    /// The DC coefficients of chroma component `c`'s four blocks: the 2x2
    /// transform of its DC levels (clause 8.5.11.1) scaled with scaling
    /// list `list` (8.5.11.2).
    fn chroma_dc(&self, c: usize, list: usize) -> [i32; 4] {
        let qp = self.chroma_qp(c);
        let [a, b, cc, d] = self.mb.chroma_dc.get(c).copied().unwrap_or([0; 4]);
        let (s0, s1, d0, d1) = (a + b, cc + d, a - b, cc - d);
        let f = [s0 + s1, d0 + d1, s0 - s1, d0 - d1];
        let scale = self
            .scale
            .l4
            .get(list)
            .and_then(|l| l.get((qp % 6) as usize))
            .and_then(|s| s.first())
            .copied()
            .unwrap_or(0);
        // (f * scale) << (qP / 6) as one product: the levels are clipped to
        // 16 bits and a scale is below 2^13, so it is exact in 64 bits.
        let factor = i64::from(scale) << (qp / 6).clamp(0, 16);
        f.map(|f| ((i64::from(f) * factor) >> 5).clamp(COEFF_MIN, COEFF_MAX) as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::{COEFF_MAX, COEFF_MIN, CoeffScale};

    #[test]
    fn scaling_stays_inside_the_specifications_range() {
        // An ordinary coefficient is scaled as the equations say: qP 24
        // and above shifts left (here by 2 at qP 36), below it rounds and
        // shifts right (by 4 at qP 0).
        let scales = [16; 16];
        assert_eq!(CoeffScale::new(&scales, 24).apply(0, 3), 48);
        assert_eq!(CoeffScale::new(&scales, 36).apply(5, -3), -192);
        assert_eq!(CoeffScale::new(&scales, 0).apply(0, 3), 3);
        assert_eq!(CoeffScale::new(&scales, 0).apply(15, -3), -3);
        assert_eq!(CoeffScale::<16>::IDENTITY.apply(3, -7), -7);
        // The largest level a parser accepts, with the largest scaling
        // list entry and the largest shift, would overflow 32 bits.
        let level = 1 << 15;
        let scales = [255 * 58; 16];
        assert_eq!(
            CoeffScale::new(&scales, 51).apply(0, level),
            COEFF_MAX as i32
        );
        assert_eq!(
            CoeffScale::new(&scales, 51).apply(0, -level),
            COEFF_MIN as i32
        );
        assert_eq!(
            CoeffScale::new(&scales, 18).apply(0, level),
            COEFF_MAX as i32
        );
        // An 8x8 block shifts by 6 rather than 4: left from qP 36 (by 1
        // at qP 42), right with rounding below it (by 1 at qP 30, by 6 at
        // qP 0), against Equations 8-356 and 8-357 written out.
        let mut scales8 = [0; 64];
        for (k, s) in scales8.iter_mut().enumerate() {
            *s = 16 + k as i32;
        }
        for qp in [0, 5, 30, 35, 36, 41, 42, 51] {
            let q = qp / 6;
            let scale = CoeffScale::new(&scales8, qp);
            for (k, &s) in scales8.iter().enumerate() {
                for level in [-300, -1, 1, 7, 2047] {
                    let expected = if q >= 6 {
                        (level * s) << (q - 6)
                    } else {
                        (level * s + (1 << (5 - q))) >> (6 - q)
                    }
                    .clamp(-32768, 32767);
                    assert_eq!(scale.apply(k, level), expected, "qp {qp} k {k}");
                }
            }
        }
        // A 4x4 block likewise shifts by 4 (Equations 8-336 and 8-337).
        let mut scales4 = [0; 16];
        for (k, s) in scales4.iter_mut().enumerate() {
            *s = 10 + 3 * k as i32;
        }
        for qp in [0, 5, 6, 17, 18, 23, 24, 29, 30, 51] {
            let q = qp / 6;
            let scale = CoeffScale::new(&scales4, qp);
            for (k, &s) in scales4.iter().enumerate() {
                for level in [-300, -2, -1, 1, 3, 7, 2047] {
                    let expected = if q >= 4 {
                        (level * s) << (q - 4)
                    } else {
                        (level * s + (1 << (3 - q))) >> (4 - q)
                    }
                    .clamp(-32768, 32767);
                    assert_eq!(scale.apply(k, level), expected, "qp {qp} k {k}");
                }
            }
        }
        assert_eq!(CoeffScale::<64>::IDENTITY.apply(63, -32768), -32768);
        assert_eq!(CoeffScale::<4>::IDENTITY.apply(1, 40000), COEFF_MAX as i32);
        let scales8 = [255 * 58; 64];
        assert_eq!(
            CoeffScale::new(&scales8, 51).apply(63, level),
            COEFF_MAX as i32
        );
    }
}
