use super::args::{Codec, Command};
use super::{Container, Job, Refusal, Stop};
use crate::fallback::NotNative;
use crate::ffprobe::{Stream, StreamKind};
use crate::output::{self, WriteFailure};
use coreav_isobmff::FileSource;
use coreav_isobmff_write::{ExtractError, ts_output};
use std::path::Path;

pub(super) fn plan(
    command: &Command,
    output: &[(usize, &Stream)],
    codecs: &[Option<Codec>],
    container: Container,
    concat: bool,
    trims: bool,
) -> Result<Job, Refusal> {
    let [(video, stream)] = output else {
        return Err(Refusal::not_native(
            "MPEG-TS copy requires one selected video",
        ));
    };
    if container != Container::Isobmff
        || Path::new(&command.output)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("m2ts"))
        || concat
        || trims
        || !command.more_inputs.is_empty()
        || !command.no_audio
        || stream.kind != StreamKind::Video
        || stream.attached_pic
        || codecs != [Some(Codec::Copy)]
    {
        return Err(Refusal::not_native(
            "MPEG-TS requires whole ISO video copy with explicit audio exclusion",
        ));
    }
    Ok(Job::Mpegts { video: *video })
}

pub(super) fn write(command: &Command, video: usize) -> Result<serde_json::Value, Stop> {
    let path = Path::new(&command.output);
    let force = super::check_output(command, path)?;
    let source = FileSource::open(Path::new(&command.input)).map_err(WriteFailure::from)?;
    let prepared =
        ts_output::prepare(source, video, &ts_output::Limits::default()).map_err(|error| {
            match error {
                ExtractError::Source(coreav_isobmff::Error::Io(_)) | ExtractError::Write(_) => {
                    Stop::Failed(WriteFailure::from(error))
                }
                _ => Stop::Refused(Refusal::NotNative(NotNative::property(
                    "an unqualified MPEG-TS copy source",
                    format!("not supported by coreav ffmpeg: {error}"),
                ))),
            }
        })?;
    output::to_file(path, force, |out| {
        prepared
            .write_to(out)
            .map_err(|error| WriteFailure::output(&error))
    })?;
    Ok(serde_json::Value::Null)
}
