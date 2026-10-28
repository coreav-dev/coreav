//! A hostile worker (decode.md, bar 3): a shell that writes chosen bytes
//! instead of the worker. Every case must end in an error, never a panic, a
//! fault or a hang past the deadline.
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::{Duration, Instant};

use coreav_decode::{Codec, Decoder, ErrorKind, Limits, Packet, StreamConfig};
use coreav_worker::{SandboxedDecoder, TransferMode, WorkerBackend, WorkerCommand, WorkerOptions};

const READY: u8 = 0x81;
const FRAME: u8 = 0x82;
const DONE: u8 = 0x83;
const ERROR: u8 = 0x84;

fn message(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut m = ((payload.len() + 1) as u32).to_le_bytes().to_vec();
    m.push(kind);
    m.extend_from_slice(payload);
    m
}

fn ready() -> Vec<u8> {
    let mut p = 4u32.to_le_bytes().to_vec();
    p.extend_from_slice(b"fake");
    message(READY, &p)
}

/// A frame descriptor: `planes` as (offset, stride, len), then `data`.
fn frame(
    seq: u64,
    slot: u32,
    w: u32,
    h: u32,
    format: [u8; 3],
    planes: &[(u64, u64, u64)],
    data: &[u8],
) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&seq.to_le_bytes());
    p.extend_from_slice(&slot.to_le_bytes());
    p.extend_from_slice(&7i64.to_le_bytes());
    p.extend_from_slice(&w.to_le_bytes());
    p.extend_from_slice(&h.to_le_bytes());
    p.extend_from_slice(&format);
    p.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0, 0xFF]);
    p.push(planes.len() as u8);
    for (o, s, l) in planes {
        p.extend_from_slice(&o.to_le_bytes());
        p.extend_from_slice(&s.to_le_bytes());
        p.extend_from_slice(&l.to_le_bytes());
    }
    p.extend_from_slice(data);
    message(FRAME, &p)
}

const YUV420: [u8; 3] = [0, 1, 8];
const PLANES_4X2: [(u64, u64, u64); 3] = [(0, 4, 8), (8, 2, 2), (10, 2, 2)];

/// A shell that drains standard input, writes `bytes`, then runs `then`.
fn shell(bytes: &[u8], then: &str) -> WorkerCommand {
    let escaped: String = bytes.iter().map(|b| format!("\\{b:03o}")).collect();
    WorkerCommand {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            format!("cat >/dev/null & printf '{escaped}'; {then}").into(),
        ],
    }
}

fn options(command: WorkerCommand) -> WorkerOptions {
    WorkerOptions {
        transfer: TransferMode::Shared,
        ffmpeg: None,
        ..WorkerOptions::new(command)
    }
}

fn limits() -> Limits {
    Limits {
        timeout: Duration::from_secs(2),
        ..Limits::default()
    }
}

fn stream() -> StreamConfig {
    StreamConfig {
        width: Some(4),
        height: Some(2),
        ..StreamConfig::new(Codec::H264)
    }
}

/// Runs a session against the fake worker: open, one packet, end, then
/// every frame. Returns the frames received or the first error's kind.
fn session(command: WorkerCommand) -> Result<usize, ErrorKind> {
    let started = Instant::now();
    let result = (|| {
        let mut d = SandboxedDecoder::open(
            &options(command),
            WorkerBackend::Ffmpeg,
            &stream(),
            &limits(),
        )
        .map_err(|e| e.kind)?;
        d.send_packet(&Packet {
            data: &[1, 2, 3],
            pts: Some(0),
            dts: Some(0),
            keyframe: true,
            discard: false,
        })
        .map_err(|e| e.kind)?;
        d.send_eof().map_err(|e| e.kind)?;
        let mut frames = 0;
        while d.receive_frame().map_err(|e| e.kind)?.is_some() {
            frames += 1;
        }
        Ok(frames)
    })();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "past the deadline"
    );
    result
}

#[test]
fn a_well_behaved_fake_works() {
    let mut bytes = ready();
    bytes.extend(frame(0, u32::MAX, 4, 2, YUV420, &PLANES_4X2, &[9; 12]));
    bytes.extend(frame(1, 1, 4, 2, YUV420, &PLANES_4X2, &[]));
    bytes.extend(message(DONE, &[]));
    assert_eq!(session(shell(&bytes, "sleep 1")), Ok(2));
}

