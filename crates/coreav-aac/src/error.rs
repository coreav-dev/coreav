//! The error type.

use core::fmt;

/// Why a configuration or a frame was not decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The data breaks the standard (a damaged frame, a truncated
    /// configuration).
    Invalid(&'static str),
    /// Valid, but outside what this crate decodes: the name of what was
    /// found (another object type, SBR, a core coder).
    Unsupported(&'static str),
    /// The configuration has more channels than [`crate::Limits`] allow.
    TooManyChannels {
        /// Channels the configuration has.
        channels: usize,
        /// The most allowed.
        max: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(what) => write!(f, "invalid AAC data: {what}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::TooManyChannels { channels, max } => {
                write!(f, "{channels} channels, more than the limit of {max}")
            }
        }
    }
}

impl std::error::Error for Error {}
