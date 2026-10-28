//! Oracle adapter for `coreav-mpegts`: `ts-adapter LEVEL FILE`.
//!
//! Prints one document of the per-level schema in `tools/oracle/SPEC.md` for
//! the `streams` or `packets` level; `count` only counts packets, for
//! timing. Names follow the oracle's vocabulary (`ffprobe`'s codec, tag and
//! color names), which is the adapter's job; the library reports code points
//! and its own types. Every named codec is allowed
//! (`TS_ADAPTER_ALLOWLIST=default` selects the library default instead). A
//! media rejection is a successful run with `probe_ok: false`; only usage
//! errors exit non-zero. Extra `properties` keys carry facts for the
//! known-divergence overlay.

use coreav_isobmff::Profile;
use coreav_mpegts::{
    Allowlist, BufferedSource, Codec, Error, FileSource, Limits, PacketSize, Stream, StreamKind,
    TransportStream, arib_caption_profile, probe,
};
use serde_json::{Value, json};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(level), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: ts-adapter streams|packets|count FILE");
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

fn open(path: &str) -> Result<TransportStream, Error> {
    let source = BufferedSource::new(FileSource::open(path)?);
    let allowlist = match std::env::var("TS_ADAPTER_ALLOWLIST").as_deref() {
        Ok("default") => Allowlist::default(),
        _ => Allowlist::all(),
    };
    probe(source, &Limits::default(), &allowlist)
}

fn rejection(schema: &str, name: &str, error: &Error) -> Value {
    let kind = match error {
        Error::NotTransportStream => "unsupported_container",
        Error::NoPrograms | Error::Invalid { .. } => "invalid_structure",
        Error::CodecNotAllowed { .. } => "codec_not_allowed",
        Error::LimitExceeded { .. } => "limit",
        Error::Io(_) => "io",
    };
    json!({
        "schema": schema, "file": name, "probe_ok": false, "format_duration_us": null,
        "streams": [],
        "properties": { "rejection": kind, "error": error.to_string() },
    })
}

/// `ffprobe`'s `codec_type`: `null` for a stream whose type names no codec
/// it knows either (measured: it leaves such a stream's media type
/// unknown), `data` for a private stream (type 0x06) it reads as binary
/// data.
fn kind_name(stream: &Stream) -> Option<&'static str> {
    if stream.codec == Codec::Other && stream.stream_type != 0x06 {
        return None;
    }
    Some(match stream.kind {
        StreamKind::Video => "video",
        StreamKind::Audio => "audio",
        StreamKind::Subtitle => "subtitle",
        StreamKind::Data => "data",
    })
}

/// `ffprobe`'s `codec_name`.
fn codec_name(stream: &Stream) -> Option<&'static str> {
    Some(match stream.codec {
        Codec::H264 => "h264",
        Codec::Hevc => "hevc",
        Codec::Vvc => "vvc",
        Codec::Mpeg1Video => "mpeg1video",
        Codec::Mpeg2Video => "mpeg2video",
        Codec::Mpeg4 => "mpeg4",
        Codec::Aac => "aac",
        Codec::AacLatm => "aac_latm",
        Codec::MpegAudio => match stream.audio.map(|a| a.detail) {
            Some(1) => "mp1",
            Some(3) => "mp3",
            _ => "mp2",
        },
        Codec::Ac3 => "ac3",
        Codec::Eac3 => "eac3",
        Codec::Dts => "dts",
        Codec::BlurayPcm => "pcm_bluray",
        Codec::Opus => "opus",
        Codec::Scte35 => "scte_35",
        Codec::Id3 => "timed_id3",
        Codec::DvbSubtitle => "dvb_subtitle",
        Codec::DvbTeletext => "dvb_teletext",
        Codec::PgsSubtitle => "hdmv_pgs_subtitle",
        Codec::AribCaption => "arib_caption",
        Codec::Epg => "epg",
        // A private stream (type 0x06) no descriptor names and whose first
        // PES packet shows no frame header: `ffprobe` reads it as binary
        // data. Any other unnamed type gets no codec name.
        Codec::Other if stream.stream_type == 0x06 => "bin_data",
        Codec::Other => return None,
    })
}

/// `ffprobe`'s `codec_tag_string`: the identifier the stream's descriptors
/// give its format, else the stream type, as a little-endian
/// four-character code with unprintable bytes as `[n]`.
fn codec_tag(stream: &Stream) -> Option<String> {
    let bytes = stream
        .format_identifier
        .unwrap_or([stream.stream_type, 0, 0, 0]);
    if bytes == [0, 0, 0, 0] {
        // `ffprobe` prints no `codec_tag_string` for a tag of zero, which
        // is what a stream no Program Map Table declares has.
        return None;
    }
    Some(
        bytes
            .iter()
            .map(|&b| {
                if b.is_ascii_alphanumeric() || b" ._-".contains(&b) {
                    char::from(b).to_string()
                } else {
                    format!("[{b}]")
                }
            })
            .collect(),
    )
}

/// `ffprobe`'s profile name.
fn profile_name(s: &Stream) -> Option<String> {
    if s.codec == Codec::AribCaption {
        return arib_caption_profile(&s.descriptors).map(ToOwned::to_owned);
    }
    if let Some((profile, _)) = s.mpeg2_profile {
        return Some(
            match profile {
                5 => "Simple",
                4 => "Main",
                3 => "SNR Scalable",
                2 => "Spatially Scalable",
                1 => "High",
                _ => return None,
            }
            .to_owned(),
        );
    }
    let name = match s.profile? {
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
            _ => return None,
        },
        _ => return None,
    };
    Some(name.to_owned())
}

