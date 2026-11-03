mod exact;
mod record;
#[cfg(test)]
mod tests;

use super::args::{self, AviAudio, Codec, Command, Encoding, MapMetadata, Muxer};
use super::{Refusal, Stop};
use crate::ffprobe::model::{Unit, Value};
use crate::ffprobe::{Report, StreamKind};
use crate::output::{self, WriteFailure};
use coreav_audio::raw::{RawF32Format, RawF32Output, RawF32Stream};
use coreav_audio::raw::{RawS16Format, RawS16Output, RawS16Stream};
use coreav_audio::{Error, Limits, Options, OutputFormat, SampleFormat, Sink, Summary};
use coreav_isobmff::{BufferedSource, FileSource, Source};
use std::io::{Read, Write};
use std::path::Path;

/// The agreement contract of an admitted decoder representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecoderEligibilityKind {
    /// Complete raw decoded samples agree with the pinned reference.
    Exact,
    /// A separately registered decoder conformance contract applies.
    Conformance,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DecoderPlan {
    Builtin(usize),
    Recorded(record::Admission),
}

/// The exact stages resolved before decoding or opening output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The registered decoder contract, independent of exact conversion stages.
    pub eligibility: DecoderEligibilityKind,
    options: Options,
    decoder: DecoderPlan,
    rate: exact::Implementation,
    input_rate: u32,
    channels: exact::ChannelPlan,
    output_rate: u32,
    output_format: SampleFormat,
}

pub(super) fn legacy(command: &Command, report: &Report, selected: &[usize]) -> bool {
    command.muxer == Muxer::S16le
        && !args::stdout(&command.output)
        && (timed(command)
            || selected.iter().any(|i| {
                report
                    .streams
                    .get(*i)
                    .is_some_and(|stream| super::int(stream, "channels") > 2)
            }))
}

fn timed(command: &Command) -> bool {
    [
        command.input_seek,
        command.input_duration,
        command.input_until,
        command.seek,
        command.duration,
        command.until,
    ]
    .iter()
    .any(Option::is_some)
}

fn command_shape(command: &Command) -> Result<(), Refusal> {
    if !command.more_inputs.is_empty()
        || command.frames != Default::default()
        || timed(command)
        || matches!(command.map_metadata, MapMetadata::Stream(_))
        || (Encoding {
            threads: None,
            ..command.encoding.clone()
        }) != Encoding::default()
    {
        return Err(Refusal::not_native(
            "raw audio with mapping metadata, timing or encoder options",
        ));
    }
    Ok(())
}

fn packing(command: &Command) -> (Codec, SampleFormat) {
    if command.muxer == Muxer::F32le {
        (Codec::PcmF32, SampleFormat::F32)
    } else {
        (Codec::AviAudio(AviAudio::PcmS16), SampleFormat::S16)
    }
}

