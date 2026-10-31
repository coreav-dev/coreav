//! Subtitles to WebVTT and chapters on synthetic movies (3GPP TS 26.245,
//! W3C WebVTT, QuickTime File Format).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[path = "../../coreav-isobmff/tests/common/text_movie.rs"]
mod text_movie;

use coreav_isobmff::{Codec, Error, Limits, Rational, SliceSource};
use coreav_text::{
    MODIFIERS, OTHER_MODIFIER, SubtitleSummary, TextError, TextLimitKind, TextLimits, TrackChoice,
    extract_subtitles, read_chapters,
};
use text_movie::{TrackSpec, bx, movie, styl, text_sample};

fn extract(data: &[u8], choice: TrackChoice) -> Result<(SubtitleSummary, String), TextError> {
    extract_with(data, choice, &TextLimits::default())
}

fn extract_with(
    data: &[u8],
    choice: TrackChoice,
    limits: &TextLimits,
) -> Result<(SubtitleSummary, String), TextError> {
    let mut out = Vec::new();
    let summary = extract_subtitles(
        SliceSource::new(data),
        &Limits::default(),
        limits,
        choice,
        &mut out,
    )?;
    assert_eq!(summary.bytes_written, out.len() as u64);
    Ok((summary, String::from_utf8(out).unwrap()))
}

fn slot(kind: &[u8; 4]) -> usize {
    MODIFIERS.iter().position(|k| *k == kind).unwrap()
}

fn subtitles(samples: Vec<(Vec<u8>, u32)>) -> Vec<u8> {
    movie(&[
        TrackSpec::video(1, &[1000, 1000]),
        TrackSpec::tx3g(2, samples),
    ])
}

#[test]
fn cues_with_gaps_styling_and_escapes() {
    let data = subtitles(vec![
        (text_sample(b"", &[]), 250),
        (text_sample(b"Hello <world>", &[styl(6, 13)]), 650),
        (text_sample(b"", &[]), 100),
        (text_sample(b"a & b --> c\r\nnext", &[]), 1000),
    ]);
    let (summary, vtt) = extract(&data, TrackChoice::First).unwrap();
    assert_eq!(
        vtt,
        "WEBVTT\n\n\
         00:00:00.250 --> 00:00:00.900\nHello &lt;world&gt;\n\n\
         00:00:01.000 --> 00:00:02.000\na &amp; b --&gt; c\nnext\n\n"
    );
    assert_eq!(summary.stream_index, 1);
    assert_eq!(summary.track_id, 2);
    assert_eq!(summary.codec, Codec::TimedText);
    assert_eq!(summary.samples, 4);
    assert_eq!(summary.cues, 2);
    assert_eq!(summary.dropped.empty, 2);
    assert_eq!(summary.dropped.styling[slot(b"styl")], 1);
}

#[test]
fn text_encodings_and_line_breaks() {
    let utf16be = [
        &[0xFE, 0xFF][..],
        &"Ü\u{2028}é"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<_>>(),
    ]
    .concat();
    let utf16le = [
        &[0xFF, 0xFE][..],
        &"lé"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    ]
    .concat();
    let data = subtitles(vec![
        (text_sample(&utf16be, &[]), 100),
        (text_sample(&utf16le, &[]), 100),
        (text_sample(b"bad \xFF byte\rcr\xC2\x85nel\0", &[]), 100),
        (text_sample("\u{FEFF}bom".as_bytes(), &[]), 100),
    ]);
    let (summary, vtt) = extract(&data, TrackChoice::First).unwrap();
    assert_eq!(
        vtt,
        "WEBVTT\n\n\
         00:00:00.000 --> 00:00:00.100\nÜ\né\n\n\
         00:00:00.100 --> 00:00:00.200\nlé\n\n\
         00:00:00.200 --> 00:00:00.300\nbad \u{FFFD} byte\ncr\nnel\n\n\
         00:00:00.300 --> 00:00:00.400\nbom\n\n"
    );
    assert_eq!(summary.dropped.invalid_text, 1);
}

