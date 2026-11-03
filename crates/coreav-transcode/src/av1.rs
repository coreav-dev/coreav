//! Validated AV1 worker output and bounded sample-spool replay.

mod timeline;

pub use timeline::Timeline;

use std::{
    fs::File,
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    num::NonZeroU64,
    path::Path,
    time::Instant,
};

use coreav_backend_svt_av1::{
    Error as BackendError, frame,
    model::Retention,
    output::{self, Output, Sample},
    protocol::{Decoder, Record, Request},
    sequence::{ChromaPosition, ColorRange},
};
use coreav_decode::{Error, ErrorKind};
use coreav_frame::{Picture, PixelFormat, Pixels};
use coreav_isobmff_write::mux::{Nclx, av01_sample_entry};
use coreav_worker::{ExecLauncher, HugePages};

use crate::{
    contract::{PacketLimits, PacketMeta, Packets, Submissions, VideoLayout},
    encoder::{self, Chunk, EncoderLimits, Process, Sink},
    timing::SampleTime,
};

/// Private worker entry for the existing sandbox launcher.
pub const WORKER_ARG: &str = "__coreav_svt_av1";

/// Launch settings shared by the input queue, output validation and final replay.
pub struct Launch<'a> {
    /// Existing sandbox launcher; `None` runs directly for process fixtures.
    pub launcher: Option<&'a ExecLauncher>,
    /// Requested per-process resource limits.
    pub limits: EncoderLimits,
    /// One deadline for input, output, process teardown and spool replay.
    pub deadline: Instant,
    /// Source sample aspect, kept without a geometry conversion.
    pub pixel_aspect: Option<(u32, u32)>,
}

/// Bounded raw-frame driver using the existing process keeper and writer queue.
pub struct Encoder {
    state: State,
    submitted: Submissions,
    frames: u64,
    source: Option<Timeline>,
}

enum State {
    Running(Process<OutputSink>),
    Failed(Error),
}

