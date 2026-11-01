//! The encoder program: its command line, frames to its standard input and
//! its byte stream from its standard output (`docs/design/transcode.md`,
//! "The encoder program" and "Frames to the encoder").
//!
//! The program is an operator-provided `x264` or `ffmpeg` with libx264
//! ([`EncoderProgram`], found by [`find_encoder`]). It gets raw planes on
//! standard input and writes an H.264 Annex B byte stream on standard
//! output; nothing else of CoreAV's reaches it. [`Encoder`] runs it,
//! through the sandbox launcher ([`ExecLauncher`]) when one is given, with
//! these threads:
//!
//! - a writer, which writes the frames [`Encoder::send_frame`] queued (at
//!   most [`QUEUED_FRAMES`]; with FFmpeg and the frames' times, the first
//!   frame is held until the second or the end of the input, which decide
//!   the stream's header) to its standard input;
//! - a reader, which hands every byte of its standard output to the
//!   [`Sink`], so the encoder never blocks on a full output pipe;
//! - a standard-error reader, which keeps the last [`KEPT_STDERR`] bytes
//!   for an error message and discards the rest;
//! - a keeper, which owns the process and kills its process group at the
//!   deadline, on an error, or when the encoder is dropped.
//!
//! Every wait of the caller ends at the deadline. A sink that refuses
//! bytes ends the encode with its error and kills the encoder.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use coreav_decode::{Error, ErrorKind};
use coreav_frame::{Chroma, ColorInfo, Picture, PixelFormat, Pixels};
use coreav_worker::exec;
use coreav_worker::{ExecLauncher, HugePages, SandboxLimits};

/// The encoder program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncoderProgram {
    /// An `x264` command-line program.
    X264(PathBuf),
    /// An `ffmpeg` program built with libx264, run as `ffmpeg -c:v
    /// libx264`.
    Ffmpeg(PathBuf),
}

impl EncoderProgram {
    /// The program's path.
    pub fn path(&self) -> &Path {
        match self {
            Self::X264(path) | Self::Ffmpeg(path) => path,
        }
    }

    /// `x264` or `ffmpeg`, for messages and summaries.
    pub fn name(&self) -> &'static str {
        match self {
            Self::X264(_) => "x264",
            Self::Ffmpeg(_) => "ffmpeg",
        }
    }

    /// The program's arguments for `settings` ([`x264_args`] or
    /// [`ffmpeg_args`]).
    pub fn args(&self, settings: &EncoderSettings) -> Result<Vec<String>, Error> {
        match self {
            Self::X264(_) => x264_args(settings),
            Self::Ffmpeg(_) => ffmpeg_args(settings),
        }
    }

    /// Whether the program may use transparent huge pages on frames of
    /// `format`: not x264 on samples of more than 8 bits, whose conversion
    /// of its input stalls on them
    /// (`docs/measurements/transcode-x264-10bit.md`, "Huge pages").
    pub fn huge_pages(&self, format: PixelFormat) -> HugePages {
        match (self, format) {
            (Self::X264(_), PixelFormat::Yuv { depth, .. }) if depth > 8 => HugePages::Off,
            _ => HugePages::Kept,
        }
    }
}

/// Which program `--encoder` asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EncoderChoice {
    /// An `x264` program when one is named or on `PATH`, else `ffmpeg`.
    #[default]
    Auto,
    /// `x264` only.
    X264,
    /// `ffmpeg` only.
    Ffmpeg,
}

/// The program for `choice` (`docs/design/transcode.md`, "The encoder
/// program"): `x264` is the path `x264` names (`--x264`), else the file
/// `COREAV_X264` names, else `x264` on `PATH`; `ffmpeg` is the path
/// `ffmpeg` names (`--ffmpeg`), else the file `COREAV_FFMPEG` names, else
/// `ffmpeg` on `PATH`, as for the decode fallback
/// ([`coreav_worker::find_ffmpeg`]). A variable set to something that is not
/// a file finds nothing, not the `PATH` program. `Auto` takes `x264` when
/// there is one and `ffmpeg` otherwise. A named path (with a `/`, or on
/// Windows a `/` or `\`) is taken as given, and a wrong one fails when
/// the encoder starts, with its path in the message; a bare name is looked
/// for on `PATH`, and finds nothing when it is not there.
pub fn find_encoder(
    choice: EncoderChoice,
    x264: Option<PathBuf>,
    ffmpeg: Option<PathBuf>,
) -> Option<EncoderProgram> {
    find_encoder_in(choice, x264, ffmpeg, &|name| std::env::var_os(name))
}

/// The `x264` program: `COREAV_X264` when it names a file, else `x264` on
/// `PATH`.
pub fn find_x264() -> Option<PathBuf> {
    find_program("COREAV_X264", "x264", &|name| std::env::var_os(name))
}

type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

fn find_encoder_in(
    choice: EncoderChoice,
    x264: Option<PathBuf>,
    ffmpeg: Option<PathBuf>,
    env: Env<'_>,
) -> Option<EncoderProgram> {
    let pick = |named: &Option<PathBuf>, variable: &str, name: &str| match named {
        Some(path) if is_path(path) => Some(path.clone()),
        Some(bare) => on_path(&bare.to_string_lossy(), env),
        None => find_program(variable, name, env),
    };
    let x264 = || pick(&x264, "COREAV_X264", "x264").map(EncoderProgram::X264);
    let ffmpeg = || pick(&ffmpeg, "COREAV_FFMPEG", "ffmpeg").map(EncoderProgram::Ffmpeg);
    match choice {
        EncoderChoice::Auto => x264().or_else(ffmpeg),
        EncoderChoice::X264 => x264(),
        EncoderChoice::Ffmpeg => ffmpeg(),
    }
}

/// The file `variable` names, else `name` in a directory of `PATH`.
fn find_program(variable: &str, name: &str, env: Env<'_>) -> Option<PathBuf> {
    if let Some(named) = env(variable) {
        let path = PathBuf::from(named);
        return path.is_file().then_some(path);
    }
    on_path(name, env)
}

/// `name` in a directory of `PATH`.
fn on_path(name: &str, env: Env<'_>) -> Option<PathBuf> {
    let path = env("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// A frame rate, `num / den` frames a second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRate {
    /// Numerator.
    pub num: u32,
    /// Denominator.
    pub den: u32,
}

/// The encoder's rate control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RateControl {
    /// Constant rate factor, 0 to 51 (`--crf`, `-crf`).
    Crf(f32),
    /// Average bit rate in kbit/s, one pass (`--bitrate`, `-b:v`), 1 to
    /// [`MAX_BITRATE`].
    Bitrate(u32),
}

/// The highest bit rate asked for, in kbit/s (2 Gbit/s, far above any
/// H.264 level's maximum).
pub const MAX_BITRATE: u32 = 2_000_000;

/// x264's presets, fastest first.
pub const PRESETS: [&str; 10] = [
    "ultrafast",
    "superfast",
    "veryfast",
    "faster",
    "fast",
    "medium",
    "slow",
    "slower",
    "veryslow",
    "placebo",
];

/// The largest width and height (`docs/design/transcode.md`, "Limits").
pub const MAX_SIZE: u32 = 16_384;

/// The most encoder threads asked for (x264's own maximum).
pub const MAX_THREADS: u32 = 128;

/// How the encoder learns when each frame is presented
/// (`docs/design/transcode.md`, "Rate control and timestamps").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Timing {
    /// A constant rate, [`EncoderSettings::frame_rate`]: raw frames
    /// (`-f rawvideo`, x264's `--fps`).
    Constant,
    /// Each frame's own time, in ticks of `num / den` seconds: x264 reads
    /// them from a timecode file (format v2, milliseconds) the caller has
    /// written, `--tcfile-in`; FFmpeg from the frames' Matroska blocks
    /// ([`crate::rawmkv`]), which [`Encoder::send_frame_at`] gives the
    /// frames' times.
    Timestamps {
        /// Seconds per tick, numerator.
        num: u64,
        /// Seconds per tick, denominator.
        den: u64,
        /// The x264 timecode file ([`write_timecodes`]); unused by FFmpeg.
        timecodes: Option<PathBuf>,
    },
}

/// What the encoder is told about the frames and how to encode them.
#[derive(Clone, Debug, PartialEq)]
pub struct EncoderSettings {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The frames' format: [`PixelFormat::Yuv`] at 4:2:0, 4:2:2 or 4:4:4,
    /// 8 or 10 bits.
    pub format: PixelFormat,
    /// The constant rate the encoder is told with [`Timing::Constant`];
    /// with [`Timing::Timestamps`], the rate FFmpeg is told when it gets
    /// one frame, whose time gives it none ([`crate::rawmkv`]).
    pub frame_rate: FrameRate,
    /// Constant rate or each frame's own time.
    pub timing: Timing,
    /// The colour description for the stream's VUI: H.273 code points and
    /// the range. Unknown fields are not signalled; the chroma location is
    /// not passed.
    pub color: ColorInfo,
    /// The sample aspect ratio, when it is signalled.
    pub sample_aspect: Option<(u32, u32)>,
    /// One of [`PRESETS`].
    pub preset: String,
    /// The rate control.
    pub rate: RateControl,
    /// Encoder threads; `None` for the encoder's own choice.
    pub threads: Option<u32>,
    /// x264's minimum GOP length (`--min-keyint`, `-keyint_min`), 1 to
    /// [`MAX_MIN_KEYINT`]; `None` for x264's own, which follows the rate
    /// it is told ([`min_keyint`]).
    pub min_keyint: Option<u32>,
}

