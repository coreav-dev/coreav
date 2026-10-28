//! `coreav mcp`: a Model Context Protocol server over standard input and
//! output that gives an agent the commands as tools (`docs/design/mcp.md`).
//!
//! The server speaks JSON-RPC 2.0, one message per line (the protocol's
//! stdio transport), and answers `initialize`, `ping`, `tools/list` and
//! `tools/call`. Each tool call runs the `coreav` command itself as a child
//! process on local files, so a tool returns exactly what the command
//! prints, and a call that runs too long or prints too much is stopped
//! without affecting the server. Paths must lie inside the server's roots;
//! outputs are always files, never standard output.

use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Protocol versions the server speaks, newest first. A client asking for
/// another gets the newest.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Longest message the server reads, in bytes; a longer line is answered
/// with a parse error and skipped.
pub const MAX_MESSAGE_BYTES: usize = 1 << 20;

/// Most bytes a tool call may print, on each of standard output and
/// standard error; a call that prints more fails.
pub const MAX_TOOL_OUTPUT_BYTES: usize = 1 << 20;

/// Most cut times `split` takes in `at`.
pub const MAX_SPLIT_TIMES: usize = 10_000;

/// How long a tool call may run by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// Which URLs the tools read (`docs/design/remote.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urls {
    /// None: local files only.
    None,
    /// `https://` URLs of public addresses (the default).
    Public,
    /// Any URL the command line reads, private addresses and `http://`
    /// included.
    Any,
}

/// How the server runs.
pub struct Config {
    /// Directories the tools may read and write in. Paths are resolved,
    /// links included, before they are checked.
    pub roots: Vec<PathBuf>,
    /// The `coreav` command the tool calls run.
    pub command: PathBuf,
    /// How long one tool call may run.
    pub timeout: Duration,
    /// Which URLs the tools read.
    pub urls: Urls,
}

/// The server's state between messages.
pub struct Server {
    roots: Vec<PathBuf>,
    command: PathBuf,
    timeout: Duration,
    urls: Urls,
}

/// A tool call's failure, returned to the agent as an error result.
struct ToolError(String);

impl From<String> for ToolError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for ToolError {
    fn from(message: &str) -> Self {
        Self(message.to_owned())
    }
}

impl Server {
    /// A server for `config`; fails when a root is not a directory.
    pub fn new(config: Config) -> Result<Self, String> {
        if config.roots.is_empty() {
            return Err("at least one root directory is needed".to_owned());
        }
        let mut roots = Vec::new();
        for root in &config.roots {
            let resolved =
                std::fs::canonicalize(root).map_err(|e| format!("root {}: {e}", root.display()))?;
            if !resolved.is_dir() {
                return Err(format!("root {} is not a directory", root.display()));
            }
            roots.push(resolved);
        }
        Ok(Self {
            roots,
            command: config.command,
            timeout: config.timeout,
            urls: config.urls,
        })
    }

