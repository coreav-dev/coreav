//! Errors and limits of the decoder.
#![forbid(unsafe_code)]

use core::fmt;

/// Which limit of [`crate::Limits`] a stream exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    /// `max_mbs`: macroblocks in one picture.
    PictureSize,
    /// `max_dpb_frames`: pictures held in the decoded picture buffer.
    DpbFrames,
    /// `max_slices_per_picture`: slices of one picture.
    SlicesPerPicture,
    /// `max_nal_bytes`: bytes of one NAL unit.
    NalBytes,
    /// `max_memory_bytes`: what the decoder holds for pictures of this
    /// size with one thread.
    Memory,
}

impl LimitKind {
    const fn as_str(self) -> &'static str {
        match self {
            LimitKind::PictureSize => "max_mbs",
            LimitKind::DpbFrames => "max_dpb_frames",
            LimitKind::SlicesPerPicture => "max_slices_per_picture",
            LimitKind::NalBytes => "max_nal_bytes",
            LimitKind::Memory => "max_memory_bytes",
        }
    }
}

/// A reason a stream, or one of its NAL units, could not be decoded.
///
/// Errors carry static reasons only, never bytes from the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A syntax structure ends before the NAL unit's data does.
    Truncated(&'static str),
    /// A syntax element or a value derived from it violates Rec. ITU-T
    /// H.264.
    Invalid(&'static str),
    /// Valid H.264 this decoder does not decode (`docs/design/h264.md`,
    /// "Scope"): field coding, 4:2:2 and 4:4:4, bit depths above 8, data
    /// partitioning, SP and SI slices, slice groups.
    Unsupported(&'static str),
    /// A limit was exceeded.
    Limit(LimitKind),
    /// The decoder asked a kernel for samples outside a picture. This is a
    /// bug in the decoder, never a property of the stream; it is an error
    /// rather than a panic so that no input can stop the process.
    Geometry(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated(what) => write!(f, "the NAL unit ends inside {what}"),
            Error::Invalid(what) => write!(f, "invalid bitstream: {what}"),
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::Limit(kind) => write!(f, "limit exceeded: {}", kind.as_str()),
            Error::Geometry(what) => write!(f, "kernel geometry: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<coreav_kernels_h264::Geometry> for Error {
    fn from(g: coreav_kernels_h264::Geometry) -> Error {
        Error::Geometry(g.what())
    }
}

/// The decoder's result type.
pub type Result<T> = core::result::Result<T, Error>;

/// Fails with [`Error::Invalid`] unless `ok` holds.
#[inline]
pub(crate) fn require(ok: bool, what: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(what))
    }
}
