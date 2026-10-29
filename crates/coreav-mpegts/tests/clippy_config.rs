//! Clippy reads the `clippy.toml` nearest a crate and none above it, so
//! the crate's own (which forbids the standard maps, docs/design/mpegts.md,
//! "Per-PID state") must repeat every setting of the workspace's.
#![allow(clippy::expect_used)]

#[test]
fn the_crate_clippy_toml_repeats_the_workspace_one() {
    let dir = env!("CARGO_MANIFEST_DIR");
    let read = |path: String| std::fs::read_to_string(&path).expect(&path);
    let workspace = read(format!("{dir}/../../clippy.toml"));
    let crate_config = read(format!("{dir}/clippy.toml"));
    let lines: Vec<&str> = crate_config.lines().map(str::trim).collect();
    for line in workspace.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        assert!(
            lines.contains(&line),
            "crates/coreav-mpegts/clippy.toml lacks the workspace setting `{line}`"
        );
    }
}