impl Encoder {
    /// Starts the private worker with its checked request, never a source path.
    /// The caller retains the spool's lifetime owner and supplies this driver
    /// its exclusive writer handle. Release callers always supply a launcher.
    pub fn spawn(
        program: &Path,
        request: Request,
        retention: Retention,
        spool: File,
        launch: Launch<'_>,
    ) -> Result<Self, Error> {
        if !program.is_absolute() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "the private AV1 worker must be an absolute path",
            ));
        }
        let layout = VideoLayout::new(
            request.settings.width(),
            request.settings.height(),
            PixelFormat::YUV420P,
            request.settings.raw_bytes() as u64,
        )?;
        let sink = OutputSink::new(
            spool,
            request,
            retention,
            launch.pixel_aspect,
            launch.deadline,
        )?;
        let mut header = Vec::new();
        header
            .try_reserve_exact(60)
            .map_err(|_| Error::new(ErrorKind::Limit, "cannot store AV1 request"))?;
        request.write(&mut header).map_err(backend_error)?;
        let command = encoder::command(
            program,
            &[WORKER_ARG.into()],
            HugePages::Kept,
            launch.launcher,
            &launch.limits,
            launch.deadline,
        );
        let mut process = Process::spawn(
            "svt-av1",
            "AV1 records",
            command,
            program,
            launch.deadline,
            sink,
        )?;
        process.enqueue(header)?;
        Ok(Self {
            state: State::Running(process),
            submitted: Submissions::new(layout, request.limits.frames()),
            frames: 0,
            source: None,
        })
    }

    /// Binds input and final duration to an immutable presented source timeline.
    /// The exact timeline-derived cadence must agree with the request's rate.
    pub fn spawn_timed(
        program: &Path,
        request: Request,
        retention: Retention,
        spool: File,
        launch: Launch<'_>,
        source: Timeline,
    ) -> Result<Self, Error> {
        let (num, den) = request.settings.rate();
        if source.deadline() != launch.deadline {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "AV1 timeline and process must share one deadline",
            ));
        }
        if Instant::now() >= launch.deadline {
            return Err(Error::new(
                ErrorKind::Timeout,
                "AV1 source deadline expired before launch",
            ));
        }
        let rate = source.rate();
        if u64::from(num) * u64::from(rate.den) != u64::from(rate.num) * u64::from(den) {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "AV1 request cadence differs from the source timeline",
            ));
        }
        if source.frames() > request.limits.frames() {
            return Err(Error::new(
                ErrorKind::Limit,
                "AV1 source timeline exceeds the frame limit",
            ));
        }
        let mut encoder = Self::spawn(program, request, retention, spool, launch)?;
        encoder.source = Some(source);
        Ok(encoder)
    }

    /// Checks and queues one owned planar 8-bit 4:2:0 picture at its source time.
    /// Every failure is terminal and stops the process; no retry or fallback occurs.
    pub fn send_frame(&mut self, picture: Picture, pts: i64) -> Result<(), Error> {
        let process = match &mut self.state {
            State::Running(process) => process,
            State::Failed(error) => return Err(error.clone()),
        };
        let result = (|| {
            if picture.format() != PixelFormat::YUV420P {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "SVT-AV1 requires planar 8-bit 4:2:0",
                ));
            }
            if let Some(source) = &mut self.source {
                source.admit(pts)?;
            }
            let frame = self.submitted.submit(picture, pts)?;
            let mut header = Vec::new();
            header
                .try_reserve_exact(17)
                .map_err(|_| Error::new(ErrorKind::Limit, "cannot store AV1 frame header"))?;
            header.extend_from_slice(&coreav_backend_svt_av1::protocol::frame_prefix(
                frame.id().index(),
                frame.pts(),
            ));
            process.enqueue(Chunk::Frame {
                head: header,
                pixels: Pixels::Cpu(frame.into_picture()),
            })?;
            self.frames += 1;
            Ok(())
        })();
        if let Err(error) = &result {
            self.state = State::Failed(error.clone());
        }
        result
    }

    /// Completes the input, drains and reaps the worker, then validates the
    /// private spool against all actual submissions and the final duration.
    pub fn finish(self, last_duration: NonZeroU64) -> Result<Encoded, Error> {
        let mut process = match self.state {
            State::Running(process) => process,
            State::Failed(error) => return Err(error),
        };
        if let Some(source) = self.source {
            source.complete(last_duration)?;
        }
        if self.frames == 0 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "no AV1 frames were submitted",
            ));
        }
        let mut end = Vec::new();
        end.try_reserve_exact(9)
            .map_err(|_| Error::new(ErrorKind::Limit, "cannot store AV1 completion"))?;
        coreav_backend_svt_av1::protocol::write_input_end(&mut end, self.frames)
            .map_err(backend_error)?;
        process.enqueue(end)?;
        process.finish()?.finish(&self.submitted, last_duration)
    }
}

/// Completed codec metadata and source-clock sample accounting.
#[derive(Debug)]
pub struct Encoded {
    /// Complete `av01` sample entry, including the validated `av1C`.
    pub sample_entry: Vec<u8>,
    /// Sample extents and times, in spool order and the source time base.
    pub packets: Vec<(PacketMeta, SampleTime)>,
    /// The pinned version observed in the worker protocol.
    pub version: String,
    /// Successfully drained and destroyed native segments.
    pub segments: u64,
    /// Total sample bytes in the caller-owned spool.
    pub bytes: u64,
}

#[derive(Clone, Copy)]
struct Spooled {
    ordinal: u64,
    bytes: u32,
    sync: bool,
}

struct SampleSpool {
    file: BufWriter<File>,
    samples: Vec<Spooled>,
    bytes: u64,
    limits: PacketLimits,
    deadline: Instant,
}

