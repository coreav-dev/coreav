//! `coreav ffprobe` through the binary
//! (`docs/design/compat-cli.md`): refusals and their exit status, the
//! command run under the name `ffprobe`, and a few outputs. Agreement with
//! `ffprobe` itself is measured by `tools/compat-corpus/run.py`.

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;
#[path = "../../coreav-mpegts/tests/common/mod.rs"]
mod ts;

use common::{aac_entry, alac_cookie, av_movie_with, dfla, lossless_entry};
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
}

fn file(name: &str, bytes: &[u8]) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Runs in strict-safe mode, so that a refused command is refused rather
/// than handed to an FFmpeg on `PATH` (`tests/fallback.rs` tests that).
fn run(program: &Path, args: &[&str]) -> Run {
    let out = Command::new(program)
        .args(args)
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn ffprobe(args: &[&str]) -> Run {
    let mut all = vec!["ffprobe"];
    all.extend_from_slice(args);
    run(Path::new(bin()), &all)
}

fn movie(name: &str) -> String {
    file(name, &av_movie_with(aac_entry(), false))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn composition_offsets_do_not_replace_the_stored_sample_durations() {
    let bytes = common::movie(&common::Spec {
        stts: Some(vec![(1, 256), (1, 512), (1, 768), (1, 1024)]),
        ctts: Some(vec![(4, 0)]),
        elst: None,
        ..common::Spec::default()
    });
    let path = file("ffprobe-stored-durations.mp4", &bytes);
    let r = ffprobe(&[
        "-v",
        "error",
        "-show_packets",
        "-show_entries",
        "packet=duration",
        "-of",
        "csv=p=0",
        &path.to_string_lossy(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "256\n512\n768\n1024\n");
}

#[test]
fn quicktime_keeps_its_zero_vendor_identifier() {
    for (brand, expected, name) in [
        (b"qt  ", true, "ffprobe-quicktime-vendor.mov"),
        (b"isom", false, "ffprobe-iso-vendor.mp4"),
    ] {
        let mut bytes = av_movie_with(aac_entry(), false);
        bytes[8..12].copy_from_slice(brand);
        let path = file(name, &bytes);
        let result = ffprobe(&[
            "-v",
            "error",
            "-show_entries",
            "stream_tags=vendor_id",
            "-of",
            "json",
            &path.to_string_lossy(),
        ]);
        assert_eq!(result.code, 0, "{}", result.stderr);
        assert_eq!(
            result.stdout.contains("[0][0][0][0]"),
            expected,
            "{}",
            result.stdout
        );
    }
}

#[test]
fn aac_mime_keeps_the_stored_object_type_with_a_sync_extension() {
    let mut body = common::audio_entry(b"mp4a").split_off(8);
    body.extend_from_slice(&common::full(
        b"esds",
        0,
        0,
        &[
            3, 25, 0, 0, 0, 4, 20, 0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 5, 0x13, 0x10,
            0x56, 0xe5, 0x98,
        ],
    ));
    let bytes = av_movie_with(common::bx(b"mp4a", &body), false);
    let path = file("ffprobe-aac-sync-mime.mp4", &bytes);
    let result = ffprobe(&[
        "-v",
        "error",
        "-select_streams",
        "a",
        "-show_entries",
        "stream=profile,mime_codec_string",
        "-of",
        "csv=p=0",
        &path.to_string_lossy(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_eq!(result.stdout, "HE-AAC,mp4a.40.2\n");
}

#[test]
fn an_option_outside_the_vocabulary_is_refused_by_name() {
    let path = movie("ffprobe-refused.mp4");
    for (args, named) in [
        (vec!["-show_frames", path.as_str()], "-show_frames"),
        (vec!["-of", "xml", "-show_streams", path.as_str()], "xml"),
        (
            vec!["-show_entries", "frame=pict_type", path.as_str()],
            "frame",
        ),
        (vec!["-f", "lavfi", "-i", "anullsrc"], "lavfi"),
        (vec!["-unit", "-show_streams", path.as_str()], "-unit"),
        (
            vec!["-select_streams", "p:1", "-show_streams", path.as_str()],
            "p:1",
        ),
        (
            vec!["-read_intervals", "10%20", "-show_packets", path.as_str()],
            "10%20",
        ),
        (
            vec!["-show_entries", "stream=nb_read_frames", path.as_str()],
            "nb_read_frames",
        ),
    ] {
        let r = ffprobe(&args);
        assert_eq!(r.code, 4, "{args:?}: {}", r.stderr);
        assert!(r.stdout.is_empty(), "{args:?}");
        assert!(r.stderr.contains(named), "{args:?}: {}", r.stderr);
    }
}

#[test]
fn a_command_that_asks_for_no_section_is_refused() {
    let path = movie("ffprobe-no-section.mp4");
    let r = ffprobe(&[path.as_str()]);
    assert_eq!(r.code, 4);
    assert!(r.stdout.is_empty());
    assert!(r.stderr.contains("not reproduced"));
}

#[test]
fn malformed_commands_are_usage_errors() {
    assert_eq!(ffprobe(&["-show_streams"]).code, 2);
    assert_eq!(ffprobe(&["-show_streams", "-of"]).code, 2);
    let path = movie("ffprobe-two.mp4");
    assert_eq!(ffprobe(&["-show_streams", &path, &path]).code, 2);
}

#[test]
fn a_missing_file_is_named() {
    let r = ffprobe(&["-show_streams", "/nonexistent/in.mp4"]);
    assert_eq!(r.code, 3);
    assert!(r.stdout.is_empty());
    assert!(
        r.stderr
            .contains("/nonexistent/in.mp4: No such file or directory")
    );
}

#[test]
fn help_is_the_usage_text() {
    for args in [
        vec!["-h"],
        vec!["-?"],
        vec!["-help"],
        vec!["--help"],
        vec!["-hide_banner", "-h"],
    ] {
        let r = ffprobe(&args);
        assert_eq!(r.code, 0, "{args:?}: {}", r.stderr);
        assert_eq!(r.stdout, coreav_cli::ffprobe::USAGE, "{args:?}");
        assert!(r.stderr.is_empty(), "{args:?}: {}", r.stderr);
    }
}

#[test]
fn help_on_a_topic_is_refused_by_name() {
    for (args, named) in [
        (vec!["-h", "long"], "'-h long'"),
        (vec!["-h", "full"], "'-h full'"),
        (vec!["--help", "demuxer=mov"], "'--help demuxer=mov'"),
        (vec!["-hide_banner", "-help", "in.mp4"], "'-help in.mp4'"),
    ] {
        let r = ffprobe(&args);
        assert_eq!(r.code, 4, "{args:?}: {}", r.stderr);
        assert!(r.stdout.is_empty(), "{args:?}");
        assert!(
            r.stderr
                .starts_with(&format!("coreav ffprobe: {named} is not supported: ")),
            "{args:?}: {}",
            r.stderr
        );
    }
}

#[test]
fn the_common_forms_print_what_ffprobe_prints() {
    let path = movie("ffprobe-forms.mp4");
    let r = ffprobe(&[
        "-v",
        "error",
        "-show_entries",
        "stream=index,codec_type",
        "-of",
        "csv=p=0",
        &path,
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "0,video\n1,audio\n");
    let r = ffprobe(&[
        "-v",
        "error",
        "-select_streams",
        "a",
        "-show_entries",
        "stream=codec_name",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
        &path,
    ]);
    assert_eq!(r.stdout, "aac\n");
    let r = ffprobe(&["-v", "quiet", "-of", "json", "-show_format", &path]);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(doc["format"]["format_name"], "mov,mp4,m4a,3gp,3g2,mj2");
    assert_eq!(doc["format"]["filename"], path.as_str());
    assert_eq!(doc["format"]["nb_streams"], 2);
    let r = ffprobe(&["-show_entries", "format=filename", "-of", "flat", &path]);
    // The flat writer escapes a backslash, as in a Windows path.
    let escaped = path.replace('\\', "\\\\");
    assert_eq!(r.stdout, format!("format.filename=\"{escaped}\"\n"));
}

/// FLAC's and ALAC's rate and bit depth are their configuration's, not
/// the sample entry's 16.16 rate, which cannot hold 96 kHz or 192 kHz.
#[test]
fn lossless_audio_takes_its_rate_and_depth_from_its_configuration() {
    for (name, entry, expected) in [
        (
            "ffprobe-flac.mp4",
            lossless_entry(b"fLaC", dfla(96_000, 24)),
            "flac,s32,96000,24\n",
        ),
        (
            "ffprobe-flac16.mp4",
            lossless_entry(b"fLaC", dfla(44_100, 16)),
            "flac,s16,44100,16\n",
        ),
        (
            "ffprobe-alac.m4a",
            lossless_entry(b"alac", alac_cookie(192_000, 16)),
            "alac,s16p,192000,16\n",
        ),
    ] {
        let path = file(name, &av_movie_with(entry, false));
        let r = ffprobe(&[
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=codec_name,sample_fmt,sample_rate,bits_per_raw_sample",
            "-of",
            "csv=p=0",
            path.to_str().unwrap(),
        ]);
        assert_eq!(r.code, 0, "{}", r.stderr);
        assert_eq!(r.stdout, expected, "{name}");
    }
}

#[test]
fn packets_are_counted_and_listed() {
    let path = movie("ffprobe-packets.mp4");
    let r = ffprobe(&[
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-count_packets",
        "-show_entries",
        "stream=nb_read_packets",
        "-of",
        "csv=p=0",
        &path,
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "4\n");
    let r = ffprobe(&[
        "-v",
        "error",
        "-read_intervals",
        "%+#2",
        "-show_entries",
        "packet=stream_index",
        "-of",
        "csv=p=0",
        &path,
    ]);
    assert_eq!(r.stdout.lines().count(), 2);
}

#[test]
fn the_binary_named_ffprobe_is_coreav_ffprobe() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("multicall");
    std::fs::create_dir_all(&dir).unwrap();
    let name = if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    };
    let link = dir.join(name);
    let _ = std::fs::remove_file(&link);
    // A link, as users make one; a copy written while other tests start
    // processes can fail to run ("text file busy").
    #[cfg(unix)]
    std::os::unix::fs::symlink(bin(), &link).unwrap();
    #[cfg(not(unix))]
    std::fs::copy(bin(), &link).unwrap();
    let path = movie("ffprobe-multicall.mp4");
    let args = [
        "-v",
        "error",
        "-show_entries",
        "stream=codec_type",
        "-of",
        "csv=p=0",
        path.as_str(),
    ];
    let through_link = run(&link, &args);
    let direct = ffprobe(&args);
    assert_eq!(through_link.code, 0, "{}", through_link.stderr);
    assert_eq!(through_link.stdout, direct.stdout);
    assert_eq!(run(&link, &["-show_frames", path.as_str()]).code, 4);
}

#[test]
fn transport_streams_are_numbered_and_their_programs_listed_as_ffprobe_does() {
    // `ffprobe` numbers the streams of the first Program Map Table after
    // the PAT first, then the others where the file first shows them from
    // its start (a PES packet before any table here, then a table read
    // before the PAT, then program 2's table); a program lists its streams
    // in its table's order, whatever their numbers (the same file through
    // `ffprobe`).
    let mut mux = ts::Mux::default();
    mux.pes(0x201, 0xC0, 90_000, &ts::adts(300), true);
    mux.pmt(0x1002, 3, &[(0x0F, 0x300)]);
    mux.pat(&[(1, 0x1000), (2, 0x1001), (3, 0x1002)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x100), (0x0F, 0x101)]);
    mux.pmt(0x1001, 2, &[(0x0F, 0x202), (0x0F, 0x201), (0x0F, 0x200)]);
    mux.pmt(0x1002, 3, &[(0x0F, 0x300)]);
    for pid in [0x100, 0x101, 0x200, 0x202, 0x300, 0x201] {
        mux.pes(pid, 0xC0, 90_000, &ts::adts(300), true);
    }
    let path = file("ffprobe-order.ts", &mux.out);
    let r = ffprobe(&[
        "-v",
        "error",
        "-show_entries",
        "program=program_id:stream=id",
        "-of",
        "csv=p=0",
        &path.to_string_lossy(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        r.stdout,
        "1,0x100\n0x101\n\n2,0x202\n0x201\n0x200\n\n3,0x300\n\n\
         0x100\n0x101\n0x201\n0x300\n0x202\n0x200\n"
    );
}
