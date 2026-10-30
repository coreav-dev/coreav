//! `coreav extract-subtitles` and `coreav chapters` (`docs/design/text.md`):
//! 3GPP timed text to WebVTT with the `coreav.extract-subtitles/1` summary,
//! and the `coreav.chapters/1` chapter list.

use crate::error::Failure;
use crate::names;
use crate::native::time;
use crate::output::WriteFailure;
use coreav_isobmff::{BufferedSource, FileSource, Limits, SliceSource};
use coreav_text::{
    Chapters, Dropped, MODIFIERS, SubtitleSummary, TextError, TextLimits, TrackChoice,
    extract_subtitles, read_chapters,
};
use serde_json::{Map, Value, json};
use std::io::Write;
use std::path::Path;

impl From<TextError> for WriteFailure {
    fn from(error: TextError) -> Self {
        match error {
            TextError::Source(error) => Self::Rejected(Failure::library(error)),
            TextError::LimitExceeded { limit, offset } => {
                Self::Rejected(Failure::TextLimit { limit, offset })
            }
            TextError::Write(kind) => Self::output(&std::io::Error::from(kind)),
        }
    }
}

/// Writes the chosen timed text track of the file at `path` to `out` as
/// WebVTT, with the library defaults.
pub fn run_subtitles<W: Write>(
    path: &Path,
    choice: TrackChoice,
    out: W,
) -> Result<SubtitleSummary, WriteFailure> {
    let source = BufferedSource::new(FileSource::open(path)?);
    Ok(extract_subtitles(
        source,
        &Limits::default(),
        &TextLimits::default(),
        choice,
        out,
    )?)
}

/// Extracts subtitles from bytes already in memory, as [`run_subtitles`]
/// does for a file.
pub fn run_subtitles_slice<W: Write>(
    data: &[u8],
    choice: TrackChoice,
    out: W,
) -> Result<SubtitleSummary, WriteFailure> {
    run_subtitles_source(SliceSource::new(data), choice, out)
}

/// Extracts subtitles from any [`Source`](coreav_isobmff::Source), as
/// [`run_subtitles`] does for a file.
pub fn run_subtitles_source<S: coreav_isobmff::Source, W: Write>(
    source: S,
    choice: TrackChoice,
    out: W,
) -> Result<SubtitleSummary, WriteFailure> {
    Ok(extract_subtitles(
        source,
        &Limits::default(),
        &TextLimits::default(),
        choice,
        out,
    )?)
}

/// Reads the chapter list of the file at `path` with the library defaults.
pub fn run_chapters(path: &Path) -> Result<Option<Chapters>, Failure> {
    let source = BufferedSource::new(FileSource::open(path).map_err(Failure::library)?);
    read_chapters(source, &Limits::default(), &TextLimits::default()).map_err(failure)
}

/// Reads the chapter list of bytes already in memory, as [`run_chapters`]
/// does for a file.
pub fn run_chapters_slice(data: &[u8]) -> Result<Option<Chapters>, Failure> {
    run_chapters_source(SliceSource::new(data))
}

/// Reads the chapter list of any [`Source`](coreav_isobmff::Source), as
/// [`run_chapters`] does for a file.
pub fn run_chapters_source<S: coreav_isobmff::Source>(
    source: S,
) -> Result<Option<Chapters>, Failure> {
    read_chapters(source, &Limits::default(), &TextLimits::default()).map_err(failure)
}

fn failure(error: TextError) -> Failure {
    match error {
        TextError::Source(error) => Failure::library(error),
        TextError::LimitExceeded { limit, offset } => Failure::TextLimit { limit, offset },
        // `read_chapters` writes nothing.
        TextError::Write(kind) => Failure::Io(kind),
    }
}

/// The `coreav.extract-subtitles/1` summary
/// (`docs/schema/extract-subtitles-v1.md`).
pub fn subtitles_document(summary: &SubtitleSummary) -> Value {
    json!({
        "schema": "coreav.extract-subtitles/1",
        "output": {"bytes": summary.bytes_written},
        "stream": {
            "stream_index": summary.stream_index,
            "track_id": summary.track_id,
            "codec": names::codec(summary.codec),
            "language": names::language(summary.language),
        },
        "samples": summary.samples,
        "cues": summary.cues,
        "dropped": dropped(&summary.dropped),
    })
}

/// The `coreav.chapters/1` document (`docs/schema/chapters-v1.md`).
pub fn chapters_document(chapters: Option<&Chapters>) -> Value {
    let Some(list) = chapters else {
        return json!({
            "schema": "coreav.chapters/1",
            "stream": null,
            "chapters": [],
            "samples": 0,
            "dropped": dropped(&Dropped::default()),
        });
    };
    let entries: Vec<Value> = list
        .chapters
        .iter()
        .map(|c| {
            json!({
                "start": time(c.start, list.timescale),
                "end": time(c.end, list.timescale),
                "title": c.title,
            })
        })
        .collect();
    json!({
        "schema": "coreav.chapters/1",
        "stream": {
            "stream_index": list.stream_index,
            "track_id": list.track_id,
            "codec": names::codec(list.codec),
        },
        "chapters": entries,
        "samples": list.samples,
        "dropped": dropped(&list.dropped),
    })
}

fn dropped(dropped: &Dropped) -> Value {
    let mut styling = Map::new();
    for (kind, count) in MODIFIERS.iter().zip(dropped.styling) {
        styling.insert(String::from_utf8_lossy(*kind).into_owned(), json!(count));
    }
    styling.insert(
        "other".to_owned(),
        json!(dropped.styling.last().copied().unwrap_or(0)),
    );
    json!({
        "empty": dropped.empty,
        "not_presented": dropped.not_presented,
        "too_short": dropped.too_short,
        "malformed": dropped.malformed,
        "invalid_text": dropped.invalid_text,
        "trailing_bytes": dropped.trailing_bytes,
        "styling": styling,
    })
}