impl SampleSpool {
    fn check_deadline(&self) -> Result<(), BackendError> {
        if Instant::now() >= self.deadline {
            return Err(BackendError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "AV1 encoding deadline expired",
            )));
        }
        Ok(())
    }

    fn put(&mut self, sample: Sample) -> Result<(), BackendError> {
        self.check_deadline()?;
        if sample.ordinal != self.samples.len() as u64 {
            return Err(BackendError::Output("AV1 sample ordinal is not continuous"));
        }
        let size = u32::try_from(sample.bytes.len())
            .ok()
            .filter(|size| *size != 0 && *size <= self.limits.max_packet_bytes)
            .ok_or(BackendError::Limit("AV1 sample exceeds its byte limit"))?;
        let bytes = self
            .bytes
            .checked_add(u64::from(size))
            .filter(|bytes| *bytes <= self.limits.max_output_bytes)
            .ok_or(BackendError::Limit(
                "AV1 sample spool exceeds its byte limit",
            ))?;
        if self.samples.len() as u64 >= self.limits.max_packets {
            return Err(BackendError::Limit(
                "AV1 sample metadata exceeds its count limit",
            ));
        }
        self.samples
            .try_reserve(1)
            .map_err(|_| BackendError::Limit("cannot store AV1 sample metadata"))?;
        self.file.write_all(&sample.bytes)?;
        self.samples.push(Spooled {
            ordinal: sample.ordinal,
            bytes: size,
            sync: sample.sync,
        });
        self.bytes = bytes;
        Ok(())
    }

    fn finish(
        &mut self,
        submissions: &Submissions,
        last_duration: NonZeroU64,
    ) -> Result<Vec<(PacketMeta, SampleTime)>, Error> {
        self.check_deadline().map_err(backend_error)?;
        self.file.flush().map_err(spool_error)?;
        if self.file.get_ref().metadata().map_err(spool_error)?.len() != self.bytes {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "AV1 spool extent changed",
            ));
        }
        self.file
            .get_mut()
            .seek(SeekFrom::Start(0))
            .map_err(spool_error)?;
        let mut packets = Packets::new(self.limits);
        for sample in &self.samples {
            self.check_deadline().map_err(backend_error)?;
            let mut bytes = Vec::new();
            let size = usize::try_from(sample.bytes)
                .map_err(|_| Error::new(ErrorKind::Limit, "AV1 sample length overflows"))?;
            bytes
                .try_reserve_exact(size)
                .map_err(|_| Error::new(ErrorKind::Limit, "cannot replay AV1 sample"))?;
            bytes.resize(size, 0);
            self.file
                .get_mut()
                .read_exact(&mut bytes)
                .map_err(spool_error)?;
            self.check_deadline().map_err(backend_error)?;
            packets.accept(bytes, sample.ordinal, sample.sync, submissions)?;
        }
        let packets = packets.finish(submissions, last_duration)?;
        self.check_deadline().map_err(backend_error)?;
        Ok(packets)
    }
}

/// Output sink for the existing encoder process's reader thread.
/// Only complete, codec-validated samples reach its caller-owned spool.
/// Source timestamp binding waits until all submissions are known.
pub struct OutputSink {
    decoder: Decoder,
    stream: Output,
    spool: SampleSpool,
    request: Request,
    pixel_aspect: Option<(u32, u32)>,
    version: Option<String>,
    completed: bool,
    segments: u64,
    segment_capacity: u64,
    segment_max_pts: Option<u64>,
    failure: Option<Error>,
}

