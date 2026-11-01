//! Helpers of the encoder tests: the pinned programs, synthetic frames, a
//! collecting sink, and `ffprobe`.
// Test-only sources outside `#[test]` functions; each test file uses a part.
#![allow(
    dead_code,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use coreav_decode::Error;
use coreav_frame::{Chroma, Picture, PixelFormat, Plane};
use coreav_transcode::encoder::{
    Encoder, EncoderLimits, EncoderProgram, EncoderSettings, FrameRate, Sink,
};
use coreav_worker::ExecLauncher;

/// The pinned x264 program: `COREAV_X264`, else the oracle's (the build of
/// `scripts/install-x264.sh`).
pub fn x264() -> Option<PathBuf> {
    std::env::var_os("COREAV_X264")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from("/opt/coreav-oracle-bin/x264")))
        .filter(|p| p.is_file())
}

/// The pinned FFmpeg: `COREAV_FFMPEG`, else the oracle's, else `PATH`.
pub fn ffmpeg() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("COREAV_FFMPEG") {
        return Some(PathBuf::from(named)).filter(|p| p.is_file());
    }
    let oracle = PathBuf::from("/opt/coreav-oracle-bin/ffmpeg");
    if oracle.is_file() {
        return Some(oracle);
    }
    coreav_worker::find_on_path("ffmpeg")
}

/// `ffprobe` next to [`ffmpeg`], else on `PATH`.
pub fn ffprobe() -> Option<PathBuf> {
    ffmpeg()
        .and_then(|f| f.parent().map(|d| d.join("ffprobe")))
        .filter(|p| p.is_file())
        .or_else(|| coreav_worker::find_on_path("ffprobe"))
}

/// Whether a missing program fails the test instead of skipping it: in CI
/// (`CI` set, as GitHub Actions sets it), where the `check` job installs
/// both (`scripts/install-oracle-ffmpeg.sh`, `scripts/install-x264.sh`).
fn required() -> bool {
    std::env::var_os("CI").is_some_and(|v| !v.is_empty() && v != "false")
}

/// Skips (a line on standard error) or, in CI, fails.
fn missing(what: &str) {
    assert!(!required(), "{what} is missing, and CI is set");
    eprintln!("skipping: {what} is missing");
}

/// The programs to test with, with a line saying which are missing.
pub fn programs() -> Vec<EncoderProgram> {
    let mut out = Vec::new();
    match x264() {
        Some(p) => out.push(EncoderProgram::X264(p)),
        None => missing("x264 (set COREAV_X264)"),
    }
    match (ffmpeg(), ffprobe()) {
        (Some(p), Some(_)) => out.push(EncoderProgram::Ffmpeg(p)),
        _ => missing("ffmpeg or ffprobe (set COREAV_FFMPEG)"),
    }
    if ffprobe().is_none() {
        out.clear();
    }
    out
}

/// Settings for `width` × `height` frames of `format` at 25 frames a
/// second, preset `veryfast`.
pub fn settings(width: u32, height: u32, format: PixelFormat) -> EncoderSettings {
    EncoderSettings {
        preset: "veryfast".into(),
        ..EncoderSettings::new(width, height, format, FrameRate { num: 25, den: 1 })
    }
}

pub fn yuv(chroma: Chroma, depth: u8) -> PixelFormat {
    PixelFormat::Yuv { chroma, depth }
}

/// A picture whose sample at (x, y) of plane p is `f(p, x, y)`, with 8
/// bytes of padding after each row.
pub fn picture(
    width: u32,
    height: u32,
    format: PixelFormat,
    f: impl Fn(usize, usize, usize) -> u16,
) -> Picture {
    let bytes = format.sample_bytes();
    let planes = (0..format.plane_count())
        .map(|p| {
            let (cols, rows) = format.plane_size(p, width, height).unwrap();
            let (cols, rows) = (cols as usize, rows as usize);
            let stride = cols * bytes + 8;
            let mut data = vec![0xAAu8; stride * rows];
            for y in 0..rows {
                for x in 0..cols {
                    let v = f(p, x, y);
                    let at = y * stride + x * bytes;
                    if bytes == 1 {
                        data[at] = v as u8;
                    } else {
                        data[at..at + 2].copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
            Plane::new(data, stride)
        })
        .collect();
    Picture::new(width, height, format, planes).unwrap()
}

/// Frame `n` of a moving gradient.
pub fn moving(width: u32, height: u32, format: PixelFormat, n: usize) -> Picture {
    let levels = 1usize << format.depth();
    let max = levels - 1;
    picture(width, height, format, move |p, x, y| {
        let v = if p == 0 {
            (x * 3 + y * 2 + n * 4) % levels
        } else {
            levels / 2 + ((x + n) % 16) * (levels / 64)
        };
        v.min(max) as u16
    })
}

/// A sink that keeps what it gets, refusing more than `limit` bytes.
pub struct Collect {
    pub bytes: Vec<u8>,
    pub limit: usize,
}

impl Collect {
    pub fn new() -> Self {
        Self::limited(256 << 20)
    }

    pub fn limited(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
}

impl Sink for Collect {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.bytes.len() + bytes.len() > self.limit {
            return Err(Error::new(
                coreav_decode::ErrorKind::Limit,
                "the encoder's output is over the test's limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

/// Encodes `frames`, returning the byte stream.
pub fn encode(
    program: &EncoderProgram,
    settings: &EncoderSettings,
    launcher: Option<&ExecLauncher>,
    frames: &[Picture],
) -> Result<Vec<u8>, Error> {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut encoder = Encoder::spawn(
        program,
        settings,
        launcher,
        &EncoderLimits::new(8 << 30),
        deadline,
        Collect::new(),
    )?;
    for frame in frames {
        encoder.send_frame(frame.clone())?;
    }
    encoder.finish().map(|sink| sink.bytes)
}

/// A scratch directory for this test process.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("coreav-encoder-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `ffprobe -show_streams -count_frames` of an H.264 byte stream: the
/// video stream's `key=value` lines.
pub fn probe(stream: &[u8], dir: &Path, name: &str) -> Vec<(String, String)> {
    let file = dir.join(format!("{name}.h264"));
    std::fs::write(&file, stream).unwrap();
    let out = Command::new(ffprobe().unwrap())
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_streams",
            "-select_streams",
            "v:0",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// The value of `key` in [`probe`]'s lines.
pub fn field<'a>(lines: &'a [(String, String)], key: &str) -> &'a str {
    lines
        .iter()
        .find(|(k, _)| k == key)
        .map_or("", |(_, v)| v.as_str())
}

/// The frames of `stream` decoded by FFmpeg in their own format, packed.
pub fn decode(stream: &[u8], dir: &Path, name: &str) -> Vec<u8> {
    let file = dir.join(format!("{name}.h264"));
    std::fs::write(&file, stream).unwrap();
    let out = Command::new(ffmpeg().unwrap())
        .args(["-v", "error", "-i"])
        .arg(&file)
        .args(["-f", "rawvideo", "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    out.stdout
}
