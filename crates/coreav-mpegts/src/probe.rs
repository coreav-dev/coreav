//! The probe: the tables and each stream's parameters from the head of the
//! file, and the last timestamps from its tail.

use crate::codec::{Allowlist, Codec, StreamKind};
use crate::demux::{Demux, Pes};
use crate::error::Warning;
use crate::frames::{self, Syntax};
use crate::packet::PacketSize;
use crate::pid_map::PidMap;
use crate::reader::{Reader, detect};
use crate::section::{Descriptor, Service};
use crate::video::{self, Nal, ParameterSets};
use crate::{Error, LimitKind, Limits, Source};
use coreav_isobmff::{
    Color, ConfigRecord, FourCc, Profile, describe_audio_specific_config, describe_config_record,
};

/// The 33-bit timestamp range at 90 kHz.
pub(crate) const WRAP: i64 = 1 << 33;
/// Timestamps up to this far (60 s) before a stream's first one belong to
/// it rather than to the next wrap.
const WRAP_WINDOW: i64 = 60 * 90_000;

/// One program of the PAT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    /// `program_number`.
    pub number: u16,
    /// The PID of its PMT.
    pub pmt_pid: u16,
    /// The PID of its clock reference.
    pub pcr_pid: u16,
    /// The PIDs of its streams, in PMT order.
    pub pids: Vec<u16>,
    /// Whether its Program Map Table was read: `pcr_pid` and `pids` are
    /// its, not defaults.
    pub pmt_read: bool,
}

/// Video parameters from a stream's parameter sets or sequence header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Video {
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
}

/// Audio parameters from a stream's first frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Audio {
    /// Samples per second.
    pub sample_rate: u32,
    /// Channels (0 when the frame header leaves them to the payload).
    pub channels: u8,
    /// The AAC audio object type, the MPEG audio layer, or for AC-3 0 and
    /// for E-AC-3 1.
    pub detail: u8,
}

/// One elementary stream.
#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    /// Its place among every stream the tables name, dropped ones
    /// included: the index `ffprobe` gives it.
    pub stream_index: usize,
    /// The PID.
    pub pid: u16,
    /// The programs that list it.
    pub programs: Vec<u16>,
    /// `stream_type`.
    pub stream_type: u8,
    /// The codec its type and descriptors name.
    pub codec: Codec,
    /// Its kind of media.
    pub kind: StreamKind,
    /// Its place in the elementary stream loop of the first Program Map
    /// Table that declared it, so that the order the tables give is kept
    /// whatever order `streams` is in; `None` for a stream no table
    /// declares (the Event Information Table).
    pub pmt_index: Option<usize>,
    /// Its ES descriptors.
    pub descriptors: Vec<Descriptor>,
    /// The four bytes its descriptors give its format: the registration
    /// descriptor's `format_identifier`, else the
    /// `metadata_format_identifier` of its metadata descriptor.
    pub format_identifier: Option<[u8; 4]>,
    /// The ISO 639 language code of its language descriptor.
    pub language: Option<String>,
    /// Video parameters, when read.
    pub video: Option<Video>,
    /// Audio parameters, when read.
    pub audio: Option<Audio>,
    /// The codec configuration in the form MP4 and Matroska carry it: an
    /// `avcC` or `hvcC` payload made from the in-band parameter sets, or an
    /// AudioSpecificConfig made from the ADTS header.
    pub config: Option<Vec<u8>>,
    /// Profile and level, from the configuration.
    pub profile: Option<Profile>,
    /// The video signal type, from the configuration.
    pub color: Color,
    /// MPEG-2 video: the escape bit and profile, and the level, of
    /// `profile_and_level_indication` (ISO/IEC 13818-2 table 8-1).
    pub mpeg2_profile: Option<(u8, u8)>,
    /// The first timestamp of the stream (90 kHz, after wrap correction).
    pub start: Option<i64>,
    /// Where its last access unit ends, from the tail of the file (90 kHz).
    pub end: Option<i64>,
    /// The timestamp of that access unit (90 kHz): `end` minus it is the
    /// unit's duration.
    pub last_pts: Option<i64>,
    /// Where the file first shows the stream (a byte offset). For a
    /// stream [`Stream::with_first_table`], the packet that completed that
    /// table (or the Event Information Table's first section). For the
    /// others, the first of: a Program Map Table section naming it,
    /// whether it comes before or after the first PAT; its first PES
    /// packet that is not padding, wherever it is; the Event Information
    /// Table's first section. `u64::MAX` when none is read. `ffprobe`
    /// numbers the streams [`Stream::with_first_table`] first, then the
    /// others in this order (`docs/design/compat-cli.md`, "MPEG-TS").
    pub found_at: u64,
    /// Whether the stream is found with the first Program Map Table the
    /// file completes after its first PAT, or is the Event Information
    /// Table read before that table: `ffprobe` reads the file up to that
    /// table first, then again from the start.
    pub with_first_table: bool,
}

