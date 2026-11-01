//! The native probe output, schema `coreav.probe/1` (every key is
//! documented in `docs/schema/probe-v1.md`).

use crate::names;
use crate::probe::Probed;
use crate::time::{rfc3339, scale, to_f64};
use coreav_isobmff::{
    Codec, DeclaredBitRate, DropReason, DurationSource, EditList, Profile, Rational, Track,
    TrackKind,
};
use serde_json::{Map, Value};

/// A JSON object whose values are moved in with `Value::from`.
/// `serde_json::json!` converts each value through `to_value(&value)`,
/// which copies a nested `Value` once for every level it sits under; the
/// probe document nests four deep, so the copies (and dropping them) cost
/// more than building it. The keys come out sorted either way (`Map` is a
/// `BTreeMap`), so the output is the same.
macro_rules! object {
    ($($key:literal: $value:expr),* $(,)?) => {{
        let mut map = Map::new();
        $(map.insert($key.to_owned(), Value::from($value));)*
        Value::Object(map)
    }};
}

/// The schema identifier.
pub const SCHEMA: &str = "coreav.probe/1";

/// The whole document.
pub fn document(probed: &Probed) -> Value {
    let movie = &probed.movie;
    let mut warnings = Vec::new();
    if movie.has_fragments() && !probed.scanned {
        warnings.push(Value::from(
            "movie fragments were not read; durations and sample counts that only \
             fragments provide are null (use --scan-fragments)",
        ));
    }
    let streams: Vec<Value> = movie
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| {
            let scanned_size = probed
                .frame_sizes
                .as_ref()
                .and_then(|sizes| sizes.get(index).copied().flatten());
            stream(track, index, probed.scanned, scanned_size)
        })
        .collect();
    let dropped: Vec<Value> = movie
        .dropped
        .iter()
        .map(|d| {
            object!(
                "stream_index": d.stream_index,
                "track_id": d.track_id,
                "fourcc": d.fourcc.to_string(),
                "reason": match d.reason {
                    DropReason::CodecNotAllowed => "codec_not_allowed",
                },
            )
        })
        .collect();
    let duration = movie
        .duration_seconds()
        .and_then(|seconds| scale(seconds, u64::from(movie.timescale)))
        .map_or(Value::Null, |ticks| time(ticks, movie.timescale));
    let presentation_duration = movie
        .presentation_duration_seconds()
        .and_then(|seconds| scale(seconds, u64::from(movie.timescale)))
        .map_or(Value::Null, |ticks| time(ticks, movie.timescale));
    object!(
        "schema": SCHEMA,
        "file": object!("size": probed.size),
        "container": object!(
            "major_brand": movie.major_brand.map(|b| b.to_string()),
            "fragmented": movie.has_fragments(),
            "timescale": movie.timescale,
            "creation_time": rfc3339(movie.creation_time),
        ),
        "duration": duration,
        "presentation_duration": presentation_duration,
        "streams": streams,
        "dropped_streams": dropped,
        "warnings": warnings,
    )
}

/// An exact rational as `{num, den}`; `null` when the numerator is outside
/// the 64-bit range (from -2^63 to 2^64 - 1).
pub fn rational(value: Rational) -> Value {
    if let Ok(num) = i64::try_from(value.num) {
        object!("num": num, "den": value.den)
    } else if let Ok(num) = u64::try_from(value.num) {
        object!("num": num, "den": value.den)
    } else {
        Value::Null
    }
}

/// A time: exact ticks, their timescale, and seconds as a convenience copy.
pub fn time(ticks: Rational, timescale: u32) -> Value {
    let seconds = Rational::new(
        ticks.num,
        ticks.den.saturating_mul(u64::from(timescale.max(1))),
    );
    object!(
        "ticks": rational(ticks),
        "timescale": timescale,
        "seconds": seconds.map(to_f64),
    )
}

fn duration_source(source: DurationSource) -> &'static str {
    match source {
        DurationSource::MediaHeader => "media_header",
        DurationSource::MediaEdit => "media_edit",
        DurationSource::Observed => "observed",
        DurationSource::Unknown => "unknown",
    }
}

/// The edit list's classification, with its entries as stored
/// (`docs/design/edit-lists.md`, decision 4).
fn edit_list(track: &Track) -> Value {
    let mut value = match track.edit_list {
        EditList::None => object!("type": "none"),
        EditList::Simple {
            empty,
            media_time,
            segment,
        } => object!(
            "type": "simple",
            "empty": empty,
            "media_time": media_time,
            "segment": segment,
        ),
        EditList::Multiple => object!("type": "multiple"),
        EditList::Unsupported => object!("type": "unsupported"),
    };
    let entries: Vec<Value> = track
        .edits
        .iter()
        .map(|e| {
            object!(
                "segment_duration": e.segment_duration,
                "media_time": e.media_time,
                "media_rate": e.media_rate,
            )
        })
        .collect();
    if let Some(object) = value.as_object_mut() {
        object.insert("entries".to_owned(), Value::Array(entries));
    }
    value
}

fn declared(rate: Option<DeclaredBitRate>) -> Value {
    rate.map_or(Value::Null, |b| {
        object!(
            "source": b.source.to_string(),
            "average": b.average,
            "maximum": b.maximum,
            "buffer_size": b.buffer_size,
        )
    })
}