    /// The answer to one message, or None for a notification.
    pub fn handle(&mut self, message: &str) -> Option<Value> {
        let Ok(request) = serde_json::from_str::<Value>(message) else {
            return Some(error_response(&Value::Null, -32700, "parse error"));
        };
        let Some(request) = request.as_object() else {
            // Batches are not part of the protocol since 2025-06-18.
            return Some(error_response(&Value::Null, -32600, "invalid request"));
        };
        let method = request.get("method").and_then(Value::as_str);
        let Some(id) = request.get("id") else {
            return None; // A notification: `notifications/initialized` and others.
        };
        // A response from the client (the server sends no requests) gets no
        // answer.
        if method.is_none() && (request.contains_key("result") || request.contains_key("error")) {
            return None;
        }
        // The protocol's ids are strings or integers, never null.
        if !(id.is_string() || id.is_i64() || id.is_u64()) {
            return Some(error_response(&Value::Null, -32600, "invalid request id"));
        }
        let Some(method) = method else {
            return Some(error_response(id, -32600, "invalid request"));
        };
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => self.call(&params),
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error_response(id, code, &message),
        })
    }

    fn call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let empty = Map::new();
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(arguments)) => arguments,
            Some(_) => return Err((-32602, "arguments must be an object".to_owned())),
        };
        if !TOOL_NAMES.contains(&name) {
            return Err((-32602, format!("unknown tool: {name}")));
        }
        let outcome = self.run_tool(name, arguments);
        Ok(match outcome {
            Ok(document) => json!({
                "content": [{ "type": "text", "text": document.to_string() }],
                "structuredContent": document,
                "isError": false,
            }),
            Err(ToolError(message)) => json!({
                "content": [{ "type": "text", "text": message }],
                "isError": true,
            }),
        })
    }

    fn run_tool(&self, name: &str, arguments: &Map<String, Value>) -> Result<Value, ToolError> {
        let args = Arguments(arguments);
        let mut command: Vec<OsString> = Vec::new();
        let mut policy_file = None;
        match name {
            "probe" => {
                args.only(&["path", "allow", "strict"])?;
                command.push("probe".into());
                allow_options(&args, &mut command)?;
                command.push(self.media(args.string("path")?)?.into());
            }
            "chapters" => {
                args.only(&["path"])?;
                command.push("chapters".into());
                command.push(self.media(args.string("path")?)?.into());
            }
            "check" => {
                args.only(&["path", "policy", "policy_path"])?;
                let policy = match (args.0.get("policy"), args.0.get("policy_path")) {
                    (Some(policy @ Value::Object(_)), None) => {
                        let file = PolicyFile::write(policy)?;
                        let path = file.0.clone();
                        policy_file = Some(file);
                        path
                    }
                    (None, Some(Value::String(path))) => self.input(path)?,
                    _ => {
                        return Err(
                            "give `policy` (a coreav.policy/1 object) or `policy_path`, not both"
                                .into(),
                        );
                    }
                };
                command.push("check".into());
                command.push(flag("--policy", policy.as_os_str()));
                command.push(self.media(args.string("path")?)?.into());
            }
            "clip" => {
                args.only(&[
                    "path",
                    "output",
                    "start",
                    "end",
                    "video",
                    "audio",
                    "allow",
                    "strict",
                    "overwrite",
                ])?;
                command.push("clip".into());
                command.push(flag("--start", time(args.0.get("start"), "start")?));
                if let Some(end) = args.0.get("end") {
                    command.push(flag("--end", time(Some(end), "end")?));
                }
                stream_options(&args, &mut command)?;
                allow_options(&args, &mut command)?;
                self.output_options(&args, &mut command)?;
            }
            "split" => {
                args.only(&[
                    "path",
                    "output",
                    "every",
                    "at",
                    "video",
                    "audio",
                    "allow",
                    "strict",
                    "overwrite",
                ])?;
                command.push("split".into());
                match (args.0.get("every"), args.0.get("at")) {
                    (Some(every), None) => {
                        command.push(flag("--every", time(Some(every), "every")?))
                    }
                    (None, Some(Value::Array(times))) if times.len() > MAX_SPLIT_TIMES => {
                        return Err(format!("`at` may list at most {MAX_SPLIT_TIMES} times").into());
                    }
                    (None, Some(Value::Array(times))) if !times.is_empty() => {
                        let times = times
                            .iter()
                            .map(|t| match t {
                                // One time per item: commas would add uncounted times.
                                Value::String(text) if text.contains(',') => {
                                    Err("`at` lists one time per item".into())
                                }
                                _ => time(Some(t), "at"),
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        command.push(flag("--at", times.join(",")));
                    }
                    _ => {
                        return Err(
                            "give `every` (seconds) or `at` (a list of seconds), not both".into(),
                        );
                    }
                }
                stream_options(&args, &mut command)?;
                allow_options(&args, &mut command)?;
                // The command expands the pattern anywhere in the path: a
                // `%` in the directory would name directories the check on
                // the output never saw.
                let output = self.output(args.string("output")?)?;
                if output
                    .parent()
                    .is_some_and(|d| d.to_string_lossy().contains('%'))
                {
                    return Err("the segment number (%d) belongs in the file name; the directory may not contain %".into());
                }
                self.output_options(&args, &mut command)?;
            }
            "remux" | "sanitize" => {
                let mut known = vec![
                    "path",
                    "output",
                    "streams",
                    "fail_on_unsupported",
                    "allow",
                    "strict",
                    "overwrite",
                ];
                known.push(if name == "remux" {
                    "drop_unsupported"
                } else {
                    "keep"
                });
                args.only(&known)?;
                command.push(name.into());
                if let Some(streams) = args.indexes("streams")? {
                    let list: Vec<String> = streams.iter().map(ToString::to_string).collect();
                    command.push(flag("--streams", list.join(",")));
                }
                if args.boolean("fail_on_unsupported")? {
                    command.push("--fail-on-unsupported".into());
                }
                if args.boolean("drop_unsupported")? {
                    command.push("--drop-unsupported".into());
                }
                if let Some(keep) = args.strings("keep")? {
                    let keep = if keep.is_empty() {
                        "none".to_owned()
                    } else {
                        keep.join(",")
                    };
                    command.push(flag("--keep", &keep));
                }
                allow_options(&args, &mut command)?;
                self.output_options(&args, &mut command)?;
            }
            "extract_audio" | "extract_subtitles" => {
                if name == "extract_audio" {
                    args.only(&["path", "output", "stream", "allow", "strict", "overwrite"])?;
                    command.push("extract-audio".into());
                    allow_options(&args, &mut command)?;
                } else {
                    args.only(&["path", "output", "stream", "overwrite"])?;
                    command.push("extract-subtitles".into());
                }
                if let Some(stream) = args.index("stream")? {
                    command.push(flag("--stream", stream.to_string()));
                }
                self.output_options(&args, &mut command)?;
            }
            _ => return Err(format!("unknown tool: {name}").into()),
        }
        let result = self
            .run(&command, name == "check")
            .map_err(|ToolError(message)| {
                // The command's words for its options, in the tool's terms.
                let mut message =
                    message.replace("pass --force to overwrite it", "pass overwrite: true");
                if let Some(file) = &policy_file {
                    message = message.replace(&file.0.display().to_string(), "(policy)");
                }
                ToolError(message)
            });
        drop(policy_file);
        result
    }

    /// Adds `--force` when asked, then INPUT and OUTPUT.
    fn output_options(
        &self,
        args: &Arguments<'_>,
        command: &mut Vec<OsString>,
    ) -> Result<(), ToolError> {
        if args.boolean("overwrite")? {
            command.push("--force".into());
        }
        let input = self.media(args.string("path")?)?;
        let output = self.output(args.string("output")?)?;
        command.push(input.into());
        command.push(output.into());
        Ok(())
    }

    /// A media input: a URL the server reads (checked here, and on every
    /// connection by the command, which runs with `COREAV_REMOTE=public`),
    /// or a file as [`Self::input`] takes it.
    fn media(&self, raw: &str) -> Result<PathBuf, ToolError> {
        if !crate::remote_url(raw) {
            return self.input(raw);
        }
        match self.urls {
            Urls::None => Err("this server reads local files only, not URLs".into()),
            Urls::Public => crate::check_remote(raw, true)
                .map(|()| PathBuf::from(raw))
                .map_err(ToolError),
            Urls::Any => crate::check_remote(raw, false)
                .map(|()| PathBuf::from(raw))
                .map_err(ToolError),
        }
    }

    /// `raw` resolved against the first root, refused unless it is a local
    /// path naming an existing file inside a root.
    fn input(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let path = self.local(raw)?;
        let Ok(resolved) = std::fs::canonicalize(&path) else {
            // Whether a path outside the roots exists is not told: the
            // check is on the nearest directory that exists.
            self.nearest_inside(&path, raw)?;
            return Err(format!("{raw} does not exist").into());
        };
        self.inside(&resolved, raw)?;
        if !resolved.is_file() {
            return Err(format!("{raw} is not a file").into());
        }
        Ok(resolved)
    }

    /// `raw` resolved against the first root: its directory must exist
    /// inside a root, and an existing file there must not be a link.
    fn output(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let path = self.local(raw)?;
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(format!("{raw} does not name a file").into());
        };
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        let Ok(directory) = std::fs::canonicalize(parent) else {
            self.nearest_inside(parent, raw)?;
            return Err(format!("the directory of {raw} does not exist").into());
        };
        self.inside(&directory, raw)?;
        let resolved = directory.join(name);
        if std::fs::symlink_metadata(&resolved).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("{raw} is a link; write the output to another path").into());
        }
        Ok(resolved)
    }

    fn local(&self, raw: &str) -> Result<PathBuf, ToolError> {
        if raw.is_empty() {
            return Err("an empty path".into());
        }
        if raw.contains("://") {
            return Err("a URL is not accepted here: outputs and policies are local files".into());
        }
        let path = Path::new(raw);
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }
        let root = self.roots.first().ok_or("no root directory")?;
        Ok(root.join(path))
    }

    /// The check of [`Self::inside`] on the nearest existing directory
    /// above `path`, for a path that does not exist.
    fn nearest_inside(&self, path: &Path, raw: &str) -> Result<(), ToolError> {
        match path
            .ancestors()
            .skip(1)
            .find_map(|a| std::fs::canonicalize(a).ok())
        {
            Some(existing) => self.inside(&existing, raw),
            None => Ok(()),
        }
    }

    fn inside(&self, resolved: &Path, raw: &str) -> Result<(), ToolError> {
        if self.roots.iter().any(|root| resolved.starts_with(root)) {
            return Ok(());
        }
        let roots: Vec<String> = self.roots.iter().map(|r| r.display().to_string()).collect();
        Err(format!(
            "{raw} is outside the directories this server may use ({})",
            roots.join(", ")
        )
        .into())
    }

    /// Runs the command with `args` and returns the document it printed. A
    /// `check` that rejects the file still printed its verdict (exit 1).
    fn run(&self, args: &[OsString], check: bool) -> Result<Value, ToolError> {
        let mut command = Command::new(&self.command);
        if self.urls == Urls::Public {
            command.env("COREAV_REMOTE", "public");
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn(&mut command)
            .map_err(|e| format!("cannot run {}: {e}", self.command.display()))?;
        let stdout = child.stdout.take().map(capture);
        let stderr = child.stderr.take().map(capture);
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() < self.timeout => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let stopped = || {
            ToolError(format!(
                "stopped after {} seconds, the server's limit",
                self.timeout.as_secs()
            ))
        };
        // A stopped call's output is not needed; its readers are left to
        // end when the pipes close (a process the command started could
        // hold them open).
        let Some(status) = status else {
            return Err(stopped());
        };
        // The readers end when every writer has closed the pipes, which is
        // waited for only until the call's time is up.
        while [&stdout, &stderr]
            .iter()
            .any(|t| t.as_ref().is_some_and(|t| !t.is_finished()))
        {
            if started.elapsed() >= self.timeout {
                return Err(stopped());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let stdout = stdout.and_then(|t| t.join().ok()).unwrap_or_default();
        let stderr = stderr.and_then(|t| t.join().ok()).unwrap_or_default();
        let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
            return Err(format!("printed more than {MAX_TOOL_OUTPUT_BYTES} bytes").into());
        };
        let code = status.code();
        if code == Some(0) || (check && code == Some(1)) {
            return serde_json::from_slice(&stdout)
                .map_err(|e| format!("the command printed no JSON document: {e}").into());
        }
        let message = String::from_utf8_lossy(&stderr).trim().to_owned();
        Err(if message.is_empty() {
            format!("the command failed ({status})").into()
        } else {
            message.into()
        })
    }
}

/// Starts the command, trying again while the kernel says its file is busy
/// (ETXTBSY). A file that a thread of this process wrote a moment ago is
/// still open for writing in a child another thread forked in that moment,
/// until that child runs its own program; executing the file in that time
/// fails, and a moment later it does not. It is a property of `fork`, not of
/// the file, so waiting is the fix.
fn spawn(command: &mut Command) -> std::io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 200 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            other => return other,
        }
    }
}

