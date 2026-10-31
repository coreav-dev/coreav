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

/// A WebM file of one Opus track with a 6.5 ms `CodecDelay` (312 samples
/// at 48 kHz): a 20 ms frame per block from `first` ms, each block with
/// its `DiscardPadding` in nanoseconds when it has one.
fn opus_webm(name: &str, first: u64, paddings: &[Option<i64>]) -> String {
    let blocks: Vec<(u8, Option<i64>)> = paddings.iter().map(|&p| (1, p)).collect();
    opus_blocks(name, 48_000, first, &blocks)
}

/// A WebM file of one Opus track with a 6.5 ms `CodecDelay` that states
/// `rate` as its input rate: from `first` ms, a block per entry of
/// `blocks` with that many 20 ms frames (laced when more than one), and
/// its `DiscardPadding` in nanoseconds when it has one.
fn opus_blocks(name: &str, rate: u32, first: u64, blocks: &[(u8, Option<i64>)]) -> String {
    use mkv::{BLOCK_GROUP, TRACKS, audio_track, cat, cluster, el, info, sint, uint};
    let frame = [0xFC, 1, 2, 3];
    let mut at = 0i16;
    let blocks: Vec<Vec<u8>> = blocks
        .iter()
        .map(|&(frames, padding)| {
            let timestamp = at;
            at += 20 * i16::from(frames);
            if frames == 1 && padding.is_none() {
                return mkv::simple_block(1, timestamp, true, &frame);
            }
            let mut block = vec![0x81];
            block.extend(timestamp.to_be_bytes());
            if frames == 1 {
                block.push(0);
            } else {
                block.extend([0x04, frames - 1]);
            }
            for _ in 0..frames {
                block.extend(frame);
            }
            let padding = padding.map(|ns| sint(0x75A2, ns)).unwrap_or_default();
            el(BLOCK_GROUP, &cat(&[el(0xA1, &block), padding]))
        })
        .collect();
    let mut head = b"OpusHead\x01\x02\x38\x01".to_vec();
    head.extend(rate.to_le_bytes());
    head.extend([0, 0, 0]);
    let track = audio_track(
        1,
        "A_OPUS",
        f64::from(rate),
        2,
        &cat(&[
            uint(0x56AA, 6_500_000),
            uint(0x23E383, 20_000_000),
            el(0x63A2, &head),
        ]),
    );
    let data = mkv::file(
        "webm",
        &[
            info(Some(100.0)),
            el(TRACKS, &track),
            cluster(first, &blocks),
        ],
    );
    file(name, &data).to_string_lossy().into_owned()
}