#[test]
fn malformed_and_odd_samples_are_counted() {
    let mut short = text_sample(b"long text", &[]);
    short.truncate(5);
    let data = subtitles(vec![
        (short, 100),
        (text_sample(b"x", &[vec![0, 0, 0, 3]]), 100),
        (
            text_sample(b"y", &[bx(b"zzzz", &[]), bx(b"krok", &[0; 6])]),
            100,
        ),
        (text_sample(b"\n\n", &[]), 100),
        (Vec::new(), 100),
    ]);
    let (summary, vtt) = extract(&data, TrackChoice::First).unwrap();
    assert_eq!(summary.cues, 2, "{vtt}");
    assert_eq!(summary.dropped.malformed, 1);
    assert_eq!(summary.dropped.trailing_bytes, 1);
    assert_eq!(summary.dropped.empty, 2);
    assert_eq!(summary.dropped.styling[OTHER_MODIFIER], 1);
    assert_eq!(summary.dropped.styling[slot(b"krok")], 1);
}

#[test]
fn times_round_to_milliseconds_and_short_cues_are_dropped() {
    let track = TrackSpec {
        timescale: 3000,
        ..TrackSpec::tx3g(
            2,
            vec![
                (text_sample(b"a", &[]), 1),
                (text_sample(b"b", &[]), 2),
                (text_sample(b"c", &[]), 3000),
            ],
        )
    };
    let data = movie(&[TrackSpec::video(1, &[1000]), track]);
    let (summary, vtt) = extract(&data, TrackChoice::First).unwrap();
    // 0..1/3 ms rounds to 0..0; 1/3..1 ms to 0..1; 1 ms..1.001 s.
    assert_eq!(
        vtt,
        "WEBVTT\n\n00:00:00.000 --> 00:00:00.001\nb\n\n00:00:00.001 --> 00:00:01.001\nc\n\n"
    );
    assert_eq!(summary.dropped.too_short, 1);
}

#[test]
fn edits_shift_and_cut_cues() {
    // An empty edit of 500 ms, then media from 1 s for 1.5 s.
    let track = TrackSpec {
        elst: Some(vec![(500, -1), (1500, 1000)]),
        ..TrackSpec::tx3g(
            2,
            vec![
                (text_sample(b"before", &[]), 1000),
                (text_sample(b"inside", &[]), 1000),
                (text_sample(b"cut", &[]), 1000),
                (text_sample(b"after", &[]), 1000),
            ],
        )
    };
    let data = movie(&[TrackSpec::video(1, &[2000]), track]);
    let (summary, vtt) = extract(&data, TrackChoice::First).unwrap();
    assert_eq!(
        vtt,
        "WEBVTT\n\n\
         00:00:00.500 --> 00:00:01.500\ninside\n\n\
         00:00:01.500 --> 00:00:02.000\ncut\n\n"
    );
    assert_eq!(summary.dropped.not_presented, 2);
}

/// Several media edits that play the media back to back present exactly
/// what no edit list does: every cue comes out whole
/// (`docs/design/edit-list-writers.md`, "`coreav-text` timing").
#[test]
fn contiguous_edits_present_the_media_unchanged() {
    let samples = || {
        vec![
            (text_sample(b"a", &[]), 1000),
            (text_sample(b"b", &[]), 1000),
            (text_sample(b"c", &[]), 1000),
        ]
    };
    let plain = movie(&[TrackSpec::video(1, &[3000]), TrackSpec::tx3g(2, samples())]);
    // The same media as four edits of 750 ticks each, which tile it: one
    // boundary inside every cue and one exactly on a cue boundary.
    let split = movie(&[
        TrackSpec::video(1, &[3000]),
        TrackSpec {
            elst: Some(vec![(750, 0), (750, 750), (750, 1500), (750, 2250)]),
            ..TrackSpec::tx3g(2, samples())
        },
    ]);
    let (plain_summary, plain_vtt) = extract(&plain, TrackChoice::First).unwrap();
    let (summary, vtt) = extract(&split, TrackChoice::First).unwrap();
    assert_eq!(vtt, plain_vtt);
    assert_eq!(summary.cues, plain_summary.cues);
    assert_eq!(summary.dropped.not_presented, 0);
}