/// Reads a child's stream to the end on a thread: Some(bytes), or None
/// when it printed more than [`MAX_TOOL_OUTPUT_BYTES`] (the rest is read
/// and dropped, so the child never blocks on a full pipe).
fn capture<R: Read + Send + 'static>(mut stream: R) -> std::thread::JoinHandle<Option<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let limit = u64::try_from(MAX_TOOL_OUTPUT_BYTES).unwrap_or(u64::MAX);
        let read = stream
            .by_ref()
            .take(limit.saturating_add(1))
            .read_to_end(&mut kept);
        let _ = std::io::copy(&mut stream, &mut std::io::sink());
        match read {
            Ok(_) if kept.len() <= MAX_TOOL_OUTPUT_BYTES => Some(kept),
            _ => None,
        }
    })
}

/// A policy given inline, written to a new file for `coreav check` and
/// removed when dropped.
struct PolicyFile(PathBuf);

impl PolicyFile {
    fn write(policy: &Value) -> Result<Self, ToolError> {
        let mut taken = None;
        // A random name, created exclusively and readable only by its
        // owner; another name if one is taken.
        for _ in 0..8 {
            let path = std::env::temp_dir().join(format!(
                "coreav-mcp-{}-{:016x}.policy.json",
                std::process::id(),
                random()
            ));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            match options.open(&path) {
                Ok(mut file) => {
                    let written = file.write_all(policy.to_string().as_bytes());
                    let file = Self(path);
                    written.map_err(|e| format!("cannot write the policy: {e}"))?;
                    return Ok(file);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    taken = Some(error);
                }
                Err(error) => return Err(format!("cannot write the policy: {error}").into()),
            }
        }
        Err(format!(
            "cannot write the policy: {}",
            taken.map_or_else(String::new, |e| e.to_string())
        )
        .into())
    }
}

