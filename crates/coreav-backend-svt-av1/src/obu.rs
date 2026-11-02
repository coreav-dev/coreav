//! Bounded framing reads for the AV1 low-overhead OBU format.
//!
//! Only headers, lengths and the empty temporal delimiter are checked. Payload
//! syntax, sequence configuration, OBU ordering and timestamps are not validated.
//! A successful read does not establish conformance or suitability for muxing.
//! Reserved OBU types are rejected as a strict backend-output subset, even though
//! AV1 decoders are required to ignore those types.
//!
//! Extension-header restrictions for non-layer-specific types follow the
//! published HTML specification's [OBU header semantics].
//!
//! [OBU header semantics]: https://aomediacodec.github.io/av1-spec/#obu-header-semantics

use std::{fmt, iter::FusedIterator};

/// Supported standard AV1 OBU types.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObuKind {
    /// Sequence configuration.
    SequenceHeader,
    /// Temporal unit delimiter.
    TemporalDelimiter,
    /// Frame header.
    FrameHeader,
    /// Coded tile group.
    TileGroup,
    /// Metadata.
    Metadata,
    /// Frame header and coded tiles.
    Frame,
    /// Redundant frame header.
    RedundantFrameHeader,
    /// Tile list.
    TileList,
    /// Padding bytes.
    Padding,
}

/// Caller-selected bounds on one borrowed packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Maximum total packet bytes, including headers and size fields.
    pub max_packet_bytes: usize,
    /// Maximum number of OBUs, including empty OBUs and padding.
    pub max_obus: usize,
}

/// A framing or resource-bound failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// The packet exceeds the caller's byte limit.
    PacketTooLarge,
    /// Another OBU would exceed the caller's count limit.
    TooManyObus,
    /// The OBU forbidden bit is set.
    ForbiddenBit,
    /// The reserved bit in the first header byte is set.
    ReservedHeaderBit,
    /// The OBU type is reserved.
    ReservedType,
    /// The low-overhead format's mandatory size field is absent.
    MissingSizeField,
    /// A non-layer-specific OBU has an extension header.
    UnexpectedExtension,
    /// An indicated extension header is missing.
    TruncatedExtension,
    /// Reserved extension-header bits are set.
    ReservedExtensionBits,
    /// The size field ends before its terminating byte.
    TruncatedSize,
    /// The eighth size byte still indicates continuation.
    SizeTooLong,
    /// The size exceeds the AV1 unsigned 32-bit bound or the address space.
    SizeOverflow,
    /// The declared payload is not entirely in the packet.
    TruncatedPayload,
    /// A temporal delimiter has a nonempty payload.
    NonemptyTemporalDelimiter,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PacketTooLarge => "AV1 packet exceeds the byte limit",
            Self::TooManyObus => "AV1 packet exceeds the OBU count limit",
            Self::ForbiddenBit => "AV1 OBU forbidden bit is set",
            Self::ReservedHeaderBit => "AV1 OBU reserved header bit is set",
            Self::ReservedType => "AV1 OBU type is reserved",
            Self::MissingSizeField => "AV1 low-overhead OBU has no size field",
            Self::UnexpectedExtension => "AV1 OBU type forbids an extension header",
            Self::TruncatedExtension => "AV1 OBU extension header is truncated",
            Self::ReservedExtensionBits => "AV1 OBU reserved extension bits are set",
            Self::TruncatedSize => "AV1 OBU size field is truncated",
            Self::SizeTooLong => "AV1 OBU size field exceeds eight bytes",
            Self::SizeOverflow => "AV1 OBU size exceeds its integer bound",
            Self::TruncatedPayload => "AV1 OBU payload is truncated",
            Self::NonemptyTemporalDelimiter => "AV1 temporal delimiter payload is not empty",
        })
    }
}

impl std::error::Error for Error {}

/// A borrowed OBU with a checked header and payload extent, not checked content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Obu<'a> {
    kind: ObuKind,
    payload: &'a [u8],
    temporal_id: u8,
    spatial_id: u8,
}

impl<'a> Obu<'a> {
    /// Returns the supported type identified by the header.
    pub fn kind(&self) -> ObuKind {
        self.kind
    }

