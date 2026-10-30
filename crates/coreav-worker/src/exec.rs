//! The sandbox launcher: a program run with resource limits and replaced
//! by another program (`docs/design/decode.md`, "The sandboxed worker",
//! "Exec"; `docs/design/transcode.md`, "Sandbox").
//!
//! The child side, [`sandbox_exec_main`], is what the `coreav` binary (and
//! the `coreav-worker` binary) runs when its first argument is
//! [`EXEC_ARG`]: it reads the limits from its arguments, sets them on
//! itself as the decode worker does ([`crate::sandbox`]: also no core
//! files and no regular-file writes), and replaces itself with the program
//! through `std`'s `exec`, with the standard streams it was given and an
//! empty environment. No process stays between the parent's pipes and the
//! program, and the program inherits the limits.
//!
//! The parent side, [`ExecLauncher::command`], builds the
//! [`std::process::Command`] that starts it: its own process group, an
//! empty environment, piped standard streams. Killing that group
//! ([`kill_group`]) ends the program and every process it started that
//! stayed in the group.

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};

use crate::parent::WorkerCommand;
use crate::protocol::SandboxLimits;

/// The argument after which the `coreav` binary (or the `coreav-worker`
/// binary) runs as the sandbox launcher.
pub const EXEC_ARG: &str = "__coreav_exec";

const ADDRESS_SPACE: &str = "--address-space";
const CPU_SECONDS: &str = "--cpu-seconds";
const OPEN_FILES: &str = "--open-files";
const NO_HUGE_PAGES: &str = "--no-huge-pages";
const END: &str = "--";

/// Exit status of a launcher whose arguments or limits failed, as `env`
/// and `timeout` use it.
const LAUNCH_FAILED: u8 = 125;
/// Exit status when the program exists but cannot be run.
const CANNOT_RUN: u8 = 126;
/// Exit status when the program does not exist.
const NOT_FOUND: u8 = 127;

/// Whether the program the launcher runs may use transparent huge pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HugePages {
    /// As the system's settings give them.
    Kept,
    /// None, whatever the program asks for (Linux's `PR_SET_THP_DISABLE`,
    /// which `exec` keeps); elsewhere the same as [`HugePages::Kept`].
    Off,
}

/// What the launcher was asked to run.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Request {
    limits: SandboxLimits,
    huge_pages: HugePages,
    program: PathBuf,
    args: Vec<OsString>,
}

/// Reads `[--address-space N] [--cpu-seconds N] [--open-files N]
/// [--no-huge-pages] -- PROGRAM [ARG]...`. The program must be a path (it has a `/`): with an
/// empty environment there is no `PATH` to search, and a search would
/// depend on the launcher's own defaults.
fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Request, String> {
    let mut args = args.into_iter();
    let mut limits = SandboxLimits {
        address_space: None,
        cpu_seconds: None,
        open_files: None,
    };
    let mut huge_pages = HugePages::Kept;
    loop {
        let Some(flag) = args.next() else {
            return Err("no program after the limits".into());
        };
        let slot = match flag.to_str() {
            Some(END) => break,
            Some(NO_HUGE_PAGES) => {
                if huge_pages == HugePages::Off {
                    return Err(format!("{NO_HUGE_PAGES} given twice"));
                }
                huge_pages = HugePages::Off;
                continue;
            }
            Some(ADDRESS_SPACE) => &mut limits.address_space,
            Some(CPU_SECONDS) => &mut limits.cpu_seconds,
            Some(OPEN_FILES) => &mut limits.open_files,
            _ => return Err(format!("unknown argument {}", flag.to_string_lossy())),
        };
        let value = args
            .next()
            .and_then(|v| v.to_str()?.parse::<u64>().ok())
            .ok_or_else(|| format!("{} needs a number", flag.to_string_lossy()))?;
        if slot.replace(value).is_some() {
            return Err(format!("{} given twice", flag.to_string_lossy()));
        }
    }
    let program = PathBuf::from(args.next().ok_or("no program after --")?);
    if !program.as_os_str().as_encoded_bytes().contains(&b'/') {
        return Err(format!(
            "the program must be a path, not a name to search for: {}",
            program.display()
        ));
    }
    Ok(Request {
        limits,
        huge_pages,
        program,
        args: args.collect(),
    })
}

