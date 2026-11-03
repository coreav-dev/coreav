//! The opt-in scalar profile-0 VP9 backend of the decode boundary.
//! It is not part of default or release builds (`docs/design/vp9.md`).

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::time::Instant;

use coreav_decode::{
    Backend, BackendKind, Codec, ConfigKind, Decoder, DecoderInfo, Error, ErrorKind, Isolation,
    Limits, Packet, StreamConfig, StreamTraits, Support,
};
use coreav_frame::{Chroma, ColorInfo, Frame, Picture, PixelFormat, Pixels, Plane};
use coreav_vp9::{ColorSpace, FrameDecoder, Samples};

/// Options of the scalar native VP9 backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeOptions {
    /// Decode keyframes and show their references only while valid.
    pub keyframes_only: bool,
}

/// CoreAV's native 8-bit 4:2:0 VP9 backend, in the calling process.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeVp9 {
    options: NativeOptions,
}

impl NativeVp9 {
    /// Decodes every frame, using scalar kernels.
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes with `options`.
    pub fn with_options(options: NativeOptions) -> Self {
        Self { options }
    }
}

impl Backend for NativeVp9 {
    fn kind(&self) -> BackendKind {
        BackendKind::Native
    }

    fn supports(&self, stream: &StreamConfig, traits: &StreamTraits) -> Support {
        match configuration(stream).and_then(|()| scope(traits)) {
            Ok(()) => Support::Yes,
            Err(error) => Support::No(error.message),
        }
    }

    fn open(&self, stream: &StreamConfig, limits: &Limits) -> Result<Box<dyn Decoder>, Error> {
        configuration(stream)?;
        scope(&stream.traits())?;
        if limits.max_reference_frames < 8 {
            return Err(limit("VP9 requires eight reference slots"));
        }
        if stream.width.is_some_and(|width| width > limits.max_width)
            || stream
                .height
                .is_some_and(|height| height > limits.max_height)
        {
            return Err(limit("declared VP9 dimensions exceed the frame limit"));
        }
        if let (Some(width), Some(height)) = (stream.width, stream.height) {
            limits.check_frame(width, height, frame_bytes(width, height))?;
        }
        let own = coreav_vp9::Limits {
            max_width: limits.max_width,
            max_height: limits.max_height,
            max_pixels: limits.max_pixels,
            max_packet_bytes: usize::try_from(limits.max_packet_bytes).unwrap_or(usize::MAX),
            max_memory_bytes: limits.max_memory_bytes,
        };
        let decoder = FrameDecoder::new(own);
        Ok(Box::new(NativeDecoder {
            decoder: if self.options.keyframes_only {
                decoder.with_keyframes_only()
            } else {
                decoder
            },
            limits: limits.clone(),
            deadline: limits.deadline(),
            color: stream.color,
            info: DecoderInfo {
                backend: BackendKind::Native,
                isolation: Isolation::InProcess,
                decoder: "coreav vp9, scalar profile 0".into(),
            },
            ready: VecDeque::new(),
            pending: 0,
            pending_memory: 0,
            decoded: 0,
            eof: false,
            failed: false,
        }))
    }
}

fn limit(reason: &'static str) -> Error {
    Error::new(ErrorKind::Limit, reason)
}
fn invalid(reason: &'static str) -> Error {
    Error::new(ErrorKind::InvalidData, reason)
}
fn unsupported(reason: &'static str) -> Error {
    Error::new(ErrorKind::Unsupported, reason)
}

fn scope(traits: &StreamTraits) -> Result<(), Error> {
    if traits.profile.is_some_and(|p| p != 0)
        || traits.bit_depth.is_some_and(|d| d != 8)
        || traits.chroma.is_some_and(|c| c != Chroma::Cs420)
    {
        return Err(unsupported(
            "native VP9 accepts profile 0, 8-bit 4:2:0 only",
        ));
    }
    Ok(())
}

