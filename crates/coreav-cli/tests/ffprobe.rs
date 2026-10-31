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

/// A movie of one AAC track (`mdhd` time scale 48,000, movie time scale
/// 1,000) of 10-byte samples timed by `stts` (count, delta), with
/// `media_duration` in its `mdhd`, the edit list `elst` (segment duration
/// in movie ticks, media time) and `udta` after the track
/// (ISO/IEC 14496-12 8.2 to 8.7).
fn aac_movie(
    stts: &[(u32, u32)],
    media_duration: u32,
    elst: Option<&[(u32, i32)]>,
    udta: &[u8],
) -> Vec<u8> {
    use common::{bx, cat, full, u32s};
    let count: u32 = stts.iter().map(|(n, _)| n).sum();
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    let mdat = bx(b"mdat", &vec![0xAB; count as usize * 10]);
    let chunk = (ftyp.len() + 8) as u32;
    let movie_duration = media_duration / 48;
    let mut mvhd = u32s(&[0, 0, 1000, movie_duration]);
    mvhd.extend_from_slice(&[0u8; 16]);
    mvhd.extend_from_slice(&u32s(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]));
    mvhd.extend_from_slice(&[0u8; 24]);
    mvhd.extend_from_slice(&u32s(&[2]));
    let mut tkhd = u32s(&[0, 0, 1, 0, movie_duration, 0, 0, 0, 0x0100_0000]);
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
    let mut mdhd = u32s(&[0, 0, 48_000, media_duration]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0u8; 13]);
    // AAC LC at 48 kHz, stereo (ISO/IEC 14496-3 1.6.2.1), in the decoder
    // configuration of an `esds` (ISO/IEC 14496-1 7.2.6.5 to 7.2.6.7).
    let esds = full(
        b"esds",
        0,
        0,
        &[
            3, 25, 0, 0, 0, 4, 17, 0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 2, 0x11, 0x90,
            6, 1, 2,
        ],
    );
    let mut entry = common::audio_entry(b"mp4a").split_off(8);
    entry.extend_from_slice(&esds);
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&bx(b"mp4a", &entry));
    let mut table = u32s(&[stts.len() as u32]);
    for (n, delta) in stts {
        table.extend_from_slice(&u32s(&[*n, *delta]));
    }
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(b"stsd", 0, 0, &stsd),
            full(b"stts", 0, 0, &table),
            full(b"stsc", 0, 0, &u32s(&[1, 1, count, 1])),
            full(b"stsz", 0, 0, &u32s(&[10, count])),
            full(b"stco", 0, 0, &u32s(&[1, chunk])),
        ]),
    );
    let dref = full(b"dref", 0, 0, &cat(&[u32s(&[1]), full(b"url ", 0, 1, &[])]));
    let minf = bx(
        b"minf",
        &cat(&[full(b"smhd", 0, 0, &[0u8; 4]), bx(b"dinf", &dref), stbl]),
    );
    let mdia = bx(
        b"mdia",
        &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
    );
    let mut trak = vec![full(b"tkhd", 0, 3, &tkhd)];
    if let Some(edits) = elst {
        let mut body = u32s(&[edits.len() as u32]);
        for (segment, media_time) in edits {
            body.extend_from_slice(&u32s(&[*segment, *media_time as u32, 0x10000]));
        }
        trak.push(bx(b"edts", &full(b"elst", 0, 0, &body)));
    }
    trak.push(mdia);
    let moov = bx(
        b"moov",
        &cat(&[
            full(b"mvhd", 0, 0, &mvhd),
            bx(b"trak", &cat(&trak)),
            udta.to_vec(),
        ]),
    );
    cat(&[ftyp, mdat, moov])
}

/// `udta`/`meta`/`ilst` with one iTunes `iTunSMPB` item holding `text`.
fn smpb_udta(text: &str) -> Vec<u8> {
    use common::{bx, cat, full, u32s};
    let data = cat(&[u32s(&[1, 0]), text.as_bytes().to_vec()]);
    let item = bx(
        b"----",
        &cat(&[
            full(b"mean", 0, 0, b"com.apple.iTunes"),
            full(b"name", 0, 0, b"iTunSMPB"),
            bx(b"data", &data),
        ]),
    );
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(b"mdirappl");
    hdlr.extend_from_slice(&[0u8; 9]);
    bx(
        b"udta",
        &full(
            b"meta",
            0,
            0,
            &cat(&[full(b"hdlr", 0, 0, &hdlr), bx(b"ilst", &item)]),
        ),
    )
}

