//! The FFmpeg fallback through the worker, shared memory and pipe, against
//! FFmpeg reading the file itself. Needs `ffmpeg` (with libx264) on `PATH`
//! and the `ffmpeg` feature; ignored by default.
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![cfg(feature = "ffmpeg")]

use std::path::{Path, PathBuf};
use std::process::Command;

use coreav_decode::{Codec, CodecConfig, ConfigKind, Decoder, Limits, Packet, StreamConfig};
use coreav_isobmff::{Allowlist, Budget, FileSource, Source, probe};
use coreav_worker::{
    SandboxedDecoder, TransferMode, WorkerBackend, WorkerCommand, WorkerOptions, find_on_path,
};

fn ffmpeg() -> PathBuf {
    find_on_path("ffmpeg").expect("ffmpeg on PATH")
}

fn reference(file: &Path) -> Vec<u8> {
    let out = Command::new(ffmpeg())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-ignore_editlist",
            "1",
            "-i",
        ])
        .arg(file)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-",
        ])
        .output()
        .expect("ffmpeg runs");
    assert!(out.status.success());
    out.stdout
}

fn decode(file: &Path, transfer: TransferMode) -> Vec<u8> {
    let mut source = FileSource::open(file).expect("open");
    let movie = probe(&mut source, &Default::default(), &Allowlist::default()).expect("probe");
    let track = movie.tracks.first().expect("track");
    let stream = StreamConfig {
        config: track
            .description
            .codec_config
            .as_ref()
            .map(|c| CodecConfig {
                kind: ConfigKind::Avcc,
                data: c.data.clone(),
            }),
        width: track.description.width.map(u32::from),
        height: track.description.height.map(u32::from),
        ..StreamConfig::new(Codec::H264)
    };
    let options = WorkerOptions {
        transfer,
        ffmpeg: Some(ffmpeg()),
        ..WorkerOptions::new(WorkerCommand {
            program: env!("CARGO_BIN_EXE_coreav-worker").into(),
            args: Vec::new(),
        })
    };
    let mut decoder =
        SandboxedDecoder::open(&options, WorkerBackend::Ffmpeg, &stream, &Limits::default())
            .expect("open");
    assert!(decoder.info().fell_back());
    let mut budget = Budget::new(&Default::default());
    let packets: Vec<_> = movie
        .packets(0, &mut source, &Default::default(), &mut budget)
        .expect("packets")
        .collect::<Result<_, _>>()
        .expect("packet list");
    let mut out = Vec::new();
    let take = |d: &mut SandboxedDecoder, out: &mut Vec<u8>| {
        while let Some(frame) = d.receive_frame().expect("receive") {
            frame
                .to_cpu()
                .expect("cpu")
                .write_packed(out)
                .expect("packed");
        }
    };
    for p in &packets {
        let mut data = vec![0u8; p.size as usize];
        source.read_exact_at(p.offset, &mut data).expect("read");
        decoder
            .send_packet(&Packet {
                data: &data,
                pts: Some(p.cts),
                dts: Some(p.dts),
                keyframe: p.keyframe,
                discard: false,
            })
            .expect("send");
        take(&mut decoder, &mut out);
    }
    decoder.send_eof().expect("eof");
    take(&mut decoder, &mut out);
    out
}

