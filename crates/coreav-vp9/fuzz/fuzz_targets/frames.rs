//! Packet sequences through the pixel decoder, including references,
//! adaptation, failures and shown-reference frames.
#![no_main]

use coreav_vp9::{FrameDecoder, Limits, Samples};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut a = FrameDecoder::new(Limits::for_size(128, 128));
    let mut b = FrameDecoder::new(Limits::for_size(128, 128));
    let mut rest = data;
    while let Some((len, after)) = rest.split_first_chunk::<2>() {
        let len = usize::from(u16::from_le_bytes(*len)).min(after.len());
        let (packet, after) = after.split_at(len);
        rest = after;
        let result = a.decode(packet);
        assert_eq!(result, b.decode(packet));
        if let Ok(pictures) = result {
            for p in pictures {
                assert!(p.width <= 128 && p.height <= 128);
                for plane in &p.planes {
                    if let Samples::Eight(samples) = plane {
                        assert!(samples.len() <= 128 * 128);
                    } else {
                        panic!("profile 0 samples");
                    }
                }
            }
        }
    }
});
