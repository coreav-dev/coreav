//! The replaced-stream remux (`docs/design/remux.md`, "One video stream
//! replaced"): a source remuxed with one video stream's samples replaced
//! must present the replacement's frames where the plain remux presents
//! the source's frames of the same times, and write every other stream
//! exactly as the plain remux does. The library round trips run on
//! synthetic movies and Matroska files; the `ffprobe` checks (black box)
//! on the fixtures need `COREAV_FIXTURES` and `ffprobe` and `ffmpeg` on
//! `PATH`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;
#[path = "../../coreav-mkv/tests/common/mod.rs"]
mod mkv;
mod movies;

use coreav_isobmff::{
    Allowlist, Budget, Codec, EditList, Error, LimitKind, Limits, Movie, Packet, Rational,
    SliceSource, Source, TrackKind, probe,
};
use coreav_isobmff_write::mux::{Nclx, RawBox, audio_sample_entry, avc1_sample_entry, esds};
use coreav_isobmff_write::{
    AudioReplacement, AudioSample, BoxDropReason, ExtractError, MatroskaRemuxError,
    MatroskaRemuxOptions, MatroskaRemuxSummary, RemuxOptions, RemuxSummary, Replacement,
    ReplacementSample, Replacements, remux_matroska, remux_matroska_replacing, remux_mp4,
    remux_mp4_replacing,
};
use movies::{S, TrackSpec, aac_audio, av_movie, ibbp_video, movie};
use std::path::Path;
use std::process::Command;

// ------------------------------------------------------------- helpers

