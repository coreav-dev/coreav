//! HLS packaging with fragmented MP4 (CMAF) segments cut on request
//! (design note: `docs/design/hls.md`), written from RFC 8216 (HLS),
//! ISO/IEC 14496-12 (movie fragments, segment boxes), ISO/IEC 14496-15
//! Annex E and the AV1 and VP codec ISO media file format bindings (codec
//! strings), and ISO/IEC 14496-3 (AAC).
//!
//! The work is split in two, so that a request handler never parses the
//! source's movie box:
//!
//! - **Once:** [`build_index`] lists the source and returns an index: per
//!   rendition (one video track, at most one AAC track), the sample entry,
//!   the timing, and per segment the samples' byte ranges, sizes,
//!   durations, sync flags and composition offsets.
//! - **Per request:** [`HlsIndex::open`] reads the index header and the
//!   segment directories; the playlists and init segments come from them
//!   alone; [`HlsIndex::media_segment`] reads one segment's records from the
//!   index and returns a [`MediaSegment`]: the `moof` box and `mdat` header
//!   as bytes, then the source byte ranges whose concatenation is the rest
//!   of the segment. A caller streams the header and then the ranges, as
//!   fetched from object storage, or lets [`MediaSegment::write`] copy them
//!   through one 64 KiB window.
//!
//! ```
//! use coreav_isobmff::{Allowlist, Limits, SliceSource};
//! use coreav_isobmff_write::hls::{HlsOptions, build_index};
//!
//! let not_a_movie = b"RIFF\x24\x00\x00\x00AVI LIST";
//! let index = build_index(
//!     SliceSource::new(not_a_movie),
//!     &Limits::default(),
//!     &Allowlist::default(),
//!     &HlsOptions::default(),
//! );
//! assert!(index.is_err());
//! ```

mod build;
mod codecs;
mod deferred;
mod index;
mod init;
pub(crate) mod nal;
mod playlist;
mod segment;

pub use build::build_index;
pub use index::{HlsIndex, IndexTrack, SegmentEntry};
pub use playlist::Resource;
pub use segment::{CopyRange, MediaSegment};

use crate::{AudioChoice, VideoChoice};
use coreav_isobmff::Rational;

/// Most bytes of [`HlsOptions::source_tag`].
pub const MAX_SOURCE_TAG: usize = 255;

/// One rendition: a media playlist, an init segment and media segments,
/// each holding one track of the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rendition {
    /// The video track.
    Video,
    /// The AAC track, an alternative audio rendition of the video.
    Audio,
}

impl Rendition {
    /// `video` or `audio`, the prefix of the rendition's resource names.
    pub fn name(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }
}

/// What [`build_index`] packages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlsOptions {
    /// The video track; `None` packages the audio alone, as does
    /// `Some(VideoChoice::First)` for a source without a video track.
    pub video: Option<VideoChoice>,
    /// The audio track. The default packages the first audio track the
    /// allowlist keeps, and fails when it is not AAC.
    pub audio: AudioChoice,
    /// Target segment duration in seconds, above 0. Video segments start
    /// at the first clean sync sample at or after each multiple of it.
    pub segment_duration: Rational,
    /// Opaque bytes stored in the index, at most [`MAX_SOURCE_TAG`]: for
    /// example the object's entity tag, so that a request handler can tell
    /// that the index was built from the object it is about to read.
    pub source_tag: Vec<u8>,
}

impl Default for HlsOptions {
    fn default() -> Self {
        Self {
            video: Some(VideoChoice::First),
            audio: AudioChoice::First,
            segment_duration: Rational::from_int(6),
            source_tag: Vec::new(),
        }
    }
}
