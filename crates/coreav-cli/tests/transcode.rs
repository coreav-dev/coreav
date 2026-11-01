//! `coreav transcode` end to end (`docs/design/transcode.md`): usage and
//! rejected files always; with the pinned FFmpeg
//! (`/opt/coreav-oracle-bin`, or `COREAV_FFMPEG`), the pinned x264
//! (`/opt/coreav-oracle-bin/x264`, or `COREAV_X264`) and the fixtures of
//! `scripts/gen-fixtures.sh` (`COREAV_FIXTURES`, else
//! `/opt/coreav-fixtures`), transcodes of the fixtures with both encoders,
//! each summary validated against `docs/schema/transcode-v1.schema.json`,
//! and the output checked against `ffprobe` of the source: the frame count
//! and every frame's time from the first, the audio packets' data and
//! times, and a clean decode. Each check skips, with a line saying why,
//! when a program or the fixtures are missing.
#![cfg(feature = "transcode")]
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[allow(dead_code)]
mod schema;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const TRANSCODE_SCHEMA: &str = include_str!("../../../docs/schema/transcode-v1.schema.json");

/// What `COREAV_FALLBACK` names for the `coreav ffmpeg` commands: no
/// program, so that a command `coreav ffmpeg` refuses stays refused (exit
/// status 4) instead of running FFmpeg in its place, while the pinned FFmpeg
/// still decodes and encodes, which strict-safe mode would refuse too
/// (`docs/design/compat-cli.md`, "Fallback to FFmpeg").
const NO_FALLBACK: &str = "/nonexistent/coreav-fallback";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
}

/// A new, empty directory for one test.
fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("transcode-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The pinned FFmpeg and its ffprobe.
fn ffmpeg() -> Option<(PathBuf, PathBuf)> {
    let ffmpeg = std::env::var_os("COREAV_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/coreav-oracle-bin/ffmpeg"));
    let ffprobe = ffmpeg.with_file_name("ffprobe");
    (ffmpeg.is_file() && ffprobe.is_file()).then_some((ffmpeg, ffprobe))
}

/// The pinned x264.
fn x264() -> Option<PathBuf> {
    let path = std::env::var_os("COREAV_X264")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/coreav-oracle-bin/x264"));
    path.is_file().then_some(path)
}

fn fixtures() -> Option<PathBuf> {
    let dir = std::env::var_os("COREAV_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/coreav-fixtures"));
    dir.join("basic.mp4").is_file().then_some(dir)
}

/// The programs and fixtures the fixture tests need, or why they skip.
struct Tools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    x264: PathBuf,
    fixtures: PathBuf,
}

/// Whether a missing program or fixture fails the test instead of skipping
/// it: in CI, or with `COREAV_REQUIRE_FFMPEG` set.
fn required() -> bool {
    std::env::var_os("CI").is_some() || std::env::var_os("COREAV_REQUIRE_FFMPEG").is_some()
}

/// Skips, or fails when the tools are [`required`].
fn missing(why: &str) -> Option<Tools> {
    assert!(!required(), "{why}");
    eprintln!("skipping: {why}");
    None
}

