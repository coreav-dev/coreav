//! Decode errors.

use core::fmt;

/// What kind of failure an [`Error`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The packets or the stream configuration are malformed.
    InvalidData,
    /// The stream or an option is outside what the backend supports.
    Unsupported,
    /// The codec is not on the decode allowlist.
    CodecNotAllowed,
    /// A decode limit was reached.
    Limit,
    /// The decoder's deadline passed.
    Timeout,
    /// The backend failed: a decoder error, or a worker that exited or
    /// broke the protocol.
    Backend,
    /// An input or output error outside the decoder (a pipe, a file).
    Io,
}

impl ErrorKind {
    /// A stable lower-case name, for JSON output.
    pub fn name(self) -> &'static str {
        match self {
            Self::InvalidData => "invalid_data",
            Self::Unsupported => "unsupported",
            Self::CodecNotAllowed => "codec_not_allowed",
            Self::Limit => "limit",
            Self::Timeout => "timeout",
            Self::Backend => "backend",
            Self::Io => "io",
        }
    }
}

/// A decode error: its kind and a message for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The kind.
    pub kind: ErrorKind,
    /// What happened.
    pub message: String,
}

impl Error {
    /// An error of `kind`.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.name(), self.message)
    }
}

impl std::error::Error for Error {}

impl From<coreav_frame::FrameError> for Error {
    fn from(e: coreav_frame::FrameError) -> Self {
        let kind = match e {
            coreav_frame::FrameError::OutOfMemory => ErrorKind::Limit,
            coreav_frame::FrameError::Device(_) => ErrorKind::Backend,
            _ => ErrorKind::InvalidData,
        };
        Self::new(kind, e.to_string())
    }
}
