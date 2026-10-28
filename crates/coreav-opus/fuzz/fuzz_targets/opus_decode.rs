//! The decoder on arbitrary packets: the first byte picks the output
//! channels; the rest is a sequence of packets, each preceded by its length
//! in one byte, decoded by one decoder so that state carries across them.
#![no_main]

use coreav_opus::Decoder;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&first, mut rest)) = data.split_first() else {
        return;
    };
    let mut decoder = Decoder::new(1 + (first & 1)).unwrap();
    let mut pcm = Vec::new();
    while let Some((&len, tail)) = rest.split_first() {
        let Some((packet, tail)) = tail.split_at_checked(usize::from(len)) else {
            return;
        };
        rest = tail;
        let before = pcm.len();
        match decoder.decode(packet, &mut pcm) {
            Ok(samples) => {
                assert!(samples <= 5760);
                assert_eq!(pcm.len() - before, samples * decoder.channels());
                // SILK stays within full scale; CELT's floating-point
                // synthesis may overshoot it, but stays finite.
                assert!(pcm[before..].iter().all(|x| x.is_finite()));
            }
            Err(_) => assert_eq!(pcm.len(), before),
        }
        pcm.clear();
    }
});
