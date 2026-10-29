//! The control protocol between the parent and the worker
//! (`docs/design/decode.md`, "The sandboxed worker"): length-prefixed
//! binary messages over the worker's standard input and output.
//!
//! Every message is a 4-byte little-endian length, then that many bytes: a
//! type byte and the payload. Integers are little-endian. The parent reads
//! the worker's messages with [`read_message`] under a size limit and
//! decodes them with [`WorkerMessage::decode`]; neither trusts a byte.

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};
use std::time::Duration;

use coreav_decode::{Codec, CodecConfig, ConfigKind, ErrorKind, Limits, StreamConfig};
use coreav_frame::{Chroma, ChromaLocation, ColorInfo, PixelFormat};

/// Largest worker message other than a frame sent inline.
pub const MAX_CONTROL_MESSAGE: usize = 4096;
/// Bytes of an error message kept.
pub const MAX_ERROR_TEXT: usize = 1024;
/// Most planes a frame descriptor may list.
pub const MAX_PLANES: usize = 4;
/// Bytes read at a time from a message.
const CHUNK: usize = 1 << 20;
/// The `slot` of a frame whose planes follow its descriptor in the same
/// message instead of being in shared memory.
pub const INLINE: u32 = u32::MAX;

// Parent to worker.
const OPEN: u8 = 0x01;
const PACKET: u8 = 0x02;
const EOF: u8 = 0x03;
const ACK: u8 = 0x04;
const CLOSE: u8 = 0x05;
// Worker to parent.
const READY: u8 = 0x81;
const FRAME: u8 = 0x82;
const DONE: u8 = 0x83;
const ERROR: u8 = 0x84;

/// A protocol violation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolError(pub &'static str);

/// Writes one message.
pub fn write_message(out: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len() + 1)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "message too long"))?;
    let [a, b, c, d] = len.to_le_bytes();
    out.write_all(&[a, b, c, d, kind])?;
    out.write_all(payload)?;
    out.flush()
}

/// What [`read_message`] read.
#[derive(Debug)]
pub enum Incoming {
    /// A message: its type and payload.
    Message(u8, Vec<u8>),
    /// The stream ended between messages.
    End,
}

/// Reads one message whose length is at most `max(type)` bytes: the limit
/// is chosen after the type byte, so that only frames sent inline may be
/// large. A length over the limit, a stream that ends inside a message or
/// memory that cannot be reserved is an error.
pub fn read_message(
    input: &mut impl Read,
    max: impl Fn(u8) -> usize,
) -> Result<Incoming, ProtocolError> {
    let mut head = [0u8; 5];
    let mut got = 0;
    while got < head.len() {
        let rest = head.get_mut(got..).ok_or(ProtocolError("header"))?;
        match input.read(rest) {
            Ok(0) if got == 0 => return Ok(Incoming::End),
            Ok(0) => return Err(ProtocolError("stream ended inside a message")),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(ProtocolError("read failed")),
        }
    }
    let [a, b, c, d, kind] = head;
    let len = u32::from_le_bytes([a, b, c, d]);
    let len = usize::try_from(len).map_err(|_| ProtocolError("length"))?;
    if len == 0 || len > max(kind).saturating_add(1) {
        return Err(ProtocolError("message length outside the limit"));
    }
    // Read in chunks, reserving as bytes arrive: a sender that announces a
    // large message makes the reader hold only what it actually sent.
    let want = len - 1;
    let mut payload = Vec::new();
    while payload.len() < want {
        let chunk = (want - payload.len()).min(CHUNK);
        let start = payload.len();
        payload
            .try_reserve(chunk)
            .map_err(|_| ProtocolError("out of memory for a message"))?;
        payload.resize(start + chunk, 0);
        let target = payload.get_mut(start..).ok_or(ProtocolError("chunk"))?;
        input
            .read_exact(target)
            .map_err(|_| ProtocolError("stream ended inside a message"))?;
    }
    Ok(Incoming::Message(kind, payload))
}

/// A cursor over a payload.
pub(crate) struct Wire<'a> {
    data: &'a [u8],
}

