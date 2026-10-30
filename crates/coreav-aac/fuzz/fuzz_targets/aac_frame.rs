//! The AAC LC decoder on arbitrary frames under an arbitrary
//! configuration: the first byte is the length of an
//! `AudioSpecificConfig` that follows (at most 63 bytes, so a program
//! configuration fits); the rest is cut into frames, each preceded by its
//! length in two bytes (little-endian), and decoded in order, as packets
//! would be. A length of 0xFFFF sets the stream position from the next
//! eight bytes instead, as a decoding thread does.
#![no_main]

use coreav_aac::{Config, Decoder, Limits};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&first, rest)) = data.split_first() else {
        return;
    };
    let length = usize::from(first % 64).min(rest.len());
    let (config, mut rest) = rest.split_at(length);
    let Ok(config) = Config::parse(config, &Limits::default()) else {
        return;
    };
    let mut decoder = Decoder::new(&config);
    let mut out = vec![0f32; config.channels() * config.frame_length()];
    while let Some((length, tail)) = rest.split_first_chunk::<2>() {
        let length = u16::from_le_bytes(*length);
        if length == u16::MAX {
            let Some((position, tail)) = tail.split_first_chunk::<8>() else {
                return;
            };
            decoder.set_position(u64::from_le_bytes(*position));
            rest = tail;
            continue;
        }
        let (frame, tail) = tail.split_at(usize::from(length).min(tail.len()));
        let _ = decoder.decode(frame, &mut out);
        rest = tail;
    }
});
