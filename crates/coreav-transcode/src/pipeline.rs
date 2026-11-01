//! The whole transcode (`docs/design/transcode.md`, "Pipeline"): the video
//! stream `coreav thumbnail` decodes, decoded through the decode boundary
//! with the caller's backends, its presented frames encoded by the encoder
//! program, and the source remuxed to MP4 with that stream's samples
//! replaced by the encoder's.
//!
//! One thread (the caller's) demuxes, sends packets, receives frames and
//! resizes them; it sends the next packet only after it has handed every
//! frame the decoder has ready to the encoder's queue, so packets in flight
//! stay near the decoder's reorder depth. The encoder's output is read on
//! its reader thread ([`crate::encoder`]) into the Annex B reader
//! ([`crate::annexb`]), whose pictures go to a spool file next to the
//! output in decode order; the spool file is removed on success and on
//! failure. Once the encoder has finished, the pictures' presentation
//! indices give the samples' times ([`crate::timing`]), and the MP4 writer
//! (`coreav_isobmff_write::remux_mp4_replacing`, `remux_matroska_replacing`)
//! writes the output with the source's audio copied.
//!
//! One deadline covers the whole transcode: the decoder's
//! `Limits::timeout` is set to what is left of it when the decoder opens,
//! the encoder is killed at it, and the main thread checks it between
//! packets.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fmt;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use coreav_decode::{Backend, Codec, DecoderInfo, ErrorKind, Packet, Policy, Skipped};
use coreav_frame::{ChromaSiting, ColorInfo, Filter, Frame, PixelFormat, Pixels};
use coreav_isobmff::{BufferedSource, FileSource, Rational, Source};
use coreav_isobmff_write::mux::{Nclx, avc1_sample_entry};
use coreav_isobmff_write::{
    AudioReplacement, ExtractError, MatroskaRemuxError, MatroskaRemuxOptions, MatroskaRemuxSummary,
    Metadata, OnUnsupported, RemuxOptions, RemuxSummary, Replacement, ReplacementSample,
    Replacements, remux_matroska_replacing, remux_mp4_replacing,
};
use coreav_thumbnail::{Container, ThumbnailOptions, VideoStream};
use coreav_worker::ExecLauncher;

use crate::aac::AacAudio;
use crate::annexb::{self, AnnexBReader};
use crate::audio::{AudioSummary, Encode, OpusAudio};
use crate::encoder::{
    Encoder, EncoderChoice, EncoderLimits, EncoderProgram, EncoderSettings, FrameRate, RateControl,
    Sink, Timing, find_encoder,
};
use crate::scale::Scale;
use crate::timing;

/// The largest spool file: 64 GiB (`docs/design/transcode.md`, "Limits").
pub const MAX_SPOOL_BYTES: u64 = 64 << 30;

/// The default `--timeout`: an hour of wall clock for the whole transcode.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3600);

/// Packets sent ahead of the frames received before the transcode waits
/// for the decoder (`docs/design/transcode.md`, "Pacing"), how long it
/// waits without any frame before it counts every packet sent as one that
/// gave none, and how often it looks.
const PACE_PACKETS: usize = 64;
const PACE_STALL: Duration = Duration::from_secs(5);
const PACE_POLL: Duration = Duration::from_millis(1);

/// The encoder's address space: frames of the output size that the budget
/// holds, beyond a fixed part, per encoder thread and for the lookahead
/// (`docs/design/transcode.md`, "Limits").
const ENCODER_FRAMES: u64 = 128;
const ENCODER_FRAMES_PER_THREAD: u64 = 16;
const ENCODER_FIXED_BYTES: u64 = 1 << 30;
/// The encoder's address space is at most this many times the decode
/// budget, and at most half the host's memory.
const ENCODER_BUDGET_TIMES: u64 = 4;

/// What happens to the source's audio.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AudioMode {
    /// Every stream MP4 can carry is stream-copied, as `coreav remux`
    /// copies it; the others are dropped and listed.
    #[default]
    Copy,
    /// Only the video stream is written.
    None,
    /// These source streams (indices among all of the source's streams)
    /// are stream-copied beside the video, as `coreav remux --streams`
    /// copies them: the streams an `ffmpeg` command line selects.
    Streams(Vec<usize>),
    /// One audio stream is encoded to Opus (`docs/design/transcode.md`,
    /// "Opus audio"); the others are dropped and listed.
    Opus(OpusAudio),
    /// One audio stream is encoded to AAC-LC by the `ffmpeg` program's
    /// `aac` encoder (`docs/design/transcode.md`, "AAC audio"); the
    /// others are dropped and listed.
    Aac(AacAudio),
}

/// The source's container, as the caller detected it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// MP4, MOV and other ISO base media files.
    Mp4,
    /// Matroska and WebM.
    Matroska,
}

/// What [`transcode`] does.
#[derive(Clone, Debug)]
pub struct TranscodeOptions {
    /// The encoder's rate control: CRF 23 by default.
    pub rate: RateControl,
    /// One of x264's presets ([`crate::encoder::PRESETS`]): `medium` by
    /// default.
    pub preset: String,
    /// The output size; `None` for the first frame's.
    pub scale: Option<Scale>,
    /// The pixel format the encoder must be given (`-pix_fmt` of
    /// `coreav ffmpeg`); frames go to the encoder unconverted, so a source
    /// decoded to another format is refused before the encoder starts.
    /// `None` for the decoded format.
    pub pixel_format: Option<PixelFormat>,
    /// Refuse a stream whose frame size changes instead of resizing its
    /// frames to the first one's (`coreav ffmpeg`: FFmpeg resizes them with
    /// its own scaler).
    pub one_size: bool,
    /// The source's audio.
    pub audio: AudioMode,
    /// The cover pictures of an MP4 source to leave out, as
    /// `RemuxOptions::left_out_covers` names them; the others are copied
    /// with the movie's metadata.
    pub left_out_covers: Vec<u64>,
    /// Which encoder program.
    pub encoder: EncoderChoice,
    /// The `x264` program (`--x264`); else `COREAV_X264`, else `PATH`.
    pub x264: Option<PathBuf>,
    /// The `ffmpeg` program (`--ffmpeg`); else `COREAV_FFMPEG`, else
    /// `PATH`: the video encoder with [`EncoderChoice::Ffmpeg`], and the
    /// AAC encoder with [`AudioMode::Aac`].
    pub ffmpeg: Option<PathBuf>,
    /// Encoder threads; `None` for the encoder's own choice.
    pub threads: Option<u32>,
    /// Wall clock for the whole transcode.
    pub timeout: Duration,
    /// Which codecs may be decoded, by which backends.
    pub policy: Policy,
    /// Limits of the MP4/MOV parser and the MP4 writer.
    pub isobmff_limits: coreav_isobmff::Limits,
    /// Limits of the Matroska parser.
    pub matroska_limits: coreav_mkv::Limits,
    /// Decode limits; `timeout` is replaced by what is left of
    /// [`TranscodeOptions::timeout`], and `max_memory_bytes` also sets the
    /// encoder's address-space budget ([`encoder_memory`]).
    /// `max_decoded_bytes` is raised to the stream's packets times a frame
    /// of its declared size where that is larger ([`decoded_bytes_cap`]).
    pub decode_limits: coreav_decode::Limits,
    /// The largest spool file.
    pub max_spool_bytes: u64,
}

impl Default for TranscodeOptions {
    fn default() -> Self {
        Self {
            rate: RateControl::Crf(23.0),
            preset: "medium".into(),
            scale: None,
            pixel_format: None,
            one_size: false,
            audio: AudioMode::Copy,
            left_out_covers: Vec::new(),
            encoder: EncoderChoice::Auto,
            x264: None,
            ffmpeg: None,
            threads: None,
            timeout: DEFAULT_TIMEOUT,
            policy: Policy::default(),
            isobmff_limits: coreav_isobmff::Limits::default(),
            matroska_limits: coreav_mkv::Limits::default(),
            decode_limits: coreav_decode::Limits::default(),
            max_spool_bytes: MAX_SPOOL_BYTES,
        }
    }
}