/// A random number, from the standard library's randomly keyed hasher.
fn random() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    hasher.finish()
}

impl Drop for PolicyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A tool call's arguments, with typed accessors that name the argument in
/// their errors.
struct Arguments<'a>(&'a Map<String, Value>);

impl Arguments<'_> {
    fn only(&self, known: &[&str]) -> Result<(), ToolError> {
        match self.0.keys().find(|k| !known.contains(&k.as_str())) {
            Some(key) => Err(format!("unknown argument `{key}`").into()),
            None => Ok(()),
        }
    }

    fn string(&self, name: &str) -> Result<&str, ToolError> {
        match self.0.get(name) {
            Some(Value::String(value)) => Ok(value),
            Some(_) => Err(format!("`{name}` must be a string").into()),
            None => Err(format!("`{name}` is required").into()),
        }
    }

    fn boolean(&self, name: &str) -> Result<bool, ToolError> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(false),
            Some(Value::Bool(value)) => Ok(*value),
            Some(_) => Err(format!("`{name}` must be true or false").into()),
        }
    }

    fn index(&self, name: &str) -> Result<Option<u64>, ToolError> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_u64()
                .map(Some)
                .ok_or_else(|| format!("`{name}` must be a stream index (0 or more)").into()),
        }
    }

    fn indexes(&self, name: &str) -> Result<Option<Vec<u64>>, ToolError> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) if !items.is_empty() => items
                .iter()
                .map(|v| {
                    v.as_u64()
                        .ok_or_else(|| format!("`{name}` must list stream indexes").into())
                })
                .collect::<Result<Vec<_>, ToolError>>()
                .map(Some),
            Some(_) => Err(format!("`{name}` must list stream indexes").into()),
        }
    }

    fn strings(&self, name: &str) -> Result<Option<Vec<String>>, ToolError> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| match v.as_str() {
                    Some(s) if !s.is_empty() && !s.contains(',') => Ok(s.to_owned()),
                    _ => Err(format!("`{name}` must list names").into()),
                })
                .collect::<Result<Vec<_>, ToolError>>()
                .map(Some),
            Some(_) => Err(format!("`{name}` must list names").into()),
        }
    }
}

