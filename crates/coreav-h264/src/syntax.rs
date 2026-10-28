//! The syntax elements of the macroblock layer, read with either entropy
//! coder (clauses 7.3.5, 9.2 and 9.3).
//!
//! The macroblock layer calls this trait and never the coders directly, so
//! that the two coders differ only in how an element is read, not in what
//! the layer does with it.
#![forbid(unsafe_code)]

use core::marker::PhantomData;

use coreav_kernels_h264::Kernels;

use crate::cabac::{self, Bin, CATEGORIES, Cabac};
use crate::cavlc;
use crate::error::{Error, Result, require};
use crate::mb::{BLK_XY, MbKind, SliceDecoder};
use crate::nb;
use crate::recon::CoeffScale;
use crate::residual::{Blk, MAX_COEFFS};
use crate::slice::SliceType;
use crate::tables as t;
use crate::tier::Tier;

/// One syntax element reader.
pub(crate) trait Syntax {
    /// Whether this is the CABAC reader, which codes 8x8 blocks as one
    /// block instead of four interleaved 4x4 lists.
    fn is_cabac(&self) -> bool;
    fn mb_type<K: Kernels>(&mut self, d: &SliceDecoder<K>) -> Result<u32>;
    fn transform8x8<K: Kernels>(&mut self, d: &SliceDecoder<K>) -> Result<bool>;
    fn sub_mb_type<K: Kernels>(&mut self, d: &SliceDecoder<K>) -> Result<u8>;
    fn prev_intra_flag(&mut self) -> Result<bool>;
    fn rem_intra_mode(&mut self) -> Result<u8>;
    fn chroma_pred_mode<K: Kernels>(&mut self, d: &SliceDecoder<K>) -> Result<u8>;
    /// ref_idx_lX of the partition whose first 4x4 block is (bx, by).
    fn ref_idx<K: Kernels>(
        &mut self,
        d: &SliceDecoder<K>,
        list: usize,
        bx: usize,
        by: usize,
    ) -> Result<u32>;
    /// mvd_lX of that partition, horizontal then vertical.
    fn mvd<K: Kernels>(
        &mut self,
        d: &SliceDecoder<K>,
        list: usize,
        bx: usize,
        by: usize,
    ) -> Result<[i32; 2]>;
    fn cbp<K: Kernels>(&mut self, d: &SliceDecoder<K>, intra: bool) -> Result<u8>;
    fn qp_delta<K: Kernels>(&mut self, d: &SliceDecoder<K>) -> Result<i32>;
    /// What reading residual block `kind` needs from its neighbours, taken
    /// before the block is read so that the block can be written into the
    /// macroblock in place: coded_block_flag's ctxIdxInc for CABAC, nC for
    /// CAVLC.
    fn block_context<K: Kernels>(&self, d: &SliceDecoder<K>, kind: Blk) -> i32;
    /// One residual block of maxNumCoeff `max`, its level at scanning
    /// position `k` scaled by `scale` and written to `out[scan[k]]`; the
    /// rest of `out` is left alone. Returns the number of non-zero
    /// coefficients (TotalCoeff for CAVLC).
    fn block<const N: usize>(
        &mut self,
        kind: Blk,
        context: i32,
        out: &mut [i32; N],
        scan: &[u8; MAX_COEFFS],
        max: usize,
        scale: &CoeffScale<N>,
    ) -> Result<u8>;
    /// The 384 samples of an I_PCM macroblock.
    fn pcm(&mut self, samples: &mut [u8; 384]) -> Result<()>;
    /// residual() (clause 7.3.5.3) of the macroblock `d` is decoding
    /// ([`SliceDecoder::parse_residual`]), in the instance `tier`.
    fn residual<K: Kernels, U: Tier>(&mut self, d: &mut SliceDecoder<K>, tier: U) -> Result<()>;
    /// Reports data the entropy decoder ran past the end of
    /// ([`Cabac::check`]).
    fn check(&self) -> Result<()> {
        Ok(())
    }
}

/// The CAVLC reader: every syntax element of the slice data, from
/// mb_skip_run to the last residual block, through one window
/// ([`cavlc::Window`]) that the slice loop makes once a slice and holds by
/// value, as a local, so that its state stays in registers. Every method
/// is inlined into the loop; the residual, out of line in its instance,
/// takes the window by value and gives it back ([`Syntax::residual`]), and
/// what runs on a cold path takes it by value or takes its fields, so that
/// nothing takes the window by reference across a call.
pub(crate) struct CavlcSyntax<'a, T> {
    pub(crate) w: cavlc::Window<'a>,
    /// The instance of the hot paths the slice runs, which the residual's
    /// blocks are inlined into.
    tier: PhantomData<T>,
}

