//! Encoded audio of a transcode (`docs/design/transcode.md`, "Opus audio"
//! and "AAC audio"; policy record 0042): one audio stream of the source
//! decoded by `coreav-audio` to 32-bit float samples, encoded by an encoder
//! process and spooled for the MP4 writer, which writes it in the source
//! stream's place. The Opus encoder is libopus in a process of its own
//! (`coreav_worker::opus`, through the sandbox launcher), on samples
//! resampled to 48 kHz; the AAC encoder is the operator's `ffmpeg` program
//! (`crate::aac`), on samples at the stream's own rate.
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
use crate::encoder::{EncoderLimits, Process, Sink, command};
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
pub(crate) enum Encode<'a> {
    /// libopus, in the encoder process [`OpusAudio::program`] runs.
    Opus(&'a OpusAudio),
    /// FFmpeg's `aac` encoder, by this `ffmpeg` program.
    Aac(&'a AacAudio, PathBuf),
}

impl Encode<'_> {
    fn codec(&self) -> &'static str {
        match self {
            Self::Opus(_) => "Opus",
            Self::Aac(..) => "AAC",
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Opus(_) => "the Opus encoder",
            Self::Aac(..) => "the AAC encoder",
        }
    }

    fn program(&self) -> &Path {
        match self {
            Self::Opus(settings) => &settings.program,
            Self::Aac(_, program) => program,
        }
    }
}

/// The encoded audio stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSummary {
    /// The source stream's position among all of its streams; the output
    /// keeps its place.
    pub stream_index: usize,
    /// The source stream's codec.
    pub codec: coreav_audio::Codec,
    /// The source stream's sample rate in Hz.
    pub sample_rate: u32,
    /// The source stream's channels.
    pub source_channels: u16,
    /// The encoder: `libopus`, or `aac` (FFmpeg's).
    pub encoder: &'static str,
    /// The output's channels: 1 or 2, the source's.
    pub channels: u8,
    /// The bit rate asked of the encoder, in bits a second; `None` for the
    /// `aac` encoder's own default.
    pub bitrate: Option<u32>,
    /// Samples per channel presented, at the output's rate (48 kHz for
    /// Opus, the source's for AAC): the decoded stream's.
    pub samples: u64,
    /// Packets written: Opus packets of 20 ms, or AAC frames of 1,024
    /// samples.
    pub packets: u64,
    /// The encoder's delay in samples at the output's rate: not presented
    /// (the edit's media time).
    pub pre_skip: u16,
}

/// The encoded audio, ready for the MP4 writer.
pub(crate) struct EncodedAudio {
    pub(crate) summary: AudioSummary,
    /// The `Opus` or `mp4a` sample entry.
    pub(crate) entry: Vec<u8>,
    /// The media timescale: the output's sample rate.
    pub(crate) timescale: u32,
    /// The track's roll distance, in samples of the track.
    pub(crate) roll: u16,
    /// The packets, back to back in `spool`.
    pub(crate) samples: Vec<AudioSample>,
    pub(crate) spool: Spool,
}

