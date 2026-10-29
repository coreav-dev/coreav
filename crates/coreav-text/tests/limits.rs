//! Each field of `TextLimits` at its boundary, and the default values.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[path = "../../coreav-isobmff/tests/common/text_movie.rs"]
mod text_movie;

use coreav_isobmff::{Error, LimitKind, Limits, SliceSource};
use coreav_text::{Chapters, TextError, TextLimits, TrackChoice, extract_subtitles, read_chapters};
use text_movie::{TrackSpec, movie, text_sample};

fn subtitles(data: &[u8], text_limits: &TextLimits) -> Result<u64, TextError> {
    extract_subtitles(
        SliceSource::new(data),
        &Limits::default(),
        text_limits,
        TrackChoice::First,
        Vec::new(),
    )
    .map(|summary| summary.cues)
}

fn chapters(data: &[u8], text_limits: &TextLimits) -> Result<Chapters, TextError> {
    read_chapters(SliceSource::new(data), &Limits::default(), text_limits).map(|list| list.unwrap())
}

fn subtitle_movie(text: &[u8]) -> Vec<u8> {
    movie(&[
        TrackSpec::video(1, &[1000, 1000]),
        TrackSpec::tx3g(2, vec![(text_sample(text, &[]), 1000)]),
    ])
}

fn chapter_movie(titles: &[&[u8]]) -> Vec<u8> {
    let samples = titles.iter().map(|t| (text_sample(t, &[]), 10)).collect();
    let chapters = TrackSpec::qt_text(2, samples);
    let video = TrackSpec {
        chap: vec![chapters.id],
        ..TrackSpec::video(1, &[1000, 1000])
    };
    movie(&[video, chapters])
}

fn refused(result: Result<impl Sized, TextError>) -> LimitKind {
    match result.err() {
        Some(TextError::Source(Error::LimitExceeded { limit, .. })) => limit,
        other => panic!("expected a limit error, got {other:?}"),
    }
}

fn is_limit_error(result: Result<impl Sized, TextError>) -> bool {
    matches!(
        result.err(),
        Some(TextError::Source(Error::LimitExceeded { .. }))
    )
}

#[test]
fn defaults_match_the_documented_values() {
    assert_eq!(
        TextLimits::default(),
        TextLimits {
            max_sample_bytes: 1 << 20,
            max_chapters: 10_000,
            max_title_bytes: 4 << 20,
        }
    );
}

#[test]
fn a_subtitle_sample_of_exactly_max_sample_bytes_is_read_and_one_more_is_refused() {
    let sample_len = text_sample(&[b'a'; 100], &[]).len() as u32;
    let limits = TextLimits {
        max_sample_bytes: sample_len,
        ..TextLimits::default()
    };
    assert_eq!(subtitles(&subtitle_movie(&[b'a'; 100]), &limits), Ok(1));
    assert_eq!(
        refused(subtitles(&subtitle_movie(&[b'a'; 101]), &limits)),
        LimitKind::PacketBytes
    );
}

#[test]
fn a_chapter_sample_of_exactly_max_sample_bytes_is_read_and_one_more_is_refused() {
    let sample_len = text_sample(&[b'a'; 100], &[]).len() as u32;
    let limits = TextLimits {
        max_sample_bytes: sample_len,
        ..TextLimits::default()
    };
    assert_eq!(
        chapters(&chapter_movie(&[&[b'a'; 100]]), &limits)
            .unwrap()
            .chapters
            .len(),
        1
    );
    assert_eq!(
        refused(chapters(&chapter_movie(&[&[b'a'; 101]]), &limits)),
        LimitKind::PacketBytes
    );
}

#[test]
fn exactly_max_chapters_are_kept_and_one_more_is_refused() {
    let limits = TextLimits {
        max_chapters: 20,
        ..TextLimits::default()
    };
    let titles = |count: usize| chapter_movie(&vec![&b"title"[..]; count]);
    assert_eq!(chapters(&titles(20), &limits).unwrap().chapters.len(), 20);
    assert!(is_limit_error(chapters(&titles(21), &limits)));
}

#[test]
fn exactly_max_title_bytes_of_titles_are_kept_and_one_more_is_refused() {
    let limits = TextLimits {
        max_title_bytes: 100,
        ..TextLimits::default()
    };
    let titles = |last: &[u8]| {
        let mut titles = vec![&b"title"[..]; 19];
        titles.push(last);
        chapter_movie(&titles)
    };
    assert_eq!(
        chapters(&titles(b"title"), &limits).unwrap().chapters.len(),
        20
    );
    assert!(is_limit_error(chapters(&titles(b"titles"), &limits)));
}
