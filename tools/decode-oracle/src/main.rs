//! `decode-oracle FILE [--packets N] [--backend ffmpeg|dav1d|libvpx|native] [--transfer shared|pipe]`
//!
//! Decodes the first `N` packets (default 120) of the first video stream of
//! an MP4, MOV, Matroska or WebM file through CoreAV's decode boundary, and
//! prints one JSON object: the frames returned, in presentation order, each
//! with its pts and the MD5 of its planes packed without padding (the layout
//! FFmpeg's `framemd5` hashes), plus `cutoff`, the smallest pts of the
//! packets that follow (`docs/design/decode.md`, bar 1). Test-only, never
//! shipped; it runs itself as the sandboxed worker. `--backend native`
//! (feature `native-h264`) decodes H.264 with CoreAV's own decoder, in
//! this process and with no other backend to fall back to: the native
//! conformance of `coreav transcode`'s output (`docs/design/transcode.md`,
//! bar B1).
//!
//! `decode-oracle thumbnail FILE --at SECONDS --skip on|off -o OUT.png`
//! writes the `--exact` thumbnail at the display size, as PNG, with
//! disposable packets skipped or not (`docs/design/thumbnails.md`, "Frames
//! nothing depends on"), and prints the frame's time and the packets sent.

use std::path::PathBuf;
use std::process::ExitCode;

use coreav_decode::{Backend, Decoder, Limits, Packet, Policy};
use coreav_isobmff::{BufferedSource, FileSource, Rational};
use coreav_thumbnail::{ThumbnailOptions, first_packets};
use coreav_worker::{SandboxedBackend, TransferMode, WorkerBackend, WorkerCommand, WorkerOptions};
use md5::{Digest, Md5};
use serde_json::{Value, json};

/// Packets after the decoded ones read to find the cutoff.
const LOOKAHEAD: usize = 64;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(coreav_worker::WORKER_ARG) {
        return coreav_worker::worker_main(|_| Ok(()));
    }
    let result = if args.first().map(String::as_str) == Some("thumbnail") {
        run_thumbnail(args.get(1..).unwrap_or(&[]))
    } else if args.first().map(String::as_str) == Some("choose") {
        run_choose(args.get(1..).unwrap_or(&[]))
    } else {
        run(&args)
    };
    match result {
        Ok(document) => {
            println!("{document}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            println!("{}", json!({ "error": message }));
            ExitCode::from(1)
        }
    }
}

