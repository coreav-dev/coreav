//! CoreAV's native H.264 decoder behind the decode boundary: the
//! `native` backend of [`coreav_decode::BackendKind`], over
//! [`coreav_h264`] (`docs/design/decode.md`, `docs/design/h264.md`).
//!
//! **Opt-in, and never in a default build.** Decoding H.264 may need a
//! patent licence where you are (`README.md` in this crate,
//! `docs/design/h264.md`, "Packaging"). The default H.264 path is
//! unchanged: the sandboxed FFmpeg fallback (an NVDEC backend is planned
//! but not built yet, `docs/design/decode.md`, "NVDEC").
//!
//! The decoder runs in the calling process ([`Isolation::InProcess`]),
//! which is a different proposition from running a C decoder there:
//! everything that touches the bitstream is safe Rust, the pixel kernels
//! are those of `coreav-kernels-h264`, called only through checked views
//! and listed in `docs/unsafe-sites.md`, and every entry point
//! is bounded by [`Limits`]. What the boundary's limits ask of a
//! sandboxed backend — an address space it cannot exceed — becomes here
//! a bound on the pictures the decoder may hold.
//!
//! ```
//! use coreav_backend_h264::NativeH264;
//! use coreav_decode::{Backend, BackendKind, Codec, StreamConfig, Support};
//!
//! let backend = NativeH264::new();
//! assert_eq!(backend.kind(), BackendKind::Native);
//! // Another codec is another backend's.
//! let stream = StreamConfig::new(Codec::Hevc);
//! assert!(matches!(
//!     backend.supports(&stream, &stream.traits()),
//!     Support::No(_)
//! ));
//! ```

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::time::Instant;

use coreav_decode::{
    Backend, BackendKind, Codec, ConfigKind, Decoder, DecoderInfo, Error, ErrorKind, Isolation,
    Limits, Packet, StreamConfig, StreamTraits, Support,
};
use coreav_frame::{Chroma, ColorInfo, Frame, Picture, PixelFormat, Pixels, Plane};
use coreav_h264::{Auto, Mode, PlaneRef};

/// Packets whose pictures have not come out yet, at most, beside one more
/// for each picture the decoder decodes at once. A decoded picture waits
/// for output behind at most a full decoded picture buffer (16 frames,
/// Annex C.4.5.3), and with threads it comes out up to one picture per
/// thread later still, so an older entry than this belongs to a packet
/// that produced no picture of its own: a parameter set, or a picture the
/// decoder skipped.
const MAX_PENDING_MARKS: usize = 64;

/// How the native backend decodes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeOptions {
    /// Decode only the pictures that need no other picture, and skip the
    /// rest ([`coreav_h264::Mode::KeyframesOnly`]).
    ///
    /// A thumbnail at a keyframe needs one picture, and skipping the rest
    /// is most of the work of a stream. The pictures the decoder then
    /// returns are the ones full decoding returns for the same access
    /// units, sample for sample (`docs/design/h264.md`, bar C5).
    pub keyframes_only: bool,
    /// Pictures decoded at once, each on a thread of its own
    /// ([`coreav_h264::Decoder::with_threads`]); `None` takes the host's
    /// cores. The count is lowered, down to one, where the pictures it
    /// keeps in flight would not fit `Limits::max_memory_bytes`, and is
    /// one where threads cannot be started (WebAssembly). The frames are
    /// the same whatever the count.
    pub threads: Option<usize>,
}

/// The native H.264 backend.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeH264 {
    options: NativeOptions,
}

impl NativeH264 {
    /// The backend with its defaults: every picture, and the fastest
    /// kernels this CPU supports.
    pub fn new() -> NativeH264 {
        NativeH264::default()
    }

    /// The backend with `options`.
    pub fn with_options(options: NativeOptions) -> NativeH264 {
        NativeH264 { options }
    }

