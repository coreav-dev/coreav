//! `coreav ffprobe`: `ffprobe`'s command line over CoreAV's own probes
//! (`docs/design/compat-cli.md`, "The command").
//!
//! [`args`] reads the command line and refuses what is outside the
//! vocabulary. Each container's builder ([`iso`], [`mkv`], [`ts`]) maps
//! its native probe result to a [`Report`]: the stream, format, chapter
//! and program sections, with their entries in `ffprobe`'s order, and the
//! packets, which are read and written one at a time. [`frames`] decodes
//! the frames of those packets. [`emit`] prints the report through the
//! writer the command chose.

pub mod args;
mod frames;
mod iso;
mod iso_meta;
mod metadata;
mod mkv;
pub mod model;
mod names;
mod ts;
pub(crate) mod video;
pub mod writer;

use crate::error::Failure;
use args::{Interval, IntervalEnd, Kind2, Options, StreamSpec};
use coreav_isobmff::Source;
use model::{Selection, Sid, Value};
use std::io::Write;
use writer::Writer;

pub use frames::Setup;

/// Keys `ffprobe` prints that CoreAV leaves unknown, and why: named in
/// `-show_entries`, they are refused.
pub const UNKNOWN_KEYS: [(&str, &str, &str); 0] = [];

/// Why a key named in `-show_entries` is refused, if it is.
pub fn undecided(section: &str, key: &str) -> Option<&'static str> {
    UNKNOWN_KEYS
        .iter()
        .find(|(s, k, _)| *s == section && *k == key)
        .map(|(_, _, why)| *why)
}

/// What a stream is, for `-select_streams`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    /// `codec_type` video.
    Video,
    /// Audio.
    Audio,
    /// Subtitles.
    Subtitle,
    /// Data, timecode included.
    Data,
    /// An attachment (Matroska fonts and pictures).
    Attachment,
}

/// Entries of a section, in order.
pub type Entries = Vec<(&'static str, Value)>;

/// A side data element: its type, as the compact writer names it, and
/// its entries.
#[derive(Clone, Debug)]
pub struct SideData {
    /// The type as the compact writer names it (`display_matrix`).
    pub typ: &'static str,
    /// The entries, `side_data_type` first.
    pub entries: Entries,
}

/// One stream.
#[derive(Clone, Debug)]
pub struct Stream {
    /// The container's stream index, before unlisted tracks are removed.
    /// Attached files and pictures have no source track.
    pub source_index: Option<usize>,
    /// What the stream is.
    pub kind: StreamKind,
    /// An attached picture (`V` leaves it out).
    pub attached_pic: bool,
    /// The entries, in `ffprobe`'s order.
    pub entries: Entries,
    /// The disposition flags.
    pub disposition: Entries,
    /// The tags, in `ffprobe`'s order.
    pub tags: Vec<(String, String)>,
    /// The side data.
    pub side_data: Vec<SideData>,
}

impl Stream {
    /// `r_frame_rate` as numerator and denominator, when both are nonzero.
    pub fn r_frame_rate(&self) -> Option<(u64, u64)> {
        self.entries
            .iter()
            .find(|(k, _)| *k == "r_frame_rate")
            .and_then(|(_, v)| match v {
                Value::Str(text) => {
                    let (n, d) = text.split_once('/')?;
                    Some((n.parse().ok()?, d.parse().ok()?))
                }
                _ => None,
            })
            .filter(|(n, d): &(u64, u64)| *n > 0 && *d > 0)
    }
}

/// One chapter.
#[derive(Clone, Debug)]
pub struct Chapter {
    /// The entries, in `ffprobe`'s order.
    pub entries: Entries,
    /// The tags (`title`).
    pub tags: Vec<(String, String)>,
}

/// One program, with the indexes of its streams.
#[derive(Clone, Debug)]
pub struct Program {
    /// The entries, in `ffprobe`'s order.
    pub entries: Entries,
    /// The tags (`service_name`, `service_provider`).
    pub tags: Vec<(String, String)>,
    /// The indexes of its streams in [`Report::streams`].
    pub streams: Vec<usize>,
}

/// One packet, as `-show_packets` prints it.
#[derive(Clone, Debug, Default)]
pub struct Packet {
    /// The stream's index in [`Report::streams`].
    pub stream: usize,
    /// Presentation time in the stream's time base; `None` is unknown.
    pub pts: Option<i64>,
    /// Decode time, likewise.
    pub dts: Option<i64>,
    /// Duration, likewise.
    pub duration: Option<i64>,
    /// Size in bytes.
    pub size: u64,
    /// Byte position in the file; `None` is unknown.
    pub pos: Option<u64>,
    /// Flag `K`.
    pub keyframe: bool,
    /// Flag `D`: not presented.
    pub discard: bool,
    /// Flag `C`.
    pub corrupt: bool,
    /// The side data.
    pub side_data: Vec<SideData>,
    /// The bytes, when the listing was asked for them.
    pub data: Vec<u8>,
}

/// The packets of a file, read one at a time.
pub trait PacketSource {
    /// Calls `f` with every packet in the order `ffprobe` lists them,
    /// until it returns `Ok(false)` or the packets end; with each packet's
    /// bytes when its stream's `payloads` entry is true, as a decoder takes them.
    fn visit(
        &mut self,
        payloads: &[bool],
        f: &mut dyn FnMut(Packet) -> Result<bool, Failure>,
    ) -> Result<(), Failure>;
}

#[cfg(test)]
mod frames_tests;

/// Everything but the packets.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// The streams, in index order.
    pub streams: Vec<Stream>,
    /// Each stream's time base, for the packets' times.
    pub time_bases: Vec<(i64, i64)>,
    /// The format's entries.
    pub format: Entries,
    /// The format's tags.
    pub format_tags: Vec<(String, String)>,
    /// The chapters.
    pub chapters: Vec<Chapter>,
    /// The programs.
    pub programs: Vec<Program>,
    /// What each stream's decoder is given besides its packets.
    pub setups: Vec<Setup>,
    /// The streams that are cover pictures of an ISO base media file's item
    /// lists, not tracks: each one's index in `streams` and where its
    /// picture starts in the file, as `RemuxOptions::left_out_covers`
    /// names it.
    pub covers: Vec<(usize, u64)>,
}