/// The longest minimum GOP length x264 chooses itself: a tenth of its
/// maximum (`--keyint`, 250, which CoreAV does not change).
pub const MAX_MIN_KEYINT: u32 = 25;

/// The minimum GOP length x264 chooses for frames at `rate`: a second of
/// frames, rounded down, from 1 to [`MAX_MIN_KEYINT`] (checked as a black
/// box: 1 at 1.5 frames a second, 9 at 9.99, 23 at 24000/1001, 25 from 25
/// up). Given explicitly, it keeps the choice FFmpeg's libx264 makes at the
/// source's rate where x264 is told no rate: with `--tcfile-in` x264
/// takes 25 frames a second whatever the times, and refuses `--fps`.
pub fn min_keyint(rate: FrameRate) -> u32 {
    (rate.num / rate.den.max(1)).clamp(1, MAX_MIN_KEYINT)
}

impl EncoderSettings {
    /// Settings for frames of `width` × `height` in `format` at
    /// `frame_rate`: preset `medium`, CRF 23 (x264's and FFmpeg's
    /// defaults), no colour description, no sample aspect ratio, the
    /// encoder's threads.
    pub fn new(width: u32, height: u32, format: PixelFormat, frame_rate: FrameRate) -> Self {
        Self {
            width,
            height,
            format,
            frame_rate,
            timing: Timing::Constant,
            color: ColorInfo::default(),
            sample_aspect: None,
            preset: "medium".into(),
            rate: RateControl::Crf(23.0),
            threads: None,
            min_keyint: None,
        }
    }

    /// Checks every field: a format of the list above (`Unsupported`
    /// otherwise), a size within [`MAX_SIZE`] that the subsampling divides,
    /// a positive frame rate, a known preset, a CRF of 0 to 51 or a
    /// positive bit rate, threads up to [`MAX_THREADS`], and a sample
    /// aspect ratio whose reduced terms fit H.264's 16 bits.
    pub fn validate(&self) -> Result<(), Error> {
        let (chroma, _) = input_format(self.format)?;
        let invalid = |what: &str| Err(Error::new(ErrorKind::InvalidData, what));
        if self.width == 0 || self.height == 0 || self.width > MAX_SIZE || self.height > MAX_SIZE {
            return Err(Error::new(
                ErrorKind::Limit,
                format!(
                    "output size {}x{} outside 1 to {MAX_SIZE}",
                    self.width, self.height
                ),
            ));
        }
        let (sx, sy) = chroma.shifts();
        if (sx == 1 && self.width % 2 != 0) || (sy == 1 && self.height % 2 != 0) {
            return Err(Error::new(
                ErrorKind::Unsupported,
                format!(
                    "{}x{} is not divisible by the chroma subsampling",
                    self.width, self.height
                ),
            ));
        }
        match &self.timing {
            Timing::Constant if self.frame_rate.num == 0 || self.frame_rate.den == 0 => {
                return invalid("frame rate of 0");
            }
            Timing::Timestamps { num, den, .. } if *num == 0 || *den == 0 => {
                return invalid("time base of 0");
            }
            _ => {}
        }
        if !PRESETS.contains(&self.preset.as_str()) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("unknown preset {:?}", self.preset),
            ));
        }
        match self.rate {
            RateControl::Crf(q) if !(0.0..=51.0).contains(&q) => {
                return invalid("CRF outside 0 to 51");
            }
            RateControl::Bitrate(k) if k == 0 || k > MAX_BITRATE => {
                return invalid("bit rate outside 1 to 2,000,000 kbit/s");
            }
            _ => {}
        }
        if self.threads.is_some_and(|t| t == 0 || t > MAX_THREADS) {
            return invalid("encoder threads outside 1 to 128");
        }
        if self
            .min_keyint
            .is_some_and(|k| k == 0 || k > MAX_MIN_KEYINT)
        {
            return invalid("minimum GOP length outside 1 to 25");
        }
        if let Some(sar) = self.sample_aspect {
            sample_aspect(sar)?;
        }
        Ok(())
    }

    /// Bytes of one frame with its rows packed, as the encoder reads it.
    pub fn frame_bytes(&self) -> Result<usize, Error> {
        let too_large = || Error::new(ErrorKind::Limit, "frame size overflows");
        (0..self.format.plane_count()).try_fold(0usize, |sum, plane| {
            let (_, rows) = self
                .format
                .plane_size(plane, self.width, self.height)
                .ok_or_else(too_large)?;
            let row = self
                .format
                .row_bytes(plane, self.width, self.height)
                .ok_or_else(too_large)?;
            usize::try_from(rows)
                .ok()
                .and_then(|rows| rows.checked_mul(row))
                .and_then(|bytes| sum.checked_add(bytes))
                .ok_or_else(too_large)
        })
    }
}

/// The chroma and depth of a format the encoder takes.
fn input_format(format: PixelFormat) -> Result<(Chroma, u8), Error> {
    match format {
        PixelFormat::Yuv {
            chroma: chroma @ (Chroma::Cs420 | Chroma::Cs422 | Chroma::Cs444),
            depth: depth @ (8 | 10),
        } => Ok((chroma, depth)),
        other => Err(Error::new(
            ErrorKind::Unsupported,
            format!(
                "the encoder takes planar 4:2:0, 4:2:2 or 4:4:4 at 8 or 10 bits, not {other:?}"
            ),
        )),
    }
}

/// The sample aspect ratio reduced, refused when a term is 0 or does not
/// fit H.264's 16-bit `sar_width` and `sar_height`.
fn sample_aspect((num, den): (u32, u32)) -> Result<(u32, u32), Error> {
    let g = gcd(num, den);
    let (n, d) = (
        num.checked_div(g).unwrap_or(0),
        den.checked_div(g).unwrap_or(0),
    );
    if n == 0 || d == 0 || n > 65_535 || d > 65_535 {
        return Err(Error::new(
            ErrorKind::Unsupported,
            format!("sample aspect ratio {num}:{den} cannot be signalled in H.264"),
        ));
    }
    Ok((n, d))
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// One H.273 code point: its value, x264's name and FFmpeg's name.
type CodePoint = (u8, &'static str, &'static str);

/// `colour_primaries` (ITU-T H.273 table 2). Not 22 (EBU Tech. 3213):
/// x264 has no name for it, and libx264 drops it when FFmpeg passes it
/// (checked as a black box: the stream's VUI says unspecified), so neither
/// program signals it.
const PRIMARIES: [CodePoint; 10] = [
    (1, "bt709", "bt709"),
    (4, "bt470m", "bt470m"),
    (5, "bt470bg", "bt470bg"),
    (6, "smpte170m", "smpte170m"),
    (7, "smpte240m", "smpte240m"),
    (8, "film", "film"),
    (9, "bt2020", "bt2020"),
    (10, "smpte428", "smpte428"),
    (11, "smpte431", "smpte431"),
    (12, "smpte432", "smpte432"),
];

/// `transfer_characteristics` (ITU-T H.273 table 3).
const TRANSFER: [CodePoint; 16] = [
    (1, "bt709", "bt709"),
    (4, "bt470m", "gamma22"),
    (5, "bt470bg", "gamma28"),
    (6, "smpte170m", "smpte170m"),
    (7, "smpte240m", "smpte240m"),
    (8, "linear", "linear"),
    (9, "log100", "log100"),
    (10, "log316", "log316"),
    (11, "iec61966-2-4", "iec61966-2-4"),
    (12, "bt1361e", "bt1361e"),
    (13, "iec61966-2-1", "iec61966-2-1"),
    (14, "bt2020-10", "bt2020-10"),
    (15, "bt2020-12", "bt2020-12"),
    (16, "smpte2084", "smpte2084"),
    (17, "smpte428", "smpte428"),
    (18, "arib-std-b67", "arib-std-b67"),
];

/// `matrix_coefficients` (ITU-T H.273 table 4).
const MATRIX: [CodePoint; 13] = [
    (0, "GBR", "rgb"),
    (1, "bt709", "bt709"),
    (4, "fcc", "fcc"),
    (5, "bt470bg", "bt470bg"),
    (6, "smpte170m", "smpte170m"),
    (7, "smpte240m", "smpte240m"),
    (8, "YCgCo", "ycgco"),
    (9, "bt2020nc", "bt2020nc"),
    (10, "bt2020c", "bt2020c"),
    (11, "smpte2085", "smpte2085"),
    (12, "chroma-derived-nc", "chroma-derived-nc"),
    (13, "chroma-derived-c", "chroma-derived-c"),
    (14, "ICtCp", "ictcp"),
];

/// The colour options of one program: for each of the three tables, its
/// flag and the name of the code point, where both exist.
fn color_options(
    color: &ColorInfo,
    flags: [&'static str; 3],
    pick: fn(&CodePoint) -> &'static str,
) -> Vec<String> {
    let fields = [
        (color.primaries, &PRIMARIES[..]),
        (color.transfer, &TRANSFER[..]),
        (color.matrix, &MATRIX[..]),
    ];
    flags
        .iter()
        .zip(fields)
        .filter_map(|(flag, (code, table))| {
            let code = code?;
            let name = table.iter().find(|c| c.0 == code).map(pick)?;
            Some([(*flag).to_owned(), name.to_owned()])
        })
        .flatten()
        .collect()
}

/// x264's arguments for `settings` (`docs/design/transcode.md`, "The
/// encoder program"): raw frames on standard input, an Annex B stream on
/// standard output.
pub fn x264_args(settings: &EncoderSettings) -> Result<Vec<String>, Error> {
    settings.validate()?;
    let (chroma, depth) = input_format(settings.format)?;
    let csp = match chroma {
        Chroma::Cs422 => "i422",
        Chroma::Cs444 => "i444",
        Chroma::Cs420 | Chroma::Mono => "i420",
    };
    let s = |v: &str| v.to_owned();
    let mut args = vec![
        s("--demuxer"),
        s("raw"),
        s("--input-csp"),
        s(csp),
        // x264 converts to its default output, 4:2:0, otherwise.
        s("--output-csp"),
        s(csp),
        s("--input-depth"),
        depth.to_string(),
        s("--output-depth"),
        depth.to_string(),
        s("--input-res"),
        format!("{}x{}", settings.width, settings.height),
    ];
    match &settings.timing {
        Timing::Constant => args.extend([
            s("--fps"),
            format!("{}/{}", settings.frame_rate.num, settings.frame_rate.den),
        ]),
        // x264 takes its time base from the timecodes: `--timebase` set to
        // the source's changed the encode on `vfr.mp4`, and without it the
        // frames decode as those of `ffmpeg -c:v libx264` on the source.
        Timing::Timestamps {
            timecodes: Some(path),
            ..
        } => args.extend([s("--tcfile-in"), path.to_string_lossy().into_owned()]),
        Timing::Timestamps {
            timecodes: None, ..
        } => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "x264 needs a timecode file for the frames' times",
            ));
        }
    }
    args.extend(color_options(
        &settings.color,
        ["--colorprim", "--transfer", "--colormatrix"],
        |c| c.1,
    ));
    if let Some(full) = settings.color.full_range {
        let range = if full { "pc" } else { "tv" };
        // The same range in and out: x264 converts the levels otherwise.
        args.extend([s("--input-range"), s(range), s("--range"), s(range)]);
    }
    if let Some(sar) = settings.sample_aspect {
        let (n, d) = sample_aspect(sar)?;
        args.extend([s("--sar"), format!("{n}:{d}")]);
    }
    args.extend([s("--preset"), settings.preset.clone()]);
    match settings.rate {
        RateControl::Crf(q) => args.extend([s("--crf"), q.to_string()]),
        RateControl::Bitrate(k) => args.extend([s("--bitrate"), k.to_string()]),
    }
    if let Some(t) = settings.threads {
        args.extend([s("--threads"), t.to_string()]);
    }
    if let Some(k) = settings.min_keyint {
        args.extend([s("--min-keyint"), k.to_string()]);
    }
    // `--quiet` would also hide the errors of x264's filters (an
    // unsupported conversion says why only at `--log-level error`).
    args.extend(
        ["--log-level", "error", "--no-progress", "--muxer", "raw"]
            .into_iter()
            .map(s),
    );
    args.extend([s("--output"), s("-"), s("-")]);
    Ok(args)
}

