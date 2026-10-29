//! Arbitrary bytes as a VP9 packet, read by `frame_sizes`: every packet
//! gives sizes or an error, never a panic, and every size is one a 16-bit
//! `frame_width_minus_1` and `frame_height_minus_1` can state. The packet
//! with the index `indexed` gives it, which is what libvpx decodes, states
//! the same sizes.
#![no_main]

use coreav_backend_libvpx::{frame_sizes, indexed};
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
});