/// `--video` and `--audio` (a stream index or `none`).
fn stream_options(args: &Arguments<'_>, command: &mut Vec<OsString>) -> Result<(), ToolError> {
    if let Some(video) = args.index("video")? {
        command.push(flag("--video", video.to_string()));
    }
    match args.0.get("audio") {
        None | Some(Value::Null) => {}
        Some(Value::String(none)) if none == "none" => command.push(flag("--audio", "none")),
        Some(_) => {
            let audio = args
                .index("audio")?
                .ok_or("`audio` must be a stream index or \"none\"")?;
            command.push(flag("--audio", audio.to_string()));
        }
    }
    Ok(())
}

/// `--allow` and `--strict`.
fn allow_options(args: &Arguments<'_>, command: &mut Vec<OsString>) -> Result<(), ToolError> {
    if let Some(allow) = args.strings("allow")? {
        if allow.is_empty() {
            return Err("`allow` must name at least one codec".into());
        }
        command.push(flag("--allow", allow.join(",")));
    }
    if args.boolean("strict")? {
        command.push("--strict".into());
    }
    Ok(())
}

/// `--name=value`, so that a value starting with `-` is never read as an
/// option.
fn flag(name: &str, value: impl AsRef<std::ffi::OsStr>) -> OsString {
    let mut flag = OsString::from(name);
    flag.push("=");
    flag.push(value);
    flag
}