/// A probed file: its report and what lists its packets.
pub struct Probed<'a> {
    /// Everything but the packets.
    pub report: Report,
    /// The packets.
    pub packets: Box<dyn PacketSource + 'a>,
}

/// Probes `source` (`size` bytes, printed as `name`) and builds its report.
pub fn probe<'a, S: Source + 'a>(
    source: &'a mut S,
    size: u64,
    name: &str,
    force: Option<args::Container>,
) -> Result<Probed<'a>, Failure> {
    let mut buffered = coreav_isobmff::BufferedSource::new(source);
    let matroska = crate::matroska::detect_source(&mut buffered)?;
    let wanted = |c: args::Container| force.is_none_or(|f| f == c);
    if matroska {
        if !wanted(args::Container::Matroska) {
            return Err(Failure::Library(coreav_isobmff::Error::NotIsobmff));
        }
        return mkv::probe(buffered, size, name);
    }
    if wanted(args::Container::Isobmff) {
        match iso::probe(&mut buffered, size, name) {
            Err(failure) if not_isobmff(&failure) => {}
            Ok(iso::IsoProbed {
                mut report,
                movie,
                priming,
                ends,
                shifts,
                steps,
                covers,
                pictures,
            }) => {
                report.setups = vec![Setup::default(); report.streams.len()];
                for track in &movie.tracks {
                    let description = &track.description;
                    if let Some(setup) = report.setups.get_mut(track.stream_index) {
                        *setup = Setup {
                            config: description
                                .codec_config
                                .as_ref()
                                .filter(|c| matches!(&c.kind.0, b"avcC" | b"dfLa"))
                                .map(|c| c.data.clone()),
                            width: description.width.map(u32::from),
                            height: description.height.map(u32::from),
                            unreadable: None,
                            skip: 0,
                        };
                    }
                }
                let listing = iso::IsoProbedPackets {
                    movie,
                    priming,
                    ends,
                    shifts,
                    steps,
                    covers,
                    pictures,
                };
                return Ok(Probed {
                    report,
                    packets: iso::packets(listing, buffered),
                });
            }
            Err(other) => return Err(other),
        }
    }
    if wanted(args::Container::Mpegts) && crate::mpegts::detect_source(&mut buffered)? {
        return ts::probe(buffered, size, name);
    }
    Err(Failure::Library(coreav_isobmff::Error::NotIsobmff))
}

fn not_isobmff(failure: &Failure) -> bool {
    matches!(failure, Failure::Library(coreav_isobmff::Error::NotIsobmff))
}

