use super::{Bytes, Command, DecoderEligibilityKind, DecoderPlan, Plan, Refusal, Stop, exact};
use crate::ffprobe::audio::flac::load_source;
use crate::output::{self, WriteFailure};
use coreav_audio::flac::prepare_stream;
use coreav_isobmff::{FileSource, Source};
use std::io::Write;
use std::path::Path;

pub(super) fn write(
    command: &Command,
    plan: &Plan,
    force: bool,
) -> Result<serde_json::Value, Stop> {
    let mut source = FileSource::open(Path::new(&command.input)).map_err(WriteFailure::from)?;
    let size = source.len().map_err(WriteFailure::from)?;
    let loaded = load_source(&mut source, size, &coreav_isobmff::Limits::default())
        .map_err(super::super::probe_failure)?;
    let DecoderPlan::Flac(index) = plan.decoder else {
        return Err(Refusal::not_native("standalone FLAC without an exact plan").into());
    };
    if !exact::DECODERS.get(index).is_some_and(|row| {
        row.codec == coreav_audio::Codec::Flac
            && row.bits == u32::from(loaded.info.bits_per_sample)
            && row.packing == plan.output_format
            && row.eligibility == DecoderEligibilityKind::Exact
    }) || loaded.info.sample_rate != plan.input_rate
        || u16::from(loaded.info.channels) != plan.channels.input
        || plan.input_rate != plan.output_rate
        || plan.channels.input != plan.channels.output
    {
        return Err(Refusal::not_native("standalone FLAC changed from its exact plan").into());
    }
    let prepared = prepare_stream(
        loaded.info,
        &loaded.payload,
        plan.output_format,
        &loaded.limits,
    )
    .map_err(super::prestart)?;
    let run = |out: &mut dyn Write| -> Result<serde_json::Value, WriteFailure> {
        prepared
            .write(&mut Bytes(out))
            .map_err(super::audio_failure)?;
        Ok(serde_json::Value::Null)
    };
    if super::args::stdout(&command.output) {
        let mut out = output::media_stdout();
        let result = run(&mut out).map_err(Stop::Failed)?;
        out.flush().map_err(|error| {
            Stop::Failed(super::audio_failure(coreav_audio::Error::Output(error)))
        })?;
        Ok(result)
    } else {
        output::to_file(Path::new(&command.output), force, run).map_err(Stop::Failed)
    }
}
