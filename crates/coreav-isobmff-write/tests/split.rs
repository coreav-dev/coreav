//! Split round trips (`docs/design/split.md`): every segment is probed by
//! `coreav-isobmff`; together the segments must hold every sample of each
//! kept track exactly once and in order, with the source's bytes, sizes,
//! durations, sync flags and composition offsets; every sample a segment
//! presents must be presented in the source at its segment time plus the
//! segment's start, and every frame the source presents must be presented
//! by its segment.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;
mod movies;

use coreav_isobmff::{
    Allowlist, Budget, EditList, Error, Limits, Movie, Packet, Rational, SliceSource, Source,
    TrackKind, probe,
};
use coreav_isobmff_write::{
    AudioChoice, ExtractError, Segment, SplitAt, SplitOptions, SplitSummary, VideoChoice, split_mp4,
};
use movies::{
    S, TrackSpec, aac_audio, av_movie, h264_slices, ibbp_video, movie, without_sync_table,
    written_sync,
};
use std::cell::RefCell;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;

fn q(num: i128, den: u64) -> Rational {
    Rational::new(num, den).unwrap()
}

fn every(step: Rational) -> SplitOptions {
    SplitOptions {
        video: VideoChoice::First,
        audio: AudioChoice::First,
        at: SplitAt::Every(step),
    }
}

fn at(times: Vec<Rational>) -> SplitOptions {
    SplitOptions {
        video: VideoChoice::First,
        audio: AudioChoice::First,
        at: SplitAt::Times(times),
    }
}

/// A writer appending to one of the collected outputs.
struct Sink(Rc<RefCell<Vec<Vec<u8>>>>, usize);

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut()[self.1].extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn split(
    source: &[u8],
    options: &SplitOptions,
) -> Result<(SplitSummary, Vec<Vec<u8>>), ExtractError> {
    split_with(source, options, &Limits::default())
}

fn split_with(
    source: &[u8],
    options: &SplitOptions,
    limits: &Limits,
) -> Result<(SplitSummary, Vec<Vec<u8>>), ExtractError> {
    let outputs = Rc::new(RefCell::new(Vec::new()));
    let opened = Rc::clone(&outputs);
    let summary = split_mp4(
        SliceSource::new(source),
        limits,
        &Allowlist::default(),
        options,
        move |index| {
            let mut all = opened.borrow_mut();
            assert_eq!(index, all.len());
            all.push(Vec::new());
            Ok(Sink(Rc::clone(&opened), index))
        },
    );
    let outputs = outputs.borrow().clone();
    let summary = summary?;
    assert_eq!(summary.segments.len(), outputs.len());
    for (segment, output) in summary.segments.iter().zip(&outputs) {
        assert_eq!(segment.bytes_written, output.len() as u64);
    }
    Ok((summary, outputs))
}

