//! Names for codes: CoreAV's own codec names, and `ffprobe`'s vocabulary for
//! the compat output (the same mapping the oracle adapter uses).

use coreav_isobmff::{Codec, PcmFormat, ProResProfile, Profile, TrackKind};

/// The codec names `--allow` accepts: every name of `codec` except `other`,
/// which an allowlist can never include.
pub const ALLOW_NAMES: [&str; 12] = [
    "h264",
    "hevc",
    "av1",
    "vp9",
    "prores",
    "aac",
    "mp3",
    "opus",
    "flac",
    "pcm",
    "timed_text",
    "timecode",
];

/// CoreAV codec names, as printed in `codec`.
#[cfg(test)]
const CODEC_NAMES: [&str; 14] = [
    "h264",
    "hevc",
    "av1",
    "vp9",
    "vp8",
    "prores",
    "aac",
    "mp3",
    "opus",
    "flac",
    "pcm",
    "timed_text",
    "timecode",
    "other",
];

/// CoreAV's name for a codec.
pub fn codec(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "h264",
        Codec::Hevc => "hevc",
        Codec::Av1 => "av1",
        Codec::Vp9 => "vp9",
        // Never on a command's allowlist (`ALLOW_NAMES`): only the decoding
        // commands keep VP8 tracks of MP4 files.
        Codec::Vp8 => "vp8",
        Codec::ProRes => "prores",
        Codec::Aac => "aac",
        Codec::Mp3 => "mp3",
        Codec::Opus => "opus",
        Codec::Flac => "flac",
        Codec::Pcm => "pcm",
        Codec::TimedText => "timed_text",
        // Never on a command's allowlist (`ALLOW_NAMES`); only the chapter
        // reader keeps such tracks.
        Codec::QuickTimeText => "quicktime_text",
        Codec::Timecode => "timecode",
        Codec::Other => "other",
    }
}

/// The codec `--allow` means by `name`; `None` for unknown names and for
/// `other`.
pub fn codec_from_name(name: &str) -> Option<Codec> {
    Some(match name {
        "h264" => Codec::H264,
        "hevc" => Codec::Hevc,
        "av1" => Codec::Av1,
        "vp9" => Codec::Vp9,
        "prores" => Codec::ProRes,
        "aac" => Codec::Aac,
        "mp3" => Codec::Mp3,
        "opus" => Codec::Opus,
        "flac" => Codec::Flac,
        "pcm" => Codec::Pcm,
        "timed_text" => Codec::TimedText,
        "timecode" => Codec::Timecode,
        _ => return None,
    })
}

/// An ISO 639-2/T language code, when the three bytes are lowercase
/// letters; packed codes outside `a`..=`z` (for example all bits set) are
/// not languages.
pub fn language(code: Option<[u8; 3]>) -> Option<String> {
    let code = code?;
    code.iter()
        .all(u8::is_ascii_lowercase)
        .then(|| String::from_utf8_lossy(&code).into_owned())
}

/// CoreAV's name for a track kind.
pub fn kind(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Subtitle => "subtitle",
        TrackKind::Timecode => "timecode",
        TrackKind::Data => "data",
    }
}

/// `ffprobe`'s `codec_type`.
pub fn ffprobe_kind(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Subtitle => "subtitle",
        TrackKind::Timecode | TrackKind::Data => "data",
    }
}

/// `ffprobe`'s `codec_name`; `None` when it prints none.
pub fn ffprobe_codec(codec: Codec, pcm: Option<PcmFormat>) -> Option<String> {
    let name = match codec {
        Codec::H264 => "h264",
        Codec::Hevc => "hevc",
        Codec::Av1 => "av1",
        Codec::Vp9 => "vp9",
        Codec::Vp8 => "vp8",
        Codec::ProRes => "prores",
        Codec::Aac => "aac",
        Codec::Mp3 => "mp3",
        Codec::Opus => "opus",
        Codec::Flac => "flac",
        Codec::TimedText => "mov_text",
        Codec::Pcm => {
            let pcm = pcm?;
            let sign = if pcm.float {
                "f"
            } else if pcm.signed {
                "s"
            } else {
                "u"
            };
            if pcm.bits <= 8 {
                return Some(format!("pcm_{sign}8"));
            }
            let order = if pcm.little_endian { "le" } else { "be" };
            return Some(format!("pcm_{sign}{}{order}", pcm.bits));
        }
        Codec::QuickTimeText | Codec::Timecode | Codec::Other => return None,
    };
    Some(name.to_owned())
}

/// ITU-T H.273 colour primaries, as `ffprobe` names them.
pub fn primaries(code: u16) -> Option<&'static str> {
    Some(match code {
        1 => "bt709",
        4 => "bt470m",
        5 => "bt470bg",
        6 => "smpte170m",
        7 => "smpte240m",
        8 => "film",
        9 => "bt2020",
        10 => "smpte428",
        11 => "smpte431",
        12 => "smpte432",
        22 => "jedec-p22",
        _ => return None,
    })
}

