//! Sections of an MPEG transport stream from `coreav-mpegts`.
//!
//! The probe ([`coreav_mpegts::probe`]) gives the programs, the streams
//! with what their tables and first PES packets say, and their times from
//! the head and the tail of the file. What `ffprobe` reports beyond that
//! (the pixel format, the frame rates, the reordering depth, the bit rates
//! audio frame headers state, the extradata size) the stream states in its
//! first access units, which [`head`] reads from the start of the file,
//! and [`params`] parses. A value the stream does not state is printed as
//! unknown, never guessed.
//!
//! # What `ffprobe` was seen to do
//!
//! Each rule below was found black box, on the non-holdout real transport
//! streams of the v1-ts-audio corpus and on streams crafted with the
//! pinned `ffmpeg` and remultiplexed (the function that applies it has
//! the evidence):
//!
//! - Packets. A PES payload is read in pieces of 204,800 bytes as they
//!   arrive (`coreav-mpegts`); the later units of a PES packet, and the
//!   pictures of one that states no time, take no time; a second field
//!   takes its first field's times one tick later ([`field_times`]). An
//!   H.264 picture lasts its ticks, rounded down ([`Clock::duration`]), a
//!   stream without a clock the period of its `r_frame_rate`. Decoding
//!   times are rebuilt from presentation times only for a stream that
//!   reorders one picture or never states one ([`DecodeTimes`]). Audio
//!   frame times follow a grid of frames for AAC, the durations summed for
//!   AC-3 and the samples rounded once for MPEG audio (`coreav-mpegts`).
//! - Streams. `has_b_frames` of H.264 without `bitstream_restriction` is
//!   what the order of its first frames shows within what `ffprobe`
//!   decodes ([`Order`]); `avg_frame_rate` counts frames, not fields, and
//!   is `0/0` for H.264 without a clock; a stream ends one `r_frame_rate`
//!   period after its latest time (H.264, [`reported_end`]), from the last
//!   250,000 bytes of the file, or 500,000 for a stream not found there
//!   ([`TAIL_BYTES`]); a stream whose end is not found has no duration.
//!   H.264 never reports closed captions. HE-AAC is read from the SBR data
//!   that ends its frames ([`params::aac_ends_with_sbr`]); Blu-ray LPCM
//!   from its frame header ([`params::bluray_lpcm`]); MPEG-2 video's bit
//!   rate is the header's unless the stream is of variable rate.
//! - The file. Its duration is the latest end less the earliest start in
//!   microseconds, each stream's start and duration rounded on their own
//!   ([`file_micros`]). A service the SDT names alone is a program
//!   ([`programs`]); DVB strings print as [`dvb_text`] has it.

mod params;

use super::model::{Unit, Value, s, time};
use super::names;
use super::{Entries, Packet, PacketSource, Probed, Program, Report, SideData, Stream, StreamKind};
use crate::compat::ratio;
use crate::error::Failure;
use crate::mpegts as native;
use coreav_isobmff::Source;
use coreav_mpegts::{
    Allowlist, Codec, Error, Limits, PacketSize, Service, StreamKind as Kind, TransportStream,
};
use params::{Mpeg2, Sps};

/// `ffprobe`'s format names for every transport stream.
const FORMAT_NAME: &str = "mpegts";
const FORMAT_LONG_NAME: &str = "MPEG-TS (MPEG-2 Transport Stream)";

/// The score `ffprobe` gives the transport streams it reads.
const PROBE_SCORE: i64 = 50;

/// Transport stream timestamps count 90 kHz ticks (ISO/IEC 13818-1
/// 2.4.3.7), and `ffprobe`'s time base for every stream is that clock.
const TIMESCALE: i64 = 90_000;

/// How many access units of each stream the head keeps the times of: enough
/// for the step between pictures to show through any reordering.
const HEAD_UNITS: usize = 64;

/// How far into the file the head reads at most, past the first packet.
const HEAD_BYTES: u64 = 8 << 20;

/// How far into the file the head looks for the service names the probe
/// did not find, past the first packet.
const SDT_WAIT: u64 = 2 << 20;

/// How far from the end of the file `ffprobe` looks for the last
/// timestamps of the streams, and how many times it looks again, twice as
/// far, for the streams it found none of: on files crafted with one
/// stream ending further and further from the end, `ffprobe` finds the
/// stream's last PES packet 469,060 bytes from the end and not 500,080,
/// and a video stream of the corpus ends at a packet 98,880 bytes from the
/// end rather than at its latest picture, 259,200 bytes from it. A stream
/// that has an end keeps the one the shorter tail gave.
const TAIL_BYTES: u64 = 250_000;
const TAIL_RETRIES: u32 = 1;

/// The largest access unit the head keeps the bytes of.
const MAX_KEPT_UNIT: usize = 1 << 20;

/// Probes `source` as a transport stream.
pub fn probe<'a, S: Source + 'a>(
    mut source: S,
    size: u64,
    name: &str,
) -> Result<Probed<'a>, Failure> {
    let limits = Limits {
        max_tail_bytes: TAIL_BYTES,
        tail_retries: TAIL_RETRIES,
        ..Limits::default()
    };
    let ts = coreav_mpegts::probe(&mut source, &limits, &Allowlist::default())
        .map_err(Failure::Mpegts)?;
    let head = head(&ts, &mut source, &limits)?;
    let mut report = Report::default();
    // `ffprobe` numbers the streams in the order the file shows them once
    // a program is known: a table that names them, or their data
    // (KD-TS-STREAM-ORDER); the probe's order breaks ties.
    let mut order: Vec<usize> = (0..ts.streams.len()).collect();
    order.sort_by_key(|&i| {
        ts.streams
            .get(i)
            .map_or((u64::MAX, usize::MAX), |st| (st.found_at, st.stream_index))
    });
    let facts: Vec<Facts> = ts
        .streams
        .iter()
        .zip(&head.streams)
        .map(|(st, h)| Facts::read(st, h))
        .collect();
    let ends: Vec<Option<i64>> = ts
        .streams
        .iter()
        .zip(&facts)
        .map(|(st, f)| reported_end(st, f))
        .collect();
    let file = FileTiming {
        start: ts.start(),
        span: file_span(&ts, &ends),
        micros: file_micros(&ts, &ends),
    };
    // The probe's streams, indexed as the listing indexes them.
    let mut listed: Vec<Listed> = ts
        .streams
        .iter()
        .map(|st| Listed {
            index: st.stream_index,
            ticks: None,
            audio: st.kind == Kind::Audio,
            dvb_subtitle: st.codec == Codec::DvbSubtitle,
            reorder: None,
            clock: None,
            lpcm: None,
        })
        .collect();
    for (rank, &i) in order.iter().enumerate() {
        let (Some(st), Some(h), Some(facts)) =
            (ts.streams.get(i), head.streams.get(i), facts.get(i))
        else {
            continue;
        };
        if let Some(slot) = listed.get_mut(i) {
            slot.index = rank;
            slot.ticks = facts.frame_ticks;
            slot.lpcm = facts.lpcm;
            // A video stream nothing configures: its packets last one
            // period of the time base `ffprobe` reports as its rate.
            if facts.unconfigured
                && matches!(st.codec, Codec::H264 | Codec::Hevc | Codec::Mpeg2Video)
            {
                slot.ticks = Some(1);
            }
            if st.codec == Codec::H264 && facts.sps.is_some() {
                slot.clock = facts.sps.map(Clock::of);
                // A stream that states no clock: `ffprobe` gives every
                // picture the period of the rate it reports.
                slot.ticks = facts
                    .rate
                    .filter(|_| slot.clock.is_some_and(|c| c.tick.is_none()))
                    .and_then(|(num, den)| {
                        i64::try_from(den.checked_mul(90_000)?.checked_div(num)?).ok()
                    });
            }
            // A stream that reorders two pictures or more and states
            // decoding times of its own keeps the presentation time as the
            // decoding time of a picture whose PES packet states none; one
            // that reorders one picture, or never states one, has it
            // rebuilt (streams crafted with the first PES packets stating
            // no decoding time and the others stating theirs, reordering
            // one picture and two).
            slot.reorder = matches!(st.codec, Codec::H264 | Codec::Hevc)
                .then(|| has_b_frames(st, facts))
                .flatten()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n <= MAX_REORDER)
                .filter(|n| *n <= 1 || !h.stated_dts);
        }
        let end = ends.get(i).copied().flatten();
        let mut times = timing(st, end, &file);
        // `ffprobe` finds no end of a video stream it cannot decode, and
        // gives it the file's duration (an MVC sub-bitstream).
        if facts.unconfigured
            && times.0.is_some()
            && matches!(st.codec, Codec::H264 | Codec::Hevc | Codec::Mpeg2Video)
        {
            times.1 = file.span;
        }
        report.streams.push(stream(&ts, st, rank, h, facts, times));
        report.time_bases.push((1, TIMESCALE));
    }
    let services = if ts.services.is_empty() {
        head.services
    } else {
        ts.services.clone()
    };
    report.programs = programs(&ts, &order, &services);
    report.format = format(&ts, &file, size, name, report.programs.len());
    Ok(Probed {
        report,
        packets: Box::new(TsPackets { ts, source, listed }),
    })
}

/// The file's start and the span `ffprobe` gives it, 90 kHz ticks.
struct FileTiming {
    start: Option<i64>,
    span: Option<i64>,
    /// The duration `ffprobe` reports for the file ([`file_micros`]).
    micros: Option<i64>,
}

/// Where `ffprobe` ends a stream: where its last access unit ends, except
/// that it adds none of that unit for E-AC-3 and Opus and half of it for
/// MPEG-1 video (`docs/known-divergences-ts.md`, KD-TS-END-ESTIMATE). An
/// H.264 stream ends one period of the base rate `ffprobe` reports for it
/// (`r_frame_rate`, [`Facts::rate`]) after its latest presentation time,
/// rounded down: on every H.264 stream of the corpus that the tail times,
/// whatever its pictures' own durations -- a field of an interlaced
/// capture, a frame its timing message shows twice, a frame of 3,749.994
/// ticks whose packet lasts 3,749 and whose stream ends 3,750 after it at
/// 24 frames a second, or a stream that states no clock.
fn reported_end(st: &coreav_mpegts::Stream, facts: &Facts) -> Option<i64> {
    if let (Codec::H264, Some(last), Some((num, den))) = (st.codec, st.last_pts, facts.rate) {
        if let Some(period) = den
            .checked_mul(90_000)
            .and_then(|n| n.checked_div(num))
            .and_then(|p| i64::try_from(p).ok())
        {
            return Some(last + period);
        }
    }
    let end = st.end?;
    Some(match (st.codec, st.last_pts) {
        (Codec::Eac3 | Codec::Opus, Some(last)) => last,
        (Codec::Mpeg1Video, Some(last)) => last + (end - last) / 2,
        _ => end,
    })
}

/// The span of the file from its streams' starts and `ends`: from the
/// earliest start to the latest end, or for a file of several programs
/// the longest program's span, as [`TransportStream::duration`] takes it.
fn file_span(ts: &TransportStream, ends: &[Option<i64>]) -> Option<i64> {
    file_bounds(ts, ends).map(|(start, end)| end - start)
}

