use std::io::{self, Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

pub fn output(
    command: &mut Command,
    input: Option<Vec<u8>>,
    limit: Duration,
) -> io::Result<Output> {
    run(command, input, limit, Duration::from_secs(5))
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
) -> io::Result<Output> {
    let deadline = Instant::now() + limit;
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
            let snapshot = snapshot(pid, &pipes);
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
fn snapshot(pid: u32, _: &[String]) -> String {
    format!("pid={pid}; detailed process, thread and pipe state requires Linux /proc")
}

#[cfg(target_os = "linux")]
fn snapshot(root: u32, pipes: &[String]) -> String {
    use std::collections::HashSet;
    use std::fmt::Write;
    use std::path::Path;

    let until = Instant::now() + Duration::from_secs(2);
    let mut text = format!("process snapshot; root={root}; tracked_pipes={pipes:?}\n");
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
            }
        }
    }
    let _ = writeln!(
        text,
        "snapshot limits: 2s, 4096 candidates, 64 processes, 256 scanned/64 listed fds, 32 threads/process, 32KiB"
    );
    if text.len() > 32 * 1024 {
        let mut end = 32 * 1024;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
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
