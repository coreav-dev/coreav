//! CoreAV's decode boundary (`docs/design/decode.md`): the [`Decoder`]
//! trait that every backend implements, the stream description it starts
//! from ([`StreamConfig`]), the codec allowlist, decode [`Limits`], and the
//! selection of a backend per stream ([`open`]).
//!
//! The boundary is at the compressed-packet level: packets in decode
//! order in, [`Frame`]s in presentation order out. No container crate is
//! a dependency; callers describe a container's track as a
//! [`StreamConfig`].
//!
//! ```
//! use coreav_decode::{Codec, DecodeAllowlist, ErrorKind, Limits, Policy, StreamConfig, open};
//!
//! // Theora is a long-tail codec: not decoded unless an operator allows it.
//! let stream = StreamConfig::new(Codec::Theora);
//! let policy = Policy::default();
//! let Err(error) = open(&stream, &policy, &Limits::default(), &[]) else {
//!     unreachable!()
//! };
//! assert_eq!(error.kind, ErrorKind::CodecNotAllowed);
//! assert!(!DecodeAllowlist::default().allows(Codec::Theora));
//! ```

#![forbid(unsafe_code)]

mod error;
mod stream;

use std::time::{Duration, Instant};

pub use coreav_frame::{Frame, PixelFormat};
pub use error::{Error, ErrorKind};
pub use stream::{Codec, CodecConfig, ConfigKind, Packet, StreamConfig, StreamTraits};

/// A decoder: packets in, frames out (`docs/design/decode.md`, "The
/// `Decoder` trait").
pub trait Decoder: Send {
    /// Sends one packet, in decode order.
    fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), Error>;

    /// Signals that no packet follows: the decoder outputs every frame it
    /// holds.
    fn send_eof(&mut self) -> Result<(), Error>;

    /// The next frame in presentation order. Before [`Decoder::send_eof`],
    /// `None` when no frame is ready, without waiting; after it, waits for
    /// the next frame and returns `None` once every frame is out.
    fn receive_frame(&mut self) -> Result<Option<Frame>, Error>;

    /// Which backend this is.
    fn info(&self) -> &DecoderInfo;
}

/// A kind of backend, in the default order of preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BackendKind {
    /// NVIDIA's hardware decoder, driven from CoreAV's own header parsing.
    Nvdec,
    /// A CoreAV-native decoder (Phase 2, Track N).
    Native,
    /// dav1d, for AV1.
    Dav1d,
    /// FFmpeg's decoders, run in the sandboxed worker.
    Fallback,
}

impl BackendKind {
    /// A stable name, for JSON output and options.
    pub fn name(self) -> &'static str {
        match self {
            Self::Nvdec => "nvdec",
            Self::Native => "native",
            Self::Dav1d => "dav1d",
            Self::Fallback => "ffmpeg",
        }
    }

    /// The kind named `name` ([`BackendKind::name`]).
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Nvdec, Self::Native, Self::Dav1d, Self::Fallback]
            .into_iter()
            .find(|k| k.name() == name)
    }
}

/// Where a backend's code runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Isolation {
    /// In the calling process.
    InProcess,
    /// In a sandboxed worker process.
    Sandboxed,
}

/// What a decoder reports about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecoderInfo {
    /// The backend.
    pub backend: BackendKind,
    /// Where it runs.
    pub isolation: Isolation,
    /// The decoder within the backend, for people (`h264`, `dav1d 1.5.1`).
    pub decoder: String,
}

impl DecoderInfo {
    /// Whether this is the FFmpeg fallback: C decoders, sandboxed
    /// (section 5.4, "whether it fell back").
    pub fn fell_back(&self) -> bool {
        self.backend == BackendKind::Fallback
    }
}

/// The codecs that may be decoded at all (`docs/design/decode.md`, "Codec
/// allowlist and limits").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeAllowlist {
    codecs: Vec<Codec>,
}

impl DecodeAllowlist {
    /// Exactly these codecs.
    pub fn new(codecs: &[Codec]) -> Self {
        Self {
            codecs: codecs.to_vec(),
        }
    }

