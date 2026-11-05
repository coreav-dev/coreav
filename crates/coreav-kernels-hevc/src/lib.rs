//! Allocation-free scalar kernels for Main and Main10 HEVC intra reconstruction.
//!
//! Enable `native-hevc` explicitly. The caller owns bitstream syntax, effective
//! QP/scaling-list derivation, neighbour availability and picture storage.
//! Algorithms and integer tables follow ITU-T H.265 V11 (01/2026), clauses
//! 8.4.4 and 8.6.2–8.6.7. Range-extension tools are not implemented.

#![no_std]

#[cfg(feature = "native-hevc")]
mod domain;
#[cfg(feature = "native-hevc")]
mod intra;
#[cfg(feature = "native-hevc")]
mod inverse;
#[cfg(feature = "native-hevc")]
mod tables;

#[cfg(feature = "native-hevc")]
pub use domain::{BitDepth, Block, BlockMut, BlockSize, Component, Error, IntraMode};
#[cfg(feature = "native-hevc")]
pub use intra::{IntraPrediction, predict_intra};
#[cfg(feature = "native-hevc")]
pub use inverse::{InverseTransform, Reconstruction, add_residual, inverse_residual};

#[cfg(feature = "native-hevc")]
const REFERENCE_SAMPLES: usize = 129;

/// Upper bound on fixed numerical arrays inside one kernel call, excluding inputs and outputs.
#[cfg(feature = "native-hevc")]
pub const SCRATCH_BYTES: usize =
    core::mem::size_of::<inverse::Scratch>() + 2 * core::mem::size_of::<[i32; REFERENCE_SAMPLES]>();

#[cfg(all(test, feature = "native-hevc"))]
extern crate std;
#[cfg(all(test, feature = "native-hevc"))]
mod tests;
