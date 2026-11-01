//! The [`Decoder`] over dav1d.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use coreav_decode::{
    Backend, BackendKind, Codec, ConfigKind, Decoder, DecoderInfo, Error, ErrorKind, Isolation,
    Limits, Packet, StreamConfig, StreamTraits, Support,
};
use coreav_frame::{Chroma, ColorInfo, Frame, Picture, PixelFormat, Pixels, Plane};
use dav1d::{PixelLayout, PlanarImageComponent, Settings};

/// Most rounds of handing dav1d the rest of a packet it held back
/// (`Again`), taking its frames between rounds, before giving up.
const MAX_ROUNDS: usize = 4096;

/// Options of the dav1d backend.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dav1dOptions {
    /// Threads dav1d may use; 0 lets dav1d choose.
    pub threads: u32,
}

/// dav1d in the calling process.
#[derive(Clone, Debug, Default)]
pub struct Dav1dBackend {
    /// Its options.
    pub options: Dav1dOptions,
}

impl Backend for Dav1dBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Dav1d
    }

    fn supports(&self, stream: &StreamConfig, _: &StreamTraits) -> Support {
        if stream.codec == Codec::Av1 {
            Support::Yes
        } else {
            Support::No("dav1d decodes AV1 only".into())
        }
    }

    fn open(&self, stream: &StreamConfig, limits: &Limits) -> Result<Box<dyn Decoder>, Error> {
        Ok(Box::new(Dav1dDecoder::open(&self.options, stream, limits)?))
    }
}

/// An AV1 decoder: one dav1d instance.
///
/// Packets are temporal units (the AV1 ISOBMFF and Matroska mappings), each
/// sent with its pts as dav1d's timestamp, which dav1d gives back on the
/// frame it decodes from it. Frames are copied out of dav1d's pictures
/// into packed planes at once, so dav1d's buffers are never held.
pub struct Dav1dDecoder {
    decoder: dav1d::Decoder,
    info: DecoderInfo,
    limits: Limits,
    color: ColorInfo,
    deadline: Instant,
    /// Frames taken from dav1d and not yet received, and their bytes.
    ready: VecDeque<Frame>,
    ready_bytes: u64,
    /// Bytes of every frame output so far.
    decoded: u64,
    /// How many packets of each pts the caller marked to discard.
    discard: HashMap<i64, u32>,
    last_pts: Option<i64>,
    /// Whether dav1d was asked for a picture since it last took data. A
    /// second request without new data makes dav1d wait for its oldest
    /// frame in flight, as when draining, so asking more than once per
    /// packet would empty its frame pipeline on every packet.
    polled: bool,
    eof: bool,
    done: bool,
    failed: bool,
}

