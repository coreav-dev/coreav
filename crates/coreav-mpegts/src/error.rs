//! Errors and warnings.

use core::fmt;

/// Which limit of [`crate::Limits`] a file exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    /// `max_probe_bytes`: the probe read that many bytes, the file goes on,
    /// and they name no program.
    ProbeBytes,
    /// `max_resync_bytes`.
    ResyncBytes,
    /// `max_programs`.
    Programs,
    /// `max_streams`.
    Streams,
    /// `max_packet_bytes`.
    PacketBytes,
    /// `max_buffered_bytes`.
    BufferedBytes,
    /// `max_listing_packets`.
    ListingPackets,
}

/// A reason a file could not be read.
///
/// Errors carry byte offsets, PIDs and static reasons only, never bytes
/// from the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input is not a transport stream: no sync byte repeats at a
    /// packet stride near its start.
    NotTransportStream,
    /// A Program Association Table and none of its Program Map Tables,
    /// or neither a Program Association Table nor a PES packet, was found
    /// in a file read to its end within `max_probe_bytes`. A file that goes
    /// on after them gets `LimitExceeded` with [`LimitKind::ProbeBytes`]
    /// instead.
    NoPrograms,
    /// A structure is invalid in a way the reader cannot skip.
    Invalid {
        /// Offset of the transport packet.
        offset: u64,
        /// What is wrong.
        reason: &'static str,
    },
    /// A stream uses a codec that is not on the caller's allowlist
    /// (strict mode).
    CodecNotAllowed {
        /// The stream's PID.
        pid: u16,
        /// The codec its type and descriptors name.
        codec: crate::Codec,
    },
    /// A configured limit was exceeded.
    LimitExceeded {
        /// The limit.
        limit: LimitKind,
        /// Offset at which it was exceeded.
        offset: u64,
    },
    /// The underlying source failed.
    Io(std::io::ErrorKind),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotTransportStream => write!(f, "not an MPEG transport stream"),
            Self::NoPrograms => write!(f, "no program tables found"),
            Self::Invalid { offset, reason } => write!(f, "{reason} at offset {offset}"),
            Self::CodecNotAllowed { pid, codec } => {
                write!(f, "codec {} of PID {pid} is not allowed", codec.name())
            }
            Self::LimitExceeded { limit, offset } => {
                write!(f, "limit {limit:?} exceeded at offset {offset}")
            }
            Self::Io(kind) => write!(f, "I/O error: {kind}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<coreav_isobmff::Error> for Error {
    fn from(error: coreav_isobmff::Error) -> Self {
        match error {
            coreav_isobmff::Error::Io(kind) => Self::Io(kind),
            coreav_isobmff::Error::Truncated { .. } => Self::Io(std::io::ErrorKind::UnexpectedEof),
            _ => Self::Io(std::io::ErrorKind::Other),
        }
    }
}

/// A recoverable problem: the reader went on past it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    /// Bytes between transport packets without a sync byte were skipped.
    LostSync {
        /// Where the skipped bytes start.
        offset: u64,
        /// How many were skipped.
        skipped: u64,
    },
    /// A packet with `transport_error_indicator` set was read; its payload
    /// may be damaged.
    TransportError {
        /// Offset of the packet.
        offset: u64,
    },
    /// A PID's continuity counter skipped: packets were lost.
    Discontinuity {
        /// The PID.
        pid: u16,
        /// Offset of the packet after the gap.
        offset: u64,
    },
    /// A packet repeated the previous one of its PID (the same continuity
    /// counter and the same bytes, its PCR aside) and was discarded, as
    /// ISO/IEC 13818-1 2.4.3.3 provides.
    Duplicate {
        /// The PID.
        pid: u16,
        /// Offset of the discarded packet.
        offset: u64,
    },
    /// A PSI section was invalid (its CRC, its length) and was skipped.
    BadSection {
        /// The PID it was on.
        pid: u16,
        /// What is wrong.
        reason: &'static str,
    },
    /// A PES packet was invalid and was skipped.
    BadPes {
        /// The PID it was on.
        pid: u16,
        /// Offset of its first transport packet.
        offset: u64,
        /// What is wrong.
        reason: &'static str,
    },
    /// A program's PMT changed version mid-stream.
    PmtChanged {
        /// The program number.
        program: u16,
    },
    /// Scrambled packets were seen on a PID; their payload is not read.
    Scrambled {
        /// The PID.
        pid: u16,
    },
    /// The file ends inside a PES packet.
    TruncatedPes {
        /// The PID.
        pid: u16,
    },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LostSync { offset, skipped } => {
                write!(
                    f,
                    "{skipped} bytes without a sync byte skipped at offset {offset}"
                )
            }
            Self::TransportError { offset } => write!(
                f,
                "the transport error indicator is set on the packet at offset {offset}; its payload may be damaged"
            ),
            Self::Discontinuity { pid, offset } => write!(
                f,
                "the continuity counter of PID {pid} skipped at offset {offset}: packets were lost"
            ),
            Self::Duplicate { pid, offset } => write!(
                f,
                "the packet of PID {pid} at offset {offset} repeats the one before it and was discarded"
            ),
            Self::BadSection { pid, reason } => {
                write!(f, "a section on PID {pid} was skipped: {reason}")
            }
            Self::BadPes {
                pid,
                offset,
                reason,
            } => write!(
                f,
                "the PES packet of PID {pid} at offset {offset} was skipped: {reason}"
            ),
            Self::PmtChanged { program } => {
                write!(
                    f,
                    "the program map table of program {program} changed version"
                )
            }
            Self::Scrambled { pid } => {
                write!(
                    f,
                    "PID {pid} carries scrambled packets; their payload is not read"
                )
            }
            Self::TruncatedPes { pid } => {
                write!(f, "the file ends inside a PES packet of PID {pid}")
            }
        }
    }
}

/// Keeps one copy of each warning, up to a bound, so that a file with a
/// lost packet every few packets cannot grow the list without end.
pub(crate) fn warn(warnings: &mut Vec<Warning>, warning: Warning) {
    const MAX_WARNINGS: usize = 256;
    if warnings.len() < MAX_WARNINGS && !warnings.contains(&warning) {
        warnings.push(warning);
    }
}