/// A time in decimal seconds, from a string or a number.
fn time(value: Option<&Value>, name: &str) -> Result<String, ToolError> {
    match value {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(Value::Number(number)) => {
            if let Some(whole) = number.as_u64() {
                return Ok(whole.to_string());
            }
            match number.as_f64() {
                Some(seconds) if seconds >= 0.0 && seconds.is_finite() => {
                    let text = format!("{seconds:.9}");
                    Ok(text.trim_end_matches('0').trim_end_matches('.').to_owned())
                }
                _ => Err(format!("`{name}` must be 0 or more seconds").into()),
            }
        }
        None => Err(format!("`{name}` is required").into()),
        Some(_) => Err(format!("`{name}` must be seconds, as a number or a string").into()),
    }
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|v| Some(**v) == asked)
        .or_else(|| PROTOCOL_VERSIONS.first())
        .copied()
        .unwrap_or_default();
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "coreav", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

const INSTRUCTIONS: &str = "Memory-safe tools for MP4, MOV, M4A, Matroska, WebM and MPEG transport stream files on \
local paths, without decoding: describe a file (probe), check an upload against a policy \
(check), cut a keyframe-aligned clip (clip) or split into segments (split), rewrite as MP4 with the index first (remux), strip \
metadata (sanitize), copy out the AAC audio (extract_audio), the subtitles as WebVTT \
(extract_subtitles) or the chapter list (chapters). They never decode or re-encode: use ffmpeg \
to transcode, scale, filter, change codecs, or read frames.";

const TOOL_NAMES: [&str; 9] = [
    "probe",
    "check",
    "clip",
    "split",
    "remux",
    "sanitize",
    "extract_audio",
    "extract_subtitles",
    "chapters",
];