impl<'a> Wire<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtocolError> {
        let (head, tail) = self
            .data
            .split_at_checked(n)
            .ok_or(ProtocolError("payload too short"))?;
        self.data = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.take(1)?.first().copied().unwrap_or(0))
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes(
            b.try_into().map_err(|_| ProtocolError("u32"))?,
        ))
    }

    fn u64(&mut self) -> Result<u64, ProtocolError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(
            b.try_into().map_err(|_| ProtocolError("u64"))?,
        ))
    }

    fn i64(&mut self) -> Result<i64, ProtocolError> {
        let b = self.take(8)?;
        Ok(i64::from_le_bytes(
            b.try_into().map_err(|_| ProtocolError("i64"))?,
        ))
    }

    fn bytes(&mut self, max: usize) -> Result<&'a [u8], ProtocolError> {
        let len = usize::try_from(self.u32()?).map_err(|_| ProtocolError("length"))?;
        if len > max {
            return Err(ProtocolError("field too long"));
        }
        self.take(len)
    }

    pub(crate) fn rest(self) -> &'a [u8] {
        self.data
    }

    fn end(&self) -> Result<(), ProtocolError> {
        if self.data.is_empty() {
            Ok(())
        } else {
            Err(ProtocolError("trailing bytes"))
        }
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_u32(out, u32::try_from(b.len()).unwrap_or(u32::MAX));
    out.extend_from_slice(b);
}

/// Which backend the worker hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkerBackend {
    /// The FFmpeg fallback (`coreav-backend-ffmpeg`).
    Ffmpeg,
    /// dav1d (`coreav-backend-dav1d`).
    Dav1d,
    /// libvpx (`coreav-backend-libvpx`).
    Libvpx,
}

impl WorkerBackend {
    fn code(self) -> u8 {
        match self {
            Self::Ffmpeg => 1,
            Self::Dav1d => 2,
            Self::Libvpx => 3,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Ffmpeg),
            2 => Some(Self::Dav1d),
            3 => Some(Self::Libvpx),
            _ => None,
        }
    }
}

/// How frames travel from the worker to the parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transfer {
    /// Every frame inline, in its descriptor's message.
    Pipe,
    /// `slots` slots of `slot_size` bytes in the memory file the worker
    /// opens at `path`; a frame larger than a slot goes inline.
    Shared {
        /// Where the worker opens the parent's memory file.
        path: String,
        /// Number of slots.
        slots: u32,
        /// Bytes per slot.
        slot_size: u64,
    },
}

/// Resource limits the worker sets on itself before it starts a decoder
/// (`docs/design/decode.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxLimits {
    /// Address space in bytes (`RLIMIT_AS`).
    pub address_space: Option<u64>,
    /// CPU seconds (`RLIMIT_CPU`).
    pub cpu_seconds: Option<u64>,
    /// Open file descriptors (`RLIMIT_NOFILE`).
    pub open_files: Option<u64>,
}

/// The first message: what to decode and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    /// The backend to host.
    pub backend: WorkerBackend,
    /// The stream.
    pub stream: StreamConfig,
    /// Decode limits.
    pub limits: Limits,
    /// Frame transfer.
    pub transfer: Transfer,
    /// The worker's own resource limits.
    pub sandbox: SandboxLimits,
    /// The `ffmpeg` binary, for [`WorkerBackend::Ffmpeg`].
    pub ffmpeg: Option<String>,
    /// Decoder threads; 0 for the backend's default.
    pub threads: u32,
}

/// What the parent sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentMessage {
    /// Start a session.
    Open(Box<Open>),
    /// One packet: pts, dts, sync flag, bytes.
    Packet {
        /// Presentation timestamp.
        pts: Option<i64>,
        /// Decode timestamp.
        dts: Option<i64>,
        /// Sync flag.
        keyframe: bool,
        /// The caller does not want the packet's frame.
        discard: bool,
        /// The packet's bytes.
        data: Vec<u8>,
    },
    /// No more packets.
    Eof,
    /// A slot is free again.
    Ack(u32),
    /// End the session.
    Close,
}

fn opt_i64(v: Option<i64>) -> i64 {
    v.unwrap_or(i64::MIN)
}

fn from_opt_i64(v: i64) -> Option<i64> {
    (v != i64::MIN).then_some(v)
}

fn put_color(out: &mut Vec<u8>, c: &ColorInfo) {
    out.push(c.primaries.unwrap_or(0xFF));
    out.push(c.transfer.unwrap_or(0xFF));
    out.push(c.matrix.unwrap_or(0xFF));
    out.push(match c.full_range {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    });
    out.push(c.chroma_location.map_or(0xFF, location_code));
}

fn location_code(l: ChromaLocation) -> u8 {
    match l {
        ChromaLocation::Left => 0,
        ChromaLocation::Center => 1,
        ChromaLocation::TopLeft => 2,
        ChromaLocation::Top => 3,
        ChromaLocation::BottomLeft => 4,
        ChromaLocation::Bottom => 5,
    }
}

