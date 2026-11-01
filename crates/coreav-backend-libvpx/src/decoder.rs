//! The [`Decoder`] over libvpx.
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::time::Instant;

use coreav_decode::{
    Backend, BackendKind, Codec, Decoder, DecoderInfo, Error, ErrorKind, Isolation, Limits, Packet,
    StreamConfig, StreamTraits, Support,
};
use coreav_frame::{Chroma, ColorInfo, Frame, Picture, PixelFormat, Pixels, Plane};

use std::borrow::Cow;

use crate::ffi::{self, FMT_HIGH_BIT_DEPTH, Failure, Interface, View, Vpx};
use crate::header;

/// Options of the libvpx backend.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibvpxOptions {
    /// Threads libvpx may use; 0: one per CPU.
    pub threads: u32,
}

/// libvpx in the calling process.
#[derive(Clone, Debug, Default)]
pub struct LibvpxBackend {
    /// Its options.
    pub options: LibvpxOptions,
}

impl Backend for LibvpxBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Libvpx
    }

    fn supports(&self, stream: &StreamConfig, _: &StreamTraits) -> Support {
        if interface(stream.codec).is_some() {
            Support::Yes
        } else {
            Support::No(ONLY.into())
        }
    }

    fn open(&self, stream: &StreamConfig, limits: &Limits) -> Result<Box<dyn Decoder>, Error> {
        Ok(Box::new(LibvpxDecoder::open(
            &self.options,
            stream,
            limits,
        )?))
    }
}

/// What the backend says of a codec other than VP8 and VP9.
const ONLY: &str = "libvpx here decodes VP8 and VP9 only";

/// The libvpx interface that decodes `codec`.
fn interface(codec: Codec) -> Option<Interface> {
    match codec {
        Codec::Vp8 => Some(Interface::Vp8),
        Codec::Vp9 => Some(Interface::Vp9),
        _ => None,
    }
}

