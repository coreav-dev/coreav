//! Bounded raw-picture requests and untrusted segmented worker records.

use std::io::{self, Read, Write};

use crate::{
    Error,
    model::{self, Color, Retention, Settings},
};

/// Versioned raw-picture input signature.
pub const INPUT_MAGIC: [u8; 8] = *b"AV1IN\0\x01\0";
/// Versioned worker-output signature.
pub const OUTPUT_MAGIC: [u8; 8] = *b"AV1OUT\x01\0";
/// Maximum whole-job output, including wire metadata.
pub const MAX_OUTPUT_BYTES: u64 = 64 << 30;

const FAILURE_RECORD_BYTES: u64 = 10;

/// Checked whole-job transport limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    retained_bytes: u64,
    output_bytes: u64,
    frames: u64,
    packet_bytes: u32,
}

impl Limits {
    /// Checks nonzero limits, the failure-only wire minimum, and hard byte ceilings.
    pub fn new(
        retained_bytes: u64,
        output_bytes: u64,
        frames: u64,
        packet_bytes: u32,
    ) -> Result<Self, Error> {
        if retained_bytes == 0
            || retained_bytes > model::RETAINED_BYTES
            || !(8 + FAILURE_RECORD_BYTES..=MAX_OUTPUT_BYTES).contains(&output_bytes)
            || frames == 0
            || packet_bytes == 0
            || u64::from(packet_bytes) > model::MAX_PACKET_BYTES as u64
        {
            return Err(Error::Limit("invalid AV1 transport limits"));
        }
        Ok(Self {
            retained_bytes,
            output_bytes,
            frames,
            packet_bytes,
        })
    }

    /// Retained-input budget B.
    pub fn retained_bytes(self) -> u64 {
        self.retained_bytes
    }
    /// Maximum complete output-wire bytes.
    pub fn output_bytes(self) -> u64 {
        self.output_bytes
    }
    /// Maximum submitted pictures across all segments.
    pub fn frames(self) -> u64 {
        self.frames
    }
    /// Maximum data bytes in one native packet.
    pub fn packet_bytes(self) -> u32 {
        self.packet_bytes
    }
}

/// Immutable settings and limits sent before any picture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    /// Checked encoder settings.
    pub settings: Settings,
    /// Checked transport limits.
    pub limits: Limits,
}

impl Request {
    /// Writes the fixed little-endian input header.
    pub fn write<W: Write>(&self, writer: &mut W) -> Result<(), Error> {
        writer.write_all(&INPUT_MAGIC)?;
        for value in [
            self.settings.width(),
            self.settings.height(),
            self.settings.rate().0,
            self.settings.rate().1,
        ] {
            writer.write_all(&value.to_le_bytes())?;
        }
        let color = self.settings.color();
        writer.write_all(&[
            self.settings.preset(),
            self.settings.crf(),
            color.primaries(),
            color.transfer(),
            color.matrix(),
            u8::from(color.full_range()),
            color.chroma_position(),
            0,
        ])?;
        for value in [
            self.limits.retained_bytes,
            self.limits.output_bytes,
            self.limits.frames,
        ] {
            writer.write_all(&value.to_le_bytes())?;
        }
        writer.write_all(&self.limits.packet_bytes.to_le_bytes())?;
        Ok(())
    }

    /// Reads and validates the fixed header without allocating picture storage.
    pub fn read<R: Read>(reader: &mut R) -> Result<Self, Error> {
        let mut bytes = [0; 60];
        exact(reader, &mut bytes)?;
        let mut bytes = bytes.as_slice();
        if take::<8>(&mut bytes)? != INPUT_MAGIC {
            return Err(Error::Output("invalid AV1 input signature"));
        }
        let width = u32::from_le_bytes(take(&mut bytes)?);
        let height = u32::from_le_bytes(take(&mut bytes)?);
        let rate = (
            u32::from_le_bytes(take(&mut bytes)?),
            u32::from_le_bytes(take(&mut bytes)?),
        );
        let [
            preset,
            crf,
            primaries,
            transfer,
            matrix,
            full_range,
            chroma_position,
            reserved,
        ] = take(&mut bytes)?;
        if full_range > 1 || reserved != 0 {
            return Err(Error::Output("invalid AV1 input flags"));
        }
        let color = Color::new(
            primaries,
            transfer,
            matrix,
            full_range == 1,
            chroma_position,
        )?;
        let settings = Settings::new(width, height, rate, preset, crf, color)?;
        let limits = Limits::new(
            u64::from_le_bytes(take(&mut bytes)?),
            u64::from_le_bytes(take(&mut bytes)?),
            u64::from_le_bytes(take(&mut bytes)?),
            u32::from_le_bytes(take(&mut bytes)?),
        )?;
        Ok(Self { settings, limits })
    }
}

