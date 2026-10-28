//! Arbitrary bytes as a file through `coreav_thumbnail::thumbnail` without
//! a decoder backend: container detection, probing, the choice of stream
//! and frame (both modes, an arbitrary time), the cut-point readers and
//! backend selection, which then fails. Every input ends in an error,
//! never a panic.
#![no_main]

use coreav_isobmff::{Rational, SliceSource};
use coreav_thumbnail::{ThumbnailOptions, thumbnail};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let mut options = ThumbnailOptions::at(Rational::new(i128::from(selector & 0x7F), 4).unwrap_or(Rational::from_int(0)));
    options.exact = selector & 0x80 != 0;
    let result = thumbnail(SliceSource::new(rest), &options, &[]);
    assert!(result.is_err());
});