impl OutputSink {
    /// Starts an empty seekable spool with the worker's exact retained layout.
    /// `file` stays private to this sink until finalization; the caller keeps
    /// the spool's lifetime owner and never gives its path to the worker.
    pub fn new(
        mut file: File,
        request: Request,
        retention: Retention,
        pixel_aspect: Option<(u32, u32)>,
        deadline: Instant,
    ) -> Result<Self, Error> {
        if file.metadata().map_err(spool_error)?.len() != 0 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "AV1 spool must be empty",
            ));
        }
        file.seek(SeekFrom::Start(0)).map_err(spool_error)?;
        let colour = request.settings.color();
        let packet = usize::try_from(request.limits.packet_bytes())
            .map_err(|_| Error::new(ErrorKind::Limit, "AV1 packet bound overflows"))?;
        let frames = request.limits.frames();
        let operations = frames
            .checked_mul(16)
            .and_then(|n| n.checked_add(64))
            .ok_or_else(|| Error::new(ErrorKind::Limit, "AV1 parsing bound overflows"))?;
        let stream = Output::new(
            output::Expected {
                dimensions: frame::Expected {
                    width: request.settings.width(),
                    height: request.settings.height(),
                },
                primaries: colour.primaries(),
                transfer: colour.transfer(),
                matrix: colour.matrix(),
                range: if colour.full_range() {
                    ColorRange::Full
                } else {
                    ColorRange::Studio
                },
                chroma_position: match colour.chroma_position() {
                    0 => ChromaPosition::Unknown,
                    1 => ChromaPosition::Vertical,
                    2 => ChromaPosition::Colocated,
                    _ => {
                        return Err(Error::new(
                            ErrorKind::InvalidData,
                            "invalid AV1 chroma position",
                        ));
                    }
                },
            },
            output::Limits {
                max_packet_bytes: packet,
                max_buffered_bytes: packet
                    .checked_mul(2)
                    .ok_or_else(|| Error::new(ErrorKind::Limit, "AV1 buffering bound overflows"))?,
                max_obu_bytes: packet,
                max_temporal_unit_bytes: packet,
                max_sequence_bytes: packet.min(4096),
                max_total_bytes: request.limits.output_bytes(),
                max_obus: operations,
                max_frames: operations,
                max_frames_per_temporal_unit: 16,
                max_samples: frames,
                max_tiles: 4096,
            },
        );
        Ok(Self {
            decoder: Decoder::new(request, retention).map_err(backend_error)?,
            stream,
            spool: SampleSpool {
                file: BufWriter::with_capacity(1 << 20, file),
                samples: Vec::new(),
                bytes: 0,
                limits: PacketLimits {
                    max_packets: frames,
                    max_packet_bytes: request.limits.packet_bytes(),
                    max_output_bytes: request.limits.output_bytes(),
                },
                deadline,
            },
            request,
            pixel_aspect,
            version: None,
            completed: false,
            segments: 0,
            segment_capacity: u64::from(retention.pictures()),
            segment_max_pts: None,
            failure: None,
        })
    }

    /// Requires protocol completion, then binds every sample to the original
    /// submissions and derives one continuous source-clock track.
    pub fn finish(
        mut self,
        submissions: &Submissions,
        last_duration: NonZeroU64,
    ) -> Result<Encoded, Error> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        self.decoder.finish().map_err(backend_error)?;
        if !self.completed {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "AV1 worker did not complete",
            ));
        }
        let packets = self.spool.finish(submissions, last_duration)?;
        let sequence = self
            .stream
            .sequence()
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "AV1 sequence is missing"))?;
        let configuration = self
            .stream
            .configuration()
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "AV1 configuration is missing"))?;
        let mut av1c = Vec::new();
        av1c.try_reserve_exact(
            configuration.len().checked_add(4).ok_or_else(|| {
                Error::new(ErrorKind::Limit, "AV1 configuration length overflows")
            })?,
        )
        .map_err(|_| Error::new(ErrorKind::Limit, "cannot store AV1 configuration"))?;
        av1c.extend_from_slice(&sequence.av1c_header().bytes());
        av1c.extend_from_slice(configuration);
        let colour = self.request.settings.color();
        let sample_entry = av01_sample_entry(
            u16::try_from(self.request.settings.width())
                .map_err(|_| Error::new(ErrorKind::Limit, "AV1 width overflows"))?,
            u16::try_from(self.request.settings.height())
                .map_err(|_| Error::new(ErrorKind::Limit, "AV1 height overflows"))?,
            &av1c,
            self.pixel_aspect,
            Some(Nclx {
                primaries: u16::from(colour.primaries()),
                transfer: u16::from(colour.transfer()),
                matrix: u16::from(colour.matrix()),
                full_range: colour.full_range(),
            }),
        )
        .map_err(|error| Error::new(ErrorKind::InvalidData, error.to_string()))?;
        self.spool.check_deadline().map_err(backend_error)?;
        Ok(Encoded {
            sample_entry,
            packets,
            version: self
                .version
                .ok_or_else(|| Error::new(ErrorKind::InvalidData, "AV1 version is missing"))?,
            segments: self.segments,
            bytes: self.spool.bytes,
        })
    }
}

