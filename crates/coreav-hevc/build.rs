//! Qualifies the allocation-capacity premise used by the private native row pipeline.

use std::{env, process::Command};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(hevc_exact_capacities)");
    println!("cargo:rerun-if-env-changed=RUSTC");
    if env::var_os("CARGO_FEATURE_NATIVE_HEVC_ROWS_PRIVATE").is_none() {
        return;
    }
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let Ok(output) = Command::new(rustc).arg("-Vv").output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let identity = String::from_utf8_lossy(&output.stdout);
    if identity.lines().any(|line| line == "release: 1.99.1")
        && identity
            .lines()
            .any(|line| line == "commit-hash: 58cf8f9c35b355199727627d07ae34a4e2846d67")
    {
        println!("cargo:rustc-cfg=hevc_exact_capacities");
    }
}
