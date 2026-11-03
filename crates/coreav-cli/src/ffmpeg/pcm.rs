use super::args::{AviAudio, Codec, Command, Encoding, MapMetadata, Muxer, Spec};
use super::{Job, Refusal, Stop};
use crate::ffprobe::args::Kind2;
use crate::ffprobe::model::{Unit, Value};
use crate::ffprobe::{Report, StreamKind};
use crate::output;
use coreav_audio::SampleFormat;
use coreav_audio::flac::encode::FlacSink;
use coreav_audio::{Error, Limits, Options, OutputFormat, Sink, WavSink};
use coreav_isobmff::{BufferedSource, FileSource, Source};
use std::io::Write;
use std::path::Path;

pub(super) fn plan(
    command: &Command,
    report: &Report,
    selected: &[usize],
    concat: bool,
) -> Result<Job, Refusal> {
    let flac = command.muxer == Muxer::Flac;
    if flac
        && selected.iter().any(|index| {
            report
                .streams
                .get(*index)
                .is_some_and(|stream| stream.kind == StreamKind::Video)
        })
    {
        return Err(Refusal::not_native("writing a FLAC PICTURE block"));
    }
    let [index] = selected else {
        return Err(Refusal::not_native(
            "lossless audio output with other than one audio stream",
        ));
    };
    let stream = report
        .streams
        .get(*index)
        .ok_or_else(|| Refusal::Usage("The selected audio stream does not exist.".to_owned()))?;
    if stream.kind != StreamKind::Audio {
        return Err(Refusal::not_native(
            "lossless audio output with a non-audio stream",
        ));
    }
    let expected = if flac {
        Codec::Flac
    } else {
        Codec::AviAudio(AviAudio::PcmS16)
    };
    if super::codecs(command, &[(*index, stream)]) != [Some(expected)] {
        return Err(Refusal::not_native(if flac {
            "FLAC output with a codec other than flac"
        } else {
            "PCM output with a codec other than pcm_s16le"
        }));
    }
    let codec = stream.entries.iter().find_map(|(key, value)| match value {
        Value::Str(name) if *key == "codec_name" => Some(name.as_str()),
        _ => None,
    });
    if !codec
        .and_then(coreav_audio::Codec::from_name)
        .is_some_and(coreav_audio::Codec::is_lossless)
    {
        return Err(Refusal::not_native(
            "lossless audio output requires a decoded lossless source (currently FLAC)",
        ));
    }
    let format = if flac {
        let bits = stream.entries.iter().find_map(|(key, value)| match value {
            Value::Str(bits) if *key == "bits_per_raw_sample" => bits.parse::<u8>().ok(),
            _ => None,
        });
        match bits {
            Some(16) => SampleFormat::S16,
            Some(24) => SampleFormat::S24,
            _ => {
                return Err(Refusal::not_native(
                    "FLAC encoding of a source other than 16-bit or 24-bit audio",
                ));
            }
        }
    } else {
        SampleFormat::S16
    };
    if concat
        || (Encoding {
            threads: None,
            ..command.encoding.clone()
        }) != Encoding::default()
    {
        return Err(Refusal::not_native(
            "lossless audio output with concat or encoder options",
        ));
    }
    if [command.input_seek, command.seek]
        .into_iter()
        .flatten()
        .any(|time| time.num != 0)
    {
        return Err(Refusal::not_native(
            "lossless audio output with a nonzero seek",
        ));
    }
    if command.input_seek.is_some() && codec != Some("flac") {
        return Err(Refusal::not_native(
            "input seeking lossless audio from a stateful audio codec",
        ));
    }
    if matches!(command.map_metadata, MapMetadata::Stream(_)) {
        return Err(Refusal::not_native(
            "lossless audio output with metadata mapped from a stream",
        ));
    }
    if matches!(command.muxer, Muxer::Wav | Muxer::Flac) && !report.chapters.is_empty() {
        return Err(Refusal::not_native(if flac {
            "copying chapters to FLAC"
        } else {
            "copying chapters to WAV"
        }));
    }
    if matches!(command.muxer, Muxer::Wav | Muxer::Flac)
        && command.map_metadata == MapMetadata::Default
        && report.format_tags.iter().any(|(key, _)| {
            !matches!(
                key.to_ascii_lowercase().as_str(),
                "encoder" | "major_brand" | "minor_version" | "compatible_brands"
            )
        })
    {
        return Err(Refusal::not_native(if flac {
            "copying metadata to FLAC (use -map_metadata -1)"
        } else {
            "copying metadata to WAV (use -map_metadata -1)"
        }));
    }
    if flac
        && command.map_metadata == MapMetadata::Default
        && stream
            .tags
            .iter()
            .any(|(key, value)| match key.to_ascii_lowercase().as_str() {
                "language" => value != "und",
                "handler_name" => value != "SoundHandler",
                "vendor_id" | "encoder" | "duration" => false,
                _ => true,
            })
    {
        return Err(Refusal::not_native(
            "copying stream metadata to FLAC (use -map_metadata -1)",
        ));
    }
    let rate = stream.entries.iter().find_map(|(key, value)| match value {
        Value::Unit(rate, Unit::Hertz) if *key == "sample_rate" => u64::try_from(*rate).ok(),
        _ => None,
    });
    if command
        .sample_rate
        .is_some_and(|asked| Some(u64::from(asked)) != rate)
    {
        return Err(Refusal::not_native("resampling PCM output"));
    }
    if command
        .channels
        .is_some_and(|asked| asked > 2 || i64::from(asked) != super::int(stream, "channels"))
    {
        return Err(Refusal::not_native("mixing PCM output channels"));
    }
    let mut samples = None;
    for (input, duration, until) in [
        (true, command.input_duration, command.input_until),
        (false, command.duration, command.until),
    ] {
        let time = match (duration, until) {
            (Some(time), _) if input && time.num == 0 => continue,
            (Some(time), _) => time,
            (None, Some(time)) if time.num == 0 => {
                return Err(Refusal::Usage(
                    "-to must be greater than the start time.".to_owned(),
                ));
            }
            (None, Some(time)) => time,
            (None, None) => continue,
        };
        let count = rate
            .and_then(|rate| time.num.checked_mul(i128::from(rate)))
            .filter(|count| count % i128::from(time.den) == 0)
            .and_then(|count| u64::try_from(count / i128::from(time.den)).ok())
            .ok_or_else(|| {
                Refusal::not_native("a lossless audio duration that is not a whole sample count")
            })?;
        samples = Some(samples.map_or(count, |before: u64| before.min(count)));
    }
    let mask = if matches!(command.muxer, Muxer::Wav | Muxer::Flac)
        && super::int(stream, "channels") > 2
    {
        let layout = stream.entries.iter().find_map(|(key, value)| match value {
            Value::Str(name) if *key == "channel_layout" => Some(name.as_str()),
            _ => None,
        });
        let mask: u32 = match layout {
            Some("2.1") => 0xB,
            Some("3.0") => 0x7,
            Some("quad") => 0x33,
            Some("4.0") => 0x107,
            Some("5.0") => 0x37,
            Some("5.0(side)") => 0x607,
            Some("5.1") => 0x3F,
            Some("5.1(side)") => 0x60F,
            Some("6.1") => 0x70F,
            Some("7.1") => 0x63F,
            _ => {
                return Err(Refusal::not_native(
                    "a speaker layout not represented by the native audio writer",
                ));
            }
        };
        if i64::from(mask.count_ones()) != super::int(stream, "channels") {
            return Err(Refusal::not_native(
                "an inconsistent speaker layout for native audio output",
            ));
        }
        if flac
            && u16::try_from(super::int(stream, "channels"))
                .ok()
                .and_then(coreav_audio::flac::encode::channel_mask)
                != Some(mask)
        {
            return Err(Refusal::not_native(
                "a speaker layout outside the FLAC channel order",
            ));
        }
        Some(mask)
    } else {
        None
    };
    Ok(Job::Pcm {
        stream: *index,
        samples,
        mask,
        format,
    })
}