/// Where [`LibvpxDecoder::send_packet_lending`] sends each frame.
type Take<'a> = &'a mut dyn FnMut(&LentFrame<'_>) -> Result<(), Error>;

/// A VP8 or VP9 decoder: one libvpx instance.
///
/// A packet is one frame (VP8), or one frame or superframe (VP9), as the
/// VP codec ISOBMFF binding and the Matroska mappings store them. libvpx
/// decodes it at once and makes ready the frame it shows, if any, which
/// takes the packet's pts; neither codec reorders frames. Frames are
/// copied out of libvpx's images into packed planes at once, so libvpx's
/// buffers are never held: into a [`Frame`] by [`Decoder::send_packet`],
/// or by the caller from a [`LentFrame`] with
/// [`LibvpxDecoder::send_packet_lending`].
pub struct LibvpxDecoder {
    vpx: Vpx,
    interface: Interface,
    info: DecoderInfo,
    limits: Limits,
    color: ColorInfo,
    deadline: Instant,
    /// Frames decoded and not yet received, and their bytes.
    ready: VecDeque<Frame>,
    ready_bytes: u64,
    /// Bytes of every frame output so far.
    decoded: u64,
    last_pts: Option<i64>,
    eof: bool,
    failed: bool,
}

impl LibvpxDecoder {
    /// Opens a decoder for a VP8 or VP9 stream. A VP9 stream above 8 bits
    /// is refused when this libvpx was built without high bit depth, and a
    /// stream whose references do not fit the memory budget (see
    /// [`fits_memory`]).
    pub fn open(
        options: &LibvpxOptions,
        stream: &StreamConfig,
        limits: &Limits,
    ) -> Result<Self, Error> {
        let Some(interface) = interface(stream.codec) else {
            return Err(Error::new(ErrorKind::Unsupported, ONLY));
        };
        if interface == Interface::Vp9
            && stream.traits().bit_depth.is_some_and(|d| d > 8)
            && !ffi::high_bit_depth()
        {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "this libvpx decodes VP9 at 8 bits only (built without high bit depth)",
            ));
        }
        fits_memory(interface, stream, limits)?;
        let threads = match options.threads {
            0 => std::thread::available_parallelism()
                .map_or(1, |n| u32::try_from(n.get()).unwrap_or(u32::MAX)),
            n => n,
        };
        let vpx = Vpx::open(interface, threads).map_err(|e| failure("opening", &e))?;
        Ok(Self {
            vpx,
            interface,
            info: DecoderInfo {
                backend: BackendKind::Libvpx,
                isolation: Isolation::InProcess,
                decoder: format!("libvpx {}", ffi::version().trim_start_matches('v')),
            },
            limits: limits.clone(),
            color: stream.color,
            deadline: limits.deadline(),
            ready: VecDeque::new(),
            ready_bytes: 0,
            decoded: 0,
            last_pts: None,
            eof: false,
            failed: false,
        })
    }

    fn usable(&self) -> Result<(), Error> {
        if self.failed {
            return Err(Error::new(ErrorKind::Backend, "the decoder failed earlier"));
        }
        if Instant::now() > self.deadline {
            return Err(Error::new(ErrorKind::Timeout, "decoding took too long"));
        }
        Ok(())
    }

    /// [`Decoder::send_packet`], except that each frame the packet makes
    /// ready goes to `take` while it is still in libvpx's buffer, instead of
    /// being copied into a [`Frame`] for [`Decoder::receive_frame`]: for a
    /// caller that copies it where it goes next at once (the sandboxed
    /// worker, into its shared memory). An error from `take` fails the
    /// decoder.
    pub fn send_packet_lending(
        &mut self,
        packet: &Packet<'_>,
        take: &mut dyn FnMut(&LentFrame<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.send(packet, Some(take))
    }

    fn send(&mut self, packet: &Packet<'_>, take: Option<Take<'_>>) -> Result<(), Error> {
        if self.eof {
            return Err(Error::new(ErrorKind::InvalidData, "packet after the end"));
        }
        self.usable()?;
        self.limits.check_packet(packet)?;
        let pts = packet
            .pts
            .or(packet.dts)
            .or_else(|| self.last_pts.map(|p| p.saturating_add(1)))
            .unwrap_or(0);
        self.last_pts = Some(pts);
        let result = self.decode(packet, pts, take);
        if result.is_err() {
            self.failed = true;
            self.ready.clear();
        }
        result
    }

    /// Checks the sizes the packet's frames state, then decodes it: a VP9
    /// packet with its superframe index, so that libvpx decodes those
    /// frames only, a VP8 packet as it is. Each frame shown goes to `take`,
    /// or without one is copied into the frames ready.
    fn decode(
        &mut self,
        packet: &Packet<'_>,
        pts: i64,
        mut take: Option<Take<'_>>,
    ) -> Result<(), Error> {
        let malformed = |codec: &str, e: header::Malformed| {
            Error::new(ErrorKind::InvalidData, format!("{codec} packet: {e}"))
        };
        let data = match self.interface {
            Interface::Vp9 => {
                let data = header::indexed(packet.data).map_err(|e| malformed("VP9", e))?;
                for size in header::frame_sizes(&data).map_err(|e| malformed("VP9", e))? {
                    self.limits.check_frame(size.width, size.height, 0)?;
                }
                data
            }
            Interface::Vp8 => {
                let size = header::vp8_frame_size(packet.data).map_err(|e| malformed("VP8", e))?;
                if let Some(size) = size {
                    self.limits.check_frame(size.width, size.height, 0)?;
                }
                Cow::Borrowed(packet.data)
            }
        };
        let (color, limits, decoded) = (self.color, &self.limits, &mut self.decoded);
        let mut made = Vec::new();
        self.vpx
            .decode(&data, |view| {
                if packet.discard {
                    let (width, height) = view.size();
                    return limits.check_frame(width, height, 0);
                }
                let layout = Layout::of(&view, limits)?;
                *decoded = decoded.saturating_add(layout.bytes);
                if *decoded > limits.max_decoded_bytes {
                    return Err(Error::new(
                        ErrorKind::Limit,
                        "decoded bytes exceed the limit",
                    ));
                }
                match take.as_mut() {
                    Some(take) => take(&LentFrame {
                        pts,
                        color,
                        layout: &layout,
                        view: &view,
                    }),
                    None => {
                        made.push(convert(&view, &layout, color)?);
                        Ok(())
                    }
                }
            })
            .map_err(|e| failure("decoding", &e))??;
        for (mut frame, bytes) in made {
            frame.pts = Some(pts);
            self.ready_bytes = self.ready_bytes.saturating_add(bytes);
            if self.ready_bytes > self.limits.max_pending_bytes {
                return Err(Error::new(
                    ErrorKind::Limit,
                    "frames waiting exceed the limit",
                ));
            }
            self.ready.push_back(frame);
        }
        Ok(())
    }
}