fn probe_out(data: &[u8]) -> Movie {
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

fn bytes(data: &[u8], offset: u64, size: u32) -> Vec<u8> {
    let mut buf = vec![0; size as usize];
    SliceSource::new(data)
        .read_exact_at(offset, &mut buf)
        .unwrap();
    buf
}

/// A replacement made of a source track's own samples: what a transcoder
/// that changes nothing would give.
struct Own {
    stream_index: usize,
    entry: Vec<u8>,
    width: u32,
    height: u32,
    samples: Vec<ReplacementSample>,
    last_duration: u32,
    data: Vec<u8>,
}

impl Own {
    fn replacement(&self) -> Replacements<'_, SliceSource<'_>> {
        Replacements {
            video: Some(Replacement {
                stream_index: self.stream_index,
                sample_entry: &self.entry,
                width: self.width,
                height: self.height,
                samples: &self.samples,
                last_duration: self.last_duration,
                data: SliceSource::new(&self.data),
            }),
            audio: None,
        }
    }

    /// Decode times moved back by `ticks`, presentation times kept.
    fn shifted(mut self, ticks: i64) -> Self {
        for s in &mut self.samples {
            s.decode_time -= ticks;
        }
        self
    }
}

/// The samples of the MP4 source's stream `stream_index`, with the source's
/// decode times and composition times; decode times move back as far as
/// needed for no sample to be presented before it is decoded (a version 1
/// `ctts` with negative offsets).
fn own_samples(source: &[u8], stream_index: usize) -> Own {
    let movie = probe_out(source);
    let track = movie
        .tracks
        .iter()
        .find(|t| t.stream_index == stream_index)
        .unwrap();
    let list = packets(source, &movie, track.index);
    let lead = list.iter().map(|p| p.dts - p.cts).max().unwrap().max(0);
    let mut data = Vec::new();
    let mut samples = Vec::new();
    for p in &list {
        data.extend(bytes(source, p.offset, p.size));
        samples.push(ReplacementSample {
            size: p.size,
            sync: p.keyframe,
            decode_time: p.dts - lead,
            presentation_time: p.cts,
        });
    }
    let d = &track.description;
    let entry = avc1_sample_entry(
        d.width.unwrap(),
        d.height.unwrap(),
        &d.codec_config.as_ref().unwrap().data,
        d.pixel_aspect.filter(|a| a.0 != a.1),
        None,
    )
    .unwrap();
    Own {
        stream_index,
        entry,
        width: track.presentation_size.0,
        height: track.presentation_size.1,
        samples,
        last_duration: list.last().unwrap().duration,
        data,
    }
}

fn plain(source: &[u8]) -> (RemuxSummary, Vec<u8>) {
    let mut out = Vec::new();
    let summary = remux_mp4(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
        &RemuxOptions::default(),
        &mut out,
    )
    .unwrap();
    (summary, out)
}

fn replaced(source: &[u8], own: &Own) -> Result<(RemuxSummary, Vec<u8>), ExtractError> {
    let mut out = Vec::new();
    let summary = remux_mp4_replacing(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
        &RemuxOptions::default(),
        own.replacement(),
        &mut out,
    )?;
    assert_eq!(summary.bytes_written, out.len() as u64);
    Ok((summary, out))
}

/// The presentation of a track: each sample's presentation time after the
/// edit list and whether it is presented, in decode order.
fn presentation(data: &[u8], movie: &Movie, index: usize) -> Vec<(Option<Rational>, bool)> {
    packets(data, movie, index)
        .iter()
        .map(|p| (p.pts, p.presented))
        .collect()
}

/// Everything a reader sees of a copied track's samples.
fn copied(data: &[u8], movie: &Movie, index: usize) -> Vec<(i64, i64, u32, bool, Vec<u8>)> {
    packets(data, movie, index)
        .iter()
        .map(|p| {
            (
                p.dts,
                p.cts,
                p.duration,
                p.keyframe,
                bytes(data, p.offset, p.size),
            )
        })
        .collect()
}

/// The output of the replacement presents the same frames at the same
/// times as the plain remux, with the replacement's bytes, and the other
/// tracks unchanged.
fn assert_same_presentation(source: &[u8], own: &Own) {
    let (plain_summary, plain_out) = plain(source);
    let (summary, out) = replaced(source, own).unwrap();
    let (a, b) = (probe_out(&plain_out), probe_out(&out));
    assert_eq!(a.tracks.len(), b.tracks.len());
    assert_eq!(summary.tracks.len(), plain_summary.tracks.len());
    for (index, (ta, tb)) in a.tracks.iter().zip(&b.tracks).enumerate() {
        let kept = &summary.tracks[index];
        assert_eq!(kept.stream_index, plain_summary.tracks[index].stream_index);
        if kept.stream_index == own.stream_index {
            assert!(kept.replaced);
            assert_eq!(kept.codec, Codec::H264);
            assert_eq!(kept.samples as usize, own.samples.len());
            assert_eq!(
                presentation(&plain_out, &a, index),
                presentation(&out, &b, index)
            );
            let written = packets(&out, &b, index);
            let mut at = 0;
            for (p, s) in written.iter().zip(&own.samples) {
                assert_eq!(p.size, s.size);
                assert_eq!(p.keyframe, s.sync);
                assert_eq!(
                    bytes(&out, p.offset, p.size),
                    own.data[at..at + s.size as usize]
                );
                at += s.size as usize;
            }
            assert_eq!(tb.matrix, ta.matrix);
            assert_eq!(tb.language, ta.language);
            assert_eq!(tb.presentation_size, (own.width, own.height));
        } else {
            assert!(!kept.replaced);
            assert_eq!(kept, &plain_summary.tracks[index]);
            assert_eq!(copied(&plain_out, &a, index), copied(&out, &b, index));
            assert_eq!(
                presentation(&plain_out, &a, index),
                presentation(&out, &b, index)
            );
        }
    }
}

// ------------------------------------------------------------- MP4

#[test]
fn own_samples_present_as_the_plain_remux() {
    let source = av_movie();
    assert_same_presentation(&source, &own_samples(&source, 0));
}

#[test]
fn decode_times_shifted_back_by_two_frames_present_the_same() {
    let source = av_movie();
    let own = own_samples(&source, 0).shifted(1024);
    let (summary, out) = replaced(&source, &own).unwrap();
    assert!(summary.tracks[0].replaced);
    // The edit's media time moves by the shift: 1,024 + 1,024.
    let movie = probe_out(&out);
    let EditList::Simple { media_time, .. } = movie.tracks[0].edit_list else {
        panic!("{:?}", movie.tracks[0].edit_list);
    };
    assert_eq!(media_time, 2048);
    assert_same_presentation(&source, &own);
}

#[test]
fn decode_times_starting_anywhere_present_the_same() {
    let source = av_movie();
    for ticks in [3, 7, 5_000_000, 1_000_000_000] {
        let own = own_samples(&source, 0).shifted(ticks);
        assert_same_presentation(&source, &own);
    }
}

#[test]
fn a_replacement_that_starts_later_keeps_the_frames_times() {
    // The first three frames in presentation order are left out (frames
    // the source does not present, for example): the empty edit grows to
    // the replacement's first decode time.
    let source = av_movie();
    let mut own = own_samples(&source, 0);
    let (plain_summary, plain_out) = plain(&source);
    // Keep the samples from the second GOP on: the first decode time is
    // after the edit's media time.
    let gop = 25;
    let skipped: usize = own.samples[..gop].iter().map(|s| s.size as usize).sum();
    own.data.drain(..skipped);
    own.samples.drain(..gop);
    let (summary, out) = replaced(&source, &own).unwrap();
    assert_eq!(summary.tracks[1], plain_summary.tracks[1]);
    let (a, b) = (probe_out(&plain_out), probe_out(&out));
    let EditList::Simple {
        empty, media_time, ..
    } = b.tracks[0].edit_list
    else {
        panic!("{:?}", b.tracks[0].edit_list);
    };
    // The edit starts at the first kept frame, presented at 1 s: 25
    // frames of 40 ms, after the two-frame delay that the source's edit
    // skips.
    assert_eq!((empty, media_time), (1000, 1024));
    let source_times = presentation(&plain_out, &a, 0);
    assert_eq!(presentation(&out, &b, 0), source_times[gop..]);
    assert_eq!(copied(&plain_out, &a, 1), copied(&out, &b, 1));
}

#[test]
fn without_an_edit_list_media_time_stays_movie_time() {
    // Video without an edit list, no reordering: presented from 0.
    let video = TrackSpec {
        edits: Vec::new(),
        samples: (0..50)
            .map(|n| S {
                size: 30 + n % 7,
                duration: 512,
                offset: 0,
                sync: n % 25 == 0,
            })
            .collect(),
        ..ibbp_video(50, 25)
    };
    let source = movie(&[video, aac_audio(90, vec![(2000, 2112)])]);
    // Decode times earlier than the source's: a media edit skips them.
    let own = own_samples(&source, 0).shifted(1024);
    assert_same_presentation(&source, &own);
    // Later ones: without the first frame, and decoded 412 ticks before
    // presentation. The media edit starts at the first frame, after an
    // empty edit of 40 ms.
    let mut own = own_samples(&source, 0);
    let first = own.samples[0].size as usize;
    own.data.drain(..first);
    own.samples.remove(0);
    for s in &mut own.samples {
        s.decode_time = s.presentation_time - 412;
    }
    let (summary, out) = replaced(&source, &own).unwrap();
    let movie_out = probe_out(&out);
    assert_eq!(movie_out.timescale, 1000);
    let EditList::Simple {
        empty, media_time, ..
    } = movie_out.tracks[0].edit_list
    else {
        panic!("{:?}", movie_out.tracks[0].edit_list);
    };
    assert_eq!((empty, media_time), (40, 412));
    let (plain_summary, plain_out) = plain(&source);
    let a = probe_out(&plain_out);
    assert_eq!(
        presentation(&out, &movie_out, 0),
        presentation(&plain_out, &a, 0)[1..]
    );
    assert_eq!(summary.tracks[1], plain_summary.tracks[1]);
    assert_eq!(copied(&plain_out, &a, 1), copied(&out, &movie_out, 1));
    // Frames presented from 100 ticks of 12,800 on: the empty edit rounds
    // in the movie timescale of 1,000, so the movie timescale becomes
    // 64,000, and the audio's edit 128,000 ticks.
    let late = offset_movie(Vec::new());
    let own = own_samples(&late, 0).shifted(1024);
    let (_, out) = replaced(&late, &own).unwrap();
    let movie_out = probe_out(&out);
    assert_eq!(movie_out.timescale, 64_000);
    let EditList::Simple {
        empty, media_time, ..
    } = movie_out.tracks[0].edit_list
    else {
        panic!("{:?}", movie_out.tracks[0].edit_list);
    };
    assert_eq!((empty, media_time), (500, 1124));
    assert_eq!(movie_out.tracks[1].edits[0].segment_duration, 128_000);
    assert_same_presentation(&late, &own);
    // The same decode times: no edit list at all.
    let own = own_samples(&source, 0);
    let (_, out) = replaced(&source, &own).unwrap();
    assert_eq!(probe_out(&out).tracks[0].edit_list, EditList::None);
}

#[test]
fn per_sample_boxes_of_the_replaced_stream_are_not_copied() {
    // An `sdtp` in the video's sample table: it describes the old samples.
    let source = av_movie();
    let stbl = source.windows(4).position(|w| w == b"stbl").unwrap() - 4;
    let size = u32::from_be_bytes(source[stbl..stbl + 4].try_into().unwrap()) as usize;
    let sdtp = common::full(b"sdtp", 0, 0, &[0x10; 100]);
    // Every enclosing box grows; the chunk offsets move with the data.
    let with = movies_with_box_inserted(&source, stbl, size, &sdtp);
    let (plain_summary, plain_out) = plain(&with);
    assert!(plain_out.windows(4).any(|w| w == b"sdtp"));
    assert!(plain_summary.dropped_boxes.is_empty());
    let own = own_samples(&with, 0);
    let (summary, out) = replaced(&with, &own).unwrap();
    assert!(!out.windows(4).any(|w| w == b"sdtp"));
    let gone = &summary.dropped_boxes;
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].stream_index, Some(0));
    assert_eq!(gone[0].reason, BoxDropReason::NotCarried);
    assert_eq!(gone[0].path.last().unwrap().0, *b"sdtp");
}

/// `source` with `inserted` appended to the box of `size` bytes at `at`
/// (the video's `stbl`): the enclosing boxes grow and every chunk offset
/// moves by its length.
fn movies_with_box_inserted(source: &[u8], at: usize, size: usize, inserted: &[u8]) -> Vec<u8> {
    let grow = inserted.len() as u32;
    let mut out = source[..at + size].to_vec();
    out.extend_from_slice(inserted);
    out.extend_from_slice(&source[at + size..]);
    // Enclosing boxes: moov, trak, mdia, minf, stbl of the first track.
    let mut parents = Vec::new();
    for kind in [b"moov", b"trak", b"mdia", b"minf", b"stbl"] {
        let pos = out.windows(4).position(|w| w == kind).unwrap() - 4;
        parents.push(pos);
    }
    for pos in parents {
        let size = u32::from_be_bytes(out[pos..pos + 4].try_into().unwrap()) + grow;
        out[pos..pos + 4].copy_from_slice(&size.to_be_bytes());
    }
    let mut from = 0;
    while let Some(pos) = out[from..].windows(4).position(|w| w == b"stco") {
        let stco = from + pos + 4;
        let count = u32::from_be_bytes(out[stco + 4..stco + 8].try_into().unwrap()) as usize;
        for i in 0..count {
            let o = stco + 8 + 4 * i;
            let v = u32::from_be_bytes(out[o..o + 4].try_into().unwrap()) + grow;
            out[o..o + 4].copy_from_slice(&v.to_be_bytes());
        }
        from = stco;
    }
    out
}

