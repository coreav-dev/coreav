//! Checked output readers for the SVT-AV1 encoder backend.
//!
//! This crate currently validates AV1 low-overhead OBU framing only. A valid
//! framing result does not validate sequence configuration, compressed frame
//! syntax, submitted-frame identity, or timestamps and is not sufficient for
//! muxing. No encoder library is linked or called.

pub mod obu;
