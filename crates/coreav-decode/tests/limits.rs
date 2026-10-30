//! Every decode limit that this crate checks itself is pinned at its
//! boundary, and the defaults are the documented values
//! (`docs/design/decode.md`, "Codec allowlist and limits"): the frame and
//! packet limits, which `open` applies to every decoder it opens whatever
//! the backend checks. The limits a backend enforces are not here: `open`
//! hands them to it unchanged, and each backend's own tests pin what it
//! does with them (the native H.264 backend's in
//! `crates/coreav-backend-h264/tests/limits.rs`).
#![allow(clippy::expect_used)]
#![forbid(unsafe_code)]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use coreav_decode::{
    Backend, BackendKind, Codec, Decoder, Error, ErrorKind, Frame, Limits, Packet, PixelFormat,
    Policy, StreamConfig, StreamTraits, Support, open,
};
use coreav_frame::{ColorInfo, Picture, Pixels};

fn kind(result: Result<(), Error>) -> Option<ErrorKind> {
    result.err().map(|e| e.kind)
}

/// Limits that refuse nothing, so that a test sets the one it pins.
fn roomy() -> Limits {
    Limits {
        max_width: u32::MAX,
        max_height: u32::MAX,
        max_pixels: u64::MAX,
        max_frame_bytes: u64::MAX,
        max_packet_bytes: u64::MAX,
        ..Limits::default()
    }
}

fn packet(data: &[u8]) -> Packet<'_> {
    Packet {
        data,
        pts: Some(0),
        dts: Some(0),
        keyframe: true,
        discard: false,
    }
}

#[test]
fn the_default_limits_are_the_documented_values() {
    assert_eq!(
        Limits::default(),
        Limits {
            max_width: 16_384,
            max_height: 16_384,
            max_pixels: 134_217_728,
            max_frame_bytes: 512 << 20,
            max_decoded_bytes: 4 << 30,
            max_pending_bytes: 1 << 30,
            max_packet_bytes: 128 << 20,
            timeout: Duration::from_secs(60),
            max_reference_frames: 16,
            max_memory_bytes: 8 << 30,
        }
    );
}

#[test]
fn a_frame_of_max_width_passes_and_one_wider_is_refused() {
    let limits = Limits {
        max_width: 640,
        ..roomy()
    };
    assert_eq!(kind(limits.check_frame(640, 1, 1)), None);
    assert_eq!(kind(limits.check_frame(641, 1, 1)), Some(ErrorKind::Limit));
}

#[test]
fn a_frame_of_max_height_passes_and_one_taller_is_refused() {
    let limits = Limits {
        max_height: 480,
        ..roomy()
    };
    assert_eq!(kind(limits.check_frame(1, 480, 1)), None);
    assert_eq!(kind(limits.check_frame(1, 481, 1)), Some(ErrorKind::Limit));
}

#[test]
fn a_frame_of_max_pixels_passes_and_one_more_pixel_is_refused() {
    let limits = Limits {
        max_pixels: 10_000,
        ..roomy()
    };
    assert_eq!(kind(limits.check_frame(100, 100, 1)), None);
    assert_eq!(kind(limits.check_frame(10_000, 1, 1)), None);
    assert_eq!(
        kind(limits.check_frame(100, 101, 1)),
        Some(ErrorKind::Limit)
    );
    assert_eq!(
        kind(limits.check_frame(10_001, 1, 1)),
        Some(ErrorKind::Limit)
    );
}

#[test]
fn a_frame_of_max_frame_bytes_passes_and_one_more_byte_is_refused() {
    let limits = Limits {
        max_frame_bytes: 1_000,
        ..roomy()
    };
    assert_eq!(kind(limits.check_frame(10, 10, 1_000)), None);
    assert_eq!(
        kind(limits.check_frame(10, 10, 1_001)),
        Some(ErrorKind::Limit)
    );
}

#[test]
fn a_frame_of_no_pixels_is_invalid_data_not_a_limit() {
    let limits = roomy();
    assert_eq!(
        kind(limits.check_frame(0, 5, 0)),
        Some(ErrorKind::InvalidData)
    );
    assert_eq!(
        kind(limits.check_frame(5, 0, 0)),
        Some(ErrorKind::InvalidData)
    );
}

#[test]
fn a_packet_of_max_packet_bytes_passes_and_one_more_byte_is_refused() {
    let limits = Limits {
        max_packet_bytes: 8,
        ..roomy()
    };
    assert_eq!(kind(limits.check_packet(&packet(&[0; 8]))), None);
    assert_eq!(
        kind(limits.check_packet(&packet(&[0; 9]))),
        Some(ErrorKind::Limit)
    );
}

#[test]
fn the_deadline_is_the_timeout_after_the_call() {
    let limits = Limits {
        timeout: Duration::from_secs(30),
        ..Limits::default()
    };
    let before = Instant::now();
    let deadline = limits.deadline();
    let after = Instant::now();
    assert!(deadline >= before + limits.timeout);
    assert!(deadline <= after + limits.timeout);
}

#[test]
fn a_timeout_of_zero_has_passed_at_once() {
    let limits = Limits {
        timeout: Duration::ZERO,
        ..Limits::default()
    };
    let deadline = limits.deadline();
    assert!(Instant::now() >= deadline);
}

#[test]
fn a_timeout_too_long_to_represent_counts_as_about_a_century() {
    let limits = Limits {
        timeout: Duration::MAX,
        ..Limits::default()
    };
    let before = Instant::now();
    let deadline = limits.deadline();
    let years = |n: u64| Duration::from_secs(n * 365 * 86_400);
    assert!(deadline >= before + years(99));
    assert!(deadline <= before + years(101));
}

struct Null(coreav_decode::DecoderInfo);