/// Global picture identity and the unchanged source presentation timestamp.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameHeader {
    /// Sequential, zero-based global picture identity.
    pub id: u64,
    /// Source timestamp, never reset at a segment boundary.
    pub pts: i64,
}

/// Builds the fixed prefix for a bounded writer queue without copying pixels.
pub fn frame_prefix(id: u64, pts: i64) -> [u8; 17] {
    let mut bytes = [0; 17];
    for (slot, value) in bytes.iter_mut().zip(
        std::iter::once(1)
            .chain(id.to_le_bytes())
            .chain(pts.to_le_bytes()),
    ) {
        *slot = value;
    }
    bytes
}

/// Writes a picture prefix; the caller writes exactly `settings.raw_bytes()` next.
pub fn write_frame_header<W: Write>(writer: &mut W, id: u64, pts: i64) -> Result<(), Error> {
    writer.write_all(&frame_prefix(id, pts))?;
    Ok(())
}

/// Writes the explicit whole-input end marker, distinct from pipe EOF.
pub fn write_input_end<W: Write>(writer: &mut W, count: u64) -> Result<(), Error> {
    writer.write_all(&[2])?;
    writer.write_all(&count.to_le_bytes())?;
    Ok(())
}

/// Reads prefixes separately so the worker can destroy a full session before allocating.
pub struct InputReader<R> {
    reader: R,
    request: Request,
    count: u64,
    last_pts: Option<i64>,
    state: InputState,
}

enum InputState {
    Header,
    Picture,
    End,
    Failed,
}

impl<R: Read> InputReader<R> {
    /// Reads the checked request from the beginning of the pipe.
    pub fn new(mut reader: R) -> Result<Self, Error> {
        let request = Request::read(&mut reader)?;
        Ok(Self {
            reader,
            request,
            count: 0,
            last_pts: None,
            state: InputState::Header,
        })
    }

    /// The immutable job request.
    pub fn request(&self) -> Request {
        self.request
    }

    /// Reads the next prefix or validates END and physical EOF, without pixel allocation.
    pub fn next_frame_header(&mut self) -> Result<Option<FrameHeader>, Error> {
        if matches!(self.state, InputState::End) {
            return Ok(None);
        }
        if !matches!(self.state, InputState::Header) {
            self.state = InputState::Failed;
            return Err(Error::Output("AV1 picture read is out of order"));
        }
        self.state = InputState::Failed;
        let mut tag = [0];
        exact(&mut self.reader, &mut tag)?;
        match tag {
            [1] => {
                let mut bytes = [0; 16];
                exact(&mut self.reader, &mut bytes)?;
                let mut bytes = bytes.as_slice();
                let id = u64::from_le_bytes(take(&mut bytes)?);
                let pts = i64::from_le_bytes(take(&mut bytes)?);
                if id != self.count
                    || self.count >= self.request.limits.frames
                    || self.last_pts.is_some_and(|last| pts <= last)
                {
                    return Err(Error::Output("invalid AV1 picture identity or timestamp"));
                }
                self.count += 1;
                self.last_pts = Some(pts);
                self.state = InputState::Picture;
                Ok(Some(FrameHeader { id, pts }))
            }
            [2] => {
                let mut count = [0; 8];
                exact(&mut self.reader, &mut count)?;
                if self.count == 0 || u64::from_le_bytes(count) != self.count {
                    return Err(Error::Output("invalid AV1 input completion count"));
                }
                eof(&mut self.reader)?;
                self.state = InputState::End;
                Ok(None)
            }
            _ => Err(Error::Output("unknown AV1 input record")),
        }
    }

