//! MPEG transport streams (`docs/design/mpegts.md`): probing with the
//! library defaults, the native schema `coreav.probe-mpegts/1`
//! (`docs/schema/probe-mpegts-v1.md`) and the `--ffprobe-compat` shape.
//!
//! A transport stream has no header to recognise, so the command offers a
//! file here only once the Matroska and ISO base media readers have
//! declined it; the reader then finds the packet size and the first
//! packet itself ([`coreav_mpegts::probe`]), which a sniff of the first
//! bytes alone would not (a stream may start after a prefix of other
//! bytes).

use crate::compat::put;
use crate::error::Failure;
use crate::names;
use crate::native::{profile, rational};
use crate::time::to_f64;
use coreav_isobmff::Rational;
use coreav_mpegts::{
    Allowlist, BufferedSource, Codec, Descriptor, Error, FileSource, Limits, PacketSize, Source,
    Stream, StreamKind, TransportStream, Video, arib_caption_profile, probe,
};
use serde_json::{Map, Value, json};
use std::path::Path;

/// The schema identifier.
pub const SCHEMA: &str = "coreav.probe-mpegts/1";

/// `ffprobe`'s format name for every transport stream.
const FORMAT_NAME: &str = "mpegts";

/// Transport stream timestamps are 90 kHz ticks (ISO/IEC 13818-1 2.4.3.7),
/// and so is `ffprobe`'s time base for them.
const TIMESCALE: u32 = 90_000;

/// A successful probe and what the output needs to describe it.
pub struct Probed {
    /// The probe result.
    pub stream: TransportStream,
    /// File size in bytes.
    pub size: u64,
    /// The packet listing ([`TransportStream::packets`]), when it was run.
    pub scan: Scanned,
}

/// Whether and how the access units were listed.
pub enum Scanned {
    /// Not requested: every stream's `scan` is `null`.
    No,
    /// The listing ran: its per-stream counts, the problems it went past
    /// and, when it stopped early, why.
    Ran(Listing),
}

/// What the packet listing produced.
pub struct Listing {
    /// Per stream, in the order the probe lists them.
    pub streams: Vec<StreamScan>,
    /// Problems the listing went past ([`coreav_mpegts::Packets::warnings`]).
    pub warnings: Vec<String>,
    /// Why the listing stopped before the end of the file, if it did.
    pub stopped: Option<String>,
}

impl Scanned {
    /// The per-stream results, if the listing ran.
    fn results(&self) -> Option<&[StreamScan]> {
        match self {
            Self::No => None,
            Self::Ran(listing) => Some(&listing.streams),
        }
    }
}

/// What the listing found in one stream. These are the listing's own
/// counts, not a second duration estimate: a stream's `duration` stays the
/// one the probe reports, which is `ffprobe`'s
/// (`docs/design/mpegts.md`, "Durations").
#[derive(Clone, Default)]
pub struct StreamScan {
    /// Access units listed.
    pub packets: u64,
    /// Their bytes together.
    pub bytes: u64,
    /// The earliest presentation timestamp (90 kHz).
    pub first_pts: Option<i64>,
    /// The latest presentation timestamp (90 kHz).
    pub last_pts: Option<i64>,
}

impl StreamScan {
    /// From the first presentation time to the last (90 kHz); `None` while
    /// fewer than two timestamps were seen. It is one access unit short of
    /// the stream's duration: it ends where the last unit starts.
    pub fn span(&self) -> Option<i64> {
        Some(self.last_pts? - self.first_pts?).filter(|d| *d > 0)
    }
}

/// Whether `source` holds stored transport packets, which is what the
/// reader accepts ([`coreav_mpegts::detect`]). The command asks only once
/// the other readers have declined the file, so a file that is no
/// container at all keeps their answer rather than this reader's.
pub fn detect_source<S: Source>(source: &mut S) -> Result<bool, Failure> {
    match coreav_mpegts::detect(source, &Limits::default()) {
        Ok(_) => Ok(true),
        Err(Error::Io(kind)) => Err(Failure::Mpegts(Error::Io(kind))),
        Err(_) => Ok(false),
    }
}

