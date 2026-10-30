//! CoreAV's sandboxed decode worker (`docs/design/decode.md`, "The
//! sandboxed worker").
//!
//! Third-party decoders run in a separate process: the parent starts it
//! ([`SandboxedDecoder`], or [`SandboxedBackend`] for backend selection),
//! talks to it over `std` pipes ([`protocol`]) and receives frames through
//! sealed shared memory ([`shm`], Linux) or the pipe. The parent treats the
//! worker as untrusted and possibly compromised: it validates every frame
//! descriptor before it reads a byte of the frame ([`descriptor`]) and owns
//! every frame it hands out.
//!
//! The same crate holds the sandbox launcher ([`exec`]): the program that
//! sets the worker's limits on itself and replaces itself with another
//! program, the encoder of `coreav transcode`.
//!
//! `unsafe` is denied crate-wide and allowed only in [`shm`], for the
//! worker's `memmap2` mapping call and the parent's pointer copy out of its
//! mapping; every other module is `#![forbid(unsafe_code)]`.

#![deny(unsafe_code)]

mod child;
pub mod descriptor;
pub mod exec;
mod parent;
pub mod protocol;
mod sandbox;
#[cfg(target_os = "linux")]
pub mod shm;

pub use child::worker_main;
pub use exec::{EXEC_ARG, ExecLauncher, HugePages, sandbox_exec_main};
pub use parent::{
    SandboxedBackend, SandboxedDecoder, TransferMode, WorkerCommand, WorkerOptions, find_ffmpeg,
    find_on_path,
};
pub use protocol::{SandboxLimits, WorkerBackend};

/// The argument after which the `coreav` binary runs as a worker.
pub const WORKER_ARG: &str = "__coreav_worker";