/// The start and end [`file_span`] spans: the longest program's, or the
/// file's.
fn file_bounds(ts: &TransportStream, ends: &[Option<i64>]) -> Option<(i64, i64)> {
    let span = |of: &dyn Fn(&coreav_mpegts::Stream) -> bool| {
        let streams = || ts.streams.iter().zip(ends).filter(|(st, _)| of(st));
        let start = streams().filter_map(|(st, _)| st.start).min()?;
        let end = streams().filter_map(|(_, end)| *end).max()?;
        Some((start, end)).filter(|(s, e)| e >= s)
    };
    if ts.programs.len() > 1 {
        return ts
            .programs
            .iter()
            .filter_map(|p| span(&|st| st.programs.contains(&p.number)))
            .max_by_key(|(s, e)| e - s);
    }
    span(&|_| true)
}

/// The file's duration as `ffprobe` keeps it, in microseconds: each
/// stream's start and duration are taken to microseconds on their own,
/// its end is their sum, and the file lasts from the earliest start to
/// the latest end (for a file of several programs, the longest program).
/// On the corpus a span of 618,970 ticks (6,877,444.4 us) lasts 6,877,445
/// so, and one of 359,040 ticks (3,989,333.3 us) 3,989,333.
fn file_micros(ts: &TransportStream, ends: &[Option<i64>]) -> Option<i64> {
    let span = |of: &dyn Fn(&coreav_mpegts::Stream) -> bool| {
        let streams = || ts.streams.iter().zip(ends).filter(|(st, _)| of(st));
        let start = streams().filter_map(|(st, _)| micros_of(st.start?)).min()?;
        let end = streams()
            .filter_map(|(st, end)| {
                let begin = st.start?;
                Some(micros_of(begin)? + micros_of((*end)? - begin)?)
            })
            .max()?;
        Some(end - start).filter(|d| *d > 0)
    };
    if ts.programs.len() > 1 {
        return ts
            .programs
            .iter()
            .filter_map(|p| span(&|st| st.programs.contains(&p.number)))
            .max();
    }
    span(&|_| true)
}

/// `ticks` of 90 kHz in microseconds, to the nearest, halves away from
/// zero.
fn micros_of(ticks: i64) -> Option<i64> {
    let scaled = i128::from(ticks).unsigned_abs().checked_mul(1_000_000)?;
    let rounded = (scaled + 45_000) / 90_000;
    let rounded = i64::try_from(rounded).ok()?;
    Some(if ticks < 0 { -rounded } else { rounded })
}

/// A stream's start and duration, 90 kHz ticks. A stream the file times
/// nowhere takes the file's start and span, as `ffprobe` fills them in
/// (KD-TS-STREAM-TIMING); one whose end the tail does not show has no
/// duration, as `ffprobe` reports on files crafted with a stream that
/// ends early.
fn timing(
    st: &coreav_mpegts::Stream,
    end: Option<i64>,
    file: &FileTiming,
) -> (Option<i64>, Option<i64>) {
    let Some(start) = st.start else {
        return (file.start, file.span);
    };
    // A stream that ends where it starts (one unit of a stream whose
    // units state no duration) has no duration either.
    let duration = end.map(|e| e - start).filter(|d| *d > 0);
    (Some(start), duration)
}

/// The clock of an H.264 stream's pictures: its sequence parameter set,
/// which says how to read a picture's slice header and timing message,
/// and the tick it states (`num_units_in_tick / time_scale`, E.2.1).
#[derive(Clone, Copy)]
struct Clock {
    sps: Sps,
    tick: Option<(u32, u32)>,
}

impl Clock {
    fn of(sps: Sps) -> Self {
        Self {
            sps,
            tick: sps.vui.timing,
        }
    }

    fn picture(&self, data: &[u8]) -> Option<params::Picture> {
        params::h264_picture(data, &self.sps)
    }

    /// `ticks` of the clock in 90 kHz ticks, rounded down, as `ffprobe`
    /// gives a picture's duration: a field of 59.94 Hz video lasts
    /// 1,501.5 ticks, and `ffprobe` lists 1,501.
    fn duration(&self, ticks: u32) -> Option<i64> {
        let (units, scale) = self.tick?;
        let num = u64::from(units)
            .checked_mul(u64::from(ticks))?
            .checked_mul(90_000)?;
        i64::try_from(num.checked_div(u64::from(scale))?)
            .ok()
            .filter(|d| *d > 0)
    }

    /// How far after the first field of a frame `ffprobe` places the
    /// second when the file times only the first: one tick, rounded to
    /// the nearest 90 kHz tick, halves up (1,501.5 ticks is 1,502).
    fn field_step(&self) -> Option<i64> {
        let (units, scale) = self.tick?;
        let scale = u64::from(scale);
        let num = u64::from(units).checked_mul(180_000)?.checked_add(scale)?;
        i64::try_from(num.checked_div(scale.checked_mul(2)?)?)
            .ok()
            .filter(|d| *d > 0)
    }
}

/// The first field of a frame, while its second is awaited: its parity
/// and its times.
type OpenField = Option<(bool, Option<i64>, Option<i64>)>;

/// The times of an H.264 picture the file gives none of its own (one
/// after the first in its PES packet, or the first of a PES packet that
/// states no time): a second field takes the first field's times one
/// [`Clock::field_step`] later, as `ffprobe` gives them on every
/// interlaced capture of the v1-ts-audio corpus, in the first field's PES
/// packet or in one of its own; any other such picture has none, as
/// `ffprobe` reports on streams crafted with two frames to a PES packet
/// and with frames in PES packets that state no time. `open` tracks the
/// first field of a frame.
fn field_times(
    clock: &Clock,
    picture: Option<params::Picture>,
    untimed: bool,
    times: (Option<i64>, Option<i64>),
    open: &mut OpenField,
) -> (Option<i64>, Option<i64>) {
    let field = picture.and_then(|p| p.field);
    if untimed {
        if let (Some(bottom), Some((first, pts, dts))) = (field, *open) {
            if bottom != first {
                *open = None;
                let step = clock.field_step();
                let later = |t: Option<i64>| t.zip(step).map(|(t, s)| t + s);
                return (later(pts), later(dts));
            }
        }
    }
    *open = field.map(|bottom| (bottom, times.0, times.1));
    times
}

/// What the first access units of one stream show.
#[derive(Default)]
struct Head {
    /// The decoding time (the presentation time when there is none) of
    /// each of the first [`HEAD_UNITS`] access units.
    times: Vec<Option<i64>>,
    /// How many ticks of its clock each of those units is shown for
    /// ([`params::Picture::ticks`]), for an H.264 stream.
    ticks: Vec<u32>,
    /// The clock of an H.264 stream, once its parameter sets were read.
    clock: Option<Clock>,
    /// The first field of a frame whose second is awaited.
    field: OpenField,
    /// The first access unit that carries the stream's configuration: the
    /// parameter sets of a coded video stream, the sequence header of
    /// MPEG video, the first frame of audio.
    unit: Option<Vec<u8>>,
    /// A picture carried ATSC A/53 closed captions.
    captions: bool,
    /// The parity of a field whose second field is awaited, for telling
    /// the frames of an H.264 stream from their second fields.
    pairing: Option<bool>,
    /// The order of an H.264 stream's frames.
    order: Order,
    /// A PES packet of the stream states a decoding time other than its
    /// presentation time.
    stated_dts: bool,
    /// For an AAC stream: of the first [`SBR_FRAMES`] frames, how many
    /// were read and how many end with SBR data
    /// ([`params::aac_ends_with_sbr`]).
    sbr: (u32, u32),
    /// For an ADTS stream: the bit rate `ffprobe` reports and how many
    /// frames it averages ([`Analysis`]).
    aac_rate: (i64, i64),
}

/// How many frames of an AAC stream are read for SBR data.
const SBR_FRAMES: u32 = 16;

/// The presentation times of the first frames of an H.264 stream, in
/// decoding order, from which [`Order::reorder`] reads how many frames
/// its decoder holds back when its sequence parameter set does not say
/// (no `bitstream_restriction`, E.1.1).
#[derive(Default)]
struct Order {
    /// Each frame's presentation time, decoding time (the presentation
    /// time when there is none), and the position of its PES packet.
    frames: Vec<(i64, i64, u64)>,
    /// The first frame's decoding time and position.
    first: Option<(i64, u64)>,
    /// The frames read cover what `ffprobe` decodes of the stream before
    /// it reports it: the head read [`ORDER_SPAN`] of the stream or
    /// [`ORDER_BYTES`] of the file past its first frame, or all of it.
    covered: bool,
}

/// What `ffprobe` decodes of a stream to report it is bounded by five
/// seconds of the stream and five million bytes of the file: on streams
/// crafted with frames that are reordered only from a given frame on, it
/// reports reordering that starts 4.8 seconds in at 25 frames a second
/// and not 5.2, 5 seconds in at 50 and not 6, and at 30 Mb/s 3.6 MB into
/// the file and not 5.4. [`Order::reorder`] reads up to [`ORDER_SPAN`] and
/// [`ORDER_BYTES`] and trusts what it finds only when the frames within
/// [`ORDER_INSIDE`] and [`ORDER_INSIDE_BYTES`] show the same, so that a
/// boundary not known to the tick is never guessed across.
const ORDER_SPAN: i64 = 6 * 90_000;
const ORDER_BYTES: u64 = 6_000_000;
const ORDER_INSIDE: i64 = 9 * 45_000;
const ORDER_INSIDE_BYTES: u64 = 4_500_000;

/// Most frames [`Order`] keeps.
const ORDER_FRAMES: usize = 4096;

impl Order {
    fn note(&mut self, pts: Option<i64>, dts: Option<i64>, offset: u64) {
        if self.covered {
            return;
        }
        let Some(pts) = pts else {
            return;
        };
        let dts = dts.unwrap_or(pts);
        let (first_dts, first_offset) = *self.first.get_or_insert((dts, offset));
        if dts - first_dts >= ORDER_SPAN || offset.saturating_sub(first_offset) >= ORDER_BYTES {
            self.covered = true;
            return;
        }
        if self.frames.len() >= ORDER_FRAMES {
            // So many frames in so little time: no window is known.
            self.frames.clear();
            self.first = None;
            self.covered = false;
            return;
        }
        self.frames.push((pts, dts, offset));
    }