    /// Allocates and reads precisely one tightly packed owned YUV420 picture.
    pub fn read_picture(&mut self) -> Result<Vec<u8>, Error> {
        if !matches!(self.state, InputState::Picture) {
            self.state = InputState::Failed;
            return Err(Error::Output("AV1 picture read is out of order"));
        }
        self.state = InputState::Failed;
        let mut data = Vec::new();
        data.try_reserve_exact(self.request.settings.raw_bytes())
            .map_err(|_| Error::Limit("AV1 picture allocation failed"))?;
        if data.capacity() != self.request.settings.raw_bytes() {
            return Err(Error::Limit("AV1 picture allocation capacity changed"));
        }
        data.resize(self.request.settings.raw_bytes(), 0);
        exact(&mut self.reader, &mut data)?;
        self.state = InputState::Header;
        Ok(data)
    }
}

/// A transport-checked record; compressed AV1 syntax still requires independent validation.
#[derive(Debug, Eq, PartialEq)]
pub enum Record {
    /// The pinned native version string.
    Version(String),
    /// A segment starting at this global picture identity.
    Begin {
        /// First global picture identity.
        base: u64,
    },
    /// Untrusted native bytes and assertions, not validated display timestamps or frame types.
    Packet {
        /// Owned native packet bytes.
        data: Vec<u8>,
        /// Native presentation assertion.
        pts: i64,
        /// Native decode assertion.
        dts: i64,
        /// Declared low-four-bit native flags.
        flags: u32,
        /// Native picture-type assertion.
        picture_type: u32,
    },
    /// A fully drained segment's checked accounting.
    End {
        /// First global picture identity.
        base: u64,
        /// Submitted pictures in this segment.
        submitted: u32,
        /// Peak retained-input footprint, including reserved owner capacity.
        peak_retained_bytes: u64,
    },
    /// The single final job completion.
    Complete {
        /// Whole-job picture count.
        frames: u64,
        /// Whole-job segment count.
        segments: u64,
    },
}

enum Phase {
    Version,
    Ready,
    Active { base: u64, packets: u64, eos: bool },
    Between { short: bool },
    Complete,
    Failed,
}

/// Incremental linear-time output framer with one bounded pending payload.
pub struct Decoder {
    request: Request,
    retention: Retention,
    phase: Phase,
    frames: u64,
    segments: u64,
    charged: u64,
    magic: usize,
    header: [u8; 5],
    header_bytes: usize,
    tag: u8,
    length: usize,
    payload: Vec<u8>,
}

impl Decoder {
    /// Binds every segment to the caller's exact checked retained-input envelope.
    pub fn new(request: Request, expected_retention: Retention) -> Result<Self, Error> {
        if expected_retention.budget() != request.limits.retained_bytes
            || expected_retention.pictures() > request.settings.keyint()
            || expected_retention.picture_bytes() < request.settings.raw_bytes() as u64
        {
            return Err(Error::Invalid("AV1 retention does not match the request"));
        }
        Ok(Self {
            request,
            retention: expected_retention,
            phase: Phase::Version,
            frames: 0,
            segments: 0,
            charged: 0,
            magic: 0,
            header: [0; 5],
            header_bytes: 0,
            tag: 0,
            length: 0,
            payload: Vec::new(),
        })
    }

    /// Consumes any chunk size, invoking the sink only for complete checked records.
    /// Any parse or sink error permanently fails the decoder.
    pub fn push<F>(&mut self, bytes: &[u8], mut callback: F) -> Result<(), Error>
    where
        F: FnMut(Record) -> Result<(), Error>,
    {
        if matches!(self.phase, Phase::Failed) {
            return Err(Error::Output("AV1 output decoder has failed"));
        }
        let result = self.push_inner(bytes, &mut callback);
        if result.is_err() {
            self.phase = Phase::Failed;
            self.payload = Vec::new();
        }
        result
    }