/// The arguments [`parse`] reads for `limits`, `huge_pages`, `program`
/// and `args`.
fn launch_args(
    limits: &SandboxLimits,
    huge_pages: HugePages,
    program: &Path,
    args: &[OsString],
) -> Vec<OsString> {
    let mut out = Vec::with_capacity(args.len() + 8);
    for (flag, value) in [
        (ADDRESS_SPACE, limits.address_space),
        (CPU_SECONDS, limits.cpu_seconds),
        (OPEN_FILES, limits.open_files),
    ] {
        if let Some(value) = value {
            out.push(flag.into());
            out.push(value.to_string().into());
        }
    }
    if huge_pages == HugePages::Off {
        out.push(NO_HUGE_PAGES.into());
    }
    out.push(END.into());
    out.push(program.as_os_str().to_owned());
    out.extend(args.iter().cloned());
    out
}

/// Runs the launcher on `args`, the arguments after [`EXEC_ARG`]: sets the
/// limits, then replaces this process with the program, when `allow`
/// allows it. Returns only when that fails, with 125 for bad arguments or
/// limits that could not be set, 126 for a program that cannot be run or
/// that `allow` refuses (with its message) and 127 for one that does not
/// exist, after a line on standard error. Outside Unix it always fails.
pub fn sandbox_exec_main(
    args: impl IntoIterator<Item = OsString>,
    allow: impl FnOnce(&Path) -> Result<(), String>,
) -> ExitCode {
    let request = match parse(args) {
        Ok(request) => request,
        Err(e) => return report(&e, LAUNCH_FAILED),
    };
    match allow(&request.program) {
        Ok(()) => run(&request),
        Err(e) => report(&e, CANNOT_RUN),
    }
}

/// One line on standard error, and the exit status. A failed write is
/// ignored: there is nowhere else to say it, and the status says enough.
fn report(message: &str, status: u8) -> ExitCode {
    let _ = writeln!(io::stderr(), "coreav exec: {message}");
    ExitCode::from(status)
}

#[cfg(unix)]
fn run(request: &Request) -> ExitCode {
    use std::os::unix::process::CommandExt;
    if let Err(e) = crate::sandbox::apply(&request.limits) {
        return report(&format!("cannot set the limits: {e}"), LAUNCH_FAILED);
    }
    if let Err(e) = huge_pages_off(request.huge_pages) {
        return report(&format!("cannot turn huge pages off: {e}"), LAUNCH_FAILED);
    }
    // `exec` returns only on failure. The standard streams are this
    // process's, which the parent piped; `std` restores `SIGPIPE`'s
    // default action for the new program.
    let error = Command::new(&request.program)
        .args(&request.args)
        .env_clear()
        .exec();
    let status = if error.kind() == io::ErrorKind::NotFound {
        NOT_FOUND
    } else {
        CANNOT_RUN
    };
    report(
        &format!("cannot run {}: {error}", request.program.display()),
        status,
    )
}

