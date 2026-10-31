//! Probe and packet results on small files built from the specification.

mod common;

use common::*;
use coreav_isobmff::{
    Allowlist, Budget, Codec, EditList, Limits, Rational, SliceSource, TrackKind, Warning, probe,
};

#[test]
fn probe_reports_track_metadata() {
    let data = movie(&Spec::default());
    let movie = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(movie.major_brand.map(|b| b.0), Some(*b"isom"));
    assert_eq!(movie.timescale, MOVIE_TIMESCALE);
    let track = &movie.tracks[0];
    assert_eq!(track.kind, TrackKind::Video);
    assert_eq!(track.description.codec, Codec::H264);
    assert_eq!(
        (track.description.width, track.description.height),
        (Some(320), Some(240))
    );
    assert_eq!(track.rotation, Some(0));
    assert_eq!(track.language, Some(*b"und"));
    assert_eq!(track.handler_name.as_deref(), Some("VideoHandler"));
    assert_eq!(track.sample_count, 4);
    assert_eq!(track.sample_duration, 2048);
    assert_eq!(
        track.edit_list,
        EditList::Simple {
            empty: 0,
            media_time: 1024,
            segment: 80
        }
    );
    // The media edit (80 ms = 1,024 ticks) caps mdhd (2,048).
    assert_eq!(track.duration, Some(Rational::from_int(1024)));
    assert!(track.warnings.is_empty(), "{:?}", track.warnings);
}

#[test]
fn packets_follow_the_tables_and_the_edit_list() {
    let data = movie(&Spec::default());
    let limits = Limits::default();
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    let mut budget = Budget::new(&limits);
    let packets: Vec<_> = movie
        .packets(0, SliceSource::new(&data), &limits, &mut budget)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(packets.len(), 4);
    let offsets: Vec<u64> = packets.iter().map(|p| p.offset).collect();
    assert_eq!(offsets, vec![28, 128, 178, 238]);
    let dts: Vec<i64> = packets.iter().map(|p| p.dts).collect();
    assert_eq!(dts, vec![0, 512, 1024, 1536]);
    let cts: Vec<i64> = packets.iter().map(|p| p.cts).collect();
    assert_eq!(cts, vec![1024, 2560, 1024, 2048]);
    let pts: Vec<_> = packets.iter().map(|p| p.pts.unwrap().round()).collect();
    assert_eq!(pts, vec![0, 1536, 0, 1024]);
    let keyframes: Vec<bool> = packets.iter().map(|p| p.keyframe).collect();
    assert_eq!(keyframes, vec![true, false, false, false]);
    // Window [1024, 2048): samples starting at 2048 and later are not presented.
    let presented: Vec<bool> = packets.iter().map(|p| p.presented).collect();
    assert_eq!(presented, vec![true, false, true, false]);
}

#[test]
fn unsupported_edit_lists_have_no_presentation_times() {
    // The second edit presents media the first one already did.
    let spec = Spec {
        elst: Some(vec![(40, 0), (40, 256)]),
        ..Spec::default()
    };
    let data = movie(&spec);
    let limits = Limits::default();
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    let track = &movie.tracks[0];
    assert_eq!(track.edit_list, EditList::Unsupported);
    assert!(track.warnings.contains(&Warning::UnsupportedEditList));
    // The entries are kept as stored all the same.
    let entries: Vec<(u64, i64, i32)> = track
        .edits
        .iter()
        .map(|e| (e.segment_duration, e.media_time, e.media_rate))
        .collect();
    assert_eq!(entries, vec![(40, 0, 0x1_0000), (40, 256, 0x1_0000)]);
    assert_eq!(track.duration, Some(Rational::from_int(2048)));
    let mut budget = Budget::new(&limits);
    let first = movie
        .packets(0, SliceSource::new(&data), &limits, &mut budget)
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(first.pts, None);
}

/// The presentation times, decode times on the presentation timeline and
/// presented flags of track 0's packets.
#[allow(clippy::unwrap_used)]
fn timeline(data: &[u8]) -> Vec<(Option<i128>, Option<i128>, bool)> {
    let limits = Limits::default();
    let movie = probe(SliceSource::new(data), &limits, &Allowlist::default()).unwrap();
    let mut budget = Budget::new(&limits);
    movie
        .packets(0, SliceSource::new(data), &limits, &mut budget)
        .unwrap()
        .map(|p| {
            let p = p.unwrap();
            (
                p.pts.map(Rational::round),
                p.presentation_dts.map(Rational::round),
                p.presented,
            )
        })
        .collect()
}

