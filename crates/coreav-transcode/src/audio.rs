//! Encoded audio of a transcode (`docs/design/transcode.md`, "Opus audio"
//! "AAC audio" and "MP3 audio"; policy record 0042): one audio stream of the source
//! decoded by `coreav-audio` to 32-bit float samples, encoded by an encoder
//! process and spooled for a container writer. The Opus encoder is libopus in a process of its own
//! (`coreav_worker::opus`, through the sandbox launcher), on samples
//! resampled to 48 kHz; the AAC encoder is the operator's `ffmpeg` program
//! (`crate::aac`), on samples at the stream's own rate. MP3 uses the
//! operator's `ffmpeg -c:a libmp3lame`, with checked Matroska readback and
//! an explicit EOF zero guard removed by the MP3 writer's gapless tag.
//!
//! The audio is encoded before the video, so that a stream CoreAV does not
//! decode, or one of more than two channels, is refused before the video
//! encoder starts.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use coreav_audio::{OutputFormat, SampleFormat, Warning};
use coreav_decode::ErrorKind;
use coreav_isobmff::{BufferedSource, FileSource, FourCc, Source};
use coreav_isobmff_write::AudioSample;
use coreav_isobmff_write::mux::{RawBox, audio_sample_entry, esds};
use coreav_worker::opus::{
    FRAME_SAMPLES, MAX_BITRATE, MAX_PRE_SKIP, MIN_BITRATE, PacketReader, ReadError, encode_args,
    packets_for,
};
use coreav_worker::{ExecLauncher, HugePages};

use crate::aac::{self, AacAudio};
use crate::encoder::{EncoderLimits, Process, Sink, command, is_path};
use crate::mp3::{self, Mp3Audio};
use crate::pipeline::{Error, Spool, spool_read};

/// The sample rate of every Opus stream (RFC 7845 section 5.1): the
/// decoded audio is resampled to it, and it is the track's timescale.
pub const SAMPLE_RATE: u32 = 48_000;

/// The roll distance of the track, in samples: 80 ms of 20 ms packets, the
/// pre-roll a decoder needs after a random access (opus-codec.org
/// "Encapsulation of Opus in ISO Base Media File Format", 4.3.6.2).
pub const ROLL: u16 = 4;

/// The roll distance of an AAC track, in frames: a frame's overlap with the
/// one before it (ISO/IEC 14496-3 4.6.11, the MDCT's overlap and add).
pub const AAC_ROLL: u16 = 1;

/// The Opus encoder process's address space: libopus needs a few
/// megabytes; the rest is the `coreav` program's own mappings.
const ENCODER_MEMORY: u64 = 1 << 30;

/// The AAC encoder process's address space: the `ffmpeg` program and the
/// libraries it maps.
const AAC_ENCODER_MEMORY: u64 = 2 << 30;

/// Bytes of the `ffmpeg` program's answer about its `aac` encoder kept.
const PROBE_ANSWER: usize = 64 << 10;

/// The default bit rate in bits a second, for `channels` (1 or 2): 64,000
/// for mono and 96,000 for stereo, the rates `ffmpeg -c:a libopus` encodes
/// at without `-b:a`.
pub fn default_bitrate(channels: u8) -> u32 {
    if channels == 1 { 64_000 } else { 96_000 }
}

/// One audio stream encoded to Opus ([`crate::AudioMode::Opus`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpusAudio {
    /// The source stream, by its position among all of the source's
    /// streams; `None` for the first audio stream that CoreAV decodes.
    pub stream: Option<usize>,
    /// Bits a second, from [`MIN_BITRATE`] to [`MAX_BITRATE`]; `None`
    /// for [`default_bitrate`].
    pub bitrate: Option<u32>,
    /// The program that runs as the Opus encoder with
    /// `coreav_worker::opus::ENCODE_ARG` first: the `coreav` program, a
    /// path. It runs through the sandbox launcher when the transcode has
    /// one.
    pub program: PathBuf,
}

/// The encoder of one audio stream, and its settings.
pub enum Encode<'a> {
    /// libopus, in the encoder process [`OpusAudio::program`] runs.
    Opus(&'a OpusAudio),
    /// FFmpeg's `aac` encoder, by this `ffmpeg` program.
    Aac(&'a AacAudio, PathBuf),
    /// FFmpeg's libmp3lame encoder, run as a separate program.
    Mp3(&'a Mp3Audio, PathBuf),
}

impl Encode<'_> {
    fn codec(&self) -> &'static str {
        match self {
            Self::Opus(_) => "Opus",
            Self::Aac(..) => "AAC",
            Self::Mp3(..) => "MP3",
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Opus(_) => "the Opus encoder",
            Self::Aac(..) => "the AAC encoder",
            Self::Mp3(..) => "the MP3 encoder",
        }
    }

    fn program(&self) -> &Path {
        match self {
            Self::Opus(settings) => &settings.program,
            Self::Aac(_, program) => program,
            Self::Mp3(_, program) => program,
        }
    }
}

/// The encoded audio stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSummary {
    /// The source stream's position among all of its streams.
    pub stream_index: usize,
    /// The source stream's codec.
    pub codec: coreav_audio::Codec,
    /// The source stream's sample rate in Hz.
    pub sample_rate: u32,
    /// The source stream's channels.
    pub source_channels: u16,
    /// The encoder: `libopus`, `aac`, or `libmp3lame`.
    pub encoder: &'static str,
    /// The output's channels: 1 or 2, the source's.
    pub channels: u8,
    /// The bit rate asked of the encoder, in bits a second; `None` for the
    /// external encoder's own default or MP3 quality mode.
    pub bitrate: Option<u32>,
    /// Samples per channel presented, at the output's rate (48 kHz for
    /// Opus, the source's for AAC and MP3): the decoded stream's.
    pub samples: u64,
    /// Audio packets written: Opus packets of 20 ms, AAC frames of 1,024
    /// samples, or MP3 frames of 1,152 or 576 samples.
    pub packets: u64,
    /// The encoded stream's presentation delay in output samples,
    /// including the MP3 decoder delay, when applicable.
    pub pre_skip: u16,
    /// Completed actual-stdin capture; not a complete transcode receipt.
    #[cfg(feature = "input-capture")]
    pub input_capture: Option<EncoderInput>,
}

/// Encoder input identity, emitted only for an explicitly requested capture.
#[cfg(feature = "input-capture")]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct EncoderInput {
    /// Sample representation and byte order on the actual input pipe.
    pub format: &'static str,
    /// The actual bytes accepted by stdin.
    pub pcm: crate::capture::Receipt,
    /// SHA-256 of the encoder executable before and after execution.
    pub encoder_sha256: String,
    /// The encoder arguments actually supplied to the launcher.
    pub argv: Vec<String>,
    /// The final command's actual launcher arguments, including its limits.
    pub launcher_argv: Vec<String>,
    /// Input sample rate in Hz.
    pub sample_rate: u32,
    /// WAV speaker mask of the supplied channel order.
    pub channel_mask: u32,
    /// First presented sample in output sample-rate ticks.
    pub start_ticks: u64,
    /// Whether the normal sandbox launcher was used.
    pub sandboxed: bool,
    /// Address-space limit supplied to the normal launcher.
    pub address_space: Option<u64>,
    /// Open-file limit supplied to the normal launcher.
    pub open_files: Option<u64>,
}