/// FFmpeg's arguments for `settings` (`docs/design/transcode.md`, "The
/// encoder program"): raw video on standard input, libx264, an Annex B
/// stream on standard output.
pub fn ffmpeg_args(settings: &EncoderSettings) -> Result<Vec<String>, Error> {
    settings.validate()?;
    let (chroma, depth) = input_format(settings.format)?;
    let pix_fmt = match (chroma, depth) {
        (Chroma::Cs422, 8) => "yuv422p",
        (Chroma::Cs444, 8) => "yuv444p",
        (Chroma::Cs420, 10) => "yuv420p10le",
        (Chroma::Cs422, 10) => "yuv422p10le",
        (Chroma::Cs444, 10) => "yuv444p10le",
        _ => "yuv420p",
    };
    let s = |v: &str| v.to_owned();
    let mut args: Vec<String> = ["-nostdin", "-hide_banner", "-loglevel", "error", "-nostats"]
        .into_iter()
        .map(s)
        .collect();
    match settings.timing {
        Timing::Constant => args.extend([
            s("-f"),
            s("rawvideo"),
            s("-pix_fmt"),
            s(pix_fmt),
            s("-video_size"),
            format!("{}x{}", settings.width, settings.height),
            s("-framerate"),
            format!("{}/{}", settings.frame_rate.num, settings.frame_rate.den),
        ]),
        // The frames with their times, in Matroska: the size, format and
        // time base come from the stream (`crate::rawmkv`).
        Timing::Timestamps { .. } => args.extend([s("-f"), s("matroska")]),
    }
    // Input options: they describe the raw frames, as a decoder's frames
    // carry their description to libx264.
    args.extend(color_options(
        &settings.color,
        ["-color_primaries", "-color_trc", "-colorspace"],
        |c| c.2,
    ));
    if let Some(full) = settings.color.full_range {
        args.extend([s("-color_range"), s(if full { "pc" } else { "tv" })]);
    }
    args.extend(
        [
            "-i",
            "pipe:0",
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-c:v",
            "libx264",
            "-preset",
        ]
        .into_iter()
        .map(s),
    );
    args.push(settings.preset.clone());
    match settings.rate {
        RateControl::Crf(q) => args.extend([s("-crf"), q.to_string()]),
        RateControl::Bitrate(k) => args.extend([s("-b:v"), format!("{k}k")]),
    }
    if let Some(sar) = settings.sample_aspect {
        // Straight to libx264: `-aspect` goes through a display aspect
        // ratio as a double and comes back rounded (4095:4096 at 720x480
        // became 8:9), and the wrapper reduces a frame's ratio to terms of
        // at most 4096.
        let (n, d) = sample_aspect(sar)?;
        args.extend([s("-x264-params"), format!("sar={n}/{d}")]);
    }
    if let Some(t) = settings.threads {
        args.extend([s("-threads"), t.to_string()]);
    }
    if let Some(k) = settings.min_keyint {
        args.extend([s("-keyint_min"), k.to_string()]);
    }
    args.extend(["-f", "h264", "pipe:1"].into_iter().map(s));
    Ok(args)
}

/// Where the encoder's output goes: every byte of its standard output, in
/// order, on the reader thread. An error stops the encode: the encoder is
/// killed and [`Encoder::finish`] (or the next [`Encoder::send_frame`])
/// returns that error.
pub trait Sink: Send + 'static {
    /// The next bytes of the stream.
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error>;
}

impl<F> Sink for F
where
    F: FnMut(&[u8]) -> Result<(), Error> + Send + 'static,
{
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self(bytes)
    }
}

/// The encoder process's resource limits, set by the sandbox launcher
/// (`docs/design/transcode.md`, "Sandbox"). The CPU time is bounded by the
/// time left to the deadline times the CPU count; no core files and no
/// regular-file writes always.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderLimits {
    /// Address space in bytes (`RLIMIT_AS`).
    pub address_space: Option<u64>,
    /// Open file descriptors (`RLIMIT_NOFILE`).
    pub open_files: Option<u64>,
}

impl EncoderLimits {
    /// `max_memory_bytes` of address space (`Limits::max_memory_bytes`)
    /// and 64 open files.
    pub fn new(max_memory_bytes: u64) -> Self {
        Self {
            address_space: Some(max_memory_bytes),
            open_files: Some(64),
        }
    }
}

/// Frames queued for the writer thread, at most.
pub const QUEUED_FRAMES: usize = 2;

/// Bytes of the encoder's standard error kept for an error message.
pub const KEPT_STDERR: usize = 64 << 10;

/// Bytes read from the encoder's standard output at a time.
const READ_CHUNK: usize = 64 << 10;

/// How long a failure waits for the rest of the encoder's messages, and
/// for the keeper to reap a killed encoder.
const REPORT_PATIENCE: Duration = Duration::from_secs(1);

/// How often the keeper looks whether the encoder has exited, at most.
const EXIT_POLL: Duration = Duration::from_millis(20);

/// How long a failure waits for the keeper to see the encoder's exit before
/// it kills it (a few of the keeper's polls).
const EXIT_SETTLE: Duration = Duration::from_millis(250);

/// How long the writer and the caller wait on each other before they
/// look at the deadline again.
const QUEUE_POLL: Duration = Duration::from_millis(50);

/// How the encoder process ended, as its keeper saw it.
#[derive(Clone, Copy, Debug)]
struct Exit {
    /// Its status (`None` when it could not be read).
    status: Option<ExitStatus>,
    /// It exited by itself, before any kill; otherwise the keeper killed it
    /// (the deadline, an error, a drop).
    by_itself: bool,
}

/// State the caller and the threads share.
#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    /// Signalled when a frame is queued or taken, the queue stops, or the
    /// encoder's exit is known.
    changed: Condvar,
    /// The first failure a thread saw (the sink's error).
    failure: Mutex<Option<Error>>,
    /// The tail of the encoder's standard error.
    stderr: Mutex<Vec<u8>>,
    /// How the encoder ended, once its keeper has reaped it.
    exit: Mutex<Option<Exit>>,
    /// The encoder's standard output has ended.
    output_ended: AtomicBool,
}

#[derive(Default)]
struct Queue {
    frames: VecDeque<Chunk>,
    /// No more frames will come: the writer closes the encoder's input
    /// once the queue is empty.
    input_done: bool,
    /// The writer is gone (a failed write) or must stop (an error, a
    /// drop, the encoder's exit): frames are no longer taken.
    stopped: bool,
    /// Frames written whole to the encoder's input.
    written: u64,
}

impl Shared {
    fn record(&self, error: Error) {
        if let Ok(mut failure) = self.failure.lock() {
            failure.get_or_insert(error);
        }
    }