impl Decoder for Null {
    fn send_packet(&mut self, _: &Packet<'_>) -> Result<(), Error> {
        Ok(())
    }
    fn send_eof(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn receive_frame(&mut self) -> Result<Option<coreav_decode::Frame>, Error> {
        Ok(None)
    }
    fn info(&self) -> &coreav_decode::DecoderInfo {
        &self.0
    }
}

struct Recording(Mutex<Vec<Limits>>);

impl Backend for Recording {
    fn kind(&self) -> BackendKind {
        BackendKind::Native
    }
    fn supports(&self, _: &StreamConfig, _: &StreamTraits) -> Support {
        Support::Yes
    }
    fn open(&self, _: &StreamConfig, limits: &Limits) -> Result<Box<dyn Decoder>, Error> {
        self.0.lock().expect("lock").push(limits.clone());
        Ok(Box::new(Null(coreav_decode::DecoderInfo {
            backend: BackendKind::Native,
            isolation: coreav_decode::Isolation::InProcess,
            decoder: "recording".into(),
        })))
    }
}

#[test]
fn open_hands_every_limit_to_the_backend_unchanged() {
    let limits = Limits {
        max_width: 1,
        max_height: 2,
        max_pixels: 3,
        max_frame_bytes: 4,
        max_decoded_bytes: 5,
        max_pending_bytes: 6,
        max_packet_bytes: 7,
        timeout: Duration::from_millis(8),
        max_reference_frames: 9,
        max_memory_bytes: 10,
    };
    let backend = Recording(Mutex::new(Vec::new()));
    open(
        &StreamConfig::new(Codec::H264),
        &Policy::default(),
        &limits,
        &[&backend],
    )
    .expect("opened");
    assert_eq!(*backend.0.lock().expect("lock"), vec![limits]);
}

/// A backend that checks no limit: it takes every packet and returns one
/// 4:2:0 frame of its size for each.
struct Careless {
    width: u32,
    height: u32,
}

struct CarelessDecoder {
    width: u32,
    height: u32,
    ready: usize,
    info: coreav_decode::DecoderInfo,
}

impl Decoder for CarelessDecoder {
    fn send_packet(&mut self, _: &Packet<'_>) -> Result<(), Error> {
        self.ready += 1;
        Ok(())
    }
    fn send_eof(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn receive_frame(&mut self) -> Result<Option<Frame>, Error> {
        if self.ready == 0 {
            return Ok(None);
        }
        self.ready -= 1;
        let picture =
            Picture::zeroed(self.width, self.height, PixelFormat::YUV420P).expect("a picture");
        Ok(Some(Frame {
            pts: Some(0),
            color: ColorInfo::default(),
            pixels: Pixels::Cpu(picture),
        }))
    }
    fn info(&self) -> &coreav_decode::DecoderInfo {
        &self.info
    }
}

impl Backend for Careless {
    fn kind(&self) -> BackendKind {
        BackendKind::Native
    }
    fn supports(&self, _: &StreamConfig, _: &StreamTraits) -> Support {
        Support::Yes
    }
    fn open(&self, _: &StreamConfig, _: &Limits) -> Result<Box<dyn Decoder>, Error> {
        Ok(Box::new(CarelessDecoder {
            width: self.width,
            height: self.height,
            ready: 0,
            info: coreav_decode::DecoderInfo {
                backend: BackendKind::Native,
                isolation: coreav_decode::Isolation::InProcess,
                decoder: "careless".into(),
            },
        }))
    }
}

/// Sends one packet of `bytes` bytes to a decoder `open` gave for
/// `backend`, and receives its frame.
fn through_open(backend: &Careless, limits: &Limits, bytes: usize) -> Result<(), Error> {
    let opened = open(
        &StreamConfig::new(Codec::H264),
        &Policy::default(),
        limits,
        &[backend],
    )?;
    let mut decoder = opened.decoder;
    decoder.send_packet(&packet(&vec![0; bytes]))?;
    decoder.receive_frame()?.expect("a frame");
    Ok(())
}

#[test]
fn open_refuses_a_packet_past_max_packet_bytes_that_the_backend_takes() {
    let backend = Careless {
        width: 16,
        height: 16,
    };
    let limits = Limits {
        max_packet_bytes: 8,
        ..roomy()
    };
    assert_eq!(kind(through_open(&backend, &limits, 8)), None);
    assert_eq!(
        kind(through_open(&backend, &limits, 9)),
        Some(ErrorKind::Limit)
    );
}

#[test]
fn open_refuses_a_frame_past_the_frame_limits_that_the_backend_returns() {
    let backend = Careless {
        width: 64,
        height: 48,
    };
    let bytes = 64 * 48 + 2 * 32 * 24;
    let cases = [
        (
            "max_width",
            Limits {
                max_width: 64,
                ..roomy()
            },
            Limits {
                max_width: 63,
                ..roomy()
            },
        ),
        (
            "max_height",
            Limits {
                max_height: 48,
                ..roomy()
            },
            Limits {
                max_height: 47,
                ..roomy()
            },
        ),
        (
            "max_pixels",
            Limits {
                max_pixels: 64 * 48,
                ..roomy()
            },
            Limits {
                max_pixels: 64 * 48 - 1,
                ..roomy()
            },
        ),
        (
            "max_frame_bytes",
            Limits {
                max_frame_bytes: bytes,
                ..roomy()
            },
            Limits {
                max_frame_bytes: bytes - 1,
                ..roomy()
            },
        ),
    ];
    for (name, at, past) in cases {
        assert_eq!(kind(through_open(&backend, &at, 1)), None, "{name}");
        assert_eq!(
            kind(through_open(&backend, &past, 1)),
            Some(ErrorKind::Limit),
            "{name}"
        );
    }
}