    /// Returns the declared payload bytes without validating their syntax.
    pub fn payload(&self) -> &'a [u8] {
        self.payload
    }

    /// Returns the temporal layer ID, or zero when no extension is present.
    pub fn temporal_id(&self) -> u8 {
        self.temporal_id
    }

    /// Returns the spatial layer ID, or zero when no extension is present.
    pub fn spatial_id(&self) -> u8 {
        self.spatial_id
    }
}

/// A single-pass, allocation-free framing reader over a borrowed packet.
///
/// Construction checks only the packet byte limit. Each iteration checks one OBU
/// and the count limit. Consume through the final `None` to check the entire
/// packet. The first error is terminal, so later bytes are never yielded after a
/// failure. An empty packet is an empty sequence, even with zero limits.
#[derive(Debug)]
pub struct Obus<'a> {
    remaining: &'a [u8],
    remaining_obus: usize,
}

impl<'a> Obus<'a> {
    /// Borrows a packet if its byte length does not exceed the supplied limit.
    ///
    /// OBU framing and count errors are returned by iteration, not construction.
    pub fn new(bytes: &'a [u8], limits: Limits) -> Result<Self, Error> {
        if bytes.len() > limits.max_packet_bytes {
            return Err(Error::PacketTooLarge);
        }
        Ok(Self {
            remaining: bytes,
            remaining_obus: limits.max_obus,
        })
    }
}

impl<'a> Iterator for Obus<'a> {
    type Item = Result<Obu<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let (&header, rest) = std::mem::take(&mut self.remaining).split_first()?;
        if self.remaining_obus == 0 {
            return Some(Err(Error::TooManyObus));
        }
        match read_obu(header, rest) {
            Ok((obu, remaining)) => {
                self.remaining = remaining;
                self.remaining_obus -= 1;
                Some(Ok(obu))
            }
            Err(error) => Some(Err(error)),
        }
    }
}

impl FusedIterator for Obus<'_> {}

fn read_obu(header: u8, mut rest: &[u8]) -> Result<(Obu<'_>, &[u8]), Error> {
    if header & 0x80 != 0 {
        return Err(Error::ForbiddenBit);
    }
    if header & 1 != 0 {
        return Err(Error::ReservedHeaderBit);
    }
    let kind = match (header >> 3) & 15 {
        1 => ObuKind::SequenceHeader,
        2 => ObuKind::TemporalDelimiter,
        3 => ObuKind::FrameHeader,
        4 => ObuKind::TileGroup,
        5 => ObuKind::Metadata,
        6 => ObuKind::Frame,
        7 => ObuKind::RedundantFrameHeader,
        8 => ObuKind::TileList,
        15 => ObuKind::Padding,
        _ => return Err(Error::ReservedType),
    };
    if header & 2 == 0 {
        return Err(Error::MissingSizeField);
    }
    let mut temporal_id = 0;
    let mut spatial_id = 0;
    if header & 4 != 0 {
        if matches!(
            kind,
            ObuKind::SequenceHeader | ObuKind::TemporalDelimiter | ObuKind::TileList
        ) {
            return Err(Error::UnexpectedExtension);
        }
        let (&extension, remaining) = rest.split_first().ok_or(Error::TruncatedExtension)?;
        rest = remaining;
        if extension & 7 != 0 {
            return Err(Error::ReservedExtensionBits);
        }
        temporal_id = extension >> 5;
        spatial_id = (extension >> 3) & 3;
    }
    let (size, rest) = read_size(rest)?;
    if kind == ObuKind::TemporalDelimiter && size != 0 {
        return Err(Error::NonemptyTemporalDelimiter);
    }
    let size = usize::try_from(size).map_err(|_| Error::SizeOverflow)?;
    let (payload, remaining) = rest.split_at_checked(size).ok_or(Error::TruncatedPayload)?;
    Ok((
        Obu {
            kind,
            payload,
            temporal_id,
            spatial_id,
        },
        remaining,
    ))
}

fn read_size(mut bytes: &[u8]) -> Result<(u32, &[u8]), Error> {
    let mut value = 0_u64;
    for index in 0..8 {
        let (&byte, rest) = bytes.split_first().ok_or(Error::TruncatedSize)?;
        bytes = rest;
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            return Ok((
                u32::try_from(value).map_err(|_| Error::SizeOverflow)?,
                bytes,
            ));
        }
    }
    Err(Error::SizeTooLong)
}