    /// The memory budget (`Limits::max_memory_bytes`) a stream of
    /// `width` by `height` needs for the backend to decode it with
    /// `threads` pictures in flight, under the rest of `limits`
    /// ([`coreav_h264::Decoder::memory_needed`]). A budget below it
    /// lowers the thread count before the first picture of that size, and
    /// one below what one thread needs refuses the stream with `Limit`.
    pub fn memory_needed(width: u32, height: u32, threads: usize, limits: &Limits) -> u64 {
        let own = decode_limits(limits);
        coreav_h264::Decoder::<Auto>::memory_needed(
            width.div_ceil(16) as usize,
            height.div_ceil(16) as usize,
            threads.clamp(1, own.max_threads.max(1)),
            &own,
        )
    }
}

impl Backend for NativeH264 {
    fn kind(&self) -> BackendKind {
        BackendKind::Native
    }

    fn supports(&self, stream: &StreamConfig, traits: &StreamTraits) -> Support {
        if stream.codec != Codec::H264 {
            return Support::No("this backend decodes H.264 only".into());
        }
        if let Some(config) = &stream.config {
            if config.kind != ConfigKind::Avcc {
                return Support::No("the H.264 configuration record is not an avcC".into());
            }
        }
        // What the configuration record says, before a packet is read.
        // A stream that carries none is accepted here and refused by the
        // decoder if its parameter sets are outside the scope.
        if let Some(depth) = traits.bit_depth {
            if depth != 8 {
                return Support::No(format!("{depth}-bit H.264 is outside this decoder"));
            }
        }
        if let Some(chroma) = traits.chroma {
            if !matches!(chroma, Chroma::Cs420 | Chroma::Mono) {
                return Support::No("4:2:2 or 4:4:4 H.264 is outside this decoder".into());
            }
        }
        match traits.profile {
            // Baseline and Constrained Baseline, Main, Extended and High:
            // the profiles of `docs/design/h264.md`, "Scope".
            None | Some(66 | 77 | 88 | 100) => {}
            Some(profile) => {
                return Support::No(format!(
                    "H.264 profile_idc {profile} is outside this decoder"
                ));
            }
        }
        // What the record's parameter sets decide, which is most of the
        // scope: a profile code point says nothing about interlaced
        // coding or slice groups, and a stream this decoder cannot
        // finish must be another backend's before one is opened, not
        // after the first packet.
        match scope(stream) {
            Ok(()) => Support::Yes,
            Err(reason) => Support::No(reason),
        }
    }

    fn open(&self, stream: &StreamConfig, limits: &Limits) -> Result<Box<dyn Decoder>, Error> {
        Ok(Box::new(NativeDecoder::open(stream, limits, self.options)?))
    }
}

/// Whether the parameter sets of `stream`'s configuration record
/// describe a stream this decoder decodes, read without decoding
/// anything (`coreav_h264::Decoder::parameter_sets_in_scope`).
///
/// A stream with no record is left to the decoder, which refuses it at
/// the first parameter set it cannot decode. That is the Annex B case,
/// where nothing is known before the packets.
fn scope(stream: &StreamConfig) -> Result<(), String> {
    let Some(config) = &stream.config else {
        return Ok(());
    };
    if config.kind != ConfigKind::Avcc {
        return Err("the H.264 configuration record is not an avcC".to_owned());
    }
    let record = parse_avcc(&config.data).map_err(|e| e.message)?;
    let mut decoder = coreav_h264::Decoder::new(coreav_h264::Limits::default());
    for unit in record.parameter_sets {
        decoder
            .decode_nal(unit)
            .map_err(|e| format!("the configuration record: {e}"))?;
    }
    decoder.parameter_sets_in_scope().map_err(|e| e.to_string())
}

/// A packet that has been sent, and what its picture takes from it.
#[derive(Clone, Copy, Debug)]
struct Mark {
    /// What [`coreav_h264::Decoder::set_mark`] attached to its pictures.
    mark: u64,
    /// The packet's presentation timestamp.
    pts: Option<i64>,
    /// The caller does not need this packet's frame.
    discard: bool,
}

