use super::model::Value;
use super::{Entries, mkv::video, ts::params};

pub fn stream(
    entries: &mut Entries,
    config: Option<&[u8]>,
    first: &[u8],
    color: Option<coreav_isobmff::Color>,
) {
    let text = |key| {
        entries.iter().find_map(|(k, v)| match v {
            Value::Str(s) if *k == key => Some(s.as_str()),
            _ => None,
        })
    };
    let codec = text("codec_name").unwrap_or_default();
    let mime = match codec {
        "h264" => config
            .filter(|c| c.first() == Some(&1))
            .and_then(|c| c.get(1..4))
            .map(<[u8]>::to_vec)
            .or_else(|| {
                video::annexb_nals(config.unwrap_or(first))
                    .into_iter()
                    .find(|n| n.first().is_some_and(|b| b & 0x1f == 7))
                    .and_then(|n| n.get(1..4))
                    .map(<[u8]>::to_vec)
            })
            .and_then(|v| match v.as_slice() {
                [profile, constraints, level] => {
                    Some(format!("avc1.{profile:02x}{constraints:02x}{level:02x}"))
                }
                _ => None,
            }),
        "aac" => {
            let object = config.and_then(|config| {
                let mut bits = super::video::Bits::new(config);
                match bits.bits(5)? {
                    0 => None,
                    31 => Some(32 + bits.bits(6)?),
                    object => Some(object),
                }
            });
            let object = object.or_else(|| match text("profile") {
                Some("Main") => Some(1),
                Some("LC") => Some(2),
                Some("SSR") => Some(3),
                Some("LTP") => Some(4),
                Some("HE-AAC") => Some(5),
                Some("HE-AACv2") => Some(29),
                Some("LD") => Some(23),
                Some("ELD") => Some(39),
                None if first.is_empty() => Some(2),
                _ => None,
            });
            object.map(|n| format!("mp4a.40.{n}"))
        }
        "mp1" => Some("mp4a.40.32".to_owned()),
        "mp2" => Some("mp4a.40.33".to_owned()),
        "mp3" => Some("mp4a.40.34".to_owned()),
        "mpeg4" => Some("mp4v.20".to_owned()),
        "vp8" => Some("vp8".to_owned()),
        "vorbis" => Some("vorbis".to_owned()),
        "ac3" => Some("ac-3".to_owned()),
        "eac3" => Some("ec-3".to_owned()),
        "opus" => Some("opus".to_owned()),
        "flac" => Some("flac".to_owned()),
        "av1" => config.and_then(|c| av1(c, color)),
        "vp9" => vp9(entries, config),
        _ => None,
    };
    let hevc = codec == "hevc";
    entries.retain(|(k, _)| !matches!(*k, "refs" | "closed_captions" | "film_grain"));
    if let Some(mime) = mime {
        let at = entries
            .iter()
            .position(|(k, _)| *k == "codec_tag")
            .map_or(entries.len(), |n| n + 1);
        entries.insert(at, ("mime_codec_string", Value::Str(mime)));
    }
    if hevc {
        let at = entries
            .iter()
            .position(|(k, _)| matches!(*k, "ts_id" | "id"))
            .unwrap_or(entries.len());
        entries.insert(at, ("view_ids_available", Value::Str(String::new())));
        entries.insert(at + 1, ("view_pos_available", Value::Str(String::new())));
    }
    downmix(entries, first);
}

fn av1(config: &[u8], color: Option<coreav_isobmff::Color>) -> Option<String> {
    use coreav_isobmff::{FourCc, Profile, describe_config_record, parameter_set_color};

    let [0x81, _, flags, _, ..] = config else {
        return None;
    };
    let kind = FourCc(*b"av1C");
    let Profile::Av1 {
        profile,
        level_idx,
        high_tier,
        high_bitdepth,
        twelve_bit,
        monochrome,
    } = describe_config_record(kind, config)?.profile?
    else {
        return None;
    };
    if profile > 2 || (24..31).contains(&level_idx) {
        return None;
    }
    let depth = if !high_bitdepth {
        8
    } else if profile == 2 && twelve_bit {
        12
    } else {
        10
    };
    let tier = if high_tier { 'H' } else { 'M' };
    let mut mime = format!("av01.{profile}.{level_idx:02}{tier}.{depth:02}");
    let x = u8::from(monochrome || flags & 8 != 0);
    let y = u8::from(monochrome || flags & 4 != 0);
    let position = if !monochrome && x == 1 && y == 1 {
        flags & 3
    } else {
        0
    };
    let stated = parameter_set_color(kind, config);
    // A sequence header that states its colour description states the
    // optional fields, also when they equal the defaults.
    let signalled = stated.is_some_and(|c| c.primaries.is_some());
    let color = color.or(stated).unwrap_or_default();
    let primaries = color.primaries.unwrap_or(1);
    let transfer = color.transfer.unwrap_or(1);
    let matrix = color.matrix.unwrap_or(1);
    let full = u8::from(color.full_range.unwrap_or(false));
    let mono = u8::from(monochrome);
    if signalled
        || (mono, x, y, position, primaries, transfer, matrix, full) != (0, 1, 1, 0, 1, 1, 1, 0)
    {
        mime.push_str(&format!(
            ".{mono}.{x}{y}{position}.{primaries:02}.{transfer:02}.{matrix:02}.{full}"
        ));
    }
    Some(mime)
}