#[test]
fn the_replaced_stream_keeps_its_place_rotation_and_language() {
    // Audio first, then video turned a quarter (translated by the 240-row
    // height) in Swedish: the replaced video stays second, keeps its
    // language, and its translation follows the new size.
    let mut source = movie(&[aac_audio(190, vec![(4000, 2112)]), ibbp_video(100, 25)]);
    let quarter = [0, 0x10000, 0, -0x10000, 0, 0, 240 << 16, 0, 0x4000_0000];
    set_matrix(&mut source, 1, quarter);
    set_language(&mut source, 1, b"swe");
    let source_movie = probe_out(&source);
    assert_eq!(source_movie.tracks[1].matrix, quarter);
    let own = Own {
        width: 640 << 16,
        height: 480 << 16,
        ..own_samples(&source, 1)
    };
    let (summary, out) = replaced(&source, &own).unwrap();
    assert_eq!(summary.tracks[0].kind, TrackKind::Audio);
    assert!(summary.tracks[1].replaced);
    let movie_out = probe_out(&out);
    let track = &movie_out.tracks[1];
    assert_eq!(track.presentation_size, (640 << 16, 480 << 16));
    assert_eq!(track.language, Some(*b"swe"));
    assert_eq!(track.description.fourcc.0, *b"avc1");
    assert_eq!(
        track.matrix,
        [0, 0x10000, 0, -0x10000, 0, 0, 480 << 16, 0, 0x4000_0000]
    );
    assert_eq!(track.rotation, source_movie.tracks[1].rotation);
    // The same size keeps the matrix as stored.
    let own = own_samples(&source, 1);
    let (_, out) = replaced(&source, &own).unwrap();
    assert_eq!(probe_out(&out).tracks[1].matrix, quarter);
    // A half turn: both axes move back.
    let half = [
        -0x10000,
        0,
        0,
        0,
        -0x10000,
        0,
        320 << 16,
        240 << 16,
        0x4000_0000,
    ];
    set_matrix(&mut source, 1, half);
    let own = Own {
        width: 160 << 16,
        height: 120 << 16,
        ..own_samples(&source, 1)
    };
    let (_, out) = replaced(&source, &own).unwrap();
    assert_eq!(
        probe_out(&out).tracks[1].matrix,
        [
            -0x10000,
            0,
            0,
            0,
            -0x10000,
            0,
            160 << 16,
            120 << 16,
            0x4000_0000
        ]
    );
    // A translation that does not place the old size: refused when the
    // size changes, kept when it does not.
    let odd = [0x10000, 0, 0, 0, 0x10000, 0, 5 << 16, 0, 0x4000_0000];
    set_matrix(&mut source, 1, odd);
    let own = Own {
        width: 640 << 16,
        height: 480 << 16,
        ..own_samples(&source, 1)
    };
    assert_eq!(
        unsupported(replaced(&source, &own)),
        "the replaced stream's matrix has a translation that does not follow a new size"
    );
    let own = own_samples(&source, 1);
    let (_, out) = replaced(&source, &own).unwrap();
    assert_eq!(probe_out(&out).tracks[1].matrix, odd);
}

