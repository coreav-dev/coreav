//! ISO timecode track-reference group counts through the command.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

#[path = "../../coreav-isobmff/tests/common/text_movie.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn movie(
    sources: &[(u32, &[u32])],
    targets: &[u32],
    reference_kind: &[u8; 4],
    target_first: bool,
) -> Vec<u8> {
    let mut tracks: Vec<_> = sources
        .iter()
        .map(|(id, references)| common::TrackSpec {
            chap: references.to_vec(),
            ..common::TrackSpec::video(*id, &[1000])
        })
        .collect();
    let mut entry = vec![0; 26];
    entry[7] = 1;
    entry[16..20].copy_from_slice(&30u32.to_be_bytes());
    entry[20..24].copy_from_slice(&1u32.to_be_bytes());
    entry[24] = 30;
    entry.extend(common::bx(b"free", &[0; 44]));
    for id in targets {
        tracks.push(common::TrackSpec {
            handler: *b"tmcd",
            entry: common::bx(b"tmcd", &entry),
            timescale: 30,
            ..common::TrackSpec::qt_text(*id, vec![(0i32.to_be_bytes().to_vec(), 30)])
        });
    }
    if target_first {
        tracks.rotate_right(targets.len());
    }
    let mut bytes = common::movie(&tracks);
    let references: Vec<_> = bytes
        .windows(4)
        .enumerate()
        .filter_map(|(index, kind)| (kind == b"chap").then_some(index))
        .collect();
    for at in references {
        bytes[at..at + 4].copy_from_slice(reference_kind);
    }
    bytes
}

fn probe(path: &Path, bytes: &[u8], extra: &[&str]) -> serde_json::Value {
    std::fs::write(path, bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args([
            "ffprobe",
            "-v",
            "error",
            "-show_entries",
            "format=nb_stream_groups",
            "-of",
            "json",
        ])
        .args(extra)
        .arg(path)
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        path,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn directory(name: &str) -> FixtureDir {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("stream-group-{name}-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    FixtureDir(path)
}

struct FixtureDir(PathBuf);

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn iso_stream_group_counts_follow_resolved_shared_track_references() {
    let directory = directory("count");
    for (name, sources, targets, expected) in [
        ("linked", vec![(1, &[2][..])], vec![2], 1),
        (
            "independent",
            vec![(1, &[3][..]), (2, &[4][..])],
            vec![3, 4],
            2,
        ),
        ("shared", vec![(1, &[3][..]), (2, &[3][..])], vec![3], 1),
        ("unlinked", vec![(1, &[][..])], vec![2], 0),
        ("missing", vec![(1, &[99][..])], vec![2], 0),
        ("multiple", vec![(1, &[2, 3][..])], vec![2, 3], 2),
        ("missing-first", vec![(1, &[99, 2][..])], vec![2], 1),
        ("missing-last", vec![(1, &[2, 99][..])], vec![2], 1),
        (
            "duplicates",
            vec![(1, &[3, 3][..]), (2, &[3][..])],
            vec![3],
            1,
        ),
        ("noncontiguous", vec![(17, &[93][..])], vec![93], 1),
        ("self", vec![(1, &[1][..])], vec![2], 0),
        ("cycle", vec![(1, &[2][..]), (2, &[1][..])], vec![], 0),
    ] {
        for target_first in [false, true] {
            let path = directory.0.join(format!("{name}-{target_first}.mov"));
            let bytes = movie(&sources, &targets, b"tmcd", target_first);
            let doc = probe(&path, &bytes, &[]);
            assert_eq!(doc["format"]["nb_stream_groups"], expected, "{name}");
        }
    }
}

#[test]
fn iso_group_counts_ignore_selection_handlers_and_unrelated_references() {
    let directory = directory("types");
    let path = directory.0.join("types.mov");
    for kind in [
        b"tmcd", b"cdsc", b"auxl", b"hint", b"sync", b"dpnd", b"thmb", b"zzzz",
    ] {
        let bytes = movie(&[(1, &[2])], &[2], kind, false);
        let doc = probe(&path, &bytes, &[]);
        assert_eq!(
            doc["format"]["nb_stream_groups"],
            i32::from(kind == b"tmcd")
        );
    }
    for handler in [b"tmcd", b"vide", b"soun", b"meta", b"text", b"zzzz"] {
        let mut bytes = movie(&[(1, &[2])], &[2], b"tmcd", false);
        let at = bytes.windows(4).rposition(|kind| kind == b"hdlr").unwrap() + 12;
        bytes[at..at + 4].copy_from_slice(handler);
        let path = directory
            .0
            .join(format!("handler-{}.mov", String::from_utf8_lossy(handler)));
        assert_eq!(probe(&path, &bytes, &[])["format"]["nb_stream_groups"], 1);
    }
    for entry in [b"tmcd", b"tc64", b"text", b"c608", b"mett", b"zzzz"] {
        let mut bytes = movie(&[(1, &[2])], &[2], b"tmcd", false);
        let at = bytes.windows(4).rposition(|kind| kind == b"stsd").unwrap() + 16;
        bytes[at..at + 4].copy_from_slice(entry);
        let path = directory
            .0
            .join(format!("entry-{}.mov", String::from_utf8_lossy(entry)));
        let doc = probe(&path, &bytes, &[]);
        assert_eq!(
            doc["format"]["nb_stream_groups"],
            i32::from(entry == b"tmcd")
        );
    }
    let mut bytes = movie(&[(1, &[2])], &[2], b"tmcd", false);
    let at = bytes.windows(4).position(|kind| kind == b"stsd").unwrap() + 16;
    bytes[at..at + 4].copy_from_slice(b"zzzz");
    assert_eq!(probe(&path, &bytes, &[])["format"]["nb_stream_groups"], 1);
    let mut bytes = movie(&[(1, &[2])], &[2], b"tmcd", false);
    let at = bytes.windows(4).rposition(|kind| kind == b"stsd").unwrap() + 36;
    bytes[at..at + 8].fill(0);
    assert_eq!(probe(&path, &bytes, &[])["format"]["nb_stream_groups"], 1);
    for selection in ["v", "a", "d", "0", "1", "99"] {
        assert_eq!(
            probe(&path, &bytes, &["-select_streams", selection])["format"]["nb_stream_groups"],
            1
        );
    }
}

#[test]
fn incomplete_references_do_not_become_a_known_zero_group_count() {
    let directory = directory("bounded");
    let path = directory.0.join("bounded.mov");
    for (ids, expected) in [(1022, Some(1)), (1023, None)] {
        let references = vec![2; ids];
        let bytes = movie(&[(1, &references)], &[2], b"tmcd", false);
        let doc = probe(&path, &bytes, &[]);
        assert_eq!(doc["format"]["nb_stream_groups"].as_i64(), expected);
        let output = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args([
                "ffprobe",
                "-v",
                "error",
                "-show_entries",
                "format=nb_stream_groups",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(&path)
            .env("COREAV_STRICT_SAFE", "1")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            if expected.is_some() {
                "nb_stream_groups=1\n"
            } else {
                "nb_stream_groups=N/A\n"
            }
        );
    }
}

#[test]
fn group_listing_is_still_refused_before_file_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["ffprobe", "-show_stream_groups", "missing-group-input.mov"])
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("stream groups are not read"), "{error}");
    assert!(!error.contains("No such file"), "{error}");
}
