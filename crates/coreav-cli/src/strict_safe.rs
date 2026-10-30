//! Strict-safe mode (`docs/design/compat-cli.md`, "Strict-safe mode"):
//! `coreav --strict-safe` or `COREAV_STRICT_SAFE`, for services that read
//! input they did not make. In it CoreAV never runs an `ffmpeg` or
//! `ffprobe` program, in any build and for any command: not in place of a
//! `coreav ffprobe` or `coreav ffmpeg` command that is not native, not as
//! the decoder, not as the encoder.
//!
//! [`StrictSafe`] decides it, and nothing else does: the command-line
//! fallback asks [`StrictSafe::on`], a transcode's encoder is an
//! [`Encoder`], which only [`StrictSafe::encoder`] makes, and the decoder
//! backends are a [`Decoders`], to which only [`Decoders::push_ffmpeg`] adds the
//! FFmpeg fallback. The hidden entries this program runs as
//! (`crate::thumbnail::run_worker`) ask it too: the decode worker
//! [`StrictSafe::worker`], the sandbox launcher [`StrictSafe::program`].

#[cfg(feature = "thumbnail")]
use std::path::Path;
#[cfg(feature = "thumbnail")]
use std::sync::{Arc, OnceLock};

#[cfg(feature = "thumbnail")]
use coreav_decode::{Backend, Codec};
#[cfg(feature = "transcode")]
use coreav_transcode::encoder::{EncoderChoice, find_encoder};

#[cfg(feature = "thumbnail")]
use crate::error::Failure;
#[cfg(feature = "thumbnail")]
use crate::output::WriteFailure;

/// How to turn the mode off, the way out every refusal names.
#[cfg(feature = "thumbnail")]
const OFF: &str = "or run without --strict-safe and COREAV_STRICT_SAFE";

/// Whether this run is in strict-safe mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrictSafe {
    on: bool,
}

impl StrictSafe {
    /// The mode of a run given `--strict-safe` (`flag`) or not: on with the
    /// flag, or when `COREAV_STRICT_SAFE` is set to anything but empty and
    /// `0`, so that a mistyped value turns it on.
    pub fn new(flag: bool) -> Self {
        let on = flag
            || std::env::var_os("COREAV_STRICT_SAFE").is_some_and(|v| !v.is_empty() && v != "0");
        Self { on }
    }

    /// Whether the mode is on.
    pub fn on(self) -> bool {
        self.on
    }

    /// Whether the decode worker hosts `backend`: in strict-safe mode not
    /// the FFmpeg fallback. Err is the refusal's message.
    #[cfg(any(feature = "ffmpeg-fallback", feature = "dav1d", feature = "libvpx"))]
    pub fn worker(self, backend: coreav_worker::WorkerBackend) -> Result<(), String> {
        if self.on && backend == coreav_worker::WorkerBackend::Ffmpeg {
            return Err(format!(
                "strict-safe mode never runs the ffmpeg program, so the decode worker does not \
                 host the FFmpeg fallback, {OFF}"
            ));
        }
        Ok(())
    }

    /// Whether the sandbox launcher runs `program`: in strict-safe mode not
    /// an `ffmpeg` or `ffprobe` program, one whose file name, or the name
    /// of the file it links to, begins with `ffmpeg` or `ffprobe` in any
    /// case. Err is the refusal's message.
    #[cfg(feature = "thumbnail")]
    pub fn program(self, program: &Path) -> Result<(), String> {
        if !self.on {
            return Ok(());
        }
        let named = |path: &Path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy().to_ascii_lowercase();
                name.starts_with("ffmpeg") || name.starts_with("ffprobe")
            })
        };
        let linked = std::fs::canonicalize(program).is_ok_and(|target| named(&target));
        if named(program) || linked {
            return Err(format!(
                "strict-safe mode never runs the ffmpeg program, so it does not run {}, {OFF}",
                program.display()
            ));
        }
        Ok(())
    }

    /// The encoder `--encoder` names (`auto`, `x264` or `ffmpeg`), where
    /// the x264 program is the one `--x264` names, if any. In strict-safe
    /// mode `ffmpeg` is refused, and `auto` is the x264 program alone,
    /// refused when there is none; both before the input is read. Err is
    /// the refusal's message.
    #[cfg(feature = "transcode")]
    pub fn encoder(self, name: &str, x264: Option<&Path>) -> Result<Encoder, String> {
        let choice = match name {
            "x264" => EncoderChoice::X264,
            "ffmpeg" => EncoderChoice::Ffmpeg,
            _ => EncoderChoice::Auto,
        };
        if !self.on {
            return Ok(Encoder(choice));
        }
        match choice {
            EncoderChoice::X264 => Ok(Encoder(EncoderChoice::X264)),
            EncoderChoice::Ffmpeg => Err(format!(
                "strict-safe mode never runs the ffmpeg program, so it cannot be the encoder: \
                 use --encoder x264 with an x264 program, {OFF}"
            )),
            EncoderChoice::Auto => {
                match find_encoder(EncoderChoice::X264, x264.map(Path::to_path_buf), None) {
                    Some(_) => Ok(Encoder(EncoderChoice::X264)),
                    None => Err(format!(
                        "strict-safe mode never runs the ffmpeg program, so the encoder is an \
                         x264 program, and none was found: install x264 (on PATH, or named by \
                         COREAV_X264 or coreav transcode's --x264), {OFF}"
                    )),
                }
            }
        }
    }
}

