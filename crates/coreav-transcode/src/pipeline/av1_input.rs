use super::*;
use crate::av1::{self, Timeline};
use crate::scale::Side;
use coreav_backend_svt_av1::{
    Error as BackendError,
    model::{Color, MAX_PACKET_BYTES, RETAINED_BYTES, Retention, Settings},
    protocol::{Limits, Request},
};
use coreav_frame::{Chroma, ChromaLocation, Picture};

/// Options shared by the source, audio, decoder and output writer.
#[derive(Clone, Debug)]
pub struct CommonOptions {
    /// Copied or encoded source audio.
    pub audio: AudioMode,
    /// FFmpeg path used only for independently selected AAC encoding.
    pub aac_program: Option<PathBuf>,
    /// Cover pictures omitted from MP4 metadata copying.
    pub left_out_covers: Vec<u64>,
    /// Whole-job time budget, including input opening after preparation.
    pub timeout: Duration,
    /// Decoder selection policy.
    pub policy: Policy,
    /// MP4 parser and writer limits.
    pub isobmff_limits: coreav_isobmff::Limits,
    /// Matroska source limits.
    pub matroska_limits: coreav_mkv::Limits,
    /// Decoder and parent staging limits.
    pub decode_limits: coreav_decode::Limits,
    /// Maximum output-wire and sample-spool bytes.
    pub max_spool_bytes: u64,
}

impl Default for CommonOptions {
    fn default() -> Self {
        let options = TranscodeOptions::default();
        Self {
            audio: options.audio,
            aac_program: options.ffmpeg,
            left_out_covers: options.left_out_covers,
            timeout: options.timeout,
            policy: options.policy,
            isobmff_limits: options.isobmff_limits,
            matroska_limits: options.matroska_limits,
            decode_limits: options.decode_limits,
            max_spool_bytes: options.max_spool_bytes,
        }
    }
}

/// Explicit AV1 settings, without external H.264 program options.
#[derive(Clone, Debug)]
pub struct Av1Options {
    /// Source, audio, decoder and writer settings.
    pub common: CommonOptions,
    /// Explicit geometry conversion; absent means unchanged visible size.
    pub scale: Option<Scale>,
    preset: u8,
    crf: u8,
}

impl Default for Av1Options {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            scale: None,
            preset: 8,
            crf: 32,
        }
    }
}

impl Av1Options {
    /// Sets a numeric preset 0 through 13 and integer CRF 0 through 63.
    pub fn with_quality(mut self, preset: u8, crf: u8) -> Result<Self, Error> {
        if preset > 13 || crf > 63 {
            return Err(unsupported("invalid SVT-AV1 preset or CRF"));
        }
        self.preset = preset;
        self.crf = crf;
        Ok(self)
    }
}

/// Effective software-encoder settings and successfully completed runtime facts.
#[derive(Clone, Debug)]
pub struct Av1Encoding {
    /// Pinned version observed in the validated worker stream.
    pub version: String,
    /// Numeric SVT preset.
    pub preset: u8,
    /// Integer constant-rate factor.
    pub crf: u8,
    /// Exact presented-average settings cadence, not a source-clock conversion.
    pub cadence: FrameRate,
    /// Effective colour, range and chroma siting.
    pub color: ColorInfo,
    /// Closed-GOP keyframe interval.
    pub keyint: u32,
    /// Assigned retained-input budget, excluding SDK internal allocations.
    pub retained_budget: u64,
    /// Fixed retained owner and EOF bytes.
    pub fixed_retained_bytes: u64,
    /// Charged retained bytes per input picture.
    pub retained_picture_bytes: u64,
    /// Maximum pictures in one nonoverlapping native session.
    pub segment_frames: u32,
    /// Successfully drained and destroyed sessions.
    pub segments: u64,
    /// Requested worker address-space allowance, not proof of OS enforcement.
    pub address_space: u64,
}

/// Source video and its explicit AV1 encoding result.
#[derive(Clone, Debug)]
pub struct Av1VideoSummary {
    /// Selected source stream index.
    pub stream_index: usize,
    /// Source codec, not the output codec.
    pub codec: Codec,
    /// Presented frames submitted and reconciled with output.
    pub frames: u64,
    /// Known reference-only frames not presented by the source.
    pub not_presented: u64,
    /// Actual decoder information.
    pub decoder: DecoderInfo,
    /// Decoder candidates skipped before selection.
    pub skipped: Vec<Skipped>,
    /// Encoded width.
    pub width: u32,
    /// Encoded height.
    pub height: u32,
    /// Exact non-square sample aspect, when present.
    pub sample_aspect: Option<(u32, u32)>,
    /// Effective encoder settings and completed runtime facts.
    pub encoding: Av1Encoding,
}

/// AV1 result, distinct from the legacy H.264 version 1 summary.
#[derive(Clone, Debug)]
pub struct Av1Summary {
    /// Encoded video.
    pub video: Av1VideoSummary,
    /// Independently encoded audio, absent when audio is copied or omitted.
    pub audio: Option<AudioSummary>,
    /// Shared replacement/remux result.
    pub remux: Remuxed,
}

/// A checked explicit AV1 job hosted by the sandboxed current executable.
pub struct Av1Transcode<'a> {
    options: TranscodeOptions,
    runtime: Runtime,
    launcher: &'a ExecLauncher,
    deadline: Instant,
}