/// Whether `spec` selects stream `index`.
fn selected(spec: Option<StreamSpec>, streams: &[Stream], index: usize) -> bool {
    let Some(spec) = spec else {
        return true;
    };
    let Some(stream) = streams.get(index) else {
        return false;
    };
    let matches = |k: Kind2, s: &Stream| match k {
        Kind2::Video => s.kind == StreamKind::Video,
        Kind2::VideoNotPicture => s.kind == StreamKind::Video && !s.attached_pic,
        Kind2::Audio => s.kind == StreamKind::Audio,
        Kind2::Subtitle => s.kind == StreamKind::Subtitle,
        Kind2::Data => s.kind == StreamKind::Data,
        Kind2::Attachment => s.kind == StreamKind::Attachment,
    };
    match spec {
        StreamSpec::Index(n) => n == index,
        StreamSpec::Kind(k, None) => matches(k, stream),
        StreamSpec::Kind(k, Some(n)) => {
            streams
                .iter()
                .enumerate()
                .filter(|(_, s)| matches(k, s))
                .nth(n)
                .map(|(i, _)| i)
                == Some(index)
        }
    }
}

/// What reading the packets counted, per stream.
struct Counts {
    packets: Vec<Option<u64>>,
}

/// Where a packet falls against the read intervals.
struct IntervalState {
    intervals: Vec<Interval>,
    /// The interval being read and the packets read in it.
    current: usize,
    read: u64,
    /// The first timestamp of the current interval, seconds.
    start: Option<f64>,
}

impl IntervalState {
    /// Whether to list a packet at `time` seconds (`None` unknown);
    /// `Err(())` when reading stops. One interval, from the start of the
    /// file, with an end ([`args::parse`] refuses the others): reading
    /// stops at the first packet past the end, whatever its stream.
    fn take(&mut self, time: Option<f64>) -> Result<bool, ()> {
        let Some(interval) = self.intervals.get(self.current).copied() else {
            return Err(());
        };
        if self.start.is_none() {
            self.start = time;
        }
        let past = match interval.end {
            None => true,
            Some(IntervalEnd::Packets(n)) => self.read >= n,
            Some(IntervalEnd::At(end)) => time.is_some_and(|t| t > end),
            Some(IntervalEnd::After(d)) => match (self.start, time) {
                (Some(first), Some(t)) => t > first + d,
                _ => false,
            },
        };
        if past {
            self.current += 1;
            return Err(());
        }
        self.read += 1;
        Ok(true)
    }
}