/// Whether the file holds stored transport packets.
pub fn detect(path: &Path) -> Result<bool, Failure> {
    let file = FileSource::open(path).map_err(|e| Failure::Mpegts(e.into()))?;
    detect_source(&mut BufferedSource::new(file))
}

/// The names of other containers that mean a transport stream codec, so
/// that one `--allow` list serves every container: `mp1`, `mp2` and `mp3`
/// are MPEG audio, whose layer only its frames show, and `dvbsub` is what
/// Matroska calls DVB subtitles.
const ALIASES: [(&str, Codec); 4] = [
    ("mp1", Codec::MpegAudio),
    ("mp2", Codec::MpegAudio),
    ("mp3", Codec::MpegAudio),
    ("dvbsub", Codec::DvbSubtitle),
];

/// The transport stream codec `--allow` means by `name`.
pub fn codec_from_name(name: &str) -> Option<Codec> {
    Codec::ALL
        .into_iter()
        .find(|codec| codec.name() == name)
        .or_else(|| {
            ALIASES
                .into_iter()
                .find_map(|(alias, codec)| (alias == name).then_some(codec))
        })
}

/// The names `--allow` accepts for transport streams.
pub fn allow_names() -> Vec<&'static str> {
    Codec::ALL.into_iter().map(Codec::name).collect()
}

/// Probes `path` with the library defaults; with `scan`, also lists every
/// access unit for each stream's exact duration, packet count and bit rate.
pub fn run_with(path: &Path, allowlist: &Allowlist, scan: bool) -> Result<Probed, Failure> {
    let size = std::fs::metadata(path).map_err(Failure::io)?.len();
    let source =
        BufferedSource::new(FileSource::open(path).map_err(|e| Failure::Mpegts(e.into()))?);
    run_source_with(source, size, allowlist, scan)
}

/// [`run_with`] for any [`Source`] of `size` bytes. A listing that stops on
/// the file's structure or a limit keeps what it read; only an I/O error
/// fails the probe.
pub fn run_source_with<S: Source>(
    mut source: S,
    size: u64,
    allowlist: &Allowlist,
    scan: bool,
) -> Result<Probed, Failure> {
    let limits = Limits::default();
    let stream = probe(&mut source, &limits, allowlist).map_err(Failure::Mpegts)?;
    let scan = if scan {
        Scanned::Ran(list(&stream, &mut source, &limits)?)
    } else {
        Scanned::No
    };
    Ok(Probed { stream, size, scan })
}

/// Lists every access unit, adding each stream's up. The listing reads the
/// whole file, so it sees the damage the probe's head and tail cannot: its
/// warnings join the document's.
fn list<S: Source>(
    stream: &TransportStream,
    source: &mut S,
    limits: &Limits,
) -> Result<Listing, Failure> {
    let mut streams: Vec<StreamScan> = vec![StreamScan::default(); stream.streams.len()];
    let mut packets = stream.packets(&mut *source, limits);
    let mut stopped = None;
    for packet in packets.by_ref() {
        let packet = match packet {
            Ok(packet) => packet,
            Err(Error::Io(kind)) => return Err(Failure::Mpegts(Error::Io(kind))),
            Err(error) => {
                stopped = Some(error.to_string());
                break;
            }
        };
        // A stream a later table version added is counted after the
        // probe's, as `Packets::streams` lists it.
        if streams.len() <= packet.stream {
            streams.resize(packet.stream.saturating_add(1), StreamScan::default());
        }
        let Some(result) = streams.get_mut(packet.stream) else {
            continue;
        };
        result.packets = result.packets.saturating_add(1);
        result.bytes = result
            .bytes
            .saturating_add(u64::try_from(packet.data.len()).unwrap_or(0));
        if let Some(pts) = packet.pts {
            if result.first_pts.is_none_or(|first| pts < first) {
                result.first_pts = Some(pts);
            }
            if result.last_pts.is_none_or(|last| pts > last) {
                result.last_pts = Some(pts);
            }
        }
    }
    let warnings = packets.warnings().iter().map(ToString::to_string).collect();
    Ok(Listing {
        streams,
        warnings,
        stopped,
    })
}