/// The native decoder: access units in, frames out in display order.
///
/// A call that fails with a limit or a broken stream ends the decode:
/// the pictures the decoder had finished are dropped with it, and the
/// counters it keeps are whatever that call left. The boundary gives a
/// caller no way to carry on after an error, and this decoder does not
/// invent one.
struct NativeDecoder {
    decoder: coreav_h264::Decoder<Auto>,
    /// Pictures the decoder decodes at once.
    threads: usize,
    info: DecoderInfo,
    limits: Limits,
    /// When the decode must be over, where the platform has a clock.
    deadline: Option<Instant>,
    /// Bytes of the length prefix of each NAL unit (`avcC`), or `None`
    /// when the packets are an Annex B byte stream.
    length_size: Option<usize>,
    color: ColorInfo,
    /// The width and height the container declares, if it does
    /// ([`visible`]).
    shown: (Option<u32>, Option<u32>),
    /// Frames decoded and not yet received, in display order, each with
    /// the bytes it holds.
    ready: VecDeque<(Frame, u64)>,
    /// The packets whose pictures have not come out yet, oldest first.
    marks: VecDeque<Mark>,
    next_mark: u64,
    decoded_bytes: u64,
    pending_bytes: u64,
    eof: bool,
}

impl NativeDecoder {
    fn open(
        stream: &StreamConfig,
        limits: &Limits,
        options: NativeOptions,
    ) -> Result<NativeDecoder, Error> {
        let kernels = Auto::detect();
        let own_limits = decode_limits(limits);
        // The decoder lowers the count where the pictures of a size it
        // meets would not fit the memory budget, before it makes them.
        let threads = options
            .threads
            .unwrap_or_else(host_threads)
            .clamp(1, own_limits.max_threads.max(1));
        let mut decoder =
            coreav_h264::Decoder::with_kernels(kernels, own_limits).with_threads(threads);
        if options.keyframes_only {
            decoder = decoder.with_mode(Mode::KeyframesOnly);
        }
        let mut length_size = None;
        if let Some(config) = &stream.config {
            if config.kind != ConfigKind::Avcc {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "the native H.264 decoder reads an avcC configuration record",
                ));
            }
            let record = parse_avcc(&config.data)?;
            length_size = Some(record.length_size);
            // The record's parameter sets, so that the first packet can
            // be a picture.
            for unit in record.parameter_sets {
                decoder.decode_nal(unit).map_err(|e| {
                    Error::new(
                        kind_of(&e),
                        format!("the configuration record's parameter sets: {e}"),
                    )
                })?;
            }
            // And what they say about the scope and about the limits,
            // so that a caller is told here rather than at its first
            // packet: `coreav_decode::open` then tries the next backend
            // and keeps a limit as the answer if no backend opens.
            decoder
                .parameter_sets_in_scope()
                .and_then(|()| decoder.parameter_sets_within_limits())
                .map_err(|e| Error::new(kind_of(&e), e.to_string()))?;
        }
        let decoder_name = if kernels.uses_simd() {
            "coreav h264, SIMD kernels"
        } else {
            "coreav h264, reference kernels"
        };
        Ok(NativeDecoder {
            threads: decoder.threads(),
            decoder,
            info: DecoderInfo {
                backend: BackendKind::Native,
                isolation: Isolation::InProcess,
                decoder: decoder_name.to_owned(),
            },
            limits: limits.clone(),
            deadline: deadline(limits),
            length_size,
            color: stream.color,
            shown: (stream.width, stream.height),
            ready: VecDeque::new(),
            marks: VecDeque::new(),
            next_mark: 0,
            decoded_bytes: 0,
            pending_bytes: 0,
            eof: false,
        })
    }

    /// Fails once the deadline has passed.
    fn check_deadline(&self) -> Result<(), Error> {
        match self.deadline {
            // The clock is read only where there is one: the guard of
            // this arm does not run when there is no deadline.
            Some(deadline) if Instant::now() > deadline => {
                Err(Error::new(ErrorKind::Timeout, "the decode deadline passed"))
            }
            _ => Ok(()),
        }
    }

    /// What the packet marked `mark` carried, and forgets it.
    fn take_mark(&mut self, mark: u64) -> Option<Mark> {
        let at = self.marks.iter().position(|m| m.mark == mark)?;
        self.marks.remove(at)
    }

    /// Turns the pictures the decoder has finished into frames.
    fn collect(&mut self) -> Result<(), Error> {
        for picture in self.decoder.take_pictures() {
            // A frame carries the timestamp of the packet whose picture
            // it is (`docs/design/decode.md`), so a picture whose packet
            // is no longer known is dropped rather than returned without
            // one: a caller picks its frame by timestamp, and a frame it
            // cannot place is not one it asked for. A packet's mark is
            // kept until its picture comes out or until
            // `MAX_PENDING_MARKS` later packets have been sent, which a
            // picture cannot wait for.
            let Some(mark) = self.take_mark(picture.mark()) else {
                continue;
            };
            if mark.discard {
                continue;
            }
            self.push_frame(&picture, mark.pts)?;
        }
        Ok(())
    }

    /// One decoded picture, checked against the limits and copied into a
    /// frame.
    fn push_frame(
        &mut self,
        picture: &coreav_h264::Picture,
        pts: Option<i64>,
    ) -> Result<(), Error> {
        let (cb, cr) = picture.chroma();
        let luma = picture.luma();
        // What the container says it shows, where that is inside the
        // picture the bitstream codes (see `visible`).
        let (shown_w, shown_h) = (
            visible(self.shown.0, luma.width()),
            visible(self.shown.1, luma.height()),
        );
        let width = u32::try_from(shown_w).unwrap_or(u32::MAX);
        let height = u32::try_from(shown_h).unwrap_or(u32::MAX);
        let bytes = u64::try_from(
            shown_w.saturating_mul(shown_h)
                + 2 * shown_w.div_ceil(2).saturating_mul(shown_h.div_ceil(2)),
        )
        .unwrap_or(u64::MAX);
        self.limits.check_frame(width, height, bytes)?;
        self.decoded_bytes = self.decoded_bytes.saturating_add(bytes);
        if self.decoded_bytes > self.limits.max_decoded_bytes {
            return Err(Error::new(
                ErrorKind::Limit,
                "the frames decoded exceed the decode limits",
            ));
        }
        if self.pending_bytes.saturating_add(bytes) > self.limits.max_pending_bytes {
            return Err(Error::new(
                ErrorKind::Limit,
                "the frames waiting to be received exceed the decode limits",
            ));
        }
        let planes = vec![
            packed(luma, shown_w, shown_h)?,
            packed(cb, shown_w.div_ceil(2), shown_h.div_ceil(2))?,
            packed(cr, shown_w.div_ceil(2), shown_h.div_ceil(2))?,
        ];
        let picture = Picture::new(width, height, PixelFormat::YUV420P, planes)
            .map_err(|e| Error::new(ErrorKind::Backend, format!("decoded picture: {e}")))?;
        self.pending_bytes = self.pending_bytes.saturating_add(bytes);
        self.ready.push_back((
            Frame {
                pts,
                color: self.color,
                pixels: Pixels::Cpu(picture),
            },
            bytes,
        ));
        Ok(())
    }

    /// Decodes the NAL units of one access unit, and returns the first
    /// error.
    ///
    /// Every NAL unit is decoded and the access unit ended even after one
    /// fails. With threads the decoder reports a slice's error from a later
    /// call than the one that sent it — possibly a NAL unit of a later
    /// packet — so stopping at the first error would leave out NAL units
    /// depending on how far the threads had got, and a caller that carries
    /// on would get other frames than with one thread.
    ///
    /// The pictures the decoder finishes are turned into frames after each
    /// NAL unit, not once the packet is done: a packet may carry many
    /// pictures, and the decoder would hold every one released meanwhile,
    /// beyond what its memory budget counts. The frames wait for the
    /// caller within `max_pending_bytes`. The first error of the frames,
    /// if there is one, comes after the decoding's; a frame over the
    /// limits drops the frames after it that the same NAL unit released,
    /// not the rest of the packet's.
    fn decode_access_unit(&mut self, data: &[u8]) -> Result<(), Error> {
        let mut first = Ok(());
        let mut framed = Ok(());
        let mut note = |this: &mut Self, result: coreav_h264::Result<()>| {
            if let Err(e) = result {
                if first.is_ok() {
                    first = Err(Error::new(kind_of(&e), e.to_string()));
                }
            }
            if let Err(e) = this.collect() {
                if framed.is_ok() {
                    framed = Err(e);
                }
            }
        };
        match self.length_size {
            Some(size) => {
                for unit in coreav_h264::length_prefixed(data, size) {
                    match unit {
                        Ok(unit) => {
                            let result = self.decoder.decode_nal(unit);
                            note(self, result);
                        }
                        // The framing is broken: nothing after it can be
                        // found.
                        Err(e) => {
                            note(self, Err(e));
                            break;
                        }
                    }
                }
            }
            None => {
                for unit in coreav_h264::annex_b(data) {
                    let result = self.decoder.decode_nal(unit);
                    note(self, result);
                }
            }
        }
        // One packet is one access unit, so the picture it carries ends
        // here: its frame is ready without waiting for the packet after
        // it.
        let result = self.decoder.end_access_unit();
        note(self, result);
        first.and(framed)
    }
}