impl<'a, T> CavlcSyntax<'a, T> {
    #[inline(always)]
    pub(crate) fn new(w: cavlc::Window<'a>, _tier: T) -> Self {
        CavlcSyntax {
            w,
            tier: PhantomData,
        }
    }
}

/// nC of a block from its neighbours' TotalCoeff (clause 9.2.1).
fn predicted_nc(a: Option<i32>, b: Option<i32>) -> i32 {
    match (a, b) {
        (Some(a), Some(b)) => (a + b + 1) >> 1,
        (Some(v), None) | (None, Some(v)) => v,
        (None, None) => 0,
    }
}

/// [`predicted_nc`] of the TotalCoeff of blocks A and B as a cache holds
/// them, [`nb::NA`] for one that is not available.
#[inline]
fn cached_nc((a, b): (u8, u8)) -> i32 {
    let of = |v: u8| (v != nb::NA).then_some(i32::from(v));
    predicted_nc(of(a), of(b))
}

impl<T: Tier> Syntax for CavlcSyntax<'_, T> {
    #[inline(always)]
    fn is_cabac(&self) -> bool {
        false
    }

    #[inline(always)]
    fn mb_type<K: Kernels>(&mut self, _d: &SliceDecoder<K>) -> Result<u32> {
        self.w.ue_max(48, "mb_type")
    }

    #[inline(always)]
    fn transform8x8<K: Kernels>(&mut self, _d: &SliceDecoder<K>) -> Result<bool> {
        Ok(self.w.flag())
    }

    #[inline(always)]
    fn sub_mb_type<K: Kernels>(&mut self, _d: &SliceDecoder<K>) -> Result<u8> {
        Ok(self.w.ue_max(12, "sub_mb_type")? as u8)
    }

    #[inline(always)]
    fn prev_intra_flag(&mut self) -> Result<bool> {
        Ok(self.w.flag())
    }

    #[inline(always)]
    fn rem_intra_mode(&mut self) -> Result<u8> {
        Ok(self.w.u(3) as u8)
    }

    #[inline(always)]
    fn chroma_pred_mode<K: Kernels>(&mut self, _d: &SliceDecoder<K>) -> Result<u8> {
        Ok(self.w.ue_max(3, "intra_chroma_pred_mode")? as u8)
    }

    #[inline(always)]
    fn ref_idx<K: Kernels>(
        &mut self,
        d: &SliceDecoder<K>,
        list: usize,
        _bx: usize,
        _by: usize,
    ) -> Result<u32> {
        let active = d.hdr.num_ref_idx_active.get(list).copied().unwrap_or(1);
        self.w.te(active.saturating_sub(1))
    }

    #[inline(always)]
    fn mvd<K: Kernels>(
        &mut self,
        _d: &SliceDecoder<K>,
        _list: usize,
        _bx: usize,
        _by: usize,
    ) -> Result<[i32; 2]> {
        Ok([self.w.se()?, self.w.se()?])
    }

    #[inline(always)]
    fn cbp<K: Kernels>(&mut self, _d: &SliceDecoder<K>, intra: bool) -> Result<u8> {
        let code = self.w.ue_max(47, "coded_block_pattern")? as usize;
        let table = if intra { t::CBP_INTRA } else { t::CBP_INTER };
        table
            .get(code)
            .copied()
            .ok_or(Error::Invalid("coded_block_pattern"))
    }

    #[inline(always)]
    fn qp_delta<K: Kernels>(&mut self, _d: &SliceDecoder<K>) -> Result<i32> {
        self.w.se()
    }

    #[inline(always)]
    fn block_context<K: Kernels>(&self, d: &SliceDecoder<K>, kind: Blk) -> i32 {
        match kind {
            Blk::ChromaDc(_) => -1,
            Blk::ChromaAc(c, b) => {
                let cache = d.nb.tc_c.get(c).map_or(&[][..], |c| &c[..]);
                cached_nc(nb::pair(cache, nb::at_c(b), nb::WC))
            }
            Blk::LumaDc | Blk::LumaAc(_) | Blk::Luma4x4(_) | Blk::Luma8x8(_) => {
                let blk = match kind {
                    Blk::LumaAc(b) | Blk::Luma4x4(b) => b,
                    _ => 0,
                };
                let (bx, by) = BLK_XY.get(blk & 15).copied().unwrap_or((0, 0));
                cached_nc(nb::pair(&d.nb.tc, nb::at(bx, by), nb::W))
            }
        }
    }

    #[inline(always)]
    fn block<const N: usize>(
        &mut self,
        _kind: Blk,
        context: i32,
        out: &mut [i32; N],
        scan: &[u8; MAX_COEFFS],
        max: usize,
        scale: &CoeffScale<N>,
    ) -> Result<u8> {
        cavlc::block_in(&mut self.w, context, out, scan, max, scale)
    }

    #[inline(always)]
    fn pcm(&mut self, samples: &mut [u8; 384]) -> Result<()> {
        let (w, read) = self.w.pcm(samples);
        self.w = w;
        read
    }

    /// Out of line in `tier`, with the window moved in and back
    /// ([`SliceDecoder::residual_cavlc`]).
    #[inline(always)]
    fn residual<K: Kernels, U: Tier>(&mut self, d: &mut SliceDecoder<K>, tier: U) -> Result<()> {
        let mut back = self.w;
        let read = d.residual_cavlc(tier, self.w, &mut back);
        self.w = back;
        read
    }

    #[inline(always)]
    fn check(&self) -> Result<()> {
        self.w.check()
    }
}