impl Decoder for LibvpxDecoder {
    fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
        self.send(packet, None)
    }

    fn send_eof(&mut self) -> Result<(), Error> {
        self.eof = true;
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<Option<Frame>, Error> {
        if let Some(frame) = self.ready.pop_front() {
            self.ready_bytes = self.ready_bytes.saturating_sub(frame_bytes(&frame));
            return Ok(Some(frame));
        }
        if !self.eof {
            self.usable()?;
        }
        Ok(None)
    }

    fn info(&self) -> &DecoderInfo {
        &self.info
    }
}

/// The fixed part of libvpx's memory in [`fits_memory`]'s estimate.
const FIXED_MEMORY: u64 = 64 << 20;

/// Frames libvpx holds in [`fits_memory`]'s estimate: the reference
/// frames (VP9's 8; VP8's last, golden and alternate reference frames),
/// the frame being decoded and the frame being output.
fn frames_held(interface: Interface) -> u64 {
    match interface {
        Interface::Vp8 => 5,
        Interface::Vp9 => 10,
    }
}

/// Refuses, with `Limit`, a stream whose declared size needs more than
/// `limits.max_memory_bytes` to decode: [`FIXED_MEMORY`] and
/// [`frames_held`] decoded frames at the declared size. Like the dav1d
/// backend's, the estimate trusts the declared size and is advisory: the
/// sizes each packet states are checked against the frame limits, and in
/// the worker the address-space limit is what holds.
fn fits_memory(interface: Interface, stream: &StreamConfig, limits: &Limits) -> Result<(), Error> {
    let (Some(w), Some(h)) = (stream.width, stream.height) else {
        return Ok(());
    };
    let need = decoded_frame_bytes(stream, w, h)
        .saturating_mul(frames_held(interface))
        .saturating_add(FIXED_MEMORY);
    if need > limits.max_memory_bytes {
        return Err(Error::new(
            ErrorKind::Limit,
            format!(
                "frames of {w}x{h} need about {} MiB to decode, over the memory limit of {} MiB",
                need >> 20,
                limits.max_memory_bytes >> 20
            ),
        ));
    }
    Ok(())
}

/// Bytes of one decoded frame at the declared size: from the bit depth and
/// chroma format the configuration record gives (samples of more than 8
/// bits take two bytes), else 6 bytes per pixel, the largest (4:4:4 at 16
/// bits).
fn decoded_frame_bytes(stream: &StreamConfig, width: u32, height: u32) -> u64 {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    let traits = stream.traits();
    let (Some(depth), Some(chroma)) = (traits.bit_depth, traits.chroma) else {
        return pixels.saturating_mul(6);
    };
    let sample = if depth > 8 { 2 } else { 1 };
    let chroma_eighths = match chroma {
        Chroma::Mono => 0,
        Chroma::Cs420 => 4,
        Chroma::Cs422 => 8,
        Chroma::Cs444 => 16,
    };
    pixels.saturating_mul(sample * (8 + chroma_eighths)) / 8
}

fn failure(what: &str, error: &Failure) -> Error {
    let kind = match error.code {
        2 => ErrorKind::Limit,
        3 | 8 => ErrorKind::Backend,
        4..=6 => ErrorKind::Unsupported,
        _ => ErrorKind::InvalidData,
    };
    Error::new(kind, format!("libvpx {what}: {}", error.message))
}