impl Decoder for NativeDecoder {
    fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
        self.check_deadline()?;
        self.limits.check_packet(packet)?;
        if self.eof {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "a packet after the end of the stream",
            ));
        }
        let mark = self.next_mark;
        self.next_mark = self.next_mark.wrapping_add(1);
        self.decoder.set_mark(mark);
        while self.marks.len() >= MAX_PENDING_MARKS + self.threads {
            self.marks.pop_front();
        }
        self.marks.push_back(Mark {
            mark,
            pts: packet.pts,
            discard: packet.discard,
        });
        // The frames are collected even when the packet failed, so that a
        // caller that carries on gets the same frames whatever the thread
        // count; one that stops at the error never asks for them.
        self.decode_access_unit(packet.data)
    }

    fn send_eof(&mut self) -> Result<(), Error> {
        self.check_deadline()?;
        if !self.eof {
            self.eof = true;
            // With threads, an error of a slice not reported yet comes
            // from here (`coreav_h264::Decoder::flush`).
            let flushed = self
                .decoder
                .flush()
                .map_err(|e| Error::new(kind_of(&e), e.to_string()));
            let collected = self.collect();
            return flushed.and(collected);
        }
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<Option<Frame>, Error> {
        self.check_deadline()?;
        let Some((frame, bytes)) = self.ready.pop_front() else {
            return Ok(None);
        };
        self.pending_bytes = self.pending_bytes.saturating_sub(bytes);
        Ok(Some(frame))
    }

    fn info(&self) -> &DecoderInfo {
        &self.info
    }
}

