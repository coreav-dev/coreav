//! `coreav ffprobe` and `coreav ffmpeg`, and the same binary run under the
//! names `ffprobe` and `ffmpeg` (`docs/design/compat-cli.md`, "Surface").
//!
//! The arguments are the tools' own, so they are read here, not by clap.
//! Messages are plain text on standard error, as the tools write theirs;
//! standard output is the tool's output and nothing else. Every command
//! that is not native goes to [`fallback_cmd::refuse`], which refuses it or
//! runs FFmpeg in its place.

use std::ffi::OsString;
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::process::ExitCode;

use coreav_cli::error::Failure;
use coreav_cli::fallback::{NotNative, Tool};
use coreav_cli::ffmpeg::{self, Refusal, Stop};
use coreav_cli::ffprobe::{self, args::Refusal as ProbeRefusal};
use coreav_cli::strict_safe::StrictSafe;
use coreav_isobmff::{BufferedSource, FileSource, Source};

use super::{EXIT_IO, EXIT_REJECTED, EXIT_USAGE, fallback_cmd};

/// Exit status of a command the compatibility layer does not implement:
/// an option, a value or a property outside its vocabulary.
pub const EXIT_UNSUPPORTED: u8 = 4;

/// Which tool a program name stands for, when the binary is started
/// through a link named `ffprobe` or `ffmpeg`.
pub fn multicall(arg0: &OsString) -> Option<&'static str> {
    let name = Path::new(arg0).file_name()?.to_str()?;
    let name = if name.len() > 4 && name.get(name.len() - 4..)?.eq_ignore_ascii_case(".exe") {
        name.get(..name.len() - 4)?
    } else {
        name
    };
    let same = |tool: &str| {
        if cfg!(windows) {
            name.eq_ignore_ascii_case(tool)
        } else {
            name == tool
        }
    };
    if same("ffprobe") {
        Some("ffprobe")
    } else if same("ffmpeg") {
        Some("ffmpeg")
    } else {
        None
    }
}

fn message(tool: &str, text: &str) {
    let _ = writeln!(std::io::stderr(), "coreav {tool}: {text}");
}

fn strings(tool: &str, args: &[OsString]) -> Result<Vec<String>, ExitCode> {
    args.iter()
        .map(|a| a.clone().into_string())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| {
            message(tool, "an argument is not valid UTF-8");
            ExitCode::from(EXIT_USAGE)
        })
}

/// `coreav ffmpeg` with `ffmpeg`'s arguments, in `coreav --strict-safe`'s
/// mode or not.
pub fn run_ffmpeg(args: &[OsString], strict_safe: StrictSafe) -> ExitCode {
    let args = match strings("ffmpeg", args) {
        Ok(args) => args,
        Err(code) => return code,
    };
    let mut spool = None;
    if let Ok(command) = ffmpeg::args::parse(&args)
        && ffmpeg::args::stdin(&command.input)
        && command.more_inputs.is_empty()
        && matches!(
            command.muxer,
            ffmpeg::args::Muxer::S16le | ffmpeg::args::Muxer::F32le
        )
        && !matches!(
            command.demuxer,
            Some(ffmpeg::args::Demuxer::S16le(_) | ffmpeg::args::Demuxer::Concat { .. })
        )
    {
        match EncodedInput::read() {
            Ok(input) => spool = Some(input),
            Err(error) => {
                message("ffmpeg", &format!("could not spool encoded stdin: {error}"));
                return ExitCode::from(EXIT_IO);
            }
        }
    }
    let result = match &spool {
        Some(input) => ffmpeg::run_spooled(&args, strict_safe, &input.path),
        None => ffmpeg::run(&args, strict_safe),
    };
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(Stop::Refused(Refusal::NotNative(refusal))) => {
            if let Some(input) = &mut spool {
                let replay = input
                    .file
                    .as_mut()
                    .ok_or_else(|| std::io::Error::other("encoded stdin file is closed"))
                    .and_then(|file| file.rewind().and_then(|()| file.try_clone()));
                #[cfg(unix)]
                let replay =
                    replay.and_then(|file| std::fs::remove_file(&input.path).map(|()| file));
                match replay {
                    Ok(file) => fallback_cmd::refuse_with_input(
                        Tool::Ffmpeg,
                        &args,
                        refusal,
                        strict_safe,
                        file,
                    ),
                    Err(error) => {
                        message(
                            "ffmpeg",
                            &format!("could not replay encoded stdin: {error}"),
                        );
                        ExitCode::from(EXIT_IO)
                    }
                }
            } else {
                fallback_cmd::refuse(Tool::Ffmpeg, &args, refusal, strict_safe)
            }
        }
        Err(Stop::Refused(Refusal::Usage(text))) => {
            message("ffmpeg", &text);
            ExitCode::from(EXIT_USAGE)
        }
        Err(Stop::Refused(Refusal::Help)) => print_usage(ffmpeg::USAGE),
        Err(Stop::Failed(failure)) => {
            if matches!(
                failure,
                coreav_cli::output::WriteFailure::Rejected(Failure::Io(
                    std::io::ErrorKind::BrokenPipe
                ))
            ) {
                return ExitCode::from(super::EXIT_BROKEN_PIPE);
            }
            let json = failure.to_json();
            let text = json
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| json.to_string(), str::to_owned);
            message("ffmpeg", &format!("{}: {text}", args_input(&args)));
            ExitCode::from(if failure.is_io() {
                EXIT_IO
            } else {
                EXIT_REJECTED
            })
        }
    }
}