fn frame_bytes(frame: &Frame) -> u64 {
    match &frame.pixels {
        Pixels::Cpu(picture) => u64::try_from(picture.packed_len()).unwrap_or(u64::MAX),
        Pixels::Cuda(_) | Pixels::Shared(_) => 0,
    }
}

/// `VPX_IMG_FMT_I420`, `I422`, `I444` and `I440`, without the 16-bit flag.
const I420: i32 = 0x102;
const I422: i32 = 0x105;
const I444: i32 = 0x106;
const I440: i32 = 0x107;

/// A libvpx image's frame-model geometry, checked against the limits.
struct Layout {
    width: u32,
    height: u32,
    format: PixelFormat,
    high: bool,
    /// Each plane's bytes per row and rows.
    planes: Vec<(usize, usize)>,
    /// Bytes of the planes packed.
    bytes: u64,
}

impl Layout {
    fn of(view: &View<'_>, limits: &Limits) -> Result<Self, Error> {
        let (width, height) = view.size();
        let high = view.format() & FMT_HIGH_BIT_DEPTH != 0;
        let (chroma, shifts) = match view.format() & !FMT_HIGH_BIT_DEPTH {
            I420 => (Chroma::Cs420, (1, 1)),
            I422 => (Chroma::Cs422, (1, 0)),
            I444 => (Chroma::Cs444, (0, 0)),
            I440 => {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "VP9 4:4:0 (profile 1 or 3) has no pixel format in CoreAV's frame model",
                ));
            }
            _ => return Err(bad("unknown format")),
        };
        if view.shifts() != shifts {
            return Err(bad("subsampling differs from its format"));
        }
        let depth = u8::try_from(view.bit_depth()).map_err(|_| bad("bit depth"))?;
        if high != (depth > 8) {
            return Err(bad("sample size differs from its bit depth"));
        }
        let format = PixelFormat::Yuv { chroma, depth };
        format.validate().map_err(|_| bad("bit depth"))?;
        let mut planes = Vec::with_capacity(format.plane_count());
        let mut bytes = 0u64;
        for index in 0..format.plane_count() {
            let row = format
                .row_bytes(index, width, height)
                .ok_or_else(|| bad("row size"))?;
            let (_, rows) = format
                .plane_size(index, width, height)
                .ok_or_else(|| bad("plane size"))?;
            let rows = usize::try_from(rows).map_err(|_| bad("plane size"))?;
            let (samples, view_rows) = view.plane_size(index).ok_or_else(|| bad("plane size"))?;
            if samples.checked_mul(view.sample_bytes()) != Some(row) || view_rows != rows {
                return Err(bad("plane size differs from the frame model's"));
            }
            let plane = row.checked_mul(rows).ok_or_else(|| bad("plane size"))?;
            bytes = bytes
                .checked_add(u64::try_from(plane).map_err(|_| bad("plane size"))?)
                .ok_or_else(|| bad("plane size"))?;
            planes.push((row, rows));
        }
        limits.check_frame(width, height, bytes)?;
        Ok(Self {
            width,
            height,
            format,
            high,
            planes,
            bytes,
        })
    }
}

fn bad(what: &str) -> Error {
    Error::new(ErrorKind::Backend, format!("libvpx image: {what}"))
}

/// Samples of more than 8 bits are little-endian in the frame model; libvpx
/// writes them in the host's order.
#[cfg_attr(target_endian = "little", allow(unused_variables))]
fn to_little_endian(high: bool, plane: &mut [u8]) {
    #[cfg(target_endian = "big")]
    if high {
        for pair in plane.chunks_exact_mut(2) {
            pair.swap(0, 1);
        }
    }
}