#[cfg(target_os = "linux")]
fn huge_pages_off(huge_pages: HugePages) -> io::Result<()> {
    match huge_pages {
        HugePages::Kept => Ok(()),
        HugePages::Off => Ok(rustix::thread::disable_transparent_huge_pages(true)?),
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn huge_pages_off(_: HugePages) -> io::Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn run(_: &Request) -> ExitCode {
    report("not supported on this system", LAUNCH_FAILED)
}

/// How to start the sandbox launcher: a program whose `main` calls
/// [`sandbox_exec_main`] when its first argument is [`EXEC_ARG`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecLauncher {
    /// The launcher program and the arguments before the limits.
    pub command: WorkerCommand,
}

impl ExecLauncher {
    /// This program, run again with [`EXEC_ARG`], as `coreav` does.
    pub fn current_exe() -> io::Result<Self> {
        WorkerCommand::current_exe(EXEC_ARG).map(|command| Self { command })
    }

    /// The `coreav-worker` binary next to this program, else on `PATH`,
    /// run with [`EXEC_ARG`].
    pub fn installed() -> Option<Self> {
        WorkerCommand::installed().map(|command| Self {
            command: WorkerCommand {
                args: vec![EXEC_ARG.into()],
                ..command
            },
        })
    }

    /// A command that runs `program` with `args` under `limits` and
    /// `huge_pages`: through the launcher, in a process group of its own
    /// (Unix), with an empty environment and piped standard input, output
    /// and error. The program must be a path, never a name to search for.
    pub fn command(
        &self,
        limits: &SandboxLimits,
        huge_pages: HugePages,
        program: &Path,
        args: &[OsString],
    ) -> Command {
        let mut command = Command::new(&self.command.program);
        command
            .args(&self.command.args)
            .args(launch_args(limits, huge_pages, program, args));
        isolate(&mut command);
        command
    }
}

/// A command for `program` with `args` run directly, without the launcher
/// and its limits, isolated as [`ExecLauncher::command`] does otherwise.
pub fn direct_command(program: &Path, args: &[OsString]) -> Command {
    let mut command = Command::new(program);
    command.args(args);
    isolate(&mut command);
    command
}

/// An empty environment, piped standard streams, and a process group of
/// its own, so that [`kill_group`] reaches the processes it starts.
fn isolate(command: &mut Command) {
    command
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
}

/// Kills the process group `child` leads (Unix). Call it before the child
/// is reaped, so the group's id cannot have been reused.
#[cfg(unix)]
pub fn kill_group(child: &Child) {
    use rustix::process::{Pid, Signal, kill_process_group};
    let _ = kill_process_group(Pid::from_child(child), Signal::KILL);
}

/// Without process groups (not Unix), nothing: only the child itself can
/// be killed (`Child::kill`).
#[cfg(not(unix))]
pub fn kill_group(_: &Child) {}

/// Whether `child` has exited, without reaping it: Linux's `waitid` with
/// `WNOWAIT`, so its process group id cannot be reused yet and
/// [`kill_group`] stays safe. Elsewhere it says no (an exit cannot be seen
/// there without reaping).
#[cfg(target_os = "linux")]
pub fn has_exited(child: &mut Child) -> bool {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    matches!(
        waitid(WaitId::Pid(Pid::from_child(child)), options),
        Ok(Some(_))
    )
}

/// Whether `child` has exited, without reaping it: not known outside
/// Linux, so no.
#[cfg(not(target_os = "linux"))]
pub fn has_exited(_: &mut Child) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn arguments_round_trip() {
        let limits = SandboxLimits {
            address_space: Some(1 << 30),
            cpu_seconds: Some(70),
            open_files: None,
        };
        let args = launch_args(
            &limits,
            HugePages::Off,
            Path::new("/opt/x/x264"),
            &os(&["--crf", "23", "-"]),
        );
        assert_eq!(
            args,
            os(&[
                "--address-space",
                "1073741824",
                "--cpu-seconds",
                "70",
                "--no-huge-pages",
                "--",
                "/opt/x/x264",
                "--crf",
                "23",
                "-"
            ])
        );
        let request = parse(args).unwrap();
        assert_eq!(request.limits, limits);
        assert_eq!(request.huge_pages, HugePages::Off);
        assert_eq!(request.program, PathBuf::from("/opt/x/x264"));
        assert_eq!(request.args, os(&["--crf", "23", "-"]));
    }

    #[test]
    fn bad_arguments_are_refused() {
        for bad in [
            &[][..],
            &["--address-space"],
            &["--address-space", "x", "--", "/bin/true"],
            &["--open-files", "1", "--open-files", "2", "--", "/bin/true"],
            &["--nice", "1", "--", "/bin/true"],
            &["--no-huge-pages", "--no-huge-pages", "--", "/bin/true"],
            &["--no-huge-pages", "1", "--", "/bin/true"],
            &["--"],
            &["--", "true"],
            &["/bin/true"],
        ] {
            assert!(parse(os(bad)).is_err(), "{bad:?}");
        }
        // After `--`, everything is the program's, flags included.
        let request = parse(os(&["--", "./x264", "--", "--open-files"])).unwrap();
        assert_eq!(request.args, os(&["--", "--open-files"]));
    }
}