/// A time: exact 90 kHz ticks, their timescale, and seconds as a
/// convenience copy.
fn time(ticks: i64) -> Value {
    json!({
        "ticks": ticks,
        "timescale": TIMESCALE,
        "seconds": Rational::new(i128::from(ticks), u64::from(TIMESCALE)).map(to_f64),
    })
}

pub(crate) fn kind(kind: StreamKind) -> &'static str {
    match kind {
        StreamKind::Video => "video",
        StreamKind::Audio => "audio",
        StreamKind::Subtitle => "subtitle",
        StreamKind::Data => "data",
    }
}

fn descriptors(list: &[Descriptor]) -> Vec<Value> {
    list.iter()
        .map(|d| json!({"tag": d.tag, "size": d.data.len()}))
        .collect()
}

/// The MPEG-2 video profile and level `profile_and_level_indication` names
/// (ISO/IEC 13818-2 table 8-1).
fn mpeg2_profile(stream: &Stream) -> Value {
    let Some((profile, level)) = stream.mpeg2_profile else {
        return Value::Null;
    };
    json!({"name": mpeg2_profile_name(profile), "profile": profile, "level": level})
}

/// `ffprobe`'s name for an MPEG-2 video profile.
fn mpeg2_profile_name(profile: u8) -> Option<&'static str> {
    Some(match profile {
        5 => "Simple",
        4 => "Main",
        3 => "SNR Scalable",
        2 => "Spatially Scalable",
        1 => "High",
        _ => return None,
    })
}

fn video(stream: &Stream) -> Value {
    // A presentation graphics subtitle states the picture it is composed
    // on, which is a size of its own, not the video stream's.
    let Some(v) = stream
        .video
        .filter(|_| stream.kind == StreamKind::Video || stream.codec == Codec::PgsSubtitle)
    else {
        return Value::Null;
    };
    let color = stream.color;
    json!({
        "width": v.width,
        "height": v.height,
        "color": {
            "primaries": color.primaries,
            "transfer": color.transfer,
            "matrix": color.matrix,
            "full_range": color.full_range,
        },
    })
}

fn audio(stream: &Stream) -> Value {
    let Some(a) = stream.audio.filter(|_| stream.kind == StreamKind::Audio) else {
        return Value::Null;
    };
    json!({
        "sample_rate": a.sample_rate,
        // 0 means the frame header leaves the channels to the payload.
        "channels": Some(a.channels).filter(|c| *c > 0),
        "detail": a.detail,
    })
}

/// The `scan` object of one stream: `null` when the listing did not run.
/// `duration` is the stream's, which the probe gives.
fn scan(result: Option<&StreamScan>, duration: Option<i64>) -> Value {
    let Some(result) = result else {
        return Value::Null;
    };
    let span = result.span();
    // The bytes over the stream's duration, which covers every unit; the
    // span ends where the last unit starts, so dividing by it would
    // overstate the rate by one unit's worth.
    let bit_rate = duration
        .filter(|ticks| *ticks > 0)
        .and_then(|ticks| {
            let bits = i128::from(result.bytes).checked_mul(8 * i128::from(TIMESCALE))?;
            Rational::new(bits, u64::try_from(ticks).ok()?)
        })
        .map_or(
            Value::Null,
            |rate| json!({"exact": rational(rate), "bits_per_second": to_f64(rate)}),
        );
    json!({
        "packets": result.packets,
        "bytes": result.bytes,
        "first_pts": result.first_pts.map_or(Value::Null, time),
        "last_pts": result.last_pts.map_or(Value::Null, time),
        "span": span.map_or(Value::Null, time),
        "bit_rate": bit_rate,
    })
}

