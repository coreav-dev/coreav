//! Errors and warnings.

use crate::FourCc;
use core::fmt;

/// Which limit of [`crate::Limits`] a file exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    /// `max_top_level_boxes`.
    TopLevelBoxes,
    /// `max_depth`.
    Depth,
    /// `max_total_boxes`.
    TotalBoxes,
    /// `max_box_bytes`.
    BoxBytes,
    /// `max_retained_bytes`.
    RetainedBytes,
    /// `max_table_bytes`.
    TableBytes,
    /// `max_packet_bytes`.
    PacketBytes,
    /// `max_sample_work`.
    SampleWork,
    /// `max_fragments`.
    Fragments,
    /// `max_tracks`.
    Tracks,
    /// `max_movie_bytes`.
    MovieBytes,
    /// `max_movie_ratio`.
    MovieRatio,
    /// Bytes requested from a remote source ([`crate::RangeReader`]).
    ReadBytes,
    /// Requests sent to a remote source ([`crate::RangeReader`]).
    ReadRequests,
}

/// A reason a file or one of its parts could not be read.
///
/// Errors carry byte offsets and static reasons only, never bytes from the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A structure extends past the end of the available bytes.
    Truncated {
        /// Offset of the structure that does not fit.
        offset: u64,
    },
    /// A box is structurally invalid.
    InvalidBox {
        /// Offset of the box header.
        offset: u64,
        /// The box type.
        kind: FourCc,
        /// What is wrong.
        reason: &'static str,
    },
    /// A configured limit was exceeded.
    LimitExceeded {
        /// The limit.
        limit: LimitKind,
        /// Offset at which it was exceeded, when known.
        offset: u64,
    },
    /// Arithmetic on values from the file overflowed.
    Overflow {
        /// Offset of the structure whose values overflowed.
        offset: u64,
    },
    /// The input is not an ISO base media or QuickTime file.
    NotIsobmff,
    /// A feature this crate does not read.
    Unsupported {
        /// What is unsupported.
        reason: &'static str,
    },
    /// A track uses a codec that is not on the caller's allowlist.
    CodecNotAllowed {
        /// The track's `track_ID`.
        track_id: u32,
        /// The sample entry type.
        fourcc: FourCc,
    },
    /// The caller asked for a track index that does not exist.
    NoSuchTrack,
    /// The underlying source failed.
    Io(std::io::ErrorKind),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { offset } => write!(f, "truncated structure at offset {offset}"),
            Self::InvalidBox {
                offset,
                kind,
                reason,
            } => write!(f, "invalid '{kind}' box at offset {offset}: {reason}"),
            Self::LimitExceeded { limit, offset } => {
                write!(f, "limit {limit:?} exceeded at offset {offset}")
            }
            Self::Overflow { offset } => write!(f, "arithmetic overflow at offset {offset}"),
            Self::NotIsobmff => f.write_str("not an ISO base media or QuickTime file"),
            Self::Unsupported { reason } => write!(f, "unsupported: {reason}"),
            Self::CodecNotAllowed { track_id, fourcc } => {
                write!(f, "codec '{fourcc}' of track {track_id} is not allowed")
            }
            Self::NoSuchTrack => f.write_str("no such track"),
            Self::Io(kind) => write!(f, "I/O error: {kind}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.kind())
    }
}

/// A recoverable problem, reported next to a successful result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    /// The edit list has a shape outside the supported model; presentation
    /// times are unknown.
    UnsupportedEditList,
    /// The media header duration differs from the sum of the decode
    /// durations; the media header value is kept.
    DurationConflict {
        /// `mdhd` duration in media ticks.
        mdhd: u64,
        /// Sum of the `stts` deltas in media ticks.
        stts: u64,
    },
    /// The track references external data; it gets no packets.
    ExternalData,
    /// A codec configuration record could not be read; the values it would
    /// provide are unknown.
    MalformedCodecConfig(FourCc),
    /// The track has more than one sample description; only the first is
    /// described.
    MultipleSampleEntries,
    /// The sample tables are inconsistent; the track has no packets.
    InvalidSampleTable(&'static str),
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedEditList => f.write_str("unsupported edit list"),
            Self::DurationConflict { mdhd, stts } => write!(
                f,
                "duration conflict: mdhd {mdhd} ticks, stts sum {stts} ticks; mdhd kept"
            ),
            Self::ExternalData => f.write_str("external data reference; no packets"),
            Self::MalformedCodecConfig(kind) => write!(f, "malformed '{kind}' configuration"),
            Self::MultipleSampleEntries => f.write_str("multiple sample descriptions"),
            Self::InvalidSampleTable(reason) => write!(f, "invalid sample table: {reason}"),
        }
    }
}

/// Records a warning once: a repeated identical warning is not stored again,
/// so file-controlled repetition (for example thousands of malformed codec
/// configuration boxes) cannot grow the list. Every warning value is small
/// and drawn from a short fixed set, so a track holds only a few.
pub(crate) fn warn(warnings: &mut Vec<Warning>, warning: Warning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}
