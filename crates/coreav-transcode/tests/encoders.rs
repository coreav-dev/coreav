//! The reader on real encoder output: synthetic raw frames encoded by an
//! `x264` program and by `ffmpeg -c:v libx264`, both used as black boxes.
//!
//! Each frame carries its index in a row of black and white macroblocks,
//! so a decoded frame says which frame it is. The presentation order the
//! reader computes is checked without its own order count code: the
//! samples (with the parameter sets from its `avcC`) are written back as a
//! byte stream, and FFmpeg decodes every prefix of them in decode order; the
//! frame a prefix adds is the one its last picture encodes. The full decode
//! must give every frame once, in order, and the `avcC` must equal the one
//! FFmpeg writes for the same stream (`-c copy` to MP4).
//!
//! The programs are found through `COREAV_X264` and `COREAV_FFMPEG`, the
//! pinned paths (`/opt/x264-4613ac3/bin/x264`, `/opt/coreav-oracle-bin/ffmpeg`)
//! or `PATH`; a test whose program is missing (or an `ffmpeg` without
//! libx264) prints why and passes, unless `COREAV_REQUIRE_FFMPEG=1` (the
//! fixtures workflow, which installs the pinned FFmpeg) makes a missing
//! `ffmpeg` a failure. A missing `x264` is always a skip.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use coreav_transcode::annexb::{AnnexBReader, Config, EncodedPicture, Limits};

const WIDTH: usize = 128;
const HEIGHT: usize = 64;
const FRAMES: usize = 40;