/// The encoded video stream.
#[derive(Clone, Debug)]
pub struct VideoSummary {
    /// The source stream's position among all of its streams; the output
    /// keeps its place.
    pub stream_index: usize,
    /// The source stream's codec.
    pub codec: Codec,
    /// Frames encoded: every frame the source presents, less the dropped.
    pub frames: u64,
    /// Frames dropped because their timestamp was below the one before
    /// (or they had none), or repeated it with no tick left before the
    /// next timestamp; other repeats are kept, moved one tick after the
    /// frame before (`docs/design/transcode.md`, "Which frames").
    pub dropped_frames: u64,
    /// Frames decoded that the source does not present (an edit list's
    /// leading frames), not encoded.
    pub not_presented: u64,
    /// The decoder that ran.
    pub decoder: DecoderInfo,
    /// Backends tried before it.
    pub skipped: Vec<Skipped>,
    /// The encoder program: `x264` or `ffmpeg`.
    pub encoder: &'static str,
    /// The output's width and height.
    pub width: u32,
    /// The output's height.
    pub height: u32,
    /// The output's sample aspect ratio, when it is not 1:1.
    pub sample_aspect: Option<(u32, u32)>,
    /// The presented frames' average rate, snapped to a common rate, for
    /// information: the encoder is given every frame's own time.
    pub frame_rate: FrameRate,
}

/// What the MP4 writer wrote.
#[derive(Clone, Debug)]
pub enum Remuxed {
    /// From an MP4 or MOV source.
    Mp4(RemuxSummary),
    /// From a Matroska or WebM source.
    Matroska(MatroskaRemuxSummary),
}

/// What [`transcode`] wrote.
#[derive(Clone, Debug)]
pub struct Summary {
    /// The video stream.
    pub video: VideoSummary,
    /// The audio stream encoded, with [`AudioMode::Opus`] or
    /// [`AudioMode::Aac`].
    pub audio: Option<AudioSummary>,
    /// The streams written and dropped.
    pub remux: Remuxed,
}

