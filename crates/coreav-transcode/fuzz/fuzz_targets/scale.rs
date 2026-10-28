//! Arbitrary text as `--scale W:H`, and an arbitrary input size: parsing
//! never panics, and every size a parsed value resolves to is within 1 to
//! 16,384 on both sides, the given sides as given (bar B7).
#![no_main]

use coreav_transcode::encoder::MAX_SIZE;
use coreav_transcode::scale::{Side, parse_scale};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let [a, b, c, d, rest @ ..] = data else {
        return;
    };
    let width = u32::from(u16::from_le_bytes([*a, *b]));
    let height = u32::from(u16::from_le_bytes([*c, *d]));
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let Some(scale) = parse_scale(text) else {
        return;
    };
    if let Some((w, h)) = scale.resolve(width, height) {
        assert!((1..=MAX_SIZE).contains(&w) && (1..=MAX_SIZE).contains(&h));
        if let Side::Pixels(given) = scale.width {
            assert_eq!(given, w);
        }
        if let Side::Pixels(given) = scale.height {
            assert_eq!(given, h);
        }
        if scale.width == Side::KeepEven && scale.height != Side::Keep
            && scale.height != Side::KeepEven
        {
            assert_eq!(w % 2, 0);
        }
        if scale.height == Side::KeepEven && scale.width != Side::Keep
            && scale.width != Side::KeepEven
        {
            assert_eq!(h % 2, 0);
        }
    }
});