    fn push_inner<F>(&mut self, mut bytes: &[u8], callback: &mut F) -> Result<(), Error>
    where
        F: FnMut(Record) -> Result<(), Error>,
    {
        while !bytes.is_empty() {
            if matches!(self.phase, Phase::Complete) {
                return Err(Error::Output("bytes follow AV1 job completion"));
            }
            if self.magic < OUTPUT_MAGIC.len() {
                let count = bytes.len().min(OUTPUT_MAGIC.len() - self.magic);
                let (part, rest) = bytes.split_at(count);
                let expected = OUTPUT_MAGIC
                    .get(self.magic..self.magic + count)
                    .ok_or(Error::Output("invalid AV1 signature extent"))?;
                if part != expected {
                    return Err(Error::Output("invalid AV1 output signature"));
                }
                self.charge(count as u64)?;
                self.magic += count;
                bytes = rest;
                continue;
            }
            if self.header_bytes < self.header.len() {
                let count = bytes.len().min(self.header.len() - self.header_bytes);
                let (part, rest) = bytes.split_at(count);
                self.header
                    .get_mut(self.header_bytes..self.header_bytes + count)
                    .ok_or(Error::Output("invalid AV1 header extent"))?
                    .copy_from_slice(part);
                self.header_bytes += count;
                bytes = rest;
                if self.header_bytes < self.header.len() {
                    continue;
                }
                let [tag, a, b, c, d] = self.header;
                let length = u32::from_le_bytes([a, b, c, d]);
                self.validate_header(tag, length)?;
                self.charge(5 + u64::from(length))?;
                self.tag = tag;
                self.length = usize::try_from(length)
                    .map_err(|_| Error::Limit("AV1 record does not fit memory"))?;
                self.payload
                    .try_reserve_exact(self.length)
                    .map_err(|_| Error::Limit("AV1 record allocation failed"))?;
            }
            let count = bytes.len().min(self.length - self.payload.len());
            let (part, rest) = bytes.split_at(count);
            self.payload.extend_from_slice(part);
            bytes = rest;
            if self.payload.len() == self.length {
                let record = self.record()?;
                self.header_bytes = 0;
                callback(record)?;
            }
        }
        Ok(())
    }

    fn charge(&mut self, bytes: u64) -> Result<(), Error> {
        self.charged = self
            .charged
            .checked_add(bytes)
            .filter(|total| *total <= self.request.limits.output_bytes)
            .ok_or(Error::Limit("AV1 output byte limit exceeded"))?;
        Ok(())
    }

    fn validate_header(&self, tag: u8, length: u32) -> Result<(), Error> {
        let valid = match (tag, &self.phase) {
            (1, Phase::Version) => matches!(length, 5 | 6),
            (2, Phase::Ready | Phase::Between { short: false }) => {
                length == 40 && self.frames < self.request.limits.frames
            }
            (
                3,
                Phase::Active {
                    packets,
                    eos: false,
                    ..
                },
            ) => {
                length >= 24
                    && u64::from(length) <= u64::from(self.request.limits.packet_bytes) + 24
                    && *packets < 4 * u64::from(self.retention.pictures()) + 32
            }
            (4, Phase::Active { eos: true, .. }) => length == 28,
            (5, Phase::Between { .. }) => length == 16,
            (6, Phase::Version | Phase::Ready | Phase::Active { .. } | Phase::Between { .. }) => {
                length == 5
            }
            _ => false,
        };
        if !valid {
            return Err(Error::Output("invalid AV1 record length or ordering"));
        }
        Ok(())
    }