/// A sample of no duration is shown by the media edit whose media holds
/// it, and by no other: the walk stops at the first edit that starts after
/// it (`crates/coreav-text/src/timing.rs`, `Windows::shown`).
#[test]
fn a_sample_of_no_duration_is_shown_by_the_edit_that_holds_it() {
    // Two media edits that play the media back to back, with a sample of
    // no duration exactly on their boundary.
    let track = TrackSpec {
        elst: Some(vec![(500, 0), (500, 500)]),
        ..TrackSpec::tx3g(
            2,
            vec![
                (text_sample(b"a", &[]), 500),
                (text_sample(b"z", &[]), 0),
                (text_sample(b"b", &[]), 500),
            ],
        )
    };
    let data = movie(&[TrackSpec::video(1, &[1000]), track]);
    let (summary, vtt) = extract(&data, TrackChoice::First).unwrap();
    assert_eq!(
        vtt,
        "WEBVTT\n\n\
         00:00:00.000 --> 00:00:00.500\na\n\n\
         00:00:00.500 --> 00:00:01.000\nb\n\n"
    );
    // The second edit holds it, so it is shown, for no time: a cue that
    // rounds to no duration, not a sample no edit presents.
    assert_eq!(summary.dropped.too_short, 1);
    assert_eq!(summary.dropped.not_presented, 0);
}

/// A sample two media edits show, with presentation time between them,
/// comes out as a cue each (ISO/IEC 14496-12 8.6.6).
#[test]
fn a_cue_two_edits_show_comes_out_twice() {
    // Media [0, 500) presented from 0, an empty edit of 500 ms, then media
    // [500, 2000) presented from 1000. The first sample spans the splice.
    let track = TrackSpec {
        elst: Some(vec![(500, 0), (500, -1), (1500, 500)]),
        ..TrackSpec::tx3g(
            2,
            vec![
                (text_sample(b"a", &[]), 1000),
                (text_sample(b"b", &[]), 1000),
            ],
        )
    };
    let data = movie(&[TrackSpec::video(1, &[2000]), track]);
    let (summary, vtt) = extract(&data, TrackChoice::First).unwrap();
    assert_eq!(
        vtt,
        "WEBVTT\n\n\
         00:00:00.000 --> 00:00:00.500\na\n\n\
         00:00:01.000 --> 00:00:01.500\na\n\n\
         00:00:01.500 --> 00:00:02.500\nb\n\n"
    );
    assert_eq!(summary.cues, 3);
    assert_eq!(summary.dropped.not_presented, 0);
}

/// Several media edits with media between them that neither shows: each
/// cue is cut to the edits that show it.
#[test]
fn several_edits_cut_each_cue_to_the_edits_that_show_it() {
    // Media [0, 700) presented from 0, an empty edit of 300 ms, then media
    // [2500, 4000) presented from 1000. Four samples of one second each.
    let track = TrackSpec {
        elst: Some(vec![(700, 0), (300, -1), (1500, 2500)]),
        ..TrackSpec::tx3g(
            2,
            vec![
                (text_sample(b"a", &[]), 1000),
                (text_sample(b"b", &[]), 1000),
                (text_sample(b"c", &[]), 1000),
                (text_sample(b"d", &[]), 1000),
            ],
        )
    };
    let data = movie(&[TrackSpec::video(1, &[4000]), track]);
    let (summary, vtt) = extract(&data, TrackChoice::First).unwrap();
    assert_eq!(
        vtt,
        "WEBVTT\n\n\
         00:00:00.000 --> 00:00:00.700\na\n\n\
         00:00:01.000 --> 00:00:01.500\nc\n\n\
         00:00:01.500 --> 00:00:02.500\nd\n\n"
    );
    // "a" is cut at the first edit's end; "b" lies in the media the edits
    // skip; "c" starts before the second edit and is shown from its start.
    assert_eq!(summary.dropped.not_presented, 1);
}

