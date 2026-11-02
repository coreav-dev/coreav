//! Checked Apple encoder output readers and a native AAC adapter.
//!
//! The metadata readers validate AAC-LC configuration, packet extents, declared
//! drain counts, and supported H.264 parameter sets and picture headers. They do
//! not decode compressed slices or AAC packets. On macOS, AudioToolbox's
//! synchronous AudioCodec API is owned behind a safe streaming interface.

pub mod aac;
#[cfg(target_os = "macos")]
pub mod audiotoolbox;
pub mod h264;
