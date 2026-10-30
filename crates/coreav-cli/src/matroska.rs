//! Matroska and WebM files (`docs/design/mkv.md`): probing with the
//! library defaults, the native schema `coreav.probe-matroska/1`
//! (`docs/schema/probe-matroska-v1.md`) and the `--ffprobe-compat` shape.

use crate::compat::{gcd, put, ratio};
use crate::error::Failure;
use crate::names;
use crate::native::{profile, rational};
use crate::time::{rounded, to_f64};
use coreav_isobmff::frames::{self, RecordSize};
use coreav_mkv::{
    Allowlist, BufferedSource, Codec, DocType, Encoding, Error, FileSource, Limits, Rational, Scan,
    Segment, Source, Track, TrackKind, TrackScan, probe,
};
use serde_json::{Map, Value, json};
use std::path::Path;

/// The schema identifier.
pub const SCHEMA: &str = "coreav.probe-matroska/1";

/// `ffprobe`'s format name for every Matroska and WebM file.
const FORMAT_NAME: &str = "matroska,webm";

/// A successful probe and what the output needs to describe it.
pub struct Probed {
    /// The probe result.
    pub segment: Segment,
    /// File size in bytes.
    pub size: u64,
    /// The block scan ([`Segment::scan_tracks`]), when it was run.
    pub scan: Scanned,
}

/// Whether and how the blocks were scanned.
pub enum Scanned {
    /// Not requested: the values only the scan gives are `null`.
    No,
    /// The scan failed on the file's structure or a limit (the message
    /// says which): the values only the scan gives are `null`.
    Failed(String),
    /// The scan's values.
    Done(Scan),
}

/// Whether a source starts like a Matroska or WebM file.
pub fn detect_source<S: Source>(source: &mut S) -> Result<bool, Failure> {
    let size = source.len().map_err(Failure::library)?;
    let mut prefix = [0u8; 4];
    if size < 4 {
        return Ok(false);
    }
    source
        .read_exact_at(0, &mut prefix)
        .map_err(Failure::library)?;
    Ok(coreav_mkv::sniff(&prefix))
}

/// Whether the file starts like a Matroska or WebM file.
pub fn detect(path: &Path) -> Result<bool, Failure> {
    use std::io::Read;
    let mut prefix = [0u8; 4];
    let mut file = std::fs::File::open(path).map_err(Failure::io)?;
    let mut read = 0;
    while read < prefix.len() {
        let slice = prefix.get_mut(read..).unwrap_or_default();
        match file.read(slice) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(Failure::io(error)),
        }
    }
    Ok(coreav_mkv::sniff(prefix.get(..read).unwrap_or_default()))
}

/// The ISO base media and Matroska allowlists of [`crate::probe::allowlists`],
/// for the commands that read those two containers only.
pub fn allowlists(
    names: Option<&[String]>,
    strict: bool,
) -> Result<(coreav_isobmff::Allowlist, Allowlist), String> {
    let (isobmff, mkv, _) = crate::probe::allowlists(names, strict)?;
    Ok((isobmff, mkv))
}

/// The Matroska codec `--allow` means by `name`.
pub fn codec_from_name(name: &str) -> Option<Codec> {
    Codec::ALL.into_iter().find(|codec| codec.name() == name)
}

/// The names `--allow` accepts for Matroska.
pub fn allow_names() -> Vec<&'static str> {
    Codec::ALL.into_iter().map(Codec::name).collect()
}

/// Probes `path` with the library defaults.
pub fn run(path: &Path, allowlist: &Allowlist) -> Result<Probed, Failure> {
    run_with(path, allowlist, false)
}

/// [`run`], and with `scan`, also lists every block for each track's
/// duration, bit rate and verified picture size
/// (`docs/design/policy-matroska.md`, decision 2).
pub fn run_with(path: &Path, allowlist: &Allowlist, scan: bool) -> Result<Probed, Failure> {
    let size = std::fs::metadata(path).map_err(Failure::io)?.len();
    let source = BufferedSource::new(FileSource::open(path).map_err(Failure::library)?);
    run_source_with(source, size, allowlist, scan)
}