fn program(env: &str, pinned: &str, name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(env) {
        return Some(PathBuf::from(path));
    }
    if Path::new(pinned).is_file() {
        return Some(PathBuf::from(pinned));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn x264() -> Option<PathBuf> {
    program("COREAV_X264", "/opt/x264-4613ac3/bin/x264", "x264")
}

fn ffmpeg() -> Option<PathBuf> {
    program("COREAV_FFMPEG", "/opt/coreav-oracle-bin/ffmpeg", "ffmpeg")
}

/// A missing `ffmpeg`: a skip, or a failure where it must be present.
fn missing_ffmpeg(what: &str) {
    assert!(
        std::env::var_os("COREAV_REQUIRE_FFMPEG").is_none_or(|v| v != "1"),
        "COREAV_REQUIRE_FFMPEG=1 and {what}"
    );
    eprintln!("{what}: skipped");
}

fn ffmpeg_with_libx264() -> Option<PathBuf> {
    let ffmpeg = ffmpeg()?;
    let out = Command::new(&ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .contains("libx264")
        .then_some(ffmpeg)
}

#[derive(Clone, Copy)]
enum Chroma {
    C420,
    C422,
    C444,
}

/// Raw planar frames: the top macroblock row holds the frame index in
/// binary (8 blocks, white for 1), the rest a gradient that moves.
fn frames(chroma: Chroma, depth: u32) -> Vec<u8> {
    let (cw, ch) = match chroma {
        Chroma::C420 => (WIDTH / 2, HEIGHT / 2),
        Chroma::C422 => (WIDTH / 2, HEIGHT),
        Chroma::C444 => (WIDTH, HEIGHT),
    };
    let scale = 1u16 << (depth - 8);
    let mut out = Vec::new();
    let put = |out: &mut Vec<u8>, v: u16| {
        let v = v * scale;
        if depth == 8 {
            out.push(v as u8);
        } else {
            out.extend_from_slice(&v.to_le_bytes());
        }
    };
    for i in 0..FRAMES {
        for r in 0..HEIGHT {
            for c in 0..WIDTH {
                let v = if r < 16 {
                    if (i >> (c / 16)) & 1 == 1 { 235 } else { 16 }
                } else {
                    60 + (((c + 2 * i + r) % 64) * 2) as u16
                };
                put(&mut out, v);
            }
        }
        for _ in 0..2 * cw * ch {
            put(&mut out, 128);
        }
    }
    out
}

/// Runs a program with `input` on its standard input; its standard output.
fn run(program: &Path, args: &[&str], input: Vec<u8>) -> Vec<u8> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the program starts");
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert!(
        out.status.success(),
        "{} {args:?}: {}",
        program.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

struct Read {
    config: Config,
    pictures: Vec<EncodedPicture>,
}

/// The stream through the reader in uneven chunks.
fn read(stream: &[u8]) -> Read {
    let mut reader = AnnexBReader::new(Limits {
        max_pictures: FRAMES as u64,
        ..Limits::default()
    });
    let mut pictures = Vec::new();
    let mut rest = stream;
    let mut size = 1;
    while !rest.is_empty() {
        let (chunk, tail) = rest.split_at(size.min(rest.len()));
        reader.push(chunk).unwrap();
        pictures.extend(std::iter::from_fn(|| reader.pop()));
        rest = tail;
        size = size * 3 % 4093 + 1;
    }
    reader.finish().unwrap();
    pictures.extend(std::iter::from_fn(|| reader.pop()));
    Read {
        config: reader.config().unwrap().clone(),
        pictures,
    }
}

/// The parameter sets of an `avcC` record.
fn parameter_sets(avcc: &[u8]) -> Vec<Vec<u8>> {
    assert_eq!((avcc[0], avcc[4]), (1, 0xFF));
    let mut out = Vec::new();
    let mut at = 6;
    let mut take = |count: usize, at: &mut usize| {
        for _ in 0..count {
            let n = u16::from_be_bytes([avcc[*at], avcc[*at + 1]]) as usize;
            out.push(avcc[*at + 2..*at + 2 + n].to_vec());
            *at += 2 + n;
        }
    };
    take((avcc[5] & 0x1f) as usize, &mut at);
    let pps = avcc[at] as usize;
    at += 1;
    take(pps, &mut at);
    out
}

/// The parameter sets, then the samples, as a byte stream.
fn byte_stream(config: &Config, samples: &[EncodedPicture]) -> Vec<u8> {
    let mut out = Vec::new();
    for set in parameter_sets(&config.avcc) {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&set);
    }
    for sample in samples {
        let mut rest = &sample.data[..];
        while !rest.is_empty() {
            let n = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&rest[4..4 + n]);
            rest = &rest[4 + n..];
        }
    }
    out
}

/// The indices of the frames FFmpeg decodes from a byte stream, in its
/// output (presentation) order.
fn decoded(ffmpeg: &Path, stream: Vec<u8>) -> Vec<usize> {
    let gray = run(
        ffmpeg,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "h264",
            "-i",
            "pipe:0",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "pipe:1",
        ],
        stream,
    );
    assert_eq!(gray.len() % (WIDTH * HEIGHT), 0);
    gray.chunks(WIDTH * HEIGHT)
        .map(|frame| {
            (0..8)
                .map(|bit| {
                    let mut sum = 0usize;
                    for r in 4..12 {
                        for c in bit * 16 + 4..bit * 16 + 12 {
                            sum += frame[r * WIDTH + c] as usize;
                        }
                    }
                    usize::from(sum / 64 > 128) << bit
                })
                .sum()
        })
        .collect()
}

fn find_box<'a>(file: &'a [u8], name: &[u8; 4]) -> Option<&'a [u8]> {
    let at = file.windows(4).position(|w| w == name)?;
    let size = u32::from_be_bytes(file[at - 4..at].try_into().unwrap()) as usize;
    Some(&file[at + 4..at - 4 + size])
}

/// The `avcC` FFmpeg writes for the stream, copied into MP4.
fn ffmpeg_avcc(ffmpeg: &Path, stream: &[u8]) -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!(
        "coreav-transcode-annexb-{}-{:x}",
        std::process::id(),
        stream.len() ^ (stream.iter().map(|b| *b as usize).sum::<usize>() << 20)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("in.h264");
    let output = dir.join("out.mp4");
    std::fs::write(&input, stream).unwrap();
    let status = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "h264",
            "-i",
        ])
        .arg(&input)
        .args(["-c", "copy", "-f", "mp4"])
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success());
    let mp4 = std::fs::read(&output).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    find_box(&mp4, b"avcC").expect("an avcC box").to_vec()
}

