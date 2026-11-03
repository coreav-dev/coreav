//! Synthetic AV1 wire/spool checks; no native encoder or tile decoder runs.

#![cfg(feature = "svt-av1")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

#[path = "../../coreav-backend-svt-av1/tests/frames.rs"]
mod fixtures;

use std::{
    fs::{File, OpenOptions},
    num::NonZeroU64,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use coreav_backend_svt_av1::{
    Error as BackendError,
    model::{Color, Retention, Settings},
    protocol::{Limits, OutputWriter, Request},
};
use coreav_decode::ErrorKind;
use coreav_frame::{Picture, PixelFormat};
use coreav_transcode::{
    av1::OutputSink,
    contract::{Submissions, VideoLayout},
    encoder::Sink,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temporary(PathBuf);

impl Temporary {
    fn new() -> (Self, File) {
        let path = std::env::temp_dir().join(format!(
            "coreav-av1-spool-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        (Self(path), file)
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn request() -> (Request, Retention) {
    let settings = Settings::new(64, 64, (30, 1), 8, 32, Color::default()).unwrap();
    let limits = Limits::new(20_000, 1 << 20, 10, 4096).unwrap();
    let retention = Retention::new(settings, limits.retained_bytes(), 64, 16, 32).unwrap();
    assert_eq!(retention.pictures(), 3);
    (Request { settings, limits }, retention)
}

fn submissions(times: &[i64]) -> Submissions {
    let mut submitted = Submissions::new(
        VideoLayout::new(64, 64, PixelFormat::YUV420P, 6144).unwrap(),
        10,
    );
    for &pts in times {
        submitted
            .submit(Picture::zeroed(64, 64, PixelFormat::YUV420P).unwrap(), pts)
            .unwrap();
    }
    submitted
}

fn wire() -> (Vec<u8>, Vec<u8>) {
    changed_wire("none", 0)
}

fn changed_wire(change: &str, native_pts: i64) -> (Vec<u8>, Vec<u8>) {
    let (request, retention) = request();
    let mut wire = Vec::new();
    let mut payload = Vec::new();
    let seq = fixtures::Seq::default();
    let mut writer = OutputWriter::new(&mut wire, "4.2.0", request.limits).unwrap();
    for (base, count) in [(0, 3), (3, 1)] {
        writer
            .begin(
                if change == "base" && base != 0 {
                    0
                } else {
                    base
                },
                retention,
                request.settings.keyint(),
            )
            .unwrap();
        let mut bytes = Vec::new();
        for index in 0..count {
            bytes.extend_from_slice(&fixtures::packet(2, &[]));
            if index == 0 {
                let mut sequence = seq.packet();
                if change == "configuration" && base != 0 {
                    sequence[1] |= 0x80;
                    sequence.insert(2, 0);
                }
                bytes.extend_from_slice(&sequence);
                payload.extend_from_slice(&sequence);
            }
            let frame = fixtures::Coded::default().packet(seq);
            bytes.extend_from_slice(&frame);
            payload.extend_from_slice(&frame);
        }
        writer.packet(native_pts, -1, 1, 0, &bytes).unwrap();
        writer
            .end(base, count, 1, retention.peak_bytes(count).unwrap())
            .unwrap();
    }
    writer.complete(4, 2).unwrap();
    (wire, payload)
}

#[test]
fn complete_segmented_samples_keep_global_clocks_and_full_configuration() {
    let (request, retention) = request();
    let (wire, payload) = wire();
    for origin in [-90000, 90000] {
        let times = [origin, origin + 1, origin + 3, origin + 4];
        let submitted = submissions(&times);
        let (temporary, file) = Temporary::new();
        let mut sink = OutputSink::new(
            file,
            request,
            retention,
            Some((4, 3)),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        for byte in &wire {
            sink.write(&[*byte]).unwrap();
        }
        let result = sink
            .finish(&submitted, NonZeroU64::new(3).unwrap())
            .unwrap();
        assert_eq!(result.version, "4.2.0");
        assert_eq!(result.segments, 2);
        assert_eq!(result.bytes, payload.len() as u64);
        assert_eq!(std::fs::read(&temporary.0).unwrap(), payload);
        assert_eq!(result.packets.len(), times.len());
        for (index, (meta, time)) in result.packets.iter().enumerate() {
            assert_eq!(meta.frame().index(), index as u64);
            assert_eq!(meta.sync(), index == 0 || index == 3);
            assert_eq!(time.pts, times[index]);
            assert_eq!(time.dts, times[index]);
            assert_eq!(time.duration, [1, 2, 1, 3][index]);
        }
        let configuration = [
            fixtures::Seq::default()
                .parse()
                .unwrap()
                .av1c_header()
                .bytes()
                .as_slice(),
            fixtures::Seq::default().packet().as_slice(),
        ]
        .concat();
        assert_eq!(&result.sample_entry[4..8], b"av01");
        let at = result
            .sample_entry
            .windows(4)
            .position(|bytes| bytes == b"av1C")
            .unwrap();
        assert_eq!(
            &result.sample_entry[at + 4..at + 4 + configuration.len()],
            configuration
        );
    }
}

#[test]
fn incomplete_work_wrong_submission_counts_and_changed_spool_extents_fail() {
    let (request, retention) = request();
    let (wire, _) = wire();
    let submitted = submissions(&[0, 1, 3, 4]);
    for failure in ["truncation", "count", "spool"] {
        let (temporary, file) = Temporary::new();
        let mut sink = OutputSink::new(
            file,
            request,
            retention,
            None,
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        let bytes = if failure == "truncation" {
            &wire[..wire.len() - 1]
        } else {
            &wire
        };
        sink.write(bytes).unwrap();
        if failure == "spool" {
            OpenOptions::new()
                .write(true)
                .open(&temporary.0)
                .unwrap()
                .set_len(1 << 20)
                .unwrap();
        }
        let wrong = submissions(&[0, 1, 3]);
        let source = if failure == "count" {
            &wrong
        } else {
            &submitted
        };
        assert_eq!(
            sink.finish(source, NonZeroU64::new(3).unwrap())
                .unwrap_err()
                .kind,
            ErrorKind::InvalidData,
            "{failure}"
        );
    }
}

#[test]
fn worker_failures_preserve_their_class_and_latch_without_fallback() {
    let (request, retention) = request();
    for (failure, expected) in [
        (BackendError::Native(0x8000_1000), ErrorKind::Backend),
        (BackendError::Limit("fixture limit"), ErrorKind::Limit),
    ] {
        let (_temporary, file) = Temporary::new();
        let mut sink = OutputSink::new(
            file,
            request,
            retention,
            None,
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        let mut bytes = Vec::new();
        OutputWriter::new(&mut bytes, "4.2.0", request.limits)
            .unwrap()
            .failure(&failure)
            .unwrap();
        let error = sink.write(&bytes).unwrap_err();
        assert_eq!(error.kind, expected);
        assert_eq!(sink.write(&[]).unwrap_err().kind, expected);
        assert_eq!(
            sink.finish(&submissions(&[]), NonZeroU64::new(1).unwrap())
                .unwrap_err()
                .kind,
            expected
        );
    }
}

#[test]
fn streaming_deadline_failures_remain_terminal_at_finalization() {
    let (request, retention) = request();
    let (_temporary, file) = Temporary::new();
    let mut sink = OutputSink::new(file, request, retention, None, Instant::now()).unwrap();
    assert_eq!(sink.write(&wire().0).unwrap_err().kind, ErrorKind::Timeout);
    assert_eq!(
        sink.finish(&submissions(&[0, 1, 3, 4]), NonZeroU64::new(1).unwrap())
            .unwrap_err()
            .kind,
        ErrorKind::Timeout
    );
}

#[test]
fn reset_global_identity_changed_configuration_and_invalid_native_assertions_fail() {
    let (request, retention) = request();
    for (change, pts) in [
        ("base", 0),
        ("configuration", 0),
        ("none", -1),
        ("none", 1),
        ("none", 3),
    ] {
        let (_temporary, file) = Temporary::new();
        let mut sink = OutputSink::new(
            file,
            request,
            retention,
            None,
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        let bytes = changed_wire(change, pts).0;
        assert_eq!(
            sink.write(&bytes).unwrap_err().kind,
            ErrorKind::InvalidData,
            "{change}/{pts}"
        );
        assert_eq!(
            sink.finish(&submissions(&[0, 1, 3, 4]), NonZeroU64::new(3).unwrap())
                .unwrap_err()
                .kind,
            ErrorKind::InvalidData
        );
    }
}