/// The tools, with their input schemas (JSON Schema).
fn tools() -> Value {
    let path = json!({ "type": "string", "description": "The media file: a local path, absolute or relative to the server's first root, or an https:// URL (a presigned object-storage URL works), read by byte ranges without downloading the whole file." });
    let output = json!({ "type": "string", "description": "Where to write the result: a local file path inside the server's roots. An existing file is kept unless `overwrite` is true; the input is never overwritten." });
    let overwrite = json!({ "type": "boolean", "description": "Replace an existing output file." });
    let allow = json!({ "type": "array", "items": { "type": "string" }, "description": "Codecs to accept (for example [\"h264\", \"aac\"]); streams with other codecs are dropped, or the file is rejected with `strict`." });
    let strict = json!({ "type": "boolean", "description": "Reject the whole file when a stream's codec is not allowed." });
    let stream = json!({ "type": "integer", "minimum": 0, "description": "The stream index, as `probe` numbers streams." });
    let seconds = json!({ "type": ["number", "string"], "description": "Seconds, such as 12.5 or \"12.5\"." });
    json!([
        {
            "name": "probe",
            "title": "Describe a media file",
            "description": "Describe an MP4, MOV, M4A, Matroska, WebM or MPEG transport stream (.ts, .m2ts, .mts) file as JSON without decoding it: container, duration, every stream's codec, resolution, frame rate, sample rate, channels, language, rotation, bit rate. Safe on untrusted files: hostile or malformed input is rejected with a structured error, under memory and time limits. Use it instead of ffprobe for these files.",
            "inputSchema": { "type": "object", "properties": { "path": path, "allow": allow, "strict": strict }, "required": ["path"], "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        },
        {
            "name": "check",
            "title": "Check an upload against a policy",
            "description": "Check an MP4, MOV, M4A, Matroska or WebM file against an upload policy (coreav.policy/1: allowed containers and codecs, limits on file size, bit rate, duration, resolution, channels, sample rate and number of streams) and return an accept or reject verdict with every reason. Resolution is checked against the size a decoder would produce, not only the declared one. For validating untrusted uploads before storing or processing them.",
            "inputSchema": { "type": "object", "properties": {
                "path": path,
                "policy": { "type": "object", "description": "A coreav.policy/1 object (unknown keys are errors; a stream kind without its own object under `streams` is not allowed), such as {\"schema\": \"coreav.policy/1\", \"container\": {\"formats\": [\"mp4\", \"mov\"]}, \"duration\": {\"max_seconds\": 600}, \"streams\": {\"video\": {\"codecs\": [\"h264\", \"hevc\"], \"max_count\": 1, \"max_width\": 1920, \"max_height\": 1920}, \"audio\": {\"codecs\": [\"aac\"], \"max_count\": 1}}}." },
                "policy_path": { "type": "string", "description": "A coreav.policy/1 JSON file inside the server's roots, instead of `policy`." }
            }, "required": ["path"], "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        },
        {
            "name": "clip",
            "title": "Cut a clip without re-encoding",
            "description": "Cut a time range of an MP4/MOV file into a new MP4 file by stream copy, without re-encoding: the clip starts at the latest keyframe at or before `start`. Fast and lossless; for a frame-exact cut or another codec, use ffmpeg.",
            "inputSchema": { "type": "object", "properties": { "path": path, "output": output, "start": seconds, "end": seconds, "video": stream, "audio": { "type": ["integer", "string"], "description": "The audio stream index, or \"none\" for no audio (default: the first audio stream)." }, "allow": allow, "strict": strict, "overwrite": overwrite }, "required": ["path", "output", "start"], "additionalProperties": false },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false },
        },
        {
            "name": "split",
            "title": "Split into segments without re-encoding",
            "description": "Split an MP4/MOV file into MP4 segments at keyframes, by stream copy without re-encoding: every `every` seconds or at the `at` times, each cut at the latest keyframe at or before it. Together the segments hold every packet exactly once. `output` names the segments with %d or %03d for the segment number, such as \"parts/seg-%03d.mp4\".",
            "inputSchema": { "type": "object", "properties": { "path": path, "output": { "type": "string", "description": "The segments' path with %d, or %0Nd for N digits, replaced by the segment number from 0; inside the server's roots." }, "every": seconds, "at": { "type": "array", "items": { "type": ["number", "string"] }, "description": "Cut times in increasing seconds." }, "video": stream, "audio": { "type": ["integer", "string"], "description": "The audio stream index, or \"none\" for no audio (default: the first audio stream)." }, "allow": allow, "strict": strict, "overwrite": { "type": "boolean", "description": "Replace segment files that exist." } }, "required": ["path", "output"], "additionalProperties": false },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false },
        },
        {
            "name": "remux",
            "title": "Rewrite as streamable MP4",
            "description": "Rewrite an MP4, MOV, Matroska or WebM file as MP4 with the index first (faststart, for streaming), by stream copy without re-encoding. Streams MP4 cannot carry are dropped and listed (or fail with `fail_on_unsupported`). Use ffmpeg instead to change codecs.",
            "inputSchema": { "type": "object", "properties": { "path": path, "output": output, "streams": { "type": "array", "items": { "type": "integer", "minimum": 0 }, "description": "Stream indexes to keep (default: every stream MP4 can carry)." }, "fail_on_unsupported": { "type": "boolean", "description": "Fail instead of dropping a stream MP4 cannot carry." }, "drop_unsupported": { "type": "boolean", "description": "Matroska and WebM: also drop a video stream MP4 cannot carry (VP8) instead of failing." }, "allow": allow, "strict": strict, "overwrite": overwrite }, "required": ["path", "output"], "additionalProperties": false },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false },
        },
        {
            "name": "sanitize",
            "title": "Strip metadata",
            "description": "Rewrite an MP4/MOV file as MP4 keeping only allowlisted boxes: removes location, device, creation dates and other metadata, by stream copy without re-encoding. `keep` chooses what to keep: rotation, language, dates, gapless, kinds (default: rotation, language, gapless, kinds; an empty list keeps none).",
            "inputSchema": { "type": "object", "properties": { "path": path, "output": output, "keep": { "type": "array", "items": { "type": "string", "enum": ["rotation", "language", "dates", "gapless", "kinds"] } }, "streams": { "type": "array", "items": { "type": "integer", "minimum": 0 } }, "fail_on_unsupported": { "type": "boolean" }, "allow": allow, "strict": strict, "overwrite": overwrite }, "required": ["path", "output"], "additionalProperties": false },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false },
        },
        {
            "name": "extract_audio",
            "title": "Copy out the AAC audio",
            "description": "Copy the AAC audio track of an MP4/MOV file into an M4A file without re-encoding. Other codecs (MP3, Opus, PCM) are refused: use ffmpeg to convert them.",
            "inputSchema": { "type": "object", "properties": { "path": path, "output": output, "stream": stream, "allow": allow, "strict": strict, "overwrite": overwrite }, "required": ["path", "output"], "additionalProperties": false },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false },
        },
        {
            "name": "extract_subtitles",
            "title": "Write the subtitles as WebVTT",
            "description": "Write the text subtitles (mov_text: 3GPP timed text or QuickTime text) of an MP4/MOV file as a WebVTT file.",
            "inputSchema": { "type": "object", "properties": { "path": path, "output": output, "stream": stream, "overwrite": overwrite }, "required": ["path", "output"], "additionalProperties": false },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false },
        },
        {
            "name": "chapters",
            "title": "List the chapters",
            "description": "List the chapters (titles and start and end times) of an MP4, MOV, M4A or M4B file, from its QuickTime chapter track.",
            "inputSchema": { "type": "object", "properties": { "path": path }, "required": ["path"], "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        },
    ])
}

