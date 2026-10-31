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
#[path = "../../coreav-mkv/tests/common/mod.rs"]
mod mkv;
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
fn vp8_mp4_reads_the_configuration_pixel_format_and_range() {
    for (flag, range) in [(0, "tv"), (1, "pc")] {
        let mut bytes = common::movie(&common::Spec {
            sample_entry: *b"vp08",
            ..common::Spec::default()
        });
        let at = bytes.windows(4).position(|b| b == b"avcC").unwrap() - 4;
        let old = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let config = common::bx(b"vpcC", &[1, 0, 0, 0, 0, 0, 0x80 | flag, 1, 1, 1, 0, 0]);
        for kind in [
            b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd", b"vp08",
        ] {
            let header = bytes.windows(4).position(|b| b == kind).unwrap() - 4;
            let size = u32::from_be_bytes(bytes[header..header + 4].try_into().unwrap());
            let size = size + u32::try_from(config.len() - old).unwrap();
            bytes[header..header + 4].copy_from_slice(&size.to_be_bytes());
        }
        bytes.splice(at..at + old, config);
        let path = file(&format!("ffprobe-vp8-{range}.mp4"), &bytes);
        let result = ffprobe(&[
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name,pix_fmt,color_range",
            "-of",
            "json",
            &path.to_string_lossy(),
        ]);
        assert_eq!(result.code, 0, "{}", result.stderr);
        let json: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
        assert_eq!(json["streams"][0]["codec_name"], "vp8");
        assert_eq!(json["streams"][0]["pix_fmt"], "yuv420p");
        assert_eq!(json["streams"][0]["color_range"], range);
    }
}

