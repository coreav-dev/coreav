//! Incremental, bounded AV1 temporal-unit samples for the ISOBMFF binding 2.4.
//!
//! Native packet boundaries and timestamps have no syntax authority here.
//! Delimiters establish temporal units; frame syntax establishes display order.
//! Every returned sample remains provisional until its segment and job succeed.
//! The frame reader checks metadata and tile extents, not entropy-coded content.

use std::fmt;

use crate::{
    frame::{self, Frames},
    obu::{self, ObuKind, Obus},
    sequence::{self, ChromaPosition, ColorRange, Sequence},
};

/// Exact output metadata required by the caller, with no implicit conversion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Expected {
    /// Required coded, upscaled, render and sequence maximum dimensions.
    pub dimensions: frame::Expected,
    /// Required AV1 color-primaries code.
    pub primaries: u8,
    /// Required AV1 transfer-characteristics code.
    pub transfer: u8,
    /// Required AV1 matrix-coefficients code.
    pub matrix: u8,
    /// Required sample range.
    pub range: ColorRange,
    /// Required chroma sample position.
    pub chroma_position: ChromaPosition,
}

/// Independent retained-byte, stream-byte and parsing-work bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Maximum bytes admitted by one push.
    pub max_packet_bytes: usize,
    /// Maximum unconsumed input bytes, including a partial OBU.
    pub max_buffered_bytes: usize,
    /// Maximum complete OBU bytes, including framing.
    pub max_obu_bytes: usize,
    /// Maximum source bytes per temporal unit, including stripped padding.
    pub max_temporal_unit_bytes: usize,
    /// Maximum retained sequence-header OBU bytes.
    pub max_sequence_bytes: usize,
    /// Maximum admitted bytes across all segments.
    pub max_total_bytes: u64,
    /// Maximum OBUs across all segments, including delimiters and padding.
    pub max_obus: u64,
    /// Maximum complete frame operations, including show-existing operations.
    pub max_frames: u64,
    /// Maximum frame operations in one temporal unit.
    pub max_frames_per_temporal_unit: u32,
    /// Maximum displayed temporal units across the job.
    pub max_samples: u64,
    /// Maximum tiles in one coded picture.
    pub max_tiles: u32,
}

/// A stream-syntax, state, allocation or bound failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// Low-overhead OBU framing is invalid.
    Framing(obu::Error),
    /// Sequence-header syntax is invalid or unsupported.
    Sequence(sequence::Error),
    /// Frame-header, reference or tile validation failed.
    Frame(frame::Error),
    /// A caller-selected byte, work, count or allocation bound was exceeded.
    Limit,
    /// An allocation failed.
    Allocation,
    /// The output's sequence metadata differs from the request.
    Metadata,
    /// A sequence header differs byte-for-byte from the job's first header.
    ConfigurationChanged,
    /// The segment or temporal-unit OBU order is invalid.
    Ordering,
    /// A temporal unit does not display exactly one picture.
    DisplayCount,
    /// A segment's first temporal unit is not a sync sample.
    RandomAccess,
    /// The number of presented pictures differs from the segment's submissions.
    PictureCount,
    /// EOF splits an OBU or a frame.
    Truncated,
    /// A complete buffered OBU must be consumed before ending the segment.
    DrainRequired,
    /// This reader is not accepting a segment, or a segment is already open.
    State,
    /// A prior failure permanently closed the reader.
    Failed,
    /// Metadata, redundant headers or tile lists are outside the supported subset.
    UnsupportedObu,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AV1 output validation failed: {self:?}")
    }
}

impl std::error::Error for Error {}

/// Owned low-overhead OBUs for exactly one complete displayed temporal unit.
#[derive(Debug, Eq, PartialEq)]
pub struct Sample {
    /// Complete sample bytes with temporal delimiters and padding removed.
    pub bytes: Vec<u8>,
    /// Global zero-based presentation ordinal, independent of native timestamps.
    pub ordinal: u64,
    /// Whether the sample includes a sequence header and begins with a shown keyframe.
    pub sync: bool,
    /// Segment-local coded-picture identity of the displayed picture.
    pub picture_identity: u64,
    /// Whether the display operation refers to a previously coded hidden picture.
    pub show_existing: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Ready,
    Active,
    Failed,
}

