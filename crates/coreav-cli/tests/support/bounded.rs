use std::io::{self, Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

pub fn output(
    command: &mut Command,
    input: Option<Vec<u8>>,
    limit: Duration,
) -> io::Result<Output> {
    let diagnostics = std::env::var_os("COREAV_TEST_DIAGNOSTICS").map(std::path::PathBuf::from);
    run(
        command,
        input,
        limit,
        Duration::from_secs(5),
        diagnostics.as_deref(),
    )
}

fn read(mut pipe: impl Read + Send + 'static) -> Receiver<io::Result<Vec<u8>>> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = pipe.read_to_end(&mut bytes).map(|_| bytes);
        let _ = send.send(result);
    });
    receive
}

fn receive<T>(channel: &Receiver<io::Result<T>>, slot: &mut Option<io::Result<T>>) {
    if slot.is_none() {
        match channel.try_recv() {
            Ok(value) => *slot = Some(value),
            Err(mpsc::TryRecvError::Disconnected) => {
                *slot = Some(Err(io::Error::other("subprocess I/O thread ended")));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
}

fn run(
    command: &mut Command,
    input: Option<Vec<u8>>,
    limit: Duration,
    grace: Duration,
    diagnostics: Option<&std::path::Path>,
) -> io::Result<Output> {
    let started = Instant::now();
    let deadline = started + limit;
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let pid = child.id();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    #[cfg(target_os = "linux")]
    let pipes = {
        use std::os::fd::AsRawFd;
        let mut pipes = Vec::new();
        for fd in [
            Some(stdout.as_raw_fd()),
            Some(stderr.as_raw_fd()),
            child.stdin.as_ref().map(AsRawFd::as_raw_fd),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(link) = std::fs::read_link(format!("/proc/self/fd/{fd}")) {
                pipes.push(link.to_string_lossy().into_owned());
            }
        }
        pipes
    };
    #[cfg(not(target_os = "linux"))]
    let pipes = Vec::new();
    let out = read(stdout);
    let err = read(stderr);
    let (send, written) = mpsc::channel();
    if let Some(bytes) = input {
        let mut stdin = child.stdin.take().unwrap();
        std::thread::spawn(move || {
            let result = stdin.write_all(&bytes);
            drop(stdin);
            let _ = send.send(result);
        });
    } else {
        let _ = send.send(Ok(()));
    }
    let (mut stdout, mut stderr, mut stdin) = (None, None, None);
    let mut status = None;
    #[cfg(target_os = "linux")]
    let mut exited = None;
    #[cfg(not(target_os = "linux"))]
    let exited: Option<Instant> = None;
    loop {
        receive(&out, &mut stdout);
        receive(&err, &mut stderr);
        receive(&written, &mut stdin);
        let complete = stdout.is_some() && stderr.is_some() && stdin.is_some();
        #[cfg(target_os = "linux")]
        if exited.is_none() && coreav_worker::exec::has_exited(&mut child) {
            exited = Some(Instant::now());
            coreav_worker::exec::kill_group(&child);
        }
        #[cfg(target_os = "linux")]
        let can_reap = exited.is_some();
        #[cfg(not(target_os = "linux"))]
        let can_reap = complete;
        if status.is_none() && can_reap {
            status = child.try_wait()?;
        }
        if complete && let Some(status) = status {
            stdin.unwrap()?;
            return Ok(Output {
                status,
                stdout: stdout.unwrap()?,
                stderr: stderr.unwrap()?,
            });
        }
        let now = Instant::now();
        let pipes_expired = exited.is_some_and(|at: Instant| now.duration_since(at) >= grace);
        if now >= deadline || pipes_expired {
            let reason = if exited.is_some() {
                "pipes remained open after exit"
            } else {
                "command deadline exceeded"
            };
            let snapshot = snapshot(pid, &pipes, started, limit, diagnostics);
            if status.is_none() {
                coreav_worker::exec::kill_group(&child);
                let _ = child.kill();
                let reap_until = Instant::now() + Duration::from_secs(5);
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) | Err(_) => break,
                        Ok(None) if Instant::now() >= reap_until => break,
                        Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    }
                }
            }
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{reason}; pid={pid}; stdin_done={} stdout_done={} stderr_done={}\n{snapshot}",
                    stdin.is_some(),
                    stdout.is_some(),
                    stderr.is_some()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(not(target_os = "linux"))]
fn snapshot(
    pid: u32,
    _: &[String],
    _: Instant,
    _: Duration,
    _: Option<&std::path::Path>,
) -> String {
    format!("pid={pid}; detailed process, thread and pipe state requires Linux /proc")
}

#[cfg(target_os = "linux")]
fn snapshot(
    root: u32,
    pipes: &[String],
    started: Instant,
    limit: Duration,
    diagnostics: Option<&std::path::Path>,
) -> String {
    let until = Instant::now() + Duration::from_secs(2);
    let pipes = pipes.to_vec();
    let diagnostics = diagnostics.map(std::path::Path::to_path_buf);
    snapshot_until(until, move || {
        let mut text = collect_snapshot(root, &pipes, started, limit, until);
        if let Some(directory) = diagnostics
            && let Err(error) = write_snapshot(&directory, root, &text)
        {
            use std::fmt::Write;
            truncate_snapshot(&mut text, 32 * 1024 - 128);
            let _ = writeln!(text, "diagnostics not written: {:?}", error.kind());
        }
        truncate_snapshot(&mut text, 32 * 1024);
        text
    })
}

#[cfg(target_os = "linux")]
fn snapshot_until(until: Instant, collect: impl FnOnce() -> String + Send + 'static) -> String {
    let (send, receive) = mpsc::channel();
    if let Err(error) = std::thread::Builder::new()
        .name("coreav-test-snapshot".into())
        .spawn(move || {
            let _ = send.send(collect());
        })
    {
        return format!("snapshot unavailable: {:?}", error.kind());
    }
    loop {
        match receive.try_recv() {
            Ok(text) => return text,
            Err(mpsc::TryRecvError::Disconnected) => return "snapshot collector ended".into(),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return "snapshot did not finish within its budget".into();
        }
        std::thread::sleep(remaining.min(Duration::from_millis(20)));
    }
}

#[cfg(target_os = "linux")]
fn write_snapshot(directory: &std::path::Path, root: u32, text: &str) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::create_dir_all(directory)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join(format!("pid-{root}.txt")))?;
    file.write_all(snapshot_prefix(text, 32 * 1024).as_bytes())
}

