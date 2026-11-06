use std::{collections::VecDeque, time::Instant};

use coreav_decode::{Error, Limits};
use coreav_frame::{Chroma, ChromaLocation, ColorInfo, Frame, Picture, PixelFormat, Pixels, Plane};
use coreav_hevc::{DecodedPicture, OutputMemory};

use crate::{check_deadline, codec_error, invalid, limit};

struct Entry {
    frame: Frame,
    bytes: u64,
    memory: usize,
}

pub(super) struct Output {
    pub(super) limits: Limits,
    deadline: Instant,
    color: ColorInfo,
    ready: VecDeque<Entry>,
    pending: u64,
    pending_memory: usize,
    decoded: u64,
    owner_bytes: usize,
}

impl Output {
    pub(super) fn new(
        limits: Limits,
        deadline: Instant,
        color: ColorInfo,
        name_capacity: usize,
    ) -> Result<Self, Error> {
        check_deadline(deadline)?;
        let owner_bytes = core::mem::size_of::<super::NativeDecoder>()
            .checked_add(name_capacity)
            .ok_or(limit("HEVC owner size overflow"))?;
        Ok(Self {
            limits,
            deadline,
            color,
            ready: VecDeque::new(),
            pending: 0,
            pending_memory: 0,
            decoded: 0,
            owner_bytes,
        })
    }

    pub(super) fn check(&self) -> Result<(), Error> {
        check_deadline(self.deadline)
    }
    pub(super) fn is_empty(&self) -> bool {
        self.ready.is_empty()
    }

    pub(super) fn clear(&mut self) {
        self.ready = VecDeque::new();
        self.pending = 0;
        self.pending_memory = 0;
    }

    pub(super) fn retained_bytes(&self) -> Result<usize, Error> {
        self.ready
            .capacity()
            .checked_mul(core::mem::size_of::<Entry>())
            .and_then(|n| n.checked_add(self.pending_memory))
            .and_then(|n| n.checked_add(self.owner_bytes))
            .ok_or(limit("HEVC output memory overflow"))
    }

    pub(super) fn pop(&mut self) -> Option<Frame> {
        self.ready.pop_front().map(|entry| {
            self.pending -= entry.bytes;
            self.pending_memory -= entry.memory;
            entry.frame
        })
    }