    fn failure(&self) -> Option<Error> {
        self.failure.lock().ok().and_then(|f| f.clone())
    }

    fn stop(&self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.stopped = true;
        }
        self.changed.notify_all();
    }

    /// Records how the encoder ended and stops the session, which wakes a
    /// caller waiting for room in the queue.
    fn ended(&self, exit: Exit) {
        let queued = {
            let mut queue = self.queue.lock().ok();
            if let Ok(mut slot) = self.exit.lock() {
                *slot = Some(exit);
            }
            match queue.as_mut() {
                Some(queue) => {
                    queue.stopped = true;
                    std::mem::take(&mut queue.frames)
                }
                None => VecDeque::new(),
            }
        };
        self.changed.notify_all();
        drop(queued);
    }

    /// How the encoder ended, waiting for it at most `patience`.
    fn wait_exit(&self, patience: Duration) -> Option<Exit> {
        let until = Instant::now() + patience;
        let mut queue = self.queue.lock().ok()?;
        loop {
            if let Some(exit) = self.exit.lock().ok().and_then(|e| *e) {
                return Some(exit);
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            queue = self
                .changed
                .wait_timeout(queue, left.min(EXIT_POLL))
                .ok()?
                .0;
        }
    }

    fn stderr_text(&self) -> String {
        self.stderr
            .lock()
            .map(|kept| {
                let start = kept.len().saturating_sub(KEPT_STDERR);
                String::from_utf8_lossy(kept.get(start..).unwrap_or_default())
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default()
    }
}

/// A running encoder process, with the threads the module describes: the
/// video encoder program's ([`Encoder`]) or the Opus encoder's
/// (`crate::audio`). Its input is a sequence of byte chunks; its output
/// goes to a [`Sink`].
pub(crate) struct Process<S: Sink> {
    shared: Arc<Shared>,
    /// Tells the keeper to kill the encoder (a message, or every sender
    /// gone); `None` once it was told.
    keeper: Option<Sender<()>>,
    writer_done: Receiver<()>,
    reader_done: Receiver<(S, Result<(), Error>)>,
    stderr_done: Receiver<()>,
    name: &'static str,
    /// What a chunk of the input is, for messages (`frames`).
    unit: &'static str,
    deadline: Instant,
    /// Chunks queued so far.
    queued: u64,
    /// The error the encode already ended with.
    error: Option<Error>,
}

/// A running encoder program.
pub struct Encoder<S: Sink> {
    process: Process<S>,
    settings: EncoderSettings,
    frame_bytes: usize,
    frames: u64,
    /// FFmpeg with the frames' times: the Matroska stream's time scale, and
    /// the first frame's time and the last one's.
    blocks: Option<Blocks>,
}

/// The frames' times in the Matroska stream to FFmpeg.
#[derive(Debug)]
struct Blocks {
    timescale: crate::rawmkv::Timescale,
    /// The stream's header for more than one frame, and for one frame
    /// (naming the frame rate, which FFmpeg cannot find from one frame's
    /// time: [`crate::rawmkv`]), until one goes out before the first frame.
    headers: Option<(Vec<u8>, Vec<u8>)>,
    /// The first frame, held until a second one or the end of the input
    /// says which header goes before it.
    held: Option<Chunk>,
    first: Option<i64>,
    last: Option<i64>,
}

/// Writes `times` (frame times in ticks of `num / den` seconds, in the
/// order the frames are sent) to `out` as an x264 timecode file of format
/// v2: one time a line in milliseconds from the first, with six decimals
/// (rounded to the nanosecond), so that x264's time base is exact for any
/// source time base. Fails when the times do not increase or a term is 0.
/// The caller writes through the handle of the file it created, never by
/// path again.
pub fn write_timecodes(
    out: &mut impl Write,
    times: &[i64],
    num: u64,
    den: u64,
) -> Result<(), Error> {
    if num == 0 || den == 0 {
        return Err(Error::new(ErrorKind::InvalidData, "time base of 0"));
    }
    let first = times.first().copied().unwrap_or(0);
    let mut text = String::from("# timecode format v2\n");
    let mut previous: Option<i64> = None;
    for &t in times {
        if previous.is_some_and(|p| p >= t) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "frame times must increase",
            ));
        }
        previous = Some(t);
        // Nanoseconds from the first frame, rounded.
        let ns = u128::try_from(i128::from(t) - i128::from(first))
            .ok()
            .and_then(|d| d.checked_mul(u128::from(num))?.checked_mul(1_000_000_000))
            .map(|n| (n + u128::from(den) / 2) / u128::from(den))
            .ok_or_else(|| Error::new(ErrorKind::Limit, "a frame time is out of range"))?;
        text.push_str(&format!("{}.{:06}\n", ns / 1_000_000, ns % 1_000_000));
    }
    out.write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|e| Error::new(ErrorKind::Io, format!("timecode file: {}", e.kind())))
}

/// The command that runs `program` with `args`: through `launcher` with
/// `limits`, `huge_pages` and a CPU time of what is left to `deadline`
/// times the CPU count (the sandbox), or directly when `launcher` is
/// `None` (no limits; for tests).
pub(crate) fn command(
    program: &Path,
    args: &[OsString],
    huge_pages: HugePages,
    launcher: Option<&ExecLauncher>,
    limits: &EncoderLimits,
    deadline: Instant,
) -> std::process::Command {
    match launcher {
        Some(launcher) => {
            let parallelism = std::thread::available_parallelism().map_or(1, |n| n.get());
            let cpu_seconds = deadline
                .saturating_duration_since(Instant::now())
                .as_secs()
                .saturating_add(1)
                .saturating_mul(u64::try_from(parallelism).unwrap_or(1))
                .saturating_add(10);
            let sandbox = SandboxLimits {
                address_space: limits.address_space,
                cpu_seconds: Some(cpu_seconds),
                open_files: limits.open_files,
            };
            launcher.command(&sandbox, huge_pages, program, args)
        }
        None => exec::direct_command(program, args),
    }
}

/// Starts a thread, or reports why it could not.
fn spawn_thread(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), Error> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .map(|_| ())
        .map_err(|e| Error::new(ErrorKind::Backend, format!("cannot start a thread: {e}")))
}

impl<S: Sink> Encoder<S> {
    /// Starts `program` for `settings`, its output going to `sink`:
    /// through `launcher` with `limits` (the sandbox), or directly when
    /// `launcher` is `None` (no limits; for tests). Every wait ends at
    /// `deadline`, when the encoder is killed. The program must be a path
    /// ([`find_encoder`] returns one).
    pub fn spawn(
        program: &EncoderProgram,
        settings: &EncoderSettings,
        launcher: Option<&ExecLauncher>,
        limits: &EncoderLimits,
        deadline: Instant,
        sink: S,
    ) -> Result<Self, Error> {
        let name = program.name();
        if !is_path(program.path()) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "the {name} program must be a path, not a name: {}",
                    program.path().display()
                ),
            ));
        }
        let args: Vec<OsString> = program
            .args(settings)?
            .into_iter()
            .map(OsString::from)
            .collect();
        let frame_bytes = settings.frame_bytes()?;
        // Checked before the encoder starts: an error after it would leave
        // the process to no keeper.
        let blocks = match (program, &settings.timing) {
            (EncoderProgram::Ffmpeg(_), Timing::Timestamps { num, den, .. }) => {
                let timescale = crate::rawmkv::Timescale::new(*num, *den)
                    .ok_or_else(|| Error::new(ErrorKind::InvalidData, "time base of 0"))?;
                let fourcc = crate::rawmkv::fourcc(settings.format).ok_or_else(|| {
                    Error::new(ErrorKind::Unsupported, "no Matroska layout for the format")
                })?;
                if frame_bytes > crate::rawmkv::MAX_FRAME_BYTES {
                    return Err(Error::new(
                        ErrorKind::Unsupported,
                        format!(
                            "a {}x{} frame is more than FFmpeg reads in one Matroska block",
                            settings.width, settings.height
                        ),
                    ));
                }
                let header = |frame_ns| {
                    crate::rawmkv::header(
                        settings.width,
                        settings.height,
                        fourcc,
                        &timescale,
                        frame_ns,
                    )
                };
                let frame_ns = crate::rawmkv::frame_duration_ns(
                    settings.frame_rate.num,
                    settings.frame_rate.den,
                );
                let headers = (header(None), header(frame_ns));
                Some(Blocks {
                    timescale,
                    headers: Some(headers),
                    held: None,
                    first: None,
                    last: None,
                })
            }
            _ => None,
        };
        let command = command(
            program.path(),
            &args,
            program.huge_pages(settings.format),
            launcher,
            limits,
            deadline,
        );
        let process = Process::spawn(name, "frames", command, program.path(), deadline, sink)?;
        Ok(Self {
            process,
            settings: settings.clone(),
            frame_bytes,
            frames: 0,
            blocks,
        })
    }

    /// Frames sent so far.
    pub fn frames_sent(&self) -> u64 {
        self.frames
    }

    /// Queues a frame for the encoder, in CPU or shared memory: its format
    /// and size must be the settings'. The writer writes its planes from
    /// where they are and drops them after. Waits while [`QUEUED_FRAMES`]
    /// are queued, at most until the deadline; an encoder that exits
    /// meanwhile ends the wait at once. With FFmpeg and
    /// [`Timing::Timestamps`], use [`Encoder::send_frame_at`].
    pub fn send_frame(&mut self, frame: impl Into<Pixels>) -> Result<(), Error> {
        self.send(frame.into(), None)
    }

    /// [`Encoder::send_frame`] for a frame presented at `time`, in ticks of
    /// the [`Timing::Timestamps`] time base; times must increase. With
    /// x264 and with [`Timing::Constant`] the time is not passed (x264 has
    /// the timecode file).
    pub fn send_frame_at(&mut self, frame: impl Into<Pixels>, time: i64) -> Result<(), Error> {
        self.send(frame.into(), Some(time))
    }

    fn send(&mut self, pixels: Pixels, time: Option<i64>) -> Result<(), Error> {
        if let Some(e) = &self.process.error {
            return Err(e.clone());
        }
        let (width, height, format, packed_len) = match &pixels {
            Pixels::Cpu(p) => (p.width(), p.height(), p.format(), Some(p.packed_len())),
            Pixels::Shared(p) => (p.width, p.height, p.format, p.packed_len()),
            Pixels::Cuda(_) => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "a frame in device memory, which the encoder does not read",
                ));
            }
        };
        if format != self.settings.format
            || width != self.settings.width
            || height != self.settings.height
        {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "frame {width}x{height} {format:?} differs from the encoder's {}x{} {:?}",
                    self.settings.width, self.settings.height, self.settings.format
                ),
            ));
        }
        if packed_len != Some(self.frame_bytes) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "frame of the wrong size",
            ));
        }
        let head = match self.blocks.as_mut() {
            Some(blocks) => {
                let Some(time) = time else {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "a frame without its time for an encoder told the frames' times",
                    ));
                };
                if blocks.last.is_some_and(|last| time <= last) {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "frame times must increase",
                    ));
                }
                let first = *blocks.first.get_or_insert(time);
                let ticks = i64::try_from(i128::from(time) - i128::from(first))
                    .ok()
                    .and_then(|t| blocks.timescale.ticks(t))
                    .ok_or_else(|| Error::new(ErrorKind::Limit, "a frame time is out of range"))?;
                blocks.last = Some(time);
                crate::rawmkv::cluster_head(ticks, self.frame_bytes as u64)
            }
            None => Vec::new(),
        };
        let chunk = Chunk::Frame { head, pixels };
        if let Some(blocks) = self.blocks.as_mut() {
            match blocks.held.take() {
                None if blocks.headers.is_some() => {
                    blocks.held = Some(chunk);
                    self.frames += 1;
                    return Ok(());
                }
                Some(held) => {
                    let (many, _) = blocks.headers.take().unwrap_or_default();
                    self.process.enqueue(held.after(many))?;
                }
                None => {}
            }
        }
        self.process.enqueue(chunk)?;
        self.frames += 1;
        Ok(())
    }

    /// Closes the encoder's input once every queued frame is written, takes
    /// the rest of its output, and waits for it to exit, at most until the
    /// deadline. Returns the sink; an encoder that exited with an error, was
    /// killed or did not read every frame is an error carrying the end of
    /// its standard error.
    pub fn finish(mut self) -> Result<S, Error> {
        if let Some(e) = self.process.error.take() {
            return Err(e);
        }
        let held = self
            .blocks
            .as_mut()
            .and_then(|b| Some((b.headers.take()?.1, b.held.take()?)));
        if let Some((one, held)) = held {
            self.process.enqueue(held.after(one))?;
        }
        self.process.finish()
    }
}