    /// How many frames the decoder of a stream that does not state it
    /// holds back, as `ffprobe` reports it (`has_b_frames`): the most
    /// frames decoded before one and presented after it. Streams crafted
    /// without `bitstream_restriction` show `ffprobe` finding that depth
    /// when the first reordered frame comes within the first seven, and
    /// only one when it comes later; so a depth above one counts only when
    /// a frame among the first four is reordered, and is unknown
    /// otherwise.
    fn reorder(&self) -> Option<u32> {
        if !self.covered {
            return None;
        }
        let Some((first_dts, first_offset)) = self.first else {
            return Some(0);
        };
        let depth = |inside: bool| {
            let frames: Vec<i64> = self
                .frames
                .iter()
                .filter(|(_, dts, offset)| {
                    !inside
                        || (dts - first_dts < ORDER_INSIDE
                            && offset.saturating_sub(first_offset) < ORDER_INSIDE_BYTES)
                })
                .map(|(pts, _, _)| *pts)
                .collect();
            let mut deepest = 0usize;
            let mut first_late = None;
            for (k, pts) in frames.iter().enumerate() {
                let earlier = frames
                    .get(k.saturating_sub(MAX_REORDER)..k)
                    .unwrap_or_default();
                let late = earlier.iter().filter(|t| *t > pts).count();
                if late > 0 && first_late.is_none() {
                    first_late = Some(k);
                }
                deepest = deepest.max(late);
            }
            (deepest, first_late)
        };
        let (inside, first_late) = depth(true);
        let (all, _) = depth(false);
        if inside != all || (all >= 2 && first_late.is_none_or(|k| k > 3)) {
            return None;
        }
        u32::try_from(all).ok()
    }
}

/// The part of the file `ffprobe` analyses before it reports the streams,
/// as far as the bit rate of an ADTS stream shows it: whole PES packets in
/// file order, up to and including the first after which some stream's
/// latest decoding time lies more than [`ANALYSIS_SPAN`] past the
/// presentation time of its first timed frame. On every AAC transport
/// stream of the fixtures and on 11 of 14 AAC files of the development
/// set, the bit rate `ffprobe` reports is the running mean of the bit
/// rates of the ADTS frames listed in that part (each frame's
/// `frame_length` bytes over 1024 samples at its sampling rate),
/// truncated to an integer at every frame as C's division does; files
/// crafted with frames of chosen sizes show it frame by frame.
#[derive(Default)]
struct Analysis {
    /// Per stream: the presentation time of its first timed frame and the
    /// decoding time of its latest one.
    spans: Vec<Option<(i64, i64)>>,
    /// The stream of the PES packet being read, and where it starts.
    current: Option<(usize, u64)>,
    /// The analysis ended.
    over: bool,
}

/// How far the analysis reads into a stream, in 90 kHz ticks: past five
/// seconds, strictly.
const ANALYSIS_SPAN: i64 = 5 * 90_000;

/// How far the analysis reads into the file: it ends after the first PES
/// packet that starts past five million bytes (a 35 Mb/s stream ends
/// there, 1 s in, and not after five seconds).
const ANALYSIS_BYTES: u64 = 5_000_000;

impl Analysis {
    /// Notes a frame of `stream`, the first of its PES packet when
    /// `starts` is set, and tells whether it is part of the analysis.
    fn note(
        &mut self,
        stream: usize,
        offset: u64,
        starts: bool,
        pts: Option<i64>,
        dts: Option<i64>,
    ) -> bool {
        if self.over {
            return false;
        }
        let new = starts || self.current.is_none_or(|(s, _)| s != stream);
        if let Some((_, at)) = self.current.filter(|_| new) {
            let past = at > ANALYSIS_BYTES
                || self
                    .spans
                    .iter()
                    .flatten()
                    .any(|(first, last)| last - first > ANALYSIS_SPAN);
            if past {
                self.over = true;
                return false;
            }
        }
        if new {
            self.current = Some((stream, offset));
        }
        if self.spans.len() <= stream {
            self.spans.resize(stream + 1, None);
        }
        if let (Some(dts), Some(span)) = (dts, self.spans.get_mut(stream)) {
            let first = span.map_or(pts.unwrap_or(dts), |(first, _)| first);
            *span = Some((first, dts));
        }
        true
    }
}

/// The bit rate of an ADTS frame, as `ffprobe` averages it
/// ([`Analysis`]), when `data` holds the whole frame.
fn adts_bit_rate(data: &[u8]) -> Option<i64> {
    let [0xFF, b1, b2, b3, b4, b5, ..] = *data else {
        return None;
    };
    if b1 & 0xF6 != 0xF0 {
        return None;
    }
    let rate = params::AAC_RATES.get(usize::from((b2 >> 2) & 0x0F))?;
    let len = (usize::from(b3 & 0x03) << 11) | (usize::from(b4) << 3) | usize::from(b5 >> 5);
    if data.len() < len {
        return None;
    }
    i64::try_from(len * 8)
        .ok()?
        .checked_mul(i64::from(*rate))
        .map(|b| b / 1024)
}

/// What the head of the file shows, per stream of the probe.
struct HeadRead {
    streams: Vec<Head>,
    services: Vec<Service>,
}

/// Whether `data` carries the configuration of a stream of `codec`.
fn configures(codec: Codec, data: &[u8]) -> bool {
    match codec {
        Codec::H264 => params::annex_b(data)
            .iter()
            .any(|(_, nal)| nal.first().is_some_and(|b| b & 0x1F == 7)),
        Codec::Hevc => params::annex_b(data)
            .iter()
            .any(|(_, nal)| nal.first().is_some_and(|b| (b >> 1) & 0x3F == 33)),
        Codec::Mpeg1Video | Codec::Mpeg2Video => data.windows(4).any(|w| w == [0, 0, 1, 0xB3]),
        _ => !data.is_empty(),
    }
}

/// The first sequence parameter set of an H.264 access unit.
fn h264_sps(unit: &[u8]) -> Option<Sps> {
    params::find_nal(unit, |nal| nal.first().is_some_and(|b| b & 0x1F == 7))
        .and_then(|nal| params::h264_sps(&nal))
}

/// ATSC A/53 closed captions in a picture: the `GA94` identifier of
/// their user data, followed by `user_data_type_code` 3 (A/53 part 4
/// 6.2.3).
fn has_captions(data: &[u8]) -> bool {
    data.windows(5).any(|w| w == *b"GA94\x03")
}

/// Reads the first access units of every stream, from the start of the
/// file, until each stream has shown [`HEAD_UNITS`] of them and its
/// configuration, or [`HEAD_BYTES`] are read.
fn head<S: Source>(
    ts: &TransportStream,
    source: &mut S,
    limits: &Limits,
) -> Result<HeadRead, Failure> {
    let mut streams: Vec<Head> = ts.streams.iter().map(|_| Head::default()).collect();
    let end = ts.first_packet.saturating_add(HEAD_BYTES);
    let mut analysis = Analysis::default();
    let adts = ts.streams.iter().any(|st| st.codec == Codec::Aac);
    let mut packets = ts.packets(&mut *source, limits).listing_as_ffprobe();
    // Whether the head read to the end of the file or of its budget, which
    // covers what `ffprobe` decodes of every stream ([`Order`]).
    let mut whole = true;
    while let Some(item) = packets.next() {
        let packet = match item {
            Ok(packet) => packet,
            Err(Error::Io(kind)) => return Err(Failure::Mpegts(Error::Io(kind))),
            Err(_) => {
                whole = false;
                break;
            }
        };
        if packet.pes_offset > end {
            break;
        }
        let Some(st) = ts.streams.get(packet.stream) else {
            continue;
        };
        let analysed = analysis.note(
            packet.stream,
            packet.pes_offset,
            packet.pos.is_some(),
            packet.pts,
            packet.dts,
        );
        if let Some(h) = streams.get_mut(packet.stream) {
            if analysed && st.codec == Codec::Aac {
                if let Some(rate) = adts_bit_rate(&packet.data) {
                    let (mean, n) = &mut h.aac_rate;
                    *n += 1;
                    *mean += (rate - *mean) / *n;
                }
            }
            let keep = h.unit.is_none()
                && packet.data.len() <= MAX_KEPT_UNIT
                && configures(st.codec, &packet.data);
            if keep && st.codec == Codec::H264 {
                h.clock = h264_sps(&packet.data).map(Clock::of);
            }
            if packet.dts.is_some() && packet.dts != packet.pts {
                h.stated_dts = true;
            }
            let mut times = (packet.pts, packet.dts);
            let mut ticks = None;
            if let Some(clock) = h.clock {
                let picture = clock.picture(&packet.data);
                let untimed = packet.pts.is_none() && packet.dts.is_none();
                times = field_times(&clock, picture, untimed, times, &mut h.field);
                ticks = Some(picture.map_or(2, params::Picture::ticks));
                // A frame, or the first field of one, is a frame of the
                // order; a second field is not.
                let field = picture.and_then(|p| p.field);
                let second = matches!((field, h.pairing), (Some(b), Some(f)) if b != f);
                h.pairing = if second { None } else { field };
                if !second {
                    h.order.note(packet.pts, packet.dts, packet.pes_offset);
                }
            }
            if st.codec == Codec::Aac && h.sbr.0 < SBR_FRAMES {
                h.sbr.0 += 1;
                h.sbr.1 += u32::from(params::aac_ends_with_sbr(&packet.data));
            }
            if h.times.len() < HEAD_UNITS {
                h.ticks.extend(ticks);
                h.times.push(times.1.or(times.0));
                if st.kind == Kind::Video && has_captions(&packet.data) {
                    h.captions = true;
                }
            }
            if keep {
                h.unit = Some(packet.data);
            }
        }
        // Subtitle and data streams state nothing the head reads. An
        // H.264 stream that does not state how many frames it reorders
        // is read until the order of its frames shows it.
        let done = streams.iter().zip(&ts.streams).all(|(h, st)| {
            !matches!(st.kind, Kind::Video | Kind::Audio)
                || st.codec == Codec::Other
                || (h.times.len() >= HEAD_UNITS
                    && h.unit.is_some()
                    && (h.order.covered || h.clock.is_none_or(|c| c.sps.vui.reorder.is_some())))
        });
        // The probe read the head and the tail of the file for the service
        // names; when it found none, the head waits a little longer for
        // them.
        let named = !ts.services.is_empty() || !packets.services().is_empty();
        let done = done && (analysis.over || !adts);
        if done && (named || packet.pes_offset > ts.first_packet.saturating_add(SDT_WAIT)) {
            whole = false;
            break;
        }
    }
    if whole {
        for h in &mut streams {
            h.order.covered = true;
        }
    }
    let services = packets.services().to_vec();
    Ok(HeadRead { streams, services })
}

/// What a stream's own bytes state, as far as they were read.
#[derive(Default)]
struct Facts {
    sps: Option<Sps>,
    mpeg: Option<Mpeg2>,
    /// The bytes `ffprobe` keeps as the stream's extradata.
    extradata: Option<usize>,
    /// The duration of one picture, in 90 kHz ticks, from the stream's own
    /// timing.
    frame_ticks: Option<i64>,
    /// The picture rate the times of the first pictures show.
    rate: Option<(u64, u64)>,
    /// How many access units the head saw.
    units: usize,
    /// For an H.264 stream that states its clock: how many frames the
    /// pictures the head saw are shown for, and how many pictures they
    /// are ([`params::Picture::ticks`], two ticks a frame).
    frames: Option<(u64, u64)>,
    /// For Blu-ray LPCM: what its first frame's header states.
    lpcm: Option<params::Lpcm>,
    /// For AAC: whether its frames carry SBR data, which `ffprobe` finds
    /// by decoding them and reports as HE-AAC at twice the rate of the
    /// AAC frames: every frame read or nearly does, none does, or not
    /// known.
    sbr: Option<bool>,
    /// For an H.264 stream: how many frames its decoder holds back, from
    /// the order of its first frames ([`Order::reorder`]).
    reordered: Option<u32>,
    /// The head found nothing that configures the stream.
    unconfigured: bool,
}