/// Why a transcode failed. Nothing is left behind: the spool file is
/// removed, and the caller discards what was written to its output.
#[derive(Debug)]
pub enum Error {
    /// The MP4/MOV parser rejected the source or could not read it.
    Isobmff(coreav_isobmff::Error),
    /// The Matroska parser rejected the source or could not read it.
    Matroska(coreav_mkv::Error),
    /// The video stream is not valid for its codec's specification: the
    /// rule broken (`coreav_thumbnail::Error::Invalid`).
    Invalid(&'static str),
    /// Decoding failed, no backend could decode the stream, or the
    /// deadline passed while decoding.
    Decode(coreav_decode::Error),
    /// The encoder program failed, was killed at the deadline, or wrote a
    /// stream that is not one picture for each frame sent.
    Encode(coreav_decode::Error),
    /// The source cannot be transcoded as asked, by design: a refusal of
    /// the input side only (a pixel format, an edit list, a size the
    /// encoder does not take, no encoder program), found before the
    /// encoder starts. Failures of the encoder, the Annex B reader and the
    /// MP4 writer are never this (`docs/design/transcode.md`).
    Unsupported(String),
    /// The MP4 writer failed on an MP4/MOV source (after the encode: a
    /// failure of the writer, whatever error it gives).
    Remux(ExtractError),
    /// The MP4 writer failed on a Matroska/WebM source.
    MatroskaRemux(MatroskaRemuxError),
    /// The spool file could not be created, written or read.
    Spool(std::io::ErrorKind),
    /// The audio stream to encode could not be decoded, or the source
    /// could not be read for it (`coreav-audio`'s error).
    Audio(coreav_audio::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Isobmff(e) => write!(f, "{e}"),
            Self::Matroska(e) => write!(f, "{e}"),
            Self::Invalid(what) => write!(f, "invalid structure: {what}"),
            Self::Decode(e) => write!(f, "decoding: {}", e.message),
            Self::Encode(e) => write!(f, "encoding: {}", e.message),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::Remux(e) => write!(f, "{e}"),
            Self::MatroskaRemux(e) => write!(f, "{e}"),
            Self::Spool(kind) => write!(f, "spool file: {kind}"),
            Self::Audio(e) => write!(f, "the audio stream: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<coreav_thumbnail::Error> for Error {
    fn from(e: coreav_thumbnail::Error) -> Self {
        use coreav_thumbnail::Error as T;
        match e {
            T::Isobmff(e) => Self::Isobmff(e),
            T::Matroska(e) => Self::Matroska(e),
            T::Invalid(what) => Self::Invalid(what),
            T::Decode(e) => Self::Decode(e),
            T::Frame(e) => Self::Unsupported(e.to_string()),
            T::NoVideo => Self::Unsupported("no video stream".into()),
            T::TimeOutOfRange => Self::Unsupported("no presented frame".into()),
            T::Unsupported(what) | T::Encode(what) => Self::Unsupported(what),
        }
    }
}

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported(what.into())
}

fn timed_out() -> Error {
    Error::Decode(coreav_decode::Error::new(
        ErrorKind::Timeout,
        "the transcode took longer than its timeout",
    ))
}

/// Transcodes `source` (`docs/design/transcode.md`): its video stream to
/// H.264 by the encoder program, written with the source's other streams
/// as MP4 to `out`.
///
/// `kind` is the source's container. `backends` decode the video (the
/// policy's order chooses among them); `launcher` runs the encoder under
/// the sandbox's limits (`None`: run directly, for tests). The spool file
/// is created next to `output`, the path the caller writes `out` to (or
/// will rename it to), and removed before this returns.
pub fn transcode<S: Source, W: Write>(
    mut source: S,
    kind: SourceKind,
    options: &TranscodeOptions,
    backends: &[&dyn Backend],
    launcher: Option<&ExecLauncher>,
    output: &Path,
    out: W,
) -> Result<Summary, Error> {
    let deadline = Instant::now()
        .checked_add(options.timeout)
        .ok_or_else(|| unsupported("timeout out of range"))?;
    let mut demux = ThumbnailOptions::at(Rational::from_int(0));
    demux.isobmff_limits = options.isobmff_limits;
    demux.matroska_limits = options.matroska_limits;
    demux.policy = options.policy.clone();
    let video = coreav_thumbnail::video_stream(&mut source, &demux)?;
    match (kind, video.container) {
        (SourceKind::Mp4, Container::Mp4) | (SourceKind::Matroska, Container::Matroska) => {}
        _ => return Err(unsupported("the source is not of the container named")),
    }
    if let Some(warning) = video.warnings.first() {
        // The packets' times would not be the sample tables' the writer
        // keeps.
        return Err(unsupported(format!("the video stream has {warning}")));
    }
    let presented = Presented::of(&video)?;
    let frame_rate = presented.frame_rate(video.time_base);
    let program = find_encoder(options.encoder, options.x264.clone(), options.ffmpeg.clone())
        .ok_or_else(|| {
            unsupported(match options.encoder {
                EncoderChoice::X264 => "no x264 program: install one, or name it with --x264",
                EncoderChoice::Ffmpeg => {
                    "no ffmpeg program: install one with libx264, or name it with --ffmpeg"
                }
                EncoderChoice::Auto => {
                    "no encoder program: install x264 or ffmpeg with libx264, or name one with --x264 or --ffmpeg"
                }
            })
        })?;
    // Before the video: a stream that cannot be encoded is refused before
    // the video encoder starts.
    let encode = match &options.audio {
        AudioMode::Opus(opus) => Some(Encode::Opus(opus)),
        AudioMode::Aac(aac) => {
            let program = find_encoder(EncoderChoice::Ffmpeg, None, options.ffmpeg.clone())
                .ok_or_else(|| {
                    unsupported(
                        "no ffmpeg program for AAC audio: install FFmpeg, whose built-in aac \
                         encoder encodes it, or name it with --ffmpeg",
                    )
                })?;
            Some(Encode::Aac(aac, program.path().to_path_buf()))
        }
        _ => None,
    };
    let audio = match encode {
        Some(encode) => Some(crate::audio::encode(
            &mut source,
            &encode,
            &coreav_audio::Limits {
                isobmff: options.isobmff_limits,
                matroska: options.matroska_limits,
                ..coreav_audio::Limits::default()
            },
            launcher,
            deadline,
            output,
            options.max_spool_bytes,
        )?),
        None => None,
    };
    let mut limits = options.decode_limits.clone();
    limits.timeout = deadline.saturating_duration_since(Instant::now());
    limits.max_decoded_bytes = decoded_bytes_cap(
        &limits,
        video.packets.len(),
        video
            .stream
            .width
            .zip(video.stream.height)
            .filter(|&(w, h)| w > 0 && h > 0),
    );
    let opened = coreav_decode::open(&video.stream, &options.policy, &limits, backends)
        .map_err(Error::Decode)?;
    let mut decoder = opened.decoder;
    let spool = Spool::create(output, "spool")?;
    // x264 reads the frames' times from a file, written before it starts:
    // the presented frames' times, which are the frames sent unless the
    // decoder leaves one out (docs/design/transcode.md).
    let timecodes = match &program {
        EncoderProgram::X264(_) => {
            let file = Spool::create(output, "timecodes")?;
            let (num, den) = time_base_terms(video.time_base)?;
            let mut handle = file.handle()?;
            crate::encoder::write_timecodes(&mut handle, &presented.times, num, den).map_err(
                |e| match e.kind {
                    ErrorKind::Io => Error::Spool(std::io::ErrorKind::Other),
                    _ => Error::Encode(e),
                },
            )?;
            Some(file)
        }
        EncoderProgram::Ffmpeg(_) => None,
    };
    let mut run = Run {
        timecodes: timecodes.as_ref().map(|t| t.path.clone()),
        options,
        video: &video,
        presented: &presented,
        frame_rate,
        program: &program,
        launcher,
        deadline,
        spool: &spool,
        encoder: None,
        first_size: None,
        sent: Arc::new(AtomicU64::new(0)),
        sent_pts: Vec::new(),
        last: None,
        decode_order: times_in_decode_order(&video),
        pending: BTreeMap::new(),
        dropped: 0,
        not_presented: 0,
    };
    // The packets sent that wait for a frame, oldest first, and their bytes.
    let mut pace = Pace {
        sizes: VecDeque::new(),
        bytes: 0,
        cap: limits.max_pending_bytes / 4,
        stall: PACE_STALL,
    };
    for (index, packet) in video.packets.iter().enumerate() {
        if Instant::now() >= deadline {
            return Err(timed_out());
        }
        // Past the cap, wait for the decoder before sending more: a decoder
        // slower than the reads (a long ProRes file through the fallback)
        // would otherwise queue the stream until the decoder's packet limit
        // fails it (`docs/design/transcode.md`, "Pacing").
        if pace.full() {
            pace.wait(
                deadline,
                || decoder.receive_frame().map_err(Error::Decode),
                |frame| {
                    let counts = presented_frame(&run, &frame);
                    run.take(frame)?;
                    Ok(counts)
                },
            )?;
        }
        let data = video.read_packet(&mut source, index, limits.max_packet_bytes)?;
        decoder
            .send_packet(&Packet {
                data: &data,
                pts: Some(packet.pts),
                dts: packet.dts,
                keyframe: packet.keyframe,
                // Decoded as a reference; its frame is not wanted.
                discard: !packet.presented,
            })
            .map_err(Error::Decode)?;
        if run.decode_order && packet.presented {
            let count = run.pending.entry(packet.pts).or_insert(0);
            *count = count.saturating_add(1);
        }
        if packet.presented {
            pace.sent(data.len());
        }
        // Every frame ready goes to the encoder before the next packet.
        while let Some(frame) = decoder.receive_frame().map_err(Error::Decode)? {
            if presented_frame(&run, &frame) {
                pace.received();
            }
            run.take(frame)?;
        }
    }
    decoder.send_eof().map_err(Error::Decode)?;
    while let Some(frame) = decoder.receive_frame().map_err(Error::Decode)? {
        run.take(frame)?;
    }
    let decoder_info = decoder.info().clone();
    drop(decoder);
    let Some((encoder, settings)) = run.encoder.take() else {
        return Err(unsupported("the decoder returned no presented frame"));
    };
    let frames = encoder.frames_sent();
    let sink = encoder.finish().map_err(Error::Encode)?;
    let encoded = sink.close(frames)?;
    let config = encoded.config;
    if config.width != settings.width || config.height != settings.height {
        return Err(encode_error(format!(
            "the encoder's stream is {}x{}, not the {}x{} of its frames",
            config.width, config.height, settings.width, settings.height
        )));
    }
    let order: Vec<u64> = encoded.pictures.iter().map(|p| p.presentation).collect();
    let last_duration = presented.last_duration(&run.sent_pts, run.last.map(|(source, _)| source));
    let times = timing::sample_times(&run.sent_pts, &order, u64::from(last_duration))
        .map_err(|e| encode_error(e.to_string()))?;
    let samples: Vec<ReplacementSample> = encoded
        .pictures
        .iter()
        .zip(&times)
        .map(|(p, t)| ReplacementSample {
            size: p.size,
            sync: p.idr,
            decode_time: t.dts,
            presentation_time: t.pts,
        })
        .collect();
    let entry = sample_entry(&config, &settings)?;
    let (tkhd_width, tkhd_height) = presentation_size(&settings)?;
    if Instant::now() >= deadline {
        return Err(timed_out());
    }
    // Read back through the handle that created the file, never by path.
    let data = BufferedSource::new(FileSource::from_file(spool.handle()?).map_err(spool_read)?);
    let replacement = Replacement {
        stream_index: video.stream_index,
        sample_entry: &entry,
        width: tkhd_width,
        height: tkhd_height,
        samples: &samples,
        last_duration,
        data,
    };
    let audio_replacement = match &audio {
        Some(encoded) => Some(AudioReplacement {
            stream_index: encoded.summary.stream_index,
            sample_entry: &encoded.entry,
            timescale: encoded.timescale,
            samples: &encoded.samples,
            start: 0,
            priming: u32::from(encoded.summary.pre_skip),
            presented: encoded.summary.samples,
            roll: encoded.roll,
            data: BufferedSource::new(
                FileSource::from_file(encoded.spool.handle()?).map_err(spool_read)?,
            ),
        }),
        None => None,
    };
    let replacements = Replacements {
        video: Some(replacement),
        audio: audio_replacement,
    };
    let streams = match &options.audio {
        AudioMode::Copy => None,
        AudioMode::None => Some(vec![video.stream_index]),
        AudioMode::Opus(_) | AudioMode::Aac(_) => Some(
            std::iter::once(video.stream_index)
                .chain(audio.as_ref().map(|a| a.summary.stream_index))
                .collect(),
        ),
        AudioMode::Streams(audio) => Some(
            std::iter::once(video.stream_index)
                .chain(audio.iter().copied().filter(|&s| s != video.stream_index))
                .collect(),
        ),
    };
    let remux = match kind {
        SourceKind::Mp4 => Remuxed::Mp4(
            remux_mp4_replacing(
                &mut source,
                &options.isobmff_limits,
                &isobmff_allowlist(video.stream.codec, audio.as_ref().map(|a| a.summary.codec)),
                &RemuxOptions {
                    streams,
                    on_unsupported: OnUnsupported::Drop,
                    metadata: Metadata::Copy,
                    left_out_covers: options.left_out_covers.clone(),
                },
                replacements,
                out,
            )
            .map_err(Error::Remux)?,
        ),
        SourceKind::Matroska => Remuxed::Matroska(
            remux_matroska_replacing(
                &mut source,
                &options.matroska_limits,
                &options.isobmff_limits,
                &matroska_allowlist(video.stream.codec, audio.as_ref().map(|a| a.summary.codec)),
                &MatroskaRemuxOptions {
                    streams,
                    on_unsupported: OnUnsupported::Drop,
                    // Another video stream that MP4 cannot carry is dropped
                    // and listed: the replaced one is the output's video.
                    drop_video: true,
                },
                replacements,
                out,
            )
            .map_err(Error::MatroskaRemux)?,
        ),
    };

    Ok(Summary {
        video: VideoSummary {
            stream_index: video.stream_index,
            codec: video.stream.codec,
            frames,
            dropped_frames: run.dropped,
            not_presented: run.not_presented,
            decoder: decoder_info,
            skipped: opened.skipped,
            encoder: program.name(),
            width: settings.width,
            height: settings.height,
            sample_aspect: settings.sample_aspect,
            frame_rate: settings.frame_rate,
        },
        audio: audio.map(|a| a.summary),
        remux,
    })
}

fn encode_error(message: String) -> Error {
    Error::Encode(coreav_decode::Error::new(ErrorKind::Backend, message))
}

pub(crate) fn spool_read(error: coreav_isobmff::Error) -> Error {
    match error {
        coreav_isobmff::Error::Io(kind) => Error::Spool(kind),
        // The spool file is CoreAV's own: never an input's error.
        other => encode_error(format!("the spool file: {other}")),
    }
}

/// The ISOBMFF allowlist of the writer: the default, VP8 when it is the
/// video stream's codec, and the codec of the audio stream encoded (FLAC),
/// which the default leaves out, so that the replaced streams are kept.
fn isobmff_allowlist(
    codec: Codec,
    audio: Option<coreav_audio::Codec>,
) -> coreav_isobmff::Allowlist {
    use coreav_isobmff::Codec as I;
    let mut codecs = coreav_isobmff::Allowlist::default().codecs().to_vec();
    let video = (codec == Codec::Vp8).then_some(I::Vp8);
    let audio = audio.map(|codec| match codec {
        coreav_audio::Codec::Flac => I::Flac,
        coreav_audio::Codec::Mp3 => I::Mp3,
        coreav_audio::Codec::Opus => I::Opus,
        #[cfg(feature = "aac")]
        coreav_audio::Codec::Aac => I::Aac,
    });
    for own in video.into_iter().chain(audio) {
        if !codecs.contains(&own) {
            codecs.push(own);
        }
    }
    coreav_isobmff::Allowlist::new(&codecs)
}

/// The Matroska allowlist of the writer: the default, the video stream's
/// codec (a long-tail codec the decode policy allows, which the default
/// leaves out) and the codec of the audio stream encoded (FLAC),
/// so that the replaced streams are kept.
fn matroska_allowlist(codec: Codec, audio: Option<coreav_audio::Codec>) -> coreav_mkv::Allowlist {
    use coreav_mkv::Codec as M;
    let mut codecs = coreav_mkv::Allowlist::default().codecs().to_vec();
    let own = match codec {
        Codec::H264 => M::H264,
        Codec::Hevc => M::Hevc,
        Codec::Av1 => M::Av1,
        Codec::Vp9 => M::Vp9,
        Codec::Vp8 => M::Vp8,
        Codec::ProRes => M::ProRes,
        Codec::Mpeg4 => M::Mpeg4,
        Codec::Mpeg1Video => M::Mpeg1Video,
        Codec::Mpeg2Video => M::Mpeg2Video,
        Codec::Mjpeg => M::Mjpeg,
        Codec::Theora => M::Theora,
        Codec::Ffv1 => M::Ffv1,
    };
    let audio = audio.map(|codec| match codec {
        coreav_audio::Codec::Flac => M::Flac,
        coreav_audio::Codec::Mp3 => M::Mp3,
        coreav_audio::Codec::Opus => M::Opus,
        #[cfg(feature = "aac")]
        coreav_audio::Codec::Aac => M::Aac,
    });
    for own in std::iter::once(own).chain(audio) {
        if !codecs.contains(&own) {
            codecs.push(own);
        }
    }
    coreav_mkv::Allowlist::new(&codecs)
}

/// The frames the source presents: their timestamps, and the durations
/// of their packets.
struct Presented {
    /// Presentation timestamps of the presented packets.
    pts: HashSet<i64>,
    /// The presented timestamps, sorted, without repeats.
    distinct: Vec<i64>,
    /// The times the frames are sent at when the decoder returns every
    /// presented frame: the presented timestamps in increasing order, each
    /// repeat moved or dropped by [`frame_time`].
    times: Vec<i64>,
    /// The last presented timestamp, before any move.
    last: i64,
    /// Per presented timestamp: its packet's duration, when known.
    durations: std::collections::HashMap<i64, u64>,
}

/// The time a presented frame at source timestamp `pts` is sent at, after
/// a frame at source timestamp `source` sent at `sent` (`last`), or `None`
/// when it is dropped (`docs/design/transcode.md`, "Which frames"): its own
/// time when that is above the last frame's; when its timestamp repeats
/// the last one's (Matroska's millisecond times can give several frames
/// one time), one tick after the last frame sent if that is below `limit`
/// (the source's next time, `None` after the last one), else dropped, so a
/// repeat never moves onto or past a later frame's own time; dropped when
/// its timestamp is below the last one's.
fn frame_time(last: Option<(i64, i64)>, pts: i64, limit: Option<i64>) -> Option<i64> {
    match last {
        None => Some(pts),
        Some((source, _)) if pts < source => None,
        Some((_, sent)) if pts > sent => Some(pts),
        Some((_, sent)) => sent
            .checked_add(1)
            .filter(|time| limit.is_none_or(|limit| *time < limit)),
    }
}

impl Presented {
    fn of(video: &VideoStream) -> Result<Self, Error> {
        let mut pts = HashSet::new();
        let mut durations = std::collections::HashMap::new();
        for p in video.packets.iter().filter(|p| p.presented) {
            pts.insert(p.pts);
            if let Some(d) = p.duration.filter(|d| *d > 0) {
                durations.insert(p.pts, d);
            }
        }
        if pts.is_empty() {
            return Err(unsupported("the source presents no video frame"));
        }
        let mut distinct: Vec<i64> = pts.iter().copied().collect();
        distinct.sort_unstable();
        let mut sorted: Vec<i64> = video
            .packets
            .iter()
            .filter(|p| p.presented)
            .map(|p| p.pts)
            .collect();
        sorted.sort_unstable();
        let mut presented = Self {
            pts,
            distinct,
            times: Vec::with_capacity(sorted.len()),
            last: 0,
            durations,
        };
        let mut last = None;
        for pts in sorted {
            if let Some(time) = frame_time(last, pts, presented.limit(pts)) {
                presented.times.push(time);
                last = Some((pts, time));
            }
        }
        let Some((last, _)) = last else {
            return Err(unsupported("the source presents no video frame"));
        };
        presented.last = last;
        Ok(presented)
    }

    /// The time a repeat of timestamp `pts` must stay below: the next
    /// presented timestamp; `None` after the last one, where no later frame
    /// can be passed (a Matroska block's duration is mostly unstated).
    fn limit(&self, pts: i64) -> Option<i64> {
        let next = self.distinct.partition_point(|t| *t <= pts);
        self.distinct.get(next).copied()
    }

    /// The presented frames' average rate (frames over the presented
    /// span, the last frame's duration included), snapped to an integer or
    /// `N × 1000/1001` rate within 0.1% of one: for the summary, and for
    /// FFmpeg when it gets one frame. The encoder is otherwise given every
    /// frame's own time, not a rate (`docs/design/transcode.md`, "Rate
    /// control and timestamps").
    fn frame_rate(&self, time_base: Rational) -> FrameRate {
        const FALLBACK: FrameRate = FrameRate { num: 25, den: 1 };
        let n = self.times.len() as u128;
        let (first, last) = match (self.times.first(), self.times.last()) {
            (Some(f), Some(l)) => (*f, *l),
            _ => return FALLBACK,
        };
        let span = u128::try_from(i128::from(last) - i128::from(first)).unwrap_or(0);
        let last_duration = self
            .durations
            .get(&self.last)
            .map(|d| u128::from(*d))
            .or_else(|| {
                // The mean interval, when the last frame's duration is unknown.
                (n > 1).then(|| span / (n - 1))
            });
        let ticks = span.saturating_add(last_duration.unwrap_or(0));
        // Seconds = ticks × num / den, so the rate is n × den / (ticks × num).
        let (Ok(tb_num), tb_den) = (u128::try_from(time_base.num), u128::from(time_base.den))
        else {
            return FALLBACK;
        };
        let (num, den) = (n.saturating_mul(tb_den), ticks.saturating_mul(tb_num));
        if num == 0 || den == 0 {
            return FALLBACK;
        }
        snap(num, den)
    }

    /// The duration of the last frame sent, in the stream's units: its
    /// packet's (the packet at `source`, its timestamp before any move),
    /// else the interval before it, else 1.
    fn last_duration(&self, sent: &[i64], source: Option<i64>) -> u32 {
        let own = source.and_then(|t| self.durations.get(&t)).copied();
        let before = match sent {
            [.., a, b] => u64::try_from(i128::from(*b) - i128::from(*a)).ok(),
            _ => None,
        };
        own.or(before)
            .and_then(|d| u32::try_from(d).ok())
            .filter(|d| *d > 0)
            .unwrap_or(1)
    }

    fn contains(&self, pts: i64) -> bool {
        self.pts.contains(&pts)
    }
}

/// Whether the stream's times are its decode times: an MP4 track of a
/// codec whose pictures can be decoded out of presentation order (H.264,
/// HEVC, MPEG-1, -2 and -4 video) and whose samples have no composition
/// offsets (no `ctts`, or only zeros), so every packet's presentation time
/// equals its decode time. A decoder that reorders pictures (B-frames)
/// then returns them with times out of order, as a muxer that had no presentation times writes them (`ffmpeg
/// -i IN.h264 -c copy OUT.mp4`). The frames are then given the presented
/// times in increasing order, the order the decoder returns them in, as
/// FFmpeg's decoder gives them its packets' decode times: dropping each
/// frame whose time is not above the last one's would drop every
/// reordered frame (`docs/design/transcode.md`, "Times in decode order").
fn times_in_decode_order(video: &VideoStream) -> bool {
    let reorders = matches!(
        video.stream.codec,
        Codec::H264 | Codec::Hevc | Codec::Mpeg4 | Codec::Mpeg1Video | Codec::Mpeg2Video
    );
    reorders
        && video.container == Container::Mp4
        && video.packets.iter().all(|p| p.dts == Some(p.pts))
}

/// `num / den` frames a second as the encoder's rate: the nearer of the
/// nearest integer rate and the nearest `N × 1000/1001` rate when it is
/// within 0.1%, else the rate itself (to a thousandth of a frame a second
/// when its terms do not fit 32 bits).
fn snap(num: u128, den: u128) -> FrameRate {
    let rate = num as f64 / den as f64;
    let integer = rate.round();
    let ntsc = (rate * 1.001).round();
    let error = |candidate: f64| (rate - candidate).abs() / candidate;
    let mut best: Option<(f64, FrameRate)> = None;
    if integer >= 1.0 && integer <= f64::from(u32::MAX) {
        best = Some((
            error(integer),
            FrameRate {
                num: integer as u32,
                den: 1,
            },
        ));
    }
    if ntsc >= 1.0 && ntsc * 1000.0 <= f64::from(u32::MAX) {
        let e = error(ntsc * 1000.0 / 1001.0);
        if best.is_none_or(|(b, _)| e < b) {
            best = Some((
                e,
                FrameRate {
                    num: (ntsc as u32) * 1000,
                    den: 1001,
                },
            ));
        }
    }
    if let Some((e, snapped)) = best {
        if e <= 0.001 {
            return snapped;
        }
    }
    let g = gcd(num, den);
    let (n, d) = (num / g.max(1), den / g.max(1));
    if let (Ok(n), Ok(d)) = (u32::try_from(n), u32::try_from(d)) {
        return FrameRate { num: n, den: d };
    }
    let thousandths = (rate * 1000.0).round();
    if thousandths >= 1.0 && thousandths <= f64::from(u32::MAX) {
        FrameRate {
            num: thousandths as u32,
            den: 1000,
        }
    } else {
        FrameRate { num: 25, den: 1 }
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The presented packets sent whose frames have not come yet, as far as
/// the transcode can tell: frames come in presentation order, so each
/// frame received retires the oldest packet.
struct Pace {
    sizes: VecDeque<u64>,
    bytes: u64,
    /// Bytes ahead of the decoder past which the transcode waits.
    cap: u64,
    /// How long without any frame before every packet counted is retired.
    stall: Duration,
}

impl Pace {
    fn sent(&mut self, len: usize) {
        let len = u64::try_from(len).unwrap_or(u64::MAX);
        self.sizes.push_back(len);
        self.bytes = self.bytes.saturating_add(len);
    }

    fn received(&mut self) {
        if let Some(len) = self.sizes.pop_front() {
            self.bytes = self.bytes.saturating_sub(len);
        }
    }

    fn full(&self) -> bool {
        self.sizes.len() > PACE_PACKETS && self.bytes > self.cap
    }

    /// Waits until the packets ahead of the decoder are under the caps:
    /// `poll` gives the next frame ready, if any, and `take` takes it and
    /// says whether it retires a packet (a presented frame). After
    /// [`PACE_STALL`] without any frame, nothing is in flight that will
    /// come (the decoder dropped those frames, or holds them until it has
    /// more), so every packet counted is retired.
    fn wait<T>(
        &mut self,
        deadline: Instant,
        mut poll: impl FnMut() -> Result<Option<T>, Error>,
        mut take: impl FnMut(T) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        let mut last = Instant::now();
        while self.full() {
            if Instant::now() >= deadline {
                return Err(timed_out());
            }
            if let Some(frame) = poll()? {
                if take(frame)? {
                    self.received();
                }
                last = Instant::now();
            } else if last.elapsed() >= self.stall {
                self.sizes.clear();
                self.bytes = 0;
            } else {
                std::thread::sleep(PACE_POLL);
            }
        }
        Ok(())
    }
}

/// Whether the decoder's `frame` is one of a presented packet, which
/// retires a packet of [`Pace`].
fn presented_frame(run: &Run<'_>, frame: &Frame) -> bool {
    frame.pts.is_some_and(|pts| run.presented.contains(pts))
}

/// Bytes of a frame of the declared size in the widest format a decoder
/// outputs (4:4:4 at two bytes a sample), for the decoded-bytes cap.
const CAP_BYTES_PER_PIXEL: u64 = 6;

/// The decoded-bytes cap of a transcode (the limits policy: decoded bytes
/// per request are bounded): the configured one, or, where larger, the
/// stream's packets times a frame of its declared size at
/// [`CAP_BYTES_PER_PIXEL`] (each packet decodes to at most one frame; four
/// times an 8-bit 4:2:0 frame leaves room for a stream that grows within
/// it), or of `max_frame_bytes` where the size is not declared. The
/// configured default (4 GiB) suits one thumbnail, and is about 55 s of
/// 1080p.
fn decoded_bytes_cap(
    limits: &coreav_decode::Limits,
    packets: usize,
    size: Option<(u32, u32)>,
) -> u64 {
    let packets = u64::try_from(packets).unwrap_or(u64::MAX);
    let frame = size.map_or(limits.max_frame_bytes, |(w, h)| {
        u64::from(w)
            .saturating_mul(u64::from(h))
            .saturating_mul(CAP_BYTES_PER_PIXEL)
            .min(limits.max_frame_bytes)
    });
    limits.max_decoded_bytes.max(packets.saturating_mul(frame))
}

/// The memory this process may use, in bytes: the host's (`MemTotal` of
/// `/proc/meminfo`), or a container's limit where it is lower (cgroup v2
/// `memory.max`, v1 `memory.limit_in_bytes`); `None` where the system says
/// neither.
fn host_memory() -> Option<u64> {
    let total = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
            let kib: u64 = line
                .trim_start_matches("MemTotal:")
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .ok()?;
            kib.checked_mul(1024)
        });
    let cgroup = [
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory/memory.limit_in_bytes",
    ]
    .iter()
    .filter_map(|path| {
        std::fs::read_to_string(path)
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
    })
    .min();
    match (total, cgroup) {
        (Some(t), Some(c)) => Some(t.min(c)),
        (t, c) => t.or(c),
    }
}

/// The encoder's address-space budget for `settings`: the decode budget,
/// or more for large frames, which x264 holds many of (one per frame
/// thread and lookahead position, and its references; 8K at `medium` on 8
/// CPUs takes about 8 GB), up to [`ENCODER_BUDGET_TIMES`] times the decode
/// budget and half of `host` (the host's memory; twice the decode budget
/// where it is unknown), so a hostile stream of large frames cannot take
/// the host's memory.
fn encoder_memory(settings: &EncoderSettings, budget: u64, host: Option<u64>) -> u64 {
    let format = settings.format;
    // A plane without a size (none of the formats the encoder takes) counts
    // as large as the cap allows.
    let frame: u64 = (0..format.plane_count())
        .map(|plane| {
            format
                .plane_size(plane, settings.width, settings.height)
                .zip(format.row_bytes(plane, settings.width, settings.height))
                .and_then(|((_, rows), row)| u64::try_from(row).ok()?.checked_mul(u64::from(rows)))
                .unwrap_or(u64::MAX)
        })
        .fold(0, u64::saturating_add);
    // x264's default: 1.5 threads a CPU.
    let threads = settings.threads.map_or_else(
        || {
            let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
            u64::try_from(cpus).unwrap_or(1).saturating_mul(3) / 2
        },
        u64::from,
    );
    let frames =
        ENCODER_FRAMES.saturating_add(ENCODER_FRAMES_PER_THREAD.saturating_mul(threads.max(1)));
    let wanted = frame
        .saturating_mul(frames)
        .saturating_add(ENCODER_FIXED_BYTES);
    let cap = budget
        .saturating_mul(ENCODER_BUDGET_TIMES)
        .min(host.map_or(budget.saturating_mul(2), |h| h / 2));
    wanted.min(cap).max(budget)
}

/// The transcode's state while frames come.
struct Run<'a> {
    options: &'a TranscodeOptions,
    video: &'a VideoStream,
    presented: &'a Presented,
    frame_rate: FrameRate,
    program: &'a EncoderProgram,
    launcher: Option<&'a ExecLauncher>,
    deadline: Instant,
    spool: &'a Spool,
    /// x264's timecode file.
    timecodes: Option<PathBuf>,
    /// The encoder, once the first presented frame started it.
    encoder: Option<(Encoder<SpoolSink>, EncoderSettings)>,
    /// The first presented frame's decoded size.
    first_size: Option<(u32, u32)>,
    /// Frames sent, for the reader's picture limit.
    sent: Arc<AtomicU64>,
    /// The time each frame was sent at: its source presentation time, or
    /// the time [`frame_time`] moved it to.
    sent_pts: Vec<i64>,
    /// The last frame sent: its source presentation time, and the time it
    /// was sent at.
    last: Option<(i64, i64)>,
    /// The source's times are its decode times
    /// ([`times_in_decode_order`]): each frame takes the earliest
    /// presented time of the packets sent whose time no frame took yet,
    /// kept in `pending` with the packets that have it, instead of its own
    /// packet's.
    decode_order: bool,
    pending: BTreeMap<i64, u32>,
    dropped: u64,
    not_presented: u64,
}

impl Run<'_> {
    /// One frame from the decoder: encoded when the source presents it
    /// and its time is not below the last one's ([`frame_time`]). Its
    /// pixels go to the encoder where they are, in CPU memory or in the
    /// decoder's shared memory, unless they are converted or resized.
    fn take(&mut self, frame: Frame) -> Result<(), Error> {
        let Some(pts) = frame.pts else {
            self.dropped += 1;
            return Ok(());
        };
        if !self.presented.contains(pts) {
            self.not_presented += 1;
            return Ok(());
        }
        let pts = if self.decode_order {
            let Some(mut earliest) = self.pending.first_entry() else {
                self.dropped += 1;
                return Ok(());
            };
            let pts = *earliest.key();
            let count = earliest.get_mut();
            *count = count.saturating_sub(1);
            if *count == 0 {
                earliest.remove();
            }
            pts
        } else {
            pts
        };
        let Some(time) = frame_time(self.last, pts, self.presented.limit(pts)) else {
            self.dropped += 1;
            return Ok(());
        };
        let frame_error = |e: coreav_frame::FrameError| unsupported(format!("frame: {e}"));
        // NVDEC's NV12 (and P010) to planar, without loss.
        let pixels = match frame.format() {
            PixelFormat::SemiPlanar { .. } => Pixels::Cpu(
                frame
                    .pixels
                    .into_cpu()
                    .and_then(|p| p.to_planar())
                    .map_err(frame_error)?,
            ),
            _ => match frame.pixels {
                Pixels::Cuda(_) => Pixels::Cpu(frame.pixels.into_cpu().map_err(frame_error)?),
                pixels => pixels,
            },
        };
        let decoded = Frame { pixels, ..frame };
        if self.encoder.is_none() {
            let settings = self.settings(&decoded)?;
            let sink = SpoolSink::new(self.spool, Arc::clone(&self.sent), self.options)?;
            let limits = EncoderLimits::new(encoder_memory(
                &settings,
                self.options.decode_limits.max_memory_bytes,
                host_memory(),
            ));
            let encoder = Encoder::spawn(
                self.program,
                &settings,
                self.launcher,
                &limits,
                self.deadline,
                sink,
            )
            .map_err(|e| match e.kind {
                // A format or size the encoder does not take: refused before
                // it starts.
                ErrorKind::Unsupported => Error::Unsupported(e.message),
                _ => Error::Encode(e),
            })?;
            self.encoder = Some((encoder, settings));
        }
        let Some((encoder, settings)) = self.encoder.as_mut() else {
            return Err(encode_error("the encoder is gone".into()));
        };
        if decoded.format() != settings.format {
            return Err(unsupported(format!(
                "the pixel format changed within the stream, from {:?} to {:?}",
                settings.format,
                decoded.format()
            )));
        }
        let size = (decoded.width(), decoded.height());
        if self.options.one_size && *self.first_size.get_or_insert(size) != size {
            return Err(unsupported(
                "the frame size changes within the stream, and the frames are not resized",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(timed_out());
        }
        let pixels = if size == (settings.width, settings.height) {
            decoded.pixels
        } else {
            // FFmpeg's default scale at the same pixel format
            // (docs/design/transcode.md, "Size").
            let picture = decoded.pixels.into_cpu().map_err(frame_error)?;
            Pixels::Cpu(
                picture
                    .resize_with(
                        settings.width,
                        settings.height,
                        Filter::Bicubic,
                        ChromaSiting::FfmpegCompatible,
                    )
                    .map_err(|e| unsupported(format!("resizing: {e}")))?,
            )
        };
        // A resize of a large frame takes time of its own.
        if Instant::now() >= self.deadline {
            return Err(timed_out());
        }
        // Before the frame is queued: the encoder may write its picture as
        // soon as the writer thread has written the frame.
        self.sent.fetch_add(1, Ordering::SeqCst);
        encoder.send_frame_at(pixels, time).map_err(Error::Encode)?;
        self.sent_pts.push(time);
        self.last = Some((pts, time));
        Ok(())
    }

    /// The encoder's settings from the first presented frame: its format
    /// (refused unless the encoder takes it), the output size, the sample
    /// aspect ratio that keeps the display aspect ratio, the colour
    /// description and the frame rate.
    fn settings(&self, frame: &Frame) -> Result<EncoderSettings, Error> {
        if let Some(wanted) = self.options.pixel_format
            && frame.format() != wanted
        {
            return Err(unsupported(
                "the frames decode to another pixel format than the one asked for, and are not converted",
            ));
        }
        let (width, height) = (frame.width(), frame.height());
        let (out_w, out_h) = match self.options.scale {
            Some(scale) => scale.resolve(width, height).ok_or_else(|| {
                unsupported(format!("--scale gives no valid size for {width}x{height}"))
            })?,
            None => (width, height),
        };
        let sample_aspect = sample_aspect(self.video.pixel_aspect, (width, height), (out_w, out_h));
        let color = output_color(self.video.stream.color.or(frame.color));
        let (num, den) = time_base_terms(self.video.time_base)?;
        let settings = EncoderSettings {
            timing: Timing::Timestamps {
                num,
                den,
                timecodes: self.timecodes.clone(),
            },
            color,
            sample_aspect,
            preset: self.options.preset.clone(),
            rate: self.options.rate,
            threads: self.options.threads,
            ..EncoderSettings::new(out_w, out_h, frame.format(), self.frame_rate)
        };
        settings.validate().map_err(|e| match e.kind {
            ErrorKind::Unsupported | ErrorKind::Limit => Error::Unsupported(e.message),
            _ => Error::Encode(e),
        })?;
        // FFmpeg reads each frame from one Matroska block, which its reader
        // caps; x264 takes every size up to 16,384 a side (`MAX_SIZE`).
        if let EncoderProgram::Ffmpeg(_) = self.program {
            let bytes = settings
                .frame_bytes()
                .map_err(|e| Error::Unsupported(e.message))?;
            if bytes > crate::rawmkv::MAX_FRAME_BYTES {
                return Err(unsupported(format!(
                    "a {out_w}x{out_h} frame of {bytes} bytes is more than ffmpeg reads in one \
                     Matroska block ({} bytes); use x264 or --scale",
                    crate::rawmkv::MAX_FRAME_BYTES
                )));
            }
        }
        Ok(settings)
    }
}

/// The colour description the encoder signals: the frames', without the
/// chroma location (not passed) and without colour primaries 22, which
/// neither program can signal (`docs/design/transcode.md`, "The encoder
/// program"), so that `colr` says what the stream's VUI says.
fn output_color(color: ColorInfo) -> ColorInfo {
    ColorInfo {
        primaries: color.primaries.filter(|p| *p != 22),
        chroma_location: None,
        ..color
    }
}

/// The output's sample aspect ratio: the source's (`pixel_aspect`, 1:1
/// when not stated) times the change of the pixel grid's shape, so that the
/// display aspect ratio is kept, reduced; `None` for 1:1.
fn sample_aspect(
    pixel_aspect: Option<(u64, u64)>,
    (in_w, in_h): (u32, u32),
    (out_w, out_h): (u32, u32),
) -> Option<(u32, u32)> {
    let (h, v) = pixel_aspect
        .filter(|(h, v)| *h > 0 && *v > 0)
        .unwrap_or((1, 1));
    let num = u128::from(h) * u128::from(in_w) * u128::from(out_h);
    let den = u128::from(v) * u128::from(in_h) * u128::from(out_w);
    let g = gcd(num, den).max(1);
    let (num, den) = (num / g, den / g);
    if num == den || num == 0 || den == 0 {
        return None;
    }
    match (u32::try_from(num), u32::try_from(den)) {
        (Ok(n), Ok(d)) => Some((n, d)),
        // Too large to state exactly: the encoder refuses what H.264
        // cannot signal anyway; keep a ratio within 32 bits that it can
        // reduce or refuse.
        _ => {
            let shift = 128 - num.max(den).leading_zeros();
            let shift = shift.saturating_sub(32);
            let (n, d) = (num >> shift, den >> shift);
            Some((
                u32::try_from(n).unwrap_or(u32::MAX).max(1),
                u32::try_from(d).unwrap_or(u32::MAX).max(1),
            ))
        }
    }
}

/// The `avc1` sample entry: `avcC` from the encoder's parameter sets,
/// `colr` when a code point is known, `pasp` when pixels are not square.
fn sample_entry(config: &annexb::Config, settings: &EncoderSettings) -> Result<Vec<u8>, Error> {
    let color = settings.color;
    let colour = (color.primaries.is_some() || color.transfer.is_some() || color.matrix.is_some())
        .then(|| Nclx {
            primaries: color.primaries.map_or(2, u16::from),
            transfer: color.transfer.map_or(2, u16::from),
            matrix: color.matrix.map_or(2, u16::from),
            full_range: color.full_range.unwrap_or(false),
        });
    let width = u16::try_from(config.width)
        .map_err(|_| encode_error("the encoder's stream is too wide for avc1".into()))?;
    let height = u16::try_from(config.height)
        .map_err(|_| encode_error("the encoder's stream is too tall for avc1".into()))?;
    avc1_sample_entry(width, height, &config.avcc, settings.sample_aspect, colour)
        .map_err(|e| encode_error(format!("the encoder's configuration: {e}")))
}

/// The track header's presentation size, 16.16 fixed point: the width
/// stretched by the sample aspect ratio, as FFmpeg writes it.
fn presentation_size(settings: &EncoderSettings) -> Result<(u32, u32), Error> {
    let (n, d) = settings.sample_aspect.unwrap_or((1, 1));
    let width = ((u64::from(settings.width) * u64::from(n)) << 16) / u64::from(d.max(1));
    let height = u64::from(settings.height) << 16;
    match (u32::try_from(width), u32::try_from(height)) {
        (Ok(w), Ok(h)) => Ok((w, h)),
        _ => Err(encode_error(
            "the display width does not fit the track header".into(),
        )),
    }
}

/// The terms of the stream's time base, seconds per tick.
fn time_base_terms(time_base: Rational) -> Result<(u64, u64), Error> {
    match u64::try_from(time_base.num) {
        Ok(num) if num > 0 => Ok((num, time_base.den)),
        _ => Err(unsupported("the video stream has no time base")),
    }
}

/// A file of the transcode next to the output (the spool file, x264's
/// timecode file), removed when dropped. It is created new and kept open:
/// every write and read goes through this handle, never through the path
/// again, so a link put in its place cannot redirect them.
pub(crate) struct Spool {
    path: PathBuf,
    file: File,
}

impl Spool {
    /// A new file `.{name}.coreav-{pid}.{kind}` next to `output`, the name
    /// shortened to fit 255 bytes, its path absolute (the encoder reads the
    /// timecode file from it).
    pub(crate) fn create(output: &Path, kind: &str) -> Result<Self, Error> {
        let name = output
            .file_name()
            .map_or_else(|| "output".into(), |n| n.to_string_lossy().into_owned());
        let suffix = format!(".coreav-{}.{kind}", std::process::id());
        let mut end = name.len().min(255usize.saturating_sub(suffix.len() + 1));
        while !name.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        let kept = name.get(..end).unwrap_or_default();
        let path = std::path::absolute(output.with_file_name(format!(".{kept}{suffix}")))
            .map_err(|e| Error::Spool(e.kind()))?;
        let mut options = File::options();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(&path).map_err(|e| Error::Spool(e.kind()))?;
        Ok(Self { path, file })
    }

    /// Another handle to the same open file.
    pub(crate) fn handle(&self) -> Result<File, Error> {
        self.file.try_clone().map_err(|e| Error::Spool(e.kind()))
    }

    fn writer(&self) -> Result<BufWriter<File>, Error> {
        self.handle().map(|f| BufWriter::with_capacity(1 << 20, f))
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One picture in the spool file.
#[derive(Clone, Copy, Debug)]
struct Spooled {
    size: u32,
    idr: bool,
    presentation: u64,
}

/// The encoder's output on its reader thread: through the Annex B reader
/// into the spool file.
struct SpoolSink {
    reader: AnnexBReader,
    sent: Arc<AtomicU64>,
    file: BufWriter<File>,
    bytes: u64,
    max_bytes: u64,
    pictures: Vec<Spooled>,
}

/// What the encoder wrote, once it finished.
struct Encoded {
    config: annexb::Config,
    pictures: Vec<Spooled>,
}

impl SpoolSink {
    fn new(spool: &Spool, sent: Arc<AtomicU64>, options: &TranscodeOptions) -> Result<Self, Error> {
        let limits = annexb::Limits {
            max_nal_unit_bytes: usize::try_from(options.isobmff_limits.max_packet_bytes)
                .unwrap_or(usize::MAX),
            max_access_unit_bytes: usize::try_from(options.isobmff_limits.max_packet_bytes)
                .unwrap_or(usize::MAX),
            max_pictures: 0,
            ..annexb::Limits::default()
        };
        Ok(Self {
            reader: AnnexBReader::new(limits),
            sent,
            file: spool.writer()?,
            bytes: 0,
            max_bytes: options.max_spool_bytes,
            pictures: Vec::new(),
        })
    }

    /// Moves the pictures the reader has finished to the spool file.
    fn drain(&mut self) -> Result<(), coreav_decode::Error> {
        while let Some(picture) = self.reader.pop() {
            let size = u32::try_from(picture.data.len())
                .map_err(|_| sink_error(ErrorKind::Limit, "an encoded picture over 4 GiB"))?;
            self.bytes = self.bytes.saturating_add(u64::from(size));
            if self.bytes > self.max_bytes {
                return Err(sink_error(
                    ErrorKind::Limit,
                    "the encoded video is larger than the spool file limit",
                ));
            }
            self.file
                .write_all(&picture.data)
                .map_err(|e| sink_error(ErrorKind::Io, &format!("spool file: {}", e.kind())))?;
            self.pictures.push(Spooled {
                size,
                idr: picture.idr,
                presentation: picture.presentation,
            });
        }
        Ok(())
    }

    /// Ends the stream: its last pictures, the file flushed and closed, and
    /// the checks that it is one picture for each of the `frames` sent.
    fn close(mut self, frames: u64) -> Result<Encoded, Error> {
        self.reader.set_max_pictures(frames);
        self.reader
            .finish()
            .map_err(|e| encode_error(format!("the encoder's output: {e}")))?;
        self.drain().map_err(Error::Encode)?;
        self.file.flush().map_err(|e| Error::Spool(e.kind()))?;
        let pictures = self.pictures;
        if u64::try_from(pictures.len()).ok() != Some(frames) {
            return Err(encode_error(format!(
                "the encoder returned {} pictures for {frames} frames",
                pictures.len()
            )));
        }
        let config = self
            .reader
            .config()
            .cloned()
            .ok_or_else(|| encode_error("the encoder's output has no coded picture".into()))?;
        Ok(Encoded { config, pictures })
    }
}

fn sink_error(kind: ErrorKind, message: &str) -> coreav_decode::Error {
    coreav_decode::Error::new(kind, message)
}

impl Sink for SpoolSink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), coreav_decode::Error> {
        self.reader
            .set_max_pictures(self.sent.load(Ordering::SeqCst));
        self.reader.push(bytes).map_err(|e| {
            coreav_decode::Error::new(ErrorKind::Backend, format!("the encoder's output: {e}"))
        })?;
        self.drain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(num: u128, den: u128) -> (u32, u32) {
        let r = snap(num, den);
        (r.num, r.den)
    }

    #[test]
    fn rates_snap_to_integer_and_ntsc_rates() {
        assert_eq!(rate(25, 1), (25, 1));
        assert_eq!(rate(30_000, 1001), (30_000, 1001));
        assert_eq!(rate(24_000, 1001), (24_000, 1001));
        assert_eq!(rate(60_000, 1001), (60_000, 1001));
        // Within 0.1% of 30: 30.02.
        assert_eq!(rate(3002, 100), (30, 1));
        // 29.97 itself is nearer 30000/1001 than 30.
        assert_eq!(rate(2997, 100), (30_000, 1001));
        // The vfr.mp4 fixture: 33 frames over 1.8 s, 55/3.
        assert_eq!(rate(55, 3), (55, 3));
        // Far from both: the rate itself, reduced.
        assert_eq!(rate(1250, 100), (25, 2));
        // Terms over 32 bits: thousandths.
        assert_eq!(rate(12_345_678_901_234, 1_000_000_000_007), (12_346, 1000));
    }

    #[test]
    fn the_encoder_budget_grows_with_the_frame() {
        let budget = 8 << 30;
        let settings = |w, h, depth, threads| EncoderSettings {
            threads: Some(threads),
            ..EncoderSettings::new(
                w,
                h,
                PixelFormat::Yuv {
                    chroma: coreav_frame::Chroma::Cs420,
                    depth,
                },
                FrameRate { num: 25, den: 1 },
            )
        };
        let host = Some(128 << 30);
        // 1080p keeps the decode budget.
        assert_eq!(
            encoder_memory(&settings(1920, 1080, 8, 12), budget, host),
            budget
        );
        // 8K at 12 threads: 49,766,400 bytes a frame, 320 frames and 1 GiB.
        assert_eq!(
            encoder_memory(&settings(7680, 4320, 8, 12), budget, host),
            49_766_400 * 320 + (1 << 30)
        );
        // At 10 bits, two bytes a sample.
        assert_eq!(
            encoder_memory(&settings(7680, 4320, 10, 12), budget, host),
            2 * 49_766_400 * 320 + (1 << 30)
        );
        // At most four times the budget,
        assert_eq!(
            encoder_memory(&settings(16_384, 16_384, 8, 12), budget, host),
            4 * budget
        );
        // half the host's memory (never below the budget),
        assert_eq!(
            encoder_memory(&settings(7680, 4320, 8, 12), budget, Some(24 << 30)),
            12 << 30
        );
        assert_eq!(
            encoder_memory(&settings(7680, 4320, 8, 12), budget, Some(16 << 30)),
            budget
        );
        // or twice the budget where the host's memory is unknown.
        assert_eq!(
            encoder_memory(&settings(16_384, 16_384, 8, 12), budget, None),
            2 * budget
        );
    }

    #[test]
    fn pacing_waits_for_frames_and_clears_a_stall() {
        let full = |stall| {
            let mut pace = Pace {
                sizes: VecDeque::new(),
                bytes: 0,
                cap: 1000,
                stall,
            };
            for _ in 0..=PACE_PACKETS {
                pace.sent(100);
            }
            pace
        };
        let far = Instant::now() + Duration::from_secs(60);
        // A frame on the third poll, presented: one packet retired.
        let mut pace = full(PACE_STALL);
        let mut polls = 0;
        let mut taken = Vec::new();
        pace.wait(
            far,
            || {
                polls += 1;
                Ok((polls >= 3).then_some(polls))
            },
            |f| {
                taken.push(f);
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(taken, [3]);
        assert!(!pace.full());
        // Frames of packets not presented retire nothing; a decoder that
        // gives nothing more clears the count after the stall.
        let mut pace = full(Duration::from_millis(20));
        let mut polls = 0;
        pace.wait(
            far,
            || {
                polls += 1;
                Ok((polls <= 2).then_some(()))
            },
            |()| Ok(false),
        )
        .unwrap();
        assert!(pace.sizes.is_empty() && pace.bytes == 0);
        // The deadline ends the wait.
        let mut pace = full(PACE_STALL);
        let past = Instant::now();
        assert!(pace.wait(past, || Ok(None::<()>), |()| Ok(true)).is_err());
    }

    #[test]
    fn the_decoded_bytes_cap_follows_the_packets() {
        let limits = coreav_decode::Limits::default();
        let hd = Some((1920, 1080));
        // A short stream keeps the configured cap.
        assert_eq!(decoded_bytes_cap(&limits, 2, hd), 4 << 30);
        // 3000 frames of 1080p: 1920 x 1080 x 6 bytes each.
        assert_eq!(decoded_bytes_cap(&limits, 3000, hd), 3000 * 1920 * 1080 * 6);
        // An undeclared size: max_frame_bytes a packet.
        assert_eq!(decoded_bytes_cap(&limits, 3000, None), 3000 * (512 << 20));
        assert_eq!(decoded_bytes_cap(&limits, usize::MAX, hd), u64::MAX);
    }

    #[test]
    fn pacing_waits_past_both_caps() {
        let mut pace = Pace {
            sizes: VecDeque::new(),
            bytes: 0,
            cap: 1000,
            stall: PACE_STALL,
        };
        for _ in 0..PACE_PACKETS {
            pace.sent(100);
        }
        // Over the bytes, not over the packets.
        assert!(!pace.full());
        pace.sent(100);
        assert!(pace.full());
        pace.received();
        assert!(!pace.full());
        // Many small packets: under the bytes.
        let mut small = Pace {
            sizes: VecDeque::new(),
            bytes: 0,
            cap: 1 << 20,
            stall: PACE_STALL,
        };
        for _ in 0..1000 {
            small.sent(10);
        }
        assert!(!small.full());
    }

    fn presented(pts: &[i64], last: u64) -> Presented {
        let times = pts.to_vec();
        Presented {
            pts: pts.iter().copied().collect(),
            distinct: times.clone(),
            durations: times.last().map(|t| (*t, last)).into_iter().collect(),
            last: times.last().copied().unwrap_or(0),
            times,
        }
    }

    #[test]
    fn repeated_times_move_one_tick() {
        // WebM blocks whose millisecond times repeat: kept, each one tick
        // after the frame before, until a gap takes the moves up.
        let send = |times: &[i64], next: &dyn Fn(i64) -> Option<i64>| {
            let mut last = None;
            let mut sent = Vec::new();
            for &pts in times {
                if let Some(time) = frame_time(last, pts, next(pts)) {
                    sent.push(time);
                    last = Some((pts, time));
                }
            }
            sent
        };
        // A burst at 9580, then 9929: kept, one tick apart.
        let times = [7863, 9580, 9580, 9580, 9929];
        let next = |pts| times.iter().copied().find(|t| *t > pts);
        assert_eq!(send(&times, &next), [7863, 9580, 9581, 9582, 9929]);
        // More frames than ticks (1,500 a second in milliseconds): a repeat
        // never reaches the next frame's own time, so the times do not
        // drift from the source's.
        let times = [0, 1, 1, 2, 3, 3, 4];
        let next = |pts| times.iter().copied().find(|t| *t > pts);
        assert_eq!(send(&times, &next), [0, 1, 2, 3, 4]);
        // The last time's repeats: no later frame to pass.
        let next = |pts| (pts < 10).then_some(10);
        assert_eq!(send(&[0, 10, 10, 10], &next), [0, 10, 11, 12]);
        // A time below the last one's is dropped.
        assert_eq!(frame_time(Some((9580, 9582)), 9579, Some(9929)), None);
        assert_eq!(frame_time(Some((i64::MAX, i64::MAX)), i64::MAX, None), None);
    }

    #[test]
    fn the_average_rate() {
        let tb = Rational::new(1, 12_800).unwrap();
        let cfr: Vec<i64> = (0..50).map(|n| n * 512).collect();
        let r = presented(&cfr, 512).frame_rate(tb);
        assert_eq!((r.num, r.den), (25, 1));
        // The vfr.mp4 fixture's times: 24 frames 512 apart, then 1536.
        let mut vfr: Vec<i64> = (0..24).map(|n| n * 512).collect();
        vfr.extend((0..9).map(|n| 12_288 + n * 1536));
        let r = presented(&vfr, 1536).frame_rate(tb);
        assert_eq!((r.num, r.den), (275, 17));
        let ms: Vec<i64> = (0..300).map(|n| n * 1001 / 30).collect();
        let r = presented(&ms, 33).frame_rate(Rational::new(1, 1000).unwrap());
        assert_eq!((r.num, r.den), (30_000, 1001));
    }

    #[test]
    fn sample_aspect_keeps_the_display_aspect() {
        assert_eq!(sample_aspect(None, (320, 240), (320, 240)), None);
        assert_eq!(
            sample_aspect(None, (320, 240), (162, 121)),
            Some((242, 243))
        );
        assert_eq!(
            sample_aspect(None, (1280, 544), (848, 360)),
            Some((900, 901))
        );
        assert_eq!(
            sample_aspect(Some((2, 1)), (720, 480), (360, 240)),
            Some((2, 1))
        );
        assert_eq!(sample_aspect(Some((4, 3)), (720, 480), (960, 480)), None);
        assert_eq!(sample_aspect(None, (320, 240), (320, 120)), Some((1, 2)));
    }

    #[test]
    fn spool_files_are_removed() {
        let dir = std::env::temp_dir().join(format!("coreav-spool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let output = dir.join(format!("{}.mp4", "x".repeat(300)));
        let spool = Spool::create(&output, "spool").unwrap();
        let path = spool.path.clone();
        assert!(path.is_file());
        assert!(path.file_name().unwrap().len() <= 255);
        // A second one in the same place is refused, never shared.
        assert!(Spool::create(&output, "spool").is_err());
        assert!(Spool::create(&output, "timecodes").is_ok());
        drop(spool);
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
