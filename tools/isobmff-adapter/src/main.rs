//! Oracle adapter for `coreav-isobmff`: `isobmff-adapter LEVEL FILE`.
//!
//! Prints one document of the per-level schema in `tools/oracle/SPEC.md` for
//! the `streams` or `packets` level. Names follow the oracle's vocabulary
//! (ffprobe's codec and color names), which is the adapter's job; the
//! library itself reports code points and its own types. A media rejection is
//! a successful run with `probe_ok: false`; only usage errors exit non-zero.
//! Extra `properties` keys carry the facts the known-divergence overlay
//! (`scripts/oracle_kd.py`) checks.

use coreav_isobmff::access::{Av1FrameStart, av1_config_reduced};
use coreav_isobmff::{
    Allowlist, Budget, BufferedSource, Codec, Color, DroppedStream, EditList, Error, FileSource,
    Limits, Movie, ProResProfile, Profile, Rational, Track, TrackKind, probe, unix_time,
};
use serde_json::{Value, json};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(level), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: isobmff-adapter streams|packets FILE");
        return ExitCode::from(2);
    };
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let document = match level.as_str() {
        "streams" => streams(path, &name),
        "packets" => packets(path, &name),
        _ => {
            eprintln!("unsupported level {level}");
            return ExitCode::from(2);
        }
    };
    println!("{document}");
    ExitCode::SUCCESS
}

/// Probe, then scan fragments when a fragmented
/// track's duration is not declared, so the oracle checks the full duration
/// path, or when its sample bytes are unknown, so
/// the bit rate can be compared. The probe-only results are covered by unit
/// tests.
fn open(path: &str) -> Result<Movie, Error> {
    let limits = Limits::default();
    let mut source = BufferedSource::new(FileSource::open(path)?);
    let mut movie = probe(&mut source, &limits, &Allowlist::default())?;
    if movie.needs_fragment_scan() {
        movie.scan_fragments(&mut source, &limits)?;
    }
    Ok(movie)
}

fn rejection(schema: &str, name: &str, error: &Error) -> Value {
    let kind = match error {
        Error::Truncated { .. } => "truncated_structure",
        Error::InvalidBox { .. } | Error::Overflow { .. } => "invalid_structure",
        Error::LimitExceeded { .. } => "limit",
        Error::NotIsobmff => "unsupported_container",
        Error::Unsupported { .. } => "unsupported_feature",
        Error::CodecNotAllowed { .. } => "codec_not_allowed",
        Error::NoSuchTrack | Error::Io(_) => "io",
    };
    let limit = match error {
        Error::LimitExceeded { limit, .. } => Some(format!("{limit:?}")),
        _ => None,
    };
    json!({
        "schema": schema, "file": name, "probe_ok": false, "format_duration_us": null,
        "streams": [],
        "properties": {"rejection": kind, "limit": limit, "error": error.to_string()},
    })
}

fn rational(value: Option<Rational>) -> Value {
    value.map_or(Value::Null, |r| json!([r.num, r.den]))
}

/// A packet, or a run of `length` packets that continue it: each one the
/// same size and duration, at the next position, every timestamp one
/// duration later, with the same flags (tools/oracle/SPEC.md, "Runs").
struct Run {
    pos: u64,
    size: u32,
    pts: Option<i128>,
    dts: Option<i128>,
    keyframe: bool,
    discard: bool,
    duration: u32,
    media_dts: i64,
    media_cts: i64,
    length: u64,
}

impl Run {
    fn of(packet: &coreav_isobmff::Packet) -> Self {
        Self {
            pos: packet.offset,
            size: packet.size,
            pts: packet.pts.map(|r| r.round()),
            dts: packet.presentation_dts.map(|r| r.round()),
            keyframe: packet.keyframe,
            discard: !packet.presented,
            duration: packet.duration,
            media_dts: packet.dts,
            media_cts: packet.cts,
            length: 1,
        }
    }