/// The checked encoded audio, ready for a container writer.
pub struct EncodedAudio {
    pub(crate) summary: AudioSummary,
    description: Description,
    /// The media timescale: the output's sample rate.
    pub(crate) timescale: u32,
    /// Media ticks of silence before the first presented sample: where a
    /// Matroska source's stream starts ([`matroska_start`]); 0 for an MP4
    /// source, whose leading empty edit the decoder writes as silence.
    pub(crate) start: u64,
    /// The packets, back to back in `spool`.
    pub(crate) samples: Vec<AudioSample>,
    pub(crate) spool: Spool,
}

enum Description {
    Mp4 { entry: Vec<u8>, roll: u16 },
    Mp3(mp3::Info),
}

impl EncodedAudio {
    /// Retains the checked MP3 frames for a bounded, per-frame container writer.
    pub fn mp3_packets(&self, deadline: Instant, max_bytes: usize) -> Result<mp3::Packets, Error> {
        if !matches!(self.description, Description::Mp3(_)) {
            return Err(Error::Unsupported("non-MP3 audio in AVI output".into()));
        }
        let mut length = 0usize;
        let mut frames = Vec::with_capacity(self.samples.len());
        for sample in &self.samples {
            let end = length
                .checked_add(usize::try_from(sample.size).unwrap_or(usize::MAX))
                .filter(|&end| end <= max_bytes)
                .ok_or_else(|| {
                    Error::Encode(coreav_decode::Error::new(
                        ErrorKind::Limit,
                        "MP3 output exceeds its buffer limit",
                    ))
                })?;
            frames.push(length..end);
            length = end;
        }
        let frame_samples = self
            .samples
            .first()
            .map(|sample| sample.duration)
            .ok_or_else(|| encoder_error("MP3 output has no frames".into()))?;
        let decoded = u128::from(frame_samples) * frames.len() as u128;
        let bytes_per_second = u32::try_from(length as u128 * u128::from(self.timescale) / decoded)
            .map_err(|_| encoder_error("MP3 byte rate overflows".into()))?;
        let mut data = Vec::with_capacity(length);
        self.write_packets(deadline, &mut data)?;
        Ok(mp3::Packets {
            data,
            frames,
            frame_samples,
            bytes_per_second,
        })
    }

    pub(crate) fn mp4_description(&self) -> Result<(&[u8], u16), Error> {
        match &self.description {
            Description::Mp4 { entry, roll } => Ok((entry, *roll)),
            Description::Mp3(_) => Err(Error::Unsupported("MP3 audio in MP4 output".into())),
        }
    }

    /// Writes ID3v2.4 metadata, a gapless Xing/Info frame and the checked
    /// Layer III frames. Discard the output if writing fails.
    pub fn write_mp3<W: Write + ?Sized>(
        &self,
        tags: &[(String, String)],
        deadline: Instant,
        out: &mut W,
    ) -> Result<(), Error> {
        let Description::Mp3(info) = &self.description else {
            return Err(Error::Unsupported("non-MP3 audio in MP3 output".into()));
        };
        let sizes: Vec<_> = self.samples.iter().map(|sample| sample.size).collect();
        let metadata = mp3::metadata(info, tags).map_err(|e| encoder_error(e.to_string()))?;
        let frame = mp3::info_frame(info, &sizes).map_err(|e| encoder_error(e.to_string()))?;
        out.write_all(&metadata)
            .and_then(|()| out.write_all(&frame))
            .map_err(|e| Error::Remux(coreav_isobmff_write::ExtractError::Write(e.kind())))?;
        self.write_packets(deadline, out)
    }
    /// Writes this checked stream alone as MP4 or M4A, without opening
    /// the source again. `file_type` selects the brands. The edit list
    /// presents exactly the submitted samples after any leading silence.
    /// A partial output must be discarded if writing fails.
    pub fn write_mp4<W: Write + ?Sized>(
        &self,
        file_type: coreav_isobmff_write::mux::FileType,
        limits: &coreav_isobmff::Limits,
        deadline: Instant,
        mut out: &mut W,
    ) -> Result<(), Error> {
        use coreav_isobmff_write::mux::{
            Edit, MovieHeader, Muxer, Sample, TrackHeader, TrackMedia,
        };
        let (entry, roll) = self.mp4_description()?;
        let writer = |error: coreav_isobmff::Error| Error::Remux(error.into());
        let mut muxer = Muxer::new(
            file_type,
            MovieHeader {
                timescale: self.timescale,
                creation_time: 0,
                boxes: Vec::new(),
            },
            limits,
        )
        .map_err(writer)?;
        let mut edits = Vec::new();
        if self.start > 0 {
            edits.push(Edit {
                duration: self.start,
                media_time: -1,
                rate: Edit::UNIT_RATE,
            });
        }
        edits.push(Edit {
            duration: self.summary.samples,
            media_time: i64::from(self.summary.pre_skip),
            rate: Edit::UNIT_RATE,
        });
        let count = u32::try_from(self.samples.len())
            .map_err(|_| encoder_error("more than 2^32 - 1 encoded audio packets".into()))?;
        let track = muxer
            .add_track(TrackHeader {
                media: TrackMedia::Audio { volume: 0x100 },
                timescale: self.timescale,
                language: *b"und",
                creation_time: 0,
                media_creation_time: 0,
                flags: 3,
                layer: 0,
                alternate_group: 1,
                edits,
                duration: None,
                media_duration: None,
                sample_entry: entry.to_vec(),
                sample_table_boxes: coreav_isobmff_write::roll_boxes(count, roll),
                boxes: Vec::new(),
            })
            .map_err(writer)?;
        for sample in &self.samples {
            if Instant::now() >= deadline {
                return Err(Error::Decode(coreav_decode::Error::new(
                    ErrorKind::Timeout,
                    "the audio encode took longer than its timeout",
                )));
            }
            muxer
                .push_sample(
                    track,
                    Sample {
                        size: sample.size,
                        duration: sample.duration,
                        sync: true,
                        composition_offset: 0,
                    },
                )
                .map_err(writer)?;
        }
        muxer.write_head(&mut out).map_err(Error::Remux)?;
        self.write_packets(deadline, out)
    }