/// Probes any [`Source`] of `size` bytes with the library defaults.
pub fn run_source<S: Source>(
    source: S,
    size: u64,
    allowlist: &Allowlist,
) -> Result<Probed, Failure> {
    run_source_with(source, size, allowlist, false)
}

/// [`run_with`] for any [`Source`] of `size` bytes. A scan that fails on
/// the file's structure or a limit leaves the scanned values unknown; only
/// an I/O error fails the probe.
pub fn run_source_with<S: Source>(
    mut source: S,
    size: u64,
    allowlist: &Allowlist,
    scan: bool,
) -> Result<Probed, Failure> {
    let limits = Limits::default();
    let segment = probe(&mut source, &limits, allowlist).map_err(Failure::Matroska)?;
    let scan = if scan {
        match segment.scan_tracks(&mut source, &limits) {
            Ok(scan) => Scanned::Done(scan),
            Err(Error::Io(kind)) => return Err(Failure::Matroska(Error::Io(kind))),
            Err(error) => Scanned::Failed(error.to_string()),
        }
    } else {
        Scanned::No
    };
    Ok(Probed {
        segment,
        size,
        scan,
    })
}

/// The bound of the picture sizes the parameter sets of a track's
/// `CodecPrivate` define (`avcC`, `hvcC`, `av1C`); `None` when it holds
/// none or cannot be read.
fn parameter_set_size(track: &Track) -> Option<(u32, u32)> {
    let kind = match track.codec {
        Codec::H264 => b"avcC",
        Codec::Hevc => b"hvcC",
        Codec::Av1 => b"av1C",
        _ => return None,
    };
    match frames::record_size(kind, track.codec_private.as_deref()?) {
        RecordSize::Bound(size) => Some(size),
        RecordSize::Absent | RecordSize::Unreadable => None,
    }
}

fn size(size: Option<(u32, u32)>) -> Value {
    size.map_or(
        Value::Null,
        |(width, height)| json!({"width": width, "height": height}),
    )
}

/// A time given in nanoseconds, in Segment Ticks when one tick is a whole
/// number of nanoseconds that divides a second, else in nanoseconds.
fn time(segment: &Segment, nanoseconds: i128) -> Value {
    let (ticks, timescale) = match timescale(segment) {
        Some(timescale) => (
            Rational::new(nanoseconds, segment.timestamp_scale),
            timescale,
        ),
        None => (Rational::new(nanoseconds, 1), 1_000_000_000),
    };
    json!({
        "ticks": ticks.map_or(Value::Null, rational),
        "timescale": timescale,
        "seconds": Rational::new(nanoseconds, 1_000_000_000).map(to_f64),
    })
}

/// Segment Ticks per second, when an integer.
fn timescale(segment: &Segment) -> Option<u64> {
    let scale = segment.timestamp_scale;
    (1_000_000_000 % scale == 0).then(|| 1_000_000_000 / scale)
}

/// The kind name; Matroska has no timecode tracks.
fn kind(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Subtitle => "subtitle",
        TrackKind::Data => "data",
    }
}

fn rfc3339(date_utc: Option<i64>) -> Option<String> {
    // DateUTC counts nanoseconds from 2001-01-01; `time::rfc3339` counts
    // seconds from 1904-01-01, 3,061,152,000 s earlier.
    let seconds = date_utc?
        .div_euclid(1_000_000_000)
        .checked_add(3_061_152_000)?;
    crate::time::rfc3339(u64::try_from(seconds).ok().filter(|s| *s > 0)?)
}

fn encoding(encoding: &Encoding) -> &'static str {
    match encoding {
        Encoding::None => "none",
        Encoding::HeaderStripping(_) => "header_stripping",
        Encoding::Encrypted => "encrypted",
        Encoding::Zlib => "zlib",
        Encoding::Unsupported => "unsupported",
    }
}