#[cfg(target_os = "linux")]
fn truncate_snapshot(text: &mut String, limit: usize) {
    let end = snapshot_prefix(text, limit).len();
    text.truncate(end);
}

#[cfg(target_os = "linux")]
fn snapshot_prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(target_os = "linux")]
fn collect_snapshot(
    root: u32,
    pipes: &[String],
    started: Instant,
    limit: Duration,
    until: Instant,
) -> String {
    use std::collections::HashSet;
    use std::fmt::Write;
    use std::path::Path;

    let mut text = format!(
        "process snapshot; root={root}; elapsed_ms={}; limit_ms={}; tracked_pipes={pipes:?}\n",
        started.elapsed().as_millis(),
        limit.as_millis()
    );
    let root_exe = std::fs::read_link(format!("/proc/{root}/exe")).ok();
    let mut processes = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten().take(4096) {
            if Instant::now() >= until {
                break;
            }
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if let Ok(stat) = std::fs::read_to_string(entry.path().join("stat"))
                && let Some((_, tail)) = stat.rsplit_once(") ")
                && let Some(parent) = tail
                    .split_whitespace()
                    .nth(1)
                    .and_then(|p| p.parse::<u32>().ok())
            {
                processes.push((pid, parent, stat));
            }
        }
    }
    let mut family = HashSet::from([root]);
    loop {
        let before = family.len();
        for (pid, parent, _) in &processes {
            if Instant::now() >= until {
                break;
            }
            if family.contains(parent) {
                family.insert(*pid);
            }
        }
        if family.len() == before || Instant::now() >= until {
            break;
        }
    }
    processes.sort_by_key(|(pid, _, _)| (*pid != root, !family.contains(pid), *pid));
    let mut shown = 0;
    for (pid, _, stat) in processes {
        if Instant::now() >= until || text.len() >= 32 * 1024 || shown >= 64 {
            break;
        }
        let path = Path::new("/proc").join(pid.to_string());
        let mut descriptors = Vec::new();
        if let Ok(entries) = std::fs::read_dir(path.join("fd")) {
            for entry in entries.flatten().take(256) {
                if Instant::now() >= until {
                    break;
                }
                if let Ok(target) = std::fs::read_link(entry.path()) {
                    descriptors.push((entry.file_name(), target.to_string_lossy().into_owned()));
                }
            }
        }
        let holds_pipe = descriptors.iter().any(|(_, target)| pipes.contains(target));
        if !family.contains(&pid) && !holds_pipe {
            continue;
        }
        shown += 1;
        let same_exe = root_exe.as_ref().is_some_and(|root_exe| {
            std::fs::read_link(path.join("exe")).as_ref().ok() == Some(root_exe)
        });
        let _ = writeln!(
            text,
            "process pid={pid} stat={stat:?} holds_tracked_pipe={holds_pipe}"
        );
        for (fd, target) in descriptors.iter().take(64) {
            let _ = writeln!(text, "  fd {fd:?} -> {target:?}");
        }
        if let Ok(entries) = std::fs::read_dir(path.join("task")) {
            for entry in entries.flatten().take(32) {
                if Instant::now() >= until || text.len() >= 32 * 1024 {
                    break;
                }
                let stat = std::fs::read_to_string(entry.path().join("stat"))
                    .unwrap_or_else(|e| format!("? {e}"));
                let wchan = std::fs::read_to_string(entry.path().join("wchan"))
                    .unwrap_or_else(|e| format!("? {e}"));
                let _ = writeln!(
                    text,
                    "  thread {:?} stat={stat:?} wchan={wchan:?}",
                    entry.file_name()
                );
                if family.contains(&pid) {
                    let _ = writeln!(text, "    {}", thread_wait(pid, &entry.path(), same_exe));
                }
            }
        }
    }
    let _ = writeln!(
        text,
        "snapshot limits: 2s, 4096 candidates, 64 processes, 256 scanned/64 listed fds, 32 threads/process, 32KiB"
    );
    truncate_snapshot(&mut text, 32 * 1024);
    text
}