/// Runs the command: prints the sections `options` asks for, of `probed`,
/// on `out`.
pub fn emit(
    options: &Options,
    probed: &mut Probed<'_>,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let sel = &options.selection;
    let report = &probed.report;
    let mut w = Writer::new(options.writer.clone(), options.style);
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 16);
    let flush = |buf: &mut Vec<u8>, out: &mut dyn Write| -> Result<(), Failure> {
        out.write_all(buf).map_err(Failure::io)?;
        buf.clear();
        Ok(())
    };
    let n = report.streams.len();
    let mut counts = Counts {
        packets: vec![None; n],
    };
    let mut frames = frames::Frames::plan(options, report)?;
    let show_packets = sel.shown(Sid::Packets);
    let show_frames = frames.is_some() && sel.shown(Sid::Frames);
    let counted = show_packets || options.count_packets;
    w.header(&mut buf, Sid::Root, None);
    if counted || frames.is_some() {
        if counted {
            for (i, c) in counts.packets.iter_mut().enumerate() {
                if selected(options.select_streams, &report.streams, i) {
                    *c = Some(0);
                }
            }
        }
        // Packets and frames print in one array when both are asked for.
        let list = match (show_packets, show_frames) {
            (true, true) => Some(Sid::PacketsAndFrames),
            (true, false) => Some(Sid::Packets),
            (false, true) => Some(Sid::Frames),
            (false, false) => None,
        };
        if let Some(sid) = list {
            w.header(&mut buf, sid, None);
        }
        let mut intervals = (!options.read_intervals.is_empty()).then(|| IntervalState {
            intervals: options.read_intervals.clone(),
            current: 0,
            read: 0,
            start: None,
        });
        let streams = &report.streams;
        let spec = options.select_streams;
        let time_bases = &report.time_bases;
        let packets = &mut counts.packets;
        let w_ref = &mut w;
        let buf_ref = &mut buf;
        let frames_ref = &mut frames;
        let payloads = frames_ref
            .as_ref()
            .map_or_else(|| vec![false; n], frames::Frames::payloads);
        // A decoder may stop where `ffprobe`'s goes on: a listing of frames
        // is held back until it is long, so that the command can still be
        // handed over whole.
        let hold = if frames_ref.is_some() { HOLD } else { 1 << 15 };
        let mut flushed = false;
        {
            let mut visit = |packet: Packet| -> Result<bool, Failure> {
                if !selected(spec, streams, packet.stream) {
                    return Ok(true);
                }
                let tb = time_bases.get(packet.stream).copied().unwrap_or((1, 1));
                if let Some(state) = intervals.as_mut() {
                    let t = packet.pts.or(packet.dts).and_then(|ts| {
                        match model::time(ts, tb.0, tb.1) {
                            Value::Time(t) => Some(t),
                            _ => None,
                        }
                    });
                    match state.take(t) {
                        Ok(true) => {}
                        Ok(false) => return Ok(true),
                        Err(()) => return Ok(false),
                    }
                }
                if let Some(Some(c)) = packets.get_mut(packet.stream) {
                    *c += 1;
                }
                match frames_ref.as_mut() {
                    Some(frames) => {
                        frames
                            .packet(packet, show_packets)
                            .map_err(|e| late(e, flushed))?;
                        print_ready(w_ref, buf_ref, sel, report, frames);
                    }
                    None if show_packets => {
                        print_packet(w_ref, buf_ref, sel, report, &packet, tb);
                    }
                    None => {}
                }
                if buf_ref.len() > hold {
                    flush(buf_ref, &mut *out)?;
                    flushed = true;
                }
                Ok(true)
            };
            probed.packets.visit(&payloads, &mut visit)?;
        }
        if let Some(frames) = frames.as_mut() {
            frames.finish().map_err(|e| late(e, flushed))?;
            print_ready(&mut w, &mut buf, sel, report, frames);
        }
        if list.is_some() {
            w.footer(&mut buf);
        }
    }
    let read_frames = match &frames {
        Some(frames) if options.count_frames => frames.counts(n),
        _ => vec![None; n],
    };
    if sel.shown(Sid::Programs) {
        w.header(&mut buf, Sid::Programs, None);
        for program in &report.programs {
            w.header(&mut buf, Sid::Program, None);
            entries(&mut w, &mut buf, sel, Sid::Program, &program.entries);
            tags(&mut w, &mut buf, sel, Sid::ProgramTags, &program.tags);
            if sel.shown(Sid::ProgramStreams) {
                w.header(&mut buf, Sid::ProgramStreams, None);
                for &index in &program.streams {
                    if !selected(options.select_streams, &report.streams, index) {
                        continue;
                    }
                    if let Some(stream) = report.streams.get(index) {
                        print_stream(
                            &mut w,
                            &mut buf,
                            sel,
                            stream,
                            [
                                Sid::ProgramStream,
                                Sid::ProgramStreamDisposition,
                                Sid::ProgramStreamTags,
                            ],
                            // A program's streams print their side data
                            // under the streams' selection, as `ffprobe`
                            // does.
                            Some(Sid::StreamSideDataList),
                            [
                                counts.packets.get(index).copied().flatten(),
                                read_frames.get(index).copied().flatten(),
                            ],
                        );
                    }
                }
                w.footer(&mut buf);
            }
            w.footer(&mut buf);
        }
        w.footer(&mut buf);
    }
    if sel.shown(Sid::StreamGroups) {
        w.header(&mut buf, Sid::StreamGroups, None);
        w.footer(&mut buf);
    }
    if sel.shown(Sid::Streams) {
        w.header(&mut buf, Sid::Streams, None);
        for (index, stream) in report.streams.iter().enumerate() {
            if !selected(options.select_streams, &report.streams, index) {
                continue;
            }
            print_stream(
                &mut w,
                &mut buf,
                sel,
                stream,
                [Sid::Stream, Sid::StreamDisposition, Sid::StreamTags],
                Some(Sid::StreamSideDataList),
                [
                    counts.packets.get(index).copied().flatten(),
                    read_frames.get(index).copied().flatten(),
                ],
            );
        }
        w.footer(&mut buf);
    }
    if sel.shown(Sid::Chapters) {
        w.header(&mut buf, Sid::Chapters, None);
        for chapter in &report.chapters {
            w.header(&mut buf, Sid::Chapter, None);
            entries(&mut w, &mut buf, sel, Sid::Chapter, &chapter.entries);
            tags(&mut w, &mut buf, sel, Sid::ChapterTags, &chapter.tags);
            w.footer(&mut buf);
        }
        w.footer(&mut buf);
    }
    if sel.shown(Sid::Format) {
        w.header(&mut buf, Sid::Format, None);
        entries(&mut w, &mut buf, sel, Sid::Format, &report.format);
        tags(&mut w, &mut buf, sel, Sid::FormatTags, &report.format_tags);
        w.footer(&mut buf);
    }
    w.footer(&mut buf);
    flush(&mut buf, out)?;
    out.flush().map_err(Failure::io)
}