impl Dav1dDecoder {
    /// Opens a decoder for an AV1 stream. The configuration record's OBUs
    /// (the sequence header), when there are any, go to dav1d first.
    pub fn open(
        options: &Dav1dOptions,
        stream: &StreamConfig,
        limits: &Limits,
    ) -> Result<Self, Error> {
        if stream.codec != Codec::Av1 {
            return Err(Error::new(ErrorKind::Unsupported, "dav1d decodes AV1 only"));
        }
        let mut settings = Settings::new();
        settings.set_n_threads(options.threads);
        settings.set_max_frame_delay(frame_delay_within(stream, limits)?);
        // One frame per temporal unit, the highest spatial layer of the
        // operating point, as FFmpeg's decoders output.
        settings.set_all_layers(false);
        settings.set_frame_size_limit(u32::try_from(limits.max_pixels).unwrap_or(u32::MAX));
        let decoder =
            dav1d::Decoder::with_settings(&settings).map_err(|e| failure("opening", e))?;
        let mut this = Self {
            decoder,
            info: DecoderInfo {
                backend: BackendKind::Dav1d,
                isolation: Isolation::InProcess,
                decoder: "dav1d".into(),
            },
            limits: limits.clone(),
            color: stream.color,
            deadline: limits.deadline(),
            ready: VecDeque::new(),
            ready_bytes: 0,
            decoded: 0,
            discard: HashMap::new(),
            last_pts: None,
            polled: false,
            eof: false,
            done: false,
            failed: false,
        };
        let obus = stream
            .config
            .as_ref()
            .filter(|c| c.kind == ConfigKind::Av1c)
            .and_then(|c| c.data.get(4..))
            .filter(|obus| !obus.is_empty());
        if let Some(obus) = obus {
            // A sync sample carries its own sequence header; the record's
            // copy only helps a stream whose first sample lacks it, so a
            // record dav1d rejects leaves the decision to the samples.
            if this.feed(obus.to_vec(), None).is_err() {
                // Nothing of the record stays in dav1d: data it still held
                // would refuse the first packet.
                this.decoder.flush();
                this.ready.clear();
                this.ready_bytes = 0;
                this.polled = false;
            }
        }
        Ok(this)
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

    /// Hands dav1d one buffer, taking frames whenever it holds data back.
    fn feed(&mut self, data: Vec<u8>, pts: Option<i64>) -> Result<(), Error> {
        let mut result = self.decoder.send_data(data, None, pts, None);
        for _ in 0..MAX_ROUNDS {
            match result {
                Ok(()) => {
                    self.polled = false;
                    return Ok(());
                }
                Err(e) if e.is_again() => {
                    // dav1d has a frame to give before it takes the rest;
                    // `send_data` must not be called until it has.
                    self.pull()?;
                    result = self.decoder.send_pending_data();
                }
                Err(e) => return Err(failure("decoding", e)),
            }
        }
        Err(Error::new(
            ErrorKind::Backend,
            "dav1d did not take a packet",
        ))
    }

    /// [`Self::pull`], once per packet taken (see `polled`); `false` when
    /// dav1d was already asked.
    fn poll(&mut self) -> Result<bool, Error> {
        if self.polled {
            return Ok(false);
        }
        self.polled = true;
        self.pull()
    }

    /// Takes one picture from dav1d into the ready queue (or drops it when
    /// it is to be discarded); `false` when dav1d has none.
    fn pull(&mut self) -> Result<bool, Error> {
        let picture = match self.decoder.get_picture() {
            Ok(picture) => picture,
            Err(e) if e.is_again() => return Ok(false),
            Err(e) => return Err(failure("decoding", e)),
        };
        let pts = picture.timestamp();
        if let Some(p) = pts {
            if let Some(count) = self.discard.get_mut(&p) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.discard.remove(&p);
                }
                return Ok(true);
            }
        }
        let (mut frame, bytes) = convert(&picture, self.color, &self.limits)?;
        frame.pts = pts;
        self.decoded = self.decoded.saturating_add(bytes);
        if self.decoded > self.limits.max_decoded_bytes {
            return Err(Error::new(
                ErrorKind::Limit,
                "decoded bytes exceed the limit",
            ));
        }
        self.ready_bytes = self.ready_bytes.saturating_add(bytes);
        if self.ready_bytes > self.limits.max_pending_bytes {
            return Err(Error::new(
                ErrorKind::Limit,
                "frames waiting exceed the limit",
            ));
        }
        self.ready.push_back(frame);
        Ok(true)
    }

    fn pop(&mut self) -> Option<Frame> {
        let frame = self.ready.pop_front()?;
        self.ready_bytes = self.ready_bytes.saturating_sub(frame_bytes(&frame));
        Some(frame)
    }

    fn guard<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        if result.is_err() {
            self.failed = true;
            self.ready.clear();
        }
        result
    }
}

impl Decoder for Dav1dDecoder {
    fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
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
        if packet.discard {
            *self.discard.entry(pts).or_insert(0) += 1;
        }
        let mut data = Vec::new();
        data.try_reserve_exact(packet.data.len())
            .map_err(|_| Error::new(ErrorKind::Limit, "out of memory for a packet"))?;
        data.extend_from_slice(packet.data);
        let result = self.feed(data, Some(pts)).and_then(|()| self.poll());
        self.guard(result).map(|_| ())
    }

    fn send_eof(&mut self) -> Result<(), Error> {
        self.eof = true;
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<Option<Frame>, Error> {
        if let Some(frame) = self.pop() {
            return Ok(Some(frame));
        }
        if self.done {
            return Ok(None);
        }
        self.usable()?;
        if !self.eof {
            let result = self.poll();
            self.guard(result)?;
            return Ok(self.pop());
        }
        // After the last packet, asking again and again drains dav1d: it
        // gives the frames it delayed, then `Again`.
        loop {
            let result = self.usable().and_then(|()| self.pull());
            if !self.guard(result)? {
                self.done = true;
                return Ok(None);
            }
            if let Some(frame) = self.pop() {
                return Ok(Some(frame));
            }
        }
    }

    fn info(&self) -> &DecoderInfo {
        &self.info
    }
}