/// Overwrites the matrix of the `nth` track header of `data`.
fn set_matrix(data: &mut [u8], nth: usize, matrix: [i32; 9]) {
    let at = nth_box(data, b"tkhd", nth) + 4 + 40;
    for (i, v) in matrix.iter().enumerate() {
        data[at + 4 * i..at + 4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
}

/// Overwrites the language of the `nth` media header of `data`.
fn set_language(data: &mut [u8], nth: usize, code: &[u8; 3]) {
    let at = nth_box(data, b"mdhd", nth) + 4 + 20;
    let packed = code
        .iter()
        .fold(0u16, |v, c| (v << 5) | u16::from(c - 0x60));
    data[at..at + 2].copy_from_slice(&packed.to_be_bytes());
}

/// Offset of the type field of the `nth` box of type `kind`.
fn nth_box(data: &[u8], kind: &[u8; 4], nth: usize) -> usize {
    data.windows(4)
        .enumerate()
        .filter(|(_, w)| *w == kind)
        .nth(nth)
        .unwrap()
        .0
}

/// Video presented 100 ticks of 12,800 after each decode time, with the
/// edit list `edits`, and AAC.
fn offset_movie(edits: Vec<(u32, i32)>) -> Vec<u8> {
    let video = TrackSpec {
        edits,
        samples: (0..50)
            .map(|n| S {
                size: 30 + n % 7,
                duration: 512,
                offset: 100,
                sync: n % 25 == 0,
            })
            .collect(),
        ..ibbp_video(50, 25)
    };
    movie(&[video, aac_audio(90, vec![(2000, 2112)])])
}

#[test]
fn the_source_edits_own_gap_is_kept() {
    // The edit starts at media time 0, the first frame at 100: an identity
    // replacement gives the source's edit list and movie timescale.
    let source = offset_movie(vec![(2000, 0)]);
    let own = own_samples(&source, 0);
    let (_, out) = replaced(&source, &own).unwrap();
    let movie_out = probe_out(&out);
    assert_eq!(movie_out.timescale, 1000);
    assert_eq!(
        movie_out.tracks[0].edit_list,
        probe_out(&plain(&source).1).tracks[0].edit_list
    );
    assert_same_presentation(&source, &own);
    // Decoded two frames earlier: the same presentation, the edit moved.
    let own = own_samples(&source, 0).shifted(1024);
    let (_, out) = replaced(&source, &own).unwrap();
    assert_eq!(probe_out(&out).timescale, 1000);
    assert_same_presentation(&source, &own);
}

#[test]
fn the_replacement_need_not_be_a_codec_mp4_carries() {
    // A source video MP4 cannot carry (here: ProRes): the plain remux drops
    // it; the replacement takes its place.
    let source = av_movie();
    let own = own_samples(&source, 0);
    let mut odd = source.clone();
    let at = odd.windows(4).position(|w| w == b"avc1").unwrap();
    odd[at..at + 4].copy_from_slice(b"apcn");
    let (plain_summary, _) = plain(&odd);
    assert_eq!(plain_summary.tracks.len(), 1);
    let (summary, out) = replaced(&odd, &own).unwrap();
    assert_eq!(summary.tracks.len(), 2);
    assert!(summary.tracks[0].replaced);
    assert_eq!(probe_out(&out).tracks[0].description.codec, Codec::H264);
}

// ------------------------------------------------------------- refusals

fn unsupported(result: Result<(RemuxSummary, Vec<u8>), ExtractError>) -> &'static str {
    match result {
        Err(ExtractError::Source(Error::Unsupported { reason })) => reason,
        other => panic!("{:?}", other.map(|(s, _)| s)),
    }
}

#[test]
fn invalid_replacements_are_refused_before_writing() {
    let source = av_movie();
    let check = |change: &dyn Fn(&mut Own), reason: &str| {
        let mut own = own_samples(&source, 0);
        change(&mut own);
        let mut out = Vec::new();
        let result = remux_mp4_replacing(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
            &RemuxOptions::default(),
            own.replacement(),
            &mut out,
        );
        match result {
            Err(ExtractError::Source(Error::Unsupported { reason: why })) => {
                assert_eq!(why, reason);
            }
            other => panic!("{other:?}"),
        }
        assert!(out.is_empty());
    };
    check(
        &|o| o.samples[3].decode_time = o.samples[2].decode_time,
        "the replacement's decode times do not strictly increase",
    );
    check(
        &|o| o.samples[3].decode_time = o.samples[2].decode_time - 1,
        "the replacement's decode times do not strictly increase",
    );
    check(
        &|o| o.samples[3].presentation_time = o.samples[3].decode_time - 1,
        "a replacement sample is presented before it is decoded",
    );
    check(
        &|o| {
            o.data.pop();
        },
        "the replacement's samples run past the end of its data",
    );
    check(
        &|o| o.samples[5].size += 1_000_000,
        "the replacement's samples run past the end of its data",
    );
    check(
        &|o| o.samples[1].decode_time = o.samples[0].decode_time + (1 << 33),
        "a replacement sample lasts too long for a 32-bit duration",
    );
    check(&|o| o.samples.clear(), "the replacement has no samples");
    check(
        &|o| o.last_duration = 0,
        "the replacement's last sample lasts 0",
    );
    check(
        &|o| o.entry[4..8].copy_from_slice(b"mp4a"),
        "the replacement sample entry is not avc1, avc3, hvc1, hev1, av01 or vp09",
    );
    check(
        &|o| {
            o.entry.push(0);
        },
        "the replacement sample entry is not one box",
    );
    check(
        &|o| o.entry[15] = 2,
        "the replacement sample entry is not a visual sample entry with data reference 1",
    );
    check(
        &|o| {
            o.entry = vec![0, 0, 0, 16, b'a', b'v', b'c', b'1', 0, 0, 0, 0, 0, 0, 0, 1];
        },
        "the replacement sample entry is not a visual sample entry with data reference 1",
    );
    check(
        &|o| o.stream_index = 1,
        "the replaced stream is not a video stream",
    );
    // Nothing presented: every frame decodes after the edit ends.
    check(
        &|o| {
            for s in &mut o.samples {
                s.decode_time += 10_000_000;
                s.presentation_time += 10_000_000;
            }
            // The edit's media time is now before the first decode time by
            // more than the edit lasts.
        },
        "nothing of the replacement is presented",
    );
}

#[test]
fn a_stream_that_does_not_exist_or_is_not_selected_is_refused() {
    let source = av_movie();
    let own = Own {
        stream_index: 7,
        ..own_samples(&source, 0)
    };
    assert!(matches!(
        replaced(&source, &own),
        Err(ExtractError::Source(Error::NoSuchTrack))
    ));
    let own = own_samples(&source, 0);
    let mut out = Vec::new();
    let options = RemuxOptions {
        streams: Some(vec![1]),
        ..RemuxOptions::default()
    };
    let result = remux_mp4_replacing(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &options,
        own.replacement(),
        &mut out,
    );
    assert!(matches!(
        result,
        Err(ExtractError::Source(Error::Unsupported {
            reason: "the replaced stream is not among the selected streams"
        }))
    ));
    // Not allowed by the allowlist.
    let result = remux_mp4_replacing(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::new(&[Codec::Aac]),
        &RemuxOptions::default(),
        own.replacement(),
        &mut out,
    );
    assert!(
        matches!(
            result,
            Err(ExtractError::Source(Error::CodecNotAllowed { .. }))
        ),
        "{result:?}"
    );
}

#[test]
fn an_edit_list_outside_the_simple_model_is_refused() {
    // Two media edits.
    let video = TrackSpec {
        edits: vec![(1000, 1024), (1000, 30_000)],
        ..ibbp_video(100, 25)
    };
    let source = movie(&[video, aac_audio(190, vec![(4000, 2112)])]);
    let own = own_samples(&source, 0);
    assert_eq!(
        unsupported(replaced(&source, &own)),
        "the replaced stream's edit list is outside the simple model \
         (at most one empty edit, then one media edit at rate 1)"
    );
}

#[test]
fn limits_apply_to_the_replacement() {
    let source = av_movie();
    let own = own_samples(&source, 0);
    let limits = Limits {
        max_table_bytes: 300,
        ..Limits::default()
    };
    let result = remux_mp4_replacing(
        SliceSource::new(&source),
        &limits,
        &Allowlist::default(),
        &RemuxOptions::default(),
        own.replacement(),
        Vec::new(),
    );
    assert!(
        matches!(
            result,
            Err(ExtractError::Source(Error::LimitExceeded {
                limit: LimitKind::TableBytes,
                ..
            }))
        ),
        "{result:?}"
    );
    let limits = Limits {
        max_packet_bytes: 40,
        ..Limits::default()
    };
    let result = remux_mp4_replacing(
        SliceSource::new(&source),
        &limits,
        &Allowlist::default(),
        &RemuxOptions::default(),
        own.replacement(),
        Vec::new(),
    );
    assert!(
        matches!(
            result,
            Err(ExtractError::Source(Error::LimitExceeded {
                limit: LimitKind::PacketBytes,
                ..
            }))
        ),
        "{result:?}"
    );
}

#[test]
fn the_avc1_entry_has_its_boxes_in_order() {
    let avcc = [1, 100, 0, 30, 0xFF, 0xE0, 0];
    let entry = avc1_sample_entry(
        1920,
        1080,
        &avcc,
        Some((4, 3)),
        Some(Nclx {
            primaries: 1,
            transfer: 1,
            matrix: 1,
            full_range: true,
        }),
    )
    .unwrap();
    assert_eq!(&entry[4..8], b"avc1");
    assert_eq!(
        u32::from_be_bytes(entry[..4].try_into().unwrap()) as usize,
        entry.len()
    );
    assert_eq!(&entry[32..36], &[0x07, 0x80, 0x04, 0x38]);
    let children = &entry[86..];
    assert_eq!(&children[4..8], b"avcC");
    assert_eq!(&children[8..15], &avcc);
    assert_eq!(&children[19..23], b"colr");
    assert_eq!(&children[23..34], b"nclx\0\x01\0\x01\0\x01\x80");
    assert_eq!(&children[38..42], b"pasp");
    assert_eq!(&children[42..50], &[0, 0, 0, 4, 0, 0, 0, 3]);
    assert_eq!(children.len(), 50);
    let bare = avc1_sample_entry(16, 16, &avcc, None, None).unwrap();
    assert_eq!(bare.len(), 86 + 15);
    assert!(avc1_sample_entry(16, 16, &[0, 1, 2, 3, 4, 5, 6], None, None).is_err());
    assert!(avc1_sample_entry(16, 16, &avcc, Some((0, 1)), None).is_err());
}

// ------------------------------------------------------------- Matroska

const CODEC_PRIVATE: u32 = 0x63A2;
const DEFAULT_DURATION: u32 = 0x23_E383;

/// The `avcC` of a high-profile parameter set pair.
fn avcc() -> Vec<u8> {
    let sps = [0x67, 100, 0, 30, 0xAC, 0xD9, 0x40, 0x50, 0x05, 0xBB];
    let pps = [0x68, 0xEB, 0xE3, 0xCB];
    let mut record = vec![1, 100, 0, 30, 0xFF, 0xE1];
    record.extend((sps.len() as u16).to_be_bytes());
    record.extend(sps);
    record.push(1);
    record.extend((pps.len() as u16).to_be_bytes());
    record.extend(pps);
    record.extend([0xFD, 0xF8, 0xF8, 0]);
    record
}

/// H.264 (with B-frames) and AAC in Matroska, the video starting 1 s into
/// the segment; `codec_id` names the video codec.
fn mkv_source(codec_id: &str) -> Vec<u8> {
    use mkv::{cat, cluster, el, file, info, simple_block, uint};
    let presentation = [0i16, 120, 40, 80, 240, 160, 200, 280, 400, 320, 360];
    let mut blocks = Vec::new();
    for (n, &t) in presentation.iter().enumerate() {
        let frame: Vec<u8> = (0..30 + n).map(|i| (i * 31 + n * 7 + 1) as u8).collect();
        blocks.push(simple_block(1, t, n == 0 || n == 8, &frame));
        if n < 9 {
            let audio: Vec<u8> = (0..12).map(|i| (i * 5 + n) as u8).collect();
            blocks.push(simple_block(2, (n as i16) * 21 + 3, true, &audio));
        }
    }
    let video = mkv::video_track(
        1,
        codec_id,
        64,
        48,
        &cat(&[
            el(CODEC_PRIVATE, &avcc()),
            uint(DEFAULT_DURATION, 40_000_000),
        ]),
    );
    let audio = mkv::audio_track(2, "A_AAC", 48_000.0, 2, &el(CODEC_PRIVATE, &[0x11, 0x90]));
    file(
        "matroska",
        &[
            info(None),
            el(mkv::TRACKS, &cat(&[video, audio])),
            cluster(1000, &blocks),
        ],
    )
}

/// The Matroska source's video frames with the decode times the plain
/// remux gives them (presentation times sorted, shifted back by the
/// largest reordering), in Segment Ticks.
fn mkv_own(source: &[u8]) -> Own {
    let limits = coreav_mkv::Limits::default();
    let segment = coreav_mkv::probe(
        SliceSource::new(source),
        &limits,
        &coreav_mkv::Allowlist::default(),
    )
    .unwrap();
    let mut data = Vec::new();
    let mut frames = Vec::new();
    for packet in segment.packets(SliceSource::new(source), &limits) {
        let packet = packet.unwrap();
        if packet.track != 0 {
            continue;
        }
        data.extend(bytes(source, packet.offset, packet.stored_size as u32));
        frames.push((
            packet.stored_size as u32,
            packet.keyframe,
            packet.pts.unwrap(),
        ));
    }
    let mut sorted: Vec<i64> = frames.iter().map(|f| f.2).collect();
    sorted.sort_unstable();
    let lead = sorted
        .iter()
        .zip(&frames)
        .map(|(s, f)| s - f.2)
        .max()
        .unwrap()
        .max(0);
    let samples = frames
        .iter()
        .zip(&sorted)
        .map(|(f, s)| ReplacementSample {
            size: f.0,
            sync: f.1,
            decode_time: s - lead,
            presentation_time: f.2,
        })
        .collect();
    Own {
        stream_index: 0,
        entry: avc1_sample_entry(64, 48, &avcc(), None, None).unwrap(),
        width: 64 << 16,
        height: 48 << 16,
        samples,
        last_duration: 40,
        data,
    }
}

fn mkv_plain(source: &[u8]) -> (MatroskaRemuxSummary, Vec<u8>) {
    let mut out = Vec::new();
    let summary = remux_matroska(
        SliceSource::new(source),
        &coreav_mkv::Limits::default(),
        &Limits::default(),
        &coreav_mkv::Allowlist::default(),
        &MatroskaRemuxOptions::default(),
        &mut out,
    )
    .unwrap();
    (summary, out)
}

fn mkv_replaced(
    source: &[u8],
    own: &Own,
) -> Result<(MatroskaRemuxSummary, Vec<u8>), MatroskaRemuxError> {
    let mut out = Vec::new();
    let summary = remux_matroska_replacing(
        SliceSource::new(source),
        &coreav_mkv::Limits::default(),
        &Limits::default(),
        &coreav_mkv::Allowlist::default(),
        &MatroskaRemuxOptions::default(),
        own.replacement(),
        &mut out,
    )?;
    assert_eq!(summary.bytes_written, out.len() as u64);
    Ok((summary, out))
}

#[test]
fn matroska_own_frames_present_as_the_plain_remux() {
    let source = mkv_source("V_MPEG4/ISO/AVC");
    let (plain_summary, plain_out) = mkv_plain(&source);
    for shift in [0, 80, 1000, 5_000] {
        let own = mkv_own(&source).shifted(shift);
        let (summary, out) = mkv_replaced(&source, &own).unwrap();
        assert!(summary.tracks[0].replaced);
        assert!(!summary.tracks[1].replaced);
        assert_eq!(summary.tracks[1], plain_summary.tracks[1]);
        let (a, b) = (probe_out(&plain_out), probe_out(&out));
        assert_eq!(presentation(&plain_out, &a, 0), presentation(&out, &b, 0));
        if shift == 0 {
            assert_eq!(a.tracks[0].edit_list, b.tracks[0].edit_list);
        }
        assert_eq!(copied(&plain_out, &a, 1), copied(&out, &b, 1));
        assert_eq!(presentation(&plain_out, &a, 1), presentation(&out, &b, 1));
        // The replacement's bytes, in decode order.
        let written: Vec<u8> = packets(&out, &b, 0)
            .iter()
            .flat_map(|p| bytes(&out, p.offset, p.size))
            .collect();
        assert_eq!(written, own.data);
    }
}

#[test]
fn matroska_video_mp4_cannot_carry_is_replaced() {
    // VP8 video: the plain remux refuses to lose it; the replacement
    // takes its place with the same timeline as copied H.264 would have.
    let source = mkv_source("V_VP8");
    assert!(matches!(
        remux_matroska(
            SliceSource::new(&source),
            &coreav_mkv::Limits::default(),
            &Limits::default(),
            &coreav_mkv::Allowlist::default(),
            &MatroskaRemuxOptions::default(),
            Vec::new(),
        ),
        Err(MatroskaRemuxError::VideoNotCopied { .. })
    ));
    let h264 = mkv_source("V_MPEG4/ISO/AVC");
    let (_, plain_out) = mkv_plain(&h264);
    let own = mkv_own(&source);
    let (summary, out) = mkv_replaced(&source, &own).unwrap();
    assert_eq!(summary.tracks[0].codec, coreav_mkv::Codec::H264);
    let (a, b) = (probe_out(&plain_out), probe_out(&out));
    assert_eq!(presentation(&plain_out, &a, 0), presentation(&out, &b, 0));
    assert_eq!(b.tracks[0].description.codec, Codec::H264);
}

#[test]
fn matroska_refusals() {
    let source = mkv_source("V_MPEG4/ISO/AVC");
    let reason = |own: &Own| match mkv_replaced(&source, own) {
        Err(MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported { reason })) => reason,
        Err(MatroskaRemuxError::Output(Error::Unsupported { reason })) => reason,
        other => panic!("{:?}", other.map(|(s, _)| s)),
    };
    let own = Own {
        stream_index: 1,
        ..mkv_own(&source)
    };
    assert_eq!(reason(&own), "the replaced stream is not a video stream");
    let own = Own {
        stream_index: 5,
        ..mkv_own(&source)
    };
    assert_eq!(reason(&own), "the replaced stream does not exist");
    let own = mkv_own(&source);
    let result = remux_matroska_replacing(
        SliceSource::new(&source),
        &coreav_mkv::Limits::default(),
        &Limits::default(),
        &coreav_mkv::Allowlist::new(&[coreav_mkv::Codec::Aac]),
        &MatroskaRemuxOptions::default(),
        own.replacement(),
        Vec::new(),
    );
    assert!(
        matches!(
            result,
            Err(MatroskaRemuxError::Source(
                coreav_mkv::Error::CodecNotAllowed {
                    track_number: 1,
                    codec: coreav_mkv::Codec::H264
                }
            ))
        ),
        "{result:?}"
    );
    let mut own = mkv_own(&source);
    own.samples[2].decode_time = own.samples[1].decode_time;
    assert_eq!(
        reason(&own),
        "the replacement's decode times do not strictly increase"
    );
    let mut own = mkv_own(&source);
    own.data.truncate(10);
    assert_eq!(
        reason(&own),
        "the replacement's samples run past the end of its data"
    );
}