/// The CABAC reader.
pub(crate) struct CabacSyntax<'c, 'a, T> {
    pub(crate) c: &'c mut Cabac<'a>,
    /// The instance of the hot paths the slice runs.
    pub(crate) t: T,
}

impl<T: Tier> CabacSyntax<'_, '_, T> {
    /// The intra mb_type bin string of Table 9-36, as a prefix of its own
    /// or as the suffix of a P or B mb_type.
    fn intra_mb_type(&mut self, base: usize, inc0: usize, suffix: bool) -> Result<u32> {
        let v = self.c.table(self.t, &cabac::BINS_MB_TYPE_I, |b, bins| {
            if suffix {
                match b {
                    0 => Bin::Ctx(base),
                    1 => Bin::Terminate,
                    2 => Bin::Ctx(base + 1),
                    3 => Bin::Ctx(base + 2),
                    4 => Bin::Ctx(base + if bins & 1 != 0 { 2 } else { 3 }),
                    _ => Bin::Ctx(base + 3),
                }
            } else {
                match b {
                    0 => Bin::Ctx(base + inc0),
                    1 => Bin::Terminate,
                    2 => Bin::Ctx(base + 3),
                    3 => Bin::Ctx(base + 4),
                    4 => Bin::Ctx(base + if bins & 1 != 0 { 5 } else { 6 }),
                    5 => Bin::Ctx(base + if bins & 2 != 0 { 6 } else { 7 }),
                    _ => Bin::Ctx(base + 7),
                }
            }
        })?;
        Ok(u32::from(v.max(0) as u8))
    }
}

impl<T: Tier> Syntax for CabacSyntax<'_, '_, T> {
    fn is_cabac(&self) -> bool {
        true
    }

    fn mb_type<K: Kernels>(&mut self, d: &SliceDecoder<K>) -> Result<u32> {
        match d.hdr.slice_type {
            SliceType::I => {
                // ctxIdxInc of the first bin (clause 9.3.3.1.1.3).
                let other = |m: Option<usize>| {
                    m.is_some_and(|m| {
                        let kind = d.info(m).kind;
                        kind != MbKind::I4x4 && kind != MbKind::I8x8
                    })
                };
                let inc = usize::from(other(d.mb_a())) + usize::from(other(d.mb_b()));
                self.intra_mb_type(3, inc, false)
            }
            SliceType::P | SliceType::Sp => {
                let v = self
                    .c
                    .table(self.t, &cabac::BINS_MB_TYPE_P, |b, bins| match b {
                        0 => Bin::Ctx(14),
                        1 => Bin::Ctx(15),
                        _ => Bin::Ctx(14 + if bins & 1 != 1 { 2 } else { 3 }),
                    })?;
                if v >= 0 {
                    Ok(v as u32)
                } else {
                    Ok(5 + self.intra_mb_type(17, 0, true)?)
                }
            }
            SliceType::B => {
                let coded = |m: Option<usize>| {
                    m.is_some_and(|m| !matches!(d.info(m).kind, MbKind::BSkip | MbKind::B(0)))
                };
                let inc = usize::from(coded(d.mb_a())) + usize::from(coded(d.mb_b()));
                let v = self
                    .c
                    .table(self.t, &cabac::BINS_MB_TYPE_B, |b, bins| match b {
                        0 => Bin::Ctx(27 + inc),
                        1 => Bin::Ctx(27 + 3),
                        2 => Bin::Ctx(27 + if bins & 1 != 0 { 4 } else { 5 }),
                        _ => Bin::Ctx(27 + 5),
                    })?;
                if v >= 0 {
                    Ok(v as u32)
                } else {
                    Ok(23 + self.intra_mb_type(32, 0, true)?)
                }
            }
            SliceType::Si => Err(Error::Unsupported("SI slices")),
        }
    }

