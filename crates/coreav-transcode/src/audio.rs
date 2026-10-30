//! Opus audio of a transcode (`docs/design/transcode.md`, "Opus audio";
//! policy record 0042): one audio stream of the source decoded by
//! `coreav-audio` to 32-bit float samples at 48 kHz, encoded by the Opus
//! encoder process (`coreav_worker::opus`: libopus in a process of its
//! own, through the sandbox launcher) and spooled for the MP4 writer, which
//! writes it as Opus in ISO BMFF in the source stream's place.
//!
//! The audio is encoded before the video, so that a stream CoreAV does not
//! decode, or one of more than two channels, is refused before the video
//! encoder starts.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use coreav_audio::{OutputFormat, SampleFormat, Warning};
use coreav_decode::ErrorKind;
use coreav_isobmff::{FourCc, Source};
use coreav_isobmff_write::AudioSample;
use coreav_isobmff_write::mux::{RawBox, audio_sample_entry};
use coreav_worker::opus::{
    FRAME_SAMPLES, MAX_BITRATE, MAX_PRE_SKIP, MIN_BITRATE, PacketReader, ReadError, encode_args,
    packets_for,
};
use coreav_worker::{ExecLauncher, HugePages};

use crate::encoder::{EncoderLimits, Process, Sink, command};
use crate::pipeline::{Error, Spool};

/// The sample rate of every Opus stream (RFC 7845 section 5.1): the
/// decoded audio is resampled to it, and it is the track's timescale.
pub const SAMPLE_RATE: u32 = 48_000;

/// The roll distance of the track, in samples: 80 ms of 20 ms packets, the
/// pre-roll a decoder needs after a random access (opus-codec.org
/// "Encapsulation of Opus in ISO Base Media File Format", 4.3.6.2).
pub const ROLL: u16 = 4;

/// The Opus encoder process's address space: libopus needs a few
/// megabytes; the rest is the `coreav` program's own mappings.
const ENCODER_MEMORY: u64 = 1 << 30;

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
    /// The output's channels: 1 or 2, the source's.
    pub channels: u8,
    /// The bit rate asked of the encoder, in bits a second.
    pub bitrate: u32,
    /// Samples per channel at 48 kHz presented: the decoded stream's.
    pub samples: u64,
    /// Opus packets written, 20 ms each.
    pub packets: u64,
    /// The encoder's delay in samples: not presented (the edit's media time).
    pub pre_skip: u16,
}

/// The encoded audio, ready for the MP4 writer.
pub(crate) struct EncodedAudio {
    pub(crate) summary: AudioSummary,
    /// The `Opus` sample entry.
    pub(crate) entry: Vec<u8>,
    /// The packets, 960 ticks each, back to back in `spool`.
    pub(crate) samples: Vec<AudioSample>,
    pub(crate) spool: Spool,
}