/// Decodes the stream `encode` names from `source` and encodes it. The
/// encoder runs through `launcher` (or directly when it is `None`; for
/// tests) and is killed at `deadline`; its packets go to a spool file next
/// to `output`, at most `max_spool_bytes` of them.
pub(crate) fn encode<S: Source>(
    source: S,
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
    if !program.as_os_str().as_encoded_bytes().contains(&b'/') {
        return Err(encoder_error(format!(
            "the {} encoder program must be a path, not a name: {}",
            encode.codec(),
            program.display()
        )));
    }
    if let Encode::Aac(_, program) = encode {
        probe_aac(program, launcher, deadline)?;
    }
    #[cfg(feature = "aac")]
    let mut source = source;
    #[cfg(feature = "aac")]
    let stream = without_aac(&mut source, requested, limits)?;
    #[cfg(not(feature = "aac"))]
    let stream = requested;
    let options = coreav_audio::Options {
        stream,
        format: SampleFormat::F32,
        threads: 1,
        sample_rate: match encode {
            Encode::Opus(_) => Some(SAMPLE_RATE),
            Encode::Aac(..) => None,
        },
        channels: None,
    };
    let spool = Spool::create(
        output,
        match encode {
            Encode::Opus(_) => "opus",
            Encode::Aac(..) => "aac",
        },
    )?;
    // The AAC encoder's Matroska, read back once it has ended.
    let raw = match encode {
        Encode::Opus(_) => None,
        Encode::Aac(..) => Some(Spool::create(output, "mkv")?),
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
    let decoded = coreav_audio::decode(source, &options, limits, &mut feed);
    let Feed { started, error, .. } = feed;
    let summary = match (decoded, error) {
        (_, Some(e)) => return Err(e),
        (Err(coreav_audio::Error::NoSuchStream), None) if requested.is_none() => {
            return Err(Error::Unsupported(
                "the source has no audio stream CoreAV decodes (FLAC or MP3)".into(),
            ));
        }
        (Err(e), None) => return Err(Error::Audio(e)),
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
        process,
        channels,
        rate,
        bitrate,
        sent,
    } = started;
    let samples = sent.load(Ordering::SeqCst);
    let (sizes, pre_skip, entry, roll) = match process.finish().map_err(Error::Encode)? {
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
            (sizes, pre_skip, entry, ROLL)
        }
        Output::Aac(Stdout { mut file, .. }) => {
            file.flush().map_err(|e| Error::Spool(e.kind()))?;
            drop(file);
            let raw = raw.ok_or_else(|| encoder_error("no AAC spool file".into()))?;
            let mut written =
                BufferedSource::new(FileSource::from_file(raw.handle()?).map_err(spool_read)?);
            let expected = aac::Expected {
                sample_rate: rate,
                channels,
                samples,
            };
            let read = aac::read_output(&mut written, &expected, &limits.matroska)
                .map_err(|e| encoder_error(e.to_string()))?;
            let mut file = BufWriter::with_capacity(1 << 16, spool.handle()?);
            let mut frame = Vec::new();
            for f in &read.frames {
                frame.resize(usize::try_from(f.size).unwrap_or(usize::MAX), 0);
                written
                    .read_exact_at(f.offset, &mut frame)
                    .map_err(spool_read)?;
                file.write_all(&frame).map_err(|e| Error::Spool(e.kind()))?;
            }
            file.flush().map_err(|e| Error::Spool(e.kind()))?;
            let pre_skip = u16::try_from(read.priming)
                .map_err(|_| encoder_error("an AAC delay above 65535 samples".into()))?;
            let entry = aac_sample_entry(&read.config, rate, channels)?;
            (
                read.frames.iter().map(|f| f.size).collect(),
                pre_skip,
                entry,
                AAC_ROLL,
            )
        }
    };
    let duration = match encode {
        Encode::Opus(_) => FRAME_SAMPLES,
        Encode::Aac(..) => aac::FRAME_SAMPLES,
    };
    Ok(EncodedAudio {
        summary: AudioSummary {
            stream_index: summary.stream_index,
            codec: summary.stream.codec,
            sample_rate: summary.stream.sample_rate,
            source_channels: summary.stream.channels,
            encoder: match encode {
                Encode::Opus(_) => "libopus",
                Encode::Aac(..) => "aac",
            },
            channels,
            bitrate,
            samples,
            packets: u64::try_from(sizes.len()).unwrap_or(u64::MAX),
            pre_skip,
        },
        entry,
        timescale: rate,
        roll,
        samples: sizes
            .into_iter()
            .map(|size| AudioSample { size, duration })
            .collect(),
        spool,
    })
}

/// Asks the `ffmpeg` program about its `aac` encoder, as it runs as the
/// encoder (through `launcher`, killed at `deadline`): one without it is
/// refused before the source is decoded.
fn probe_aac(
    program: &Path,
    launcher: Option<&ExecLauncher>,
    deadline: Instant,
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
        &aac::probe_args(),
        HugePages::Kept,
        launcher,
        &EncoderLimits::new(AAC_ENCODER_MEMORY),
        deadline,
    );
    let Answer(answer) = Process::spawn(
        "the AAC encoder",
        "sample blocks",
        command,
        program,
        deadline,
        Answer(Vec::new()),
    )
    .and_then(Process::finish)
    .map_err(Error::Encode)?;
    if !aac::has_encoder(&answer) {
        return Err(Error::Unsupported(format!(
            "the ffmpeg program {} has no aac encoder: AAC audio needs FFmpeg's built-in aac \
             encoder; name another ffmpeg with --ffmpeg",
            program.display()
        )));
    }
    Ok(())
}

