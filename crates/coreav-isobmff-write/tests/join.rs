//! Join round trips (`docs/design/join.md`): the output is probed by
//! `coreav-isobmff`; each output track must hold the inputs' samples one
//! input after another, every sample once and in order, with each input's
//! bytes, sizes, decode durations (the last one of an input possibly
//! extended), sync flags and composition offsets. The output's edit lists,
//! read here independently of the writer, must present every sample an
//! input presents at its input time plus the input's offset, and nothing
//! else. Incompatible inputs must fail with the input and the difference.
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
    Allowlist, Budget, Codec, EditList, Error, FourCc, Limits, Movie, Packet, Rational,
    SliceSource, Source, TrackKind, probe,
};
use coreav_isobmff_write::{
    AudioChoice, ClipOptions, ExtractError, JoinAudio, JoinError, JoinOptions, JoinSummary,
    JoinTrack, Mismatch, SplitAt, SplitOptions, VideoChoice, clip_mp4, join_mp4, split_mp4,
};
use movies::{S, TrackSpec, aac_audio, av_movie, ibbp_video, movie};
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

fn q(num: i128, den: u64) -> Rational {
    Rational::new(num, den).unwrap()
}

fn sum(a: Rational, b: Rational) -> Rational {
    a.checked_add(b).unwrap()
}

fn join_with(inputs: &[&[u8]], audio: JoinAudio) -> Result<(JoinSummary, Vec<u8>), JoinError> {
    let mut out = Vec::new();
    let summary = join_mp4(
        inputs.iter().map(|i| SliceSource::new(i)).collect(),
        &Limits::default(),
        &Allowlist::default(),
        &JoinOptions { audio },
        &mut out,
    )?;
    assert_eq!(summary.bytes_written, out.len() as u64);
    Ok((summary, out))
}

fn join(inputs: &[&[u8]]) -> Result<(JoinSummary, Vec<u8>), JoinError> {
    join_with(inputs, JoinAudio::First)
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

// --- Edit lists, read independently of the writer (ISO/IEC 14496-12) -----

fn children(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= data.len() {
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
        out.push((kind, &data[at + 8..at + size]));
        at += size;
    }
    out
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(data)
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, b)| b)
}

/// A track's movie timescale, media timescale and edit list as (duration,
/// media time).
type EditListOf = (u32, u32, Vec<(u64, i64)>);

/// Per track, its [`EditListOf`].
fn edit_lists(file: &[u8]) -> Vec<EditListOf> {
    let moov = child(file, b"moov").unwrap();
    let mvhd = child(moov, b"mvhd").unwrap();
    let tm = if mvhd[0] == 1 {
        u32::from_be_bytes(mvhd[20..24].try_into().unwrap())
    } else {
        u32::from_be_bytes(mvhd[12..16].try_into().unwrap())
    };
    let mut out = Vec::new();
    for (kind, trak) in children(moov) {
        if &kind != b"trak" {
            continue;
        }
        let mdhd = child(child(trak, b"mdia").unwrap(), b"mdhd").unwrap();
        let ts = if mdhd[0] == 1 {
            u32::from_be_bytes(mdhd[20..24].try_into().unwrap())
        } else {
            u32::from_be_bytes(mdhd[12..16].try_into().unwrap())
        };
        let mut edits = Vec::new();
        if let Some(elst) = child(trak, b"edts").and_then(|e| child(e, b"elst")) {
            let count = u32::from_be_bytes(elst[4..8].try_into().unwrap()) as usize;
            for i in 0..count {
                if elst[0] == 1 {
                    let e = &elst[8 + i * 20..];
                    edits.push((
                        u64::from_be_bytes(e[..8].try_into().unwrap()),
                        i64::from_be_bytes(e[8..16].try_into().unwrap()),
                    ));
                } else {
                    let e = &elst[8 + i * 12..];
                    edits.push((
                        u64::from(u32::from_be_bytes(e[..4].try_into().unwrap())),
                        i64::from(i32::from_be_bytes(e[4..8].try_into().unwrap())),
                    ));
                }
            }
        }
        out.push((tm, ts, edits));
    }
    out
}