fn get_color(w: &mut Wire<'_>) -> Result<ColorInfo, ProtocolError> {
    let code = |v: u8| (v != 0xFF).then_some(v);
    let primaries = code(w.u8()?);
    let transfer = code(w.u8()?);
    let matrix = code(w.u8()?);
    let full_range = match w.u8()? {
        0 => None,
        1 => Some(false),
        2 => Some(true),
        _ => return Err(ProtocolError("range")),
    };
    let chroma_location = match w.u8()? {
        0xFF => None,
        v => Some(ChromaLocation::from_code(v).ok_or(ProtocolError("chroma location"))?),
    };
    Ok(ColorInfo {
        primaries,
        transfer,
        matrix,
        full_range,
        chroma_location,
    })
}

const KINDS: [ConfigKind; 7] = [
    ConfigKind::Avcc,
    ConfigKind::Hvcc,
    ConfigKind::Av1c,
    ConfigKind::Vpcc,
    ConfigKind::ProResFourcc,
    ConfigKind::Mpeg4Dsi,
    ConfigKind::MatroskaPrivate,
];

/// Largest codec configuration sent.
const MAX_CONFIG: usize = 1 << 20;

impl ParentMessage {
    /// Writes the message.
    pub fn write(&self, out: &mut impl Write) -> io::Result<()> {
        let mut p = Vec::new();
        let kind = match self {
            Self::Open(open) => {
                encode_open(&mut p, open);
                OPEN
            }
            Self::Packet {
                pts,
                dts,
                keyframe,
                discard,
                data,
            } => {
                p.reserve(data.len() + 17);
                p.extend_from_slice(&opt_i64(*pts).to_le_bytes());
                p.extend_from_slice(&opt_i64(*dts).to_le_bytes());
                p.push(u8::from(*keyframe) | (u8::from(*discard) << 1));
                p.extend_from_slice(data);
                PACKET
            }
            Self::Eof => EOF,
            Self::Ack(slot) => {
                put_u32(&mut p, *slot);
                ACK
            }
            Self::Close => CLOSE,
        };
        write_message(out, kind, &p)
    }

    /// Decodes a message the worker read.
    pub fn decode(kind: u8, payload: &[u8]) -> Result<Self, ProtocolError> {
        let mut w = Wire::new(payload);
        let message = match kind {
            OPEN => Self::Open(Box::new(decode_open(&mut w)?)),
            PACKET => {
                let pts = from_opt_i64(w.i64()?);
                let dts = from_opt_i64(w.i64()?);
                let flags = w.u8()?;
                return Ok(Self::Packet {
                    pts,
                    dts,
                    keyframe: flags & 1 != 0,
                    discard: flags & 2 != 0,
                    data: w.rest().to_vec(),
                });
            }
            EOF => Self::Eof,
            ACK => Self::Ack(w.u32()?),
            CLOSE => Self::Close,
            _ => return Err(ProtocolError("unknown message type")),
        };
        w.end()?;
        Ok(message)
    }
}

fn encode_open(p: &mut Vec<u8>, open: &Open) {
    p.push(open.backend.code());
    let s = &open.stream;
    let codec = Codec::ALL.iter().position(|c| *c == s.codec).unwrap_or(0);
    p.push(u8::try_from(codec).unwrap_or(0));
    match &s.config {
        Some(config) => {
            let kind = KINDS.iter().position(|k| *k == config.kind).unwrap_or(0);
            p.push(u8::try_from(kind).unwrap_or(0));
            put_bytes(p, &config.data);
        }
        None => p.push(0xFF),
    }
    put_u32(p, s.width.unwrap_or(0));
    put_u32(p, s.height.unwrap_or(0));
    put_color(p, &s.color);
    let l = &open.limits;
    put_u32(p, l.max_width);
    put_u32(p, l.max_height);
    put_u64(p, l.max_pixels);
    put_u64(p, l.max_frame_bytes);
    put_u64(p, l.max_decoded_bytes);
    put_u64(p, l.max_pending_bytes);
    put_u64(p, l.max_packet_bytes);
    put_u64(p, u64::try_from(l.timeout.as_millis()).unwrap_or(u64::MAX));
    put_u32(p, l.max_reference_frames);
    put_u64(p, l.max_memory_bytes);
    match &open.transfer {
        Transfer::Pipe => p.push(0),
        Transfer::Shared {
            path,
            slots,
            slot_size,
        } => {
            p.push(1);
            put_bytes(p, path.as_bytes());
            put_u32(p, *slots);
            put_u64(p, *slot_size);
        }
    }
    let opt = |v: Option<u64>| v.unwrap_or(u64::MAX);
    put_u64(p, opt(open.sandbox.address_space));
    put_u64(p, opt(open.sandbox.cpu_seconds));
    put_u64(p, opt(open.sandbox.open_files));
    put_bytes(p, open.ffmpeg.as_deref().unwrap_or("").as_bytes());
    put_u32(p, open.threads);
}