/// Decodes the stream `settings` names from `source` and encodes it. The
/// Opus encoder runs through `launcher` (or directly when it is `None`;
/// for tests) and is killed at `deadline`; its packets go to `spool`, at
/// most `max_spool_bytes` of them.
pub(crate) fn encode<S: Source>(
    source: S,
    settings: &OpusAudio,
    limits: &coreav_audio::Limits,
    launcher: Option<&ExecLauncher>,
    deadline: Instant,
    spool: Spool,
    max_spool_bytes: u64,
) -> Result<EncodedAudio, Error> {
    if settings
        .bitrate
        .is_some_and(|b| !(MIN_BITRATE..=MAX_BITRATE).contains(&b))
    {
        return Err(Error::Unsupported(format!(
            "an Opus bit rate outside {MIN_BITRATE} to {MAX_BITRATE} bits a second"
        )));
    }
    let options = coreav_audio::Options {
        stream: settings.stream,
        format: SampleFormat::F32,
        threads: 1,
        sample_rate: Some(SAMPLE_RATE),
        channels: None,
    };
    let mut feed = Feed {
        settings,
        launcher,
        deadline,
        spool: &spool,
        max_spool_bytes,
        started: None,
        error: None,
    };
    let decoded = coreav_audio::decode(source, &options, limits, &mut feed);
    let Feed { started, error, .. } = feed;
    let summary = match (decoded, error) {
        (_, Some(e)) => return Err(e),
        (Err(coreav_audio::Error::NoSuchStream), None) if settings.stream.is_none() => {
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
        bitrate,
        sent,
    } = started;
    let samples = sent.load(Ordering::SeqCst);
    let packets = process.finish().map_err(Error::Encode)?;
    let Packets {
        reader,
        mut file,
        sizes,
        ..
    } = packets;
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
    Ok(EncodedAudio {
        summary: AudioSummary {
            stream_index: summary.stream_index,
            codec: summary.stream.codec,
            sample_rate: summary.stream.sample_rate,
            source_channels: summary.stream.channels,
            channels,
            bitrate,
            samples,
            packets: count,
            pre_skip,
        },
        entry,
        samples: sizes
            .into_iter()
            .map(|size| AudioSample {
                size,
                duration: FRAME_SAMPLES,
            })
            .collect(),
        spool,
    })
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

fn encoder_error(message: String) -> Error {
    Error::Encode(coreav_decode::Error::new(ErrorKind::Backend, message))
}

/// The decoded samples' way to the Opus encoder: it starts the encoder
/// once the decoder names the output's channels.
struct Feed<'a> {
    settings: &'a OpusAudio,
    launcher: Option<&'a ExecLauncher>,
    deadline: Instant,
    spool: &'a Spool,
    max_spool_bytes: u64,
    started: Option<Started>,
    /// The error that stopped the decode, which the decoder reports only
    /// as an output error.
    error: Option<Error>,
}

struct Started {
    process: Process<Packets>,
    channels: u8,
    bitrate: u32,
    /// Samples per channel sent to the encoder.
    sent: Arc<AtomicU64>,
}

impl Feed<'_> {
    /// Records `error` and returns what stops the decoder.
    fn stop(&mut self, error: Error) -> coreav_audio::Error {
        self.error.get_or_insert(error);
        coreav_audio::Error::Output(std::io::Error::other("the Opus encode stopped"))
    }

    fn start_encoder(&mut self, format: &OutputFormat) -> Result<Started, Error> {
        let channels = u8::try_from(format.channels)
            .ok()
            .filter(|c| matches!(c, 1 | 2))
            .ok_or_else(|| {
                Error::Unsupported(format!(
                    "Opus encoding of {} channels: CoreAV encodes mono and stereo",
                    format.channels
                ))
            })?;
        if format.sample_rate != SAMPLE_RATE || format.format != SampleFormat::F32 {
            return Err(encoder_error(
                "the decoder's output is not 48 kHz float".into(),
            ));
        }
        let bitrate = self
            .settings
            .bitrate
            .unwrap_or_else(|| default_bitrate(channels));
        let program = &self.settings.program;
        if !program.as_os_str().as_encoded_bytes().contains(&b'/') {
            return Err(encoder_error(format!(
                "the Opus encoder program must be a path, not a name: {}",
                program.display()
            )));
        }
        let sent = Arc::new(AtomicU64::new(0));
        let packets = Packets {
            reader: PacketReader::new(),
            sent: Arc::clone(&sent),
            file: BufWriter::with_capacity(1 << 16, self.spool.handle()?),
            bytes: 0,
            max_bytes: self.max_spool_bytes,
            sizes: Vec::new(),
        };
        let command = command(
            program,
            &encode_args(channels, bitrate),
            HugePages::Kept,
            self.launcher,
            &EncoderLimits::new(ENCODER_MEMORY),
            self.deadline,
        );
        let process = Process::spawn(
            "the Opus encoder",
            "sample blocks",
            command,
            program,
            self.deadline,
            packets,
        )
        .map_err(Error::Encode)?;
        Ok(Started {
            process,
            channels,
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
        // Counted before the encoder can answer: its packets are checked
        // against the samples sent.
        started.sent.fetch_add(count, Ordering::SeqCst);
        match started.process.enqueue(samples.to_vec()) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.stop(Error::Encode(e))),
        }
    }
}

/// The encoder's output on its reader thread: checked by the packet reader,
/// each packet into the spool file.
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
}