#[test]
fn several_media_edits_are_applied_exactly() {
    // 40 ms is 512 ticks at 12,800 Hz: windows [1024, 1536) and
    // [2048, 2560), presented from 0 and from 512. Composition times are
    // 1024, 2560, 1024 and 2048; decode times 0, 512, 1024 and 1536.
    let spec = Spec {
        elst: Some(vec![(40, 1024), (40, 2048)]),
        ..Spec::default()
    };
    let data = movie(&spec);
    let limits = Limits::default();
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    let track = &movie.tracks[0];
    assert_eq!(track.edit_list, EditList::Multiple);
    assert!(!track.warnings.contains(&Warning::UnsupportedEditList));
    // The duration is what the media edits present, 2 × 512 ticks.
    assert_eq!(track.duration, Some(Rational::from_int(1024)));
    assert_eq!(
        timeline(&data),
        vec![
            (Some(0), Some(-1024), true),
            // Starts at the end of the last window: mapped by it, not shown.
            (Some(1024), Some(-1024), false),
            (Some(0), Some(0), true),
            (Some(512), Some(0), true),
        ]
    );
}

#[test]
fn a_sample_in_a_gap_takes_the_next_edit() {
    // Windows [0, 256) and [1280, 1792), presented from 0 and from 256:
    // the sample at 1024 starts in the gap and is still shown by the second
    // edit, before its start; the one at 512 is in the gap entirely.
    let spec = Spec {
        composition: vec![0, 0, 0, 0],
        elst: Some(vec![(20, 0), (40, 1280)]),
        ..Spec::default()
    };
    assert_eq!(
        timeline(&movie(&spec)),
        vec![
            (Some(0), Some(0), true),
            (Some(-512), Some(-512), false),
            (Some(0), Some(0), true),
            (Some(512), Some(512), true),
        ]
    );
}

#[test]
fn empty_edits_between_media_edits_move_the_next_one() {
    // 10 ms of empty edit (128 ticks) between [0, 512) and [1024, 1536).
    let spec = Spec {
        composition: vec![0, 0, 0, 0],
        elst: Some(vec![(40, 0), (10, -1), (40, 1024)]),
        ..Spec::default()
    };
    let data = movie(&spec);
    let limits = Limits::default();
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    assert_eq!(movie.tracks[0].edit_list, EditList::Multiple);
    assert_eq!(
        timeline(&data),
        vec![
            (Some(0), Some(0), true),
            (Some(128), Some(128), false),
            (Some(640), Some(640), true),
            (Some(1152), Some(1152), false),
        ]
    );
}

#[test]
fn empty_edits_between_media_edits_are_inside_the_span() {
    // [0, 512) shown from 0, 10 ms empty, [1024, 1536) shown from 640: the
    // stream lasts 1024 ticks but ends at 1152 (decision 5).
    let spec = Spec {
        composition: vec![0, 0, 0, 0],
        elst: Some(vec![(40, 0), (10, -1), (40, 1024)]),
        ..Spec::default()
    };
    let data = movie(&spec);
    let movie = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let track = &movie.tracks[0];
    assert_eq!(track.duration, Some(Rational::from_int(1024)));
    let (start, end) = track.presentation(movie.timescale).unwrap();
    assert_eq!(start, Rational::from_int(0));
    assert_eq!(end, Rational::new(1152, 12_800).unwrap());
}

#[test]
fn several_edits_in_a_fragmented_movie_stay_unsupported() {
    // A second media edit added to the fragmented movie's edit list: the
    // model was measured without fragments only (decision 1).
    let mut data = fragmented_movie(None, Some((40, 0)));
    let elst = data.windows(4).position(|w| w == b"elst").unwrap() - 4;
    let entries = elst + 16;
    let second = u32s(&[40, 1024, 0x10000]);
    data.splice(entries + 12..entries + 12, second);
    data[entries - 4..entries].copy_from_slice(&2u32.to_be_bytes());
    for kind in [b"elst", b"edts", b"trak", b"moov"] {
        let at = data.windows(4).position(|w| w == kind).unwrap() - 4;
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap());
        data[at..at + 4].copy_from_slice(&(size + 12).to_be_bytes());
    }
    let movie = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(movie.tracks[0].edits.len(), 2);
    assert_eq!(movie.tracks[0].edit_list, EditList::Unsupported);
    assert!(
        movie.tracks[0]
            .warnings
            .contains(&Warning::UnsupportedEditList)
    );
}