fn stream(s: &Stream, index: usize, result: Option<&StreamScan>) -> Value {
    json!({
        "index": index,
        "stream_index": s.stream_index,
        "pid": s.pid,
        "programs": s.programs,
        "pmt_index": s.pmt_index,
        "stream_type": s.stream_type,
        "codec": s.codec.name(),
        "kind": kind(s.kind),
        "language": s.language,
        "descriptors": descriptors(&s.descriptors),
        "format_identifier": s.format_identifier.map(codec_tag_string),
        "config_size": s.config.as_ref().map(Vec::len),
        "profile": profile(s.profile),
        "mpeg2_profile": mpeg2_profile(s),
        "start": s.start.map_or(Value::Null, time),
        "duration": s.duration().map_or(Value::Null, time),
        "scan": scan(result, s.duration()),
        "video": video(s),
        "audio": audio(s),
    })
}

/// The native document, schema `coreav.probe-mpegts/1`.
pub fn document(probed: &Probed) -> Value {
    let ts = &probed.stream;
    let results = probed.scan.results();
    let mut warnings: Vec<String> = ts.warnings.iter().map(ToString::to_string).collect();
    if let Scanned::Ran(listing) = &probed.scan {
        // The listing reads the whole file; the probe reads its head and
        // its tail, so these are problems only the listing can see.
        for warning in &listing.warnings {
            if !warnings.contains(warning) {
                warnings.push(warning.clone());
            }
        }
        if let Some(reason) = &listing.stopped {
            warnings.push(format!(
                "the packet listing stopped ({reason}); each stream's scan counts the access \
                 units read before it"
            ));
        }
    }
    json!({
        "schema": SCHEMA,
        "file": {"size": probed.size},
        "container": {
            "packet_size": packet_size(ts.packet_size),
            "first_packet": ts.first_packet,
            "timescale": TIMESCALE,
            "scanned": !matches!(probed.scan, Scanned::No),
            "programs": ts.programs.iter().map(|p| json!({
                "number": p.number,
                "pmt_pid": p.pmt_pid,
                "pcr_pid": p.pcr_pid,
                "stream_pids": p.pids,
            })).collect::<Vec<_>>(),
        },
        "start": ts.start().map_or(Value::Null, time),
        "duration": ts.duration().map_or(Value::Null, time),
        "streams": ts
            .streams
            .iter()
            .enumerate()
            .map(|(i, s)| stream(s, i, results.and_then(|r| r.get(i))))
            .collect::<Vec<_>>(),
        "dropped_streams": ts.dropped.iter().map(|d| json!({
            "stream_index": d.stream_index,
            "pid": d.pid,
            "stream_type": d.stream_type,
            "codec": d.codec.name(),
            "reason": "codec_not_allowed",
        })).collect::<Vec<_>>(),
        "warnings": warnings,
    })
}

/// Bytes a stored packet takes.
fn packet_size(size: PacketSize) -> u16 {
    match size {
        PacketSize::Ts188 => 188,
        PacketSize::Ts192 => 192,
        PacketSize::Ts204 => 204,
    }
}

/// `ffprobe`'s `codec_name`.
pub(crate) fn ffprobe_codec(s: &Stream) -> Option<&'static str> {
    Some(match s.codec {
        Codec::H264 => "h264",
        Codec::Hevc => "hevc",
        Codec::Vvc => "vvc",
        Codec::Mpeg1Video => "mpeg1video",
        Codec::Mpeg2Video => "mpeg2video",
        Codec::Mpeg4 => "mpeg4",
        Codec::Aac => "aac",
        Codec::AacLatm => "aac_latm",
        // The layer is in the frames, which the probe reads.
        Codec::MpegAudio => match s.audio.map(|a| a.detail) {
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
        Codec::Other if s.stream_type == 0x06 => "bin_data",
        Codec::Other => return None,
    })
}

/// The four bytes `ffprobe`'s `codec_tag` holds: the identifier the
/// stream's descriptors give its format, else the stream type.
pub(crate) fn tag_bytes(s: &Stream) -> [u8; 4] {
    s.format_identifier.unwrap_or([s.stream_type, 0, 0, 0])
}