impl<S: Sink> Process<S> {
    /// Starts `command` (running `program`), its output going to `sink`;
    /// every wait ends at `deadline`, when the process is killed.
    pub(crate) fn spawn(
        name: &'static str,
        unit: &'static str,
        mut command: std::process::Command,
        program: &Path,
        deadline: Instant,
        sink: S,
    ) -> Result<Self, Error> {
        let mut child = command.spawn().map_err(|e| {
            Error::new(
                ErrorKind::Backend,
                format!("cannot start {name} {}: {e}", program.display()),
            )
        })?;
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let shared = Arc::new(Shared::default());
        // From here on the keeper owns the process: every path that gives up
        // tells it to kill.
        let (keeper, orders) = mpsc::channel();
        let slot = Arc::new(Mutex::new(Some(child)));
        {
            let kept = Arc::clone(&slot);
            let shared = Arc::clone(&shared);
            let started = spawn_thread("coreav-encoder-keeper", move || {
                if let Some(child) = kept.lock().ok().and_then(|mut c| c.take()) {
                    keep(child, deadline, &orders, &shared);
                }
            });
            if let Err(e) = started {
                if let Some(mut child) = slot.lock().ok().and_then(|mut c| c.take()) {
                    exec::kill_group(&child);
                    let _ = child.kill();
                    let _ = child.wait();
                }
                return Err(e);
            }
        }
        let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
            let _ = keeper.send(());
            return Err(Error::new(ErrorKind::Backend, "encoder pipes missing"));
        };
        let (writer_tx, writer_done) = mpsc::channel();
        let (reader_tx, reader_done) = mpsc::channel();
        let (stderr_tx, stderr_done) = mpsc::channel();
        let started = {
            let shared = Arc::clone(&shared);
            spawn_thread("coreav-encoder-stderr", move || {
                keep_stderr(stderr, &shared);
                let _ = stderr_tx.send(());
            })
        }
        .and_then(|()| {
            let shared = Arc::clone(&shared);
            spawn_thread("coreav-encoder-writer", move || {
                write_frames(stdin, &shared);
                let _ = writer_tx.send(());
            })
        })
        .and_then(|()| {
            let shared = Arc::clone(&shared);
            let keeper = keeper.clone();
            spawn_thread("coreav-encoder-reader", move || {
                let outcome = read_output(stdout, sink, &shared, &keeper);
                let _ = reader_tx.send(outcome);
            })
        });
        if let Err(e) = started {
            shared.stop();
            let _ = keeper.send(());
            return Err(e);
        }
        Ok(Self {
            shared,
            keeper: Some(keeper),
            writer_done,
            reader_done,
            stderr_done,
            name,
            unit,
            deadline,
            queued: 0,
            error: None,
        })
    }

    /// Queues one chunk of the input for the writer, waiting while
    /// [`QUEUED_FRAMES`] are queued, at most until the deadline.
    pub(crate) fn enqueue(&mut self, chunk: impl Into<Chunk>) -> Result<(), Error> {
        let chunk = chunk.into();
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        let outcome = {
            let Ok(mut queue) = self.shared.queue.lock() else {
                return Err(self.fail(Error::new(ErrorKind::Backend, "encoder state lost")));
            };
            loop {
                if queue.stopped {
                    break Err(stopped_reading(self.name));
                }
                if queue.frames.len() < QUEUED_FRAMES {
                    queue.frames.push_back(chunk);
                    break Ok(());
                }
                let remaining = self.deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break Err(timeout());
                }
                match self
                    .shared
                    .changed
                    .wait_timeout(queue, remaining.min(QUEUE_POLL))
                {
                    Ok((guard, _)) => queue = guard,
                    Err(_) => break Err(Error::new(ErrorKind::Backend, "encoder state lost")),
                }
            }
        };
        match outcome {
            Ok(()) => {
                self.queued += 1;
                self.shared.changed.notify_all();
                Ok(())
            }
            Err(e) => Err(self.fail(e)),
        }
    }

    /// Closes the input once every queued chunk is written, takes the rest
    /// of the output, and waits for the process to exit, at most until the
    /// deadline. Returns the sink; a process that exited with an error, was
    /// killed or did not read every chunk is an error carrying the end of
    /// its standard error.
    pub(crate) fn finish(mut self) -> Result<S, Error> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        if let Ok(mut queue) = self.shared.queue.lock() {
            queue.input_done = true;
        }
        self.shared.changed.notify_all();
        // The keeper reaps the encoder when it exits, after it has killed
        // what is left of its group, which ends any process that inherited
        // its pipes (and so the writer's and the reader's waits); at the
        // deadline it kills it.
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        let exit = self
            .shared
            .wait_exit(remaining.saturating_add(REPORT_PATIENCE));
        let status = match exit {
            Some(Exit {
                status: Some(status),
                by_itself: true,
            }) => status,
            Some(Exit {
                status: None,
                by_itself: true,
            }) => {
                return Err(self.fail(Error::new(
                    ErrorKind::Backend,
                    format!("cannot read how {} ended", self.name),
                )));
            }
            // Killed: at the deadline, or after the sink's error (which
            // `fail` reports first).
            Some(_) => return Err(self.fail(stopped_reading(self.name))),
            None => return Err(self.fail(timeout())),
        };
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        let _ = self.writer_done.recv_timeout(remaining);
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        let outcome = self.reader_done.recv_timeout(remaining);
        let _ = self
            .stderr_done
            .recv_timeout(REPORT_PATIENCE.min(remaining));
        if let Some(e) = self.shared.failure() {
            return Err(e);
        }
        let (sink, read) = match outcome {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => return Err(self.fail(timeout())),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(self.fail(Error::new(ErrorKind::Backend, "encoder output lost")));
            }
        };
        if !status.success() {
            return Err(self.describe(exit_error(self.name, status)));
        }
        read.map_err(|e| self.describe(e))?;
        let written = self.shared.queue.lock().map_or(0, |q| q.written);
        if written != self.queued {
            return Err(self.describe(Error::new(
                ErrorKind::Backend,
                format!(
                    "{} exited after reading {written} of {} {}",
                    self.name, self.queued, self.unit
                ),
            )));
        }
        Ok(sink)
    }

    /// Ends the encode with `error`: stops the writer, kills the encoder,
    /// and adds how it ended and the end of its messages. A failure a
    /// thread saw first (the sink's error) wins, then the encoder's own
    /// failing exit.
    fn fail(&mut self, error: Error) -> Error {
        self.shared.stop();
        // A failed write usually means the encoder is exiting: its pipes
        // close before the system reports the exit. Give the keeper a few
        // polls to see it, so the error carries the encoder's status rather
        // than the write's.
        if error.kind == ErrorKind::Backend && self.shared.failure().is_none() {
            let _ = self.shared.wait_exit(EXIT_SETTLE);
        }
        if let Some(keeper) = self.keeper.take() {
            let _ = keeper.send(());
        }
        let exit = self.shared.wait_exit(REPORT_PATIENCE);
        let error = match self.shared.failure() {
            // The sink's own error says what went wrong.
            Some(first) => first,
            None => {
                let error = match exit {
                    Some(Exit {
                        status: Some(status),
                        by_itself: true,
                    }) if !status.success() => exit_error(self.name, status),
                    // The keeper's kill at the deadline can wake a waiting
                    // caller before the caller sees the deadline itself.
                    _ if Instant::now() >= self.deadline => timeout(),
                    _ => error,
                };
                let _ = self.stderr_done.recv_timeout(REPORT_PATIENCE);
                self.describe(error)
            }
        };
        self.error = Some(error.clone());
        error
    }

    /// `error` with the end of the encoder's standard error.
    fn describe(&self, error: Error) -> Error {
        let stderr = self.shared.stderr_text();
        if stderr.is_empty() || error.message.contains(&stderr) {
            return error;
        }
        Error::new(error.kind, format!("{}: {stderr}", error.message))
    }
}