/// The `profile` object: code points, `type` and the `ffprobe` name.
pub fn profile(profile: Option<Profile>) -> Value {
    let Some(profile) = profile else {
        return Value::Null;
    };
    let name = names::profile(profile);
    match profile {
        Profile::H264 {
            profile_idc,
            constraint_flags,
            level_idc,
        } => object!(
            "type": "h264",
            "name": name,
            "profile_idc": profile_idc,
            "constraint_flags": constraint_flags,
            "level_idc": level_idc,
        ),
        Profile::Hevc {
            profile_space,
            high_tier,
            profile_idc,
            compatibility_flags,
            constraint_flags,
            level_idc,
        } => object!(
            "type": "hevc",
            "name": name,
            "profile_space": profile_space,
            "high_tier": high_tier,
            "profile_idc": profile_idc,
            "compatibility_flags": compatibility_flags,
            "constraint_flags": constraint_flags,
            "level_idc": level_idc,
        ),
        Profile::Av1 {
            profile,
            level_idx,
            high_tier,
            high_bitdepth,
            twelve_bit,
            monochrome,
        } => object!(
            "type": "av1",
            "name": name,
            "profile": profile,
            "level_idx": level_idx,
            "high_tier": high_tier,
            "high_bitdepth": high_bitdepth,
            "twelve_bit": twelve_bit,
            "monochrome": monochrome,
        ),
        Profile::Vp9 {
            profile,
            level,
            bit_depth,
        } => object!(
            "type": "vp9",
            "name": name,
            "profile": profile,
            "level": level,
            "bit_depth": bit_depth,
        ),
        Profile::Aac {
            object_type,
            sbr,
            ps,
        } => object!(
            "type": "aac",
            "name": name,
            "object_type": object_type,
            "sbr": sbr,
            "ps": ps,
        ),
        Profile::ProRes(_) => object!("type": "prores", "name": name),
    }
}

fn size(size: Option<(u32, u32)>) -> Value {
    size.map_or(
        Value::Null,
        |(width, height)| object!("width": width, "height": height),
    )
}

fn video(track: &Track, scanned_size: Option<(u32, u32)>) -> Value {
    if track.kind != TrackKind::Video {
        return Value::Null;
    }
    let d = &track.description;
    let color = d.color;
    object!(
        "width": d.width,
        "height": d.height,
        "parameter_set_size": size(d.parameter_set_size),
        "scanned_size": size(scanned_size),
        "pixel_aspect": d.pixel_aspect.map(|(h, v)| object!("num": h, "den": v)),
        "rotation": track.rotation,
        "matrix": track.matrix,
        "color": object!(
            "primaries": color.primaries,
            "transfer": color.transfer,
            "matrix": color.matrix,
            "full_range": color.full_range,
        ),
        "parameter_set_color": d.parameter_set_color.map(|c| object!(
            "primaries": c.primaries,
            "transfer": c.transfer,
            "matrix": c.matrix,
            "full_range": c.full_range,
        )),
    )
}

fn audio(track: &Track) -> Value {
    if track.kind != TrackKind::Audio {
        return Value::Null;
    }
    let d = &track.description;
    let sample_rate = match d.codec {
        Codec::Flac | Codec::Alac => d.configured_sample_rate.or(d.sample_rate),
        _ => d.sample_rate,
    };
    object!(
        "channels": d.channels,
        "implicit_ps_possible": d.implicit_ps_possible,
        "sample_rate": sample_rate,
        "pcm": d.pcm.map(|p| object!(
            "float": p.float,
            "bits": p.bits,
            "little_endian": p.little_endian,
            "signed": p.signed,
        )),
    )
}

/// Stream warnings: the library's, and a difference between the size the
/// sample entry declares and the size the parameter sets or the scanned
/// samples give (`docs/design/frame-sizes.md`, decision 5).
fn warnings(track: &Track, scanned_size: Option<(u32, u32)>) -> Vec<String> {
    let mut out: Vec<String> = track.warnings.iter().map(ToString::to_string).collect();
    let d = &track.description;
    if let (Some(width), Some(height)) = (d.width, d.height) {
        let declared = (u32::from(width), u32::from(height));
        for (what, verified) in [
            ("parameter sets give", d.parameter_set_size),
            ("the scanned samples give", scanned_size),
        ] {
            if let Some((w, h)) = verified.filter(|v| *v != declared) {
                out.push(format!(
                    "declared size {}x{}, {what} {w}x{h}",
                    declared.0, declared.1
                ));
            }
        }
    }
    out
}

fn stream(track: &Track, index: usize, scanned: bool, scanned_size: Option<(u32, u32)>) -> Value {
    let d = &track.description;
    let sample_count =
        scanned.then(|| u64::from(track.sample_count).saturating_add(track.fragment_samples));
    let bit_rate = track.bit_rate().map_or(
        Value::Null,
        |rate| object!("exact": rational(rate), "bits_per_second": to_f64(rate)),
    );
    object!(
        "index": index,
        "stream_index": track.stream_index,
        "track_id": track.track_id,
        "kind": names::kind(track.kind),
        "codec": names::codec(d.codec),
        "fourcc": d.fourcc.to_string(),
        "handler": track.handler.to_string(),
        "handler_name": track.handler_name.clone(),
        "language": names::language(track.language),
        "timescale": track.timescale,
        "duration": track.duration.map_or(Value::Null, |ticks| time(ticks, track.timescale)),
        "duration_source": duration_source(track.duration_source),
        "sample_count": sample_count,
        "sample_descriptions": track.descriptions,
        "bit_rate": bit_rate,
        "declared_bit_rate": declared(d.declared_bit_rate),
        "profile": profile(d.profile),
        "edit_list": edit_list(track),
        "creation_time": rfc3339(track.media_creation_time),
        "external_data": track.external_data,
        "tables_valid": track.tables_valid,
        "video": video(track, scanned_size),
        "audio": audio(track),
        "warnings": warnings(track, scanned_size),
    )
}