#[test]
fn hostile_workers_end_in_errors() {
    let r = ready();
    let with = |tail: Vec<u8>| {
        let mut b = r.clone();
        b.extend(tail);
        b
    };
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("garbage", b"\x05\x00\x00\x00\x99abcd".to_vec(), "sleep 1"),
        ("exits after ready", r.clone(), "exit 0"),
        ("exits with a signal", r.clone(), "kill -9 $$"),
        ("never ready", Vec::new(), "sleep 30"),
        ("hangs after ready", r.clone(), "sleep 30"),
        (
            "huge length",
            with(vec![0xF0, 0xFF, 0xFF, 0xFF, FRAME]),
            "sleep 1",
        ),
        (
            "frame before ready",
            frame(0, u32::MAX, 4, 2, YUV420, &PLANES_4X2, &[0; 12]),
            "sleep 1",
        ),
        ("ready twice", with(ready()), "sleep 1"),
        (
            "sequence skips",
            with(frame(5, u32::MAX, 4, 2, YUV420, &PLANES_4X2, &[0; 12])),
            "sleep 1",
        ),
        (
            "no such slot",
            with(frame(0, 7, 4, 2, YUV420, &PLANES_4X2, &[])),
            "sleep 1",
        ),
        (
            "plane past the slot",
            with(frame(
                0,
                1,
                4,
                2,
                YUV420,
                &[(0, 4, 8), (8, 2, 2), (u64::MAX - 1, 2, 2)],
                &[],
            )),
            "sleep 1",
        ),
        (
            "short stride",
            with(frame(
                0,
                0,
                4,
                2,
                YUV420,
                &[(0, 3, 7), (8, 2, 2), (10, 2, 2)],
                &[],
            )),
            "sleep 1",
        ),
        (
            "length off by one",
            with(frame(
                0,
                0,
                4,
                2,
                YUV420,
                &[(0, 4, 9), (8, 2, 2), (10, 2, 2)],
                &[],
            )),
            "sleep 1",
        ),
        (
            "oversize geometry",
            with(frame(0, 0, 100_000, 100_000, YUV420, &PLANES_4X2, &[])),
            "sleep 1",
        ),
        (
            "unknown format",
            with(frame(0, 0, 4, 2, [9, 9, 9], &PLANES_4X2, &[])),
            "sleep 1",
        ),
        (
            "missing plane",
            with(frame(0, 0, 4, 2, YUV420, &PLANES_4X2[..2], &[])),
            "sleep 1",
        ),
        (
            "inline data short",
            with(frame(0, u32::MAX, 4, 2, YUV420, &PLANES_4X2, &[0; 11])),
            "sleep 1",
        ),
        (
            "data after a slot descriptor",
            with(frame(0, 0, 4, 2, YUV420, &PLANES_4X2, &[1])),
            "sleep 1",
        ),
        (
            "worker error",
            with(message(ERROR, &[3, 3, 0, 0, 0, b'b', b'a', b'd'])),
            "sleep 1",
        ),
        (
            "truncated message",
            with(vec![50, 0, 0, 0, FRAME, 1, 2]),
            "exit 0",
        ),
        ("flood", r.clone(), "yes"),
    ];
    for (name, bytes, then) in cases {
        let result = session(shell(&bytes, then));
        assert!(result.is_err(), "{name}: {result:?}");
    }
}

fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:03o}")).collect()
}

#[test]
fn a_worker_that_never_reads_cannot_hang_the_parent() {
    // Ready, then never reads its input: a packet larger than the pipe
    // holds blocks the parent's write until the deadline kills the worker.
    let command = WorkerCommand {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            format!("printf '{}'; exec sleep 30", escaped(&ready())).into(),
        ],
    };
    let started = Instant::now();
    let mut d = SandboxedDecoder::open(
        &options(command),
        WorkerBackend::Ffmpeg,
        &stream(),
        &limits(),
    )
    .expect("open");
    let big = vec![0u8; 4 << 20];
    let mut result = Ok(());
    for _ in 0..4 {
        result = d.send_packet(&Packet {
            data: &big,
            pts: Some(0),
            dts: Some(0),
            keyframe: true,
            discard: false,
        });
        if result.is_err() {
            break;
        }
    }
    assert_eq!(result.map_err(|e| e.kind), Err(ErrorKind::Timeout));
    drop(d);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "past the deadline: {:?}",
        started.elapsed()
    );
}

