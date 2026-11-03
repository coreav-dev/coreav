//! Checked output readers for the SVT-AV1 encoder backend.
//!
//! The readers validate low-overhead OBU framing, sequence configuration,
//! frame metadata and temporal-unit samples for 8-bit 4:2:0 SDR output.
//! They do not decode entropy-coded tiles or establish source timestamps.
//! The optional native feature links the verified static encoder installation.

pub mod obu;

pub mod frame;
pub mod output;
pub mod protocol;
pub mod sequence;

mod error;
pub mod model;
#[cfg(any(feature = "native", test))]
mod retained;

#[cfg(feature = "native")]
#[allow(unsafe_code)]
mod native;

#[cfg(feature = "native")]
pub mod worker;

pub use error::Error;
