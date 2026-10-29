//! Memory-safe extraction of text from ISO base media and QuickTime files:
//! subtitles (3GPP timed text `tx3g`, also called `mov_text`, and QuickTime
//! text) to WebVTT, and QuickTime chapter tracks to a chapter list
//! (`docs/design/text.md`).
//!
//! Written from 3GPP TS 26.245 (the timed text sample format), the W3C
//! WebVTT specification and Apple's QuickTime File Format documentation
//! (chapter lists, text sample data). The source is untrusted; it is read
//! through `coreav-isobmff` under the caller's [`Limits`](coreav_isobmff::Limits) and
//! [`TextLimits`], and malformed input produces an error or a reported
//! count, never a panic. Nothing is decoded or re-encoded beyond the text
//! itself: styling is dropped and counted.
//!
//! ```
//! use coreav_isobmff::{Limits, SliceSource};
//! use coreav_text::{TextLimits, TrackChoice, extract_subtitles};
//!
//! let not_a_movie = b"RIFF\x24\x00\x00\x00AVI LIST";
//! let mut out = Vec::new();
//! let result = extract_subtitles(
//!     SliceSource::new(not_a_movie),
//!     &Limits::default(),
//!     &TextLimits::default(),
//!     TrackChoice::First,
//!     &mut out,
//! );
//! assert!(result.is_err());
//! assert!(out.is_empty());
//! ```

mod chapters;
mod sample;
mod subtitles;
mod timing;
mod webvtt;

pub use chapters::{Chapter, Chapters, read_chapter_tracks, read_chapters};
pub use sample::{
    MODIFIERS, ModifierCounts, OTHER_MODIFIER, SampleError, TextSample, modifier_type, parse_sample,
};
pub use subtitles::{SubtitleSummary, extract_subtitles};
pub use webvtt::cue_text;

use core::fmt;
use coreav_isobmff::Error;

/// Limits of this crate, on top of `coreav-isobmff`'s [`Limits`].
///
/// [`Limits`]: coreav_isobmff::Limits
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextLimits {
    /// Largest text sample read, in bytes. TS 26.245 asks authors for at
    /// most 2,048 bytes of text; a sample also holds its modifier boxes.
    pub max_sample_bytes: u32,
    /// Most chapters kept.
    pub max_chapters: u32,
    /// Most bytes of chapter titles kept, in total.
    pub max_title_bytes: u64,
}

impl Default for TextLimits {
    fn default() -> Self {
        Self {
            max_sample_bytes: 1 << 20,
            max_chapters: 10_000,
            max_title_bytes: 4 << 20,
        }
    }
}

/// Which subtitle track to extract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackChoice {
    /// The first text track (3GPP timed text or QuickTime text) that is not
    /// a chapter track.
    First,
    /// The track at this position among all tracks of the file
    /// (`Track::stream_index`).
    Stream(usize),
}

/// Why extraction failed. Nothing about the source's bytes is included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextError {
    /// The source could not be read, is malformed, exceeds a limit, or has
    /// no track that can be extracted (`Error::Unsupported`,
    /// `Error::NoSuchTrack`).
    Source(Error),
    /// Writing the output failed; the output is incomplete.
    Write(std::io::ErrorKind),
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(f),
            Self::Write(kind) => write!(f, "write error: {kind}"),
        }
    }
}

impl std::error::Error for TextError {}

impl From<Error> for TextError {
    fn from(error: Error) -> Self {
        Self::Source(error)
    }
}

impl From<std::io::Error> for TextError {
    fn from(error: std::io::Error) -> Self {
        Self::Write(error.kind())
    }
}

/// What reading text samples found that the output does not carry. Every
/// count is of samples, except `styling`, which counts modifier boxes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dropped {
    /// Samples with no text (TS 26.245 does not define them; writers use
    /// them for gaps).
    pub empty: u64,
    /// Samples outside the track's edit, so never presented.
    pub not_presented: u64,
    /// Samples presented for less than the output's time resolution (one
    /// millisecond in WebVTT).
    pub too_short: u64,
    /// Samples shorter than their declared text length; skipped.
    pub malformed: u64,
    /// Samples whose text was not valid UTF-8 or UTF-16; decoded with
    /// U+FFFD replacements.
    pub invalid_text: u64,
    /// Samples with bytes after the text that are not whole boxes; those
    /// bytes are ignored.
    pub trailing_bytes: u64,
    /// Modifier boxes (styling, highlighting, karaoke, links, ...), by type:
    /// one slot per entry of [`MODIFIERS`], then every other type.
    pub styling: ModifierCounts,
}

impl Dropped {
    fn add_sample(&mut self, sample: &TextSample) {
        if sample.invalid_text {
            self.invalid_text = self.invalid_text.saturating_add(1);
        }
        if sample.trailing_bytes {
            self.trailing_bytes = self.trailing_bytes.saturating_add(1);
        }
        for (total, count) in self.styling.iter_mut().zip(sample.modifiers) {
            *total = total.saturating_add(count);
        }
    }
}
