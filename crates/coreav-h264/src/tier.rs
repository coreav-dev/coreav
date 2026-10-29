//! The instances of the hot paths (`docs/design/h264.md`, "Kernel round:
//! the hot paths compiled twice" and "Kernel round: the hot paths compiled
//! twice, wider"). The functions that gain from a newer x86-64 level are
//! compiled twice, once for the baseline and once for x86-64-v3; the
//! instance a slice runs is chosen once, before its first macroblock, and
//! every call below that is static. [`Tier`] runs a function's body in an
//! instance ([`Tier::enter`]), and names the per-block functions that have
//! instances of their own; [`Base`] is the baseline instance, the only one
//! a build without the feature `fast`, or for another architecture, has.
//! The v3 instance is in `fast`.
//!
//! The callers between the slice loop and those functions take a
//! `T: Tier`, so that each instance has its own copy of the chain; so do
//! the helpers that the chain's functions call from one place, unused,
//! so that each instance's copy still has one caller and the compiler
//! inlines it as it does with one instance.
#![forbid(unsafe_code)]

use coreav_kernels_h264::{Kernels, Macroblocks, Ops, Part, Planes};

use crate::cabac::{self, Cabac, Category, Engine, Source};
use crate::error::Result;
use crate::inter::Target;
use crate::mb::SliceDecoder;
use crate::params::{LevelScale, Pps};
use crate::picture::Motion;
use crate::recon::CoeffScale;
use crate::residual::MAX_COEFFS;

/// One instance of the hot paths. A value of the type is what a slice
/// passes down to their callers; holding one is the evidence that its
/// instance may run on this CPU.
pub(crate) trait Tier: Copy {
    /// Runs `f` in this instance. The baseline runs it in place; the v3
    /// instance runs it in a function compiled for x86-64-v3, into which
    /// `f`, an `#[inline(always)]` closure over a function's body, is
    /// inlined, and which the compiler may inline into a caller of the
    /// same instance. For functions called once a slice or once a
    /// macroblock, with few arguments: the closure's captures are passed
    /// in memory where the function stays out of line.
    fn enter<R>(self, f: impl FnOnce() -> R) -> R;

    /// DecodeDecision against the context state `state`
    /// ([`Engine::decision`], which the baseline runs and the v3 instance
    /// replaces with the site `decision`).
    fn decision(self, e: &mut Engine, state: &mut u8, src: &mut Source<'_>) -> u32;

    /// A coded residual block ([`cabac::residual`]).
    fn residual<const N: usize>(
        self,
        c: &mut Cabac<'_>,
        cat: Category,
        max: usize,
        out: &mut [i32; N],
        scan: &[u8; MAX_COEFFS],
        scale: &CoeffScale<N>,
    ) -> Result<u8>;

    /// One list's prediction of a block ([`SliceDecoder::predict_fast`]).
    fn predict_fast<K: Kernels>(
        self,
        d: &mut SliceDecoder<'_, K>,
        list: usize,
        m: &Motion,
        target: Target<'_>,
        part: Part,
    ) -> Result<()>;

    /// [`Kernels::predict_part`], from [`SliceDecoder::predict_fast`].
    #[allow(clippy::too_many_arguments)]
    fn predict_kernel<K: Kernels>(
        self,
        k: &K,
        dst: [&mut [u8]; 3],
        mbs: &Macroblocks,
        mb: [usize; 2],
        part: Part,
        src: &Planes<'_>,
        mv: [i16; 2],
        ops: Ops,
    ) -> bool;
}

/// The baseline instance: the functions as the build compiles everything
/// else.
#[derive(Clone, Copy)]
pub(crate) struct Base;

impl Tier for Base {
    #[inline(always)]
    fn enter<R>(self, f: impl FnOnce() -> R) -> R {
        f()
    }

    #[inline(always)]
    fn decision(self, e: &mut Engine, state: &mut u8, src: &mut Source<'_>) -> u32 {
        e.decision(state, src)
    }

    #[inline(always)]
    fn residual<const N: usize>(
        self,
        c: &mut Cabac<'_>,
        cat: Category,
        max: usize,
        out: &mut [i32; N],
        scan: &[u8; MAX_COEFFS],
        scale: &CoeffScale<N>,
    ) -> Result<u8> {
        cabac::residual::<N>(c, cat, max, out, scan, scale)
    }

    #[inline(always)]
    fn predict_fast<K: Kernels>(
        self,
        d: &mut SliceDecoder<'_, K>,
        list: usize,
        m: &Motion,
        target: Target<'_>,
        part: Part,
    ) -> Result<()> {
        d.predict_fast(list, m, target, part)
    }

    #[inline(always)]
    fn predict_kernel<K: Kernels>(
        self,
        k: &K,
        dst: [&mut [u8]; 3],
        mbs: &Macroblocks,
        mb: [usize; 2],
        part: Part,
        src: &Planes<'_>,
        mv: [i16; 2],
        ops: Ops,
    ) -> bool {
        k.predict_part(dst, mbs, mb, part, src, mv, ops)
    }
}

/// The instance a decoder's slices run, chosen once when the decoder is
/// made: the v3 one where the build has it and the CPU has every feature
/// it is compiled for, unless the caller asked for the baseline.
#[derive(Clone, Copy, Default)]
pub(crate) struct Choice {
    #[cfg(all(feature = "fast", target_arch = "x86_64"))]
    v3: Option<crate::fast::V3>,
}

impl Choice {
    /// Detects the CPU's features where `allow_v3`, or chooses the
    /// baseline.
    pub(crate) fn detect(allow_v3: bool) -> Choice {
        #[cfg(all(feature = "fast", target_arch = "x86_64"))]
        {
            Choice {
                v3: if allow_v3 {
                    crate::fast::V3::detect()
                } else {
                    None
                },
            }
        }
        #[cfg(not(all(feature = "fast", target_arch = "x86_64")))]
        {
            let _ = allow_v3;
            Choice {}
        }
    }
}

impl Choice {
    /// [`LevelScale::new`], in the chosen instance: once a slice.
    pub(crate) fn level_scale(self, pps: &Pps) -> Box<LevelScale> {
        #[cfg(all(feature = "fast", target_arch = "x86_64"))]
        if let Some(v3) = self.v3 {
            return v3.enter(
                #[inline(always)]
                || LevelScale::new_body(pps),
            );
        }
        LevelScale::new(pps)
    }
}

impl<K: Kernels> SliceDecoder<'_, K> {
    /// slice_data() (clause 7.3.4), in the instance `choice` names.
    pub(crate) fn decode(&mut self, rbsp: &[u8], choice: Choice) -> Result<()> {
        #[cfg(all(feature = "fast", target_arch = "x86_64"))]
        if let Some(v3) = choice.v3 {
            return self.decode_slice_data(v3, rbsp);
        }
        let _ = choice;
        self.decode_slice_data(Base, rbsp)
    }
}
