//! `coreav transcode` (`docs/design/transcode.md`; experimental for sources
//! other than H.264): the
//! request from the command's arguments, the decoder backends and the
//! encoder's sandbox launcher of this build, and the `coreav.transcode/1`
//! summary (`docs/schema/transcode-v1.md`).
//!
//! Built only with the `transcode` feature, which every decoder backend
//! feature turns on: a build without a decoder cannot transcode.

use std::path::{Path, PathBuf};
use std::time::Duration;

use coreav_isobmff::Source;
use coreav_transcode::encoder::RateControl;
use coreav_transcode::{
    AacAudio, AudioMode, OpusAudio, PixelFormat, Remuxed, Scale, SourceKind, Summary,
    TranscodeOptions, parse_scale,
};
use coreav_worker::{EXEC_ARG, ExecLauncher, WorkerCommand};
use serde_json::{Value, json};

use crate::error::Failure;
use crate::output::WriteFailure;
use crate::strict_safe::{Encoder, StrictSafe};

/// What `coreav transcode` asks for, from its arguments.
#[derive(Clone, Debug)]
pub struct Request {
    /// `--crf`.
    pub crf: Option<f32>,
    /// `--bitrate`, in kbit/s.
    pub bitrate: Option<u32>,
    /// `--preset`.
    pub preset: String,
    /// `--scale`, read by [`parse_scale`].
    pub scale: Option<Scale>,
    /// `--audio none`.
    pub no_audio: bool,
    /// The audio streams to copy (source stream indices), instead of every
    /// one MP4 carries: an `ffmpeg` command line's selection.
    pub audio_streams: Option<Vec<usize>>,
    /// `--audio opus`: one audio stream encoded to Opus instead.
    pub opus: Option<OpusRequest>,
    /// `--audio aac`: one audio stream encoded to AAC-LC instead.
    pub aac: Option<AacRequest>,
    /// The pixel format the encoder must be given (`-pix_fmt`).
    pub pixel_format: Option<PixelFormat>,
    /// Refuse a stream whose frame size changes, instead of resizing it.
    pub one_size: bool,
    /// `--encoder`, as strict-safe mode allows it.
    pub encoder: Encoder,
    /// `--x264`.
    pub x264: Option<PathBuf>,
    /// `--ffmpeg`.
    pub ffmpeg: Option<PathBuf>,
    /// `--threads`.
    pub threads: Option<u32>,
    /// `--timeout`, in seconds.
    pub timeout: u64,
    /// `--strict-safe`: no FFmpeg decoder.
    pub strict_safe: StrictSafe,
}

/// The audio stream encoded to Opus, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpusRequest {
    /// The source stream; `None` for the first audio stream CoreAV decodes.
    pub stream: Option<usize>,
    /// Bits a second; `None` for 64,000 for mono and 96,000 for stereo.
    pub bitrate: Option<u32>,
}

/// The audio stream encoded to AAC-LC, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AacRequest {
    /// The source stream; `None` for the first audio stream CoreAV decodes.
    pub stream: Option<usize>,
    /// Bits a second; `None` for the `aac` encoder's own default.
    pub bitrate: Option<u32>,
}

/// The `--scale` value, or the usage message.
pub fn scale(text: &str) -> Result<Scale, String> {
    parse_scale(text).ok_or_else(|| {
        format!(
            "--scale must be W:H, each side 1 to 16384 pixels, -1 (keep the aspect ratio) or -2 (keep it, even), not '{text}'"
        )
    })
}