/// Prints the error section `-show_error` asks for, in place of the
/// rest, for a file that could not be read.
pub fn emit_error(
    options: &Options,
    code: i64,
    message: &str,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let mut w = Writer::new(options.writer.clone(), options.style);
    let mut buf = Vec::new();
    w.header(&mut buf, Sid::Root, None);
    w.header(&mut buf, Sid::Error, None);
    w.entry(&mut buf, "code", &Value::Int(code));
    w.entry(&mut buf, "string", &Value::Str(message.to_owned()));
    w.footer(&mut buf);
    w.footer(&mut buf);
    out.write_all(&buf).map_err(Failure::io)?;
    out.flush().map_err(Failure::io)
}

fn entries(w: &mut Writer, buf: &mut Vec<u8>, sel: &Selection, sid: Sid, list: &Entries) {
    for (key, value) in list {
        if sel.entry(sid, key) {
            w.entry(buf, key, value);
        }
    }
}

fn tags(w: &mut Writer, buf: &mut Vec<u8>, sel: &Selection, sid: Sid, list: &[(String, String)]) {
    // No tags at all: no section, as `ffprobe` prints none; tags none of
    // which is selected: an empty one.
    if !sel.shown(sid) || list.is_empty() {
        return;
    }
    w.header(buf, sid, None);
    for (key, value) in list {
        // Tag keys are named without regard to case (`stream_tags=duration`
        // selects `DURATION`), as `ffprobe` matches them.
        if sel.entry_ignoring_case(sid, key) {
            w.entry(buf, key, &Value::Str(value.clone()));
        }
    }
    w.footer(buf);
}

fn print_stream(
    w: &mut Writer,
    buf: &mut Vec<u8>,
    sel: &Selection,
    stream: &Stream,
    [sid, disposition, tag_sid]: [Sid; 3],
    side_data: Option<Sid>,
    [read_packets, read_frames]: [Option<u64>; 2],
) {
    w.header(buf, sid, None);
    for (key, value) in &stream.entries {
        if !sel.entry(sid, key) {
            continue;
        }
        let read = match *key {
            "nb_read_packets" => Some(read_packets),
            "nb_read_frames" => Some(read_frames),
            _ => None,
        };
        if let Some(read) = read {
            // A stream none of whose packets or frames was read has no
            // count.
            let count = read
                .filter(|n| *n > 0)
                .and_then(|n| i64::try_from(n).ok())
                .map_or(Value::Na, |n| Value::Str(n.to_string()));
            w.entry(buf, key, &count);
        } else {
            w.entry(buf, key, value);
        }
    }
    if sel.shown(disposition) {
        w.header(buf, disposition, None);
        entries(w, buf, sel, disposition, &stream.disposition);
        w.footer(buf);
    }
    tags(w, buf, sel, tag_sid, &stream.tags);
    if let Some(list) = side_data {
        let item = Sid::StreamSideData;
        if !stream.side_data.is_empty() {
            w.header(buf, list, None);
            for data in &stream.side_data {
                w.header(buf, item, Some(data.typ));
                entries(w, buf, sel, item, &data.entries);
                w.footer(buf);
            }
            w.footer(buf);
        }
    }
    w.footer(buf);
}

/// Bytes of a listing of frames held back before any is written.
const HOLD: usize = 64 << 20;

/// A refusal once part of the listing is written is an error: handing the
/// command over then would print that part twice.
fn late(failure: Failure, flushed: bool) -> Failure {
    match failure {
        Failure::Unsupported(text) if flushed => Failure::VideoDecode(coreav_decode::Error::new(
            coreav_decode::ErrorKind::InvalidData,
            text,
        )),
        other => other,
    }
}