struct Expect {
    profile: u8,
    chroma: u8,
    depth: u8,
    /// Some picture is presented out of decode order.
    reordered: bool,
    /// The fewest IDR pictures.
    idr: usize,
    /// The fewest slices in every picture.
    slices: usize,
}

fn check(stream: &[u8], expect: Expect) {
    let Read { config, pictures } = read(stream);
    assert_eq!(pictures.len(), FRAMES);
    assert_eq!((config.width, config.height), (WIDTH as u32, HEIGHT as u32));
    assert_eq!(config.profile_idc, expect.profile);
    assert_eq!(config.chroma_format_idc, expect.chroma);
    assert_eq!(
        (config.bit_depth_luma, config.bit_depth_chroma),
        (expect.depth, expect.depth)
    );
    let order: Vec<u64> = pictures.iter().map(|p| p.presentation).collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..FRAMES as u64).collect::<Vec<_>>());
    assert_eq!(
        order.iter().enumerate().any(|(k, p)| *p != k as u64),
        expect.reordered,
        "{order:?}"
    );
    assert!(pictures[0].idr);
    assert!(pictures.iter().filter(|p| p.idr).count() >= expect.idr);
    for picture in &pictures {
        let mut slices = 0;
        let mut rest = &picture.data[..];
        while !rest.is_empty() {
            let n = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            let kind = rest[4] & 0x1f;
            assert!(kind != 7 && kind != 8, "a parameter set left in a sample");
            slices += usize::from(kind == 1 || kind == 5);
            rest = &rest[4 + n..];
        }
        assert!(slices >= expect.slices);
    }

    let Some(ffmpeg) = ffmpeg() else {
        missing_ffmpeg("no ffmpeg to check the decoded order");
        return;
    };
    // Every frame once, in order, from the samples and the avcC.
    let all = decoded(&ffmpeg, byte_stream(&config, &pictures));
    assert_eq!(all, (0..FRAMES).collect::<Vec<_>>());
    // The frame each picture in decode order adds.
    let mut seen = [false; FRAMES];
    for (k, picture) in pictures.iter().enumerate() {
        let frames = decoded(&ffmpeg, byte_stream(&config, &pictures[..=k]));
        assert_eq!(frames.len(), k + 1);
        let added: Vec<usize> = frames.iter().copied().filter(|f| !seen[*f]).collect();
        assert_eq!(added, [picture.presentation as usize], "picture {k}");
        seen[added[0]] = true;
    }
    assert_eq!(ffmpeg_avcc(&ffmpeg, stream), config.avcc);
}

fn x264_stream(csp: &str, depth: u32, args: &[&str]) -> Option<Vec<u8>> {
    let Some(x264) = x264() else {
        eprintln!("no x264 program: skipped");
        return None;
    };
    let chroma = match csp {
        "i422" => Chroma::C422,
        "i444" => Chroma::C444,
        _ => Chroma::C420,
    };
    let depth_arg = depth.to_string();
    let size = format!("{WIDTH}x{HEIGHT}");
    let mut all = vec![
        // Without --output-csp, this x264 (built without swscale) refuses
        // 4:2:2 and 4:4:4 input instead of encoding them as such.
        "--quiet",
        "--demuxer",
        "raw",
        "--input-csp",
        csp,
        "--output-csp",
        csp,
        "--input-depth",
        &depth_arg,
        "--output-depth",
        &depth_arg,
        "--input-res",
        &size,
        "--fps",
        "25",
    ];
    all.extend_from_slice(args);
    all.extend_from_slice(&["--output", "-", "-"]);
    Some(run(&x264, &all, frames(chroma, depth)))
}

fn high(reordered: bool) -> Expect {
    Expect {
        profile: 100,
        chroma: 1,
        depth: 8,
        reordered,
        idr: 1,
        slices: 1,
    }
}

#[test]
fn x264_ultrafast() {
    if let Some(s) = x264_stream("i420", 8, &["--preset", "ultrafast"]) {
        check(
            &s,
            Expect {
                profile: 66,
                ..high(false)
            },
        );
    }
}