    fn transform8x8<K: Kernels>(&mut self, d: &SliceDecoder<K>) -> Result<bool> {
        let uses = |m: Option<usize>| m.is_some_and(|m| d.info(m).t8x8);
        let inc = usize::from(uses(d.mb_a())) + usize::from(uses(d.mb_b()));
        Ok(self.c.decision(self.t, 399 + inc) == 1)
    }

    fn sub_mb_type<K: Kernels>(&mut self, d: &SliceDecoder<K>) -> Result<u8> {
        let v = if d.hdr.slice_type.is_b() {
            self.c
                .table(self.t, &cabac::BINS_SUB_MB_TYPE_B, |b, bins| match b {
                    0 => Bin::Ctx(36),
                    1 => Bin::Ctx(37),
                    2 => Bin::Ctx(36 + if bins & 1 != 0 { 2 } else { 3 }),
                    _ => Bin::Ctx(39),
                })?
        } else {
            self.c
                .table(self.t, &cabac::BINS_SUB_MB_TYPE_P, |b, _| Bin::Ctx(21 + b))?
        };
        Ok(v.max(0) as u8)
    }

    fn prev_intra_flag(&mut self) -> Result<bool> {
        Ok(self.c.decision(self.t, 68) == 1)
    }

    fn rem_intra_mode(&mut self) -> Result<u8> {
        let b0 = self.c.decision(self.t, 69);
        let b1 = self.c.decision(self.t, 69);
        let b2 = self.c.decision(self.t, 69);
        Ok((b0 | (b1 << 1) | (b2 << 2)) as u8)
    }

    fn chroma_pred_mode<K: Kernels>(&mut self, d: &SliceDecoder<K>) -> Result<u8> {
        // Only an intra macroblock other than I_PCM leaves a chroma_mode
        // (`NbInfo`).
        let predicts = |m: Option<usize>| m.is_some_and(|m| d.neighbour(m).chroma_mode != 0);
        let inc = usize::from(predicts(d.mb_a())) + usize::from(predicts(d.mb_b()));
        let v = self
            .c
            .unary(self.t, Some(3), |b| if b == 0 { 64 + inc } else { 67 })?;
        Ok(v as u8)
    }

    fn ref_idx<K: Kernels>(
        &mut self,
        d: &SliceDecoder<K>,
        list: usize,
        bx: usize,
        by: usize,
    ) -> Result<u32> {
        // ctxIdxInc of the first bin (clause 9.3.3.1.1.6).
        let above_zero = |above: bool| -> bool {
            let Some((m, r)) = d.neighbour_4x4(bx, by, above) else {
                return false;
            };
            // A skipped or intra macroblock leaves no bit set (`NbInfo`).
            let b8 = ((r / 4) / 2) * 2 + (r % 4) / 2;
            d.nb_info(m).ref_gt0 >> ((4 * list + b8) & 7) & 1 != 0
        };
        let inc = usize::from(above_zero(false)) + 2 * usize::from(above_zero(true));
        let v = self.c.unary(self.t, Some(32), |b| match b {
            0 => 54 + inc,
            1 => 58,
            _ => 59,
        })?;
        require(v < 32, "ref_idx")?;
        Ok(v)
    }

    fn mvd<K: Kernels>(
        &mut self,
        d: &SliceDecoder<K>,
        list: usize,
        bx: usize,
        by: usize,
    ) -> Result<[i32; 2]> {
        // ctxIdxInc of each component's first bin (clause 9.3.3.1.1.7).
        // The cache holds 0 for a neighbour that is not available, skipped
        // or intra.
        let (a, b) =
            d.nb.mvd
                .get(list)
                .map_or((0, 0), |c| nb::mvd_pair(c, nb::at(bx, by)));
        let inc = |shift: u32| {
            let sum = u32::from((a >> shift) & 255) + u32::from((b >> shift) & 255);
            if sum > 32 {
                2
            } else if sum > 2 {
                1
            } else {
                0
            }
        };
        self.c.mvd(self.t, [inc(0), inc(8)])
    }