    fn write_packets<W: Write + ?Sized>(
        &self,
        deadline: Instant,
        out: &mut W,
    ) -> Result<(), Error> {
        let mut data =
            BufferedSource::new(FileSource::from_file(self.spool.handle()?).map_err(spool_read)?);
        let mut offset = 0u64;
        let mut bytes = Vec::new();
        for sample in &self.samples {
            if Instant::now() >= deadline {
                return Err(Error::Decode(coreav_decode::Error::new(
                    ErrorKind::Timeout,
                    "the audio encode took longer than its timeout",
                )));
            }
            bytes.resize(usize::try_from(sample.size).unwrap_or(usize::MAX), 0);
            data.read_exact_at(offset, &mut bytes).map_err(spool_read)?;
            out.write_all(&bytes)
                .map_err(|e| Error::Remux(coreav_isobmff_write::ExtractError::Write(e.kind())))?;
            offset = offset
                .checked_add(u64::from(sample.size))
                .ok_or_else(|| encoder_error("encoded audio size overflow".into()))?;
        }
        Ok(())
    }
}

/// Decodes the stream `encode` names from `source` and encodes it. The
/// encoder runs through `launcher` (or directly when it is `None`; for
/// tests) and is killed at `deadline`; its packets go to a spool file next
/// to `output`, at most `max_spool_bytes` of them.
pub fn encode<S: Source>(
    mut source: S,
    encode: &Encode<'_>,
    limits: &coreav_audio::Limits,
    launcher: Option<&ExecLauncher>,
    deadline: Instant,
    output: &Path,
    max_spool_bytes: u64,
) -> Result<EncodedAudio, Error> {
    encode_with(
        |requested, feed| {
            match encode {
                Encode::Aac(_, program) => probe_encoder(program, launcher, deadline, "aac")?,
                Encode::Mp3(_, program) => {
                    probe_encoder(program, launcher, deadline, "libmp3lame")?
                }
                Encode::Opus(_) => {}
            }
            let options = coreav_audio::Options {
                stream: requested,
                format: SampleFormat::F32,
                threads: 1,
                sample_rate: match encode {
                    Encode::Opus(_) => Some(SAMPLE_RATE),
                    Encode::Aac(..) | Encode::Mp3(..) => None,
                },
                channels: None,
            };
            let summary =
                coreav_audio::decode(&mut source, &options, limits, feed).map_err(Error::Audio)?;
            let start = matroska_start(&mut source, Some(summary.stream_index), limits)?;
            Ok((summary, start))
        },
        encode,
        limits,
        launcher,
        deadline,
        output,
        max_spool_bytes,
    )
}

/// Encodes already-decoded interleaved F32 PCM through the same feeder and audit.
pub fn encode_pcm(
    samples: &[u8],
    summary: &coreav_audio::Summary,
    encode: &Encode<'_>,
    launcher: Option<&ExecLauncher>,
    deadline: Instant,
    output: &Path,
    max_spool_bytes: u64,
) -> Result<EncodedAudio, Error> {
    use coreav_audio::Sink as _;
    let frame = u64::from(summary.output.channels) * 4;
    if summary.output.format != SampleFormat::F32
        || frame == 0
        || u64::try_from(samples.len()).ok() != summary.samples.checked_mul(frame)
    {
        return Err(encoder_error(
            "PCM bytes do not match the decoded summary".into(),
        ));
    }
    encode_with(
        |_, feed| {
            feed.start(&summary.output).map_err(Error::Audio)?;
            for bytes in samples.chunks(32 << 10) {
                feed.write(bytes).map_err(Error::Audio)?;
            }
            Ok((summary.clone(), 0))
        },
        encode,
        &coreav_audio::Limits::default(),
        launcher,
        deadline,
        output,
        max_spool_bytes,
    )
}