#[test]
fn x264_medium() {
    if let Some(s) = x264_stream("i420", 8, &["--preset", "medium", "--crf", "23"]) {
        check(&s, high(true));
    }
}

#[test]
fn x264_sixteen_b_frames() {
    let args = [
        "--preset",
        "medium",
        "--bframes",
        "16",
        "--b-adapt",
        "0",
        "--b-pyramid",
        "normal",
    ];
    if let Some(s) = x264_stream("i420", 8, &args) {
        check(&s, high(true));
    }
}

#[test]
fn x264_short_keyint() {
    let args = ["--preset", "medium", "--keyint", "10", "--min-keyint", "1"];
    if let Some(s) = x264_stream("i420", 8, &args) {
        check(
            &s,
            Expect {
                idr: 4,
                ..high(true)
            },
        );
    }
}

#[test]
fn x264_open_gop() {
    let args = ["--preset", "medium", "--keyint", "10", "--open-gop"];
    if let Some(s) = x264_stream("i420", 8, &args) {
        check(&s, high(true));
    }
}

#[test]
fn x264_high10() {
    if let Some(s) = x264_stream("i420", 10, &["--preset", "medium"]) {
        check(
            &s,
            Expect {
                profile: 110,
                depth: 10,
                ..high(true)
            },
        );
    }
}

#[test]
fn x264_422() {
    if let Some(s) = x264_stream("i422", 8, &["--preset", "medium"]) {
        check(
            &s,
            Expect {
                profile: 122,
                chroma: 2,
                ..high(true)
            },
        );
    }
}

#[test]
fn x264_444() {
    if let Some(s) = x264_stream("i444", 8, &["--preset", "medium"]) {
        check(
            &s,
            Expect {
                profile: 244,
                chroma: 3,
                ..high(true)
            },
        );
    }
}

#[test]
fn x264_four_slices() {
    if let Some(s) = x264_stream("i420", 8, &["--preset", "medium", "--slices", "4"]) {
        check(
            &s,
            Expect {
                slices: 4,
                ..high(true)
            },
        );
    }
}

#[test]
fn x264_access_unit_delimiters() {
    if let Some(s) = x264_stream("i420", 8, &["--preset", "medium", "--aud"]) {
        check(&s, high(true));
    }
}

#[test]
fn x264_interlaced_mbaff() {
    if let Some(s) = x264_stream("i420", 8, &["--preset", "medium", "--tff"]) {
        check(&s, high(true));
    }
}

fn ffmpeg_stream(pix_fmt: &str, depth: u32, args: &[&str]) -> Option<Vec<u8>> {
    let Some(ffmpeg) = ffmpeg_with_libx264() else {
        missing_ffmpeg("no ffmpeg with libx264");
        return None;
    };
    let size = format!("{WIDTH}x{HEIGHT}");
    let mut all = vec![
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "rawvideo",
        "-pix_fmt",
        pix_fmt,
        "-video_size",
        &size,
        "-framerate",
        "25",
        "-i",
        "pipe:0",
        "-c:v",
        "libx264",
    ];
    all.extend_from_slice(args);
    all.extend_from_slice(&["-f", "h264", "pipe:1"]);
    Some(run(&ffmpeg, &all, frames(Chroma::C420, depth)))
}

#[test]
fn ffmpeg_libx264_medium() {
    if let Some(s) = ffmpeg_stream("yuv420p", 8, &["-preset", "medium"]) {
        check(&s, high(true));
    }
}

#[test]
fn ffmpeg_libx264_ultrafast() {
    if let Some(s) = ffmpeg_stream("yuv420p", 8, &["-preset", "ultrafast"]) {
        check(
            &s,
            Expect {
                profile: 66,
                ..high(false)
            },
        );
    }
}

#[test]
fn ffmpeg_libx264_high10() {
    if let Some(s) = ffmpeg_stream("yuv420p10le", 10, &["-preset", "medium"]) {
        check(
            &s,
            Expect {
                profile: 110,
                depth: 10,
                ..high(true)
            },
        );
    }
}