fn decode_open(w: &mut Wire<'_>) -> Result<Open, ProtocolError> {
    let backend = WorkerBackend::from_code(w.u8()?).ok_or(ProtocolError("backend"))?;
    let codec = *Codec::ALL
        .get(usize::from(w.u8()?))
        .ok_or(ProtocolError("codec"))?;
    let config = match w.u8()? {
        0xFF => None,
        k => Some(CodecConfig {
            kind: *KINDS
                .get(usize::from(k))
                .ok_or(ProtocolError("config kind"))?,
            data: w.bytes(MAX_CONFIG)?.to_vec(),
        }),
    };
    let nonzero = |v: u32| (v != 0).then_some(v);
    let width = nonzero(w.u32()?);
    let height = nonzero(w.u32()?);
    let color = get_color(w)?;
    let limits = Limits {
        max_width: w.u32()?,
        max_height: w.u32()?,
        max_pixels: w.u64()?,
        max_frame_bytes: w.u64()?,
        max_decoded_bytes: w.u64()?,
        max_pending_bytes: w.u64()?,
        max_packet_bytes: w.u64()?,
        timeout: Duration::from_millis(w.u64()?),
        max_reference_frames: w.u32()?,
        max_memory_bytes: w.u64()?,
    };
    let transfer = match w.u8()? {
        0 => Transfer::Pipe,
        1 => Transfer::Shared {
            path: String::from_utf8(w.bytes(4096)?.to_vec()).map_err(|_| ProtocolError("path"))?,
            slots: w.u32()?,
            slot_size: w.u64()?,
        },
        _ => return Err(ProtocolError("transfer")),
    };
    let opt = |v: u64| (v != u64::MAX).then_some(v);
    let sandbox = SandboxLimits {
        address_space: opt(w.u64()?),
        cpu_seconds: opt(w.u64()?),
        open_files: opt(w.u64()?),
    };
    let ffmpeg = String::from_utf8(w.bytes(4096)?.to_vec()).map_err(|_| ProtocolError("path"))?;
    let threads = w.u32()?;
    Ok(Open {
        backend,
        stream: StreamConfig {
            codec,
            config,
            width,
            height,
            color,
        },
        limits,
        transfer,
        sandbox,
        ffmpeg: (!ffmpeg.is_empty()).then_some(ffmpeg),
        threads,
    })
}

/// Where one plane of a frame is, relative to its slot (or to the data
/// that follows an inline descriptor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneDescriptor {
    /// Offset of the first row.
    pub offset: u64,
    /// Bytes from one row to the next.
    pub stride: u64,
    /// Bytes of the plane: `stride × (rows − 1) + row bytes`.
    pub len: u64,
}

/// A frame descriptor, as the worker sent it: unvalidated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameDescriptor {
    /// Sequence number, from 0.
    pub seq: u64,
    /// The slot, or [`INLINE`].
    pub slot: u32,
    /// Presentation timestamp.
    pub pts: Option<i64>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format, as sent: `(layout, chroma, depth)`.
    pub format: (u8, u8, u8),
    /// Colour description.
    pub color: ColorInfo,
    /// The planes.
    pub planes: Vec<PlaneDescriptor>,
}

/// The wire code of a pixel format.
pub fn format_code(format: PixelFormat) -> (u8, u8, u8) {
    let chroma = |c: Chroma| match c {
        Chroma::Mono => 0,
        Chroma::Cs420 => 1,
        Chroma::Cs422 => 2,
        Chroma::Cs444 => 3,
    };
    match format {
        PixelFormat::Yuv { chroma: c, depth } => (0, chroma(c), depth),
        PixelFormat::SemiPlanar { depth } => (1, 1, depth),
        PixelFormat::Gbr { depth } => (2, 0, depth),
        PixelFormat::Rgb24 => (3, 0, 8),
        PixelFormat::Rgba => (4, 0, 8),
    }
}