impl<'a> Av1Transcode<'a> {
    /// Whether this build includes native support on a Unix release target.
    /// This is not a native execution or correctness verdict.
    pub const fn compiled() -> bool {
        cfg!(all(
            feature = "svt-av1-native",
            any(
                all(
                    target_os = "linux",
                    target_env = "musl",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(
                    target_os = "macos",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                )
            )
        ))
    }

    /// Checks build and parameter requirements before media or output I/O.
    /// The host must implement the private worker entry in this executable.
    pub fn prepare(options: Av1Options, launcher: &'a ExecLauncher) -> Result<Self, Error> {
        check_scale(options.scale)?;
        if !Self::compiled() {
            return Err(unsupported(
                "no AV1 encoder: this build has no native SVT-AV1 backend for this target",
            ));
        }
        if options.common.decode_limits.max_memory_bytes == 0
            || options.common.decode_limits.max_packet_bytes == 0
            || options.common.max_spool_bytes < 18
        {
            return Err(unsupported("invalid AV1 memory, packet or spool limits"));
        }
        let deadline = Instant::now()
            .checked_add(options.common.timeout)
            .ok_or_else(|| unsupported("timeout out of range"))?;
        let program = std::env::current_exe().map_err(|error| {
            Error::Encode(coreav_decode::Error::new(
                ErrorKind::Io,
                format!("current AV1 worker: {}", error.kind()),
            ))
        })?;
        if !program.is_absolute() {
            return Err(unsupported(
                "the private AV1 worker must be an absolute path",
            ));
        }
        let runtime = Runtime {
            program,
            preset: options.preset,
            crf: options.crf,
            retention: compiled_retention,
            #[cfg(test)]
            synthetic: false,
        };
        let mut common = options.common;
        if matches!(common.audio, AudioMode::Aac(_)) {
            let program = find_encoder(EncoderChoice::Ffmpeg, None, common.aac_program.clone())
                .filter(|program| crate::selection::executable(program.path()))
                .ok_or_else(|| {
                    unsupported("no AAC encoder: the selected FFmpeg program is unavailable")
                })?;
            common.aac_program = Some(program.path().to_path_buf());
        }
        let options = TranscodeOptions {
            audio: common.audio,
            ffmpeg: common.aac_program,
            left_out_covers: common.left_out_covers,
            timeout: common.timeout,
            policy: common.policy,
            isobmff_limits: common.isobmff_limits,
            matroska_limits: common.matroska_limits,
            decode_limits: common.decode_limits,
            max_spool_bytes: common.max_spool_bytes,
            scale: options.scale,
            ..TranscodeOptions::default()
        };
        Ok(Self {
            options,
            runtime,
            launcher,
            deadline,
        })
    }

    /// Executes through the common decode, pacing, audio and MP4 remux owners.
    pub fn run<S: Source, W: Write>(
        self,
        source: S,
        kind: SourceKind,
        backends: &[&dyn Backend],
        output: &Path,
        out: W,
    ) -> Result<Av1Summary, Error> {
        self.check_deadline()?;
        let done = execute(
            source,
            kind,
            Job {
                options: &self.options,
                launcher: Some(self.launcher),
                deadline: self.deadline,
                video: VideoMode::Av1(self.runtime),
            },
            backends,
            output,
            out,
        )?;
        let encoding = match done.video.encoding {
            Encoding::Av1(encoding) => encoding,
            Encoding::H264 { .. } => {
                return Err(encode_error("H.264 cannot produce an AV1 summary".into()));
            }
        };
        Ok(Av1Summary {
            video: Av1VideoSummary {
                stream_index: done.stream_index,
                codec: done.source_codec,
                frames: done.video.frames,
                not_presented: done.video.not_presented,
                decoder: done.decoder,
                skipped: done.skipped,
                width: done.video.width,
                height: done.video.height,
                sample_aspect: done.video.sample_aspect,
                encoding,
            },
            audio: done.audio,
            remux: done.remux,
        })
    }

    /// Checks the stored deadline before host-side source opening or identification.
    pub fn check_deadline(&self) -> Result<(), Error> {
        if Instant::now() >= self.deadline {
            return Err(timed_out());
        }
        Ok(())
    }
}

fn compiled_retention(request: Request) -> Result<Retention, BackendError> {
    #[cfg(feature = "svt-av1-native")]
    {
        coreav_backend_svt_av1::worker::retention(request)
    }
    #[cfg(not(feature = "svt-av1-native"))]
    {
        let _ = request;
        Err(BackendError::Unsupported("native SVT-AV1 is not compiled"))
    }
}

pub(super) struct Runtime {
    program: PathBuf,
    preset: u8,
    crf: u8,
    retention: fn(Request) -> Result<Retention, BackendError>,
    #[cfg(test)]
    synthetic: bool,
}

pub(super) struct Plan {
    runtime: Runtime,
    timeline: Timeline,
    references: Vec<i64>,
    staging_budget: u64,
}

impl Plan {
    pub(super) fn new(
        video: &VideoStream,
        options: &TranscodeOptions,
        runtime: Runtime,
        deadline: Instant,
    ) -> Result<Self, Error> {
        let traits = video.stream.traits();
        if traits.bit_depth.is_some_and(|depth| depth != 8)
            || traits.chroma.is_some_and(|chroma| chroma != Chroma::Cs420)
        {
            return Err(unsupported("AV1 requires an 8-bit 4:2:0 source"));
        }
        check_header_color(video.stream.color)?;
        let budget = options.decode_limits.max_memory_bytes / 4;
        let count = u64::try_from(video.packets.len())
            .map_err(|_| unsupported("too many AV1 source packets"))?
            .max(1);
        let timeline = Timeline::new(&video.packets, video.time_base, count, budget, deadline)
            .map_err(source_error)?;
        let references_count = video
            .packets
            .iter()
            .filter(|packet| !packet.presented)
            .count();
        if references_count as u128 * 8 > u128::from(budget) {
            return Err(source_error(coreav_decode::Error::new(
                ErrorKind::Limit,
                "AV1 reference timeline exceeds its budget",
            )));
        }
        let mut references = Vec::new();
        references
            .try_reserve_exact(references_count)
            .map_err(|_| {
                source_error(coreav_decode::Error::new(
                    ErrorKind::Limit,
                    "cannot store AV1 reference times",
                ))
            })?;
        if references.capacity() as u128 * 8 > u128::from(budget) {
            return Err(source_error(coreav_decode::Error::new(
                ErrorKind::Limit,
                "AV1 reference capacity exceeds its budget",
            )));
        }
        for packet in &video.packets {
            if Instant::now() >= deadline {
                return Err(timed_out());
            }
            if !packet.presented {
                references.push(packet.pts);
            }
        }
        references.sort_unstable();
        if Instant::now() >= deadline {
            return Err(timed_out());
        }
        let staging_budget = options
            .decode_limits
            .max_memory_bytes
            .checked_sub(timeline.metadata_bytes())
            .and_then(|bytes| bytes.checked_sub(references.capacity() as u64 * 8))
            .ok_or_else(|| unsupported("AV1 source metadata exceeds the parent budget"))?;
        Ok(Self {
            runtime,
            timeline,
            references,
            staging_budget,
        })
    }
}

pub(super) struct Run<'a> {
    video: &'a VideoStream,
    options: &'a TranscodeOptions,
    launcher: Option<&'a ExecLauncher>,
    deadline: Instant,
    spool: Arc<Spool>,
    state: State,
    references: Vec<i64>,
    not_presented: u64,
    staging_budget: u64,
}