#[cfg(target_os = "linux")]
fn proc_text(path: &std::path::Path) -> io::Result<String> {
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(4096)
        .read_to_string(&mut text)?;
    Ok(text.trim().to_owned())
}

#[cfg(target_os = "linux")]
fn thread_wait(pid: u32, task: &std::path::Path, same_exe: bool) -> String {
    use std::fmt::Write;
    let slack = proc_text(&task.join("timerslack_ns"))
        .unwrap_or_else(|error| format!("unavailable: {:?}", error.kind()));
    let mut text = format!("timerslack_ns={slack:?}");
    let raw = match proc_text(&task.join("syscall")) {
        Ok(raw) => raw,
        Err(error) => {
            let _ = write!(text, " syscall=unavailable({:?})", error.kind());
            return text;
        }
    };
    let mut fields = raw.split_whitespace();
    let number = fields.next().and_then(|n| n.parse::<i64>().ok());
    let _ = write!(text, " syscall={number:?}");
    #[cfg(target_arch = "x86_64")]
    if number == Some(202)
        && let Some((operation, pointer)) = fields.next().and_then(|_| {
            let operation = syscall_word(fields.next()?)?;
            fields.next()?;
            Some((operation, syscall_word(fields.next()?)?))
        })
    {
        let _ = write!(
            text,
            " futex_op=0x{operation:x} timeout_set={}",
            pointer != 0
        );
        if same_exe && pointer != 0 && matches!(operation & 0x7f, 0 | 9) {
            match futex_expiry(pid, task, &raw, operation, pointer) {
                Ok(expiry) => text.push_str(&expiry),
                Err(error) => {
                    let _ = write!(text, " futex_error={:?}", error.kind());
                }
            }
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (pid, same_exe);
    text
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn syscall_word(word: &str) -> Option<u64> {
    match word.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => word.parse().ok(),
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn futex_expiry(
    pid: u32,
    task: &std::path::Path,
    raw: &str,
    operation: u64,
    pointer: u64,
) -> io::Result<String> {
    use rustix::time::{ClockId, clock_gettime};
    use std::os::unix::fs::FileExt;
    let mut bytes = [0; 16];
    std::fs::File::open(format!("/proc/{pid}/mem"))?.read_exact_at(&mut bytes, pointer)?;
    if proc_text(&task.join("syscall"))? != raw {
        return Ok(" stable=false".into());
    }
    let seconds = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
    let nanos = i64::from_ne_bytes(bytes[8..].try_into().unwrap());
    if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let realtime = operation & 0x100 != 0;
    let now = clock_gettime(if realtime {
        ClockId::Realtime
    } else {
        ClockId::Monotonic
    });
    let value = i128::from(seconds) * 1_000_000_000 + i128::from(nanos);
    let absolute = operation & 0x7f == 9;
    let remaining = if absolute {
        value - (i128::from(now.tv_sec) * 1_000_000_000 + i128::from(now.tv_nsec))
    } else {
        value
    };
    Ok(format!(
        " stable=true clock={} absolute={absolute} timespec={seconds}.{nanos:09} now={}.{:09} remaining_ms={}",
        if realtime { "REALTIME" } else { "MONOTONIC" },
        now.tv_sec,
        now.tv_nsec,
        remaining / 1_000_000
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn status_and_both_streams_survive() {
        let out = output(
            Command::new("/bin/sh").args(["-c", "printf out; printf err >&2; exit 3"]),
            None,
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
    }

    #[test]
    fn input_and_output_are_drained_concurrently() {
        let bytes = vec![b'x'; 4 * 1024 * 1024];
        let out = output(
            Command::new("/bin/sh").args(["-c", "cat"]),
            Some(bytes.clone()),
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, bytes);
    }

    #[test]
    fn a_running_child_and_blocked_input_have_a_deadline() {
        for input in [None, Some(vec![0; 4 * 1024 * 1024])] {
            let start = Instant::now();
            let err = output(
                Command::new("/bin/sh").args(["-c", "exec sleep 30"]),
                input,
                Duration::from_secs(1),
            )
            .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::TimedOut);
            assert!(err.to_string().contains("command deadline exceeded"));
            assert!(err.to_string().contains("pid="));
            assert!(start.elapsed() < Duration::from_secs(12), "{err}");
            #[cfg(target_os = "linux")]
            {
                assert!(err.to_string().contains("sleep"), "{err}");
                assert!(err.to_string().contains("thread"), "{err}");
                assert!(err.to_string().contains("fd"), "{err}");
            }
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    #[ignore = "subprocess target for the futex snapshot test"]
    fn timed_wait_target() {
        if std::env::var_os("COREAV_TEST_WAIT_TARGET").is_none() {
            return;
        }
        let (send, receive) = mpsc::channel::<()>();
        let _ = receive.recv_timeout(Duration::from_secs(60));
        drop(send);
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn a_timed_futex_records_its_expiry() {
        let err = output(
            Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", "bounded::tests::timed_wait_target"])
                .env("COREAV_TEST_WAIT_TARGET", "1"),
            None,
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        let text = err.to_string();
        let line = text
            .lines()
            .find(|line| {
                line.contains("futex_op=0x89")
                    && line.contains("stable=true")
                    && line.contains("clock=MONOTONIC")
                    && line.contains("absolute=true")
            })
            .unwrap_or_else(|| panic!("no stable timed futex: {text}"));
        let remaining = line
            .split_whitespace()
            .find_map(|field| field.strip_prefix("remaining_ms="))
            .unwrap()
            .parse::<i128>()
            .unwrap();
        assert!((1..=60_000).contains(&remaining), "{text}");
        assert!(line.contains("timerslack_ns="), "{text}");
        assert!(text.contains("limit_ms=1000"), "{text}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_timeout_writes_a_bounded_artifact() {
        let dir =
            std::env::temp_dir().join(format!("coreav-snapshot-artifact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let err = run(
            Command::new("/bin/sh")
                .args(["-c", "sleep 30", "diagnostic-argument-sentinel"])
                .env(
                    "COREAV_TEST_CAPTURE_SENTINEL",
                    "diagnostic-environment-sentinel",
                ),
            None,
            Duration::from_secs(1),
            Duration::from_secs(5),
            Some(&dir),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(files.len(), 1);
        let text = std::fs::read_to_string(files[0].as_ref().unwrap().path()).unwrap();
        assert!(text.len() <= 32 * 1024);
        assert!(text.contains("process snapshot"), "{text}");
        assert!(text.contains("syscall=Some("), "{text}");
        assert!(!text.contains("futex_op="), "{text}");
        assert!(!text.contains("COREAV_TEST_DIAGNOSTICS"));
        assert!(!text.contains("diagnostic-argument-sentinel"));
        assert!(!text.contains("diagnostic-environment-sentinel"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_artifact_failure_preserves_the_timeout() {
        let path =
            std::env::temp_dir().join(format!("coreav-snapshot-file-{}", std::process::id()));
        std::fs::write(&path, b"not a directory").unwrap();
        let err = run(
            Command::new("/bin/sh").args(["-c", "exec sleep 30"]),
            None,
            Duration::from_secs(1),
            Duration::from_secs(5),
            Some(&path),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(
            err.to_string().contains("diagnostics not written:"),
            "{err}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"not a directory");
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn successful_commands_write_no_artifact() {
        let dir =
            std::env::temp_dir().join(format!("coreav-snapshot-success-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let out = run(
            Command::new("/bin/sh").args(["-c", "exit 0"]),
            None,
            Duration::from_secs(5),
            Duration::from_secs(5),
            Some(&dir),
        )
        .unwrap();
        assert!(out.status.success());
        assert!(!dir.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_blocked_snapshot_does_not_delay_cleanup() {
        let (release, wait) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        let start = Instant::now();
        let text = snapshot_until(start + Duration::from_millis(50), move || {
            wait.recv().unwrap();
            done.send(()).unwrap();
            String::new()
        });
        assert!(text.contains("snapshot did not finish within its budget"));
        assert!(start.elapsed() < Duration::from_secs(5));
        release.send(()).unwrap();
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn artifact_bytes_are_capped_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("coreav-snapshot-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let text = "x".repeat(32 * 1024 - 1) + &"é".repeat(128);
        write_snapshot(&dir, 7, &text).unwrap();
        let saved = std::fs::read_to_string(dir.join("pid-7.txt")).unwrap();
        assert_eq!(saved.len(), 32 * 1024 - 1);
        let error = write_snapshot(&dir, 7, "replacement").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read_to_string(dir.join("pid-7.txt")).unwrap(),
            saved
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn syscall_words_reject_invalid_addresses() {
        assert_eq!(syscall_word("0x89"), Some(137));
        assert_eq!(syscall_word("202"), Some(202));
        for word in ["running", "-1", "0x", "0x10000000000000000"] {
            assert_eq!(syscall_word(word), None);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_pipe_holder_in_the_group_ends_with_the_child() {
        let start = Instant::now();
        let out = output(
            Command::new("/bin/sh").args(["-c", "sleep 30 & exit 0"]),
            None,
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(out.status.success());
        assert!(start.elapsed() < Duration::from_secs(12));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_escaped_pipe_holder_is_reported_without_waiting() {
        let setsid = ["/usr/bin/setsid", "/bin/setsid"]
            .into_iter()
            .find(|p| std::path::Path::new(p).is_file());
        let Some(setsid) = setsid else {
            eprintln!("skipping: setsid is absent");
            return;
        };
        let dir = std::env::temp_dir().join(format!("coreav-pipe-holder-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ready = dir.join("ready");
        let _ = std::fs::remove_file(&ready);
        let body = format!(
            "{setsid} /bin/sh -c 'echo ready > \"$1\"; exec sleep 15' sh '{}' & while ! test -f '{}'; do sleep 0.01; done; exit 0",
            ready.display(),
            ready.display()
        );
        let start = Instant::now();
        let err = run(
            Command::new("/bin/sh").args(["-c", &body]),
            None,
            Duration::from_secs(10),
            Duration::from_millis(200),
            None,
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(
            err.to_string().contains("pipes remained open after exit"),
            "{err}"
        );
        assert!(err.to_string().contains("holds_tracked_pipe=true"), "{err}");
        assert!(err.to_string().contains("sleep"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(12), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