/// Where the edit list presents a sample whose composition interval is
/// `[cts, cts + sample_duration)`, in seconds (its start may lie before the
/// edit's when the edit starts inside it), when an edit's media range
/// overlaps that interval; `None` when no edit does.
fn present(
    edits: &[(u64, i64)],
    tm: u32,
    ts: u32,
    cts: i64,
    sample_duration: u32,
) -> Option<Rational> {
    if edits.is_empty() {
        return Some(q(i128::from(cts), u64::from(ts)));
    }
    let mut at = q(0, 1);
    for &(duration, media_time) in edits {
        let length = q(i128::from(duration), u64::from(tm));
        if media_time >= 0 {
            let start = q(i128::from(media_time), u64::from(ts));
            let end = sum(start, length);
            let t = q(i128::from(cts), u64::from(ts));
            let t_end = sum(t, q(i128::from(sample_duration), u64::from(ts)));
            // Its interval overlaps the edit's media range (a sample of
            // duration 0 when its start lies in it), as ISO/IEC 14496-12
            // presents the media from the edit's media time.
            let overlaps = if sample_duration > 0 {
                t.checked_lt(end).unwrap() && start.checked_lt(t_end).unwrap()
            } else {
                !t.checked_lt(start).unwrap() && t.checked_lt(end).unwrap()
            };
            if overlaps {
                return Some(sum(at, sum(t, q(-start.num, start.den))));
            }
        }
        at = sum(at, length);
    }
    None
}

fn seconds(r: Rational) -> f64 {
    r.num as f64 / r.den as f64
}

/// The round trip: each output track is the inputs' samples, in order, and
/// presents exactly what each input presents, offset by the input's offset.
fn assert_join(inputs: &[&[u8]], output: &[u8], summary: &JoinSummary) {
    let out_movie = probe_all(output);
    assert!(!out_movie.has_fragments());
    let lists = edit_lists(output);
    for (track_index, kind) in [TrackKind::Video, TrackKind::Audio].into_iter().enumerate() {
        let Some(out_track) = out_movie.tracks.iter().find(|t| t.kind == kind) else {
            assert!(
                summary
                    .inputs
                    .iter()
                    .all(|i| kind == TrackKind::Video || i.audio.is_none())
            );
            continue;
        };
        let (tm, ts, edits) = &lists[track_index];
        let written = packets(output, &out_movie, out_track.index);
        let mut next = 0usize;
        for (n, (input, joined)) in inputs.iter().zip(&summary.inputs).enumerate() {
            let in_movie = probe_all(input);
            let kept = if kind == TrackKind::Video {
                joined.video
            } else {
                joined.audio.unwrap()
            };
            let in_track = in_movie
                .tracks
                .iter()
                .find(|t| t.stream_index == kept.stream_index)
                .unwrap();
            assert_eq!(in_track.timescale, *ts);
            let run = packets(input, &in_movie, in_track.index);
            assert_eq!(run.len(), kept.samples as usize);
            let out_run = &written[next..next + run.len()];
            let base = out_run[0].dts - run[0].dts;
            for (k, (s, o)) in run.iter().zip(out_run).enumerate() {
                assert_eq!(o.size, s.size, "input {n} sample {k}");
                assert_eq!(bytes(output, o), bytes(input, s), "input {n} sample {k}");
                assert_eq!(o.keyframe, s.keyframe);
                assert_eq!(o.cts - o.dts, s.cts - s.dts);
                assert_eq!(o.dts, s.dts + base, "input {n} sample {k}: decode time");
                let last = k + 1 == run.len();
                if last && kind == TrackKind::Video {
                    assert_eq!(
                        u64::from(o.duration),
                        u64::from(s.duration) + joined.extended
                    );
                } else {
                    assert_eq!(o.duration, s.duration, "input {n} sample {k}");
                }
                // The input's own presentation (one simple edit at most).
                let shown = if s.presented { s.pts } else { None };
                let in_seconds = shown.map(|p| q(p.num, p.den * u64::from(*ts)));
                let out_seconds = present(edits, *tm, *ts, o.cts, o.duration);
                match (in_seconds, out_seconds) {
                    (Some(a), Some(b)) => {
                        let gap = seconds(sum(a, joined.offset)) - seconds(b);
                        let tick = 1.0 / f64::from(*ts);
                        assert!(
                            gap.abs() <= if kind == TrackKind::Video { 0.0 } else { tick },
                            "{kind:?} input {n} sample {k}: {gap} s apart"
                        );
                    }
                    (None, Some(b)) => {
                        panic!(
                            "{kind:?} input {n} sample {k} presented only in the output at {b:?}"
                        )
                    }
                    // A sample whose start the input's edit cuts off (audio
                    // priming that ends inside the edit, say) is presented
                    // from inside it in both.
                    (Some(a), None) => {
                        let edit_start = in_track
                            .presentation_offset(in_movie.timescale)
                            .unwrap_or(q(0, 1));
                        assert!(
                            kind == TrackKind::Audio
                                && s.pts.unwrap().checked_lt(edit_start).unwrap(),
                            "{kind:?} input {n} sample {k} at {a:?} missing from the output"
                        );
                    }
                    (None, None) => {}
                }
            }
            next += run.len();
        }
        assert_eq!(next, written.len(), "every sample once");
    }
}