/// `ffprobe`'s `codec_tag_string`: the four bytes as a little-endian
/// four-character code, each unprintable byte as `[n]`
/// (`av_fourcc_make_string`).
pub(crate) fn codec_tag_string(bytes: [u8; 4]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b.is_ascii_alphanumeric() || b" ._-".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("[{b}]")
            }
        })
        .collect()
}

/// `ffprobe`'s `codec_tag`: the same four bytes as a little-endian number,
/// at least four hexadecimal digits.
pub(crate) fn codec_tag(bytes: [u8; 4]) -> String {
    format!("0x{:04x}", u32::from_le_bytes(bytes))
}

/// `ffprobe`'s profile name, MPEG-2 video's included.
pub(crate) fn profile_name(s: &Stream) -> Option<String> {
    if s.codec == Codec::AribCaption {
        return arib_caption_profile(&s.descriptors).map(ToOwned::to_owned);
    }
    if s.mpeg2_profile.is_some() {
        return mpeg2_profile(s)
            .get("name")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
    }
    s.profile.and_then(names::profile)
}

/// `ffprobe`'s `level`: the stored integer, MPEG-2 video's included.
pub(crate) fn level(s: &Stream) -> Option<u8> {
    if let Some((_, level)) = s.mpeg2_profile {
        return Some(level);
    }
    s.profile.and_then(names::level)
}

fn compat_stream(s: &Stream) -> Value {
    let mut out = Map::new();
    out.insert("index".to_owned(), json!(s.stream_index));
    // `ffprobe` prints the PID as the stream's id, which names the same
    // stream whatever order the two readers list the streams in.
    out.insert("id".to_owned(), json!(format!("0x{:x}", s.pid)));
    put(&mut out, "codec_name", ffprobe_codec(s).map(Value::from));
    put(&mut out, "profile", profile_name(s).map(Value::from));
    // `ffprobe` leaves the media type of a stream whose type names no
    // codec it knows unknown, and prints no `codec_type` for it; a private
    // stream (type 0x06) it reads as binary data is `data`.
    if s.codec != Codec::Other || s.stream_type == 0x06 {
        out.insert("codec_type".to_owned(), json!(kind(s.kind)));
    }
    let bytes = tag_bytes(s);
    out.insert(
        "codec_tag_string".to_owned(),
        json!(codec_tag_string(bytes)),
    );
    out.insert("codec_tag".to_owned(), json!(codec_tag(bytes)));
    if s.kind == StreamKind::Video || s.video.is_some() {
        // `ffprobe` prints a size for every video stream, 0 when it has
        // not learnt one (a stream whose parameter sets it never saw, an
        // MVC sub-bitstream or a scrambled one); and for a presentation
        // graphics subtitle the picture its composition segment states.
        let v = s.video.unwrap_or(Video {
            width: 0,
            height: 0,
        });
        out.insert("width".to_owned(), json!(v.width));
        out.insert("height".to_owned(), json!(v.height));
        let color = s.color;
        put(
            &mut out,
            "color_range",
            color
                .full_range
                .map(|full| Value::from(if full { "pc" } else { "tv" })),
        );
        put(
            &mut out,
            "color_space",
            color.matrix.and_then(names::matrix).map(Value::from),
        );
        put(
            &mut out,
            "color_transfer",
            color.transfer.and_then(names::transfer).map(Value::from),
        );
        put(
            &mut out,
            "color_primaries",
            color.primaries.and_then(names::primaries).map(Value::from),
        );
    }
    put(&mut out, "level", level(s).map(Value::from));
    if let Some(a) = s.audio.filter(|_| s.kind == StreamKind::Audio) {
        out.insert("sample_rate".to_owned(), json!(a.sample_rate.to_string()));
        if a.channels > 0 {
            out.insert("channels".to_owned(), json!(a.channels));
        }
    }
    out.insert("time_base".to_owned(), json!(format!("1/{TIMESCALE}")));
    let duration_ts = s.duration();
    put(&mut out, "duration_ts", duration_ts.map(Value::from));
    put(
        &mut out,
        "duration",
        duration_ts.map(|ticks| Value::from(seconds_string(ticks))),
    );
    if let Some(language) = s
        .language
        .as_deref()
        .filter(|l| !l.is_empty() && *l != "und")
    {
        out.insert("tags".to_owned(), json!({ "language": language }));
    }
    Value::Object(out)
}

