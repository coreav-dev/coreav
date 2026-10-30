//! The FFmpeg fallback (`docs/design/compat-cli.md`, "Fallback to FFmpeg"): a
//! command `coreav ffprobe` or `coreav ffmpeg` does not run natively runs the
//! real program instead, with the arguments unchanged and standard input,
//! output and error inherited, unless strict-safe mode is on. Standard
//! output stays the program's: CoreAV writes one notice line on standard
//! error, and the log line to `COREAV_FALLBACK_LOG`.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use coreav_cli::fallback::{self, NotNative, NotRun, Tool, executable};
use coreav_cli::strict_safe::StrictSafe;

use super::compat_cmd::{EXIT_UNSUPPORTED, multicall};

/// Set for the program that runs instead, so that a `coreav` it leads back
/// to refuses rather than loops.
const GUARD: &str = "COREAV_FALLBACK_ACTIVE";

/// Seconds from 1904-01-01 to 1970-01-01, the epoch [`coreav_cli::time::rfc3339`]
/// counts from.
const SECONDS_1904_TO_1970: u64 = 2_082_844_800;

/// A command that is not native: the program that runs in its place, whose
/// exit status becomes this one's, or the refusal with exit status 4.
pub fn refuse(
    tool: Tool,
    args: &[String],
    refusal: NotNative,
    strict_safe: StrictSafe,
) -> ExitCode {
    let found = if strict_safe.on() {
        Err((NotRun::StrictSafe, None))
    } else if std::env::var_os(GUARD).is_some() {
        Err((
            NotRun::Guard,
            Some(format!(
                "the fallback to {} led back to CoreAV ({GUARD} is set), so nothing else runs",
                tool.name()
            )),
        ))
    } else {
        program(tool)
    };
    let reason = &refusal.reason;
    match found {
        Ok(path) => {
            log(tool, args, reason, Ok(&path.to_string_lossy()));
            let _ = writeln!(
                std::io::stderr(),
                "coreav: ran {} instead (not native: {})",
                path.display(),
                reason.redacted(tool)
            );
            let error = run(&path, args);
            message(tool, &format!("could not run {}: {error}", path.display()));
            log(tool, args, reason, Err(NotRun::Unusable));
            ExitCode::from(EXIT_UNSUPPORTED)
        }
        Err((not_run, why)) => {
            message(tool, &refusal.message);
            if let Some(why) = why {
                message(tool, &why);
            }
            log(tool, args, reason, Err(not_run));
            ExitCode::from(EXIT_UNSUPPORTED)
        }
    }
}

fn message(tool: Tool, text: &str) {
    let _ = writeln!(std::io::stderr(), "coreav {}: {text}", tool.name());
}

/// The program to run: the one `COREAV_FALLBACK` names, a directory's or a
/// file, else the first on `PATH` that is not this program. Never `PATH`
/// when the variable names nothing usable, which would run another FFmpeg
/// than the one asked for.
fn program(tool: Tool) -> Result<PathBuf, (NotRun, Option<String>)> {
    let file = format!("{}{}", tool.name(), std::env::consts::EXE_SUFFIX);
    if let Some(named) = std::env::var_os("COREAV_FALLBACK").filter(|v| !v.is_empty()) {
        let named = PathBuf::from(named);
        let unusable = |why: String| {
            Err((
                NotRun::Unusable,
                Some(format!(
                    "COREAV_FALLBACK={} {why}, so nothing else runs",
                    named.display()
                )),
            ))
        };
        let path = if named.is_dir() {
            named.join(&file)
        } else if multicall(&OsString::from(named.as_os_str())) == Some(tool.name()) {
            named.clone()
        } else {
            return unusable(format!("names no {} program", tool.name()));
        };
        return if executable(&path) {
            std::path::absolute(&path).or_else(|error| unusable(error.to_string()))
        } else if path.exists() {
            unusable(format!(
                "names {}, which is not an executable file",
                path.display()
            ))
        } else {
            unusable(format!("names {}, which does not exist", path.display()))
        };
    }
    let this = std::env::current_exe().ok();
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(&file))
        .filter_map(|path| std::path::absolute(path).ok())
        .find(|path| executable(path) && !this.as_deref().is_some_and(|this| same(path, this)))
        .ok_or_else(|| {
            (
                NotRun::NoProgram,
                Some(format!("no other {} on PATH to run instead", tool.name())),
            )
        })
}

/// Whether two paths are the same program: a symbolic link to this program
/// (the multicall link) and a hard link to it included. Windows has no file
/// identity in the standard library, so there the same bytes count, which
/// takes in a copy too.
fn same(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let len = |path: &Path| std::fs::metadata(path).map(|m| m.len()).ok();
        len(a).is_some()
            && len(a) == len(b)
            && std::fs::read(a).is_ok_and(|a| std::fs::read(b).is_ok_and(|b| a == b))
    }
}

/// Appends the command's line to `COREAV_FALLBACK_LOG` in one write, so that
/// processes running at once do not interleave lines. A log that cannot be
/// written costs one line on standard error and never stops the command.
fn log(tool: Tool, args: &[String], reason: &fallback::Reason, ran: Result<&str, NotRun>) {
    let Some(path) = std::env::var_os("COREAV_FALLBACK_LOG").filter(|v| !v.is_empty()) else {
        return;
    };
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let time =
        coreav_cli::time::rfc3339(seconds.saturating_add(SECONDS_1904_TO_1970)).unwrap_or_default();
    let line = fallback::log_line(&time, tool, args, reason, ran);
    let written = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()));
    if let Err(error) = written {
        message(
            tool,
            &format!(
                "COREAV_FALLBACK_LOG={}: {error}",
                Path::new(&path).display()
            ),
        );
    }
}

/// Runs the program in this process's place: on Unix it replaces this
/// process (`exec`), so the process id, signals and exit status are the
/// program's; elsewhere it waits for it and exits with its exit code.
/// Returns only when the program could not start.
fn run(path: &Path, args: &[String]) -> std::io::Error {
    let mut command = Command::new(path);
    command.args(args).env(GUARD, "1");
    #[cfg(unix)]
    let error = {
        use std::os::unix::process::CommandExt;
        command.exec()
    };
    #[cfg(not(unix))]
    let error = match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => error,
    };
    error
}