    /// Whether `next` is the packet that follows the last of this run.
    fn continues_with(&self, next: &Self) -> bool {
        let steps = i64::try_from(self.length).unwrap_or(i64::MAX);
        let advance = |first: i64| first.checked_add(i64::from(self.duration).checked_mul(steps)?);
        let presented = |first: i128| {
            first.checked_add(i128::from(self.duration).checked_mul(i128::from(steps))?)
        };
        let pos = u64::from(self.size)
            .checked_mul(self.length)
            .and_then(|bytes| self.pos.checked_add(bytes));
        next.length == 1
            && next.size == self.size
            && next.duration == self.duration
            && self.duration > 0
            && next.keyframe == self.keyframe
            && next.discard == self.discard
            && pos == Some(next.pos)
            && self
                .pts
                .and_then(presented)
                .is_some_and(|v| Some(v) == next.pts)
            && self
                .dts
                .and_then(presented)
                .is_some_and(|v| Some(v) == next.dts)
            && advance(self.media_dts) == Some(next.media_dts)
            && advance(self.media_cts) == Some(next.media_cts)
    }

    fn document(&self) -> Value {
        let mut packet = json!({
            "pos": self.pos,
            "pts": self.pts,
            "dts": self.dts,
            "size": self.size,
            "keyframe": self.keyframe,
            "discard": self.discard,
            "duration": self.duration,
            "media_dts": self.media_dts,
            "media_cts": self.media_cts,
        });
        if self.length > 1 {
            if let Some(object) = packet.as_object_mut() {
                object.insert("run".to_owned(), json!(self.length));
            }
        }
        packet
    }
}

fn round(value: Option<Rational>) -> Value {
    value.map_or(Value::Null, |r| json!(r.round()))
}

fn kind_name(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Subtitle => "subtitle",
        TrackKind::Timecode | TrackKind::Data => "data",
    }
}

fn codec_name(track: &Track) -> Option<String> {
    let d = &track.description;
    Some(
        match d.codec {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
            Codec::Av1 => "av1",
            Codec::Vp9 => "vp9",
            Codec::ProRes => "prores",
            Codec::Aac => "aac",
            Codec::Mp3 => "mp3",
            Codec::Opus => "opus",
            Codec::Flac => "flac",
            Codec::Alac => "alac",
            Codec::Ac3 => "ac3",
            Codec::Eac3 => "eac3",
            Codec::TimedText => "mov_text",
            Codec::Pcm => {
                let pcm = d.pcm?;
                let kind = if pcm.float {
                    "f"
                } else if pcm.signed {
                    "s"
                } else {
                    "u"
                };
                if pcm.bits <= 8 {
                    return Some(format!("pcm_{kind}8"));
                }
                let order = if pcm.little_endian { "le" } else { "be" };
                return Some(format!("pcm_{kind}{}{order}", pcm.bits));
            }
            Codec::QuickTimeText | Codec::Timecode | Codec::Other => return None,
        }
        .to_owned(),
    )
}

/// The colour description the oracle reports, from the two sources probe
/// keeps apart (`docs/design/probe-color.md`): the sample entry's `colr`
/// box (`color`, whose range falls back to the codec configuration's flag)
/// and the parameter set of the codec configuration, raw
/// (`parameter_set_color`). `ffprobe` 7.0.2, measured as a black box,
/// takes for H.264 the parameter set's colour description when its video
/// usability information carries one ("unspecified" included) and its
/// range when it signals one, else the box's (the box's also for a track
/// without frames in the file, as `ffprobe` applies the SPS only to a
/// decoded frame); for HEVC and AV1 always the
/// parameter set, with HEVC's range inferred as limited when the parameter
/// set does not signal it (ITU-T H.265 E.3.1: `video_full_range_flag`
/// inferred 0); for other codecs the box. `coreav probe --ffprobe-compat`
/// (`crates/coreav-cli/src/compat.rs`) reports the same.
fn reported_color(track: &Track) -> Color {
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
    match (codec, set) {
        (Codec::H264, Some(set)) if frames => {
            let described = set.primaries.is_some();
            let pick = |own: Option<u16>, other: Option<u16>| if described { own } else { other };
            Color {
                primaries: pick(set.primaries, boxed.primaries),
                transfer: pick(set.transfer, boxed.transfer),
                matrix: pick(set.matrix, boxed.matrix),
                full_range: set.full_range.or(boxed.full_range),
            }
        }
        (Codec::Hevc, Some(set)) => Color {
            full_range: Some(set.full_range.unwrap_or(false)),
            ..set
        },
        (Codec::Av1, Some(set)) => set,
        _ => boxed,
    }
}

