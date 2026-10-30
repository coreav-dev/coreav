//! HLS round trips (docs/design/hls.md, "Verification"): per rendition, the
//! init segment followed by every media segment is probed by
//! `coreav-isobmff`; its samples must be the source track's, all of them,
//! with the same bytes, sizes, durations, sync flags and composition
//! offsets, and every sample presented at the source's presentation time.
//! Segments must start with sync samples and partition the presentation
//! timeline. A damaged index must fail cleanly.
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
use coreav_isobmff_write::hls::{
    HlsIndex, HlsOptions, IndexTrack, MediaSegment, Rendition, Resource, build_index,
};
use coreav_isobmff_write::{AudioChoice, ExtractError, VideoChoice};
use movies::{S, TrackSpec, aac_audio, ibbp_video, movie};
use std::path::Path;

fn q(num: i128, den: u64) -> Rational {
    Rational::new(num, den).unwrap()
}

fn options(seconds: Rational) -> HlsOptions {
    HlsOptions {
        segment_duration: seconds,
        ..HlsOptions::default()
    }
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

/// Every segment of `rendition` after its init segment, and the samples
/// each one holds: a deferred rendition's directory does not say, since
/// the fragment it names does.
fn concatenation(
    index_bytes: &[u8],
    index: &HlsIndex,
    source: &[u8],
    r: Rendition,
) -> (Vec<u8>, Vec<u32>) {
    let mut out = index.init_segment(r).unwrap();
    let mut counts = Vec::new();
    let track = index.track(r).unwrap();
    for n in 0..track.segments().len() {
        let segment = index
            .media_segment(
                r,
                n as u32,
                SliceSource::new(index_bytes),
                SliceSource::new(source),
                &Limits::default(),
            )
            .unwrap();
        let before = out.len();
        counts.push(segment.samples());
        let written = segment.write(SliceSource::new(source), &mut out).unwrap();
        assert_eq!(written, segment.len());
        assert_eq!((out.len() - before) as u64, segment.len());
    }
    (out, counts)
}

/// Packages `source` and checks every rendition; returns the index.
fn assert_round_trip(source: &[u8], options: &HlsOptions) -> HlsIndex {
    let index_bytes = build_index(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
        options,
    )
    .unwrap();
    let index = HlsIndex::open(SliceSource::new(&index_bytes), &Limits::default()).unwrap();
    assert_eq!(index.source_len(), source.len() as u64);
    assert_eq!(index.source_tag(), options.source_tag.as_slice());
    let src_movie = probe(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    for track in index.tracks() {
        let (out, counts) = concatenation(&index_bytes, &index, source, track.rendition);
        assert_track(source, &src_movie, &out, track, &counts);
    }
    // Playlists name every segment once, and parse back.
    let master = index.master_playlist();
    for track in index.tracks() {
        let playlist = index.media_playlist(track.rendition).unwrap();
        assert!(master.contains(&Resource::Playlist(track.rendition).to_string()));
        let names: Vec<&str> = playlist.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(names.len(), track.segments().len());
        for (n, name) in names.iter().enumerate() {
            assert_eq!(
                name.parse::<Resource>(),
                Ok(Resource::Segment(track.rendition, n as u32))
            );
        }
        assert!(playlist.ends_with("#EXT-X-ENDLIST\n"));
    }
    index
}

/// One rendition's concatenation against its source track.
fn assert_track(source: &[u8], src_movie: &Movie, out: &[u8], track: &IndexTrack, counts: &[u32]) {
    let out_movie = probe(
        SliceSource::new(out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert!(out_movie.has_fragments());
    assert_eq!(out_movie.tracks.len(), 1);
    let src_track = src_movie
        .tracks
        .iter()
        .find(|t| t.stream_index == track.stream_index as usize)
        .unwrap();
    let out_track = &out_movie.tracks[0];
    assert_eq!(out_track.timescale, src_track.timescale);
    assert_eq!(out_track.description.codec, src_track.description.codec);
    assert_eq!(
        out_track.description.codec_config,
        src_track.description.codec_config
    );
    if src_track.kind == TrackKind::Video {
        assert_eq!(out_track.matrix, src_track.matrix);
        assert_eq!(out_track.presentation_size, src_track.presentation_size);
    }
    let src = packets(source, src_movie, src_track.index);
    let written = packets(out, &out_movie, 0);
    // The copied samples are a run of the source's; the samples left out,
    // before it (a lead the edit hides) or after it (a tail decoded after
    // the last presented sample), are all unpresented.
    let first = (0..=src.len().saturating_sub(written.len()))
        .find(|&k| {
            src[..k].iter().all(|p| !p.presented)
                && bytes(source, &src[k]) == bytes(out, &written[0])
        })
        .expect("the copied samples are a run of the source's");
    if first > 0 {
        assert!(src[first].keyframe);
    }
    assert!(src[first + written.len()..].iter().all(|p| !p.presented));
    let src = &src[first..first + written.len()];
    assert_eq!(written.len(), src.len());
    // A deferred rendition's header does not count its samples: only the
    // fragments do.
    if track.deferred.is_none() {
        assert_eq!(track.samples, src.len() as u64);
    }
    // One decode time shift for the whole rendition.
    let shift = written[0].dts - src[0].dts;
    for (s, o) in src.iter().zip(&written) {
        assert_eq!(o.size, s.size);
        assert_eq!(o.duration, s.duration);
        assert_eq!(o.keyframe, s.keyframe, "sample {}", s.sample);
        assert_eq!(o.cts - o.dts, s.cts - s.dts, "sample {}", s.sample);
        assert_eq!(o.dts - s.dts, shift);
        assert_eq!(bytes(out, o), bytes(source, s));
        assert_eq!(o.presented, s.presented, "sample {}", s.sample);
        if s.presented {
            // Exact, except that an empty edit of a fraction of a tick is
            // rounded to the nearest tick.
            let (a, b) = (o.pts.unwrap(), s.pts.unwrap());
            let gap = (a.num as f64 / a.den as f64 - b.num as f64 / b.den as f64).abs();
            assert!(
                a == b || (b.den != 1 && gap <= 0.5),
                "sample {}: {a:?} against {b:?}",
                s.sample
            );
        }
    }
    // Segments start with sync samples and partition the presentation:
    // every sample of a segment is presented at or after its start, and
    // before the next segment's.
    let timescale = u64::from(track.timescale);
    let mut first = 0usize;
    for (n, entry) in track.segments().iter().enumerate() {
        let samples = counts.get(n).copied().unwrap_or(entry.samples) as usize;
        let run = &src[first..first + samples];
        assert!(run[0].keyframe, "segment {n}");
        let end = track.segment_end(n).unwrap();
        for p in run.iter().filter(|p| p.presented) {
            let pts = p.pts.unwrap();
            let pts = q(pts.num, pts.den);
            if n > 0 {
                assert!(
                    !pts.checked_lt(q(entry.start.into(), 1)).unwrap(),
                    "segment {n}: sample {} at {pts:?} before {}",
                    p.sample,
                    entry.start
                );
            }
            if n + 1 < track.segments().len() && track.rendition == Rendition::Video {
                assert!(pts.checked_lt(q(end.into(), 1)).unwrap(), "segment {n}");
            }
        }
        first += samples;
        let duration = track.segment_duration(n).unwrap();
        assert_eq!(
            duration,
            q(i128::from(end) - i128::from(entry.start), timescale)
        );
    }
    assert_eq!(first, src.len());
}

fn av() -> Vec<u8> {
    // 4 s of video in GOPs of 25 frames (1 s), 4 s of audio after priming.
    movie(&[ibbp_video(100, 25), aac_audio(190, vec![(4000, 2112)])])
}

#[test]
fn video_segments_start_at_the_first_sync_sample_past_each_target() {
    let source = av();
    // Targets every 1.5 s: GOPs start every second, so cuts at 2 s and 3 s.
    let index = assert_round_trip(&source, &options(q(3, 2)));
    let video = index.track(Rendition::Video).unwrap();
    let starts: Vec<u64> = video.segments().iter().map(|s| s.start).collect();
    assert_eq!(starts, [0, 2 * 12_800, 3 * 12_800]);
    assert_eq!(video.end, 4 * 12_800);
    assert!(video.independent);
    assert_eq!(video.codecs, "avc1.64001E");
    // Audio follows the video's starts: the first audio frame presented at
    // or after each, at 48 kHz.
    let audio = index.track(Rendition::Audio).unwrap();
    let starts: Vec<u64> = audio.segments().iter().map(|s| s.start).collect();
    assert_eq!(starts, [0, 2 * 48_000, 3 * 48_000]);
    assert_eq!(audio.codecs, "mp4a.40.2");
    // The playlists.
    let master = index.master_playlist();
    assert!(master.contains("#EXT-X-INDEPENDENT-SEGMENTS"));
    assert!(master.contains("CODECS=\"avc1.64001E,mp4a.40.2\""));
    assert!(master.contains("AUDIO=\"audio\""));
    assert!(master.contains("RESOLUTION=320x240,FRAME-RATE=25.000"));
    let playlist = index.media_playlist(Rendition::Video).unwrap();
    assert!(playlist.contains("#EXT-X-TARGETDURATION:2\n"));
    assert!(playlist.contains("#EXTINF:2.000000,\nvideo-0.m4s\n#EXTINF:1.000000,\nvideo-1.m4s\n"));
}

#[test]
fn targets_shorter_than_a_gop_cut_at_every_sync_sample() {
    let source = av();
    let index = assert_round_trip(&source, &options(q(1, 10)));
    assert_eq!(index.track(Rendition::Video).unwrap().segments().len(), 4);
    // A target longer than the movie gives one segment per rendition.
    let index = assert_round_trip(&source, &options(q(60, 1)));
    for track in index.tracks() {
        assert_eq!(track.segments().len(), 1);
    }
}

/// Open GOPs of 20 frames at 25 frames per second: the first GOP is closed
/// (I P B B ...); every later one starts I B B in decode order, the two B
/// frames presented before the I frame and after the previous GOP. Every
/// offset includes a two-frame delay, which the edit list removes.
fn open_gop_video(gops: usize) -> TrackSpec {
    let mut display = Vec::new();
    for g in 0..gops {
        let base = 20 * g;
        let order: Vec<usize> = if g == 0 {
            let mut o = vec![0];
            for p in (3..18).step_by(3) {
                o.extend([p, p - 2, p - 1]);
            }
            o.extend([19, 16, 17, 18]);
            o.truncate(20);
            o
        } else {
            let mut o = vec![2, 0, 1];
            for p in (5..20).step_by(3) {
                o.extend([p, p - 2, p - 1]);
            }
            o.extend([19, 18]);
            o.truncate(20);
            o
        };
        display.extend(order.into_iter().map(|d| base + d));
    }
    let mut sorted = display.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..20 * gops).collect::<Vec<_>>());
    let samples = display
        .iter()
        .enumerate()
        .map(|(n, d)| S {
            size: 30 + (n as u32 % 7),
            duration: 512,
            offset: (*d as i32 + 2 - n as i32) * 512,
            sync: n % 20 == 0,
        })
        .collect();
    TrackSpec {
        video: true,
        timescale: 12_800,
        edits: vec![((20 * gops) as u32 * 40, 1024)],
        samples,
        entry: *b"avc1",
    }
}

#[test]
fn open_gops_start_segments_at_their_leading_samples() {
    let video = open_gop_video(3);
    let source = movie(&[video]);
    let index = assert_round_trip(&source, &options(q(1, 2)));
    let track = index.track(Rendition::Video).unwrap();
    assert!(!track.independent);
    assert!(!index.master_playlist().contains("INDEPENDENT"));
    // Each later segment starts at its first leading sample, presented
    // before its sync sample.
    let src_movie = probe(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let src = packets(&source, &src_movie, 0);
    for (n, entry) in track.segments().iter().enumerate().skip(1) {
        let first: usize = track.segments()[..n]
            .iter()
            .map(|s| s.samples as usize)
            .sum();
        let sync = src[first].pts.unwrap();
        assert!(q(entry.start.into(), 1).checked_lt(sync).unwrap());
    }
}

#[test]
fn audio_alone() {
    let source = movie(&[aac_audio(200, vec![(4000, 2112)])]);
    let index = assert_round_trip(&source, &options(q(1, 1)));
    assert_eq!(index.tracks().len(), 1);
    let audio = index.track(Rendition::Audio).unwrap();
    assert!(audio.segments().len() >= 4);
    let master = index.master_playlist();
    assert!(master.contains("CODECS=\"mp4a.40.2\""));
    assert!(master.contains("\naudio.m3u8\n"));
    assert!(!master.contains("EXT-X-MEDIA"));
    // A dropped track that is not video (a chapter text track in an
    // audiobook, say) leaves the default at audio alone; found in the
    // corpus.
    let mut other = aac_audio(200, vec![(4000, 2112)]);
    other.entry = *b"text";
    let source = movie(&[aac_audio(200, vec![(4000, 2112)]), other]);
    let index = assert_round_trip(&source, &options(q(1, 1)));
    assert_eq!(index.tracks().len(), 1);
    assert!(index.track(Rendition::Audio).is_some());
    // Chapter images: a dropped video track (JPEG) that the audio's `chap`
    // reference names does not count either; any other dropped video
    // track does (an encrypted one, say). Found in the corpus.
    let mut images = ibbp_video(10, 1);
    images.entry = *b"jpeg";
    let source = movie(&[aac_audio(200, vec![(4000, 2112)]), images]);
    let refused = build_index(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &options(q(1, 1)),
    );
    assert!(refused.is_err());
    let index = assert_round_trip(&with_chapter_reference(&source, 2), &options(q(1, 1)));
    assert_eq!(index.tracks().len(), 1);
    assert!(index.track(Rendition::Audio).is_some());
    // Asked for no video with a video track present.
    let index = assert_round_trip(
        &av(),
        &HlsOptions {
            video: None,
            ..options(q(1, 1))
        },
    );
    assert!(index.track(Rendition::Video).is_none());
}

/// `source` (ftyp, moov, mdat) with its movie box changed by `change`, the
/// movie box's size and every chunk offset moved to match.
fn grow_moov(source: &[u8], change: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let at = source.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let size = u32::from_be_bytes(source[at..at + 4].try_into().unwrap()) as usize;
    let mut moov = source[at..at + size].to_vec();
    change(&mut moov);
    let grown = (moov.len() - size) as u32;
    let moov_size = moov.len() as u32;
    moov[..4].copy_from_slice(&moov_size.to_be_bytes());
    // Every chunk offset moves by the bytes added before the media data.
    let mut from = 0;
    while let Some(i) = moov[from..].windows(4).position(|w| w == b"stco") {
        let stco = from + i + 4;
        let count = u32::from_be_bytes(moov[stco + 4..stco + 8].try_into().unwrap()) as usize;
        for n in 0..count {
            let entry = stco + 8 + 4 * n;
            let offset = u32::from_be_bytes(moov[entry..entry + 4].try_into().unwrap());
            moov[entry..entry + 4].copy_from_slice(&(offset + grown).to_be_bytes());
        }
        from = stco;
    }
    [&source[..at], &moov[..], &source[at + size..]].concat()
}

/// `source` with an `mvex` box for `tracks` tracks: a movie with fragments
/// (none of them present), where an edit may last to the end of the media.
fn with_movie_extends(source: &[u8], tracks: u32) -> Vec<u8> {
    grow_moov(source, |moov| {
        let mut trex = Vec::new();
        for id in 1..=tracks {
            trex.extend_from_slice(&32u32.to_be_bytes());
            trex.extend_from_slice(b"trex");
            trex.extend_from_slice(&[0; 4]);
            for value in [id, 1, 0, 0, 0] {
                trex.extend_from_slice(&value.to_be_bytes());
            }
        }
        moov.extend_from_slice(&(trex.len() as u32 + 8).to_be_bytes());
        moov.extend_from_slice(b"mvex");
        moov.extend_from_slice(&trex);
    })
}

/// `source` with a `chap` track reference from its first track to track
/// `chapters` (QuickTime chapter lists).
fn with_chapter_reference(source: &[u8], chapters: u32) -> Vec<u8> {
    grow_moov(source, |moov| {
        let trak = moov.windows(4).position(|w| w == b"trak").unwrap() - 4;
        let size = u32::from_be_bytes(moov[trak..trak + 4].try_into().unwrap());
        let mut tref = Vec::new();
        tref.extend_from_slice(&20u32.to_be_bytes());
        tref.extend_from_slice(b"tref");
        tref.extend_from_slice(&12u32.to_be_bytes());
        tref.extend_from_slice(b"chap");
        tref.extend_from_slice(&chapters.to_be_bytes());
        let end = trak + size as usize;
        moov.splice(end..end, tref);
        moov[trak..trak + 4].copy_from_slice(&(size + 20).to_be_bytes());
    })
}

#[test]
fn edits_that_last_to_the_end_of_the_media() {
    // A media edit of duration 0 lasts to the end of the media (14496-12
    // 8.6.6), as fragmented recordings store it; found in the corpus. The
    // video is presented as with an edit exactly as long as the media,
    // and the init segments keep the duration of 0.
    let mut video = ibbp_video(100, 25);
    video.edits = vec![(0, 1024)];
    let to_end = with_movie_extends(&movie(&[video, aac_audio(190, vec![(0, 2112)])]), 2);
    let index = assert_round_trip(&to_end, &options(q(1, 1)));
    let full = assert_round_trip(&av(), &options(q(1, 1)));
    assert_eq!(
        index.media_playlist(Rendition::Video),
        full.media_playlist(Rendition::Video)
    );
    for rendition in [Rendition::Video, Rendition::Audio] {
        let init = index.init_segment(rendition).unwrap();
        let movie = probe(
            SliceSource::new(&init),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap();
        assert!(
            matches!(
                movie.tracks[0].edit_list,
                EditList::Simple { segment: 0, .. }
            ),
            "{rendition:?}: {:?}",
            movie.tracks[0].edit_list
        );
    }
    // After a leading empty edit as well: the empty edit becomes a decode
    // time shift, as for a media edit with a duration, and the media edit
    // keeps its duration of 0.
    let late = |edits: Vec<(u32, i32)>| {
        let mut video = ibbp_video(100, 25);
        video.edits = edits;
        movie(&[video])
    };
    let to_end = with_movie_extends(&late(vec![(500, -1), (0, 1024)]), 1);
    let full = late(vec![(500, -1), (4000, 1024)]);
    let build = |source: &[u8]| {
        build_index(
            SliceSource::new(source),
            &Limits::default(),
            &Allowlist::default(),
            &options(q(1, 1)),
        )
        .unwrap()
    };
    let (a, b) = (build(&to_end), build(&full));
    let open = |bytes: &[u8]| HlsIndex::open(SliceSource::new(bytes), &Limits::default()).unwrap();
    let (index, expected) = (open(&a), open(&b));
    assert_eq!(
        index.media_playlist(Rendition::Video),
        expected.media_playlist(Rendition::Video)
    );
    let segments = index.track(Rendition::Video).unwrap().segments().len();
    for n in 0..segments as u32 {
        let segment = |index: &HlsIndex, bytes: &[u8], source: &[u8]| {
            index
                .media_segment(
                    Rendition::Video,
                    n,
                    SliceSource::new(bytes),
                    SliceSource::new(source),
                    &Limits::default(),
                )
                .unwrap()
                .header()
                .to_vec()
        };
        assert_eq!(
            segment(&index, &a, &to_end),
            segment(&expected, &b, &full),
            "segment {n}"
        );
    }
    let init = index.init_segment(Rendition::Video).unwrap();
    let movie = probe(
        SliceSource::new(&init),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert!(matches!(
        movie.tracks[0].edit_list,
        EditList::Simple {
            empty: 0,
            media_time: 1024,
            segment: 0
        }
    ));
}

#[test]
fn video_alone_and_refusals() {
    let source = av();
    let index = assert_round_trip(
        &source,
        &HlsOptions {
            audio: AudioChoice::None,
            ..options(q(1, 1))
        },
    );
    assert_eq!(index.tracks().len(), 1);
    assert!(!index.master_playlist().contains("AUDIO="));
    let refuse = |options: &HlsOptions| {
        build_index(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
            options,
        )
        .unwrap_err()
    };
    assert!(matches!(
        refuse(&HlsOptions {
            video: Some(VideoChoice::Stream(1)),
            ..options(q(1, 1))
        }),
        ExtractError::Source(_)
    ));
    assert!(matches!(refuse(&options(q(0, 1))), ExtractError::Source(_)));
    assert!(matches!(
        refuse(&HlsOptions {
            source_tag: vec![0; 256],
            ..options(q(1, 1))
        }),
        ExtractError::Source(_)
    ));
    // Several edits: refused until the packager takes them on
    // (docs/design/edit-list-writers.md, common decision 1); nothing is
    // written before the index exists.
    let mut video = ibbp_video(100, 25);
    video.edits = vec![(1000, 1024), (2000, 1024 + 25_600)];
    let spliced = movie(&[video]);
    match build_index(
        SliceSource::new(&spliced),
        &Limits::default(),
        &Allowlist::default(),
        &options(q(1, 1)),
    ) {
        Err(ExtractError::Source(Error::Unsupported { reason })) => assert_eq!(
            reason,
            "edit list with several edits (multi-edit), not supported here yet"
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_late_stream_keeps_its_delay() {
    // The audio starts 0.5 s after the video; its priming is skipped.
    let source = movie(&[
        ibbp_video(100, 25),
        aac_audio(170, vec![(500, -1), (3500, 2112)]),
    ]);
    assert_round_trip(&source, &options(q(1, 1)));
    // The video starts 0.3 s after the audio, which has no priming edit.
    let mut video = ibbp_video(100, 25);
    video.edits = vec![(300, -1), (4000, 1024)];
    let source = movie(&[video, aac_audio(200, vec![])]);
    let index = assert_round_trip(&source, &options(q(1, 1)));
    // Nothing is presented during the empty edit, so it became a decode
    // time shift: 0.3 s at 12,800 Hz.
    let video = index.track(Rendition::Video).unwrap();
    assert_eq!(video.segments()[0].base_decode_time, 3_840);
}

/// Found by the `hls` fuzz target: a source can mark AAC frames as not
/// sync samples; segments still start only at sync samples.
#[test]
fn audio_marked_not_sync_never_starts_a_segment() {
    let mut audio = aac_audio(190, vec![(4000, 2112)]);
    for (n, sample) in audio.samples.iter_mut().enumerate() {
        sample.sync = n % 4 == 0;
    }
    let source = movie(&[ibbp_video(100, 25), audio]);
    assert_round_trip(&source, &options(q(1, 1)));
    let source = movie(&[aac_audio(190, vec![]), ibbp_video(100, 25)]);
    let mut first_not_sync = aac_audio(190, vec![]);
    first_not_sync.samples[0].sync = false;
    let audio_only = movie(&[first_not_sync]);
    assert_round_trip(&source, &options(q(1, 1)));
    assert!(matches!(
        build_index(
            SliceSource::new(&audio_only),
            &Limits::default(),
            &Allowlist::default(),
            &options(q(1, 1))
        ),
        Err(ExtractError::Source(_))
    ));
}

/// A trimmed source: both edits start 1.5 s into the media. The video is
/// copied from the sync sample at 1 s; the audio keeps every sample, so a
/// reader that counts the edit's media time from the first decode time
/// (the pinned ffmpeg does) still presents all of it.
#[test]
fn an_edit_inside_the_media_leaves_the_video_lead_out() {
    let mut video = ibbp_video(100, 25);
    video.edits = vec![(2_500, 1_024 + 19_200)];
    let audio = aac_audio(190, vec![(2_500, 2_112 + 72_000)]);
    let source = movie(&[video, audio]);
    let index = assert_round_trip(&source, &options(q(1, 1)));
    let video = index.track(Rendition::Video).unwrap();
    assert_eq!(video.samples, 75);
    let audio = index.track(Rendition::Audio).unwrap();
    assert_eq!(audio.samples, 190);
}

/// The edit starts exactly at a sync sample's composition time, or inside
/// its display interval: the video is copied from that sample, the
/// latest sync sample at or before the edit's start, not from the GOP
/// before it.
#[test]
fn an_edit_at_a_sync_sample_starts_there() {
    for start in [12_800 + 1_024, 12_800 + 1_024 + 200] {
        let mut video = ibbp_video(100, 25);
        video.edits = vec![(3_000, start)];
        let audio = aac_audio(190, vec![(3_000, 2_112 + 48_000)]);
        let source = movie(&[video, audio]);
        let index = assert_round_trip(&source, &options(q(1, 1)));
        let video = index.track(Rendition::Video).unwrap();
        assert_eq!(video.samples, 75, "edit at media time {start}");
        let audio = index.track(Rendition::Audio).unwrap();
        assert_eq!(audio.samples, 190, "edit at media time {start}");
    }
}

/// `udta`/`meta`/`ilst` with one iTunes `iTunSMPB` item holding `text`,
/// appended to the movie box; chunk offsets move with the media data.
fn with_smpb(movie: &[u8], text: &str) -> Vec<u8> {
    let data = common::cat(&[common::u32s(&[1, 0]), text.as_bytes().to_vec()]);
    let item = common::bx(
        b"----",
        &common::cat(&[
            common::full(b"mean", 0, 0, b"com.apple.iTunes"),
            common::full(b"name", 0, 0, b"iTunSMPB"),
            common::bx(b"data", &data),
        ]),
    );
    let mut hdlr = common::u32s(&[0]);
    hdlr.extend_from_slice(b"mdirappl");
    hdlr.extend_from_slice(&[0u8; 9]);
    let meta = common::full(
        b"meta",
        0,
        0,
        &common::cat(&[
            common::full(b"hdlr", 0, 0, &hdlr),
            common::bx(b"ilst", &item),
        ]),
    );
    let udta = common::bx(b"udta", &meta);
    let at = movie.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let len = u32::from_be_bytes(movie[at..at + 4].try_into().unwrap()) as usize;
    let mut out = movie[..at + len].to_vec();
    out.extend_from_slice(&udta);
    out.extend_from_slice(&movie[at + len..]);
    out[at..at + 4].copy_from_slice(&((len + udta.len()) as u32).to_be_bytes());
    let mut from = at;
    while let Some(i) = out[from..at + len].windows(4).position(|w| w == b"stco") {
        let entry = from + i + 12;
        let offset = u32::from_be_bytes(out[entry..entry + 4].try_into().unwrap());
        out[entry..entry + 4].copy_from_slice(&(offset + udta.len() as u32).to_be_bytes());
        from = entry;
    }
    out
}

/// An audio track without an edit list whose `iTunSMPB` item adds up to
/// its samples: the item becomes the rendition's edit list, as for clips
/// and extraction.
#[test]
fn a_matching_gapless_item_becomes_the_audio_edit_list() {
    // 100 frames of 1024 = 102,400 = 2,112 delay + 100,000 + 288 padding.
    let plain = movie(&[aac_audio(100, vec![])]);
    let source = with_smpb(
        &plain,
        " 00000000 00000840 00000120 00000000000186A0 00000000",
    );
    let index_bytes = build_index(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &options(q(1, 1)),
    )
    .unwrap();
    let index = HlsIndex::open(SliceSource::new(&index_bytes), &Limits::default()).unwrap();
    let audio = index.track(Rendition::Audio).unwrap();
    assert_eq!(audio.samples, 100);
    assert_eq!(audio.end, 100_000);
    let (out, _) = concatenation(&index_bytes, &index, &source, Rendition::Audio);
    let out_movie = probe(
        SliceSource::new(&out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(
        out_movie.tracks[0].edit_list,
        EditList::Simple {
            empty: 0,
            media_time: 2_112,
            segment: 100_000
        }
    );
    // A tag that does not add up is ignored.
    let source = with_smpb(
        &plain,
        " 00000000 00000840 00000120 00000000000186A1 00000000",
    );
    let index = assert_round_trip(&source, &options(q(1, 1)));
    assert_eq!(index.track(Rendition::Audio).unwrap().end, 102_400);
}

/// Found by review: negative offsets stored in a version 0 table, without
/// an edit list. They are copied as stored, and planned with as signed,
/// as players read them, so the rendition lasts its four frames rather than
/// 2^32 ticks.
#[test]
fn unsigned_offsets_meant_as_negative_plan_as_signed() {
    let mut source = common::movie(&common::Spec {
        composition: vec![512, 1024, -512, 0],
        elst: None,
        ..common::Spec::default()
    });
    let at = source.windows(4).position(|w| w == b"ctts").unwrap();
    source[at + 4] = 0;
    let index_bytes = build_index(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &options(q(1, 1)),
    )
    .unwrap();
    let index = HlsIndex::open(SliceSource::new(&index_bytes), &Limits::default()).unwrap();
    let video = index.track(Rendition::Video).unwrap();
    assert_eq!(video.samples, 4);
    // The latest frame ends at 2,048 ticks (decode time 1,536, offset 512).
    assert_eq!(video.end, 2_048);
    let playlist = index.media_playlist(Rendition::Video).unwrap();
    assert!(playlist.contains("#EXT-X-TARGETDURATION:1\n"));
    // The offsets are copied as stored, in a version 0 run.
    let (out, _) = concatenation(&index_bytes, &index, &source, Rendition::Video);
    let trun = out.windows(4).position(|w| w == b"trun").unwrap();
    assert_eq!(out[trun + 4], 0);
}

/// Found by review: one short sample must not set FRAME-RATE. The last
/// sample's duration is left out; a stray short one elsewhere leaves the
/// attribute out.
#[test]
fn a_stray_short_sample_does_not_set_the_frame_rate() {
    let mut video = ibbp_video(100, 25);
    video.samples[99].duration = 1;
    let index = assert_round_trip(&movie(&[video]), &options(q(1, 1)));
    assert!(index.master_playlist().contains("FRAME-RATE=25.000"));
    let mut video = ibbp_video(100, 25);
    video.samples[50].duration = 1;
    video.samples[51].duration = 1023;
    let index = assert_round_trip(&movie(&[video]), &options(q(1, 1)));
    assert!(!index.master_playlist().contains("FRAME-RATE"));
}

/// Found by the corpus run: a video track without a sync sample table, so
/// every sample is a sync sample by ISO/IEC 14496-12, whose bitstream has
/// IDR pictures at samples 0 and 50 only. Segments start at IDR pictures.
#[test]
fn every_sample_marked_sync_cuts_at_idr_pictures() {
    let mut video = ibbp_video(100, 25);
    for sample in &mut video.samples {
        sample.sync = true;
        sample.offset = 0;
    }
    video.edits = vec![];
    let mut source = movie(&[video]);
    let src_movie = probe(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    for packet in packets(&source, &src_movie, 0) {
        // One NAL unit filling the sample: IDR (5) or a non-IDR slice (1).
        let at = packet.offset as usize;
        let len = packet.size - 4;
        source[at..at + 4].copy_from_slice(&len.to_be_bytes());
        source[at + 4] = if packet.sample % 50 == 0 { 0x65 } else { 0x41 };
    }
    let index = assert_round_trip(&source, &options(q(1, 1)));
    let video = index.track(Rendition::Video).unwrap();
    let starts: Vec<u64> = video.segments().iter().map(|s| s.start).collect();
    assert_eq!(starts, [0, 2 * 12_800]);
}

/// Found by review: the same stream trimmed by an edit that starts at frame
/// 60 is copied from the IDR picture at frame 50, not from the frame before
/// the edit that the missing sync table also marks sync.
#[test]
fn a_trimmed_stream_marked_sync_starts_at_an_idr_picture() {
    let mut video = ibbp_video(100, 25);
    for sample in &mut video.samples {
        sample.sync = true;
        sample.offset = 0;
    }
    video.edits = vec![(1_600, 60 * 512)];
    let mut source = movie(&[video]);
    let src_movie = probe(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    for packet in packets(&source, &src_movie, 0) {
        let at = packet.offset as usize;
        let len = packet.size - 4;
        source[at..at + 4].copy_from_slice(&len.to_be_bytes());
        source[at + 4] = if packet.sample % 50 == 0 { 0x65 } else { 0x41 };
    }
    let index = assert_round_trip(&source, &options(q(1, 1)));
    assert_eq!(index.track(Rendition::Video).unwrap().samples, 50);
}

#[test]
fn source_tag_and_changed_source() {
    let source = av();
    let opts = HlsOptions {
        source_tag: b"\"etag-1\"".to_vec(),
        ..options(q(1, 1))
    };
    let index_bytes = build_index(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &opts,
    )
    .unwrap();
    let index = HlsIndex::open(SliceSource::new(&index_bytes), &Limits::default()).unwrap();
    assert_eq!(index.source_tag(), b"\"etag-1\"");
    let segment = index
        .media_segment(
            Rendition::Video,
            1,
            SliceSource::new(&index_bytes),
            SliceSource::new(&source),
            &Limits::default(),
        )
        .unwrap();
    let mut longer = source.clone();
    longer.push(0);
    assert_eq!(
        segment.write(SliceSource::new(&longer), Vec::new()),
        Err(ExtractError::SourceChanged)
    );
    // Past the last segment, and a rendition the index lacks.
    let count = index.track(Rendition::Video).unwrap().segments().len() as u32;
    assert!(
        index
            .media_segment(
                Rendition::Video,
                count,
                SliceSource::new(&index_bytes),
                SliceSource::new(&source),
                &Limits::default()
            )
            .is_err()
    );
    let video_only = build_index(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &HlsOptions {
            audio: AudioChoice::None,
            ..options(q(1, 1))
        },
    )
    .unwrap();
    let video_only = HlsIndex::open(SliceSource::new(&video_only), &Limits::default()).unwrap();
    assert!(video_only.init_segment(Rendition::Audio).is_err());
    assert!(video_only.media_playlist(Rendition::Audio).is_none());
}

/// Every one-byte change and every truncation of an index either fails to
/// open or to cut a segment, or gives segments whose ranges lie inside
/// the source; nothing panics.
#[test]
fn damaged_indexes_fail_cleanly() {
    let source = av();
    let index_bytes = build_index(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &options(q(1, 1)),
    )
    .unwrap();
    let limits = Limits::default();
    let mut opened = 0;
    let check = |bytes: &[u8]| -> bool {
        let Ok(index) = HlsIndex::open(SliceSource::new(bytes), &limits) else {
            return false;
        };
        let _ = index.master_playlist();
        for track in index.tracks() {
            let _ = index.media_playlist(track.rendition);
            let _ = index.init_segment(track.rendition);
            for n in 0..track.segments().len() as u32 {
                let Ok(segment): Result<MediaSegment, _> = index.media_segment(
                    track.rendition,
                    n,
                    SliceSource::new(bytes),
                    SliceSource::new(&source),
                    &limits,
                ) else {
                    continue;
                };
                for range in segment.ranges() {
                    assert!(range.end() <= index.source_len());
                }
                if index.source_len() == source.len() as u64 {
                    let mut out = Vec::new();
                    segment.write(SliceSource::new(&source), &mut out).unwrap();
                    assert_eq!(out.len() as u64, segment.len());
                }
            }
        }
        true
    };
    assert!(check(&index_bytes));
    for at in 0..index_bytes.len() {
        for flip in [0x01u8, 0x80, 0xff] {
            let mut damaged = index_bytes.clone();
            damaged[at] ^= flip;
            opened += usize::from(check(&damaged));
        }
        assert!(!check(&index_bytes[..at]) || at == index_bytes.len());
    }
    // Most bytes are sample records and directory values whose changes stay
    // within the checks; the header's are refused.
    assert!(opened > 0);
    let mut wrong_magic = index_bytes.clone();
    wrong_magic[0] = b'X';
    assert!(matches!(
        HlsIndex::open(SliceSource::new(&wrong_magic), &limits),
        Err(ExtractError::InvalidIndex { .. })
    ));
}

#[test]
fn indexes_respect_the_table_limit() {
    let source = av();
    let tight = Limits {
        max_table_bytes: 64,
        ..Limits::default()
    };
    assert!(matches!(
        build_index(
            SliceSource::new(&source),
            &tight,
            &Allowlist::default(),
            &options(q(1, 1))
        ),
        Err(ExtractError::Source(
            coreav_isobmff::Error::LimitExceeded { .. }
        ))
    ));
}

/// Every MP4, MOV and M4A fixture the packager accepts, at targets of 1 s
/// and 6 s. The fixtures named in `REQUIRED` must be accepted.
#[test]
#[ignore = "needs COREAV_FIXTURES"]
fn fixtures_round_trip() {
    const REQUIRED: &[&str] = &[
        "basic.mp4",
        "basic.mov",
        "av1.mp4",
        "vp9.mp4",
        "hevc_hvc1.mp4",
        "negative_cts.mp4",
        "edit_list.mp4",
        "vfr.mp4",
        "rotate_ccw90.mp4",
        "fragmented.mp4",
        "fragmented_sidx.mp4",
        "fragmented_many.mp4",
        "clip_open_gop.mp4",
        "clip_audio_late.mp4",
        "clip_video_late.mp4",
        "hls_h264.mp4",
        "hls_open_gop.mp4",
        "hls_hevc.mp4",
        "hls_long_gop.mp4",
        "audio_aac.m4a",
    ];
    let dir = std::env::var("COREAV_FIXTURES").expect("COREAV_FIXTURES");
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
    let mut packaged = std::collections::BTreeSet::new();
    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read(path).unwrap();
        for seconds in [q(1, 1), q(6, 1)] {
            let result = build_index(
                SliceSource::new(&source),
                &Limits::default(),
                &Allowlist::default(),
                &options(seconds),
            );
            match result {
                Ok(_) => {
                    let index = assert_round_trip(&source, &options(seconds));
                    packaged.insert(name.clone());
                    eprintln!(
                        "{name} at {seconds:?} s: {:?}",
                        index
                            .tracks()
                            .iter()
                            .map(|t| (t.rendition.name(), t.segments().len()))
                            .collect::<Vec<_>>()
                    );
                }
                Err(e) => {
                    assert!(!REQUIRED.contains(&name.as_str()), "{name}: {e}");
                    eprintln!("{name}: refused: {e}");
                }
            }
        }
    }
    for name in REQUIRED {
        if generated.contains(&format!("generated {name}")) {
            assert!(packaged.contains(*name), "{name} was not packaged");
        }
    }
}

/// An AAC LC, 48 kHz stereo `esds` (ISO/IEC 14496-1 7.2.6.5, 14496-3
/// 1.6.2.1).
fn esds() -> Vec<u8> {
    common::full(
        b"esds",
        0,
        0,
        &[
            3, 25, 0, 1, 0, // ES_Descriptor
            4, 17, 0x40, 0x15, 0, 0, 0, 0, 0, 0xfa, 0, 0, 0, 0xfa,
            0, // DecoderConfigDescriptor
            5, 2, 0x11, 0x90, // AudioSpecificConfig
            6, 1, 2, // SLConfigDescriptor
        ],
    )
}

/// A fragmented AAC source: `fragments` movie fragments of
/// `samples` samples each (1024 ticks and 10 bytes apiece, from `trex`),
/// with a segment index before the first one when `sap` is given
/// (`starts_with_SAP` and the `SAP_type`). `tfdt` is written unless
/// `decode_times` is false. `listed` is how many fragments the segment
/// index names, `None` for all of them.
fn fragmented_aac_sidx(
    fragments: usize,
    samples: u32,
    sap: Option<(bool, u8)>,
    decode_times: bool,
) -> Vec<u8> {
    fragmented_aac_sidx_over(fragments, samples, sap, decode_times, None)
}

fn fragmented_aac_sidx_over(
    fragments: usize,
    samples: u32,
    sap: Option<(bool, u8)>,
    decode_times: bool,
    listed: Option<usize>,
) -> Vec<u8> {
    fragmented_aac_sidx_tracks(fragments, samples, sap, decode_times, listed, 1)
}

/// The same with `tracks` `trex` entries: more than one says a fragment
/// may hold another track.
fn fragmented_aac_sidx_tracks(
    fragments: usize,
    samples: u32,
    sap: Option<(bool, u8)>,
    decode_times: bool,
    listed: Option<usize>,
    tracks: u32,
) -> Vec<u8> {
    use common::{bx, cat, full, u32s};
    const TIMESCALE: u32 = 48_000;
    const SAMPLE_BYTES: u32 = 10;
    const SAMPLE_TICKS: u32 = 1024;
    let ftyp = bx(b"ftyp", b"iso6\0\0\x02\0");
    let mut mvhd = u32s(&[0, 0, 1000, 0]);
    mvhd.extend_from_slice(&[0u8; 80]);
    let mut tkhd = u32s(&[0, 0, 0, 0, 0]);
    tkhd.extend_from_slice(&[0u8; 16]);
    tkhd.extend_from_slice(&u32s(&[
        0x10000,
        0,
        0,
        0,
        0x10000,
        0,
        0,
        0,
        0x4000_0000,
        0,
        0,
    ]));
    let mut mdhd = u32s(&[0, 0, TIMESCALE, 0]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut entry = vec![0u8; 6];
    entry.extend_from_slice(&1u16.to_be_bytes());
    entry.extend_from_slice(&[0u8; 8]);
    entry.extend_from_slice(&2u16.to_be_bytes());
    entry.extend_from_slice(&16u16.to_be_bytes());
    entry.extend_from_slice(&[0u8; 4]);
    entry.extend_from_slice(&(TIMESCALE << 16).to_be_bytes());
    entry.extend_from_slice(&esds());
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&bx(b"mp4a", &entry));
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(b"stsd", 0, 0, &stsd),
            full(b"stts", 0, 0, &u32s(&[0])),
            full(b"stsc", 0, 0, &u32s(&[0])),
            full(b"stsz", 0, 0, &u32s(&[0, 0])),
            full(b"stco", 0, 0, &u32s(&[0])),
        ]),
    );
    let dref = full(
        b"dref",
        0,
        0,
        &[u32s(&[1]), full(b"url ", 0, 1, &[])].concat(),
    );
    let minf = bx(b"minf", &cat(&[bx(b"dinf", &dref), stbl]));
    let mdia = bx(
        b"mdia",
        &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
    );
    let trak = |id: u32| {
        let mut header = tkhd.clone();
        if let Some(field) = header.get_mut(8..12) {
            field.copy_from_slice(&id.to_be_bytes());
        }
        bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &header), mdia.clone()]))
    };
    let moov = bx(
        b"moov",
        &cat(&[
            vec![full(b"mvhd", 0, 0, &mvhd)],
            (1..=tracks).map(trak).collect::<Vec<_>>(),
            vec![bx(
                b"mvex",
                &cat(&(1..=tracks)
                    .map(|id| {
                        full(
                            b"trex",
                            0,
                            0,
                            &u32s(&[id, 1, SAMPLE_TICKS, SAMPLE_BYTES, 0]),
                        )
                    })
                    .collect::<Vec<_>>()),
            )],
        ]
        .concat()),
    );
    let fragment = |n: usize| -> Vec<u8> {
        let tfhd = full(b"tfhd", 0, 0x02_0000, &u32s(&[1]));
        let tfdt = if decode_times {
            full(b"tfdt", 0, 0, &u32s(&[n as u32 * samples * SAMPLE_TICKS]))
        } else {
            Vec::new()
        };
        let traf_len = 8 + tfhd.len() + tfdt.len() + 20;
        let moof_len = 8 + 16 + traf_len;
        let trun = full(b"trun", 0, 0x1, &u32s(&[samples, (moof_len + 8) as u32]));
        let moof = bx(
            b"moof",
            &cat(&[
                full(b"mfhd", 0, 0, &u32s(&[n as u32 + 1])),
                bx(b"traf", &cat(&[tfhd, tfdt, trun])),
            ]),
        );
        assert_eq!(moof.len(), moof_len);
        let mdat = bx(
            b"mdat",
            &vec![n as u8 + 1; (samples * SAMPLE_BYTES) as usize],
        );
        cat(&[moof, mdat])
    };
    let sizes: Vec<u32> = (0..fragments).map(|n| fragment(n).len() as u32).collect();
    let sidx = sap.map(|(starts, kind)| {
        let named = listed.unwrap_or(fragments).min(fragments);
        let mut payload = u32s(&[1, TIMESCALE, 0, 0]);
        payload.extend_from_slice(&0u16.to_be_bytes());
        payload.extend_from_slice(&(named as u16).to_be_bytes());
        for size in sizes.get(..named).unwrap_or_default() {
            payload.extend_from_slice(&size.to_be_bytes());
            payload.extend_from_slice(&(samples * SAMPLE_TICKS).to_be_bytes());
            let sap = (u32::from(starts) << 31) | (u32::from(kind) << 28);
            payload.extend_from_slice(&sap.to_be_bytes());
        }
        full(b"sidx", 0, 0, &payload)
    });
    let mut parts = vec![ftyp, moov];
    parts.extend(sidx);
    parts.extend((0..fragments).map(fragment));
    cat(&parts)
}

/// With a segment index, the segments are the subsegments it lists, and a
/// request reads the fragment rather than records in the index.
#[test]
fn a_segment_index_gives_the_segments() {
    let source = fragmented_aac_sidx(5, 8, Some((true, 1)), true);
    // Eight samples of 1024 ticks at 48 kHz is 0.17 s, far under the
    // target: the grid would make one segment of all of them.
    let index = assert_round_trip(&source, &options(q(6, 1)));
    let track = index.track(Rendition::Audio).unwrap();
    assert_eq!(track.segments().len(), 5);
    assert!(track.deferred.is_some());
    assert!(track.independent);
    for (n, entry) in track.segments().iter().enumerate() {
        assert_eq!(entry.samples, 0);
        assert_eq!(entry.start, n as u64 * 8 * 1024);
    }
    // Without the segment index the grid applies, and every sample of the
    // source lands in one segment.
    let listed = fragmented_aac_sidx(5, 8, None, true);
    let plain = assert_round_trip(&listed, &options(q(6, 1)));
    let track = plain.track(Rendition::Audio).unwrap();
    assert_eq!(track.segments().len(), 1);
    assert!(track.deferred.is_none());
    assert_eq!(track.samples, 40);
}

/// A segment index that names only the first subsegments leaves the rest
/// of the media out, so the source is listed: `ffmpeg -movflags +dash`
/// writes one index per subsegment, and probe reads only the first.
#[test]
fn a_segment_index_that_stops_short_is_listed() {
    let source = fragmented_aac_sidx_over(5, 8, Some((true, 1)), true, Some(1));
    let index = assert_round_trip(&source, &options(q(6, 1)));
    let track = index.track(Rendition::Audio).unwrap();
    assert!(track.deferred.is_none());
    assert_eq!(track.samples, 40);
}

/// A movie whose fragments may hold a second track is listed: the
/// subsegment's bytes, which the bit rates come from, would be more than
/// the packaged track's.
#[test]
fn a_source_whose_fragments_may_hold_another_track_is_listed() {
    // A second `trex`, for a track the file has no samples of: the movie
    // then says a fragment may hold two tracks.
    let two = fragmented_aac_sidx_tracks(5, 8, Some((true, 1)), true, None, 2);
    let index = assert_round_trip(&two, &options(q(6, 1)));
    let track = index.track(Rendition::Audio).unwrap();
    assert!(track.deferred.is_none());
    assert_eq!(track.samples, 40);
}

/// A `sidx` that marks no access point, one that marks a gradual refresh
/// (`SAP_type` 4), and fragments without a decode time are listed as
/// usual: the segment index does not say what the playlist needs.
#[test]
fn segment_indexes_the_playlist_cannot_use_are_listed() {
    for source in [
        fragmented_aac_sidx(5, 8, Some((false, 1)), true),
        fragmented_aac_sidx(5, 8, Some((true, 4)), true),
        fragmented_aac_sidx(5, 8, Some((true, 1)), false),
    ] {
        let index = assert_round_trip(&source, &options(q(6, 1)));
        let track = index.track(Rendition::Audio).unwrap();
        assert!(track.deferred.is_none());
        assert_eq!(track.samples, 40);
    }
}

/// An access point whose type the segment index does not state (0) or
/// states as an open GOP (3) still gives the segments, without
/// `EXT-X-INDEPENDENT-SEGMENTS`: the subsegments partition the
/// presentation timeline by their earliest presentation times, but a
/// player cannot start at any one of them alone.
#[test]
fn segments_of_an_unstated_access_point_are_not_independent() {
    for kind in [0, 3] {
        let source = fragmented_aac_sidx(5, 8, Some((true, kind)), true);
        let index = assert_round_trip(&source, &options(q(6, 1)));
        let track = index.track(Rendition::Audio).unwrap();
        assert!(track.deferred.is_some(), "SAP type {kind}");
        assert_eq!(track.segments().len(), 5);
        assert!(!track.independent);
        let playlist = index.media_playlist(Rendition::Audio).unwrap();
        assert!(!playlist.contains("#EXT-X-INDEPENDENT-SEGMENTS"));
    }
}

/// A deferred index whose entry names bytes that are not a fragment of the
/// track fails the request cleanly, and a source of another length is
/// refused before it is read.
#[test]
fn a_deferred_request_checks_the_source() {
    let source = fragmented_aac_sidx(5, 8, Some((true, 1)), true);
    let index_bytes = build_index(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &options(q(6, 1)),
    )
    .unwrap();
    let index = HlsIndex::open(SliceSource::new(&index_bytes), &Limits::default()).unwrap();
    let mut longer = source.clone();
    longer.push(0);
    assert_eq!(
        index.media_segment(
            Rendition::Audio,
            0,
            SliceSource::new(&index_bytes),
            SliceSource::new(&longer),
            &Limits::default()
        ),
        Err(ExtractError::SourceChanged)
    );
    // The second fragment's box type, which its entry names.
    let mut damaged = source.clone();
    let at = damaged
        .windows(4)
        .enumerate()
        .filter(|(_, window)| *window == b"moof")
        .map(|(at, _)| at)
        .nth(1)
        .unwrap();
    damaged[at] = b'x';
    assert!(
        index
            .media_segment(
                Rendition::Audio,
                1,
                SliceSource::new(&index_bytes),
                SliceSource::new(&damaged),
                &Limits::default()
            )
            .is_err()
    );
}