/// The pixel format of a wire code, for the formats a decoder outputs.
pub fn format_from_code(code: (u8, u8, u8)) -> Option<PixelFormat> {
    let (layout, chroma, depth) = code;
    let format = match layout {
        0 => PixelFormat::Yuv {
            chroma: match chroma {
                0 => Chroma::Mono,
                1 => Chroma::Cs420,
                2 => Chroma::Cs422,
                3 => Chroma::Cs444,
                _ => return None,
            },
            depth,
        },
        1 => PixelFormat::SemiPlanar { depth },
        2 => PixelFormat::Gbr { depth },
        _ => return None,
    };
    format.validate().ok()?;
    Some(format)
}

/// What the worker sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerMessage {
    /// The session is open; the decoder's name.
    Ready(String),
    /// A frame: its descriptor, and for an inline frame the data its
    /// planes' offsets refer to.
    Frame(FrameDescriptor, Vec<u8>),
    /// Every frame is out.
    Done,
    /// The session failed.
    Error(ErrorKind, String),
}

const ERROR_KINDS: [ErrorKind; 7] = [
    ErrorKind::InvalidData,
    ErrorKind::Unsupported,
    ErrorKind::CodecNotAllowed,
    ErrorKind::Limit,
    ErrorKind::Timeout,
    ErrorKind::Backend,
    ErrorKind::Io,
];

impl WorkerMessage {
    /// The largest payload the parent accepts for a message of type `kind`:
    /// small for everything but a frame, which may carry its planes inline
    /// (at most `max_frame_bytes`).
    pub fn max_len(kind: u8, max_frame_bytes: u64) -> usize {
        if kind == FRAME {
            usize::try_from(max_frame_bytes)
                .unwrap_or(usize::MAX)
                .saturating_add(MAX_CONTROL_MESSAGE)
        } else {
            MAX_CONTROL_MESSAGE
        }
    }

    /// Writes the message; an inline frame's data follows its descriptor.
    pub fn write(&self, out: &mut impl Write) -> io::Result<()> {
        let mut p = Vec::new();
        let kind = match self {
            Self::Ready(name) => {
                put_bytes(&mut p, name.as_bytes());
                READY
            }
            Self::Frame(d, data) => {
                p.reserve(data.len() + 128);
                put_u64(&mut p, d.seq);
                put_u32(&mut p, d.slot);
                p.extend_from_slice(&opt_i64(d.pts).to_le_bytes());
                put_u32(&mut p, d.width);
                put_u32(&mut p, d.height);
                p.extend_from_slice(&[d.format.0, d.format.1, d.format.2]);
                put_color(&mut p, &d.color);
                p.push(u8::try_from(d.planes.len()).unwrap_or(u8::MAX));
                for plane in &d.planes {
                    put_u64(&mut p, plane.offset);
                    put_u64(&mut p, plane.stride);
                    put_u64(&mut p, plane.len);
                }
                p.extend_from_slice(data);
                FRAME
            }
            Self::Done => DONE,
            Self::Error(kind, text) => {
                let code = ERROR_KINDS.iter().position(|k| k == kind).unwrap_or(5);
                p.push(u8::try_from(code).unwrap_or(5));
                let text = text.as_bytes();
                put_bytes(
                    &mut p,
                    text.get(..text.len().min(MAX_ERROR_TEXT)).unwrap_or(&[]),
                );
                ERROR
            }
        };
        write_message(out, kind, &p)
    }

