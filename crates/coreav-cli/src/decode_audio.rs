//! `coreav decode-audio` (`docs/design/audio-decode.md`): one audio track
//! of an MP4/MOV, Matroska or WebM file decoded to a WAV file, and the
//! `coreav.decode-audio/1` summary (`docs/schema/decode-audio-v1.md`).

use crate::error::Failure;
use crate::native::time;
use crate::output::WriteFailure;
use coreav_audio::{Error, Limits, Options, Summary, Warning, WavSink};
use coreav_isobmff::{BufferedSource, FileSource, Rational, Source};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

/// Decodes the stream `options` names from the file at `path` into `out`
/// as WAV with the library defaults. Returns the summary and the final
/// header, for an output that can be rewritten from its start
/// ([`crate::output::to_file_patched`]).
pub fn run(
    path: &Path,
    options: &Options,
    out: &mut dyn Write,
) -> Result<(Summary, Option<Vec<u8>>), WriteFailure> {
    let source = BufferedSource::new(FileSource::open(path).map_err(Error::Isobmff)?);
    run_source(source, options, out)
}

/// [`run`] for any [`Source`].
pub fn run_source<S: Source>(
    source: S,
    options: &Options,
    out: &mut dyn Write,
) -> Result<(Summary, Option<Vec<u8>>), WriteFailure> {
    let mut sink = WavSink::new(out);
    let mut summary = coreav_audio::decode(source, options, &Limits::default(), &mut sink)?;
    let header = sink.header().map(|(header, fits)| {
        if !fits {
            summary.warnings.push(Warning::WavTooLarge);
        }
        header
    });
    Ok((summary, header))
}

impl From<Error> for WriteFailure {
    fn from(error: Error) -> Self {
        let object = |kind: &str, limit: Option<&str>| {
            json!({
                "schema": "coreav.error/1",
                "kind": kind,
                "limit": limit,
                "offset": null,
                "message": error.to_string(),
            })
        };
        match error {
            Error::Isobmff(error) => Self::Rejected(Failure::library(error)),
            Error::Matroska(error) => Self::Rejected(Failure::Matroska(error)),
            Error::Output(ref e) => Self::output(e),
            Error::Invalid(_) => Self::Rejected(Failure::Decode(object("invalid_structure", None))),
            Error::Truncated => {
                Self::Rejected(Failure::Decode(object("truncated_structure", None)))
            }
            Error::LimitExceeded(kind) => {
                Self::Rejected(Failure::Decode(object("limit", Some(kind.name()))))
            }
            Error::UnknownContainer => {
                Self::Rejected(Failure::Decode(object("unsupported_container", None)))
            }
            Error::Unsupported(_) | Error::NoSuchStream | Error::CodecNotDecoded(_) => {
                Self::Rejected(Failure::Decode(object("unsupported_feature", None)))
            }
        }
    }
}

/// The `coreav.decode-audio/1` summary (`docs/schema/decode-audio-v1.md`).
pub fn document(summary: &Summary, bytes: u64) -> Value {
    let output = &summary.output;
    json!({
        "schema": "coreav.decode-audio/1",
        "output": {
            "bytes": bytes,
            "sample_format": output.format.name(),
            "sample_rate": output.sample_rate,
            "channels": output.channels,
            "channel_mask": output.channel_mask,
            "samples": summary.samples,
            "duration": time(
                Rational::from_int(i128::from(summary.samples)),
                output.sample_rate,
            ),
        },
        "stream": {
            "stream_index": summary.stream_index,
            "codec": summary.stream.codec.name(),
            "sample_rate": summary.stream.sample_rate,
            "channels": summary.stream.channels,
            "bits_per_sample": summary.stream.bits_per_sample,
            "packets": summary.packets,
        },
        "timeline": {
            "start": summary.timeline.start,
            "silence": summary
                .timeline
                .silence
                .iter()
                .map(|span| json!({"at": span.at, "samples": span.samples}))
                .collect::<Vec<_>>(),
        },
        "warnings": summary.warnings.iter().map(ToString::to_string).collect::<Vec<_>>(),
    })
}