enum Output<'a> {
    Wav(WavSink<&'a mut dyn Write>),
    Raw(&'a mut dyn Write),
    Flac(Box<FlacSink<&'a mut dyn Write>>),
}

impl Sink for Output<'_> {
    fn start(&mut self, format: &OutputFormat) -> Result<(), Error> {
        match self {
            Self::Wav(sink) => sink.start(format),
            Self::Raw(_) => Ok(()),
            Self::Flac(sink) => sink.start(format),
        }
    }

    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        match self {
            Self::Wav(sink) => sink.write(samples),
            Self::Raw(out) => out.write_all(samples).map_err(Error::Output),
            Self::Flac(sink) => sink.write(samples),
        }
    }
}

impl Output<'_> {
    fn patch(self) -> Result<Option<Vec<u8>>, Error> {
        match self {
            Self::Raw(_) => Ok(None),
            Self::Flac(flac) => Ok(Some((*flac).finish()?)),
            Self::Wav(wav) => match wav.header() {
                Some((header, true)) => Ok(Some(header)),
                _ => Err(Error::Unsupported(
                    "PCM output larger than the WAV size fields",
                )),
            },
        }
    }
}

pub(super) fn write_raw(
    command: &Command,
    format: coreav_audio::raw::RawS16Format,
) -> Result<serde_json::Value, Stop> {
    if !matches!(command.muxer, Muxer::Wav | Muxer::S16le)
        || !command.more_inputs.is_empty()
        || command.frames != Default::default()
        || command.no_audio
        || !command.maps.is_empty()
        || matches!(command.map_metadata, MapMetadata::Stream(_))
        || [
            command.input_seek,
            command.input_duration,
            command.input_until,
            command.seek,
            command.duration,
            command.until,
        ]
        .iter()
        .any(Option::is_some)
        || (Encoding {
            threads: None,
            ..command.encoding.clone()
        }) != Encoding::default()
    {
        return Err(Refusal::not_native(
            "raw PCM input with a non-PCM output, mapping, seeking or encoder options",
        )
        .into());
    }
    if command.codecs.iter().any(|(spec, codec)| {
        *codec != Codec::AviAudio(AviAudio::PcmS16)
            || !matches!(
                spec,
                Spec::All | Spec::Index(0) | Spec::Kind(Kind2::Audio, None | Some(0))
            )
    }) {
        return Err(
            Refusal::not_native("raw PCM output with another codec or stream specifier").into(),
        );
    }
    let mut source =
        FileSource::open(Path::new(&command.input)).map_err(output::WriteFailure::from)?;
    let bytes = source.len().map_err(output::WriteFailure::from)?;
    let plan =
        coreav_audio::raw::RawS16Plan::new(format, command.sample_rate, bytes, &Limits::default())
            .map_err(|error| match error {
                Error::Unsupported(reason) => Stop::from(Refusal::not_native(reason)),
                error => Stop::from(output::WriteFailure::from(error)),
            })?;
    if command
        .channels
        .is_some_and(|channels| channels != plan.output().channels)
    {
        return Err(Refusal::not_native("mixing raw PCM output channels").into());
    }
    let output_bytes = plan
        .output_frames()
        .checked_mul(u64::from(plan.output().channels) * 2)
        .ok_or_else(|| Refusal::not_native("raw PCM output size overflows"))?;
    if command.muxer == Muxer::Wav && !coreav_audio::wav::fits(output_bytes) {
        return Err(Refusal::not_native("PCM output larger than the WAV size fields").into());
    }
    let path = Path::new(&command.output);
    let force = super::check_output(command, path)?;
    Ok(output::to_file_patched(path, force, |out| {
        let mut sink = if command.muxer == Muxer::Wav {
            Output::Wav(WavSink::new(out))
        } else {
            Output::Raw(out)
        };
        let summary = plan.run(source, &mut sink)?;
        let patch = sink.patch()?;
        Ok((
            serde_json::json!({"input_frames": summary.input_frames,
            "output_frames": summary.output_frames, "sample_rate": summary.output.sample_rate,
            "channels": summary.output.channels}),
            patch,
        ))
    })?)
}