/// The threads to decode with where the caller does not say: the host's
/// cores.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn host_threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

/// `wasm32-unknown-unknown` cannot start a thread: one.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn host_threads() -> usize {
    1
}

/// When a decode that starts now must be over.
///
/// `wasm32-unknown-unknown` has no clock in its standard library:
/// [`Instant::now`] is not implemented there, and a decoder that called
/// it would abort the module. The host that runs the module keeps the
/// time instead, as it keeps the memory (`docs/design/h264.md`, "The
/// decode-boundary backend"); every other limit is enforced here as
/// usual.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn deadline(limits: &Limits) -> Option<Instant> {
    Some(limits.deadline())
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn deadline(_limits: &Limits) -> Option<Instant> {
    None
}

/// One plane's first `height` rows, `width` samples each, packed without
/// padding.
fn packed(rows: PlaneRef<'_>, width: usize, height: usize) -> Result<Plane, Error> {
    let mut data = Vec::new();
    data.try_reserve_exact(width.saturating_mul(height))
        .map_err(|_| Error::new(ErrorKind::Limit, "out of memory for a decoded frame"))?;
    for row in rows.rows().take(height) {
        data.extend_from_slice(row.get(..width).unwrap_or(row));
    }
    Ok(Plane::new(data, width))
}