/// The fixed part of dav1d's memory in [`frame_delay_within`]'s estimate.
const FIXED_MEMORY: u64 = 64 << 20;

/// The frames dav1d decodes at once (its frame delay) when the memory
/// budget allows. dav1d's own choice on 8 threads is about 3, and 8
/// decodes a long run of 4K frames about 1.5 times as fast
/// (`docs/design/decode.md`, "dav1d").
const FRAME_DELAY: u64 = 8;

/// dav1d's frame delay within `limits.max_memory_bytes`, in its settings'
/// terms (0: dav1d's own choice). dav1d keeps the 8 reference frames AV1
/// allows, the frame being output and one frame per frame decoded in
/// parallel (the delay), so the estimate is `FIXED_MEMORY` and one decoded
/// frame at the declared size per frame held. A stream whose references
/// and one frame in flight do not fit is refused before dav1d starts;
/// otherwise the delay is [`FRAME_DELAY`], lowered until the estimate
/// fits. Like the FFmpeg
/// backend's fitting, this trusts the declared size and the configuration
/// record, so it is advisory and the address-space limit is what holds;
/// without a declared size, dav1d's own choice stands.
fn frame_delay_within(stream: &StreamConfig, limits: &Limits) -> Result<u32, Error> {
    let (Some(w), Some(h)) = (stream.width, stream.height) else {
        return Ok(0);
    };
    let frame = decoded_frame_bytes(stream, w, h);
    let need = |delay: u64| {
        frame
            .saturating_mul(delay.saturating_add(9))
            .saturating_add(FIXED_MEMORY)
    };
    let budget = limits.max_memory_bytes;
    if need(1) > budget {
        return Err(Error::new(
            ErrorKind::Limit,
            format!(
                "frames of {w}x{h} need about {} MiB to decode, over the memory limit of {} MiB",
                need(1) >> 20,
                budget >> 20
            ),
        ));
    }
    let mut delay = FRAME_DELAY;
    while delay > 1 && need(delay) > budget {
        delay -= 1;
    }
    Ok(u32::try_from(delay).unwrap_or(1))
}

/// Bytes of one decoded frame at the declared size: from the bit depth and
/// chroma format `av1C` gives (samples of more than 8 bits take two
/// bytes), else 6 bytes per pixel, the largest (4:4:4 at 16 bits).
fn decoded_frame_bytes(stream: &StreamConfig, width: u32, height: u32) -> u64 {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    let traits = stream.traits();
    let (Some(depth), Some(chroma)) = (traits.bit_depth, traits.chroma) else {
        return pixels.saturating_mul(6);
    };
    let sample = if depth > 8 { 2 } else { 1 };
    // Planes in eighths of the luma plane.
    let chroma_eighths = match chroma {
        Chroma::Mono => 0,
        Chroma::Cs420 => 4,
        Chroma::Cs422 => 8,
        Chroma::Cs444 => 16,
    };
    pixels.saturating_mul(sample * (8 + chroma_eighths)) / 8
}

fn failure(what: &str, error: dav1d::Error) -> Error {
    let kind = match error {
        dav1d::Error::UnsupportedBitstream => ErrorKind::Unsupported,
        dav1d::Error::NotEnoughMemory => ErrorKind::Limit,
        _ => ErrorKind::InvalidData,
    };
    Error::new(kind, format!("dav1d {what}: {error}"))
}

fn frame_bytes(frame: &Frame) -> u64 {
    match &frame.pixels {
        Pixels::Cpu(picture) => u64::try_from(picture.packed_len()).unwrap_or(u64::MAX),
        Pixels::Cuda(_) | Pixels::Shared(_) => 0,
    }
}

