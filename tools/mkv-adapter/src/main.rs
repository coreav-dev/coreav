//! Oracle adapter for `coreav-mkv`: `mkv-adapter LEVEL FILE`.
//!
//! Prints one document of the per-level schema in `tools/oracle/SPEC.md` for
//! the `streams` or `packets` level; `count` only counts packets, for
//! timing. Names follow the oracle's vocabulary
//! (`ffprobe`'s codec and color names), which is the adapter's job; the
//! library reports code points and its own types. Every named codec is
//! allowed, so only codecs the library cannot name are dropped
//! (`MKV_ADAPTER_ALLOWLIST=default` selects the library default instead). A media
//! rejection is a successful run with `probe_ok: false`; only usage errors
//! exit non-zero. Extra `properties` keys carry the facts the
//! known-divergence overlay (`scripts/oracle_kd_mkv.py`) checks.

use coreav_mkv::{
    Allowlist, BufferedSource, Codec, Encoding, Error, FileSource, Limits, Profile, Rational,
    Segment, Track, TrackKind, probe,
};
use serde_json::{Value, json};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(level), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: mkv-adapter streams|packets|count FILE");
        return ExitCode::from(2);
    };
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let document = match level.as_str() {
        "streams" => streams(path, &name),
        "packets" => packets(path, &name),
        "count" => count(path),
        _ => {
            eprintln!("unsupported level {level}");
            return ExitCode::from(2);
        }
    };
    println!("{document}");
    ExitCode::SUCCESS
}

/// Every named codec, unless `MKV_ADAPTER_ALLOWLIST=default` asks for the
/// library default (what `coreav probe` uses, for comparing the binary with
/// the library).
fn open(path: &str) -> Result<Segment, Error> {
    let source = BufferedSource::new(FileSource::open(path)?);
    let allowlist = match std::env::var("MKV_ADAPTER_ALLOWLIST").as_deref() {
        Ok("default") => Allowlist::default(),
        _ => Allowlist::all(),
    };
    probe(source, &Limits::default(), &allowlist)
}

fn rejection_kind(error: &Error) -> &'static str {
    match error {
        Error::Truncated { .. } => "truncated_structure",
        Error::InvalidElement { .. } | Error::Overflow { .. } => "invalid_structure",
        Error::LimitExceeded { .. } => "limit",
        Error::NotMatroska => "unsupported_container",
        Error::Unsupported { .. } => "unsupported_feature",
        Error::CodecNotAllowed { .. } => "codec_not_allowed",
        Error::Io(_) => "io",
    }
}

fn rejection(schema: &str, name: &str, error: &Error) -> Value {
    let limit = match error {
        Error::LimitExceeded { limit, .. } => Some(format!("{limit:?}")),
        _ => None,
    };
    json!({
        "schema": schema, "file": name, "probe_ok": false, "format_duration_us": null,
        "streams": [],
        "properties": {
            "rejection": rejection_kind(error), "limit": limit, "error": error.to_string(),
        },
    })
}

fn kind_name(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Subtitle => "subtitle",
        TrackKind::Data => "data",
    }
}

/// `ffprobe`'s `codec_name` for a track.
fn codec_name(track: &Track) -> Option<String> {
    Some(
        match track.codec {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
            Codec::Av1 => "av1",
            Codec::Vp8 => "vp8",
            Codec::Vp9 => "vp9",
            Codec::ProRes => "prores",
            Codec::Mpeg1Video => "mpeg1video",
            Codec::Mpeg2Video => "mpeg2video",
            Codec::Mpeg4 => "mpeg4",
            Codec::Theora => "theora",
            Codec::Mjpeg => "mjpeg",
            Codec::Ffv1 => "ffv1",
            Codec::Aac => "aac",
            Codec::Mp3 => "mp3",
            Codec::Mp2 => "mp2",
            Codec::Opus => "opus",
            Codec::Vorbis => "vorbis",
            Codec::Flac => "flac",
            Codec::Ac3 => "ac3",
            Codec::Eac3 => "eac3",
            Codec::Dts => "dts",
            Codec::TrueHd => "truehd",
            Codec::Alac => "alac",
            Codec::SubRip => "subrip",
            Codec::Ass => "ass",
            Codec::WebVtt => "webvtt",
            Codec::Pgs => "hdmv_pgs_subtitle",
            Codec::VobSub => "dvd_subtitle",
            Codec::DvbSub => "dvb_subtitle",
            Codec::Pcm => return pcm_name(track),
            Codec::Other => return None,
        }
        .to_owned(),
    )
}

/// PCM names from the codec ID and `BitDepth` (the Matroska codec
/// mappings: `A_PCM/INT/LIT` little-endian signed, unsigned at 8 bits;
/// `A_PCM/INT/BIG` big-endian; `A_PCM/FLOAT/IEEE` little-endian float).
fn pcm_name(track: &Track) -> Option<String> {
    let bits = track.audio?.bit_depth?;
    Some(match track.codec_id.as_str() {
        "A_PCM/INT/LIT" if bits == 8 => "pcm_u8".to_owned(),
        "A_PCM/INT/LIT" => format!("pcm_s{bits}le"),
        "A_PCM/INT/BIG" if bits == 8 => "pcm_s8".to_owned(),
        "A_PCM/INT/BIG" => format!("pcm_s{bits}be"),
        "A_PCM/FLOAT/IEEE" => format!("pcm_f{bits}le"),
        _ => return None,
    })
}