pub(super) fn plan(
    command: &Command,
    report: &Report,
    selected: &[usize],
    concat: bool,
) -> Result<Plan, Refusal> {
    command_shape(command)?;
    if concat {
        return Err(Refusal::not_native("raw audio from a concat list"));
    }
    let [index] = selected else {
        return Err(Refusal::not_native(
            "raw audio requires exactly one audio stream",
        ));
    };
    let stream = report
        .streams
        .get(*index)
        .filter(|stream| stream.kind == StreamKind::Audio)
        .ok_or_else(|| Refusal::not_native("raw audio requires an audio stream"))?;
    let (codec, output_format) = packing(command);
    if super::codecs(command, &[(*index, stream)]) != [Some(codec)] {
        return Err(Refusal::not_native(
            "raw audio with another effective output codec",
        ));
    }
    let codec = stream.entries.iter().find_map(|(key, value)| match value {
        Value::Str(name) if *key == "codec_name" => coreav_audio::Codec::from_name(name),
        _ => None,
    });
    let bits = stream.entries.iter().find_map(|(key, value)| match value {
        Value::Str(bits) if *key == "bits_per_raw_sample" => bits.parse::<u32>().ok(),
        _ => None,
    });
    let channels = u16::try_from(super::int(stream, "channels"))
        .map_err(|_| Refusal::not_native("raw audio with unknown channels"))?;
    let layout = stream.entries.iter().find_map(|(key, value)| match value {
        Value::Str(layout) if *key == "channel_layout" => Some(layout.as_str()),
        _ => None,
    });
    if !matches!((channels, layout), (1, Some("mono")) | (2, Some("stereo"))) {
        return Err(Refusal::not_native(
            "an unregistered raw audio speaker layout",
        ));
    }
    let output_channels = command.channels.unwrap_or(channels);
    let channel_plan = exact::channels(channels, output_channels, bits, output_format)
        .ok_or_else(|| Refusal::not_native("an unregistered raw audio channel conversion"))?;
    let input_rate = stream
        .entries
        .iter()
        .find_map(|(key, value)| match value {
            Value::Unit(rate, Unit::Hertz) if *key == "sample_rate" => u32::try_from(*rate).ok(),
            _ => None,
        })
        .ok_or_else(|| Refusal::not_native("raw audio with unknown sample rate"))?;
    let output_rate = command.sample_rate.unwrap_or(input_rate);
    if channels != output_channels && input_rate != output_rate {
        return Err(Refusal::not_native(
            "combined channel and rate conversion is not registered",
        ));
    }
    let decoded_format =
        if output_rate != input_rate && bits == Some(16) && output_format == SampleFormat::S16 {
            SampleFormat::S16
        } else if output_rate != input_rate {
            SampleFormat::F32
        } else {
            output_format
        };
    let builtin = exact::DECODERS.iter().position(|row| {
        Some(row.codec) == codec
            && Some(row.bits) == bits
            && row.packing == decoded_format
            && !row.evidence.is_empty()
    });
    let container = match super::format_name(report) {
        Some("mov,mp4,m4a,3gp,3g2,mj2") => "mp4",
        Some("matroska,webm") => "matroska",
        _ => {
            return Err(Refusal::not_native(
                "an unregistered raw audio input container",
            ));
        }
    };
    let (decoder, eligibility) = if let Some(index) = builtin {
        let row = exact::DECODERS
            .get(index)
            .ok_or_else(|| Refusal::not_native("missing decoder registration"))?;
        (DecoderPlan::Builtin(index), row.eligibility)
    } else {
        let admission = codec
            .and_then(|codec| record::admission(codec, container))
            .ok_or_else(|| {
                Refusal::not_native("an unregistered decoder representation or presentation")
            })?;
        let kind = admission.kind;
        (DecoderPlan::Recorded(admission), kind)
    };
    let rate = exact::rate(
        decoded_format,
        output_format,
        channels,
        input_rate,
        output_rate,
    )
    .ok_or_else(|| Refusal::not_native("an unregistered exact raw audio rate conversion"))?;
    Ok(Plan {
        eligibility,
        options: Options {
            stream: Some(*index),
            format: decoded_format,
            threads: command.input_threads.unwrap_or(1),
            channels: channel_plan.decoder_channels(),
            ..Options::default()
        },
        decoder,
        rate,
        input_rate,
        channels: channel_plan,
        output_rate,
        output_format,
    })
}

struct Bytes<'a>(&'a mut dyn Write);

impl Sink for Bytes<'_> {
    fn start(&mut self, _: &OutputFormat) -> Result<(), Error> {
        Ok(())
    }
    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        self.0.write_all(samples).map_err(Error::Output)
    }
}

struct Feed<'a> {
    stream: Conversion<'a>,
    rate: u32,
    channels: u16,
}

impl Sink for Feed<'_> {
    fn start(&mut self, format: &OutputFormat) -> Result<(), Error> {
        if format.sample_rate != self.rate
            || format.channels != self.channels
            || format.format != self.stream.input_format()
        {
            return Err(Error::Unsupported(
                "decoder output changed from the exact plan",
            ));
        }
        Ok(())
    }
    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        self.stream.write(samples)
    }
}

