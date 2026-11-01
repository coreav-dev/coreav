//! The worker's resource limits on itself, inherited by every process it
//! starts (`docs/design/decode.md`, "Process"), set through `rustix`'s safe
//! API.

#![forbid(unsafe_code)]

use crate::protocol::SandboxLimits;

pub(crate) fn configure_allocator(command: &mut std::process::Command) {
    command.env(
        "MIMALLOC_ARENA_RESERVE",
        include_str!("mimalloc-arena-reserve-kib.txt").trim(),
    );
}

/// Applies `limits`, plus no core files and no regular-file writes
/// (`RLIMIT_FSIZE` 0: decoders write only to pipes). Each limit is set as
/// both the soft and the hard value, so the worker cannot raise it again;
/// an inherited limit (soft or hard) below the one asked for is kept. The
/// address space is limited only where [`enforces_memory_limit`]; elsewhere
/// the budget is advisory (`docs/design/decode.md`, "Memory").
#[cfg(unix)]
pub fn apply(limits: &SandboxLimits) -> std::io::Result<()> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    // A limit the worker inherited lower than the one asked for is kept,
    // the soft one included: a process cannot raise its hard limit, and the
    // soft one is what its parent meant.
    let set = |resource, value: u64| {
        let inherited = getrlimit(resource);
        let value = [inherited.current, inherited.maximum]
            .into_iter()
            .flatten()
            .fold(value, u64::min);
        setrlimit(
            resource,
            Rlimit {
                current: Some(value),
                maximum: Some(value),
            },
        )
    };
    set(Resource::Core, 0)?;
    set(Resource::Fsize, 0)?;
    if let Some(bytes) = limits.address_space.filter(|_| enforces_memory_limit()) {
        set(Resource::As, bytes)?;
    }
    if let Some(seconds) = limits.cpu_seconds {
        set(Resource::Cpu, seconds)?;
    }
    if let Some(files) = limits.open_files {
        set(Resource::Nofile, files)?;
    }
    Ok(())
}

/// Whether this platform holds a sandboxed process to its memory budget
/// (`RLIMIT_AS`): Unix but not macOS, which refuses (`EINVAL`) any limit
/// below the address space a process already reserves, hundreds of GiB
/// from its start. Where it does not, the budget is advisory and
/// strict-safe mode starts no sandboxed process (`docs/design/decode.md`,
/// "Memory").
pub fn enforces_memory_limit() -> bool {
    cfg!(all(unix, not(target_vendor = "apple")))
}

/// The address-space limit in force (the soft one), if any.
#[cfg(unix)]
pub fn address_space() -> Option<u64> {
    rustix::process::getrlimit(rustix::process::Resource::As).current
}

/// No address-space limit outside Unix.
#[cfg(not(unix))]
pub fn address_space() -> Option<u64> {
    None
}

/// No resource limits outside Unix.
#[cfg(not(unix))]
pub fn apply(_limits: &SandboxLimits) -> std::io::Result<()> {
    Ok(())
}