/// One job's byte-identical configuration, continuous ordinals and fresh segment references.
///
/// Call `begin_segment`, then alternate `push` and draining `next_sample` until
/// it returns `None`. At native EOS, drain again and call `end_segment` for the
/// final sample. Native timestamps must be cross-checked by the caller against
/// the returned ordinals and the original source clock, never substituted for
/// those ordinals. Do not publish any spooled sample after a later failure.
#[derive(Debug)]
pub struct Output {
    expected: Expected,
    limits: Limits,
    state: State,
    configuration: Vec<u8>,
    sequence: Option<Sequence>,
    frames: Option<Frames>,
    buffer: Vec<u8>,
    consumed: usize,
    drained: bool,
    sample: Vec<u8>,
    tu_open: bool,
    tu_bytes: usize,
    tu_frames: u32,
    tu_sequence: bool,
    displayed: Option<frame::Picture>,
    first_random_access: bool,
    segment_sequence: bool,
    segment_samples: u64,
    segment_expected: u64,
    total_bytes: u64,
    total_obus: u64,
    total_frames: u64,
    next_ordinal: u64,
}

impl Output {
    /// Creates a job reader with no active segment and presentation ordinal zero.
    pub fn new(expected: Expected, limits: Limits) -> Self {
        Self {
            expected,
            limits,
            state: State::Ready,
            configuration: Vec::new(),
            sequence: None,
            frames: None,
            buffer: Vec::new(),
            consumed: 0,
            drained: true,
            sample: Vec::new(),
            tu_open: false,
            tu_bytes: 0,
            tu_frames: 0,
            tu_sequence: false,
            displayed: None,
            first_random_access: false,
            segment_sequence: false,
            segment_samples: 0,
            segment_expected: 0,
            total_bytes: 0,
            total_obus: 0,
            total_frames: 0,
            next_ordinal: 0,
        }
    }

    /// Begins a segment, bounding its session cap by the remaining job samples.
    /// This parser allowance does not change the native retained-input capacity.
    pub fn begin_segment(&mut self, capacity: u64) -> Result<(), Error> {
        let result = if self.state == State::Failed {
            Err(Error::Failed)
        } else if self.state != State::Ready {
            Err(Error::State)
        } else if capacity == 0 {
            Err(Error::PictureCount)
        } else if self.next_ordinal >= self.limits.max_samples {
            Err(Error::Limit)
        } else {
            self.state = State::Active;
            self.segment_expected = capacity.min(self.limits.max_samples - self.next_ordinal);
            self.segment_samples = 0;
            self.segment_sequence = false;
            self.frames = None;
            Ok(())
        };
        self.record(result)
    }