fn primaries(code: Option<u16>) -> Option<&'static str> {
    Some(match code? {
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

fn transfer(code: Option<u16>) -> Option<&'static str> {
    Some(match code? {
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

fn matrix(code: Option<u16>) -> Option<&'static str> {
    Some(match code? {
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

fn profile_name(profile: Option<Profile>) -> Option<String> {
    let name = match profile? {
        Profile::H264 {
            profile_idc,
            constraint_flags,
            ..
        } => {
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
        Profile::ProRes(_) => return None,
    };
    Some(name.to_owned())
}

fn level(profile: Option<Profile>) -> Option<u8> {
    match profile? {
        Profile::H264 { level_idc, .. } | Profile::Hevc { level_idc, .. } => Some(level_idc),
        Profile::Av1 { level_idx, .. } => Some(level_idx),
        Profile::Vp9 { .. } | Profile::Aac { .. } | Profile::ProRes(_) => None,
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

fn ratio(a: u64, b: u64) -> Value {
    if a == 0 || b == 0 {
        return Value::Null;
    }
    let g = gcd(a, b);
    json!([a / g, b / g])
}

/// The stream time base: one Track Tick, `TimestampScale` nanoseconds.
fn time_base(segment: &Segment) -> (u64, u64) {
    let g = gcd(segment.timestamp_scale, 1_000_000_000);
    (segment.timestamp_scale / g, 1_000_000_000 / g)
}

fn encoding(encoding: &Encoding) -> Value {
    match encoding {
        Encoding::None => json!("none"),
        Encoding::HeaderStripping(prefix) => json!({"header_stripping": prefix.len()}),
        Encoding::Encrypted => json!("encrypted"),
        Encoding::Zlib => json!("zlib"),
        Encoding::Unsupported => json!("unsupported"),
    }
}

fn stream(segment: &Segment, track: &Track) -> Value {
    let video = track.video.filter(|_| track.kind == TrackKind::Video);
    let audio = track.audio.filter(|_| track.kind == TrackKind::Audio);
    let (sar, dar) = match video {
        Some(v) => match (v.pixel_width, v.pixel_height) {
            (Some(w), Some(h)) => {
                let shown_w = w.saturating_sub(v.crop[2]).saturating_sub(v.crop[3]);
                let shown_h = h.saturating_sub(v.crop[0]).saturating_sub(v.crop[1]);
                let (dw, dh) = v.display_aspect().unwrap_or((shown_w, shown_h));
                (
                    ratio(dw.saturating_mul(h), dh.saturating_mul(w)),
                    ratio(dw, dh),
                )
            }
            _ => (Value::Null, Value::Null),
        },
        None => (Value::Null, Value::Null),
    };
    let (num, den) = time_base(segment);
    let language = Some(track.language.as_str()).filter(|l| *l != "und");
    json!({
        "index": track.stream_index,
        "kind": kind_name(track.kind),
        // `ffprobe` prints a codec tag only for Video for Windows tracks.
        "codec_fourcc": track.fourcc.map(|f| String::from_utf8_lossy(&f).into_owned()),
        "codec_name": codec_name(track),
        "width": video.and_then(|v| v.pixel_width).or(track.image_size.map(|s| u64::from(s.0))),
        "height": video.and_then(|v| v.pixel_height).or(track.image_size.map(|s| u64::from(s.1))),
        // The oracle schema counts counter-clockwise degrees.
        "rotation": video.map(|v| json!(v.rotation().map(|cw| (360 - cw) % 360))),
        "time_base": [num, den],
        "timescale": if num == 1 { json!(den) } else { Value::Null },
        "duration_ts": null,
        "color_primaries": primaries(track.color.primaries),
        "color_transfer": transfer(track.color.transfer),
        "color_matrix": matrix(track.color.matrix),
        "color_range": track.color.full_range.map(|full| if full { "pc" } else { "tv" }),
        "sample_rate": audio.and_then(|a| a.sample_rate()),
        "channels": audio.map(|a| a.channels),
        "language": language,
        "sample_aspect_ratio": sar,
        "display_aspect_ratio": dar,
        "handler_name": null,
        // `ffprobe` names a VP8 profile by its `version` number.
        "profile": profile_name(track.profile).or_else(|| {
            (track.codec == Codec::Vp8).then(|| track.frame_profile.map(|v| v.to_string())).flatten()
        }),
        "level": level(track.profile),
        "properties": {
            "track_number": track.number,
            "codec_id": track.codec_id,
            "codec_private_len": track.codec_private.as_ref().map(Vec::len),
            "codec_delay_ns": track.codec_delay,
            "default_duration_ns": track.default_duration,
            "lacing": track.lacing,
            "encoding": encoding(&track.encoding),
            "projection_pose": video.map(|v| v.pose.to_vec()),
            "display": video.map(|v| json!({
                "width": v.display_width, "height": v.display_height, "unit": v.display_unit,
                "crop": v.crop.to_vec(),
            })),
            "profile_code_points": track.profile.map(|p| format!("{p:?}")),
            "warnings": track.warnings.iter().map(ToString::to_string).collect::<Vec<_>>(),
        },
    })
}

fn micros(value: Option<Rational>) -> Value {
    value.map_or(Value::Null, |r| {
        let scaled = r.num.saturating_mul(1_000_000);
        json!(Rational::new(scaled, r.den).map(Rational::round))
    })
}

fn segment_document(schema: &str, name: &str, segment: &Segment, streams: Vec<Value>) -> Value {
    json!({
        "schema": schema, "file": name, "probe_ok": true,
        "format_duration_us": micros(segment.duration_seconds()),
        "streams": streams,
        "properties": {
            "container": "matroska",
            "doc_type": format!("{:?}", segment.doc_type),
            "timestamp_scale": segment.timestamp_scale,
            "raw_duration": segment.duration,
            "segment_size_known": segment.size.is_some(),
            "cues": segment.cues.is_some(),
            "attachments": segment.attachments.iter().map(|a| json!({
                "media_type": a.media_type, "size": a.size,
            })).collect::<Vec<_>>(),
            "dropped": segment.dropped.iter().map(|d| json!({
                "stream_index": d.stream_index,
                "track_number": d.number,
                "codec_id": d.codec_id,
            })).collect::<Vec<_>>(),
            "warnings": segment.warnings.iter().map(ToString::to_string).collect::<Vec<_>>(),
        },
    })
}

fn streams(path: &str, name: &str) -> Value {
    let schema = "coreav-oracle/streams/1";
    match open(path) {
        Ok(segment) => {
            let list = segment.tracks.iter().map(|t| stream(&segment, t)).collect();
            segment_document(schema, name, &segment, list)
        }
        Err(error) => rejection(schema, name, &error),
    }
}

/// Probes and lists every packet without printing them: the packet count
/// per track, for timing the demuxer against `ffprobe -count_packets`.
fn count(path: &str) -> Value {
    let segment = match open(path) {
        Ok(segment) => segment,
        Err(error) => return json!({"error": error.to_string()}),
    };
    let source = match FileSource::open(path) {
        Ok(file) => BufferedSource::new(file),
        Err(error) => return json!({"error": Error::from(error).to_string()}),
    };
    let mut counts = vec![0u64; segment.tracks.len()];
    let mut error = None;
    for packet in segment.packets(source, &Limits::default()) {
        match packet {
            Ok(packet) => {
                if let Some(count) = counts.get_mut(packet.track) {
                    *count += 1;
                }
            }
            Err(e) => {
                error = Some(e.to_string());
                break;
            }
        }
    }
    json!({"packets": counts, "error": error})
}

fn packets(path: &str, name: &str) -> Value {
    let schema = "coreav-oracle/packets/1";
    let segment = match open(path) {
        Ok(segment) => segment,
        Err(error) => return rejection(schema, name, &error),
    };
    let source = match FileSource::open(path) {
        Ok(file) => BufferedSource::new(file),
        Err(error) => return rejection(schema, name, &error.into()),
    };
    let limits = Limits::default();
    let mut lists: Vec<Vec<Value>> = vec![Vec::new(); segment.tracks.len()];
    let mut truncated_at = None;
    let mut iter = segment.packets(source, &limits).with_inflated_sizes();
    for packet in iter.by_ref() {
        let packet = match packet {
            Ok(packet) => packet,
            Err(Error::Truncated { offset }) => {
                truncated_at = Some(offset);
                break;
            }
            Err(error) => return rejection(schema, name, &error),
        };
        let Some(list) = lists.get_mut(packet.track) else {
            continue;
        };
        list.push(json!({
            "pos": packet.block_offset,
            "pts": packet.pts,
            "dts": packet.dts,
            "size": packet.size(),
            "keyframe": packet.keyframe,
            "discard": false,
            "duration": packet.block_duration,
            "block_timestamp": packet.block_timestamp,
            "lace": packet.lace,
            "frames_in_block": packet.frames_in_block,
            "frame_offset": packet.offset,
            "invisible": packet.invisible,
        }));
    }
    let warnings: Vec<String> = iter.warnings().iter().map(ToString::to_string).collect();
    let mut list = Vec::new();
    for (track, packets) in segment.tracks.iter().zip(lists) {
        let mut entry = stream(&segment, track);
        if let Some(object) = entry.as_object_mut() {
            object.insert("packets".to_owned(), Value::Array(packets));
        }
        list.push(entry);
    }
    let mut document = segment_document(schema, name, &segment, list);
    if let Some(properties) = document
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        properties.insert("truncated_at".to_owned(), json!(truncated_at));
        properties.insert("packet_warnings".to_owned(), json!(warnings));
    }
    document
}