/// How much of a decoded picture a frame shows, given what the container
/// declares.
///
/// A container may say that the picture the bitstream codes shows fewer
/// samples than it holds: an MP4 whose sample entry is 854 wide for a
/// stream whose sequence parameter set crops to 864, say. FFmpeg honours
/// that, so the sandboxed fallback's frames are the smaller size, and two
/// backends of the same boundary must not return different pictures for
/// the same file.
///
/// Only a declaration inside the coded picture and within one macroblock
/// of it is honoured: that is the padding a container trims, and a
/// container that says something else — a thumbnail-sized width, or
/// nothing at all — is metadata this decoder does not trust against the
/// bitstream.
fn visible(declared: Option<u32>, decoded: usize) -> usize {
    match declared.map(|d| d as usize) {
        // Inside the picture, within a macroblock of it, and still most
        // of it: a 16-sample picture whose container says 1 is not a
        // picture with padding.
        Some(d) if d <= decoded && decoded - d < 16 && d.saturating_mul(2) > decoded => d,
        _ => decoded,
    }
}

/// The boundary's limits as the decoder's own, each no wider than what
/// the decoder allows by default.
fn decode_limits(limits: &Limits) -> coreav_h264::Limits {
    let default = coreav_h264::Limits::default();
    // A picture is coded in whole macroblocks and cropped to what it
    // shows, so a stream the boundary allows at 1920x1080 codes 120 by
    // 68 macroblocks, not 120 by 67: every bound here counts the coded
    // picture, or a frame the boundary allows would be refused for a
    // limit the caller did not set.
    let (wide, high) = (
        u64::from(limits.max_width).div_ceil(16),
        u64::from(limits.max_height).div_ceil(16),
    );
    let by_size = wide.saturating_mul(high);
    // What `max_pixels` allows, with the padding of one macroblock row
    // and column that cropping hides.
    let by_pixels = limits
        .max_pixels
        .div_ceil(256)
        .saturating_add(wide)
        .saturating_add(high);
    let max_mbs = usize::try_from(by_pixels.min(by_size))
        .unwrap_or(usize::MAX)
        .clamp(1, default.max_mbs);
    coreav_h264::Limits {
        max_mbs,
        max_dpb_frames: usize::try_from(limits.max_reference_frames)
            .unwrap_or(default.max_dpb_frames)
            .clamp(1, default.max_dpb_frames),
        max_nal_bytes: usize::try_from(limits.max_packet_bytes)
            .unwrap_or(default.max_nal_bytes)
            .clamp(1, default.max_nal_bytes),
        max_memory_bytes: limits.max_memory_bytes,
        ..default
    }
}

/// Which decode error a decoder error is.
fn kind_of(error: &coreav_h264::Error) -> ErrorKind {
    match error {
        coreav_h264::Error::Unsupported(_) => ErrorKind::Unsupported,
        coreav_h264::Error::Limit(_) => ErrorKind::Limit,
        coreav_h264::Error::Geometry(_) => ErrorKind::Backend,
        coreav_h264::Error::Truncated(_) | coreav_h264::Error::Invalid(_) => ErrorKind::InvalidData,
    }
}

/// What the backend reads from an `AVCDecoderConfigurationRecord`.
struct Avcc<'a> {
    length_size: usize,
    parameter_sets: Vec<&'a [u8]>,
}