    fn record(&mut self) -> Result<Record, Error> {
        let mut payload = std::mem::take(&mut self.payload);
        let mut bytes = payload.as_slice();
        match self.tag {
            1 => {
                if bytes != b"4.2.0" && bytes != b"v4.2.0" {
                    return Err(Error::Output("unexpected SVT-AV1 version"));
                }
                let version = String::from_utf8(payload)
                    .map_err(|_| Error::Output("invalid SVT-AV1 version"))?;
                self.phase = Phase::Ready;
                Ok(Record::Version(version))
            }
            2 => {
                let base = u64::from_le_bytes(take(&mut bytes)?);
                let budget = u64::from_le_bytes(take(&mut bytes)?);
                let fixed = u64::from_le_bytes(take(&mut bytes)?);
                let picture = u64::from_le_bytes(take(&mut bytes)?);
                let pictures = u32::from_le_bytes(take(&mut bytes)?);
                let keyint = u32::from_le_bytes(take(&mut bytes)?);
                if base != self.frames
                    || budget != self.retention.budget()
                    || fixed != self.retention.fixed_bytes()
                    || picture != self.retention.picture_bytes()
                    || pictures != self.retention.pictures()
                    || keyint != self.request.settings.keyint()
                {
                    return Err(Error::Output("AV1 segment envelope changed"));
                }
                self.phase = Phase::Active {
                    base,
                    packets: 0,
                    eos: false,
                };
                Ok(Record::Begin { base })
            }
            3 => {
                let pts = i64::from_le_bytes(take(&mut bytes)?);
                let dts = i64::from_le_bytes(take(&mut bytes)?);
                let flags = u32::from_le_bytes(take(&mut bytes)?);
                let picture_type = u32::from_le_bytes(take(&mut bytes)?);
                if flags & !15 != 0 {
                    return Err(Error::Native(flags));
                }
                if bytes.is_empty() && flags != 1 {
                    return Err(Error::Output("empty AV1 packet without standalone EOS"));
                }
                let Phase::Active { packets, eos, .. } = &mut self.phase else {
                    return Err(Error::Output("AV1 packet outside segment"));
                };
                *packets += 1;
                *eos = flags & 1 != 0;
                payload.copy_within(24.., 0);
                payload.truncate(payload.len() - 24);
                Ok(Record::Packet {
                    data: payload,
                    pts,
                    dts,
                    flags,
                    picture_type,
                })
            }
            4 => {
                let base = u64::from_le_bytes(take(&mut bytes)?);
                let submitted = u32::from_le_bytes(take(&mut bytes)?);
                let packet_count = u64::from_le_bytes(take(&mut bytes)?);
                let peak_retained_bytes = u64::from_le_bytes(take(&mut bytes)?);
                let Phase::Active {
                    base: expected,
                    packets,
                    eos: true,
                } = self.phase
                else {
                    return Err(Error::Output("AV1 segment ended before EOS"));
                };
                if base != expected
                    || submitted == 0
                    || submitted > self.retention.pictures()
                    || packet_count != packets
                    || peak_retained_bytes != self.retention.peak_bytes(submitted)?
                {
                    return Err(Error::Output("invalid AV1 segment completion accounting"));
                }
                self.frames = self
                    .frames
                    .checked_add(u64::from(submitted))
                    .filter(|frames| *frames <= self.request.limits.frames)
                    .ok_or(Error::Limit("AV1 frame limit exceeded"))?;
                self.segments += 1;
                self.phase = Phase::Between {
                    short: submitted < self.retention.pictures(),
                };
                Ok(Record::End {
                    base,
                    submitted,
                    peak_retained_bytes,
                })
            }
            5 => {
                let frames = u64::from_le_bytes(take(&mut bytes)?);
                let segments = u64::from_le_bytes(take(&mut bytes)?);
                if frames == 0 || frames != self.frames || segments != self.segments {
                    return Err(Error::Output("invalid AV1 job completion accounting"));
                }
                self.phase = Phase::Complete;
                Ok(Record::Complete { frames, segments })
            }
            6 => {
                let [category] = take(&mut bytes)?;
                let code = u32::from_le_bytes(take(&mut bytes)?);
                if category != 4 && code != 0 {
                    return Err(Error::Output("non-native AV1 failure has a native code"));
                }
                Err(match category {
                    1 => Error::Invalid("AV1 worker rejected invalid input"),
                    2 => Error::Unsupported("AV1 worker rejected unsupported input"),
                    3 => Error::Limit("AV1 worker resource limit exceeded"),
                    4 => Error::Native(code),
                    5 => Error::Output("AV1 worker rejected native output"),
                    6 => Error::Io(io::Error::other("AV1 worker I/O failure")),
                    _ => Error::Output("unknown AV1 worker failure category"),
                })
            }
            _ => Err(Error::Output("unknown AV1 output record")),
        }
    }

    /// Requires a complete signature, every payload, and exactly one final completion.
    pub fn finish(&mut self) -> Result<(), Error> {
        if matches!(self.phase, Phase::Complete) && self.header_bytes == 0 {
            return Ok(());
        }
        self.phase = Phase::Failed;
        self.payload = Vec::new();
        Err(Error::Output("truncated AV1 worker output"))
    }
}