enum Conversion<'a> {
    S16(RawS16Stream<'a>),
    F32(RawF32Stream<'a>),
}

impl Conversion<'_> {
    fn input_format(&self) -> SampleFormat {
        match self {
            Self::S16(_) => SampleFormat::S16,
            Self::F32(_) => SampleFormat::F32,
        }
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        match self {
            Self::S16(stream) => stream.write(bytes),
            Self::F32(stream) => stream.write(bytes),
        }
    }
    fn finish(self) -> Result<coreav_audio::raw::RawSummary, Error> {
        match self {
            Self::S16(stream) => stream.finish(),
            Self::F32(stream) => stream.finish(),
        }
    }
}

struct Checked<'a> {
    sink: &'a mut dyn Sink,
    plan: &'a Plan,
}

impl Sink for Checked<'_> {
    fn start(&mut self, format: &OutputFormat) -> Result<(), Error> {
        if format.sample_rate != self.plan.output_rate
            || format.channels != self.plan.channels.output
            || format.format != self.plan.output_format
        {
            return Err(Error::Unsupported("raw output changed from the exact plan"));
        }
        self.sink.start(format)
    }
    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        self.sink.write(samples)
    }
}

#[derive(PartialEq, Eq)]
struct Pass {
    decoded: Summary,
    converted: Option<coreav_audio::raw::RawSummary>,
}

fn decode(command: &Command, plan: &Plan, sink: &mut dyn Sink) -> Result<Pass, Error> {
    let source = BufferedSource::new(FileSource::open(Path::new(&command.input))?);
    let limits = Limits::default();
    let mut checked = Checked { sink, plan };
    let (decoded, converted) = match plan.rate {
        exact::Implementation::Identity => (
            coreav_audio::decode(source, &plan.options, &limits, &mut checked)?,
            None,
        ),
        exact::Implementation::S16Compatibility | exact::Implementation::F32Compatibility => {
            let stream = if plan.rate == exact::Implementation::S16Compatibility {
                Conversion::S16(RawS16Stream::new(
                    RawS16Format::new(plan.input_rate, plan.channels.output)?,
                    RawS16Output::S16 {
                        sample_rate: Some(plan.output_rate),
                    },
                    &limits,
                    &mut checked,
                )?)
            } else {
                let output = if plan.output_format == SampleFormat::F32 {
                    RawF32Output::F32 {
                        sample_rate: Some(plan.output_rate),
                    }
                } else {
                    RawF32Output::S16 {
                        sample_rate: Some(plan.output_rate),
                    }
                };
                Conversion::F32(RawF32Stream::new(
                    RawF32Format::new(plan.input_rate, plan.channels.output)?,
                    output,
                    &limits,
                    &mut checked,
                )?)
            };
            let mut feed = Feed {
                stream,
                rate: plan.input_rate,
                channels: plan.channels.output,
            };
            let decoded = coreav_audio::decode(source, &plan.options, &limits, &mut feed)?;
            (decoded, Some(feed.stream.finish()?))
        }
    };
    let valid_decoder = match &plan.decoder {
        DecoderPlan::Builtin(index) => exact::DECODERS.get(*index).is_some_and(|row| {
            plan.eligibility == row.eligibility
                && decoded.stream.codec == row.codec
                && decoded.stream.bits_per_sample == Some(row.bits)
                && decoded.timeline.start == 0
                && decoded.timeline.silence.is_empty()
        }),
        DecoderPlan::Recorded(admission) => {
            plan.eligibility == admission.kind
                && decoded.stream.codec == admission.codec
                && (!admission.requirements.contains(&record::Requirement::AacLc)
                    || coreav_audio::Codec::from_name("aac") == Some(decoded.stream.codec))
        }
    };
    if !valid_decoder
        || decoded.stream.sample_rate != plan.input_rate
        || decoded.stream.channels != plan.channels.input
        || !decoded.warnings.is_empty()
    {
        return Err(Error::Unsupported(
            "raw audio decoder or presentation differs from its exact registration",
        ));
    }
    Ok(Pass { decoded, converted })
}