    pub(super) fn push(
        &mut self,
        picture: DecodedPicture<'_>,
        memory: &mut OutputMemory<'_>,
    ) -> Result<(), Error> {
        self.check()?;
        let origin = picture
            .origin
            .ok_or(invalid("HEVC output has no packet identity"))?;
        if origin & 1 != 0 {
            return Ok(());
        }
        let info = picture.info;
        if info.width == 0
            || info.height == 0
            || info.width % 2 != 0
            || info.height % 2 != 0
            || info.visible_width % 2 != 0
            || info.visible_height % 2 != 0
        {
            return Err(invalid("invalid HEVC output geometry"));
        }
        let [left, right, top, bottom] = info.conformance_window;
        let cropped = |coded: u32, first: u32, last: u32| {
            first
                .checked_add(last)
                .and_then(|n| n.checked_mul(2))
                .and_then(|n| coded.checked_sub(n))
                .filter(|n| *n != 0)
        };
        if cropped(info.width, left, right) != Some(info.visible_width)
            || cropped(info.height, top, bottom) != Some(info.visible_height)
        {
            return Err(invalid("invalid HEVC conformance crop"));
        }
        let sample_bytes = match info.bit_depth {
            8 => 1usize,
            10 => 2usize,
            _ => return Err(invalid("invalid HEVC output sample depth")),
        };
        let bytes = (u64::from(info.visible_width) * u64::from(info.visible_height))
            .checked_mul(3)
            .and_then(|n| n.checked_div(2))
            .and_then(|n| n.checked_mul(sample_bytes as u64))
            .ok_or(limit("HEVC output size overflow"))?;
        self.limits
            .check_frame(info.visible_width, info.visible_height, bytes)?;
        let pending = self
            .pending
            .checked_add(bytes)
            .filter(|n| *n <= self.limits.max_pending_bytes)
            .ok_or(limit("pending HEVC frames exceed the output budget"))?;
        let decoded = self
            .decoded
            .checked_add(bytes)
            .filter(|n| *n <= self.limits.max_decoded_bytes)
            .ok_or(limit("decoded HEVC frames exceed the output budget"))?;
        if self.ready.len() == self.ready.capacity() {
            let old = self
                .ready
                .capacity()
                .checked_mul(core::mem::size_of::<Entry>())
                .ok_or(limit("HEVC queue size overflow"))?;
            let requested = self
                .ready
                .len()
                .checked_add(1)
                .and_then(|n| n.checked_mul(core::mem::size_of::<Entry>()))
                .ok_or(limit("HEVC queue size overflow"))?;
            memory.reserve(requested).map_err(codec_error)?;
            self.ready
                .try_reserve_exact(1)
                .map_err(|_| limit("HEVC queue allocation"))?;
            let actual = self
                .ready
                .capacity()
                .checked_mul(core::mem::size_of::<Entry>())
                .ok_or(limit("HEVC queue size overflow"))?;
            memory
                .reserve(
                    actual
                        .checked_sub(requested)
                        .ok_or(invalid("HEVC queue capacity"))?,
                )
                .map_err(codec_error)?;
            memory.release(old).map_err(codec_error)?;
        }
        let mut planes = Vec::new();
        memory
            .reserve(3 * core::mem::size_of::<Plane>())
            .map_err(codec_error)?;
        planes
            .try_reserve_exact(3)
            .map_err(|_| limit("HEVC plane descriptors allocation"))?;
        let mut owned = planes
            .capacity()
            .checked_mul(core::mem::size_of::<Plane>())
            .ok_or(limit("HEVC plane descriptor size overflow"))?;
        memory
            .reserve(
                owned
                    .checked_sub(3 * core::mem::size_of::<Plane>())
                    .ok_or(invalid("HEVC plane capacity"))?,
            )
            .map_err(codec_error)?;
        for (component, source) in picture.planes.into_iter().enumerate() {
            let scale = if component == 0 { 2usize } else { 1 };
            let width = info.visible_width as usize * scale / 2;
            let height = info.visible_height as usize * scale / 2;
            let left = info
                .conformance_window
                .first()
                .copied()
                .ok_or(invalid("HEVC crop offsets"))? as usize
                * scale;
            let top = info
                .conformance_window
                .get(2)
                .copied()
                .ok_or(invalid("HEVC crop offsets"))? as usize
                * scale;
            let stride = width
                .checked_mul(sample_bytes)
                .ok_or(limit("HEVC row size overflow"))?;
            let length = stride
                .checked_mul(height)
                .ok_or(limit("HEVC plane size overflow"))?;
            memory.reserve(length).map_err(codec_error)?;
            let mut data = Vec::new();
            data.try_reserve_exact(length)
                .map_err(|_| limit("HEVC plane allocation"))?;
            memory
                .reserve(
                    data.capacity()
                        .checked_sub(length)
                        .ok_or(invalid("HEVC plane capacity"))?,
                )
                .map_err(codec_error)?;
            owned = owned
                .checked_add(data.capacity())
                .ok_or(limit("HEVC output capacity overflow"))?;
            data.resize(length, 0);
            for (row, destination) in data.chunks_exact_mut(stride).enumerate() {
                let start = top
                    .checked_add(row)
                    .and_then(|n| n.checked_mul(source.stride))
                    .and_then(|n| n.checked_add(left))
                    .ok_or(limit("HEVC crop offset overflow"))?;
                let end = start
                    .checked_add(width)
                    .ok_or(limit("HEVC crop offset overflow"))?;
                let samples = source
                    .samples
                    .get(start..end)
                    .ok_or(invalid("HEVC cropped plane is short"))?;
                for (sample, destination) in samples
                    .iter()
                    .zip(destination.chunks_exact_mut(sample_bytes))
                {
                    if u32::from(*sample) >= 1u32 << info.bit_depth {
                        return Err(invalid("HEVC output sample exceeds its depth"));
                    }
                    let packed = sample.to_le_bytes();
                    destination.copy_from_slice(
                        packed
                            .get(..sample_bytes)
                            .ok_or(invalid("HEVC sample width"))?,
                    );
                }
            }
            planes.push(Plane::new(data, stride));
        }
        let pixels = Picture::new(
            info.visible_width,
            info.visible_height,
            PixelFormat::Yuv {
                chroma: Chroma::Cs420,
                depth: info.bit_depth as u8,
            },
            planes,
        )?;
        let vui = info.vui;
        let code = |value: Option<u32>| value.and_then(|n| u8::try_from(n).ok());
        let color = ColorInfo {
            primaries: code(vui.colour_primaries),
            transfer: code(vui.transfer_characteristics),
            matrix: code(vui.matrix_coeffs),
            full_range: vui.full_range,
            chroma_location: vui
                .chroma_location
                .and_then(|(top, _)| code(Some(top)))
                .and_then(ChromaLocation::from_code),
        }
        .or(self.color);
        self.pending_memory = self
            .pending_memory
            .checked_add(owned)
            .ok_or(limit("HEVC output memory overflow"))?;
        self.ready.push_back(Entry {
            frame: Frame {
                pts: picture.timestamp,
                color,
                pixels: Pixels::Cpu(pixels),
            },
            bytes,
            memory: owned,
        });
        self.pending = pending;
        self.decoded = decoded;
        self.check()
    }
}
