//! An H.264 (AVC) decoder: High profile, 8-bit 4:2:0,
//! progressive frames, written from Rec. ITU-T H.264 (08/2024).
//!
//! **This crate is opt-in and is never part of a default build.** Decoding
//! H.264 may need a patent licence where you are; CoreAV ships it as a
//! separately packaged backend that an operator turns on deliberately
//! (`README.md` in this crate, `docs/design/h264.md`, "Packaging").
//! The default H.264 path stays the sandboxed FFmpeg fallback (an NVDEC
//! backend is planned but not built yet, `docs/design/decode.md`, "NVDEC").
//!
//! The decoder is sans-IO: it takes NAL units and returns pictures and
//! reads no files. It starts no threads unless asked to
//! ([`Decoder::with_threads`]), so it also runs in a WebAssembly sandbox.
//! Every entry point takes [`Limits`]; hostile input produces an
//! [`Error`], never a panic or unbounded work. Parsing, entropy
//! decoding and control code are safe Rust; the pixel kernels live in
//! `coreav-kernels-h264` behind views that check every length, stride and
//! range.
//!
//! The design, the scope and the bars the decoder is measured against are in
//! `docs/design/h264.md`.
//!
//! ```
//! use coreav_h264::{AnnexB, Limits, NalHeader, NalKind};
//!
//! // A sequence parameter set, as it arrives in a byte stream.
//! let stream = [0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x0a];
//! let mut splitter = AnnexB::new(&Limits::default());
//! let mut kinds = Vec::new();
//! let mut collect = |nal: &[u8]| {
//!     kinds.push(NalHeader::parse(nal)?.kind);
//!     Ok(())
//! };
//! splitter.push(&stream, &mut collect)?;
//! splitter.finish(&mut collect)?;
//! assert_eq!(kinds, [NalKind::Sps]);
//! # Ok::<(), coreav_h264::Error>(())
//! ```

mod bits;
mod cabac;
mod cavlc;
mod deblock;
mod decoder;
mod dpb;
mod error;
#[cfg(all(feature = "fast", target_arch = "x86_64"))]
mod fast;
mod inter;
mod limits;
mod mb;
mod nal;
mod nb;
mod params;
mod picture;
mod recon;
mod residual;
mod slice;
mod syntax;
#[rustfmt::skip]
mod tables;
#[cfg(test)]
#[path = "../tests/common/streams.rs"]
mod test_streams;
mod tier;
mod work;

// The kernels are part of this crate's surface: a caller picks them and
// hands them to the decoder.
pub use coreav_kernels_h264::{self as kernels, Auto, Kernels, Scalar, Simd};

pub use bits::BitReader;
pub use decoder::{Decoder, MAX_WARNINGS, Mode, Warning};
pub use error::{Error, LimitKind, Result};
pub use limits::Limits;
pub use nal::{
    AnnexB, AnnexBUnits, LengthPrefixedUnits, NalHeader, NalKind, annex_b, length_prefixed, rbsp,
};
pub use picture::{MOTION_BYTES_PER_MB, Picture, PlaneRef};
