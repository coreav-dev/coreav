//! Errors of the frame model and the frame operations.

use core::fmt;

/// Why a picture could not be built or an operation could not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// A width or height of 0, a depth outside 8 to 16 bits, or a size whose
    /// byte count overflows.
    InvalidGeometry(&'static str),
    /// A plane is missing, too short for its rows, or has a stride shorter
    /// than a row.
    InvalidPlane {
        /// The plane's index.
        plane: usize,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The operation does not apply to this pixel format or these
    /// arguments (for example a crop whose edges fall between chroma
    /// samples).
    Unsupported(&'static str),
    /// Memory for the result could not be reserved.
    OutOfMemory,
    /// Copying device memory to the CPU failed.
    Device(String),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGeometry(what) => write!(f, "invalid picture geometry: {what}"),
            Self::InvalidPlane { plane, reason } => write!(f, "invalid plane {plane}: {reason}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::OutOfMemory => f.write_str("out of memory"),
            Self::Device(message) => write!(f, "device memory: {message}"),
        }
    }
}

impl std::error::Error for FrameError {}
