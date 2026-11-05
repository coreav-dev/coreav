//! The private AV1 worker and its separate version 2 result document.

#[cfg(feature = "svt-av1")]
use std::ffi::OsString;
#[cfg(feature = "svt-av1")]
use std::io::Write;
#[cfg(feature = "svt-av1")]
use std::process::ExitCode;

#[cfg(feature = "svt-av1")]
use crate::strict_safe::{FLAG, StrictSafe};

/// Whether this CLI compiled a supported private native worker, not a runtime verdict.
pub const fn compiled() -> bool {
    cfg!(feature = "svt-av1-native") && coreav_transcode::Av1Transcode::compiled()
}

/// Prepares a compiled CLI worker before any media or output I/O.
pub fn prepare(
    options: coreav_transcode::Av1Options,
    preset: u8,
    crf: u8,
    launcher: &coreav_worker::ExecLauncher,
) -> Result<coreav_transcode::Av1Transcode<'_>, crate::output::WriteFailure> {
    if !compiled() {
        return Err(crate::output::WriteFailure::Rejected(
            crate::error::Failure::Unsupported(
                "no AV1 encoder: this build has no SVT-AV1 backend".into(),
            ),
        ));
    }
    options
        .with_quality(preset, crf)
        .and_then(|options| coreav_transcode::Av1Transcode::prepare(options, launcher))
        .map_err(super::failure)
}

/// Checks host-side preparation against the job's original deadline.
pub fn check_deadline(
    job: &coreav_transcode::Av1Transcode<'_>,
) -> Result<(), crate::output::WriteFailure> {
    job.check_deadline().map_err(super::failure)
}

/// Identifies the source under the prepared deadline, then consumes the AV1 job.
pub fn make<S: coreav_isobmff::Source>(
    job: coreav_transcode::Av1Transcode<'_>,
    this: &std::path::Path,
    strict_safe: crate::strict_safe::StrictSafe,
    mut source: S,
    output: &std::path::Path,
    out: &mut dyn std::io::Write,
) -> Result<serde_json::Value, crate::output::WriteFailure> {
    check_deadline(&job)?;
    let detected = crate::matroska::detect_source(&mut source);
    check_deadline(&job)?;
    let kind = if detected.map_err(crate::output::WriteFailure::Rejected)? {
        coreav_transcode::SourceKind::Matroska
    } else {
        coreav_transcode::SourceKind::Mp4
    };
    let decoders = crate::thumbnail::backends(Some(this), 0, true, strict_safe);
    let summary = job
        .run(source, kind, &decoders.list(), output, out)
        .map_err(|error| decoders.explain(super::failure(error)))?;
    Ok(document(&summary))
}

/// Serializes a completed AV1 job without changing the legacy H.264 summary.
pub fn document(summary: &coreav_transcode::Av1Summary) -> serde_json::Value {
    use coreav_frame::ChromaLocation;
    use serde_json::json;

    let video = &summary.video;
    let encoding = &video.encoding;
    let color = encoding.color;
    super::document_common(
        "coreav.transcode/2",
        json!({
            "stream_index": video.stream_index,
            "codec": video.codec.name(),
            "frames": video.frames,
            "not_presented": video.not_presented,
            "backend": video.decoder.backend.name(),
            "width": video.width,
            "height": video.height,
            "sample_aspect": video.sample_aspect.map(|(num, den)| format!("{num}:{den}")),
            "encoding": {
                "codec": "av1",
                "backend": "svt-av1",
                "version": encoding.version,
                "kind": "software",
                "isolation": "sandboxed",
                "preset": encoding.preset,
                "crf": encoding.crf,
                "cadence": {"num": encoding.cadence.num, "den": encoding.cadence.den},
                "bit_depth": 8,
                "chroma": "4:2:0",
                "color": {
                    "primaries": color.primaries,
                    "transfer": color.transfer,
                    "matrix": color.matrix,
                    "full_range": color.full_range,
                    "chroma_location": color.chroma_location.map(|location| match location {
                        ChromaLocation::Left => "left",
                        ChromaLocation::Center => "center",
                        ChromaLocation::TopLeft => "topleft",
                        ChromaLocation::Top => "top",
                        ChromaLocation::BottomLeft => "bottomleft",
                        ChromaLocation::Bottom => "bottom",
                    }),
                },
                "keyint": encoding.keyint,
                "parallelism_level": 1,
                "thread_limit": null,
                "retained_budget": encoding.retained_budget,
                "fixed_retained_bytes": encoding.fixed_retained_bytes,
                "retained_picture_bytes": encoding.retained_picture_bytes,
                "segment_frames": encoding.segment_frames,
                "segments": encoding.segments,
                "address_space": encoding.address_space,
            },
        }),
        &summary.remux,
        summary.audio.as_ref(),
    )
}

#[cfg(feature = "svt-av1")]
pub(crate) fn worker_main(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let mut args = args.into_iter().peekable();
    let strict_safe = StrictSafe::new(args.next_if(|arg| arg == FLAG).is_some());
    if args.next().is_some() {
        let _ = writeln!(std::io::stderr(), "invalid private AV1 worker arguments");
        return ExitCode::from(2);
    }
    if let Err(message) = strict_safe.sandbox() {
        let _ = writeln!(std::io::stderr(), "{message}");
        return ExitCode::from(4);
    }
    if !compiled() {
        let _ = writeln!(
            std::io::stderr(),
            "no AV1 encoder: this build has no native SVT-AV1 backend for this target"
        );
        return ExitCode::from(4);
    }
    #[cfg(feature = "svt-av1-native")]
    {
        if coreav_backend_svt_av1::worker::run().is_ok() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
    #[cfg(not(feature = "svt-av1-native"))]
    ExitCode::from(4)
}
