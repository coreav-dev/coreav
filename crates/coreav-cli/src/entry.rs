//! The `coreav` binary and its release-only allocator.

#[cfg(all(
    feature = "release-allocator",
    target_os = "linux",
    target_env = "musl",
    target_arch = "x86_64"
))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[path = "main.rs"]
mod command;

fn main() -> std::process::ExitCode {
    command::main()
}