enum State {
    Waiting(Plan),
    Running(Box<Active>),
    Failed(coreav_decode::Error),
}

struct Active {
    encoder: av1::Encoder,
    settings: Settings,
    retention: Retention,
    input_size: (u32, u32),
    sample_aspect: Option<(u32, u32)>,
    presentation_size: (u32, u32),
    color: ColorInfo,
    duration: std::num::NonZeroU64,
    frames: u64,
}

impl<'a> Run<'a> {
    pub(super) fn new(
        mut plan: Plan,
        video: &'a VideoStream,
        options: &'a TranscodeOptions,
        launcher: Option<&'a ExecLauncher>,
        deadline: Instant,
        spool: Arc<Spool>,
    ) -> Result<Self, Error> {
        let direct_fixture = {
            #[cfg(test)]
            {
                plan.runtime.synthetic
            }
            #[cfg(not(test))]
            {
                false
            }
        };
        if launcher.is_none() && !direct_fixture {
            return Err(unsupported("AV1 requires the sandbox launcher"));
        }
        let references = std::mem::take(&mut plan.references);
        let staging_budget = plan.staging_budget;
        Ok(Self {
            video,
            options,
            launcher,
            deadline,
            spool,
            state: State::Waiting(plan),
            references,
            not_presented: 0,
            staging_budget,
        })
    }

    pub(super) fn presented(&self, frame: &Frame) -> bool {
        let Some(pts) = frame.pts else {
            return false;
        };
        match &self.state {
            State::Waiting(plan) => plan.timeline.contains(pts),
            State::Running(active) => active.encoder.presents(pts),
            State::Failed(_) => false,
        }
    }

    pub(super) fn take(&mut self, frame: Frame) -> Result<(), Error> {
        if let State::Failed(error) = &self.state {
            return Err(Error::Encode(error.clone()));
        }
        let started = matches!(self.state, State::Running(_));
        let result = self.take_inner(frame);
        if let Err(error) = &result {
            let failure = match error {
                Error::Encode(error) | Error::Decode(error) => error.clone(),
                _ => coreav_decode::Error::new(ErrorKind::InvalidData, error.to_string()),
            };
            self.state = State::Failed(failure.clone());
            if started {
                return Err(Error::Encode(failure));
            }
        }
        result
    }

    fn take_inner(&mut self, frame: Frame) -> Result<(), Error> {
        if Instant::now() >= self.deadline {
            return Err(timed_out());
        }
        let pts = frame
            .pts
            .ok_or_else(|| unsupported("an AV1 source frame has no timestamp"))?;
        if !self.presented(&frame) {
            if self.references.binary_search(&pts).is_ok() {
                self.not_presented += 1;
                return Ok(());
            }
            return Err(unsupported("an AV1 frame has no matching source timestamp"));
        }
        if let State::Waiting(_) = self.state {
            let failed = coreav_decode::Error::new(
                ErrorKind::Backend,
                "AV1 initialization did not complete",
            );
            let State::Waiting(plan) = std::mem::replace(&mut self.state, State::Failed(failed))
            else {
                return Err(encode_error("AV1 source state changed".into()));
            };
            let active = self.start(plan, frame, pts)?;
            self.state = State::Running(Box::new(active));
            return Ok(());
        }
        let State::Running(active) = &mut self.state else {
            return Err(encode_error("AV1 is not accepting frames".into()));
        };
        if frame.format() != PixelFormat::YUV420P
            || (frame.width(), frame.height()) != active.input_size
        {
            return Err(unsupported("the AV1 source format or visible size changed"));
        }
        let (color, _) = admit_color(self.video.stream.color, frame.color)?;
        if color != active.color {
            return Err(unsupported("the AV1 source colour changed"));
        }
        let picture = prepare_picture(frame, active.settings, active.color, self.deadline)?;
        active
            .encoder
            .send_frame(picture, pts)
            .map_err(Error::Encode)
    }