struct EncodedInput {
    path: std::path::PathBuf,
    file: Option<std::fs::File>,
}

impl EncodedInput {
    fn read() -> std::io::Result<Self> {
        let tag = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_nanos();
        let mut created = None;
        for attempt in 0..32 {
            let path = std::env::temp_dir().join(format!(
                "coreav-encoded-stdin-{}-{tag}-{attempt}",
                std::process::id()
            ));
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    created = Some(Self {
                        path,
                        file: Some(file),
                    });
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        let mut input = created.ok_or_else(|| {
            std::io::Error::other("could not exclusively create encoded stdin file")
        })?;
        let file = input
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("encoded stdin file is closed"))?;
        let mut stdin = std::io::stdin().lock();
        let mut bytes = [0u8; 65_536];
        let mut size = 0u64;
        loop {
            let count = match stdin.read(&mut bytes) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                break;
            }
            size += count as u64;
            if size > 1_073_741_824 {
                return Err(std::io::Error::other(
                    "encoded stdin exceeds the 1 GiB limit",
                ));
            }
            file.write_all(
                bytes
                    .get(..count)
                    .ok_or_else(|| std::io::Error::other("encoded stdin read buffer"))?,
            )?;
        }
        file.rewind()?;
        Ok(input)
    }
}

impl Drop for EncodedInput {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(feature = "input-capture")]
pub fn run_ffmpeg_captured(args: &[OsString], strict_safe: StrictSafe, target: &Path) -> ExitCode {
    let args = match strings("ffmpeg", args) {
        Ok(args) => args,
        Err(code) => return code,
    };
    match ffmpeg::run_captured(&args, strict_safe, target) {
        Ok(_) => ExitCode::SUCCESS,
        Err(Stop::Refused(Refusal::Usage(text))) => {
            message("ffmpeg", &text);
            ExitCode::from(EXIT_USAGE)
        }
        Err(Stop::Refused(_)) => {
            message(
                "ffmpeg",
                "input capture requires a supported native AAC encode",
            );
            ExitCode::from(EXIT_UNSUPPORTED)
        }
        Err(Stop::Failed(failure)) => {
            message("ffmpeg", &failure.to_json().to_string());
            ExitCode::from(if failure.is_io() {
                EXIT_IO
            } else {
                EXIT_REJECTED
            })
        }
    }
}

/// A tool's usage text on standard output, for `-h` and `--help`.
fn print_usage(text: &str) -> ExitCode {
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => super::write_error(&error),
    }
}

/// The value of the first `-i`, to name the input in a message.
fn args_input(args: &[String]) -> &str {
    args.iter()
        .position(|a| a == "-i")
        .and_then(|i| args.get(i + 1))
        .map_or("", String::as_str)
}