/// Transcodes `source` as `request` asks, writing the MP4 to `out`, which
/// the caller renames to `output` (the spool file goes next to it). `this`
/// is the running program: the sandboxed decode backends run it as their
/// worker, and the encoder runs under it as the sandbox launcher.
pub fn make<S: Source>(
    request: &Request,
    this: Option<&Path>,
    mut source: S,
    output: &Path,
    out: &mut dyn std::io::Write,
) -> Result<Value, WriteFailure> {
    let Some(this) = this else {
        return Err(WriteFailure::Rejected(Failure::Unsupported(
            "cannot find the path of the running coreav command, which runs the encoder".into(),
        )));
    };
    let matroska = crate::matroska::detect_source(&mut source).map_err(WriteFailure::Rejected)?;
    let kind = if matroska {
        SourceKind::Matroska
    } else {
        SourceKind::Mp4
    };
    let options = TranscodeOptions {
        rate: match (request.bitrate, request.crf) {
            (Some(kbps), _) => RateControl::Bitrate(kbps),
            (None, Some(q)) => RateControl::Crf(q),
            (None, None) => RateControl::Crf(23.0),
        },
        preset: request.preset.clone(),
        scale: request.scale,
        pixel_format: request.pixel_format,
        one_size: request.one_size,
        audio: match (
            &request.audio_streams,
            request.no_audio,
            request.opus,
            request.aac,
        ) {
            (_, true, _, _) => AudioMode::None,
            (_, false, Some(opus), _) => AudioMode::Opus(OpusAudio {
                stream: opus.stream,
                bitrate: opus.bitrate,
                program: this.to_path_buf(),
            }),
            (_, false, None, Some(aac)) => AudioMode::Aac(AacAudio {
                stream: aac.stream,
                bitrate: aac.bitrate,
            }),
            (Some(streams), false, None, None) => AudioMode::Streams(streams.clone()),
            (None, false, None, None) => AudioMode::Copy,
        },
        encoder: request.encoder.choice(),
        x264: request.x264.clone(),
        ffmpeg: request.ffmpeg.clone(),
        threads: request.threads,
        timeout: Duration::from_secs(request.timeout),
        ..TranscodeOptions::default()
    };
    let launcher = ExecLauncher {
        command: WorkerCommand {
            program: this.to_path_buf(),
            args: request.strict_safe.entry(EXEC_ARG),
        },
    };
    // The decoder's own thread choice; the default policy's order (native,
    // dav1d and libvpx before the fallback), as `coreav thumbnail` without
    // `--exact`.
    let decoders = crate::thumbnail::backends(Some(this), 0, request.strict_safe);
    let summary = coreav_transcode::transcode(
        &mut source,
        kind,
        &options,
        &decoders.list(),
        Some(&launcher),
        output,
        out,
    )
    .map_err(|e| decoders.explain(failure(e)))?;
    Ok(document(&summary))
}

/// The `coreav.transcode/1` summary.
pub fn document(summary: &Summary) -> Value {
    let video = &summary.video;
    let mut audio = Vec::new();
    let mut other = Vec::new();
    let mut dropped = Vec::new();
    let bytes;
    let reason = |r: coreav_isobmff_write::TrackDropReason| {
        use coreav_isobmff_write::TrackDropReason as R;
        match r {
            R::NotSelected => ("not_selected", None),
            R::CodecNotAllowed => ("codec_not_allowed", None),
            R::Unsupported { reason } => ("unsupported", Some(reason)),
        }
    };
    match &summary.remux {
        Remuxed::Mp4(remuxed) => {
            bytes = remuxed.bytes_written;
            for t in remuxed.tracks.iter().filter(|t| !t.replaced) {
                let entry = json!({
                    "stream_index": t.stream_index,
                    "codec": crate::names::codec(t.codec),
                });
                if t.kind == coreav_isobmff::TrackKind::Audio {
                    audio.push(entry);
                } else {
                    other.push(json!({
                        "stream_index": t.stream_index,
                        "kind": crate::names::kind(t.kind),
                        "codec": crate::names::codec(t.codec),
                    }));
                }
            }
            for d in &remuxed.dropped_tracks {
                let (reason, message) = reason(d.reason);
                dropped.push(json!({
                    "stream_index": d.stream_index,
                    "reason": reason,
                    "message": message,
                }));
            }
        }
        Remuxed::Matroska(remuxed) => {
            bytes = remuxed.bytes_written;
            for t in remuxed.tracks.iter().filter(|t| !t.replaced) {
                if t.kind == coreav_mkv::TrackKind::Audio {
                    audio.push(json!({
                        "stream_index": t.stream_index,
                        "codec": t.codec.name(),
                    }));
                } else {
                    other.push(json!({
                        "stream_index": t.stream_index,
                        "kind": match t.kind {
                            coreav_mkv::TrackKind::Video => "video",
                            coreav_mkv::TrackKind::Audio => "audio",
                            coreav_mkv::TrackKind::Subtitle => "subtitle",
                            coreav_mkv::TrackKind::Data => "data",
                        },
                        "codec": t.codec.name(),
                    }));
                }
            }
            for d in &remuxed.dropped_tracks {
                let (reason, message) = reason(d.reason);
                dropped.push(json!({
                    "stream_index": d.stream_index,
                    "reason": reason,
                    "message": message,
                }));
            }
        }
    }
    json!({
        "schema": "coreav.transcode/1",
        "output": {"bytes": bytes},
        "video": {
            "stream_index": video.stream_index,
            "codec": video.codec.name(),
            "frames": video.frames,
            "dropped_frames": video.dropped_frames,
            "not_presented": video.not_presented,
            "backend": video.decoder.backend.name(),
            "encoder": video.encoder,
            "width": video.width,
            "height": video.height,
            "sample_aspect": video.sample_aspect.map(|(n, d)| format!("{n}:{d}")),
            "frame_rate": {"num": video.frame_rate.num, "den": video.frame_rate.den},
        },
        "audio": audio,
        "encoded_audio": summary.audio.as_ref().map(|a| json!({
            "stream_index": a.stream_index,
            "codec": a.codec.name(),
            "sample_rate": a.sample_rate,
            "encoder": a.encoder,
            "channels": a.channels,
            "bit_rate": a.bitrate,
            "samples": a.samples,
            "pre_skip": a.pre_skip,
            "packets": a.packets,
        })),
        "other": other,
        "dropped": dropped,
    })
}

