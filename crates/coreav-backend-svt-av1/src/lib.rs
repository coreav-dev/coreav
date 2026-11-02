//! Checked output readers for the SVT-AV1 encoder backend.
//!
//! The readers validate AV1 low-overhead OBU framing and sequence-header
//! configuration for 8-bit 4:2:0 SDR output. They do not validate compressed
//! frame syntax, submitted-frame identity, or timestamps. These checks alone
//! are not sufficient for muxing. No encoder library is linked or called.

pub mod obu;

pub mod sequence;
