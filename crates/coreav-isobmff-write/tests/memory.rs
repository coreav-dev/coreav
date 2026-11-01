//! The pinned memory bound of the limits on the writer's paths:
//! a stream copy of a 2.2 GB fragmented file grows memory by under 32 MiB
//! (`docs/design/limits.md`).
//!
//! The writer reads the whole file and writes an output as large, so what
//! it may hold is one copy window, the cut points it has found and the
//! output's sample tables (which `max_table_bytes` bounds), and nothing
//! that follows the file's size.
// Test-only helpers outside `#[test]` functions index their own buffers.
#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

use common::fragments::{Fragmented, Track};
use coreav_counting_alloc::{Counters, CountingAllocator, measure, report};
use coreav_isobmff::{Allowlist, BufferedSource, Limits, MAX_READ, Rational, Source};
use coreav_isobmff_write::{
    AudioChoice, ClipOptions, HeaderMetadata, SplitAt, SplitOptions, TrackChoice, VideoChoice,
    clip_mp4, extract_audio, split_mp4,
};
use std::io::{self, Write};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

/// Pinned bound on peak memory growth on the 2.2 GB fragmented file.
const COPY_PEAK: u64 = 32 << 20;

/// Fragments of the pinned file; the whole file is 2.2 GB.
const FRAGMENTS: u64 = 20_000;

/// Cut every minute of presentation time: 134 segments of the pinned file.
const SEGMENT_SECONDS: i128 = 60;

/// Checks a measured region against the bar. A region that made no
/// allocator call at all would satisfy every bound here, so it is refused
/// first: that is what a bar looks like when the allocator is not
/// installed. A trough of zero then says the region never freed memory
/// allocated before it, so its peak is growth and nothing that was offset
/// against an earlier free.
fn under_bar(what: &str, counters: &Counters) {
    report(what, counters);
    assert!(counters.calls() > 0, "{what}: measured nothing");
    assert!(counters.peak_bytes < COPY_PEAK, "{what}: {counters}");
    assert_eq!(counters.trough_bytes, 0, "{what}: {counters}");
}

