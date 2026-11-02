//! Checked readers for output from Apple's encoders.
//!
//! These readers make no framework calls. They validate AAC-LC configuration,
//! packet extents, and declared drain counts, not the compressed AAC syntax.

pub mod aac;
