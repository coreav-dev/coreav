//! The worker as a program of its own, for library users whose program
//! does not run itself as a worker (`WorkerCommand::installed`). With
//! `EXEC_ARG` first, it is the sandbox launcher instead
//! (`ExecLauncher::installed`).

#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new(coreav_worker::EXEC_ARG)) {
        return coreav_worker::sandbox_exec_main(args);
    }
    coreav_worker::worker_main()
}
