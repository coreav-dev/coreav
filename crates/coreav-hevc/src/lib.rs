//! Opt-in HEVC headers and scalar picture reconstruction from ITU-T H.265 (V11).
//!
//! HEVC may require a patent licence. This unpublished, separately packaged
//! crate is never part of default artifacts. Enable `native-hevc` explicitly.
//! `HeaderParser` inspects headers without reconstructing samples.
//! `IntraPictureDecoder` reconstructs one selected Main/Main10 intra picture.
//! `PictureDecoder` reconstructs complete streams with scalar inter prediction
//! and presentation-order output. Production backend wiring is absent.

#![forbid(unsafe_code)]
#![cfg(feature = "native-hevc")]

mod bits;
mod cabac;
mod ctu;
mod decoder;
mod dpb;
mod intra;
mod nal;
mod params;
mod parser;
mod picture_hash;
mod reconstruction;
mod residual;
mod rps;
mod scaling;
mod sei;
mod slice;
mod substream;
mod vui;

pub use decoder::{DecodeLimits, DecodeSummary, DecodedPicture, OutputMemory, PictureDecoder};
pub use dpb::{OutputEvent, PictureOrder};
pub use intra::{IntraLimits, IntraPicture, IntraPictureDecoder, IntraPlane, IntraStats};
pub use nal::{AnnexB, LengthPrefixed, NalHeader, packet_nal};
pub use parser::{HeaderParser, NalInfo, PictureInfo, Summary};
pub use reconstruction::InterStats;
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use residual::coefficient_levels_match_reference;
pub use rps::Rps;
pub use sei::{PictureHash, SeiInfo};
pub use slice::{LongTermRef, SliceHeader};
pub use vui::{Timing, Vui};

#[cfg(test)]
mod tests;

/// Parser failures contain static labels, never untrusted input bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Syntax, including a truncated header, violates H.265.
    InvalidData(&'static str),
    /// Valid syntax signals a coding tool outside Main/Main10 scope.
    Unsupported(&'static str),
    /// A caller budget or a fallible allocation was exhausted.
    Limit(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidData(reason) => write!(f, "invalid data: {reason}"),
            Self::Unsupported(reason) => write!(f, "unsupported: {reason}"),
            Self::Limit(reason) => write!(f, "limit: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

/// Result of parsing untrusted HEVC syntax.
pub type Result<T> = core::result::Result<T, Error>;

/// Tolerated declarations that do not change full-layer sample reconstruction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Warnings {
    /// A Main10 sequence also claimed compatibility with an eight-bit-only profile.
    pub profile_bit_depth: bool,
    /// A non-coprime Extended_SAR was interpreted as an unspecified aspect ratio.
    pub invalid_sample_aspect_ratio: bool,
    /// A higher temporal layer used a NAL type inconsistent with temporal nesting.
    pub temporal_nesting_nal_type: bool,
}

/// Operational budgets, separate from syntax and profile constraints.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Maximum complete NAL size; also bounds RBSP scratch bytes.
    pub max_nal_bytes: usize,
    /// Maximum coded width in luma samples.
    pub max_width: u32,
    /// Maximum coded height in luma samples.
    pub max_height: u32,
    /// Maximum coded luma width times height, before conformance cropping.
    pub max_pixels: u64,
    /// Maximum admitted DPB capacity, including the current reference picture.
    pub max_reference_frames: u32,
    /// Maximum CTUs in one coded picture and entry points in a header.
    pub max_ctus: u32,
    /// Maximum coded pictures over the entire parser session, including CVS changes.
    pub max_pictures: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_nal_bytes: 16 * 1024 * 1024,
            max_width: 16384,
            max_height: 16384,
            max_pixels: 1 << 28,
            max_reference_frames: 16,
            max_ctus: 1_048_576,
            max_pictures: 1_000_000,
        }
    }
}
