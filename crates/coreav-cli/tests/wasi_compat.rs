#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

#[test]
fn wasi_reports_unavailable_fallback_and_refuses_urls() {
    let Some(wasm) = std::env::var_os("COREAV_TEST_WASI") else {
        eprintln!(
            "Set COREAV_TEST_WASI to the built coreav.wasm to run the WASI compatibility regression"
        );
        return;
    };
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("wasi-compat");
    std::fs::create_dir_all(&dir).unwrap();
    let ffprobe = dir.join("ffprobe");
    std::fs::copy(env!("CARGO_BIN_EXE_coreav"), &ffprobe).unwrap();
    let runner = r#"
import { readFileSync } from 'node:fs';
import { WASI } from 'node:wasi';
const [wasm, path, ...args] = process.argv.slice(1);
const wasi = new WASI({ version: 'preview1', args: ['coreav', ...args],
  env: { PATH: path, COREAV_CWD: process.cwd() }, preopens: { '/': '/' }, returnOnExit: true });
const module = await WebAssembly.compile(readFileSync(wasm));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
process.exitCode = wasi.start(instance);
"#;
    for tool in ["ffprobe", "ffmpeg"] {
        let result = Command::new("node")
            .args(["--no-warnings", "--input-type=module", "-e", runner])
            .arg(&wasm)
            .arg(&dir)
            .args([tool, "-version"])
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(4));
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(
            stderr.contains(&format!("the WebAssembly build cannot run {tool}")),
            "{stderr}"
        );
        assert!(!stderr.contains("no other"), "{stderr}");
    }
    for args in [
        vec!["probe", "https://example.invalid/input.mp4"],
        vec!["ffprobe", "https://example.invalid/input.mp4"],
        vec![
            "ffprobe",
            "-show_streams",
            "https://example.invalid/input.mp4",
        ],
        vec![
            "ffmpeg",
            "-i",
            "https://example.invalid/input.mp4",
            "-c",
            "copy",
            "out.mp4",
        ],
    ] {
        let result = Command::new("node")
            .args(["--no-warnings", "--input-type=module", "-e", runner])
            .arg(&wasm)
            .arg(&dir)
            .args(&args)
            .output()
            .unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert_eq!(result.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("network"), "{stderr}");
        assert!(!stderr.contains("No such file"), "{stderr}");
    }
}