/// Reads an `avcC` (ISO/IEC 14496-15 5.3.3.1): the length size, then the
/// sequence and picture parameter sets it carries.
fn parse_avcc(data: &[u8]) -> Result<Avcc<'_>, Error> {
    let invalid = |what: &str| Error::new(ErrorKind::InvalidData, format!("avcC: {what}"));
    let length_size = match data.get(4) {
        Some(byte) => usize::from(byte & 3) + 1,
        None => return Err(invalid("shorter than its header")),
    };
    if length_size == 3 {
        return Err(invalid("a NAL unit length of three bytes"));
    }
    let mut parameter_sets = Vec::new();
    let mut at = 5usize;
    for kind in 0..2 {
        let count = match data.get(at) {
            // The high bits of the sequence parameter set count are
            // reserved; the picture parameter set count is a whole byte.
            Some(byte) if kind == 0 => usize::from(byte & 0x1f),
            Some(byte) => usize::from(*byte),
            None => return Err(invalid("ends before its parameter sets")),
        };
        at += 1;
        for _ in 0..count {
            let (Some(high), Some(low)) = (data.get(at), data.get(at + 1)) else {
                return Err(invalid("ends inside the length of a parameter set"));
            };
            let length = usize::from(u16::from_be_bytes([*high, *low]));
            at += 2;
            let end = at.checked_add(length).ok_or_else(|| invalid("overflows"))?;
            let unit = data
                .get(at..end)
                .ok_or_else(|| invalid("a parameter set past the end of the record"))?;
            at = end;
            if !unit.is_empty() {
                parameter_sets.push(unit);
            }
        }
    }
    Ok(Avcc {
        length_size,
        parameter_sets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Version 1, profile 66, level 10, a length size of four, one
    /// sequence parameter set of three bytes and one picture parameter
    /// set of two.
    const RECORD: [u8; 16] = [
        1, 66, 0, 10, 0xff, 0xe1, 0, 3, 0x67, 0x42, 0x0a, 1, 0, 2, 0x68, 0xce,
    ];

    #[test]
    fn an_avcc_record_gives_its_length_size_and_parameter_sets() {
        let record = parse_avcc(&RECORD).expect("record");
        assert_eq!(record.length_size, 4);
        assert_eq!(
            record.parameter_sets,
            vec![&[0x67u8, 0x42, 0x0a][..], &[0x68, 0xce][..]]
        );
    }

    #[test]
    fn a_record_that_ends_early_is_refused_and_never_panics() {
        for n in 0..RECORD.len() {
            let short = RECORD.get(..n).expect("prefix");
            if n < 6 {
                assert!(parse_avcc(short).is_err(), "{n} bytes");
            } else {
                // Longer prefixes either end inside a parameter set, or
                // on a boundary and read what is there.
                let _ = parse_avcc(short);
            }
        }
    }

    #[test]
    fn a_record_that_lies_about_a_parameter_set_length_is_refused() {
        let mut data = RECORD;
        if let Some(byte) = data.get_mut(7) {
            *byte = 0xff;
        }
        assert!(parse_avcc(&data).is_err());
    }

    #[test]
    fn the_backend_leaves_what_it_does_not_decode_to_another() {
        let backend = NativeH264::new();
        let mut stream = StreamConfig::new(Codec::H264);
        // High 4:2:2 (profile_idc 122), 10-bit: the record's own fields
        // say so before a packet is read.
        stream.config = Some(coreav_decode::CodecConfig {
            kind: ConfigKind::Avcc,
            data: vec![1, 122, 0, 40, 0xff, 0xe0, 0, 0xfd, 0xf8, 0xf8, 0, 0, 2],
        });
        let traits = stream.traits();
        assert!(matches!(backend.supports(&stream, &traits), Support::No(_)));

        // A stream with no configuration record is this backend's, and
        // its parameter sets are checked when they arrive.
        let plain = StreamConfig::new(Codec::H264);
        assert_eq!(backend.supports(&plain, &plain.traits()), Support::Yes);
    }

    #[test]
    fn a_monochrome_stream_is_this_backends() {
        // High profile with chroma_format_idc 0, whose pictures the decoder
        // gives as 4:2:0 with neutral chroma (docs/design/h264.md,
        // "Monochrome").
        let backend = NativeH264::new();
        let mut stream = StreamConfig::new(Codec::H264);
        stream.config = Some(coreav_decode::CodecConfig {
            kind: ConfigKind::Avcc,
            data: vec![
                1, 100, 0, 40, 0xff, 0xe1, 0, 7, 0x67, 100, 0, 40, 0xf3, 0x69, 0xe4, 1, 0, 4, 0x68,
                0xce, 0x38, 0x80,
            ],
        });
        let traits = stream.traits();
        assert_eq!(traits.chroma, Some(Chroma::Mono));
        assert_eq!(backend.supports(&stream, &traits), Support::Yes);
    }

    #[test]
    fn a_container_may_trim_the_padding_of_a_coded_picture() {
        // What an MP4 whose sample entry is 854 wide says about a stream
        // the sequence parameter set crops to 864, and what a QuickTime
        // file says about an odd size.
        assert_eq!(visible(Some(854), 864), 854);
        assert_eq!(visible(Some(375), 376), 375);
        // Nothing to trim.
        assert_eq!(visible(Some(864), 864), 864);
        assert_eq!(visible(None, 864), 864);
        // Metadata this decoder does not trust against the bitstream: a
        // declaration further than a macroblock inside the picture, one
        // larger than it, zero, and one that would take most of a small
        // picture.
        assert_eq!(visible(Some(848), 864), 864);
        assert_eq!(visible(Some(320), 864), 864);
        assert_eq!(visible(Some(900), 864), 864);
        assert_eq!(visible(Some(0), 864), 864);
        assert_eq!(visible(Some(1), 16), 16);
        assert_eq!(visible(Some(8), 16), 16);
        assert_eq!(visible(Some(9), 16), 9);
    }

    #[test]
    fn the_memory_budget_is_the_decoders() {
        let limits = Limits {
            max_memory_bytes: 123 << 20,
            ..Limits::default()
        };
        assert_eq!(decode_limits(&limits).max_memory_bytes, 123 << 20);
        // 1920x1080 is coded as 120 by 68 macroblocks, and more threads
        // need more.
        let own = decode_limits(&limits);
        let one = NativeH264::memory_needed(1920, 1080, 1, &limits);
        assert_eq!(
            one,
            coreav_h264::Decoder::<Auto>::memory_needed(120, 68, 1, &own)
        );
        assert!(NativeH264::memory_needed(1920, 1080, 8, &limits) > one);
        // The defaults keep every thread a caller may ask for up to 4k.
        let roomy = Limits::default();
        let most = decode_limits(&roomy).max_threads;
        assert!(NativeH264::memory_needed(3840, 2160, most, &roomy) <= roomy.max_memory_bytes);
    }

    #[test]
    fn the_boundarys_limits_become_the_decoders_own() {
        let limits = Limits {
            max_width: 1920,
            max_height: 1080,
            max_pixels: 1920 * 1080,
            max_reference_frames: 4,
            max_packet_bytes: 1 << 20,
            ..Limits::default()
        };
        let own = decode_limits(&limits);
        // 1920x1080 is coded as 120 by 68 macroblocks: the picture holds
        // 1088 rows and shows 1080.
        assert_eq!(own.max_mbs, 120 * 68);
        assert_eq!(own.max_dpb_frames, 4);
        assert_eq!(own.max_nal_bytes, 1 << 20);

        // Wider limits than the decoder's own do not widen it.
        let default = coreav_h264::Limits::default();
        let wide = decode_limits(&Limits {
            max_width: u32::MAX,
            max_height: u32::MAX,
            max_pixels: u64::MAX,
            max_reference_frames: u32::MAX,
            max_packet_bytes: u64::MAX,
            ..Limits::default()
        });
        assert_eq!(wide.max_mbs, default.max_mbs);
        assert_eq!(wide.max_dpb_frames, default.max_dpb_frames);
        assert_eq!(wide.max_nal_bytes, default.max_nal_bytes);
    }
}