/// ITU-T H.273 code points to the oracle's names.
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

/// Profile code points to the oracle's names.
fn profile_name(profile: Option<Profile>) -> Option<String> {
    let name = match profile? {
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
        Profile::ProRes(profile) => match profile {
            ProResProfile::Proxy => "Proxy",
            ProResProfile::Lt => "LT",
            ProResProfile::Standard => "Standard",
            ProResProfile::Hq => "HQ",
            ProResProfile::P4444 => "4444",
            ProResProfile::P4444Xq => "XQ",
        },
    };
    Some(name.to_owned())
}

/// The level as the oracle prints it: the stored integer. The oracle has no
/// VP9 or ProRes level.
fn level(profile: Option<Profile>) -> Option<u8> {
    match profile? {
        Profile::H264 { level_idc, .. } | Profile::Hevc { level_idc, .. } => Some(level_idc),
        Profile::Av1 { level_idx, .. } => Some(level_idx),
        Profile::Vp9 { .. } | Profile::Aac { .. } | Profile::ProRes(_) => None,
    }
}

/// A stored creation time as an ISO 8601 UTC string with microseconds, the
/// oracle's tag format; `None` when not set.
fn iso_time(stored: u64) -> Option<String> {
    let unix = unix_time(stored)?;
    let days = unix.div_euclid(86_400);
    let seconds = unix.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000000Z",
        seconds / 3_600,
        seconds % 3_600 / 60,
        seconds % 60
    ))
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

