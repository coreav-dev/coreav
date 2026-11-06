//! H.264 encoding from Rec. ITU-T H.264 (08/2024).
//!
//! The `full` feature is required. This crate is absent from default product
//! builds. H.264 encoding may require a patent licence in your jurisdiction.
//! Runtime SIMD dispatch preserves scalar output. The `scalar` feature forces
//! the scalar kernels. Unsafe arithmetic is confined to the private `simd`
//! module; there is no IO, thread, or source-format conversion.
#![deny(unsafe_code)]

#[cfg(feature = "full")]
mod api;
#[cfg(feature = "full")]
mod bits;
#[cfg(feature = "full")]
mod cavlc;
#[cfg(feature = "full")]
mod codec;
#[cfg(feature = "full")]
mod deblock;
#[cfg(feature = "full")]
mod kernels;
#[cfg(feature = "full")]
mod motion;
#[cfg(all(feature = "full", target_arch = "x86_64", not(feature = "scalar")))]
mod simd;
#[cfg(feature = "full")]
mod syntax;
#[cfg(feature = "full")]
mod transform;
#[cfg(feature = "full")]
mod vlc;
#[cfg(feature = "full")]
pub use api::*;

#[cfg(all(test, feature = "full"))]
mod tests;
