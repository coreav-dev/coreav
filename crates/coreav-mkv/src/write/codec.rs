use super::{Media, Track};
use crate::{DocType, Error};

/// Matroska codec mappings supported by the writer. Adding a mapping does
/// not change block, cluster or index serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecId {
    /// FFV1 version 3 with its Configuration Record.
    Ffv1,
    /// AVC configuration record and length-prefixed NAL units.
    H264,
    /// HEVC configuration record and length-prefixed NAL units.
    Hevc,
    /// AV1 configuration record and low-overhead OBUs.
    Av1,
    /// VP8 frames.
    Vp8,
    /// VP9 frames, with optional feature metadata in CodecPrivate.
    Vp9,
    /// AudioSpecificConfig and raw AAC access units.
    Aac,
    /// MPEG audio layer III frames.
    Mp3,
    /// OpusHead and Opus packets.
    Opus,
    /// Xiph-laced Vorbis headers and Vorbis packets.
    Vorbis,
    /// FLAC initialization metadata and frames.
    Flac,
    /// AC-3 syncframes.
    Ac3,
    /// Enhanced AC-3 syncframes.
    Eac3,
    /// Apple Lossless configuration and packets.
    Alac,
    /// Signed little-endian 16-bit PCM.
    PcmS16le,
}

impl CodecId {
    /// The Matroska CodecID string.
    pub fn as_str(self) -> &'static str {
        self.mapping().0
    }

    fn mapping(self) -> (&'static str, bool, bool, bool) {
        match self {
            Self::Ffv1 => ("V_FFV1", true, true, false),
            Self::H264 => ("V_MPEG4/ISO/AVC", true, true, false),
            Self::Hevc => ("V_MPEGH/ISO/HEVC", true, true, false),
            Self::Av1 => ("V_AV1", true, true, true),
            Self::Vp8 => ("V_VP8", true, false, true),
            Self::Vp9 => ("V_VP9", true, false, true),
            Self::Aac => ("A_AAC", false, true, false),
            Self::Mp3 => ("A_MPEG/L3", false, false, false),
            Self::Opus => ("A_OPUS", false, true, true),
            Self::Vorbis => ("A_VORBIS", false, true, true),
            Self::Flac => ("A_FLAC", false, true, false),
            Self::Ac3 => ("A_AC3", false, false, false),
            Self::Eac3 => ("A_EAC3", false, false, false),
            Self::Alac => ("A_ALAC", false, true, false),
            Self::PcmS16le => ("A_PCM/INT/LIT", false, false, false),
        }
    }
}

pub(super) fn validate(track: &Track, doc_type: DocType) -> Result<(), Error> {
    let (_, video, private, webm) = track.codec.mapping();
    if doc_type == DocType::Webm && !webm {
        return Err(invalid("the codec is not permitted in WebM"));
    }
    if video != matches!(track.media, Media::Video { .. }) {
        return Err(invalid("the codec does not match the track media type"));
    }
    if private && track.private.is_empty() {
        return Err(invalid("the codec requires CodecPrivate"));
    }
    if track.default_duration_ns == Some(0) {
        return Err(invalid("DefaultDuration must be positive"));
    }
    match track.media {
        Media::Video {
            width,
            height,
            display,
            colour,
            interlaced,
            field_order,
        } => {
            if width == 0 || height == 0 || display.is_some_and(|(w, h)| w == 0 || h == 0) {
                return Err(invalid("video dimensions must be positive"));
            }
            if colour.matrix.is_some_and(|v| !matches!(v, 0..=2 | 4..=14))
                || colour
                    .transfer
                    .is_some_and(|v| !matches!(v, 1..=2 | 4..=18))
                || colour
                    .primaries
                    .is_some_and(|v| !matches!(v, 1..=2 | 4..=12 | 22))
            {
                return Err(invalid(
                    "video colour code points must be defined by Matroska",
                ));
            }
            if !matches!(interlaced, 0..=2) || !matches!(field_order, 0..=2 | 6 | 9 | 14) {
                return Err(invalid("video scan flags must be defined by Matroska"));
            }
        }
        Media::Audio {
            sample_rate,
            channels,
            bit_depth,
        } => {
            if sample_rate == 0 || channels == 0 || bit_depth == Some(0) {
                return Err(invalid("audio parameters must be positive"));
            }
        }
    }
    if track.codec == CodecId::PcmS16le
        && !matches!(
            track.media,
            Media::Audio {
                bit_depth: Some(16),
                ..
            }
        )
    {
        return Err(invalid("PCM s16le requires a 16-bit audio track"));
    }
    if track.codec == CodecId::Opus {
        let head = &track.private;
        if head.get(..8) != Some(b"OpusHead") || head.len() < 19 {
            return Err(invalid("Opus CodecPrivate must contain OpusHead"));
        }
        let channels = match track.media {
            Media::Audio { channels, .. } => channels,
            _ => 0,
        };
        if head.get(8).is_none_or(|v| *v == 0 || *v > 15)
            || head.get(9).is_none_or(|c| u32::from(*c) != channels)
            || head.get(18) == Some(&0) && (channels > 2 || head.len() != 19)
            || head.get(18).is_some_and(|m| *m != 0) && head.len() < 21 + channels as usize
        {
            return Err(invalid("OpusHead does not match its audio parameters"));
        }
        let skip = head
            .get(10..12)
            .and_then(|b| <[u8; 2]>::try_from(b).ok())
            .map(u16::from_le_bytes)
            .ok_or_else(|| invalid("missing Opus pre-skip"))?;
        let delay = (u64::from(skip) * 1_000_000_000 + 24_000) / 48_000;
        if track.codec_delay_ns != delay || track.seek_pre_roll_ns < 80_000_000 {
            return Err(invalid(
                "Opus requires matching CodecDelay and 80 ms SeekPreRoll",
            ));
        }
    }
    Ok(())
}

pub(super) fn invalid(reason: &'static str) -> Error {
    Error::Unsupported { reason }
}