    /// Admits an arbitrary native byte chunk, including a fragment of an OBU.
    ///
    /// Drain samples between pushes to keep unconsumed input within its bound.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let result = self.append(bytes);
        self.record(result)
    }

    /// Returns the next complete delimited sample, or waits for more bytes.
    pub fn next_sample(&mut self) -> Result<Option<Sample>, Error> {
        let result = self.read();
        self.record(result)
    }

    /// Validates drain against the actual submission count and returns the last sample.
    ///
    /// This retains the job's exact configuration and ordinal counter. A fresh
    /// segment cannot refer to a previous segment's pictures.
    pub fn end_segment(&mut self, actual_pictures: u64) -> Result<Sample, Error> {
        let result = self.end(actual_pictures);
        self.record(result)
    }

    /// Returns the complete first validated sequence-header OBU for `av1C`.
    pub fn configuration(&self) -> Option<&[u8]> {
        if self.configuration.is_empty() {
            None
        } else {
            Some(&self.configuration)
        }
    }

    /// Returns the checked sequence configuration, without native metadata.
    pub fn sequence(&self) -> Option<Sequence> {
        self.sequence
    }

    fn active(&self) -> Result<(), Error> {
        match self.state {
            State::Active => Ok(()),
            State::Ready => Err(Error::State),
            State::Failed => Err(Error::Failed),
        }
    }

    fn record<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        if result.is_err() {
            self.state = State::Failed;
            self.buffer.clear();
            self.sample.clear();
            self.frames = None;
        }
        result
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.active()?;
        if !self.drained {
            return Err(Error::DrainRequired);
        }
        if bytes.len() > self.limits.max_packet_bytes {
            return Err(Error::Limit);
        }
        let total = self
            .total_bytes
            .checked_add(u64::try_from(bytes.len()).map_err(|_| Error::Limit)?)
            .filter(|n| *n <= self.limits.max_total_bytes)
            .ok_or(Error::Limit)?;
        let retained = self.buffer.len() - self.consumed;
        if retained
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.limits.max_buffered_bytes)
        {
            return Err(Error::Limit);
        }
        if self.consumed != 0 {
            self.buffer.drain(..self.consumed);
            self.consumed = 0;
        }
        reserve(
            &mut self.buffer,
            bytes.len(),
            self.limits.max_buffered_bytes,
        )?;
        self.buffer.extend_from_slice(bytes);
        self.total_bytes = total;
        self.drained = bytes.is_empty();
        Ok(())
    }

    fn read(&mut self) -> Result<Option<Sample>, Error> {
        self.active()?;
        loop {
            let rest = self.buffer.get(self.consumed..).ok_or(Error::State)?;
            let Some(size) = extent(rest, self.limits.max_obu_bytes)? else {
                self.drained = true;
                return Ok(None);
            };
            let bytes = rest.get(..size).ok_or(Error::State)?;
            if bytes.first().is_some_and(|b| b & 4 != 0) {
                return Err(Error::UnsupportedObu);
            }
            let mut obus = Obus::new(
                bytes,
                obu::Limits {
                    max_packet_bytes: size,
                    max_obus: 1,
                },
            )
            .map_err(Error::Framing)?;
            let obu = obus.next().ok_or(Error::State)?.map_err(Error::Framing)?;
            self.total_obus = self
                .total_obus
                .checked_add(1)
                .filter(|n| *n <= self.limits.max_obus)
                .ok_or(Error::Limit)?;
            if obu.kind() == ObuKind::TemporalDelimiter {
                self.consumed += size;
                if self.tu_open {
                    let sample = self.close_unit()?;
                    self.tu_open = true;
                    return Ok(Some(sample));
                }
                self.tu_open = true;
                continue;
            }
            if !self.tu_open {
                return Err(Error::Ordering);
            }
            self.tu_bytes = self
                .tu_bytes
                .checked_add(size)
                .filter(|n| *n <= self.limits.max_temporal_unit_bytes)
                .ok_or(Error::Limit)?;
            match obu.kind() {
                ObuKind::SequenceHeader => {
                    if self.tu_frames != 0 || self.frames.as_ref().is_some_and(Frames::incomplete) {
                        return Err(Error::Ordering);
                    }
                    if bytes.len() > self.limits.max_sequence_bytes {
                        return Err(Error::Limit);
                    }
                    if !self.configuration.is_empty() && bytes != self.configuration {
                        return Err(Error::ConfigurationChanged);
                    }
                    let seq = Sequence::parse(
                        obu,
                        sequence::Limits {
                            max_payload_bytes: self.limits.max_sequence_bytes,
                            max_width: self.expected.dimensions.width,
                            max_height: self.expected.dimensions.height,
                        },
                    )
                    .map_err(Error::Sequence)?;
                    let color = seq.color();
                    if color.primaries() != self.expected.primaries
                        || color.transfer() != self.expected.transfer
                        || color.matrix() != self.expected.matrix
                        || color.range() != self.expected.range
                        || color.chroma_position() != self.expected.chroma_position
                    {
                        return Err(Error::Metadata);
                    }
                    if !self.segment_sequence {
                        self.frames = Some(
                            Frames::new(seq, self.expected.dimensions, self.limits.max_tiles)
                                .map_err(Error::Frame)?,
                        );
                    }
                    if self.configuration.is_empty() {
                        self.configuration
                            .try_reserve_exact(size)
                            .map_err(|_| Error::Allocation)?;
                        self.configuration.extend_from_slice(bytes);
                        self.sequence = Some(seq);
                    }
                    self.segment_sequence = true;
                    self.tu_sequence = true;
                }
                ObuKind::Frame | ObuKind::FrameHeader | ObuKind::TileGroup => {
                    if self.displayed.is_some() {
                        return Err(Error::DisplayCount);
                    }
                    let frames = self.frames.as_mut().ok_or(Error::Ordering)?;
                    if let Some(picture) = frames.push(obu).map_err(Error::Frame)? {
                        self.total_frames = self
                            .total_frames
                            .checked_add(1)
                            .filter(|n| *n <= self.limits.max_frames)
                            .ok_or(Error::Limit)?;
                        self.tu_frames = self
                            .tu_frames
                            .checked_add(1)
                            .filter(|n| *n <= self.limits.max_frames_per_temporal_unit)
                            .ok_or(Error::Limit)?;
                        if self.tu_frames == 1 {
                            self.first_random_access = picture.random_access;
                        }
                        if picture.displayed {
                            self.displayed = Some(picture);
                        }
                    }
                }
                ObuKind::Padding => {
                    self.consumed += size;
                    continue;
                }
                _ => return Err(Error::UnsupportedObu),
            }
            reserve(&mut self.sample, size, self.limits.max_temporal_unit_bytes)?;
            self.sample.extend_from_slice(bytes);
            self.consumed += size;
        }
    }

    fn close_unit(&mut self) -> Result<Sample, Error> {
        if self.frames.as_ref().is_some_and(Frames::incomplete) {
            return Err(Error::Truncated);
        }
        let picture = self.displayed.ok_or(Error::DisplayCount)?;
        let sync = self.first_random_access && self.tu_sequence;
        if self.segment_samples == 0 && !sync {
            return Err(Error::RandomAccess);
        }
        let segment_samples = self
            .segment_samples
            .checked_add(1)
            .filter(|n| *n <= self.segment_expected)
            .ok_or(Error::PictureCount)?;
        let next = self
            .next_ordinal
            .checked_add(1)
            .filter(|n| *n <= self.limits.max_samples)
            .ok_or(Error::Limit)?;
        let sample = Sample {
            bytes: std::mem::take(&mut self.sample),
            ordinal: self.next_ordinal,
            sync,
            picture_identity: picture.identity,
            show_existing: picture.show_existing,
        };
        self.segment_samples = segment_samples;
        self.next_ordinal = next;
        self.displayed = None;
        self.first_random_access = false;
        self.tu_sequence = false;
        self.tu_frames = 0;
        self.tu_bytes = 0;
        self.tu_open = false;
        Ok(sample)
    }

    fn end(&mut self, actual_pictures: u64) -> Result<Sample, Error> {
        self.active()?;
        let rest = self.buffer.get(self.consumed..).ok_or(Error::State)?;
        if !rest.is_empty() {
            return Err(if extent(rest, self.limits.max_obu_bytes)?.is_some() {
                Error::DrainRequired
            } else {
                Error::Truncated
            });
        }
        self.frames
            .as_mut()
            .ok_or(Error::Ordering)?
            .finish()
            .map_err(Error::Frame)?;
        let sample = self.close_unit()?;
        if actual_pictures == 0
            || actual_pictures > self.segment_expected
            || self.segment_samples != actual_pictures
        {
            return Err(Error::PictureCount);
        }
        self.buffer.clear();
        self.consumed = 0;
        self.drained = true;
        self.frames = None;
        self.state = State::Ready;
        Ok(sample)
    }
}

