//! `--ffprobe-compat`: the shape of `ffprobe -of json -show_format
//! -show_streams`, limited to the keys listed in
//! `docs/schema/probe-v1.md`. A key whose value is unknown is left out, as
//! `ffprobe` leaves it out; nothing is guessed. Every printed key is compared
//! with `ffprobe` by `scripts/compat_check.py`.

use crate::names;
use crate::probe::Probed;
use crate::time::rounded;
use coreav_isobmff::{Codec, Color, FourCc, Movie, Rational, Track, TrackKind};
use serde_json::{Map, Value, json};

/// `ffprobe`'s format name for every ISO base media and QuickTime file.
const FORMAT_NAME: &str = "mov,mp4,m4a,3gp,3g2,mj2";

/// The whole document. `filename` is printed as given, like `ffprobe` does.
pub fn document(probed: &Probed, filename: &str) -> Value {
    let movie = &probed.movie;
    let streams: Vec<Value> = movie.tracks.iter().map(|t| stream(movie, t)).collect();
    let mut format = Map::new();
    format.insert("filename".to_owned(), json!(filename));
    format.insert(
        "nb_streams".to_owned(),
        json!(movie.tracks.len().saturating_add(movie.dropped.len())),
    );
    format.insert("format_name".to_owned(), json!(FORMAT_NAME));
    // ffprobe's format duration is the span of the audio and video
    // streams (of every stream without them), not the movie header's.
    // As the oracle adapter counts it: whole stream ticks, then microseconds.
    let micros = movie
        .ffprobe_format_duration_micros()
        .and_then(|us| i64::try_from(us).ok().filter(|us| *us >= 0))
        .or_else(|| movie.duration_seconds().and_then(micros));
    if let Some(us) = micros {
        format.insert("duration".to_owned(), json!(seconds_string(us)));
    }
    format.insert("size".to_owned(), json!(probed.size.to_string()));
    if let Some(rate) = micros.and_then(|us| format_bit_rate(probed.size, us)) {
        format.insert("bit_rate".to_owned(), json!(rate.to_string()));
    }
    json!({"streams": streams, "format": format})
}

/// The size `ffprobe` reports for a 3GPP timed text stream: the track
/// header's width and height (signed 16.16 fixed point, ISO/IEC 14496-12
/// 8.3.2), whole pixels, when both whole parts are positive (measured on
/// crafted files; the `tx3g` entry's default text box plays no part).
/// Known limit: `ffprobe` also reports nothing for sizes it rejects as a
/// picture size, measured between 16,000 and 16,384 pixels a side; those
/// are reported here. A track that a `chap` track reference names, from
/// any track, gets no size: `ffprobe` lists it as a `bin_data` data stream
/// (`KD-CHAPTER-TEXT`). `tools/isobmff-adapter` maps the
/// oracle's fields the same way.
pub(crate) fn text_size(movie: &Movie, track: &Track) -> Option<(u32, u32)> {
    let whole = |fixed: u32| i32::from_be_bytes(fixed.to_be_bytes()) >> 16;
    let (width, height) = (
        whole(track.presentation_size.0),
        whole(track.presentation_size.1),
    );
    (track.description.codec == Codec::TimedText
        && !movie.chapter_references.contains(&track.track_id)
        && width > 0
        && height > 0)
        .then(|| (width.unsigned_abs(), height.unsigned_abs()))
}

/// The colour description `ffprobe` reports, from the two sources probe
/// keeps apart (`docs/design/probe-color.md`): the sample entry's `colr` box
/// (`color`) and the parameter set of the codec configuration, raw
/// (`parameter_set_color`). For H.264 the parameter set's colour description
/// when its video usability information carries one ("unspecified"
/// included) and its range when it signals one, else the box's (only for a
/// track with frames in the file); for HEVC and AV1 always the parameter set, with HEVC's range limited when the
/// parameter set does not signal it (ITU-T H.265 E.3.1); for other codecs
/// the box. `tools/isobmff-adapter` maps the oracle's fields the same way.
pub(crate) fn reported_color(track: &Track) -> Color {
    let d = &track.description;
    // ffprobe applies an H.264 SPS's colour only to a frame it decodes: a
    // track without samples in the file (an initialization segment) keeps
    // the box's.
    let frames = !track.external_data && (track.sample_count > 0 || track.fragment_samples > 0);
    precedence(d.codec, d.color, d.parameter_set_color, frames)
}