fn packet_presentation(command: &Command, plan: &Plan, pass: &Pass) -> Result<(), Stop> {
    let mut source = BufferedSource::new(
        FileSource::open(Path::new(&command.input)).map_err(WriteFailure::from)?,
    );
    let size = source.len().map_err(WriteFailure::from)?;
    let mut probed = crate::ffprobe::probe(&mut source, size, &command.input, None)
        .map_err(WriteFailure::Rejected)?;
    let index = plan
        .options
        .stream
        .ok_or_else(|| Refusal::not_native("raw audio without a selected stream"))?;
    let mut payloads = vec![false; probed.report.streams.len()];
    if let Some(payload) = payloads.get_mut(index) {
        *payload = true;
    }
    let mut packets = 0u64;
    let mut samples = 0u64;
    let mut unsupported = false;
    probed
        .packets
        .visit(&payloads, &mut |packet| {
            if packet.stream != index {
                return Ok(true);
            }
            unsupported |= packet.corrupt;
            match &plan.decoder {
                DecoderPlan::Builtin(index) => {
                    let Some(row) = exact::DECODERS.get(*index) else {
                        unsupported = true;
                        return Ok(false);
                    };
                    let exact::Presentation::CompletePackets { frames } = row.presentation;
                    unsupported |= packet.discard
                        || !packet.side_data.is_empty()
                        || packet.pts != packet.dts
                        || packets == 0 && packet.pts != Some(0);
                    if let Some(count) = frames(&packet.data) {
                        samples = samples.saturating_add(u64::from(count));
                    } else {
                        unsupported = true;
                    }
                }
                DecoderPlan::Recorded(admission) => {
                    for requirement in &admission.requirements {
                        unsupported |= match requirement {
                            record::Requirement::SupportedMp4Timeline => {
                                packet.pts.is_none() || packet.pts != packet.dts
                            }
                            record::Requirement::Mpeg1Layer3 => !mpeg1_frames(&packet.data),
                            record::Requirement::AacLc => {
                                coreav_audio::Codec::from_name("aac")
                                    != Some(pass.decoded.stream.codec)
                            }
                        };
                    }
                }
            }
            packets = packets.saturating_add(1);
            Ok(true)
        })
        .map_err(WriteFailure::Rejected)?;
    if unsupported
        || packets != pass.decoded.packets
        || matches!(plan.decoder, DecoderPlan::Builtin(_)) && samples != pass.decoded.samples
    {
        return Err(Refusal::not_native(
            "raw audio presentation differs from complete decoded packets",
        )
        .into());
    }
    Ok(())
}

fn mpeg1_frames(mut bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    while !bytes.is_empty() {
        let Ok(header) = coreav_audio::mp3::Header::parse(bytes) else {
            return false;
        };
        if header.version != coreav_audio::mp3::Version::Mpeg1 {
            return false;
        }
        let Some(length) = header.frame_bytes() else {
            return false;
        };
        let Some(tail) = bytes.get(length..) else {
            return false;
        };
        bytes = tail;
    }
    true
}

pub(super) fn write(command: &Command, plan: &Plan) -> Result<serde_json::Value, Stop> {
    let force = if args::stdout(&command.output) {
        false
    } else {
        super::check_output(command, Path::new(&command.output))?
    };
    let before = decode(command, plan, &mut Bytes(&mut std::io::sink())).map_err(prestart)?;
    packet_presentation(command, plan, &before)?;
    let run = |out: &mut dyn Write| -> Result<serde_json::Value, Error> {
        let after = decode(command, plan, &mut Bytes(out))?;
        if before != after {
            return Err(Error::Invalid(
                "raw audio changed between preflight and execution",
            ));
        }
        Ok(serde_json::Value::Null)
    };
    if args::stdout(&command.output) {
        let mut out = output::media_stdout();
        let result = run(&mut out).map_err(|e| Stop::Failed(audio_failure(e)))?;
        out.flush()
            .map_err(|e| Stop::Failed(audio_failure(Error::Output(e))))?;
        Ok(result)
    } else {
        output::to_file(Path::new(&command.output), force, |out| {
            run(out).map_err(audio_failure)
        })
        .map_err(Stop::Failed)
    }
}

fn prestart(error: Error) -> Stop {
    match error {
        Error::Unsupported(reason) => Refusal::not_native(reason).into(),
        error => Stop::Failed(audio_failure(error)),
    }
}