    /// Every codec the boundary knows, long-tail ones included.
    pub fn all() -> Self {
        Self::new(&Codec::ALL)
    }

    /// Whether `codec` may be decoded.
    pub fn allows(&self, codec: Codec) -> bool {
        self.codecs.contains(&codec)
    }

    /// The allowed codecs.
    pub fn codecs(&self) -> &[Codec] {
        &self.codecs
    }
}

impl Default for DecodeAllowlist {
    /// The mainstream codecs: H.264, HEVC, AV1, VP9, VP8 and ProRes.
    fn default() -> Self {
        Self::new(&[
            Codec::H264,
            Codec::Hevc,
            Codec::Av1,
            Codec::Vp9,
            Codec::Vp8,
            Codec::ProRes,
        ])
    }
}

/// Decode limits (`docs/design/decode.md`), checked on every
/// frame whatever the backend says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest frame width.
    pub max_width: u32,
    /// Largest frame height.
    pub max_height: u32,
    /// Largest width × height.
    pub max_pixels: u64,
    /// Largest frame in bytes, every plane packed.
    pub max_frame_bytes: u64,
    /// Bytes of every frame a decoder outputs, together.
    pub max_decoded_bytes: u64,
    /// Bytes of frames decoded but not yet received.
    pub max_pending_bytes: u64,
    /// Largest packet.
    pub max_packet_bytes: u64,
    /// Wall-clock time from opening a decoder to its last frame.
    pub timeout: Duration,
    /// Most reference frames a stream may keep (NVDEC's DPB size).
    pub max_reference_frames: u32,
    /// Memory each process of a sandboxed decoder may use (its address
    /// space): the worker, and each process it starts. A backend fits its
    /// work to it (fewer threads) or refuses a stream it cannot fit with
    /// `Limit`; running into it fails with `Limit`, never an out-of-memory
    /// kill of the host.
    pub max_memory_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_width: 16_384,
            max_height: 16_384,
            max_pixels: 1 << 27,
            max_frame_bytes: 512 << 20,
            max_decoded_bytes: 4 << 30,
            max_pending_bytes: 1 << 30,
            max_packet_bytes: 128 << 20,
            timeout: Duration::from_secs(60),
            max_reference_frames: 16,
            max_memory_bytes: 8 << 30,
        }
    }
}

impl Limits {
    /// The instant [`Limits::timeout`] from now. A timeout too long to
    /// represent counts as about a century.
    pub fn deadline(&self) -> Instant {
        let now = Instant::now();
        now.checked_add(self.timeout)
            .or_else(|| now.checked_add(Duration::from_secs(100 * 365 * 86_400)))
            .unwrap_or(now)
    }

    /// Checks a frame's size against the limits.
    pub fn check_frame(&self, width: u32, height: u32, bytes: u64) -> Result<(), Error> {
        if width == 0 || height == 0 {
            return Err(Error::new(ErrorKind::InvalidData, "frame of size 0"));
        }
        if width > self.max_width
            || height > self.max_height
            || u64::from(width) * u64::from(height) > self.max_pixels
        {
            return Err(Error::new(
                ErrorKind::Limit,
                format!("frame of {width}x{height} exceeds the decode limits"),
            ));
        }
        if bytes > self.max_frame_bytes {
            return Err(Error::new(
                ErrorKind::Limit,
                format!("frame of {bytes} bytes exceeds the decode limits"),
            ));
        }
        Ok(())
    }

    /// Checks a packet's size.
    pub fn check_packet(&self, packet: &Packet<'_>) -> Result<(), Error> {
        if u64::try_from(packet.data.len()).unwrap_or(u64::MAX) > self.max_packet_bytes {
            return Err(Error::new(
                ErrorKind::Limit,
                "packet exceeds the decode limits",
            ));
        }
        Ok(())
    }
}

/// Whether a backend can decode a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Support {
    /// It can.
    Yes,
    /// It cannot, and why.
    No(String),
}

/// A backend: says which streams it supports and opens decoders for them.
pub trait Backend: Send + Sync {
    /// Its kind.
    fn kind(&self) -> BackendKind;

