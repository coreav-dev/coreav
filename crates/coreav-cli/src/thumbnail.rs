//! `coreav thumbnail` (`docs/design/thumbnails.md`): the `--at` time, the
//! decoder backends of this build, and the `coreav.thumbnail/1` summary
//! (`docs/schema/thumbnail-v1.md`).
//!
//! The thumbnail itself (`make`) and the subcommand are built only with
//! the `thumbnail` feature, which a decoder backend feature turns on: a
//! build without a decoder cannot make one, and its code and arguments
//! would slow every other command's start-up (`docs/design/benchmarks.md`).

use coreav_decode::Codec;
use coreav_isobmff::Rational;
#[cfg(feature = "thumbnail")]
use coreav_isobmff::Source;
#[cfg(feature = "thumbnail")]
use serde_json::Value;

#[cfg(feature = "thumbnail")]
use crate::output::WriteFailure;

/// The argument after which the `coreav` binary runs as a decode worker.
pub const WORKER_ARG: &str = "__coreav_worker";

/// A time in seconds: `12`, `12.5`, `12.5s`, `1500ms`, `1:02.5` or
/// `1:02:03.5`.
pub fn parse_at(text: &str) -> Option<Rational> {
    let text = text.trim();
    if let Some(ms) = text.strip_suffix("ms") {
        let value = crate::clip::parse_time(ms)?;
        return Rational::new(value.num, value.den.checked_mul(1000)?);
    }
    let text = text.strip_suffix('s').unwrap_or(text);
    let parts: Vec<&str> = text.split(':').collect();
    match parts.as_slice() {
        [seconds] => crate::clip::parse_time(seconds),
        [minutes, seconds] => sexagesimal(&[minutes], seconds),
        [hours, minutes, seconds] => sexagesimal(&[hours, minutes], seconds),
        _ => None,
    }
}

fn sexagesimal(units: &[&str], seconds: &str) -> Option<Rational> {
    let mut whole: i128 = 0;
    for unit in units {
        if unit.is_empty() || !unit.bytes().all(|b| b.is_ascii_digit()) || unit.len() > 9 {
            return None;
        }
        whole = whole
            .checked_mul(60)?
            .checked_add(unit.parse::<i128>().ok()?)?;
    }
    let rest = crate::clip::parse_time(seconds)?;
    if rest.num >= 60 * i128::from(rest.den) {
        return None;
    }
    Rational::from_int(whole.checked_mul(60)?).checked_add(rest)
}

/// Runs the decode worker when this build has a sandboxed backend and the
/// program's first argument is [`WORKER_ARG`], or the sandbox launcher
/// (`coreav_worker::exec`, the encoder of `coreav transcode`) in any build
/// with a decoder backend when it is `coreav_worker::EXEC_ARG`; `None`
/// otherwise. Each hosts or runs only what strict-safe mode allows
/// (`crate::strict_safe`), on with `--strict-safe` right after the entry
/// argument, which every launch in the mode passes
/// (`StrictSafe::entry`), or with `COREAV_STRICT_SAFE`. A build without a
/// decoder backend does nothing here, not even read its arguments, which
/// every command's start-up would pay for (`docs/design/benchmarks.md`).
pub fn run_worker() -> Option<std::process::ExitCode> {
    #[cfg(feature = "thumbnail")]
    {
        use crate::strict_safe::{FLAG, StrictSafe};
        let mut args = std::env::args_os().skip(1).peekable();
        let first = args.next();
        #[cfg(any(feature = "ffmpeg-fallback", feature = "dav1d", feature = "libvpx"))]
        if first.as_deref() == Some(std::ffi::OsStr::new(WORKER_ARG)) {
            let strict_safe = StrictSafe::new(args.next_if(|a| a == FLAG).is_some());
            return Some(coreav_worker::worker_main(|backend| {
                strict_safe.worker(backend)
            }));
        }
        if first.as_deref() == Some(std::ffi::OsStr::new(coreav_worker::EXEC_ARG)) {
            let strict_safe = StrictSafe::new(args.next_if(|a| a == FLAG).is_some());
            return Some(coreav_worker::sandbox_exec_main(args, |program| {
                strict_safe.program(program)
            }));
        }
    }
    None
}