fn run(args: &[String]) -> Result<Value, String> {
    let mut file = None;
    let mut packets = 120usize;
    let mut transfer = TransferMode::Shared;
    let mut backend = WorkerBackend::Ffmpeg;
    let mut native = false;
    let mut in_process = false;
    let mut max_memory = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--packets" => {
                packets = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--packets takes a number")?;
            }
            "--transfer" => {
                transfer = match it.next().map(String::as_str) {
                    Some("pipe") => TransferMode::Pipe,
                    Some("shared") => TransferMode::Shared,
                    _ => return Err("--transfer takes shared or pipe".into()),
                };
            }
            "--backend" => {
                native = false;
                backend = match it.next().map(String::as_str) {
                    Some("ffmpeg") => WorkerBackend::Ffmpeg,
                    Some("dav1d") => WorkerBackend::Dav1d,
                    Some("libvpx") => WorkerBackend::Libvpx,
                    Some("native") => {
                        native = true;
                        WorkerBackend::Ffmpeg
                    }
                    _ => return Err("--backend takes ffmpeg, dav1d, libvpx or native".into()),
                };
            }
            // The backend without the worker: for debugging, and for the
            // worker's overhead (docs/design/decode.md, bar 5).
            "--in-process" => in_process = true,
            // A larger memory budget for each sandboxed decoder process, in
            // GiB, on a host that has it: for frames too large for the
            // default with 120 packets sent ahead.
            "--max-memory-gib" => {
                let gib: u64 = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&gib| gib > 0)
                    .ok_or("--max-memory-gib takes a positive number")?;
                max_memory = Some(gib.saturating_mul(1 << 30));
            }
            other => file = Some(PathBuf::from(other)),
        }
    }
    let file = file.ok_or(
        "usage: decode-oracle FILE [--packets N] [--backend ffmpeg|dav1d|libvpx|native] [--in-process] \
         [--transfer shared|pipe] [--max-memory-gib N]",
    )?;
    let source = FileSource::open(&file).map_err(|e| e.to_string())?;
    let mut options = ThumbnailOptions::at(Rational::from_int(0));
    options.policy = Policy::default();
    // A whole run of up to 120 frames of up to 8K, its packets sent ahead
    // of the decoder: past the per-request defaults, which suit one
    // thumbnail (`docs/design/decode.md`, "Threads and backpressure").
    options.decode_limits.max_decoded_bytes = 64 << 30;
    options.decode_limits.max_pending_bytes = 16 << 30;
    options.decode_limits.timeout = std::time::Duration::from_secs(900);
    if let Some(bytes) = max_memory {
        options.decode_limits.max_memory_bytes = bytes;
    }
    let (stream_index, stream, list) = first_packets(
        BufferedSource::new(source),
        &options,
        packets.saturating_add(LOOKAHEAD),
    )
    .map_err(|e| e.to_string())?;
    let (fed, rest) = list.split_at(packets.min(list.len()));
    let cutoff = rest.iter().map(|p| p.pts).min();
    let program = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut worker = WorkerOptions::new(WorkerCommand {
        program,
        args: vec![coreav_worker::WORKER_ARG.into()],
    });
    worker.transfer = transfer;
    // The worker options' cap would otherwise hold the budget below it.
    if max_memory.is_some() {
        worker.address_space = max_memory;
    }
    let sandboxed = SandboxedBackend {
        backend,
        options: worker,
    };
    let native_backend = native_h264()?;
    let backends: [&dyn Backend; 1] = if native {
        [native_backend.as_ref()]
    } else {
        [&sandboxed]
    };
    let mut opened: Box<dyn Decoder> = if native {
        coreav_decode::open(&stream, &options.policy, &options.decode_limits, &backends)
            .map_err(|e| e.to_string())?
            .decoder
    } else if in_process && backend == WorkerBackend::Dav1d {
        in_process_dav1d(&stream, &options.decode_limits)?
    } else if in_process && backend == WorkerBackend::Libvpx {
        in_process_libvpx(&stream, &options.decode_limits)?
    } else if in_process {
        let program = coreav_worker::find_ffmpeg().ok_or("no ffmpeg")?;
        let options = coreav_backend_ffmpeg::FfmpegOptions {
            program,
            threads: None,
        };
        Box::new(
            coreav_backend_ffmpeg::FfmpegDecoder::open(
                &options,
                &stream,
                &Limits {
                    max_decoded_bytes: 64 << 30,
                    ..Limits::default()
                },
            )
            .map_err(|e| e.to_string())?,
        )
    } else {
        coreav_decode::open(&stream, &options.policy, &options.decode_limits, &backends)
            .map_err(|e| e.to_string())?
            .decoder
    };
    let mut frames = Vec::new();
    for p in fed {
        opened
            .send_packet(&Packet {
                data: &p.data,
                pts: Some(p.pts),
                dts: p.dts,
                keyframe: p.keyframe,
                discard: false,
            })
            .map_err(|e| e.to_string())?;
        drain(opened.as_mut(), &mut frames)?;
    }
    opened.send_eof().map_err(|e| e.to_string())?;
    drain(opened.as_mut(), &mut frames)?;
    Ok(json!({
        "codec": stream.codec.name(),
        "stream_index": stream_index,
        "decoder": opened.info().decoder,
        "fed": fed.len(),
        "cutoff": cutoff,
        "frames": frames,
    }))
}

fn drain(decoder: &mut dyn Decoder, frames: &mut Vec<Value>) -> Result<(), String> {
    while let Some(frame) = decoder.receive_frame().map_err(|e| e.to_string())? {
        let picture = frame.to_cpu().map_err(|e| e.to_string())?;
        let mut packed = Vec::new();
        picture
            .write_packed(&mut packed)
            .map_err(|e| e.to_string())?;
        let digest = Md5::digest(&packed);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        frames.push(json!({
            "pts": frame.pts,
            "width": picture.width(),
            "height": picture.height(),
            "format": format!("{:?}", picture.format()),
            "md5": hex,
        }));
    }
    Ok(())
}