fn encode_with(
    decode: impl FnOnce(Option<usize>, &mut Feed<'_>) -> Result<(coreav_audio::Summary, u64), Error>,
    encode: &Encode<'_>,
    limits: &coreav_audio::Limits,
    launcher: Option<&ExecLauncher>,
    deadline: Instant,
    output: &Path,
    max_spool_bytes: u64,
) -> Result<EncodedAudio, Error> {
    let (requested, bitrate, range) = match encode {
        Encode::Opus(s) => (s.stream, s.bitrate, MIN_BITRATE..=MAX_BITRATE),
        Encode::Aac(s, _) => (s.stream, s.bitrate, aac::MIN_BITRATE..=aac::MAX_BITRATE),
        Encode::Mp3(s, _) => {
            let bitrate = match s.mode {
                mp3::Mode::Constant(rate) => rate,
                mp3::Mode::Quality(q) if q.is_finite() && (0.0..=9.0).contains(&q) => None,
                mp3::Mode::Quality(_) => {
                    return Err(Error::Unsupported("MP3 quality outside 0 to 9".into()));
                }
            };
            (s.stream, bitrate, 8_000..=320_000)
        }
    };
    if bitrate.is_some_and(|b| !range.contains(&b)) {
        return Err(Error::Unsupported(format!(
            "an {} bit rate outside {} to {} bits a second",
            encode.codec(),
            range.start(),
            range.end()
        )));
    }
    let program = encode.program();
    if !is_path(program) {
        return Err(encoder_error(format!(
            "the {} encoder program must be a path, not a name: {}",
            encode.codec(),
            program.display()
        )));
    }
    let spool = Spool::create(
        output,
        match encode {
            Encode::Opus(_) => "opus",
            Encode::Aac(..) => "aac",
            Encode::Mp3(..) => "mp3",
        },
    )?;
    // The AAC encoder's Matroska, read back once it has ended.
    let raw = match encode {
        Encode::Opus(_) => None,
        Encode::Aac(..) | Encode::Mp3(..) => Some(Spool::create(output, "mkv")?),
    };
    let mut feed = Feed {
        encode,
        launcher,
        deadline,
        spool: raw.as_ref().unwrap_or(&spool),
        max_spool_bytes,
        started: None,
        error: None,
    };
    let decoded = decode(requested, &mut feed);
    let Feed { started, error, .. } = feed;
    let (summary, start_ns) = match (decoded, error) {
        (_, Some(e)) => return Err(e),
        (Err(Error::Audio(coreav_audio::Error::NoSuchStream)), None) if requested.is_none() => {
            return Err(Error::Unsupported(
                "the source has no audio stream CoreAV decodes".into(),
            ));
        }
        (Err(e), None) => return Err(e),
        (Ok(summary), None) => summary,
    };
    if let Some(warning) = summary
        .warnings
        .iter()
        .find(|w| matches!(w, Warning::EditListIgnored | Warning::DemuxStopped { .. }))
    {
        // The decoded samples would not be the stream's presentation.
        return Err(Error::Unsupported(format!("the audio stream: {warning}")));
    }
    let Some(started) = started.filter(|s| s.sent.load(Ordering::SeqCst) > 0) else {
        return Err(Error::Unsupported("the audio stream has no samples".into()));
    };
    let Started {
        mut process,
        channels,
        rate,
        bitrate,
        sent,
        #[cfg(feature = "input-capture")]
        capture,
    } = started;
    let samples = sent.load(Ordering::SeqCst);
    let guard = if matches!(encode, Encode::Mp3(..)) {
        let frame = if rate >= 32_000 { 1152u64 } else { 576 };
        let guard = (frame - samples % frame) % frame;
        if guard > 0 {
            sent.fetch_add(guard, Ordering::SeqCst);
            let bytes = usize::try_from(guard * 4 * u64::from(channels))
                .map_err(|_| encoder_error("MP3 EOF guard size overflow".into()))?;
            process.enqueue(vec![0; bytes]).map_err(Error::Encode)?;
        }
        u16::try_from(guard).map_err(|_| encoder_error("MP3 EOF guard sample overflow".into()))?
    } else {
        0
    };
    let (sizes, pre_skip, description, duration) = match process.finish().map_err(Error::Encode)? {
        Output::Opus(Packets {
            reader,
            mut file,
            sizes,
            ..
        }) => {
            let pre_skip = reader.finish().map_err(|e| encoder_error(e.to_string()))?;
            file.flush().map_err(|e| Error::Spool(e.kind()))?;
            drop(file);
            let count = u64::try_from(sizes.len()).unwrap_or(u64::MAX);
            if count != packets_for(pre_skip, samples) {
                return Err(encoder_error(format!(
                    "the Opus encoder wrote {count} packets for {samples} samples"
                )));
            }
            let entry = sample_entry(channels, pre_skip, summary.stream.sample_rate)?;
            (
                sizes,
                pre_skip,
                Description::Mp4 { entry, roll: ROLL },
                FRAME_SAMPLES,
            )
        }
        Output::External(Stdout {
            mut file, codec, ..
        }) => {
            file.flush().map_err(|e| Error::Spool(e.kind()))?;
            drop(file);
            let raw = raw.ok_or_else(|| encoder_error("no external audio spool file".into()))?;
            let mut written = TimedSource {
                source: BufferedSource::new(
                    FileSource::from_file(raw.handle()?).map_err(spool_read)?,
                ),
                deadline,
            };
            let (frames, pre_skip, description, duration) = match codec {
                External::Aac => {
                    let expected = aac::Expected {
                        sample_rate: rate,
                        channels,
                        samples,
                    };
                    let read = checked_readback(
                        aac::read_output(&mut written, &expected, &limits.matroska),
                        deadline,
                    )?;
                    let entry = aac_sample_entry(&read.config, rate, channels)?;
                    (
                        read.frames
                            .iter()
                            .map(|f| (f.offset, f.size))
                            .collect::<Vec<_>>(),
                        read.priming,
                        Description::Mp4 {
                            entry,
                            roll: AAC_ROLL,
                        },
                        aac::FRAME_SAMPLES,
                    )
                }
                External::Mp3 => {
                    let expected = mp3::Expected {
                        sample_rate: rate,
                        channels,
                        samples: sent.load(Ordering::SeqCst),
                        guard,
                        variable: matches!(
                            encode,
                            Encode::Mp3(
                                Mp3Audio {
                                    mode: mp3::Mode::Quality(_),
                                    ..
                                },
                                _
                            )
                        ),
                    };
                    let read = checked_readback(
                        mp3::read_output(&mut written, &expected, &limits.matroska),
                        deadline,
                    )?;
                    (
                        read.frames
                            .iter()
                            .map(|f| (f.offset, f.size))
                            .collect::<Vec<_>>(),
                        read.priming,
                        Description::Mp3(read.info),
                        read.frame_samples,
                    )
                }
            };
            let mut file = BufWriter::with_capacity(1 << 16, spool.handle()?);
            let mut frame = Vec::new();
            for &(offset, size) in &frames {
                if Instant::now() >= deadline {
                    return Err(Error::Decode(coreav_decode::Error::new(
                        ErrorKind::Timeout,
                        "the audio encode took longer than its timeout",
                    )));
                }
                frame.resize(usize::try_from(size).unwrap_or(usize::MAX), 0);
                written
                    .read_exact_at(offset, &mut frame)
                    .map_err(spool_read)?;
                file.write_all(&frame).map_err(|e| Error::Spool(e.kind()))?;
            }
            file.flush().map_err(|e| Error::Spool(e.kind()))?;
            let pre_skip = u16::try_from(pre_skip)
                .map_err(|_| encoder_error("an audio delay above 65535 samples".into()))?;
            (
                frames.iter().map(|f| f.1).collect(),
                pre_skip,
                description,
                duration,
            )
        }
    };
    #[cfg(feature = "input-capture")]
    let input_capture = capture
        .map(|capture| {
            let sha256 = crate::capture::file_sha256(encode.program())
                .map_err(|e| Error::Spool(e.kind()))?;
            if sha256 != capture.encoder_sha256 {
                return Err(encoder_error(
                    "the captured encoder executable changed".into(),
                ));
            }
            Ok(EncoderInput {
                format: "f32le",
                pcm: capture
                    .pending
                    .finish(samples, channels)
                    .map_err(|e| encoder_error(e.to_string()))?,
                encoder_sha256: sha256,
                argv: capture.argv,
                launcher_argv: capture.launcher_argv,
                sample_rate: rate,
                channel_mask: capture.channel_mask,
                start_ticks: ticks(start_ns, rate),
                sandboxed: launcher.is_some(),
                address_space: launcher.map(|_| AAC_ENCODER_MEMORY),
                open_files: launcher
                    .and_then(|_| EncoderLimits::new(AAC_ENCODER_MEMORY).open_files),
            })
        })
        .transpose()?;
    Ok(EncodedAudio {
        summary: AudioSummary {
            stream_index: summary.stream_index,
            codec: summary.stream.codec,
            sample_rate: summary.stream.sample_rate,
            source_channels: summary.stream.channels,
            encoder: match encode {
                Encode::Opus(_) => "libopus",
                Encode::Aac(..) => "aac",
                Encode::Mp3(..) => "libmp3lame",
            },
            channels,
            bitrate,
            samples,
            packets: u64::try_from(sizes.len()).unwrap_or(u64::MAX),
            pre_skip,
            #[cfg(feature = "input-capture")]
            input_capture,
        },
        description,
        timescale: rate,
        start: ticks(start_ns, rate),
        samples: sizes
            .into_iter()
            .map(|size| AudioSample { size, duration })
            .collect(),
        spool,
    })
}

/// Asks the `ffmpeg` program about its requested encoder, as it runs as the
/// encoder (through `launcher`, killed at `deadline`): one without it is
/// refused before the source is decoded.
fn probe_encoder(
    program: &Path,
    launcher: Option<&ExecLauncher>,
    deadline: Instant,
    codec: &str,
) -> Result<(), Error> {
    struct Answer(Vec<u8>);
    impl Sink for Answer {
        fn write(&mut self, bytes: &[u8]) -> Result<(), coreav_decode::Error> {
            let room = PROBE_ANSWER.saturating_sub(self.0.len());
            self.0
                .extend_from_slice(bytes.get(..room.min(bytes.len())).unwrap_or_default());
            Ok(())
        }
    }
    let command = command(
        program,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-h",
            &format!("encoder={codec}"),
        ]
        .map(std::ffi::OsString::from),
        HugePages::Kept,
        launcher,
        &EncoderLimits::new(AAC_ENCODER_MEMORY),
        deadline,
    );
    let Answer(answer) = Process::spawn(
        if codec == "aac" {
            "the AAC encoder"
        } else {
            "the MP3 encoder"
        },
        "sample blocks",
        command,
        program,
        deadline,
        Answer(Vec::new()),
    )
    .and_then(Process::finish)
    .map_err(Error::Encode)?;
    if !answer.starts_with(format!("Encoder {codec} [").as_bytes()) {
        return Err(Error::Unsupported(format!(
            "the ffmpeg program {} has no {codec} encoder; install FFmpeg with that encoder",
            program.display()
        )));
    }
    Ok(())
}