fn level(s: &Stream) -> Option<u8> {
    if let Some((_, level)) = s.mpeg2_profile {
        return Some(level);
    }
    match s.profile? {
        Profile::H264 { level_idc, .. } | Profile::Hevc { level_idc, .. } => Some(level_idc),
        _ => None,
    }
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

fn stream(s: &Stream) -> Value {
    let video = s.kind == StreamKind::Video;
    json!({
        // `ffprobe`'s index: the stream's place among every stream the
        // tables name, those the allowlist leaves out included.
        "index": s.stream_index,
        "kind": kind_name(s),
        "codec_fourcc": codec_tag(s),
        "codec_name": codec_name(s),
        // `ffprobe` prints a size for every video stream, 0 when it has
        // not learnt one (a stream whose parameter sets it never saw).
        "width": s.video.map(|v| v.width).or(video.then_some(0)),
        "height": s.video.map(|v| v.height).or(video.then_some(0)),
        "rotation": if video { Some(0) } else { None },
        "time_base": [1, 90000],
        "timescale": 90000,
        "duration_ts": s.duration(),
        "color_primaries": primaries(s.color.primaries),
        "color_transfer": transfer(s.color.transfer),
        "color_matrix": matrix(s.color.matrix),
        "color_range": s.color.full_range.map(|f| if f { "pc" } else { "tv" }),
        "sample_rate": s.audio.map(|a| a.sample_rate),
        "channels": s.audio.map(|a| a.channels).filter(|c| *c > 0),
        "language": s.language,
        "sample_aspect_ratio": null,
        "display_aspect_ratio": null,
        "handler_name": null,
        "profile": profile_name(s),
        "level": level(s),
        "properties": {
            "pid": s.pid, "stream_type": s.stream_type, "programs": s.programs,
            "start_pts": s.start,
            "end_unit_ticks": s.end.zip(s.last_pts).map(|(end, last)| end - last),
        },
    })
}

fn document(schema: &str, name: &str, ts: &TransportStream, streams: Vec<Value>) -> Value {
    let duration_us = ts
        .duration()
        .map(|ticks| (i128::from(ticks) * 1_000_000 + 45_000).div_euclid(90_000));
    json!({
        "schema": schema, "file": name, "probe_ok": true,
        "format_duration_us": duration_us,
        "streams": streams,
        "properties": {
            "packet_size": format!("{:?}", ts.packet_size),
            "programs": ts.programs.len(),
            "dropped": ts.dropped.iter().map(|d| json!({"pid": d.pid, "stream_type": d.stream_type})).collect::<Vec<_>>(),
            "warnings": ts.warnings.iter().map(|w| format!("{w:?}")).collect::<Vec<_>>(),
        },
    })
}

fn streams(path: &str, name: &str) -> Value {
    let schema = "coreav-oracle/streams/1";
    match open(path) {
        Ok(ts) => {
            let list = ts.streams.iter().map(stream).collect();
            document(schema, name, &ts, list)
        }
        Err(error) => rejection(schema, name, &error),
    }
}

fn packets(path: &str, name: &str) -> Value {
    let schema = "coreav-oracle/packets/1";
    let ts = match open(path) {
        Ok(ts) => ts,
        Err(error) => return rejection(schema, name, &error),
    };
    let source = match FileSource::open(path) {
        Ok(file) => BufferedSource::new(file),
        Err(error) => return rejection(schema, name, &error.into()),
    };
    // `ffprobe` reports the position of a 204-byte packet 16 bytes before
    // it (where the previous packet's parity bytes start).
    let shift = if ts.packet_size == PacketSize::Ts204 {
        16
    } else {
        0
    };
    let mut lists: Vec<Vec<Value>> = vec![Vec::new(); ts.streams.len()];
    let mut iter = ts.packets(source, &Limits::default());
    for packet in iter.by_ref() {
        let packet = match packet {
            Ok(packet) => packet,
            Err(error) => return rejection(schema, name, &error),
        };
        // A stream a later PMT version added gets its list where it appears.
        if lists.len() <= packet.stream {
            lists.resize(packet.stream + 1, Vec::new());
        }
        if let Some(list) = lists.get_mut(packet.stream) {
            list.push(json!({
                "pos": packet.pos.map(|p| p.saturating_sub(shift)),
                "pts": packet.pts,
                "dts": packet.dts,
                "size": packet.data.len(),
                "keyframe": packet.keyframe,
                "discard": false,
                "duration": packet.duration,
            }));
        }
    }
    let warnings: Vec<String> = iter.warnings().iter().map(|w| format!("{w:?}")).collect();
    let all = iter.streams().to_vec();
    lists.resize(all.len(), Vec::new());
    let list = all
        .iter()
        .zip(lists)
        .map(|(s, packets)| {
            let mut entry = stream(s);
            if let Some(object) = entry.as_object_mut() {
                object.insert("packets".to_owned(), Value::Array(packets));
            }
            entry
        })
        .collect();
    let mut doc = document(schema, name, &ts, list);
    if let Some(properties) = doc.get_mut("properties").and_then(Value::as_object_mut) {
        properties.insert("packet_warnings".to_owned(), json!(warnings));
    }
    doc
}

fn count(path: &str) -> Value {
    let ts = match open(path) {
        Ok(ts) => ts,
        Err(error) => return json!({ "error": error.to_string() }),
    };
    let Ok(file) = FileSource::open(path) else {
        return json!({ "error": "open" });
    };
    let mut packets = 0u64;
    for packet in ts.packets(BufferedSource::new(file), &Limits::default()) {
        if packet.is_err() {
            break;
        }
        packets += 1;
    }
    json!({ "packets": packets })
}