fn run_thumbnail(args: &[String]) -> Result<Value, String> {
    let mut file = None;
    let mut at = None;
    let mut skip = true;
    let mut exact = true;
    let mut width = None;
    let mut backend = WorkerBackend::Ffmpeg;
    let mut in_process = false;
    let mut out = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--at" => {
                let seconds: f64 = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|s: &f64| s.is_finite() && *s >= 0.0 && *s < 1e12)
                    .ok_or("--at takes seconds")?;
                // Milliseconds, as the measurements give them.
                at = Rational::new((seconds * 1000.0).round() as i128, 1000);
            }
            "--skip" => {
                skip = match it.next().map(String::as_str) {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("--skip takes on or off".into()),
                };
            }
            // Keyframe mode and a width, as `coreav thumbnail` without
            // `--exact` (the worker-overhead bar, docs/design/decode.md).
            "--keyframe" => exact = false,
            "--width" => {
                width = Some(
                    it.next()
                        .and_then(|v| v.parse().ok())
                        .ok_or("--width takes a number")?,
                );
            }
            "--backend" => {
                backend = match it.next().map(String::as_str) {
                    Some("ffmpeg") => WorkerBackend::Ffmpeg,
                    Some("dav1d") => WorkerBackend::Dav1d,
                    Some("libvpx") => WorkerBackend::Libvpx,
                    _ => return Err("--backend takes ffmpeg, dav1d or libvpx".into()),
                };
            }
            // dav1d in this process instead of the worker.
            "--in-process" => in_process = true,
            "-o" => out = it.next().map(PathBuf::from),
            other => file = Some(PathBuf::from(other)),
        }
    }
    let (file, at, out) = match (file, at, out) {
        (Some(f), Some(a), Some(o)) => (f, a, o),
        _ => {
            return Err(
                "usage: decode-oracle thumbnail FILE --at SECONDS [--skip on|off] [--keyframe] \
                 [--width N] [--backend ffmpeg|dav1d|libvpx] [--in-process] -o OUT"
                    .into(),
            );
        }
    };
    let source = FileSource::open(&file).map_err(|e| e.to_string())?;
    let mut options = ThumbnailOptions::at(at);
    options.exact = exact;
    options.skip_disposable = skip;
    options.width = width;
    options.format = if out.extension().is_some_and(|e| e == "jpg") {
        coreav_thumbnail::ImageFormat::Jpeg
    } else {
        coreav_thumbnail::ImageFormat::Png
    };
    let program = std::env::current_exe().map_err(|e| e.to_string())?;
    let sandboxed = SandboxedBackend {
        backend,
        options: WorkerOptions::new(WorkerCommand {
            program,
            args: vec![coreav_worker::WORKER_ARG.into()],
        }),
    };
    let in_process_backend = in_process_dav1d_backend()?;
    let backends: [&dyn Backend; 1] = if in_process {
        if backend != WorkerBackend::Dav1d {
            return Err("--in-process takes --backend dav1d".into());
        }
        [in_process_backend.as_ref()]
    } else {
        [&sandboxed]
    };
    let made = coreav_thumbnail::thumbnail(BufferedSource::new(source), &options, &backends)
        .map_err(|e| e.to_string())?;
    std::fs::write(&out, &made.image).map_err(|e| e.to_string())?;
    Ok(json!({
        "frame": [made.time.num.to_string(), made.time.den.to_string()],
        "packets_decoded": made.packets_decoded,
    }))
}

/// `choose FILE --at S1,S2,... [--exact] --seek on|off`: what a thumbnail
/// at each time would decode, without decoding (`scripts/seek_check.py`).
fn run_choose(args: &[String]) -> Result<Value, String> {
    let mut file = None;
    let mut times = Vec::new();
    let mut exact = false;
    let mut seek = true;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--at" => {
                for value in it.next().ok_or("--at takes seconds")?.split(',') {
                    let seconds: f64 = value
                        .parse()
                        .ok()
                        .filter(|s: &f64| s.is_finite() && *s >= 0.0 && *s < 1e12)
                        .ok_or("--at takes seconds")?;
                    times.push(
                        Rational::new((seconds * 1000.0).round() as i128, 1000)
                            .ok_or("--at takes seconds")?,
                    );
                }
            }
            "--exact" => exact = true,
            "--seek" => {
                seek = match it.next().map(String::as_str) {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("--seek takes on or off".into()),
                };
            }
            other => file = Some(PathBuf::from(other)),
        }
    }
    let file =
        file.ok_or("usage: decode-oracle choose FILE --at S1,S2,... [--exact] --seek on|off")?;
    let mut out = Vec::new();
    for at in times {
        let source = FileSource::open(&file).map_err(|e| e.to_string())?;
        let mut options = ThumbnailOptions::at(at);
        options.exact = exact;
        options.seek = seek;
        out.push(
            match coreav_thumbnail::oracle_choice(BufferedSource::new(source), &options) {
                Ok(c) => json!({
                    "start": c.start,
                    "target": c.target,
                    "time": c.time.map(|t| [t.num.to_string(), t.den.to_string()]),
                    "packets": c.packets,
                    "windowed": c.windowed,
                    "first_keyframe": c.first_keyframe,
                }),
                Err(e) => json!({ "error": e.to_string() }),
            },
        );
    }
    Ok(Value::Array(out))
}