/// An output that keeps nothing: the bytes written would otherwise be the
/// measurement, and a 2.2 GB output does not fit in a test's memory.
#[derive(Default)]
struct Sink {
    total: u64,
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.total += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The H.264 and AAC fragmented movie, with its video samples scaled to
/// `size` bytes. The sample count is the same whatever the size, which is
/// what bar 2 of `docs/design/limits.md` varies.
fn source(fragments: u64, size: u32) -> BufferedSource<Fragmented> {
    let video = Track {
        size,
        ..Track::H264
    };
    BufferedSource::new(Fragmented::new(&[video, Track::AAC], fragments))
}

/// One measured copy of the fragmented movie: how much it wrote (bytes,
/// or segments for a split) and the memory it grew by.
type Measured = (u64, Counters);

/// One of the copies below, by `fragments` and video sample size.
type Measure = fn(u64, u32) -> Measured;

/// Copies the audio track out, returning the bytes written and the memory
/// the copy grew by.
fn extract(fragments: u64, size: u32) -> Measured {
    let mut source = source(fragments, size);
    let mut out = Sink::default();
    let (summary, counters) = measure(|| {
        extract_audio(
            &mut source,
            &Limits::default(),
            &Allowlist::default(),
            TrackChoice::FirstAudio,
            HeaderMetadata::Copy,
            &mut out,
        )
        .unwrap()
    });
    assert_eq!(
        summary.samples as u64,
        fragments * u64::from(Track::AAC.samples)
    );
    assert_eq!(summary.bytes_written, out.total);
    assert!(source.largest_read() <= MAX_READ);
    (out.total, counters)
}

/// Clips the whole video track, returning the bytes written and the memory
/// the clip grew by.
fn clip(fragments: u64, size: u32) -> Measured {
    let mut source = source(fragments, size);
    let mut out = Sink::default();
    let options = ClipOptions {
        video: VideoChoice::First,
        audio: AudioChoice::First,
        start: Rational::from_int(0),
        end: None,
    };
    let (summary, counters) = measure(|| {
        clip_mp4(
            &mut source,
            &Limits::default(),
            &Allowlist::default(),
            &options,
            &mut out,
        )
        .unwrap()
    });
    assert_eq!(
        summary.video.samples as u64,
        fragments * u64::from(Track::H264.samples)
    );
    assert_eq!(summary.bytes_written, out.total);
    assert!(source.largest_read() <= MAX_READ);
    (out.total, counters)
}

/// Splits the whole file into one-minute segments, returning the segments
/// written and the memory the split grew by.
fn split(fragments: u64, size: u32) -> Measured {
    let mut source = source(fragments, size);
    let options = SplitOptions {
        video: VideoChoice::First,
        audio: AudioChoice::First,
        at: SplitAt::Every(Rational::from_int(SEGMENT_SECONDS)),
    };
    let (summary, counters) = measure(|| {
        split_mp4(
            &mut source,
            &Limits::default(),
            &Allowlist::default(),
            &options,
            |_| Ok(Sink::default()),
        )
        .unwrap()
    });
    let samples: u64 = summary
        .segments
        .iter()
        .map(|s| u64::from(s.video.samples))
        .sum();
    assert_eq!(samples, fragments * u64::from(Track::H264.samples));
    assert!(source.largest_read() <= MAX_READ);
    (summary.segments.len() as u64, counters)
}

#[test]
#[ignore = "copies a 2.2 GB virtual file; run with --ignored --release"]
fn extracting_audio_from_a_2_2_gb_fragmented_file_stays_under_32_mib() {
    let (bytes, counters) = extract(FRAGMENTS, Track::H264.size);
    under_bar("extract-audio/fragmented-2.2gb", &counters);
    println!("extract-audio/fragmented-2.2gb: {bytes} bytes written");
    assert!(bytes > 100 << 20);
}

#[test]
#[ignore = "copies a 2.2 GB virtual file; run with --ignored --release"]
fn clipping_a_2_2_gb_fragmented_file_stays_under_32_mib() {
    let total = source(FRAGMENTS, Track::H264.size).len().unwrap();
    let (bytes, counters) = clip(FRAGMENTS, Track::H264.size);
    under_bar("clip/fragmented-2.2gb", &counters);
    println!("clip/fragmented-2.2gb: source {total} bytes, {bytes} written");
    assert!(total > 2_200_000_000 && bytes > 2_200_000_000);
}

#[test]
#[ignore = "copies a 2.2 GB virtual file; run with --ignored --release"]
fn splitting_a_2_2_gb_fragmented_file_stays_under_32_mib() {
    let (segments, counters) = split(FRAGMENTS, Track::H264.size);
    under_bar("split/fragmented-2.2gb", &counters);
    println!("split/fragmented-2.2gb: {segments} segments");
    assert!(segments > 100);
}

/// Bar 2 of `docs/design/limits.md`: a stream copy's memory follows the
/// samples it copies, never the file's bytes. Ten times smaller video
/// samples, the same sample count: the same peak, give or take one read
/// window.
#[test]
fn stream_copy_memory_does_not_follow_the_file_size() {
    let fragments = 200;
    let big = Track::H264.size;
    let small = big / 10;
    let (fat_file, thin_file) = (
        source(fragments, big).len().unwrap(),
        source(fragments, small).len().unwrap(),
    );
    println!("stream-copy scaling: sources {fat_file} and {thin_file} bytes");
    assert!(fat_file > thin_file * 5);
    for (name, copy) in [
        ("extract-audio", extract as Measure),
        ("clip", clip),
        ("split", split),
    ] {
        let (fat_out, fat) = copy(fragments, big);
        let (thin_out, thin) = copy(fragments, small);
        under_bar(&format!("{name}/video-samples-{big}b"), &fat);
        under_bar(&format!("{name}/video-samples-{small}b"), &thin);
        assert!(fat_out >= thin_out, "{name}");
        assert!(
            fat.peak_bytes <= thin.peak_bytes + MAX_READ as u64,
            "{name}: {big} B samples peaked at {}, {small} B at {}",
            fat.peak_bytes,
            thin.peak_bytes
        );
    }
}