// ------------------------------------------------------------- fixtures

/// The fixtures with H.264 video, and the stream replaced.
const FIXTURES: [(&str, usize); 10] = [
    ("basic.mp4", 0),
    ("basic.mov", 0),
    ("edit_list.mp4", 0),
    ("clip_no_stss_bframes.mp4", 0),
    ("rotate_ccw90.mp4", 0),
    ("vfr.mp4", 0),
    ("negative_cts.mp4", 0),
    ("fragmented.mp4", 0),
    ("clip_video_late.mp4", 0),
    ("multitrack.mp4", 0),
];

fn ffprobe(path: &Path, args: &[&str]) -> String {
    let out = Command::new("ffprobe")
        .args(["-v", "error"])
        .args(args)
        .arg(path)
        .output()
        .expect("ffprobe on PATH");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// What `ffprobe` sees of a file: the video frames' presentation times
/// (decoded, in presentation order; or, for samples that are not H.264,
/// the packets' presentation times in increasing order), and every audio
/// packet with its times and data.
fn oracle_view(path: &Path, decoded: bool) -> (String, String) {
    let frames = if decoded {
        ffprobe(
            path,
            &[
                "-select_streams",
                "v:0",
                "-show_entries",
                "frame=best_effort_timestamp_time",
                "-of",
                "csv=p=0",
            ],
        )
    } else {
        let listed = ffprobe(
            path,
            &[
                "-select_streams",
                "v:0",
                "-show_entries",
                "packet=pts_time",
                "-of",
                "csv=p=0",
            ],
        );
        let mut times: Vec<f64> = listed.lines().map(|l| l.trim().parse().unwrap()).collect();
        times.sort_by(f64::total_cmp);
        times.iter().map(|t| format!("{t:.6}\n")).collect()
    };
    let audio = ffprobe(
        path,
        &[
            "-select_streams",
            "a",
            "-show_data_hash",
            "MD5",
            "-show_entries",
            "packet=stream_index,pts_time,dts_time,duration_time,size,flags,data_hash",
            "-of",
            "csv=p=0",
        ],
    );
    (frames, audio)
}

fn check_with_oracle(name: &str, source: &[u8], dir: &Path, own: &Own, mp4: bool, decoded: bool) {
    let plain_out = if mp4 {
        plain(source).1
    } else {
        mkv_plain(source).1
    };
    let plain_path = dir.join(format!("{name}.plain.mp4"));
    std::fs::write(&plain_path, &plain_out).unwrap();
    let expected = oracle_view(&plain_path, decoded);
    assert!(!expected.0.is_empty(), "{name}: no frames");
    let two_frames = 2 * i64::from(own.last_duration);
    for (label, shift) in [("own", 0), ("shifted", two_frames)] {
        let shifted = Own {
            samples: own
                .samples
                .iter()
                .map(|s| ReplacementSample {
                    decode_time: s.decode_time - shift,
                    ..*s
                })
                .collect(),
            entry: own.entry.clone(),
            data: own.data.clone(),
            ..*own
        };
        let out = if mp4 {
            replaced(source, &shifted).unwrap().1
        } else {
            mkv_replaced(source, &shifted).unwrap().1
        };
        let path = dir.join(format!("{name}.{label}.mp4"));
        std::fs::write(&path, &out).unwrap();
        let seen = oracle_view(&path, decoded);
        assert_eq!(seen.0, expected.0, "{name} {label}: video frames");
        assert_eq!(seen.1, expected.1, "{name} {label}: audio packets");
    }
}

#[test]
#[ignore = "needs COREAV_FIXTURES, and ffprobe and ffmpeg on PATH"]
fn fixtures_present_as_the_plain_remux() {
    let dir = std::env::var("COREAV_FIXTURES").expect("COREAV_FIXTURES");
    let out = std::env::temp_dir().join(format!("coreav-replace-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    for (name, stream) in FIXTURES {
        let source = std::fs::read(Path::new(&dir).join(name)).unwrap();
        let own = own_samples(&source, stream);
        check_with_oracle(name, &source, &out, &own, true, true);
    }
    // The rotation: kept as stored at the same size, and still a quarter
    // turn in ffprobe at another size (the translation follows it).
    let rotated = std::fs::read(Path::new(&dir).join("rotate_ccw90.mp4")).unwrap();
    let rotation = |path: &Path| {
        ffprobe(
            path,
            &[
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream_side_data=rotation",
                "-of",
                "csv=p=0",
            ],
        )
    };
    let source_rotation = rotation(&Path::new(&dir).join("rotate_ccw90.mp4"));
    assert!(!source_rotation.trim().is_empty());
    let own = own_samples(&rotated, 0);
    let (_, same) = replaced(&rotated, &own).unwrap();
    assert_eq!(
        probe_out(&same).tracks[0].matrix,
        probe_out(&rotated).tracks[0].matrix
    );
    let half = Own {
        width: own.width / 2,
        height: own.height / 2,
        ..own_samples(&rotated, 0)
    };
    let (_, scaled) = replaced(&rotated, &half).unwrap();
    let scaled_path = out.join("rotate_ccw90.scaled.mp4");
    std::fs::write(&scaled_path, &scaled).unwrap();
    assert_eq!(rotation(&scaled_path), source_rotation);
    // Synthetic movies (samples that are not H.264, so packets): the
    // source edit's own gap before its first frame; and no edit list with
    // frames 100 ticks after their decode times, whose two-frame shift
    // needs an empty edit that switches the movie timescale to 64,000.
    for (name, source) in [
        ("gap.mp4", offset_movie(vec![(2000, 0)])),
        ("late.mp4", offset_movie(Vec::new())),
    ] {
        let own = own_samples(&source, 0);
        check_with_oracle(name, &source, &out, &own, true, false);
    }
    let late = offset_movie(Vec::new());
    let (_, switched) = replaced(&late, &own_samples(&late, 0).shifted(1024)).unwrap();
    assert_eq!(probe_out(&switched).timescale, 64_000);
    // Two media edits: refused.
    let source = std::fs::read(Path::new(&dir).join("edit_multi_bframes.mp4")).unwrap();
    let own = own_samples(&source, 0);
    assert!(unsupported(replaced(&source, &own)).contains("simple model"));
    // H.264 with B-frames and AAC in Matroska, made by the oracle ffmpeg.
    let mkv_path = out.join("h264_aac.mkv");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=25:duration=2",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2:sample_rate=48000",
        ])
        .args([
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "25", "-bf", "3",
        ])
        .args(["-c:a", "aac", "-shortest"])
        .arg(&mkv_path)
        .status()
        .expect("ffmpeg on PATH");
    assert!(status.success());
    let source = std::fs::read(&mkv_path).unwrap();
    let mut own = mkv_own(&source);
    let segment = coreav_mkv::probe(
        SliceSource::new(&source),
        &coreav_mkv::Limits::default(),
        &coreav_mkv::Allowlist::default(),
    )
    .unwrap();
    let track = &segment.tracks[0];
    own.entry = avc1_sample_entry(
        320,
        240,
        track.codec_private.as_deref().unwrap(),
        None,
        None,
    )
    .unwrap();
    own.width = 320 << 16;
    own.height = 240 << 16;
    check_with_oracle("h264_aac.mkv", &source, &out, &own, false, true);
    std::fs::remove_dir_all(&out).unwrap();
}

