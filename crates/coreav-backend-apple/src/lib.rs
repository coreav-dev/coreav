//! Checked Apple encoder output readers and native AAC/H.264/HEVC adapters.
//!
//! The metadata readers validate AAC-LC configuration, packet extents, declared
//! drain counts, and supported H.264 parameter sets and picture headers. They do
//! not decode compressed slices or AAC packets. On macOS, AudioToolbox's
//! synchronous AudioCodec API and asynchronous VideoToolbox callbacks are owned
//! behind safe, owner-thread-only interfaces with bounded source/output ledgers.

pub mod aac;
#[cfg(target_os = "macos")]
pub mod audiotoolbox;
pub mod h264;
pub mod hevc;
#[cfg(target_os = "macos")]
pub mod videotoolbox;