/// Copies a dav1d picture into a frame with packed planes, checked against
/// the limits first; also returns its bytes.
fn convert(
    picture: &dav1d::Picture,
    color: ColorInfo,
    limits: &Limits,
) -> Result<(Frame, u64), Error> {
    let bad = |what: &str| Error::new(ErrorKind::Backend, format!("dav1d picture: {what}"));
    let (width, height) = (picture.width(), picture.height());
    let depth = picture
        .bits_per_component()
        .and_then(|b| u8::try_from(b.0).ok())
        .ok_or_else(|| bad("unknown bit depth"))?;
    let chroma = match picture.pixel_layout() {
        PixelLayout::I400 => Chroma::Mono,
        PixelLayout::I420 => Chroma::Cs420,
        PixelLayout::I422 => Chroma::Cs422,
        PixelLayout::I444 => Chroma::Cs444,
    };
    // An identity matrix (RGB coded as G, B, R) stays in the Y'CbCr layout;
    // the colour description says how to convert it.
    let format = PixelFormat::Yuv { chroma, depth };
    format.validate().map_err(|_| bad("bit depth"))?;
    // dav1d stores samples of more than 8 bits in 16 bits in the host's
    // byte order; the frame model's are little-endian (swapped below on a
    // big-endian host).
    if picture.bit_depth() != usize::from(depth) {
        return Err(bad("bit depth"));
    }
    let components = [
        PlanarImageComponent::Y,
        PlanarImageComponent::U,
        PlanarImageComponent::V,
    ];
    let mut sizes = Vec::with_capacity(format.plane_count());
    let mut total = 0u64;
    for index in 0..format.plane_count() {
        let row = format
            .row_bytes(index, width, height)
            .ok_or_else(|| bad("row size"))?;
        let (_, rows) = format
            .plane_size(index, width, height)
            .ok_or_else(|| bad("plane size"))?;
        let rows = usize::try_from(rows).map_err(|_| bad("plane size"))?;
        let bytes = row.checked_mul(rows).ok_or_else(|| bad("plane size"))?;
        total = total
            .checked_add(u64::try_from(bytes).map_err(|_| bad("plane size"))?)
            .ok_or_else(|| bad("plane size"))?;
        sizes.push((row, rows, bytes));
    }
    limits.check_frame(width, height, total)?;
    let mut planes = Vec::with_capacity(sizes.len());
    for (component, (row, rows, bytes)) in components.into_iter().zip(sizes) {
        let stride = usize::try_from(picture.stride(component)).map_err(|_| bad("stride"))?;
        let source = picture.plane(component);
        let source: &[u8] = &source;
        let mut data = Vec::new();
        data.try_reserve_exact(bytes)
            .map_err(|_| Error::new(ErrorKind::Limit, "out of memory for a frame"))?;
        for r in 0..rows {
            let line = r
                .checked_mul(stride)
                .and_then(|start| source.get(start..start.checked_add(row)?))
                .ok_or_else(|| bad("plane shorter than its rows"))?;
            data.extend_from_slice(line);
        }
        #[cfg(target_endian = "big")]
        if format.sample_bytes() == 2 {
            for pair in data.chunks_exact_mut(2) {
                pair.swap(0, 1);
            }
        }
        planes.push(Plane::new(data, row));
    }
    let picture = Picture::new(width, height, format, planes).map_err(|e| bad(&e.to_string()))?;
    let frame = Frame {
        pts: None,
        color,
        pixels: Pixels::Cpu(picture),
    };
    Ok((frame, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(width: u32, height: u32) -> StreamConfig {
        StreamConfig {
            width: Some(width),
            height: Some(height),
            ..StreamConfig::new(Codec::Av1)
        }
    }

    #[test]
    fn the_frame_delay_fits_the_memory_budget() {
        let limits = |gib: u64| Limits {
            max_memory_bytes: gib << 30,
            ..Limits::default()
        };
        // 1080p, no record: 12 MiB a frame; everything fits in 1 GiB.
        assert_eq!(
            frame_delay_within(&stream(1920, 1080), &limits(1)).ok(),
            Some(8)
        );
        // 8K: 190 MiB a frame; 1 GiB holds none, 3 GiB a delay of 6, 8 GiB
        // the whole delay.
        assert_eq!(
            frame_delay_within(&stream(7680, 4320), &limits(1))
                .err()
                .map(|e| e.kind),
            Some(ErrorKind::Limit)
        );
        assert_eq!(
            frame_delay_within(&stream(7680, 4320), &limits(3)).ok(),
            Some(6)
        );
        assert_eq!(
            frame_delay_within(&stream(7680, 4320), &limits(8)).ok(),
            Some(8)
        );
        // 2^27 pixels: 768 MiB a frame; 8 GiB holds the references and one.
        let big = frame_delay_within(&stream(16384, 8192), &limits(8));
        assert_eq!(big.ok(), Some(1));
        assert_eq!(
            frame_delay_within(&StreamConfig::new(Codec::Av1), &limits(1)).ok(),
            Some(0)
        );
    }
}