/// The stream to decode in a build that decodes AAC: the one a build
/// without the feature decodes, AAC being no source of the Opus or the AAC
/// encode (`docs/design/transcode.md`, "Opus audio" and "AAC audio"). A named AAC stream is
/// refused as the decoder refuses it without the feature; with none named,
/// the first audio stream of FLAC or MP3, else the first audio stream (one
/// of AAC refused here, any other refused by the decoder).
#[cfg(feature = "aac")]
fn without_aac<S: Source>(
    source: &mut S,
    stream: Option<usize>,
    limits: &coreav_audio::Limits,
) -> Result<Option<usize>, Error> {
    use coreav_isobmff as mp4;
    use coreav_mkv as mkv;
    let audio_error = |e: coreav_audio::Error| Error::Audio(e);
    let mut prefix = [0u8; 4];
    if source.len().map_err(|e| audio_error(e.into()))? >= 4 {
        source
            .read_exact_at(0, &mut prefix)
            .map_err(|e| audio_error(e.into()))?;
    }
    // Each audio stream: its index, whether it is AAC, and whether it is
    // FLAC or MP3.
    let audio: Vec<(usize, bool, bool)> = if mkv::sniff(&prefix) {
        mkv::probe(&mut *source, &limits.matroska, &mkv::Allowlist::all())
            .map_err(|e| audio_error(e.into()))?
            .tracks
            .iter()
            .filter(|t| t.kind == mkv::TrackKind::Audio)
            .map(|t| {
                let aac = t.codec == mkv::Codec::Aac;
                (
                    t.stream_index,
                    aac,
                    matches!(t.codec, mkv::Codec::Flac | mkv::Codec::Mp3),
                )
            })
            .collect()
    } else {
        // The demuxer's allowlist of `coreav-audio`, so that the streams
        // are the decoder's.
        let allowlist = mp4::Allowlist::new(&[
            mp4::Codec::Aac,
            mp4::Codec::Mp3,
            mp4::Codec::Opus,
            mp4::Codec::Flac,
            mp4::Codec::Pcm,
        ]);
        mp4::probe(&mut *source, &limits.isobmff, &allowlist)
            .map_err(|e| audio_error(e.into()))?
            .tracks
            .iter()
            .filter(|t| t.kind == mp4::TrackKind::Audio)
            .map(|t| {
                let codec = t.description.codec;
                let aac = codec == mp4::Codec::Aac;
                (
                    t.stream_index,
                    aac,
                    matches!(codec, mp4::Codec::Flac | mp4::Codec::Mp3),
                )
            })
            .collect()
    };
    let chosen = match stream {
        Some(index) => audio.iter().find(|&&(i, ..)| i == index),
        None => audio.iter().find(|&&(.., taken)| taken).or(audio.first()),
    };
    match chosen {
        Some((_, true, _)) => Err(audio_error(coreav_audio::Error::CodecNotDecoded(
            "aac".into(),
        ))),
        Some(&(index, ..)) => Ok(Some(index)),
        None => Ok(stream),
    }
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
                    sent: Arc::clone(&sent),
                    file,
                    bytes: 0,
                    max_bytes: self.max_spool_bytes,
                };
                (
                    settings.bitrate,
                    aac::encode_args(rate, channels, settings.bitrate),
                    AAC_ENCODER_MEMORY,
                    Output::Aac(stdout),
                )
            }
        };
        let program = self.encode.program();
        let command = command(
            program,
            &args,
            HugePages::Kept,
            self.launcher,
            &EncoderLimits::new(memory),
            self.deadline,
        );
        let process = Process::spawn(
            self.encode.name(),
            "sample blocks",
            command,
            program,
            self.deadline,
            output,
        )
        .map_err(Error::Encode)?;
        Ok(Started {
            process,
            channels,
            rate,
            bitrate,
            sent,
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
    Aac(Stdout),
}

impl Sink for Output {
    fn write(&mut self, bytes: &[u8]) -> Result<(), coreav_decode::Error> {
        match self {
            Self::Opus(packets) => packets.write(bytes),
            Self::Aac(stdout) => stdout.write(bytes),
        }
    }
}

/// The AAC encoder's Matroska, into its spool file as it comes: read back
/// by [`aac::read_output`] once the encoder has ended.
struct Stdout {
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
        if self.bytes > aac::most_bytes(self.sent.load(Ordering::SeqCst)) {
            return Err(coreav_decode::Error::new(
                ErrorKind::Backend,
                "the AAC encoder wrote more than its samples make",
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
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("coreav-opus-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("encoder.sh");
        std::fs::write(&script, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
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
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("coreav-aac-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("ffmpeg");
        std::fs::write(&script, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
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
            let result = probe_aac(&script, None, deadline);
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
        let output = Output::Aac(Stdout {
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