/// Prints what the frame listing has ready, in order.
fn print_ready(
    w: &mut Writer,
    buf: &mut Vec<u8>,
    sel: &Selection,
    report: &Report,
    frames: &mut frames::Frames,
) {
    while let Some(item) = frames.next() {
        match item {
            frames::Item::Packet(packet) => {
                let tb = report
                    .time_bases
                    .get(packet.stream)
                    .copied()
                    .unwrap_or((1, 1));
                print_packet(w, buf, sel, report, &packet, tb);
            }
            frames::Item::Frame(row) => {
                w.header(buf, Sid::Frame, None);
                entries(w, buf, sel, Sid::Frame, &row.entries);
                if !row.side_data.is_empty() {
                    w.header(buf, Sid::FrameSideDataList, None);
                    for data in &row.side_data {
                        w.header(buf, Sid::FrameSideData, Some(data.typ));
                        entries(w, buf, sel, Sid::FrameSideData, &data.entries);
                        w.footer(buf);
                    }
                    w.footer(buf);
                }
                w.footer(buf);
            }
        }
    }
}

fn print_packet(
    w: &mut Writer,
    buf: &mut Vec<u8>,
    sel: &Selection,
    report: &Report,
    packet: &Packet,
    (num, den): (i64, i64),
) {
    let sid = Sid::Packet;
    w.header(buf, sid, None);
    // The stream's own `codec_type` (`unknown` for a track of no known
    // kind), as `ffprobe` labels its packets.
    let kind = report
        .streams
        .get(packet.stream)
        .and_then(|s| {
            s.entries.iter().find_map(|(k, v)| match (k, v) {
                (&"codec_type", Value::Str(t)) => Some(t.as_str()),
                _ => None,
            })
        })
        .unwrap_or("unknown");
    let ts = |v: Option<i64>| v.map_or(Value::Na, Value::Int);
    let t = |v: Option<i64>| v.map_or(Value::Na, |v| model::time(v, num, den));
    let mut flags = String::with_capacity(3);
    flags.push(if packet.keyframe { 'K' } else { '_' });
    flags.push(if packet.discard { 'D' } else { '_' });
    flags.push(if packet.corrupt { 'C' } else { '_' });
    let list: [(&str, Value); 11] = [
        ("codec_type", Value::Str(kind.to_owned())),
        (
            "stream_index",
            Value::Int(i64::try_from(packet.stream).unwrap_or(0)),
        ),
        ("pts", ts(packet.pts)),
        ("pts_time", t(packet.pts)),
        ("dts", ts(packet.dts)),
        ("dts_time", t(packet.dts)),
        ("duration", ts(packet.duration)),
        ("duration_time", t(packet.duration)),
        (
            "size",
            Value::Unit(
                i64::try_from(packet.size).unwrap_or(i64::MAX),
                model::Unit::Byte,
            ),
        ),
        (
            "pos",
            packet.pos.map_or(Value::Na, |p| Value::Str(p.to_string())),
        ),
        ("flags", Value::Str(flags)),
    ];
    for (key, value) in &list {
        if sel.entry(sid, key) {
            w.entry(buf, key, value);
        }
    }
    if !packet.side_data.is_empty() {
        w.header(buf, Sid::PacketSideDataList, None);
        for data in &packet.side_data {
            w.header(buf, Sid::PacketSideData, Some(data.typ));
            entries(w, buf, sel, Sid::PacketSideData, &data.entries);
            w.footer(buf);
        }
        w.footer(buf);
    }
    w.footer(buf);
}

/// The usage text of `coreav ffprobe -h`.
pub const USAGE: &str = "\
usage: coreav ffprobe [OPTIONS] INPUT

Experimental: values can differ from ffprobe's for MP4/MOV, Matroska/WebM
and MPEG-TS. docs/measurements/compat-ffprobe.md has the agreement of
every key.

The common ffprobe invocations, over CoreAV's own probes. Options:
  -show_format -show_streams -show_chapters -show_programs -show_packets
  -show_frames -show_error -count_packets -count_frames
  -show_entries SECTION[=KEYS][:...]
  -select_streams SPEC -read_intervals INTERVALS -f FORMAT
  -of|-print_format default|compact|csv|flat|json[=OPTIONS]
  -pretty -sexagesimal -v LEVEL -loglevel LEVEL -hide_banner -nostdin
Reads MP4, MOV, Matroska, WebM and MPEG-TS files and HTTP(S) URLs.
Frames are decoded for MP3, FLAC and PCM, and for H.264 in a build
with CoreAV's H.264 decoder; other codecs' frames are refused.

For untrusted input, set COREAV_STRICT_SAFE=1: anything else is then
refused (exit status 4). Without it, anything else runs the ffprobe
program on PATH instead (or the one COREAV_FALLBACK names), which is
FFmpeg's C code, and says so on standard error. Help on a topic
(-h long, -h full, -h demuxer=NAME, ...) is handled the same way.
See docs/design/compat-cli.md.
";