/// The failure of a transcode.
fn failure(error: coreav_transcode::Error) -> WriteFailure {
    use coreav_transcode::Error as E;
    match error {
        E::Isobmff(e) => WriteFailure::Rejected(Failure::Library(e)),
        E::Matroska(e) => WriteFailure::Rejected(Failure::Matroska(e)),
        E::Invalid(what) => WriteFailure::Rejected(Failure::Invalid(what)),
        E::Decode(e) => WriteFailure::Rejected(Failure::VideoDecode(e)),
        E::Encode(e) => WriteFailure::Rejected(Failure::Encode(e)),
        E::Unsupported(what) => WriteFailure::Rejected(Failure::Unsupported(what)),
        E::Remux(e) => writer_failure(e.into()),
        E::MatroskaRemux(e) => writer_failure(e.into()),
        E::Spool(kind) => WriteFailure::Rejected(Failure::Io(kind)),
        E::Audio(e) => WriteFailure::from(e),
    }
}

/// A failure of the MP4 writer, which runs after the encode: an I/O error
/// or a limit as it is, anything else as a failed backend (`decode`,
/// "encoding: writing the MP4: ..."), never a refusal's or a malformed
/// input's kind (`docs/design/transcode.md`, `transcode-v1.md`).
fn writer_failure(failure: WriteFailure) -> WriteFailure {
    let object = failure.to_json();
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if failure.is_io() || kind == "io" || kind == "limit" {
        return failure;
    }
    let message = object
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    WriteFailure::Object {
        json: json!({
            "schema": "coreav.error/1",
            "kind": "decode",
            "limit": null,
            "offset": null,
            "message": format!("encoding: writing the MP4: {message}"),
        }),
        io: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_failures_are_never_refusals() {
        use coreav_isobmff_write::ExtractError;
        let refused =
            WriteFailure::from(ExtractError::Source(coreav_isobmff::Error::Unsupported {
                reason: "x",
            }));
        let object = writer_failure(refused).to_json();
        assert_eq!(object["kind"], "decode");
        assert!(
            object["message"]
                .as_str()
                .unwrap()
                .starts_with("encoding: writing the MP4:")
        );
        let structure =
            WriteFailure::from(ExtractError::Source(coreav_isobmff::Error::Truncated {
                offset: 7,
            }));
        assert_eq!(writer_failure(structure).to_json()["kind"], "decode");
        let full = WriteFailure::from(ExtractError::Write(std::io::ErrorKind::StorageFull));
        let kept = writer_failure(full);
        assert!(kept.is_io());
        assert_eq!(kept.to_json()["kind"], "io");
        let encoder = Failure::Encode(coreav_decode::Error::new(
            coreav_decode::ErrorKind::Unsupported,
            "x",
        ));
        assert_eq!(encoder.to_json()["kind"], "decode");
        let late = Failure::Encode(coreav_decode::Error::new(
            coreav_decode::ErrorKind::Timeout,
            "x",
        ));
        assert_eq!(late.to_json()["limit"], "timeout");
    }

    #[test]
    fn scale_messages() {
        assert!(scale("-2:360").is_ok());
        let message = scale("0:360").unwrap_err();
        assert!(message.contains("'0:360'"), "{message}");
    }
}
