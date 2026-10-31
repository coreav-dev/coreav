//! The decoder of a whole track on arbitrary packets after arbitrary earlier
//! ones, under every channel mapping: the first byte is the length of an
//! identification header's fields after `OpusHead` (any family, stream
//! count and table), the bytes after it are the track's samples, each
//! preceded by its length in two bytes, decoded by one decoder so that
//! state carries across them.
#![no_main]

use coreav_opus::{Head, MAX_PACKET_SAMPLES, MultistreamDecoder};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&len, rest)) = data.split_first() else {
        return;
    };
    let Some((fields, mut rest)) = rest.split_at_checked(usize::from(len)) else {
        return;
    };
    let config = [b"OpusHead".as_slice(), fields].concat();
    let Ok(head) = Head::from_config(&config) else {
        return;
    };
    let Ok(mut decoder) = MultistreamDecoder::new(&head) else {
        return;
    };
    let mut planes = Vec::new();
    while let [a, b, tail @ ..] = rest {
        let Some((sample, tail)) = tail.split_at_checked(usize::from(u16::from_le_bytes([*a, *b])))
        else {
            return;
        };
        rest = tail;
        match decoder.decode(sample, &mut planes) {
            Ok(samples) => {
                assert!(samples <= MAX_PACKET_SAMPLES as usize);
                assert_eq!(planes.len(), samples * decoder.channels());
                assert!(planes.iter().all(|x| x.is_finite()));
            }
            Err(_) => assert!(planes.is_empty()),
        }
    }
});