#[test]
fn unsupported_tracks_are_refused_before_writing() {
    // Two media edits whose windows overlap, which no model applies
    // (docs/design/edit-lists.md, decision 1).
    let track = TrackSpec {
        elst: Some(vec![(500, 0), (500, 0)]),
        ..TrackSpec::tx3g(2, vec![(text_sample(b"a", &[]), 1000)])
    };
    let data = movie(&[TrackSpec::video(1, &[1000]), track]);
    let mut out = Vec::new();
    let result = extract_subtitles(
        SliceSource::new(&data),
        &Limits::default(),
        &TextLimits::default(),
        TrackChoice::First,
        &mut out,
    );
    assert!(matches!(
        result,
        Err(TextError::Source(Error::Unsupported { .. }))
    ));
    assert!(out.is_empty());
    // Zero composition offsets are no offsets.
    let track = TrackSpec {
        ctts: Some(vec![0]),
        ..TrackSpec::tx3g(2, vec![(text_sample(b"a", &[]), 1000)])
    };
    let data = movie(&[TrackSpec::video(1, &[1000]), track]);
    assert!(extract(&data, TrackChoice::First).is_ok());
    // Composition offsets.
    let track = TrackSpec {
        ctts: Some(vec![5]),
        ..TrackSpec::tx3g(2, vec![(text_sample(b"a", &[]), 1000)])
    };
    let data = movie(&[TrackSpec::video(1, &[1000]), track]);
    assert!(matches!(
        extract(&data, TrackChoice::First),
        Err(TextError::Source(Error::Unsupported { .. }))
    ));
}

#[test]
fn track_choice() {
    let chapters = TrackSpec::qt_text(3, vec![(text_sample(b"Chapter", &[]), 1000)]);
    let video = TrackSpec {
        chap: vec![3],
        ..TrackSpec::video(1, &[1000])
    };
    let subs = TrackSpec::tx3g(2, vec![(text_sample(b"Sub", &[]), 1000)]);
    let data = movie(&[video.clone(), chapters.clone(), subs]);
    // The first text track that is not a chapter track.
    let (summary, vtt) = extract(&data, TrackChoice::First).unwrap();
    assert_eq!(summary.stream_index, 2);
    assert!(vtt.contains("Sub"));
    // A chapter track by index; QuickTime text.
    let (summary, vtt) = extract(&data, TrackChoice::Stream(1)).unwrap();
    assert_eq!(summary.codec, Codec::QuickTimeText);
    assert!(vtt.contains("Chapter"));
    // Not a text stream, no such stream, no subtitle track.
    let refused = |result: Result<_, TextError>| match result {
        Err(TextError::Source(error)) => error,
        other => panic!("{other:?}"),
    };
    assert!(matches!(
        refused(extract(&data, TrackChoice::Stream(0))),
        Error::Unsupported { .. }
    ));
    assert_eq!(
        refused(extract(&data, TrackChoice::Stream(9))),
        Error::NoSuchTrack
    );
    let data = movie(&[video, chapters]);
    assert_eq!(
        refused(extract(&data, TrackChoice::First)),
        Error::NoSuchTrack
    );
}

#[test]
fn large_samples_hit_the_limit() {
    let data = subtitles(vec![(text_sample(&[b'a'; 600], &[]), 1000)]);
    let limits = TextLimits {
        max_sample_bytes: 512,
        ..TextLimits::default()
    };
    assert!(matches!(
        extract_with(&data, TrackChoice::First, &limits),
        Err(TextError::LimitExceeded {
            limit: TextLimitKind::SampleBytes,
            ..
        })
    ));
}

#[test]
fn write_errors_are_reported() {
    struct Full;
    impl std::io::Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::StorageFull.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let data = subtitles(vec![(text_sample(b"a", &[]), 1000)]);
    let result = extract_subtitles(
        SliceSource::new(&data),
        &Limits::default(),
        &TextLimits::default(),
        TrackChoice::First,
        Full,
    );
    assert_eq!(
        result,
        Err(TextError::Write(std::io::ErrorKind::StorageFull))
    );
}

