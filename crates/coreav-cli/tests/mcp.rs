//! `coreav mcp` end to end: the protocol handshake, every tool against the
//! command's own output, paths kept inside the roots, overwrite rules,
//! inline policies, malformed messages.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;
#[path = "support/http.rs"]
mod http;

use common::{aac_entry, av_movie_with};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
}

/// A new, empty directory under Cargo's per-test temporary directory.
fn directory(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

struct Session {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Session {
    fn start(roots: &[&Path]) -> Self {
        Self::start_with(roots, &[])
    }

    fn start_with(roots: &[&Path], extra: &[&str]) -> Self {
        let mut command = Command::new(bin());
        command.arg("mcp").args(extra);
        for root in roots {
            command.arg("--root").arg(root);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut session = Self {
            child,
            input,
            output,
            next: 0,
        };
        let init = session.request(
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        session.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        session
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.input, "{message}").unwrap();
        self.input.flush().unwrap();
    }

    fn receive(&mut self) -> Value {
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let answer = self.receive();
        assert_eq!(answer["id"], id);
        answer
    }

    /// A tool call's result: (isError, the text, structuredContent).
    fn call(&mut self, tool: &str, arguments: Value) -> (bool, String, Value) {
        let answer = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        let result = &answer["result"];
        (
            result["isError"].as_bool().unwrap(),
            result["content"][0]["text"].as_str().unwrap().to_owned(),
            result["structuredContent"].clone(),
        )
    }

    fn finish(mut self) {
        drop(self.input);
        assert!(self.child.wait().unwrap().success());
    }
}

fn movie(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, av_movie_with(aac_entry(), false)).unwrap();
    path
}

fn command_output(args: &[&str]) -> Value {
    let out = Command::new(bin()).args(args).output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn the_handshake_and_the_tool_list() {
    let root = directory("mcp_handshake");
    let mut session = Session::start(&[&root]);
    assert_eq!(session.request("ping", json!({}))["result"], json!({}));
    let tools = session.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "probe",
            "check",
            "clip",
            "split",
            "remux",
            "sanitize",
            "extract_audio",
            "extract_subtitles",
            "chapters"
        ]
    );
    for tool in tools["result"]["tools"].as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert!(!tool["description"].as_str().unwrap().is_empty());
    }
    // A version the server does not speak gets its newest.
    let other = session.request("initialize", json!({ "protocolVersion": "1999-01-01" }));
    assert_eq!(other["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(other["result"]["serverInfo"]["name"], "coreav");
    session.finish();
}

#[test]
fn tools_return_what_the_command_prints() {
    let root = directory("mcp_tools");
    let input = movie(&root, "in.mp4");
    let mut session = Session::start(&[&root]);
    // probe, by a path relative to the root.
    let (error, text, document) = session.call("probe", json!({ "path": "in.mp4" }));
    assert!(!error, "{text}");
    assert_eq!(
        document,
        command_output(&["probe", input.to_str().unwrap()])
    );
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), document);
    // remux, then the same output again: refused without overwrite.
    let (error, text, summary) = session.call(
        "remux",
        json!({ "path": input.to_str().unwrap(), "output": "out.mp4" }),
    );
    assert!(!error, "{text}");
    assert_eq!(summary["schema"], "coreav.remux/1");
    assert!(root.join("out.mp4").is_file());
    let (error, text, _) = session.call("remux", json!({ "path": "in.mp4", "output": "out.mp4" }));
    assert!(error);
    assert!(text.contains("overwrite: true"), "{text}");
    let (error, text, _) = session.call(
        "remux",
        json!({ "path": "in.mp4", "output": "out.mp4", "overwrite": true }),
    );
    assert!(!error, "{text}");
    // Never over the input, even with overwrite.
    let (error, _, _) = session.call(
        "sanitize",
        json!({ "path": "in.mp4", "output": "in.mp4", "overwrite": true }),
    );
    assert!(error);
    // The other writing tools.
    let (error, text, summary) = session.call(
        "clip",
        json!({ "path": "in.mp4", "output": "clip.mp4", "start": 0, "end": "0.5" }),
    );
    assert!(!error, "{text}");
    assert_eq!(summary["schema"], "coreav.clip/1");
    std::fs::create_dir(root.join("parts")).unwrap();
    let (error, text, summary) = session.call(
        "split",
        json!({ "path": "in.mp4", "output": "parts/seg-%03d.mp4", "every": 1 }),
    );
    assert!(!error, "{text}");
    assert_eq!(summary["schema"], "coreav.split/1");
    assert!(root.join("parts/seg-000.mp4").is_file());
    let (error, _, _) = session.call(
        "split",
        json!({ "path": "in.mp4", "output": "parts/x-%d.mp4", "every": 1, "at": [1] }),
    );
    assert!(error);
    let (error, text, summary) = session.call(
        "extract_audio",
        json!({ "path": "in.mp4", "output": "audio.m4a" }),
    );
    assert!(!error, "{text}");
    assert_eq!(summary["schema"], "coreav.extract-audio/1");
    let (error, text, summary) = session.call(
        "sanitize",
        json!({ "path": "in.mp4", "output": "clean.mp4", "keep": [] }),
    );
    assert!(!error, "{text}");
    assert_eq!(summary["metadata"]["mode"], "sanitize");
    // A rejected file is an error result with the coreav.error/1 object.
    std::fs::write(root.join("junk.mp4"), b"not a movie").unwrap();
    let (error, text, _) = session.call("probe", json!({ "path": "junk.mp4" }));
    assert!(error);
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap()["schema"],
        "coreav.error/1"
    );
    session.finish();
}