#[test]
fn edit_lists_outside_the_model_stay_unsupported() {
    let outside = [
        // Backwards.
        vec![(40, 1024), (40, 0)],
        // A dwell, then a media edit.
        vec![(40, 0), (40, 1024)],
        // A media edit of duration 0 among several.
        vec![(0, 0), (40, 1024)],
        // A media time below -1.
        vec![(40, -2), (40, 1024)],
    ];
    for (i, elst) in outside.into_iter().enumerate() {
        let mut data = movie(&Spec {
            elst: Some(elst),
            ..Spec::default()
        });
        if i == 1 {
            // The first entry's rate: 0 (a dwell).
            let at = data.windows(4).position(|w| w == b"elst").unwrap() + 4 + 8 + 8;
            data[at..at + 4].copy_from_slice(&0u32.to_be_bytes());
        }
        let limits = Limits::default();
        let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
        assert_eq!(movie.tracks[0].edit_list, EditList::Unsupported, "case {i}");
        assert!(
            timeline(&data).iter().all(|(pts, ..)| pts.is_none()),
            "case {i}"
        );
    }
}

#[test]
fn empty_edit_shifts_presentation_exactly() {
    // 33 ms of empty edit at 12,800 Hz is 422.4 ticks: kept exact.
    let spec = Spec {
        elst: Some(vec![(33, -1), (160, 0)]),
        ..Spec::default()
    };
    let data = movie(&spec);
    let limits = Limits::default();
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    let offset = movie.tracks[0]
        .presentation_offset(movie.timescale)
        .unwrap();
    assert_eq!(offset, Rational::new(2112, 5).unwrap());
    let mut budget = Budget::new(&limits);
    let first = movie
        .packets(0, SliceSource::new(&data), &limits, &mut budget)
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(first.pts, Rational::new(1024 * 5 + 2112, 5));
}

#[test]
fn no_edit_list_and_no_sync_table() {
    let spec = Spec {
        elst: None,
        sync: vec![],
        ..Spec::default()
    };
    let data = movie(&spec);
    let limits = Limits::default();
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    assert_eq!(movie.tracks[0].duration, Some(Rational::from_int(2048)));
    let mut budget = Budget::new(&limits);
    let all_key = movie
        .packets(0, SliceSource::new(&data), &limits, &mut budget)
        .unwrap()
        .all(|p| p.is_ok_and(|p| p.keyframe && p.presented));
    assert!(all_key);
}

#[test]
fn fragmented_probe_reports_declared_or_unknown_durations() {
    use coreav_isobmff::DurationSource;
    let limits = Limits::default();
    // No mehd, no edit: unknown until scanned.
    let data = fragmented_movie(None, None);
    let mut movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    assert!(movie.fragmented);
    assert_eq!(movie.fragment_duration, None);
    assert_eq!(movie.tracks[0].duration, None);
    assert_eq!(movie.tracks[0].duration_source, DurationSource::Unknown);
    assert_eq!(movie.duration_seconds(), None);
    movie
        .scan_fragments(SliceSource::new(&data), &limits)
        .unwrap();
    assert_eq!(movie.tracks[0].duration, Some(Rational::from_int(2048)));
    assert_eq!(movie.tracks[0].duration_source, DurationSource::Observed);
    assert_eq!(
        movie.duration_seconds(),
        Rational::new(2048, u64::from(TIMESCALE))
    );

    // mehd gives the movie duration, never a track duration.
    let data = fragmented_movie(Some(160), None);
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    assert_eq!(movie.fragment_duration, Some(160));
    assert_eq!(
        movie.duration_seconds(),
        Rational::new(160, u64::from(MOVIE_TIMESCALE))
    );
    assert_eq!(movie.tracks[0].duration, None);

    // A simple media edit declares the track duration (80 ms = 1,024 ticks).
    let data = fragmented_movie(None, Some((80, 0)));
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    assert_eq!(movie.tracks[0].duration, Some(Rational::from_int(1024)));
    assert_eq!(movie.tracks[0].duration_source, DurationSource::MediaEdit);
}