/// Where the stream `index` of a Matroska source starts on the segment's
/// timeline, in nanoseconds: its first frame's presentation time
/// (`coreav_mkv::Packet::pts_ns`) plus `CodecDelay`, the samples the
/// decoder drops from its start (RFC 9559 5.1.4.1.24), as the remux
/// presents a copied stream (`docs/design/mkv-remux.md`, "Edit lists").
/// The decoder writes the frames from the first one on, so the encode would
/// otherwise start the stream at time 0. 0 for an MP4 source, whose leading
/// empty edit the decoder writes as silence, and for no stream named. A
/// start before 0, or one not known, is refused.
fn matroska_start<S: Source>(
    source: &mut S,
    index: Option<usize>,
    limits: &coreav_audio::Limits,
) -> Result<u64, Error> {
    use coreav_mkv as mkv;
    let audio_error = |e: coreav_audio::Error| Error::Audio(e);
    let mut prefix = [0u8; 4];
    if source.len().map_err(|e| audio_error(e.into()))? >= 4 {
        source
            .read_exact_at(0, &mut prefix)
            .map_err(|e| audio_error(e.into()))?;
    }
    let Some(index) = index.filter(|_| mkv::sniff(&prefix)) else {
        return Ok(0);
    };
    let segment = mkv::probe(&mut *source, &limits.matroska, &mkv::Allowlist::all())
        .map_err(|e| audio_error(e.into()))?;
    let Some(track) = segment.tracks.iter().position(|t| t.stream_index == index) else {
        return Ok(0);
    };
    for packet in segment.packets(&mut *source, &limits.matroska) {
        let packet = packet.map_err(|e| audio_error(e.into()))?;
        if packet.track != track {
            continue;
        }
        let delay = segment.tracks.get(track).map_or(0, |t| t.codec_delay);
        let start = packet
            .pts_ns(&segment)
            .and_then(|ns| ns.checked_add(i128::from(delay)));
        return match start.map(u64::try_from) {
            Some(Ok(ns)) => Ok(ns),
            Some(Err(_)) => Err(Error::Unsupported(
                "an audio stream that starts before time 0 (the audio encode does not trim it)"
                    .into(),
            )),
            None => Err(Error::Unsupported(
                "an audio stream whose start time is not known".into(),
            )),
        };
    }
    Ok(0)
}

/// `ns` nanoseconds in ticks of `rate` per second, to the nearest tick.
fn ticks(ns: u64, rate: u32) -> u64 {
    let ticks = (u128::from(ns) * u128::from(rate) + 500_000_000) / 1_000_000_000;
    u64::try_from(ticks).unwrap_or(u64::MAX)
}

/// The `Opus` sample entry with its `dOps` box (opus-codec.org
/// "Encapsulation of Opus in ISO Base Media File Format", 4.3.1 and 4.3.2):
/// `channels` output channels, channel mapping family 0 (mono or stereo),
/// the pre-skip, the source's sample rate as the input rate and no gain.
pub fn sample_entry(channels: u8, pre_skip: u16, input_rate: u32) -> Result<Vec<u8>, Error> {
    let mut payload = vec![0, channels];
    payload.extend_from_slice(&pre_skip.to_be_bytes());
    payload.extend_from_slice(&input_rate.to_be_bytes());
    payload.extend_from_slice(&0i16.to_be_bytes());
    payload.push(0);
    audio_sample_entry(
        FourCc(*b"Opus"),
        u16::from(channels),
        16,
        SAMPLE_RATE,
        &[RawBox {
            kind: FourCc(*b"dOps"),
            payload,
        }],
    )
    .map_err(|e| encoder_error(format!("the Opus sample entry: {e}")))
}

/// The `mp4a` sample entry of AAC (ISO/IEC 14496-14 5.6): `channels`
/// channels at `rate` Hz, with an `esds` box holding `config`, the
/// stream's `AudioSpecificConfig`.
pub fn aac_sample_entry(config: &[u8], rate: u32, channels: u8) -> Result<Vec<u8>, Error> {
    audio_sample_entry(
        FourCc(*b"mp4a"),
        u16::from(channels),
        16,
        rate,
        &[RawBox {
            kind: FourCc(*b"esds"),
            payload: esds(config),
        }],
    )
    .map_err(|e| encoder_error(format!("the AAC sample entry: {e}")))
}

fn encoder_error(message: String) -> Error {
    Error::Encode(coreav_decode::Error::new(ErrorKind::Backend, message))
}

struct TimedSource<S> {
    source: S,
    deadline: Instant,
}

impl<S> TimedSource<S> {
    fn check(&self) -> Result<(), coreav_isobmff::Error> {
        if Instant::now() >= self.deadline {
            return Err(coreav_isobmff::Error::Io(std::io::ErrorKind::TimedOut));
        }
        Ok(())
    }
}

impl<S: Source> Source for TimedSource<S> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        self.check()?;
        let len = self.source.len()?;
        self.check()?;
        Ok(len)
    }

    fn read_exact_at(
        &mut self,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<(), coreav_isobmff::Error> {
        self.check()?;
        self.source.read_exact_at(offset, bytes)?;
        self.check()
    }
}

fn checked_readback<T, E: std::fmt::Display>(
    result: Result<T, E>,
    deadline: Instant,
) -> Result<T, Error> {
    if Instant::now() >= deadline {
        return Err(Error::Decode(coreav_decode::Error::new(
            ErrorKind::Timeout,
            "the audio encode took longer than its timeout",
        )));
    }
    result.map_err(|error| encoder_error(error.to_string()))
}

/// The decoded samples' way to the encoder: it starts the encoder once the
/// decoder names the output's channels and rate.
struct Feed<'a> {
    encode: &'a Encode<'a>,
    launcher: Option<&'a ExecLauncher>,
    deadline: Instant,
    /// Where the encoder's output goes: the Opus packets, or the AAC
    /// encoder's Matroska.
    spool: &'a Spool,
    max_spool_bytes: u64,
    started: Option<Started>,
    /// The error that stopped the decode, which the decoder reports only
    /// as an output error.
    error: Option<Error>,
}

struct Started {
    process: Process<Output>,
    channels: u8,
    /// The samples' rate in Hz.
    rate: u32,
    bitrate: Option<u32>,
    /// Samples per channel sent to the encoder.
    sent: Arc<AtomicU64>,
    #[cfg(feature = "input-capture")]
    capture: Option<CaptureStarted>,
}