impl Stream {
    /// The stream's duration in 90 kHz ticks, from its first timestamp to
    /// the end of its last access unit.
    pub fn duration(&self) -> Option<i64> {
        Some(self.end? - self.start?).filter(|d| *d >= 0)
    }
}

/// A stream whose codec is not on the allowlist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedStream {
    /// Its place among every stream the tables name: the index `ffprobe`
    /// gives it.
    pub stream_index: usize,
    /// The PID.
    pub pid: u16,
    /// `stream_type`.
    pub stream_type: u8,
    /// The codec its type and descriptors name.
    pub codec: Codec,
    /// Where the file first shows the stream ([`Stream::found_at`]).
    pub found_at: u64,
}

/// The probe result.
#[derive(Clone, Debug, PartialEq)]
pub struct TransportStream {
    /// How packets are stored.
    pub packet_size: PacketSize,
    /// Offset of the first stored packet.
    pub first_packet: u64,
    /// Bytes in the file.
    pub size: u64,
    /// `transport_stream_id` of the PAT.
    pub transport_stream_id: Option<u16>,
    /// The services the Service Description Table of this transport
    /// stream names, from the part of the file the probe read; empty when
    /// it read none.
    pub services: Vec<Service>,
    /// Whether the file shows its first Service Description Table before
    /// its first Program Association Table (in the part the probe read
    /// from the start).
    pub services_first: bool,
    /// Programs, in PAT order.
    pub programs: Vec<Program>,
    /// Kept streams, in the order the PMTs list them.
    pub streams: Vec<Stream>,
    /// Streams the allowlist left out. A stream whose codec this crate
    /// cannot name is never one of them: it is listed in `streams` with
    /// [`Codec::Other`].
    pub dropped: Vec<DroppedStream>,
    /// Problems the probe went past.
    pub warnings: Vec<Warning>,
    /// The first timestamp of the file, raw: the reference for the 33-bit
    /// wrap of a PID no program declares.
    pub(crate) wrap_reference: Option<u64>,
    /// Per PID, the first timestamp of its program, raw: a program has a
    /// system time base of its own (ISO/IEC 13818-1 2.4.2.1), so the
    /// programs of one file can be hours apart and a timestamp is only
    /// before or after the wrap of its own program.
    pub(crate) program_reference: PidMap<u64>,
    pub(crate) allowlist: Allowlist,
}

impl TransportStream {
    /// The earliest start of a stream (90 kHz).
    pub fn start(&self) -> Option<i64> {
        self.streams.iter().filter_map(|s| s.start).min()
    }

    /// From the earliest start of a stream to the latest end (90 kHz).
    ///
    /// A file with more than one program has no one timeline: each
    /// program keeps a system time base of its own (ISO/IEC 13818-1
    /// 2.4.2.1), and the clocks of a broadcast capture's programs are
    /// commonly hours apart, so a span across them means nothing. Such a
    /// file lasts as long as its longest program, which is what `ffprobe`
    /// reports too.
    pub fn duration(&self) -> Option<i64> {
        if self.programs.len() > 1 {
            return self
                .programs
                .iter()
                .filter_map(|p| self.program_span(p.number))
                .max();
        }
        let end = self.streams.iter().filter_map(|s| s.end).max()?;
        Some(end - self.start()?).filter(|d| *d >= 0)
    }

    /// From the earliest start of a stream of `number` to the latest end.
    fn program_span(&self, number: u16) -> Option<i64> {
        let of = || self.streams.iter().filter(|s| s.programs.contains(&number));
        let start = of().filter_map(|s| s.start).min()?;
        let end = of().filter_map(|s| s.end).max()?;
        Some(end - start).filter(|d| *d >= 0)
    }

    /// A timestamp of `pid` after the wrap correction of its program.
    pub(crate) fn unwrap(&self, pid: u16, raw: u64) -> i64 {
        let reference = self
            .program_reference
            .get(&pid)
            .copied()
            .or(self.wrap_reference)
            .unwrap_or(raw);
        unwrap(raw, reference)
    }
}

/// Records the wrap reference of every PID of every program the
/// demultiplexer knows: the first timestamp of the earliest PES packet of
/// any of the program's PIDs. A PID that two programs declare keeps the
/// first program's.
fn note_references(references: &mut PidMap<u64>, demux: &Demux) {
    for program in &demux.programs {
        let base = program
            .pids
            .iter()
            .filter_map(|pid| demux.first_pes.get(pid))
            .min_by_key(|(offset, _)| *offset)
            .map(|(_, time)| *time);
        if let Some(time) = base {
            for pid in &program.pids {
                let _ = references.get_or_insert_with(*pid, || time);
            }
        }
    }
}