    /// Whether it supports `stream` (`traits` is `stream.traits()`).
    fn supports(&self, stream: &StreamConfig, traits: &StreamTraits) -> Support;

    /// Opens a decoder for `stream`.
    fn open(&self, stream: &StreamConfig, limits: &Limits) -> Result<Box<dyn Decoder>, Error>;
}

/// Which codecs may be decoded and in which order backends are tried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Codecs that may be decoded.
    pub allowlist: DecodeAllowlist,
    /// Backends to try, in order; a backend not listed is never used.
    pub order: Vec<BackendKind>,
}

impl Default for Policy {
    /// The default allowlist; NVDEC, native, dav1d, then the fallback.
    fn default() -> Self {
        Self {
            allowlist: DecodeAllowlist::default(),
            order: vec![
                BackendKind::Nvdec,
                BackendKind::Native,
                BackendKind::Dav1d,
                BackendKind::Fallback,
            ],
        }
    }
}

/// A backend that was not used, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// The backend.
    pub backend: BackendKind,
    /// Why: it does not support the stream, it failed to open, or it is
    /// not available in this build or on this host.
    pub reason: String,
}

/// A decoder opened by [`open`], with the backends tried before it.
pub struct Opened {
    /// The decoder.
    pub decoder: Box<dyn Decoder>,
    /// Backends earlier in the policy's order that were not used.
    pub skipped: Vec<Skipped>,
}