#[cfg(feature = "input-capture")]
struct CaptureStarted {
    pending: crate::capture::Pending,
    encoder_sha256: String,
    argv: Vec<String>,
    launcher_argv: Vec<String>,
    channel_mask: u32,
}

impl Feed<'_> {
    /// Records `error` and returns what stops the decoder.
    fn stop(&mut self, error: Error) -> coreav_audio::Error {
        self.error.get_or_insert(error);
        coreav_audio::Error::Output(std::io::Error::other(format!(
            "the {} encode stopped",
            self.encode.codec()
        )))
    }

    fn start_encoder(&mut self, format: &OutputFormat) -> Result<Started, Error> {
        let codec = self.encode.codec();
        let channels = u8::try_from(format.channels)
            .ok()
            .filter(|c| matches!(c, 1 | 2))
            .ok_or_else(|| {
                Error::Unsupported(format!(
                    "{codec} encoding of {} channels: CoreAV encodes mono and stereo",
                    format.channels
                ))
            })?;
        if format.format != SampleFormat::F32 {
            return Err(encoder_error("the decoder's output is not float".into()));
        }
        let rate = format.sample_rate;
        let sent = Arc::new(AtomicU64::new(0));
        let file = BufWriter::with_capacity(1 << 16, self.spool.handle()?);
        let (bitrate, args, memory, output) = match self.encode {
            Encode::Opus(settings) => {
                if rate != SAMPLE_RATE {
                    return Err(encoder_error("the decoder's output is not 48 kHz".into()));
                }
                let bitrate = settings
                    .bitrate
                    .unwrap_or_else(|| default_bitrate(channels));
                let packets = Packets {
                    reader: PacketReader::new(),
                    sent: Arc::clone(&sent),
                    file,
                    bytes: 0,
                    max_bytes: self.max_spool_bytes,
                    sizes: Vec::new(),
                };
                (
                    Some(bitrate),
                    encode_args(channels, bitrate),
                    ENCODER_MEMORY,
                    Output::Opus(packets),
                )
            }
            Encode::Aac(settings, _) => {
                if !aac::takes_rate(rate) {
                    return Err(Error::Unsupported(format!(
                        "AAC encoding at {rate} Hz: CoreAV encodes AAC-LC at the rates of 8 to \
                         64 kHz that AAC defines, and does not resample"
                    )));
                }
                let stdout = Stdout {
                    codec: External::Aac,
                    sent: Arc::clone(&sent),
                    file,
                    bytes: 0,
                    max_bytes: self.max_spool_bytes,
                };
                (
                    settings.bitrate,
                    aac::encode_args(rate, channels, settings.bitrate),
                    AAC_ENCODER_MEMORY,
                    Output::External(stdout),
                )
            }
            Encode::Mp3(settings, _) => {
                if !mp3::takes_rate(rate) {
                    return Err(Error::Unsupported(format!(
                        "MP3 encoding at {rate} Hz without resampling"
                    )));
                }
                let bitrate = match settings.mode {
                    mp3::Mode::Constant(rate) => rate,
                    mp3::Mode::Quality(_) => None,
                };
                (
                    bitrate,
                    mp3::encode_args(rate, channels, settings.mode),
                    AAC_ENCODER_MEMORY,
                    Output::External(Stdout {
                        codec: External::Mp3,
                        sent: Arc::clone(&sent),
                        file,
                        bytes: 0,
                        max_bytes: self.max_spool_bytes,
                    }),
                )
            }
        };
        let program = self.encode.program();
        #[cfg(feature = "input-capture")]
        let (mut capture, recording) = match self.encode {
            Encode::Aac(settings, _) if settings.input_capture.is_some() => {
                let target = settings
                    .input_capture
                    .as_ref()
                    .ok_or_else(|| encoder_error("no capture target".into()))?;
                let encoder_sha256 =
                    crate::capture::file_sha256(program).map_err(|e| Error::Spool(e.kind()))?;
                let argv = args
                    .iter()
                    .map(|arg| {
                        arg.to_str()
                            .map(str::to_owned)
                            .ok_or_else(|| encoder_error("non-UTF-8 encoder arguments".into()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let (recording, pending) = target.create().map_err(|e| Error::Spool(e.kind()))?;
                (
                    Some(CaptureStarted {
                        pending,
                        encoder_sha256,
                        argv,
                        launcher_argv: Vec::new(),
                        channel_mask: format.channel_mask,
                    }),
                    Some(recording),
                )
            }
            _ => (None, None),
        };
        let command = command(
            program,
            &args,
            HugePages::Kept,
            self.launcher,
            &EncoderLimits::new(memory),
            self.deadline,
        );
        #[cfg(feature = "input-capture")]
        if let Some(capture) = capture.as_mut() {
            capture.launcher_argv = command
                .get_args()
                .map(|arg| {
                    arg.to_str()
                        .map(str::to_owned)
                        .ok_or_else(|| encoder_error("non-UTF-8 launcher arguments".into()))
                })
                .collect::<Result<Vec<_>, _>>()?;
        }
        #[cfg(not(feature = "input-capture"))]
        let process = Process::spawn(
            self.encode.name(),
            "sample blocks",
            command,
            program,
            self.deadline,
            output,
        )
        .map_err(Error::Encode)?;
        #[cfg(feature = "input-capture")]
        let process = match recording {
            Some(recording) => Process::spawn_captured(
                self.encode.name(),
                command,
                program,
                self.deadline,
                output,
                recording,
            ),
            None => Process::spawn(
                self.encode.name(),
                "sample blocks",
                command,
                program,
                self.deadline,
                output,
            ),
        }
        .map_err(Error::Encode)?;
        Ok(Started {
            process,
            channels,
            rate,
            bitrate,
            sent,
            #[cfg(feature = "input-capture")]
            capture,
        })
    }
}

impl coreav_audio::Sink for Feed<'_> {
    fn start(&mut self, format: &OutputFormat) -> Result<(), coreav_audio::Error> {
        match self.start_encoder(format) {
            Ok(started) => {
                self.started = Some(started);
                Ok(())
            }
            Err(e) => Err(self.stop(e)),
        }
    }

    fn write(&mut self, samples: &[u8]) -> Result<(), coreav_audio::Error> {
        if Instant::now() >= self.deadline {
            return Err(self.stop(Error::Decode(coreav_decode::Error::new(
                ErrorKind::Timeout,
                "the transcode took longer than its timeout",
            ))));
        }
        let Some(started) = self.started.as_mut() else {
            return Err(self.stop(encoder_error("samples before the format".into())));
        };
        let frame = 4 * u64::from(started.channels);
        let count = u64::try_from(samples.len()).unwrap_or(u64::MAX) / frame;
        // Counted before the encoder can answer: its output is checked
        // against the samples sent.
        started.sent.fetch_add(count, Ordering::SeqCst);
        match started.process.enqueue(samples.to_vec()) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.stop(Error::Encode(e))),
        }
    }
}