impl<S: Sink> Drop for Process<S> {
    /// Stops the writer and tells the keeper to kill the encoder; never
    /// blocks.
    fn drop(&mut self) {
        self.shared.stop();
        if let Some(keeper) = self.keeper.take() {
            let _ = keeper.send(());
        }
    }
}

/// Whether `path` names a file by its path (it has a separator: a `/`, or
/// on Windows a `/` or `\`), not a name to search for.
pub(crate) fn is_path(path: &Path) -> bool {
    has_separator(path.as_os_str().as_encoded_bytes(), cfg!(windows))
}

/// Whether `path` has a separator, `\` being one when `windows`.
fn has_separator(path: &[u8], windows: bool) -> bool {
    path.iter().any(|&b| b == b'/' || (windows && b == b'\\'))
}

fn stopped_reading(name: &str) -> Error {
    Error::new(
        ErrorKind::Backend,
        format!("{name} stopped reading its input"),
    )
}

fn timeout() -> Error {
    Error::new(ErrorKind::Timeout, "encoding took too long")
}

/// The error for an encoder that exited with `status`. (The CPU limit is
/// set as both the soft and the hard limit, so reaching it is a `SIGKILL`,
/// not told apart from other kills.)
fn exit_error(name: &str, status: ExitStatus) -> Error {
    Error::new(ErrorKind::Backend, format!("{name} failed ({status})"))
}

/// Owns the encoder process (`docs/design/transcode.md`, "Sandbox"). It
/// looks every [`EXIT_POLL`] whether the encoder has exited; when it has,
/// or at the deadline, or when told, it kills the encoder's process group
/// and only then reaps it, so the group's id cannot have been reused, and
/// records how it ended ([`Shared::ended`]), which stops the session.
fn keep(mut child: Child, deadline: Instant, orders: &Receiver<()>, shared: &Shared) {
    let mut output_ended_at = None;
    let mut stopped_at = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match orders.recv_timeout(remaining.min(EXIT_POLL)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if has_exited(&mut child, shared, &mut output_ended_at) {
            // What is left of the group (a process that inherited the
            // pipes) ends with it.
            exec::kill_group(&child);
            let status = child.wait().ok();
            shared.ended(Exit {
                status,
                by_itself: true,
            });
            return;
        }
        if shared.queue.lock().is_ok_and(|queue| queue.stopped)
            && stopped_at.get_or_insert_with(Instant::now).elapsed() >= EXIT_SETTLE
        {
            break;
        }
    }
    let exited = exec::has_exited(&mut child);
    exec::kill_group(&child);
    let _ = child.kill();
    let status = child.wait().ok();
    // An encoder that was already exiting when it was killed (its pipes
    // closed, its exit not yet reported) keeps its own status: only the kill
    // ends a process with `SIGKILL`.
    let by_itself = exited || status.is_some_and(|s| !killed(s));
    shared.ended(Exit { status, by_itself });
}

/// Whether `status` is that of a process ended by `SIGKILL`, the keeper's
/// kill (or another that cannot be told apart from it).
#[cfg(unix)]
fn killed(status: ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal() == Some(9)
}

/// Without signals, every status after a kill counts as the kill's.
#[cfg(not(unix))]
fn killed(_: ExitStatus) -> bool {
    true
}

/// Whether the encoder has exited, left unreaped (Linux's `waitid` with
/// `WNOWAIT`).
#[cfg(target_os = "linux")]
fn has_exited(child: &mut Child, _: &Shared, _: &mut Option<Instant>) -> bool {
    exec::has_exited(child)
}

/// How long an encoder may run on after its output ended, where its exit
/// cannot be seen without reaping it.
#[cfg(not(target_os = "linux"))]
const EXIT_GRACE: Duration = Duration::from_millis(500);

/// Elsewhere an exit cannot be seen without reaping the process, which
/// would let its group's id be reused before the group is killed. So an
/// encoder counts as exited [`EXIT_GRACE`] after its output ended: it is
/// then killed with its group and reaped, and one that had exited keeps its
/// own status.
#[cfg(not(target_os = "linux"))]
fn has_exited(_: &mut Child, shared: &Shared, since: &mut Option<Instant>) -> bool {
    if !shared.output_ended.load(Ordering::SeqCst) {
        return false;
    }
    since.get_or_insert_with(Instant::now).elapsed() >= EXIT_GRACE
}

/// One write to the encoder's input: bytes, or a frame's planes packed
/// after a header.
#[derive(Debug)]
pub(crate) enum Chunk {
    Bytes(Vec<u8>),
    Frame { head: Vec<u8>, pixels: Pixels },
}

impl From<Vec<u8>> for Chunk {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}

impl Chunk {
    /// The chunk with `header` written before it.
    fn after(self, header: Vec<u8>) -> Self {
        match self {
            Self::Bytes(bytes) => Self::Bytes([header, bytes].concat()),
            Self::Frame { head, pixels } => Self::Frame {
                head: [header, head].concat(),
                pixels,
            },
        }
    }

    /// Writes the chunk, a frame's planes in order with their rows packed:
    /// straight from a picture whose rows are, else gathered through
    /// `buffer` (a picture in shared memory is read only by copying out).
    fn write_to(&self, out: &mut impl Write, buffer: &mut Vec<u8>) -> io::Result<()> {
        let (head, pixels) = match self {
            Self::Bytes(bytes) => return out.write_all(bytes),
            Self::Frame { head, pixels } => (head, pixels),
        };
        out.write_all(head)?;
        if buffer.is_empty() {
            buffer.resize(WRITE_BUFFER, 0);
        }
        match pixels {
            Pixels::Cpu(picture) => write_picture(out, picture, buffer),
            Pixels::Shared(picture) => {
                let mut at = 0;
                loop {
                    let n = picture
                        .memory
                        .read_packed(at, buffer)
                        .ok_or_else(|| io::Error::other("a frame outside its shared memory"))?;
                    if n == 0 {
                        return Ok(());
                    }
                    out.write_all(buffer.get(..n).unwrap_or_default())?;
                    at += n;
                }
            }
            Pixels::Cuda(_) => Err(io::Error::other("a frame in device memory")),
        }
    }
}

/// Bytes of the writer's buffer for rows it gathers: a few times the pipe's
/// own buffer, and small enough to stay in a core's cache.
const WRITE_BUFFER: usize = 1 << 20;

/// Writes `picture`'s planes with their rows packed: each plane whose rows
/// are packed in one write, the rows of the others gathered in `buffer`.
fn write_picture(out: &mut impl Write, picture: &Picture, buffer: &mut [u8]) -> io::Result<()> {
    for (index, plane) in picture.planes().iter().enumerate() {
        let (rows, row_bytes) = picture
            .plane_rows(index)
            .ok_or_else(|| io::Error::other("a plane without rows"))?;
        if plane.stride() == row_bytes {
            let data = rows
                .checked_mul(row_bytes)
                .and_then(|len| plane.data().get(..len))
                .ok_or_else(|| io::Error::other("a plane shorter than its rows"))?;
            out.write_all(data)?;
            continue;
        }
        let mut filled = 0;
        for row in picture.rows(index) {
            if filled + row.len() > buffer.len() {
                out.write_all(buffer.get(..filled).unwrap_or_default())?;
                filled = 0;
            }
            match buffer.get_mut(filled..filled + row.len()) {
                Some(target) => {
                    target.copy_from_slice(row);
                    filled += row.len();
                }
                None => out.write_all(row)?,
            }
        }
        out.write_all(buffer.get(..filled).unwrap_or_default())?;
    }
    Ok(())
}