/// `coreav ffprobe` with `ffprobe`'s arguments, in `coreav --strict-safe`'s
/// mode or not.
pub fn run_ffprobe(args: &[OsString], strict_safe: StrictSafe) -> ExitCode {
    let args = match strings("ffprobe", args) {
        Ok(args) => args,
        Err(code) => return code,
    };
    let options = match ffprobe::args::parse(&args) {
        Ok(options) => options,
        Err(ProbeRefusal::Help) => return print_usage(ffprobe::USAGE),
        Err(ProbeRefusal::Unsupported(refusal)) => {
            return fallback_cmd::refuse(Tool::Ffprobe, &args, refusal, strict_safe);
        }
        Err(ProbeRefusal::Usage(text)) => {
            message("ffprobe", &text);
            return ExitCode::from(EXIT_USAGE);
        }
    };
    #[cfg(not(feature = "net"))]
    if options.input.starts_with("http://") || options.input.starts_with("https://") {
        message(
            "ffprobe",
            "this build has no network; URL inputs are not supported",
        );
        return ExitCode::from(EXIT_USAGE);
    }
    if !options.sections {
        let refusal = NotNative::property(
            "no section",
            "a command without -show_* or -show_entries is not supported: ffprobe's summary of the input on standard error is not reproduced; ask for the sections, for example -show_format -show_streams",
        );
        return fallback_cmd::refuse(Tool::Ffprobe, &args, refusal, strict_safe);
    }
    let input = options.input.clone();
    let result = if coreav_cli::remote_url(&input) {
        remote(&input, &options)
    } else {
        local(&input, &options)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Outcome::Write(error)) => super::write_error(&error),
        // A command the file shows to need what is not implemented.
        Err(Outcome::Read(Failure::Unsupported(text))) => {
            let refusal = NotNative::property("what the input needs", text);
            fallback_cmd::refuse(Tool::Ffprobe, &args, refusal, strict_safe)
        }
        Err(Outcome::Read(failure)) => {
            let (code, text) = describe(&failure);
            if options.show_error {
                let mut out = std::io::stdout().lock();
                if let Err(Failure::Io(kind)) = ffprobe::emit_error(&options, code, &text, &mut out)
                {
                    return super::write_error(&kind.into());
                }
            }
            message("ffprobe", &format!("{input}: {text}"));
            ExitCode::from(if failure.is_io() {
                EXIT_IO
            } else {
                EXIT_REJECTED
            })
        }
    }
}

enum Outcome {
    /// The input could not be read or was rejected.
    Read(Failure),
    /// Standard output could not be written.
    Write(std::io::Error),
}

/// The error code and message `ffprobe` reports for a failure like this
/// one, as `-show_error` prints them.
fn describe(failure: &Failure) -> (i64, String) {
    match failure {
        Failure::Io(std::io::ErrorKind::NotFound)
        | Failure::Library(coreav_isobmff::Error::Io(std::io::ErrorKind::NotFound)) => {
            (-2, "No such file or directory".to_owned())
        }
        Failure::Io(std::io::ErrorKind::PermissionDenied) => (-13, "Permission denied".to_owned()),
        Failure::Remote(reason) => (-5, reason.clone()),
        other if other.is_io() => (-5, "Input/output error".to_owned()),
        _ => (
            -1_094_995_529,
            "Invalid data found when processing input".to_owned(),
        ),
    }
}

fn local(input: &str, options: &ffprobe::args::Options) -> Result<(), Outcome> {
    let path = Path::new(input.strip_prefix("file:").unwrap_or(input));
    let size = std::fs::metadata(path)
        .map_err(|e| Outcome::Read(Failure::io(e)))?
        .len();
    let file = FileSource::open(path).map_err(|e| Outcome::Read(Failure::library(e)))?;
    run(file, size, input, options)
}

#[cfg(feature = "net")]
fn remote(input: &str, options: &ffprobe::args::Options) -> Result<(), Outcome> {
    let mut source = coreav_cli::remote::open(input, &coreav_cli::remote::Options::from_env())
        .map_err(Outcome::Read)?;
    let size = source.len_known();
    let result = run(&mut source, size, input, options);
    match result {
        Err(Outcome::Read(failure)) => Err(Outcome::Read(source.explain(failure))),
        other => other,
    }
}

#[cfg(not(feature = "net"))]
fn remote(_input: &str, _options: &ffprobe::args::Options) -> Result<(), Outcome> {
    Err(Outcome::Read(Failure::Unsupported(
        "this build of coreav reads local files only, not URLs".to_owned(),
    )))
}

fn run<S: Source>(
    source: S,
    size: u64,
    name: &str,
    options: &ffprobe::args::Options,
) -> Result<(), Outcome> {
    let mut source = BufferedSource::new(source);
    let mut probed =
        ffprobe::probe(&mut source, size, name, options.force).map_err(Outcome::Read)?;
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::with_capacity(1 << 16, stdout.lock());
    match ffprobe::emit(options, &mut probed, &mut out) {
        Ok(()) => Ok(()),
        Err(Failure::Io(kind)) => Err(Outcome::Write(kind.into())),
        Err(other) => Err(Outcome::Read(other)),
    }
}