/// Copies a libvpx image into a frame with packed planes; also returns its
/// bytes.
fn convert(view: &View<'_>, layout: &Layout, color: ColorInfo) -> Result<(Frame, u64), Error> {
    let mut planes = Vec::with_capacity(layout.planes.len());
    for (index, &(row, rows)) in layout.planes.iter().enumerate() {
        let mut data = Vec::new();
        data.try_reserve_exact(row.saturating_mul(rows))
            .map_err(|_| Error::new(ErrorKind::Limit, "out of memory for a frame"))?;
        for r in 0..rows {
            let line = view
                .row(index, r)
                .ok_or_else(|| bad("plane shorter than its rows"))?;
            data.extend_from_slice(line);
        }
        to_little_endian(layout.high, &mut data);
        planes.push(Plane::new(data, row));
    }
    let picture = Picture::new(layout.width, layout.height, layout.format, planes)
        .map_err(|e| bad(&e.to_string()))?;
    let frame = Frame {
        pts: None,
        color,
        pixels: Pixels::Cpu(picture),
    };
    Ok((frame, layout.bytes))
}

/// A frame shown by the packet [`LibvpxDecoder::send_packet_lending`] is
/// decoding, still in libvpx's buffer, which it is lent from for one call:
/// checked against the frame model and the limits.
pub struct LentFrame<'a> {
    /// The packet's presentation time.
    pub pts: i64,
    /// The stream's colour description.
    pub color: ColorInfo,
    layout: &'a Layout,
    view: &'a View<'a>,
}

impl LentFrame<'_> {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.layout.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.layout.height
    }

    /// Pixel format.
    pub fn format(&self) -> PixelFormat {
        self.layout.format
    }

    /// Bytes of the planes in order with their rows packed.
    pub fn packed_len(&self) -> u64 {
        self.layout.bytes
    }

    /// Copies the planes in order with their rows packed, as
    /// [`Picture::write_packed`] lays them out, into `out`, which is
    /// [`LentFrame::packed_len`] bytes long.
    pub fn write_packed(&self, out: &mut [u8]) -> Result<(), Error> {
        let short = || Error::new(ErrorKind::Backend, "frame buffer of the wrong size");
        let mut rest = out;
        for (index, &(row, rows)) in self.layout.planes.iter().enumerate() {
            let (plane, after) = rest
                .split_at_mut_checked(row.checked_mul(rows).ok_or_else(short)?)
                .ok_or_else(short)?;
            for (r, target) in plane.chunks_exact_mut(row.max(1)).enumerate() {
                let line = self
                    .view
                    .row(index, r)
                    .ok_or_else(|| bad("plane shorter than its rows"))?;
                target.copy_from_slice(line);
            }
            to_little_endian(self.layout.high, plane);
            rest = after;
        }
        if rest.is_empty() {
            Ok(())
        } else {
            Err(short())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_estimate_refuses_what_does_not_fit() {
        let stream = |width: u32, height: u32| StreamConfig {
            width: Some(width),
            height: Some(height),
            ..StreamConfig::new(Codec::Vp9)
        };
        let limits = |gib: u64| Limits {
            max_memory_bytes: gib << 30,
            ..Limits::default()
        };
        let vp9 = Interface::Vp9;
        // 1080p, no record: 12 MiB a frame, 10 frames and 64 MiB fit 1 GiB.
        assert!(fits_memory(vp9, &stream(1920, 1080), &limits(1)).is_ok());
        // 8K: 190 MiB a frame; 1 GiB holds none of it, 2 GiB all.
        assert_eq!(
            fits_memory(vp9, &stream(7680, 4320), &limits(1))
                .err()
                .map(|e| e.kind),
            Some(ErrorKind::Limit)
        );
        assert!(fits_memory(vp9, &stream(7680, 4320), &limits(2)).is_ok());
        assert!(fits_memory(vp9, &StreamConfig::new(Codec::Vp9), &limits(1)).is_ok());
        // VP8 holds 5 frames: 8K's 950 MiB and 64 MiB fit 1 GiB.
        assert!(fits_memory(Interface::Vp8, &stream(7680, 4320), &limits(1)).is_ok());
        assert_eq!(
            fits_memory(Interface::Vp8, &stream(7680, 8640), &limits(1))
                .err()
                .map(|e| e.kind),
            Some(ErrorKind::Limit)
        );
    }
}