/// The encoder's output on its reader thread.
enum Output {
    Opus(Packets),
    External(Stdout),
}

impl Sink for Output {
    fn write(&mut self, bytes: &[u8]) -> Result<(), coreav_decode::Error> {
        match self {
            Self::Opus(packets) => packets.write(bytes),
            Self::External(stdout) => stdout.write(bytes),
        }
    }
}

#[derive(Clone, Copy)]
enum External {
    Aac,
    Mp3,
}

struct Stdout {
    codec: External,
    sent: Arc<AtomicU64>,
    file: BufWriter<File>,
    bytes: u64,
    max_bytes: u64,
}

impl Sink for Stdout {
    fn write(&mut self, bytes: &[u8]) -> Result<(), coreav_decode::Error> {
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if self.bytes > self.max_bytes {
            return Err(coreav_decode::Error::new(
                ErrorKind::Limit,
                "the encoded audio is larger than the spool file limit",
            ));
        }
        // At most what the samples sent so far make: an encoder that
        // writes more is refused before it fills the disk.
        let most = match self.codec {
            External::Aac => aac::most_bytes(self.sent.load(Ordering::SeqCst)),
            External::Mp3 => mp3::most_bytes(self.sent.load(Ordering::SeqCst)),
        };
        if self.bytes > most {
            return Err(coreav_decode::Error::new(
                ErrorKind::Backend,
                "the audio encoder wrote more than its samples make",
            ));
        }
        self.file.write_all(bytes).map_err(|e| {
            coreav_decode::Error::new(ErrorKind::Io, format!("spool file: {}", e.kind()))
        })
    }
}

/// The Opus encoder's output on its reader thread: checked by the packet
/// reader, each packet into the spool file.
struct Packets {
    reader: PacketReader,
    sent: Arc<AtomicU64>,
    file: BufWriter<File>,
    bytes: u64,
    max_bytes: u64,
    sizes: Vec<u32>,
}

/// An error of the packet reader or of the spool file.
struct Refused(coreav_decode::Error);

impl From<ReadError> for Refused {
    fn from(e: ReadError) -> Self {
        Self(coreav_decode::Error::new(ErrorKind::Backend, e.to_string()))
    }
}