fn reserve(bytes: &mut Vec<u8>, additional: usize, limit: usize) -> Result<(), Error> {
    let needed = bytes
        .len()
        .checked_add(additional)
        .filter(|n| *n <= limit)
        .ok_or(Error::Limit)?;
    if needed > bytes.capacity() {
        let target = bytes.capacity().saturating_mul(2).min(limit).max(needed);
        bytes
            .try_reserve_exact(target - bytes.len())
            .map_err(|_| Error::Allocation)?;
    }
    Ok(())
}

fn extent(bytes: &[u8], limit: usize) -> Result<Option<usize>, Error> {
    let Some(&header) = bytes.first() else {
        return Ok(None);
    };
    if header & 2 == 0 {
        return Err(Error::Framing(obu::Error::MissingSizeField));
    }
    let start = if header & 4 != 0 { 2 } else { 1 };
    let mut value = 0_u64;
    for i in 0..8 {
        let Some(&byte) = bytes.get(start + i) else {
            return Ok(None);
        };
        value |= u64::from(byte & 127) << (i * 7);
        if byte & 128 == 0 {
            let value =
                u32::try_from(value).map_err(|_| Error::Framing(obu::Error::SizeOverflow))?;
            let size = usize::try_from(value)
                .ok()
                .and_then(|n| n.checked_add(start + i + 1))
                .ok_or(Error::Limit)?;
            if size > limit {
                return Err(Error::Limit);
            }
            return Ok((bytes.len() >= size).then_some(size));
        }
    }
    Err(Error::Framing(obu::Error::SizeTooLong))
}