/// 90 kHz ticks as `ffprobe` prints a duration: seconds with six decimals.
fn seconds_string(ticks: i64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let seconds = ticks as f64 * (1.0 / f64::from(TIMESCALE));
    format!("{seconds:.6}")
}

/// The `--ffprobe-compat` document.
pub fn compat_document(probed: &Probed, filename: &str) -> Value {
    let ts = &probed.stream;
    let mut format = Map::new();
    format.insert("filename".to_owned(), json!(filename));
    format.insert(
        "nb_streams".to_owned(),
        json!(ts.streams.len().saturating_add(ts.dropped.len())),
    );
    format.insert("format_name".to_owned(), json!(FORMAT_NAME));
    // Whole 90 kHz ticks, then microseconds, as the oracle adapter counts
    // them: half a tick up, so that a duration printed by both sides rounds
    // the same way.
    let micros = ts.duration().and_then(|ticks| {
        i64::try_from(
            (i128::from(ticks) * 1_000_000 + i128::from(TIMESCALE) / 2)
                .div_euclid(i128::from(TIMESCALE)),
        )
        .ok()
        .filter(|us| *us > 0)
    });
    if let Some(us) = micros {
        format.insert(
            "duration".to_owned(),
            json!(format!("{}.{:06}", us / 1_000_000, us % 1_000_000)),
        );
    }
    format.insert("size".to_owned(), json!(probed.size.to_string()));
    if let Some(rate) = micros
        .and_then(|us| u128::try_from(us).ok())
        .and_then(|us| {
            u128::from(probed.size)
                .checked_mul(8_000_000)
                .and_then(|bits| bits.checked_div(us))
        })
        .and_then(|rate| u64::try_from(rate).ok())
    {
        format.insert("bit_rate".to_owned(), json!(rate.to_string()));
    }
    let streams: Vec<Value> = ts.streams.iter().map(compat_stream).collect();
    json!({"streams": streams, "format": format})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_tags_follow_ffprobe() {
        assert_eq!(codec_tag_string(*b"HEVC"), "HEVC");
        assert_eq!(codec_tag(*b"HEVC"), "0x43564548");
        assert_eq!(codec_tag_string([0x1B, 0, 0, 0]), "[27][0][0][0]");
        assert_eq!(codec_tag([0x1B, 0, 0, 0]), "0x001b");
        assert_eq!(codec_tag_string(*b"AC-3"), "AC-3");
    }

    #[test]
    fn every_allow_name_is_a_codec() {
        for name in allow_names() {
            assert_eq!(codec_from_name(name).map(Codec::name), Some(name));
        }
        assert_eq!(codec_from_name("other"), None);
        // The other containers' names for the same codecs.
        assert_eq!(codec_from_name("mp3"), Some(Codec::MpegAudio));
        assert_eq!(codec_from_name("mp2"), Some(Codec::MpegAudio));
        assert_eq!(codec_from_name("dvbsub"), Some(Codec::DvbSubtitle));
    }

    #[test]
    fn a_scan_spans_the_presentation_times_it_saw() {
        let result = StreamScan {
            packets: 3,
            bytes: 30,
            first_pts: Some(900),
            last_pts: Some(8_100),
        };
        assert_eq!(result.span(), Some(7_200));
        // One timestamp, or none, spans nothing.
        assert_eq!(
            StreamScan {
                first_pts: Some(900),
                last_pts: Some(900),
                ..StreamScan::default()
            }
            .span(),
            None
        );
        assert_eq!(StreamScan::default().span(), None);
    }
}
