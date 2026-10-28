//! The FLAC decoder on arbitrary frames under arbitrary streaminfo values:
//! the first 34 bytes are the streaminfo body, the rest is decoded frame
//! after frame (skipping a byte after an error), as a packet would be.
#![no_main]

use coreav_audio::flac::{FlacDecoder, StreamInfo};
use coreav_audio::{Limits, Pcm};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((head, mut rest)) = data.split_at_checked(StreamInfo::LEN) else {
        return;
    };
    let Ok(info) = StreamInfo::parse(head) else {
        return;
    };
    let Ok(mut decoder) = FlacDecoder::new(info, &Limits::default()) else {
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
