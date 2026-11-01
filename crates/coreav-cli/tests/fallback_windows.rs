#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::panic)]

use std::fs::{File, copy, hard_link};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("fallback-windows")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stand_in() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tmp("stand-in");
        let result = Command::new("rustc")
            .arg("--edition=2024")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/fallback_program.rs"))
            .arg("-o")
            .arg(dir.join("ffmpeg.exe"))
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        copy(dir.join("ffmpeg.exe"), dir.join("ffprobe.exe")).unwrap();
        dir
    })
}

fn args(tool: &str) -> &'static [&'static str] {
    match tool {
        "ffmpeg" => &["-i", "in.mp4", "-vf", "x", "out.mp4"],
        _ => &["-show_data", "in.mp4"],
    }
}

fn command(tool: &str, path: &[&Path], work: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
    command
        .arg(tool)
        .args(args(tool))
        .env_remove("COREAV_FALLBACK")
        .env_remove("COREAV_FALLBACK_ACTIVE")
        .env_remove("COREAV_STRICT_SAFE")
        .env_remove("COREAV_FALLBACK_LOG")
        .env("PATH", std::env::join_paths(path).unwrap())
        .env("FALLBACK_TEST_EXIT", "37")
        .stdin(Stdio::null())
        .stdout(File::create(work.join("stdout")).unwrap())
        .stderr(File::create(work.join("stderr")).unwrap());
    command
}

fn wait(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("CoreAV did not finish the fallback within 20 seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn waits_for_the_program_and_preserves_its_exit_code() {
    for tool in ["ffmpeg", "ffprobe"] {
        for code in [0, 37, 321] {
            let work = tmp(&format!("wait-{tool}-{code}"));
            let started = work.join("started");
            let release = work.join("release");
            let mut child = command(tool, &[stand_in()], &work)
                .env("FALLBACK_TEST_EXIT", code.to_string())
                .env("FALLBACK_TEST_STARTED", &started)
                .env("FALLBACK_TEST_RELEASE", &release)
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !started.exists() && Instant::now() < deadline {
                if let Some(status) = child.try_wait().unwrap() {
                    panic!("CoreAV exited before starting {tool}: {status:?}");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let running = child.try_wait().unwrap().is_none();
            std::fs::write(&release, b"release").unwrap();
            let status = wait(&mut child);
            assert!(started.exists(), "{tool} never started");
            assert!(running, "CoreAV exited while {tool} was still waiting");
            assert_eq!(status.code(), Some(code));
            assert_eq!(
                std::fs::read_to_string(work.join("stdout")).unwrap(),
                format!("args: {}\n", args(tool).join(" "))
            );
            let stderr = std::fs::read_to_string(work.join("stderr")).unwrap();
            assert!(stderr.contains("guard=1"), "{stderr}");
        }
    }
}

#[test]
fn skips_copies_and_hard_links_of_coreav_on_path() {
    for tool in ["ffmpeg", "ffprobe"] {
        let work = tmp(&format!("skip-{tool}"));
        let copied = work.join("copy");
        let linked = work.join("hard-link");
        std::fs::create_dir_all(&copied).unwrap();
        std::fs::create_dir_all(&linked).unwrap();
        let name = format!("{tool}.exe");
        copy(env!("CARGO_BIN_EXE_coreav"), copied.join(&name)).unwrap();
        hard_link(env!("CARGO_BIN_EXE_coreav"), linked.join(&name)).unwrap();
        let mut child = command(tool, &[&copied, &linked, stand_in()], &work)
            .spawn()
            .unwrap();
        assert_eq!(wait(&mut child).code(), Some(37));
        let stderr = std::fs::read_to_string(work.join("stderr")).unwrap();
        let program = stand_in().join(&name);
        assert!(
            stderr.contains(&format!("coreav: ran {} instead", program.display())),
            "{stderr}"
        );
        assert!(!stderr.contains("led back to CoreAV"), "{stderr}");
    }
}

#[test]
fn an_explicit_copy_of_coreav_is_stopped_by_the_recursion_guard() {
    for tool in ["ffmpeg", "ffprobe"] {
        let work = tmp(&format!("guard-{tool}"));
        let program = work.join(format!("{tool}.exe"));
        copy(env!("CARGO_BIN_EXE_coreav"), &program).unwrap();
        let mut child = command(tool, &[stand_in()], &work)
            .env("COREAV_FALLBACK", &program)
            .spawn()
            .unwrap();
        assert_eq!(wait(&mut child).code(), Some(4));
        assert!(std::fs::read(work.join("stdout")).unwrap().is_empty());
        let stderr = std::fs::read_to_string(work.join("stderr")).unwrap();
        assert!(stderr.contains("COREAV_FALLBACK_ACTIVE is set"), "{stderr}");
    }
}