/// Small bounded writer; native lifecycle and record counts remain the worker's responsibility.
pub struct OutputWriter<W> {
    writer: W,
    limits: Limits,
    written: u64,
    state: WriterState,
}

enum WriterState {
    Open,
    FailureOnly,
    Closed,
}

impl<W: Write> OutputWriter<W> {
    /// Writes the signature and pinned version under the whole-job wire limit.
    pub fn new(writer: W, version: &str, limits: Limits) -> Result<Self, Error> {
        if version != "4.2.0" && version != "v4.2.0" {
            return Err(Error::Output("unexpected SVT-AV1 version"));
        }
        if limits.output_bytes < 8 + 5 + version.len() as u64 + FAILURE_RECORD_BYTES {
            return Err(Error::Limit(
                "AV1 version and failure reserve exceed the output byte limit",
            ));
        }
        let mut this = Self {
            writer,
            limits,
            written: 0,
            state: WriterState::Open,
        };
        this.charge(8, FAILURE_RECORD_BYTES)?;
        this.writer.write_all(&OUTPUT_MAGIC)?;
        this.record(1, version.as_bytes(), &[])?;
        Ok(this)
    }

    /// Writes the immutable segment envelope and configured keyframe interval K.
    pub fn begin(&mut self, base: u64, retention: Retention, keyint: u32) -> Result<(), Error> {
        let mut bytes = [0; 40];
        let mut writer = bytes.as_mut_slice();
        for value in [
            base,
            retention.budget(),
            retention.fixed_bytes(),
            retention.picture_bytes(),
        ] {
            writer.write_all(&value.to_le_bytes())?;
        }
        writer.write_all(&retention.pictures().to_le_bytes())?;
        writer.write_all(&keyint.to_le_bytes())?;
        self.record(2, &bytes, &[])
    }

    /// Writes raw native assertions and bytes without interpreting AV1 syntax.
    pub fn packet(
        &mut self,
        pts: i64,
        dts: i64,
        flags: u32,
        picture_type: u32,
        data: &[u8],
    ) -> Result<(), Error> {
        if !matches!(self.state, WriterState::Open) {
            return Err(Error::Output("AV1 output writer is not accepting records"));
        }
        if data.len() as u64 > u64::from(self.limits.packet_bytes) {
            self.state = WriterState::FailureOnly;
            return Err(Error::Limit("AV1 packet byte limit exceeded"));
        }
        let mut bytes = [0; 24];
        let mut writer = bytes.as_mut_slice();
        writer.write_all(&pts.to_le_bytes())?;
        writer.write_all(&dts.to_le_bytes())?;
        writer.write_all(&flags.to_le_bytes())?;
        writer.write_all(&picture_type.to_le_bytes())?;
        self.record(3, &bytes, data)
    }

    /// Maximum next packet data size before copying native output into Rust storage.
    /// Keeps ten bytes available for terminal failure; zero still permits standalone EOS.
    pub fn packet_capacity(&mut self) -> Result<usize, Error> {
        if !matches!(self.state, WriterState::Open) {
            return Err(Error::Output("AV1 output writer is not accepting records"));
        }
        let remaining = self.limits.output_bytes - self.written;
        let Some(data) = remaining.checked_sub(29 + FAILURE_RECORD_BYTES) else {
            self.state = WriterState::FailureOnly;
            return Err(Error::Limit(
                "AV1 packet overhead and failure reserve exceed the output byte limit",
            ));
        };
        usize::try_from(data.min(u64::from(self.limits.packet_bytes)))
            .map_err(|_| Error::Limit("AV1 packet capacity does not fit memory"))
    }

    /// Writes segment accounting after complete native drain and destruction.
    pub fn end(
        &mut self,
        base: u64,
        submitted: u32,
        packet_count: u64,
        peak: u64,
    ) -> Result<(), Error> {
        let mut bytes = [0; 28];
        let mut writer = bytes.as_mut_slice();
        writer.write_all(&base.to_le_bytes())?;
        writer.write_all(&submitted.to_le_bytes())?;
        writer.write_all(&packet_count.to_le_bytes())?;
        writer.write_all(&peak.to_le_bytes())?;
        self.record(4, &bytes, &[])
    }