fn clip(source: &[u8], start: Rational, end: Option<Rational>) -> Result<Vec<u8>, ExtractError> {
    let mut out = Vec::new();
    clip_mp4(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
        &ClipOptions {
            video: VideoChoice::First,
            audio: AudioChoice::First,
            start,
            end,
        },
        &mut out,
    )?;
    Ok(out)
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

fn split(source: &[u8], step: Rational) -> Vec<Vec<u8>> {
    split_from(source, step).1
}

/// The segments of `source`, with the first segment's start on the
/// source's timeline.
fn split_from(source: &[u8], step: Rational) -> (Rational, Vec<Vec<u8>>) {
    try_split_from(source, step).unwrap()
}

/// [`split_from`], or the split's refusal.
fn try_split_from(source: &[u8], step: Rational) -> Result<(Rational, Vec<Vec<u8>>), ExtractError> {
    let outputs = Rc::new(RefCell::new(Vec::new()));
    let opened = Rc::clone(&outputs);
    let summary = split_mp4(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
        &SplitOptions {
            video: VideoChoice::First,
            audio: AudioChoice::First,
            at: SplitAt::Every(step),
        },
        move |index| {
            opened.borrow_mut().push(Vec::new());
            Ok(Sink(Rc::clone(&opened), index))
        },
    )?;
    let outputs = outputs.borrow().clone();
    Ok((summary.segments[0].start, outputs))
}

#[test]
fn split_segments_join_back_into_the_source() {
    let source = av_movie();
    let segments = split(&source, q(1, 1));
    assert_eq!(segments.len(), 4);
    let inputs: Vec<&[u8]> = segments.iter().map(Vec::as_slice).collect();
    let (summary, out) = join(&inputs).unwrap();
    assert_join(&inputs, &out, &summary);
    // One edit per track: the segments meet exactly.
    assert_eq!(summary.video_edits, 1);
    assert!(summary.audio_edits <= 1);
    assert!(summary.inputs.iter().all(|i| i.extended == 0));
    // The source's samples, times and edits.
    let (src, joined) = (probe_all(&source), probe_all(&out));
    for (a, b) in src.tracks.iter().zip(&joined.tracks) {
        // The same edit, in each file's movie timescale.
        assert_eq!(
            a.media_edit_duration(src.timescale),
            b.media_edit_duration(joined.timescale),
            "{:?}",
            a.kind
        );
        assert_eq!(
            a.presentation_offset(src.timescale),
            b.presentation_offset(joined.timescale)
        );
        let (pa, pb) = (
            packets(&source, &src, a.index),
            packets(&out, &joined, b.index),
        );
        assert_eq!(pa.len(), pb.len());
        for (x, y) in pa.iter().zip(&pb) {
            assert_eq!(
                (x.size, x.duration, x.dts, x.cts, x.keyframe),
                (y.size, y.duration, y.dts, y.cts, y.keyframe)
            );
            assert_eq!(x.pts, y.pts);
            assert_eq!(bytes(&source, x), bytes(&out, y));
        }
    }
    assert_eq!(summary.duration, q(4, 1));
}

#[test]
fn segments_of_a_source_with_several_edits_join_back() {
    // The first second of the media, then the third and the fourth: the
    // splice falls on a cut, so each segment carries one window and the
    // join puts the two windows back (docs/design/edit-list-writers.md,
    // split decision 5).
    let mut video = ibbp_video(100, 25);
    video.edits = vec![(1000, 1024), (2000, 1024 + 25_600)];
    let source = movie(&[video]);
    let segments = split(&source, q(1, 1));
    assert_eq!(segments.len(), 3);
    let inputs: Vec<&[u8]> = segments.iter().map(Vec::as_slice).collect();
    let (summary, out) = join_with(&inputs, JoinAudio::None).unwrap();
    assert_join(&inputs, &out, &summary);
    assert_eq!(summary.duration, q(3, 1));
    // One edit per window of the source, and no decode duration extended.
    assert_eq!(summary.video_edits, 2);
    assert!(summary.inputs.iter().all(|i| i.extended == 0));
    // Every sample is back, byte for byte, with its times and its place on
    // the presentation timeline.
    let (src, joined) = (probe_all(&source), probe_all(&out));
    let (pa, pb) = (packets(&source, &src, 0), packets(&out, &joined, 0));
    assert_eq!(pa.len(), pb.len());
    for (x, y) in pa.iter().zip(&pb) {
        assert_eq!(
            (x.size, x.duration, x.dts, x.cts, x.keyframe, x.presented),
            (y.size, y.duration, y.dts, y.cts, y.keyframe, y.presented),
            "sample {}",
            x.sample
        );
        assert_eq!(x.pts, y.pts, "sample {}", x.sample);
        assert_eq!(bytes(&source, x), bytes(&out, y));
    }
}

#[test]
fn clips_keep_their_edits_at_the_joins() {
    let source = av_movie();
    // Clips that end inside a group of pictures, with audio pre-roll.
    let a = clip(&source, q(0, 1), Some(q(13, 10))).unwrap();
    let b = clip(&source, q(21, 10), Some(q(33, 10))).unwrap();
    let inputs: Vec<&[u8]> = vec![&a, &b];
    let (summary, out) = join(&inputs).unwrap();
    assert_join(&inputs, &out, &summary);
    assert_eq!(summary.inputs[1].offset, q(13, 10));
    // 1.3 s each: the second clip starts at the sync sample at 2 s.
    assert_eq!(summary.duration, q(13, 5));
    // Frames after the first clip's end are in its run: its last decode
    // duration is extended so they stay before the second clip's media.
    assert!(summary.inputs[0].extended > 0);
    assert_eq!(summary.video_edits, 2);
    assert_eq!(summary.audio_edits, 2);
    // CoreAV's own reader reports a list of more than one edit as such.
    assert_eq!(probe_all(&out).tracks[0].edit_list, EditList::Multiple);
}

#[test]
fn one_input_is_copied() {
    let source = av_movie();
    let (summary, out) = join(&[&source]).unwrap();
    assert_join(&[&source], &out, &summary);
    let (src, joined) = (probe_all(&source), probe_all(&out));
    for (a, b) in src.tracks.iter().zip(&joined.tracks) {
        assert_eq!(
            a.media_edit_duration(src.timescale),
            b.media_edit_duration(joined.timescale)
        );
    }
}

#[test]
fn a_later_empty_edit_stays_a_gap() {
    // The second input's video and audio start 0.2 s into its movie.
    let mut video = ibbp_video(50, 25);
    video.edits = vec![(200, -1), (2000, 1024)];
    let late = movie(&[video, aac_audio(100, vec![(200, -1), (2000, 2112)])]);
    let source = av_movie();
    let inputs: Vec<&[u8]> = vec![&source, &late];
    let (summary, out) = join(&inputs).unwrap();
    assert_join(&inputs, &out, &summary);
    // Its earliest presented sample starts where the first input's video
    // ends.
    assert_eq!(summary.inputs[1].video_start, q(4, 1));
    assert_eq!(summary.inputs[1].offset, q(19, 5));
}

#[test]
fn video_only() {
    let source = av_movie();
    let video_only = movie(&[ibbp_video(50, 25)]);
    let inputs: Vec<&[u8]> = vec![&source, &video_only];
    assert!(matches!(
        join(&inputs),
        Err(JoinError::Mismatch {
            input: 1,
            mismatch: Mismatch::AudioTrack { present: false }
        })
    ));
    let (summary, out) = join_with(&inputs, JoinAudio::None).unwrap();
    assert!(summary.inputs.iter().all(|i| i.audio.is_none()));
    assert_eq!(probe_all(&out).tracks.len(), 1);
    assert_join(&inputs, &out, &summary);
}

#[test]
fn segments_of_frames_that_overlap_by_a_tick_join_back() {
    // P frames presented one tick late: each overlaps the next frame by a
    // tick, as reordered frames of variable frame rate sources do, and the
    // last frame shown before each cut (a P frame) ends a tick after the
    // segment's edit.
    let mut video = ibbp_video(100, 25);
    for sample in &mut video.samples {
        if sample.offset == 2048 {
            sample.offset += 1;
        }
    }
    let source = movie(&[video]);
    let segments = split(&source, q(1, 1));
    assert!(segments.len() > 1);
    let inputs: Vec<&[u8]> = segments.iter().map(Vec::as_slice).collect();
    let (summary, out) = join_with(&inputs, JoinAudio::None).unwrap();
    assert_join(&inputs, &out, &summary);
    assert_eq!(summary.video_edits, 1, "{summary:?}");
    assert!(
        summary.inputs.iter().all(|i| i.extended == 0),
        "{summary:?}"
    );
}

#[test]
fn an_edit_within_the_declared_media_ends_the_input() {
    // A second of video (media 1,024 to 13,824 composed) whose edit, from
    // media time 1,024, lasts 1.2 s: shown until the edit ends when the
    // media's declared duration covers it, and until the last frame ends
    // when not, as the stream duration has it.
    let mut video = ibbp_video(25, 25);
    video.edits = vec![(1200, 1024)];
    let short = movie(&[video]);
    // The media header's timescale (12,800) and duration (12,800), the
    // duration patched to 16,384: the edit's end.
    let declared = patch(
        &short,
        &[0, 0, 0x32, 0, 0, 0, 0x32, 0],
        &[0, 0, 0x32, 0, 0, 0, 0x40, 0],
    );
    for (source, end) in [(&declared, q(6, 5)), (&short, q(1, 1))] {
        let inputs: Vec<&[u8]> = vec![source, source];
        let (summary, out) = join_with(&inputs, JoinAudio::None).unwrap();
        assert_join(&inputs, &out, &summary);
        assert_eq!(summary.inputs[0].video_end, end);
        assert_eq!(summary.inputs[1].offset, end);
    }
}

#[test]
fn the_work_limit_is_checked_before_writing() {
    // Each listing visits an input's samples once: a limit that the first
    // two listings fit in but not the copy is refused before any byte.
    let source = av_movie();
    let inputs: Vec<&[u8]> = vec![&source, &source];
    let (_, out) = join(&inputs).unwrap();
    assert!(!out.is_empty());
    for work in [100, 400, 686, 875] {
        let limits = Limits {
            max_sample_work: work,
            ..Limits::default()
        };
        let mut out = Vec::new();
        let result = join_mp4(
            inputs.iter().map(|i| SliceSource::new(i)).collect(),
            &limits,
            &Allowlist::default(),
            &JoinOptions {
                audio: JoinAudio::First,
            },
            &mut out,
        );
        assert!(
            matches!(
                result,
                Err(JoinError::Input {
                    error: ExtractError::Source(Error::LimitExceeded { .. }),
                    ..
                })
            ),
            "{work}: {result:?}"
        );
        assert!(out.is_empty(), "{work}: {} bytes written", out.len());
    }
}

/// `av_movie`'s 100 frames and audio, its video with `edits`, and its
/// audio presented for as long as the video.
fn spliced(edits: Vec<(u32, i32)>) -> Vec<u8> {
    let shown = edits.iter().filter(|e| e.1 >= 0).map(|e| e.0).sum();
    let mut video = ibbp_video(100, 25);
    video.edits = edits;
    movie(&[video, aac_audio(190, vec![(shown, 2112)])])
}

#[test]
fn inputs_with_several_edits_keep_them() {
    // Seconds 0 to 1 of the media, then 2 to 4: the second GOP is cut out.
    let several = spliced(vec![(1000, 1024), (2000, 1024 + 25_600)]);
    let single = av_movie();
    for inputs in [
        vec![&several[..], &single[..]],
        vec![&single[..], &several[..]],
        vec![&several[..], &several[..]],
    ] {
        let (summary, out) = join(&inputs).unwrap();
        assert_join(&inputs, &out, &summary);
        // Each input presents its three or four seconds.
        let expected = inputs
            .iter()
            .map(|i| if *i == &several[..] { 3 } else { 4 })
            .sum::<i128>();
        assert_eq!(summary.duration, q(expected, 1));
        // The two windows of each spliced input and the single input's one
        // edit, where each input's media and presentation follow the
        // previous one's, merged.
        let windows = inputs.iter().filter(|i| **i == &several[..]).count();
        assert_eq!(
            summary.video_edits,
            2 * windows + (inputs.len() - windows) - 1
        );
    }
}

#[test]
fn an_empty_edit_inside_an_input_is_kept() {
    // Half a second of nothing between the two windows.
    let gap = spliced(vec![(1000, 1024), (500, -1), (2000, 1024 + 25_600)]);
    let inputs: Vec<&[u8]> = vec![&gap, &gap];
    let (summary, out) = join(&inputs).unwrap();
    assert_join(&inputs, &out, &summary);
    assert_eq!(summary.duration, q(7, 1));
    let empty: Vec<_> = edit_lists(&out)[0]
        .2
        .iter()
        .filter(|e| e.1 == -1)
        .copied()
        .collect();
    assert_eq!(empty.len(), 2, "{empty:?}");
}

#[test]
fn audio_with_several_edits_keeps_its_windows() {
    // Audio: its first second after priming, then a skip of one second.
    let skipping = movie(&[
        ibbp_video(100, 25),
        aac_audio(190, vec![(1000, 2112), (2000, 2112 + 96_000)]),
    ]);
    let single = av_movie();
    for inputs in [
        vec![&skipping[..], &single[..]],
        vec![&single[..], &skipping[..]],
    ] {
        let (summary, out) = join(&inputs).unwrap();
        assert_join(&inputs, &out, &summary);
        assert!(summary.audio_edits >= 3, "{}", summary.audio_edits);
    }
}

#[test]
fn segments_across_an_empty_edit_join_back() {
    // The cut at 1 s falls where the gap starts: the first segment ends
    // with frames no edit presents, the second starts with the gap.
    let source = spliced(vec![(1000, 1024), (500, -1), (2000, 1024 + 25_600)]);
    let segments = split(&source, q(1, 1));
    let inputs: Vec<&[u8]> = segments.iter().map(Vec::as_slice).collect();
    let (summary, out) = join(&inputs).unwrap();
    assert_join(&inputs, &out, &summary);
    // The gap stays an empty edit rather than the first segment's hidden
    // frames; the second segment starts at its audio, which starts after
    // the cut by less than an AAC frame (docs/design/join.md, Offsets).
    let edits = &edit_lists(&out)[0].2;
    assert_eq!(edits.iter().filter(|e| e.1 == -1).count(), 1, "{edits:?}");
    let lead = sum(q(7, 2), q(-summary.duration.num, summary.duration.den));
    assert!(!lead.checked_lt(q(0, 1)).unwrap() && lead.checked_lt(q(1024, 48_000)).unwrap());
    let (src, joined) = (probe_all(&source), probe_all(&out));
    let (pa, pb) = (packets(&source, &src, 0), packets(&out, &joined, 0));
    assert_eq!(pa.len(), pb.len());
    for (x, y) in pa.iter().zip(&pb) {
        assert_eq!(
            (x.size, x.dts, x.cts, x.keyframe, x.presented),
            (y.size, y.dts, y.cts, y.keyframe, y.presented),
            "sample {}",
            x.sample
        );
        if !x.presented {
            continue;
        }
        let pts = x.pts.unwrap();
        let expected = if pts.checked_lt(q(12_800, 1)).unwrap() {
            pts
        } else {
            sum(pts, q(-lead.num * 12_800, lead.den))
        };
        assert_eq!(y.pts, Some(expected), "sample {}", x.sample);
    }
}

#[test]
fn unsigned_composition_offsets_are_not_moved_past() {
    // Offsets that are negative for B frames, stored in a version 0 table:
    // ISO/IEC 14496-12 reads them as unsigned, composing those frames some
    // 2^32 ticks after their decode, past every edit (KD-C-CTTS-UNSIGNED).
    let mut video = ibbp_video(50, 25);
    for sample in &mut video.samples {
        sample.offset -= 1024;
    }
    video.edits = vec![(2000, 0)];
    let source = patch(&movie(&[video]), b"ctts\x01", b"ctts\x00");
    let parsed = probe_all(&source);
    assert!(
        packets(&source, &parsed, 0)
            .iter()
            .any(|p| p.cts - p.dts >= 1 << 31)
    );
    let inputs: Vec<&[u8]> = vec![&source, &source];
    let (summary, out) = join_with(&inputs, JoinAudio::None).unwrap();
    assert_join(&inputs, &out, &summary);
    // Without leaving those frames out, the first input's last decode
    // duration would grow by some 2^32 ticks.
    assert!(
        summary.inputs.iter().all(|i| i.extended == 0),
        "{summary:?}"
    );
    assert_eq!(summary.video_edits, 1);
}

/// `data` with the first `from` replaced by `to`.
fn patch(data: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let at = data
        .windows(from.len())
        .position(|w| w == from)
        .expect("pattern");
    let mut out = data.to_vec();
    out[at..at + to.len()].copy_from_slice(to);
    out
}

#[test]
fn incompatible_inputs_are_refused_with_the_difference() {
    let source = av_movie();
    let mismatch = |other: Vec<u8>| -> Mismatch {
        let inputs: Vec<&[u8]> = vec![&source, &source, &other];
        match join(&inputs) {
            Err(JoinError::Mismatch { input: 2, mismatch }) => mismatch,
            other => panic!("{:?}", other.map(|r| r.0)),
        }
    };
    // avcC with level 31 instead of 30.
    let config = patch(&source, b"avcC\x01\x64\x00\x1e", b"avcC\x01\x64\x00\x1f");
    assert_eq!(
        mismatch(config),
        Mismatch::Configuration {
            track: JoinTrack::Video,
            kind: FourCc(*b"avcC")
        }
    );
    // A coded width of 640 instead of 320.
    let size = patch(
        &source,
        &[0x01, 0x40, 0x00, 0xf0],
        &[0x02, 0x80, 0x00, 0xf0],
    );
    assert_eq!(
        mismatch(size),
        Mismatch::CodedSize {
            first: (320, 240),
            this: (640, 240)
        }
    );
    // Another video timescale.
    let mut video = ibbp_video(100, 25);
    video.timescale = 25_600;
    for s in &mut video.samples {
        s.duration *= 2;
        s.offset *= 2;
    }
    video.edits = vec![(4000, 2048)];
    let timescale = movie(&[video, aac_audio(190, vec![(4000, 2112)])]);
    assert_eq!(
        mismatch(timescale),
        Mismatch::Timescale {
            track: JoinTrack::Video,
            first: 12_800,
            this: 25_600
        }
    );
    // Audio at 44.1 kHz.
    let rate = movie(&[
        ibbp_video(100, 25),
        TrackSpec {
            timescale: 44_100,
            ..aac_audio(170, vec![])
        },
    ]);
    assert_eq!(
        mismatch(rate),
        Mismatch::SampleRate {
            first: 48_000,
            this: 44_100
        }
    );
    // A rotation.
    let rotated = patch(
        &source,
        &[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0],
        &[
            0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0, 0, 0, 0,
        ],
    );
    assert_eq!(mismatch(rotated), Mismatch::Matrix);
    // Messages name the input and the difference.
    let error = JoinError::Mismatch {
        input: 2,
        mismatch: Mismatch::Timescale {
            track: JoinTrack::Video,
            first: 12_800,
            this: 25_600,
        },
    };
    assert_eq!(
        error.to_string(),
        "input 2 cannot be joined to the first: its video timescale is 25600, the first input's is 12800"
    );
}

#[test]
fn inputs_that_cannot_be_joined_on_their_own() {
    let source = av_movie();
    // A second input whose video does not start with a sync sample.
    let mut video = ibbp_video(50, 25);
    video.samples[0].sync = false;
    let unsynced = movie(&[video, aac_audio(100, vec![(2000, 2112)])]);
    match join(&[&source, &unsynced]) {
        Err(JoinError::Input {
            input: 1,
            error: ExtractError::Source(Error::Unsupported { reason }),
        }) => assert!(reason.contains("sync sample"), "{reason}"),
        other => panic!("{:?}", other.map(|r| r.0)),
    }
    // As the first input it is joined: nothing comes before it.
    assert!(join(&[&unsynced, &source]).is_ok());
    // A second input that starts at an open-GOP sync sample: frames
    // decoded after it are presented before it.
    let s = |offset, sync| S {
        size: 30,
        duration: 512,
        offset,
        sync,
    };
    let open = movie(&[TrackSpec {
        video: true,
        timescale: 12_800,
        edits: vec![(200, 1536)],
        samples: vec![
            s(1536, true),
            s(512, false),
            s(512, false),
            s(2048, false),
            s(512, false),
        ],
        entry: *b"avc1",
    }]);
    let video_only = movie(&[ibbp_video(50, 25)]);
    match join_with(&[&video_only, &open], JoinAudio::None) {
        Err(JoinError::Input {
            input: 1,
            error: ExtractError::Source(Error::Unsupported { reason }),
        }) => assert!(reason.contains("open-GOP"), "{reason}"),
        other => panic!("{:?}", other.map(|r| r.0)),
    }
    assert!(join_with(&[&open, &video_only], JoinAudio::None).is_ok());
    // Not a movie.
    assert!(matches!(
        join(&[&source, b"not a movie"]),
        Err(JoinError::Input { input: 1, .. })
    ));
    assert!(matches!(
        join_mp4(
            Vec::<SliceSource<'_>>::new(),
            &Limits::default(),
            &Allowlist::default(),
            &JoinOptions {
                audio: JoinAudio::First
            },
            Vec::new()
        ),
        Err(JoinError::NoInputs)
    ));
    let _ = Codec::H264;
    let _ = S {
        size: 0,
        duration: 0,
        offset: 0,
        sync: true,
    };
}

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

/// Writes the inputs and output of a join for `scripts/join_check.py`.
fn record(
    out_dir: &std::path::Path,
    name: &str,
    source: Option<(&std::path::Path, Rational)>,
    inputs: &[&[u8]],
    output: &[u8],
    summary: &JoinSummary,
    manifest: &mut Vec<String>,
) {
    // Picture sizes that change inside the track (docs/design/
    // frame-sizes.md): the decoder reinitializes mid-stream, which the
    // oracles' size and frame comparisons do not model, as for the clip and
    // split fixtures. The round trip still covers them.
    if ["size_inband_sps.mp4", "size_vp9_resize.mp4"]
        .iter()
        .any(|fixture| name.contains(fixture))
    {
        return;
    }
    let mut entries = Vec::new();
    for (n, (input, joined)) in inputs.iter().zip(&summary.inputs).enumerate() {
        let path = out_dir.join(format!("{name}.in{n}.mp4"));
        std::fs::write(&path, input).unwrap();
        entries.push(format!(
            "{{\"path\": {}, \"offset\": {}, \"video_start\": {}, \"video_end\": {}, \"extended\": {}, \"video_stream\": {}, \"audio_stream\": {}}}",
            json_string(&path.display().to_string()),
            json_rational(joined.offset),
            json_rational(joined.video_start),
            json_rational(joined.video_end),
            joined.extended,
            joined.video.stream_index,
            joined.audio.map_or("null".to_string(), |a| a.stream_index.to_string()),
        ));
    }
    let path = out_dir.join(format!("{name}.joined.mp4"));
    std::fs::write(&path, output).unwrap();
    manifest.push(format!(
        "{{\"label\": {}, \"source\": {}, \"source_offset\": {}, \"output\": {}, \"duration\": {}, \"video_edits\": {}, \"audio_edits\": {}, \"inputs\": [{}]}}",
        json_string(name),
        source.map_or("null".to_string(), |s| json_string(&s.0.display().to_string())),
        source.map_or("null".to_string(), |s| json_rational(s.1)),
        json_string(&path.display().to_string()),
        json_rational(summary.duration),
        summary.video_edits,
        summary.audio_edits,
        entries.join(", ")
    ));
}

/// Every MP4 and MOV fixture with a video track the writer accepts: its
/// segments every second joined back, two of its clips joined (one that
/// ends inside a group of pictures, then one that starts after a gap), and
/// every pair of such fixtures joined, which must either succeed or fail
/// with a mismatch. With `COREAV_JOIN_OUT` the inputs, outputs and a
/// manifest are written for `scripts/join_check.py`.
#[test]
#[ignore = "needs COREAV_FIXTURES"]
fn fixtures_round_trip() {
    use std::path::Path;
    let dir = std::env::var("COREAV_FIXTURES").expect("COREAV_FIXTURES");
    let out_dir = std::env::var("COREAV_JOIN_OUT").ok();
    if let Some(out_dir) = &out_dir {
        std::fs::create_dir_all(out_dir).unwrap();
    }
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("mp4" | "mov")))
        .collect();
    entries.sort();
    let mut manifest = Vec::new();
    let mut usable = Vec::new();
    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read(path).unwrap();
        // Only sources the clip writer accepts.
        let mut probe_out = Vec::new();
        if clip_mp4(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
            &ClipOptions {
                video: VideoChoice::First,
                audio: AudioChoice::First,
                start: q(0, 1),
                end: None,
            },
            &mut probe_out,
        )
        .is_err()
        {
            continue;
        }
        let (start, segments) = match try_split_from(&source, q(1, 1)) {
            Ok(split) => split,
            Err(ExtractError::Source(Error::Unsupported { reason })) => {
                eprintln!("{name}: refused: {reason}");
                continue;
            }
            Err(e) => panic!("{name} split: {e}"),
        };
        let inputs: Vec<&[u8]> = segments.iter().map(Vec::as_slice).collect();
        let (summary, out) = join(&inputs).unwrap_or_else(|e| panic!("{name} segments: {e}"));
        assert_join(&inputs, &out, &summary);
        eprintln!(
            "{name} segments: {} inputs, {} video and {} audio edits",
            inputs.len(),
            summary.video_edits,
            summary.audio_edits
        );
        if let Some(out_dir) = &out_dir {
            record(
                Path::new(out_dir),
                &format!("{name}.segments"),
                Some((path, start)),
                &inputs,
                &out,
                &summary,
                &mut manifest,
            );
        }
        let clips = clip(&source, q(0, 1), Some(q(7, 10)))
            .and_then(|a| Ok((a, clip(&source, q(11, 10), None)?)));
        let (a, b) = match clips {
            Ok(clips) => clips,
            // A fixture shorter than the second clip's start, say.
            Err(e) => {
                eprintln!("{name} clips: {e}");
                usable.push((name, source));
                continue;
            }
        };
        let inputs: Vec<&[u8]> = vec![&a, &b];
        match join(&inputs) {
            Ok((summary, out)) => {
                assert_join(&inputs, &out, &summary);
                eprintln!(
                    "{name} clips: {} video and {} audio edits, extended {:?}",
                    summary.video_edits,
                    summary.audio_edits,
                    summary
                        .inputs
                        .iter()
                        .map(|i| i.extended)
                        .collect::<Vec<_>>()
                );
                if let Some(out_dir) = &out_dir {
                    record(
                        Path::new(out_dir),
                        &format!("{name}.clips"),
                        None,
                        &inputs,
                        &out,
                        &summary,
                        &mut manifest,
                    );
                }
            }
            // A clip whose video does not start with a sync sample cannot
            // follow another.
            Err(e) => eprintln!("{name} clips: {e}"),
        }
        usable.push((name, source));
    }
    assert!(usable.len() >= 10, "{} fixtures", usable.len());
    let mut joined_pairs = 0;
    for (an, a) in &usable {
        for (bn, b) in &usable {
            if an >= bn {
                continue;
            }
            match join(&[a, b]) {
                Ok((summary, out)) => {
                    assert_join(&[a, b], &out, &summary);
                    joined_pairs += 1;
                    if let Some(out_dir) = &out_dir {
                        record(
                            Path::new(out_dir),
                            &format!("{an}+{bn}"),
                            None,
                            &[a, b],
                            &out,
                            &summary,
                            &mut manifest,
                        );
                    }
                }
                Err(JoinError::Mismatch { input: 1, mismatch }) => {
                    eprintln!("{an} + {bn}: {mismatch}");
                }
                Err(e) => panic!("{an} + {bn}: {e}"),
            }
        }
    }
    eprintln!("{joined_pairs} pairs of fixtures joined");
    if let Some(out_dir) = &out_dir {
        std::fs::write(
            Path::new(out_dir).join("manifest.json"),
            format!("[\n{}\n]\n", manifest.join(",\n")),
        )
        .unwrap();
    }
}