/// Writes the queued frames to the encoder's input until the queue is
/// done and empty, or a write fails, or the encode stops; then closes the
/// input. A frame is dropped only after its write, so a frame lent in the
/// decoder's shared memory goes back to it then.
fn write_frames(mut stdin: ChildStdin, shared: &Shared) {
    let mut buffer = Vec::new();
    loop {
        let frame = {
            let Ok(mut queue) = shared.queue.lock() else {
                return;
            };
            loop {
                if queue.stopped {
                    return;
                }
                if let Some(frame) = queue.frames.pop_front() {
                    break frame;
                }
                if queue.input_done {
                    // Dropping `stdin` closes the encoder's input.
                    return;
                }
                match shared.changed.wait_timeout(queue, QUEUE_POLL) {
                    Ok((guard, _)) => queue = guard,
                    Err(_) => return,
                }
            }
        };
        shared.changed.notify_all();
        let written = frame
            .write_to(&mut stdin, &mut buffer)
            .and_then(|()| stdin.flush());
        drop(frame);
        if written.is_err() {
            shared.stop();
            return;
        }
        if let Ok(mut queue) = shared.queue.lock() {
            queue.written += 1;
        }
    }
}

/// Hands the encoder's output to `sink` until it ends. A sink error is
/// recorded, stops the writer and kills the encoder.
fn read_output<S: Sink>(
    mut stdout: ChildStdout,
    mut sink: S,
    shared: &Shared,
    keeper: &Sender<()>,
) -> (S, Result<(), Error>) {
    let mut buf = vec![0u8; READ_CHUNK];
    let result = loop {
        match stdout.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                let Some(bytes) = buf.get(..n) else {
                    break Err(Error::new(ErrorKind::Backend, "short read"));
                };
                if let Err(e) = sink.write(bytes) {
                    shared.record(e.clone());
                    shared.stop();
                    let _ = keeper.send(());
                    break Err(e);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                break Err(Error::new(
                    ErrorKind::Io,
                    format!("reading the encoder's output: {e}"),
                ));
            }
        }
    };
    shared.output_ended.store(true, Ordering::SeqCst);
    (sink, result)
}