/// [`reported_color`] from its inputs: the codec, the box's colour, the
/// parameter set's (raw), and whether the track has frames in the file.
fn precedence(codec: Codec, boxed: Color, set: Option<Color>, frames: bool) -> Color {
    let reads_set = match codec {
        Codec::H264 => frames,
        Codec::Hevc | Codec::Av1 => true,
        _ => false,
    };
    let pick = |own: Option<u16>, other: Option<u16>| own.filter(|&code| code != 2).or(other);
    let color = match set.filter(|_| reads_set) {
        None => boxed,
        Some(set) => Color {
            primaries: pick(boxed.primaries, set.primaries),
            transfer: pick(boxed.transfer, set.transfer),
            matrix: pick(boxed.matrix, set.matrix),
            full_range: boxed.full_range.or(set.full_range),
        },
    };
    let limited = match codec {
        Codec::Hevc => set.is_some(),
        Codec::ProRes => true,
        _ => false,
    };
    Color {
        full_range: color.full_range.or(limited.then_some(false)),
        ..color
    }
}

/// A duration in seconds rounded to whole microseconds, half away from zero.
pub(crate) fn micros(seconds: Rational) -> Option<i64> {
    let scaled = Rational::new(seconds.num.checked_mul(1_000_000)?, seconds.den)?;
    rounded(scaled).filter(|us| *us >= 0)
}

/// Microseconds as `ffprobe` prints seconds: six decimals.
pub(crate) fn seconds_string(micros: i64) -> String {
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

/// The format bit rate: file size in bits over the duration, truncated.
pub(crate) fn format_bit_rate(size: u64, micros: i64) -> Option<u64> {
    let micros = u128::try_from(micros).ok().filter(|us| *us > 0)?;
    let bits = u128::from(size).checked_mul(8_000_000)?;
    u64::try_from(bits / micros).ok()
}

/// A four-character code as `ffprobe` prints `codec_tag_string`: printable
/// bytes as characters, others as `[N]`.
pub(crate) fn tag_string(fourcc: FourCc) -> String {
    let mut out = String::new();
    for byte in fourcc.0 {
        if byte.is_ascii_graphic() || byte == b' ' {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("[{byte}]"));
        }
    }
    out
}

pub(crate) fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

/// `N:D` in lowest terms; `None` when either side is zero.
pub(crate) fn ratio(a: u64, b: u64) -> Option<String> {
    if a == 0 || b == 0 {
        return None;
    }
    let g = gcd(a, b);
    Some(format!("{}:{}", a / g, b / g))
}

/// The largest term of a display aspect ratio `ffprobe` prints.
const MAX_RATIO_TERM: u64 = 1 << 20;

/// `num / den` as the closest fraction whose terms are both at most
/// [`MAX_RATIO_TERM`], which is how `ffprobe` prints a display aspect
/// ratio that does not reduce that far (1045314:784271 for a 914x686
/// picture of samples 2740:2739, exactly 1252180:939477): the best of
/// the convergents and intermediate fractions of its continued fraction
/// within the bound. `None` when either term is 0.
pub(crate) fn bounded_ratio(num: u64, den: u64) -> Option<(u64, u64)> {
    if num == 0 || den == 0 {
        return None;
    }
    let g = gcd(num, den);
    let (num, den) = (num / g, den / g);
    if num <= MAX_RATIO_TERM && den <= MAX_RATIO_TERM {
        return Some((num, den));
    }
    // Convergents p/q of num/den, and between two of them the
    // intermediate fractions, kept while both terms stay in bounds.
    let (mut p0, mut q0, mut p1, mut q1) = (0u64, 1u64, 1u64, 0u64);
    let (mut a, mut b) = (num, den);
    let mut best: Option<(u64, u64)> = None;
    // |p/q - num/den| times q * den, exactly.
    let off = |(p, q): (u64, u64)| {
        (u128::from(p) * u128::from(den)).abs_diff(u128::from(num) * u128::from(q))
    };
    // Whether x lies closer to num/den than y does.
    let closer = |x: (u64, u64), y: (u64, u64)| off(x) * u128::from(y.1) < off(y) * u128::from(x.1);
    while b != 0 {
        let q = a / b;
        for t in 1..=q {
            let p = t.checked_mul(p1).and_then(|v| v.checked_add(p0));
            let k = t.checked_mul(q1).and_then(|v| v.checked_add(q0));
            let (Some(p), Some(k)) = (p, k) else {
                return best;
            };
            if p > MAX_RATIO_TERM || k > MAX_RATIO_TERM {
                return best;
            }
            if best.is_none_or(|b| closer((p, k), b)) {
                best = Some((p, k));
            }
        }
        (p0, p1) = (p1, q * p1 + p0);
        (q0, q1) = (q1, q * q1 + q0);
        (a, b) = (b, a % b);
    }
    best
}

