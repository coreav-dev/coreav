//! Clip round trips: every output is probed by
//! `coreav-isobmff`; each kept track's samples must equal the source's kept
//! run, every presented output sample must be presented in the source at
//! its output time plus the snapped start, and no frame presented inside
//! the clip may be missing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

use coreav_isobmff::{
    Allowlist, Budget, Codec, EditList, Error, FourCc, Limits, Movie, Packet, Rational,
    SliceSource, Source, TrackKind, probe,
};
use coreav_isobmff_write::{
    AudioChoice, AudioClipOptions, AudioClipSummary, ClipOptions, ClipSummary, ClipTrack,
    ExtractError, LeftOutReason, TrackChoice, VideoChoice, clip_audio_mp4, clip_mp4,
};
use std::path::Path;

use movies::{
    S, TrackSpec, aac_audio, av_movie, fragmented_video, h264_frames, h264_slices, ibbp_video,
    movie, without_sync_table, written_sync,
};

mod movies;

fn q(num: i128, den: u64) -> Rational {
    Rational::new(num, den).unwrap()
}

fn options(start: Rational, end: Option<Rational>) -> ClipOptions {
    ClipOptions {
        video: VideoChoice::First,
        audio: AudioChoice::First,
        start,
        end,
    }
}

fn clip(source: &[u8], options: &ClipOptions) -> Result<(ClipSummary, Vec<u8>), ExtractError> {
    let mut out = Vec::new();
    let summary = clip_mp4(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
        options,
        &mut out,
    )?;
    assert_eq!(summary.bytes_written, out.len() as u64);
    Ok((summary, out))
}

fn audio_clip(
    source: &[u8],
    audio: TrackChoice,
    start: Rational,
    end: Option<Rational>,
) -> Result<(AudioClipSummary, Vec<u8>), ExtractError> {
    let mut out = Vec::new();
    let summary = clip_audio_mp4(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
        &AudioClipOptions { audio, start, end },
        &mut out,
    )?;
    assert_eq!(summary.bytes_written, out.len() as u64);
    Ok((summary, out))
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

/// `a - b` in exact arithmetic, as a floating-point value for messages and
/// tolerances.
fn diff(a: Rational, b: Rational) -> f64 {
    a.num as f64 / a.den as f64 - b.num as f64 / b.den as f64
}

fn ticks(seconds: Rational, timescale: u32) -> Rational {
    q(seconds.num * i128::from(timescale), seconds.den)
}

/// Checks one kept track of `output` (output track `out_index`) against the
/// source track the summary names. Times on the output's presentation
/// timeline plus the snapped start must equal the source's, exactly for
/// the video and within one tick for the audio, whose edit is rounded to
/// whole ticks.
#[allow(clippy::too_many_arguments)]
fn assert_track(
    source: &[u8],
    source_movie: &Movie,
    output: &[u8],
    out_movie: &Movie,
    out_index: usize,
    kept: &ClipTrack,
    snapped_start: Rational,
    exact: bool,
) -> Vec<Packet> {
    let src_track = source_movie
        .tracks
        .iter()
        .find(|t| t.stream_index == kept.stream_index)
        .unwrap();
    let out_track = &out_movie.tracks[out_index];
    assert_eq!(out_track.timescale, kept.timescale);
    assert_eq!(out_track.description.codec, src_track.description.codec);
    assert_eq!(
        out_track.description.codec_config,
        movies::excerpt_config(&src_track.description.codec_config)
    );
    let all = packets(source, source_movie, src_track.index);
    let first = kept.first_sample as usize;
    let run = &all[first..first + kept.samples as usize];
    // A run the container marks sync throughout gets the coded data's
    // flags; any other keeps the source's (docs/design/cut-points.md).
    let marked = run.iter().all(|p| p.keyframe);
    let written = packets(output, out_movie, out_track.index);
    assert_eq!(written.len(), run.len());
    let zero = ticks(snapped_start, kept.timescale);
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
                "sample {} presented only in the clip",
                s.sample
            );
            let out_pts = o.pts.unwrap().checked_add(zero).unwrap();
            let gap = diff(out_pts, s.pts.unwrap()).abs();
            assert!(
                if exact { gap == 0.0 } else { gap <= 1.0 },
                "sample {}: {gap} ticks apart",
                s.sample
            );
        }
    }
    all
}

