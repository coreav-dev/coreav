//! Memory-safe probing, packet listing and seek indexing for Matroska and
//! WebM files, written from RFC 9559 (Matroska), RFC 8794 (EBML) and the
//! IETF CELLAR Matroska codec mappings.
//!
//! Every entry point takes [`Limits`] and an [`Allowlist`]; malformed or
//! hostile input produces an [`Error`], never a panic or unbounded work. The
//! design, the limits and the rejected alternatives are recorded in
//! `docs/design/mkv.md`.
//!
//! ```
//! use coreav_mkv::{probe, Allowlist, Limits, SliceSource};
//!
//! let not_matroska = b"\x00\x00\x00\x18ftypisom";
//! let result = probe(SliceSource::new(not_matroska), &Limits::default(), &Allowlist::default());
//! assert!(result.is_err());
//! ```

mod audit;
mod bitstream;
mod block;
mod ebml;
mod error;
mod ids;
mod index;
mod inflate;
mod limits;
mod meta;
mod packets;
mod reader;
mod scan;
mod schema;
mod segment;
mod track;

pub use block::{BlockHeader, MAX_LACED_FRAMES, parse_block};
pub use coreav_isobmff::{
    BufferedSource, Color, FileSource, MAX_READ, Profile, Rational, SliceSource, Source,
};
pub use ebml::{ElementHeader, MAX_HEADER_LEN, Parsed, parse_element_header};
pub use error::{Error, LimitKind, Warning};
pub use index::{Index, IndexSource, SeekPoint};
pub use limits::Limits;
pub use meta::{Chapter, Edition, MAX_NESTING, SimpleTag, Tag};
pub use packets::{Packet, Packets};
pub use scan::{Scan, TrackScan};
pub use segment::{Attachment, DocType, Segment, probe, probe_with_metadata};
pub use track::{Allowlist, Audio, Codec, DroppedTrack, Encoding, Track, TrackKind, Video};

/// The first four bytes of every Matroska and WebM file: the EBML header
/// element ID (RFC 8794 8.1).
pub const MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

/// Whether `prefix` (the first bytes of a file) starts like a Matroska or
/// WebM file, for container detection.
pub fn sniff(prefix: &[u8]) -> bool {
    prefix.starts_with(&MAGIC)
}