fn configuration(stream: &StreamConfig) -> Result<(), Error> {
    if stream.codec != Codec::Vp9 {
        return Err(unsupported("this backend decodes VP9 only"));
    }
    let Some(config) = &stream.config else {
        return Ok(());
    };
    let data = config.data.as_slice();
    if data.len() > 65_547 {
        return Err(invalid("VP9 configuration is too large"));
    }
    match config.kind {
        ConfigKind::Vpcc => {
            let Some((&profile, tail)) = data.get(4..).and_then(<[u8]>::split_first) else {
                return Err(invalid("truncated vpcC"));
            };
            if !data.starts_with(&[1, 0, 0, 0]) || tail.len() < 7 {
                return Err(invalid("invalid vpcC header"));
            }
            let packed = data.get(6).copied().ok_or(invalid("truncated vpcC"))?;
            let length = data
                .get(10..12)
                .and_then(|d| d.first_chunk::<2>())
                .map(|b| usize::from(u16::from_be_bytes(*b)))
                .ok_or(invalid("truncated vpcC"))?;
            if data.len() != 12 + length {
                return Err(invalid("vpcC initialization length"));
            }
            if length != 0 {
                return Err(unsupported("VP9 initialization data is not supported"));
            }
            if profile != 0 || packed >> 4 != 8 || ((packed >> 1) & 7) > 1 {
                return Err(unsupported("vpcC is outside VP9 profile 0"));
            }
        }
        ConfigKind::MatroskaPrivate => {
            let mut rest = data;
            let mut seen = 0u8;
            while !rest.is_empty() {
                let [id, len, tail @ ..] = rest else {
                    return Err(invalid("truncated VP9 feature"));
                };
                let n = usize::from(*len);
                let (Some(value), Some(next)) = (tail.get(..n), tail.get(n..)) else {
                    return Err(invalid("truncated VP9 feature value"));
                };
                if (1..=4).contains(id) {
                    let mask = 1u8 << id;
                    if n != 1 || seen & mask != 0 {
                        return Err(invalid("duplicate or invalid VP9 feature"));
                    }
                    seen |= mask;
                    let v = value.first().copied().ok_or(invalid("empty VP9 feature"))?;
                    if (*id == 1 && v != 0) || (*id == 3 && v != 8) || (*id == 4 && v > 1) {
                        return Err(unsupported("VP9 features are outside profile 0"));
                    }
                }
                rest = next;
            }
        }
        _ => return Err(unsupported("VP9 requires vpcC or Matroska codec features")),
    }
    scope(&stream.traits())
}

fn frame_bytes(width: u32, height: u32) -> u64 {
    (u64::from(width) * u64::from(height))
        .saturating_add(2 * u64::from(width.div_ceil(2)) * u64::from(height.div_ceil(2)))
}

struct NativeDecoder {
    decoder: FrameDecoder,
    info: DecoderInfo,
    limits: Limits,
    deadline: Instant,
    color: ColorInfo,
    ready: VecDeque<(Frame, u64, u64)>,
    pending: u64,
    pending_memory: u64,
    decoded: u64,
    eof: bool,
    failed: bool,
}

impl NativeDecoder {
    fn check(&self) -> Result<(), Error> {
        if self.failed {
            return Err(invalid("the VP9 decode request failed"));
        }
        if Instant::now() >= self.deadline {
            return Err(Error::new(ErrorKind::Timeout, "the decode deadline passed"));
        }
        Ok(())
    }

    fn fail(&mut self) {
        self.failed = true;
        self.ready.clear();
        self.pending = 0;
        self.pending_memory = 0;
    }

    fn send(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
        self.check()?;
        if self.eof {
            return Err(invalid("a packet after the end of the stream"));
        }
        self.limits.check_packet(packet)?;
        let retained = self.decoder.retained_memory_bytes().map_err(codec_error)?;
        let old_capacity = self.ready.capacity();
        let requested = self
            .ready
            .len()
            .checked_add(if packet.discard { 0 } else { 8 })
            .ok_or(limit("VP9 queue length overflow"))?
            .max(self.ready.capacity());
        let grows = requested > old_capacity;
        let old_queue_bytes = if grows {
            old_capacity
                .checked_mul(core::mem::size_of::<(Frame, u64, u64)>())
                .and_then(|n| u64::try_from(n).ok())
                .ok_or(limit("VP9 queue capacity overflow"))?
        } else {
            0
        };
        let metadata = if packet.discard {
            0
        } else {
            (8 * 3 * core::mem::size_of::<Plane>()) as u64
        };
        let minimum = requested
            .checked_mul(core::mem::size_of::<(Frame, u64, u64)>())
            .and_then(|n| u64::try_from(n).ok())
            .and_then(|n| n.checked_add(self.pending_memory))
            .and_then(|n| n.checked_add(old_queue_bytes))
            .and_then(|n| n.checked_add(metadata))
            .and_then(|n| n.checked_add(retained))
            .ok_or(limit("VP9 queue memory overflow"))?;
        if minimum > self.limits.max_memory_bytes {
            return Err(limit("VP9 queue exceeds memory budget"));
        }
        if !packet.discard {
            self.ready
                .try_reserve_exact(8)
                .map_err(|_| limit("out of memory for pending VP9 frames"))?;
        }
        let queue_bytes = self
            .ready
            .capacity()
            .checked_mul(core::mem::size_of::<(Frame, u64, u64)>())
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(limit("VP9 queue capacity overflow"))?;
        let budget = self
            .limits
            .max_memory_bytes
            .checked_sub(self.pending_memory)
            .and_then(|n| n.checked_sub(queue_bytes))
            .and_then(|n| n.checked_sub(old_queue_bytes))
            .and_then(|n| n.checked_sub(metadata))
            .ok_or(limit("pending frames exceed VP9 memory budget"))?;
        let pictures = self
            .decoder
            .decode_with_budget(
                packet.data,
                !packet.discard,
                budget,
                self.limits.max_frame_bytes,
            )
            .map_err(codec_error)?;
        for picture in pictures {
            self.check()?;
            let bytes = frame_bytes(picture.width, picture.height);
            self.limits
                .check_frame(picture.width, picture.height, bytes)?;
            let decoded = self
                .decoded
                .checked_add(bytes)
                .filter(|n| *n <= self.limits.max_decoded_bytes)
                .ok_or(limit("decoded VP9 frames exceed the output budget"))?;
            let pending = self
                .pending
                .checked_add(bytes)
                .filter(|n| *n <= self.limits.max_pending_bytes)
                .ok_or(limit("pending VP9 frames exceed the output budget"))?;
            let (frame, memory) = convert(picture, packet.pts, self.color)?;
            self.pending_memory = self
                .pending_memory
                .checked_add(memory)
                .ok_or(limit("VP9 memory overflow"))?;
            self.ready.push_back((frame, bytes, memory));
            self.decoded = decoded;
            self.pending = pending;
        }
        self.check()
    }
}

