//! A VP9 decoder written from the VP9 Bitstream & Decoding Process
//! Specification, version 0.7.
//!
//! **This crate is opt-in and is never part of a default build.** Google's
//! patent grant for VP9 covers the implementations Google distributes, such
//! as libvpx, and not this one; CoreAV ships it as a separately packaged
//! backend that an operator turns on deliberately (`README.md` in this
//! crate, `docs/design/vp9.md`, "Packaging").
//!
//! The decoder is sans-IO: it takes packets (a frame, or a superframe) and
//! reads no files. Every decoder takes [`Limits`]; hostile input produces
//! an [`Error`], never a panic. Today it reads headers only: each frame's
//! uncompressed header (clause 6.2) and compressed header (clause 6.3),
//! the superframe index (Annex B), the reference slots' sizes and the saved
//! probability contexts, so it can say which frames a stream shows and at
//! what size.
//!
//! ```
//! use coreav_vp9::{Decoder, Header, Limits};
//!
//! // The uncompressed header of a 64x48 key frame, then one byte of
//! // compressed header.
//! let frame = [
//!     0x82, 0x49, 0x83, 0x42, 0x40, 0x03, 0xf0, 0x02, 0xf5, 0x80, 0x07, 0x80, 0x00, 0x01, 0x00,
//! ];
//! let mut decoder = Decoder::new(Limits::default());
//! let headers = decoder.read_headers(&frame)?;
//! let Some(Header::Frame { header, .. }) = headers.first() else { panic!() };
//! assert_eq!((header.width, header.height), (64, 48));
//! # Ok::<(), coreav_vp9::Error>(())
//! ```

#![forbid(unsafe_code)]

mod bits;
mod bool;
mod compressed;
mod decoder;
mod error;
mod header;
mod limits;
mod probs;
mod superframe;
mod tables;
mod tile;

pub use compressed::{CompressedHeader, ReferenceMode, TxMode};
pub use decoder::Decoder;
pub use error::{Error, LimitKind, Result};
pub use header::{
    ColorConfig, ColorSpace, FrameHeader, Header, InterpolationFilter, LoopFilter, MAX_SEGMENTS,
    Quantization, SEG_LVL_MAX, Segmentation, TileInfo,
};
pub use limits::Limits;
pub use superframe::split as split_superframe;
