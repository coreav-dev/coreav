//! The decoder's hot-path module: its only `unsafe`, compiled
//! with the feature `fast` alone (`docs/design/h264.md`, "Kernel round: the
//! hot paths compiled twice", and `docs/unsafe-sites.md`).
//!
//! Two sites. `v3-entry`: the x86-64-v3 instance of the hot paths
//! ([`Tier`]). Each function of [`v3`] is a safe function with
//! `#[target_feature]` whose body is the baseline instance's, inlined, so
//! that the compiler builds it for the v3 level: two per-block functions
//! with their own arguments, and `enter`, which runs a closure over any
//! other function's body. [`V3`]'s implementation of [`Tier`] calls them.
//! Such a call is sound where the CPU has every feature the attribute
//! enables, and a [`V3`] exists only where [`V3::detect`] found them all:
//! it has no other constructor.
//!
//! The second, `decision` ([`decision`]): the v3 instance's DecodeDecision,
//! one `asm!` block (`docs/design/h264.md`, "Entropy round: DecodeDecision
//! in assembly"). Nothing else here is `unsafe`.

#![allow(unsafe_code)]

use coreav_kernels_h264::{Kernels, Macroblocks, Ops, Part, Planes};

use crate::cabac::{self, Cabac, Category, Engine, Source};
use crate::error::Result;
use crate::inter::Target;
use crate::mb::SliceDecoder;
use crate::picture::Motion;
use crate::recon::CoeffScale;
use crate::residual::MAX_COEFFS;
use crate::tier::Tier;

mod decision;

/// The x86-64-v3 instance. Holding one is the evidence that the CPU has
/// every feature of [`v3`]'s functions' `#[target_feature]`.
#[derive(Clone, Copy)]
pub(crate) struct V3 {
    _detected: (),
}

impl V3 {
    /// A [`V3`] where the CPU has AVX, AVX2, BMI1, BMI2, F16C, FMA, LZCNT,
    /// MOVBE, POPCNT, SSE3, SSSE3, SSE4.1 and SSE4.2, the features
    /// [`v3`]'s functions are compiled with.
    pub(crate) fn detect() -> Option<V3> {
        let all = std::is_x86_feature_detected!("avx")
            && std::is_x86_feature_detected!("avx2")
            && std::is_x86_feature_detected!("bmi1")
            && std::is_x86_feature_detected!("bmi2")
            && std::is_x86_feature_detected!("f16c")
            && std::is_x86_feature_detected!("fma")
            && std::is_x86_feature_detected!("lzcnt")
            && std::is_x86_feature_detected!("movbe")
            && std::is_x86_feature_detected!("popcnt")
            && std::is_x86_feature_detected!("sse3")
            && std::is_x86_feature_detected!("ssse3")
            && std::is_x86_feature_detected!("sse4.1")
            && std::is_x86_feature_detected!("sse4.2");
        all.then_some(V3 { _detected: () })
    }
}

/// The v3 instances. Safe functions: calling one from code without the
/// same features needs `unsafe`, which only [`V3`]'s methods below do.
mod v3 {
    use super::*;

    #[target_feature(
        enable = "avx,avx2,bmi1,bmi2,f16c,fma,lzcnt,movbe,popcnt,sse3,ssse3,sse4.1,sse4.2"
    )]
    #[inline]
    pub(super) fn enter<R>(f: impl FnOnce() -> R) -> R {
        f()
    }

    #[target_feature(
        enable = "avx,avx2,bmi1,bmi2,f16c,fma,lzcnt,movbe,popcnt,sse3,ssse3,sse4.1,sse4.2"
    )]
    pub(super) fn residual<const N: usize>(
        t: V3,
        c: &mut Cabac<'_>,
        cat: Category,
        max: usize,
        out: &mut [i32; N],
        scan: &[u8; MAX_COEFFS],
        scale: &CoeffScale<N>,
    ) -> Result<u8> {
        cabac::residual_body::<N, V3>(t, c, cat, max, out, scan, scale)
    }

    #[target_feature(
        enable = "avx,avx2,bmi1,bmi2,f16c,fma,lzcnt,movbe,popcnt,sse3,ssse3,sse4.1,sse4.2"
    )]
    pub(super) fn predict_fast<K: Kernels>(
        t: V3,
        d: &mut SliceDecoder<'_, K>,
        list: usize,
        m: &Motion,
        target: Target<'_>,
        part: Part,
    ) -> Result<()> {
        d.predict_fast_body(t, list, m, target, part)
    }
}

// Site `v3-entry`. Each call below is to a function of `v3`, compiled with
// the features `V3::detect` checked; `self` is a `V3`, so the check passed.
impl Tier for V3 {
    #[inline(always)]
    fn enter<R>(self, f: impl FnOnce() -> R) -> R {
        // SAFETY: `self` proves the CPU has the features (`V3::detect`).
        unsafe { v3::enter(f) }
    }

    #[inline(always)]
    fn decision(self, e: &mut Engine, state: &mut u8, src: &mut Source<'_>) -> u32 {
        decision::decision(self, e, state, src)
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
        // SAFETY: `self` proves the CPU has the features (`V3::detect`).
        unsafe { v3::residual::<N>(self, c, cat, max, out, scan, scale) }
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
        // SAFETY: `self` proves the CPU has the features (`V3::detect`).
        unsafe { v3::predict_fast(self, d, list, m, target, part) }
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
        k.predict_part_inlined(dst, mbs, mb, part, src, mv, ops)
    }
}
