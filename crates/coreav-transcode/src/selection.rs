//! Transcode-only encoder selection (`docs/design/encoders.md`).
//! New backends are not connected. Resolution checks program availability
//! without opening media, creating output or starting a process.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::encoder::{EncoderChoice, EncoderProgram, find_encoder};

/// The requested output video codec, independent of its encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264, the unchanged default.
    H264,
    /// AV1, which requires the SVT-AV1 backend.
    Av1,
}

impl FromStr for VideoCodec {
    type Err = &'static str;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "h264" => Ok(Self::H264),
            "av1" => Ok(Self::Av1),
            _ => Err("unknown output video codec"),
        }
    }
}

/// A transcode video request, not the external-program `EncoderChoice`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoEncoder {
    /// Select an eligible encoder for the requested codec.
    Auto,
    /// The operator's x264 program, for H.264 only.
    X264,
    /// The operator's FFmpeg with libx264, for H.264 only.
    Ffmpeg,
    /// An in-process platform H.264 encoder, not connected yet.
    Platform,
    /// The sandboxed SVT-AV1 encoder, not connected yet.
    SvtAv1,
}

impl FromStr for VideoEncoder {
    type Err = &'static str;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "auto" => Ok(Self::Auto),
            "x264" => Ok(Self::X264),
            "ffmpeg" => Ok(Self::Ffmpeg),
            "platform" => Ok(Self::Platform),
            "svt-av1" => Ok(Self::SvtAv1),
            _ => Err("unknown video encoder"),
        }
    }
}

/// The independently requested AAC encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioEncoder {
    /// Select an eligible AAC encoder.
    Auto,
    /// An in-process platform AAC encoder, not connected yet.
    Platform,
    /// The operator's FFmpeg AAC encoder.
    Ffmpeg,
}

impl FromStr for AudioEncoder {
    type Err = &'static str;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "auto" => Ok(Self::Auto),
            "platform" => Ok(Self::Platform),
            "ffmpeg" => Ok(Self::Ffmpeg),
            _ => Err("unknown audio encoder"),
        }
    }
}

/// A parsed selection request, checked before opening media or an output.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    /// The requested output codec.
    pub video: VideoCodec,
    /// The requested video backend.
    pub encoder: VideoEncoder,
    /// An explicit AAC selection; absence retains the legacy audio path.
    pub audio_encoder: Option<AudioEncoder>,
    /// Require a hardware platform video implementation, without fallback.
    pub require_hardware: bool,
    /// An explicit CRF, not the legacy default.
    pub crf: Option<f32>,
    /// An explicit preset, including an explicitly supplied `medium`.
    pub preset: Option<String>,
}

/// Available external programs for a selection using existing backends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The selected H.264 program, pinned rather than rediscovered by auto.
    pub video: EncoderProgram,
    /// The AAC program, independent of the selected video program.
    pub aac: Option<PathBuf>,
}