/// The codecs `--allow` names, by `ffprobe`'s names.
pub fn decode_codecs(names: &[String]) -> Result<Vec<Codec>, String> {
    names
        .iter()
        .map(|n| Codec::from_name(n).ok_or_else(|| n.clone()))
        .collect()
}

/// Checks the names `--allow` gives; the usage message for an unknown one.
pub fn check_allow(names: &[String]) -> Result<(), String> {
    decode_codecs(names).map(|_| ()).map_err(|unknown| {
        let known: Vec<&str> = Codec::ALL.iter().map(|c| c.name()).collect();
        format!(
            "unknown codec '{unknown}' in --allow; known: {}",
            known.join(", ")
        )
    })
}

/// What `coreav thumbnail` asks for, from its arguments.
#[derive(Clone, Debug)]
pub struct Request {
    /// The time.
    pub at: Rational,
    /// `--exact`.
    pub exact: bool,
    /// `--width`.
    pub width: Option<u32>,
    /// `--height`.
    pub height: Option<u32>,
    /// `--format`, if given.
    pub format: Option<String>,
    /// The output path, whose extension names the format otherwise.
    pub output: String,
    /// `--quality`.
    pub quality: u8,
    /// `--stream`.
    pub stream: Option<usize>,
    /// `--allow`, checked by [`check_allow`].
    pub allow: Option<Vec<String>>,
    /// `--threads`.
    pub threads: u32,
    /// `--timeout`, in seconds.
    pub timeout: u64,
    /// `--strict-safe`: no FFmpeg decoder.
    pub strict_safe: crate::strict_safe::StrictSafe,
}

/// Makes the thumbnail `request` asks for from `source`: the image and the
/// summary document. `this` is the running program, which the sandboxed
/// backends start again as their worker.
#[cfg(feature = "thumbnail")]
pub fn make<S: Source>(
    request: &Request,
    this: Option<&std::path::Path>,
    source: S,
) -> Result<(Vec<u8>, Value), WriteFailure> {
    use coreav_thumbnail::{ImageFormat, ThumbnailOptions};
    let format = request
        .format
        .as_deref()
        .or_else(|| {
            std::path::Path::new(&request.output)
                .extension()
                .and_then(|e| e.to_str())
        })
        .and_then(ImageFormat::from_name)
        .unwrap_or_default();
    let mut options = ThumbnailOptions::at(request.at);
    options.exact = request.exact;
    options.width = request.width;
    options.height = request.height;
    options.format = format;
    options.quality = request.quality;
    options.stream = request.stream;
    options.decode_limits.timeout = std::time::Duration::from_secs(request.timeout);
    if let Some(names) = &request.allow {
        let codecs = decode_codecs(names).map_err(|unknown| {
            WriteFailure::Rejected(crate::error::Failure::Unsupported(format!(
                "unknown codec {unknown}"
            )))
        })?;
        options.policy.allowlist = coreav_decode::DecodeAllowlist::new(&codecs);
    }
    options.policy.order = order(request.exact);
    let decoders = backends(this, request.threads, request.strict_safe);
    let made = coreav_thumbnail::thumbnail(source, &options, &decoders.list())
        .map_err(|e| decoders.explain(failure(e)))?;
    let document = document(&made, request.at);
    Ok((made.image, document))
}