    /// Decodes a message the parent read. Untrusted: every field is
    /// checked for form here, and frames for meaning by
    /// [`crate::descriptor::validate`].
    pub fn decode(kind: u8, payload: &[u8]) -> Result<Self, ProtocolError> {
        let mut w = Wire::new(payload);
        let message = match kind {
            READY => Self::Ready(String::from_utf8_lossy(w.bytes(256)?).into_owned()),
            FRAME => {
                let seq = w.u64()?;
                let slot = w.u32()?;
                let pts = from_opt_i64(w.i64()?);
                let width = w.u32()?;
                let height = w.u32()?;
                let format = (w.u8()?, w.u8()?, w.u8()?);
                let color = get_color(&mut w)?;
                let count = usize::from(w.u8()?);
                if count > MAX_PLANES {
                    return Err(ProtocolError("too many planes"));
                }
                let mut planes = Vec::with_capacity(count);
                for _ in 0..count {
                    planes.push(PlaneDescriptor {
                        offset: w.u64()?,
                        stride: w.u64()?,
                        len: w.u64()?,
                    });
                }
                let data = w.rest();
                if slot != INLINE && !data.is_empty() {
                    return Err(ProtocolError("data after a shared-memory descriptor"));
                }
                return Ok(Self::Frame(
                    FrameDescriptor {
                        seq,
                        slot,
                        pts,
                        width,
                        height,
                        format,
                        color,
                        planes,
                    },
                    data.to_vec(),
                ));
            }
            DONE => Self::Done,
            ERROR => {
                let kind = *ERROR_KINDS
                    .get(usize::from(w.u8()?))
                    .ok_or(ProtocolError("error kind"))?;
                let text = String::from_utf8_lossy(w.bytes(MAX_ERROR_TEXT)?).into_owned();
                Self::Error(kind, text)
            }
            _ => return Err(ProtocolError("unknown message type")),
        };
        w.end()?;
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_parent(m: &ParentMessage) -> ParentMessage {
        let mut buf = Vec::new();
        m.write(&mut buf).expect("write");
        let Incoming::Message(kind, payload) =
            read_message(&mut buf.as_slice(), |_| 1 << 20).expect("read")
        else {
            panic!("end");
        };
        ParentMessage::decode(kind, &payload).expect("decode")
    }

    #[test]
    fn parent_messages_round_trip() {
        let open = ParentMessage::Open(Box::new(Open {
            backend: WorkerBackend::Ffmpeg,
            stream: StreamConfig {
                config: Some(CodecConfig {
                    kind: ConfigKind::Hvcc,
                    data: vec![1, 2, 3],
                }),
                width: Some(1920),
                height: None,
                color: ColorInfo {
                    matrix: Some(1),
                    full_range: Some(false),
                    chroma_location: Some(ChromaLocation::Top),
                    ..ColorInfo::default()
                },
                ..StreamConfig::new(Codec::Hevc)
            },
            limits: Limits::default(),
            transfer: Transfer::Shared {
                path: "/proc/1/fd/3".into(),
                slots: 2,
                slot_size: 1 << 20,
            },
            sandbox: SandboxLimits {
                address_space: Some(1 << 33),
                cpu_seconds: None,
                open_files: Some(64),
            },
            ffmpeg: Some("/usr/bin/ffmpeg".into()),
            threads: 0,
        }));
        assert_eq!(round_trip_parent(&open), open);
        for m in [
            ParentMessage::Packet {
                pts: Some(-5),
                dts: None,
                keyframe: true,
                discard: true,
                data: vec![0, 1, 2],
            },
            ParentMessage::Eof,
            ParentMessage::Ack(1),
            ParentMessage::Close,
        ] {
            assert_eq!(round_trip_parent(&m), m);
        }
    }

    #[test]
    fn worker_messages_round_trip_and_limits() {
        let frame = WorkerMessage::Frame(
            FrameDescriptor {
                seq: 3,
                slot: INLINE,
                pts: Some(9),
                width: 2,
                height: 2,
                format: (0, 1, 8),
                color: ColorInfo::default(),
                planes: vec![PlaneDescriptor {
                    offset: 0,
                    stride: 2,
                    len: 4,
                }],
            },
            vec![1, 2, 3, 4],
        );
        for m in [
            WorkerMessage::Ready("x".into()),
            frame,
            WorkerMessage::Done,
            WorkerMessage::Error(ErrorKind::Limit, "y".into()),
        ] {
            let mut buf = Vec::new();
            m.write(&mut buf).expect("write");
            let Incoming::Message(kind, payload) =
                read_message(&mut buf.as_slice(), |k| WorkerMessage::max_len(k, 64)).expect("read")
            else {
                panic!("end");
            };
            assert_eq!(WorkerMessage::decode(kind, &payload).expect("decode"), m);
        }
        // A control message over the limit is refused before allocating.
        let mut big = Vec::new();
        write_message(&mut big, READY, &[0; 5000]).expect("write");
        assert!(read_message(&mut big.as_slice(), |k| WorkerMessage::max_len(k, 64)).is_err());
        let huge = [0xFF, 0xFF, 0xFF, 0xFF, FRAME];
        assert!(read_message(&mut huge.as_slice(), |k| WorkerMessage::max_len(k, 64)).is_err());
        assert!(matches!(
            read_message(&mut [].as_slice(), |_| 10),
            Ok(Incoming::End)
        ));
        assert!(read_message(&mut [5, 0].as_slice(), |_| 10).is_err());
        assert!(WorkerMessage::decode(0x99, &[]).is_err());
        assert!(WorkerMessage::decode(DONE, &[1]).is_err());
    }
}