    fn start(&self, plan: Plan, frame: Frame, pts: i64) -> Result<Active, Error> {
        if frame.format() != PixelFormat::YUV420P {
            return Err(unsupported("AV1 requires native planar 8-bit 4:2:0 frames"));
        }
        let input_size = (frame.width(), frame.height());
        let size = self
            .options
            .scale
            .map_or(Some(input_size), |scale| {
                scale.resolve(input_size.0, input_size.1)
            })
            .ok_or_else(|| unsupported("the explicit AV1 scale is invalid"))?;
        let (color, native_color) = admit_color(self.video.stream.color, frame.color)?;
        if input_size != size && color.chroma_location.is_none() {
            return Err(unsupported("AV1 scaling requires known chroma siting"));
        }
        let rate = plan.timeline.rate();
        let settings = Settings::new(
            size.0,
            size.1,
            (rate.num, rate.den),
            plan.runtime.preset,
            plan.runtime.crf,
            native_color,
        )
        .map_err(backend_source_error)?;
        let input_bytes = (0..3)
            .try_fold(0u64, |sum, plane| {
                let (_, rows) =
                    PixelFormat::YUV420P.plane_size(plane, input_size.0, input_size.1)?;
                let row = PixelFormat::YUV420P.row_bytes(plane, input_size.0, input_size.1)?;
                sum.checked_add(u64::try_from(row).ok()?.checked_mul(u64::from(rows))?)
            })
            .ok_or_else(|| unsupported("AV1 parent frame size overflows"))?;
        (settings.raw_bytes() as u64)
            .checked_mul(crate::encoder::QUEUED_FRAMES as u64 + 2)
            .and_then(|bytes| bytes.checked_add(input_bytes))
            .filter(|bytes| *bytes <= self.staging_budget)
            .ok_or_else(|| unsupported("AV1 parent picture staging exceeds its budget"))?;
        let sample_aspect = exact_aspect(self.video.pixel_aspect, input_size, size)?;
        let (num, den) = sample_aspect.unwrap_or((1, 1));
        let width = (u128::from(size.0) * u128::from(num) * 65536 / u128::from(den))
            .try_into()
            .ok()
            .filter(|width: &u32| *width != 0)
            .ok_or_else(|| unsupported("AV1 display width exceeds the track header"))?;
        let height = size
            .1
            .checked_mul(65536)
            .ok_or_else(|| unsupported("AV1 display height exceeds the track header"))?;
        let request = Request {
            settings,
            limits: Limits::new(
                self.options
                    .decode_limits
                    .max_memory_bytes
                    .min(RETAINED_BYTES),
                self.options
                    .max_spool_bytes
                    .min(coreav_backend_svt_av1::protocol::MAX_OUTPUT_BYTES),
                plan.timeline.frames(),
                u32::try_from(
                    self.options
                        .decode_limits
                        .max_packet_bytes
                        .min(MAX_PACKET_BYTES as u64),
                )
                .map_err(|_| unsupported("AV1 packet limit exceeds its field"))?,
            )
            .map_err(backend_source_error)?,
        };
        let retention = (plan.runtime.retention)(request)
            .map_err(|error| Error::Encode(av1::backend_error(error)))?;
        let duration = plan.timeline.last_duration();
        let frames = plan.timeline.frames();
        let picture = prepare_picture(frame, settings, color, self.deadline)?;
        let mut encoder = av1::Encoder::spawn_retaining(
            &plan.runtime.program,
            request,
            retention,
            self.spool.handle()?,
            av1::Launch {
                launcher: self.launcher,
                limits: EncoderLimits::new(self.options.decode_limits.max_memory_bytes),
                deadline: self.deadline,
                pixel_aspect: sample_aspect,
            },
            plan.timeline,
            self.spool.clone(),
        )
        .map_err(Error::Encode)?;
        encoder.send_frame(picture, pts).map_err(Error::Encode)?;
        Ok(Active {
            encoder,
            settings,
            retention,
            input_size,
            sample_aspect,
            presentation_size: (width, height),
            color,
            duration,
            frames,
        })
    }

    pub(super) fn finish(self) -> Result<CompletedVideo, Error> {
        let active = match self.state {
            State::Waiting(_) => {
                return Err(unsupported("the decoder returned no presented AV1 frame"));
            }
            State::Failed(error) => return Err(Error::Encode(error)),
            State::Running(active) => active,
        };
        let encoded = active
            .encoder
            .finish(active.duration)
            .map_err(Error::Encode)?;
        let samples = encoded
            .packets
            .iter()
            .map(|(packet, time)| ReplacementSample {
                size: packet.bytes(),
                sync: packet.sync(),
                decode_time: time.dts,
                presentation_time: time.pts,
            })
            .collect();
        let (num, den) = active.settings.rate();
        let encoding = Av1Encoding {
            version: encoded.version,
            preset: active.settings.preset(),
            crf: active.settings.crf(),
            cadence: FrameRate { num, den },
            color: active.color,
            keyint: active.settings.keyint(),
            retained_budget: active.retention.budget(),
            fixed_retained_bytes: active.retention.fixed_bytes(),
            retained_picture_bytes: active.retention.picture_bytes(),
            segment_frames: active.retention.pictures(),
            segments: encoded.segments,
            address_space: self.options.decode_limits.max_memory_bytes,
        };
        Ok(CompletedVideo {
            sample_entry: encoded.sample_entry,
            samples,
            last_duration: u32::try_from(active.duration.get())
                .map_err(|_| unsupported("AV1 duration exceeds MP4 fields"))?,
            presentation_size: active.presentation_size,
            frames: active.frames,
            dropped: 0,
            not_presented: self.not_presented,
            encoding: Encoding::Av1(encoding),
            width: active.settings.width(),
            height: active.settings.height(),
            sample_aspect: active.sample_aspect,
        })
    }
}

fn source_error(error: coreav_decode::Error) -> Error {
    match error.kind {
        ErrorKind::Unsupported | ErrorKind::InvalidData => Error::Unsupported(error.message),
        _ => Error::Encode(error),
    }
}