impl Facts {
    fn read(st: &coreav_mpegts::Stream, head: &Head) -> Self {
        let unit = head.unit.as_deref().unwrap_or_default();
        let mut facts = Facts {
            units: head.times.len(),
            rate: picture_rate(&head.times),
            ..Facts::default()
        };
        if head.clock.is_some_and(|c| c.tick.is_some()) && !head.ticks.is_empty() {
            let ticks: u64 = head.ticks.iter().map(|t| u64::from(*t)).sum();
            let pictures = u64::try_from(head.ticks.len()).unwrap_or(u64::MAX);
            facts.frames = Some((ticks, pictures.saturating_mul(2)));
        }
        facts.reordered = head.order.reorder();
        // Reading from the end of a block, a plain AAC frame shows what
        // looks like SBR data now and then (one frame in twenty of a
        // capture of the corpus): a stream counts as one or the other
        // when at most two frames of what was read disagree.
        let (read, with_sbr) = head.sbr;
        facts.sbr = if with_sbr == 0 || (read >= 8 && with_sbr <= 2) {
            Some(false)
        } else if read >= 8 && with_sbr + 2 >= read {
            Some(true)
        } else {
            None
        };
        facts.unconfigured = head.unit.is_none();
        match st.codec {
            Codec::H264 => {
                facts.sps = h264_sps(unit);
                facts.extradata = params::annex_b_extradata(unit, false);
                facts.frame_ticks = facts
                    .sps
                    .and_then(|sps| sps.vui.timing)
                    .and_then(|(units, scale)| exact_ticks(2 * u64::from(units), u64::from(scale)));
            }
            Codec::Hevc => {
                facts.sps = params::find_nal(unit, |nal| {
                    nal.first().is_some_and(|b| (b >> 1) & 0x3F == 33)
                })
                .and_then(|nal| params::hevc_sps(&nal));
                facts.extradata = params::annex_b_extradata(unit, true);
                facts.frame_ticks = facts
                    .sps
                    .and_then(|sps| sps.vui.timing)
                    .and_then(|(units, scale)| exact_ticks(u64::from(units), u64::from(scale)));
            }
            Codec::Mpeg1Video | Codec::Mpeg2Video => {
                facts.mpeg = params::mpeg2(unit);
                facts.extradata = params::mpeg2_extradata(unit);
                facts.frame_ticks = facts
                    .mpeg
                    .and_then(|m| mpeg_rate(&m))
                    .and_then(|(n, d)| exact_ticks(u64::from(d), u64::from(n)));
            }
            Codec::AacLatm => facts.extradata = st.config.as_ref().map(Vec::len),
            Codec::BlurayPcm => facts.lpcm = params::bluray_lpcm(unit),
            Codec::Opus => {
                facts.extradata = opus_channels(st).and_then(|channels| match channels {
                    1 | 2 => Some(19),
                    3..=8 => Some(21 + usize::from(channels)),
                    _ => None,
                });
            }
            // A teletext descriptor (EN 300 468 6.2.43): two bytes for each
            // of its five-byte entries (type and magazine, page), as
            // `ffprobe` reports for streams of one entry and of two.
            Codec::DvbTeletext => {
                facts.extradata = st
                    .descriptors
                    .iter()
                    .find(|d| d.tag == 0x56)
                    .map(|d| d.data.len())
                    .filter(|n| *n > 0 && n % 5 == 0)
                    .map(|n| n / 5 * 2);
            }
            // A subtitling descriptor of one entry; what `ffprobe` keeps of
            // several is not known.
            Codec::DvbSubtitle => {
                facts.extradata = st
                    .descriptors
                    .iter()
                    .find(|d| d.tag == 0x59)
                    .filter(|d| d.data.len() == 8)
                    .map(|_| 5);
            }
            _ => {}
        }
        facts
    }
}

/// `seconds * 90 000` ticks for a duration of `num / den` seconds, when it
/// is a whole number of ticks.
fn exact_ticks(num: u64, den: u64) -> Option<i64> {
    let scaled = num.checked_mul(90_000)?;
    if den == 0 || scaled % den != 0 {
        return None;
    }
    i64::try_from(scaled / den).ok().filter(|t| *t > 0)
}

/// The frame rate an MPEG-1 or MPEG-2 sequence states, extension included.
fn mpeg_rate(m: &Mpeg2) -> Option<(u32, u32)> {
    let (num, den) = params::mpeg2_frame_rate(m.frame_rate_code)?;
    let (n, d) = m.extension.map_or((0, 0), |e| e.frame_rate_extension);
    Some((num * (u32::from(n) + 1), den * (u32::from(d) + 1)))
}

/// Frame rates of the video in common use, as fractions, which a stream
/// whose pictures do not fall on whole ticks (23.976 frames a second is
/// 3,753.75 ticks a picture) is matched against.
const COMMON_RATES: [(u64, u64); 16] = [
    (24_000, 1001),
    (24, 1),
    (25, 1),
    (30_000, 1001),
    (30, 1),
    (48, 1),
    (50, 1),
    (60_000, 1001),
    (60, 1),
    (100, 1),
    (120, 1),
    (15, 1),
    (12, 1),
    (10, 1),
    (5, 1),
    (1, 1),
];

/// The picture rate the first pictures' times show, as a fraction: the
/// times in order, each gap counted as the whole number of shortest gaps
/// it spans (a lost picture leaves a gap of two), over the span they
/// cover. A rate within a thousandth of a common one is that one; any
/// other is kept only when its denominator is small, as a real clock's
/// is.
fn picture_rate(times: &[Option<i64>]) -> Option<(u64, u64)> {
    let mut known: Vec<i64> = times.iter().flatten().copied().collect();
    known.sort_unstable();
    known.dedup();
    let gaps: Vec<i64> = known
        .windows(2)
        .filter_map(|w| match *w {
            [a, b] => Some(b - a),
            _ => None,
        })
        .collect();
    let shortest = *gaps.iter().min()?;
    if shortest <= 0 {
        return None;
    }
    let pictures: i64 = gaps.iter().map(|g| (g + shortest / 2) / shortest).sum();
    let span = known.last()? - known.first()?;
    let num = u64::try_from(pictures).ok()?.checked_mul(90_000)?;
    let den = u64::try_from(span).ok().filter(|d| *d > 0)?;
    let g = crate::compat::gcd(num, den);
    let (n, d) = (num / g, den / g);
    if COMMON_RATES.contains(&(n, d)) {
        return Some((n, d));
    }
    for (common_n, common_d) in COMMON_RATES {
        let a = u128::from(num) * u128::from(common_d);
        let b = u128::from(common_n) * u128::from(den);
        if a.abs_diff(b) * 1000 <= b {
            return Some((common_n, common_d));
        }
    }
    (d <= 1001).then_some((n, d))
}

/// A rate of `num / den` per second as `ffprobe` prints one, reduced.
fn rate(num: u64, den: u64) -> Option<String> {
    if num == 0 || den == 0 {
        return None;
    }
    let g = crate::compat::gcd(num, den);
    Some(format!("{}/{}", num / g, den / g))
}

fn int(v: impl TryInto<i64>) -> Value {
    v.try_into().map_or(Value::Na, Value::Int)
}

fn unknown() -> Value {
    Value::Unset("unknown")
}

fn kind(st: &coreav_mpegts::Stream) -> StreamKind {
    match st.kind {
        Kind::Video => StreamKind::Video,
        Kind::Audio => StreamKind::Audio,
        Kind::Subtitle => StreamKind::Subtitle,
        Kind::Data => StreamKind::Data,
    }
}

/// `ffprobe`'s profile name.
fn profile(st: &coreav_mpegts::Stream) -> Option<String> {
    // MPEG-2 video's 4:2:2 profile is an escaped indication (ISO/IEC
    // 13818-2 8.2, table 8-3: 0x82 and 0x85).
    if let Some((8, 2 | 5)) = st.mpeg2_profile {
        return Some("4:2:2".to_owned());
    }
    native::profile_name(st)
}

/// The chroma sample location `ffprobe` names for a code of ITU-T H.273
/// (`chroma_sample_loc_type`).
fn chroma_location(code: u32) -> Option<&'static str> {
    Some(match code {
        0 => "left",
        1 => "center",
        2 => "topleft",
        3 => "top",
        4 => "bottomleft",
        5 => "bottom",
        _ => return None,
    })
}

/// The pixel format a decoder of a coded video stream puts out, as
/// `ffprobe` names it.
fn pix_fmt(sps: &Sps) -> Option<String> {
    let rgb = sps.vui.colour.is_some_and(|(_, _, matrix)| matrix == 0);
    let full = sps.vui.full_range == Some(true);
    let depth = sps.bit_depth;
    let base = match (sps.chroma_format_idc, rgb) {
        (3, true) => "gbrp",
        (0 | 1, _) => "yuv420p",
        (2, _) => "yuv422p",
        (3, false) => "yuv444p",
        _ => return None,
    };
    match depth {
        8 if full && base != "gbrp" => Some(base.replacen("yuv", "yuvj", 1)),
        8 => Some(base.to_owned()),
        9 | 10 | 12 | 14 | 16 => Some(format!("{base}{depth}le")),
        _ => None,
    }
}

/// The sample aspect ratio of MPEG-1 or MPEG-2 video.
fn mpeg_sar(m: &Mpeg2, mpeg1: bool, width: u32, height: u32) -> Option<(u64, u64)> {
    let (w, h) = (u64::from(width), u64::from(height));
    if w == 0 || h == 0 {
        return None;
    }
    match (mpeg1, m.aspect_ratio_information) {
        (_, 1) => Some((1, 1)),
        // MPEG-2 states the display aspect ratio (13818-2 table 6-3).
        (false, 2) => Some((4 * h, 3 * w)),
        (false, 3) => Some((16 * h, 9 * w)),
        (false, 4) => Some((221 * h, 100 * w)),
        _ => None,
    }
}

/// `ffprobe`'s `field_order` of MPEG-2 video.
fn mpeg_field_order(m: &Mpeg2) -> Value {
    let Some(ext) = m.extension else {
        // MPEG-1 video is progressive (ISO/IEC 11172-2).
        return s("progressive");
    };
    if ext.progressive_sequence {
        return s("progressive");
    }
    match m.picture {
        Some((3, _, true)) => s("progressive"),
        Some((3, true, false)) => s("tt"),
        Some((3, false, false)) => s("bb"),
        _ => unknown(),
    }
}

