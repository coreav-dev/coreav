//! ISO user-data names retain their movie or track scope through ffprobe.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

#[path = "../../coreav-isobmff/tests/common/text_movie.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use coreav_cli::error::Failure;
use coreav_cli::ffprobe;
use coreav_isobmff::{Limits, SliceSource};

type Item<'a> = (&'a [u8; 4], &'a [u8]);

fn movie(tracks: [&[Item<'_>]; 2], format: &[Item<'_>]) -> Vec<u8> {
    let mut bytes = common::movie(&[
        common::TrackSpec::video(19, &[1000]),
        common::TrackSpec::video(7, &[1000]),
    ]);
    let moov = bytes.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let mut at = moov + 8;
    let mut positions = Vec::new();
    while at < bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if &bytes[at + 4..at + 8] == b"trak" {
            positions.push(at);
        }
        at += size;
    }
    for (position, items) in positions.into_iter().zip(tracks).rev() {
        append_udta(&mut bytes, position, items);
    }
    let size = (bytes.len() - moov) as u32;
    bytes[moov..moov + 4].copy_from_slice(&size.to_be_bytes());
    append_udta(&mut bytes, moov, format);
    bytes
}

fn append_udta(bytes: &mut Vec<u8>, at: usize, items: &[Item<'_>]) {
    if items.is_empty() {
        return;
    }
    let payload: Vec<_> = items
        .iter()
        .flat_map(|(kind, value)| common::bx(kind, value))
        .collect();
    let extra = common::bx(b"udta", &payload);
    let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    bytes[at..at + 4].copy_from_slice(&((size + extra.len()) as u32).to_be_bytes());
    bytes.splice(at + size..at + size, extra);
}

struct FixtureDir(PathBuf);

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn probe(bytes: &[u8], selection: Option<&str>) -> serde_json::Value {
    serde_json::from_str(&probe_output(
        bytes,
        selection,
        "stream=index:stream_tags=name:format_tags=name",
        "json",
    ))
    .unwrap()
}

fn probe_output(bytes: &[u8], selection: Option<&str>, entries: &str, writer: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "track-names-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&dir).unwrap();
    let dir = FixtureDir(dir);
    let path = dir.0.join("names.mov");
    std::fs::write(&path, bytes).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
    command.args([
        "ffprobe",
        "-v",
        "error",
        "-show_entries",
        entries,
        "-of",
        writer,
    ]);
    if let Some(selection) = selection {
        command.args(["-select_streams", selection]);
    }
    let out = command
        .arg(path)
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn track_names_stay_with_their_owning_tracks_after_selection() {
    let bytes = movie(
        [&[(b"name", b"First track")], &[(b"name", b"Second track")]],
        &[(b"name", b"Movie name")],
    );
    let all = probe(&bytes, None);
    assert_eq!(all["streams"].as_array().unwrap().len(), 2);
    assert_eq!(all["streams"][0]["tags"]["name"], "First track");
    assert_eq!(all["streams"][1]["tags"]["name"], "Second track");
    assert_eq!(all["format"]["tags"]["name"], "Movie name");

    let selected = probe(&bytes, Some("v:1"));
    assert_eq!(selected["streams"].as_array().unwrap().len(), 1);
    assert_eq!(selected["streams"][0]["index"], 1);
    assert_eq!(selected["streams"][0]["tags"]["name"], "Second track");
    assert_eq!(selected["format"]["tags"]["name"], "Movie name");
}

#[test]
fn movie_and_track_names_do_not_fill_unnamed_scopes() {
    let track = probe(&movie([&[], &[(b"name", b"Only second")]], &[]), None);
    assert!(track["streams"][0]["tags"].get("name").is_none());
    assert_eq!(track["streams"][1]["tags"]["name"], "Only second");
    assert!(track["format"]["tags"].get("name").is_none());

    let format = probe(&movie([&[], &[]], &[(b"name", b"Only movie")]), None);
    for stream in format["streams"].as_array().unwrap() {
        assert!(stream["tags"].get("name").is_none());
    }
    assert_eq!(format["format"]["tags"]["name"], "Only movie");
}

#[test]
fn names_decode_and_overwrite_identically_in_movie_and_track_scopes() {
    let cases: &[(&str, &[Item<'_>], &str)] = &[
        ("empty", &[(b"name", b"")], ""),
        ("nul", &[(b"name", b"First\0Second")], "First"),
        (
            "unicode",
            &[(b"name", "café \u{1f3ac}".as_bytes())],
            "café \u{1f3ac}",
        ),
        ("mac-roman", &[(b"name", b"a\xffb")], "aˇb"),
        (
            "duplicate",
            &[(b"name", b"first"), (b"name", b"last")],
            "last",
        ),
        ("last-empty", &[(b"name", b"first"), (b"name", b"")], ""),
    ];
    for (label, items, expected) in cases {
        let result = probe(&movie([&[], items], items), None);
        assert!(
            result["streams"][0]["tags"].get("name").is_none(),
            "{label}"
        );
        assert_eq!(result["streams"][1]["tags"]["name"], *expected, "{label}");
        assert_eq!(result["format"]["tags"]["name"], *expected, "{label}");
    }
}

#[test]
fn title_atoms_do_not_infer_names() {
    let result = probe(
        &movie(
            [
                &[(b"\xa9nam", b"First title")],
                &[(b"\xa9nam", b"Second title")],
            ],
            &[(b"\xa9nam", b"Movie title")],
        ),
        None,
    );
    for stream in result["streams"].as_array().unwrap() {
        assert!(stream["tags"].get("name").is_none());
    }
    assert!(result["format"]["tags"].get("name").is_none());
}

#[test]
fn name_encoding_uses_the_full_payload_length_before_nul() {
    for (payload, expected) in [
        (b"\xc3\xa9\0\xff".as_slice(), "√©"),
        (b"\xc3\xa9\0\0\0".as_slice(), "é"),
        (b"\xff\xfeA\0".as_slice(), "ˇ˛A"),
        (b"\xef\xbb\xbfA".as_slice(), "ÔªøA"),
        (b"\xef\xbb\xbfA\0".as_slice(), "\u{feff}A"),
    ] {
        let items = [(b"name", payload)];
        let result = probe(&movie([&items, &[]], &items), None);
        assert_eq!(result["streams"][0]["tags"]["name"], expected);
        assert_eq!(result["format"]["tags"]["name"], expected);
    }
}

#[test]
fn unsupported_name_encodings_refuse_before_output_but_not_unrelated_fields() {
    for payload in [
        b"\xff\xfeA\0B\0".as_slice(),
        b"\xfe\xffA\0B\0".as_slice(),
        b"\xc3\xc3\xc3\xc3".as_slice(),
        b"\xe1\x80\xffAB".as_slice(),
        b"\xc0\x80ABC".as_slice(),
        b"\xed\xa0\x80AB".as_slice(),
    ] {
        for (tracks, format) in [
            ([&[(b"name", payload)][..], &[][..]], &[][..]),
            ([&[][..], &[][..]], &[(b"name", payload)][..]),
        ] {
            let bytes = movie(tracks, format);
            for writer in ["json", "default"] {
                for entries in [
                    "stream_tags=name",
                    "format_tags=NAME",
                    "packet=stream_index:stream_tags=name",
                    "stream=index:format=nb_streams",
                ] {
                    let mut source = SliceSource::new(&bytes);
                    let mut probed =
                        ffprobe::probe(&mut source, bytes.len() as u64, "names.mov", None)
                            .unwrap_or_else(|failure| panic!("{}", failure.to_json()));
                    let args =
                        ["-show_entries", entries, "-of", writer, "names.mov"].map(str::to_owned);
                    let options = ffprobe::args::parse(&args).ok().unwrap();
                    let mut output = Vec::new();
                    let result = ffprobe::emit(&options, &mut probed, &mut output);
                    if entries == "stream=index:format=nb_streams" {
                        assert!(result.is_ok());
                        assert!(!output.is_empty());
                    } else {
                        assert!(matches!(result, Err(Failure::Unsupported(_))));
                        assert!(output.is_empty());
                    }
                }
            }
        }
    }
}

fn move_udta_before(bytes: &mut Vec<u8>, before: &[u8; 4]) {
    let at = bytes.windows(4).position(|w| w == b"udta").unwrap() - 4;
    let target = bytes.windows(4).position(|w| w == before).unwrap() - 4;
    assert!(target < at);
    let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let udta: Vec<_> = bytes.drain(at..at + size).collect();
    bytes.splice(target..target, udta);
}

#[test]
fn track_name_order_follows_its_position_relative_to_media_metadata() {
    for early in [false, true] {
        let mut bytes = movie([&[(b"name", b"Track name")], &[]], &[]);
        if early {
            move_udta_before(&mut bytes, b"mdia");
        }
        let output = probe_output(&bytes, Some("v:0"), "stream_tags=name,language", "default");
        let name = output
            .find("TAG:name=Track name\n")
            .unwrap_or_else(|| panic!("missing track name: {output}"));
        let language = output
            .find("TAG:language=und\n")
            .unwrap_or_else(|| panic!("missing track language: {output}"));
        assert_eq!(name < language, early, "{output}");
    }
}

#[test]
fn movie_name_order_follows_its_position_relative_to_movie_header() {
    for early in [false, true] {
        let mut bytes = movie([&[], &[]], &[(b"name", b"Movie name")]);
        let mvhd = bytes.windows(4).position(|w| w == b"mvhd").unwrap();
        bytes[mvhd + 8..mvhd + 12].copy_from_slice(&1u32.to_be_bytes());
        if early {
            move_udta_before(&mut bytes, b"mvhd");
        }
        let output = probe_output(&bytes, None, "format_tags=name,creation_time", "default");
        let name = output
            .find("TAG:name=Movie name\n")
            .unwrap_or_else(|| panic!("missing movie name: {output}"));
        let creation = output
            .find("TAG:creation_time=")
            .unwrap_or_else(|| panic!("missing movie creation time: {output}"));
        assert_eq!(name < creation, early, "{output}");
    }
}

fn limited_name_output(entries: &str) -> (Result<(), Failure>, Vec<u8>) {
    let name = vec![b'n'; 16 * 1024];
    let bytes = movie([&[(b"name", &name)], &[]], &[(b"name", &name)]);
    let mut source = SliceSource::new(&bytes);
    let limits = Limits {
        max_retained_bytes: 4096,
        ..Limits::default()
    };
    let mut probed =
        ffprobe::probe_limited(&mut source, bytes.len() as u64, "names.mov", None, &limits)
            .unwrap_or_else(|failure| panic!("limited probe failed: {}", failure.to_json()));
    assert_eq!(probed.report.streams.len(), 2);
    let args = ["-show_entries", entries, "-of", "json", "names.mov"].map(str::to_owned);
    let options = ffprobe::args::parse(&args).ok().unwrap();
    let mut output = Vec::new();
    let result = ffprobe::emit(&options, &mut probed, &mut output);
    (result, output)
}

#[test]
fn incomplete_name_metadata_refuses_requested_names_before_output() {
    for entries in [
        "stream_tags=name",
        "format_tags=NAME",
        "packet=stream_index:stream_tags=name",
    ] {
        let (result, output) = limited_name_output(entries);
        assert!(
            matches!(result, Err(Failure::Unsupported(_))),
            "expected an unsupported-name refusal for {entries}"
        );
        assert!(output.is_empty(), "wrote output before refusing {entries}");
    }
}

#[test]
fn incomplete_name_metadata_keeps_unrelated_fields_native() {
    for entries in ["stream=index", "format=nb_streams"] {
        let (result, output) = limited_name_output(entries);
        result.unwrap_or_else(|failure| panic!("{entries}: {}", failure.to_json()));
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        if entries == "stream=index" {
            let indices: Vec<_> = value["streams"]
                .as_array()
                .unwrap()
                .iter()
                .map(|stream| stream["index"].as_u64().unwrap())
                .collect();
            assert_eq!(indices, [0, 1]);
        } else {
            assert_eq!(value["format"]["nb_streams"], 2);
        }
    }
}

#[test]
fn duplicate_names_straddling_media_metadata_keep_dictionary_overwrite_order() {
    let mut bytes = movie([&[(b"name", b"First")], &[]], &[]);
    move_udta_before(&mut bytes, b"mdia");
    let mdhd = bytes.windows(4).position(|w| w == b"mdhd").unwrap();
    bytes[mdhd + 8..mdhd + 12].copy_from_slice(&2_082_844_801u32.to_be_bytes());
    let track = bytes.windows(4).position(|w| w == b"trak").unwrap() - 4;
    append_udta(&mut bytes, track, &[(b"name", b"Last")]);
    let moov = bytes.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let size = (bytes.len() - moov) as u32;
    bytes[moov..moov + 4].copy_from_slice(&size.to_be_bytes());
    let output = probe_output(
        &bytes,
        Some("v:0"),
        "stream_tags=name,creation_time,language",
        "default",
    );
    assert!(
        output.contains(
            "TAG:language=und\nTAG:creation_time=1970-01-01T00:00:01.000000Z\nTAG:name=Last\n"
        ),
        "{output}"
    );
}