// ------------------------------------------------------------------ chapters

fn chapter_movie(chapters: TrackSpec) -> Vec<u8> {
    let video = TrackSpec {
        chap: vec![chapters.id],
        ..TrackSpec::video(1, &[1000, 1000])
    };
    movie(&[video, chapters])
}

fn ticks(n: i128) -> Rational {
    Rational::from_int(n)
}

#[test]
fn chapters_from_a_quicktime_text_track() {
    let data = chapter_movie(TrackSpec::qt_text(
        2,
        vec![
            (text_sample(b"Intro", &[bx(b"encd", &[0, 0, 1, 0])]), 800),
            (text_sample("Ënd\0".as_bytes(), &[]), 1200),
            (text_sample(b"", &[]), 0),
        ],
    ));
    let list = read_chapters(
        SliceSource::new(&data),
        &Limits::default(),
        &TextLimits::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(list.stream_index, 1);
    assert_eq!(list.track_id, 2);
    assert_eq!(list.codec, Codec::QuickTimeText);
    assert_eq!(list.timescale, 1000);
    let got: Vec<_> = list
        .chapters
        .iter()
        .map(|c| (c.start, c.end, c.title.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (ticks(0), ticks(800), "Intro"),
            (ticks(800), ticks(2000), "Ënd"),
            (ticks(2000), ticks(2000), ""),
        ]
    );
    assert_eq!(list.dropped.styling[OTHER_MODIFIER], 1);
}

#[test]
fn chapters_from_tx3g_and_through_an_edit() {
    let track = TrackSpec {
        enabled: false,
        elst: Some(vec![(1000, 500)]),
        ..TrackSpec::tx3g(
            2,
            vec![
                (text_sample(b"A", &[]), 500),
                (text_sample(b"B", &[]), 1500),
            ],
        )
    };
    let data = chapter_movie(track);
    let list = read_chapters(
        SliceSource::new(&data),
        &Limits::default(),
        &TextLimits::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(list.codec, Codec::TimedText);
    assert_eq!(list.chapters.len(), 1);
    assert_eq!(list.chapters[0].title, "B");
    assert_eq!(
        (list.chapters[0].start, list.chapters[0].end),
        (ticks(0), ticks(1000))
    );
    assert_eq!(list.dropped.not_presented, 1);
}

#[test]
fn no_chapter_track() {
    let none = |data: &[u8]| {
        read_chapters(
            SliceSource::new(data),
            &Limits::default(),
            &TextLimits::default(),
        )
        .unwrap()
        .is_none()
    };
    // No reference.
    assert!(none(&movie(&[TrackSpec::video(1, &[1000])])));
    // A reference to a video track, or to no track.
    let video = TrackSpec {
        chap: vec![3, 1],
        ..TrackSpec::video(1, &[1000])
    };
    assert!(none(&movie(&[video])));
}

#[test]
fn chapter_limits() {
    let samples: Vec<_> = (0..20).map(|_| (text_sample(b"title", &[]), 10)).collect();
    let data = chapter_movie(TrackSpec::qt_text(2, samples));
    let read =
        |limits: TextLimits| read_chapters(SliceSource::new(&data), &Limits::default(), &limits);
    assert_eq!(
        read(TextLimits::default()).unwrap().unwrap().chapters.len(),
        20
    );
    let few = TextLimits {
        max_chapters: 19,
        ..TextLimits::default()
    };
    assert!(matches!(
        read(few),
        Err(TextError::LimitExceeded {
            limit: TextLimitKind::Chapters,
            ..
        })
    ));
    let short = TextLimits {
        max_title_bytes: 99,
        ..TextLimits::default()
    };
    assert!(matches!(
        read(short),
        Err(TextError::LimitExceeded {
            limit: TextLimitKind::TitleBytes,
            ..
        })
    ));
}