/// Places a 33-bit timestamp on the timeline of its program. The
/// program's first timestamp is the reference: if it is within 60 s of
/// the wrap,
/// timestamps from 60 s before it on are taken as before the wrap
/// (negative); otherwise timestamps that are more than 60 s before it are
/// taken as after the wrap (plus 2^33).
pub(crate) fn unwrap(raw: u64, reference: u64) -> i64 {
    let raw = i64::try_from(raw & ((1 << 33) - 1)).unwrap_or(0);
    let reference = i64::try_from(reference & ((1 << 33) - 1)).unwrap_or(0);
    let floor = reference - WRAP_WINDOW;
    if WRAP - reference <= WRAP_WINDOW {
        if raw >= floor { raw - WRAP } else { raw }
    } else if floor >= 0 {
        if raw < floor { raw + WRAP } else { raw }
    } else if raw >= floor + WRAP {
        raw - WRAP
    } else {
        raw
    }
}

/// What the probe has learnt of one kept stream so far.
#[derive(Default)]
struct Learn {
    parameters: ParameterSets,
    done: bool,
    start_raw: Option<u64>,
    video: Option<Video>,
    audio: Option<Audio>,
    config: Option<Vec<u8>>,
    record: Option<ConfigRecord>,
    mpeg2_profile: Option<(u8, u8)>,
    /// The colour description of an MPEG-2 video sequence display
    /// extension, which the configuration record of the other codecs
    /// carries instead.
    colour: Option<Color>,
    /// MPEG-1 or MPEG-2 video, as its sequence header shows.
    mpeg1: Option<bool>,
}

fn nal_of(codec: Codec) -> Option<Nal> {
    match codec {
        Codec::H264 => Some(Nal::H264),
        Codec::Hevc => Some(Nal::Hevc),
        _ => None,
    }
}

/// The AudioSpecificConfig an ADTS header stands for (ISO/IEC 14496-3
/// 1.6.2.1): object type, sampling frequency index, channel configuration.
fn adts_config(header: &[u8]) -> Option<Vec<u8>> {
    let [_, _, b2, b3, ..] = *header else {
        return None;
    };
    let object = (b2 >> 6) + 1;
    let index = (b2 >> 2) & 0x0F;
    let channels = ((b2 & 1) << 2) | (b3 >> 6);
    Some(vec![
        (object << 3) | (index >> 1),
        ((index & 1) << 7) | (channels << 3),
    ])
}

/// A two-byte AudioSpecificConfig for an object type, a sample rate of
/// the standard table and a channel configuration.
fn asc(object: u8, rate: u32, channels: u8) -> Option<Vec<u8>> {
    const RATES: [u32; 13] = [
        96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
        8_000, 7_350,
    ];
    let index = u8::try_from(RATES.iter().position(|r| *r == rate)?).ok()?;
    (object < 31).then(|| {
        vec![
            (object << 3) | (index >> 1),
            ((index & 1) << 7) | (channels << 3),
        ]
    })
}

impl Learn {
    fn take(&mut self, codec: Codec, pes: &Pes) {
        if self.start_raw.is_none() {
            self.start_raw = pes.header.pts.or(pes.header.dts);
        }
        if self.done {
            return;
        }
        let data = pes.payload.as_slice();
        if let Some(nal) = nal_of(codec) {
            self.parameters.add(nal, data);
            if let Some(record) = self.parameters.record(nal) {
                let kind = FourCc(if nal == Nal::H264 { *b"avcC" } else { *b"hvcC" });
                let described = describe_config_record(kind, &record);
                // A transport stream states no picture size of its own:
                // the size is what the first sequence parameter set
                // describes, its whole conformance crop taken off (a 720p
                // stream is commonly coded 1920x1088 and cropped), not the
                // bound `parameter_set_size` keeps for framing samples.
                if let Some((width, height)) = described
                    .as_ref()
                    .and_then(|d| d.parameter_set_display_size)
                {
                    self.video = Some(Video { width, height });
                }
                self.record = described;
                self.config = Some(record);
                self.done = true;
            }
        } else if codec == Codec::PgsSubtitle {
            // The picture a presentation graphics subtitle is composed
            // on, which the stream itself states.
            if let Some((width, height)) = video::pgs_size(data) {
                self.video = Some(Video { width, height });
                self.done = true;
            }
        } else if matches!(codec, Codec::Mpeg1Video | Codec::Mpeg2Video) {
            if let Some((width, height)) = video::mpeg2_size(data) {
                self.video = Some(Video { width, height });
                self.mpeg2_profile = video::mpeg2_profile(data);
                // The sequence display extension's colour description
                // (13818-2 6.2.2.4), when the sequence carries one.
                if let Some((primaries, transfer, matrix)) = video::mpeg2_colour(data) {
                    self.colour = Some(Color {
                        primaries: Some(primaries),
                        transfer: Some(transfer),
                        matrix: Some(matrix),
                        full_range: Some(false),
                    });
                }
                // A sequence header without a sequence extension is MPEG-1
                // video (13818-2 6.1.1.6), whatever the stream type says.
                self.mpeg1 = Some(self.mpeg2_profile.is_none());
                self.done = true;
            }
        } else if let Some(syntax) = codec.frames() {
            // The frames of a PES packet in turn: a LOAS frame carries the
            // stream's configuration only now and then (`useSameStreamMux`
            // leaves it out of the others), so the first frame of a PES
            // packet commonly states no rate.
            if let Some((at, frame)) = frames::first_with_rate(syntax, data) {
                self.audio = Some(Audio {
                    sample_rate: frame.sample_rate,
                    channels: frame.channels,
                    detail: frame.detail,
                });
                let rest = data.get(at..).unwrap_or_default();
                if syntax == Syntax::Adts {
                    self.config = adts_config(rest);
                }
                // The configuration as the frame carries it, or, where
                // its length is not read, the two bytes that state what
                // the frame header does.
                if syntax == Syntax::Latm {
                    self.config = rest
                        .get(3..)
                        .and_then(frames::latm_audio_specific_config)
                        .or_else(|| asc(frame.detail, frame.sample_rate, frame.channels));
                }
                self.done = true;
            }
        } else {
            self.done = true;
        }
    }
}

