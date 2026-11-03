//! Transcode-only encoder selection (`docs/design/encoders.md`).
//! Resolution checks compiled-host and program availability
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
    /// The sandboxed SVT-AV1 encoder.
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
    /// An explicit video bit rate.
    pub bitrate: Option<u32>,
    /// An explicit video thread-count request.
    pub threads: Option<u32>,
}

/// A codec-specific resolved encoder, never an AV1 job disguised as H.264.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedVideo {
    /// The selected external H.264 program.
    H264(EncoderProgram),
    /// Checked settings for the compiled sandboxed AV1 host.
    SvtAv1 {
        /// Numeric preset.
        preset: u8,
        /// Integer constant-rate factor.
        crf: u8,
    },
}

/// Resolved video and independently selected external AAC encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The selected video encoder and its codec-specific settings.
    pub video: ResolvedVideo,
    /// The AAC program, independent of the selected video program.
    pub aac: Option<PathBuf>,
}

impl Selection {
    /// Refuse incompatible or unconnected selections without I/O.
    /// `aac` says whether the command requests AAC output audio. Legacy
    /// selections return `Ok` and retain their existing validation order.
    /// `svt_av1` says that the caller compiled a supported private worker host.
    pub fn preflight(
        &self,
        aac: bool,
        strict_safe: bool,
        svt_av1: bool,
    ) -> Result<(), &'static str> {
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
            if self.require_hardware {
                return Err(
                    "no hardware H.264 encoder is available: this build has no VideoToolbox backend",
                );
            }
            return Err("no platform H.264 encoder: this build has no platform backend");
        }
        if self.video == VideoCodec::Av1 {
            if self.threads.is_some() {
                return Err(
                    "AV1 does not support --threads: parallelism level is not a thread limit",
                );
            }
            if self.bitrate.is_some() {
                return Err("AV1 supports integer --crf only, not --bitrate");
            }
            if self
                .crf
                .is_some_and(|q| !(0.0..=63.0).contains(&q) || q.fract() != 0.0)
            {
                return Err("AV1 --crf must be an integer from 0 to 63");
            }
            if self
                .preset
                .as_deref()
                .is_some_and(|preset| preset.parse::<u8>().map_or(true, |preset| preset > 13))
            {
                return Err("AV1 --preset must be an integer from 0 to 13");
            }
            if !svt_av1 {
                return Err("no AV1 encoder: this build has no SVT-AV1 backend");
            }
        }
        match self.audio_encoder {
            Some(AudioEncoder::Platform) => {
                Err("no platform AAC encoder: this build has no platform backend")
            }
            Some(AudioEncoder::Auto | AudioEncoder::Ffmpeg) | None => Ok(()),
        }
    }

    /// Resolve codec-specific encoders before any media or output I/O.
    /// Operator paths and environment/PATH discovery retain the existing
    /// precedence, but missing or nonexecutable paths are refused here.
    /// The CLI retains its strict-safe program-name and symlink checks.
    pub fn resolve(
        &self,
        aac: bool,
        strict_safe: bool,
        x264: Option<&Path>,
        ffmpeg: Option<&Path>,
        svt_av1: bool,
    ) -> Result<Resolved, &'static str> {
        self.preflight(aac, strict_safe, svt_av1)?;
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
        let video = if self.video == VideoCodec::Av1 {
            if x264.is_some() {
                return Err("--x264 does not apply to AV1 encoding");
            }
            if ffmpeg.is_some() && !aac {
                return Err("with AV1, --ffmpeg applies only to --audio aac");
            }
            ResolvedVideo::SvtAv1 {
                preset: self
                    .preset
                    .as_deref()
                    .unwrap_or("8")
                    .parse()
                    .map_err(|_| "AV1 --preset must be an integer from 0 to 13")?,
                crf: self.crf.unwrap_or(32.0) as u8,
            }
        } else {
            ResolvedVideo::H264(
                match self.encoder {
                    VideoEncoder::Auto if strict_safe => find(EncoderChoice::X264),
                    VideoEncoder::Auto => {
                        find(EncoderChoice::X264).or_else(|| find(EncoderChoice::Ffmpeg))
                    }
                    VideoEncoder::X264 => find(EncoderChoice::X264),
                    VideoEncoder::Ffmpeg => find(EncoderChoice::Ffmpeg),
                    VideoEncoder::Platform | VideoEncoder::SvtAv1 => None,
                }
                .ok_or(match self.encoder {
                    VideoEncoder::X264 => {
                        "no H.264 encoder: the selected x264 program is unavailable"
                    }
                    VideoEncoder::Ffmpeg => {
                        "no H.264 encoder: the selected FFmpeg program is unavailable"
                    }
                    VideoEncoder::Auto | VideoEncoder::Platform | VideoEncoder::SvtAv1 => {
                        "no H.264 encoder: install x264 or FFmpeg with libx264"
                    }
                })?,
            )
        };
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