// ------------------------------------------------------------- audio

/// An encoded audio stream: its sample entry, its samples, and their bytes.
struct EncodedStream {
    entry: Vec<u8>,
    samples: Vec<AudioSample>,
    data: Vec<u8>,
}

/// An Opus stream of `count` 20 ms packets: `dOps` for stereo with a
/// pre-skip of 312, packets whose bytes are their number.
fn opus_stream(count: usize, last: u32) -> EncodedStream {
    let mut dops = vec![0, 2];
    dops.extend_from_slice(&312u16.to_be_bytes());
    dops.extend_from_slice(&44_100u32.to_be_bytes());
    dops.extend_from_slice(&[0, 0, 0]);
    let entry = audio_sample_entry(
        coreav_isobmff::FourCc(*b"Opus"),
        2,
        48_000,
        &[RawBox {
            kind: coreav_isobmff::FourCc(*b"dOps"),
            payload: dops,
        }],
    )
    .unwrap();
    let mut samples = Vec::new();
    let mut data = Vec::new();
    for n in 0..count {
        let size = 20 + (n % 9) as u32;
        data.extend(std::iter::repeat_n(n as u8, size as usize));
        let duration = if n + 1 == count { last } else { 960 };
        samples.push(AudioSample { size, duration });
    }
    EncodedStream {
        entry,
        samples,
        data,
    }
}

