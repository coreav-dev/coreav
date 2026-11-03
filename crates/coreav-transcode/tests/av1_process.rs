//! Synthetic wire and real process/pipe checks, without native encoding or tile decoding.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../coreav-backend-svt-av1/tests/frames.rs"]
#[allow(dead_code)]
mod fixtures;

use std::{
    fs::File,
    num::NonZeroU64,
    path::PathBuf,
    process::ExitCode,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use coreav_backend_svt_av1::{
    Error as BackendError,
    model::{Color, Retention, Settings},
    protocol::{InputReader, Limits, OutputWriter, Request},
};
use coreav_decode::ErrorKind;
use coreav_frame::{Chroma, Picture, PixelFormat, Plane};
use coreav_isobmff::Rational;
use coreav_thumbnail::StreamPacket;
use coreav_transcode::{
    av1::{Encoder, Launch, Timeline, WORKER_ARG},
    encoder::EncoderLimits,
};
use coreav_worker::ExecLauncher;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temporary(PathBuf);

impl Temporary {
    fn new() -> (Self, File) {
        let path = std::env::temp_dir().join(format!(
            "coreav-av1-process-{}-{}",
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

fn retention(request: Request) -> Retention {
    Retention::new(
        request.settings,
        request.limits.retained_bytes(),
        64,
        16,
        32,
    )
    .unwrap()
}

fn picture(index: u8) -> Picture {
    let planes = (0..3)
        .map(|plane| {
            let width = if plane == 0 { 64 } else { 32 };
            let stride = width + 7;
            let mut bytes = vec![0xee; stride * width];
            for row in bytes.chunks_mut(stride) {
                row[..width].fill(index * 3 + plane as u8);
            }
            Plane::new(bytes, stride)
        })
        .collect();
    Picture::new(64, 64, PixelFormat::YUV420P, planes).unwrap()
}

fn worker() -> ExitCode {
    let mut input = InputReader::new(std::io::stdin().lock()).unwrap();
    let request = input.request();
    let mode = request.settings.crf();
    assert_eq!(
        request.settings.rate(),
        if mode == 41 {
            (60_000, 2002)
        } else {
            (30_000, 1001)
        }
    );
    if mode == 38 {
        eprintln!("synthetic worker blocked before pictures");
        loop {
            std::thread::park();
        }
    }
    let mut count = 0;
    let mut origin = None;
    while let Some(header) = input.next_frame_header().unwrap() {
        let first = *origin.get_or_insert(header.pts);
        assert_eq!(
            header.pts - first,
            if mode == 40 || mode == 41 {
                [0, 3003, 6006, 9009][count]
            } else {
                [0, 1, 3, 4][count]
            }
        );
        let data = input.read_picture().unwrap();
        assert!(data[..4096].iter().all(|&value| value == count as u8 * 3));
        assert!(
            data[4096..5120]
                .iter()
                .all(|&value| value == count as u8 * 3 + 1)
        );
        assert!(
            data[5120..]
                .iter()
                .all(|&value| value == count as u8 * 3 + 2)
        );
        count += 1;
    }
    assert_eq!(count, 4);
    if mode == 37 {
        eprintln!("synthetic worker blocked after EOF");
        loop {
            std::thread::park();
        }
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut writer = OutputWriter::new(&mut out, "4.2.0", request.limits).unwrap();
    if mode == 34 {
        writer.failure(&BackendError::Native(0x8000_1000)).unwrap();
        return ExitCode::SUCCESS;
    }
    let retention = retention(request);
    assert_eq!(retention.pictures(), 3);
    let seq = fixtures::Seq::default();
    for (base, count) in [(0, 3), (3, 1)] {
        writer
            .begin(base, retention, request.settings.keyint())
            .unwrap();
        let mut bytes = Vec::new();
        for index in 0..count {
            bytes.extend_from_slice(&fixtures::packet(2, &[]));
            if index == 0 {
                bytes.extend_from_slice(&seq.packet());
            }
            bytes.extend_from_slice(&fixtures::Coded::default().packet(seq));
        }
        writer.packet(0, -1, 1, 0, &bytes).unwrap();
        writer
            .end(base, count, 1, retention.peak_bytes(count).unwrap())
            .unwrap();
    }
    if mode != 33 {
        writer.complete(if mode == 35 { 3 } else { 4 }, 2).unwrap();
    }
    if mode == 36 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn start(mode: u8, frames: u64, timeout: Duration, sandboxed: bool) -> (Temporary, Encoder) {
    let request = Request {
        settings: Settings::new(64, 64, (30_000, 1001), 8, mode, Color::default()).unwrap(),
        limits: Limits::new(20_000, 1 << 20, frames, 4096).unwrap(),
    };
    let (temporary, file) = Temporary::new();
    let launcher = ExecLauncher::current_exe().unwrap();
    let encoder = Encoder::spawn(
        &std::env::current_exe().unwrap(),
        request,
        retention(request),
        file,
        Launch {
            launcher: sandboxed.then_some(&launcher),
            limits: EncoderLimits::new(1 << 30),
            deadline: Instant::now() + timeout,
            pixel_aspect: Some((4, 3)),
        },
    )
    .unwrap();
    (temporary, encoder)
}

fn feed(encoder: &mut Encoder, origin: i64) {
    for (index, offset) in [0, 1, 3, 4].into_iter().enumerate() {
        encoder
            .send_frame(picture(index as u8), origin + offset)
            .unwrap();
    }
}

fn complete() {
    for sandboxed in [false, true] {
        for origin in [-90_000, 90_000] {
            let (temporary, mut encoder) = start(32, 10, Duration::from_secs(10), sandboxed);
            feed(&mut encoder, origin);
            let result = encoder.finish(NonZeroU64::new(3).unwrap()).unwrap();
            assert_eq!(result.version, "4.2.0");
            assert_eq!(result.segments, 2);
            assert_eq!(std::fs::metadata(&temporary.0).unwrap().len(), result.bytes);
            for (index, (meta, time)) in result.packets.iter().enumerate() {
                assert_eq!(meta.frame().index(), index as u64);
                assert_eq!(meta.sync(), index == 0 || index == 3);
                assert_eq!(time.pts, origin + [0, 1, 3, 4][index]);
                assert_eq!(time.dts, time.pts);
                assert_eq!(time.duration, [1, 2, 1, 3][index]);
            }
            assert_eq!(result.packets.len(), 4);
        }
    }
}

fn bad_output() {
    for (mode, expected) in [
        (33, ErrorKind::InvalidData),
        (34, ErrorKind::Backend),
        (35, ErrorKind::InvalidData),
        (36, ErrorKind::Backend),
    ] {
        let (_temporary, mut encoder) = start(mode, 10, Duration::from_secs(10), true);
        feed(&mut encoder, 0);
        let error = encoder.finish(NonZeroU64::new(3).unwrap()).unwrap_err();
        assert_eq!(error.kind, expected, "mode {mode}: {error}");
    }
}

fn bad_input() {
    for change in ["format", "geometry", "timestamp", "count"] {
        let (_temporary, mut encoder) = start(32, 1, Duration::from_secs(10), true);
        encoder.send_frame(picture(0), 0).unwrap();
        let (frame, pts, kind) = match change {
            "format" => (
                Picture::zeroed(
                    64,
                    64,
                    PixelFormat::Yuv {
                        chroma: Chroma::Cs420,
                        depth: 10,
                    },
                )
                .unwrap(),
                1,
                ErrorKind::Unsupported,
            ),
            "geometry" => (
                Picture::zeroed(66, 64, PixelFormat::YUV420P).unwrap(),
                1,
                ErrorKind::InvalidData,
            ),
            "timestamp" => (picture(1), 0, ErrorKind::InvalidData),
            _ => (picture(1), 1, ErrorKind::Limit),
        };
        let error = encoder.send_frame(frame, pts).unwrap_err();
        assert_eq!(error.kind, kind, "{change}");
        assert_eq!(
            encoder.send_frame(picture(1), 2).unwrap_err().to_string(),
            error.to_string()
        );
        assert_eq!(
            encoder
                .finish(NonZeroU64::new(1).unwrap())
                .unwrap_err()
                .to_string(),
            error.to_string()
        );
    }
    let (_temporary, encoder) = start(32, 10, Duration::from_secs(10), true);
    assert_eq!(
        encoder
            .finish(NonZeroU64::new(1).unwrap())
            .unwrap_err()
            .kind,
        ErrorKind::InvalidData
    );
}

fn hangs_after_input() {
    let (_temporary, mut encoder) = start(37, 10, Duration::from_secs(2), true);
    feed(&mut encoder, 0);
    let error = encoder.finish(NonZeroU64::new(3).unwrap()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Timeout);
    assert!(
        error
            .to_string()
            .contains("synthetic worker blocked after EOF"),
        "{error}"
    );
}

fn never_reads_pictures() {
    let (_temporary, mut encoder) = start(38, 1024, Duration::from_secs(2), true);
    let error = (0..1024)
        .find_map(|index| encoder.send_frame(picture(0), i64::from(index)).err())
        .unwrap();
    assert_eq!(error.kind, ErrorKind::Timeout);
    assert!(
        error
            .to_string()
            .contains("synthetic worker blocked before pictures"),
        "{error}"
    );
    assert_eq!(
        encoder.send_frame(picture(0), 100).unwrap_err().to_string(),
        error.to_string()
    );
    assert_eq!(
        encoder
            .finish(NonZeroU64::new(1).unwrap())
            .unwrap_err()
            .to_string(),
        error.to_string()
    );
}

fn refuses_path_search() {
    let request = Request {
        settings: Settings::new(64, 64, (30_000, 1001), 8, 32, Color::default()).unwrap(),
        limits: Limits::new(20_000, 1 << 20, 10, 4096).unwrap(),
    };
    let (_temporary, file) = Temporary::new();
    let result = Encoder::spawn(
        std::path::Path::new("coreav"),
        request,
        retention(request),
        file,
        Launch {
            launcher: None,
            limits: EncoderLimits::new(1 << 30),
            deadline: Instant::now() + Duration::from_secs(10),
            pixel_aspect: None,
        },
    );
    assert_eq!(result.err().unwrap().kind, ErrorKind::InvalidData);
}

fn start_timed(origin: i64, change: &str) -> (Temporary, Result<Encoder, coreav_decode::Error>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let packets: Vec<_> = [0, 3003, 6006, 9009]
        .into_iter()
        .map(|offset| StreamPacket {
            offset: 0,
            size: 0,
            pts: origin + offset,
            time: None,
            dts: None,
            duration: Some(3003),
            keyframe: false,
            presented: true,
        })
        .collect();
    let source =
        Timeline::new(&packets, Rational::new(1, 90_000).unwrap(), 4, 32, deadline).unwrap();
    let rate = match change {
        "cadence" => (30, 1),
        "equivalent" => (60_000, 2002),
        _ => (30_000, 1001),
    };
    let request = Request {
        settings: Settings::new(
            64,
            64,
            rate,
            8,
            if change == "equivalent" { 41 } else { 40 },
            Color::default(),
        )
        .unwrap(),
        limits: Limits::new(
            20_000,
            1 << 20,
            if change == "count" { 3 } else { 10 },
            4096,
        )
        .unwrap(),
    };
    let launcher = ExecLauncher::current_exe().unwrap();
    let (temporary, file) = Temporary::new();
    let encoder = Encoder::spawn_timed(
        &std::env::current_exe().unwrap(),
        request,
        retention(request),
        file,
        Launch {
            launcher: Some(&launcher),
            limits: EncoderLimits::new(1 << 30),
            deadline: if change == "deadline" {
                deadline + Duration::from_millis(1)
            } else {
                deadline
            },
            pixel_aspect: None,
        },
        source,
    );
    (temporary, encoder)
}

fn timed_source_clocks() {
    for origin in [-90_000, 90_000] {
        for change in ["none", "equivalent"] {
            let (_temporary, encoder) = start_timed(origin, change);
            let mut encoder = encoder.unwrap();
            for (index, offset) in [0, 3003, 6006, 9009].into_iter().enumerate() {
                encoder
                    .send_frame(picture(index as u8), origin + offset)
                    .unwrap();
            }
            let result = encoder.finish(NonZeroU64::new(3003).unwrap()).unwrap();
            assert_eq!(result.packets.len(), 4);
            for (index, (_, time)) in result.packets.iter().enumerate() {
                assert_eq!(time.pts, origin + [0, 3003, 6006, 9009][index]);
                assert_eq!(time.dts, time.pts);
                assert_eq!(time.duration, 3003);
            }
        }
    }
}

fn timed_refusals() {
    for (change, expected) in [
        ("cadence", ErrorKind::Unsupported),
        ("count", ErrorKind::Limit),
        ("deadline", ErrorKind::InvalidData),
    ] {
        let (temporary, result) = start_timed(0, change);
        assert_eq!(result.err().unwrap().kind, expected);
        assert_eq!(std::fs::metadata(&temporary.0).unwrap().len(), 0);
    }
    let (_temporary, encoder) = start_timed(0, "none");
    let mut encoder = encoder.unwrap();
    encoder.send_frame(picture(0), 0).unwrap();
    let error = encoder.send_frame(picture(1), 3004).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidData);
    assert_eq!(
        encoder
            .send_frame(picture(1), 3003)
            .unwrap_err()
            .to_string(),
        error.to_string()
    );
    assert_eq!(
        encoder
            .finish(NonZeroU64::new(3003).unwrap())
            .unwrap_err()
            .to_string(),
        error.to_string()
    );
    for change in ["missing", "duration", "extra"] {
        let (_temporary, encoder) = start_timed(0, "none");
        let mut encoder = encoder.unwrap();
        for (index, offset) in [0, 3003, 6006, 9009]
            .into_iter()
            .enumerate()
            .take(if change == "missing" { 3 } else { 4 })
        {
            encoder.send_frame(picture(index as u8), offset).unwrap();
        }
        if change == "extra" {
            assert_eq!(
                encoder.send_frame(picture(0), 12012).unwrap_err().kind,
                ErrorKind::InvalidData
            );
        }
        assert_eq!(
            encoder
                .finish(NonZeroU64::new(if change == "duration" { 3004 } else { 3003 }).unwrap())
                .unwrap_err()
                .kind,
            ErrorKind::InvalidData
        );
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new(coreav_worker::EXEC_ARG)) {
        return coreav_worker::sandbox_exec_main(args, |_| Ok(()));
    }
    if first.as_deref() == Some(std::ffi::OsStr::new(WORKER_ARG)) {
        return worker();
    }
    if !cfg!(unix) || std::env::args().any(|arg| arg == "--list") {
        return ExitCode::SUCCESS;
    }
    for (name, run) in [
        ("complete source clocks and packed planes", complete as fn()),
        ("bad wire and process exits", bad_output as fn()),
        ("terminal input refusals", bad_input as fn()),
        (
            "deadline after physical input EOF",
            hangs_after_input as fn(),
        ),
        (
            "deadline in a stalled picture write",
            never_reads_pictures as fn(),
        ),
        (
            "private worker never searches PATH",
            refuses_path_search as fn(),
        ),
        (
            "bound source timeline and exact cadence",
            timed_source_clocks as fn(),
        ),
        ("terminal source timeline refusals", timed_refusals as fn()),
    ] {
        run();
        println!("test {name} ... ok");
    }
    ExitCode::SUCCESS
}
