//! Opus encoding with libopus (`docs/design/transcode.md`, "Opus audio").
//!
//! libopus (BSD-3) is reached through the `opus` crate and its
//! `audiopus_sys` binding, so this crate has no `unsafe` of its own
//! (policy record 0042). It is built only with the `libopus` feature,
//! which links the system's libopus; without it the crate is empty.
//! [`OpusEncoder`] encodes in the calling process; `coreav-worker` runs it
//! in a sandboxed process of its own, which is how the CLI uses it.

#[cfg(feature = "libopus")]
mod encoder;

#[cfg(feature = "libopus")]
pub use encoder::{FRAME_SAMPLES, OpusEncoder, SAMPLE_RATE};