fn vp9(entries: &Entries, config: Option<&[u8]>) -> Option<String> {
    if let Some([1, 0, 0, 0, profile, level, packed, ..]) = config {
        return Some(format!("vp09.{profile:02}.{level:02}.{:02}", packed >> 4));
    }
    let text = |key| {
        entries.iter().find_map(|(k, v)| match v {
            Value::Str(s) if *k == key => Some(s.as_str()),
            _ => None,
        })
    };
    let number = |key| {
        entries.iter().find_map(|(k, v)| match v {
            Value::Int(n) | Value::Unit(n, _) if *k == key => u64::try_from(*n).ok(),
            _ => None,
        })
    };
    let profile: u8 = text("profile")?.strip_prefix("Profile ")?.parse().ok()?;
    let format = text("pix_fmt")?;
    let depth = if format.contains("12") {
        12
    } else if format.contains("10") {
        10
    } else {
        8
    };
    let width = number("width")?;
    let height = number("height")?;
    let picture = width.checked_mul(height)?;
    let rate = ["avg_frame_rate", "r_frame_rate"]
        .into_iter()
        .find_map(|key| {
            let (n, d) = text(key)?.split_once('/')?;
            let n: u64 = n.parse().ok()?;
            let d: u64 = d.parse().ok()?;
            (n > 0 && d > 0).then_some((n, d))
        });
    let sample_rate = rate
        .and_then(|(n, d)| picture.checked_mul(n).map(|v| v.div_ceil(d)))
        .unwrap_or(0);
    let bitrate = number("bit_rate").unwrap_or(0);
    const LEVELS: [(u8, u64, u64, u64, u64); 14] = [
        (10, 829_440, 36_864, 200_000, 512),
        (11, 2_764_800, 73_728, 800_000, 768),
        (20, 4_608_000, 122_880, 1_800_000, 960),
        (21, 9_216_000, 245_760, 3_600_000, 1344),
        (30, 20_736_000, 552_960, 7_200_000, 2048),
        (31, 36_864_000, 983_040, 12_000_000, 2752),
        (40, 83_558_400, 2_228_224, 18_000_000, 4160),
        (41, 160_432_128, 2_228_224, 30_000_000, 4160),
        (50, 311_951_360, 8_912_896, 60_000_000, 8384),
        (51, 588_251_136, 8_912_896, 120_000_000, 8384),
        (52, 1_176_502_272, 8_912_896, 180_000_000, 8384),
        (60, 1_176_502_272, 35_651_584, 180_000_000, 16832),
        (61, 2_353_004_544, 35_651_584, 240_000_000, 16832),
        (62, 4_706_009_088, 35_651_584, 480_000_000, 16832),
    ];
    let level = LEVELS
        .into_iter()
        .find(|(_, samples, pixels, bits, edge)| {
            sample_rate <= *samples
                && picture <= *pixels
                && bitrate <= *bits
                && width.max(height) <= *edge
        })
        .map_or(0, |(level, ..)| level);
    Some(format!("vp09.{profile:02}.{level:02}.{depth:02}"))
}