impl Decoder for NativeDecoder {
    fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
        let result = self.send(packet);
        if result.is_err() {
            self.fail();
        }
        result
    }
    fn send_eof(&mut self) -> Result<(), Error> {
        let result = self.check();
        if result.is_err() {
            self.fail();
        }
        result?;
        self.eof = true;
        Ok(())
    }
    fn receive_frame(&mut self) -> Result<Option<Frame>, Error> {
        if let Err(error) = self.check() {
            self.fail();
            return Err(error);
        }
        Ok(self.ready.pop_front().map(|(frame, bytes, memory)| {
            self.pending -= bytes;
            self.pending_memory -= memory;
            frame
        }))
    }
    fn info(&self) -> &DecoderInfo {
        &self.info
    }
}

fn codec_error(error: coreav_vp9::Error) -> Error {
    let kind = match error {
        coreav_vp9::Error::Limit(_) => ErrorKind::Limit,
        coreav_vp9::Error::Unsupported(_) => ErrorKind::Unsupported,
        coreav_vp9::Error::Invalid(_) | coreav_vp9::Error::Truncated(_) => ErrorKind::InvalidData,
    };
    Error::new(kind, error.to_string())
}

fn convert(
    picture: coreav_vp9::Picture,
    pts: Option<i64>,
    color: ColorInfo,
) -> Result<(Frame, u64), Error> {
    if picture.bit_depth != 8 || !picture.subsampling_x || !picture.subsampling_y {
        return Err(unsupported("native VP9 picture is outside profile 0"));
    }
    let matrix = match picture.color_space {
        ColorSpace::Bt601 | ColorSpace::Smpte170 => Some(6),
        ColorSpace::Bt709 => Some(1),
        ColorSpace::Smpte240 => Some(7),
        ColorSpace::Bt2020 => Some(9),
        ColorSpace::Rgb => Some(0),
        ColorSpace::Unknown | ColorSpace::Reserved => None,
    };
    let color = color.or(ColorInfo {
        matrix,
        full_range: Some(picture.full_range),
        ..ColorInfo::default()
    });
    let [Samples::Eight(y), Samples::Eight(u), Samples::Eight(v)] = picture.planes else {
        return Err(invalid("VP9 sample storage differs from its depth"));
    };
    let mut planes = Vec::new();
    planes
        .try_reserve_exact(3)
        .map_err(|_| limit("out of memory for VP9 plane descriptors"))?;
    let memory = y
        .capacity()
        .checked_add(u.capacity())
        .and_then(|n| n.checked_add(v.capacity()))
        .and_then(|n| n.checked_add(planes.capacity() * core::mem::size_of::<Plane>()))
        .and_then(|n| u64::try_from(n).ok())
        .ok_or(limit("VP9 sample capacity overflow"))?;
    planes.push(Plane::new(y, picture.width as usize));
    planes.push(Plane::new(u, picture.width.div_ceil(2) as usize));
    planes.push(Plane::new(v, picture.width.div_ceil(2) as usize));
    let pixels = Picture::new(picture.width, picture.height, PixelFormat::YUV420P, planes)?;
    Ok((
        Frame {
            pts,
            color,
            pixels: Pixels::Cpu(pixels),
        },
        memory,
    ))
}

#[cfg(test)]
mod tests;