fn audio_failure(error: Error) -> WriteFailure {
    match error {
        Error::Output(error) => WriteFailure::Rejected(crate::error::Failure::io(error)),
        error => error.into(),
    }
}

pub(super) fn write_raw(
    command: &Command,
    format: RawS16Format,
) -> Result<serde_json::Value, Stop> {
    command_shape(command)?;
    if command.no_audio || !command.maps.is_empty() {
        return Err(Refusal::not_native("raw PCM with stream mapping or disabled audio").into());
    }
    let (codec, packing) = packing(command);
    let mut effective = codec;
    for (spec, value) in &command.codecs {
        if matches!(
            spec,
            args::Spec::All
                | args::Spec::Index(0)
                | args::Spec::Kind(crate::ffprobe::args::Kind2::Audio, None | Some(0))
        ) {
            effective = *value;
        }
    }
    if effective != codec
        || command
            .channels
            .is_some_and(|channels| channels != format.channels())
        || exact::channels(format.channels(), format.channels(), Some(16), packing).is_none()
    {
        return Err(Refusal::not_native(
            "raw PCM with an unregistered codec or channel conversion",
        )
        .into());
    }
    let rate = command.sample_rate.unwrap_or(format.rate());
    exact::rate(
        SampleFormat::S16,
        packing,
        format.channels(),
        format.rate(),
        rate,
    )
    .ok_or_else(|| Refusal::not_native("an unregistered exact raw PCM rate conversion"))?;
    let output = if packing == SampleFormat::F32 {
        RawS16Output::F32
    } else {
        RawS16Output::S16 {
            sample_rate: Some(rate),
        }
    };
    let limits = Limits::default();
    RawS16Stream::new(format, output, &limits, &mut Bytes(&mut std::io::sink()))
        .map_err(prestart)?;
    let pipe = args::stdin(&command.input);
    if !pipe {
        let bytes = std::fs::metadata(&command.input)
            .map_err(|e| Stop::Failed(audio_failure(Error::Output(e))))?
            .len();
        coreav_audio::raw::RawS16Plan::new(format, command.sample_rate, bytes, &limits)
            .map_err(prestart)?;
    }
    let mut input: Box<dyn Read> = if pipe {
        Box::new(std::io::stdin().lock())
    } else {
        Box::new(
            std::fs::File::open(&command.input)
                .map_err(|e| Stop::Failed(Error::Output(e).into()))?,
        )
    };
    let force = if args::stdout(&command.output) {
        false
    } else {
        super::check_output(command, Path::new(&command.output))?
    };
    let mut run = |out: &mut dyn Write| -> Result<serde_json::Value, Error> {
        let mut sink = Bytes(out);
        let mut stream = RawS16Stream::new(format, output, &limits, &mut sink)?;
        let frame = usize::from(format.channels()) * 2;
        let mut bytes = [0u8; 65_536];
        let mut held = 0;
        loop {
            let tail = bytes
                .get_mut(held..)
                .ok_or(Error::Invalid("raw PCM read buffer"))?;
            let count = match input.read(tail) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                result => result.map_err(Error::Output)?,
            };
            if count == 0 {
                break;
            }
            let end = held + count;
            let complete = end / frame * frame;
            stream.write(
                bytes
                    .get(..complete)
                    .ok_or(Error::Invalid("raw PCM read buffer"))?,
            )?;
            bytes.copy_within(complete..end, 0);
            held = end - complete;
        }
        if held != 0 {
            return Err(Error::Invalid("raw PCM input ends inside a sample frame"));
        }
        stream.finish()?;
        Ok(serde_json::Value::Null)
    };
    if args::stdout(&command.output) {
        let mut out = output::media_stdout();
        let result = run(&mut out).map_err(|e| Stop::Failed(audio_failure(e)))?;
        out.flush()
            .map_err(|e| Stop::Failed(audio_failure(Error::Output(e))))?;
        Ok(result)
    } else {
        output::to_file(Path::new(&command.output), force, |out| {
            run(out).map_err(audio_failure)
        })
        .map_err(Stop::Failed)
    }
}
