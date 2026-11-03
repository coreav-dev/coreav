//! The private AV1 worker and its separate version 2 result document.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use crate::strict_safe::{FLAG, StrictSafe};

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
    if !cfg!(feature = "svt-av1-native") || !coreav_transcode::Av1Transcode::compiled() {
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
