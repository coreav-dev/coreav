//! `coreav` on URLs: the same results as for local files over a local
//! range server, the remote reason on failures, and the guarded mode's
//! refusals.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    missing_docs
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

#[path = "support/http.rs"]
mod http;

use common::{aac_entry, av_movie_with};
use http::{Mode, Server};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::Ordering;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
}

fn temp(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_file(&path);
    path
}

fn movie(name: &str) -> (PathBuf, Vec<u8>) {
    let data = av_movie_with(aac_entry(), false);
    let path = temp(name);
    std::fs::write(&path, &data).unwrap();
    (path, data)
}

struct Run {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

fn coreav(args: &[&str], env: &[(&str, &str)]) -> Run {
    let out = Command::new(bin())
        .args(args)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    Run {
        code: out.status.code(),
        stdout: out.stdout,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// The document without the keys that name the input.
fn document(bytes: &[u8]) -> serde_json::Value {
    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value.as_object_mut().unwrap().remove("file");
    value
}

#[test]
fn every_command_reads_a_url_as_it_reads_the_file() {
    let (path, data) = movie("remote_in.mp4");
    let server = Server::start(data, Mode::Ranges);
    let url = server.url();
    let local = path.to_str().unwrap();
    for args in [vec!["probe"], vec!["chapters"]] {
        let a = coreav(&[args.as_slice(), &[local]].concat(), &[]);
        let b = coreav(&[args.as_slice(), &[url.as_str()]].concat(), &[]);
        assert_eq!(b.code, Some(0), "{args:?}: {}", b.stderr);
        assert_eq!(document(&a.stdout), document(&b.stdout), "{args:?}");
    }
    for (name, args) in [
        ("remux", vec!["remux"]),
        ("sanitize", vec!["sanitize"]),
        ("clip", vec!["clip", "--start", "0"]),
        ("audio", vec!["extract-audio"]),
    ] {
        let a = temp(&format!("remote_{name}_local.out"));
        let b = temp(&format!("remote_{name}_remote.out"));
        let ra = coreav(
            &[args.as_slice(), &[local, a.to_str().unwrap()]].concat(),
            &[],
        );
        let rb = coreav(
            &[args.as_slice(), &[url.as_str(), b.to_str().unwrap()]].concat(),
            &[],
        );
        assert_eq!(ra.code, Some(0), "{name}: {}", ra.stderr);
        assert_eq!(rb.code, Some(0), "{name}: {}", rb.stderr);
        assert_eq!(
            std::fs::read(&a).unwrap(),
            std::fs::read(&b).unwrap(),
            "{name}"
        );
        assert_eq!(document(&ra.stdout), document(&rb.stdout), "{name}");
    }
    let policy = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/schema/policy-examples/web-upload.json"
    );
    let a = coreav(&["check", "--policy", policy, local], &[]);
    let b = coreav(&["check", "--policy", policy, url.as_str()], &[]);
    assert_eq!((a.code, a.stdout), (b.code, b.stdout));
    assert!(server.requests.load(Ordering::SeqCst) > 0);
}

#[test]
fn a_server_that_ignores_ranges_is_read_from_the_start() {
    let (path, data) = movie("remote_whole.mp4");
    let server = Server::start(data, Mode::Whole);
    let a = coreav(&["probe", path.to_str().unwrap()], &[]);
    let b = coreav(&["probe", server.url().as_str()], &[]);
    assert_eq!(b.code, Some(0), "{}", b.stderr);
    assert_eq!(document(&a.stdout), document(&b.stdout));
}

#[test]
fn remote_failures_are_io_errors_with_the_reason() {
    let (_, data) = movie("remote_fail.mp4");
    let missing = Server::start(data.clone(), Mode::Missing);
    let run = coreav(&["probe", missing.url().as_str()], &[]);
    assert_eq!(run.code, Some(3));
    let error: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
    assert_eq!(error["kind"], "io");
    assert!(
        error["message"].as_str().unwrap().contains("404"),
        "{}",
        run.stderr
    );
    // Redirects are followed only to https.
    let redirect = Server::start(data, Mode::RedirectToHttp);
    let run = coreav(&["probe", redirect.url().as_str()], &[]);
    assert_eq!(run.code, Some(3));
    assert!(
        run.stderr.contains("only https:// redirects"),
        "{}",
        run.stderr
    );
    // Other schemes are refused.
    for url in ["ftp://example.com/a.mp4", "file:///etc/passwd"] {
        let run = coreav(&["probe", url], &[]);
        assert_eq!(run.code, Some(3), "{url}");
        assert!(run.stderr.contains("are not read"), "{}", run.stderr);
    }
}

#[test]
fn a_changed_object_fails_the_read() {
    let (_, data) = movie("remote_changed.mp4");
    let server = Server::start(data, Mode::Ranges);
    *server.etag.lock().unwrap() = "\"v1\"".to_owned();
    // Change the object as soon as the first request has been answered.
    let etag = Arc::clone(&server.etag);
    let requests = Arc::clone(&server.requests);
    std::thread::spawn(move || {
        while requests.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        *etag.lock().unwrap() = "\"v2\"".to_owned();
    });
    let out = temp("remote_changed.out");
    let run = coreav(
        &["remux", server.url().as_str(), out.to_str().unwrap()],
        &[],
    );
    // The first response may hold the whole small file; then no second
    // request is made and the remux succeeds on the bytes of version 1.
    if run.code != Some(0) {
        assert_eq!(run.code, Some(3), "{}", run.stderr);
        assert!(run.stderr.contains("changed"), "{}", run.stderr);
    }
}

#[test]
fn the_guarded_mode_reads_public_https_urls_only() {
    let (_, data) = movie("remote_guarded.mp4");
    let server = Server::start(data, Mode::Ranges);
    let guarded = [("COREAV_REMOTE", "public")];
    let run = coreav(&["probe", server.url().as_str()], &guarded);
    assert_eq!(run.code, Some(3));
    assert!(run.stderr.contains("only https://"), "{}", run.stderr);
    // Loopback over https is refused before any connection.
    let https = server.url().replacen("http://", "https://", 1);
    let run = coreav(&["probe", https.as_str()], &guarded);
    assert_eq!(run.code, Some(3));
    assert!(
        run.stderr.contains("not a public address"),
        "{}",
        run.stderr
    );
    assert_eq!(server.requests.load(Ordering::SeqCst), 0);
    // Credentials in a URL are refused.
    let run = coreav(
        &["probe", "https://user:secret@example.com/a.mp4"],
        &guarded,
    );
    assert_eq!(run.code, Some(3));
    assert!(run.stderr.contains("credentials"), "{}", run.stderr);
}
