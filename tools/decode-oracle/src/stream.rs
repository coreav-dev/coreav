//! `decode-oracle stream FILE`: decodes the whole video stream of an MP4,
//! MOV, Matroska or WebM file as `coreav transcode` would, and prints one
//! JSON object: `stream_index`, `codec`, the `backend` that opened the
//! stream and its `decoder`, every frame's `pts` and `md5` in the order the
//! decoder returned them (presentation order), and the `outcome`: `"ok"`, or
//! `{stage, kind, message, packets_sent}` of the failure (V1 of
//! `docs/measurements/compat-0.1.0/codec-coverage-plan.md`, "CoreAV's side").
//! Exit status 0 with outcome `ok`, 1 otherwise; the object is printed either
//! way.
//!
//! The stream is the one `coreav_thumbnail::video_stream` chooses, the
//! backends are the full build's in the CLI's order (CoreAV's H.264 decoder,
//! then dav1d and libvpx in the sandboxed worker) with strict-safe mode on,
//! and the decode limits are the ones a transcode applies to a whole stream.
//! The wall-clock limit is the caller's: the decoder's own deadline is a week.
//!
//! `stage` is `read` (the file, the stream or a packet's bytes), `open`,
//! `packet` (sending a packet or taking the frames after it) or `eof`.
//! `kind` is a `coreav-decode` error kind at the last three, and the
//! `coreav.error/1` kind the CLI would print at `read`. `packets_sent` counts
//! the calls of `send_packet`, a failing one included.

use std::path::Path;
use std::time::Duration;

use coreav_decode::{Backend, Decoder, Limits, Packet, Policy};
use coreav_isobmff::{BufferedSource, FileSource, Rational};
use coreav_thumbnail::ThumbnailOptions;
use coreav_worker::WorkerBackend;
use md5::{Digest, Md5};
use serde_json::{Value, json};

/// The argument after the worker's entry that turns strict-safe mode on,
/// as the `coreav` binary passes it (`crate::strict_safe` there).
pub const STRICT_SAFE_ARG: &str = "--strict-safe";

/// The decoder's own deadline: far past the runner's, which is the clock.
const TIMEOUT: Duration = Duration::from_secs(7 * 24 * 3600);

struct Failure {
    stage: &'static str,
    kind: String,
    message: String,
}

#[derive(Default)]
struct Report {
    stream_index: Option<usize>,
    codec: Option<&'static str>,
    backend: Option<&'static str>,
    decoder: Option<String>,
    frames: Vec<Value>,
    packets_sent: usize,
}

/// Strict-safe mode's rule for the worker: never the FFmpeg fallback, and
/// no sandboxed decoder where the memory limit is not enforced.
pub fn worker_allows(backend: WorkerBackend, strict_safe: bool) -> Result<(), String> {
    if !strict_safe {
        return Ok(());
    }
    if backend == WorkerBackend::Ffmpeg {
        return Err("strict-safe mode never runs the ffmpeg program".into());
    }
    if !coreav_worker::enforces_memory_limit() {
        return Err("strict-safe: no enforceable memory limit for the sandbox".into());
    }
    Ok(())
}

pub fn run(args: &[String]) -> Result<(Value, bool), String> {
    let [file] = args else {
        return Err("usage: decode-oracle stream FILE".into());
    };
    let mut report = Report::default();
    let failure = decode(Path::new(file), &mut report).err();
    let ok = failure.is_none();
    let outcome = failure.map_or(json!("ok"), |f| {
        json!({
            "stage": f.stage,
            "kind": f.kind,
            "message": f.message,
            "packets_sent": report.packets_sent,
        })
    });
    let document = json!({
        "stream_index": report.stream_index,
        "codec": report.codec,
        "backend": report.backend,
        "decoder": report.decoder,
        "frames": report.frames,
        "outcome": outcome,
    });
    Ok((document, ok))
}

fn decode(file: &Path, report: &mut Report) -> Result<(), Failure> {
    let mut source = BufferedSource::new(
        FileSource::open(file).map_err(|e| read_failure(coreav_thumbnail::Error::Isobmff(e)))?,
    );
    let demux = ThumbnailOptions::at(Rational::from_int(0));
    let video = coreav_thumbnail::video_stream(&mut source, &demux).map_err(read_failure)?;
    report.stream_index = Some(video.stream_index);
    report.codec = Some(video.stream.codec.name());
    let mut limits = Limits::default();
    limits.timeout = TIMEOUT;
    limits.max_decoded_bytes = coreav_transcode::pipeline::decoded_bytes_cap(
        &limits,
        video.packets.len(),
        video
            .stream
            .width
            .zip(video.stream.height)
            .filter(|&(w, h)| w > 0 && h > 0),
    );
    let owned = backends();
    let list: Vec<&dyn Backend> = owned.iter().map(|b| b.as_ref()).collect();
    let opened = coreav_decode::open(&video.stream, &Policy::default(), &limits, &list)
        .map_err(|e| decode_failure("open", &e))?;
    let mut decoder = opened.decoder;
    report.backend = Some(decoder.info().backend.name());
    report.decoder = Some(decoder.info().decoder.clone());
    for (index, packet) in video.packets.iter().enumerate() {
        let data = video
            .read_packet(&mut source, index, limits.max_packet_bytes)
            .map_err(read_failure)?;
        report.packets_sent += 1;
        decoder
            .send_packet(&Packet {
                data: &data,
                pts: Some(packet.pts),
                dts: packet.dts,
                keyframe: packet.keyframe,
                discard: false,
            })
            .map_err(|e| decode_failure("packet", &e))?;
        drain(decoder.as_mut(), &mut report.frames).map_err(|e| decode_failure("packet", &e))?;
    }
    decoder.send_eof().map_err(|e| decode_failure("eof", &e))?;
    drain(decoder.as_mut(), &mut report.frames).map_err(|e| decode_failure("eof", &e))
}