/// Which backend to try first (`docs/design/decode.md`, "Selection";
/// `docs/spikes/h264-native-thumbnail/`).
///
/// A thumbnail at a keyframe decodes one picture, and CoreAV's own
/// decoder is the better path for it: it is 0.437 of the `ffmpeg`
/// command line and 0.986 of the sandboxed fallback, which has to start
/// a worker and an `ffmpeg` and copy the frame back. `--exact` decodes
/// every picture from the keyframe to the frame asked for, and there the
/// fallback wins by the cores it uses: the native decoder is
/// single-threaded, which costs 5.6 times the fallback's time and 4.6
/// times FFmpeg's. Until the decoder decodes on more than one thread,
/// `--exact` prefers the fallback. dav1d (AV1 only) decodes on its own
/// threads and is ahead of the fallback in both modes: through the
/// fallback, `--exact` on AV1 was 1.186 of FFmpeg's time, through dav1d
/// 0.934 (`docs/spikes/thumbnail-bench/`, "AV1 through dav1d"). libvpx
/// (VP9 only) comes where dav1d does, before the fallback in both modes
/// (`docs/design/decode.md`, "libvpx").
#[cfg(feature = "thumbnail")]
fn order(exact: bool) -> Vec<coreav_decode::BackendKind> {
    use coreav_decode::BackendKind::{Dav1d, Fallback, Libvpx, Native, Nvdec};
    if exact {
        vec![Nvdec, Dav1d, Libvpx, Fallback, Native]
    } else {
        vec![Nvdec, Native, Dav1d, Libvpx, Fallback]
    }
}

/// The decoder backends of this build: CoreAV's own H.264 decoder with
/// the `native-h264` feature, and, run by this program as its sandboxed
/// worker, dav1d with the `dav1d` feature, libvpx with the `libvpx` feature
/// and the FFmpeg fallback with the `ffmpeg-fallback` feature, which
/// strict-safe mode refuses (`crate::strict_safe`). The policy's order
/// decides between them (`docs/design/decode.md`).
#[cfg(feature = "thumbnail")]
#[cfg_attr(
    not(any(
        feature = "ffmpeg-fallback",
        feature = "native-h264",
        feature = "dav1d",
        feature = "libvpx"
    )),
    allow(unused_mut)
)]
#[cfg_attr(not(feature = "ffmpeg-fallback"), allow(unused_variables))]
pub(crate) fn backends(
    this: Option<&std::path::Path>,
    threads: u32,
    strict_safe: crate::strict_safe::StrictSafe,
) -> crate::strict_safe::Decoders {
    let mut out = crate::strict_safe::Decoders::default();
    #[cfg(feature = "native-h264")]
    out.push(Box::new(coreav_backend_h264::NativeH264::new()));
    #[cfg(any(feature = "ffmpeg-fallback", feature = "dav1d", feature = "libvpx"))]
    if let Some(program) = this {
        use coreav_worker::{SandboxedBackend, WorkerBackend, WorkerCommand, WorkerOptions};
        let options = WorkerOptions {
            threads,
            ..WorkerOptions::new(WorkerCommand {
                program: program.to_path_buf(),
                args: strict_safe.entry(WORKER_ARG),
            })
        };
        #[cfg(feature = "dav1d")]
        out.push_sandboxed(
            strict_safe,
            SandboxedBackend {
                backend: WorkerBackend::Dav1d,
                options: options.clone(),
            },
        );
        #[cfg(feature = "libvpx")]
        out.push_sandboxed(
            strict_safe,
            SandboxedBackend {
                backend: WorkerBackend::Libvpx,
                options: options.clone(),
            },
        );
        #[cfg(feature = "ffmpeg-fallback")]
        out.push_ffmpeg(
            strict_safe,
            SandboxedBackend {
                backend: WorkerBackend::Ffmpeg,
                options,
            },
        );
    }
    out
}