/// Serves `input` until it ends: one message per line, each answer on its
/// own line of `output`, flushed. Fails only when `output` fails.
pub fn serve(
    server: &mut Server,
    mut input: impl BufRead,
    mut output: impl Write,
) -> std::io::Result<()> {
    let limit = u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX);
    loop {
        let mut line = Vec::new();
        let read = input
            .by_ref()
            .take(limit.saturating_add(1))
            .read_until(b'\n', &mut line);
        match read {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Ok(()),
        }
        let answer = if line.len() > MAX_MESSAGE_BYTES && line.last() != Some(&b'\n') {
            // Skip the rest of the long line.
            let mut rest = Vec::new();
            loop {
                rest.clear();
                match input.by_ref().take(limit).read_until(b'\n', &mut rest) {
                    Ok(0) => break,
                    Ok(_) if rest.last() == Some(&b'\n') => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            Some(error_response(
                &Value::Null,
                -32700,
                &format!("message longer than {MAX_MESSAGE_BYTES} bytes"),
            ))
        } else {
            match std::str::from_utf8(&line) {
                Ok(text) if text.trim().is_empty() => None,
                Ok(text) => server.handle(text.trim()),
                Err(_) => Some(error_response(&Value::Null, -32700, "parse error")),
            }
        };
        if let Some(answer) = answer {
            writeln!(output, "{answer}")?;
            output.flush()?;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A server whose "command" is a shell script, in a new root holding a
    /// file to name.
    fn server(name: &str, script: &str, timeout: Duration) -> Server {
        let root =
            std::env::temp_dir().join(format!("coreav-mcp-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("in.mp4"), b"x").unwrap();
        let command = root.join("command.sh");
        std::fs::write(&command, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755)).unwrap();
        Server::new(Config {
            roots: vec![root],
            command,
            timeout,
            urls: Urls::Public,
        })
        .unwrap()
    }

    fn cleanup(server: &Server) {
        for root in &server.roots {
            let _ = std::fs::remove_dir_all(root);
        }
    }

    fn probe(server: &mut Server) -> Value {
        let message = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                              "params": { "name": "probe", "arguments": { "path": "in.mp4" } } });
        server.handle(&message.to_string()).unwrap()["result"].clone()
    }

    #[test]
    fn a_call_over_its_time_is_stopped() {
        let mut server = server("slow", "sleep 30", Duration::from_secs(1));
        let started = Instant::now();
        let result = probe(&mut server);
        cleanup(&server);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(result["isError"], true);
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("stopped after 1 seconds")
        );
    }

    /// The failure behind a flaky `a_call_that_prints_too_much_fails`: another
    /// test's fork held the script open for writing at the moment it was run.
    /// Here the file is held open on purpose, then released.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_command_file_busy_for_writing_is_waited_for() {
        let path = std::env::temp_dir().join(format!("coreav-mcp-busy-{}.sh", std::process::id()));
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let held = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let plain = Command::new(&path).status();
        assert_eq!(
            plain.unwrap_err().kind(),
            std::io::ErrorKind::ExecutableFileBusy
        );
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(held);
        });
        let mut command = Command::new(&path);
        let status = spawn(&mut command).unwrap().wait().unwrap();
        release.join().unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(status.success());
    }

    #[test]
    fn a_call_that_prints_too_much_fails() {
        let mut server = server("flood", "head -c 3000000 /dev/zero", DEFAULT_TIMEOUT);
        let result = probe(&mut server);
        cleanup(&server);
        assert_eq!(result["isError"], true);
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("printed more than")
        );
    }
}