/// Opens a decoder for `stream`: the first backend in the policy's order
/// that is among `backends`, supports the stream and opens
/// (`docs/design/decode.md`, "Selection").
pub fn open(
    stream: &StreamConfig,
    policy: &Policy,
    limits: &Limits,
    backends: &[&dyn Backend],
) -> Result<Opened, Error> {
    if !policy.allowlist.allows(stream.codec) {
        return Err(Error::new(
            ErrorKind::CodecNotAllowed,
            format!(
                "codec {} is not on the decode allowlist",
                stream.codec.name()
            ),
        ));
    }
    let traits = stream.traits();
    let mut skipped = Vec::new();
    // A backend that refused the stream for a limit: if no other opens it,
    // that is the answer, not "no decoder".
    let mut limited = None;
    for kind in &policy.order {
        let candidates: Vec<&&dyn Backend> =
            backends.iter().filter(|b| b.kind() == *kind).collect();
        if candidates.is_empty() {
            skipped.push(Skipped {
                backend: *kind,
                reason: "not available".into(),
            });
            continue;
        }
        for backend in candidates {
            match backend.supports(stream, &traits) {
                Support::No(reason) => skipped.push(Skipped {
                    backend: *kind,
                    reason,
                }),
                Support::Yes => match backend.open(stream, limits) {
                    Ok(decoder) => return Ok(Opened { decoder, skipped }),
                    Err(e) => {
                        skipped.push(Skipped {
                            backend: *kind,
                            reason: e.to_string(),
                        });
                        if e.kind == ErrorKind::Limit && limited.is_none() {
                            limited = Some(e);
                        }
                    }
                },
            }
        }
    }
    let reasons: Vec<String> = skipped
        .iter()
        .map(|s| format!("{}: {}", s.backend.name(), s.reason))
        .collect();
    if let Some(e) = limited {
        // The other backends' reasons beside it.
        let own = e.to_string();
        let others: Vec<String> = skipped
            .iter()
            .filter(|s| s.reason != own)
            .map(|s| format!("{}: {}", s.backend.name(), s.reason))
            .collect();
        let message = if others.is_empty() {
            e.message
        } else {
            format!("{} ({})", e.message, others.join("; "))
        };
        return Err(Error::new(ErrorKind::Limit, message));
    }
    Err(Error::new(
        ErrorKind::Unsupported,
        format!(
            "no decoder for {}: {}",
            stream.codec.name(),
            reasons.join("; ")
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        kind: BackendKind,
        support: bool,
        opens: bool,
    }

    struct Null(DecoderInfo);

    impl Decoder for Null {
        fn send_packet(&mut self, _: &Packet<'_>) -> Result<(), Error> {
            Ok(())
        }
        fn send_eof(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn receive_frame(&mut self) -> Result<Option<Frame>, Error> {
            Ok(None)
        }
        fn info(&self) -> &DecoderInfo {
            &self.0
        }
    }

    impl Backend for Fake {
        fn kind(&self) -> BackendKind {
            self.kind
        }
        fn supports(&self, _: &StreamConfig, _: &StreamTraits) -> Support {
            if self.support {
                Support::Yes
            } else {
                Support::No("no".into())
            }
        }
        fn open(&self, _: &StreamConfig, _: &Limits) -> Result<Box<dyn Decoder>, Error> {
            if self.opens {
                Ok(Box::new(Null(DecoderInfo {
                    backend: self.kind,
                    isolation: Isolation::InProcess,
                    decoder: "fake".into(),
                })))
            } else {
                Err(Error::new(ErrorKind::Backend, "failed"))
            }
        }
    }

    /// A backend that refuses every stream for a limit.
    struct Limited(BackendKind);

    impl Backend for Limited {
        fn kind(&self) -> BackendKind {
            self.0
        }
        fn supports(&self, _: &StreamConfig, _: &StreamTraits) -> Support {
            Support::Yes
        }
        fn open(&self, _: &StreamConfig, _: &Limits) -> Result<Box<dyn Decoder>, Error> {
            Err(Error::new(ErrorKind::Limit, "over the memory limit"))
        }
    }

    #[test]
    fn a_limit_is_the_answer_when_no_backend_opens() {
        let stream = StreamConfig::new(Codec::Av1);
        let limited = Limited(BackendKind::Nvdec);
        let fallback = Fake {
            kind: BackendKind::Fallback,
            support: true,
            opens: true,
        };
        // Another backend that opens the stream still does.
        let opened = open(
            &stream,
            &Policy::default(),
            &Limits::default(),
            &[&limited, &fallback],
        )
        .expect("opened");
        assert_eq!(opened.decoder.info().backend, BackendKind::Fallback);
        // None does: the limit, not "no decoder".
        let failing = Fake {
            kind: BackendKind::Fallback,
            support: true,
            opens: false,
        };
        let refused = open(
            &stream,
            &Policy::default(),
            &Limits::default(),
            &[&limited, &failing],
        );
        assert_eq!(refused.err().map(|e| e.kind), Some(ErrorKind::Limit));
    }

    #[test]
    fn order_support_and_failures() {
        let nvdec = Fake {
            kind: BackendKind::Nvdec,
            support: true,
            opens: false,
        };
        let dav1d = Fake {
            kind: BackendKind::Dav1d,
            support: false,
            opens: true,
        };
        let fallback = Fake {
            kind: BackendKind::Fallback,
            support: true,
            opens: true,
        };
        let stream = StreamConfig::new(Codec::Av1);
        let opened = open(
            &stream,
            &Policy::default(),
            &Limits::default(),
            &[&fallback, &dav1d, &nvdec],
        )
        .expect("opened");
        assert_eq!(opened.decoder.info().backend, BackendKind::Fallback);
        assert!(opened.decoder.info().fell_back());
        let skipped: Vec<BackendKind> = opened.skipped.iter().map(|s| s.backend).collect();
        assert_eq!(
            skipped,
            vec![BackendKind::Nvdec, BackendKind::Native, BackendKind::Dav1d]
        );
        let none = open(
            &stream,
            &Policy {
                order: vec![BackendKind::Dav1d],
                ..Policy::default()
            },
            &Limits::default(),
            &[&fallback, &dav1d],
        );
        assert_eq!(none.err().map(|e| e.kind), Some(ErrorKind::Unsupported));
    }

    #[test]
    fn limits() {
        let l = Limits::default();
        assert!(l.check_frame(1920, 1080, 3_110_400).is_ok());
        assert!(l.check_frame(16_385, 2, 10).is_err());
        assert!(l.check_frame(16_384, 16_384, 10).is_err());
        assert!(l.check_frame(0, 5, 0).is_err());
        assert_eq!(Codec::from_name("prores"), Some(Codec::ProRes));
        assert_eq!(
            BackendKind::from_name("ffmpeg"),
            Some(BackendKind::Fallback)
        );
    }
}