pub fn downmix(entries: &mut Entries, first: &[u8]) {
    let dolby = entries.iter().any(|(k, v)| {
        *k == "codec_name" && matches!(v, Value::Str(s) if s == "ac3" || s == "eac3")
    });
    if let Some(mix) = dolby
        .then(|| params::ac3(first))
        .flatten()
        .map(|a| a.downmix)
    {
        let at = entries
            .iter()
            .position(|(k, _)| *k == "initial_padding")
            .map_or(entries.len(), |n| n + 1);
        entries.splice(at..at, mix.entries());
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::s;
    use super::*;

    #[test]
    fn avc_mime_reads_the_profile_constraints_and_level() {
        for config in [vec![1, 100, 0, 40], vec![0, 0, 1, 0x67, 100, 0, 40]] {
            let mut entries = vec![("codec_name", s("h264")), ("codec_tag", s("0x0000"))];
            stream(&mut entries, Some(&config), &[], None);
            assert_eq!(
                entries.last(),
                Some(&("mime_codec_string", s("avc1.640028")))
            );
        }
        let mut entries = vec![("codec_name", s("h264"))];
        stream(&mut entries, Some(&[1, 100]), &[], None);
        assert_eq!(entries, vec![("codec_name", s("h264"))]);
    }

    #[test]
    fn aac_mime_distinguishes_extension_object_types() {
        for (profile, mime) in [
            ("LC", "mp4a.40.2"),
            ("HE-AAC", "mp4a.40.5"),
            ("HE-AACv2", "mp4a.40.29"),
        ] {
            let mut entries = vec![("codec_name", s("aac")), ("profile", s(profile))];
            stream(&mut entries, None, &[], None);
            assert_eq!(entries.last(), Some(&("mime_codec_string", s(mime))));
        }
    }

    #[test]
    fn latm_is_not_listed_as_an_mp4_audio_sample_entry() {
        let mut entries = vec![("codec_name", s("aac_latm")), ("profile", s("LC"))];
        let expected = entries.clone();
        stream(&mut entries, None, &[], None);
        assert_eq!(entries, expected);
    }

    #[test]
    fn dolby_downmix_follows_the_initial_padding_field() {
        let mut entries = vec![
            ("codec_name", s("ac3")),
            ("bits_per_sample", Value::Int(0)),
            ("initial_padding", Value::Int(0)),
            ("id", s("0x1")),
        ];
        downmix(&mut entries, &[0x0b, 0x77, 0, 0, 0x1e, 0x40, 0xe0, 0x40]);
        assert_eq!(
            entries.into_iter().map(|(key, _)| key).collect::<Vec<_>>(),
            [
                "codec_name",
                "bits_per_sample",
                "initial_padding",
                "dmix_mode",
                "ltrt_cmixlev",
                "ltrt_surmixlev",
                "loro_cmixlev",
                "loro_surmixlev",
                "id",
            ]
        );
    }

    #[test]
    fn removed_decoder_fields_are_not_listed() {
        let mut entries = vec![
            ("codec_name", s("h264")),
            ("refs", Value::Int(1)),
            ("closed_captions", Value::Int(0)),
            ("film_grain", Value::Int(0)),
        ];
        stream(&mut entries, None, &[], None);
        assert_eq!(entries, vec![("codec_name", s("h264"))]);
    }

    #[test]
    fn named_codecs_have_their_registered_mime_strings() {
        for (codec, mime) in [
            ("vp8", "vp8"),
            ("vorbis", "vorbis"),
            ("mpeg4", "mp4v.20"),
            ("mp1", "mp4a.40.32"),
            ("mp2", "mp4a.40.33"),
            ("mp3", "mp4a.40.34"),
        ] {
            let mut entries = vec![("codec_name", s(codec))];
            stream(&mut entries, None, &[], None);
            assert_eq!(entries.last(), Some(&("mime_codec_string", s(mime))));
        }
    }

    #[test]
    fn aac_mime_uses_the_stored_object_not_the_decoded_extension_profile() {
        for (config, profile, mime) in [
            (vec![0x11, 0x90], "HE-AAC", "mp4a.40.2"),
            (vec![0x2b, 0x92, 0x08], "HE-AAC", "mp4a.40.5"),
            (vec![0xeb, 0x92, 0x08], "HE-AACv2", "mp4a.40.29"),
            (vec![0xf8, 0xe6, 0x40], "ELD", "mp4a.40.39"),
        ] {
            let mut entries = vec![("codec_name", s("aac")), ("profile", s(profile))];
            stream(&mut entries, Some(&config), &[], None);
            assert_eq!(entries.last(), Some(&("mime_codec_string", s(mime))));
        }
    }

    #[test]
    fn av1_mime_uses_the_configuration_record() {
        let mut entries = vec![("codec_name", s("av1"))];
        stream(&mut entries, Some(&[0x81, 0x28, 0xc0, 0]), &[], None);
        assert_eq!(
            entries.last(),
            Some(&("mime_codec_string", s("av01.1.08H.10.0.000.01.01.01.0")))
        );
    }

    #[test]
    fn av1_mime_preserves_raw_colour_and_the_nclx_override() {
        let hdr = [
            0x81, 0x00, 0x4e, 0x00, 0x0a, 0x0d, 0x00, 0x00, 0x00, 0x01, 0x9f, 0xf9, 0xb5, 0xf3,
            0xa1, 0x22, 0x01, 0x28, 0x80,
        ];
        assert_eq!(
            av1(&hdr, None).as_deref(),
            Some("av01.0.00M.10.0.112.09.16.09.0")
        );
        let nclx = coreav_isobmff::Color {
            primaries: Some(2),
            transfer: Some(2),
            matrix: Some(2),
            full_range: Some(true),
        };
        assert_eq!(
            av1(&hdr, Some(nclx)).as_deref(),
            Some("av01.0.00M.10.0.112.02.02.02.1")
        );
    }

    #[test]
    fn av1_mime_states_a_signalled_colour_description_even_at_the_defaults() {
        // The configuration records the pinned ffmpeg writes for a 64x64
        // libaom stream with BT.709 colour and without colour; the pinned
        // ffprobe prints these strings for both.
        let signalled = [
            0x81, 0x00, 0x0c, 0x00, 0x0a, 0x0d, 0x00, 0x00, 0x00, 0x02, 0xaf, 0xff, 0x9f, 0x5f,
            0x32, 0x02, 0x02, 0x02, 0x08,
        ];
        assert_eq!(
            av1(&signalled, None).as_deref(),
            Some("av01.0.00M.08.0.110.01.01.01.0")
        );
        let unsignalled = [
            0x81, 0x00, 0x0c, 0x00, 0x0a, 0x0a, 0x00, 0x00, 0x00, 0x02, 0xaf, 0xff, 0x9b, 0x5f,
            0x20, 0x08,
        ];
        assert_eq!(av1(&unsignalled, None).as_deref(), Some("av01.0.00M.08"));
    }

    #[test]
    fn av1_mime_retains_monochrome_rgb_and_maximum_parameters() {
        for (record, expected) in [
            (
                vec![
                    0x81, 0x00, 0x1c, 0x00, 0x0a, 0x09, 0, 0, 0, 1, 0x9f, 0xf9, 0xb5, 0xf3, 0x54,
                ],
                "av01.0.00M.08.1.110.01.01.01.1",
            ),
            (
                vec![
                    0x81, 0x20, 0, 0, 0x0a, 0x0c, 0x20, 0, 0, 1, 0x9f, 0xf9, 0xb5, 0xf3, 0x40,
                    0x43, 0x40, 8,
                ],
                "av01.1.00M.08.0.000.01.13.00.1",
            ),
            (
                vec![
                    0x81, 0x1f, 0x0c, 0, 0x0a, 0x0a, 0, 0, 0, 0xf8, 0xcf, 0xfc, 0xda, 0xf9, 0x80,
                    0x40,
                ],
                "av01.0.31M.08",
            ),
        ] {
            assert_eq!(av1(&record, None).as_deref(), Some(expected));
        }
        assert_eq!(av1(&[0x81, 24, 0x0c, 0], None), None);
        assert_eq!(av1(&[0x81, 0, 0x0c], None), None);
    }

    #[test]
    fn vp9_mime_uses_the_record_or_the_required_level() {
        let mut entries = vec![("codec_name", s("vp9"))];
        stream(&mut entries, Some(&[1, 0, 0, 0, 2, 51, 0xa0]), &[], None);
        assert_eq!(
            entries.last(),
            Some(&("mime_codec_string", s("vp09.02.51.10")))
        );
        for (width, height, level) in [(48, 48, "10"), (1920, 1080, "40")] {
            let mut entries = vec![
                ("codec_name", s("vp9")),
                ("profile", s("Profile 0")),
                ("pix_fmt", s("yuv420p")),
                ("width", Value::Int(width)),
                ("height", Value::Int(height)),
                ("avg_frame_rate", s("25/1")),
            ];
            stream(&mut entries, None, &[], None);
            assert_eq!(
                entries.last(),
                Some(&("mime_codec_string", s(format!("vp09.00.{level}.08"))))
            );
        }
    }
}