fn packet_lines(path: &str) -> Vec<String> {
    let r = ffprobe(&["-v", "error", "-of", "compact", "-show_packets", path]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    r.stdout.lines().map(str::to_owned).collect()
}

fn skip_samples(skip: u64, discard: u64) -> String {
    format!(
        "side_datum/skip_samples:side_data_type=Skip Samples|\
         side_datum/skip_samples:skip_samples={skip}|\
         side_datum/skip_samples:discard_padding={discard}|\
         side_datum/skip_samples:skip_reason=0|side_datum/skip_samples:discard_reason=0"
    )
}

#[test]
fn an_audio_stream_starts_with_the_first_sample_its_codec_delay_leaves() {
    // RFC 9559 `CodecDelay`: presentation times are 6.5 ms (7 ticks)
    // before the block timestamps, and the 312 samples of that delay are
    // discarded, so the first one played is at the first block's own
    // timestamp. Values found on the pinned ffprobe with the same file.
    let path = opus_webm("ffprobe-codec-delay.webm", 1000, &[None, None, None]);
    let r = ffprobe(&[
        "-v",
        "error",
        "-of",
        "compact",
        "-show_entries",
        "stream=start_pts,start_time,initial_padding:format=start_time",
        &path,
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        r.stdout,
        "stream|initial_padding=312|start_pts=1000|start_time=1.000000\n\
         format|start_time=1.000000\n"
    );
    let packets = packet_lines(&path);
    assert!(packets[0].contains("|pts=993|"), "{}", packets[0]);
    assert!(
        packets[0].ends_with(&skip_samples(312, 0)),
        "{}",
        packets[0]
    );
    assert!(packets[1].ends_with("flags=K__"), "{}", packets[1]);
}

#[test]
fn discard_padding_is_discarded_at_the_end_or_skipped_at_the_start_by_its_sign() {
    // RFC 9559 `DiscardPadding`: padding at the end of a block when
    // positive, at its start when negative; none when zero. Samples round
    // to the nearest (577 and 241 here), as the pinned ffprobe gives them.
    let path = opus_webm(
        "ffprobe-discard-padding.webm",
        0,
        &[None, Some(12_010_417), Some(-5_010_417), Some(0)],
    );
    let packets = packet_lines(&path);
    assert!(
        packets[0].ends_with(&skip_samples(312, 0)),
        "{}",
        packets[0]
    );
    assert!(
        packets[1].ends_with(&skip_samples(0, 577)),
        "{}",
        packets[1]
    );
    assert!(
        packets[2].ends_with(&skip_samples(241, 0)),
        "{}",
        packets[2]
    );
    assert!(packets[3].ends_with("flags=K__"), "{}", packets[3]);
}

#[test]
fn a_first_block_discards_its_padding_beside_the_codec_delay() {
    // Both elements name samples a decoder discards (RFC 9559), so a first
    // block with padding has both. The pinned ffprobe drops the padding of
    // the first block (docs/known-divergences-ffprobe9.md).
    let path = opus_webm("ffprobe-first-padding.webm", 0, &[Some(12_000_000), None]);
    assert!(packet_lines(&path)[0].ends_with(&skip_samples(312, 576)));
    let path = opus_webm("ffprobe-first-negative.webm", 0, &[Some(-5_000_000), None]);
    assert!(packet_lines(&path)[0].ends_with(&skip_samples(552, 0)));
}

#[test]
fn discard_padding_pads_a_laced_block_once() {
    // RFC 9559 `DiscardPadding` pads the block: the start of its first
    // frame when negative, the end of its last when positive. The pinned
    // ffprobe gives every frame of the lace the whole padding
    // (docs/known-divergences-ffprobe9.md).
    let path = opus_blocks(
        "ffprobe-laced-padding.webm",
        48_000,
        0,
        &[(1, None), (3, Some(-5_000_000)), (3, Some(12_000_000))],
    );
    let packets = packet_lines(&path);
    assert_eq!(packets.len(), 7, "{packets:?}");
    let expected = [
        Some(skip_samples(312, 0)),
        Some(skip_samples(240, 0)),
        None,
        None,
        None,
        None,
        Some(skip_samples(0, 576)),
    ];
    for (line, expected) in packets.iter().zip(expected) {
        match expected {
            Some(side_data) => assert!(line.ends_with(&side_data), "{line}"),
            None => assert!(line.ends_with("flags=K__"), "{line}"),
        }
    }
}

#[test]
fn discard_padding_longer_than_a_frame_spans_the_lace() {
    // Padding that outlasts one 960-sample frame of a lace comes off the
    // next frames too, so the samples trimmed are the padding, capped at
    // the lace's: 35 ms is 1,680 samples, 960 and 720; 80 ms takes the
    // whole lace of 2,880.
    let path = opus_blocks(
        "ffprobe-laced-long-padding.webm",
        48_000,
        0,
        &[
            (1, None),
            (3, Some(-35_000_000)),
            (3, Some(35_000_000)),
            (3, Some(-80_000_000)),
            (3, Some(80_000_000)),
        ],
    );
    let packets = packet_lines(&path);
    assert_eq!(packets.len(), 13, "{packets:?}");
    let expected = [
        Some((312, 0)),
        Some((960, 0)),
        Some((720, 0)),
        None,
        None,
        Some((0, 720)),
        Some((0, 960)),
        Some((960, 0)),
        Some((960, 0)),
        Some((960, 0)),
        Some((0, 960)),
        Some((0, 960)),
        Some((0, 960)),
    ];
    for (line, expected) in packets.iter().zip(expected) {
        match expected {
            Some((skip, discard)) => {
                assert!(line.ends_with(&skip_samples(skip, discard)), "{line}");
            }
            None => assert!(line.ends_with("flags=K__"), "{line}"),
        }
    }
}

#[test]
fn opus_counts_its_delay_and_padding_at_48_khz() {
    // Opus decodes at 48 kHz whatever input rate the track and its
    // `OpusHead` state (RFC 7845 5.1), so 6.5 ms of `CodecDelay` is 312
    // samples and 5 ms of padding 240, as the pinned ffprobe gives them.
    let path = opus_blocks(
        "ffprobe-opus-24k.webm",
        24_000,
        0,
        &[(1, None), (1, Some(-5_000_000)), (1, Some(5_000_000))],
    );
    let r = ffprobe(&[
        "-v",
        "error",
        "-of",
        "compact",
        "-show_entries",
        "stream=initial_padding",
        &path,
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "stream|initial_padding=312\n");
    let packets = packet_lines(&path);
    assert!(packets[0].contains("|duration=20|"), "{}", packets[0]);
    assert!(
        packets[0].ends_with(&skip_samples(312, 0)),
        "{}",
        packets[0]
    );
    assert!(
        packets[1].ends_with(&skip_samples(240, 0)),
        "{}",
        packets[1]
    );
    assert!(
        packets[2].ends_with(&skip_samples(0, 240)),
        "{}",
        packets[2]
    );
}

#[test]
fn decoded_frames_discard_the_codec_delay_once() {
    // An MP3 track whose `CodecDelay` is 1,105 samples at 44.1 kHz: they
    // come off the first 1,152-sample frame only, as on the pinned ffprobe.
    use mkv::{TRACKS, audio_track, cat, cluster, el, info, uint};
    let mut frame = vec![0xFF, 0xFB, 0x90, 0x64];
    frame.resize(417, 0);
    let blocks: Vec<Vec<u8>> = (0..3)
        .map(|i| mkv::simple_block(1, i * 26, true, &frame))
        .collect();
    let track = audio_track(
        1,
        "A_MPEG/L3",
        44_100.0,
        2,
        &cat(&[uint(0x56AA, 25_056_689)]),
    );
    let data = mkv::file(
        "matroska",
        &[info(Some(78.0)), el(TRACKS, &track), cluster(0, &blocks)],
    );
    let path = file("ffprobe-codec-delay-frames.mkv", &data);
    let r = ffprobe(&[
        "-v",
        "error",
        "-of",
        "compact",
        "-show_entries",
        "frame=nb_samples",
        &path.to_string_lossy(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        r.stdout,
        "frame|nb_samples=47\nframe|nb_samples=1152\nframe|nb_samples=1152\n"
    );
}

#[test]
fn padding_counts_the_decoded_output_rate() {
    // AAC at 24 kHz whose `OutputSamplingFrequency` is 48 kHz, as SBR
    // doubles it (RFC 9559 5.1.4.1.29.2): 10 ms of `CodecDelay` is 480
    // decoded samples, as on the pinned ffprobe, and each 1,024-sample
    // frame decodes to 2,048, so 60 ms of padding on a lace of two is
    // 2,880 samples, 832 and 2,048.
    use mkv::{
        BLOCK_GROUP, TRACK_ENTRY, TRACKS, cat, cluster, el, float, info, sint, string, uint,
    };
    let frame = [
        0x21, 0x10, 0x05, 0x00, 0xA0, 0x19, 0x33, 0x87, 0xF0, 0x00, 0x00,
    ];
    let audio = el(
        0xE1,
        &cat(&[
            float(0xB5, 24_000.0),
            float(0x78B5, 48_000.0),
            uint(0x9F, 2),
        ]),
    );
    let track = el(
        TRACK_ENTRY,
        &cat(&[
            uint(0xD7, 1),
            uint(0x73C5, 1000),
            uint(0x83, 2),
            string(0x86, "A_AAC"),
            audio,
            uint(0x56AA, 10_000_000),
            el(0x63A2, &[0x13, 0x10]),
        ]),
    );
    let mut laced = vec![0x81];
    laced.extend(43i16.to_be_bytes());
    laced.extend([0x04, 1]);
    laced.extend(frame);
    laced.extend(frame);
    let blocks = vec![
        mkv::simple_block(1, 0, true, &frame),
        el(
            BLOCK_GROUP,
            &cat(&[el(0xA1, &laced), sint(0x75A2, 60_000_000)]),
        ),
    ];
    let data = mkv::file(
        "matroska",
        &[info(Some(128.0)), el(TRACKS, &track), cluster(0, &blocks)],
    );
    let path = file("ffprobe-sbr-padding.mkv", &data)
        .to_string_lossy()
        .into_owned();
    let r = ffprobe(&[
        "-v",
        "error",
        "-of",
        "compact",
        "-show_entries",
        "stream=sample_rate,initial_padding",
        &path,
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "stream|sample_rate=48000|initial_padding=480\n");
    let packets = packet_lines(&path);
    assert_eq!(packets.len(), 3, "{packets:?}");
    assert!(
        packets[0].ends_with(&skip_samples(480, 0)),
        "{}",
        packets[0]
    );
    assert!(
        packets[1].ends_with(&skip_samples(0, 832)),
        "{}",
        packets[1]
    );
    assert!(
        packets[2].ends_with(&skip_samples(0, 2048)),
        "{}",
        packets[2]
    );
}

#[test]
fn a_padded_lace_of_256_frames_is_decoded_frame_by_frame() {
    // The frames of a block with end padding wait for its last frame, but
    // their payloads do not: each is read when its frame is listed. Here
    // the most frames a lace has, 256 of 48 mono 16-bit samples, with
    // 4.5 ms (216 samples) of padding taken from the last five.
    use mkv::{
        BLOCK_GROUP, TRACK_ENTRY, TRACKS, cat, cluster, el, float, info, sint, string, uint,
    };
    let audio = el(
        0xE1,
        &cat(&[float(0xB5, 48_000.0), uint(0x9F, 1), uint(0x6264, 16)]),
    );
    let track = el(
        TRACK_ENTRY,
        &cat(&[
            uint(0xD7, 1),
            uint(0x73C5, 1000),
            uint(0x83, 2),
            string(0x86, "A_PCM/INT/LIT"),
            audio,
        ]),
    );
    let mut laced = vec![0x81, 0, 0, 0x04, 255];
    for i in 0..256u32 {
        laced.extend((0..96).map(|j| (i + j) as u8));
    }
    let blocks = vec![el(
        BLOCK_GROUP,
        &cat(&[el(0xA1, &laced), sint(0x75A2, 4_500_000)]),
    )];
    let data = mkv::file(
        "matroska",
        &[info(Some(256.0)), el(TRACKS, &track), cluster(0, &blocks)],
    );
    let path = file("ffprobe-long-padded-lace.mkv", &data);
    let r = ffprobe(&[
        "-v",
        "error",
        "-of",
        "compact",
        "-show_entries",
        "frame=nb_samples",
        &path.to_string_lossy(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let mut want = "frame|nb_samples=48\n".repeat(251);
    want.push_str("frame|nb_samples=24\n");
    assert_eq!(r.stdout, want);
}