/// What one read of the tail of the file shows, per stream.
#[derive(Default)]
struct TailEnds {
    /// Where the stream's last access unit ends.
    ends: Vec<Option<i64>>,
    /// The timestamp of that unit.
    lasts: Vec<Option<i64>>,
    /// Every presentation time read.
    times: Vec<Vec<i64>>,
}

impl TailEnds {
    #[inline]
    fn resize(&mut self, streams: usize) {
        // Called for every packet of the tail: most add no stream.
        if self.ends.len() == streams {
            return;
        }
        self.ends.resize(streams, None);
        self.lasts.resize(streams, None);
        self.times.resize(streams, Vec::new());
    }

    /// Takes what `longer` shows of the streams this read shows no end of.
    fn fill_from(&mut self, longer: TailEnds) {
        for (index, end) in self.ends.iter_mut().enumerate() {
            if end.is_some() {
                continue;
            }
            *end = longer.ends.get(index).copied().flatten();
            if let Some(last) = self.lasts.get_mut(index) {
                *last = longer.lasts.get(index).copied().flatten();
            }
        }
    }
}

/// Where and how a read of the tail reads.
struct TailRead<'a> {
    packet_size: PacketSize,
    len: u64,
    first_packet: u64,
    /// Where the read starts.
    start: u64,
    limits: &'a Limits,
    /// The streams from this index on are learnt from the tail: those the
    /// head did not show.
    learn_from: usize,
    program_reference: &'a PidMap<u64>,
    reference: Option<u64>,
}

/// Reads the tail of the file from `at.start` to the end with `tail`, a
/// demultiplexer that resumes the head's tables, for the last timestamps
/// of each stream.
fn read_tail<S: Source>(
    source: &mut S,
    at: &TailRead<'_>,
    tail: &mut Demux,
    learnt: &mut Vec<Learn>,
    tail_warnings: &mut Vec<Warning>,
) -> Result<TailEnds, Error> {
    let mut out = TailEnds::default();
    out.resize(tail.streams.len());
    let mut tail_reader = Reader::new(at.packet_size, at.len, at.first_packet, at.limits);
    // The tail only refines durations: a tail that cannot be read (lost
    // sync past the resynchronisation bound) leaves them unknown rather
    // than failing the probe. A source that fails to read is not a damaged
    // tail: its error fails the probe.
    let tail_ok = match tail_reader.seek(source, at.start) {
        Ok(()) => true,
        Err(error @ Error::Io(_)) => return Err(error),
        Err(_) => false,
    };
    loop {
        let next = if tail_ok {
            match tail_reader.next(source, tail_warnings) {
                Ok(next) => next,
                Err(error @ Error::Io(_)) => return Err(error),
                Err(_) => None,
            }
        } else {
            None
        };
        match next {
            Some((offset, packet)) => {
                if tail.feed(offset, &packet, tail_warnings).is_err() {
                    break;
                }
            }
            None => tail.finish(tail_warnings),
        }
        // Streams a later PMT version added, which the tail's tables name.
        let known = tail.streams.len();
        out.resize(known);
        learnt.resize_with(known, Learn::default);
        while let Some(pes) = tail.ready.pop_front() {
            let Some(index) = tail.stream_of(pes.pid) else {
                continue;
            };
            if index >= at.learn_from {
                tail.sniffed(index, &pes);
                if let (Some(entry), Some(learn)) = (tail.streams.get(index), learnt.get_mut(index))
                {
                    if entry.kept && !entry.sniff {
                        learn.take(entry.codec, &pes);
                    }
                }
            }
            let Some(entry) = tail.streams.get(index) else {
                continue;
            };
            let Some(raw) = pes.header.pts else { continue };
            let pts = unwrap(
                raw,
                at.program_reference
                    .get(&pes.pid)
                    .copied()
                    .or(at.reference)
                    .unwrap_or(raw),
            );
            let rate = learnt
                .get(index)
                .and_then(|l| l.audio)
                .map(|a| a.sample_rate);
            let span = pes_span(entry.codec, &pes.payload, rate);
            if let Some(list) = out.times.get_mut(index) {
                list.push(pts);
            }
            if let (Some(span), Some(end), Some(last)) =
                (span, out.ends.get_mut(index), out.lasts.get_mut(index))
            {
                let candidate = pts + span;
                if end.is_none_or(|e| candidate > e) {
                    *end = Some(candidate);
                    *last = Some(pts);
                }
            }
        }
        if next.is_none() {
            break;
        }
    }
    Ok(out)
}