/// A packet's presentation time, duration and Skip Samples side data
/// (samples skipped, padding discarded).
type Gapless = (i64, i64, Option<(i64, i64)>);

/// A file's audio stream's `initial_padding` and its packets, as
/// `coreav ffprobe` lists them.
fn gapless(name: &str, bytes: &[u8]) -> (i64, Vec<Gapless>) {
    let path = file(name, bytes);
    let r = ffprobe(&[
        "-v",
        "error",
        "-show_streams",
        "-show_packets",
        "-of",
        "json",
        &path.to_string_lossy(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    let int = |v: &serde_json::Value| v.as_i64().or_else(|| v.as_str()?.parse().ok()).unwrap();
    let padding = int(&doc["streams"][0]["initial_padding"]);
    let packets = doc["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let skip = p["side_data_list"].as_array().and_then(|list| {
                list.iter()
                    .find(|s| s["side_data_type"] == "Skip Samples")
                    .map(|s| (int(&s["skip_samples"]), int(&s["discard_padding"])))
            });
            (int(&p["pts"]), int(&p["duration"]), skip)
        })
        .collect();
    (padding, packets)
}

#[test]
fn the_last_aac_packet_discards_what_it_decodes_past_the_media_end() {
    // Six frames of 1,024 samples; the media header ends the media at
    // 5,900, so the last frame's 244 samples past it are discarded.
    let (padding, packets) = gapless(
        "gapless-mdhd.mp4",
        &aac_movie(&[(6, 1024)], 5900, None, &[]),
    );
    assert_eq!(padding, 0);
    assert_eq!(packets.last(), Some(&(5120, 1024, Some((0, 244)))));
    assert!(packets[..5].iter().all(|p| p.2.is_none()));
    // A last sample of 700 ticks still decodes to a whole frame of 1,024
    // samples (ISO/IEC 14496-3 4.5.1), 324 of them past the media's end.
    let (_, packets) = gapless(
        "gapless-short-last.mp4",
        &aac_movie(&[(5, 1024), (1, 700)], 5820, None, &[]),
    );
    assert_eq!(packets.last(), Some(&(5120, 700, Some((0, 324)))));
}

#[test]
fn a_media_edit_skips_the_samples_before_it_and_discards_those_after_it() {
    // Media time 1,500 for 100 ms (4,800 ticks): the first packet skips
    // 1,500 samples, and the presentation ends at 6,300, 868 samples
    // before the end of the frame that starts at 6,144.
    let bytes = aac_movie(&[(10, 1024)], 10240, Some(&[(100, 1500)]), &[]);
    let (padding, packets) = gapless("gapless-edit.mp4", &bytes);
    assert_eq!(padding, 1500);
    assert_eq!(packets.first(), Some(&(-1500, 1024, Some((1500, 0)))));
    assert_eq!(packets.last(), Some(&(4644, 1024, Some((0, 868)))));
    assert_eq!(packets.len(), 7);
}

#[test]
fn an_itunes_gapless_item_times_a_track_without_an_edit_list() {
    // 10,240 samples: 2,112 of encoder delay, 7,828 of content and 300 of
    // padding.
    let udta = smpb_udta(" 00000000 00000840 0000012C 0000000000001E94 00000000");
    let (padding, packets) = gapless(
        "gapless-itunes.mp4",
        &aac_movie(&[(10, 1024)], 10240, None, &udta),
    );
    assert_eq!(padding, 2112);
    assert_eq!(packets.first(), Some(&(0, 1024, Some((2112, 0)))));
    assert_eq!(packets.last(), Some(&(9216, 1024, Some((0, 300)))));
}

#[test]
fn an_empty_edit_does_not_move_where_the_media_edit_ends() {
    // An empty edit of 50 ms, then media time 1,024 for 100 ms: the media
    // edit still ends at media time 5,824 (ISO/IEC 14496-12 8.6.6), past
    // the media's end at 5,820, where the last frame has 324 samples left
    // (`docs/known-divergences-ffprobe9.md`, KD-FF9-EMPTY-EDIT-PADDING).
    let bytes = aac_movie(
        &[(5, 1024), (1, 700)],
        5820,
        Some(&[(50, -1), (100, 1024)]),
        &[],
    );
    let (padding, packets) = gapless("gapless-empty-edit.mp4", &bytes);
    assert_eq!(padding, 1024);
    assert_eq!(packets.first().map(|p| p.2), Some(Some((1024, 0))));
    assert_eq!(
        packets.last().map(|p| (p.1, p.2)),
        Some((700, Some((0, 324))))
    );
}