#[cfg(feature = "dav1d")]
fn in_process_dav1d(
    stream: &coreav_decode::StreamConfig,
    limits: &Limits,
) -> Result<Box<dyn Decoder>, String> {
    let options = coreav_backend_dav1d::Dav1dOptions::default();
    coreav_backend_dav1d::Dav1dDecoder::open(&options, stream, limits)
        .map(|d| Box::new(d) as Box<dyn Decoder>)
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "dav1d"))]
fn in_process_dav1d(
    _: &coreav_decode::StreamConfig,
    _: &Limits,
) -> Result<Box<dyn Decoder>, String> {
    Err("built without the dav1d feature".into())
}

#[cfg(feature = "libvpx")]
fn in_process_libvpx(
    stream: &coreav_decode::StreamConfig,
    limits: &Limits,
) -> Result<Box<dyn Decoder>, String> {
    let options = coreav_backend_libvpx::LibvpxOptions::default();
    coreav_backend_libvpx::LibvpxDecoder::open(&options, stream, limits)
        .map(|d| Box::new(d) as Box<dyn Decoder>)
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "libvpx"))]
fn in_process_libvpx(
    _: &coreav_decode::StreamConfig,
    _: &Limits,
) -> Result<Box<dyn Decoder>, String> {
    Err("built without the libvpx feature".into())
}

/// dav1d in this process, for `thumbnail --in-process`.
#[cfg(feature = "dav1d")]
fn in_process_dav1d_backend() -> Result<Box<dyn Backend>, String> {
    Ok(Box::new(coreav_backend_dav1d::Dav1dBackend::default()))
}

#[cfg(not(feature = "dav1d"))]
fn in_process_dav1d_backend() -> Result<Box<dyn Backend>, String> {
    Ok(Box::new(Unavailable(coreav_decode::BackendKind::Dav1d)))
}

/// The native H.264 backend, for `--backend native`.
#[cfg(feature = "native-h264")]
fn native_h264() -> Result<Box<dyn Backend>, String> {
    Ok(Box::new(coreav_backend_h264::NativeH264::new()))
}

/// Without the feature, a backend that refuses every stream, so
/// `--backend native` fails rather than falls back.
#[cfg(not(feature = "native-h264"))]
fn native_h264() -> Result<Box<dyn Backend>, String> {
    Ok(Box::new(Unavailable(coreav_decode::BackendKind::Native)))
}

/// A backend that supports nothing, in a build without it.
#[cfg(not(all(feature = "dav1d", feature = "native-h264")))]
struct Unavailable(coreav_decode::BackendKind);

#[cfg(not(all(feature = "dav1d", feature = "native-h264")))]
impl Backend for Unavailable {
    fn kind(&self) -> coreav_decode::BackendKind {
        self.0
    }

    fn supports(
        &self,
        _: &coreav_decode::StreamConfig,
        _: &coreav_decode::StreamTraits,
    ) -> coreav_decode::Support {
        coreav_decode::Support::No(format!("built without the {:?} backend", self.0))
    }

    fn open(
        &self,
        _: &coreav_decode::StreamConfig,
        _: &Limits,
    ) -> Result<Box<dyn Decoder>, coreav_decode::Error> {
        Err(coreav_decode::Error::new(
            coreav_decode::ErrorKind::Unsupported,
            format!("built without the {:?} backend", self.0),
        ))
    }
}