/// Ends the streams whose PES packets carry no frame of their own: a
/// video or audio stream ends at its last presentation time plus the
/// smallest step between presentation times, which is the picture
/// interval or the length of an audio unit the reader cannot read
/// (Blu-ray LPCM); a subtitle, metadata or unnamed stream ends at its last
/// presentation time, because the step between its units is not a
/// duration -- a subtitle or an identifier lasts what it says, not until
/// the next one.
///
/// `durationless`: whether a stream whose units state no duration (a
/// subtitle, metadata or unnamed stream) takes an end at all.
fn finish_ends(tail: &Demux, learnt: &[Learn], out: &mut TailEnds, durationless: bool) {
    for (index, entry) in tail.streams.iter().enumerate() {
        // A stream whose frames carry their length already has its end,
        // unless none of its frames could be read (a LATM stream whose
        // configuration is out of band): then this rule has the last word.
        if out.ends.get(index).copied().flatten().is_some() {
            continue;
        }
        let Some(list) = out.times.get(index) else {
            continue;
        };
        let mut sorted = list.clone();
        sorted.sort_unstable();
        sorted.dedup();
        let continuous = matches!(entry.codec.kind(), StreamKind::Video | StreamKind::Audio);
        if !continuous && !durationless {
            continue;
        }
        let step = if continuous {
            sorted
                .windows(2)
                .filter_map(|w| match *w {
                    [a, b] => Some(b - a),
                    _ => None,
                })
                .filter(|d| *d > 0)
                .min()
        } else {
            Some(0)
        };
        // A stream the file times once shows no step, and its last access
        // unit still lasts: an H.264 stream whose sequence parameter set
        // carries timing information (ITU-T H.264 E.2.1) states a clock,
        // and one tick of it is the smallest interval that clock can
        // express, as the step above is the smallest step the file shows.
        // The file's own timestamps come first: a stream's timing
        // information may name a clock its timestamps do not keep.
        let step = step.or_else(|| {
            matches!(entry.codec, Codec::H264)
                .then(|| learnt.get(index)?.config.as_deref())
                .flatten()
                .and_then(coreav_isobmff::avc_timing)
                .and_then(|(units, scale)| {
                    // A tick longer than a second is not a picture clock:
                    // a parameter set may state anything.
                    Some(i64::from(units).checked_mul(90_000)? / i64::from(scale))
                        .filter(|ticks| (1..=90_000).contains(ticks))
                })
        });
        if let (Some(&last), Some(step), Some(end), Some(last_pts)) = (
            sorted.last(),
            step,
            out.ends.get_mut(index),
            out.lasts.get_mut(index),
        ) {
            *end = Some(last + step);
            *last_pts = Some(last);
        }
    }
}

/// Takes every PES packet the demultiplexer has finished into what the
/// probe has learnt of its stream, and notes the file's first timestamp.
fn learn_ready(demux: &mut Demux, learnt: &mut [Learn], reference: &mut Option<u64>) {
    while let Some(pes) = demux.ready.pop_front() {
        if reference.is_none() {
            *reference = pes.header.dts.or(pes.header.pts);
        }
        let Some(index) = demux.stream_of(pes.pid) else {
            continue;
        };
        demux.sniffed(index, &pes);
        if let (Some(entry), Some(learn)) = (demux.streams.get(index), learnt.get_mut(index)) {
            // A stream still being identified has nothing to learn yet.
            if entry.kept && !entry.sniff {
                learn.take(entry.codec, &pes);
            }
        }
    }
}

/// Where the transport packets of a file are.
struct HeadAt {
    packet_size: PacketSize,
    len: u64,
    first_packet: u64,
}