impl Sink for OutputSink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.spool.check_deadline().and_then(|()| {
            self.decoder.push(bytes, |record| {
                self.spool.check_deadline()?;
                match record {
                    Record::Version(version) => self.version = Some(version),
                    Record::Begin { .. } => {
                        self.stream
                            .begin_segment(self.segment_capacity)
                            .map_err(sample_error)?;
                        self.segment_max_pts = None;
                    }
                    Record::Packet { data, pts, .. } => {
                        if !data.is_empty() {
                            let pts = u64::try_from(pts)
                                .ok()
                                .filter(|pts| *pts < self.segment_capacity)
                                .ok_or(BackendError::Output(
                                    "AV1 native picture assertion is out of range",
                                ))?;
                            self.segment_max_pts =
                                Some(self.segment_max_pts.map_or(pts, |max| max.max(pts)));
                        }
                        self.stream.push(&data).map_err(sample_error)?;
                        while let Some(sample) = self.stream.next_sample().map_err(sample_error)? {
                            self.spool.put(sample)?;
                        }
                    }
                    Record::End { submitted, .. } => {
                        if self
                            .segment_max_pts
                            .is_some_and(|pts| pts >= u64::from(submitted))
                        {
                            return Err(BackendError::Output(
                                "AV1 native picture assertion exceeds submitted frames",
                            ));
                        }
                        self.spool.put(
                            self.stream
                                .end_segment(u64::from(submitted))
                                .map_err(sample_error)?,
                        )?;
                        self.segments += 1;
                    }
                    Record::Complete { frames, segments } => {
                        if frames != self.spool.samples.len() as u64 || segments != self.segments {
                            return Err(BackendError::Output(
                                "AV1 completion differs from validated samples",
                            ));
                        }
                        self.completed = true;
                    }
                }
                Ok(())
            })
        });
        if let Err(error) = result {
            let error = backend_error(error);
            self.failure = Some(error.clone());
            return Err(error);
        }
        Ok(())
    }
}

fn sample_error(error: output::Error) -> BackendError {
    match error {
        output::Error::Limit => BackendError::Limit("AV1 sample parser limit exceeded"),
        _ => BackendError::Output("AV1 output sample validation failed"),
    }
}

fn spool_error(error: io::Error) -> Error {
    Error::new(ErrorKind::Io, format!("AV1 sample spool: {}", error.kind()))
}

fn backend_error(error: BackendError) -> Error {
    let kind = match &error {
        BackendError::Invalid(_) | BackendError::Output(_) => ErrorKind::InvalidData,
        BackendError::Unsupported(_) => ErrorKind::Unsupported,
        BackendError::Limit(_) => ErrorKind::Limit,
        BackendError::Native(_) => ErrorKind::Backend,
        BackendError::Io(error) if error.kind() == io::ErrorKind::TimedOut => ErrorKind::Timeout,
        BackendError::Io(_) => ErrorKind::Io,
    };
    Error::new(kind, error.to_string())
}