#[test]
fn the_backends_children_end_with_the_worker() {
    // The worker starts a child of its own (as the FFmpeg backend does),
    // then hangs; the deadline must end both.
    let dir = std::env::temp_dir().join(format!("coreav-hostile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let pid_file = dir.join("pid");
    let command = WorkerCommand {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            format!(
                "sleep 60 & echo $! > '{}'; cat >/dev/null & printf '{}'; exec sleep 30",
                pid_file.display(),
                escaped(&ready())
            )
            .into(),
        ],
    };
    assert_eq!(session(command), Err(ErrorKind::Timeout));
    let pid = std::fs::read_to_string(&pid_file).expect("pid");
    let stat = std::path::Path::new("/proc").join(pid.trim()).join("stat");
    let until = Instant::now() + Duration::from_secs(2);
    // Gone, or a zombie waiting for init to reap it.
    let alive = || {
        std::fs::read_to_string(&stat).is_ok_and(|s| {
            s.rsplit_once(") ")
                .is_some_and(|(_, rest)| !rest.starts_with('Z'))
        })
    };
    while alive() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(), "the worker's child outlived it");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_worker_that_escapes_its_group_cannot_hang_the_parent() {
    // The worker hands its input to a process in a session of its own,
    // which killing the worker's group does not reach: the parent's write
    // must still end at the deadline.
    let dir = std::env::temp_dir().join(format!("coreav-escape-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let pid_file = dir.join("pid");
    let command = WorkerCommand {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            format!(
                "setsid sleep 30 <&0 >/dev/null 2>&1 & echo $! > '{}'; printf '{}'; exec sleep 30",
                pid_file.display(),
                escaped(&ready())
            )
            .into(),
        ],
    };
    let started = Instant::now();
    let mut d = SandboxedDecoder::open(
        &options(command),
        WorkerBackend::Ffmpeg,
        &stream(),
        &limits(),
    )
    .expect("open");
    let big = vec![0u8; 4 << 20];
    let mut result = Ok(());
    for _ in 0..4 {
        result = d.send_packet(&Packet {
            data: &big,
            pts: Some(0),
            dts: Some(0),
            keyframe: true,
            discard: false,
        });
        if result.is_err() {
            break;
        }
    }
    assert_eq!(result.map_err(|e| e.kind), Err(ErrorKind::Timeout));
    drop(d);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "past the deadline: {:?}",
        started.elapsed()
    );
    if let Ok(pid) = std::fs::read_to_string(&pid_file) {
        let _ = std::process::Command::new("kill").arg(pid.trim()).status();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_worker_cannot_queue_acknowledgements_without_bound() {
    // Frames, all in slot 0, from a worker that never reads its input. The
    // parent holds the slot until its acknowledgement is written, so a
    // frame in it before then (here, the second) ends the session with a
    // protocol violation, instead of every frame queueing one more
    // acknowledgement.
    let dir = std::env::temp_dir().join(format!("coreav-acks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("frames");
    let mut bytes = ready();
    for seq in 0..100_000 {
        bytes.extend(frame(seq, 0, 4, 2, YUV420, &PLANES_4X2, &[]));
    }
    std::fs::write(&file, &bytes).expect("write");
    let command = WorkerCommand {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            format!("cat '{}'; exec sleep 30", file.display()).into(),
        ],
    };
    let mut d = SandboxedDecoder::open(
        &options(command),
        WorkerBackend::Ffmpeg,
        &stream(),
        &limits(),
    )
    .expect("open");
    let mut frames = 0u32;
    let kind = loop {
        match d.receive_frame() {
            Ok(Some(_)) => frames += 1,
            Ok(None) => std::thread::sleep(Duration::from_millis(1)),
            Err(e) => break e.kind,
        }
    };
    assert_eq!(kind, ErrorKind::Backend, "after {frames} frames");
    assert!(frames < 50_000, "{frames} frames accepted");
    let _ = std::fs::remove_dir_all(&dir);
}