fn probe_all(data: &[u8]) -> Movie {
    probe(
        SliceSource::new(data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap()
}

fn packets(data: &[u8], movie: &Movie, index: usize) -> Vec<Packet> {
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    movie
        .packets(index, SliceSource::new(data), &limits, &mut budget)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn bytes(data: &[u8], packet: &Packet) -> Vec<u8> {
    let mut buf = vec![0; packet.size as usize];
    SliceSource::new(data)
        .read_exact_at(packet.offset, &mut buf)
        .unwrap();
    buf
}

fn ticks(seconds: Rational, timescale: u32) -> Rational {
    q(seconds.num * i128::from(timescale), seconds.den)
}

fn diff(a: Rational, b: Rational) -> f64 {
    a.num as f64 / a.den as f64 - b.num as f64 / b.den as f64
}

/// The partition property: the segments hold every sample of each kept
/// track exactly once, in order, with the source's data and timing.
fn assert_partition(source: &[u8], outputs: &[Vec<u8>], summary: &SplitSummary) {
    let source_movie = probe_all(source);
    let segments = &summary.segments;
    assert!(!segments.is_empty());
    // The segments tile the video's presentation: each starts where the
    // previous one ends. A segment's video starts later than the segment
    // itself only where the source presents nothing there: a gap between
    // two windows of a source with several edits, which opens the segment
    // with an empty edit, and in the first segment audio before the video.
    for pair in segments.windows(2) {
        assert_eq!(pair[0].end, pair[1].start);
        assert!(!pair[1].video_start.checked_lt(pair[1].start).unwrap());
    }
    let movies: Vec<Movie> = outputs.iter().map(|o| probe_all(o)).collect();
    for movie in &movies {
        assert!(!movie.has_fragments());
        assert_eq!(movie.tracks[0].kind, TrackKind::Video);
    }
    let video_stream = segments[0].video.stream_index;
    assert_track(
        source,
        &source_movie,
        video_stream,
        outputs,
        &movies,
        segments,
        TrackKind::Video,
    );
    if let Some(audio) = segments.iter().find_map(|s| s.audio) {
        assert_track(
            source,
            &source_movie,
            audio.stream_index,
            outputs,
            &movies,
            segments,
            TrackKind::Audio,
        );
    }
}

fn assert_track(
    source: &[u8],
    source_movie: &Movie,
    stream_index: usize,
    outputs: &[Vec<u8>],
    movies: &[Movie],
    segments: &[Segment],
    kind: TrackKind,
) {
    let src_track = source_movie
        .tracks
        .iter()
        .find(|t| t.stream_index == stream_index)
        .unwrap();
    let all = packets(source, source_movie, src_track.index);
    // A track the container marks sync throughout gets the coded data's
    // flags; any other keeps the source's (docs/design/cut-points.md).
    let marked = all.iter().all(|p| p.keyframe);
    let mut next = 0usize;
    for (n, ((segment, output), movie)) in segments.iter().zip(outputs).zip(movies).enumerate() {
        let kept = match kind {
            TrackKind::Video => Some(segment.video),
            _ => segment.audio,
        };
        let Some(kept) = kept else {
            assert_eq!(movie.tracks.len(), 1, "segment {n}");
            continue;
        };
        assert_eq!(kept.stream_index, stream_index);
        assert_eq!(kept.first_sample, next as u64, "segment {n}");
        let out_track = movie.tracks.iter().find(|t| t.kind == kind).unwrap();
        assert_eq!(out_track.timescale, src_track.timescale);
        assert_eq!(
            out_track.description.codec_config,
            src_track.description.codec_config
        );
        let written = packets(output, movie, out_track.index);
        assert_eq!(written.len(), kept.samples as usize);
        let run = &all[next..next + written.len()];
        let start = ticks(segment.start, src_track.timescale);
        for (s, o) in run.iter().zip(&written) {
            assert_eq!(o.size, s.size);
            assert_eq!(o.duration, s.duration);
            assert_eq!(
                o.keyframe,
                if marked {
                    written_sync(source, source_movie, src_track, s)
                } else {
                    s.keyframe
                },
                "sample {}",
                s.sample
            );
            assert_eq!(o.dts, s.dts - run[0].dts);
            assert_eq!(o.cts - o.dts, s.cts - s.dts);
            assert_eq!(bytes(output, o), bytes(source, s));
            if o.presented {
                assert!(
                    s.presented,
                    "sample {} presented only in segment {n}",
                    s.sample
                );
                let gap = diff(o.pts.unwrap().checked_add(start).unwrap(), s.pts.unwrap());
                if kind == TrackKind::Video {
                    assert_eq!(gap, 0.0, "sample {} in segment {n}", s.sample);
                } else {
                    assert!(gap.abs() <= 1.0, "sample {}: {gap} ticks", s.sample);
                }
            } else if s.presented {
                // Only frames past the video's end may stay unpresented.
                let end = ticks(segments[segments.len() - 1].end, src_track.timescale);
                assert!(
                    kind == TrackKind::Video && !s.pts.unwrap().checked_lt(end).unwrap(),
                    "sample {} presented in the source but not in segment {n}",
                    s.sample
                );
            }
        }
        next += written.len();
    }
    assert_eq!(next, all.len(), "every sample exactly once");
}

#[test]
fn every_second_cuts_at_each_gop() {
    let source = av_movie();
    let (summary, outputs) = split(&source, &every(q(1, 1))).unwrap();
    assert_eq!(summary.segments.len(), 4);
    let starts: Vec<Rational> = summary.segments.iter().map(|s| s.video_start).collect();
    assert_eq!(starts, [q(0, 1), q(1, 1), q(2, 1), q(3, 1)]);
    for segment in &summary.segments {
        assert_eq!(segment.video.samples, 25);
    }
    assert_partition(&source, &outputs, &summary);
    // The first segment keeps the source's audio priming; the others start
    // with the first audio frame whose presentation starts at or after their
    // cut, after an empty edit shorter than one frame.
    let first = probe_all(&outputs[0]);
    assert!(matches!(
        first.tracks[1].edit_list,
        EditList::Simple {
            empty: 0,
            media_time: 2112,
            ..
        }
    ));
    let second = probe_all(&outputs[1]);
    match second.tracks[1].edit_list {
        EditList::Simple {
            empty, media_time, ..
        } => {
            assert_eq!(media_time, 0);
            // The first frame presented at or after 1 s (48,000 ticks) is
            // frame 49, at 49 * 1024 - 2112 = 48,064 ticks: 64 ticks after
            // the cut, at 192,000 movie ticks per second (the least common
            // multiple of 12,800 and 48,000).
            assert_eq!(empty, 64 * 4);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn targets_snap_to_the_latest_cut_at_or_before_them() {
    let source = av_movie();
    // 1.5 s: the cut at 1 s; 3 s: the cut at 3 s; 4.5 s: past the video.
    let (summary, outputs) = split(&source, &every(q(3, 2))).unwrap();
    let starts: Vec<Rational> = summary.segments.iter().map(|s| s.video_start).collect();
    assert_eq!(starts, [q(0, 1), q(1, 1), q(3, 1)]);
    assert_partition(&source, &outputs, &summary);
    // 0.5 s has no cut before it, 2.5 and 2.7 s both snap to 2 s, and 10 s
    // to the last cut.
    let (summary, outputs) =
        split(&source, &at(vec![q(1, 2), q(5, 2), q(27, 10), q(10, 1)])).unwrap();
    let starts: Vec<Rational> = summary.segments.iter().map(|s| s.video_start).collect();
    assert_eq!(starts, [q(0, 1), q(2, 1), q(3, 1)]);
    assert_partition(&source, &outputs, &summary);
    // No usable target: one segment, the whole file.
    let (summary, outputs) = split(&source, &at(vec![])).unwrap();
    assert_eq!(summary.segments.len(), 1);
    assert_partition(&source, &outputs, &summary);
    assert_eq!(summary.segments[0].end, q(4, 1));
}

/// I P B B (closed), then I with two leading B frames (open), then a closed
/// GOP again; 25 frames per second.
fn open_gop_video() -> TrackSpec {
    let s = |offset, sync| S {
        size: 30,
        duration: 512,
        offset,
        sync,
    };
    let samples = vec![
        s(1024, true),  // I, frame 0
        s(2048, false), // P, frame 3
        s(512, false),  // B, frame 1
        s(512, false),  // B, frame 2
        s(2048, true),  // I (open GOP), frame 6
        s(512, false),  // leading B, frame 4
        s(512, false),  // leading B, frame 5
        s(2048, false), // P, frame 9
        s(512, false),  // B, frame 7
        s(512, false),  // B, frame 8
        s(1024, true),  // I (closed), frame 10
        s(2048, false), // P, frame 13
        s(512, false),  // B, frame 11
        s(512, false),  // B, frame 12
    ];
    TrackSpec {
        video: true,
        timescale: 12_800,
        edits: vec![(560, 1024)],
        samples,
        entry: *b"avc1",
    }
}

#[test]
fn an_open_gop_sync_sample_is_not_a_cut() {
    let source = movie(&[open_gop_video()]);
    // Every 40 ms: the sync sample of frame 6 has leading frames, so the
    // only cut is at frame 10 (sample 10).
    let (summary, outputs) = split(&source, &every(q(1, 25))).unwrap();
    let firsts: Vec<u64> = summary
        .segments
        .iter()
        .map(|s| s.video.first_sample)
        .collect();
    assert_eq!(firsts, [0, 10]);
    assert_eq!(summary.segments[1].video_start, q(10, 25));
    assert_partition(&source, &outputs, &summary);
}

#[test]
fn audio_before_the_video_stays_in_the_first_segment() {
    // The video starts 0.2 s into the movie (an empty edit); the audio at 0.
    let mut video = ibbp_video(50, 25);
    video.edits = vec![(200, -1), (2000, 1024)];
    let source = movie(&[video, aac_audio(110, vec![(2200, 2112)])]);
    let (summary, outputs) = split(&source, &every(q(1, 1))).unwrap();
    let first = summary.segments[0];
    assert_eq!(first.start, q(0, 1));
    assert_eq!(first.video_start, q(1, 5));
    assert_eq!(summary.segments[1].video_start, q(6, 5));
    assert_partition(&source, &outputs, &summary);
    let out = probe_all(&outputs[0]);
    match out.tracks[0].edit_list {
        // 0.2 s at 192,000 movie ticks per second.
        EditList::Simple { empty, .. } => assert_eq!(empty, 38_400),
        other => panic!("{other:?}"),
    }
}

#[test]
fn audio_that_ends_early_leaves_later_segments_without_audio() {
    // 1.5 s of audio.
    let source = movie(&[ibbp_video(100, 25), aac_audio(80, vec![(1500, 2112)])]);
    let (summary, outputs) = split(&source, &every(q(1, 1))).unwrap();
    assert_eq!(summary.segments.len(), 4);
    assert!(summary.segments[0].audio.is_some());
    assert!(summary.segments[1].audio.is_some());
    assert!(summary.segments[2].audio.is_none());
    assert!(summary.segments[3].audio.is_none());
    assert_partition(&source, &outputs, &summary);
}

#[test]
fn a_video_only_split() {
    let source = av_movie();
    let options = SplitOptions {
        audio: AudioChoice::None,
        ..every(q(2, 1))
    };
    let (summary, outputs) = split(&source, &options).unwrap();
    assert_eq!(summary.segments.len(), 2);
    assert!(summary.segments.iter().all(|s| s.audio.is_none()));
    assert_partition(&source, &outputs, &summary);
}

#[test]
fn refusals_happen_before_any_segment_is_opened() {
    let source = av_movie();
    let opened = RefCell::new(0);
    for options in [
        every(q(0, 1)),
        at(vec![q(2, 1), q(1, 1)]),
        SplitOptions {
            video: VideoChoice::Stream(1),
            ..every(q(1, 1))
        },
    ] {
        let result = split_mp4(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
            &options,
            |_| {
                *opened.borrow_mut() += 1;
                Ok(Vec::new())
            },
        );
        assert!(
            matches!(result, Err(ExtractError::Source(Error::Unsupported { .. }))),
            "{result:?}"
        );
    }
    // A first sample presented before it is decoded (a negative
    // composition offset) is refused while the source is listed, before
    // any segment is opened.
    let s = |offset, sync| S {
        size: 30,
        duration: 512,
        offset,
        sync,
    };
    let early = movie(&[TrackSpec {
        video: true,
        timescale: 12_800,
        edits: vec![],
        samples: vec![s(-512, true), s(512, false), s(0, true), s(0, false)],
        entry: *b"avc1",
    }]);
    let result = split_mp4(
        SliceSource::new(&early),
        &Limits::default(),
        &Allowlist::default(),
        &every(q(1, 25)),
        |_| {
            *opened.borrow_mut() += 1;
            Ok(Vec::new())
        },
    );
    assert!(
        matches!(result, Err(ExtractError::Source(Error::Unsupported { .. }))),
        "{result:?}"
    );
    assert_eq!(*opened.borrow(), 0);
}

#[test]
fn cut_points_count_against_the_table_limit() {
    // Every frame a sync sample: 99 candidates (after the first sample) of
    // 40 bytes.
    let samples = (0..100)
        .map(|_| S {
            size: 30,
            duration: 512,
            offset: 0,
            sync: true,
        })
        .collect();
    let video = TrackSpec {
        video: true,
        timescale: 12_800,
        edits: vec![],
        samples,
        entry: *b"avc1",
    };
    let mut source = movie(&[video]);
    // Without a sync sample table the coded data decides
    // (docs/design/cut-points.md): make each sample an IDR slice.
    let probed = probe(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let mut budget = Budget::new(&Limits::default());
    let offsets: Vec<usize> = probed
        .packets(
            0,
            SliceSource::new(&source),
            &Limits::default(),
            &mut budget,
        )
        .unwrap()
        .map(|p| p.unwrap().offset as usize)
        .collect();
    for at in offsets {
        // A 26-byte NAL unit: an IDR slice header (first_mb_in_slice 0,
        // slice_type 7), then zeros.
        source[at..at + 30]
            .copy_from_slice(&[[0, 0, 0, 26, 0x65, 0x88].as_slice(), &[0; 24]].concat());
    }
    let limits = Limits {
        max_table_bytes: 2_000,
        ..Limits::default()
    };
    let result = split_with(&source, &every(q(1, 25)), &limits);
    // Each segment counts too: the candidates fit in 6,000 bytes with 4
    // segments, but not with 100.
    let segments = Limits {
        max_table_bytes: 6_000,
        ..Limits::default()
    };
    assert!(matches!(
        split_with(&source, &every(q(1, 25)), &segments),
        Err(ExtractError::Source(Error::LimitExceeded { .. }))
    ));
    assert_eq!(
        split_with(&source, &every(q(1, 1)), &segments)
            .unwrap()
            .0
            .segments
            .len(),
        4
    );
    assert!(
        matches!(
            result,
            Err(ExtractError::Source(Error::LimitExceeded { .. }))
        ),
        "{:?}",
        result.map(|r| r.0)
    );
    let (summary, outputs) = split(&source, &every(q(1, 25))).unwrap();
    assert_eq!(summary.segments.len(), 100);
    assert_partition(&source, &outputs, &summary);
}

/// Fixtures the CI job requires to split (scripts/gen-fixtures.sh).
const REQUIRED: &[&str] = &[
    "basic.mp4",
    "basic.mov",
    "faststart.mp4",
    "edit_list.mp4",
    "fragmented.mp4",
    "vfr.mp4",
    "negative_cts.mp4",
    "rotate_ccw90.mp4",
    "hevc_hvc1.mp4",
    "av1.mp4",
    "vp9.mp4",
    "clip_open_gop.mp4",
    "clip_hevc_open_gop.mp4",
    "clip_audio_late.mp4",
    "clip_video_late.mp4",
    "clip_no_stss.mp4",
    "clip_no_stss_bframes.mp4",
    "clip_stss_all.mp4",
    "clip_stss_all_flash.mp4",
    "clip_hevc_stss_all.mp4",
    "split_hevc.mp4",
    "split_av1.mp4",
    "split_vp9.mp4",
    "edit_multi_splice.mp4",
    "edit_multi_bframes.mp4",
    "edit_multi_empty.mp4",
    "edit_multi_av.mp4",
];

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_rational(r: Rational) -> String {
    format!("{{\"num\": {}, \"den\": {}}}", r.num, r.den)
}

/// Every MP4 and MOV fixture with a video track the writer accepts is
/// split every second and at 0.5 s and 1.3 s. With `COREAV_SPLIT_OUT` the
/// segments and a manifest are written for `scripts/split_check.py`, but
/// for the two fixtures whose picture size changes inside the track.
#[test]
#[ignore = "needs COREAV_FIXTURES"]
fn fixtures_round_trip() {
    let dir = std::env::var("COREAV_FIXTURES").expect("COREAV_FIXTURES");
    let out_dir = std::env::var("COREAV_SPLIT_OUT").ok();
    if let Some(out_dir) = &out_dir {
        std::fs::create_dir_all(out_dir).unwrap();
    }
    let generated = std::fs::read_to_string(Path::new(&dir).join("MANIFEST")).unwrap_or_default();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("mp4" | "mov")))
        .collect();
    entries.sort();
    let mut manifest = Vec::new();
    let mut split_files = std::collections::BTreeSet::new();
    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read(path).unwrap();
        if probe(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
        )
        .is_err()
        {
            continue;
        }
        for (label, options) in [
            ("every1", every(q(1, 1))),
            ("at", at(vec![q(1, 2), q(13, 10)])),
        ] {
            let (summary, outputs) = match split(&source, &options) {
                Ok(done) => done,
                Err(ExtractError::Source(Error::Unsupported { reason })) => {
                    assert!(
                        !REQUIRED.contains(&name.as_str()),
                        "{name} {label}: refused: {reason}"
                    );
                    eprintln!("{name} {label}: refused: {reason}");
                    continue;
                }
                Err(ExtractError::Source(Error::NoSuchTrack)) => {
                    assert!(!REQUIRED.contains(&name.as_str()), "{name}: no video");
                    continue;
                }
                Err(ExtractError::Source(e @ Error::CodecNotAllowed { .. })) => {
                    assert!(!REQUIRED.contains(&name.as_str()), "{name} {label}: {e}");
                    eprintln!("{name} {label}: {e}");
                    continue;
                }
                Err(ExtractError::Source(e @ Error::InvalidStream { .. }))
                    if common::INVALID_STREAMS.contains(&name.as_str()) =>
                {
                    eprintln!("{name} {label}: refused: {e}");
                    continue;
                }
                Err(e) => panic!("{name} {label}: {e}"),
            };
            assert!(
                !common::INVALID_STREAMS.contains(&name.as_str()),
                "{name} {label}: written"
            );
            assert_partition(&source, &outputs, &summary);
            split_files.insert(name.clone());
            // Picture sizes that change inside the track (docs/design/
            // frame-sizes.md): the decoder reinitializes mid-stream, which
            // the oracles' size and frame comparisons do not model, as for
            // the clip fixtures. The round trip above still covers them.
            if matches!(name.as_str(), "size_inband_sps.mp4" | "size_vp9_resize.mp4") {
                continue;
            }
            eprintln!(
                "{name} {label}: {} segments at {:?}",
                summary.segments.len(),
                summary
                    .segments
                    .iter()
                    .map(|s| s.video_start)
                    .collect::<Vec<_>>()
            );
            if let Some(out_dir) = &out_dir {
                let mut segments = Vec::new();
                for (index, (segment, output)) in summary.segments.iter().zip(&outputs).enumerate()
                {
                    let output_path =
                        Path::new(out_dir).join(format!("{name}.{label}.{index:03}.mp4"));
                    std::fs::write(&output_path, output).unwrap();
                    segments.push(format!(
                        "{{\"output\": {}, \"start\": {}, \"video_start\": {}, \"end\": {}, \"video_first\": {}, \"video_samples\": {}, \"audio_first\": {}, \"audio_samples\": {}}}",
                        json_string(&output_path.display().to_string()),
                        json_rational(segment.start),
                        json_rational(segment.video_start),
                        json_rational(segment.end),
                        segment.video.first_sample,
                        segment.video.samples,
                        segment.audio.map_or(0, |a| a.first_sample),
                        segment.audio.map_or(0, |a| a.samples),
                    ));
                }
                let first = &summary.segments[0];
                manifest.push(format!(
                    "{{\"source\": {}, \"label\": {}, \"video_stream\": {}, \"audio_stream\": {}, \"segments\": [{}]}}",
                    json_string(&path.display().to_string()),
                    json_string(label),
                    first.video.stream_index,
                    summary
                        .segments
                        .iter()
                        .find_map(|s| s.audio)
                        .map_or("null".to_string(), |a| a.stream_index.to_string()),
                    segments.join(", "),
                ));
            }
        }
    }
    for name in REQUIRED {
        if generated.contains(&format!("generated {name}")) {
            assert!(split_files.contains(*name), "{name} gave no split");
        }
    }
    if let Some(out_dir) = &out_dir {
        std::fs::write(
            Path::new(out_dir).join("manifest.json"),
            format!("[\n{}\n]\n", manifest.join(",\n")),
        )
        .unwrap();
    }
}

#[test]
fn segments_without_a_sync_table_start_on_decodable_frames() {
    // docs/design/cut-points.md: 30 frames of 40 ms, no stss, IDR pictures
    // at 0, 10 and 20. A cut every 200 ms falls on P pictures except at
    // those three: the segments start there only.
    let samples = (0..30)
        .map(|_| S {
            size: 40,
            duration: 512,
            offset: 0,
            sync: true,
        })
        .collect();
    let video = TrackSpec {
        video: true,
        timescale: 12_800,
        edits: vec![],
        samples,
        entry: *b"avc1",
    };
    let mut source = movie(&[video]);
    let idr: Vec<bool> = (0..30).map(|n| n % 10 == 0).collect();
    h264_slices(&mut source, &idr);
    let (summary, outputs) = split_with(&source, &every(q(1, 5)), &Limits::default()).unwrap();
    let starts: Vec<u64> = summary
        .segments
        .iter()
        .map(|s| s.video.first_sample)
        .collect();
    assert_eq!(starts, [0, 10, 20]);
    assert_partition(&source, &outputs, &summary);
    // Each segment marks only its IDR picture as a sync sample.
    for output in &outputs {
        let flags: Vec<bool> = packets(output, &probe_all(output), 0)
            .iter()
            .map(|p| p.keyframe)
            .collect();
        assert_eq!(flags, (0..10).map(|n| n == 0).collect::<Vec<_>>());
    }
}

#[test]
fn a_long_presentation_with_a_short_step_is_bounded_by_the_cut_points() {
    // Three IDR pictures of 0x5555_5555 ticks at 1 tick a second: some
    // 4.3e9 s cut every second. The targets between cut points are
    // skipped, not walked one by one.
    let samples = (0..3)
        .map(|_| S {
            size: 40,
            duration: 0x5555_5555,
            offset: 0,
            sync: true,
        })
        .collect();
    let video = TrackSpec {
        video: true,
        timescale: 1,
        edits: vec![],
        samples,
        entry: *b"avc1",
    };
    let mut source = movie(&[video]);
    h264_slices(&mut source, &[true, true, true]);
    let started = std::time::Instant::now();
    let (summary, outputs) = split(&source, &every(q(1, 1))).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let starts: Vec<u64> = summary
        .segments
        .iter()
        .map(|s| s.video.first_sample)
        .collect();
    assert_eq!(starts, [0, 1, 2]);
    assert_partition(&source, &outputs, &summary);
}

#[test]
#[ignore = "needs COREAV_FIXTURES"]
fn fixtures_round_trip_av1_without_a_sync_table() {
    // docs/design/cut-points.md: split_av1.mp4 without its table splits at
    // the key frames its table lists, and its segments mark the same
    // samples as sync.
    let dir = std::env::var("COREAV_FIXTURES").expect("COREAV_FIXTURES");
    let generated = std::fs::read_to_string(Path::new(&dir).join("MANIFEST")).unwrap_or_default();
    if !generated.lines().any(|l| l == "generated split_av1.mp4") {
        eprintln!("split_av1.mp4 was not generated");
        return;
    }
    let source = std::fs::read(Path::new(&dir).join("split_av1.mp4")).unwrap();
    let untabled = without_sync_table(&source);
    assert_ne!(untabled, source, "split_av1.mp4 has no sync sample table");
    let (expected, expected_outputs) = split(&source, &every(q(1, 1))).unwrap();
    let (summary, outputs) = split(&untabled, &every(q(1, 1))).unwrap();
    let starts = |s: &SplitSummary| -> Vec<u64> {
        s.segments.iter().map(|s| s.video.first_sample).collect()
    };
    assert_eq!(starts(&summary), starts(&expected));
    assert!(summary.segments.len() > 1);
    assert_partition(&untabled, &outputs, &summary);
    let flags = |data: &[u8]| -> Vec<bool> {
        let movie = probe_all(data);
        let video = movie
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Video)
            .unwrap();
        packets(data, &movie, video.index)
            .iter()
            .map(|p| p.keyframe)
            .collect()
    };
    for (output, expected) in outputs.iter().zip(&expected_outputs) {
        assert_eq!(flags(output), flags(expected));
    }
}

/// The video of `av_movie` (100 frames of 40 ms at 12,800 Hz in GOPs of
/// 25, composition offsets that include a two-frame delay of 1,024 ticks)
/// with `edits`, and its audio.
fn spliced(edits: Vec<(u32, i32)>) -> Vec<u8> {
    let mut video = ibbp_video(100, 25);
    video.edits = edits;
    movie(&[video, aac_audio(190, vec![(4000, 2112)])])
}

/// The output's edit list entries of track `index`, `(duration, media
/// time)`.
fn output_edits(output: &[u8], index: usize) -> Vec<(u64, i64)> {
    probe_all(output).tracks[index]
        .edits
        .iter()
        .map(|e| (e.segment_duration, e.media_time))
        .collect()
}

#[test]
fn every_segment_of_a_spliced_source_carries_its_window() {
    // The first second of the media, then the third and the fourth: the
    // second group of pictures is cut out, and the splice falls on a cut.
    let source = spliced(vec![(1000, 1024), (2000, 1024 + 25_600)]);
    let (summary, outputs) = split(&source, &every(q(1, 1))).unwrap();
    assert_eq!(summary.segments.len(), 3);
    let starts: Vec<Rational> = summary.segments.iter().map(|s| s.video_start).collect();
    assert_eq!(starts, [q(0, 1), q(1, 1), q(2, 1)]);
    assert_partition(&source, &outputs, &summary);
    // Each segment lies inside one window, so each has one media edit
    // (192,000 movie ticks a second, the least common multiple of 12,800
    // and 48,000). The first segment holds the samples the splice skips,
    // which its edit leaves unpresented.
    assert_eq!(summary.segments[0].video.samples, 50);
    for output in &outputs {
        assert_eq!(output_edits(output, 0), vec![(192_000, 1024)]);
    }
}

#[test]
fn a_segment_across_a_splice_keeps_both_windows() {
    let source = spliced(vec![(1000, 1024), (2000, 1024 + 25_600)]);
    // 1.3 s falls inside the second window, whose only cut point is at
    // 1 s: one segment up to the splice, then one across it.
    let (summary, outputs) = split(&source, &at(vec![q(13, 10), q(3, 2)])).unwrap();
    assert_eq!(summary.segments.len(), 2);
    assert_partition(&source, &outputs, &summary);
    assert_eq!(summary.segments[1].end, q(3, 1));
    assert_eq!(output_edits(&outputs[1], 0), vec![(384_000, 1024)]);
    // A split at 0.5 s has no cut before it; the one segment keeps both
    // windows, as a whole clip does.
    let (summary, outputs) = split(&source, &at(vec![q(1, 2)])).unwrap();
    assert_eq!(summary.segments.len(), 1);
    assert_partition(&source, &outputs, &summary);
    assert_eq!(
        output_edits(&outputs[0], 0),
        vec![(192_000, 1024), (384_000, 1024 + 25_600)]
    );
}

#[test]
fn a_gap_between_two_windows_opens_the_segment_after_it() {
    // Half a second of nothing between the two windows: the cut at the
    // second window's first sample.
    let source = spliced(vec![(1000, 1024), (500, -1), (1000, 1024 + 25_600)]);
    let (summary, outputs) = split(&source, &every(q(1, 1))).unwrap();
    assert_eq!(summary.segments.len(), 2);
    assert_partition(&source, &outputs, &summary);
    // The first segment ends where its window does; the second starts
    // there, with the gap as an empty edit before its video.
    let (first, second) = (summary.segments[0], summary.segments[1]);
    assert_eq!(first.end, q(1, 1));
    assert_eq!(second.start, q(1, 1));
    assert_eq!(second.video_start, q(3, 2));
    assert_eq!(second.end, q(5, 2));
    assert_eq!(output_edits(&outputs[0], 0), vec![(192_000, 1024)]);
    assert_eq!(
        output_edits(&outputs[1], 0),
        vec![(96_000, -1), (192_000, 1024)]
    );
    // The audio has one edit and runs through the gap, so the samples the
    // gap covers belong to the segment the gap belongs to: the two
    // segments last as long as the source presents, no more (split
    // decision 3). Their movie durations are each track's edits.
    let duration = |output: &[u8]| -> Rational {
        let movie = probe_all(output);
        let ticks: u64 = movie
            .tracks
            .iter()
            .map(|t| t.edits.iter().map(|e| e.segment_duration).sum::<u64>())
            .max()
            .unwrap();
        q(i128::from(ticks), u64::from(movie.timescale))
    };
    let total = duration(&outputs[0])
        .checked_add(duration(&outputs[1]))
        .unwrap();
    // The source presents 2.5 s of video and 4 s of audio, so its
    // presentation lasts 4 s; the segments add up to that, and the last
    // one's audio may end up to one AAC frame (1,024 of 48,000 ticks)
    // after its video.
    let source_span = duration(&source);
    assert_eq!(source_span, q(4, 1));
    let over = diff(total, source_span);
    assert!((0.0..1024.0 / 48_000.0).contains(&over), "{total:?}");
}

#[test]
fn audio_with_several_edits_is_split_with_its_windows() {
    // Audio: its first second after priming, then a skip of one second of
    // media; the video has one edit.
    let audio = aac_audio(190, vec![(1000, 2112), (2000, 2112 + 96_000)]);
    let source = movie(&[ibbp_video(100, 25), audio]);
    let (summary, outputs) = split(&source, &every(q(1, 1))).unwrap();
    assert_eq!(summary.segments.len(), 4);
    assert_partition(&source, &outputs, &summary);
    // Each segment presents the windows it holds, and only those: the
    // frame that straddles the boundary is shown from the second window's
    // start, so it belongs to the segment that presents that window and
    // no audio is presented twice (split decisions 1 and 5).
    for (index, output) in outputs.iter().enumerate() {
        if summary.segments[index].audio.is_none() {
            continue;
        }
        let edits = output_edits(output, 1);
        assert_eq!(
            edits.iter().filter(|e| e.1 != -1).count(),
            1,
            "segment {index}: {edits:?}"
        );
    }
    assert_eq!(output_edits(&outputs[0], 1), vec![(192_000, 2112)]);
    // Every audio sample is in exactly one segment (assert_partition), and
    // the last segment has none: the audio's windows end at 3 s.
    assert!(summary.segments[3].audio.is_none());
}

#[test]
fn several_edits_that_cannot_be_written_exactly_refuse() {
    let refused = |source: &[u8]| {
        let outputs = Rc::new(RefCell::new(Vec::new()));
        let opened = Rc::clone(&outputs);
        let error = split_mp4(
            SliceSource::new(source),
            &Limits::default(),
            &Allowlist::default(),
            &every(q(1, 1)),
            move |index| {
                let mut all = opened.borrow_mut();
                all.push(Vec::new());
                Ok(Sink(Rc::clone(&opened), index))
            },
        )
        .expect_err("refused");
        // Every refusal is made while the source is listed, before the
        // first writer is opened (docs/design/split.md, "Errors").
        assert!(outputs.borrow().is_empty(), "{error:?}");
        match error {
            ExtractError::Source(Error::Unsupported { reason }) => reason,
            other => panic!("{other:?}"),
        }
    };
    // Overlapping windows are outside the edit model.
    assert_eq!(
        refused(&spliced(vec![(2000, 1024), (2000, 1024 + 6400)])),
        "edit list outside the supported model"
    );
    // A window inside a segment that lasts 4,294,967.291 media ticks: a
    // media timescale of 4,294,967,291 in a 1,000 Hz movie, so its
    // duration is whole only in a movie timescale above 32 bits.
    let mut video = ibbp_video(100, 25);
    video.timescale = 4_294_967_291;
    for sample in &mut video.samples {
        sample.duration = 1_000_000;
    }
    video.edits = vec![(1, 1024), (1, 50_000_000)];
    assert_eq!(
        refused(&movie(&[video])),
        "edit list whose edit durations need a movie timescale above 32 bits"
    );
    // More than 4,096 empty edits in a row between two media edits.
    let mut edits = vec![(1000, 1024)];
    edits.extend(std::iter::repeat_n((1, -1), 4097));
    edits.push((1000, 1024 + 25_600));
    assert_eq!(
        refused(&spliced(edits)),
        "edit list with a long run of empty edits"
    );
}
#[test]
fn a_gap_that_is_not_a_whole_video_tick_moves_the_movie_timescale() {
    // A gap of one movie tick at 1,000 Hz: 12.8 ticks of the 12,800 Hz
    // video. The segment after it opens with that gap as an empty edit, so
    // the movie timescale must make it whole, as it makes the media edits
    // whole (docs/design/edit-list-writers.md, split decision 6).
    let mut video = ibbp_video(100, 25);
    video.edits = vec![(1000, 1024), (1, -1), (1000, 1024 + 25_600)];
    let source = movie(&[video]);
    let (summary, outputs) = split(&source, &every(q(1, 1))).unwrap();
    assert_eq!(summary.segments.len(), 2);
    assert_partition(&source, &outputs, &summary);
    assert_eq!(summary.segments[1].start, q(1, 1));
    assert_eq!(summary.segments[1].video_start, q(1001, 1000));
    // 64,000 movie ticks a second (five times the video's), so the gap is
    // 64 ticks exactly.
    assert_eq!(output_edits(&outputs[1], 0), vec![(64, -1), (64_000, 1024)]);
}

#[test]
fn a_sample_that_straddles_a_window_is_shown_from_the_window() {
    // A splice whose second window starts inside an AAC frame: that frame
    // is presented, but only from the window's start, which is later than
    // where the window's mapping places its media
    // (docs/design/edit-list-writers.md, split decision 1). Taking the
    // mapping for its presentation would put the segment that starts with
    // it inside the window before it.
    let audio = aac_audio(190, vec![(1001, 0), (1000, 51_224)]);
    let source = movie(&[ibbp_video(100, 25), audio]);
    let (summary, outputs) = split(&source, &every(q(1, 1))).unwrap();
    assert_partition(&source, &outputs, &summary);
    // The straddling frame opens the second segment, whose edits present
    // the second window only.
    let second = output_edits(&outputs[1], 1);
    assert_eq!(second.iter().filter(|e| e.1 != -1).count(), 1);
    assert!(second[0].1 == -1 || second.len() == 1);
    // The first segment's audio stops where its window does.
    assert_eq!(output_edits(&outputs[0], 1).len(), 1);
}

#[test]
fn a_cut_on_a_straddling_sync_sample_starts_at_its_window() {
    // The sync sample of the third group of pictures straddles the gap:
    // its media starts before the second window and reaches into it. The
    // segment it opens starts where the window does, after one empty edit
    // for the gap, and its reported video start is that.
    let mut video = ibbp_video(100, 25);
    video.edits = vec![(1000, 1024), (500, -1), (2000, 14_080)];
    let source = movie(&[video]);
    let (summary, outputs) = split(&source, &every(q(1, 1))).unwrap();
    assert_partition(&source, &outputs, &summary);
    let second = summary.segments[1];
    assert_eq!(second.start, q(1, 1));
    assert_eq!(second.video_start, q(3, 2));
    // 12,800 movie ticks a second: half a second of gap, then the window.
    assert_eq!(
        output_edits(&outputs[1], 0),
        vec![(6400, -1), (12_544, 1280)]
    );
}

#[test]
fn an_audio_window_that_starts_between_ticks_is_split() {
    // The audio's first window lasts one movie tick at 1,000 Hz: 44.1
    // ticks of a 44,100 Hz track, so its second window starts between two
    // media ticks. A segment's audio has no pre-roll, so its run starts at
    // the first sample it presents and its empty edit must not round down
    // below it (docs/design/edit-list-writers.md, split decision 5).
    // 600 Hz video, so the movie timescale is 88,200 (with 44,100 Hz
    // audio), which 1,000 Hz movie ticks do not divide.
    let video = TrackSpec {
        video: true,
        timescale: 600,
        edits: vec![(2000, 1000)],
        samples: (0..100)
            .map(|n| S {
                size: 40 + (n as u32 % 13),
                duration: 24,
                offset: match n % 25 {
                    0 => 48,
                    k if k % 3 == 1 => 96,
                    _ => 24,
                },
                sync: n % 25 == 0,
            })
            .collect(),
        entry: *b"avc1",
    };
    let audio = TrackSpec {
        video: false,
        timescale: 44_100,
        samples: (0..190)
            .map(|n| S {
                size: 20 + (n as u32 % 5),
                duration: 1024,
                offset: 0,
                sync: true,
            })
            .collect(),
        edits: vec![(1, 1000), (1500, 1500)],
        entry: *b"mp4a",
    };
    let source = movie(&[video, audio]);
    let (summary, outputs) = split(&source, &every(q(1, 2))).unwrap();
    assert!(summary.segments.len() > 1);
    assert_partition(&source, &outputs, &summary);
}

#[test]
fn a_sample_two_windows_show_is_shown_by_the_segment_that_holds_it() {
    // The source pauses with its media running on: the audio sample at
    // media 44,100 is shown by both windows (docs/design/edit-lists.md,
    // decision 3). A split copies it into one segment, so only that
    // segment can show it (docs/design/edit-list-writers.md, split
    // decision 5): every sample is still in exactly one segment at its
    // source time, and the segments present nothing twice.
    let video = TrackSpec {
        video: true,
        timescale: 600,
        edits: vec![(4000, 0)],
        samples: (0..100)
            .map(|n| S {
                size: 40 + (n as u32 % 13),
                duration: 24,
                offset: 0,
                sync: n % 25 == 0,
            })
            .collect(),
        entry: *b"avc1",
    };
    let audio = TrackSpec {
        video: false,
        timescale: 44_100,
        samples: (0..190)
            .map(|n| S {
                size: 20 + (n as u32 % 5),
                duration: 1024,
                offset: 0,
                sync: true,
            })
            .collect(),
        edits: vec![(1000, 0), (500, -1), (2000, 44_100)],
        entry: *b"mp4a",
    };
    let source = movie(&[video, audio]);
    // One segment holds both showings of the straddling sample.
    let (summary, outputs) = split(&source, &at(vec![])).unwrap();
    assert_eq!(summary.segments.len(), 1);
    assert_partition(&source, &outputs, &summary);
    assert_eq!(output_edits(&outputs[0], 1).len(), 3);
    // A cut between the two showings: each segment shows the showings its
    // own samples give it, and the samples are still partitioned.
    let (summary, outputs) = split(&source, &every(q(1, 2))).unwrap();
    assert!(summary.segments.len() > 2);
    assert_partition(&source, &outputs, &summary);
}