fn drain(decoder: &mut dyn Decoder, frames: &mut Vec<Value>) -> Result<(), coreav_decode::Error> {
    while let Some(frame) = decoder.receive_frame()? {
        let picture = frame.to_cpu()?;
        let mut packed = Vec::new();
        picture.write_packed(&mut packed)?;
        let hex: String = Md5::digest(&packed)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        frames.push(json!({ "pts": frame.pts, "md5": hex }));
    }
    Ok(())
}

fn decode_failure(stage: &'static str, error: &coreav_decode::Error) -> Failure {
    Failure {
        stage,
        kind: error.kind.name().into(),
        message: error.message.clone(),
    }
}

/// The failure of the container glue, as the `coreav.error/1` object the
/// CLI would print for it (`kind`, `message`).
fn read_failure(error: coreav_thumbnail::Error) -> Failure {
    use coreav_thumbnail::Error as E;
    let (kind, message) = match error {
        E::Isobmff(e) => (isobmff_kind(&e), e.to_string()),
        E::Matroska(e) => (matroska_kind(&e), e.to_string()),
        E::Invalid(what) => ("invalid_structure", format!("invalid structure: {what}")),
        E::Decode(e) => (e.kind.name(), e.message),
        E::TimeOutOfRange => (
            "out_of_range",
            "the time is at or after the end of the video".into(),
        ),
        E::NoVideo => ("unsupported_feature", "unsupported: no video stream".into()),
        E::Unsupported(what) => ("unsupported_feature", format!("unsupported: {what}")),
        E::Frame(e) => ("unsupported_feature", format!("unsupported: {e}")),
        E::Encode(what) => (
            "unsupported_feature",
            format!("unsupported: encoding the image: {what}"),
        ),
    };
    Failure {
        stage: "read",
        kind: kind.into(),
        message,
    }
}

fn isobmff_kind(error: &coreav_isobmff::Error) -> &'static str {
    use coreav_isobmff::Error as E;
    match error {
        E::Truncated { .. } => "truncated_structure",
        E::InvalidBox { .. } | E::Overflow { .. } | E::InvalidStream { .. } => "invalid_structure",
        E::LimitExceeded { .. } => "limit",
        E::NotIsobmff => "unsupported_container",
        E::Unsupported { .. } | E::NoSuchTrack => "unsupported_feature",
        E::CodecNotAllowed { .. } => "codec_not_allowed",
        E::Io(_) => "io",
    }
}

fn matroska_kind(error: &coreav_mkv::Error) -> &'static str {
    use coreav_mkv::Error as E;
    match error {
        E::Truncated { .. } => "truncated_structure",
        E::InvalidElement { .. } | E::Overflow { .. } => "invalid_structure",
        E::LimitExceeded { .. } => "limit",
        E::NotMatroska => "unsupported_container",
        E::Unsupported { .. } => "unsupported_feature",
        E::CodecNotAllowed { .. } => "codec_not_allowed",
        E::Io(_) => "io",
    }
}

/// The backends of the full build that this one has, in the CLI's order
/// (`coreav_cli::thumbnail::backends`); the policy's order decides between
/// them. In strict-safe mode a sandboxed backend is left out where the
/// sandbox cannot enforce its memory limit.
#[allow(unused_mut, clippy::vec_init_then_push)]
fn backends() -> Vec<Box<dyn Backend>> {
    let mut out: Vec<Box<dyn Backend>> = Vec::new();
    #[cfg(feature = "native-h264")]
    out.push(Box::new(coreav_backend_h264::NativeH264::new()));
    #[cfg(any(feature = "dav1d", feature = "libvpx"))]
    if coreav_worker::enforces_memory_limit()
        && let Ok(program) = std::env::current_exe()
    {
        use coreav_worker::{SandboxedBackend, WorkerCommand, WorkerOptions};
        let options = WorkerOptions::new(WorkerCommand {
            program,
            args: vec![coreav_worker::WORKER_ARG.into(), STRICT_SAFE_ARG.into()],
        });
        #[cfg(feature = "dav1d")]
        out.push(Box::new(SandboxedBackend {
            backend: WorkerBackend::Dav1d,
            options: options.clone(),
        }));
        #[cfg(feature = "libvpx")]
        out.push(Box::new(SandboxedBackend {
            backend: WorkerBackend::Libvpx,
            options,
        }));
    }
    out
}
