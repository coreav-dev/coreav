//! Arbitrary input through `extract_subtitles` and `read_chapters`
//! (docs/design/text.md): each either fails with an error or returns a
//! result that keeps its promises. The WebVTT has the header, timestamps
//! with three-digit milliseconds, cues that end after they start and start
//! in order, escaped text and no empty line in a cue; its byte count is the
//! summary's. The chapter list stays within its limits, and every chapter
//! ends no earlier than it starts.
#![no_main]

use coreav_isobmff::{BufferedSource, Limits, SliceSource};
use coreav_text::{TextLimits, TrackChoice, extract_subtitles, read_chapters};
use libfuzzer_sys::fuzz_target;

fn millis(stamp: &str) -> u64 {
    let (clock, fraction) = stamp.split_once('.').expect("a fraction");
    assert_eq!(fraction.len(), 3, "{stamp}");
    let parts: Vec<&str> = clock.split(':').collect();
    assert_eq!(parts.len(), 3, "{stamp}");
    assert!(parts[0].len() >= 2 && parts[1].len() == 2 && parts[2].len() == 2, "{stamp}");
    let (h, m, s): (u64, u64, u64) = (
        parts[0].parse().unwrap(),
        parts[1].parse().unwrap(),
        parts[2].parse().unwrap(),
    );
    assert!(m < 60 && s < 60, "{stamp}");
    ((h * 60 + m) * 60 + s) * 1000 + fraction.parse::<u64>().unwrap()
}

fn check_webvtt(out: &[u8], cues: u64) {
    let text = std::str::from_utf8(out).expect("UTF-8");
    let body = text.strip_prefix("WEBVTT\n\n").expect("the header");
    assert!(body.is_empty() || body.ends_with("\n\n"));
    let mut previous = 0;
    let mut count = 0;
    for block in body.split("\n\n").filter(|b| !b.is_empty()) {
        let mut lines = block.split('\n');
        let timing = lines.next().unwrap();
        let (start, end) = timing.split_once(" --> ").expect("timings");
        let (start, end) = (millis(start), millis(end));
        assert!(end > start && start >= previous, "{timing}");
        previous = start;
        let mut payload = 0;
        for line in lines {
            assert!(!line.trim().is_empty(), "an empty line in a cue");
            assert!(!line.contains('<') && !line.contains('>') && !line.contains('\r'));
            assert!(!line.contains('\0'));
            let rest = line.replace("&amp;", "").replace("&lt;", "").replace("&gt;", "");
            assert!(!rest.contains('&'), "{line}");
            payload += 1;
        }
        assert!(payload > 0, "a cue without text");
        count += 1;
    }
    assert_eq!(count, cues);
}

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_sample_work: 1 << 20,
        max_table_bytes: 1 << 20,
        ..Limits::default()
    };
    let text_limits = TextLimits {
        max_sample_bytes: 1 << 16,
        max_chapters: 64,
        max_title_bytes: 1 << 12,
    };
    // The first byte picks the track: the first text track, or a stream.
    let (choice, input) = match data.split_first() {
        None => return,
        Some((0, rest)) => (TrackChoice::First, rest),
        Some((n, rest)) => (TrackChoice::Stream(usize::from(*n) - 1), rest),
    };
    let mut out = Vec::new();
    let source = BufferedSource::new(SliceSource::new(input));
    if let Ok(summary) = extract_subtitles(source, &limits, &text_limits, choice, &mut out) {
        assert_eq!(summary.bytes_written, out.len() as u64);
        assert!(summary.cues <= summary.samples);
        check_webvtt(&out, summary.cues);
    }
    if let Ok(Some(list)) = read_chapters(SliceSource::new(input), &limits, &text_limits) {
        assert!(list.chapters.len() <= text_limits.max_chapters as usize);
        let titles: usize = list.chapters.iter().map(|c| c.title.len()).sum();
        assert!(titles as u64 <= text_limits.max_title_bytes);
        for chapter in &list.chapters {
            assert!(!chapter.end.checked_lt(chapter.start).unwrap_or(true));
            assert!(!chapter.title.contains('\0'));
        }
    }
});