/// Ticks times the time base, as `ffprobe` prints a stream duration.
pub(crate) fn duration_string(ticks: i64, timescale: u32) -> Option<String> {
    if timescale == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let seconds = ticks as f64 * (1.0 / f64::from(timescale));
    Some(format!("{seconds:.6}"))
}

pub(crate) fn put(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), value);
    }
}

fn stream(movie: &Movie, track: &Track) -> Value {
    let d = &track.description;
    let video = track.kind == TrackKind::Video;
    let audio = track.kind == TrackKind::Audio;
    let mut s = Map::new();
    s.insert("index".to_owned(), json!(track.stream_index));
    put(
        &mut s,
        "codec_name",
        names::ffprobe_codec(d.codec, d.pcm).map(Value::from),
    );
    put(
        &mut s,
        "profile",
        d.profile.and_then(names::profile).map(Value::from),
    );
    s.insert(
        "codec_type".to_owned(),
        json!(names::ffprobe_kind(track.kind)),
    );
    s.insert("codec_tag_string".to_owned(), json!(tag_string(d.fourcc)));
    s.insert(
        "codec_tag".to_owned(),
        json!(format!("0x{:08x}", u32::from_le_bytes(d.fourcc.0))),
    );
    if video {
        put(&mut s, "width", d.width.map(Value::from));
        put(&mut s, "height", d.height.map(Value::from));
        if let (Some((h, v)), Some(w), Some(ht)) = (d.pixel_aspect, d.width, d.height) {
            put(
                &mut s,
                "sample_aspect_ratio",
                ratio(u64::from(h), u64::from(v)).map(Value::from),
            );
            put(
                &mut s,
                "display_aspect_ratio",
                ratio(
                    u64::from(w).saturating_mul(u64::from(h)),
                    u64::from(ht).saturating_mul(u64::from(v)),
                )
                .map(Value::from),
            );
        }
        let color = reported_color(track);
        put(
            &mut s,
            "color_range",
            color
                .full_range
                .map(|full| Value::from(if full { "pc" } else { "tv" })),
        );
        put(
            &mut s,
            "color_space",
            color.matrix.and_then(names::matrix).map(Value::from),
        );
        put(
            &mut s,
            "color_transfer",
            color.transfer.and_then(names::transfer).map(Value::from),
        );
        put(
            &mut s,
            "color_primaries",
            color.primaries.and_then(names::primaries).map(Value::from),
        );
    } else if let Some((width, height)) = text_size(movie, track) {
        put(&mut s, "width", Some(Value::from(width)));
        put(&mut s, "height", Some(Value::from(height)));
    }
    put(
        &mut s,
        "level",
        d.profile.and_then(names::level).map(Value::from),
    );
    if audio {
        // AAC: the rate its configuration declares, as a reader that does
        // not decode sees it; else the sample entry's.
        put(
            &mut s,
            "sample_rate",
            d.configured_sample_rate
                .or(d.sample_rate)
                .map(|r| Value::from(r.to_string())),
        );
        // A mono configuration under a stereo sample entry: the native
        // document keeps the entry's 2 for a possible implicit parametric
        // stereo, which only decoding shows; this output prints the
        // configuration's 1, the channels it declares (ISO/IEC 14496-12
        // 12.2.3), as ffprobe does without decoding.
        let channels = if d.implicit_ps_possible {
            Some(1)
        } else {
            d.channels
        };
        put(&mut s, "channels", channels.map(Value::from));
    }
    s.insert(
        "time_base".to_owned(),
        json!(format!("1/{}", track.timescale)),
    );
    let duration_ts = track.duration.and_then(rounded);
    put(&mut s, "duration_ts", duration_ts.map(Value::from));
    put(
        &mut s,
        "duration",
        duration_ts
            .and_then(|ts| duration_string(ts, track.timescale))
            .map(Value::from),
    );
    // No stream `bit_rate`: `ffprobe` divides by its own duration (AAC
    // priming included) and counts part of the samples of fragmented files,
    // so the exact value cannot be checked against it
    // (`docs/measurements/0025-probe-metadata.md`). The native output has it.
    let mut tags = Map::new();
    put(
        &mut tags,
        "language",
        crate::names::language(track.language).map(Value::from),
    );
    put(
        &mut tags,
        "handler_name",
        track.handler_name.clone().map(Value::from),
    );
    if !tags.is_empty() {
        s.insert("tags".to_owned(), Value::Object(tags));
    }
    let mut side_data = Vec::new();
    // The Dolby Vision configuration record first, as `ffprobe` lists it.
    if let Some(dv) = d.dolby_vision.filter(|_| video) {
        side_data.push(json!({
            "side_data_type": "DOVI configuration record",
            "dv_version_major": dv.version_major,
            "dv_version_minor": dv.version_minor,
            "dv_profile": dv.profile,
            "dv_level": dv.level,
            "rpu_present_flag": u8::from(dv.rpu_present),
            "el_present_flag": u8::from(dv.el_present),
            "bl_present_flag": u8::from(dv.bl_present),
            "dv_bl_signal_compatibility_id": dv.bl_signal_compatibility_id,
        }));
    }
    // `ffprobe` reports a display matrix rotation in counter-clockwise
    // degrees in [-180, 180); none for an identity matrix.
    if let Some(clockwise) = track.rotation.filter(|r| *r != 0 && video) {
        let ccw = (360 - i32::from(clockwise % 360)) % 360;
        let signed = if ccw >= 180 { ccw - 360 } else { ccw };
        side_data.push(json!({"side_data_type": "Display Matrix", "rotation": signed}));
    }
    if !side_data.is_empty() {
        s.insert("side_data_list".to_owned(), Value::Array(side_data));
    }
    Value::Object(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_aspect_ratios_within_twenty_bits() {
        // 914x686 at 2740:2739 is 1252180:939477, which `ffprobe` prints
        // as the closest fraction of terms up to 2^20.
        assert_eq!(
            bounded_ratio(914 * 2740, 686 * 2739),
            Some((1_045_314, 784_271))
        );
        assert_eq!(bounded_ratio(1920, 1080), Some((16, 9)));
        assert_eq!(bounded_ratio(0, 9), None);
    }

    #[test]
    fn number_formats() {
        assert_eq!(seconds_string(2_000_000), "2.000000");
        assert_eq!(seconds_string(1_234_567), "1.234567");
        assert_eq!(format_bit_rate(92_353, 2_000_000), Some(369_412));
        assert_eq!(format_bit_rate(1, 0), None);
        assert_eq!(duration_string(25_600, 12_800).as_deref(), Some("2.000000"));
        assert_eq!(ratio(1280, 960).as_deref(), Some("4:3"));
        assert_eq!(tag_string(FourCc(*b"avc1")), "avc1");
        assert_eq!(tag_string(FourCc([0, b'a', b'b', b'c'])), "[0]abc");
    }

    #[test]
    fn micros_round_half_away_from_zero() {
        let third = Rational::new(1, 3).unwrap();
        assert_eq!(micros(third), Some(333_333));
        let half_micro = Rational::new(1, 2_000_000).unwrap();
        assert_eq!(micros(half_micro), Some(1));
    }
}

#[cfg(test)]
mod colour_tests {
    use super::precedence;
    use coreav_isobmff::{Codec, Color};

    fn c(p: Option<u16>, t: Option<u16>, m: Option<u16>, r: Option<bool>) -> Color {
        Color {
            primaries: p,
            transfer: t,
            matrix: m,
            full_range: r,
        }
    }

    #[test]
    fn ffprobe_precedence() {
        let boxed = c(Some(1), Some(1), Some(1), Some(true));
        let sps = c(Some(6), Some(6), Some(6), Some(false));
        let range_only = c(None, None, None, Some(false));
        let silent = Color::default();
        let unspecified = c(Some(2), Some(2), Some(2), None);
        // H.264 with frames, HEVC and AV1: each code of the box unless it is
        // 2 (unspecified), else the parameter set's; the range likewise.
        assert_eq!(precedence(Codec::H264, boxed, Some(sps), true), boxed);
        assert_eq!(precedence(Codec::H264, unspecified, Some(sps), true), sps);
        assert_eq!(precedence(Codec::H264, silent, Some(sps), true), sps);
        assert_eq!(precedence(Codec::Hevc, boxed, Some(sps), true), boxed);
        assert_eq!(
            precedence(Codec::Av1, silent, Some(range_only), true),
            range_only
        );
        assert_eq!(precedence(Codec::Av1, boxed, Some(sps), true), boxed);
        // H.264 without frames, or without a parameter set: the box's.
        assert_eq!(
            precedence(Codec::H264, unspecified, Some(sps), false),
            unspecified
        );
        assert_eq!(precedence(Codec::H264, boxed, None, true), boxed);
        // HEVC with a parameter set, and ProRes: an unsignalled range is
        // limited.
        assert_eq!(
            precedence(Codec::Hevc, silent, Some(silent), true),
            range_only
        );
        assert_eq!(precedence(Codec::Hevc, silent, None, true), silent);
        assert_eq!(precedence(Codec::ProRes, silent, None, true), range_only);
        assert_eq!(precedence(Codec::ProRes, boxed, None, true), boxed);
        // Other codecs: the box's.
        assert_eq!(precedence(Codec::Vp9, silent, Some(sps), true), silent);
    }
}