#[test]
fn an_option_outside_the_vocabulary_is_refused_by_name() {
    let path = movie("ffprobe-refused.mp4");
    for (args, named) in [
        (
            vec!["-show_data", "-show_packets", path.as_str()],
            "-show_data",
        ),
        (vec!["-of", "xml", "-show_streams", path.as_str()], "xml"),
        (
            vec!["-show_entries", "stream_group=index", path.as_str()],
            "stream_group",
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
    assert_eq!(
        run(&link, &["-show_data", "-show_packets", path.as_str()]).code,
        4
    );
}

fn pcm_movie(name: &str) -> String {
    file(name, &common::av_movie(b"sowt"))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn chapter_picture_packets_include_requested_payloads() {
    let mut bytes = common::av_movie(b"sowt");
    let moov = bytes.windows(4).position(|b| b == b"moov").unwrap() - 4;
    let track = bytes
        .windows(4)
        .enumerate()
        .filter(|(_, b)| *b == b"trak")
        .nth(1)
        .unwrap()
        .0
        - 4;
    let reference = common::bx(b"tref", &common::bx(b"chap", &1u32.to_be_bytes()));
    for at in [moov, track] {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        bytes[at..at + 4].copy_from_slice(&(size + reference.len() as u32).to_be_bytes());
    }
    bytes.extend_from_slice(&reference);
    for wanted in [true, false] {
        let mut source = coreav_isobmff::SliceSource::new(&bytes);
        let mut probed = coreav_cli::ffprobe::probe(
            &mut source,
            bytes.len() as u64,
            "chapter-picture.mp4",
            None,
        )
        .unwrap_or_else(|_| panic!("chapter-picture probe failed"));
        assert!(
            probed.report.streams[0]
                .disposition
                .iter()
                .any(|(key, value)| *key == "timed_thumbnails"
                    && matches!(value, coreav_cli::ffprobe::model::Value::Int(1)))
        );
        let mut first = None;
        assert!(
            probed
                .packets
                .visit(&[wanted, false], &mut |packet| {
                    first = Some(packet);
                    Ok(false)
                })
                .is_ok()
        );
        let packet = first.unwrap();
        assert_eq!(packet.stream, 0);
        if wanted {
            let at = packet.pos.unwrap() as usize;
            assert_eq!(packet.data, bytes[at..at + packet.size as usize]);
        } else {
            assert!(packet.data.is_empty());
        }
    }
}

#[test]
fn audio_frames_are_listed_and_counted() {
    let path = pcm_movie("ffprobe-frames.mov");
    let r = ffprobe(&[
        "-v",
        "error",
        "-select_streams",
        "a",
        "-show_entries",
        "frame=media_type,stream_index,pts,nb_samples,channels",
        "-of",
        "csv=p=0",
        &path,
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    // Each packet holds whole samples of both channels: 400 bytes of
    // 16-bit stereo are 100 samples, however they are split into packets.
    let mut samples = 0;
    let lines: Vec<&str> = r.stdout.lines().collect();
    for line in &lines {
        let fields: Vec<&str> = line.split(',').collect();
        assert_eq!(fields[..2], ["audio", "1"], "{line}");
        assert_eq!(fields[4], "2", "{line}");
        samples += fields[3].parse::<u32>().unwrap();
    }
    assert_eq!(samples, 100);
    let r = ffprobe(&[
        "-v",
        "error",
        "-select_streams",
        "a",
        "-count_frames",
        "-count_packets",
        "-show_entries",
        "stream=nb_read_frames,nb_read_packets",
        "-of",
        "csv=p=0",
        &path,
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, format!("{0},{0}\n", lines.len()));
}

#[test]
fn frame_counts_do_not_require_frame_records() {
    let path = pcm_movie("ffprobe-count-only.mov");
    let r = ffprobe(&[
        "-v",
        "error",
        "-select_streams",
        "a",
        "-read_intervals",
        "%+#1",
        "-count_frames",
        "-show_entries",
        "stream=nb_read_frames",
        "-of",
        "json",
        &path,
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(doc["streams"][0]["nb_read_frames"], "1");
    assert!(doc.get("frames").is_none());
}

#[test]
fn packets_and_frames_share_one_list() {
    let path = pcm_movie("ffprobe-packets-and-frames.mov");
    let r = ffprobe(&[
        "-v",
        "error",
        "-select_streams",
        "a",
        "-read_intervals",
        "%+#2",
        "-show_entries",
        "packet=pts:frame=pts",
        "-of",
        "json",
        &path,
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    let types: Vec<&str> = doc["packets_and_frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["packet", "frame", "packet", "frame"]);
    let r = ffprobe(&[
        "-v",
        "error",
        "-select_streams",
        "a",
        "-read_intervals",
        "%+#1",
        "-show_entries",
        "packet=pts:frame=pts",
        "-of",
        "flat",
        &path,
    ]);
    assert_eq!(
        r.stdout,
        "packets_and_frames.packet.0.pts=0\npackets_and_frames.frame.0.pts=0\n"
    );
}

#[test]
fn frames_coreav_cannot_decode_are_refused_before_anything_is_printed() {
    // The movie's video samples are one slice header each, not pictures:
    // a build without the H.264 decoder refuses the stream, a build with
    // it stops at the first packet. Either way nothing is printed.
    let path = movie("ffprobe-frames-refused.mp4");
    for args in [
        vec!["-show_frames", path.as_str()],
        vec!["-select_streams", "a", "-show_frames", path.as_str()],
        vec![
            "-count_frames",
            "-show_entries",
            "stream=nb_read_frames",
            path.as_str(),
        ],
    ] {
        let r = ffprobe(&args);
        assert_eq!(r.code, 4, "{args:?}: {}", r.stderr);
        assert!(r.stdout.is_empty(), "{args:?}");
        assert!(
            r.stderr.contains("needs the frames of stream"),
            "{args:?}: {}",
            r.stderr
        );
    }
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

/// The values of each listed item's side data of \`kind\`, joined by \`|\`.
fn side_data_values(stdout: &str, kind: &str) -> Vec<String> {
    let prefix = format!("side_datum/{kind}:");
    stdout
        .lines()
        .map(|line| {
            line.split('|')
                .filter_map(|item| item.strip_prefix(prefix.as_str()))
                .filter_map(|kv| kv.split_once('=').map(|(_, v)| v))
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect()
}

#[test]
fn matroska_stereo_modes_are_listed_as_stereo_3d_side_data() {
    // One VP9 track per RFC 9559 `StereoMode` value, 0 to 14; values
    // found on the pinned ffprobe with the same file. Matroska states no
    // view, eye, baseline, disparity or field of view.
    use mkv::{TRACKS, cat, cluster, el, info, uint, video_track};
    let tracks: Vec<Vec<u8>> = (0..15u64)
        .map(|mode| {
            let mut track = video_track(mode + 1, "V_VP9", 320, 240, &[]);
            let video = el(
                0xE0,
                &cat(&[uint(0xB0, 320), uint(0xBA, 240), uint(0x53B8, mode)]),
            );
            let plain = el(0xE0, &cat(&[uint(0xB0, 320), uint(0xBA, 240)]));
            let at = track.windows(plain.len()).position(|w| w == plain).unwrap();
            track.splice(at..at + plain.len(), video);
            let len = track.len() - 2;
            track[1] = 0x80 | u8::try_from(len).unwrap();
            track
        })
        .collect();
    let blocks: Vec<Vec<u8>> = (1..=15)
        .map(|n| mkv::simple_block(n, 0, true, &mkv::VP9_KEY_FRAME))
        .collect();
    let data = mkv::file(
        "matroska",
        &[
            info(Some(40.0)),
            el(TRACKS, &tracks.concat()),
            cluster(0, &blocks),
        ],
    );
    let path = file("ffprobe-stereo-modes.mkv", &data);
    let r = ffprobe(&[
        "-v",
        "error",
        "-of",
        "compact",
        "-show_streams",
        &path.to_string_lossy(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let rest = "packed|none|0|0/1|0/1";
    let want: Vec<String> = [
        Some(("2D", 0)),
        Some(("side by side", 0)),
        Some(("top and bottom", 1)),
        Some(("top and bottom", 0)),
        Some(("checkerboard", 1)),
        Some(("checkerboard", 0)),
        Some(("interleaved lines", 1)),
        Some(("interleaved lines", 0)),
        Some(("interleaved columns", 1)),
        Some(("interleaved columns", 0)),
        None,
        Some(("side by side", 1)),
        None,
        Some(("frame alternate", 0)),
        Some(("frame alternate", 1)),
    ]
    .iter()
    .map(|m| match m {
        Some((typ, inverted)) => format!("Stereo 3D|{typ}|{inverted}|{rest}"),
        None => String::new(),
    })
    .collect();
    assert_eq!(side_data_values(&r.stdout, "stereo_3d"), want);
}

#[test]
fn only_codec_defined_block_additions_are_listed() {
    // RFC 9559 `BlockAddID`: 1, also when left out, is data the codec
    // defines; another value needs the track's mapping, which this track
    // does not have. Values found on the pinned ffprobe.
    use mkv::{BLOCK_GROUP, TRACKS, cat, cluster, el, info, uint, video_track};
    let group = |i: i16, ids: &[Option<u64>]| {
        let mut block = vec![0x81];
        block.extend((i * 40).to_be_bytes());
        block.push(0);
        block.extend(mkv::VP9_KEY_FRAME);
        let mores: Vec<Vec<u8>> = ids
            .iter()
            .map(|id| {
                let add_id = id.map(|v| uint(0xEE, v)).unwrap_or_default();
                el(0xA6, &cat(&[add_id, el(0xA5, &[1, 2, 3, 4])]))
            })
            .collect();
        el(
            BLOCK_GROUP,
            &cat(&[el(0xA1, &block), el(0x75A1, &mores.concat())]),
        )
    };
    let blocks = vec![
        group(0, &[Some(1)]),
        group(1, &[None]),
        group(2, &[Some(2), Some(1)]),
        group(3, &[Some(4)]),
    ];
    let track = video_track(1, "V_VP9", 320, 240, &[]);
    let data = mkv::file(
        "matroska",
        &[info(Some(160.0)), el(TRACKS, &track), cluster(0, &blocks)],
    );
    let path = file("ffprobe-block-additions.mkv", &data);
    let r = ffprobe(&[
        "-v",
        "error",
        "-of",
        "compact",
        "-show_packets",
        &path.to_string_lossy(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let listed = "Matroska BlockAdditional|1";
    assert_eq!(
        side_data_values(&r.stdout, "matroska_blockadditional"),
        [listed, listed, listed, ""]
    );
}

#[test]
fn a_malformed_block_addition_leaves_the_packets_after_it() {
    // A `BlockAddID` longer than eight bytes, and a header cut off at the
    // end of its `BlockMore`: each frame is listed without an addition,
    // and the packets after it are listed.
    use mkv::{BLOCK_GROUP, TRACKS, cat, cluster, el, info, uint, video_track};
    let group = |i: i16, additions: &[u8]| {
        let mut block = vec![0x81];
        block.extend((i * 40).to_be_bytes());
        block.push(0);
        block.extend(mkv::VP9_KEY_FRAME);
        el(
            BLOCK_GROUP,
            &cat(&[el(0xA1, &block), el(0x75A1, additions)]),
        )
    };
    let valid = el(0xA6, &cat(&[uint(0xEE, 1), el(0xA5, &[1, 2, 3, 4])]));
    let long_id = el(0xA6, &[0xEE, 0x89, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    let cut_off = el(0xA6, &[0xEE]);
    let blocks = vec![
        group(0, &long_id),
        group(1, &valid),
        group(2, &cut_off),
        group(3, &valid),
    ];
    let track = video_track(1, "V_VP9", 320, 240, &[]);
    let data = mkv::file(
        "matroska",
        &[info(Some(160.0)), el(TRACKS, &track), cluster(0, &blocks)],
    );
    let path = file("ffprobe-malformed-block-additions.mkv", &data);
    let r = ffprobe(&[
        "-v",
        "error",
        "-of",
        "compact",
        "-show_packets",
        &path.to_string_lossy(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let listed = "Matroska BlockAdditional|1";
    assert_eq!(
        side_data_values(&r.stdout, "matroska_blockadditional"),
        ["", listed, "", listed]
    );
}

#[test]
fn a_block_more_without_its_addition_is_not_listed() {
    // RFC 9559 5.1.3.5.2.2: a `BlockMore` holds exactly one
    // `BlockAdditional`. Without it, of `BlockAddID` 1 or empty, the
    // block carries no addition. Values found on the pinned ffprobe.
    use mkv::{BLOCK_GROUP, TRACKS, cat, cluster, el, info, uint, video_track};
    let group = |i: i16, more: Vec<u8>| {
        let mut block = vec![0x81];
        block.extend((i * 40).to_be_bytes());
        block.push(0);
        block.extend(mkv::VP9_KEY_FRAME);
        el(
            BLOCK_GROUP,
            &cat(&[el(0xA1, &block), el(0x75A1, &el(0xA6, &more))]),
        )
    };
    let addition = el(0xA5, &[1, 2, 3, 4]);
    let blocks = vec![
        group(0, uint(0xEE, 1)),
        group(1, Vec::new()),
        group(2, cat(&[uint(0xEE, 1), addition.clone()])),
        group(3, addition),
    ];
    let track = video_track(1, "V_VP9", 320, 240, &[]);
    let data = mkv::file(
        "matroska",
        &[info(Some(160.0)), el(TRACKS, &track), cluster(0, &blocks)],
    );
    let path = file("ffprobe-empty-block-more.mkv", &data);
    let r = ffprobe(&[
        "-v",
        "error",
        "-of",
        "compact",
        "-show_packets",
        &path.to_string_lossy(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let listed = "Matroska BlockAdditional|1";
    assert_eq!(
        side_data_values(&r.stdout, "matroska_blockadditional"),
        ["", "", listed, listed]
    );
}