fn check_scale(scale: Option<Scale>) -> Result<(), Error> {
    if let Some(scale) = scale {
        for (side, max) in [(scale.width, 16_384), (scale.height, 8_704)] {
            if let Side::Pixels(value) = side
                && (!(64..=max).contains(&value) || value % 2 != 0)
            {
                return Err(unsupported(
                    "explicit AV1 scale requires even dimensions within native bounds",
                ));
            }
        }
    }
    Ok(())
}

fn backend_source_error(error: BackendError) -> Error {
    source_error(av1::backend_error(error))
}

fn check_header_color(color: ColorInfo) -> Result<(), Error> {
    Color::new(
        color.primaries.filter(|value| *value != 2).unwrap_or(1),
        color.transfer.filter(|value| *value != 2).unwrap_or(1),
        color.matrix.filter(|value| *value != 2).unwrap_or(1),
        color.full_range.unwrap_or(false),
        chroma_position(color.chroma_location)?,
    )
    .map_err(backend_source_error)?;
    Ok(())
}

fn chroma_position(location: Option<ChromaLocation>) -> Result<u8, Error> {
    match location {
        None => Ok(0),
        Some(ChromaLocation::Left) => Ok(1),
        Some(ChromaLocation::TopLeft) => Ok(2),
        _ => Err(unsupported(
            "AV1 cannot preserve this source chroma location",
        )),
    }
}

fn admit_color(header: ColorInfo, decoded: ColorInfo) -> Result<(ColorInfo, Color), Error> {
    fn merge<T: Copy + Eq>(header: Option<T>, decoded: Option<T>) -> Result<Option<T>, Error> {
        if matches!((header, decoded), (Some(a), Some(b)) if a != b) {
            return Err(unsupported(
                "source and decoded AV1 colour metadata disagree",
            ));
        }
        Ok(header.or(decoded))
    }
    let assigned = |value: Option<u8>| value.filter(|value| *value != 2);
    let mut color = ColorInfo {
        primaries: merge(assigned(header.primaries), assigned(decoded.primaries))?,
        transfer: merge(assigned(header.transfer), assigned(decoded.transfer))?,
        matrix: merge(assigned(header.matrix), assigned(decoded.matrix))?,
        full_range: merge(header.full_range, decoded.full_range)?,
        chroma_location: merge(header.chroma_location, decoded.chroma_location)?,
    };
    let transfer = color
        .transfer
        .ok_or_else(|| unsupported("AV1 needs positive SDR transfer metadata"))?;
    let range = color
        .full_range
        .ok_or_else(|| unsupported("AV1 needs an applied source range"))?;
    let native = Color::new(
        color.primaries.unwrap_or(2),
        transfer,
        color.matrix.unwrap_or(2),
        range,
        chroma_position(color.chroma_location)?,
    )
    .map_err(backend_source_error)?;
    color.primaries = Some(native.primaries());
    color.matrix = Some(native.matrix());
    Ok((color, native))
}

fn exact_aspect(
    aspect: Option<(u64, u64)>,
    input: (u32, u32),
    output: (u32, u32),
) -> Result<Option<(u32, u32)>, Error> {
    let (num, den) = aspect.unwrap_or((1, 1));
    if num == 0 || den == 0 {
        return Err(unsupported("AV1 source sample aspect is zero"));
    }
    let num = u128::from(num) * u128::from(input.0) * u128::from(output.1);
    let den = u128::from(den) * u128::from(input.1) * u128::from(output.0);
    let divisor = gcd(num, den);
    let (num, den) = (num / divisor, den / divisor);
    if num == den {
        return Ok(None);
    }
    let terms = u32::try_from(num)
        .ok()
        .zip(u32::try_from(den).ok())
        .ok_or_else(|| unsupported("exact AV1 sample aspect exceeds MP4 fields"))?;
    Ok(Some(terms))
}

fn prepare_picture(
    frame: Frame,
    settings: Settings,
    color: ColorInfo,
    deadline: Instant,
) -> Result<Picture, Error> {
    let original = (frame.width(), frame.height(), frame.format());
    let mut picture = frame
        .pixels
        .into_cpu()
        .map_err(|error| unsupported(format!("AV1 frame copy: {error}")))?;
    if (picture.width(), picture.height(), picture.format()) != original {
        return Err(unsupported(
            "the copied AV1 picture differs from its decoded descriptor",
        ));
    }
    if (picture.width(), picture.height()) != (settings.width(), settings.height()) {
        picture = picture
            .resize_with(
                settings.width(),
                settings.height(),
                Filter::Bicubic,
                ChromaSiting::Location(color.chroma_location),
            )
            .map_err(|error| unsupported(format!("explicit AV1 scale: {error}")))?;
    }
    if Instant::now() >= deadline {
        return Err(timed_out());
    }
    Ok(picture)
}

