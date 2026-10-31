use super::model::Value;
use super::{Entries, mkv::video, ts::params};

pub fn stream(entries: &mut Entries, config: Option<&[u8]>, first: &[u8]) {
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
        "aac" | "aac_latm" => {
            let object = match text("profile") {
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
            };
            object.map(|n| format!("mp4a.40.{n}"))
        }
        "ac3" => Some("ac-3".to_owned()),
        "eac3" => Some("ec-3".to_owned()),
        "opus" => Some("opus".to_owned()),
        "flac" => Some("flac".to_owned()),
        "av1" => config.and_then(|c| match c {
            [marker, profile_level, flags, _, ..] if *marker == 0x81 => {
                let profile = profile_level >> 5;
                let level = profile_level & 31;
                let tier = if flags & 0x80 == 0 { 'M' } else { 'H' };
                let depth = if flags & 0x40 == 0 {
                    8
                } else if flags & 0x20 == 0 {
                    10
                } else {
                    12
                };
                Some(format!("av01.{profile}.{level:02}{tier}.{depth:02}"))
            }
            _ => None,
        }),
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
            .position(|(k, _)| *k == "bits_per_sample")
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
            stream(&mut entries, Some(&config), &[]);
            assert_eq!(
                entries.last(),
                Some(&("mime_codec_string", s("avc1.640028")))
            );
        }
        let mut entries = vec![("codec_name", s("h264"))];
        stream(&mut entries, Some(&[1, 100]), &[]);
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
            stream(&mut entries, None, &[]);
            assert_eq!(entries.last(), Some(&("mime_codec_string", s(mime))));
        }
    }

    #[test]
    fn removed_decoder_fields_are_not_listed() {
        let mut entries = vec![
            ("codec_name", s("h264")),
            ("refs", Value::Int(1)),
            ("closed_captions", Value::Int(0)),
            ("film_grain", Value::Int(0)),
        ];
        stream(&mut entries, None, &[]);
        assert_eq!(entries, vec![("codec_name", s("h264"))]);
    }

    #[test]
    fn av1_mime_uses_the_configuration_record() {
        let mut entries = vec![("codec_name", s("av1"))];
        stream(&mut entries, Some(&[0x81, 0x28, 0xc0, 0]), &[]);
        assert_eq!(
            entries.last(),
            Some(&("mime_codec_string", s("av01.1.08H.10")))
        );
    }

    #[test]
    fn vp9_mime_uses_the_record_or_the_required_level() {
        let mut entries = vec![("codec_name", s("vp9"))];
        stream(&mut entries, Some(&[1, 0, 0, 0, 2, 51, 0xa0]), &[]);
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
            stream(&mut entries, None, &[]);
            assert_eq!(
                entries.last(),
                Some(&("mime_codec_string", s(format!("vp09.00.{level}.08"))))
            );
        }
    }
}