/// Reads the head of the file, from its first packet, until the tables are
/// complete (or the demultiplexer was given its streams without them) and
/// every stream has shown what it states, or `max_probe_bytes` are read.
/// Returns where the budget stopped the read when packets were left after
/// it, `None` when the read stopped for another reason.
fn read_head<S: Source>(
    source: &mut S,
    at: &HeadAt,
    limits: &Limits,
    demux: &mut Demux,
    learnt: &mut Vec<Learn>,
    reference: &mut Option<u64>,
    warnings: &mut Vec<Warning>,
) -> Result<Option<u64>, Error> {
    let mut reader = Reader::new(at.packet_size, at.len, at.first_packet, limits);
    let budget_end = at.first_packet.saturating_add(limits.max_probe_bytes);
    let mut at_end = false;
    while reader.position() < budget_end {
        let Some((offset, packet)) = reader.next(source, warnings)? else {
            at_end = true;
            break;
        };
        demux.feed(offset, &packet, warnings)?;
        learnt.resize_with(demux.streams.len(), Learn::default);
        learn_ready(demux, learnt, reference);
        // A stream whose codec this crate cannot name has no parameters to
        // learn and may carry sections rather than PES packets, so waiting
        // for one would read to `max_probe_bytes` on every file with one.
        let all_learnt = demux
            .streams
            .iter()
            .zip(learnt.iter())
            .all(|(entry, learn)| {
                !entry.sniff
                    && (!entry.kept
                        || entry.codec == Codec::Other
                        || (learn.done && learn.start_raw.is_some()))
            });
        if (demux.tables_complete() || demux.tableless) && all_learnt {
            break;
        }
    }
    // A PES packet only the end of the file completes is the stream's too:
    // a video stream whose PES packets state no length (ISO/IEC 13818-1
    // 2.4.3.7 allows 0 for video) and whose first one runs to the end of a
    // short file carries the parameter sets, and leaving it in the
    // reassembler lost them, with the picture size, the profile and the
    // level they describe. Only the end of the file flushes: a PES packet
    // cut by the probe's budget is incomplete, and its payload would
    // describe the stream from bytes the file does not end at.
    if at_end {
        demux.finish(warnings);
        learnt.resize_with(demux.streams.len(), Learn::default);
        learn_ready(demux, learnt, reference);
    }
    Ok((reader.position() >= budget_end && !reader.exhausted()).then_some(budget_end))
}