/// Reads the encoder's standard error to its end, keeping the last
/// [`KEPT_STDERR`] bytes (up to twice that between trims).
fn keep_stderr(mut stderr: ChildStderr, shared: &Shared) {
    let mut buf = [0u8; 4096];
    loop {
        match stderr.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                if let (Ok(mut kept), Some(chunk)) = (shared.stderr.lock(), buf.get(..n)) {
                    kept.extend_from_slice(chunk);
                    if kept.len() > 2 * KEPT_STDERR {
                        let cut = kept.len() - KEPT_STDERR;
                        kept.drain(..cut);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use coreav_frame::{Plane, SharedMemory, SharedPicture};

    use super::*;

    struct Packed(Vec<u8>);

    impl SharedMemory for Packed {
        fn read_packed(&self, at: usize, out: &mut [u8]) -> Option<usize> {
            let rest = self.0.get(at..)?;
            let n = rest.len().min(out.len());
            out.get_mut(..n)?.copy_from_slice(rest.get(..n)?);
            Some(n)
        }
    }

    #[test]
    fn stopping_an_encoder_returns_its_queued_decoder_frames() {
        let shared = Shared::default();
        let leases: Vec<_> = (0..QUEUED_FRAMES)
            .map(|_| {
                let memory = Arc::new(Packed(vec![0; 6]));
                let lease = Arc::downgrade(&memory);
                shared.queue.lock().unwrap().frames.push_back(Chunk::Frame {
                    head: Vec::new(),
                    pixels: Pixels::Shared(SharedPicture {
                        width: 2,
                        height: 2,
                        format: PixelFormat::YUV420P,
                        memory,
                    }),
                });
                lease
            })
            .collect();
        assert!(leases.iter().all(|lease| lease.upgrade().is_some()));
        shared.ended(Exit {
            status: None,
            by_itself: true,
        });
        assert!(
            leases.iter().all(|lease| lease.upgrade().is_none()),
            "a stopped encoder must return every queued decoder slot"
        );
        shared.stop();
    }

    #[test]
    fn a_frame_goes_out_packed_from_either_memory_through_any_buffer() {
        let (width, height, format) = (5, 3, PixelFormat::YUV420P);
        let planes = (0..format.plane_count())
            .map(|index| {
                let (_, rows) = format.plane_size(index, width, height).unwrap();
                let stride = format.row_bytes(index, width, height).unwrap() + 3;
                let data = (0..stride * rows as usize)
                    .map(|b| u8::try_from((b * 7 + index) % 256).unwrap())
                    .collect();
                Plane::new(data, stride)
            })
            .collect();
        let picture = Picture::new(width, height, format, planes).unwrap();
        let mut expected = b"head".to_vec();
        picture.write_packed(&mut expected).unwrap();
        let shared = SharedPicture {
            width,
            height,
            format,
            memory: Arc::new(Packed(expected.get(4..).unwrap().to_vec())),
        };
        for size in [1, 4, 7, WRITE_BUFFER] {
            for pixels in [Pixels::Cpu(picture.clone()), Pixels::Shared(shared.clone())] {
                let mut out = Vec::new();
                Chunk::Frame {
                    head: b"head".to_vec(),
                    pixels,
                }
                .write_to(&mut out, &mut vec![0; size])
                .unwrap();
                assert_eq!(out, expected, "{size}");
            }
        }
    }

    #[test]
    fn a_windows_absolute_path_names_a_file_on_windows() {
        assert!(has_separator(br"C:\ffmpeg\bin\ffmpeg.exe", true));
        assert!(has_separator(br"\\server\share\ffmpeg.exe", true));
        assert!(has_separator(b"C:/ffmpeg/bin/ffmpeg.exe", true));
        assert!(!has_separator(b"ffmpeg.exe", true));
        assert!(!has_separator(br"C:\ffmpeg\bin\ffmpeg.exe", false));
        assert!(has_separator(b"/usr/bin/ffmpeg", false));
        assert!(!has_separator(b"ffmpeg", false));
    }

    fn settings() -> EncoderSettings {
        EncoderSettings::new(
            1920,
            1080,
            PixelFormat::YUV420P,
            FrameRate {
                num: 30_000,
                den: 1001,
            },
        )
    }

    fn split(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn x264_command_line() {
        let s = settings();
        assert_eq!(
            x264_args(&s).unwrap(),
            split(
                "--demuxer raw --input-csp i420 --output-csp i420 --input-depth 8 \
                 --output-depth 8 --input-res 1920x1080 --fps 30000/1001 \
                 --preset medium --crf 23 --log-level error --no-progress --muxer raw \
                 --output - -"
            )
        );
        let s = EncoderSettings {
            format: PixelFormat::Yuv {
                chroma: Chroma::Cs422,
                depth: 10,
            },
            color: ColorInfo {
                primaries: Some(9),
                transfer: Some(16),
                matrix: Some(9),
                full_range: Some(true),
                chroma_location: None,
            },
            sample_aspect: Some((20, 22)),
            preset: "veryfast".into(),
            rate: RateControl::Bitrate(1000),
            threads: Some(4),
            min_keyint: Some(5),
            ..settings()
        };
        assert_eq!(
            x264_args(&s).unwrap(),
            split(
                "--demuxer raw --input-csp i422 --output-csp i422 --input-depth 10 \
                 --output-depth 10 --input-res 1920x1080 --fps 30000/1001 \
                 --colorprim bt2020 --transfer smpte2084 --colormatrix bt2020nc \
                 --input-range pc --range pc --sar 10:11 --preset veryfast \
                 --bitrate 1000 --threads 4 --min-keyint 5 --log-level error --no-progress \
                 --muxer raw --output - -"
            )
        );
    }

    #[test]
    fn ffmpeg_command_line() {
        let s = settings();
        assert_eq!(
            ffmpeg_args(&s).unwrap(),
            split(
                "-nostdin -hide_banner -loglevel error -nostats -f rawvideo -pix_fmt yuv420p \
                 -video_size 1920x1080 -framerate 30000/1001 -i pipe:0 -map 0:v:0 \
                 -fps_mode passthrough -c:v libx264 -preset medium -crf 23 -f h264 pipe:1"
            )
        );
        let s = EncoderSettings {
            format: PixelFormat::Yuv {
                chroma: Chroma::Cs444,
                depth: 10,
            },
            color: ColorInfo {
                primaries: Some(22),
                transfer: Some(4),
                matrix: Some(14),
                full_range: Some(false),
                chroma_location: None,
            },
            sample_aspect: Some((4095, 4096)),
            rate: RateControl::Crf(18.5),
            threads: Some(2),
            min_keyint: Some(23),
            ..settings()
        };
        assert_eq!(
            ffmpeg_args(&s).unwrap(),
            split(
                "-nostdin -hide_banner -loglevel error -nostats -f rawvideo \
                 -pix_fmt yuv444p10le -video_size 1920x1080 -framerate 30000/1001 \
                 -color_trc gamma22 -colorspace ictcp \
                 -color_range tv -i pipe:0 -map 0:v:0 -fps_mode passthrough -c:v libx264 \
                 -preset medium -crf 18.5 -x264-params sar=4095/4096 -threads 2 \
                 -keyint_min 23 -f h264 pipe:1"
            )
        );
        // Primaries 22 are signalled by neither program.
        let x = x264_args(&s).unwrap();
        assert!(!x.contains(&"--colorprim".to_owned()), "{x:?}");
        assert!(x.contains(&"ICtCp".to_owned()));
    }

    #[test]
    fn the_frames_times_replace_the_rate() {
        let s = EncoderSettings {
            timing: Timing::Timestamps {
                num: 1,
                den: 12_800,
                timecodes: Some(PathBuf::from("/tmp/x/.out.mp4.coreav-1.timecodes")),
            },
            ..settings()
        };
        assert_eq!(
            x264_args(&s).unwrap(),
            split(
                "--demuxer raw --input-csp i420 --output-csp i420 --input-depth 8 \
                 --output-depth 8 --input-res 1920x1080 \
                 --tcfile-in /tmp/x/.out.mp4.coreav-1.timecodes \
                 --preset medium --crf 23 --log-level error --no-progress --muxer raw \
                 --output - -"
            )
        );
        assert_eq!(
            ffmpeg_args(&s).unwrap(),
            split(
                "-nostdin -hide_banner -loglevel error -nostats -f matroska -i pipe:0 \
                 -map 0:v:0 -fps_mode passthrough -c:v libx264 -preset medium -crf 23 \
                 -f h264 pipe:1"
            )
        );
        // x264 needs the file; a time base of 0 is refused.
        let no_file = EncoderSettings {
            timing: Timing::Timestamps {
                num: 1,
                den: 12_800,
                timecodes: None,
            },
            ..settings()
        };
        assert!(x264_args(&no_file).is_err());
        assert!(ffmpeg_args(&no_file).is_ok());
        let zero = EncoderSettings {
            timing: Timing::Timestamps {
                num: 0,
                den: 12_800,
                timecodes: None,
            },
            ..settings()
        };
        assert!(zero.validate().is_err());
    }

    #[test]
    fn timecode_files() {
        let mut out = Vec::new();
        write_timecodes(&mut out, &[1024, 1536, 3072, 3073], 1, 12_800).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "# timecode format v2\n0.000000\n40.000000\n160.000000\n160.078125\n"
        );
        // 1/90,000 s ticks: rounded to the nanosecond.
        let mut out = Vec::new();
        write_timecodes(&mut out, &[0, 3003, 6006], 1, 90_000).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "# timecode format v2\n0.000000\n33.366667\n66.733333\n"
        );
        assert!(write_timecodes(&mut Vec::new(), &[0, 0], 1, 1000).is_err());
        assert!(write_timecodes(&mut Vec::new(), &[0, 1], 0, 1000).is_err());
    }

    #[test]
    fn unknown_code_points_are_not_signalled() {
        let s = EncoderSettings {
            color: ColorInfo {
                primaries: Some(2),
                transfer: Some(200),
                matrix: None,
                full_range: None,
                chroma_location: None,
            },
            ..settings()
        };
        assert_eq!(x264_args(&s).unwrap(), x264_args(&settings()).unwrap());
        assert_eq!(ffmpeg_args(&s).unwrap(), ffmpeg_args(&settings()).unwrap());
    }

    #[test]
    fn settings_are_checked() {
        let bad = [
            EncoderSettings {
                format: PixelFormat::SemiPlanar { depth: 8 },
                ..settings()
            },
            EncoderSettings {
                format: PixelFormat::Yuv {
                    chroma: Chroma::Mono,
                    depth: 8,
                },
                ..settings()
            },
            EncoderSettings {
                format: PixelFormat::Yuv {
                    chroma: Chroma::Cs420,
                    depth: 12,
                },
                ..settings()
            },
            EncoderSettings {
                format: PixelFormat::Gbr { depth: 8 },
                ..settings()
            },
            EncoderSettings {
                width: 1919,
                ..settings()
            },
            EncoderSettings {
                height: 1081,
                ..settings()
            },
            EncoderSettings {
                width: 16_386,
                ..settings()
            },
            EncoderSettings {
                frame_rate: FrameRate { num: 25, den: 0 },
                ..settings()
            },
            EncoderSettings {
                preset: "--help".into(),
                ..settings()
            },
            EncoderSettings {
                rate: RateControl::Crf(52.0),
                ..settings()
            },
            EncoderSettings {
                rate: RateControl::Crf(f32::NAN),
                ..settings()
            },
            EncoderSettings {
                rate: RateControl::Bitrate(0),
                ..settings()
            },
            EncoderSettings {
                rate: RateControl::Bitrate(MAX_BITRATE + 1),
                ..settings()
            },
            EncoderSettings {
                threads: Some(0),
                ..settings()
            },
            EncoderSettings {
                sample_aspect: Some((65_536, 1)),
                ..settings()
            },
            EncoderSettings {
                sample_aspect: Some((0, 1)),
                ..settings()
            },
        ];
        for s in bad {
            assert!(s.validate().is_err(), "{s:?}");
            assert!(x264_args(&s).is_err() && ffmpeg_args(&s).is_err(), "{s:?}");
        }
        // 4:2:2 needs an even width only; 4:4:4 neither.
        let y422 = EncoderSettings {
            height: 1081,
            format: PixelFormat::Yuv {
                chroma: Chroma::Cs422,
                depth: 8,
            },
            ..settings()
        };
        assert!(y422.validate().is_ok());
        let y444 = EncoderSettings {
            width: 1919,
            height: 1081,
            format: PixelFormat::Yuv {
                chroma: Chroma::Cs444,
                depth: 10,
            },
            ..settings()
        };
        assert!(y444.validate().is_ok());
        assert_eq!(y444.frame_bytes().unwrap(), 1919 * 1081 * 3 * 2);
        assert_eq!(settings().frame_bytes().unwrap(), 1920 * 1080 * 3 / 2);
        // A ratio that reduces into 16 bits is fine, reduced.
        let reduced = EncoderSettings {
            sample_aspect: Some((131_070, 131_068)),
            ..settings()
        };
        assert!(
            x264_args(&reduced)
                .unwrap()
                .contains(&"65535:65534".to_owned())
        );
        assert!(
            ffmpeg_args(&reduced)
                .unwrap()
                .contains(&"sar=65535/65534".to_owned())
        );
    }

    #[test]
    fn x264_runs_without_huge_pages_on_samples_of_more_than_8_bits() {
        let x264 = EncoderProgram::X264(PathBuf::from("/opt/x264"));
        let ffmpeg = EncoderProgram::Ffmpeg(PathBuf::from("/opt/ffmpeg"));
        for chroma in [Chroma::Cs420, Chroma::Cs422, Chroma::Cs444] {
            let at = |depth| PixelFormat::Yuv { chroma, depth };
            assert_eq!(x264.huge_pages(at(10)), HugePages::Off);
            assert_eq!(x264.huge_pages(at(8)), HugePages::Kept);
            assert_eq!(ffmpeg.huge_pages(at(10)), HugePages::Kept);
            assert_eq!(ffmpeg.huge_pages(at(8)), HugePages::Kept);
        }
    }

    #[test]
    fn programs_are_found_in_the_documented_order() {
        let dir = std::env::temp_dir().join(format!("coreav-find-encoder-{}", std::process::id()));
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("ffmpeg"), b"").unwrap();
        std::fs::write(b.join("x264"), b"").unwrap();
        std::fs::write(b.join("ffmpeg"), b"").unwrap();
        let path = std::env::join_paths([&a, &b]).unwrap();
        let env_with = |vars: Vec<(&'static str, OsString)>| {
            move |name: &str| {
                vars.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v.clone())
            }
        };
        let only_path = env_with(vec![("PATH", path.clone())]);
        let find = |choice, x: Option<&str>, f: Option<&str>, env: Env<'_>| {
            find_encoder_in(choice, x.map(PathBuf::from), f.map(PathBuf::from), env)
        };
        // Auto: x264 on PATH wins over ffmpeg.
        assert_eq!(
            find(EncoderChoice::Auto, None, None, &only_path),
            Some(EncoderProgram::X264(b.join("x264")))
        );
        // A named x264 wins; a named ffmpeg does not beat x264 on PATH.
        assert_eq!(
            find(EncoderChoice::Auto, Some("/x/x264"), None, &only_path),
            Some(EncoderProgram::X264("/x/x264".into()))
        );
        assert_eq!(
            find(EncoderChoice::Auto, None, Some("/f/ffmpeg"), &only_path),
            Some(EncoderProgram::X264(b.join("x264")))
        );
        assert_eq!(
            find(EncoderChoice::Ffmpeg, None, None, &only_path),
            Some(EncoderProgram::Ffmpeg(a.join("ffmpeg")))
        );
        // COREAV_X264 naming a file wins over PATH; naming nothing finds
        // nothing, and Auto falls back to ffmpeg.
        let named = env_with(vec![
            ("PATH", path.clone()),
            ("COREAV_X264", a.join("ffmpeg").into()),
        ]);
        assert_eq!(
            find(EncoderChoice::X264, None, None, &named),
            Some(EncoderProgram::X264(a.join("ffmpeg")))
        );
        let missing = env_with(vec![
            ("PATH", path.clone()),
            ("COREAV_X264", dir.join("none").into()),
            ("COREAV_FFMPEG", b.join("ffmpeg").into()),
        ]);
        assert_eq!(find(EncoderChoice::X264, None, None, &missing), None);
        assert_eq!(
            find(EncoderChoice::Auto, None, None, &missing),
            Some(EncoderProgram::Ffmpeg(b.join("ffmpeg")))
        );
        // A bare name is looked for on PATH, and finds nothing when absent.
        assert_eq!(
            find(EncoderChoice::Ffmpeg, None, Some("ffmpeg"), &only_path),
            Some(EncoderProgram::Ffmpeg(a.join("ffmpeg")))
        );
        assert_eq!(
            find(EncoderChoice::X264, Some("x265"), None, &only_path),
            None
        );
        let nothing = env_with(vec![]);
        assert_eq!(find(EncoderChoice::Auto, None, None, &nothing), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
