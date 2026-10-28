//! `coreav transcode` end to end (`docs/design/transcode.md`): usage and
//! rejected files always; with the pinned FFmpeg
//! (`/opt/coreav-oracle-bin`, or `COREAV_FFMPEG`), the pinned x264
//! (`/opt/x264-4613ac3/bin/x264`, or `COREAV_X264`) and the fixtures of
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
        .unwrap_or_else(|| PathBuf::from("/opt/x264-4613ac3/bin/x264"));
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
    ] {
        let mut args = vec![junk_s, out_s];
        args.extend_from_slice(bad);
        let run = transcode(None, &args);
        assert_eq!(run.code, 2, "{bad:?}: {}", run.stderr);
    }
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
        // VP9 and Opus in WebM.
        cases.push("browser.webm");
        cases.push("hevc_hvc1.mp4");
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
/// whose time is not above the last one's dropped two thirds of them
/// (#586).
#[test]
fn repeated_webm_times_keep_every_frame() {
    // VP8 decodes through the fallback.
    if !cfg!(feature = "ffmpeg-fallback") {
        return;
    }
    let Some(tools) = tools() else { return };
    let dir = dir("repeated-times");
    let source = dir.join("repeated-source.webm");
    let status = Command::new(&tools.ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=size=64x48:rate=30,settb=1/10000,setpts=floor(N/3)*400+mod(N\\,3)")
        .args(["-frames:v", "12", "-fps_mode", "passthrough"])
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
    let Some(tools) = tools() else { return };
    let dir = dir("vfr");
    let source = tools.fixtures.join("vfr.mp4");
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

/// A stand-in encoder: a `/bin/sh` script given as the x264 program.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(format!("{name}.sh"));
    std::fs::write(&path, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

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