#[test]
fn check_takes_a_policy_inline_or_from_a_file() {
    let root = directory("mcp_check");
    movie(&root, "in.mp4");
    let mut session = Session::start(&[&root]);
    let accept = json!({ "schema": "coreav.policy/1", "streams": { "video": {}, "audio": {} } });
    let (error, text, verdict) =
        session.call("check", json!({ "path": "in.mp4", "policy": accept }));
    assert!(!error, "{text}");
    assert_eq!(verdict["verdict"], "accept");
    // A rejected file is a verdict, not an error.
    let reject = json!({ "schema": "coreav.policy/1", "streams": { "audio": {} } });
    let (error, text, verdict) =
        session.call("check", json!({ "path": "in.mp4", "policy": reject }));
    assert!(!error, "{text}");
    assert_eq!(verdict["verdict"], "reject");
    // An invalid policy names no temporary file.
    let (error, text, _) = session.call(
        "check",
        json!({ "path": "in.mp4", "policy": { "schema": "coreav.policy/1" } }),
    );
    assert!(error);
    assert!(!text.contains("coreav-mcp-"), "{text}");
    std::fs::write(root.join("policy.json"), accept.to_string()).unwrap();
    let (error, text, verdict) = session.call(
        "check",
        json!({ "path": "in.mp4", "policy_path": "policy.json" }),
    );
    assert!(!error, "{text}");
    assert_eq!(verdict["verdict"], "accept");
    let (error, _, _) = session.call(
        "check",
        json!({ "path": "in.mp4", "policy": accept, "policy_path": "policy.json" }),
    );
    assert!(error);
    let pid = session.child.id();
    session.finish();
    let left = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            name.to_string_lossy()
                .starts_with(&format!("coreav-mcp-{pid}-"))
        })
        .count();
    assert_eq!(left, 0, "a temporary policy was left behind");
}

#[test]
fn paths_stay_inside_the_roots() {
    let root = directory("mcp_roots");
    let outside = directory("mcp_roots_outside");
    let secret = movie(&outside, "secret.mp4");
    movie(&root, "in.mp4");
    let mut session = Session::start(&[&root]);
    for path in [
        secret.to_str().unwrap().to_owned(),
        "../mcp_roots_outside/secret.mp4".to_owned(),
        "http://127.0.0.1:9/a.mp4".to_owned(),
        String::new(),
    ] {
        let (error, text, _) = session.call("probe", json!({ "path": path }));
        assert!(error, "{path}: {text}");
    }
    let (error, text, _) = session.call(
        "remux",
        json!({ "path": "in.mp4", "output": outside.join("out.mp4") }),
    );
    assert!(error, "{text}");
    assert!(!outside.join("out.mp4").exists());
    let (error, _, _) = session.call(
        "remux",
        json!({ "path": "in.mp4", "output": "../mcp_roots_outside/out.mp4" }),
    );
    assert!(error);
    #[cfg(unix)]
    {
        // A link inside the root to a file outside is followed, then refused.
        std::os::unix::fs::symlink(&secret, root.join("link.mp4")).unwrap();
        let (error, _, _) = session.call("probe", json!({ "path": "link.mp4" }));
        assert!(error);
        // A link as the output is refused, even with overwrite.
        std::os::unix::fs::symlink(outside.join("target.mp4"), root.join("out.mp4")).unwrap();
        let (error, text, _) = session.call(
            "remux",
            json!({ "path": "in.mp4", "output": "out.mp4", "overwrite": true }),
        );
        assert!(error, "{text}");
        assert!(!outside.join("target.mp4").exists());
    }
    // Whether a path outside exists is not told.
    let (_, there, _) = session.call("probe", json!({ "path": secret }));
    let (_, missing, _) = session.call("probe", json!({ "path": outside.join("missing.mp4") }));
    assert!(
        there.contains("outside") && missing.contains("outside"),
        "{there} / {missing}"
    );
    #[cfg(unix)]
    {
        // The segment pattern expands only in the file name: a directory
        // `a%d` next to links `a0` and `a1` pointing outside cannot route
        // the segments there.
        std::fs::create_dir(root.join("a%d")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("a0")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("a1")).unwrap();
        let (error, text, _) = session.call(
            "split",
            json!({ "path": "in.mp4", "output": "a%d/seg.mp4", "every": 1 }),
        );
        assert!(error, "{text}");
        assert!(!outside.join("seg.mp4").exists());
    }
    // A second root is usable.
    session.finish();
    let mut session = Session::start(&[&root, &outside]);
    let (error, text, _) = session.call("probe", json!({ "path": secret }));
    assert!(!error, "{text}");
    session.finish();
}