    /// Writes the one final completion, preventing further records.
    pub fn complete(&mut self, frames: u64, segments: u64) -> Result<(), Error> {
        let mut bytes = [0; 16];
        let mut writer = bytes.as_mut_slice();
        writer.write_all(&frames.to_le_bytes())?;
        writer.write_all(&segments.to_le_bytes())?;
        self.record(5, &bytes, &[])?;
        self.state = WriterState::Closed;
        Ok(())
    }

    /// Writes a terminal typed failure without exposing arbitrary diagnostic text.
    /// Categories 1 through 6 mean invalid, unsupported, limit, native, output, and I/O.
    /// Only native failures carry a nonzero code; I/O preserves its category, not OS details.
    pub fn failure(&mut self, error: &Error) -> Result<(), Error> {
        let (category, code) = match error {
            Error::Invalid(_) => (1, 0u32),
            Error::Unsupported(_) => (2, 0),
            Error::Limit(_) => (3, 0),
            Error::Native(code) => (4, *code),
            Error::Output(_) => (5, 0),
            Error::Io(_) => (6, 0),
        };
        let [a, b, c, d] = code.to_le_bytes();
        let result = self.record(6, &[category, a, b, c, d], &[]);
        self.state = WriterState::Closed;
        result
    }

    fn charge(&mut self, count: u64, reserve: u64) -> Result<(), Error> {
        let Some(written) = self.written.checked_add(count).filter(|written| {
            written
                .checked_add(reserve)
                .is_some_and(|total| total <= self.limits.output_bytes)
        }) else {
            self.state = WriterState::FailureOnly;
            return Err(Error::Limit("AV1 output byte limit exceeded"));
        };
        self.written = written;
        Ok(())
    }

    fn record(&mut self, tag: u8, fixed: &[u8], data: &[u8]) -> Result<(), Error> {
        if matches!(self.state, WriterState::Closed)
            || (matches!(self.state, WriterState::FailureOnly) && tag != 6)
        {
            return Err(Error::Output("AV1 output writer is not accepting records"));
        }
        let length = u32::try_from(fixed.len().saturating_add(data.len()))
            .map_err(|_| Error::Limit("AV1 record size overflows"))?;
        let terminal = matches!(tag, 5 | 6);
        self.charge(
            5 + u64::from(length),
            if terminal { 0 } else { FAILURE_RECORD_BYTES },
        )?;
        self.state = WriterState::Closed;
        self.writer.write_all(&[tag])?;
        self.writer.write_all(&length.to_le_bytes())?;
        self.writer.write_all(fixed)?;
        self.writer.write_all(data)?;
        self.state = if terminal {
            WriterState::Closed
        } else {
            WriterState::Open
        };
        Ok(())
    }
}

/// Writes a bounded failure-only stream when initialization fails before version discovery.
pub fn write_failure<W: Write>(writer: &mut W, error: &Error, limits: Limits) -> Result<(), Error> {
    let mut output = OutputWriter {
        writer,
        limits,
        written: 0,
        state: WriterState::Open,
    };
    output.charge(8, FAILURE_RECORD_BYTES)?;
    output.writer.write_all(&OUTPUT_MAGIC)?;
    output.failure(error)
}

fn exact<R: Read>(reader: &mut R, bytes: &mut [u8]) -> Result<(), Error> {
    reader.read_exact(bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            Error::Output("truncated AV1 input")
        } else {
            Error::Io(error)
        }
    })
}

fn eof<R: Read>(reader: &mut R) -> Result<(), Error> {
    let mut byte = [0];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => return Ok(()),
            Ok(_) => return Err(Error::Output("bytes follow AV1 input completion")),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(Error::Output("truncated AV1 input"));
            }
            Err(error) => return Err(Error::Io(error)),
        }
    }
}

fn take<const N: usize>(bytes: &mut &[u8]) -> Result<[u8; N], Error> {
    let (value, rest) = bytes
        .split_first_chunk::<N>()
        .ok_or(Error::Output("truncated AV1 record"))?;
    *bytes = rest;
    Ok(*value)
}
