#![no_main]

use coreav_mkv::{Limits, SliceSource};
use coreav_transcode::mp3::{Expected, MAX_FRAME_BYTES, read_output};
use libfuzzer_sys::fuzz_target;

const RATES: [u32; 9] = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000];

fuzz_target!(|data: &[u8]| {
    let [rate, channels, a, b, rest @ ..] = data else { return; };
    let Some(&sample_rate) = RATES.get(usize::from(*rate) % RATES.len()) else { return; };
    let expected = Expected { sample_rate, channels: 1 + (channels & 1),
        samples: u64::from(u16::from_le_bytes([*a, *b])), guard: 0, variable: channels & 2 != 0 };
    let Ok(output) = read_output(SliceSource::new(rest), &expected, &Limits::default()) else { return; };
    assert!(!output.frames.is_empty());
    assert!(matches!(output.frame_samples, 576 | 1152));
    let len = u64::try_from(rest.len()).unwrap_or(u64::MAX);
    for frame in &output.frames {
        assert!((4..=MAX_FRAME_BYTES).contains(&frame.size));
        assert!(frame.offset.checked_add(u64::from(frame.size)).is_some_and(|end| end <= len));
    }
});
