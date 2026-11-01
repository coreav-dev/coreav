//! The release allocator must also fit the resource-limited exec launcher.
#![cfg(all(
    feature = "thumbnail",
    feature = "release-allocator",
    target_os = "linux",
    target_env = "musl",
    target_arch = "x86_64"
))]

use coreav_worker::{SandboxLimits, WorkerCommand};
use std::path::Path;

#[test]
fn release_allocator_fits_the_launcher_and_keeps_the_encoder_environment_empty() {
    let launcher = coreav_worker::exec::ExecLauncher {
        command: WorkerCommand {
            program: env!("CARGO_BIN_EXE_coreav").into(),
            args: vec![coreav_worker::exec::EXEC_ARG.into()],
        },
    };
    let output = launcher
        .command(
            &SandboxLimits {
                address_space: Some(512 << 20),
                cpu_seconds: Some(5),
                open_files: Some(32),
            },
            coreav_worker::exec::HugePages::Kept,
            Path::new("/usr/bin/env"),
            &[],
        )
        .output()
        .expect("the launcher must start");
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
}
