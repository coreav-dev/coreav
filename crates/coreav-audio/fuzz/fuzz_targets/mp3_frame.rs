//! The MP3 decoder on arbitrary frames: the first byte picks the stated
//! channel count (none, one or two), the rest is decoded frame after frame
//! (skipping a byte after an error), as packets would be, so that each
//! frame's bit reservoir holds the earlier frames' main data.
#![no_main]

use coreav_audio::mp3::Mp3Decoder;
use coreav_audio::{Limits, Pcm};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&first, mut rest)) = data.split_first() else {
        return;
    };
    let channels = match first % 3 {
        0 => None,
        n => Some(u16::from(n)),
    };
    let Ok(mut decoder) = Mp3Decoder::new(channels, &Limits::default()) else {
        return;
    };
    let mut pcm = Pcm::new();
    while !rest.is_empty() {
        let step = match decoder.decode(rest, &mut pcm) {
            Ok(frame) => frame.bytes.max(1),
            Err(_) => 1,
        };
        rest = rest.get(step..).unwrap_or_default();
    }
});
