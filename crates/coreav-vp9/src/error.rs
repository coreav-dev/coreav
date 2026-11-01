//! Errors of the decoder.

use core::fmt;

/// Which limit of [`crate::Limits`] a stream exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    /// `max_width`: samples across one frame.
    Width,
    /// `max_height`: samples down one frame.
    Height,
    /// `max_pixels`: luma samples of one frame.
    Pixels,
    /// `max_packet_bytes`: bytes of one packet.
    PacketBytes,
}

impl LimitKind {
    const fn as_str(self) -> &'static str {
        match self {
            LimitKind::Width => "max_width",
            LimitKind::Height => "max_height",
            LimitKind::Pixels => "max_pixels",
            LimitKind::PacketBytes => "max_packet_bytes",
        }
    }
}

/// A reason a packet, or one of its frames, could not be decoded.
///
/// Errors carry static reasons only, never bytes from the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A syntax structure ends before the data that holds it does.
    Truncated(&'static str),
    /// A syntax element, or a value derived from it, breaks a requirement
    /// of the specification this decoder relies on.
    Invalid(&'static str),
    /// Valid VP9 this decoder does not decode yet (`docs/design/vp9.md`,
    /// "Scope").
    Unsupported(&'static str),
    /// A limit was exceeded.
    Limit(LimitKind),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated(what) => write!(f, "the frame ends inside {what}"),
            Error::Invalid(what) => write!(f, "invalid bitstream: {what}"),
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::Limit(kind) => write!(f, "limit exceeded: {}", kind.as_str()),
        }
    }
}

impl std::error::Error for Error {}

/// The decoder's result type.
pub type Result<T> = core::result::Result<T, Error>;