#[cfg(test)]
#[path = "../../../coreav-backend-svt-av1/tests/frames.rs"]
#[allow(dead_code)]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_backend_svt_av1::protocol::OutputWriter;
    use coreav_isobmff::{Allowlist, Budget, FourCc, SliceSource};
    use coreav_isobmff_write::mux::{self, FileType, MovieHeader, Muxer, TrackHeader, TrackMedia};
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicU64, AtomicUsize},
        time::Duration,
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Temporary(PathBuf);
    impl Temporary {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "coreav-av1-engine-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn color() -> ColorInfo {
        ColorInfo {
            primaries: Some(1),
            transfer: Some(1),
            matrix: Some(1),
            full_range: Some(false),
            chroma_location: Some(ChromaLocation::Left),
        }
    }

    fn sequence(transfer: u32) -> Vec<u8> {
        let mut syntax = fixtures::Seq::default().syntax();
        syntax.change("chroma_position", 1).unwrap();
        syntax
            .replace(
                "color_description",
                &[(1, 1), (8, 1), (8, transfer), (8, 1)],
            )
            .unwrap();
        fixtures::packet(1, &syntax.bytes())
    }

    fn native_plan(request: Request) -> Result<Retention, BackendError> {
        Retention::new(
            request.settings,
            request.limits.retained_bytes(),
            64,
            16,
            32,
        )
    }

    fn runtime(dir: &Temporary, changed: &str) -> Runtime {
        let settings = Settings::new(
            64,
            64,
            (120, 7),
            8,
            32,
            Color::new(1, 1, 1, false, 1).unwrap(),
        )
        .unwrap();
        let request = Request {
            settings,
            limits: Limits::new(40_000, 1 << 20, 4, 4096).unwrap(),
        };
        let retention = native_plan(request).unwrap();
        let mut bytes = Vec::new();
        let mut writer = OutputWriter::new(&mut bytes, "4.2.0", request.limits).unwrap();
        let mut segments = 0;
        let mut base = 0;
        while base < 4 {
            let count = (4 - base).min(u64::from(retention.pictures())) as u32;
            writer.begin(base, retention, settings.keyint()).unwrap();
            let mut payload = Vec::new();
            for index in 0..count {
                payload.extend_from_slice(&fixtures::packet(2, &[]));
                if index == 0 {
                    payload.extend_from_slice(&sequence(1));
                }
                payload.extend_from_slice(
                    &fixtures::Coded::default().packet(fixtures::Seq::default()),
                );
            }
            writer.packet(0, -1, 1, 0, &payload).unwrap();
            writer
                .end(base, count, 1, retention.peak_bytes(count).unwrap())
                .unwrap();
            segments += 1;
            base += u64::from(count);
        }
        writer.complete(4, segments).unwrap();
        if changed == "truncated" {
            bytes.pop();
        }
        let program = dir.0.join("synthetic-worker");
        let escaped = bytes
            .iter()
            .map(|byte| format!("\\{byte:03o}"))
            .collect::<String>();
        std::fs::write(
            &program,
            format!("#!/bin/sh\n/bin/cat >/dev/null\nprintf '%b' '{escaped}'\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Runtime {
            program,
            preset: 8,
            crf: 32,
            retention: native_plan,
            synthetic: true,
        }
    }

    fn track(entry: Vec<u8>, media: TrackMedia, timescale: u32) -> TrackHeader {
        TrackHeader {
            media,
            timescale,
            language: *b"und",
            creation_time: 0,
            media_creation_time: 0,
            flags: 3,
            layer: 0,
            alternate_group: 0,
            edits: Vec::new(),
            duration: None,
            media_duration: None,
            sample_entry: entry,
            sample_table_boxes: Vec::new(),
            boxes: Vec::new(),
        }
    }

    fn source(origin: i64, transfer: u32, audio: bool) -> Vec<u8> {
        let parsed = fixtures::parse_sequence(&sequence(1)).unwrap();
        let sequence = sequence(transfer);
        let config = [parsed.av1c_header().bytes().as_slice(), sequence.as_slice()].concat();
        let entry = mux::av01_sample_entry(64, 64, &config, None, None).unwrap();
        let mut muxer = Muxer::new(
            FileType {
                major: FourCc(*b"isom"),
                minor: 0,
                compatible: vec![FourCc(*b"isom"), FourCc(*b"av01")],
            },
            MovieHeader {
                timescale: 30,
                creation_time: 0,
                boxes: Vec::new(),
            },
            &coreav_isobmff::Limits::default(),
        )
        .unwrap();
        let mut header = track(
            entry,
            TrackMedia::Video {
                matrix: [65536, 0, 0, 0, 65536, 0, 0, 0, 1 << 30],
                width: 64 << 16,
                height: 64 << 16,
            },
            30,
        );
        if origin != 0 {
            header.edits.push(mux::Edit {
                duration: 7,
                media_time: origin,
                rate: mux::Edit::UNIT_RATE,
            });
        }
        let video = muxer.add_track(header).unwrap();
        let payload = fixtures::Coded::default().packet(fixtures::Seq::default());
        for duration in [1, 2, 1, 3] {
            muxer
                .push_sample(
                    video,
                    mux::Sample {
                        size: payload.len() as u32,
                        duration,
                        sync: true,
                        composition_offset: origin,
                    },
                )
                .unwrap();
        }
        if audio {
            let entry = mux::audio_sample_entry(
                FourCc(*b"mp4a"),
                2,
                16,
                48_000,
                &[mux::RawBox {
                    kind: FourCc(*b"esds"),
                    payload: mux::esds(&[0x11, 0x90]),
                }],
            )
            .unwrap();
            let track = muxer
                .add_track(track(entry, TrackMedia::Audio { volume: 256 }, 48_000))
                .unwrap();
            for _ in 0..4 {
                muxer
                    .push_sample(
                        track,
                        mux::Sample {
                            size: 3,
                            duration: 1024,
                            sync: true,
                            composition_offset: 0,
                        },
                    )
                    .unwrap();
            }
        }
        let mut bytes = Vec::new();
        muxer.write_head(&mut bytes).unwrap();
        muxer
            .for_each_chunk::<coreav_isobmff::Error>(|index, count| {
                for _ in 0..count {
                    bytes.extend_from_slice(if index == video {
                        &payload
                    } else {
                        &[0xaa, 0xbb, 0xcc]
                    });
                }
                Ok(())
            })
            .unwrap();
        bytes
    }

    struct SyntheticDecoder {
        frames: VecDeque<Frame>,
        sent: usize,
        change: &'static str,
        info: DecoderInfo,
    }
    impl coreav_decode::Decoder for SyntheticDecoder {
        fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), coreav_decode::Error> {
            let mut frame = Frame {
                pts: packet.pts,
                color: color(),
                pixels: Pixels::Cpu(Picture::zeroed(64, 64, PixelFormat::YUV420P).unwrap()),
            };
            if self.sent == 1 {
                match self.change {
                    "timestamp" => frame.pts = frame.pts.map(|pts| pts + 1),
                    "geometry" => {
                        frame.pixels =
                            Pixels::Cpu(Picture::zeroed(66, 64, PixelFormat::YUV420P).unwrap())
                    }
                    "colour" => frame.color.primaries = Some(9),
                    "missing" => {
                        self.sent += 1;
                        return Ok(());
                    }
                    _ => {}
                }
            }
            self.frames.push_back(frame);
            self.sent += 1;
            Ok(())
        }
        fn send_eof(&mut self) -> Result<(), coreav_decode::Error> {
            Ok(())
        }
        fn receive_frame(&mut self) -> Result<Option<Frame>, coreav_decode::Error> {
            Ok(self.frames.pop_front())
        }
        fn info(&self) -> &DecoderInfo {
            &self.info
        }
    }
    struct SyntheticBackend {
        opened: Arc<AtomicUsize>,
        change: &'static str,
    }
    impl Backend for SyntheticBackend {
        fn kind(&self) -> coreav_decode::BackendKind {
            coreav_decode::BackendKind::Native
        }
        fn supports(
            &self,
            _: &coreav_decode::StreamConfig,
            _: &coreav_decode::StreamTraits,
        ) -> coreav_decode::Support {
            coreav_decode::Support::Yes
        }
        fn open(
            &self,
            _: &coreav_decode::StreamConfig,
            _: &coreav_decode::Limits,
        ) -> Result<Box<dyn coreav_decode::Decoder>, coreav_decode::Error> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(SyntheticDecoder {
                frames: VecDeque::new(),
                sent: 0,
                change: self.change,
                info: DecoderInfo {
                    backend: self.kind(),
                    isolation: coreav_decode::Isolation::InProcess,
                    decoder: "synthetic frame source".into(),
                },
            }))
        }
    }

    fn options() -> TranscodeOptions {
        let mut options = TranscodeOptions::default();
        options.decode_limits.max_memory_bytes = 40_000;
        options.decode_limits.max_packet_bytes = 4096;
        options.max_spool_bytes = 1 << 20;
        options.timeout = Duration::from_secs(10);
        options
    }

    fn packet_facts(bytes: &[u8], index: usize) -> Vec<(Option<Rational>, u32, Vec<u8>)> {
        let limits = coreav_isobmff::Limits::default();
        let movie =
            coreav_isobmff::probe(SliceSource::new(bytes), &limits, &Allowlist::default()).unwrap();
        let mut budget = Budget::new(&limits);
        movie
            .packets(index, SliceSource::new(bytes), &limits, &mut budget)
            .unwrap()
            .map(|packet| {
                let packet = packet.unwrap();
                (
                    packet.pts,
                    packet.duration,
                    bytes[packet.offset as usize..packet.offset as usize + packet.size as usize]
                        .to_vec(),
                )
            })
            .collect()
    }

    #[test]
    #[cfg(unix)]
    fn synthetic_common_engine_keeps_source_times_and_copied_audio() {
        for origin in [0, 5] {
            let temporary = Temporary::new();
            let source = source(origin, 1, true);
            let backend = SyntheticBackend {
                opened: Arc::new(AtomicUsize::new(0)),
                change: "none",
            };
            let options = options();
            let mut output = Vec::new();
            let completed = execute(
                SliceSource::new(&source),
                SourceKind::Mp4,
                Job {
                    options: &options,
                    launcher: None,
                    deadline: Instant::now() + options.timeout,
                    video: VideoMode::Av1(runtime(&temporary, "none")),
                },
                &[&backend],
                &temporary.0.join("output.mp4"),
                &mut output,
            )
            .unwrap();
            assert_eq!(completed.video.frames, 4);
            assert_eq!(completed.video.last_duration, 3);
            let Encoding::Av1(encoding) = completed.video.encoding else {
                panic!("wrong output codec");
            };
            assert_eq!(encoding.cadence, FrameRate { num: 120, den: 7 });
            assert_eq!(encoding.version, "4.2.0");
            let source_times: Vec<_> = packet_facts(&source, 0)
                .iter()
                .map(|(time, duration, _)| (*time, *duration))
                .collect();
            let output_times: Vec<_> = packet_facts(&output, 0)
                .iter()
                .map(|(time, duration, _)| (*time, *duration))
                .collect();
            assert_eq!(output_times, source_times, "origin {origin}");
            assert_eq!(packet_facts(&output, 1), packet_facts(&source, 1));
            assert_eq!(backend.opened.load(Ordering::SeqCst), 1);
            let spool = temporary
                .0
                .join(format!(".output.mp4.coreav-{}.spool", std::process::id()));
            let deadline = Instant::now() + Duration::from_secs(2);
            while spool.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(!spool.exists(), "completed actors must release the spool");
        }
    }

    #[test]
    #[cfg(unix)]
    fn synthetic_pipeline_refusals_do_not_write_replacements() {
        for change in ["timestamp", "geometry", "colour", "missing", "truncated"] {
            let temporary = Temporary::new();
            let input = source(0, 1, false);
            let backend = SyntheticBackend {
                opened: Arc::new(AtomicUsize::new(0)),
                change,
            };
            let options = options();
            let mut output = Vec::new();
            assert!(
                execute(
                    SliceSource::new(&input),
                    SourceKind::Mp4,
                    Job {
                        options: &options,
                        launcher: None,
                        deadline: Instant::now() + options.timeout,
                        video: VideoMode::Av1(runtime(&temporary, change))
                    },
                    &[&backend],
                    &temporary.0.join("output.mp4"),
                    &mut output
                )
                .is_err(),
                "{change}"
            );
            assert!(output.is_empty(), "{change}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn known_hdr_refusal_precedes_decoder_opening() {
        let temporary = Temporary::new();
        let input = source(0, 16, false);
        let backend = SyntheticBackend {
            opened: Arc::new(AtomicUsize::new(0)),
            change: "none",
        };
        let options = options();
        let result = execute(
            SliceSource::new(&input),
            SourceKind::Mp4,
            Job {
                options: &options,
                launcher: None,
                deadline: Instant::now() + options.timeout,
                video: VideoMode::Av1(runtime(&temporary, "none")),
            },
            &[&backend],
            &temporary.0.join("output.mp4"),
            Vec::new(),
        );
        assert!(matches!(result, Err(Error::Unsupported(_))));
        assert_eq!(backend.opened.load(Ordering::SeqCst), 0);
    }

    #[test]
    #[cfg(unix)]
    fn native_layout_failure_is_not_an_unsupported_source() {
        let temporary = Temporary::new();
        let input = source(0, 1, false);
        let backend = SyntheticBackend {
            opened: Arc::new(AtomicUsize::new(0)),
            change: "none",
        };
        let options = options();
        let mut runtime = runtime(&temporary, "none");
        runtime.retention = |_| Err(BackendError::Output("invalid native allocation footprint"));
        let mut output = Vec::new();
        let result = execute(
            SliceSource::new(&input),
            SourceKind::Mp4,
            Job {
                options: &options,
                launcher: None,
                deadline: Instant::now() + options.timeout,
                video: VideoMode::Av1(runtime),
            },
            &[&backend],
            &temporary.0.join("output.mp4"),
            &mut output,
        );
        assert!(
            matches!(result, Err(Error::Encode(error)) if error.kind == ErrorKind::InvalidData)
        );
        assert!(output.is_empty());
    }

    #[test]
    fn source_colour_and_geometry_are_not_h264_normalizations() {
        let mut source = color();
        source.primaries = Some(22);
        assert_eq!(admit_color(source, source).unwrap().1.primaries(), 22);
        for transfer in [2, 16, 18] {
            let mut unsupported_color = color();
            unsupported_color.transfer = Some(transfer);
            assert!(admit_color(unsupported_color, unsupported_color).is_err());
        }
        let mut unknown_range = color();
        unknown_range.full_range = None;
        assert!(admit_color(unknown_range, unknown_range).is_err());
        for siting in [
            ChromaLocation::Center,
            ChromaLocation::Top,
            ChromaLocation::BottomLeft,
            ChromaLocation::Bottom,
        ] {
            let mut unsupported_color = color();
            unsupported_color.chroma_location = Some(siting);
            assert!(admit_color(unsupported_color, unsupported_color).is_err());
        }
        assert_eq!(
            exact_aspect(Some((4, 3)), (64, 64), (128, 64)).unwrap(),
            Some((2, 3))
        );
        assert!(exact_aspect(Some((u64::MAX, 1)), (64, 64), (64, 64)).is_err());
        assert!(exact_aspect(Some((0, 1)), (64, 64), (64, 64)).is_err());
    }

    #[test]
    fn parser_feature_cannot_advertise_native_availability() {
        let launcher = ExecLauncher::current_exe().unwrap();
        if !Av1Transcode::compiled() {
            assert!(matches!(
                Av1Transcode::prepare(Av1Options::default(), &launcher),
                Err(Error::Unsupported(_))
            ));
        }
        assert!(Av1Options::default().with_quality(14, 32).is_err());
        assert!(Av1Options::default().with_quality(8, 64).is_err());
        for scale in ["32:64", "65:64", "64:9000"] {
            assert!(check_scale(crate::parse_scale(scale)).is_err());
        }
        assert!(check_scale(crate::parse_scale("128:-2")).is_ok());
    }

    #[test]
    fn expired_prepared_job_does_not_access_the_source() {
        struct FailingSource {
            calls: usize,
        }
        impl Source for FailingSource {
            fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
                self.calls += 1;
                Err(coreav_isobmff::Error::Truncated { offset: 0 })
            }
            fn read_exact_at(
                &mut self,
                offset: u64,
                _: &mut [u8],
            ) -> Result<(), coreav_isobmff::Error> {
                self.calls += 1;
                Err(coreav_isobmff::Error::Truncated { offset })
            }
        }
        let temporary = Temporary::new();
        let launcher = ExecLauncher::current_exe().unwrap();
        let backend = SyntheticBackend {
            opened: Arc::new(AtomicUsize::new(0)),
            change: "none",
        };
        let job = Av1Transcode {
            options: options(),
            runtime: Runtime {
                program: temporary.0.join("absent-worker"),
                preset: 8,
                crf: 32,
                retention: native_plan,
                synthetic: false,
            },
            launcher: &launcher,
            deadline: Instant::now() - Duration::from_secs(1),
        };
        let mut source = FailingSource { calls: 0 };
        let mut output = Vec::new();
        let result = job.run(
            &mut source,
            SourceKind::Mp4,
            &[&backend],
            &temporary.0.join("output.mp4"),
            &mut output,
        );
        assert_eq!(source.calls, 0);
        assert!(matches!(result, Err(Error::Decode(error)) if error.kind == ErrorKind::Timeout));
        assert_eq!(backend.opened.load(Ordering::SeqCst), 0);
        assert!(output.is_empty());
        assert_eq!(std::fs::read_dir(&temporary.0).unwrap().count(), 0);
    }
}
