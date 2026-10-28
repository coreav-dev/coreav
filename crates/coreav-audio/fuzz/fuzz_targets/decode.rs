//! The whole pipeline on an arbitrary file: container detection, track
//! choice, demuxing, decoding and conversion, resampled and downmixed for
//! some inputs, into a sink that drops the samples. The output limit is
//! lowered so that one input stays fast.
#![no_main]

use coreav_audio::{Error, Limits, Options, OutputFormat, SampleFormat, Sink, decode};
use coreav_isobmff::SliceSource;
use libfuzzer_sys::fuzz_target;

struct Drop;

impl Sink for Drop {
    fn start(&mut self, _: &OutputFormat) -> Result<(), Error> {
        Ok(())
    }
    fn write(&mut self, _: &[u8]) -> Result<(), Error> {
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_output_samples: 1 << 22,
        ..Limits::default()
    };
    let format = match data.last().copied().unwrap_or(0) % 4 {
        0 => SampleFormat::S16,
        1 => SampleFormat::S24,
        2 => SampleFormat::S32,
        _ => SampleFormat::F32,
    };
    let threads = 1 + usize::from(data.first().copied().unwrap_or(0) % 2) * 3;
    // The second byte picks a conversion: none, 16 kHz mono, 48 kHz, a
    // rate prime to the common ones, or stereo.
    let (sample_rate, channels) = match data.get(1).copied().unwrap_or(0) % 5 {
        0 => (None, None),
        1 => (Some(16_000), Some(1)),
        2 => (Some(48_000), None),
        3 => (Some(7_919), Some(1)),
        _ => (None, Some(2)),
    };
    let options = Options {
        stream: None,
        format,
        threads,
        sample_rate,
        channels,
    };
    let _ = decode(SliceSource::new(data), &options, &limits, &mut Drop);
});
