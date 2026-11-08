use std::{collections::VecDeque, time::Instant};

use coreav_decode::native_hevc_private::OutputFrame;
use coreav_decode::{Error, Limits};
use coreav_frame::{Chroma, ChromaLocation, ColorInfo, Frame, Picture, PixelFormat, Pixels, Plane};
use coreav_hevc::{
    DecodedPicture, IntraPlane, OutputMemory, PictureInfo,
    native_rows::{OutputHeader, SinkCost},
};

use crate::{check_deadline, codec_error, invalid, limit};

pub(super) struct Output {
    pub(super) limits: Limits,
    deadline: Instant,
    color: ColorInfo,
    pub(super) ready: VecDeque<OutputFrame>,
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

    pub(super) fn retained_bytes(&self) -> Result<usize, Error> {
        self.ready
            .capacity()
            .checked_mul(core::mem::size_of::<OutputFrame>())
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

    pub(super) fn consume(&mut self, offset: usize) -> Result<(), Error> {
        let entry = self
            .ready
            .get(offset)
            .ok_or(invalid("missing HEVC serial output"))?;
        self.pending = self
            .pending
            .checked_sub(entry.bytes)
            .ok_or(invalid("HEVC pending byte underflow"))?;
        Ok(())
    }

    pub(super) fn pop_rows(&mut self) -> Option<Frame> {
        self.ready.pop_front().map(|entry| {
            self.pending_memory -= entry.memory;
            entry.frame
        })
    }

    #[cfg(test)]
    pub(super) fn push(
        &mut self,
        picture: DecodedPicture<'_>,
        memory: &mut OutputMemory<'_>,
    ) -> Result<(), Error> {
        self.queue(
            OutputHeader {
                index: picture.index,
                poc: picture.poc,
                timestamp: picture.timestamp,
                origin: picture.origin,
                info: picture.info,
            },
            Some(picture.planes),
            memory,
        )?;
        self.check()
    }

    pub(super) fn prepare(
        &mut self,
        header: OutputHeader,
        memory: &mut OutputMemory<'_>,
    ) -> Result<bool, Error> {
        self.queue(header, None, memory)
    }

    fn queue(
        &mut self,
        picture: OutputHeader,
        sources: Option<[IntraPlane<'_>; 3]>,
        memory: &mut OutputMemory<'_>,
    ) -> Result<bool, Error> {
        self.check()?;
        let origin = picture
            .origin
            .ok_or(invalid("HEVC output has no packet identity"))?;
        if origin & 1 != 0 {
            return Ok(false);
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
                .checked_mul(core::mem::size_of::<OutputFrame>())
                .ok_or(limit("HEVC queue size overflow"))?;
            let requested = self
                .ready
                .len()
                .checked_add(1)
                .and_then(|n| n.checked_mul(core::mem::size_of::<OutputFrame>()))
                .ok_or(limit("HEVC queue size overflow"))?;
            memory.reserve(requested).map_err(codec_error)?;
            self.ready
                .try_reserve_exact(1)
                .map_err(|_| limit("HEVC queue allocation"))?;
            let actual = self
                .ready
                .capacity()
                .checked_mul(core::mem::size_of::<OutputFrame>())
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
        for component in 0..3 {
            let scale = if component == 0 { 2usize } else { 1 };
            let width = info.visible_width as usize * scale / 2;
            let height = info.visible_height as usize * scale / 2;
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
            planes.push(Plane::new(data, stride));
        }
        let mut pixels = Picture::new(
            info.visible_width,
            info.visible_height,
            PixelFormat::Yuv {
                chroma: Chroma::Cs420,
                depth: info.bit_depth as u8,
            },
            planes,
        )?;
        if let Some(sources) = sources {
            Self::fill_planes(info, sources, &mut pixels)?;
        }
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
        self.ready.push_back(OutputFrame {
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
        Ok(true)
    }

    pub(super) fn fill(&mut self, offset: usize, picture: DecodedPicture<'_>) -> Result<(), Error> {
        let entry = self
            .ready
            .get_mut(offset)
            .ok_or(invalid("missing prepared HEVC frame"))?;
        let Pixels::Cpu(destination) = &mut entry.frame.pixels else {
            return Err(invalid("prepared HEVC frame is not on CPU"));
        };
        if entry.frame.pts != picture.timestamp
            || (
                destination.width(),
                destination.height(),
                destination.format().depth(),
            ) != (
                picture.info.visible_width,
                picture.info.visible_height,
                picture.info.bit_depth as u8,
            )
        {
            return Err(invalid("prepared HEVC frame identity"));
        }
        Self::fill_planes(picture.info, picture.planes, destination)
    }

    fn fill_planes(
        info: PictureInfo,
        sources: [IntraPlane<'_>; 3],
        destination: &mut Picture,
    ) -> Result<(), Error> {
        let sample_bytes = if info.bit_depth == 8 { 1 } else { 2 };
        for (component, source) in sources.into_iter().enumerate() {
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
            for row in 0..height {
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
                let target = destination
                    .row_mut(component, row)
                    .ok_or(invalid("prepared HEVC row is short"))?;
                for (sample, packed) in samples.iter().zip(target.chunks_exact_mut(sample_bytes)) {
                    if u32::from(*sample) >= 1u32 << info.bit_depth {
                        return Err(invalid("HEVC output sample exceeds its depth"));
                    }
                    packed.copy_from_slice(
                        sample
                            .to_le_bytes()
                            .get(..sample_bytes)
                            .ok_or(invalid("HEVC sample width"))?,
                    );
                }
            }
        }
        Ok(())
    }

    pub(super) fn truncate(&mut self, keep: usize) {
        while self.ready.len() > keep {
            if let Some(entry) = self.ready.pop_back() {
                self.pending_memory -= entry.memory;
            }
        }
        self.pending = self.ready.iter().map(|entry| entry.bytes).sum();
    }

    pub(super) fn cost(&self, debt: usize, frame_bytes: usize) -> coreav_hevc::Result<SinkCost> {
        if debt > self.ready.len() {
            return Err(coreav_hevc::Error::Internal("output_ordinal"));
        }
        let mut remaining = self
            .ready
            .iter()
            .take(debt)
            .try_fold(0usize, |bytes, entry| {
                bytes.checked_add(usize::try_from(entry.bytes).ok()?)
            })
            .ok_or(coreav_hevc::Error::Limit("reconstruction_memory"))?;
        let mut debt_peak_bytes = remaining;
        for (index, entry) in self.ready.iter().take(debt).enumerate() {
            remaining = remaining
                .checked_sub(
                    usize::try_from(entry.bytes)
                        .map_err(|_| coreav_hevc::Error::Limit("reconstruction_memory"))?,
                )
                .ok_or(coreav_hevc::Error::Internal("output_ordinal"))?;
            let future = index
                .checked_add(1)
                .and_then(|count| count.checked_mul(frame_bytes))
                .and_then(|bytes| bytes.checked_add(remaining))
                .ok_or(coreav_hevc::Error::Limit("reconstruction_memory"))?;
            debt_peak_bytes = debt_peak_bytes.max(future);
        }
        Ok(SinkCost {
            max_packet_bytes: usize::try_from(self.limits.max_packet_bytes).unwrap_or(usize::MAX),
            max_nals: super::MAX_NALS,
            max_pending_bytes: self.limits.max_pending_bytes,
            queue_capacity: self.ready.capacity(),
            queued_frames: self.ready.len(),
            record_bytes: core::mem::size_of::<OutputFrame>(),
            plane_records_bytes: 3 * core::mem::size_of::<Plane>(),
            owner_bytes: self.owner_bytes,
            debt_peak_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(width: u32, height: u32) -> Result<OutputFrame, Error> {
        let width = width as usize;
        let height = height as usize;
        let bytes = width * height * 3 / 2;
        let picture = Picture::new(
            width as u32,
            height as u32,
            PixelFormat::Yuv {
                chroma: Chroma::Cs420,
                depth: 8,
            },
            vec![
                Plane::new(vec![0; width * height], width),
                Plane::new(vec![0; width * height / 4], width / 2),
                Plane::new(vec![0; width * height / 4], width / 2),
            ],
        )?;
        Ok(OutputFrame {
            frame: Frame {
                pts: None,
                color: Default::default(),
                pixels: Pixels::Cpu(picture),
            },
            bytes: bytes as u64,
            memory: bytes + 3 * core::mem::size_of::<Plane>(),
        })
    }

    #[test]
    fn debt_capacity_covers_every_old_prefix_replaced_by_new_geometry() -> Result<(), Error> {
        for (sizes, expected) in [([(2, 2), (20, 20)], 960), ([(20, 20), (2, 2)], 720)] {
            let mut output = Output::new(
                Limits::default(),
                Instant::now() + std::time::Duration::from_secs(30),
                Default::default(),
                0,
            )?;
            for (width, height) in sizes {
                output.ready.push_back(packed(width, height)?);
            }
            assert_eq!(output.cost(2, 360).unwrap().debt_peak_bytes, expected);
            assert_eq!(output.cost(0, 360).unwrap().debt_peak_bytes, 0);
            assert!(output.cost(3, 360).is_err());
        }
        Ok(())
    }
}
