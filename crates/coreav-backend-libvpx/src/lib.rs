//! VP9 decoding with libvpx (`docs/design/decode.md`, "libvpx").
//!
//! libvpx (BSD-3) is reached through CoreAV's own declarations of its
//! decoder API, in `ffi`, the only module of the crate that may use
//! `unsafe` (`docs/unsafe-sites.md`). It is built only with the `libvpx`
//! feature, which links the system's libvpx. [`frame_sizes`] needs no
//! library: it reads the frame sizes a packet states, in safe Rust, so
//! that a frame over the decode limits is refused before libvpx allocates
//! it. [`LibvpxBackend`] decodes in the calling process; `coreav-worker`
//! runs the same decoder inside its sandbox, which is how the CLI uses it.
#![deny(unsafe_code)]

mod header;

pub use header::{FrameSize, Malformed, frame_sizes};

#[cfg(feature = "libvpx")]
mod decoder;
#[cfg(feature = "libvpx")]
#[allow(unsafe_code)]
mod ffi;

#[cfg(feature = "libvpx")]
pub use decoder::{LibvpxBackend, LibvpxDecoder, LibvpxOptions};
