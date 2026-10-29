//! Arbitrary bytes as a VP9 packet, read by `frame_sizes`: every packet
//! gives sizes or an error, never a panic, and every size is one a 16-bit
//! `frame_width_minus_1` and `frame_height_minus_1` can state.
#![no_main]

use coreav_backend_libvpx::frame_sizes;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(sizes) = frame_sizes(data) {
        assert!(sizes.len() <= 8);
        for size in sizes {
            assert!((1..=65_536).contains(&size.width));
            assert!((1..=65_536).contains(&size.height));
        }
    }
});