fn number(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn video(track: &Track, scanned_size: Option<(u32, u32)>) -> Value {
    let Some(v) = track.video.filter(|_| track.kind == TrackKind::Video) else {
        return Value::Null;
    };
    let color = track.color;
    let [yaw, pitch, roll] = v.pose;
    json!({
        "width": v.pixel_width,
        "height": v.pixel_height,
        "parameter_set_size": size(parameter_set_size(track)),
        "scanned_size": size(scanned_size),
        "display_width": v.display_width,
        "display_height": v.display_height,
        "display_unit": v.display_unit,
        "crop": {"top": v.crop[0], "bottom": v.crop[1], "left": v.crop[2], "right": v.crop[3]},
        "rotation": v.rotation(),
        "projection": {
            "type": v.projection_type, "yaw": number(yaw), "pitch": number(pitch), "roll": number(roll),
        },
        "interlaced": v.interlaced,
        "stereo_mode": v.stereo_mode,
        "alpha_mode": v.alpha_mode,
        "bits_per_channel": v.bits_per_channel,
        "color": {
            "primaries": color.primaries,
            "transfer": color.transfer,
            "matrix": color.matrix,
            "full_range": color.full_range,
        },
    })
}

fn audio(track: &Track) -> Value {
    let Some(a) = track.audio.filter(|_| track.kind == TrackKind::Audio) else {
        return Value::Null;
    };
    json!({
        "channels": a.channels,
        "sample_rate": a.sample_rate(),
        "sampling_frequency": number(a.sampling_frequency),
        "output_sampling_frequency": a.output_sampling_frequency.map(number),
        "bit_depth": a.bit_depth,
    })
}

/// Stream warnings: the library's, and a difference between `PixelWidth`
/// and `PixelHeight` and the size the parameter sets or the scanned frames
/// give (as `docs/design/frame-sizes.md`, decision 5).
fn warnings(track: &Track, scanned_size: Option<(u32, u32)>) -> Vec<String> {
    let mut out: Vec<String> = track.warnings.iter().map(ToString::to_string).collect();
    let declared = track
        .video
        .filter(|_| track.kind == TrackKind::Video)
        .and_then(|v| v.pixel_width.zip(v.pixel_height));
    if let Some(declared) = declared {
        for (what, verified) in [
            ("parameter sets give", parameter_set_size(track)),
            ("the scanned frames give", scanned_size),
        ] {
            let verified = verified.map(|(w, h)| (u64::from(w), u64::from(h)));
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

/// One stream; `scanned` says whether the block scan ran and succeeded
/// (`None` when it did not run), `scan` is the track's result.
fn stream(
    segment: &Segment,
    track: &Track,
    scanned: Option<bool>,
    scan: Option<&TrackScan>,
) -> Value {
    // Whether every frame of the track was listed: false for a track whose
    // frames cannot be listed, and for every track when the scan failed.
    let frames_listed = scanned.map(|ok| ok && scan.is_some_and(|s| s.listed));
    let duration = scan.and_then(TrackScan::duration_ns);
    let bit_rate = scan
        .zip(duration.filter(|ns| *ns > 0))
        .and_then(|(scan, ns)| {
            let bits = i128::from(scan.bytes).checked_mul(8_000_000_000)?;
            Rational::new(bits, u64::try_from(ns).ok()?)
        })
        .map_or(
            Value::Null,
            |rate| json!({"exact": rational(rate), "bits_per_second": to_f64(rate)}),
        );
    let scanned_size = scan.and_then(|s| s.frame_size);
    json!({
        "index": track.index,
        "stream_index": track.stream_index,
        "track_number": track.number,
        "track_uid": track.uid,
        "kind": kind(track.kind),
        "codec": track.codec.name(),
        "codec_id": track.codec_id,
        "codec_private_size": track.codec_private.as_ref().map(Vec::len),
        "name": track.name,
        "language": track.language,
        "enabled": track.enabled,
        "default": track.default,
        "forced": track.forced,
        "lacing": track.lacing,
        "default_duration_ns": track.default_duration,
        "codec_delay_ns": track.codec_delay,
        "seek_pre_roll_ns": track.seek_pre_roll,
        "encoding": encoding(&track.encoding),
        "packets_available": track.packets_available(),
        "frames_listed": frames_listed,
        "duration": duration.map_or(Value::Null, |ns| time(segment, ns)),
        "frame_count": scan.filter(|s| s.listed).map(|s| s.frames),
        "bit_rate": bit_rate,
        "profile": profile(track.profile),
        "frame_profile": track.frame_profile,
        "fourcc": track.fourcc.map(|f| String::from_utf8_lossy(&f).into_owned()),
        "video": video(track, scanned_size),
        "audio": audio(track),
        "warnings": warnings(track, scanned_size),
    })
}

/// The native document, schema `coreav.probe-matroska/1`.
pub fn document(probed: &Probed) -> Value {
    let segment = &probed.segment;
    let scan = match &probed.scan {
        Scanned::Done(scan) => Some(scan),
        Scanned::No | Scanned::Failed(_) => None,
    };
    let mut warnings: Vec<String> = segment.warnings.iter().map(ToString::to_string).collect();
    if let Scanned::Failed(error) = &probed.scan {
        warnings.push(format!(
            "the block scan failed ({error}); every stream's frames_listed is false, and its \
             duration, bit rate and scanned size are null"
        ));
    }
    let scanned = match &probed.scan {
        Scanned::No => None,
        Scanned::Done(_) => Some(true),
        Scanned::Failed(_) => Some(false),
    };
    let tick = Rational::new(i128::from(segment.timestamp_scale), 1_000_000_000);
    json!({
        "schema": SCHEMA,
        "file": {"size": probed.size},
        "container": {
            "doc_type": match segment.doc_type {
                DocType::Matroska => "matroska",
                DocType::Webm => "webm",
            },
            "doc_type_version": segment.doc_type_version,
            "doc_type_read_version": segment.doc_type_read_version,
            "timestamp_scale": segment.timestamp_scale,
            "tick": tick.map_or(Value::Null, rational),
            "live": segment.size.is_none(),
            "cues": segment.cues.is_some(),
            "chapters": segment.has_chapters,
            "tags": segment.has_tags,
            "title": segment.title,
            "muxing_app": segment.muxing_app,
            "writing_app": segment.writing_app,
            "date_utc": rfc3339(segment.date_utc),
            "attachments": segment.attachments.iter().map(|a| json!({
                "name": a.name, "media_type": a.media_type, "size": a.size,
            })).collect::<Vec<_>>(),
        },
        "duration": segment.duration_ns().map_or(Value::Null, |ns| time(segment, ns)),
        "streams": segment
            .tracks
            .iter()
            .enumerate()
            .map(|(i, track)| {
                stream(segment, track, scanned, scan.and_then(|s| s.tracks.get(i)))
            })
            .collect::<Vec<_>>(),
        "dropped_streams": segment.dropped.iter().map(|d| json!({
            "stream_index": d.stream_index,
            "track_number": d.number,
            "codec_id": d.codec_id,
            "codec": d.codec.name(),
            "reason": "codec_not_allowed",
        })).collect::<Vec<_>>(),
        "warnings": warnings,
    })
}

/// `ffprobe`'s `codec_name`.
fn ffprobe_codec(track: &Track) -> Option<String> {
    let name = match track.codec {
        Codec::Pgs => "hdmv_pgs_subtitle",
        Codec::VobSub => "dvd_subtitle",
        Codec::DvbSub => "dvb_subtitle",
        Codec::Other => return None,
        Codec::Pcm => {
            let bits = track.audio?.bit_depth?;
            return Some(match track.codec_id.as_str() {
                "A_PCM/INT/LIT" if bits == 8 => "pcm_u8".to_owned(),
                "A_PCM/INT/LIT" => format!("pcm_s{bits}le"),
                "A_PCM/INT/BIG" if bits == 8 => "pcm_s8".to_owned(),
                "A_PCM/INT/BIG" => format!("pcm_s{bits}be"),
                "A_PCM/FLOAT/IEEE" => format!("pcm_f{bits}le"),
                _ => return None,
            });
        }
        other => other.name(),
    };
    Some(name.to_owned())
}

fn compat_stream(segment: &Segment, track: &Track) -> Value {
    let mut s = Map::new();
    s.insert("index".to_owned(), json!(track.stream_index));
    put(&mut s, "codec_name", ffprobe_codec(track).map(Value::from));
    let profile = track.profile.and_then(names::profile).or_else(|| {
        // `ffprobe` names a VP8 profile by its `version` number.
        (track.codec == Codec::Vp8)
            .then(|| track.frame_profile.map(|v| v.to_string()))
            .flatten()
    });
    put(&mut s, "profile", profile.map(Value::from));
    s.insert("codec_type".to_owned(), json!(kind(track.kind)));
    s.insert("codec_tag_string".to_owned(), json!("[0][0][0][0]"));
    s.insert("codec_tag".to_owned(), json!("0x0000"));
    if let Some(v) = track.video.filter(|_| track.kind == TrackKind::Video) {
        put(&mut s, "width", v.pixel_width.map(Value::from));
        put(&mut s, "height", v.pixel_height.map(Value::from));
        if let (Some(w), Some(h)) = (v.pixel_width, v.pixel_height) {
            let shown = (
                w.saturating_sub(v.crop[2]).saturating_sub(v.crop[3]),
                h.saturating_sub(v.crop[0]).saturating_sub(v.crop[1]),
            );
            let (dw, dh) = v.display_aspect().unwrap_or(shown);
            put(
                &mut s,
                "sample_aspect_ratio",
                ratio(dw.saturating_mul(h), dh.saturating_mul(w)).map(Value::from),
            );
            put(
                &mut s,
                "display_aspect_ratio",
                ratio(dw, dh).map(Value::from),
            );
        }
        let color = track.color;
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
    }
    put(
        &mut s,
        "level",
        track.profile.and_then(names::level).map(Value::from),
    );
    if let Some(a) = track.audio.filter(|_| track.kind == TrackKind::Audio) {
        // The output rate, as a decoder reports it: an SBR track stores its
        // core rate as `SamplingFrequency`.
        put(
            &mut s,
            "sample_rate",
            a.output_sample_rate().map(|r| Value::from(r.to_string())),
        );
        s.insert("channels".to_owned(), json!(a.channels));
    }
    let g = gcd(segment.timestamp_scale, 1_000_000_000);
    s.insert(
        "time_base".to_owned(),
        json!(format!(
            "{}/{}",
            segment.timestamp_scale / g,
            1_000_000_000 / g
        )),
    );
    // The stored `Language` element (ISO 639-2), as ffprobe's tag: not the
    // `LanguageBCP47` that overrides it, nor the default `eng` of a track
    // that stores none (RFC 9559 5.1.4.1.19), which the native document
    // reports.
    if let Some(language) = track
        .language_iso639
        .as_deref()
        .filter(|l| !l.is_empty() && *l != "und")
    {
        s.insert("tags".to_owned(), json!({ "language": language }));
    }
    if let Some(clockwise) = track
        .video
        .filter(|_| track.kind == TrackKind::Video)
        .and_then(|v| v.rotation())
        .filter(|r| *r != 0)
    {
        let ccw = (360 - i32::from(clockwise % 360)) % 360;
        let signed = if ccw >= 180 { ccw - 360 } else { ccw };
        s.insert(
            "side_data_list".to_owned(),
            json!([{"side_data_type": "Display Matrix", "rotation": signed}]),
        );
    }
    Value::Object(s)
}

/// The `--ffprobe-compat` document.
pub fn compat_document(probed: &Probed, filename: &str) -> Value {
    let segment = &probed.segment;
    let mut format = Map::new();
    format.insert("filename".to_owned(), json!(filename));
    // `ffprobe` lists attachments as streams too.
    let streams = segment.tracks.len() + segment.dropped.len() + segment.attachments.len();
    format.insert("nb_streams".to_owned(), json!(streams));
    format.insert("format_name".to_owned(), json!(FORMAT_NAME));
    let micros = segment
        .duration_ns()
        .and_then(|ns| Rational::new(ns, 1_000))
        .and_then(rounded)
        .filter(|us| *us > 0);
    if let Some(us) = micros {
        format.insert(
            "duration".to_owned(),
            json!(format!("{}.{:06}", us / 1_000_000, us % 1_000_000)),
        );
    }
    format.insert("size".to_owned(), json!(probed.size.to_string()));
    if let Some(us) = micros.and_then(|us| u128::try_from(us).ok()) {
        if let Some(rate) = u128::from(probed.size)
            .checked_mul(8_000_000)
            .and_then(|bits| bits.checked_div(us))
            .and_then(|rate| u64::try_from(rate).ok())
        {
            format.insert("bit_rate".to_owned(), json!(rate.to_string()));
        }
    }
    let streams: Vec<Value> = segment
        .tracks
        .iter()
        .map(|t| compat_stream(segment, t))
        .collect();
    json!({"streams": streams, "format": format})
}
