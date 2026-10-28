//! AV1 decoding with dav1d (`docs/design/decode.md`, "dav1d").
//!
//! dav1d (BSD-2) is reached through the `dav1d` crate, the Rust binding of
//! the rust-av project, so this crate has no `unsafe` of its own. It is
//! built only with the `dav1d` feature, which links the system's libdav1d;
//! without it the crate is empty. [`Dav1dBackend`] decodes in the calling
//! process; `coreav-worker` runs the same decoder inside its sandbox, which
//! is how the CLI uses it.

#[cfg(feature = "dav1d")]
mod decoder;

#[cfg(feature = "dav1d")]
pub use decoder::{Dav1dBackend, Dav1dDecoder, Dav1dOptions};