/// The round trip described at the top of this file.
fn assert_round_trip(source: &[u8], output: &[u8], summary: &ClipSummary) {
    let source_movie = probe(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let out_movie = probe(
        SliceSource::new(output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert!(!out_movie.has_fragments());
    let expected_tracks = 1 + usize::from(summary.audio.is_some());
    assert_eq!(out_movie.tracks.len(), expected_tracks);
    assert_eq!(out_movie.tracks[0].kind, TrackKind::Video);
    let video = &summary.video;
    let all = assert_track(
        source,
        &source_movie,
        output,
        &out_movie,
        0,
        video,
        summary.snapped_start,
        true,
    );
    let src_video = source_movie
        .tracks
        .iter()
        .find(|t| t.stream_index == video.stream_index)
        .unwrap();
    assert_eq!(out_movie.tracks[0].matrix, src_video.matrix);
    assert_eq!(
        out_movie.tracks[0].presentation_size,
        src_video.presentation_size
    );
    assert_eq!(
        out_movie.tracks[0].description.entry_boxes,
        src_video.description.entry_boxes
    );
    // No frame presented inside the clip is missing.
    let zero = ticks(summary.snapped_start, video.timescale);
    let end = ticks(summary.written_end, video.timescale);
    let first = video.first_sample;
    let last = first + u64::from(video.samples);
    for p in all.iter().filter(|p| p.presented) {
        let pts = p.pts.unwrap();
        if !zero.checked_lt(pts).unwrap() && pts != zero {
            continue;
        }
        if !pts.checked_lt(end).unwrap() {
            continue;
        }
        assert!(
            (first..last).contains(&p.sample),
            "frame {} at {pts:?} is missing",
            p.sample
        );
    }
    // CoreAV's own stream duration is the clip's length, less
    // the empty edits between windows of a source with several edits.
    let mut length = ticks(summary.written_end, video.timescale)
        .checked_add(q(-zero.num, zero.den))
        .unwrap();
    if out_movie.tracks[0].edit_list == EditList::Multiple {
        let empty: u64 = out_movie.tracks[0]
            .edits
            .iter()
            .filter(|e| e.media_time == -1)
            .map(|e| e.segment_duration)
            .sum();
        let empty = ticks(
            q(i128::from(empty), u64::from(out_movie.timescale)),
            video.timescale,
        );
        length = length.checked_add(q(-empty.num, empty.den)).unwrap();
    }
    assert_eq!(out_movie.tracks[0].duration, Some(length));
    // The video edit is the clip: from the snapped start to the written end.
    if let EditList::Simple { empty, segment, .. } = out_movie.tracks[0].edit_list {
        assert_eq!(empty, 0);
        assert_eq!(
            q(i128::from(segment), u64::from(out_movie.timescale)),
            q(
                (summary.written_end.num * i128::from(summary.snapped_start.den))
                    - summary.snapped_start.num * i128::from(summary.written_end.den),
                summary.written_end.den * summary.snapped_start.den
            )
        );
    }
    if let Some(audio) = &summary.audio {
        assert_eq!(out_movie.tracks[1].kind, TrackKind::Audio);
        assert_track(
            source,
            &source_movie,
            output,
            &out_movie,
            1,
            audio,
            summary.snapped_start,
            false,
        );
    }
}

#[test]
fn snaps_to_the_sync_sample_at_or_before_the_start() {
    let source = av_movie();
    // 1.5 s lies in the second GOP, which starts at 1 s.
    let (summary, out) = clip(&source, &options(q(3, 2), Some(q(5, 2)))).unwrap();
    assert_eq!(summary.snapped_start, q(1, 1));
    assert_eq!(summary.written_end, q(5, 2));
    assert_eq!(summary.video.first_sample, 25);
    assert_round_trip(&source, &out, &summary);
    let audio = summary.audio.unwrap();
    assert_eq!(audio.timescale, 48_000);
    // Pre-roll: the audio frame before the one presented at 1 s.
    // Presentation p = dts - 2112, so the frame holding 48,000 starts at
    // dts 49,152 (sample 48) and the pre-roll is sample 47.
    assert_eq!(audio.first_sample, 47);
    assert!(summary.left_out.is_empty());
    assert_eq!(
        summary.boxes_left_out,
        vec![coreav_isobmff::FourCc(*b"fiel")]
    );
}

#[test]
fn a_start_on_a_sync_sample_keeps_it() {
    let source = av_movie();
    let (summary, out) = clip(&source, &options(q(2, 1), None)).unwrap();
    assert_eq!(summary.snapped_start, q(2, 1));
    assert_eq!(summary.video.first_sample, 50);
    assert_eq!(summary.video.samples, 50);
    assert_round_trip(&source, &out, &summary);
}

#[test]
fn the_whole_file_and_an_end_past_the_end() {
    let source = av_movie();
    for end in [None, Some(q(100, 1))] {
        let (summary, out) = clip(&source, &options(q(0, 1), end)).unwrap();
        assert_eq!(summary.snapped_start, q(0, 1));
        assert_eq!(summary.video.samples, 100);
        assert_eq!(summary.written_end, q(4, 1));
        assert_round_trip(&source, &out, &summary);
    }
}

#[test]
fn an_exact_end_keeps_the_frames_it_needs() {
    let source = av_movie();
    // End at 1/7 s: frames presented before it are 0..=3 (0, 40, 80, 120
    // ms); the decode-order run must reach the P frame presented at 120 ms
    // (decode slot 1) and the B frames before it.
    let (summary, out) = clip(&source, &options(q(0, 1), Some(q(1, 7)))).unwrap();
    // 12,800 / 7 = 1828.57 ticks, rounded up.
    assert_eq!(summary.written_end, q(1829, 12_800));
    assert_round_trip(&source, &out, &summary);
}

/// The round trip of a clip of audio alone: one audio track in the
/// audio's movie timescale, the source's kept run presented from the
/// requested start, and edits (or, without them, the media) that last to
/// the written end.
fn assert_audio_round_trip(source: &[u8], output: &[u8], summary: &AudioClipSummary) {
    let source_movie = probe(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let out_movie = probe(
        SliceSource::new(output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert!(!out_movie.has_fragments());
    assert_eq!(out_movie.tracks.len(), 1);
    let track = &out_movie.tracks[0];
    assert_eq!(track.kind, TrackKind::Audio);
    assert_eq!(out_movie.timescale, summary.audio.timescale);
    assert_track(
        source,
        &source_movie,
        output,
        &out_movie,
        0,
        &summary.audio,
        summary.start,
        false,
    );
    let presented: u64 = if track.edits.is_empty() {
        track.mdhd_duration
    } else {
        track.edits.iter().map(|e| e.segment_duration).sum()
    };
    assert_eq!(
        summary
            .start
            .checked_add(q(i128::from(presented), u64::from(out_movie.timescale)))
            .unwrap(),
        summary.written_end
    );
}

#[test]
fn audio_alone_is_clipped_from_one_frame_before_the_start() {
    // 100 frames of 1,024 ticks at 48 kHz, no edit list.
    let source = movie(&[aac_audio(100, vec![])]);
    let (summary, out) =
        audio_clip(&source, TrackChoice::FirstAudio, q(1, 2), Some(q(3, 2))).unwrap();
    // 0.5 s is tick 24,000, inside frame 23; frame 22 is the pre-roll. 1.5 s
    // is tick 72,000, inside frame 70.
    assert_eq!(summary.audio.first_sample, 22);
    assert_eq!(summary.audio.samples, 49);
    assert_eq!(summary.written_end, q(3, 2));
    assert_eq!(output_edits(&out, 0), vec![(48_000, 24_000 - 22 * 1024)]);
    assert_audio_round_trip(&source, &out, &summary);
}

#[test]
fn audio_alone_without_an_end_runs_to_the_end_of_the_track() {
    // Priming of 2,112 ticks skipped by the source's edit, which presents 2 s.
    let source = movie(&[aac_audio(100, vec![(2000, 2112)])]);
    let (summary, out) = audio_clip(&source, TrackChoice::FirstAudio, q(1, 1), None).unwrap();
    // 1 s is media tick 50,112, inside frame 48; frame 47 is the pre-roll.
    assert_eq!(summary.audio.first_sample, 47);
    assert_eq!(summary.written_end, q(2, 1));
    assert_eq!(output_edits(&out, 0), vec![(48_000, 50_112 - 47 * 1024)]);
    assert_audio_round_trip(&source, &out, &summary);
}

#[test]
fn audio_alone_is_taken_from_a_file_with_video() {
    let source = av_movie();
    let (summary, out) =
        audio_clip(&source, TrackChoice::Stream(1), q(1, 1), Some(q(2, 1))).unwrap();
    assert_eq!(summary.audio.stream_index, 1);
    assert_eq!(summary.left_out.len(), 1);
    assert_eq!(summary.left_out[0].stream_index, 0);
    assert_eq!(summary.left_out[0].reason, LeftOutReason::NotSelected);
    assert_audio_round_trip(&source, &out, &summary);
}

#[test]
fn audio_alone_is_refused_where_the_range_presents_none() {
    let source = movie(&[aac_audio(100, vec![])]);
    for (start, end) in [(q(3, 1), Some(q(4, 1))), (q(1, 1), Some(q(1, 1)))] {
        let refused = audio_clip(&source, TrackChoice::FirstAudio, start, end);
        assert!(
            matches!(
                refused,
                Err(ExtractError::Source(Error::Unsupported { .. }))
            ),
            "{start:?} {end:?}: {refused:?}"
        );
    }
    let video_only = movie(&[ibbp_video(25, 25)]);
    assert!(matches!(
        audio_clip(&video_only, TrackChoice::FirstAudio, q(0, 1), None),
        Err(ExtractError::Source(Error::NoSuchTrack))
    ));
}

#[test]
fn a_start_before_the_first_sync_sample_uses_it() {
    // The video starts 0.5 s into the movie (an empty edit).
    let mut video = ibbp_video(50, 25);
    video.edits = vec![(500, -1), (2000, 1024)];
    let source = movie(&[video, aac_audio(100, vec![(2000, 2112)])]);
    let (summary, out) = clip(&source, &options(q(0, 1), Some(q(1, 1)))).unwrap();
    assert_eq!(summary.snapped_start, q(1, 2));
    assert_round_trip(&source, &out, &summary);
}

#[test]
fn audio_that_starts_after_the_clip_gets_an_empty_edit() {
    let video = ibbp_video(50, 25);
    // The audio starts 0.3 s into the movie.
    let source = movie(&[video, aac_audio(100, vec![(300, -1), (1700, 2112)])]);
    let (summary, out) = clip(&source, &options(q(0, 1), Some(q(1, 1)))).unwrap();
    assert_round_trip(&source, &out, &summary);
    let out_movie = probe(
        SliceSource::new(&out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    match out_movie.tracks[1].edit_list {
        // 0.3 s in the output's movie timescale, the least common multiple
        // of the video's 12,800 and the audio's 48,000.
        EditList::Simple { empty, .. } => assert_eq!(empty, 57_600),
        other => panic!("{other:?}"),
    }
}

#[test]
fn open_gop_leading_samples_are_kept_but_not_presented() {
    // GOP of 6 in decode order: I(sync) P B B, then a second I whose two
    // leading B frames come after it in decode order and are presented
    // before it.
    let s = |offset, sync| S {
        size: 30,
        duration: 512,
        offset,
        sync,
    };
    let samples = vec![
        s(1024, true),  // I0, frame 0
        s(2048, false), // P, frame 3
        s(512, false),  // B, frame 1
        s(512, false),  // B, frame 2
        s(2048, true),  // I (open GOP), frame 6
        s(512, false),  // leading B, frame 4
        s(512, false),  // leading B, frame 5
        s(2048, false), // P, frame 9
        s(512, false),  // B, frame 7
        s(512, false),  // B, frame 8
    ];
    let video = TrackSpec {
        video: true,
        timescale: 12_800,
        edits: vec![(400, 1024)],
        samples,
        entry: *b"avc1",
    };
    let source = movie(&[video]);
    // Start at frame 6 (sample 4): its leading frames 4 and 5 are presented
    // before it, so they are kept outside the edit.
    let start = q(6 * 512, 12_800);
    let (summary, out) = clip(&source, &options(start, None)).unwrap();
    assert_eq!(summary.snapped_start, start);
    assert_eq!(summary.video.first_sample, 4);
    assert_eq!(summary.video.samples, 6);
    assert_round_trip(&source, &out, &summary);
    let out_movie = probe(
        SliceSource::new(&out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let written = packets(&out, &out_movie, 0);
    assert!(!written[1].presented && !written[2].presented);
    assert!(written[0].presented && written[3].presented);
}

#[test]
fn negative_composition_offsets_are_copied_as_stored() {
    let s = |offset, sync| S {
        size: 30,
        duration: 512,
        offset,
        sync,
    };
    // A version 1 table: I P B with offsets 0, 512, -512 (frames 0, 2, 1,
    // then 3, 5, 4).
    let samples = vec![
        s(0, true),
        s(512, false),
        s(-512, false),
        s(0, true),
        s(512, false),
        s(-512, false),
    ];
    let video = TrackSpec {
        video: true,
        timescale: 12_800,
        edits: vec![],
        samples,
        entry: *b"avc1",
    };
    let source = movie(&[video]);
    // 125 ms lies after the second I frame (120 ms).
    let (summary, out) = clip(&source, &options(q(1, 8), None)).unwrap();
    assert_eq!(summary.video.first_sample, 3);
    assert_round_trip(&source, &out, &summary);
}

#[test]
fn a_last_sample_at_the_stream_end_is_not_copied() {
    // 50 frames of 40 ms, sync every 25; the last has a decode duration of
    // 0, so it is presented at the media duration (1.96 s), which the
    // stream's end is. The edit (1.961 s) presents it in the source.
    let samples = (0..50)
        .map(|i| S {
            size: 30,
            duration: if i == 49 { 0 } else { 512 },
            offset: 0,
            sync: i % 25 == 0,
        })
        .collect();
    let video = TrackSpec {
        video: true,
        timescale: 12_800,
        edits: vec![(1961, 0)],
        samples,
        entry: *b"avc1",
    };
    let source = movie(&[video]);
    for start in [q(0, 1), q(3, 2)] {
        let (summary, out) = clip(&source, &options(start, None)).unwrap();
        assert_eq!(summary.written_end, q(49, 25));
        assert_eq!(
            summary.video.first_sample + u64::from(summary.video.samples),
            49
        );
        assert_round_trip(&source, &out, &summary);
    }
}

#[test]
fn refusals_fail_before_writing() {
    let source = av_movie();
    let refused = |source: &[u8], options: &ClipOptions| {
        let mut out = Vec::new();
        let result = clip_mp4(
            SliceSource::new(source),
            &Limits::default(),
            &Allowlist::default(),
            options,
            &mut out,
        );
        assert!(out.is_empty());
        result.unwrap_err()
    };
    // An end at or before the start, a negative start, a start past the end.
    for (start, end) in [(q(2, 1), Some(q(2, 1))), (q(-1, 1), None), (q(5, 1), None)] {
        assert!(matches!(
            refused(&source, &options(start, end)),
            ExtractError::Source(Error::Unsupported { .. })
        ));
    }
    // avc3 carries parameter sets in the samples (choice 6).
    let mut video = ibbp_video(25, 25);
    video.entry = *b"avc3";
    let avc3 = movie(&[video]);
    assert!(matches!(
        refused(&avc3, &options(q(0, 1), None)),
        ExtractError::Source(Error::Unsupported { .. })
    ));
    // The audio track is not a video track; a missing stream.
    let mut wrong = options(q(0, 1), None);
    wrong.video = VideoChoice::Stream(1);
    assert!(matches!(
        refused(&source, &wrong),
        ExtractError::Source(Error::Unsupported { .. })
    ));
    wrong.video = VideoChoice::Stream(7);
    assert!(matches!(
        refused(&source, &wrong),
        ExtractError::Source(Error::NoSuchTrack)
    ));
    // Encrypted video (`encv`), which the default allowlist drops: the
    // clip has no video because its codec is not allowed.
    let mut video = ibbp_video(25, 25);
    video.entry = *b"encv";
    let encrypted = movie(&[video, aac_audio(50, vec![])]);
    let parsed = probe(
        SliceSource::new(&encrypted),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(parsed.dropped.len(), 1);
    assert_eq!(parsed.dropped[0].kind, TrackKind::Video);
    assert!(matches!(
        refused(&encrypted, &options(q(0, 1), None)),
        ExtractError::Source(Error::CodecNotAllowed {
            track_id: 1,
            fourcc: FourCc(f)
        }) if &f == b"encv"
    ));
    // MP3 in QuickTime's `.mp3` entry is audio the default choice
    // finds, and MP3 is not copied into a clip, in `.mp3` as in `mp4a`.
    let mut mp3 = aac_audio(50, vec![]);
    mp3.entry = *b".mp3";
    let with_mp3 = movie(&[ibbp_video(25, 25), mp3]);
    assert!(matches!(
        refused(&with_mp3, &options(q(0, 1), None)),
        ExtractError::Source(Error::Unsupported { .. })
    ));
    // Audio-only files have no video.
    let audio_only = movie(&[aac_audio(50, vec![])]);
    assert!(matches!(
        refused(&audio_only, &options(q(0, 1), None)),
        ExtractError::Source(Error::NoSuchTrack)
    ));
}

#[test]
fn audio_can_be_left_out() {
    let source = av_movie();
    let mut o = options(q(0, 1), Some(q(1, 1)));
    o.audio = AudioChoice::None;
    let (summary, out) = clip(&source, &o).unwrap();
    assert!(summary.audio.is_none());
    assert_eq!(summary.left_out.len(), 1);
    assert_eq!(summary.left_out[0].reason, LeftOutReason::NotSelected);
    assert_round_trip(&source, &out, &summary);
}

#[test]
fn table_bytes_are_limited() {
    let source = av_movie();
    let limits = Limits {
        max_table_bytes: 64,
        ..Limits::default()
    };
    let mut out = Vec::new();
    let result = clip_mp4(
        SliceSource::new(&source),
        &limits,
        &Allowlist::default(),
        &options(q(0, 1), None),
        &mut out,
    );
    assert!(matches!(
        result,
        Err(ExtractError::Source(Error::LimitExceeded { .. }))
    ));
    assert!(out.is_empty());
}

// --- Fixtures ----------------------------------------------------------------

/// Fixtures that must give clips: every one of the
/// required clip fixtures of `scripts/gen-fixtures.sh` that is generated.
const REQUIRED: &[&str] = &[
    "basic.mp4",
    "basic.mov",
    "faststart.mp4",
    "fragmented.mp4",
    "fragmented_moov.mp4",
    "edit_list.mp4",
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
    "edit_multi_splice.mp4",
    "edit_multi_bframes.mp4",
    "edit_multi_empty.mp4",
    "edit_multi_av.mp4",
    "clip_no_stss.mp4",
    "clip_no_stss_bframes.mp4",
    "clip_stss_all.mp4",
    "clip_stss_all_flash.mp4",
    "clip_hevc_stss_all.mp4",
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

/// A kept track for the manifest, with the byte offset and stored decode
/// time of its first sample, so that `scripts/clip_check.py` can find the
/// run in `ffprobe`'s packet list of the source.
fn json_track(source: &[u8], movie: &Movie, t: &ClipTrack) -> String {
    let track = movie
        .tracks
        .iter()
        .find(|x| x.stream_index == t.stream_index)
        .unwrap();
    let first = packets(source, movie, track.index)
        .into_iter()
        .nth(t.first_sample as usize)
        .unwrap();
    format!(
        "{{\"stream_index\": {}, \"first_sample\": {}, \"samples\": {}, \"timescale\": {}, \"first_pos\": {}, \"first_dts\": {}}}",
        t.stream_index, t.first_sample, t.samples, t.timescale, first.offset, first.dts
    )
}

/// Every MP4 and MOV fixture with a video track the writer accepts is
/// clipped at these ranges: from the start, starting
/// between sync samples, starting exactly on one, ending past the end, and
/// the whole file. Every MP4, MOV and M4A fixture without video whose audio
/// is AAC is clipped with [`clip_audio_mp4`] from the start, between two
/// frames, past the end, whole, and from 3 s to 8 s, which crosses the
/// windows of the edit-list fixtures and is refused on shorter files. With
/// `COREAV_CLIP_OUT` the outputs and a manifest are written for
/// `scripts/clip_check.py`.
#[test]
#[ignore = "needs COREAV_FIXTURES"]
fn fixtures_round_trip() {
    let dir = std::env::var("COREAV_FIXTURES").expect("COREAV_FIXTURES");
    let out_dir = std::env::var("COREAV_CLIP_OUT").ok();
    if let Some(out_dir) = &out_dir {
        std::fs::create_dir_all(out_dir).unwrap();
    }
    let generated = std::fs::read_to_string(Path::new(&dir).join("MANIFEST")).unwrap_or_default();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("mp4" | "mov" | "m4a")
            )
        })
        .collect();
    entries.sort();
    let mut manifest = Vec::new();
    let mut clipped = std::collections::BTreeSet::new();
    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        // Picture sizes that change inside the track (docs/design/
        // frame-sizes.md): the decoder reinitializes mid-stream, which the
        // clip oracle's frame comparison does not model.
        if matches!(name.as_str(), "size_inband_sps.mp4" | "size_vp9_resize.mp4") {
            continue;
        }
        let source = std::fs::read(path).unwrap();
        let Ok(movie) = probe(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
        ) else {
            continue;
        };
        let Some(video) = movie.tracks.iter().find(|t| t.kind == TrackKind::Video) else {
            let copied = movie.tracks.iter().any(|t| {
                t.kind == TrackKind::Audio
                    && matches!(
                        t.description.codec,
                        Codec::Aac | Codec::Flac | Codec::Alac | Codec::Ac3 | Codec::Eac3
                    )
            });
            if !copied {
                continue;
            }
            for (label, start, end) in [
                ("audio_head", q(0, 1), Some(q(1, 1))),
                ("audio_between", q(3, 5), Some(q(8, 5))),
                ("audio_past_end", q(3, 10), Some(q(100, 1))),
                ("audio_whole", q(0, 1), None),
                ("audio_late", q(3, 1), Some(q(8, 1))),
            ] {
                let (summary, output) =
                    match audio_clip(&source, TrackChoice::FirstAudio, start, end) {
                        Ok(done) => done,
                        Err(ExtractError::Source(Error::Unsupported { reason }))
                            if label == "audio_late" =>
                        {
                            eprintln!("{name} {label}: refused: {reason}");
                            continue;
                        }
                        Err(e) => panic!("{name} {label}: {e}"),
                    };
                assert_audio_round_trip(&source, &output, &summary);
                eprintln!(
                    "{name} {label}: start {start:?} end {:?}, {} audio samples",
                    summary.written_end, summary.audio.samples
                );
                if let Some(out_dir) = &out_dir {
                    let output_path = Path::new(out_dir).join(format!("{name}.{label}.mp4"));
                    std::fs::write(&output_path, &output).unwrap();
                    manifest.push(format!(
                        "{{\"source\": {}, \"output\": {}, \"label\": {}, \"start\": {}, \"end\": {}, \"snapped_start\": {}, \"written_end\": {}, \"video\": null, \"audio\": {}}}",
                        json_string(&path.display().to_string()),
                        json_string(&output_path.display().to_string()),
                        json_string(label),
                        json_rational(start),
                        end.map_or("null".to_string(), json_rational),
                        json_rational(summary.start),
                        json_rational(summary.written_end),
                        json_track(&source, &movie, &summary.audio),
                    ));
                }
            }
            continue;
        };
        // The second sync sample's presentation time, to start exactly on it.
        let syncs: Vec<Rational> = packets(&source, &movie, video.index)
            .iter()
            .filter(|p| p.keyframe && p.presented)
            .filter_map(|p| p.pts)
            .collect();
        let on_sync = syncs
            .get(1)
            .map(|p| q(p.num, p.den * u64::from(video.timescale)));
        let mut ranges = vec![
            ("head", q(0, 1), Some(q(1, 1))),
            ("between", q(3, 5), Some(q(8, 5))),
            ("past_end", q(3, 10), Some(q(100, 1))),
            ("whole", q(0, 1), None),
        ];
        if let Some(on_sync) = on_sync {
            ranges.push(("on_sync", on_sync, None));
        }
        // An empty edit between two media edits: a clip that ends in the
        // middle of it, and one that starts there.
        if video.edit_list == EditList::Multiple {
            let mut at = 0u64;
            for edit in &video.edits {
                if edit.media_time == -1 && at > 0 {
                    let mid = q(
                        i128::from(at * 2 + edit.segment_duration),
                        2 * u64::from(movie.timescale),
                    );
                    let before = q(mid.num - i128::from(mid.den), mid.den);
                    ranges.push(("end_in_gap", before, Some(mid)));
                    ranges.push(("start_in_gap", mid, None));
                    break;
                }
                at += edit.segment_duration;
            }
        }
        for (label, start, end) in ranges {
            let result = clip(&source, &options(start, end));
            let (summary, output) = match result {
                Ok(done) => done,
                Err(ExtractError::Source(Error::Unsupported { reason })) => {
                    assert!(
                        !REQUIRED.contains(&name.as_str()),
                        "{name} {label}: refused: {reason}"
                    );
                    eprintln!("{name} {label}: refused: {reason}");
                    continue;
                }
                Err(e) => panic!("{name} {label}: {e}"),
            };
            assert_round_trip(&source, &output, &summary);
            clipped.insert(name.clone());
            eprintln!(
                "{name} {label}: start {:?} snapped {:?} end {:?}, {} video and {} audio samples",
                start,
                summary.snapped_start,
                summary.written_end,
                summary.video.samples,
                summary.audio.map_or(0, |a| a.samples)
            );
            if let Some(out_dir) = &out_dir {
                // The source's extension stays in the name: basic.mp4 and
                // basic.mov give different clips.
                let output_path = Path::new(out_dir).join(format!("{name}.{label}.mp4"));
                std::fs::write(&output_path, &output).unwrap();
                manifest.push(format!(
                    "{{\"source\": {}, \"output\": {}, \"label\": {}, \"start\": {}, \"end\": {}, \"snapped_start\": {}, \"written_end\": {}, \"video\": {}, \"audio\": {}}}",
                    json_string(&path.display().to_string()),
                    json_string(&output_path.display().to_string()),
                    json_string(label),
                    json_rational(start),
                    end.map_or("null".to_string(), json_rational),
                    json_rational(summary.snapped_start),
                    json_rational(summary.written_end),
                    json_track(&source, &movie, &summary.video),
                    summary
                        .audio
                        .as_ref()
                        .map_or("null".to_string(), |a| json_track(&source, &movie, a)),
                ));
            }
        }
    }
    for name in REQUIRED {
        if generated.contains(&format!("generated {name}")) {
            assert!(clipped.contains(*name), "{name} gave no clip");
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
fn an_open_ended_media_edit_of_a_fragmented_source_stays_open() {
    // Four 512-tick samples from fragments; the edit starts at media time
    // 512 and lasts to the end of the media, so the source presents the
    // last three (1,536 ticks).
    let source = common::fragmented_movie(None, Some((0, 512)));
    let (summary, out) = clip(&source, &options(q(0, 1), None)).unwrap();
    let out_movie = probe(
        SliceSource::new(&out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let presented: Vec<Rational> = packets(&out, &out_movie, 0)
        .iter()
        .filter(|p| p.presented)
        .filter_map(|p| p.pts)
        .collect();
    assert_eq!(
        presented,
        [0, 512, 1024].map(Rational::from_int),
        "{summary:?}"
    );
    assert_eq!(out_movie.tracks[0].duration, Some(Rational::from_int(1536)));
    // The sample before the edit is not needed (every sample is a sync
    // sample), so it is not copied.
    assert_eq!(packets(&out, &out_movie, 0).len(), 3);
}

/// A source that records the advice it gets.
struct Advised<'a> {
    data: SliceSource<'a>,
    advice: Vec<(u64, u64)>,
}

impl coreav_isobmff::Source for Advised<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        self.data.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        self.data.read_exact_at(offset, buf)
    }

    fn will_read(&mut self, offset: u64, len: u64) {
        self.advice.push((offset, len));
    }
}

#[test]
fn the_copy_pass_advises_where_it_starts_and_how_much_it_copies() {
    let source = av_movie();
    let mut advised = Advised {
        data: SliceSource::new(&source),
        advice: Vec::new(),
    };
    let mut out = Vec::new();
    let summary = clip_mp4(
        &mut advised,
        &Limits::default(),
        &Allowlist::default(),
        &options(q(3, 2), Some(q(5, 2))),
        &mut out,
    )
    .unwrap();
    let movie = coreav_isobmff::probe(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let mut first = u64::MAX;
    let mut bytes = 0u64;
    for kept in [Some(summary.video), summary.audio].into_iter().flatten() {
        let track = movie
            .tracks
            .iter()
            .position(|t| t.stream_index == kept.stream_index)
            .unwrap();
        let all = packets(&source, &movie, track);
        let copied = &all[kept.first_sample as usize..][..kept.samples as usize];
        first = first.min(copied[0].offset);
        bytes += copied.iter().map(|p| u64::from(p.size)).sum::<u64>();
    }
    assert_eq!(advised.advice, vec![(first, bytes)]);
}

/// 20 frames of 40 ms with an IDR picture at 0 and 10 and P pictures
/// between, every sample marked sync (no `stss`) unless `table`, which
/// keeps a sync sample table listing every sample but the last.
fn idr_every_ten(table: bool) -> Vec<u8> {
    let samples = (0..20)
        .map(|n| S {
            size: 40,
            duration: 512,
            offset: 0,
            sync: !table || n != 19,
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
    let idr: Vec<bool> = (0..20).map(|n| n % 10 == 0).collect();
    h264_slices(&mut source, &idr);
    source
}

#[test]
fn a_cut_without_a_sync_table_starts_on_a_decodable_frame() {
    // docs/design/cut-points.md: without stss every sample is a sync
    // sample; the coded data says only the IDR pictures start decoding.
    // 500 ms is frame 12, a P picture: the clip starts at frame 10.
    let source = idr_every_ten(false);
    let (summary, out) = clip(&source, &options(q(1, 2), None)).unwrap();
    assert_eq!(summary.video.first_sample, 10);
    assert_round_trip(&source, &out, &summary);
    // The clip marks only the IDR picture as a sync sample.
    let out_movie = probe(
        SliceSource::new(&out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let flags: Vec<bool> = packets(&out, &out_movie, 0)
        .iter()
        .map(|p| p.keyframe)
        .collect();
    assert_eq!(flags, (0..10).map(|n| n == 0).collect::<Vec<_>>());
    // The same when the sync sample table lists the P pictures.
    let source = idr_every_ten(true);
    let (summary, _) = clip(&source, &options(q(1, 2), None)).unwrap();
    assert_eq!(summary.video.first_sample, 10);
}

/// 20 frames of 40 ms without a sync sample table, coded as `kinds`
/// (see `h264_frames`).
fn untabled(kinds: &str) -> Vec<u8> {
    let samples = (0..20)
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
    h264_frames(&mut source, kinds);
    source
}

#[test]
fn an_exact_recovery_point_starts_a_clip() {
    // An IDR picture at 0, an exact recovery point at 8: 400 ms (frame 10)
    // starts at 8.
    let source = untabled("IPPPPPPPRPPPPPPPPPPP");
    let (summary, out) = clip(&source, &options(q(2, 5), None)).unwrap();
    assert_eq!(summary.video.first_sample, 8);
    assert_round_trip(&source, &out, &summary);
}

#[test]
fn a_track_with_no_decodable_start_is_refused() {
    // No sync sample table and only P pictures: nothing to start from.
    let source = untabled("PPPPPPPPPPPPPPPPPPPP");
    assert!(matches!(
        clip(&source, &options(q(2, 5), None)),
        Err(ExtractError::Source(Error::Unsupported { .. }))
    ));
}

/// 20 frames of 40 ms decoded in an order that shows frame 10 (a leading
/// picture) after 11 and 12 are decoded: a recovery point at decode index
/// 11 and an IDR picture at 16. Without `table` there is no sync sample
/// table; with it, the table lists 0, the P frame at 10, 11 and 16.
fn reordered(table: bool) -> Vec<u8> {
    // Display index of each decoded frame; frame k is presented at
    // (k + 2) * 40 ms, so frame 10 at 0.48 s to 0.52 s.
    let display = [
        0, 3, 1, 2, 6, 4, 5, 9, 7, 8, 11, 12, 10, 13, 14, 15, 16, 17, 18, 19,
    ];
    let samples = display
        .iter()
        .enumerate()
        .map(|(d, &k)| S {
            size: 40,
            duration: 512,
            offset: (k - d as i32 + 2) * 512,
            sync: !table || matches!(d, 0 | 10 | 11 | 16),
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
    h264_frames(&mut source, "IPPPPPPPPPPRPPPPIPPP");
    source
}

#[test]
fn a_sync_sample_shown_after_the_end_keeps_the_leading_frames_before_it() {
    // The P frame decoded at 10 is shown at 0.52 s, after the end; the
    // recovery point after it is a real cut point, and the frame shown at
    // 0.48 s is decoded after both. The clip must keep it.
    for table in [false, true] {
        let source = reordered(table);
        for (start, end) in [
            (q(97, 200), q(51, 100)),
            (q(3, 10), q(51, 100)),
            (q(97, 200), q(3, 5)),
        ] {
            let (summary, out) = clip(&source, &options(start, Some(end))).unwrap();
            assert_eq!(summary.video.first_sample, 0, "table {table}");
            assert_eq!(
                summary.video.samples, 13,
                "table {table}, {start:?} to {end:?}"
            );
            assert_round_trip(&source, &out, &summary);
        }
    }
}

/// The AV1 fixture whose table lists its key frames, and the same file
/// without the table, or `None` when the fixture was not generated.
fn av1_with_and_without_table() -> Option<(Vec<u8>, Vec<u8>)> {
    let dir = std::env::var("COREAV_FIXTURES").expect("COREAV_FIXTURES");
    let generated = std::fs::read_to_string(Path::new(&dir).join("MANIFEST")).unwrap_or_default();
    if !generated.lines().any(|l| l == "generated split_av1.mp4") {
        return None;
    }
    let source = std::fs::read(Path::new(&dir).join("split_av1.mp4")).unwrap();
    let untabled = without_sync_table(&source);
    assert_ne!(untabled, source, "split_av1.mp4 has no sync sample table");
    Some((source, untabled))
}

#[test]
#[ignore = "needs COREAV_FIXTURES"]
fn fixtures_round_trip_av1_without_a_sync_table() {
    // docs/design/cut-points.md: without its table every sample of
    // split_av1.mp4 is a sync sample; the coded data says only its key
    // frames are. Clips start where the table would have them start, and
    // mark the same samples as sync.
    let Some((source, untabled)) = av1_with_and_without_table() else {
        eprintln!("split_av1.mp4 was not generated");
        return;
    };
    for (start, end) in [(q(1, 2), q(3, 2)), (q(3, 2), q(5, 2)), (q(5, 2), q(7, 2))] {
        let (expected, expected_out) = clip(&source, &options(start, Some(end))).unwrap();
        let (summary, out) = clip(&untabled, &options(start, Some(end))).unwrap();
        assert_eq!(summary.video.first_sample, expected.video.first_sample);
        assert_eq!(summary.video.samples, expected.video.samples);
        assert_round_trip(&untabled, &out, &summary);
        let flags = |data: &[u8]| -> Vec<bool> {
            let movie = probe(
                SliceSource::new(data),
                &Limits::default(),
                &Allowlist::default(),
            )
            .unwrap();
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
        assert_eq!(flags(&out), flags(&expected_out));
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
    let parsed = probe(
        SliceSource::new(output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    parsed.tracks[index]
        .edits
        .iter()
        .map(|e| (e.segment_duration, e.media_time))
        .collect()
}

#[test]
fn a_clip_across_a_splice_keeps_both_windows() {
    // Seconds 0 to 1 of the media, then 2 to 4: the second GOP is cut out.
    let source = spliced(vec![(1000, 1024), (2000, 1024 + 25_600)]);
    let (summary, out) = clip(&source, &options(q(1, 2), Some(q(2, 1)))).unwrap();
    assert_eq!(summary.snapped_start, q(0, 1));
    assert_eq!(summary.written_end, q(2, 1));
    // The decode-order run reaches into the third GOP, through the cut-out
    // second one, which no edit presents.
    assert_eq!(summary.video.first_sample, 0);
    assert!(summary.video.samples > 50);
    assert_round_trip(&source, &out, &summary);
    // Two media edits: the first second, then the second second of the
    // clip from the third GOP's media on (movie timescale 192,000, a common
    // multiple of the video's and the audio's).
    assert_eq!(
        output_edits(&out, 0),
        vec![(192_000, 1024), (192_000, 1024 + 25_600)]
    );
}

#[test]
fn a_clip_inside_one_window_has_one_edit() {
    let source = spliced(vec![(1000, 1024), (2000, 1024 + 25_600)]);
    let (summary, out) = clip(&source, &options(q(3, 2), Some(q(5, 2)))).unwrap();
    // The third GOP's sync sample is presented at 1 s.
    assert_eq!(summary.snapped_start, q(1, 1));
    assert_eq!(summary.video.first_sample, 50);
    assert_round_trip(&source, &out, &summary);
    assert_eq!(output_edits(&out, 0), vec![(288_000, 1024)]);
}

#[test]
fn an_empty_edit_between_windows_is_kept() {
    // Half a second of nothing between the two windows.
    let source = spliced(vec![(1000, 1024), (500, -1), (1000, 1024 + 25_600)]);
    let (summary, out) = clip(&source, &options(q(0, 1), Some(q(5, 2)))).unwrap();
    assert_eq!(summary.written_end, q(5, 2));
    assert_round_trip(&source, &out, &summary);
    assert_eq!(
        output_edits(&out, 0),
        vec![(192_000, 1024), (96_000, -1), (192_000, 1024 + 25_600)]
    );
}

#[test]
fn audio_with_several_edits_keeps_its_windows() {
    // Audio: its first second after priming, then a skip of one second.
    let audio = aac_audio(190, vec![(1000, 2112), (2000, 2112 + 96_000)]);
    let source = movie(&[ibbp_video(100, 25), audio]);
    let (summary, out) = clip(&source, &options(q(0, 1), Some(q(5, 2)))).unwrap();
    assert!(summary.audio.is_some());
    assert_round_trip(&source, &out, &summary);
    let edits = output_edits(&out, 1);
    // The first second, then the half second of the second window inside
    // the clip, from 3 s of the media on.
    assert_eq!(edits.len(), 2);
    assert_eq!(
        edits[0].0 + edits[1].0,
        192_000 * 5 / 2 - edits_start(&edits)
    );
}

/// Where the audio starts after the clip's start (an empty edit first): 0
/// here.
fn edits_start(edits: &[(u64, i64)]) -> u64 {
    edits.iter().take_while(|e| e.1 == -1).map(|e| e.0).sum()
}

#[test]
fn several_edits_that_cannot_be_written_exactly_refuse() {
    let refused = |source: &[u8]| {
        let mut out = Vec::new();
        let result = clip_mp4(
            SliceSource::new(source),
            &Limits::default(),
            &Allowlist::default(),
            &options(q(0, 1), None),
            &mut out,
        );
        let result = result.map(|_| ()).map_err(|e| (e, out.len()));
        let (error, written) = result.unwrap_err();
        assert_eq!(written, 0, "{error:?}");
        match error {
            ExtractError::Source(Error::Unsupported { reason }) => reason,
            other => panic!("{other:?}"),
        }
    };
    // Overlapping windows are outside the edit model.
    let source = spliced(vec![(2000, 1024), (2000, 1024 + 6400)]);
    assert_eq!(refused(&source), "edit list outside the supported model");
    // A whole window inside the clip that lasts 4,294,967.291 media ticks: a
    // media timescale of 4,294,967,291 in a 1,000 Hz movie, so its duration
    // is whole only in a movie timescale above 32 bits. Frames of 1,000,000
    // ticks so that the window holds several.
    let mut video = ibbp_video(100, 25);
    video.timescale = 4_294_967_291;
    for sample in &mut video.samples {
        sample.duration = 1_000_000;
    }
    video.edits = vec![(1, 1024), (1, 50_000_000)];
    let source = movie(&[video]);
    assert_eq!(
        refused(&source),
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

/// `source` with the entries of its first edit list replaced by `entries`,
/// `(segment duration, media time, media rate as 16.16)`, and the sizes of
/// the boxes around it (`moov`, the first `trak`, `edts`) adjusted. For
/// sources whose sample offsets do not move with `moov`: a movie with
/// `mdat` first, or fragments addressed from their `moof`.
fn with_edits(source: &[u8], entries: &[(u32, i32, u32)]) -> Vec<u8> {
    let find = |tag: &[u8; 4]| source.windows(4).position(|w| w == tag).unwrap() - 4;
    let size = |at: usize| u32::from_be_bytes(source[at..at + 4].try_into().unwrap());
    let elst = find(b"elst");
    let mut body = vec![0u8; 4];
    body.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (duration, media_time, rate) in entries {
        body.extend_from_slice(&duration.to_be_bytes());
        body.extend_from_slice(&media_time.to_be_bytes());
        body.extend_from_slice(&rate.to_be_bytes());
    }
    let new_size = 8 + body.len() as u32;
    let grow = new_size - size(elst);
    let mut out = source[..elst].to_vec();
    out.extend_from_slice(&new_size.to_be_bytes());
    out.extend_from_slice(b"elst");
    out.extend_from_slice(&body);
    out.extend_from_slice(&source[elst + size(elst) as usize..]);
    for tag in [b"moov", b"trak", b"edts"] {
        let at = find(tag);
        out[at..at + 4].copy_from_slice(&(size(at) + grow).to_be_bytes());
    }
    out
}

#[test]
fn every_edit_list_outside_the_model_refuses() {
    // Common decision 1 of docs/design/edit-list-writers.md: each case
    // fails with its message and writes nothing.
    let refused = |source: &[u8]| {
        let mut out = Vec::new();
        let result = clip_mp4(
            SliceSource::new(source),
            &Limits::default(),
            &Allowlist::default(),
            &options(q(0, 1), None),
            &mut out,
        );
        let (error, written) = result.map(|_| ()).map_err(|e| (e, out.len())).unwrap_err();
        assert_eq!(written, 0, "{error:?}");
        match error {
            ExtractError::Source(Error::Unsupported { reason }) => reason,
            other => panic!("{other:?}"),
        }
    };
    let outside = "edit list outside the supported model";
    let base = spliced(vec![(1000, 1024), (2000, 1024 + 25_600)]);
    // Backward windows: the second before the first in media time.
    assert_eq!(
        refused(&spliced(vec![(1000, 1024 + 25_600), (1000, 1024)])),
        outside
    );
    // A rate other than 1, and a dwell (rate 0), in either edit.
    for rate in [0x8000, 0x20000, 0] {
        let source = with_edits(&base, &[(1000, 1024, 0x10000), (2000, 26_624, rate)]);
        assert_eq!(refused(&source), outside, "rate {rate:#x}");
    }
    // A media time below -1.
    assert_eq!(refused(&spliced(vec![(1000, -2), (2000, 1024)])), outside);
    // A media edit of duration 0 in a movie without fragments.
    assert_eq!(
        refused(&spliced(vec![(0, 1024), (2000, 1024 + 25_600)])),
        outside
    );
    // Several edits in a movie with fragments.
    let fragmented = with_edits(
        &common::fragmented_movie(None, Some((0, 0))),
        &[(1, 0, 0x10000), (1, 1024, 0x10000)],
    );
    assert_eq!(refused(&fragmented), outside);
}

#[test]
fn audio_that_starts_in_a_gap_waits_for_its_window() {
    // Audio: one second, half a second of nothing, then from 1.5 s on, half
    // a second of media later (no sample is shown by both windows, which
    // the round trip's one presentation time per sample cannot express).
    let audio = aac_audio(190, vec![(1000, 2112), (500, -1), (1500, 2112 + 72_000)]);
    let source = movie(&[ibbp_video(100, 25), audio]);
    // From the sync sample at 1 s: the audio's gap first.
    let (summary, out) = clip(&source, &options(q(6, 5), Some(q(5, 2)))).unwrap();
    assert_eq!(summary.snapped_start, q(1, 1));
    assert!(summary.audio.is_some());
    assert_round_trip(&source, &out, &summary);
    let edits = output_edits(&out, 1);
    assert_eq!(edits.first(), Some(&(96_000, -1)), "{edits:?}");
    // A clip inside the gap has no audio, and keeps its video.
    let (summary, out) = clip(&source, &options(q(1, 1), Some(q(7, 5)))).unwrap();
    assert!(summary.audio.is_none());
    assert_round_trip(&source, &out, &summary);
}

#[test]
fn an_end_between_windows_ends_where_the_earlier_one_does() {
    let source = spliced(vec![(1000, 1024), (500, -1), (1000, 1024 + 25_600)]);
    for (start, end) in [(q(0, 1), q(5, 4)), (q(1, 2), q(3, 2))] {
        let (summary, out) = clip(&source, &options(start, Some(end))).unwrap();
        assert_eq!(summary.written_end, q(1, 1), "{start:?} to {end:?}");
        assert_round_trip(&source, &out, &summary);
        assert_eq!(output_edits(&out, 0), vec![(192_000, 1024)]);
    }
}

#[test]
fn a_window_that_ends_between_video_ticks_ends_the_clip_there() {
    // A 1,000 Hz movie and a 12,800 Hz video: an edit of 1,001 movie ticks
    // ends 12,812.8 video ticks in.
    for (edits, end) in [
        (vec![(1000, 1024), (1001, 1024 + 25_600)], None),
        (
            vec![(1001, 1024), (500, -1), (1000, 1024 + 25_600)],
            Some(q(5, 4)),
        ),
    ] {
        let source = spliced(edits);
        let (summary, out) = clip(&source, &options(q(0, 1), end)).unwrap();
        let presented: u64 = output_edits(&out, 0).iter().map(|e| e.0).sum();
        let movie_timescale = u64::from(
            probe(
                SliceSource::new(&out),
                &Limits::default(),
                &Allowlist::default(),
            )
            .unwrap()
            .timescale,
        );
        assert_eq!(
            summary.written_end,
            q(i128::from(presented), movie_timescale),
            "{end:?}"
        );
        assert_round_trip(&source, &out, &summary);
    }
}

#[test]
fn a_long_gop_without_a_sync_table_starts_at_its_idr_picture() {
    // 1,500 frames, an IDR picture at 0 and P pictures after it, no stss:
    // more sync samples before the start than the start search keeps
    // (1,024), so it checks them as listed, and still starts at frame 0.
    let samples = (0..1500)
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
    let idr: Vec<bool> = (0..1500).map(|n| n == 0).collect();
    h264_slices(&mut source, &idr);
    // 60 s of frames; a clip from 56 s to 58 s.
    let (summary, out) = clip(&source, &options(q(56, 1), Some(q(58, 1)))).unwrap();
    assert_eq!(summary.video.first_sample, 0);
    assert_round_trip(&source, &out, &summary);
}

#[test]
fn a_fragmented_track_marked_sync_throughout_starts_at_a_random_access_point() {
    // docs/design/cut-points.md: 2,000 fragment samples that the track
    // defaults mark as sync. With an IDR picture every second frame, more
    // than the 1,024 candidates the start search keeps lie before a start
    // near the end; it keeps the latest and starts at the IDR picture at or
    // before the start, not at the P picture the flags also offer.
    let kinds: String = (0..2000)
        .map(|n| if n % 2 == 0 { 'I' } else { 'P' })
        .collect();
    let source = fragmented_video(&kinds);
    // Frame 1,901 (a P picture) is at 76.04 s.
    let (summary, out) = clip(&source, &options(q(7604, 100), Some(q(77, 1)))).unwrap();
    assert_eq!(summary.video.first_sample, 1900);
    assert_round_trip(&source, &out, &summary);
    // With an IDR picture at 0 only, none of the 1,024 kept candidates is a
    // random access point: the listing checks the sync samples from the
    // track's start, and the clip starts at frame 0, never on a P picture.
    let kinds: String = (0..2000).map(|n| if n == 0 { 'I' } else { 'P' }).collect();
    let source = fragmented_video(&kinds);
    let (summary, out) = clip(&source, &options(q(7604, 100), Some(q(77, 1)))).unwrap();
    assert_eq!(summary.video.first_sample, 0);
    assert_round_trip(&source, &out, &summary);
}
