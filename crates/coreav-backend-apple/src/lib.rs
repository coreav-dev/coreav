//! Checked Apple encoder output readers and a native AAC adapter.
//!
//! The metadata readers validate AAC-LC configuration, packet extents, and
//! declared drain counts, not compressed AAC syntax. On macOS, AudioToolbox's
//! synchronous AudioCodec API is owned behind a safe streaming interface.

pub mod aac;
#[cfg(target_os = "macos")]
pub mod audiotoolbox;