impl Selection {
    /// Refuse incompatible or unconnected selections without I/O.
    /// `aac` says whether the command requests AAC output audio. Legacy
    /// selections return `Ok` and retain their existing validation order.
    pub fn preflight(&self, aac: bool, strict_safe: bool) -> Result<(), &'static str> {
        if self.audio_encoder.is_some() && !aac {
            return Err("--audio-encoder needs --audio aac");
        }
        let encoder = if self.require_hardware && self.encoder == VideoEncoder::Auto {
            VideoEncoder::Platform
        } else {
            self.encoder
        };
        match (self.video, encoder) {
            (VideoCodec::H264, VideoEncoder::SvtAv1) => {
                return Err("--encoder svt-av1 requires --video av1");
            }
            (VideoCodec::Av1, VideoEncoder::X264) => {
                return Err("--encoder x264 cannot encode AV1");
            }
            (VideoCodec::Av1, VideoEncoder::Ffmpeg) => {
                return Err("--video av1 --encoder ffmpeg is not supported");
            }
            (VideoCodec::Av1, VideoEncoder::Platform) => {
                return Err("--encoder platform cannot encode AV1");
            }
            (
                VideoCodec::H264,
                VideoEncoder::Auto
                | VideoEncoder::X264
                | VideoEncoder::Ffmpeg
                | VideoEncoder::Platform,
            )
            | (VideoCodec::Av1, VideoEncoder::Auto | VideoEncoder::SvtAv1) => {}
        }
        if self.require_hardware && encoder != VideoEncoder::Platform {
            return Err("--require-hardware is incompatible with a non-platform video encoder");
        }
        if strict_safe
            && (encoder == VideoEncoder::Platform
                || self.audio_encoder == Some(AudioEncoder::Platform))
        {
            return Err("strict-safe mode does not permit an in-process platform encoder");
        }
        if encoder == VideoEncoder::Platform {
            if self.crf.is_some() {
                return Err(
                    "the selected platform encoder cannot honor --crf; use --bitrate or select x264",
                );
            }
            if self.preset.is_some() {
                return Err(
                    "the selected platform encoder cannot honor --preset; omit it or select x264",
                );
            }
            return Err("no platform H.264 encoder: this build has no platform backend");
        }
        if self.video == VideoCodec::Av1 {
            return Err("no AV1 encoder: this build has no SVT-AV1 backend");
        }
        match self.audio_encoder {
            Some(AudioEncoder::Platform) => {
                Err("no platform AAC encoder: this build has no platform backend")
            }
            Some(AudioEncoder::Auto | AudioEncoder::Ffmpeg) | None => Ok(()),
        }
    }

    /// Resolve existing external encoders before any media or output I/O.
    /// Operator paths and environment/PATH discovery retain the existing
    /// precedence, but missing or nonexecutable paths are refused here.
    /// The CLI retains its strict-safe program-name and symlink checks.
    pub fn resolve(
        &self,
        aac: bool,
        strict_safe: bool,
        x264: Option<&Path>,
        ffmpeg: Option<&Path>,
    ) -> Result<Resolved, &'static str> {
        self.preflight(aac, strict_safe)?;
        if strict_safe && aac {
            return Err(
                "strict-safe mode never runs the ffmpeg program, so it cannot encode AAC audio: use --audio copy, none or opus",
            );
        }
        if strict_safe && self.encoder == VideoEncoder::Ffmpeg {
            return Err(
                "strict-safe mode never runs the ffmpeg program, so it cannot be the encoder",
            );
        }
        if strict_safe && !coreav_worker::enforces_memory_limit() {
            return Err(
                "strict-safe: no enforceable memory limit for the sandbox on this platform",
            );
        }
        let find = |choice| {
            find_encoder(
                choice,
                x264.map(Path::to_path_buf),
                ffmpeg.map(Path::to_path_buf),
            )
            .filter(|program| executable(program.path()))
        };
        let video = match self.encoder {
            VideoEncoder::Auto if strict_safe => find(EncoderChoice::X264),
            VideoEncoder::Auto => find(EncoderChoice::X264).or_else(|| find(EncoderChoice::Ffmpeg)),
            VideoEncoder::X264 => find(EncoderChoice::X264),
            VideoEncoder::Ffmpeg => find(EncoderChoice::Ffmpeg),
            VideoEncoder::Platform | VideoEncoder::SvtAv1 => None,
        }
        .ok_or(match self.encoder {
            VideoEncoder::X264 => "no H.264 encoder: the selected x264 program is unavailable",
            VideoEncoder::Ffmpeg => "no H.264 encoder: the selected FFmpeg program is unavailable",
            VideoEncoder::Auto | VideoEncoder::Platform | VideoEncoder::SvtAv1 => {
                "no H.264 encoder: install x264 or FFmpeg with libx264"
            }
        })?;
        let aac = if aac {
            Some(
                find(EncoderChoice::Ffmpeg)
                    .ok_or("no AAC encoder: install FFmpeg or select --audio opus")?
                    .path()
                    .to_path_buf(),
            )
        } else {
            None
        };
        Ok(Resolved { video, aac })
    }
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
