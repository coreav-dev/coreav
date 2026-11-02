//! Raw PCM framing, resource budgets, byte identity and captured rate outputs.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use coreav_audio::raw::{RawS16Format, RawS16Plan};
use coreav_audio::{Error, LimitKind, Limits, OutputFormat, Sink};
use coreav_isobmff::{SliceSource, Source};

#[derive(Default)]
struct Keep {
    format: Option<OutputFormat>,
    bytes: Vec<u8>,
    largest_write: usize,
}

impl Sink for Keep {
    fn start(&mut self, format: &OutputFormat) -> Result<(), Error> {
        assert!(self.format.is_none());
        self.format = Some(*format);
        Ok(())
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.largest_write = self.largest_write.max(bytes.len());
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

struct Watched<'a> {
    source: SliceSource<'a>,
    largest_read: usize,
    reads: usize,
}

impl Source for Watched<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        self.source.len()
    }
    fn read_exact_at(
        &mut self,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<(), coreav_isobmff::Error> {
        self.largest_read = self.largest_read.max(bytes.len());
        self.reads += 1;
        self.source.read_exact_at(offset, bytes)
    }
}

#[test]
fn equal_rate_is_an_exact_bounded_integer_copy() {
    let input: Vec<u8> = (0..200_000u32).map(|n| n.wrapping_mul(73) as u8).collect();
    for channels in [1, 2] {
        for rate in [None, Some(44100)] {
            let mut source = Watched {
                source: SliceSource::new(&input),
                largest_read: 0,
                reads: 0,
            };
            let mut sink = Keep::default();
            let plan = RawS16Plan::new(
                RawS16Format::new(44100, channels).unwrap(),
                rate,
                input.len() as u64,
                &Limits::default(),
            )
            .unwrap();
            let summary = plan.run(&mut source, &mut sink).unwrap();
            assert_eq!(sink.bytes, input);
            assert_eq!(
                summary.input_frames,
                input.len() as u64 / (u64::from(channels) * 2)
            );
            assert_eq!(summary.output_frames, summary.input_frames);
            assert!(source.largest_read <= 65536);
            assert!(source.reads <= 4);
            assert!(sink.largest_write <= 65536);
        }
    }
}

#[test]
fn framing_and_budgets_fail_before_the_sink_starts() {
    let format = RawS16Format::new(8000, 2).unwrap();
    assert!(matches!(
        RawS16Plan::new(format, None, 3, &Limits::default()),
        Err(Error::Invalid(_))
    ));
    let limits = Limits {
        max_output_samples: 3,
        ..Limits::default()
    };
    assert!(matches!(
        RawS16Plan::new(format, None, 8, &limits),
        Err(Error::LimitExceeded(LimitKind::OutputSamples))
    ));
    let limits = Limits {
        max_channels: 1,
        ..Limits::default()
    };
    assert!(matches!(
        RawS16Plan::new(format, None, 4, &limits),
        Err(Error::LimitExceeded(LimitKind::Channels))
    ));
    let limits = Limits {
        max_packet_bytes: 127,
        ..Limits::default()
    };
    assert!(matches!(
        RawS16Plan::new(format, Some(44100), 4096, &limits),
        Err(Error::LimitExceeded(LimitKind::PacketBytes))
    ));
    assert!(RawS16Format::new(0, 2).is_err());
    assert!(RawS16Format::new(44100, 3).is_err());
    assert!(RawS16Plan::new(format, Some(44099), 4096, &Limits::default()).is_err());
}

#[test]
fn source_size_is_rechecked_before_starting_output() {
    let plan = RawS16Plan::new(
        RawS16Format::new(44100, 1).unwrap(),
        None,
        2,
        &Limits::default(),
    )
    .unwrap();
    let mut sink = Keep::default();
    assert!(plan.run(SliceSource::new(&[]), &mut sink).is_err());
    assert!(sink.format.is_none());
}

#[test]
fn rate_conversion_bounds_its_reads_and_writes() {
    let input: Vec<u8> = (0..200_000u32).map(|n| n.wrapping_mul(73) as u8).collect();
    let mut source = Watched {
        source: SliceSource::new(&input),
        largest_read: 0,
        reads: 0,
    };
    let mut sink = Keep::default();
    let plan = RawS16Plan::new(
        RawS16Format::new(8000, 2).unwrap(),
        Some(44100),
        input.len() as u64,
        &Limits::default(),
    )
    .unwrap();
    let frames = plan.output_frames();
    let summary = plan.run(&mut source, &mut sink).unwrap();
    assert_eq!(summary.output_frames, frames);
    assert_eq!(sink.bytes.len() as u64, frames * 4);
    assert!(source.largest_read <= 65536);
    assert!(source.reads <= 4);
    assert!(sink.largest_write <= 65536);
}

#[test]
#[ignore = "needs COREAV_SWR_REPLAY from scripts/raw_pcm_check.py; run in audio-conformance"]
fn replay_the_captured_raw_s16_cases() {
    let manifest = std::fs::read_to_string(std::env::var_os("COREAV_SWR_REPLAY").unwrap()).unwrap();
    let mut count = 0;
    for line in manifest.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        let input = std::fs::read(fields[0]).unwrap();
        let expected = std::fs::read(fields[1]).unwrap();
        let input_rate = fields[2].parse().unwrap();
        let output_rate = fields[3].parse().unwrap();
        let channels = fields[4].parse().unwrap();
        let plan = RawS16Plan::new(
            RawS16Format::new(input_rate, channels).unwrap(),
            Some(output_rate),
            input.len() as u64,
            &Limits::default(),
        )
        .unwrap();
        let mut sink = Keep::default();
        plan.run(SliceSource::new(&input), &mut sink).unwrap();
        assert_eq!(
            sink.bytes, expected,
            "{} -> {}, {}",
            input_rate, output_rate, fields[0]
        );
        count += 1;
    }
    assert_eq!(count, 56);
}
