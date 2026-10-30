//! The worker's resource limits on itself, inherited by every process it
//! starts (`docs/design/decode.md`, "Process"), set through `rustix`'s safe
//! API.

#![forbid(unsafe_code)]

use crate::protocol::SandboxLimits;

/// Applies `limits`, plus no core files and no regular-file writes
/// (`RLIMIT_FSIZE` 0: decoders write only to pipes). Each limit is set as
/// both the soft and the hard value, so the worker cannot raise it again;
/// an inherited limit (soft or hard) below the one asked for is kept. On
/// macOS the address space is not limited, so the budget is advisory there
/// (`docs/design/decode.md`, "Memory").
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
    // macOS refuses (EINVAL) any `RLIMIT_AS` below the address space a
    // process already reserves, hundreds of GiB from its start.
    #[cfg(not(target_vendor = "apple"))]
    if let Some(bytes) = limits.address_space {
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