/// The size `ffprobe` reports for a 3GPP timed text stream: the track
/// header's width and height (signed 16.16 fixed point, ISO/IEC 14496-12
/// 8.3.2), whole pixels, when both whole parts are positive (measured on
/// crafted files; the `tx3g` entry's default text box plays no part).
/// Known limit: `ffprobe` also reports nothing for sizes it rejects as a
/// picture size, measured between 16,000 and 16,384 pixels a side; those
/// are reported here. A track that a `chap` track reference names, from
/// any track, gets no size: `ffprobe` lists it as a `bin_data` data stream
/// (`KD-CHAPTER-TEXT`). `crates/coreav-cli/src/compat.rs`
/// reports the same.
fn text_size(movie: &Movie, track: &Track) -> Option<(u32, u32)> {
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

fn edits(edit: EditList) -> Value {
    match edit {
        EditList::None => json!("none"),
        EditList::Simple {
            empty,
            media_time,
            segment,
        } => json!({"empty": empty, "media_time": media_time, "segment": segment}),
        EditList::Multiple => json!("multiple"),
        EditList::Unsupported => json!("unsupported"),
    }
}

fn stream(movie: &Movie, track: &Track) -> Value {
    let d = &track.description;
    let color = reported_color(track);
    let video = track.kind == TrackKind::Video;
    let audio = track.kind == TrackKind::Audio;
    let (sar, dar) = match (video, d.pixel_aspect, d.width, d.height) {
        (true, Some((h, v)), Some(w), Some(ht)) => (
            ratio(u64::from(h), u64::from(v)),
            ratio(u64::from(w) * u64::from(h), u64::from(ht) * u64::from(v)),
        ),
        _ => (Value::Null, Value::Null),
    };
    let fixed_size = track.constant_sample_size();
    let declared = d.declared_bit_rate.map(|b| {
        json!({
            "source": b.source.to_string(),
            "average": b.average,
            "maximum": b.maximum,
            "buffer_size": b.buffer_size,
        })
    });
    // The bytes stay out of the output.
    let config = d
        .codec_config
        .as_ref()
        .map(|c| json!({"kind": c.kind.to_string(), "len": c.data.len()}));
    let properties = json!({
        "track_id": track.track_id,
        "mdhd_duration_ticks": track.mdhd_duration,
        "duration_source": format!("{:?}", track.duration_source),
        "stts_duration_ticks": track.sample_duration,
        "media_edit_duration_ticks": rational(track.media_edit_duration(movie.timescale)),
        "presentation_offset_ticks": rational(track.presentation_offset(movie.timescale)),
        "edits": edits(track.edit_list),
        "edit_entries": track
            .edits
            .iter()
            .map(|e| json!([e.segment_duration, e.media_time, e.media_rate]))
            .collect::<Vec<_>>(),
        "ctts_version": track.ctts_version(),
        "ctts_signed": track.ctts_signed(),
        // Only for a stream whose samples are all in fragments: with movie
        // box samples, the first fragment's decode time is where they end.
        "fragment_start_ticks": track.fragment_start.filter(|_| track.sample_count == 0),
        "sidx_end_ticks": sidx_end(movie, track.track_id, track.timescale),
        "fixed_sample_size": fixed_size,
        "bits_per_sample": d.pcm.map(|p| p.bits),
        "profile_code_points": d.profile.map(|p| format!("{p:?}")),
        "vp9_level": match d.profile {
            Some(Profile::Vp9 { level, .. }) => Some(level),
            _ => None,
        },
        "bit_rate_exact": rational(track.bit_rate()),
        "sample_bytes": track.sample_bytes,
        "declared_bit_rate": declared,
        "codec_config": config,
        "tkhd_creation_time": track.creation_time,
        "mdhd_creation_time": track.media_creation_time,
        "warnings": track.warnings.iter().map(ToString::to_string).collect::<Vec<_>>(),
    });
    json!({
        "index": track.stream_index,
        "kind": kind_name(track.kind),
        "codec_fourcc": d.fourcc.to_string(),
        "codec_name": codec_name(track),
        "width": if video { json!(d.width) } else { json!(text_size(movie, track).map(|(w, _)| w)) },
        "height": if video { json!(d.height) } else { json!(text_size(movie, track).map(|(_, h)| h)) },
        // The oracle schema counts counter-clockwise degrees.
        "rotation": if video { json!(track.rotation.map(|cw| (360 - cw) % 360)) } else { Value::Null },
        "time_base": [1, track.timescale],
        "timescale": track.timescale,
        "duration_ts": round(track.duration),
        "color_primaries": primaries(color.primaries),
        "color_transfer": transfer(color.transfer),
        "color_matrix": matrix(color.matrix),
        "color_range": color.full_range.map(|full| if full { "pc" } else { "tv" }),
        "sample_rate": if audio { json!(d.sample_rate) } else { Value::Null },
        "channels": if audio { json!(d.channels) } else { Value::Null },
        "language": track.language.map(|l| String::from_utf8_lossy(&l).into_owned()),
        "sample_aspect_ratio": sar,
        "display_aspect_ratio": dar,
        "handler_name": track.handler_name,
        "profile": profile_name(d.profile),
        "level": level(d.profile),
        "bit_rate": round(track.bit_rate()),
        "creation_time": iso_time(track.media_creation_time),
        "properties": properties,
    })
}

fn micros(value: Option<Rational>) -> Value {
    value.map_or(Value::Null, |r| {
        let scaled = r.num.saturating_mul(1_000_000);
        json!(Rational::new(scaled, r.den).map(Rational::round))
    })
}

/// The derived presentation duration in microseconds as the reference
/// counts it: each stream's duration rounded to whole stream ticks (its
/// `duration_ts`), then its start and that duration rounded to microseconds
/// on their own, then the latest end minus the earliest start. Falls back to
/// the declared duration when no stream's timing is known.
fn format_micros(movie: &Movie) -> Value {
    match movie.ffprobe_format_duration_micros() {
        Some(us) => json!(us),
        None => micros(movie.duration_seconds()),
    }
}

fn movie_document(schema: &str, name: &str, movie: &Movie, streams: Vec<Value>) -> Value {
    json!({
        "schema": schema, "file": name, "probe_ok": true,
        // The oracle's format duration is the streams' span: CoreAV's derived
        // presentation duration (the declared movie header value is
        // `duration` in probe's own output), in the reference's microseconds.
        "format_duration_us": format_micros(movie),
        "streams": streams,
        "properties": {
            "container": "isobmff",
            "major_brand": movie.major_brand.map(|b| b.to_string()),
            "movie_timescale": movie.timescale,
            "raw_movie_duration_ticks": movie.mvhd_duration,
            "fragmented": movie.fragmented,
            "creation_time": iso_time(movie.creation_time),
            // Streams left out because their codec is not allowed.
            "dropped": movie.dropped.iter().map(|d| dropped(movie, d)).collect::<Vec<_>>(),
        },
    })
}

/// Where the first segment index (`sidx`) of track `track_id` ends: its
/// earliest presentation time plus the durations of what it references, in
/// the track's ticks; `None` without one, or when its timescale is not the
/// track's.
fn sidx_end(movie: &Movie, track_id: u32, timescale: u32) -> Option<u64> {
    let index = movie
        .segment_indexes
        .iter()
        .find(|i| i.reference_id == track_id && i.timescale == timescale)?;
    index
        .references
        .iter()
        .try_fold(index.earliest_presentation_time, |end, r| {
            end.checked_add(u64::from(r.duration))
        })
}

/// A stream the allowlist dropped: what it is, and its timing as CoreAV
/// places it (`DroppedStream::presentation`), for the overlay's format
/// duration clauses.
fn dropped(movie: &Movie, d: &DroppedStream) -> Value {
    let span = d.presentation(movie.timescale, movie.has_fragments());
    let ticks = |r: Rational| {
        Rational::new(r.num.checked_mul(i128::from(d.timescale))?, r.den).map(Rational::round)
    };
    let duration = span.and_then(|(start, end)| {
        let negated = Rational::new(start.num.checked_neg()?, start.den)?;
        ticks(end.checked_add(negated)?)
    });
    json!({
        "stream_index": d.stream_index,
        "track_id": d.track_id,
        "codec_fourcc": d.fourcc.to_string(),
        "reason": format!("{:?}", d.reason),
        "kind": kind_name(d.kind),
        "timescale": d.timescale,
        "edits": edits(d.edit_list),
        "start_ticks": span.and_then(|(start, _)| ticks(start)),
        "duration_ticks": duration,
        "fragment_start_ticks": d.fragment_start.filter(|_| d.mdhd_duration == 0),
        "sidx_end_ticks": sidx_end(movie, d.track_id, d.timescale),
    })
}

fn streams(path: &str, name: &str) -> Value {
    let schema = "coreav-oracle/streams/1";
    match open(path) {
        Ok(movie) => {
            let list = movie.tracks.iter().map(|t| stream(&movie, t)).collect();
            movie_document(schema, name, &movie, list)
        }
        Err(error) => rejection(schema, name, &error),
    }
}

fn packets(path: &str, name: &str) -> Value {
    let schema = "coreav-oracle/packets/1";
    let movie = match open(path) {
        Ok(movie) => movie,
        Err(error) => return rejection(schema, name, &error),
    };
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    let mut list = Vec::new();
    for track in &movie.tracks {
        let source = match FileSource::open(path) {
            Ok(file) => BufferedSource::new(file),
            Err(error) => return rejection(schema, name, &error),
        };
        let mut iter = match movie.packets(track.index, source, &limits, &mut budget) {
            Ok(iter) => iter,
            Err(error) => return rejection(schema, name, &error),
        };
        // H.264: the keyframe flag of the frame's coded data, as `ffprobe`
        // sets it (tools/oracle/SPEC.md, "Keyframes"); the sync sample
        // table where a frame cannot be read.
        let access = movie.access_format(track.index);
        // AV1: a key frame shown at once, read from the first frame header
        // of each sample with the latest sequence header (the `av1C`
        // configuration's, then each sample's), as `ffprobe` flags it; the
        // sync sample table where a frame cannot be read
        // (docs/design/random-access.md).
        let av1 = track.description.codec == Codec::Av1;
        let mut av1_reduced = track
            .description
            .codec_config
            .as_ref()
            .filter(|c| c.kind.0 == *b"av1C")
            .and_then(|c| av1_config_reduced(&c.data));
        // With more than one reference frame (or an unreadable count),
        // frames after an intra frame may reference frames before it.
        let single_reference = movie
            .max_num_ref_frames(track.index)
            .is_some_and(|refs| refs <= 1);
        let mut entry = stream(&movie, track);
        // Constant-size PCM samples come as runs (tools/oracle/SPEC.md,
        // "Runs"): one packet per sample would make the document grow with
        // the sample count.
        let as_runs = track.description.pcm.is_some()
            && entry
                .pointer("/properties/fixed_sample_size")
                .is_some_and(|size| !size.is_null());
        let mut packets = Vec::new();
        let mut run: Option<Run> = None;
        while let Some(packet) = iter.next() {
            let packet = match packet {
                Ok(packet) => packet,
                Err(error) => return rejection(schema, name, &error),
            };
            let mut sample = Run::of(&packet);
            if av1 {
                match iter.av1_frame_start_at(packet.offset, packet.size, &mut av1_reduced) {
                    Ok(Some(Av1FrameStart {
                        key_frame: Some(key),
                        ..
                    })) => sample.keyframe = key,
                    Ok(_) => {}
                    Err(error) => return rejection(schema, name, &error),
                }
            }
            if let Some(format) = access {
                match iter.frame_access(format, &packet) {
                    Ok(Some(frame)) => {
                        sample.keyframe = frame.idr
                            || frame.recovery_frames.is_some()
                            || (frame.intra && single_reference);
                    }
                    Ok(None) => {}
                    Err(error) => return rejection(schema, name, &error),
                }
            }
            if !as_runs {
                packets.push(sample.document());
                continue;
            }
            match run.as_mut() {
                Some(current) if current.continues_with(&sample) => current.length += 1,
                _ => {
                    if let Some(done) = run.replace(sample) {
                        packets.push(done.document());
                    }
                }
            }
        }
        if let Some(done) = run {
            packets.push(done.document());
        }
        if let Some(object) = entry.as_object_mut() {
            object.insert("packets".to_owned(), Value::Array(packets));
        }
        list.push(entry);
    }
    movie_document(schema, name, &movie, list)
}

#[cfg(test)]
#[path = "../../../crates/coreav-isobmff/tests/common/mod.rs"]
#[allow(dead_code, clippy::all, clippy::pedantic)]
mod common;

#[cfg(test)]
mod tests {
    use super::Run;
    use coreav_isobmff::{Allowlist, Limits, SliceSource, probe};

    fn sample(pos: u64, t: i64) -> Run {
        Run {
            pos,
            size: 2,
            pts: Some(i128::from(t)),
            dts: Some(i128::from(t)),
            keyframe: true,
            discard: false,
            duration: 1,
            media_dts: t,
            media_cts: t,
            length: 1,
        }
    }

    #[test]
    fn the_format_duration_counts_whole_stream_ticks() {
        // A media edit of 1 movie tick (1 ms) is 12.8 ticks at 12,800 Hz:
        // `duration_ts` 13, so 13/12,800 s = 1,015.625 µs, 1,016 µs as the
        // reference counts it, not the exact 1,000.
        let data = super::common::movie(&super::common::Spec {
            elst: Some(vec![(1, 0)]),
            ..super::common::Spec::default()
        });
        let movie = probe(
            SliceSource::new(&data),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap();
        assert_eq!(super::format_micros(&movie), serde_json::json!(1016));
    }

    #[test]
    fn runs_continue_only_with_the_next_sample() {
        let mut run = sample(100, 0);
        assert!(run.continues_with(&sample(102, 1)));
        run.length = 3;
        assert!(run.continues_with(&sample(106, 3)));
        // A gap in position or time, or another size, ends the run.
        assert!(!run.continues_with(&sample(108, 3)));
        assert!(!run.continues_with(&sample(106, 4)));
        let mut other = sample(106, 3);
        other.size = 4;
        assert!(!run.continues_with(&other));
        let mut unpresented = sample(106, 3);
        unpresented.pts = None;
        assert!(!run.continues_with(&unpresented));
        assert_eq!(run.document()["run"], 3);
        assert!(sample(0, 0).document().get("run").is_none());
    }
}
