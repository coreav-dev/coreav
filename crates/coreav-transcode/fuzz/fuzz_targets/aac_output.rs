//! Arbitrary bytes as the AAC encoder's output (policy record 0042,
//! point 3; docs/design/transcode.md, "AAC audio"), for a rate, channels
//! and samples sent from the first bytes: the reader never panics, and
//! what it takes is within its bounds: a delay of at most two frames, as
//! many frames as cover the delay and the samples or one more, each of 1
//! to 8,184 bytes and within the output.
#![no_main]

use coreav_mkv::{Limits, SliceSource};
use coreav_transcode::aac::{
    Expected, FRAME_SAMPLES, MAX_FRAME_BYTES, MAX_PRIMING, read_output,
};
use libfuzzer_sys::fuzz_target;

const RATES: [u32; 10] = [
    64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
];

fuzz_target!(|data: &[u8]| {
    let [rate, channels, a, b, rest @ ..] = data else {
        return;
    };
    let Some(&sample_rate) = RATES.get(usize::from(*rate) % RATES.len()) else {
        return;
    };
    let channels = 1 + (channels & 1);
    let samples = u64::from(u16::from_le_bytes([*a, *b]));
    let expected = Expected {
        sample_rate,
        channels,
        samples,
    };
    let Ok(output) = read_output(SliceSource::new(rest), &expected, &Limits::default()) else {
        return;
    };
    assert!(output.priming <= MAX_PRIMING);
    let needed = (u64::from(output.priming) + samples).div_ceil(u64::from(FRAME_SAMPLES));
    let count = u64::try_from(output.frames.len()).unwrap_or(u64::MAX);
    assert!(count == needed || count == needed + 1);
    let len = u64::try_from(rest.len()).unwrap_or(u64::MAX);
    for frame in &output.frames {
        assert!((1..=MAX_FRAME_BYTES).contains(&frame.size));
        assert!(frame.offset + u64::from(frame.size) <= len);
    }
});