impl Sink for Packets {
    fn write(&mut self, bytes: &[u8]) -> Result<(), coreav_decode::Error> {
        let Self {
            reader,
            sent,
            file,
            bytes: written,
            max_bytes,
            sizes,
        } = self;
        // At most the packets of the samples sent so far: an encoder that
        // writes more is refused before it fills the disk.
        let most = packets_for(MAX_PRE_SKIP, sent.load(Ordering::SeqCst));
        reader
            .push::<Refused>(bytes, &mut |packet| {
                let refuse =
                    |kind, message: &str| Refused(coreav_decode::Error::new(kind, message));
                if u64::try_from(sizes.len()).unwrap_or(u64::MAX) >= most {
                    return Err(refuse(
                        ErrorKind::Backend,
                        "the Opus encoder wrote more packets than its samples make",
                    ));
                }
                let size = u32::try_from(packet.len()).unwrap_or(u32::MAX);
                *written = written.saturating_add(u64::from(size));
                if *written > *max_bytes {
                    return Err(refuse(
                        ErrorKind::Limit,
                        "the encoded audio is larger than the spool file limit",
                    ));
                }
                file.write_all(packet)
                    .map_err(|e| refuse(ErrorKind::Io, &format!("spool file: {}", e.kind())))?;
                sizes.push(size);
                Ok(())
            })
            .map_err(|Refused(e)| e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::time::Duration;

    #[test]
    fn readback_checks_the_deadline_inside_parser_reads() {
        use std::cell::Cell;
        struct Slow<'a> {
            source: coreav_isobmff::SliceSource<'a>,
            reads: &'a Cell<usize>,
        }
        impl Source for Slow<'_> {
            fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
                self.source.len()
            }
            fn read_exact_at(
                &mut self,
                offset: u64,
                bytes: &mut [u8],
            ) -> Result<(), coreav_isobmff::Error> {
                self.reads.set(self.reads.get() + 1);
                if self.reads.get() == 1 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                self.source.read_exact_at(offset, bytes)
            }
        }
        let mut frame = vec![0xff, 0xfb, 0x54, 0xc4];
        frame.resize(192, 0);
        let bytes = crate::mp3::tests::fixture(
            "A_MPEG/L3",
            23_020_833,
            &[frame.clone(), frame],
            &[None, Some(979_167)],
        );
        let reads = Cell::new(0);
        let deadline = Instant::now() + std::time::Duration::from_millis(20);
        let source = TimedSource {
            source: Slow {
                source: coreav_isobmff::SliceSource::new(&bytes),
                reads: &reads,
            },
            deadline,
        };
        let expected = mp3::Expected {
            sample_rate: 48000,
            channels: 1,
            samples: 1152,
            guard: 0,
            variable: false,
        };
        let result = checked_readback(
            mp3::read_output(source, &expected, &coreav_mkv::Limits::default()),
            deadline,
        );
        assert!(matches!(result, Err(Error::Decode(ref e)) if e.kind == ErrorKind::Timeout));
        assert!(
            reads.get() <= 1,
            "readback continued after the deadline: {} reads",
            reads.get()
        );
    }

    /// Writes `body` at `path` as an executable `/bin/sh` script. A child
    /// process writes it, so this process never holds it open for writing:
    /// a child another test forks would inherit that descriptor until its
    /// exec, and running the script meanwhile fails with ETXTBSY.
    #[cfg(unix)]
    fn write_script(path: &Path, body: &str) {
        let mut sh = std::process::Command::new("/bin/sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(path)
            .env("PATH", "/usr/bin:/bin")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        sh.stdin
            .take()
            .unwrap()
            .write_all(format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n").as_bytes())
            .unwrap();
        assert!(sh.wait().unwrap().success());
    }

    /// Runs the Opus encoder process's reading side on `body`, a `/bin/sh`
    /// script that stands in for the encoder, sending it `samples` stereo
    /// samples of silence. Returns the packet sizes, or the error, and how
    /// long it took.
    #[cfg(unix)]
    fn stand_in(
        name: &str,
        body: &str,
        samples: usize,
    ) -> (Result<Vec<u32>, coreav_decode::Error>, Duration) {
        let dir = std::env::temp_dir().join(format!("coreav-opus-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("encoder.sh");
        write_script(&script, body);
        let spool = Spool::create(&dir.join("out.mp4"), "opus").unwrap();
        let start = Instant::now();
        let deadline = start + Duration::from_secs(5);
        let sent = Arc::new(AtomicU64::new(0));
        let packets = Packets {
            reader: PacketReader::new(),
            sent: Arc::clone(&sent),
            file: BufWriter::new(spool.handle().unwrap()),
            bytes: 0,
            max_bytes: 1 << 20,
            sizes: Vec::new(),
        };
        let command = command(
            &script,
            &encode_args(2, 96_000),
            HugePages::Kept,
            None,
            &EncoderLimits::new(1 << 30),
            deadline,
        );
        let result = Process::spawn(
            "the Opus encoder",
            "sample blocks",
            command,
            &script,
            deadline,
            packets,
        )
        .and_then(|mut process| {
            sent.fetch_add(u64::try_from(samples).unwrap(), Ordering::SeqCst);
            process.enqueue(vec![0; samples * 8])?;
            process.finish()
        })
        .and_then(|Packets { reader, sizes, .. }| {
            reader
                .finish()
                .map(|_| sizes)
                .map_err(|e| coreav_decode::Error::new(ErrorKind::Backend, e.to_string()))
        });
        drop(spool);
        std::fs::remove_dir_all(&dir).unwrap();
        (result, start.elapsed())
    }

    /// The header: `Opus`, then a pre-skip of 312 (`printf` octal).
    #[cfg(unix)]
    const HEADER: &str = r"printf 'Opus\070\001'";

    /// One record: a 3-byte packet of one 20 ms SILK frame (configuration
    /// 1, code 0).
    #[cfg(unix)]
    const RECORD: &str = r"printf '\003\000\010\000\000'";

    #[cfg(unix)]
    #[test]
    fn an_encoder_that_writes_packets_for_its_samples_is_read() {
        let body = format!("cat > /dev/null; {HEADER}; {RECORD}; {RECORD}");
        let (result, _) = stand_in("valid", &body, 960);
        assert_eq!(result.unwrap(), vec![3, 3]);
    }

    #[cfg(unix)]
    #[test]
    fn an_encoder_that_floods_packets_is_stopped_at_once() {
        let body = format!("{HEADER}; while :; do {RECORD}; done");
        let (result, took) = stand_in("flood", &body, 960);
        let e = result.unwrap_err();
        assert!(
            e.message.contains("more packets than its samples make"),
            "{e:?}"
        );
        assert!(took < Duration::from_secs(4), "{took:?}");
    }

    #[cfg(unix)]
    #[test]
    fn invalid_output_is_refused() {
        for (name, body, says) in [
            (
                "header",
                "cat > /dev/null; printf 'OggS\\000\\000'".to_owned(),
                "header",
            ),
            (
                "empty",
                format!("cat > /dev/null; {HEADER}; printf '\\000\\000'"),
                "of 0 bytes",
            ),
            (
                "short",
                format!("cat > /dev/null; {HEADER}; printf '\\003\\000\\000\\000\\000'"),
                "not 960",
            ),
            (
                "cut",
                format!("cat > /dev/null; {HEADER}; printf '\\003\\000\\010'"),
                "ends inside",
            ),
            ("silent", "cat > /dev/null".to_owned(), "ends inside"),
        ] {
            let (result, _) = stand_in(name, &body, 960);
            let e = result.unwrap_err();
            assert!(e.message.contains(says), "{name}: {e:?}");
        }
    }

    #[test]
    fn the_sample_entry_is_opus_with_its_dops() {
        let entry = sample_entry(2, 312, 44_100).unwrap();
        assert_eq!(&entry[4..8], b"Opus");
        // The audio sample entry's channel count and rate.
        assert_eq!(&entry[24..26], &[0, 2]);
        assert_eq!(&entry[32..36], &(48_000u32 << 16).to_be_bytes());
        let dops = &entry[36..];
        assert_eq!(&dops[4..8], b"dOps");
        assert_eq!(&dops[8..], &[0, 2, 0x01, 0x38, 0, 0, 0xac, 0x44, 0, 0, 0]);
        assert_eq!(
            usize::try_from(u32::from_be_bytes(dops[..4].try_into().unwrap())).unwrap(),
            dops.len()
        );
    }

    /// A directory of its own for `name`, with `body` in it as a `/bin/sh`
    /// script that stands in for the `ffmpeg` program.
    #[cfg(unix)]
    fn ffmpeg_stand_in(name: &str, body: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("coreav-aac-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("ffmpeg");
        write_script(&script, body);
        (dir, script)
    }

    #[cfg(unix)]
    #[test]
    fn an_ffmpeg_without_the_aac_encoder_is_refused() {
        let deadline = Instant::now() + Duration::from_secs(5);
        for (name, body, says) in [
            (
                "with",
                "echo 'Encoder aac [AAC (Advanced Audio Coding)]:'",
                None,
            ),
            (
                "without",
                "echo \"Codec 'aac' is not recognized by FFmpeg.\"",
                Some("has no aac encoder"),
            ),
            ("silent", "true", Some("has no aac encoder")),
            ("failing", "echo 'Encoder aac ['; exit 1", Some("exit")),
        ] {
            let (dir, script) = ffmpeg_stand_in(name, body);
            let result = probe_encoder(&script, None, deadline, "aac");
            std::fs::remove_dir_all(&dir).unwrap();
            match (result, says) {
                (Ok(()), None) => {}
                (Err(e), Some(says)) => assert!(e.to_string().contains(says), "{name}: {e}"),
                (result, _) => panic!("{name}: {result:?}"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_aac_encoder_that_floods_its_output_is_stopped_at_once() {
        let (dir, script) = ffmpeg_stand_in("flood", "exec yes");
        let spool = Spool::create(&dir.join("out.mp4"), "mkv").unwrap();
        let start = Instant::now();
        let deadline = start + Duration::from_secs(5);
        let output = Output::External(Stdout {
            codec: External::Aac,
            sent: Arc::new(AtomicU64::new(1024)),
            file: BufWriter::new(spool.handle().unwrap()),
            bytes: 0,
            max_bytes: 1 << 30,
        });
        let command = command(
            &script,
            &aac::encode_args(48_000, 2, None),
            HugePages::Kept,
            None,
            &EncoderLimits::new(1 << 30),
            deadline,
        );
        let result = Process::spawn(
            "the AAC encoder",
            "sample blocks",
            command,
            &script,
            deadline,
            output,
        )
        .and_then(|mut process| {
            process.enqueue(vec![0; 8192])?;
            process.finish()
        })
        .map(|_| ());
        drop(spool);
        std::fs::remove_dir_all(&dir).unwrap();
        let e = result.unwrap_err();
        assert!(e.message.contains("more than its samples make"), "{e:?}");
        assert!(start.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn the_aac_sample_entry_is_mp4a_with_its_esds() {
        let entry = aac_sample_entry(&[0x12, 0x10], 44_100, 2).unwrap();
        assert_eq!(&entry[4..8], b"mp4a");
        assert_eq!(&entry[24..26], &[0, 2]);
        assert_eq!(&entry[32..36], &(44_100u32 << 16).to_be_bytes());
        let esds = &entry[36..];
        assert_eq!(&esds[4..8], b"esds");
        assert!(
            esds.windows(7)
                .any(|w| w == [0x05, 0x80, 0x80, 0x80, 0x02, 0x12, 0x10])
        );
        assert_eq!(
            usize::try_from(u32::from_be_bytes(esds[..4].try_into().unwrap())).unwrap(),
            esds.len()
        );
    }
}