#[test]
fn truncated_fragments_fail_when_read() {
    let data = fragmented_movie(None, None);
    let cut = &data[..data.len() - 50];
    let limits = Limits::default();
    // Probe stops at the first fragment header, so it succeeds.
    let mut movie = probe(SliceSource::new(cut), &limits, &Allowlist::default()).unwrap();
    assert!(
        movie
            .scan_fragments(SliceSource::new(cut), &limits)
            .is_err()
    );
    let mut budget = Budget::new(&limits);
    let listed: Result<Vec<_>, _> = movie
        .packets(0, SliceSource::new(cut), &limits, &mut budget)
        .unwrap()
        .collect();
    assert!(listed.is_err());
}

#[test]
fn header_durations_are_ignored_when_fragments_exist() {
    use coreav_isobmff::DurationSource;
    let mut data = fragmented_movie(None, None);
    // Give mvhd and mdhd non-zero durations, as when the first fragment's
    // samples stay in the movie box: they describe only those samples.
    let mvhd = u32s(&[0, 0, MOVIE_TIMESCALE, 0]);
    let at = data
        .windows(mvhd.len())
        .position(|w| w == mvhd.as_slice())
        .unwrap();
    data[at + 12..at + 16].copy_from_slice(&40u32.to_be_bytes());
    let mdhd = u32s(&[0, 0, TIMESCALE, 0]);
    let at = data
        .windows(mdhd.len())
        .position(|w| w == mdhd.as_slice())
        .unwrap();
    data[at + 12..at + 16].copy_from_slice(&512u32.to_be_bytes());
    let limits = Limits::default();
    let mut movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    assert!(movie.has_fragments());
    assert_eq!(movie.tracks[0].mdhd_duration, 512);
    assert_eq!(movie.tracks[0].duration_source, DurationSource::Unknown);
    assert_eq!(movie.duration_seconds(), None);
    movie
        .scan_fragments(SliceSource::new(&data), &limits)
        .unwrap();
    assert_eq!(movie.tracks[0].duration, Some(Rational::from_int(2048)));
    assert_eq!(
        movie.duration_seconds(),
        Rational::new(2048, u64::from(TIMESCALE))
    );
}

#[test]
fn disallowed_audio_is_dropped_and_video_kept() {
    // Video plus DTS audio: DTS is not on the default allowlist.
    let data = av_movie(b"dtsc");
    let limits = Limits::default();
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    assert_eq!(movie.tracks.len(), 1);
    let video = &movie.tracks[0];
    assert_eq!((video.index, video.stream_index, video.track_id), (0, 0, 1));
    assert_eq!(video.description.codec, Codec::H264);
    assert_eq!(movie.dropped.len(), 1);
    let audio = movie.dropped[0];
    assert_eq!((audio.stream_index, audio.track_id), (1, 2));
    assert_eq!(audio.fourcc.0, *b"dtsc");
    // Only the kept stream is reachable; its packets are the video samples.
    let mut budget = Budget::new(&limits);
    let offsets: Vec<u64> = movie
        .packets(0, SliceSource::new(&data), &limits, &mut budget)
        .unwrap()
        .map(|p| p.unwrap().offset)
        .collect();
    assert_eq!(offsets, vec![28, 128, 228, 328]);
    assert!(
        movie
            .packets(1, SliceSource::new(&data), &limits, &mut budget)
            .is_err()
    );
    // Strict mode rejects the file.
    let strict = Allowlist::default().strict();
    assert!(matches!(
        probe(SliceSource::new(&data), &limits, &strict),
        Err(coreav_isobmff::Error::CodecNotAllowed { track_id: 2, .. })
    ));
    // An allowed audio codec keeps both streams, in file order.
    let both = av_movie(b"sowt");
    let movie = probe(SliceSource::new(&both), &limits, &Allowlist::default()).unwrap();
    assert_eq!(movie.tracks.len(), 2);
    assert!(movie.dropped.is_empty());
    assert_eq!(movie.tracks[1].stream_index, 1);
}

/// The corpus file large.mp4 (H.264 video and AC-3 audio) is private and not
/// in the repository; set COREAV_CORPUS_DIR to run this check locally.
#[test]
#[ignore = "needs the private corpus; set COREAV_CORPUS_DIR"]
fn corpus_large_mp4_keeps_ac3() {
    let dir = std::env::var("COREAV_CORPUS_DIR").expect("COREAV_CORPUS_DIR");
    let path = std::path::Path::new(&dir).join("large.mp4");
    let limits = Limits::default();
    let source = coreav_isobmff::FileSource::open(&path).unwrap();
    let movie = probe(source, &limits, &Allowlist::default()).unwrap();
    let kept: Vec<_> = movie
        .tracks
        .iter()
        .map(|t| (t.stream_index, t.description.codec))
        .collect();
    let dropped: Vec<_> = movie
        .dropped
        .iter()
        .map(|d| (d.stream_index, d.fourcc.to_string()))
        .collect();
    println!("kept {kept:?} dropped {dropped:?}");
    assert!(
        movie
            .tracks
            .iter()
            .any(|t| t.description.codec == Codec::H264)
    );
    assert!(
        movie
            .tracks
            .iter()
            .any(|t| t.description.codec == Codec::Ac3)
    );
}

