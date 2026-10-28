//! Errors and warnings.

use crate::Codec;
use core::fmt;

/// Which limit of [`crate::Limits`] a file exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    /// `max_elements`.
    Elements,
    /// `max_element_bytes`.
    ElementBytes,
    /// `max_retained_bytes`.
    RetainedBytes,
    /// `max_index_bytes`.
    IndexBytes,
    /// `max_packet_bytes`.
    PacketBytes,
    /// `max_block_work`.
    BlockWork,
    /// `max_tracks`.
    Tracks,
}

/// A reason a file or one of its parts could not be read.
///
/// Errors carry byte offsets, element IDs and static reasons only, never
/// bytes from the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A structure extends past the end of the available bytes.
    Truncated {
        /// Offset of the structure that does not fit.
        offset: u64,
    },
    /// An element is structurally invalid.
    InvalidElement {
        /// Offset of the element header.
        offset: u64,
        /// The element ID, marker bits included (`0x1A45DFA3` for the EBML
        /// header); 0 when the ID itself could not be read.
        id: u32,
        /// What is wrong.
        reason: &'static str,
    },
    /// A configured limit was exceeded.
    LimitExceeded {
        /// The limit.
        limit: LimitKind,
        /// Offset at which it was exceeded.
        offset: u64,
    },
    /// Arithmetic on values from the file overflowed.
    Overflow {
        /// Offset of the structure whose values overflowed.
        offset: u64,
    },
    /// The input is not a Matroska or WebM file.
    NotMatroska,
    /// A feature this crate does not read.
    Unsupported {
        /// What is unsupported.
        reason: &'static str,
    },
    /// A track uses a codec that is not on the caller's allowlist (strict
    /// mode).
    CodecNotAllowed {
        /// The track's `TrackNumber`.
        track_number: u64,
        /// The codec the `CodecID` names.
        codec: Codec,
    },
    /// The underlying source failed.
    Io(std::io::ErrorKind),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { offset } => write!(f, "truncated structure at offset {offset}"),
            Self::InvalidElement { offset, id, reason } => {
                write!(f, "invalid element 0x{id:X} at offset {offset}: {reason}")
            }
            Self::LimitExceeded { limit, offset } => {
                write!(f, "limit {limit:?} exceeded at offset {offset}")
            }
            Self::Overflow { offset } => write!(f, "arithmetic overflow at offset {offset}"),
            Self::NotMatroska => f.write_str("not a Matroska or WebM file"),
            Self::Unsupported { reason } => write!(f, "unsupported: {reason}"),
            Self::CodecNotAllowed {
                track_number,
                codec,
            } => write!(
                f,
                "codec {} of track {track_number} is not allowed",
                codec.name()
            ),
            Self::Io(kind) => write!(f, "I/O error: {kind}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<coreav_isobmff::Error> for Error {
    /// Maps the errors a [`coreav_isobmff::Source`] can return.
    fn from(error: coreav_isobmff::Error) -> Self {
        match error {
            coreav_isobmff::Error::Truncated { offset } => Self::Truncated { offset },
            coreav_isobmff::Error::LimitExceeded { offset, .. } => Self::LimitExceeded {
                limit: LimitKind::ElementBytes,
                offset,
            },
            coreav_isobmff::Error::Io(kind) => Self::Io(kind),
            _ => Self::Io(std::io::ErrorKind::Other),
        }
    }
}

/// A recoverable problem, reported next to a successful result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    /// The segment's declared size runs past the end of the file; the file
    /// is read up to its end.
    TruncatedSegment,
    /// A `SeekHead` entry points outside the segment or the file; it is
    /// ignored.
    InvalidSeekEntry,
    /// A codec private record could not be read; the values it would
    /// provide are unknown.
    MalformedCodecPrivate,
    /// The track's frames are compressed or encrypted in a way this crate
    /// does not undo; the track has no packets.
    UnsupportedEncoding,
    /// The track's frames are encrypted; packets are listed as stored.
    Encrypted,
    /// The track's frames are zlib-compressed; packets are listed with their
    /// stored ranges and inflated sizes.
    ZlibCompressed,
    /// `TrackTimestampScale` is not 1.0; presentation times are unknown.
    UnsupportedTrackTimestampScale,
    /// A block names a track number that `Tracks` does not define; the block
    /// is skipped.
    UnknownTrackNumber,
    /// The segment has no `Cues`; seeking uses a cluster scan.
    NoCues,
    /// A cue point is invalid (for example its cluster position lies outside
    /// the segment); it is ignored.
    InvalidCuePoint,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TruncatedSegment => "segment size runs past the end of the file",
            Self::InvalidSeekEntry => "a SeekHead entry points outside the segment; ignored",
            Self::MalformedCodecPrivate => "malformed codec private data",
            Self::UnsupportedEncoding => {
                "frames are compressed or encrypted in an unsupported way; no packets"
            }
            Self::Encrypted => "frames are encrypted; packets are listed as stored",
            Self::ZlibCompressed => {
                "frames are zlib-compressed; packets are listed with their inflated sizes"
            }
            Self::UnsupportedTrackTimestampScale => {
                "TrackTimestampScale is not 1.0; presentation times unknown"
            }
            Self::UnknownTrackNumber => "a block names an undefined track number; skipped",
            Self::NoCues => "no Cues; seeking scans clusters",
            Self::InvalidCuePoint => "an invalid cue point was ignored",
        })
    }
}

/// Records a warning once, so file-controlled repetition cannot grow the
/// list.
pub(crate) fn warn(warnings: &mut Vec<Warning>, warning: Warning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}