/// How many pictures a video stream's decoder holds back for reordering,
/// as the stream states it: H.264's `max_num_reorder_frames`, H.265's
/// `sps_max_num_reorder_pics`, and for MPEG-2 video one unless the
/// sequence is low delay (13818-2 6.3.5); MPEG-1 video always has one.
///
/// An H.264 stream that does not state it: none when its pictures are
/// presented in the order they are decoded (`pic_order_cnt_type` 2,
/// 8.2.1.3), else what the order of its first frames shows
/// ([`Order::reorder`]). A video stream the head found nothing to
/// configure, `ffprobe` could not decode either, and it reports none.
fn has_b_frames(st: &coreav_mpegts::Stream, facts: &Facts) -> Option<u32> {
    if facts.unconfigured && matches!(st.codec, Codec::H264 | Codec::Hevc | Codec::Mpeg2Video) {
        return Some(0);
    }
    match st.codec {
        Codec::H264 => facts.sps.and_then(|sps| {
            sps.vui
                .reorder
                .or((sps.slices.poc_type == 2).then_some(0))
                .or(facts.reordered)
        }),
        Codec::Hevc => facts.sps.and_then(|sps| sps.reorder),
        Codec::Mpeg1Video => facts.mpeg.map(|_| 1),
        Codec::Mpeg2Video => facts
            .mpeg
            .and_then(|m| m.extension)
            .map(|ext| u32::from(!ext.low_delay)),
        _ => None,
    }
}

/// The video entries, from `width` to `refs` and the H.264 pair after.
fn video_entries(st: &coreav_mpegts::Stream, head: &Head, facts: &Facts, e: &mut Entries) {
    let (w, h) = st.video.map_or((0, 0), |v| (v.width, v.height));
    let coded = matches!(st.codec, Codec::H264 | Codec::Hevc);
    e.push(("width", int(w)));
    e.push(("height", int(h)));
    e.push(("coded_width", int(if coded { w } else { 0 })));
    e.push(("coded_height", int(if coded { h } else { 0 })));
    // `ffprobe` reports no captions for an H.264 stream whatever its
    // pictures carry: three captures of the corpus whose pictures carry
    // A/53 captions, and a stream crafted with valid ones in every
    // picture (which `-show_frames` does show), report 0.
    let captions = head.captions && st.codec != Codec::H264;
    e.push(("closed_captions", Value::Int(i64::from(captions))));
    e.push(("film_grain", Value::Int(0)));
    let sps = facts.sps;
    let mpeg = facts.mpeg;
    let mpeg1 = st.codec == Codec::Mpeg1Video;
    e.push((
        "has_b_frames",
        has_b_frames(st, facts).map_or(Value::Na, int),
    ));
    let sar = match st.codec {
        Codec::H264 | Codec::Hevc => sps
            .and_then(|sps| sps.vui.sar)
            .map(|(a, b)| (u64::from(a), u64::from(b))),
        Codec::Mpeg1Video | Codec::Mpeg2Video => mpeg.and_then(|m| mpeg_sar(&m, mpeg1, w, h)),
        _ => None,
    };
    e.push((
        "sample_aspect_ratio",
        sar.and_then(|(a, b)| ratio(a, b)).map_or(Value::Na, s),
    ));
    e.push((
        "display_aspect_ratio",
        sar.and_then(|(a, b)| ratio(u64::from(w).checked_mul(a)?, u64::from(h).checked_mul(b)?))
            .map_or(Value::Na, s),
    ));
    let pix = match st.codec {
        Codec::H264 | Codec::Hevc => sps.as_ref().and_then(pix_fmt),
        Codec::Mpeg1Video => mpeg.map(|_| "yuv420p".to_owned()),
        Codec::Mpeg2Video => mpeg.and_then(|m| match m.extension?.chroma_format {
            1 => Some("yuv420p".to_owned()),
            2 => Some("yuv422p".to_owned()),
            3 => Some("yuv444p".to_owned()),
            _ => None,
        }),
        _ => None,
    };
    e.push(("pix_fmt", pix.map_or(unknown(), s)));
    e.push((
        "level",
        Value::Int(native::level(st).map_or(-99, i64::from)),
    ));
    let (range, colour) = match st.codec {
        Codec::H264 | Codec::Hevc => (
            sps.and_then(|sps| sps.vui.full_range),
            sps.and_then(|sps| sps.vui.colour)
                .map(|(p, t, m)| (u16::from(p), u16::from(t), u16::from(m))),
        ),
        Codec::Mpeg1Video | Codec::Mpeg2Video => (
            Some(false),
            match (st.color.primaries, st.color.transfer, st.color.matrix) {
                (Some(p), Some(t), Some(m)) => Some((p, t, m)),
                _ => None,
            },
        ),
        _ => (None, None),
    };
    // A stream that does not state its range is limited range to an H.265
    // decoder, and unknown to an H.264 one.
    let range = range.or((st.codec == Codec::Hevc && sps.is_some()).then_some(false));
    e.push((
        "color_range",
        range.map_or(unknown(), |full| s(if full { "pc" } else { "tv" })),
    ));
    let named = |f: fn(u16) -> Option<&'static str>, code: Option<u16>| {
        code.and_then(f).map_or(unknown(), s)
    };
    e.push((
        "color_space",
        named(crate::names::matrix, colour.map(|c| c.2)),
    ));
    e.push((
        "color_transfer",
        named(crate::names::transfer, colour.map(|c| c.1)),
    ));
    e.push((
        "color_primaries",
        named(crate::names::primaries, colour.map(|c| c.0)),
    ));
    let location = match st.codec {
        Codec::H264 => sps.map(|sps| {
            sps.vui
                .chroma_location
                .and_then(chroma_location)
                .unwrap_or("left")
        }),
        Codec::Hevc => sps.map(|sps| {
            sps.vui.chroma_location.and_then(chroma_location).unwrap_or(
                if sps.chroma_format_idc == 1 {
                    "left"
                } else {
                    "unspecified"
                },
            )
        }),
        Codec::Mpeg1Video => mpeg.map(|_| "center"),
        Codec::Mpeg2Video => mpeg.and_then(|m| match m.extension?.chroma_format {
            1 => Some("left"),
            2 => Some("topleft"),
            _ => None,
        }),
        _ => None,
    };
    e.push((
        "chroma_location",
        location.map_or(Value::Unset("unspecified"), s),
    ));
    let field_order = match st.codec {
        Codec::H264 => match sps {
            Some(sps) if sps.frame_mbs_only => s("progressive"),
            Some(sps) => head
                .unit
                .as_deref()
                .and_then(|unit| params::h264_field_order(unit, &sps))
                .map_or(unknown(), s),
            None => unknown(),
        },
        Codec::Mpeg1Video | Codec::Mpeg2Video => mpeg.as_ref().map_or(unknown(), mpeg_field_order),
        _ => unknown(),
    };
    e.push(("field_order", field_order));
    e.push(("refs", Value::Int(1)));
    if st.codec == Codec::H264 {
        e.push(("is_avc", s("false")));
        e.push(("nal_length_size", s("0")));
    }
}

/// The channel layout `ffprobe` names for an audio stream.
fn channel_layout(st: &coreav_mpegts::Stream, head: &Head) -> Option<&'static str> {
    let channels = Some(rate_and_channels(st).1).filter(|c| *c > 0)?;
    match st.codec {
        Codec::Aac | Codec::AacLatm => st
            .config
            .as_deref()
            .and_then(|c| c.get(1))
            .and_then(|b| names::aac_layout((b >> 3) & 0x0F)),
        Codec::Ac3 | Codec::Eac3 => head
            .unit
            .as_deref()
            .and_then(params::ac3)
            .and_then(|h| params::ac3_layout(h.acmod, h.lfe)),
        Codec::Opus => match channels {
            1 => Some("mono"),
            2 => Some("stereo"),
            6 => Some("5.1"),
            _ => None,
        },
        _ => names::default_layout(u32::from(channels)),
    }
}

/// The bit rate an audio stream's frame headers state, or for ADTS the
/// mean [`Analysis`] takes.
fn audio_bit_rate(st: &coreav_mpegts::Stream, head: &Head) -> Option<u32> {
    let unit = head.unit.as_deref()?;
    match st.codec {
        Codec::MpegAudio => params::mpeg_audio_bit_rate(unit),
        Codec::Ac3 | Codec::Eac3 => params::ac3(unit).map(|h| h.bit_rate),
        Codec::Aac => match head.aac_rate {
            (mean, n) if n > 0 => u32::try_from(mean).ok(),
            _ => None,
        },
        _ => None,
    }
}

/// The disposition flags, all clear but those the descriptors set: the
/// audio type of an ISO 639 language descriptor (13818-1 2.6.19, table
/// 2-60) and the subtitling type of a DVB subtitling descriptor (EN 300
/// 468 table 26: 0x20 to 0x25 are for the hard of hearing).
fn disposition(st: &coreav_mpegts::Stream) -> Entries {
    let audio_type = st
        .descriptors
        .iter()
        .find(|d| d.tag == 0x0A)
        .and_then(|d| d.data.get(3).copied());
    let subtitling = st
        .descriptors
        .iter()
        .find(|d| d.tag == 0x59)
        .and_then(|d| d.data.get(3).copied());
    let set = |key: &str| match key {
        "clean_effects" => audio_type == Some(1),
        "hearing_impaired" => {
            audio_type == Some(2) || subtitling.is_some_and(|t| (0x20..=0x25).contains(&t))
        }
        "visual_impaired" => audio_type == Some(3),
        _ => false,
    };
    super::iso::disposition(false)
        .into_iter()
        .map(|(k, _)| (k, Value::Int(i64::from(set(k)))))
        .collect()
}

/// The channels an Opus stream's descriptor states (ETSI TS 102 366
/// annex: the extension descriptor's `channel_config_code`): 1 to 8 for
/// that many channels, 0x80 for two; `None` for the others.
fn opus_channels(st: &coreav_mpegts::Stream) -> Option<u8> {
    let code = st
        .descriptors
        .iter()
        .find(|d| d.tag == 0x7F && d.data.first() == Some(&0x80))
        .and_then(|d| d.data.get(1).copied())?;
    match code {
        1..=8 => Some(code),
        0x80 => Some(2),
        _ => None,
    }
}

/// Whether a LATM stream's configuration leaves SBR unstated where it is
/// commonly implicit: AAC-LC at 24 kHz or less, which `ffprobe` decodes
/// as HE-AAC at twice the rate when the frames carry SBR data (a DVB
/// capture of the corpus). The frames of LATM are not read for it.
fn latm_may_hide_sbr(st: &coreav_mpegts::Stream) -> bool {
    st.codec == Codec::AacLatm
        && st
            .audio
            .is_some_and(|a| a.sample_rate > 0 && a.sample_rate <= 24_000)
        && profile(st).as_deref() == Some("LC")
}

/// The sample rate and channels `ffprobe` reports for an audio stream, 0
/// when they are not known. Opus is always decoded at 48 kHz (RFC 7845
/// 5.1), with the channels its descriptor states.
fn rate_and_channels(st: &coreav_mpegts::Stream) -> (u32, u8) {
    if st.codec == Codec::Opus {
        return (48_000, opus_channels(st).unwrap_or(0));
    }
    st.audio.map_or((0, 0), |a| (a.sample_rate, a.channels))
}