#[test]
fn implicit_base_follows_the_preceding_track_fragment() {
    let (data, mdat) = implicit_base_fragments();
    let limits = Limits::default();
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    let offsets = |track: usize| -> Vec<(u64, u32)> {
        let mut budget = Budget::new(&limits);
        movie
            .packets(track, SliceSource::new(&data), &limits, &mut budget)
            .unwrap()
            .map(|p| p.map(|p| (p.offset, p.size)).unwrap())
            .collect()
    };
    assert_eq!(
        offsets(0),
        (0..4).map(|i| (mdat + 100 * i, 100)).collect::<Vec<_>>()
    );
    // Track 2 starts where track 1's 400 bytes end, not inside the moof.
    assert_eq!(
        offsets(1),
        (0..3)
            .map(|i| (mdat + 400 + 50 * i, 50))
            .collect::<Vec<_>>()
    );
}

#[test]
fn undeterminable_implicit_base_is_an_error() {
    let (mut data, _) = implicit_base_fragments();
    // Give the first traf an unknown track ID: its default sample size is then
    // unknown, so the second traf's implicit base cannot be determined.
    let tfhd = full(b"tfhd", 0, 0, &u32s(&[1]));
    let at = data
        .windows(tfhd.len())
        .position(|w| w == tfhd.as_slice())
        .unwrap();
    let id = at + tfhd.len() - 4;
    data[id..id + 4].copy_from_slice(&9u32.to_be_bytes());
    let limits = Limits::default();
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    let mut budget = Budget::new(&limits);
    let listed: Result<Vec<_>, _> = movie
        .packets(1, SliceSource::new(&data), &limits, &mut budget)
        .unwrap()
        .collect();
    assert!(listed.is_err());
}