impl EncodedStream {
    fn replacement(
        &self,
        stream_index: usize,
        start: u64,
    ) -> AudioReplacement<'_, SliceSource<'_>> {
        let total: u64 = self.samples.iter().map(|s| u64::from(s.duration)).sum();
        AudioReplacement {
            stream_index,
            sample_entry: &self.entry,
            timescale: 48_000,
            samples: &self.samples,
            start,
            priming: 312,
            presented: total - 312,
            roll: 4,
            data: SliceSource::new(&self.data),
        }
    }
}

/// The payload of the first box of `kind` in `data` (a plain byte search,
/// for boxes whose type appears once).
fn find_box<'a>(data: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
    let at = data.windows(4).position(|w| w == kind).unwrap();
    let size = u32::from_be_bytes(data[at - 4..at].try_into().unwrap()) as usize;
    &data[at + 4..at - 4 + size]
}

fn assert_opus_track(out: &[u8], movie: &Movie, index: usize, opus: &EncodedStream, empty: u64) {
    let track = &movie.tracks[index];
    assert_eq!(track.kind, TrackKind::Audio);
    assert_eq!(track.description.codec, Codec::Opus);
    assert_eq!(track.timescale, 48_000);
    let presented: u64 = opus
        .samples
        .iter()
        .map(|s| u64::from(s.duration))
        .sum::<u64>()
        - 312;
    // Sample-exact: the movie timescale is a multiple of 48 kHz.
    assert_eq!(movie.timescale % 48_000, 0);
    let scale = u64::from(movie.timescale / 48_000);
    assert_eq!(
        track.edit_list,
        EditList::Simple {
            empty: empty * scale,
            media_time: 312,
            segment: presented * scale,
        }
    );
    let written = packets(out, movie, index);
    assert_eq!(written.len(), opus.samples.len());
    let mut at = 0;
    for (p, s) in written.iter().zip(&opus.samples) {
        assert_eq!((p.size, p.duration, p.keyframe), (s.size, s.duration, true));
        assert_eq!(
            bytes(out, p.offset, p.size),
            opus.data[at..at + s.size as usize]
        );
        at += s.size as usize;
    }
    // Every sample a sync sample (no `stss` but the video's), and the roll
    // group: four samples outside it, the rest at a distance of -4.
    assert_eq!(out.windows(4).filter(|w| *w == b"stss").count(), 1);
    assert_eq!(
        find_box(out, b"sgpd"),
        [
            1, 0, 0, 0, b'r', b'o', b'l', b'l', 0, 0, 0, 2, 0, 0, 0, 1, 0xFF, 0xFC
        ]
    );
    let rest = (opus.samples.len() - 4) as u32;
    let mut sbgp = vec![
        0, 0, 0, 0, b'r', b'o', b'l', b'l', 0, 0, 0, 2, 0, 0, 0, 4, 0, 0, 0, 0,
    ];
    sbgp.extend_from_slice(&rest.to_be_bytes());
    sbgp.extend_from_slice(&1u32.to_be_bytes());
    assert_eq!(find_box(out, b"sbgp"), sbgp);
}

#[test]
fn an_opus_replacement_takes_the_audio_streams_place() {
    let source = av_movie();
    let (plain_summary, plain_out) = plain(&source);
    let opus = opus_stream(60, 500);
    let mut out = Vec::new();
    let summary = remux_mp4_replacing(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &RemuxOptions::default(),
        Replacements {
            video: None,
            audio: Some(opus.replacement(1, 0)),
        },
        &mut out,
    )
    .unwrap();
    assert_eq!(summary.bytes_written, out.len() as u64);
    assert_eq!(summary.tracks[0], plain_summary.tracks[0]);
    let track = &summary.tracks[1];
    assert!(track.replaced);
    assert_eq!(
        (track.codec, track.timescale, track.samples),
        (Codec::Opus, 48_000, 60)
    );
    let (a, b) = (probe_out(&plain_out), probe_out(&out));
    assert_eq!(b.tracks[1].language, a.tracks[1].language);
    assert_opus_track(&out, &b, 1, &opus, 0);
    // The video is copied as the plain remux copies it, and presented at
    // the same times though the movie timescale changed.
    assert_eq!(copied(&plain_out, &a, 0), copied(&out, &b, 0));
    assert_eq!(presentation(&plain_out, &a, 0), presentation(&out, &b, 0));
    assert!(find_box(&out, b"ftyp").windows(4).any(|w| w == b"iso2"));
}

#[test]
fn video_and_audio_replaced_together() {
    let source = av_movie();
    let own = own_samples(&source, 0);
    let opus = opus_stream(9, 960);
    let mut replacements = own.replacement();
    replacements.audio = Some(opus.replacement(1, 0));
    let mut out = Vec::new();
    let summary = remux_mp4_replacing(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &RemuxOptions::default(),
        replacements,
        &mut out,
    )
    .unwrap();
    assert!(summary.tracks.iter().all(|t| t.replaced));
    let (plain_summary, plain_out) = plain(&source);
    assert_eq!(summary.tracks[0].samples, plain_summary.tracks[0].samples);
    let (a, b) = (probe_out(&plain_out), probe_out(&out));
    assert_eq!(presentation(&plain_out, &a, 0), presentation(&out, &b, 0));
    assert_opus_track(&out, &b, 1, &opus, 0);
}