/// The languages of a DVB subtitling descriptor (EN 300 468 6.2.41,
/// eight bytes an entry) or teletext descriptor (6.2.43, five bytes an
/// entry), joined by commas, for a stream without an ISO 639 language
/// descriptor.
fn entry_languages(st: &coreav_mpegts::Stream) -> Option<String> {
    let (descriptor, size) = st.descriptors.iter().find_map(|d| match d.tag {
        0x59 => Some((d, 8)),
        0x56 => Some((d, 5)),
        _ => None,
    })?;
    let languages: Vec<String> = descriptor
        .data
        .chunks_exact(size)
        .filter_map(|entry| entry.get(..3))
        .filter(|code| code.iter().all(u8::is_ascii_alphabetic))
        .map(|code| String::from_utf8_lossy(code).into_owned())
        .collect();
    (!languages.is_empty()).then(|| languages.join(","))
}

fn stream(
    ts: &TransportStream,
    st: &coreav_mpegts::Stream,
    index: usize,
    head: &Head,
    facts: &Facts,
    timing: (Option<i64>, Option<i64>),
) -> Stream {
    let mut e: Entries = Vec::with_capacity(48);
    e.push(("index", int(index)));
    let codec_name = native::ffprobe_codec(st);
    match codec_name {
        Some(n) => {
            e.push(("codec_name", s(n)));
            e.push(("codec_long_name", names::long_name(n).map_or(unknown(), s)));
        }
        None => {
            e.push(("codec_name", unknown()));
            e.push(("codec_long_name", unknown()));
        }
    }
    // AAC with SBR data is HE-AAC when its frames are stereo; a mono one
    // may carry parametric stereo too (HE-AACv2), which is not read.
    let aac_channels = st.audio.map(|a| a.channels);
    let profile = match (st.codec, facts.sbr) {
        (Codec::Aac, Some(true)) if aac_channels == Some(2) => Some("HE-AAC".to_owned()),
        (Codec::Aac, Some(true) | None) => None,
        _ if latm_may_hide_sbr(st) => None,
        _ => profile(st),
    };
    e.push(("profile", profile.map_or(unknown(), s)));
    // A stream whose type names no codec `ffprobe` knows has no media
    // type; a private stream it reads as binary data is `data`.
    let typed = st.codec != Codec::Other || st.stream_type == 0x06;
    e.push((
        "codec_type",
        if typed {
            s(native::kind(st.kind))
        } else {
            unknown()
        },
    ));
    let tag = native::tag_bytes(st);
    e.push(("codec_tag_string", s(native::codec_tag_string(tag))));
    e.push(("codec_tag", s(native::codec_tag(tag))));
    let video = st.kind == Kind::Video;
    let audio = st.kind == Kind::Audio;
    if video {
        video_entries(st, head, facts, &mut e);
    } else if audio {
        let (rate, channels) = match facts.lpcm {
            Some(l) => (l.sample_rate, u8::try_from(l.channels).unwrap_or(0)),
            None => rate_and_channels(st),
        };
        // SBR doubles the rate of the AAC frames.
        let rate = match (st.codec, facts.sbr) {
            (Codec::Aac, Some(true)) => Some(rate.saturating_mul(2)),
            (Codec::Aac, None) => None,
            _ if latm_may_hide_sbr(st) => None,
            _ => Some(rate),
        };
        let sbr_mono = st.codec == Codec::Aac && facts.sbr != Some(false) && channels != 2;
        // 24-bit LPCM is decoded to 32-bit samples.
        let sample_fmt = match facts.lpcm {
            Some(l) if l.bits > 16 => Some("s32"),
            _ => codec_name.and_then(|n| names::sample_fmt(n, None)),
        };
        e.push(("sample_fmt", sample_fmt.map_or(unknown(), s)));
        e.push((
            "sample_rate",
            rate.map_or(Value::Na, |r| Value::Unit(i64::from(r), Unit::Hertz)),
        ));
        e.push((
            "channels",
            if sbr_mono {
                Value::Na
            } else {
                Value::Int(i64::from(channels))
            },
        ));
        e.push((
            "channel_layout",
            facts
                .lpcm
                .map(|l| l.layout)
                .or_else(|| channel_layout(st, head))
                .filter(|_| !sbr_mono)
                .map_or(unknown(), s),
        ));
        e.push(("bits_per_sample", Value::Int(0)));
        e.push(("initial_padding", Value::Int(0)));
    } else if st.kind == Kind::Subtitle && typed {
        // The picture a presentation graphics stream is composed on; for
        // teletext, the page `ffprobe`'s teletext decoder lays out
        // (KD-TS-TELETEXT-SIZE).
        let size = match st.codec {
            Codec::PgsSubtitle => st.video.map(|v| (v.width, v.height)),
            Codec::DvbTeletext => Some((492, 250)),
            _ => None,
        };
        e.push(("width", size.map_or(Value::Na, |(w, _)| int(w))));
        e.push(("height", size.map_or(Value::Na, |(_, h)| int(h))));
    }
    e.push((
        "ts_id",
        ts.transport_stream_id
            .map_or(Value::Na, |id| s(id.to_string())),
    ));
    e.push(("ts_packetsize", s(packet_size(ts.packet_size).to_string())));
    e.push(("id", s(format!("0x{:x}", st.pid))));
    let (r_frame_rate, avg_frame_rate) = if video {
        frame_rates(st, facts)
    } else {
        (s("0/0"), s("0/0"))
    };
    e.push(("r_frame_rate", r_frame_rate));
    e.push(("avg_frame_rate", avg_frame_rate));
    e.push(("time_base", s(format!("1/{TIMESCALE}"))));
    let (start, duration) = timing;
    e.push(("start_pts", start.map_or(Value::Na, Value::Int)));
    e.push((
        "start_time",
        start.map_or(Value::Na, |v| time(v, 1, TIMESCALE)),
    ));
    e.push(("duration_ts", duration.map_or(Value::Na, Value::Int)));
    e.push((
        "duration",
        duration.map_or(Value::Na, |v| time(v, 1, TIMESCALE)),
    ));
    let bit_rate = match st.codec {
        Codec::Mpeg1Video => facts.mpeg.map(|m| u64::from(m.bit_rate) * 400),
        // MPEG-2 video: the rate its sequence header states, unless the
        // stream is of variable rate (`vbv_delay` 0xFFFF, 13818-2 6.3.9),
        // where that rate is only a bound and `ffprobe` reports none, or
        // the rate is the unstated 0x3FFFF.
        Codec::Mpeg2Video => facts
            .mpeg
            .filter(|m| m.bit_rate != 0x3FFFF && m.bit_rate != 0)
            .filter(|m| m.vbv_delay.is_some_and(|d| d != 0xFFFF))
            .map(|m| u64::from(m.bit_rate) * 400),
        _ if audio => facts
            .lpcm
            .map(params::Lpcm::bit_rate)
            .or_else(|| audio_bit_rate(st, head).map(u64::from)),
        _ => None,
    };
    e.push((
        "bit_rate",
        bit_rate.map_or(Value::Na, |b| Value::Unit(int_u64(b), Unit::BitRate)),
    ));
    e.push(("max_bit_rate", Value::Na));
    let raw_bits = (st.codec == Codec::H264)
        .then(|| facts.sps.map(|sps| sps.bit_depth))
        .flatten()
        .or(facts.lpcm.map(|l| l.bits).filter(|b| *b > 16));
    e.push((
        "bits_per_raw_sample",
        raw_bits.map_or(Value::Na, |b| s(b.to_string())),
    ));
    e.push(("nb_frames", Value::Na));
    e.push(("nb_read_frames", Value::Na));
    e.push(("nb_read_packets", Value::Na));
    if let Some(size) = facts.extradata {
        e.push(("extradata_size", int(size)));
    }
    let mut tags = Vec::new();
    if let Some(language) = st
        .language
        .clone()
        .or_else(|| entry_languages(st))
        .filter(|l| !l.is_empty())
    {
        tags.push(("language".to_owned(), language));
    }
    let mut side_data = Vec::new();
    if let Some(m) = facts.mpeg.filter(|_| video) {
        // The coded picture buffer the sequence header states (13818-2
        // 6.3.3): its size, and for MPEG-2 the bit rate it is filled at,
        // which `0x3FFFF` leaves unstated.
        let max = if st.codec == Codec::Mpeg2Video && m.bit_rate != 0x3FFFF {
            u64::from(m.bit_rate) * 400
        } else {
            0
        };
        side_data.push(SideData {
            typ: "cpb_properties",
            entries: vec![
                ("side_data_type", s("CPB properties")),
                ("max_bitrate", int(max)),
                ("min_bitrate", Value::Int(0)),
                ("avg_bitrate", Value::Int(0)),
                ("buffer_size", int(u64::from(m.vbv_buffer_size) * 16 * 1024)),
                ("vbv_delay", Value::Int(-1)),
            ],
        });
    }
    Stream {
        kind: kind(st),
        attached_pic: false,
        entries: e,
        disposition: disposition(st),
        tags,
        side_data,
    }
}

/// `r_frame_rate` and `avg_frame_rate` of a video stream: the picture rate
/// its first pictures' times show ([`picture_rate`]); with a single
/// picture, the clock its parameter sets state and no average.
///
/// A stream the head found nothing to configure has the base rate of the
/// time base and no average, as `ffprobe` reports for a stream it could
/// not decode (an MVC sub-bitstream, a stream the file never sends).
fn frame_rates(st: &coreav_mpegts::Stream, facts: &Facts) -> (Value, Value) {
    if facts.unconfigured && matches!(st.codec, Codec::H264 | Codec::Hevc | Codec::Mpeg2Video) {
        return (s(format!("{TIMESCALE}/1")), s("0/0"));
    }
    let Some((num, den)) = facts.rate else {
        if facts.units <= 1 {
            let tick = facts
                .sps
                .and_then(|sps| sps.vui.timing)
                .filter(|_| st.codec == Codec::H264)
                .and_then(|(units, scale)| rate(u64::from(scale), u64::from(units)));
            return (tick.map_or(Value::Na, s), s("0/0"));
        }
        return (Value::Na, Value::Na);
    };
    let base = rate(num, den);
    // `ffprobe` reports twice the picture rate of MPEG-1 video as its
    // real base rate.
    let real = if st.codec == Codec::Mpeg1Video {
        rate(num.saturating_mul(2), den)
    } else {
        base.clone()
    };
    let avg = if st.codec == Codec::H264 && facts.sps.is_some() {
        // The average counts frames, not pictures: two fields make one,
        // and a picture whose timing message repeats it counts for as
        // many frames as it is shown for. A stream that states no clock
        // has none (`0/0`), whatever its timestamps show.
        match facts.frames {
            Some((ticks, halves)) => num
                .checked_mul(ticks)
                .zip(den.checked_mul(halves))
                .and_then(|(n, d)| rate(n, d)),
            None => Some("0/0".to_owned()),
        }
    } else {
        base
    };
    (real.map_or(Value::Na, s), avg.map_or(Value::Na, s))
}

fn packet_size(size: PacketSize) -> u16 {
    match size {
        PacketSize::Ts188 => 188,
        PacketSize::Ts192 => 192,
        PacketSize::Ts204 => 204,
    }
}