    fn cbp<K: Kernels>(&mut self, d: &SliceDecoder<K>, _intra: bool) -> Result<u8> {
        // coded_block_pattern, luma bit by luma bit (clause 9.3.3.1.1.4).
        let mut luma = 0u8;
        for b8 in 0..4usize {
            let (bx, by) = ((b8 % 2) * 2, (b8 / 2) * 2);
            let empty = |above: bool, decoded: u8| -> usize {
                let Some((m, r)) = d.neighbour_4x4(bx, by, above) else {
                    return 0;
                };
                let nb8 = ((r / 4) / 2) * 2 + (r % 4) / 2;
                if m == d.addr {
                    // A block of this macroblock, decoded a moment ago.
                    return usize::from(decoded & (1 << nb8) == 0);
                }
                // An I_PCM macroblock leaves every bit set and a skipped
                // one none (`NbInfo`).
                usize::from(d.neighbour(m).cbp & (1 << nb8) == 0)
            };
            let inc = empty(false, luma) + 2 * empty(true, luma);
            luma |= (self.c.decision(self.t, 73 + inc) as u8) << b8;
        }
        let chroma_of = |m: Option<usize>, bin: u32| -> usize {
            let Some(m) = m else { return 0 };
            // CodedBlockPatternChroma 2 for an I_PCM macroblock, 0 for a
            // skipped one (`NbInfo`).
            let c = d.neighbour(m).cbp >> 4;
            usize::from(if bin == 0 { c != 0 } else { c == 2 })
        };
        let (a, b) = (d.mb_a(), d.mb_b());
        let mut chroma = 0u8;
        if self
            .c
            .decision(self.t, 77 + chroma_of(a, 0) + 2 * chroma_of(b, 0))
            == 1
        {
            chroma = 1;
            if self
                .c
                .decision(self.t, 77 + 4 + chroma_of(a, 1) + 2 * chroma_of(b, 1))
                == 1
            {
                chroma = 2;
            }
        }
        Ok(luma | (chroma << 4))
    }

    fn qp_delta<K: Kernels>(&mut self, d: &SliceDecoder<K>) -> Result<i32> {
        let inc0 = usize::from(d.prev_qp_delta_nonzero);
        let v = self.c.unary(self.t, Some(53), |b| match b {
            0 => 60 + inc0,
            1 => 62,
            _ => 63,
        })?;
        require(v <= 52, "mb_qp_delta")?;
        let v = v as i32;
        // The mapping of clause 9.3.2.7.
        Ok(if v % 2 == 1 { (v + 1) / 2 } else { -(v / 2) })
    }

    #[inline(always)]
    fn block_context<K: Kernels>(&self, d: &SliceDecoder<K>, kind: Blk) -> i32 {
        d.coded_block_flag_inc(kind) as i32
    }

    /// Inlined into the macroblock layer's loops, so that an empty block,
    /// most of them, costs its coded_block_flag and no call; the
    /// coefficients are decoded out of line ([`Cabac::residual`]).
    #[inline(always)]
    fn block<const N: usize>(
        &mut self,
        kind: Blk,
        context: i32,
        out: &mut [i32; N],
        scan: &[u8; MAX_COEFFS],
        max: usize,
        scale: &CoeffScale<N>,
    ) -> Result<u8> {
        let cat = kind.category();
        let contexts = CATEGORIES
            .get(cat)
            .copied()
            .ok_or(Error::Invalid("ctxBlockCat"))?;
        // coded_block_flag (clause 9.3.3.1.1.9), which an 8x8 block of a
        // 4:2:0 picture does not have.
        if cat != 5
            && self
                .c
                .decision(self.t, usize::from(contexts.cbf) + context as usize)
                == 0
        {
            return Ok(0);
        }
        self.c.residual(self.t, contexts, max, out, scan, scale)
    }

    fn check(&self) -> Result<()> {
        self.c.check()
    }

    fn pcm(&mut self, samples: &mut [u8; 384]) -> Result<()> {
        self.c.check()?;
        let mut r = self.c.pcm_reader();
        while !r.byte_aligned() {
            require(r.u(1)? == 0, "pcm_alignment_zero_bit")?;
        }
        for s in samples.iter_mut() {
            *s = r.u(8)? as u8;
        }
        self.c.resume(&r)
    }

    /// In place: the CABAC slice loop runs in `tier` already.
    #[inline(always)]
    fn residual<K: Kernels, U: Tier>(&mut self, d: &mut SliceDecoder<K>, _tier: U) -> Result<()> {
        d.parse_residual_blocks(self)
    }
}
