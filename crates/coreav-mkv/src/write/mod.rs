//! Front-to-back Matroska and WebM output from packet metadata.
//!
//! [`plan`] checks the track descriptions and packet layout before any
//! output is written. [`Plan::write`] then copies payloads in the supplied
//! order, without seeking or retaining media bytes. Timestamps are
//! presentation times, not decode times; callers keep each track's
//! packets in decode order. See `docs/design/mkv-write.md`.

mod codec;
mod ebml;
mod plan;

pub use codec::CodecId;
pub use plan::{Plan, plan};

use crate::{DocType, Limits};

/// The parameters that differ between video and audio tracks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    /// Pixel dimensions and optional display dimensions in pixels.
    Video {
        /// Encoded width in pixels.
        width: u32,
        /// Encoded height in pixels.
        height: u32,
        /// Display width and height in pixels, without changing encoded size.
        display: Option<(u32, u32)>,
    },
    /// Sampling frequency, channel count and optional PCM bit depth.
    Audio {
        /// Samples per second, positive.
        sample_rate: u32,
        /// Number of channels, positive.
        channels: u32,
        /// Bits per decoded PCM sample, when known.
        bit_depth: Option<u32>,
    },
}

/// One output TrackEntry. Track numbers are the one-based slice positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// Mapping that defines the payload and initialization representation.
    pub codec: CodecId,
    /// The initialization bytes defined by the Matroska codec mapping.
    pub private: Vec<u8>,
    /// Audio or video parameters for this mapping.
    pub media: Media,
    /// Nanoseconds per frame, only when the caller has established cadence.
    pub default_duration_ns: Option<u64>,
    /// Decoder priming, subtracted from stored block timestamps.
    pub codec_delay_ns: u64,
    /// Nanoseconds decoded after seeking before playback can resume.
    pub seek_pre_roll_ns: u64,
    /// ISO 639-2 language code, or `und`.
    pub language: String,
    /// Optional UTF-8 track name.
    pub name: Option<String>,
    /// Whether this track is selected by default.
    pub default: bool,
}

/// One packet, in the caller's storage order. Payloads are not retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    /// Zero-based position in the track slice passed to [`plan`].
    pub track: usize,
    /// Presentation time after subtracting CodecDelay, in nanoseconds.
    pub pts_ns: i64,
    /// Presentation duration before DiscardPadding, in nanoseconds.
    pub duration_ns: Option<u64>,
    /// Exact payload byte count.
    pub size: u64,
    /// A random access point that does not depend on another block.
    pub keyframe: bool,
    /// Decode this frame without displaying it.
    pub invisible: bool,
    /// Positive trims the end, negative trims the beginning, in nanoseconds.
    pub discard_padding_ns: Option<i64>,
}

/// Segment properties. Every timestamp is rounded to the nearest tick,
/// with half ticks rounded toward positive infinity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Matroska, or its codec-restricted WebM profile.
    pub doc_type: DocType,
    /// Nanoseconds per Segment Tick; 1,000,000 is the interoperable default.
    pub timestamp_scale_ns: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            doc_type: DocType::Matroska,
            timestamp_scale_ns: 1_000_000,
        }
    }
}

/// Output work and memory bounds. The parser's limits also bound a writer:
/// tracks, retained track metadata, packet bytes, layout/index bytes and
/// packet count (`max_block_work`).
pub type WriteLimits = Limits;
