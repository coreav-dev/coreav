//! Arbitrary bytes as a VP9 packet, read by `frame_sizes`: every packet
//! gives sizes or an error, never a panic, and every size is one a 16-bit
//! `frame_width_minus_1` and `frame_height_minus_1` can state. The packet
//! with the index `indexed` gives it, which is what libvpx decodes, states
//! the same sizes. The same bytes as a VP8 packet, read by
//! `vp8_frame_size`: a size or none or an error, never a panic, and a
//! size is one the 14-bit width and height can state, never 0.
#![no_main]

use coreav_backend_libvpx::{frame_sizes, indexed, vp8_frame_size};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let sizes = frame_sizes(data);
    if let Ok(sizes) = &sizes {
        assert!(sizes.len() <= 8);
        for size in sizes {
            assert!((1..=65_536).contains(&size.width));
            assert!((1..=65_536).contains(&size.height));
        }
    }
    if let Ok(with) = indexed(data) {
        assert_eq!(frame_sizes(&with), sizes);
    }
    if let Ok(Some(size)) = vp8_frame_size(data) {
        assert!((1..=16_383).contains(&size.width));
        assert!((1..=16_383).contains(&size.height));
    }
});