#[test]
fn box_order_inside_mdia_does_not_change_the_result() {
    let limits = Limits::default();
    let results: Vec<_> = [false, true]
        .into_iter()
        .map(|minf_first| {
            let data = av_movie_with(aac_entry(), minf_first);
            let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
            assert!(movie.dropped.is_empty(), "minf first: {minf_first}");
            movie
                .tracks
                .iter()
                .map(|t| {
                    let d = &t.description;
                    (d.codec, d.width, d.height, d.channels, d.sample_rate)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(results[0], results[1]);
    assert_eq!(
        results[1][0],
        (Codec::H264, Some(320), Some(240), None, None)
    );
    assert_eq!(
        results[1][1],
        (Codec::Aac, None, None, Some(2), Some(48_000))
    );
}

#[test]
fn derived_presentation_duration_next_to_the_declared_one() {
    let limits = Limits::default();
    let seconds = |ticks: i128, scale: u32| Rational::new(ticks, u64::from(scale)).unwrap();
    // The default movie declares 160 ms (the whole media) in its movie
    // header, but its edit list presents 80 ms from media time 1,024.
    let data = movie(&Spec::default());
    let parsed = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    assert_eq!(
        parsed.duration_seconds(),
        Some(seconds(160, MOVIE_TIMESCALE))
    );
    assert_eq!(
        parsed.presentation_duration_seconds(),
        Some(seconds(1024, TIMESCALE))
    );
    // Without an edit list, a stream starts at its first composition time.
    let data = movie(&Spec {
        elst: None,
        ..Spec::default()
    });
    let parsed = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    assert_eq!(
        parsed.tracks[0].presentation(parsed.timescale),
        Some((seconds(1024, TIMESCALE), seconds(1024 + 2048, TIMESCALE)))
    );
    // An empty edit is rounded to the stream's ticks: 10 ms at 12,800 Hz
    // is 128 ticks exactly; 1 movie tick at 1,000 Hz is 12.8, so 13.
    for (empty, start) in [(10, 128), (1, 13)] {
        let data = movie(&Spec {
            elst: Some(vec![(empty, -1), (80, 1024)]),
            ..Spec::default()
        });
        let parsed = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
        assert_eq!(
            parsed.tracks[0].presentation(parsed.timescale),
            Some((seconds(start, TIMESCALE), seconds(start + 1024, TIMESCALE)))
        );
    }
}

#[test]
fn dropped_streams_count_in_the_derived_duration() {
    // The audio stream is dropped; its media header and edit list still
    // place it on the timeline.
    let data = av_movie(b"sowt");
    let parsed = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::new(&[Codec::H264]),
    )
    .unwrap();
    assert_eq!(parsed.dropped.len(), 1);
    let dropped = &parsed.dropped[0];
    assert_eq!(
        (dropped.timescale, dropped.mdhd_duration),
        (TIMESCALE, 2048)
    );
    assert_eq!(dropped.edit_list, EditList::None);
    let end = Rational::new(2048, u64::from(TIMESCALE)).unwrap();
    assert_eq!(
        dropped.presentation(parsed.timescale, false),
        Some((Rational::from_int(0), end))
    );
    assert_eq!(parsed.presentation_duration_seconds(), Some(end));
}

#[test]
fn dropped_streams_of_fragmented_movies_ignore_the_media_header() {
    // The media header of a fragmented movie describes only the movie box
    // samples: a dropped stream counts with its media edit
    // only, as a kept one does without a fragment scan.
    for (edit, expected) in [
        (None, None),
        (Some((80, 0)), Rational::new(80, u64::from(MOVIE_TIMESCALE))),
    ] {
        let mut data = fragmented_movie(Some(5000), edit);
        let at = data.windows(4).position(|w| w == b"mdhd").unwrap() + 20;
        // 512 ticks, shorter than the 1,024-tick media edit: a cap by the
        // media header would change both cases.
        data[at..at + 4].copy_from_slice(&512u32.to_be_bytes());
        let parsed = probe(
            SliceSource::new(&data),
            &Limits::default(),
            &Allowlist::new(&[Codec::Aac]),
        )
        .unwrap();
        assert_eq!(parsed.dropped.len(), 1);
        assert_eq!(parsed.dropped[0].mdhd_duration, 512);
        assert_eq!(parsed.presentation_duration_seconds(), expected);
        let kept = probe(
            SliceSource::new(&data),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap();
        assert_eq!(kept.presentation_duration_seconds(), expected);
    }
}

#[test]
fn open_ended_media_edits_last_to_the_end_of_fragmented_media() {
    use coreav_isobmff::DurationSource;
    let limits = Limits::default();
    // A media edit of duration 0 from media time 512: the four fragment
    // samples of 512 ticks start at composition time 0, so the first is
    // before the edit and the stream lasts 3 × 512 ticks.
    let data = fragmented_movie(None, Some((0, 512)));
    let mut movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    assert_eq!(
        movie.tracks[0].edit_list,
        EditList::Simple {
            empty: 0,
            media_time: 512,
            segment: 0
        }
    );
    assert_eq!(movie.tracks[0].duration, None);
    assert_eq!(movie.tracks[0].duration_source, DurationSource::Unknown);
    movie
        .scan_fragments(SliceSource::new(&data), &limits)
        .unwrap();
    assert_eq!(movie.tracks[0].duration, Some(Rational::from_int(1536)));
    assert_eq!(
        movie.presentation_duration_seconds(),
        Rational::new(1536, u64::from(TIMESCALE))
    );
    let mut budget = Budget::new(&limits);
    let packets: Vec<_> = movie
        .packets(0, SliceSource::new(&data), &limits, &mut budget)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let presented: Vec<bool> = packets.iter().map(|p| p.presented).collect();
    assert_eq!(presented, [false, true, true, true]);
    let pts: Vec<Option<Rational>> = packets.iter().map(|p| p.pts).collect();
    assert_eq!(
        pts,
        [-512, 0, 512, 1024].map(|t| Some(Rational::from_int(t)))
    );

    // A dropped stream with such an edit has no known end: no sample
    // tables or fragments are read for it.
    let data = fragmented_movie(Some(5000), Some((0, 512)));
    let parsed = probe(
        SliceSource::new(&data),
        &limits,
        &Allowlist::new(&[Codec::Aac]),
    )
    .unwrap();
    assert_eq!(parsed.dropped[0].presentation(parsed.timescale, true), None);
    assert_eq!(parsed.presentation_duration_seconds(), None);
}

#[test]
fn open_ended_media_edits_are_unsupported_without_fragments() {
    // ISO/IEC 14496-12 8.6.6 gives a duration of 0 its meaning only in a
    // movie with fragments.
    let data = movie(&Spec {
        elst: Some(vec![(0, 1024)]),
        ..Spec::default()
    });
    let parsed = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(parsed.tracks[0].edit_list, EditList::Unsupported);
    assert_eq!(parsed.tracks[0].warnings, [Warning::UnsupportedEditList]);
}

#[test]
fn version_0_composition_offsets_are_signed_in_quicktime_files() {
    // A version 0 `ctts` holding -512 as stored by QuickTime writers: the
    // QuickTime File Format makes the offsets signed, ISO/IEC 14496-12
    // version 0 unsigned.
    let mut data = movie(&Spec {
        composition: vec![0, 1024, -512, 512],
        elst: None,
        ..Spec::default()
    });
    let ctts = data.windows(4).position(|w| w == b"ctts").unwrap() + 4;
    data[ctts] = 0;
    let cts = |data: &[u8]| -> Vec<i64> {
        let limits = Limits::default();
        let parsed = probe(SliceSource::new(data), &limits, &Allowlist::default()).unwrap();
        let mut budget = Budget::new(&limits);
        parsed
            .packets(0, SliceSource::new(data), &limits, &mut budget)
            .unwrap()
            .map(|p| p.unwrap().cts)
            .collect()
    };
    // `isom`: unsigned, as stored.
    assert_eq!(cts(&data)[2], 1024 + (1 << 32) - 512);
    let parsed = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(parsed.tracks[0].ctts_signed(), Some(false));
    // `qt  `: signed.
    let brand = data.windows(4).position(|w| w == b"ftyp").unwrap() + 4;
    data[brand..brand + 4].copy_from_slice(b"qt  ");
    assert_eq!(cts(&data), [0, 512 + 1024, 1024 - 512, 1536 + 512]);
    let parsed = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(parsed.tracks[0].ctts_signed(), Some(true));
}

#[test]
fn version_0_composition_offsets_are_signed_in_quicktime_files_without_ftyp() {
    let mut data = movie(&Spec {
        composition: vec![0, 1024, -512, 512],
        elst: None,
        ..Spec::default()
    });
    let ctts = data.windows(4).position(|w| w == b"ctts").unwrap() + 4;
    data[ctts] = 0;
    let signed = |data: &[u8]| {
        probe(
            SliceSource::new(data),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap()
        .tracks[0]
            .ctts_signed()
    };
    // The QuickTime component type `mhlr` in an ISO file keeps the ISO
    // reading.
    let component = data.windows(4).position(|w| w == b"hdlr").unwrap() + 8;
    data[component..component + 4].copy_from_slice(b"mhlr");
    assert_eq!(signed(&data), Some(false));
    // Without `ftyp`, it makes the file a QuickTime file: signed.
    let ftyp = data.windows(4).position(|w| w == b"ftyp").unwrap();
    data[ftyp..ftyp + 4].copy_from_slice(b"free");
    assert_eq!(signed(&data), Some(true));
    // Without `ftyp` and with `pre_defined` 0: the ISO reading.
    data[component..component + 4].copy_from_slice(&[0; 4]);
    assert_eq!(signed(&data), Some(false));
}

#[test]
fn dropped_streams_of_fragmented_movies_take_their_fragments_timing() {
    let limits = Limits::default();
    // The fragment starts at decode time 7 (`tfdt` version 0).
    let mut data = fragmented_movie(None, None);
    let tfdt = data.windows(4).position(|w| w == b"tfdt").unwrap() + 8;
    data[tfdt..tfdt + 4].copy_from_slice(&7u32.to_be_bytes());
    let mut kept = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    kept.scan_fragments(SliceSource::new(&data), &limits)
        .unwrap();
    assert_eq!(kept.tracks[0].fragment_start, Some(7));
    // Dropped: probe knows no timing and asks for a scan; the scan gives the
    // fragments' four samples of 512 ticks and their start.
    let mut dropped = probe(
        SliceSource::new(&data),
        &limits,
        &Allowlist::new(&[Codec::Aac]),
    )
    .unwrap();
    assert!(dropped.needs_fragment_scan());
    assert_eq!(dropped.presentation_duration_seconds(), None);
    dropped
        .scan_fragments(SliceSource::new(&data), &limits)
        .unwrap();
    assert!(!dropped.needs_fragment_scan());
    let stream = &dropped.dropped[0];
    assert_eq!(
        (stream.fragment_duration, stream.fragment_start),
        (Some(2048), Some(7))
    );
    assert_eq!(
        dropped.presentation_duration_seconds(),
        Rational::new(2048, u64::from(TIMESCALE))
    );
    // With an edit that lasts to the end of the media from media time 512,
    // the first sample is cut, as for a kept stream.
    let data = fragmented_movie(None, Some((0, 512)));
    let mut dropped = probe(
        SliceSource::new(&data),
        &limits,
        &Allowlist::new(&[Codec::Aac]),
    )
    .unwrap();
    dropped
        .scan_fragments(SliceSource::new(&data), &limits)
        .unwrap();
    assert_eq!(
        dropped.presentation_duration_seconds(),
        Rational::new(1536, u64::from(TIMESCALE))
    );
}

#[test]
fn the_ffprobe_format_span_leaves_out_text_that_ends_a_second_after_the_video() {
    // The second track becomes a text track `factor` times longer than the
    // 160 ms video: 0.48 s ends less than a second after the video and
    // counts; 1.6 s ends 1.44 s after it and does not. The presentation
    // spans it either way.
    for (factor, counted) in [(3, true), (10, false)] {
        let mut data = av_movie(b"mp4a");
        let soun = data.windows(4).position(|w| w == b"soun").unwrap();
        data[soun..soun + 4].copy_from_slice(b"text");
        let mdhd = data
            .windows(4)
            .enumerate()
            .filter(|(_, w)| *w == b"mdhd")
            .nth(1)
            .unwrap()
            .0;
        // After the type: version and flags, two times, the timescale.
        let at = mdhd + 4 + 4 + 8 + 4;
        let stored = u32::from_be_bytes(data[at..at + 4].try_into().unwrap());
        data[at..at + 4].copy_from_slice(&(stored * factor).to_be_bytes());
        let parsed = probe(
            SliceSource::new(&data),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap();
        let spans = parsed.presentation_spans();
        let length = |kind| {
            let s = spans.iter().find(|s| s.kind == kind).unwrap();
            s.end
                .checked_add(Rational::new(-s.start.num, s.start.den).unwrap())
                .unwrap()
        };
        assert_eq!(
            parsed.presentation_duration_seconds(),
            Some(length(TrackKind::Subtitle))
        );
        let expected = if counted {
            length(TrackKind::Subtitle)
        } else {
            length(TrackKind::Video)
        };
        assert_eq!(
            parsed.ffprobe_format_duration_seconds(),
            Some(expected),
            "{factor}"
        );
    }
}

#[test]
fn an_open_ended_edit_before_the_first_fragment_cuts_nothing() {
    // The edit starts at media time 512; the fragments start at decode time
    // 1,024 (`tfdt`), after it, so no sample lies before the edit: the
    // stream lasts its four samples of 512 ticks, kept or dropped.
    let limits = Limits::default();
    let mut data = fragmented_movie(None, Some((0, 512)));
    let tfdt = data.windows(4).position(|w| w == b"tfdt").unwrap() + 8;
    data[tfdt..tfdt + 4].copy_from_slice(&1024u32.to_be_bytes());
    for allowlist in [Allowlist::default(), Allowlist::new(&[Codec::Aac])] {
        let mut parsed = probe(SliceSource::new(&data), &limits, &allowlist).unwrap();
        parsed
            .scan_fragments(SliceSource::new(&data), &limits)
            .unwrap();
        let spans = parsed.presentation_spans();
        let s = spans.first().unwrap();
        assert_eq!(
            s.end
                .checked_add(Rational::new(-s.start.num, s.start.den).unwrap()),
            Rational::new(2048, u64::from(TIMESCALE))
        );
    }
}

#[test]
fn the_ffprobe_format_duration_in_microseconds_counts_whole_ticks() {
    // A media edit of 1 movie tick (1 ms) is 12.8 ticks at 12,800 Hz: 13
    // whole ticks, 1,015.625 µs, so 1,016 µs; the exact span is 1,000 µs.
    let data = movie(&Spec {
        elst: Some(vec![(1, 0)]),
        ..Spec::default()
    });
    let parsed = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(
        parsed.ffprobe_format_duration_seconds(),
        Rational::new(1, u64::from(MOVIE_TIMESCALE))
    );
    assert_eq!(parsed.ffprobe_format_duration_micros(), Some(1016));
}
