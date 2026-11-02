use super::args::{AviAudio, Codec, Command, Encoding, MapMetadata, Muxer};
use super::{Job, Refusal, Stop};
use crate::ffprobe::model::{Unit, Value};
use crate::ffprobe::{Report, StreamKind};
use crate::output;
use coreav_audio::{Error, Limits, Options, OutputFormat, Sink, WavSink};
use coreav_isobmff::{BufferedSource, FileSource};
use std::io::Write;
use std::path::Path;

pub(super) fn plan(
    command: &Command,
    report: &Report,
    selected: &[usize],
    concat: bool,
) -> Result<Job, Refusal> {
    let [index] = selected else {
        return Err(Refusal::not_native(
            "PCM output with other than one audio stream",
        ));
    };
    let stream = report
        .streams
        .get(*index)
        .ok_or_else(|| Refusal::Usage("The selected audio stream does not exist.".to_owned()))?;
    if stream.kind != StreamKind::Audio {
        return Err(Refusal::not_native("PCM output with a non-audio stream"));
    }
    if super::codecs(command, &[(*index, stream)]) != [Some(Codec::AviAudio(AviAudio::PcmS16))] {
        return Err(Refusal::not_native(
            "PCM output with a codec other than pcm_s16le",
        ));
    }
    let codec = stream.entries.iter().find_map(|(key, value)| match value {
        Value::Str(name) if *key == "codec_name" => Some(name.as_str()),
        _ => None,
    });
    let decoded = matches!(codec, Some("flac" | "mp3" | "opus"))
        || cfg!(feature = "aac") && codec == Some("aac");
    if !decoded {
        return Err(Refusal::not_native(
            "PCM output from an audio codec this build does not decode",
        ));
    }
    if concat || command.encoding != Encoding::default() {
        return Err(Refusal::not_native(
            "PCM output with concat or encoder options",
        ));
    }
    if [command.input_seek, command.seek]
        .into_iter()
        .flatten()
        .any(|time| time.num != 0)
    {
        return Err(Refusal::not_native("PCM output with a nonzero seek"));
    }
    if matches!(command.map_metadata, MapMetadata::Stream(_)) {
        return Err(Refusal::not_native(
            "PCM output with metadata mapped from a stream",
        ));
    }
    if command.muxer == Muxer::Wav && !report.chapters.is_empty() {
        return Err(Refusal::not_native("copying chapters to WAV"));
    }
    if command.muxer == Muxer::Wav
        && command.map_metadata == MapMetadata::Default
        && report.format_tags.iter().any(|(key, _)| {
            !matches!(
                key.to_ascii_lowercase().as_str(),
                "encoder" | "major_brand" | "minor_version" | "compatible_brands"
            )
        })
    {
        return Err(Refusal::not_native(
            "copying metadata to WAV (use -map_metadata -1)",
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
    for time in [
        command.input_duration.or(command.input_until),
        command.duration.or(command.until),
    ]
    .into_iter()
    .flatten()
    {
        let count = rate
            .and_then(|rate| time.num.checked_mul(i128::from(rate)))
            .filter(|count| count % i128::from(time.den) == 0)
            .and_then(|count| u64::try_from(count / i128::from(time.den)).ok())
            .ok_or_else(|| {
                Refusal::not_native("a PCM duration that is not a whole sample count")
            })?;
        samples = Some(samples.map_or(count, |before: u64| before.min(count)));
    }
    let mask = if command.muxer == Muxer::Wav && super::int(stream, "channels") > 2 {
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
                    "a speaker layout not represented by PCM WAV output",
                ));
            }
        };
        if i64::from(mask.count_ones()) != super::int(stream, "channels") {
            return Err(Refusal::not_native(
                "an inconsistent speaker layout for PCM WAV output",
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
    })
}

enum Output<'a> {
    Wav(WavSink<&'a mut dyn Write>),
    Raw(&'a mut dyn Write),
}

impl Sink for Output<'_> {
    fn start(&mut self, format: &OutputFormat) -> Result<(), Error> {
        match self {
            Self::Wav(sink) => sink.start(format),
            Self::Raw(_) => Ok(()),
        }
    }

    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        match self {
            Self::Wav(sink) => sink.write(samples),
            Self::Raw(out) => out.write_all(samples).map_err(Error::Output),
        }
    }
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
) -> Result<serde_json::Value, Stop> {
    let path = Path::new(&command.output);
    let force = super::check_output(command, path)?;
    let mut unsupported = None;
    let result = output::to_file_patched(path, force, |out| {
        let source = BufferedSource::new(FileSource::open(Path::new(&command.input))?);
        let output = if command.muxer == Muxer::Wav {
            Output::Wav(WavSink::new(out))
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
            unsupported = Some("PCM output with an edit list the audio decoder does not apply");
            return Err(Error::Unsupported("PCM edit list").into());
        }
        if !summary.warnings.is_empty() {
            return Err(
                Error::Invalid("PCM decoding reported damaged or inconsistent input").into(),
            );
        }
        let patch = match sink.output {
            Output::Wav(wav) => match wav.header() {
                Some((header, true)) => Some(header),
                _ => {
                    unsupported = Some("PCM output larger than the WAV size fields");
                    return Err(Error::Unsupported("WAV size").into());
                }
            },
            Output::Raw(_) => None,
        };
        Ok((serde_json::Value::Null, patch))
    });
    match (result, unsupported) {
        (_, Some(reason)) => Err(Refusal::not_native(reason).into()),
        (result, None) => Ok(result?),
    }
}