#[test]
#[ignore = "needs ffmpeg"]
fn fallback_through_the_worker_matches_ffmpeg() {
    let dir = std::env::temp_dir().join(format!("coreav-worker-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("b.mp4");
    let status = Command::new(ffmpeg())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .args([
            "testsrc2=size=320x240:rate=25",
            "-t",
            "2",
            "-c:v",
            "libx264",
            "-bf",
            "2",
        ])
        .arg(&file)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    let expected = reference(&file);
    for transfer in [TransferMode::Shared, TransferMode::Pipe] {
        let ours = decode(&file, transfer);
        assert_eq!(ours.len(), expected.len(), "{transfer:?}");
        assert!(ours == expected, "{transfer:?}: frames differ");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every packet first, then the end, then frames received slowly: the
/// worker's queues fill and FFmpeg waits on its output while the end of
/// its input is still being written. Before the backpressure fix this hung
/// (`docs/design/decode.md`, "Threads and backpressure").
#[test]
#[ignore = "needs ffmpeg"]
fn slow_consumer_after_the_end_of_input() {
    let dir = std::env::temp_dir().join(format!("coreav-worker-slow-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("big.mp4");
    let status = Command::new(ffmpeg())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .args([
            "testsrc2=size=1280x720:rate=25",
            "-t",
            "3",
            "-c:v",
            "libx264",
            "-bf",
            "2",
        ])
        .arg(&file)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    let mut source = FileSource::open(&file).expect("open");
    let movie = probe(&mut source, &Default::default(), &Allowlist::default()).expect("probe");
    let track = movie.tracks.first().expect("track");
    let stream = StreamConfig {
        config: track
            .description
            .codec_config
            .as_ref()
            .map(|c| CodecConfig {
                kind: ConfigKind::Avcc,
                data: c.data.clone(),
            }),
        width: track.description.width.map(u32::from),
        height: track.description.height.map(u32::from),
        ..StreamConfig::new(Codec::H264)
    };
    let options = WorkerOptions {
        ffmpeg: Some(ffmpeg()),
        ..WorkerOptions::new(WorkerCommand {
            program: env!("CARGO_BIN_EXE_coreav-worker").into(),
            args: Vec::new(),
        })
    };
    let limits = Limits {
        timeout: std::time::Duration::from_secs(60),
        ..Limits::default()
    };
    let mut decoder =
        SandboxedDecoder::open(&options, WorkerBackend::Ffmpeg, &stream, &limits).expect("open");
    let mut budget = Budget::new(&Default::default());
    let packets: Vec<_> = movie
        .packets(0, &mut source, &Default::default(), &mut budget)
        .expect("packets")
        .collect::<Result<_, _>>()
        .expect("packet list");
    for p in &packets {
        let mut data = vec![0u8; p.size as usize];
        source.read_exact_at(p.offset, &mut data).expect("read");
        decoder
            .send_packet(&Packet {
                data: &data,
                pts: Some(p.cts),
                dts: Some(p.dts),
                keyframe: p.keyframe,
                discard: false,
            })
            .expect("send");
    }
    decoder.send_eof().expect("eof");
    let mut frames = 0;
    while let Some(_frame) = decoder.receive_frame().expect("receive") {
        frames += 1;
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(frames, packets.len());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every packet, then frames waited for without another packet or the
/// end: the frames FFmpeg decodes meanwhile reach the parent, as a caller
/// that paces its packets on the frames it gets needs. Before the worker
/// handed frames on while idle they waited for the next message, and such
/// a caller stalled (#585).
#[test]
#[ignore = "needs ffmpeg"]
fn frames_come_while_no_packet_does() {
    let dir = std::env::temp_dir().join(format!("coreav-worker-idle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("idle.mp4");
    let status = Command::new(ffmpeg())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .args([
            "testsrc2=size=640x360:rate=25",
            "-t",
            "4",
            "-c:v",
            "libx264",
            "-bf",
            "2",
        ])
        .arg(&file)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    let mut source = FileSource::open(&file).expect("open");
    let movie = probe(&mut source, &Default::default(), &Allowlist::default()).expect("probe");
    let track = movie.tracks.first().expect("track");
    let stream = StreamConfig {
        config: track
            .description
            .codec_config
            .as_ref()
            .map(|c| CodecConfig {
                kind: ConfigKind::Avcc,
                data: c.data.clone(),
            }),
        width: track.description.width.map(u32::from),
        height: track.description.height.map(u32::from),
        ..StreamConfig::new(Codec::H264)
    };
    let options = WorkerOptions {
        ffmpeg: Some(ffmpeg()),
        threads: 1,
        ..WorkerOptions::new(WorkerCommand {
            program: env!("CARGO_BIN_EXE_coreav-worker").into(),
            args: Vec::new(),
        })
    };
    let limits = Limits {
        timeout: std::time::Duration::from_secs(60),
        ..Limits::default()
    };
    let mut decoder =
        SandboxedDecoder::open(&options, WorkerBackend::Ffmpeg, &stream, &limits).expect("open");
    let mut budget = Budget::new(&Default::default());
    let packets: Vec<_> = movie
        .packets(0, &mut source, &Default::default(), &mut budget)
        .expect("packets")
        .collect::<Result<_, _>>()
        .expect("packet list");
    for p in &packets {
        let mut data = vec![0u8; p.size as usize];
        source.read_exact_at(p.offset, &mut data).expect("read");
        decoder
            .send_packet(&Packet {
                data: &data,
                pts: Some(p.cts),
                dts: Some(p.dts),
                keyframe: p.keyframe,
                discard: false,
            })
            .expect("send");
    }
    // FFmpeg holds back a few frames (its reordering) until the end; every
    // other frame comes without another message from the parent.
    let wanted = packets.len() - 8;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut frames = 0;
    while frames < wanted && std::time::Instant::now() < until {
        match decoder.receive_frame().expect("receive") {
            Some(_) => frames += 1,
            None => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    assert!(frames >= wanted, "{frames} of {} frames", packets.len());
    decoder.send_eof().expect("eof");
    while decoder.receive_frame().expect("receive").is_some() {
        frames += 1;
    }
    assert_eq!(frames, packets.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs ffmpeg"]
fn packets_past_the_pending_limit_fail_with_limit() {
    // Packets the worker has not handed to the decoder are capped by
    // `max_pending_bytes`; past it the session ends with `Limit`, not a
    // generic backend error. One slot, and every packet sent before any
    // frame is received, so that when the cap is passed the worker is
    // waiting for the slot back: acknowledgements must keep reaching it.
    let dir = std::env::temp_dir().join(format!("coreav-cap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("c.mp4");
    let status = Command::new(ffmpeg())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .args([
            "testsrc2=size=320x240:rate=25",
            "-t",
            "4",
            "-c:v",
            "libx264",
            "-bf",
            "0",
        ])
        .arg(&file)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    let mut source = FileSource::open(&file).expect("open");
    let movie = probe(&mut source, &Default::default(), &Allowlist::default()).expect("probe");
    let track = movie.tracks.first().expect("track");
    let stream = StreamConfig {
        config: track
            .description
            .codec_config
            .as_ref()
            .map(|c| CodecConfig {
                kind: ConfigKind::Avcc,
                data: c.data.clone(),
            }),
        width: track.description.width.map(u32::from),
        height: track.description.height.map(u32::from),
        ..StreamConfig::new(Codec::H264)
    };
    let mut budget = Budget::new(&Default::default());
    let packets: Vec<_> = movie
        .packets(0, &mut source, &Default::default(), &mut budget)
        .expect("packets")
        .collect::<Result<_, _>>()
        .expect("packet list");
    // The first packets decode into more frames than the slot and the
    // parent's queue hold, so the worker ends up waiting for the slot; the
    // rest then pass the cap.
    const FIRST: usize = 30;
    let rest: u64 = packets.iter().skip(FIRST).map(|p| u64::from(p.size)).sum();
    let options = WorkerOptions {
        slots: 1,
        ffmpeg: Some(ffmpeg()),
        ..WorkerOptions::new(WorkerCommand {
            program: env!("CARGO_BIN_EXE_coreav-worker").into(),
            args: Vec::new(),
        })
    };
    let limits = Limits {
        max_pending_bytes: rest / 2,
        timeout: std::time::Duration::from_secs(20),
        ..Limits::default()
    };
    let mut decoder =
        SandboxedDecoder::open(&options, WorkerBackend::Ffmpeg, &stream, &limits).expect("open");
    let mut kind = None;
    for (i, p) in packets.iter().enumerate() {
        if i == FIRST {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        let mut data = vec![0u8; p.size as usize];
        source.read_exact_at(p.offset, &mut data).expect("read");
        let sent = decoder.send_packet(&Packet {
            data: &data,
            pts: Some(p.cts),
            dts: Some(p.dts),
            keyframe: p.keyframe,
            discard: false,
        });
        if let Err(e) = sent {
            kind = Some(e.kind);
            break;
        }
    }
    if kind.is_none() {
        kind = decoder.send_eof().err().map(|e| e.kind);
    }
    while kind.is_none() {
        match decoder.receive_frame() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => kind = Some(e.kind),
        }
    }
    assert_eq!(kind, Some(coreav_decode::ErrorKind::Limit));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs ffmpeg"]
fn running_into_the_memory_limit_fails_with_limit() {
    // 4K ProRes 4444 (about 66 MB a frame) with no declared size, so that
    // nothing is fitted before FFmpeg starts, and one decoder thread: the
    // frame buffers, not the host's CPU count, run into a 512 MiB budget,
    // and the session ends with `Limit`, not a backend fault.
    let dir = std::env::temp_dir().join(format!("coreav-mem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("m.mov");
    let status = Command::new(ffmpeg())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .args([
            "testsrc2=size=3840x2160:rate=25",
            "-t",
            "0.4",
            "-c:v",
            "prores_ks",
            "-profile:v",
            "4444",
            "-pix_fmt",
            "yuva444p10le",
        ])
        .arg(&file)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    let mut source = FileSource::open(&file).expect("open");
    let movie = probe(&mut source, &Default::default(), &Allowlist::default()).expect("probe");
    let stream = StreamConfig::new(Codec::ProRes);
    let mut budget = Budget::new(&Default::default());
    let packets: Vec<_> = movie
        .packets(0, &mut source, &Default::default(), &mut budget)
        .expect("packets")
        .collect::<Result<_, _>>()
        .expect("packet list");
    let options = WorkerOptions {
        ffmpeg: Some(ffmpeg()),
        threads: 1,
        ..WorkerOptions::new(WorkerCommand {
            program: env!("CARGO_BIN_EXE_coreav-worker").into(),
            args: Vec::new(),
        })
    };
    let limits = Limits {
        max_memory_bytes: 512 << 20,
        timeout: std::time::Duration::from_secs(30),
        ..Limits::default()
    };
    let started = std::time::Instant::now();
    let mut kind = None;
    match SandboxedDecoder::open(&options, WorkerBackend::Ffmpeg, &stream, &limits) {
        Err(e) => kind = Some(e.kind),
        Ok(mut decoder) => {
            for p in &packets {
                let mut data = vec![0u8; p.size as usize];
                source.read_exact_at(p.offset, &mut data).expect("read");
                let sent = decoder.send_packet(&Packet {
                    data: &data,
                    pts: Some(p.cts),
                    dts: Some(p.dts),
                    keyframe: p.keyframe,
                    discard: false,
                });
                if let Err(e) = sent {
                    kind = Some(e.kind);
                    break;
                }
                while kind.is_none() {
                    match decoder.receive_frame() {
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(e) => kind = Some(e.kind),
                    }
                }
                if kind.is_some() {
                    break;
                }
            }
            if kind.is_none() {
                kind = decoder.send_eof().err().map(|e| e.kind);
            }
            while kind.is_none() {
                match decoder.receive_frame() {
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(e) => kind = Some(e.kind),
                }
            }
        }
    }
    assert_eq!(kind, Some(coreav_decode::ErrorKind::Limit));
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    let _ = std::fs::remove_dir_all(&dir);
}
