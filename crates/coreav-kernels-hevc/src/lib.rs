//! Allocation-free kernels for Main and Main10 HEVC reconstruction and filters.
//!
//! Enable `native-hevc` explicitly. The caller owns bitstream syntax, effective
//! QP/scaling-list derivation, neighbour availability and picture storage.
//! Algorithms and integer tables follow ITU-T H.265 V11 (01/2026), clauses
//! 8.4.4, 8.5.3.3.3–8.5.3.3.4, 8.6.2–8.6.7 and 8.7.
//! Range-extension tools are not implemented.

#![no_std]
#![cfg_attr(feature = "scalar", forbid(unsafe_code))]

#[cfg(feature = "native-hevc")]
#[forbid(unsafe_code)]
mod domain;
#[cfg(feature = "native-hevc")]
#[forbid(unsafe_code)]
mod filter;
#[cfg(feature = "native-hevc")]
#[forbid(unsafe_code)]
mod inter;
#[cfg(feature = "native-hevc")]
#[forbid(unsafe_code)]
mod intra;
#[cfg(feature = "native-hevc")]
#[forbid(unsafe_code)]
mod inverse;
#[cfg(feature = "native-hevc")]
#[forbid(unsafe_code)]
mod tables;

#[cfg(all(
    feature = "native-hevc",
    not(feature = "scalar"),
    target_arch = "x86_64"
))]
extern crate std;
#[cfg(all(
    feature = "native-hevc",
    not(feature = "scalar"),
    target_arch = "x86_64"
))]
#[allow(unsafe_code)]
mod simd;

#[cfg(feature = "native-hevc")]
pub use domain::{
    BitDepth, Block, BlockMut, BlockSize, Component, Error, IntraMode, Rect, RectMut,
    chroma_qp_index,
};
#[cfg(feature = "native-hevc")]
pub use filter::{
    BoundaryStrength, SaoClass, deblock_chroma, deblock_luma, deblock_luma_with_strength,
    sao_sample,
};
#[cfg(all(feature = "native-hevc", feature = "test-support"))]
#[doc(hidden)]
pub use inter::interpolate_inter_scalar;
#[cfg(feature = "native-hevc")]
pub use inter::{
    InterPrediction, MotionVector, PredictionWeight, RefPlane, interpolate_inter, predict_bi,
    predict_uni, weight_bi, weight_uni,
};
#[cfg(feature = "native-hevc")]
pub use intra::{IntraPrediction, predict_intra};
#[cfg(feature = "native-hevc")]
pub use inverse::{InverseTransform, Reconstruction, add_residual, inverse_residual};

#[cfg(feature = "native-hevc")]
const REFERENCE_SAMPLES: usize = 129;

/// Upper bound on fixed numerical arrays inside one kernel call, excluding inputs and outputs.
#[cfg(feature = "native-hevc")]
pub const SCRATCH_BYTES: usize = {
    let inverse = core::mem::size_of::<inverse::Scratch<32>>()
        + 2 * core::mem::size_of::<[i32; REFERENCE_SAMPLES]>();
    let inter = core::mem::size_of::<inter::InterScratch>();
    if inverse > inter { inverse } else { inter }
};

#[cfg(all(
    test,
    feature = "native-hevc",
    any(feature = "scalar", not(target_arch = "x86_64"))
))]
extern crate std;
#[cfg(all(test, feature = "native-hevc"))]
mod inter_tests;
#[cfg(all(test, feature = "native-hevc"))]
mod tests;