fn tools() -> Option<Tools> {
    let Some((ffmpeg, ffprobe)) = ffmpeg() else {
        return missing("no pinned ffmpeg and ffprobe (set COREAV_FFMPEG)");
    };
    let Some(x264) = x264() else {
        return missing("no pinned x264 (set COREAV_X264)");
    };
    let Some(fixtures) = fixtures() else {
        return missing("no fixtures (set COREAV_FIXTURES)");
    };
    Some(Tools {
        ffmpeg,
        ffprobe,
        x264,
        fixtures,
    })
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn transcode(tools: Option<&Tools>, args: &[&str]) -> Run {
    let mut command = Command::new(bin());
    command.arg("transcode").args(args);
    if let Some(tools) = tools {
        // The fallback decoder's FFmpeg.
        command.env("COREAV_FFMPEG", &tools.ffmpeg);
    }
    let out = command.output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// The encoder arguments of each program.
fn encoders(tools: &Tools) -> Vec<(&'static str, Vec<String>)> {
    vec![
        (
            "x264",
            vec![
                "--encoder".into(),
                "x264".into(),
                "--x264".into(),
                tools.x264.to_str().unwrap().into(),
            ],
        ),
        (
            "ffmpeg",
            vec![
                "--encoder".into(),
                "ffmpeg".into(),
                "--ffmpeg".into(),
                tools.ffmpeg.to_str().unwrap().into(),
            ],
        ),
    ]
}

fn probe_json(tools: &Tools, args: &[&str], path: &Path) -> Value {
    let out = Command::new(&tools.ffprobe)
        .args(["-v", "error", "-of", "json"])
        .args(args)
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "ffprobe {args:?} {}", path.display());
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The video frames' times (`best_effort_timestamp`, in the stream's time
/// base) of the first video stream, and that time base.
fn frame_times(tools: &Tools, path: &Path) -> (Vec<i64>, String) {
    let frames = probe_json(
        tools,
        &[
            "-select_streams",
            "v:0",
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp:stream=time_base",
        ],
        path,
    );
    let times = frames["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["best_effort_timestamp"].as_i64().unwrap())
        .collect();
    let base = probe_json(
        tools,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=time_base",
        ],
        path,
    );
    (
        times,
        base["streams"][0]["time_base"]
            .as_str()
            .unwrap()
            .to_string(),
    )
}

/// Each audio stream's packets: (pts, duration, MD5 of the data).
fn audio_packets(tools: &Tools, path: &Path) -> Vec<(Option<i64>, Option<i64>, String)> {
    let packets = probe_json(
        tools,
        &[
            "-select_streams",
            "a",
            "-show_packets",
            "-show_data_hash",
            "MD5",
            "-show_entries",
            "packet=stream_index,pts,duration,data_hash",
        ],
        path,
    );
    packets["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["pts"].as_i64(),
                p["duration"].as_i64(),
                p["data_hash"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// `ffmpeg -v error -i OUT -map 0:v -f null -` exits 0 and prints nothing
/// (bar B1).
fn decodes_cleanly(tools: &Tools, path: &Path) {
    let out = Command::new(&tools.ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:v", "-f", "null", "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", path.display());
    assert!(
        out.stderr.is_empty(),
        "{}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// No spool or temporary file is left in `dir`.
fn nothing_left(dir: &Path) {
    let left: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    assert!(left.is_empty(), "left behind: {left:?}");
}

/// Seconds per tick of an `ffprobe` time base, `N/D`.
fn seconds(base: &str) -> f64 {
    let (n, d) = base.split_once('/').unwrap();
    n.parse::<f64>().unwrap() / d.parse::<f64>().unwrap()
}

fn relative(times: &[i64]) -> Vec<i64> {
    times.iter().map(|t| t - times[0]).collect()
}

#[test]
fn usage_and_rejected_files() {
    let dir = dir("usage");
    let junk = dir.join("junk.bin");
    std::fs::write(&junk, b"RIFF\x24\x00\x00\x00AVI LIST").unwrap();
    let out = dir.join("out.mp4");
    let junk_s = junk.to_str().unwrap();
    let out_s = out.to_str().unwrap();
    let run = transcode(None, &[junk_s, out_s]);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(run.stdout.is_empty());
    let error: Value = serde_json::from_str(run.stderr.trim()).unwrap();
    assert_eq!(error["schema"], "coreav.error/1");
    assert_eq!(error["kind"], "unsupported_container");
    assert!(!out.exists());
    nothing_left(&dir);
    for bad in [
        &["--scale", "0:120"][..],
        &["--scale", "-3:120"],
        &["--scale", "320"],
        &["--crf", "52"],
        &["--crf", "20", "--bitrate", "500"],
        &["--bitrate", "0"],
        &["--preset", "quick"],
        &["--audio", "mp3"],
        &["--encoder", "x265"],
        &["--threads", "0"],
        &["--audio-bitrate", "64"],
        &["--audio", "opus", "--audio-bitrate", "5"],
        &["--audio", "opus", "--audio-bitrate", "511"],
        &["--audio", "aac", "--audio-bitrate", "15"],
        &["--audio", "aac", "--audio-bitrate", "513"],
        &["--audio", "none", "--audio-bitrate", "64"],
    ] {
        let mut args = vec![junk_s, out_s];
        args.extend_from_slice(bad);
        let run = transcode(None, &args);
        assert_eq!(run.code, 2, "{bad:?}: {}", run.stderr);
    }
    // Without the libopus feature, Opus audio is refused before the input
    // is read.
    let run = transcode(None, &[junk_s, out_s, "--audio", "opus"]);
    assert_eq!(run.code, 1, "{}", run.stderr);
    let error: Value = serde_json::from_str(run.stderr.trim()).unwrap();
    if cfg!(feature = "libopus") {
        assert_eq!(error["kind"], "unsupported_container");
    } else {
        assert_eq!(error["kind"], "unsupported_feature");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("libopus feature")
        );
    }
    assert!(!out.exists());
    // The output may not be the input, standard output, or an existing
    // file without --force.
    assert_eq!(transcode(None, &[junk_s, junk_s]).code, 2);
    assert_eq!(transcode(None, &[junk_s, "-"]).code, 2);
    std::fs::write(&out, b"keep").unwrap();
    assert_eq!(transcode(None, &[junk_s, out_s]).code, 2);
    assert_eq!(std::fs::read(&out).unwrap(), b"keep");
    // The help says what the command is.
    let help = Command::new(bin())
        .args(["transcode", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&help.stdout).contains("xperimental"));
}

/// The fixtures every decoder build decodes (H.264), and those that need
/// the fallback or dav1d.
fn cases() -> Vec<&'static str> {
    let mut cases = vec![
        // B-frames (has_b_frames 2), with AAC.
        "basic.mp4",
        // Variable frame rate: 24 frames 40 ms apart, then 120 ms.
        "vfr.mp4",
        // A display matrix, kept.
        "rotate_ccw90.mp4",
        // An empty edit before the video.
        "edit_list.mp4",
        // An open GOP.
        "clip_open_gop.mp4",
        "basic.mov",
    ];
    if cfg!(feature = "ffmpeg-fallback") {
        cases.push("hevc_hvc1.mp4");
    }
    if cfg!(any(feature = "ffmpeg-fallback", feature = "libvpx")) {
        // VP9 and Opus in WebM.
        cases.push("browser.webm");
    }
    if cfg!(any(feature = "ffmpeg-fallback", feature = "dav1d")) {
        cases.push("av1.mp4");
    }
    cases
}

#[test]
fn fixtures_keep_their_frames_times_and_audio() {
    let Some(tools) = tools() else { return };
    let schema: Value = serde_json::from_str(TRANSCODE_SCHEMA).unwrap();
    let dir = dir("fixtures");
    for name in cases() {
        let source = tools.fixtures.join(name);
        let (source_times, source_base) = frame_times(&tools, &source);
        let source_audio = audio_packets(&tools, &source);
        let remuxed = dir.join(format!("{name}-remux.mp4"));
        let status = Command::new(bin())
            .arg("remux")
            .arg(&source)
            .arg(&remuxed)
            .arg("--force")
            .output()
            .unwrap();
        assert!(status.status.success(), "remux {name}");
        let remuxed_audio = audio_packets(&tools, &remuxed);
        for (encoder, args) in encoders(&tools) {
            let out = dir.join(format!("{name}-{encoder}.mp4"));
            let mut all = vec![
                source.to_str().unwrap(),
                out.to_str().unwrap(),
                "--preset",
                "veryfast",
            ];
            all.extend(args.iter().map(String::as_str));
            let run = transcode(Some(&tools), &all);
            assert_eq!(run.code, 0, "{name} {encoder}: {}", run.stderr);
            let summary: Value = serde_json::from_str(&run.stdout).unwrap();
            schema::validate(&summary, &schema, &schema, "$")
                .unwrap_or_else(|e| panic!("{name} {encoder}: {e}\n{summary:#}"));
            assert_eq!(summary["video"]["encoder"], encoder);
            // The default policy's order: CoreAV's own H.264 decoder first.
            if cfg!(feature = "native-h264") && summary["video"]["codec"] == "h264" {
                assert_eq!(summary["video"]["backend"], "native", "{name}");
            }
            let (times, base) = frame_times(&tools, &out);
            assert_eq!(
                summary["video"]["frames"].as_u64().unwrap(),
                source_times.len() as u64,
                "{name} {encoder}"
            );
            assert_eq!(times.len(), source_times.len(), "{name} {encoder}");
            // MP4 sources keep their time base, so the times are exact;
            // others within half a tick of the source's (bar B2).
            if base == source_base {
                assert_eq!(
                    relative(&times),
                    relative(&source_times),
                    "{name} {encoder}"
                );
            } else {
                let (s, o) = (seconds(&source_base), seconds(&base));
                for (a, b) in relative(&source_times).iter().zip(relative(&times)) {
                    let diff = (*a as f64 * s - b as f64 * o).abs();
                    assert!(diff <= s / 2.0 + 1e-9, "{name} {encoder}: {a} {b}");
                }
            }
            // The audio: the source's packets, timed as `coreav remux`
            // times them (bar B3).
            let audio = audio_packets(&tools, &out);
            let data = |p: &[(Option<i64>, Option<i64>, String)]| -> Vec<String> {
                p.iter().map(|x| x.2.clone()).collect()
            };
            assert_eq!(data(&audio), data(&source_audio), "{name} {encoder}");
            assert_eq!(audio, remuxed_audio, "{name} {encoder}");
            let streams = probe_json(
                &tools,
                &["-show_entries", "stream=codec_name,profile"],
                &out,
            );
            assert_eq!(streams["streams"][0]["codec_name"], "h264", "{name}");
            decodes_cleanly(&tools, &out);
            nothing_left(&dir);
        }
    }
}

/// B-frames in an MP4 without composition offsets (raw H.264 that FFmpeg
/// wrote to MP4 with stream copy): the samples' times are their decode
/// times, so the frames come out of the decoder with times out of order.
/// Every presented frame is encoded, given the presented times in
/// increasing order, where dropping each frame whose time is not above
/// the last one's would drop half of them.
#[test]
fn times_in_decode_order_keep_every_frame() {
    let Some(tools) = tools() else { return };
    let dir = dir("decode-order");
    let source = tools.fixtures.join("size_inband_sps.mp4");
    let (source_times, _) = frame_times(&tools, &source);
    // The presented samples' times: the media edit starts at 0.
    let packets = probe_json(
        &tools,
        &["-select_streams", "v:0", "-show_entries", "packet=pts,dts"],
        &source,
    );
    let mut expected: Vec<i64> = packets["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            assert_eq!(p["pts"], p["dts"], "the fixture has composition offsets");
            p["pts"].as_i64().unwrap()
        })
        .filter(|t| *t >= 0)
        .collect();
    expected.sort_unstable();
    assert_eq!(expected.len(), source_times.len());
    for (encoder, args) in encoders(&tools) {
        let out = dir.join(format!("{encoder}.mp4"));
        let mut all = vec![
            source.to_str().unwrap(),
            out.to_str().unwrap(),
            "--preset",
            "ultrafast",
        ];
        all.extend(args.iter().map(String::as_str));
        let run = transcode(Some(&tools), &all);
        assert_eq!(run.code, 0, "{encoder}: {}", run.stderr);
        let summary: Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(summary["video"]["dropped_frames"], 0, "{encoder}");
        let (times, _) = frame_times(&tools, &out);
        assert_eq!(relative(&times), relative(&expected), "{encoder}");
        decodes_cleanly(&tools, &out);
    }
}

/// WebM blocks whose millisecond times repeat (bursts of three frames
/// 40 ms apart, at 0, 0, 0, 40, 40, 40, ... ms): every frame is encoded,
/// each repeat one tick after the frame before, where dropping each frame
/// whose time is not above the last one's dropped two thirds of them.
#[test]
fn repeated_webm_times_keep_every_frame() {
    // VP8 decodes through libvpx, else the fallback.
    if !cfg!(any(feature = "ffmpeg-fallback", feature = "libvpx")) {
        return;
    }
    let backend = if cfg!(feature = "libvpx") {
        "libvpx"
    } else {
        "ffmpeg"
    };
    let Some(tools) = tools() else { return };
    let dir = dir("repeated-times");
    let source = dir.join("repeated-source.webm");
    let status = Command::new(&tools.ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=size=64x48:rate=30,settb=1/10000,setpts=floor(N/3)*400+mod(N\\,3)")
        .args(["-frames:v", "12", "-fps_mode", "passthrough"])
        .args(["-enc_time_base:v", "1/10000"])
        .args(["-c:v", "libvpx", "-pix_fmt", "yuv420p"])
        .arg(&source)
        .status()
        .unwrap();
    assert!(status.success());
    let (source_times, base) = frame_times(&tools, &source);
    assert_eq!(base, "1/1000");
    assert_eq!(
        source_times,
        [0, 0, 0, 40, 40, 40, 80, 80, 80, 120, 120, 120]
    );
    for (encoder, args) in encoders(&tools) {
        let out = dir.join(format!("{encoder}.mp4"));
        let mut all = vec![
            source.to_str().unwrap(),
            out.to_str().unwrap(),
            "--preset",
            "ultrafast",
        ];
        all.extend(args.iter().map(String::as_str));
        let run = transcode(Some(&tools), &all);
        assert_eq!(run.code, 0, "{encoder}: {}", run.stderr);
        let summary: Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(summary["video"]["frames"], 12, "{encoder}");
        assert_eq!(summary["video"]["dropped_frames"], 0, "{encoder}");
        assert_eq!(summary["video"]["backend"], backend, "{encoder}");
        let (times, base) = frame_times(&tools, &out);
        assert_eq!(base, "1/1000", "{encoder}");
        assert_eq!(
            relative(&times),
            [0, 1, 2, 40, 41, 42, 80, 81, 82, 120, 121, 122],
            "{encoder}"
        );
        decodes_cleanly(&tools, &out);
    }
    nothing_left(&dir);
}

#[test]
fn rotation_is_kept() {
    let Some(tools) = tools() else { return };
    let dir = dir("rotation");
    let out = dir.join("out.mp4");
    let source = tools.fixtures.join("rotate_ccw90.mp4");
    let encoder = encoders(&tools).remove(1).1;
    let mut args = vec![source.to_str().unwrap(), out.to_str().unwrap()];
    args.extend(encoder.iter().map(String::as_str));
    let run = transcode(Some(&tools), &args);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let rotation = |path: &Path| {
        probe_json(
            &tools,
            &[
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream_side_data=rotation",
            ],
            path,
        )["streams"][0]["side_data_list"][0]["rotation"]
            .clone()
    };
    assert_eq!(rotation(&out), rotation(&source));
    assert_eq!(rotation(&out), 90);
}

#[test]
fn scale_bitrate_and_no_audio() {
    let Some(tools) = tools() else { return };
    let dir = dir("options");
    let source = tools.fixtures.join("basic.mp4");
    let (source_times, _) = frame_times(&tools, &source);
    for (encoder, args) in encoders(&tools) {
        // -2:120 of 320x240: 160x120, square pixels.
        let out = dir.join(format!("scaled-{encoder}.mp4"));
        let mut all = vec![
            source.to_str().unwrap(),
            out.to_str().unwrap(),
            "--scale",
            "-2:120",
            "--bitrate",
            "300",
            "--audio",
            "none",
        ];
        all.extend(args.iter().map(String::as_str));
        let run = transcode(Some(&tools), &all);
        assert_eq!(run.code, 0, "{encoder}: {}", run.stderr);
        let summary: Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(summary["video"]["width"], 160);
        assert_eq!(summary["video"]["height"], 120);
        assert_eq!(summary["video"]["sample_aspect"], Value::Null);
        assert_eq!(summary["audio"], serde_json::json!([]));
        assert_eq!(summary["dropped"][0]["stream_index"], 1);
        assert_eq!(summary["dropped"][0]["reason"], "not_selected");
        let streams = probe_json(
            &tools,
            &["-show_entries", "stream=codec_type,width,height"],
            &out,
        );
        let streams = streams["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 1, "{encoder}: video only");
        assert_eq!(streams[0]["width"], 160);
        assert_eq!(streams[0]["height"], 120);
        let (times, _) = frame_times(&tools, &out);
        assert_eq!(relative(&times), relative(&source_times));
        decodes_cleanly(&tools, &out);
    }
    // A size that changes the shape of the pixel grid: the sample aspect
    // ratio keeps the display aspect ratio, as FFmpeg's scale filter does.
    let out = dir.join("stretched.mp4");
    let encoder = encoders(&tools).remove(0).1;
    let mut all = vec![
        source.to_str().unwrap(),
        out.to_str().unwrap(),
        "--scale",
        "-2:122",
    ];
    all.extend(encoder.iter().map(String::as_str));
    let run = transcode(Some(&tools), &all);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let summary: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(summary["video"]["width"], 162);
    assert_eq!(summary["video"]["sample_aspect"], "244:243");
    let streams = probe_json(
        &tools,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=sample_aspect_ratio",
        ],
        &out,
    );
    assert_eq!(streams["streams"][0]["sample_aspect_ratio"], "244:243");
    // An odd size for 4:2:0 is refused before the encoder starts.
    let odd = dir.join("odd.mp4");
    let mut all = vec![
        source.to_str().unwrap(),
        odd.to_str().unwrap(),
        "--scale",
        "-1:121",
    ];
    all.extend(encoder.iter().map(String::as_str));
    let run = transcode(Some(&tools), &all);
    assert_eq!(run.code, 1, "{}", run.stderr);
    let error: Value = serde_json::from_str(run.stderr.trim()).unwrap();
    assert_eq!(error["kind"], "unsupported_feature");
    assert!(!odd.exists());
    nothing_left(&dir);
}

#[test]
fn an_edit_list_that_trims_the_start() {
    let Some(tools) = tools() else { return };
    let dir = dir("trimmed");
    // `ffmpeg -ss` with stream copy keeps the frames from the keyframe
    // before 0.3 s and an edit list that starts at 0.38 s.
    let source = dir.join("trimmed-source.mp4");
    let status = Command::new(&tools.ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-ss", "0.3", "-i"])
        .arg(tools.fixtures.join("basic.mp4"))
        .args(["-c", "copy"])
        .arg(&source)
        .status()
        .unwrap();
    assert!(status.success());
    let (source_times, _) = frame_times(&tools, &source);
    let source_audio = audio_packets(&tools, &source);
    let out = dir.join("out.mp4");
    let encoder = encoders(&tools).remove(1).1;
    let mut args = vec![source.to_str().unwrap(), out.to_str().unwrap()];
    args.extend(encoder.iter().map(String::as_str));
    let run = transcode(Some(&tools), &args);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let (times, _) = frame_times(&tools, &out);
    assert_eq!(relative(&times), relative(&source_times));
    assert_eq!(audio_packets(&tools, &out), source_audio);
    // The first video frame and the first audio packet are where the
    // source has them.
    let first = |path: &Path, stream: &str, show: &str, list: &str, field: &str| {
        let entries = format!("{}={field}", &list[..list.len() - 1]);
        let v = probe_json(
            &tools,
            &["-select_streams", stream, show, "-show_entries", &entries],
            path,
        );
        v[list][0][field].clone()
    };
    for (stream, show, list, field) in [
        (
            "v:0",
            "-show_frames",
            "frames",
            "best_effort_timestamp_time",
        ),
        ("a:0", "-show_packets", "packets", "pts_time"),
    ] {
        assert_eq!(
            first(&out, stream, show, list, field),
            first(&source, stream, show, list, field),
            "{stream}"
        );
    }
    decodes_cleanly(&tools, &out);
    nothing_left(&dir);
}

/// MD5 of `ffmpeg -i PATH -map 0:v ARGS -`: the video as decoded frames
/// (`-f framemd5`, the hashes only) or as its H.264 stream.
fn video_md5(tools: &Tools, path: &Path, args: &[&str]) -> String {
    let out = Command::new(&tools.ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:v"])
        .args(args)
        .arg("-")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", path.display());
    let text = if args.contains(&"framemd5") {
        // Only the frames' hashes: the header names the time base.
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| l.rsplit(',').next().unwrap().trim().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    } else {
        out.stdout
    };
    // A short, stable digest for the messages (not cryptographic).
    format!(
        "{:x}",
        text.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
        })
    )
}

/// A variable-rate source: the encoder gets the frames' own times, as
/// libx264 gets them from `ffmpeg -c:v libx264` on the source itself, so
/// the encode is the baseline's: the same H.264 stream with the ffmpeg
/// encoder, the same decoded frames with x264 (whose headers differ).
#[test]
fn variable_rate_sources_encode_as_ffmpeg_does() {
    encodes_as_ffmpeg_does("vfr.mp4");
}

/// A source of one frame: no times to find a rate from, so FFmpeg is told
/// the source's (`coreav-transcode`'s `rawmkv`), as it finds it in the
/// source itself, and the encode is the baseline's.
#[test]
fn a_single_frame_encodes_as_ffmpeg_does() {
    encodes_as_ffmpeg_does("codec_h264_one_frame.mp4");
}

fn encodes_as_ffmpeg_does(name: &str) {
    let Some(tools) = tools() else { return };
    let dir = dir(name);
    let source = tools.fixtures.join(name);
    let baseline = dir.join("baseline.mp4");
    let status = Command::new(&tools.ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-noautorotate", "-i"])
        .arg(&source)
        .args([
            "-map",
            "0:0",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
            "-fps_mode",
            "passthrough",
        ])
        .arg(&baseline)
        .status()
        .unwrap();
    assert!(status.success());
    let stream = ["-c", "copy", "-f", "h264"];
    let frames = ["-f", "framemd5"];
    for (encoder, args) in encoders(&tools) {
        let out = dir.join(format!("{encoder}.mp4"));
        let mut all = vec![
            source.to_str().unwrap(),
            out.to_str().unwrap(),
            "--preset",
            "veryfast",
        ];
        all.extend(args.iter().map(String::as_str));
        let run = transcode(Some(&tools), &all);
        assert_eq!(run.code, 0, "{encoder}: {}", run.stderr);
        assert_eq!(
            video_md5(&tools, &out, &frames),
            video_md5(&tools, &baseline, &frames),
            "{encoder}: the decoded frames"
        );
        if encoder == "ffmpeg" {
            assert_eq!(
                video_md5(&tools, &out, &stream),
                video_md5(&tools, &baseline, &stream),
                "the H.264 stream"
            );
        }
        decodes_cleanly(&tools, &out);
        nothing_left(&dir);
    }
}

/// x264's settings string (`options: ...` of its SEI message) in the H.264
/// stream of `path`.
fn x264_options(tools: &Tools, path: &Path) -> String {
    let out = Command::new(&tools.ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:v", "-c", "copy", "-f", "h264", "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", path.display());
    let start = out
        .stdout
        .windows(9)
        .position(|w| w == b"options: ")
        .unwrap_or_else(|| panic!("no x264 settings string in {}", path.display()));
    out.stdout[start..]
        .iter()
        .take_while(|b| (b' '..=b'~').contains(*b))
        .map(|b| char::from(*b))
        .collect()
}

/// Sources below 25 frames a second: x264 then takes a minimum GOP length
/// below its default 25, from the rate FFmpeg finds for the stream
/// (`r_frame_rate`), and both encoders must be given the baseline's. In
/// the Matroska file the frames are 50 ms apart and `DefaultDuration`
/// says 100 ms, which FFmpeg takes; in the MP4 file the rate is the
/// frames' own.
#[test]
fn the_minimum_gop_follows_the_rate_ffmpeg_finds() {
    let Some(tools) = tools() else { return };
    let dir = dir("min-keyint");
    let ffmpeg = |args: &[&str]| {
        let status = Command::new(&tools.ffmpeg)
            .args(["-nostdin", "-v", "error", "-y"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "{args:?}");
    };
    let made = |rate: u32, path: &Path| {
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=size=64x48:rate={rate}"),
            "-frames:v",
            "30",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-bf",
            "0",
            "-pix_fmt",
            "yuv420p",
            path.to_str().unwrap(),
        ]);
    };
    let ten = dir.join("ten.mkv");
    made(10, &ten);
    let declared = dir.join("declared.mkv");
    ffmpeg(&[
        "-i",
        ten.to_str().unwrap(),
        "-c",
        "copy",
        "-bsf:v",
        "setts=ts=TS/2",
        declared.to_str().unwrap(),
    ]);
    let five = dir.join("five.mp4");
    made(5, &five);
    let skipped = dir.join("unlisted-track.mkv");
    ffmpeg(&[
        "-i",
        five.to_str().unwrap(),
        "-map",
        "0:v",
        "-map",
        "0:v",
        "-c",
        "copy",
        skipped.to_str().unwrap(),
    ]);
    let mut bytes = std::fs::read(&skipped).unwrap();
    let track_type = bytes.windows(3).position(|w| w == [0x83, 0x81, 1]).unwrap();
    bytes[track_type + 2] = 3;
    std::fs::write(&skipped, bytes).unwrap();
    for (source, rate, keyint_min) in [
        (&declared, "10/1", 10),
        (&five, "5/1", 5),
        (&skipped, "5/1", 5),
    ] {
        let probed = probe_json(&tools, &["-show_entries", "stream=r_frame_rate"], source);
        assert_eq!(probed["streams"][0]["r_frame_rate"], rate);
        let own = Command::new(bin())
            .args([
                "ffprobe",
                "-v",
                "error",
                "-show_entries",
                "stream=r_frame_rate",
                "-of",
                "json",
            ])
            .arg(source)
            .output()
            .unwrap();
        assert!(own.status.success());
        let own: Value = serde_json::from_slice(&own.stdout).unwrap();
        assert_eq!(own["streams"][0]["r_frame_rate"], rate);
        let baseline = dir.join("baseline.mp4");
        ffmpeg(&[
            "-noautorotate",
            "-i",
            source.to_str().unwrap(),
            "-map",
            "0:0",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
            "-fps_mode",
            "passthrough",
            baseline.to_str().unwrap(),
        ]);
        let expected = x264_options(&tools, &baseline);
        assert!(
            expected.contains(&format!(" keyint_min={keyint_min} ")),
            "{expected}"
        );
        for (encoder, args) in encoders(&tools) {
            let out = source.with_extension(format!("{encoder}.mp4"));
            let mut all = vec![
                source.to_str().unwrap(),
                out.to_str().unwrap(),
                "--preset",
                "veryfast",
            ];
            all.extend(args.iter().map(String::as_str));
            let run = transcode(Some(&tools), &all);
            assert_eq!(run.code, 0, "{encoder}: {}", run.stderr);
            if source == &skipped {
                let summary: Value = serde_json::from_str(&run.stdout).unwrap();
                assert_eq!(summary["video"]["stream_index"], 1);
            }
            assert_eq!(
                x264_options(&tools, &out),
                expected,
                "{encoder}: {}",
                source.display()
            );
        }
    }
    nothing_left(&dir);
}

#[test]
fn unsnapped_h264_times_keep_the_vui_rate() {
    let Some(tools) = tools() else { return };
    let dir = dir("vui-rate");
    let source = dir.join("source.mp4");
    let irregular = dir.join("irregular.mp4");
    for args in [
        vec![
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=5",
            "-frames:v",
            "30",
            "-c:v",
            "libx264",
            source.to_str().unwrap(),
        ],
        vec![
            "-i",
            source.to_str().unwrap(),
            "-c",
            "copy",
            "-bsf:v",
            "setts=ts=N*N*500+N*17",
            irregular.to_str().unwrap(),
        ],
    ] {
        let status = Command::new(&tools.ffmpeg)
            .args(["-nostdin", "-v", "error", "-y"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let own = Command::new(bin())
        .args([
            "ffprobe",
            "-v",
            "error",
            "-show_entries",
            "stream=r_frame_rate",
            "-of",
            "json",
        ])
        .arg(&irregular)
        .output()
        .unwrap();
    assert!(own.status.success());
    let own: Value = serde_json::from_slice(&own.stdout).unwrap();
    assert_eq!(own["streams"][0]["r_frame_rate"], "10/1");
    for (encoder, args) in encoders(&tools) {
        let out = dir.join(format!("{encoder}.mp4"));
        let mut all = vec![irregular.to_str().unwrap(), out.to_str().unwrap()];
        all.extend(args.iter().map(String::as_str));
        let run = transcode(Some(&tools), &all);
        assert_eq!(run.code, 0, "{encoder}: {}", run.stderr);
        assert!(x264_options(&tools, &out).contains(" keyint_min=10 "));
    }
    nothing_left(&dir);
}

/// A stand-in encoder: a `/bin/sh` script given as the x264 program.
#[cfg(unix)]
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(format!("{name}.sh"));
    std::fs::write(&path, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn hostile_encoders_end_in_an_error_and_leave_nothing() {
    let Some(tools) = tools() else { return };
    let dir = dir("hostile");
    let scripts = dir.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    // One real picture of the right size: fewer pictures than frames.
    let one = scripts.join("one.h264");
    let status = Command::new(&tools.ffmpeg)
        .args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=25",
            "-frames:v",
            "1",
            "-c:v",
            "libx264",
            "-f",
            "h264",
        ])
        .arg(&one)
        .status()
        .unwrap();
    assert!(status.success());
    let cases = [
        (
            "garbage",
            "cat > /dev/null\nprintf 'this is not H.264 at all'".to_owned(),
        ),
        ("nothing", "cat > /dev/null".to_owned()),
        (
            "too_few",
            format!("cat > /dev/null\ncat '{}'", one.display()),
        ),
        (
            "fails",
            "cat > /dev/null\necho 'broken' >&2\nexit 3".to_owned(),
        ),
        ("exits_early", "head -c 1000 > /dev/null\nexit 0".to_owned()),
    ];
    let source = tools.fixtures.join("basic.mp4");
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    for (name, body) in cases {
        let program = script(&scripts, name, &body);
        let out = out_dir.join(format!("{name}.mp4"));
        let run = transcode(
            Some(&tools),
            &[
                source.to_str().unwrap(),
                out.to_str().unwrap(),
                "--encoder",
                "x264",
                "--x264",
                program.to_str().unwrap(),
                "--timeout",
                "60",
            ],
        );
        assert_eq!(run.code, 1, "{name}: {}", run.stderr);
        assert!(run.stdout.is_empty(), "{name}");
        let error: Value = serde_json::from_str(run.stderr.trim())
            .unwrap_or_else(|e| panic!("{name}: {e}: {}", run.stderr));
        assert_eq!(error["schema"], "coreav.error/1", "{name}");
        assert_eq!(error["kind"], "decode", "{name}: {error}");
        assert!(
            error["message"].as_str().unwrap().starts_with("encoding:"),
            "{name}: {error}"
        );
        assert!(!out.exists(), "{name}");
        nothing_left(&out_dir);
    }
    // An encoder that never ends is stopped at the deadline.
    let program = script(&scripts, "hangs", "cat > /dev/null\nsleep 600");
    let out = out_dir.join("hangs.mp4");
    let start = std::time::Instant::now();
    let run = transcode(
        Some(&tools),
        &[
            source.to_str().unwrap(),
            out.to_str().unwrap(),
            "--encoder",
            "x264",
            "--x264",
            program.to_str().unwrap(),
            "--timeout",
            "3",
        ],
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(20));
    assert_eq!(run.code, 1, "{}", run.stderr);
    let error: Value = serde_json::from_str(run.stderr.trim()).unwrap();
    assert_eq!(error["kind"], "limit", "{error}");
    assert_eq!(error["limit"], "timeout", "{error}");
    assert!(!out.exists());
    nothing_left(&out_dir);
    // No encoder program at all.
    let run = transcode(
        Some(&tools),
        &[
            source.to_str().unwrap(),
            out.to_str().unwrap(),
            "--encoder",
            "x264",
            "--x264",
            "/nonexistent/x264",
        ],
    );
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(!out.exists());
    nothing_left(&out_dir);
}

/// `coreav ffmpeg` with `-c:v libx264` (`docs/design/transcode.md`,
/// "Through `coreav ffmpeg`"): the pipeline's encode, the audio copied; and
/// the refusals where `ffmpeg`'s defaults would give another output, at
/// exit status 4 with nothing written.
#[test]
fn ffmpeg_libx264_commands_run_the_pipeline() {
    let Some(tools) = tools() else { return };
    let dir = dir("ffmpeg-libx264");
    let run = |words: &str, source: &str, out: &Path| {
        let mut command = Command::new(bin());
        command
            .arg("ffmpeg")
            .args(words.split(' ').map(|w| match w {
                "IN" => tools.fixtures.join(source).into_os_string(),
                "OUT" => out.as_os_str().to_owned(),
                _ => w.into(),
            }))
            .env("COREAV_X264", &tools.x264)
            .env("COREAV_FFMPEG", &tools.ffmpeg)
            .env("COREAV_FALLBACK", NO_FALLBACK);
        let out = command.output().unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let out = dir.join("out.mp4");
    let (code, stderr) = run(
        "-i IN -c:v libx264 -preset veryfast -crf 23 -c:a copy OUT",
        "basic.mp4",
        &out,
    );
    assert_eq!(code, 0, "{stderr}");
    let streams = probe_json(
        &tools,
        &["-show_entries", "stream=codec_type,codec_name"],
        &out,
    );
    let kinds: Vec<(&str, &str)> = streams["streams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["codec_type"].as_str().unwrap(),
                s["codec_name"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(kinds, [("video", "h264"), ("audio", "aac")]);
    assert_eq!(
        audio_packets(&tools, &out),
        audio_packets(&tools, &tools.fixtures.join("basic.mp4"))
    );
    decodes_cleanly(&tools, &out);
    std::fs::remove_file(&out).unwrap();
    for (words, source) in [
        ("-i IN -c:v libx264 OUT", "rotate_ccw90.mp4"),
        ("-i IN -c:v libx264 OUT", "vfr.mp4"),
        ("-i IN -an -c:v libx264 OUT", "edit_list.mp4"),
        ("-i IN -an -c:v libx264 OUT", "size_inband_sps.mp4"),
        ("-i IN -an -c:v libx264 OUT", "timecode.mov"),
        ("-i IN -an -c:v libx264 OUT", "chapters.mov"),
        ("-i IN -c:v libx264 -pix_fmt yuv444p OUT", "basic.mp4"),
        ("-i IN -c:v libx264 -vf hflip OUT", "basic.mp4"),
        ("-i IN -c:v libx264 OUT", "basic.mp4"),
        ("-i IN -c copy -crf 20 OUT", "basic.mp4"),
    ] {
        let (code, stderr) = run(words, source, &out);
        assert_eq!(code, 4, "{words} on {source}: {stderr}");
        assert!(!out.exists(), "{words} on {source}");
    }
    let (code, stderr) = run(
        "-noautorotate -i IN -an -c:v libx264 -preset ultrafast OUT",
        "rotate_ccw90.mp4",
        &out,
    );
    assert_eq!(code, 0, "{stderr}");
    let (code, stderr) = run(
        "-i IN -an -c:v libx264 -preset ultrafast -fps_mode passthrough OUT",
        "vfr.mp4",
        &dir.join("vfr.mp4"),
    );
    assert_eq!(code, 0, "{stderr}");
    nothing_left(&dir);
}

/// `coreav ffmpeg -c:v libx264` keeps a cover picture only where the
/// selection copies it, as the pinned `ffmpeg` does: on `basic.mp4` with a
/// JPEG cover picture, both list the same streams and the same cover
/// picture bytes. A selection that would encode the cover picture is
/// refused; `ffmpeg` fails on it.
#[test]
fn ffmpeg_libx264_keeps_a_cover_picture_only_when_it_is_copied() {
    let Some(tools) = tools() else { return };
    let dir = dir("ffmpeg-libx264-cover");
    let ffmpeg = |args: &[&str]| {
        Command::new(&tools.ffmpeg)
            .args(["-nostdin", "-v", "error", "-y"])
            .args(args)
            .current_dir(&dir)
            .status()
            .unwrap()
            .success()
    };
    let basic = tools.fixtures.join("basic.mp4");
    assert!(ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=32x32",
        "-frames:v",
        "1",
        "cover.jpg"
    ]));
    assert!(ffmpeg(&[
        "-i",
        basic.to_str().unwrap(),
        "-i",
        "cover.jpg",
        "-map",
        "0:v",
        "-map",
        "0:a",
        "-map",
        "1",
        "-c",
        "copy",
        "-disposition:v:1",
        "attached_pic",
        "in.mp4"
    ]));
    let coreav = |words: &[&str], out: &str| {
        let out = Command::new(bin())
            .arg("ffmpeg")
            .args(["-y", "-i", "in.mp4"])
            .args(words)
            .arg(out)
            .current_dir(&dir)
            .env("COREAV_X264", &tools.x264)
            .env("COREAV_FFMPEG", &tools.ffmpeg)
            .env("COREAV_FALLBACK", NO_FALLBACK)
            .output()
            .unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    // Each stream's type, codec and attached-picture flag, and the MD5 of
    // each attached picture's one packet.
    let listing = |name: &str| {
        let path = dir.join(name);
        let streams = probe_json(
            &tools,
            &[
                "-show_entries",
                "stream=codec_type,codec_name:stream_disposition=attached_pic",
            ],
            &path,
        );
        let packets = probe_json(
            &tools,
            &[
                "-show_data_hash",
                "MD5",
                "-show_entries",
                "packet=stream_index,data_hash",
            ],
            &path,
        );
        streams["streams"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, s)| {
                let picture = s["disposition"]["attached_pic"] == 1;
                let hash = packets["packets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|p| picture && p["stream_index"] == index)
                    .map(|p| p["data_hash"].as_str().unwrap().to_owned());
                (
                    s["codec_type"].as_str().unwrap().to_owned(),
                    s["codec_name"].as_str().unwrap().to_owned(),
                    picture,
                    hash,
                )
            })
            .collect::<Vec<_>>()
    };
    let cover = listing("in.mp4").into_iter().find(|s| s.2).unwrap();
    let encode = ["-c:v", "libx264", "-preset", "ultrafast"];
    let cases: [(&[&str], bool); 3] = [
        (&["-c:a", "copy"], false),
        (&["-map", "0:v:0", "-map", "0:a", "-c:a", "copy"], false),
        (&["-map", "0", "-c:v:1", "copy", "-c:a", "copy"], true),
    ];
    for (words, kept) in cases {
        let words: Vec<&str> = encode.iter().chain(words).copied().collect();
        let (code, stderr) = coreav(&words, "coreav.mp4");
        assert_eq!(code, 0, "{words:?}: {stderr}");
        let mut args = vec!["-i", "in.mp4"];
        args.extend_from_slice(&words);
        args.push("ffmpeg.mp4");
        assert!(ffmpeg(&args), "{words:?}");
        let listed = listing("coreav.mp4");
        assert_eq!(listed, listing("ffmpeg.mp4"), "{words:?}");
        assert_eq!(
            listed.iter().find(|s| s.2) == Some(&cover),
            kept,
            "{words:?}"
        );
        assert_eq!(listed.len(), if kept { 3 } else { 2 }, "{words:?}");
    }
    let words: Vec<&str> = encode
        .iter()
        .chain(&["-map", "0", "-c:a", "copy"])
        .copied()
        .collect();
    std::fs::remove_file(dir.join("coreav.mp4")).unwrap();
    let (code, stderr) = coreav(&words, "coreav.mp4");
    assert_eq!(code, 4, "{stderr}");
    assert!(stderr.contains("encoding a cover picture"), "{stderr}");
    assert!(!dir.join("coreav.mp4").exists());
    let mut args = vec!["-i", "in.mp4"];
    args.extend_from_slice(&words);
    args.push("ffmpeg-encoded.mp4");
    assert!(!ffmpeg(&args));
}

/// A track matrix that is not a quarter turn (−45°): `ffmpeg` turns the
/// pixels unless `-noautorotate`, so `coreav ffmpeg` refuses it without
/// that option, and with it writes the matrix `ffmpeg` writes.
#[test]
fn ffmpeg_libx264_refuses_a_matrix_ffmpeg_would_apply() {
    let Some(tools) = tools() else { return };
    let dir = dir("ffmpeg-libx264-matrix");
    let ffmpeg = |args: &[&str]| {
        let status = Command::new(&tools.ffmpeg)
            .args(["-nostdin", "-v", "error", "-y"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg {args:?}");
    };
    let path = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    let (source, turned) = (path("source.mp4"), path("turned.mp4"));
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=s=64x48:d=1:r=25",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        &source,
    ]);
    ffmpeg(&[
        "-display_rotation",
        "-45",
        "-i",
        &source,
        "-c",
        "copy",
        &turned,
    ]);
    let coreav = |args: &[&str]| {
        let out = Command::new(bin())
            .arg("ffmpeg")
            .args(args)
            .env("COREAV_X264", &tools.x264)
            .env("COREAV_FFMPEG", &tools.ffmpeg)
            .env("COREAV_FALLBACK", NO_FALLBACK)
            .output()
            .unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let out = path("out.mp4");
    let (code, stderr) = coreav(&["-i", &turned, "-c:v", "libx264", &out]);
    assert_eq!(code, 4, "{stderr}");
    assert!(stderr.contains("-noautorotate"), "{stderr}");
    assert!(!Path::new(&out).exists());
    let (code, stderr) = coreav(&["-noautorotate", "-i", &turned, "-c:v", "libx264", &out]);
    assert_eq!(code, 0, "{stderr}");
    let baseline = path("baseline.mp4");
    ffmpeg(&["-noautorotate", "-i", &turned, "-c:v", "libx264", &baseline]);
    let matrix = |file: &str| {
        probe_json(
            &tools,
            &["-select_streams", "v", "-show_entries", "stream_side_data"],
            Path::new(file),
        )["streams"][0]["side_data_list"]
            .clone()
    };
    assert_eq!(matrix(&out), matrix(&baseline));
    assert_eq!(matrix(&out)[0]["rotation"], -45);
    nothing_left(&dir);
}

/// Sources the pipeline would encode with another result than `ffmpeg`,
/// or fail on only after the encode, are refused before it: copied PCM
/// audio (MOV and Matroska), copied FLAC audio from MP4 (a codec the MP4
/// writer's allowlist leaves out), repeated frame times without
/// `-fps_mode passthrough`, an H.264 left crop, and a concat list.
#[test]
fn ffmpeg_libx264_refuses_before_encoding() {
    let Some(tools) = tools() else { return };
    let dir = dir("ffmpeg-libx264-refusals");
    let ffmpeg = |args: &[&str]| {
        let status = Command::new(&tools.ffmpeg)
            .args(["-nostdin", "-v", "error", "-y"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg {args:?}");
    };
    let path = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    let video = ["-f", "lavfi", "-i", "testsrc=s=64x48:d=1:r=25"];
    let pcm = [
        "-f",
        "lavfi",
        "-i",
        "sine=d=1",
        "-c:v",
        "libx264",
        "-c:a",
        "pcm_s16le",
    ];
    let (mov, mkv, repeated) = (path("pcm.mov"), path("pcm.mkv"), path("repeated.mkv"));
    ffmpeg(&[&video[..], &pcm[..], &[mov.as_str()]].concat());
    ffmpeg(&[&video[..], &pcm[..], &[mkv.as_str()]].concat());
    let flac = path("flac.mp4");
    ffmpeg(
        &[
            &video[..],
            &[
                "-f", "lavfi", "-i", "sine=d=1", "-c:v", "libx264", "-c:a", "flac",
            ],
            &[flac.as_str()],
        ]
        .concat(),
    );
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=s=64x48:r=25,settb=1/1000,setpts=max(N-1\\,0)*40",
        "-frames:v",
        "10",
        "-fps_mode",
        "passthrough",
        "-c:v",
        "libx264",
        "-bf",
        "0",
        &repeated,
    ]);
    let (times, _) = frame_times(&tools, Path::new(&repeated));
    assert_eq!(relative(&times)[..4], [0, 0, 40, 80]);
    let crop = tools.fixtures.join("size_left_crop.mp4");
    let out = path("out.mp4");
    for (source, why) in [
        (mov.as_str(), "copying audio the MP4 writer does not take"),
        (mkv.as_str(), "copying audio the MP4 writer does not take"),
        (flac.as_str(), "copying audio the MP4 writer does not take"),
        (repeated.as_str(), "not evenly spaced"),
        (crop.to_str().unwrap(), "cropped at its left or top edge"),
    ] {
        let run = Command::new(bin())
            .args([
                "ffmpeg", "-i", source, "-c:v", "libx264", "-c:a", "copy", &out,
            ])
            .env("COREAV_X264", &tools.x264)
            .env("COREAV_FFMPEG", &tools.ffmpeg)
            .env("COREAV_FALLBACK", NO_FALLBACK)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert_eq!(run.status.code(), Some(4), "{source}: {stderr}");
        assert!(stderr.contains(why), "{source}: {stderr}");
        assert!(!Path::new(&out).exists(), "{source}");
    }
    let list = path("list.txt");
    std::fs::write(&list, format!("file '{mov}'\nfile '{mov}'\n")).unwrap();
    let run = Command::new(bin())
        .args(["ffmpeg", "-f", "concat", "-safe", "0", "-i", &list])
        .args(["-c:v", "libx264", "-c:a", "copy", &out])
        .env("COREAV_X264", &tools.x264)
        .env("COREAV_FALLBACK", NO_FALLBACK)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(run.status.code(), Some(4), "{stderr}");
    assert!(
        stderr.contains("encoding the files of a concat list"),
        "{stderr}"
    );
    assert!(!Path::new(&out).exists());
    nothing_left(&dir);
}

/// An HEVC stream whose conformance window leaves no picture is not valid
/// HEVC (ITU-T H.265 7.4.3.2.1): `coreav transcode`, `coreav thumbnail`
/// and `coreav ffmpeg` reject it as an invalid structure before decoding,
/// and write nothing.
#[test]
fn an_hevc_window_that_leaves_no_picture_is_rejected() {
    let Some(tools) = tools() else { return };
    let dir = dir("hevc-window");
    let out = dir.join("out.mp4");
    let out_s = out.to_str().unwrap();
    for name in ["size_hevc_window.mp4", "size_hevc_mixed_window.mp4"] {
        let source = tools.fixtures.join(name);
        let source_s = source.to_str().unwrap();
        let run = transcode(Some(&tools), &[source_s, out_s]);
        assert_eq!(run.code, 1, "{name}: {}", run.stderr);
        let error: Value = serde_json::from_str(run.stderr.trim()).unwrap();
        assert_eq!(error["kind"], "invalid_structure", "{name}");
        assert!(!out.exists(), "{name}");
        let image = dir.join("out.png");
        let run = Command::new(bin())
            .args(["thumbnail", source_s, "-o"])
            .arg(&image)
            .env("COREAV_FFMPEG", &tools.ffmpeg)
            .output()
            .unwrap();
        assert_eq!(run.status.code(), Some(1), "{name}");
        let error: Value = serde_json::from_slice(&run.stderr).unwrap();
        assert_eq!(error["kind"], "invalid_structure", "{name}");
        assert!(!image.exists(), "{name}");
        if !cfg!(feature = "ffmpeg-fallback") {
            // No decoder of this build takes HEVC: `coreav ffmpeg` refuses
            // the stream before the transcode reads it.
            continue;
        }
        let run = Command::new(bin())
            .args(["ffmpeg", "-i", source_s, "-c:v", "libx264", out_s])
            .env("COREAV_X264", &tools.x264)
            .env("COREAV_FFMPEG", &tools.ffmpeg)
            .env("COREAV_STRICT_SAFE", "1")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert_eq!(run.status.code(), Some(1), "{name}: {stderr}");
        assert!(stderr.contains("invalid structure"), "{name}: {stderr}");
        assert!(!out.exists(), "{name}");
    }
    nothing_left(&dir);
}

/// In a build whose decoders do not take the source's video (here HEVC,
/// with the native H.264 decoder alone), the command is refused with exit
/// status 4, so the fallback hands it to FFmpeg.
#[test]
fn ffmpeg_libx264_refuses_video_no_decoder_takes() {
    if cfg!(feature = "ffmpeg-fallback") || cfg!(feature = "dav1d") {
        return;
    }
    let Some(tools) = tools() else { return };
    let dir = dir("ffmpeg-libx264-no-decoder");
    let out = dir.join("out.mp4");
    let source = tools.fixtures.join("hevc_hvc1.mp4");
    let run = Command::new(bin())
        .args([
            "ffmpeg",
            "-i",
            source.to_str().unwrap(),
            "-an",
            "-c:v",
            "libx264",
        ])
        .arg(&out)
        .env("COREAV_X264", &tools.x264)
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(run.status.code(), Some(4), "{stderr}");
    assert!(stderr.contains("no decoder for hevc"), "{stderr}");
    assert!(!out.exists());
    nothing_left(&dir);
}

/// A source of `name` made by the pinned FFmpeg: two seconds of H.264
/// (no B-frames) and a 440 Hz sine encoded with `audio`.
fn made(tools: &Tools, dir: &Path, name: &str, audio: &[&str]) -> PathBuf {
    let path = dir.join(name);
    let status = Command::new(&tools.ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=160x120:rate=25:duration=2")
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2:sample_rate=48000",
        ])
        .args(["-c:v", "libx264", "-bf", "0", "-pix_fmt", "yuv420p"])
        .args(audio)
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success(), "{name}");
    path
}

/// The audio of `path` as `ffmpeg` decodes it: 16-bit samples at 48 kHz,
/// with the edit list applied; and what it printed.
fn decoded_audio(tools: &Tools, path: &Path) -> (Vec<u8>, String) {
    let out = Command::new(&tools.ffmpeg)
        .args(["-nostdin", "-v", "error"])
        .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
        .arg("-i")
        .arg(path)
        .args(["-map", "0:a", "-ar", "48000", "-f", "s16le", "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", path.display());
    (
        out.stdout,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[cfg(feature = "libopus")]
#[test]
fn opus_audio_takes_the_place_of_the_source_audio() {
    let Some(tools) = tools() else { return };
    let dir = dir("opus");
    let schema: Value = serde_json::from_str(TRANSCODE_SCHEMA).unwrap();
    let x264 = ["--encoder", "x264", "--x264", tools.x264.to_str().unwrap()];
    for (name, audio, codec, channels) in [
        (
            "flac-mono.mp4",
            &["-ac", "1", "-ar", "44100", "-c:a", "flac"][..],
            "flac",
            1,
        ),
        (
            "mp3-stereo.mkv",
            &["-ac", "2", "-c:a", "libmp3lame"],
            "mp3",
            2,
        ),
        ("flac-stereo.mkv", &["-ac", "2", "-c:a", "flac"], "flac", 2),
    ] {
        let source = made(&tools, &dir, name, audio);
        let out = dir.join(format!("{name}.mp4"));
        let mut args = vec![
            source.to_str().unwrap(),
            out.to_str().unwrap(),
            "--audio",
            "opus",
        ];
        args.extend(x264);
        let run = transcode(Some(&tools), &args);
        assert_eq!(run.code, 0, "{name}: {}", run.stderr);
        let summary: Value = serde_json::from_str(&run.stdout).unwrap();
        schema::validate(&summary, &schema, &schema, "$").unwrap();
        let encoded = &summary["encoded_audio"];
        assert_eq!(encoded["stream_index"], 1, "{name}");
        assert_eq!(encoded["codec"], codec);
        assert_eq!(encoded["encoder"], "libopus");
        assert_eq!(encoded["channels"], channels);
        assert_eq!(
            encoded["bit_rate"],
            if channels == 1 { 64_000 } else { 96_000 }
        );
        assert_eq!(summary["audio"], serde_json::json!([]));
        let samples = encoded["samples"].as_u64().unwrap();
        // Two seconds, to the millisecond (an MP3 decoder's own trimming).
        assert!(samples.abs_diff(96_000) <= 48, "{name}: {samples}");
        let streams = probe_json(
            &tools,
            &[
                "-show_entries",
                "stream=codec_name,channels,sample_rate,initial_padding,start_pts,duration_ts",
            ],
            &out,
        );
        let streams = streams["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 2, "{name}");
        assert_eq!(streams[1]["codec_name"], "opus");
        assert_eq!(streams[1]["channels"], channels);
        assert_eq!(streams[1]["sample_rate"], "48000");
        assert_eq!(streams[1]["initial_padding"], encoded["pre_skip"]);
        // The edit list presents every sample from time 0, and no more: it
        // leaves out the encoder's delay and the last packet's padding.
        assert_eq!(streams[1]["start_pts"], 0, "{name}");
        assert_eq!(streams[1]["duration_ts"], samples, "{name}");
        // Every packet decodes. (FFmpeg leaves out the delay, not the
        // padding: at least the presented samples.)
        let (pcm, messages) = decoded_audio(&tools, &out);
        assert!(messages.is_empty(), "{name}: {messages}");
        assert!(pcm.len() as u64 >= samples * 2 * channels, "{name}");
        decodes_cleanly(&tools, &out);
    }
    // A bit rate, and the default one's output is larger.
    let source = dir.join("flac-mono.mp4");
    let out = dir.join("low.mp4");
    let mut args = vec![
        source.to_str().unwrap(),
        out.to_str().unwrap(),
        "--audio",
        "opus",
        "--audio-bitrate",
        "16",
    ];
    args.extend(x264);
    let run = transcode(Some(&tools), &args);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let summary: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(summary["encoded_audio"]["bit_rate"], 16_000);
    let size = |p: &Path| {
        probe_json(
            &tools,
            &["-select_streams", "a", "-show_entries", "stream=bit_rate"],
            p,
        )["streams"][0]["bit_rate"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
    };
    assert!(size(&out) < size(&dir.join("flac-mono.mp4.mp4")));
    nothing_left(&dir);
}

#[cfg(feature = "libopus")]
#[test]
fn the_opus_encode_passes_over_aac_for_the_flac_after_it() {
    let Some(tools) = tools() else { return };
    let dir = dir("opus-after-aac");
    let source = made(
        &tools,
        &dir,
        "aac-flac.mp4",
        &[
            "-map", "0:v", "-map", "1:a", "-map", "1:a", "-c:a:0", "aac", "-c:a:1", "flac",
        ],
    );
    let out = dir.join("out.mp4");
    let mut args = vec![
        source.to_str().unwrap(),
        out.to_str().unwrap(),
        "--audio",
        "opus",
    ];
    args.extend(["--encoder", "x264", "--x264", tools.x264.to_str().unwrap()]);
    let run = transcode(Some(&tools), &args);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let summary: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(summary["encoded_audio"]["stream_index"], 2, "{summary}");
    assert_eq!(summary["encoded_audio"]["codec"], "flac", "{summary}");
    nothing_left(&dir);
}

fn the_flac_after_an_opus_stream_is_taken(audio: &str) {
    let Some(tools) = tools() else { return };
    let dir = dir(&format!("{audio}-after-opus"));
    let source = made(
        &tools,
        &dir,
        "opus-flac.mp4",
        &[
            "-map", "0:v", "-map", "1:a", "-map", "1:a", "-c:a:0", "opus", "-strict", "-2",
            "-c:a:1", "flac",
        ],
    );
    let out = dir.join("out.mp4");
    let mut args = vec![
        source.to_str().unwrap(),
        out.to_str().unwrap(),
        "--audio",
        audio,
    ];
    args.extend(["--encoder", "x264", "--x264", tools.x264.to_str().unwrap()]);
    let run = transcode(Some(&tools), &args);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let summary: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(summary["encoded_audio"]["stream_index"], 2, "{summary}");
    assert_eq!(summary["encoded_audio"]["codec"], "flac", "{summary}");
    nothing_left(&dir);
}

#[test]
fn the_aac_encode_takes_the_flac_after_an_opus_stream() {
    the_flac_after_an_opus_stream_is_taken("aac");
}

#[cfg(feature = "libopus")]
#[test]
fn the_opus_encode_takes_the_flac_after_an_opus_stream() {
    the_flac_after_an_opus_stream_is_taken("opus");
}

#[cfg(feature = "libopus")]
#[test]
fn audio_the_opus_encode_does_not_take_is_refused_first() {
    let Some(tools) = tools() else { return };
    let dir = dir("opus-refused");
    let x264 = ["--encoder", "x264", "--x264", tools.x264.to_str().unwrap()];
    for (name, audio, says) in [
        // The Opus encode takes no AAC, in a build that decodes it too.
        ("aac.mp4", &["-c:a", "aac"][..], "aac is not decoded"),
        // Nor Opus, which every build decodes.
        (
            "opus.mp4",
            &["-c:a", "opus", "-strict", "-2"][..],
            "opus is not decoded",
        ),
        (
            "six.mkv",
            &[
                "-af",
                "pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0",
                "-c:a",
                "flac",
            ],
            "6 channels",
        ),
    ] {
        let source = made(&tools, &dir, name, audio);
        let out = dir.join("out.mp4");
        let mut args = vec![
            source.to_str().unwrap(),
            out.to_str().unwrap(),
            "--audio",
            "opus",
        ];
        args.extend(x264);
        let run = transcode(Some(&tools), &args);
        assert_eq!(run.code, 1, "{name}: {}", run.stderr);
        let error: Value = serde_json::from_str(run.stderr.trim()).unwrap();
        assert_eq!(error["kind"], "unsupported_feature", "{name}");
        assert!(
            error["message"].as_str().unwrap().contains(says),
            "{name}: {error}"
        );
        assert!(!out.exists());
    }
    nothing_left(&dir);
}

#[test]
fn the_aac_encode_takes_the_place_of_the_source_audio() {
    let Some(tools) = tools() else { return };
    let dir = dir("aac");
    let schema: Value = serde_json::from_str(TRANSCODE_SCHEMA).unwrap();
    let x264 = ["--encoder", "x264", "--x264", tools.x264.to_str().unwrap()];
    for (name, audio, codec, channels, rate) in [
        (
            "flac-mono.mp4",
            &["-ac", "1", "-ar", "44100", "-c:a", "flac"][..],
            "flac",
            1,
            44_100,
        ),
        (
            "mp3-stereo.mkv",
            &["-ac", "2", "-c:a", "libmp3lame"],
            "mp3",
            2,
            48_000,
        ),
        (
            "flac-stereo.mkv",
            &["-ac", "2", "-ar", "32000", "-c:a", "flac"],
            "flac",
            2,
            32_000,
        ),
    ] {
        let source = made(&tools, &dir, name, audio);
        let out = dir.join(format!("{name}.mp4"));
        let mut args = vec![
            source.to_str().unwrap(),
            out.to_str().unwrap(),
            "--audio",
            "aac",
        ];
        args.extend(x264);
        let run = transcode(Some(&tools), &args);
        assert_eq!(run.code, 0, "{name}: {}", run.stderr);
        let summary: Value = serde_json::from_str(&run.stdout).unwrap();
        schema::validate(&summary, &schema, &schema, "$").unwrap();
        let encoded = &summary["encoded_audio"];
        assert_eq!(encoded["stream_index"], 1, "{name}");
        assert_eq!(encoded["codec"], codec);
        assert_eq!(encoded["encoder"], "aac");
        assert_eq!(encoded["channels"], channels);
        assert_eq!(encoded["sample_rate"], rate);
        assert_eq!(encoded["bit_rate"], Value::Null);
        assert_eq!(summary["audio"], serde_json::json!([]));
        let samples = encoded["samples"].as_u64().unwrap();
        // Two seconds at the source's rate, to the millisecond (an MP3
        // decoder's own trimming).
        assert!(
            samples.abs_diff(2 * rate) <= rate / 1000,
            "{name}: {samples}"
        );
        let delay = encoded["pre_skip"].as_u64().unwrap();
        assert_eq!(
            encoded["packets"].as_u64().unwrap(),
            (delay + samples).div_ceil(1024),
            "{name}"
        );
        let streams = probe_json(
            &tools,
            &[
                "-show_entries",
                "stream=codec_name,profile,channels,sample_rate,start_pts,duration_ts",
            ],
            &out,
        );
        let streams = streams["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 2, "{name}");
        assert_eq!(streams[1]["codec_name"], "aac");
        assert_eq!(streams[1]["profile"], "LC");
        assert_eq!(streams[1]["channels"], channels);
        assert_eq!(streams[1]["sample_rate"], rate.to_string());
        // The edit list presents every sample from time 0, and no more: it
        // leaves out the encoder's delay and the last frame's padding.
        assert_eq!(streams[1]["start_pts"], 0, "{name}");
        assert_eq!(streams[1]["duration_ts"], samples, "{name}");
        let (pcm, messages) = decoded_audio(&tools, &out);
        assert!(messages.is_empty(), "{name}: {messages}");
        assert!(!pcm.is_empty(), "{name}");
        decodes_cleanly(&tools, &out);
    }
    // A bit rate, and the default one's output is larger.
    let source = dir.join("flac-mono.mp4");
    let out = dir.join("low.mp4");
    let mut args = vec![
        source.to_str().unwrap(),
        out.to_str().unwrap(),
        "--audio",
        "aac",
        "--audio-bitrate",
        "24",
    ];
    args.extend(x264);
    let run = transcode(Some(&tools), &args);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let summary: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(summary["encoded_audio"]["bit_rate"], 24_000);
    let size = |p: &Path| {
        probe_json(
            &tools,
            &["-select_streams", "a", "-show_entries", "stream=bit_rate"],
            p,
        )["streams"][0]["bit_rate"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
    };
    assert!(size(&out) < size(&dir.join("flac-mono.mp4.mp4")));
    nothing_left(&dir);
}

#[cfg(unix)]
#[test]
fn audio_the_aac_encode_does_not_take_is_refused_first() {
    let Some(tools) = tools() else { return };
    let dir = dir("aac-refused");
    let x264 = ["--encoder", "x264", "--x264", tools.x264.to_str().unwrap()];
    let without = script(
        &dir,
        "ffmpeg-without-aac",
        "echo \"Codec 'aac' is not recognized by FFmpeg.\"",
    );
    let flac = made(&tools, &dir, "flac.mp4", &["-c:a", "flac"]);
    for (name, source, ffmpeg, says) in [
        // The AAC encode takes no AAC, in a build that decodes it too.
        (
            "aac",
            made(&tools, &dir, "aac.mp4", &["-c:a", "aac"]),
            None,
            "aac is not decoded",
        ),
        // Nor Opus, which every build decodes.
        (
            "opus",
            made(&tools, &dir, "opus.mp4", &["-c:a", "opus", "-strict", "-2"]),
            None,
            "opus is not decoded",
        ),
        (
            "six",
            made(
                &tools,
                &dir,
                "six.mkv",
                &[
                    "-af",
                    "pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0",
                    "-c:a",
                    "flac",
                ],
            ),
            None,
            "6 channels",
        ),
        (
            "96k",
            made(&tools, &dir, "96k.mkv", &["-ar", "96000", "-c:a", "flac"]),
            None,
            "at 96000 Hz",
        ),
        (
            "no-ffmpeg",
            flac.clone(),
            Some(Path::new("/nonexistent/ffmpeg")),
            "no ffmpeg program for AAC audio",
        ),
        (
            "without-aac",
            flac.clone(),
            Some(without.as_path()),
            "has no aac encoder",
        ),
    ] {
        let out = dir.join("out.mp4");
        let mut command = Command::new(bin());
        command
            .args(["transcode", source.to_str().unwrap(), out.to_str().unwrap()])
            .args(["--audio", "aac"])
            .args(x264)
            .env("COREAV_FFMPEG", &tools.ffmpeg);
        match ffmpeg {
            // Named by the variable, which finds nothing when it names no
            // file.
            Some(path) if !path.exists() => command.env("COREAV_FFMPEG", path),
            Some(path) => command.arg("--ffmpeg").arg(path),
            None => &mut command,
        };
        let run = command.output().unwrap();
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert_eq!(run.status.code(), Some(1), "{name}: {stderr}");
        let error: Value = serde_json::from_str(stderr.trim()).unwrap();
        assert_eq!(error["kind"], "unsupported_feature", "{name}");
        assert!(
            error["message"].as_str().unwrap().contains(says),
            "{name}: {error}"
        );
        assert!(!out.exists());
    }
    nothing_left(&dir);
}

/// `coreav ffmpeg` lines that encode the audio with libopus beside the
/// video with libx264 (`docs/design/transcode.md`, "Through `coreav
/// ffmpeg`"): run natively, with no fallback, and `-b:a` taken as the
/// Opus bit rate; the lines the pipeline does not take refused.
#[cfg(feature = "libopus")]
#[test]
fn ffmpeg_libopus_lines_encode_the_audio() {
    let Some(tools) = tools() else { return };
    let dir = dir("ffmpeg-libopus");
    let source = made(&tools, &dir, "in.mp4", &["-ac", "2", "-c:a", "flac"]);
    let log = dir.join("fallback.log");
    let ffmpeg = |args: &[&str], strict: bool| {
        let mut command = Command::new(bin());
        if strict {
            command.env("COREAV_STRICT_SAFE", "1");
        }
        let out = command
            .arg("ffmpeg")
            .args(args)
            .env("COREAV_FFMPEG", &tools.ffmpeg)
            .env("COREAV_X264", &tools.x264)
            .env("COREAV_FALLBACK_LOG", &log)
            .output()
            .unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8(out.stderr).unwrap(),
        )
    };
    let input = source.to_str().unwrap();
    let mut rates = Vec::new();
    for (name, rate) in [("default.mp4", None), ("low.mp4", Some("24k"))] {
        let out = dir.join(name);
        let mut args = vec!["-i", input, "-c:v", "libx264", "-c:a", "libopus"];
        args.extend(rate.iter().flat_map(|r| ["-b:a", *r]));
        args.push(out.to_str().unwrap());
        let (code, stderr) = ffmpeg(&args, false);
        assert_eq!(code, 0, "{name}: {stderr}");
        assert!(
            !log.exists() || std::fs::read(&log).unwrap().is_empty(),
            "{name}: fell back"
        );
        let streams = probe_json(
            &tools,
            &[
                "-show_entries",
                "stream=codec_name,channels,start_pts,bit_rate",
            ],
            &out,
        );
        let streams = streams["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 2, "{name}");
        assert_eq!(streams[1]["codec_name"], "opus");
        assert_eq!(streams[1]["channels"], 2);
        assert_eq!(streams[1]["start_pts"], 0);
        rates.push(
            streams[1]["bit_rate"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap(),
        );
        decodes_cleanly(&tools, &out);
    }
    assert!(rates[1] < rates[0], "{rates:?}");
    let out = dir.join("refused.mp4");
    let out = out.to_str().unwrap();
    for (args, says) in [
        (
            vec![
                "-i", input, "-c:v", "libx264", "-c:a", "libopus", "-b:a", "4k", out,
            ],
            "-b:a",
        ),
        (
            vec!["-i", input, "-c:v", "copy", "-c:a", "libopus", out],
            "libopus",
        ),
        (
            vec!["-i", input, "-c:v", "libx264", "-b:a", "64k", out],
            "-b:a",
        ),
    ] {
        let (code, stderr) = ffmpeg(&args, true);
        assert_eq!(code, 4, "{args:?}: {stderr}");
        assert!(stderr.contains(says), "{args:?}: {stderr}");
    }
    nothing_left(&dir);
}