/// The encoder program of a transcode, as [`StrictSafe::encoder`] allows it.
#[cfg(feature = "transcode")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Encoder(EncoderChoice);

#[cfg(feature = "transcode")]
impl Encoder {
    /// The program to look for.
    pub fn choice(self) -> EncoderChoice {
        self.0
    }
}

/// The decoder backends of a run (`crate::thumbnail::backends`), and the
/// codec strict-safe mode refused the FFmpeg fallback for, once the
/// policy's order reached it.
#[cfg(feature = "thumbnail")]
#[derive(Default)]
pub struct Decoders {
    owned: Vec<Box<dyn Backend>>,
    refused: Arc<OnceLock<Codec>>,
}

#[cfg(feature = "thumbnail")]
impl Decoders {
    /// Adds a backend that runs no `ffmpeg` program.
    #[cfg_attr(
        not(any(feature = "native-h264", feature = "dav1d", feature = "libvpx")),
        allow(dead_code)
    )]
    pub(crate) fn push(&mut self, backend: Box<dyn Backend>) {
        self.owned.push(backend);
    }

    /// Adds the FFmpeg fallback, or in strict-safe mode a stand-in in its
    /// place that says which streams it would take and opens none.
    #[cfg(feature = "ffmpeg-fallback")]
    pub(crate) fn push_ffmpeg(
        &mut self,
        strict: StrictSafe,
        backend: coreav_worker::SandboxedBackend,
    ) {
        if strict.on {
            self.owned.push(Box::new(Refused {
                backend,
                codec: Arc::clone(&self.refused),
            }));
        } else {
            self.owned.push(Box::new(backend));
        }
    }

    /// The backends, for `coreav_decode::open`.
    pub fn list(&self) -> Vec<&dyn Backend> {
        self.owned.iter().map(|b| b.as_ref()).collect()
    }

    /// `failure` as strict-safe mode's refusal (exit status 4) when the
    /// FFmpeg fallback was refused and no other backend decoded the
    /// stream; otherwise `failure` unchanged.
    pub fn explain(&self, failure: WriteFailure) -> WriteFailure {
        match (self.refused.get(), failure) {
            (Some(codec), WriteFailure::Rejected(Failure::VideoDecode(error)))
                if error.kind == coreav_decode::ErrorKind::Unsupported =>
            {
                WriteFailure::Rejected(Failure::StrictSafe(format!(
                    "strict-safe mode never runs the ffmpeg program, so it cannot decode the \
                     {} video, and no other decoder of this build does: {}, {OFF} ({})",
                    codec.name(),
                    build_hint(*codec),
                    error.message
                )))
            }
            (_, failure) => failure,
        }
    }
}

/// The build that decodes `codec` without FFmpeg.
#[cfg(feature = "thumbnail")]
fn build_hint(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "build coreav with the native-h264 feature",
        Codec::Av1 => "build coreav with the dav1d feature",
        Codec::Vp9 => "build coreav with the libvpx feature",
        _ => "no CoreAV decoder takes this codec yet",
    }
}

/// The FFmpeg fallback in strict-safe mode.
#[cfg(feature = "ffmpeg-fallback")]
struct Refused {
    backend: coreav_worker::SandboxedBackend,
    codec: Arc<OnceLock<Codec>>,
}

#[cfg(feature = "ffmpeg-fallback")]
impl Backend for Refused {
    fn kind(&self) -> coreav_decode::BackendKind {
        self.backend.kind()
    }

    fn supports(
        &self,
        stream: &coreav_decode::StreamConfig,
        traits: &coreav_decode::StreamTraits,
    ) -> coreav_decode::Support {
        self.backend.supports(stream, traits)
    }

    fn open(
        &self,
        stream: &coreav_decode::StreamConfig,
        _limits: &coreav_decode::Limits,
    ) -> Result<Box<dyn coreav_decode::Decoder>, coreav_decode::Error> {
        let _ = self.codec.set(stream.codec);
        Err(coreav_decode::Error::new(
            coreav_decode::ErrorKind::Unsupported,
            "not run in strict-safe mode",
        ))
    }
}
