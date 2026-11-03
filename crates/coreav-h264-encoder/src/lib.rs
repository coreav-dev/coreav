//! Safe scalar, intra-only H.264 encoding from Rec. ITU-T H.264 (08/2024).
//!
//! The `full` feature is required. This crate is absent from default product
//! builds. H.264 encoding may require a patent licence in your jurisdiction.
//! There is no IO, thread, runtime SIMD dispatch, or source-format conversion.
#![forbid(unsafe_code)]

#[cfg(feature = "full")]
mod api;
#[cfg(feature = "full")]
mod bits;
#[cfg(feature = "full")]
mod cavlc;
#[cfg(feature = "full")]
mod codec;
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