struct Samples<'a> {
    output: Output<'a>,
    remaining: Option<u64>,
    frame_bytes: usize,
    mask: Option<u32>,
}

impl Sink for Samples<'_> {
    fn start(&mut self, format: &OutputFormat) -> Result<(), Error> {
        self.frame_bytes = usize::from(format.channels) * format.format.bytes();
        let format = OutputFormat {
            channel_mask: self.mask.unwrap_or(format.channel_mask),
            ..*format
        };
        self.output.start(&format)
    }

    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        let count = samples.len().checked_div(self.frame_bytes).unwrap_or(0);
        let count = self.remaining.map_or(count, |left| {
            count.min(usize::try_from(left).unwrap_or(usize::MAX))
        });
        if let Some(left) = &mut self.remaining {
            *left = left.saturating_sub(count as u64);
        }
        self.output
            .write(samples.get(..count * self.frame_bytes).unwrap_or_default())
    }
}

pub(super) fn write(
    command: &Command,
    stream: usize,
    samples: Option<u64>,
    mask: Option<u32>,
    format: SampleFormat,
) -> Result<serde_json::Value, Stop> {
    let path = Path::new(&command.output);
    let force = super::check_output(command, path)?;
    let mut unsupported = None;
    let result = output::to_file_patched(path, force, |out| {
        let source = BufferedSource::new(FileSource::open(Path::new(&command.input))?);
        let output = if command.muxer == Muxer::Wav {
            Output::Wav(WavSink::new(out))
        } else if command.muxer == Muxer::Flac {
            Output::Flac(Box::new(FlacSink::new(out, Limits::default())))
        } else {
            Output::Raw(out)
        };
        let mut sink = Samples {
            output,
            remaining: samples,
            frame_bytes: 0,
            mask,
        };
        let options = Options {
            stream: Some(stream),
            format,
            threads: command.input_threads.unwrap_or(1),
            ..Options::default()
        };
        let summary = coreav_audio::decode(source, &options, &Limits::default(), &mut sink)
            .map_err(|error| {
                if let Error::Unsupported(reason) = &error {
                    unsupported = Some(*reason);
                }
                crate::output::WriteFailure::from(error)
            })?;
        if summary
            .warnings
            .iter()
            .any(|warning| matches!(warning, coreav_audio::Warning::EditListIgnored))
        {
            unsupported =
                Some("lossless audio output with an edit list the audio decoder does not apply");
            return Err(Error::Unsupported("PCM edit list").into());
        }
        if !summary.warnings.is_empty() {
            return Err(Error::Invalid(
                "lossless audio decoding reported damaged or inconsistent input",
            )
            .into());
        }
        let patch = sink.output.patch().map_err(|error| {
            if let Error::Unsupported(reason) = &error {
                unsupported = Some(*reason);
            }
            output::WriteFailure::from(error)
        })?;
        Ok((serde_json::Value::Null, patch))
    });
    match (result, unsupported) {
        (_, Some(reason)) => Err(Refusal::not_native(reason).into()),
        (result, None) => Ok(result?),
    }
}