#[test]
fn audio_replacements_are_checked_before_anything_is_written() {
    let source = av_movie();
    let opus = opus_stream(9, 960);
    let run = |audio: AudioReplacement<'_, SliceSource<'_>>| {
        let mut out = Vec::new();
        let result = remux_mp4_replacing(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
            &RemuxOptions::default(),
            Replacements {
                video: None,
                audio: Some(audio),
            },
            &mut out,
        );
        assert!(out.is_empty());
        unsupported(result.map(|s| (s, Vec::new())))
    };
    assert!(run(opus.replacement(0, 0)).contains("not an audio stream"));
    let mut too_long = opus.replacement(1, 0);
    too_long.presented += 1;
    assert!(run(too_long).contains("more than its samples hold"));
    let mut nothing = opus.replacement(1, 0);
    nothing.presented = 0;
    assert!(run(nothing).contains("presents nothing"));
    let entry = avc1_sample_entry(64, 48, &[1, 100, 0, 30, 0xFF, 0xE0, 0], None, None).unwrap();
    let mut video_entry = opus.replacement(1, 0);
    video_entry.sample_entry = &entry;
    assert!(run(video_entry).contains("not Opus or mp4a"));
    let short = &opus.data[..opus.data.len() - 1];
    let mut cut = opus.replacement(1, 0);
    cut.data = SliceSource::new(short);
    assert!(run(cut).contains("past the end of its data"));
}

#[test]
fn matroska_audio_replaced_after_an_empty_edit() {
    let source = mkv_source("V_MPEG4/ISO/AVC");
    let (plain_summary, plain_out) = mkv_plain(&source);
    let opus = opus_stream(12, 7);
    // The source's audio starts 3 ms into the segment: 144 ticks at 48 kHz.
    let mut out = Vec::new();
    let summary = remux_matroska_replacing(
        SliceSource::new(&source),
        &coreav_mkv::Limits::default(),
        &Limits::default(),
        &coreav_mkv::Allowlist::default(),
        &MatroskaRemuxOptions::default(),
        Replacements {
            video: None,
            audio: Some(opus.replacement(1, 144)),
        },
        &mut out,
    )
    .unwrap();
    assert_eq!(summary.bytes_written, out.len() as u64);
    assert_eq!(summary.tracks[0], plain_summary.tracks[0]);
    assert!(summary.tracks[1].replaced);
    assert_eq!(summary.tracks[1].codec, coreav_mkv::Codec::Opus);
    let (a, b) = (probe_out(&plain_out), probe_out(&out));
    assert_opus_track(&out, &b, 1, &opus, 144);
    assert_eq!(presentation(&plain_out, &a, 0), presentation(&out, &b, 0));
    assert_eq!(copied(&plain_out, &a, 0), copied(&out, &b, 0));
}

/// An AAC-LC stream of `count` frames at 44.1 kHz, stereo, with a delay
/// of 1,024 samples and 300 samples of padding: an `mp4a` entry with its
/// `esds`, frames whose bytes are their number.
fn aac_stream(count: usize) -> EncodedStream {
    let entry = audio_sample_entry(
        coreav_isobmff::FourCc(*b"mp4a"),
        2,
        44_100,
        &[RawBox {
            kind: coreav_isobmff::FourCc(*b"esds"),
            payload: esds(&[0x12, 0x10, 0x56, 0xE5, 0x00]),
        }],
    )
    .unwrap();
    let mut samples = Vec::new();
    let mut data = Vec::new();
    for n in 0..count {
        let size = 200 + (n % 9) as u32;
        data.extend(std::iter::repeat_n(n as u8, size as usize));
        samples.push(AudioSample {
            size,
            duration: 1024,
        });
    }
    EncodedStream {
        entry,
        samples,
        data,
    }
}

fn aac_replacement(
    aac: &EncodedStream,
    stream_index: usize,
) -> AudioReplacement<'_, SliceSource<'_>> {
    AudioReplacement {
        stream_index,
        sample_entry: &aac.entry,
        timescale: 44_100,
        samples: &aac.samples,
        start: 0,
        priming: 1024,
        presented: aac.samples.len() as u64 * 1024 - 1024 - 300,
        roll: 1,
        data: SliceSource::new(&aac.data),
    }
}

fn assert_aac_track(out: &[u8], movie: &Movie, index: usize, aac: &EncodedStream) {
    let track = &movie.tracks[index];
    assert_eq!(track.kind, TrackKind::Audio);
    assert_eq!(track.description.codec, Codec::Aac);
    assert_eq!(track.description.sample_rate, Some(44_100));
    assert_eq!(track.description.channels, Some(2));
    assert_eq!(track.timescale, 44_100);
    let EditList::Simple {
        empty, media_time, ..
    } = track.edit_list
    else {
        panic!("{:?}", track.edit_list);
    };
    assert_eq!((empty, media_time), (0, 1024));
    let written = packets(out, movie, index);
    assert_eq!(written.len(), aac.samples.len());
    let mut at = 0;
    for (p, s) in written.iter().zip(&aac.samples) {
        assert_eq!((p.size, p.duration, p.keyframe), (s.size, 1024, true));
        assert_eq!(
            bytes(out, p.offset, p.size),
            aac.data[at..at + s.size as usize]
        );
        at += s.size as usize;
    }
    // The roll group: one frame outside it, the rest at a distance of -1.
    assert_eq!(
        find_box(out, b"sgpd"),
        [
            1, 0, 0, 0, b'r', b'o', b'l', b'l', 0, 0, 0, 2, 0, 0, 0, 1, 0xFF, 0xFF
        ]
    );
}

#[test]
fn an_aac_replacement_takes_the_audio_streams_place() {
    let source = av_movie();
    let aac = aac_stream(40);
    let mut out = Vec::new();
    let summary = remux_mp4_replacing(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &RemuxOptions::default(),
        Replacements {
            video: None,
            audio: Some(aac_replacement(&aac, 1)),
        },
        &mut out,
    )
    .unwrap();
    let track = &summary.tracks[1];
    assert!(track.replaced);
    assert_eq!(
        (track.codec, track.timescale, track.samples),
        (Codec::Aac, 44_100, 40)
    );
    assert_aac_track(&out, &probe_out(&out), 1, &aac);
    assert!(!find_box(&out, b"ftyp").windows(4).any(|w| w == b"Opus"));
}

#[test]
fn matroska_audio_replaced_with_aac() {
    let source = mkv_source("V_MPEG4/ISO/AVC");
    let aac = aac_stream(12);
    let mut out = Vec::new();
    let summary = remux_matroska_replacing(
        SliceSource::new(&source),
        &coreav_mkv::Limits::default(),
        &Limits::default(),
        &coreav_mkv::Allowlist::default(),
        &MatroskaRemuxOptions::default(),
        Replacements {
            video: None,
            audio: Some(aac_replacement(&aac, 1)),
        },
        &mut out,
    )
    .unwrap();
    assert!(summary.tracks[1].replaced);
    assert_eq!(summary.tracks[1].codec, coreav_mkv::Codec::Aac);
    assert_aac_track(&out, &probe_out(&out), 1, &aac);
}
