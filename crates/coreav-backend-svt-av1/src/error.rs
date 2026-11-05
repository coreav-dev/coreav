//! Failures at the encoder's parameter, native, and wire boundaries.

use std::{fmt, io};

/// A terminal encoder failure. A failed session is never retried.
#[derive(Debug)]
pub enum Error {
    /// Invalid parameters or an invalid state transition.
    Invalid(&'static str),
    /// A valid request outside this backend's supported subset.
    Unsupported(&'static str),
    /// A checked resource or allocation limit.
    Limit(&'static str),
    /// The native API returned this error code.
    Native(u32),
    /// Native output or worker records violated their contract.
    Output(&'static str),
    /// An input or output operation failed.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message)
            | Self::Unsupported(message)
            | Self::Limit(message)
            | Self::Output(message) => f.write_str(message),
            Self::Native(code) => write!(f, "SVT-AV1 failed with native status 0x{code:08x}"),
            Self::Io(error) => write!(f, "SVT-AV1 transport: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