/// The summary document.
#[cfg(feature = "thumbnail")]
fn document(thumbnail: &coreav_thumbnail::Thumbnail, requested: Rational) -> Value {
    use crate::native::rational;
    use crate::time::to_f64;
    serde_json::json!({
        "schema": "coreav.thumbnail/1",
        "output": {
            "bytes": thumbnail.image.len(),
            "format": thumbnail.format.name(),
            "width": thumbnail.width,
            "height": thumbnail.height,
        },
        "time": {
            "requested": rational(requested),
            "frame": rational(thumbnail.time),
            "frame_seconds": to_f64(thumbnail.time),
            "decode_start": rational(thumbnail.decode_start),
        },
        "stream_index": thumbnail.stream_index,
        "codec": thumbnail.codec.name(),
        "keyframe": thumbnail.keyframe,
        "packet": {
            "offset": thumbnail.packet.0,
            "size": thumbnail.packet.1,
        },
        "packets_decoded": thumbnail.packets_decoded,
        "decoder": {
            "backend": thumbnail.decoder.backend.name(),
            "name": thumbnail.decoder.decoder,
            "isolation": match thumbnail.decoder.isolation {
                coreav_decode::Isolation::InProcess => "in_process",
                coreav_decode::Isolation::Sandboxed => "sandboxed",
            },
            "fell_back": thumbnail.decoder.fell_back(),
        },
        "warnings": thumbnail.warnings,
        "skipped": thumbnail.skipped.iter().map(|s| serde_json::json!({
            "backend": s.backend.name(),
            "reason": s.reason,
        })).collect::<Vec<_>>(),
    })
}

/// The failure of a thumbnail.
#[cfg(feature = "thumbnail")]
fn failure(error: coreav_thumbnail::Error) -> WriteFailure {
    use crate::error::Failure;
    use coreav_thumbnail::Error;
    let failure = match error {
        Error::Isobmff(e) => Failure::Library(e),
        Error::Matroska(e) => Failure::Matroska(e),
        Error::Invalid(what) => Failure::Invalid(what),
        Error::Decode(e) => Failure::VideoDecode(e),
        Error::TimeOutOfRange => Failure::OutOfRange,
        Error::NoVideo => Failure::Unsupported("no video stream".into()),
        Error::Unsupported(what) => Failure::Unsupported(what),
        Error::Frame(e) => Failure::Unsupported(e.to_string()),
        Error::Encode(what) => Failure::Unsupported(format!("encoding the image: {what}")),
    };
    WriteFailure::Rejected(failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        let r = |n, d| Rational::new(n, d);
        assert_eq!(parse_at("10"), r(10, 1));
        assert_eq!(parse_at("10s"), r(10, 1));
        assert_eq!(parse_at("10.5"), r(21, 2));
        assert_eq!(parse_at("1500ms"), r(3, 2));
        assert_eq!(parse_at("1:02.5"), r(125, 2));
        assert_eq!(parse_at("1:00:00"), r(3600, 1));
        assert_eq!(parse_at("1:60"), None);
        assert_eq!(parse_at("-1"), None);
        assert_eq!(parse_at("a"), None);
        assert_eq!(parse_at("1::2"), None);
    }

    /// What `--exact` changes about which decoder runs
    /// (`docs/spikes/h264-native-thumbnail/`).
    #[cfg(feature = "thumbnail")]
    #[test]
    fn exact_prefers_the_sandboxed_decoder() {
        use coreav_decode::BackendKind::{Fallback, Native, Nvdec};
        use coreav_decode::Policy;
        let keyframe = order(false);
        let exact = order(true);
        let at = |o: &[coreav_decode::BackendKind], k| o.iter().position(|b| *b == k);
        // A thumbnail at a keyframe decodes one picture, where CoreAV's
        // own decoder is ahead; `--exact` decodes every picture from the
        // keyframe, where the sandboxed FFmpeg uses every core.
        assert!(at(&keyframe, Native) < at(&keyframe, Fallback));
        assert!(at(&exact, Fallback) < at(&exact, Native));
        // Hardware first either way, and every backend is still there:
        // an order is a preference, not an allowlist.
        assert_eq!(keyframe.first(), Some(&Nvdec));
        assert_eq!(exact.first(), Some(&Nvdec));
        for order in [&keyframe, &exact] {
            assert_eq!(order.len(), Policy::default().order.len());
            for backend in &Policy::default().order {
                assert!(order.contains(backend), "{backend:?}");
            }
        }
    }
}