/// Reads the tables and each kept stream's parameters, and the last
/// timestamps from the end of the file.
///
/// # Errors
///
/// [`Error::NotTransportStream`] when no packet size fits the start of the
/// file, [`Error::NoPrograms`] when a PAT is found and none of the PMTs it
/// names, or neither a PAT nor a PES packet is (a file without a PAT lists
/// the PIDs that carry PES packets), in a file read to its end,
/// [`LimitKind::ProbeBytes`] when that is so of the first
/// `max_probe_bytes` and the file goes on after them, and errors of the
/// source.
pub fn probe<S: Source>(
    mut source: S,
    limits: &Limits,
    allowlist: &Allowlist,
) -> Result<TransportStream, Error> {
    let len = source.len()?;
    let (packet_size, first_packet) = detect(&mut source, len, limits)?;
    let mut warnings = Vec::new();
    let at = HeadAt {
        packet_size,
        len,
        first_packet,
    };
    let mut demux = Demux::new(allowlist, limits);
    demux.name_undeclared = true;
    let mut learnt: Vec<Learn> = Vec::new();
    let mut reference: Option<u64> = None;
    let budget_spent = read_head(
        &mut source,
        &at,
        limits,
        &mut demux,
        &mut learnt,
        &mut reference,
        &mut warnings,
    )?;
    // A file without a Program Association Table names no stream, and
    // `ffprobe` lists the PIDs that carry PES packets all the same, in the
    // order their first PES packets come, each with the codec its payload
    // shows and in no program. The head is read again with those PIDs
    // as its streams, from the start: their first PES packets were read
    // before the reader knew there would be no table.
    let tableless = !demux.pat_read() && !demux.pes_pids.is_empty();
    if tableless {
        let pids = demux.pes_pids.clone();
        demux = Demux::new(allowlist, limits);
        demux.without_tables(&pids)?;
        learnt.clear();
        reference = None;
        warnings.clear();
        read_head(
            &mut source,
            &at,
            limits,
            &mut demux,
            &mut learnt,
            &mut reference,
            &mut warnings,
        )?;
    }
    if !tableless
        && !demux.tables_complete()
        && demux.programs.iter().all(|p| p.pmt_version.is_none())
    {
        // The tables may come after the budget: the limit, so that a
        // caller can tell a file that names no program from one whose
        // head is longer than it allowed.
        return Err(match budget_spent {
            Some(offset) => Error::LimitExceeded {
                limit: LimitKind::ProbeBytes,
                offset,
            },
            None => Error::NoPrograms,
        });
    }
    // A program has a system time base of its own (ISO/IEC 13818-1
    // 2.4.2.1): the reference for the 33-bit wrap is the program's own
    // first timestamp, from the earliest PES packet of any of its PIDs.
    // Taking the file's first timestamp for every stream put the streams
    // of a multi-program capture whose clocks are hours apart a whole
    // wrap (2^33 ticks, 26 hours) from where the file has them.
    let mut program_reference = PidMap::streams(limits);
    note_references(&mut program_reference, &demux);
    // The tail: where each stream's last access unit ends.
    let mut tail = demux.resume();
    let mut tail_warnings = Vec::new();
    let mut window = limits.max_tail_bytes;
    let mut read = read_tail(
        &mut source,
        &TailRead {
            packet_size,
            len,
            first_packet,
            start: len.saturating_sub(window).max(first_packet),
            limits,
            learn_from: demux.streams.len(),
            program_reference: &program_reference,
            reference,
        },
        &mut tail,
        &mut learnt,
        &mut tail_warnings,
    )?;
    finish_ends(&tail, &learnt, &mut read, true);
    // A stream the tail shows no end of is looked for again in a tail
    // twice as long, as many times as the limits allow; a stream that has
    // an end keeps it, whatever the longer tail shows of it, and a longer
    // tail ends only the streams whose units last (video and audio): on
    // the captures of the v1-ts-audio corpus, `ffprobe` gives a
    // presentation graphics stream an end from the last 250,000 bytes and
    // none from further back.
    for _ in 0..limits.tail_retries {
        if read.ends.iter().all(Option::is_some) {
            break;
        }
        window = window.saturating_mul(2);
        let mut again = demux.resume();
        let mut longer = read_tail(
            &mut source,
            &TailRead {
                packet_size,
                len,
                first_packet,
                start: len.saturating_sub(window).max(first_packet),
                limits,
                learn_from: usize::MAX,
                program_reference: &program_reference,
                reference,
            },
            &mut again,
            &mut learnt,
            &mut Vec::new(),
        )?;
        finish_ends(&again, &learnt, &mut longer, false);
        read.fill_from(longer);
    }
    let TailEnds {
        mut ends,
        mut lasts,
        ..
    } = read;
    // Programs and PIDs the tail's tables added, and the PIDs that
    // carried PES packets no table declares, after the declared streams.
    note_references(&mut program_reference, &tail);
    tail.add_undeclared(Some(&demux.pes_pids))?;
    note_references(&mut program_reference, &tail);
    ends.resize(tail.streams.len(), None);
    lasts.resize(tail.streams.len(), None);
    learnt.resize_with(tail.streams.len(), Learn::default);
    for warning in tail_warnings {
        if matches!(warning, Warning::PmtChanged { .. }) {
            crate::error::warn(&mut warnings, warning);
        }
    }
    // Where the file first shows each stream ([`Stream::found_at`]). The
    // tail may start before the head's first table (a short file is read
    // whole again); its data counts all the same, as the head's does.
    let first_pmt = demux.first_pmt_at;
    let with_first_table =
        |entry: &crate::demux::Entry| first_pmt.is_some_and(|at| entry.found_at <= at);
    let found = |entry: &crate::demux::Entry| {
        if with_first_table(entry) {
            return entry.found_at;
        }
        let early = demux
            .early_pmts
            .iter()
            .filter(|e| {
                e.pids.contains(&entry.pid)
                    && tail
                        .programs
                        .iter()
                        .any(|p| p.number == e.number && p.pmt_pid == e.pmt_pid)
            })
            .map(|e| e.at)
            .min();
        [
            Some(entry.found_at),
            early,
            demux.pes_pids.get(&entry.pid).copied(),
            tail.pes_pids.get(&entry.pid).copied(),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(u64::MAX)
    };
    let mut streams = Vec::new();
    let mut dropped = Vec::new();
    // The head's streams, then those the tail's tables added.
    for (index, entry) in tail.streams.iter().enumerate() {
        if !entry.kept {
            if allowlist.is_strict() {
                return Err(Error::CodecNotAllowed {
                    pid: entry.pid,
                    codec: entry.codec,
                });
            }
            dropped.push(DroppedStream {
                stream_index: index,
                pid: entry.pid,
                stream_type: entry.stream_type,
                codec: entry.codec,
                found_at: found(entry),
            });
            continue;
        }
        let learn = learnt.get(index);
        let language = entry
            .descriptors
            .iter()
            .find(|d| d.tag == 0x0A)
            .and_then(|d| d.data.get(..3))
            .filter(|code| code.iter().all(u8::is_ascii_alphabetic))
            .map(|code| String::from_utf8_lossy(code).into_owned());
        let record = learn.and_then(|l| l.record.as_ref());
        // A stream starts at the first timestamp of its PES packets, which
        // commonly come before the Program Map Table that names the PID:
        // those packets are the stream's too, and `ffprobe`, which reads
        // the file again from the start once it has the tables, reports
        // their timestamp. A stream only the tail's tables name has no
        // head timestamp, and takes the first one read.
        let start = demux
            .first_pes
            .get(&entry.pid)
            .or_else(|| tail.first_pes.get(&entry.pid))
            .map(|(_, time)| *time)
            .or_else(|| learn.and_then(|l| l.start_raw))
            .map(|raw| {
                unwrap(
                    raw,
                    program_reference
                        .get(&entry.pid)
                        .copied()
                        .or(reference)
                        .unwrap_or(raw),
                )
            });
        let codec = match (entry.codec, learn.and_then(|l| l.mpeg1)) {
            (Codec::Mpeg1Video | Codec::Mpeg2Video, Some(true)) => Codec::Mpeg1Video,
            (Codec::Mpeg1Video | Codec::Mpeg2Video, Some(false)) => Codec::Mpeg2Video,
            (codec, _) => codec,
        };
        // The sequence header may have told MPEG-1 from MPEG-2 video, which
        // the stream type gets wrong: the allowlist judges the codec the
        // stream turned out to carry, not the one its type named. A codec
        // this crate cannot name is not the allowlist's to judge.
        if codec != Codec::Other && !allowlist.allows(codec) {
            if allowlist.is_strict() {
                return Err(Error::CodecNotAllowed {
                    pid: entry.pid,
                    codec,
                });
            }
            dropped.push(DroppedStream {
                stream_index: index,
                pid: entry.pid,
                stream_type: entry.stream_type,
                codec,
                found_at: found(entry),
            });
            continue;
        }
        streams.push(Stream {
            stream_index: index,
            pid: entry.pid,
            programs: entry.programs.clone(),
            stream_type: entry.stream_type,
            codec,
            kind: codec.kind(),
            pmt_index: entry.pmt_index,
            descriptors: entry.descriptors.clone(),
            format_identifier: entry.format_identifier,
            language,
            video: learn.and_then(|l| l.video),
            audio: learn.and_then(|l| l.audio),
            config: learn.and_then(|l| l.config.clone()),
            profile: record.and_then(|r| r.profile).or_else(|| {
                matches!(entry.codec, Codec::Aac | Codec::AacLatm)
                    .then(|| learn.and_then(|l| l.config.as_deref()))
                    .flatten()
                    .and_then(describe_audio_specific_config)
            }),
            color: match entry.codec {
                // MPEG-1 and MPEG-2 video samples use the limited range
                // (ISO/IEC 11172-2 2.4.3.2, 13818-2 6.3.3); the sequence
                // display extension names the colours when there is one.
                Codec::Mpeg1Video | Codec::Mpeg2Video => {
                    learn.and_then(|l| l.colour).unwrap_or(Color {
                        full_range: Some(false),
                        ..Color::default()
                    })
                }
                _ => record.map(|r| r.color).unwrap_or_default(),
            },
            mpeg2_profile: learn.and_then(|l| l.mpeg2_profile),
            start,
            end: ends.get(index).copied().flatten(),
            last_pts: lasts.get(index).copied().flatten(),
            found_at: found(entry),
            with_first_table: with_first_table(entry),
        });
    }
    let programs = tail
        .programs
        .iter()
        .map(|p| Program {
            pmt_read: p.pmt_version.is_some(),
            number: p.number,
            pmt_pid: p.pmt_pid,
            pcr_pid: p.pcr_pid.unwrap_or(0x1FFF),
            pids: p.pids.clone(),
        })
        .collect();
    Ok(TransportStream {
        packet_size,
        first_packet,
        size: len,
        transport_stream_id: tail.transport_stream_id,
        services: tail.services.clone(),
        services_first: demux.sdt_before_pat == Some(true),
        programs,
        streams,
        dropped,
        warnings,
        wrap_reference: reference,
        program_reference,
        allowlist: allowlist.clone(),
    })
}

/// The 90 kHz duration of the first audio frame that starts in a PES
/// payload: a
/// stream ends at its last timestamp plus the access unit it times, as
/// `ffprobe` estimates durations from the end of a transport stream.
/// `None` for codecs without frames, or a payload that does not start
/// with one.
pub(crate) fn pes_span(codec: Codec, payload: &[u8], stream_rate: Option<u32>) -> Option<i64> {
    // The PTS times the first frame that starts in the PES packet, after
    // the end of one that started in the previous packet.
    let syntax = codec.frames()?;
    let frame = (0..payload.len()).find_map(|at| frames::header(syntax, payload.get(at..)?))?;
    let rate = Some(frame.sample_rate).filter(|r| *r > 0).or(stream_rate)?;
    (rate > 0).then(|| i64::from(frame.samples) * 90_000 / i64::from(rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_correction() {
        let near = (1u64 << 33) - 90_000; // one second before the wrap
        assert_eq!(unwrap(near, near), -90_000);
        assert_eq!(unwrap(5_000, near), 5_000);
        let far = (1u64 << 33) - 70 * 90_000;
        assert_eq!(unwrap(far, far), far as i64);
        assert_eq!(unwrap(5_000, far), 5_000 + (1 << 33));
        // Near zero: a timestamp just before the wrap is earlier than the
        // first.
        assert_eq!(unwrap((1 << 33) - 100, 1_000), -100);
        assert_eq!(unwrap(2_000, 1_000), 2_000);
    }
}