/// ITU-T H.273 transfer characteristics, as `ffprobe` names them.
pub fn transfer(code: u16) -> Option<&'static str> {
    Some(match code {
        1 => "bt709",
        // `ffprobe` names these after the systems that use them.
        4 => "bt470m",
        5 => "bt470bg",
        6 => "smpte170m",
        7 => "smpte240m",
        8 => "linear",
        9 => "log100",
        10 => "log316",
        11 => "iec61966-2-4",
        12 => "bt1361e",
        13 => "iec61966-2-1",
        14 => "bt2020-10",
        15 => "bt2020-12",
        16 => "smpte2084",
        17 => "smpte428",
        18 => "arib-std-b67",
        _ => return None,
    })
}

/// ITU-T H.273 matrix coefficients, as `ffprobe` names them (`color_space`).
pub fn matrix(code: u16) -> Option<&'static str> {
    Some(match code {
        0 => "gbr",
        1 => "bt709",
        4 => "fcc",
        5 => "bt470bg",
        6 => "smpte170m",
        7 => "smpte240m",
        8 => "ycgco",
        9 => "bt2020nc",
        10 => "bt2020c",
        11 => "smpte2085",
        12 => "chroma-derived-nc",
        13 => "chroma-derived-c",
        14 => "ictcp",
        _ => return None,
    })
}

/// The profile name `ffprobe` prints.
pub fn profile(profile: Profile) -> Option<String> {
    let name = match profile {
        Profile::H264 {
            profile_idc,
            constraint_flags,
            ..
        } => {
            // constraint_set1_flag and constraint_set3_flag (ITU-T H.264 A.2).
            let set1 = constraint_flags & 0x40 != 0;
            let set3 = constraint_flags & 0x10 != 0;
            match (profile_idc, set1, set3) {
                (66, true, _) => "Constrained Baseline",
                (66, false, _) => "Baseline",
                (77, ..) => "Main",
                (88, ..) => "Extended",
                (100, ..) => "High",
                (110, _, false) => "High 10",
                (110, _, true) => "High 10 Intra",
                (122, _, false) => "High 4:2:2",
                (122, _, true) => "High 4:2:2 Intra",
                (244, _, false) => "High 4:4:4 Predictive",
                (244, _, true) => "High 4:4:4 Intra",
                (44, ..) => "CAVLC 4:4:4",
                _ => return None,
            }
        }
        Profile::Hevc { profile_idc, .. } => match profile_idc {
            1 => "Main",
            2 => "Main 10",
            3 => "Main Still Picture",
            4 => "Rext",
            _ => return None,
        },
        Profile::Av1 { profile, .. } => match profile {
            0 => "Main",
            1 => "High",
            2 => "Professional",
            _ => return None,
        },
        Profile::Vp9 { profile, .. } => return Some(format!("Profile {profile}")),
        Profile::Aac {
            object_type,
            sbr,
            ps,
        } => match (object_type, sbr, ps) {
            (_, _, true) => "HE-AACv2",
            (_, true, false) => "HE-AAC",
            (1, ..) => "Main",
            (2, ..) => "LC",
            (3, ..) => "SSR",
            (4, ..) => "LTP",
            (23, ..) => "LD",
            (39, ..) => "ELD",
            _ => return None,
        },
        Profile::ProRes(profile) => prores(profile),
    };
    Some(name.to_owned())
}

/// The ProRes profile name.
pub fn prores(profile: ProResProfile) -> &'static str {
    match profile {
        ProResProfile::Proxy => "Proxy",
        ProResProfile::Lt => "LT",
        ProResProfile::Standard => "Standard",
        ProResProfile::Hq => "HQ",
        ProResProfile::P4444 => "4444",
        ProResProfile::P4444Xq => "XQ",
    }
}

/// The level `ffprobe` prints: the stored integer; none for VP9, AAC and
/// ProRes.
pub fn level(profile: Profile) -> Option<u8> {
    match profile {
        Profile::H264 { level_idc, .. } | Profile::Hevc { level_idc, .. } => Some(level_idc),
        Profile::Av1 { level_idx, .. } => Some(level_idx),
        Profile::Vp9 { .. } | Profile::Aac { .. } | Profile::ProRes(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_allow_name_round_trips() {
        for name in ALLOW_NAMES {
            let parsed = codec_from_name(name).map(codec);
            assert_eq!(parsed, Some(name));
        }
        // `other` is printed but can never be allowed.
        assert!(CODEC_NAMES.contains(&"other"));
        for name in ALLOW_NAMES {
            assert!(CODEC_NAMES.contains(&name), "{name}");
        }
        assert_eq!(codec_from_name("other"), None);
        assert_eq!(codec_from_name("magicyuv"), None);
    }

    #[test]
    fn only_letter_codes_are_languages() {
        assert_eq!(language(Some(*b"und")).as_deref(), Some("und"));
        assert_eq!(language(Some([0x7f; 3])), None);
        assert_eq!(language(None), None);
    }

    #[test]
    fn pcm_names_follow_the_layout() {
        let s16le = PcmFormat {
            float: false,
            bits: 16,
            little_endian: true,
            signed: true,
        };
        assert_eq!(
            ffprobe_codec(Codec::Pcm, Some(s16le)).as_deref(),
            Some("pcm_s16le")
        );
        assert_eq!(ffprobe_codec(Codec::Pcm, None), None);
        assert_eq!(ffprobe_codec(Codec::Timecode, None), None);
    }
}