fn int_u64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// A DVB string (ETSI EN 300 468 annex A) as `ffprobe` prints it, which
/// files crafted with every first byte show:
///
/// - a string that names UTF-8 (`0x15`) is the bytes after that byte;
/// - one that names `0x11` or `0x14` is the bytes after it read as UTF-16
///   big-endian;
/// - one that names a table of annex A that `ffprobe` does not convert
///   (`0x01` to `0x07`, `0x09` to `0x0B`, `0x12`, `0x13`, and `0x10 0x00`
///   with the ISO/IEC 8859 part after it) is its bytes as they are, the
///   byte naming the table included;
/// - one that starts with a byte annex A reserves (`0x08`, `0x0C` to
///   `0x0F`, `0x10` not followed by `0x00`, `0x16` to `0x1F`) is the bytes
///   after it when they are all ASCII, and all its bytes when not;
/// - any other, the default table's included, its bytes as they are.
///
/// The result ends at its first NUL byte (`0x10 0x00 0x01` prints as the
/// one byte `0x10`), and each byte that does not belong to a well-formed
/// UTF-8 sequence is U+FFFD, one for every such byte.
fn dvb_text(bytes: &[u8]) -> Option<String> {
    let raw: Vec<u8> = match bytes {
        [0x11 | 0x14, rest @ ..] => {
            let units = rest
                .chunks_exact(2)
                .map(|pair| match *pair {
                    [hi, lo] => u16::from_be_bytes([hi, lo]),
                    _ => 0,
                })
                .take_while(|unit| *unit != 0);
            return Some(
                char::decode_utf16(units)
                    .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect(),
            );
        }
        [0x15, rest @ ..] => rest.to_vec(),
        [0x01..=0x07 | 0x09..=0x0B | 0x12 | 0x13, ..] | [0x10, 0x00, ..] => bytes.to_vec(),
        [0x08 | 0x0C..=0x10 | 0x16..=0x1F, rest @ ..] if rest.is_ascii() => rest.to_vec(),
        _ => bytes.to_vec(),
    };
    let text = raw.split(|b| *b == 0).next().unwrap_or_default();
    Some(utf8_bytewise(text))
}

/// `bytes` as UTF-8, each byte that does not start a well-formed sequence
/// replaced by U+FFFD on its own (a sequence cut short is one U+FFFD a
/// byte, as `ffprobe` validates strings).
fn utf8_bytewise(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                out.push_str(text);
                break;
            }
            Err(error) => {
                let (good, bad) = rest.split_at(error.valid_up_to());
                out.push_str(std::str::from_utf8(good).unwrap_or_default());
                out.push(char::REPLACEMENT_CHARACTER);
                rest = bad.get(1..).unwrap_or_default();
            }
        }
    }
    out
}

/// The programs, in the order of the PAT, each with its streams.
///
/// A service the Service Description Table names and the PAT does not is
/// a program too, with no table and no streams, as `ffprobe` lists it
/// (a capture of the corpus, and files crafted with such a service): the
/// programs are in the order the two tables first name them, so a file
/// whose first SDT comes before its first PAT lists its services first.
fn programs(ts: &TransportStream, order: &[usize], services: &[Service]) -> Vec<Program> {
    let tags_of = |number: u16| {
        let mut tags = Vec::new();
        if let Some(service) = services.iter().find(|sv| sv.id == number) {
            if let (Some(name), Some(provider)) =
                (dvb_text(&service.name), dvb_text(&service.provider))
            {
                tags.push(("service_name".to_owned(), name));
                tags.push(("service_provider".to_owned(), provider));
            }
        }
        tags
    };
    let from_pat = ts.programs.iter().map(|p| {
        let streams: Vec<usize> = order
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                ts.streams
                    .get(**i)
                    .is_some_and(|st| st.programs.contains(&p.number))
            })
            .map(|(position, _)| position)
            .collect();
        let program = Program {
            entries: vec![
                ("program_id", Value::Int(i64::from(p.number))),
                ("program_num", Value::Int(i64::from(p.number))),
                ("nb_streams", int(streams.len())),
                ("pmt_pid", Value::Int(i64::from(p.pmt_pid))),
                // `ffprobe` reports 0 for a program whose table it never read.
                (
                    "pcr_pid",
                    Value::Int(if p.pmt_read { i64::from(p.pcr_pid) } else { 0 }),
                ),
            ],
            tags: tags_of(p.number),
            streams,
        };
        (p.number, program)
    });
    let mut listed: Vec<(u16, Program)> = from_pat.collect();
    let only_named = services
        .iter()
        .filter(|sv| !ts.programs.iter().any(|p| p.number == sv.id))
        .map(|sv| {
            let program = Program {
                entries: vec![
                    ("program_id", Value::Int(i64::from(sv.id))),
                    ("program_num", Value::Int(0)),
                    ("nb_streams", Value::Int(0)),
                    ("pmt_pid", Value::Int(0)),
                    ("pcr_pid", Value::Int(0)),
                ],
                tags: tags_of(sv.id),
                streams: Vec::new(),
            };
            (sv.id, program)
        });
    listed.extend(only_named);
    if ts.services_first {
        // The services in the SDT's order, then the programs it does not
        // name.
        let rank = |number: u16| {
            services
                .iter()
                .position(|sv| sv.id == number)
                .unwrap_or(usize::MAX)
        };
        listed.sort_by_key(|(number, _)| rank(*number));
    }
    listed.into_iter().map(|(_, program)| program).collect()
}

fn format(
    ts: &TransportStream,
    file: &FileTiming,
    size: u64,
    name: &str,
    programs: usize,
) -> Entries {
    // `ffprobe` keeps a file's start and end in microseconds, each
    // rounded on its own, and the duration is their difference: a span
    // of 618,970 ticks (6,877,444.4 us) from a start 0.11 us past a whole
    // microsecond to an end 0.56 us past one lasts 6,877,445.
    let micros = file.micros;
    vec![
        ("filename", s(name)),
        (
            "nb_streams",
            int(ts.streams.len().saturating_add(ts.dropped.len())),
        ),
        ("nb_programs", int(programs)),
        ("nb_stream_groups", Value::Int(0)),
        ("format_name", s(FORMAT_NAME)),
        ("format_long_name", s(FORMAT_LONG_NAME)),
        (
            "start_time",
            file.start.map_or(Value::Na, |t| time(t, 1, TIMESCALE)),
        ),
        (
            "duration",
            micros.map_or(Value::Na, |us| time(us, 1, 1_000_000)),
        ),
        ("size", Value::Unit(int_u64(size), Unit::Byte)),
        (
            "bit_rate",
            micros
                .and_then(|us| crate::compat::format_bit_rate(size, us))
                .map_or(Value::Na, |b| Value::Unit(int_u64(b), Unit::BitRate)),
        ),
        ("probe_score", Value::Int(PROBE_SCORE)),
    ]
}

/// What the listing needs of each stream of the probe.
struct Listed {
    /// The index `ffprobe` prints.
    index: usize,
    /// The duration of one picture of a video stream, 90 kHz ticks.
    ticks: Option<i64>,
    audio: bool,
    dvb_subtitle: bool,
    /// How many pictures an H.264 or H.265 stream reorders.
    reorder: Option<usize>,
    /// The clock of an H.264 stream's pictures, which gives each its
    /// duration ([`Clock::duration`]) and a second field its times
    /// ([`field_times`]).
    clock: Option<Clock>,
    /// The header of a Blu-ray LPCM stream's first frame, which gives each
    /// frame its duration from its size.
    lpcm: Option<params::Lpcm>,
}

/// The most reordered pictures the decoding times are rebuilt for: far
/// more than any stream states (H.264 allows 16).
const MAX_REORDER: usize = 32;

/// Rebuilds the decoding time `ffprobe` gives a coded picture whose PES
/// packet states none of its own (none, or one equal to its presentation
/// time), from the presentation times of the last `reorder + 1` pictures
/// (`docs/known-divergences-ts.md`, KD-TS-DECODE-TIME): they are kept in
/// order, each new one taking the place of the earliest, and the earliest
/// left is the decoding time; while fewer than `reorder + 1` pictures have
/// been seen there is none.
#[derive(Default)]
struct DecodeTimes {
    /// In order, earliest first; `None` is a place no picture filled yet,
    /// earlier than any.
    window: Vec<Option<i64>>,
    /// The decoding time of the last picture.
    last: Option<i64>,
}

impl DecodeTimes {
    /// The decoding time of the next picture, presented at `pts`.
    fn next(&mut self, pts: i64, reorder: usize) -> Option<i64> {
        if self.window.len() != reorder + 1 {
            self.window = vec![None; reorder + 1];
        }
        if let Some(first) = self.window.first_mut() {
            *first = Some(pts);
        }
        for i in 0..reorder {
            let (Some(&a), Some(&b)) = (self.window.get(i), self.window.get(i + 1)) else {
                break;
            };
            // `None` sorts before every time.
            if a <= b {
                break;
            }
            self.window.swap(i, i + 1);
        }
        self.window.first().copied().flatten()
    }
}

/// The bytes `ffprobe` lists of a unit: a DVB subtitle PES payload
/// without its `data_identifier` and `subtitle_stream_id` and its closing
/// `end_of_PES_data_field_marker` (EN 300 743 7.1), the others whole.
fn packet_bytes(data: &[u8], dvb_subtitle: bool) -> usize {
    match data {
        [0x20, 0x00, rest @ ..] if dvb_subtitle => {
            rest.len() - usize::from(rest.last() == Some(&0xFF))
        }
        _ => data.len(),
    }
}

/// The access units of every stream, in the order the reader lists them.
struct TsPackets<S> {
    ts: TransportStream,
    source: S,
    /// Per stream of the probe.
    listed: Vec<Listed>,
}

