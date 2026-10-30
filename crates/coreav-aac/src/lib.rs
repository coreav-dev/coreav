//! Memory-safe AAC-LC decoding (`docs/design/audio-decode.md`, "AAC-LC").
//!
//! **Opt-in, and never part of a default CoreAV build.** Decoding AAC may
//! need a patent licence where you are; the sources disagree on whether
//! patents on AAC-LC are still in force. CoreAV reaches this crate only
//! through the `aac` features of `coreav-audio` and `coreav-cli`, which
//! are off by default, and CI fails if a default build links it. Nothing
//! here is legal advice. See `README.md`.
//!
//! Written from ISO/IEC 14496-3:2009 (subpart 1: `AudioSpecificConfig`;
//! subpart 4: the general audio syntax, decoding process and Annex 4.A
//! tables), with ISO/IEC 13818-7:2004 as a cross-check of the tables the
//! two share; `tables/gen_aac.py` writes `src/tables.rs` from both.
//!
//! - [`Config`] parses an `AudioSpecificConfig`: object type 2 (AAC LC),
//!   frames of 1,024 or 960 samples, every sample rate, channel
//!   configurations 1 to 7 and a program configuration. Other object
//!   types, and SBR or PS signalled in the configuration (HE-AAC), are
//!   [`Error::Unsupported`] with their name.
//! - [`Decoder`] decodes `raw_data_block()`s, as MP4 and Matroska store
//!   them, to planar `f32` in WAV's channel order: single channel, channel
//!   pair, LFE and coupling channel elements; section data, scalefactors,
//!   pulses, the eleven spectrum codebooks with escapes, M/S and intensity
//!   stereo, perceptual noise substitution, TNS, and the filterbank with
//!   every window sequence and both window shapes. Data stream and fill
//!   elements are skipped; an SBR payload in a fill element is
//!   [`Error::Unsupported`]. Dynamic range control and the matrix mixdown
//!   (both optional for a decoder) are not applied.
//!
//! Choices, beyond what the standard fixes:
//! - An element goes to the slot its configuration names its
//!   `element_instance_tag` for, else to the first free slot of its type
//!   (order of appearance); a frame lacking an element of the
//!   configuration, or holding one too many, is damaged.
//! - Noise substitution draws from a linear congruential generator seeded
//!   per frame from the packet's bytes and its position in the stream
//!   ([`Decoder::set_position`]), so output is the same on every run and
//!   thread count. [`Frame::noise_bands`] reports how many bands a frame
//!   coded as noise.
//! - A frame that fails to decode resets the decoder (silent overlap, sine
//!   window shape), so the state after a frame depends on that frame
//!   alone: a parallel decoder primed with one frame decodes as one pass.
//! - Arithmetic is `f32`; the windows, |q|^(4/3) and the FFT's twiddles
//!   are generated tables, so no C math library is linked.
//!
//! Sans-IO, no `unsafe`, no panics; every loop is bounded by the frame's
//! bits or by the configuration, and [`Limits`] bound the channels.

mod bits;
mod config;
mod decoder;
mod error;
mod filterbank;
mod ics;
mod tables;
mod tools;

pub use config::Config;
pub use decoder::{Decoder, Frame};
pub use error::Error;

/// Bounds on what a configuration may ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most output channels.
    pub max_channels: usize,
}

impl Default for Limits {
    /// 64 channels: more than any channel configuration, fewer than the
    /// 93 a program configuration can name.
    fn default() -> Self {
        Self { max_channels: 64 }
    }
}