pub(crate) fn executable(path: &Path) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn av1() -> Selection {
        Selection {
            video: VideoCodec::Av1,
            encoder: VideoEncoder::Auto,
            audio_encoder: None,
            require_hardware: false,
            crf: None,
            preset: None,
            bitrate: None,
            threads: None,
        }
    }

    #[test]
    fn a_synthetic_available_host_selects_av1_without_an_external_video_program() {
        for encoder in [VideoEncoder::Auto, VideoEncoder::SvtAv1] {
            let request = Selection { encoder, ..av1() };
            let resolved = request.resolve(false, false, None, None, true).unwrap();
            assert_eq!(resolved.video, ResolvedVideo::SvtAv1 { preset: 8, crf: 32 });
            assert_eq!(resolved.aac, None);
        }
        let request = Selection {
            preset: Some("13".into()),
            crf: Some(63.0),
            ..av1()
        };
        assert_eq!(
            request
                .resolve(false, false, None, None, true)
                .unwrap()
                .video,
            ResolvedVideo::SvtAv1 {
                preset: 13,
                crf: 63
            }
        );
    }

    #[test]
    fn av1_refuses_unavailable_hosts_incompatible_programs_and_unmatched_controls() {
        assert_eq!(
            av1().preflight(false, false, false),
            Err("no AV1 encoder: this build has no SVT-AV1 backend")
        );
        for request in [
            Selection {
                encoder: VideoEncoder::X264,
                ..av1()
            },
            Selection {
                encoder: VideoEncoder::Ffmpeg,
                ..av1()
            },
            Selection {
                encoder: VideoEncoder::Platform,
                ..av1()
            },
            Selection {
                require_hardware: true,
                ..av1()
            },
            Selection {
                bitrate: Some(1000),
                ..av1()
            },
            Selection {
                threads: Some(1),
                ..av1()
            },
            Selection {
                crf: Some(32.5),
                ..av1()
            },
            Selection {
                crf: Some(f32::NAN),
                ..av1()
            },
            Selection {
                crf: Some(64.0),
                ..av1()
            },
            Selection {
                preset: Some("14".into()),
                ..av1()
            },
            Selection {
                preset: Some("medium".into()),
                ..av1()
            },
        ] {
            assert!(
                request.preflight(false, false, true).is_err(),
                "{request:?}"
            );
        }
        for (x264, ffmpeg) in [
            (Some(Path::new("/absent/x264")), None),
            (None, Some(Path::new("/absent/ffmpeg"))),
        ] {
            assert!(av1().resolve(false, false, x264, ffmpeg, true).is_err());
        }
    }

    #[test]
    fn h264_compatibility_checks_do_not_depend_on_av1_host_availability() {
        let request = Selection {
            video: VideoCodec::H264,
            encoder: VideoEncoder::X264,
            crf: Some(23.5),
            preset: Some("medium".into()),
            threads: Some(8),
            ..av1()
        };
        assert_eq!(request.preflight(false, false, false), Ok(()));
        assert_eq!(request.preflight(false, false, true), Ok(()));
        let wrong = Selection {
            encoder: VideoEncoder::SvtAv1,
            ..request
        };
        assert_eq!(
            wrong.preflight(false, false, true),
            Err("--encoder svt-av1 requires --video av1")
        );
    }
}