impl<S: Source> PacketSource for TsPackets<S> {
    fn visit(&mut self, f: &mut dyn FnMut(Packet) -> Result<bool, Failure>) -> Result<(), Failure> {
        let limits = Limits::default();
        let packets = self
            .ts
            .packets(&mut self.source, &limits)
            .listing_as_ffprobe();
        let known = self.listed.len();
        let mut decode: Vec<DecodeTimes> =
            self.listed.iter().map(|_| DecodeTimes::default()).collect();
        let mut fields: Vec<OpenField> = self.listed.iter().map(|_| None).collect();
        let shift = if self.ts.packet_size == PacketSize::Ts204 {
            16
        } else {
            0
        };
        for item in packets {
            let p = match item {
                Ok(p) => p,
                Err(Error::Io(kind)) => return Err(Failure::Mpegts(Error::Io(kind))),
                // `ffprobe` lists what it read before damage it cannot
                // go past.
                Err(_) => return Ok(()),
            };
            let listed = self.listed.get(p.stream);
            // A stream a later table added: `ffprobe` numbers it after the
            // probe's.
            let stream = listed.map_or(p.stream.max(known), |l| l.index);
            let (mut pts, mut dts) = (p.pts, p.dts);
            if let (Some(reorder), Some(t), Some(times)) = (
                listed.and_then(|l| l.reorder),
                p.pts,
                decode.get_mut(p.stream),
            ) {
                let rebuilt = times.next(t, reorder);
                // A rebuilt time that does not move past the last one
                // (a picture lost before it) is not taken. The last one
                // counts as no later than its own picture's presentation
                // time: a muxer that states decoding times after the
                // presentation times of its B-pictures has `ffprobe` take
                // a rebuilt time equal to such a stated one (two real
                // captures), while a lost picture has it keep the
                // presentation time (`ts_lost_packets`).
                let forward = match (rebuilt, times.last) {
                    (Some(r), Some(last)) => r > last,
                    _ => true,
                };
                if p.dts.is_none_or(|d| d == t) && forward {
                    dts = rebuilt;
                }
                times.last = dts.map(|d| d.min(t));
            }
            let clock = listed.and_then(|l| l.clock);
            let picture = clock.and_then(|c| c.picture(&p.data));
            if let (Some(clock), Some(open)) = (clock, fields.get_mut(p.stream)) {
                let untimed = p.pts.is_none() && p.dts.is_none();
                (pts, dts) = field_times(&clock, picture, untimed, (pts, dts), open);
            }
            let duration = match clock {
                Some(c) if c.tick.is_some() => {
                    c.duration(picture.map_or(2, params::Picture::ticks))
                }
                _ => p
                    .duration
                    .or_else(|| listed.and_then(|l| l.lpcm)?.duration(p.data.len()))
                    .or(listed.and_then(|l| l.ticks)),
            };
            // An audio frame the file gives no time of its own is placed
            // after the one before it on the decoding timeline, which
            // `ffprobe` reports as its presentation time too
            // (KD-TS-FRAME-TIME).
            if listed.is_some_and(|l| l.audio) && p.pos.is_none() && p.dts.is_some() {
                pts = p.dts;
            }
            // `ffprobe` gives the stream id of each PES packet it reads to
            // the first packet that reading lists.
            let side_data = match p.fed {
                Some(id) => vec![SideData {
                    typ: "mpegts_stream_id",
                    entries: vec![
                        ("side_data_type", s("MPEGTS Stream ID")),
                        ("id", Value::Int(i64::from(id))),
                    ],
                }],
                _ => Vec::new(),
            };
            let packet = Packet {
                stream,
                pts,
                dts,
                duration,
                size: u64::try_from(packet_bytes(
                    &p.data,
                    listed.is_some_and(|l| l.dvb_subtitle),
                ))
                .unwrap_or(u64::MAX),
                // `ffprobe` places a 204-byte packet 16 bytes before it
                // starts.
                pos: p.pos.map(|pos| pos.saturating_sub(shift)),
                keyframe: p.keyframe,
                discard: false,
                corrupt: p.corrupt,
                side_data,
            };
            if !f(packet)? {
                return Ok(());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_rates_from_times() {
        // Reordered times, a lost picture and a time missing.
        let times = [Some(7200), Some(0), Some(3600), None, Some(14_400)];
        assert_eq!(picture_rate(&times), Some((25, 1)));
        // 23.976 frames a second falls between whole ticks.
        let film: Vec<Option<i64>> = (0..48).map(|k| Some(k * 3_753_750 / 1000)).collect();
        assert_eq!(picture_rate(&film), Some((24_000, 1001)));
        // An uncommon rate with a small denominator is kept as it is.
        assert_eq!(
            picture_rate(&[Some(0), Some(4000), Some(8000)]),
            Some((45, 2))
        );
        assert_eq!(picture_rate(&[Some(0)]), None);
        assert_eq!(picture_rate(&[Some(10), Some(10)]), None);
    }

    #[test]
    fn exact_ticks_only_for_whole_ticks() {
        assert_eq!(exact_ticks(2, 50), Some(3600));
        assert_eq!(exact_ticks(2002, 60_000), Some(3003));
        assert_eq!(exact_ticks(1, 7), None);
        assert_eq!(exact_ticks(1, 0), None);
    }

    #[test]
    fn pixel_formats_follow_depth_range_and_matrix() {
        let mut sps = Sps {
            chroma_format_idc: 1,
            bit_depth: 8,
            ..Sps::default()
        };
        assert_eq!(pix_fmt(&sps).as_deref(), Some("yuv420p"));
        sps.vui.full_range = Some(true);
        assert_eq!(pix_fmt(&sps).as_deref(), Some("yuvj420p"));
        sps.bit_depth = 10;
        assert_eq!(pix_fmt(&sps).as_deref(), Some("yuv420p10le"));
        sps.chroma_format_idc = 3;
        sps.bit_depth = 8;
        sps.vui.colour = Some((1, 13, 0));
        assert_eq!(pix_fmt(&sps).as_deref(), Some("gbrp"));
    }

    #[test]
    fn dvb_strings() {
        let text = |b: &[u8]| dvb_text(b).unwrap_or_default();
        assert_eq!(text(b"Service01"), "Service01");
        assert_eq!(text(b"\x15caf\xc3\xa9"), "caf\u{e9}");
        // The NUL of `0x10 0x00 0x01` ends the string.
        assert_eq!(text(b"\x10\x00\x01caf\xe9"), "\u{10}");
        assert_eq!(text(b"\x0bORF 2 N\xf6"), "\u{b}ORF 2 N\u{fffd}");
        assert_eq!(text(b"AB\xf6\xe4C"), "AB\u{fffd}\u{fffd}C");
        assert_eq!(text(b"x\xe2\x82"), "x\u{fffd}\u{fffd}");
        assert_eq!(text(b"\x11\x00\xe9\x00A"), "\u{e9}A");
        assert_eq!(text(b"\x14ab"), "\u{6162}");
        assert_eq!(text(b"\x1f\x01ab"), "\u{1}ab");
        assert_eq!(text(b"\x0eABC"), "ABC");
        assert_eq!(text(b"\x0eAB\xc6"), "\u{e}AB\u{fffd}");
        assert_eq!(text(b"\x01ABC"), "\u{1}ABC");
        assert_eq!(text(b"\x10AB"), "AB");
    }

    #[test]
    fn picture_clock_durations_and_field_steps() {
        let mut sps = Sps::default();
        // 59.94 fields a second: a field is 1,501.5 ticks.
        sps.vui.timing = Some((1001, 60_000));
        let clock = Clock::of(sps);
        assert_eq!(clock.duration(1), Some(1501));
        assert_eq!(clock.duration(2), Some(3003));
        assert_eq!(clock.field_step(), Some(1502));
        // 23.976... frames of 3,749.994 ticks last 3,749.
        sps.vui.timing = Some((208_333, 10_000_000));
        assert_eq!(Clock::of(sps).duration(2), Some(3749));
        sps.vui.timing = None;
        assert_eq!(Clock::of(sps).duration(2), None);
    }

    #[test]
    fn picture_ticks_follow_fields_and_pic_struct() {
        let ticks = |field, pic_struct| params::Picture { field, pic_struct }.ticks();
        assert_eq!(ticks(Some(false), Some(1)), 1);
        assert_eq!(ticks(None, None), 2);
        assert_eq!(ticks(None, Some(0)), 2);
        assert_eq!(ticks(None, Some(5)), 3);
        assert_eq!(ticks(None, Some(7)), 4);
        assert_eq!(ticks(None, Some(8)), 6);
    }

    #[test]
    fn a_second_field_takes_the_first_fields_times() {
        let mut sps = Sps::default();
        sps.vui.timing = Some((1, 50));
        let clock = Clock::of(sps);
        let field = |bottom| {
            Some(params::Picture {
                field: Some(bottom),
                pic_struct: None,
            })
        };
        let mut open = None;
        let first = field_times(
            &clock,
            field(false),
            false,
            (Some(100), Some(90)),
            &mut open,
        );
        assert_eq!(first, (Some(100), Some(90)));
        let second = field_times(&clock, field(true), true, (None, None), &mut open);
        assert_eq!(second, (Some(1900), Some(1890)));
        // A frame the file gives no time of its own takes none.
        let frame = Some(params::Picture::default());
        assert_eq!(
            field_times(&clock, frame, true, (None, None), &mut open),
            (None, None)
        );
        // Nor does a field of the same parity as the open one.
        field_times(&clock, field(false), false, (Some(5), None), &mut open);
        assert_eq!(
            field_times(&clock, field(false), true, (None, None), &mut open),
            (None, None)
        );
    }

    #[test]
    fn reordering_from_the_order_of_the_first_frames() {
        let order = |pts: &[i64]| {
            let mut order = Order::default();
            for (k, t) in pts.iter().enumerate() {
                let k = i64::try_from(k).unwrap();
                order.note(Some(*t), Some(k * 3600), u64::try_from(k).unwrap() * 1000);
            }
            order.covered = true;
            order
        };
        // In order: nothing held back.
        assert_eq!(order(&[0, 3600, 7200]).reorder(), Some(0));
        // I P B: one.
        assert_eq!(order(&[0, 7200, 3600, 14_400, 10_800]).reorder(), Some(1));
        // A pyramid from the start: two.
        assert_eq!(order(&[0, 14_400, 7200, 3600, 10_800]).reorder(), Some(2));
        // A pyramid that starts later: not known.
        assert_eq!(
            order(&[
                0, 3600, 7200, 10_800, 14_400, 28_800, 21_600, 18_000, 25_200
            ])
            .reorder(),
            None
        );
        // Not covered: not known.
        let mut partial = order(&[0, 3600]);
        partial.covered = false;
        assert_eq!(partial.reorder(), None);
    }

    #[test]
    fn the_analysis_ends_after_the_pes_packet_past_five_seconds() {
        let mut a = Analysis::default();
        assert!(a.note(0, 0, true, Some(0), Some(0)));
        assert!(a.note(1, 0, true, Some(100), Some(100)));
        // Exactly five seconds is not past them.
        assert!(a.note(0, 0, true, Some(450_000), Some(450_000)));
        assert!(a.note(0, 0, true, Some(450_001), Some(450_001)));
        // The frames of the same PES packet are still read.
        assert!(a.note(0, 0, false, None, Some(452_000)));
        assert!(!a.note(1, 0, true, Some(9_000), Some(9_000)));
        assert!(!a.note(0, 0, true, Some(460_000), Some(460_000)));
        // Five million bytes end it too, after the PES packet that starts
        // past them.
        let mut a = Analysis::default();
        assert!(a.note(0, 4_999_000, true, Some(0), Some(0)));
        assert!(a.note(0, 5_000_100, true, Some(10), Some(10)));
        assert!(a.note(0, 5_000_100, false, None, Some(20)));
        assert!(!a.note(1, 5_000_300, true, Some(30), Some(30)));
    }

    #[test]
    fn adts_frames_count_their_whole_length() {
        // 48 kHz (index 3), frame_length 371.
        let mut frame = vec![0xFF, 0xF1, 0x4C, 0x80, 0x2E, 0x7F, 0xFC];
        assert_eq!(adts_bit_rate(&frame), None);
        frame.resize(371, 0);
        assert_eq!(adts_bit_rate(&frame), Some(371 * 8 * 48_000 / 1024));
    }

    #[test]
    fn rates_are_reduced() {
        assert_eq!(rate(90_000, 3600).as_deref(), Some("25/1"));
        assert_eq!(rate(90_000, 3003).as_deref(), Some("30000/1001"));
        assert_eq!(rate(0, 1), None);
    }
}