#[test]
fn malformed_messages_get_errors_and_the_server_goes_on() {
    let root = directory("mcp_malformed");
    let mut session = Session::start(&[&root]);
    writeln!(session.input, "not json").unwrap();
    assert_eq!(session.receive()["error"]["code"], -32700);
    writeln!(session.input, "[1, 2]").unwrap();
    assert_eq!(session.receive()["error"]["code"], -32600);
    // A line longer than the limit is skipped whole.
    let long = format!(
        "{{\"x\": \"{}\"}}",
        "a".repeat(coreav_cli::mcp::MAX_MESSAGE_BYTES)
    );
    writeln!(session.input, "{long}").unwrap();
    assert_eq!(session.receive()["error"]["code"], -32700);
    assert_eq!(session.request("nope", json!({}))["error"]["code"], -32601);
    assert_eq!(
        session.request("tools/call", json!({ "name": "nope" }))["error"]["code"],
        -32602
    );
    // Bad arguments are tool errors the agent can read.
    for (tool, arguments) in [
        ("probe", json!({})),
        ("probe", json!({ "path": 3 })),
        ("probe", json!({ "path": "x.mp4", "bogus": true })),
        (
            "clip",
            json!({ "path": "x.mp4", "output": "y.mp4", "start": -1 }),
        ),
        (
            "remux",
            json!({ "path": "x.mp4", "output": "y.mp4", "streams": "0" }),
        ),
    ] {
        let (error, text, _) = session.call(tool, arguments);
        assert!(error, "{tool}: {text}");
    }
    // Ids are strings or integers.
    session.send(&json!({ "jsonrpc": "2.0", "id": null, "method": "ping" }));
    assert_eq!(session.receive()["error"]["code"], -32600);
    session.send(&json!({ "jsonrpc": "2.0", "id": "a", "method": "ping" }));
    assert_eq!(session.receive()["id"], "a");
    // A response from the client gets no answer.
    session.send(&json!({ "jsonrpc": "2.0", "id": 99, "result": {} }));
    // A notification gets no answer: the next line answers the ping.
    session.send(&json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": {} }));
    assert_eq!(session.request("ping", json!({}))["result"], json!({}));
    session.finish();
}

#[test]
fn a_root_must_be_a_directory() {
    let root = directory("mcp_bad_root");
    let file = root.join("file");
    std::fs::write(&file, b"x").unwrap();
    for bad in [file, root.join("missing")] {
        let out = Command::new(bin())
            .args(["mcp", "--root"])
            .arg(&bad)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
    }
}

#[test]
fn urls_follow_the_servers_rule() {
    let root = directory("mcp_urls");
    let data = av_movie_with(aac_entry(), false);
    let server = http::Server::start(data, http::Mode::Ranges);
    let url = server.url();
    let https = url.replacen("http://", "https://", 1);
    // The default: public https:// URLs only.
    let mut session = Session::start(&[&root]);
    let (error, text, _) = session.call("probe", json!({ "path": url }));
    assert!(error && text.contains("only https://"), "{text}");
    let (error, text, _) = session.call("probe", json!({ "path": https }));
    assert!(error && text.contains("not a public address"), "{text}");
    // Outputs and policies are never URLs.
    std::fs::write(root.join("in.mp4"), av_movie_with(aac_entry(), false)).unwrap();
    let (error, _, _) = session.call("remux", json!({ "path": "in.mp4", "output": https }));
    assert!(error);
    session.finish();
    assert_eq!(server.requests.load(std::sync::atomic::Ordering::SeqCst), 0);
    // `--urls none`: local files only.
    let mut session = Session::start_with(&[&root], &["--urls", "none"]);
    let (error, text, _) = session.call("probe", json!({ "path": url }));
    assert!(error && text.contains("local files only"), "{text}");
    session.finish();
    // `--urls any`: as the command line, a local server included.
    let mut session = Session::start_with(&[&root], &["--urls", "any"]);
    let (error, text, document) = session.call("probe", json!({ "path": url }));
    assert!(!error, "{text}");
    assert_eq!(document["schema"], "coreav.probe/1");
    let (error, text, summary) =
        session.call("remux", json!({ "path": url, "output": "from-url.mp4" }));
    assert!(!error, "{text}");
    assert_eq!(summary["schema"], "coreav.remux/1");
    assert!(root.join("from-url.mp4").is_file());
    session.finish();
}
