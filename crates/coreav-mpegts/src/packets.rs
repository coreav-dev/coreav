//! The packet listing: every access unit of every kept stream, each
//! stream's in storage order. The streams are interleaved in storage
//! order too, except where units wait for a picture period the stream
//! has not shown and are released together.
//!
//! [`Packets::listing_as_ffprobe`] turns on the listing `ffprobe` gives
//! instead: the streams interleaved in the order their units are finished
//! (a unit is listed once the bytes after it show where it ends), the
//! PES packets of coded video read in pieces of [`PES_PIECE`] bytes, the
//! later units of a PES packet with no time, and audio frames placed and
//! released as `ffprobe` places and releases them. `coreav ffprobe` uses
//! it; every other caller gets the default listing.

use crate::codec::{Codec, StreamKind};
use crate::demux::{Demux, Pes};
use crate::error::Warning;
use crate::frames;
use crate::probe::{Audio, Stream, TransportStream};
use crate::reader::Reader;
use crate::video::{self, Nal};
use crate::{Error, LimitKind, Limits, Source};
use std::collections::{HashMap, VecDeque};

/// One access unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    /// Index into [`Packets::streams`]: the probe's streams
    /// ([`TransportStream::streams`]), then those a later PMT version adds.
    pub stream: usize,
    /// Offset of the stored packet that starts the PES packet, for the
    /// first access unit that starts in it; `None` for the later frames
    /// of a PES packet holding several audio frames.
    pub pos: Option<u64>,
    /// Offset of the stored packet that starts the PES packet it came from.
    pub pes_offset: u64,
    /// Presentation timestamp (90 kHz, wrap-corrected).
    pub pts: Option<i64>,
    /// Decoding timestamp (90 kHz, wrap-corrected); the PTS when the PES
    /// header gives only that.
    pub dts: Option<i64>,
    /// Duration in 90 kHz ticks, for audio frames.
    pub duration: Option<i64>,
    /// Decoding can start here.
    pub keyframe: bool,
    /// The access unit's bytes (for audio frames, header included).
    pub data: Vec<u8>,
    /// A transport packet of the PES packet whose reading released this
    /// unit was lost, repeated or flagged as in error.
    pub corrupt: bool,
    /// The `stream_id` of the PES packet whose reading released this
    /// unit, when it is the first unit of its stream that reading
    /// released. A unit is released once the bytes after it show where it
    /// ends, commonly the start of the next unit in the next PES packet,
    /// so a unit is often released by the PES packet after its own.
    /// `None` for the later units of one reading, and for the units only
    /// the end of the file released.
    pub fed: Option<u8>,
    /// An audio frame whose header an earlier PES packet held whole, and
    /// whose last bytes a later one gave (a frame cut inside its header
    /// is read again from its start, once whole, and is not).
    pub split: bool,
}

/// Per stream: the bytes of an audio frame cut by the end of its PES
/// packet, and the timeline of its frames.
#[derive(Default)]
struct Frames {
    leftover: Vec<u8>,
    /// The length of the frame `leftover` starts, once its header was
    /// read: bytes are appended until it is whole, without reading it
    /// again.
    need: usize,
    /// A frame of this stream has been listed: the bytes before the first
    /// frame header are the tail of one that began before the file, and
    /// are listed as a packet of their own.
    started: bool,
    /// The bytes before the stream's first frame header, when they join
    /// that frame's packet rather than being one of their own (MPEG audio
    /// in the listing as `ffprobe` gives it), until the frame is whole.
    lead: Vec<u8>,
    /// When the next frame starts (90 kHz), from [`Frames::anchor`].
    next: Option<i64>,
    /// The last time a frame was given (a PES packet's), the samples of
    /// the frames since it, their rate, and the samples of the frame it
    /// gave the time to. A frame that takes no time of its own starts
    /// that many samples after the anchor, rounded once (in the listing
    /// as `ffprobe` gives it, AAC frames are placed by [`grid_time`], on
    /// a grid of frames counted from time zero), rather than one truncated
    /// frame duration after the frame before it:
    /// a frame of 1,024 samples at 22,050 Hz lasts 4,179.59 ticks, so
    /// adding 4,179 each time loses a tick every other frame over a run of
    /// hundreds.
    anchor: Option<(i64, u64, u32, u32)>,
    /// An E-AC-3 access unit, held until the next independent frame shows
    /// that no dependent substream follows it, and the dependent
    /// substreams joined to it.
    held: Option<(Packet, u8)>,
    /// A DTS stream has shown a core frame: its extension substreams
    /// extend the core frame before them (TS 102 114 7.5), rather than
    /// being access units of their own as in a stream without a core
    /// (DTS Express).
    dts_core: bool,
    /// The PTS and position of the PES packet in which the frame
    /// `leftover` starts, when it is the first frame that starts there.
    cut: Option<(Option<i64>, i64, u64)>,
    /// The frame `leftover` starts is an AC-3 or E-AC-3 frame of which its
    /// PES packet holds fewer than [`AC3_TIMED`] bytes: in the listing as
    /// `ffprobe` gives it, it takes the time and position of the PES
    /// packet it goes on in, and so does the first frame that starts
    /// there.
    late: bool,
    /// What the stream's presentation times are ahead of its decoding
    /// times, from the PES packet the frames of a run take their time
    /// from: a muxer that states a decoding time for sound states one
    /// the same distance before every presentation time (13818-1 2.7.5
    /// would have it state none at all).
    offset: i64,
    /// The shortest a picture of a video stream has lasted: the smallest
    /// per-unit step a run the stream's own stated times closed has
    /// shown. It places the last run of the file, which no stated time
    /// closes. (Not used by the listing as `ffprobe` gives it.)
    period: Option<i64>,
    /// The access unit of a coded video stream being read, with the bytes
    /// it has so far, and the splitter that finds where the next one
    /// begins: a PES packet boundary cuts an access unit, so neither can
    /// be read a PES packet at a time.
    open: Option<Open>,
    splitter: video::AccessUnits,
    /// The access units since the last stated time, the first of them
    /// first: a run, held until the next stated time shows how many there
    /// are and so what each of them takes of the picture interval.
    run: Vec<(Au, Vec<u8>)>,
    /// The access units of a run held because the picture interval was not
    /// known yet, each with its place in its run and the run's length: the
    /// first run of a stream states one time, and the interval needs two.
    /// The shift is applied when the interval arrives, before the next
    /// run's units are listed, so storage order is kept.
    pending: Vec<(Packet, usize, usize)>,
}

/// What an access unit of a coded video stream takes from the PES packet
/// it starts in.
#[derive(Clone, Copy, Debug, Default)]
struct Au {
    /// The times its PES packet states, if any.
    pts: Option<i64>,
    dts: Option<i64>,
    /// Offset of the stored packet that starts its PES packet, when it is
    /// the first access unit that starts there; the later ones take no
    /// position, as they take no time of the file's own.
    pos: Option<u64>,
    pes_offset: u64,
    random_access: bool,
    /// The `stream_id` of the PES packet whose reading finished the unit,
    /// when it is the first unit that reading finished ([`Packet::fed`]):
    /// two fields of a frame in one PES packet are finished by two
    /// readings, the first by its own and the second by the next. Set only
    /// in the listing as `ffprobe` gives it, where units are listed as they
    /// are finished rather than as runs.
    fed: Option<u8>,
}

/// An access unit being read: what it takes from its PES packet, and the
/// bytes of it read so far.
#[derive(Debug, Default)]
struct Open {
    au: Au,
    data: Vec<u8>,
}

impl Frames {
    /// Takes the per-unit step a closed run showed as the stream's
    /// picture period, keeping the smallest.
    fn note_period(&mut self, step: i64) {
        if step > 0 {
            self.period = Some(self.period.map_or(step, |least: i64| least.min(step)));
        }
    }
}

/// The bytes of an AC-3 or E-AC-3 frame a PES packet has to hold for the
/// frame to take the time and position of the PES packet it starts in,
/// in the listing as `ffprobe` gives it: a frame the end of its PES
/// packet cuts within its first six bytes takes those of the next one,
/// as the first frame that starts there does too (a capture of the
/// corpus whose AC-3 frame starts six bytes before the end of its PES
/// packet, and AC-3 and E-AC-3 streams crafted with every cut from one
/// byte to sixteen; ADTS, MP2 and MP3 streams crafted so show no such
/// rule). The six bytes are AC-3's `syncinfo` and the `bsid` after it
/// (ATSC A/52 5.3.1, 5.4.2.1), and E-AC-3's first six.
const AC3_TIMED: usize = 7;

/// Most dependent substreams in one E-AC-3 access unit (ATSC A/52 annex E
/// 2.3.1.2: `substreamid` 0 to 7).
const MAX_DEPENDENT: u8 = 8;

/// How far after the start of a run its `k`th of `units` access units
/// falls, `span` being what the run lasts: the span divided equally,
/// rounded to the nearest tick.
fn step(span: i64, k: usize, units: usize) -> Option<i64> {
    let (k, units) = (i64::try_from(k).ok()?, i64::try_from(units).ok()?);
    if units <= 0 {
        return None;
    }
    let scaled = span.checked_mul(k)?.checked_add(units / 2)?;
    Some(scaled / units)
}

/// `samples` at `rate` in 90 kHz ticks, rounded to the nearest tick.
fn ticks(samples: u64, rate: u32) -> Option<i64> {
    let rate = u64::from(rate);
    let scaled = samples
        .checked_mul(90_000)?
        .checked_add(rate.checked_div(2)?)?
        .checked_div(rate)?;
    i64::try_from(scaled).ok()
}

/// The most bytes of a PES payload `ffprobe` reads at once, as soon as
/// they arrive (in the listing as `ffprobe` gives it): on streams crafted with two pictures to a PES packet, the
/// second takes the PES packet's position when it starts 204,800 bytes
/// into the payload or later, and none when it starts 204,700 in; on the
/// interlaced captures of the v1-ts-audio corpus, so does the second field
/// of a frame whose first field is longer than that, and the unit before
/// such a PES packet is listed among the audio frames that arrive while
/// its first piece does.
const PES_PIECE: usize = 204_800;

/// A piece of a PES payload ([`PES_PIECE`]) and what of its PES packet the
/// coded video reader takes.
#[derive(Clone, Copy)]
struct Piece<'a> {
    payload: &'a [u8],
    offset: u64,
    random_access: bool,
    stream_id: u8,
}

/// The start code syntax whose access units a stream of `codec` is cut
/// into across its PES packets: H.264 and HEVC always; MPEG-1 and MPEG-2
/// video in the listing as `ffprobe` gives it, which lists their pictures
/// one by one however many a PES packet holds (a capture of the corpus
/// with four pictures to a PES packet of 120 KB, and streams crafted with
/// one to three, and a fixed 5,000 bytes, to a PES packet). Elsewhere a
/// PES packet of MPEG video is a packet, the unit the file delivers.
fn units_of(codec: Codec, as_ffprobe: bool) -> Option<Nal> {
    match codec {
        Codec::H264 => Some(Nal::H264),
        Codec::Hevc => Some(Nal::Hevc),
        Codec::Mpeg1Video | Codec::Mpeg2Video if as_ffprobe => Some(Nal::Mpeg2),
        _ => None,
    }
}

/// `num / den` rounded to the nearest integer, halves up, for either sign
/// of `num`; `None` unless `den` > 0.
fn round_div(num: i128, den: i128) -> Option<i128> {
    if den <= 0 {
        return None;
    }
    Some(
        num.checked_mul(2)?
            .checked_add(den)?
            .div_euclid(den.checked_mul(2)?),
    )
}

/// The time of an audio frame that starts `samples` after the frame a PES
/// packet times at `base`, at `rate`, in frames of `unit` samples (those
/// of the frame the PES packet times). The base is taken to the nearest
/// whole frame `n` of a grid of frames counted from time zero, and the
/// frame starts as far after the base as the grid places `samples` after
/// frame `n`, each end of that span rounded to the nearest tick. On the
/// AAC streams of the v1-ts-audio corpus, and on AAC streams crafted with
/// PES times at every alignment to that grid, this is the time `ffprobe`
/// gives each such frame; rounding the span since the base once, as this
/// reader did, misplaces one frame in three by a tick wherever a frame is
/// not a whole number of ticks (44.1 and 22.05 kHz).
///
/// The arithmetic is done in `i64` and, only where a product would not
/// fit, again in `i128`: every step is exact in either width, so the
/// result is the same, and the common case costs no 128-bit division.
fn grid_time(base: i64, samples: u64, rate: u32, unit: u32) -> Option<i64> {
    grid_time_narrow(base, samples, rate, unit)
        .or_else(|| grid_time_wide(base, samples, rate, unit))
}

/// [`round_div`] in `i64`; `None` where a step overflows as well.
fn round_div_narrow(num: i64, den: i64) -> Option<i64> {
    if den <= 0 {
        return None;
    }
    Some(
        num.checked_mul(2)?
            .checked_add(den)?
            .div_euclid(den.checked_mul(2)?),
    )
}

/// [`grid_time`] in `i64`: `None` where it does not fit, and then
/// [`grid_time_wide`] decides.
fn grid_time_narrow(base: i64, samples: u64, rate: u32, unit: u32) -> Option<i64> {
    let rate = i64::from(rate);
    let unit = i64::from(unit);
    if rate == 0 || unit == 0 {
        return None;
    }
    let n = round_div_narrow(base.checked_mul(rate)?, unit.checked_mul(90_000)?)?;
    let start = n.checked_mul(unit)?;
    let at = |s: i64| round_div_narrow(s.checked_mul(90_000)?, rate);
    let span = at(start.checked_add(i64::try_from(samples).ok()?)?)?.checked_sub(at(start)?)?;
    base.checked_add(span)
}

/// [`grid_time`] in `i128`.
fn grid_time_wide(base: i64, samples: u64, rate: u32, unit: u32) -> Option<i64> {
    let rate = i128::from(rate);
    let unit = i128::from(unit);
    if rate == 0 || unit == 0 {
        return None;
    }
    // Frame n of the grid starts at n * unit * 90 000 / rate ticks.
    let n = round_div(
        i128::from(base).checked_mul(rate)?,
        unit.checked_mul(90_000)?,
    )?;
    let start = n.checked_mul(unit)?;
    let at = |s: i128| round_div(s.checked_mul(90_000)?, rate);
    let span = at(start.checked_add(i128::from(samples))?)?.checked_sub(at(start)?)?;
    i64::try_from(i128::from(base).checked_add(span)?).ok()
}

/// Iterator over packets; see [`TransportStream::packets`].
pub struct Packets<'a, S> {
    ts: &'a TransportStream,
    source: S,
    reader: Reader,
    demux: Demux,
    queue: VecDeque<Packet>,
    /// The probe's streams, then those a later PMT version adds.
    streams: Vec<Stream>,
    by_pid: HashMap<u16, usize>,
    frames: Vec<Frames>,
    /// Bytes of audio frames carried from one PES packet to the next.
    carried: usize,
    /// Bytes of the access units waiting across every stream for a
    /// picture period, kept as a running total: a stream whose stated
    /// times never step forward holds units by the hundred thousand,
    /// and summing them on every run made the listing quadratic in
    /// their number.
    held: usize,
    /// The listing as `ffprobe` gives it ([`Packets::listing_as_ffprobe`]).
    as_ffprobe: bool,
    max_buffered: usize,
    /// Per stream: `max_num_ref_frames` of an H.264 stream's first SPS.
    max_refs: Vec<Option<u32>>,
    read: u64,
    max_read: u64,
    done: bool,
    warnings: Vec<Warning>,
}

impl TransportStream {
    /// Lists the access units of every kept stream, each stream's in
    /// storage order.
    pub fn packets<S: Source>(&self, source: S, limits: &Limits) -> Packets<'_, S> {
        let mut demux = Demux::new(&self.allowlist, limits);
        // The streams the probe kept (sniffed codecs included) are read
        // whatever the tables say; others when a PMT names them.
        demux.always = self.streams.iter().map(|s| s.pid).collect();
        // The Event Information Table is listed as a stream, so its
        // sections are its packets.
        demux.keep_epg = true;
        Packets {
            ts: self,
            source,
            reader: Reader::new(self.packet_size, self.size, self.first_packet, limits),
            demux,
            queue: VecDeque::new(),
            streams: self.streams.clone(),
            by_pid: self
                .streams
                .iter()
                .enumerate()
                .map(|(i, s)| (s.pid, i))
                .collect(),
            frames: (0..self.streams.len()).map(|_| Frames::default()).collect(),
            carried: 0,
            held: 0,
            as_ffprobe: false,
            max_buffered: usize::try_from(limits.max_buffered_bytes).unwrap_or(usize::MAX),
            max_refs: self
                .streams
                .iter()
                .map(|s| {
                    (s.codec == Codec::H264)
                        .then_some(s.config.as_deref())
                        .flatten()
                        .and_then(coreav_isobmff::avc_max_num_ref_frames)
                })
                .collect(),
            read: 0,
            max_read: limits.max_listing_packets,
            done: false,
            warnings: Vec::new(),
        }
    }
}

impl<S: Source> Packets<'_, S> {
    /// Lists the packets as `ffprobe` lists them, for a caller that
    /// reproduces its output (off by default; call it before the first
    /// packet is read):
    ///
    /// - the PES packets of H.264, HEVC, MPEG-1 and MPEG-2 video are read
    ///   in pieces of 204,800 payload bytes as they arrive, and the first
    ///   unit that starts in a later piece takes the PES packet's position;
    /// - an H.264 or HEVC access unit, or an MPEG-1 or MPEG-2 picture
    ///   (with its second field, if it is a field), is listed as soon as
    ///   the start of the next one shows where it ends, and the units after
    ///   the first one of a PES packet take no time and no position, rather
    ///   than dividing the step to the next stated time between them as a
    ///   run (MPEG video is otherwise listed a PES packet at a time);
    ///   [`Packet::fed`] is the stream id of the PES packet whose reading
    ///   finished the unit;
    /// - the bytes before an MPEG audio stream's first frame are the start
    ///   of that frame's packet, and an AC-3 or E-AC-3 frame of which its
    ///   PES packet holds fewer than seven bytes takes the next PES
    ///   packet's time and position;
    /// - the splice information sections of an SCTE 35 stream are its
    ///   packets, timed by the last program clock reference of its
    ///   program;
    /// - AAC frames a PES packet does not time fall on a grid of frames
    ///   counted from time zero, AC-3 and E-AC-3 frames one truncated frame
    ///   duration after the one before, and an AC-3 frame held for the next
    ///   one is listed once that one's header is read, even when the end of
    ///   the PES packet cuts it;
    /// - at the end of the file, what each stream still holds is listed
    ///   stream by stream.
    #[must_use]
    pub fn listing_as_ffprobe(mut self) -> Self {
        self.as_ffprobe = true;
        self.demux.pieces = Some(PES_PIECE);
        self.demux.keep_splices = true;
        self
    }

    /// Problems met so far.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// The source, so that a caller can read between calls.
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }

    /// The streams [`Packet::stream`] indexes: the probe's
    /// ([`TransportStream::streams`]), then those a later PMT version
    /// added, from the packet where they appeared.
    pub fn streams(&self) -> &[Stream] {
        &self.streams
    }

    /// The services of the Service Description Table of this transport
    /// stream read so far.
    pub fn services(&self) -> &[crate::Service] {
        &self.demux.services
    }

    /// The index of the stream on `pid`; a stream a PMT names after the
    /// probe is added, once its codec is known and allowed.
    fn index_of(&mut self, pes: &Pes) -> Option<usize> {
        if let Some(&index) = self.by_pid.get(&pes.pid) {
            return Some(index);
        }
        let entry_index = self.demux.stream_of(pes.pid)?;
        self.demux.sniffed(entry_index, pes);
        let entry = self.demux.streams.get(entry_index)?;
        if !entry.kept {
            return None;
        }
        let audio = entry.codec.frames().and_then(|syntax| {
            frames::header(syntax, &pes.payload)
                .filter(|f| f.sample_rate > 0)
                .map(|f| Audio {
                    sample_rate: f.sample_rate,
                    channels: f.channels,
                    detail: f.detail,
                })
        });
        let index = self.streams.len();
        self.streams.push(Stream {
            stream_index: entry_index,
            pid: entry.pid,
            programs: entry.programs.clone(),
            stream_type: entry.stream_type,
            codec: entry.codec,
            kind: entry.codec.kind(),
            pmt_index: entry.pmt_index,
            descriptors: entry.descriptors.clone(),
            format_identifier: entry.format_identifier,
            language: None,
            video: None,
            audio,
            config: None,
            profile: None,
            color: coreav_isobmff::Color::default(),
            mpeg2_profile: None,
            start: pes
                .header
                .pts
                .or(pes.header.dts)
                .map(|t| self.ts.unwrap(pes.pid, t)),
            end: None,
            last_pts: None,
            found_at: entry.found_at,
            with_first_table: false,
        });
        self.by_pid.insert(entry.pid, index);
        self.frames.push(Frames::default());
        self.max_refs.push(None);
        Some(index)
    }

    fn advance(&mut self) -> Result<(), Error> {
        while self.queue.is_empty() {
            let Some((offset, packet)) = self.reader.next(&mut self.source, &mut self.warnings)?
            else {
                self.demux.finish(&mut self.warnings);
                self.drain_at_end()?;
                self.flush_leftovers();
                self.done = true;
                return Ok(());
            };
            self.read += 1;
            if self.read > self.max_read {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::ListingPackets,
                    offset,
                });
            }
            self.demux.feed(offset, &packet, &mut self.warnings)?;
            self.drain()?;
        }
        Ok(())
    }

    /// Drains the PES packets the end of the file closed. In the listing
    /// as `ffprobe` gives it, the last PES packet of a video stream listed
    /// whole (MPEG-4 Visual, for one), which only the end of the file
    /// completes, carries no stream id ([`Packet::fed`]): a capture of the
    /// corpus and streams crafted by `ffmpeg`. The units of H.264, HEVC,
    /// MPEG-1 and MPEG-2 video are listed as they are cut out of the PES
    /// packets, and take theirs as the rest of the listing does.
    fn drain_at_end(&mut self) -> Result<(), Error> {
        let before = self.queue.len();
        self.drain()?;
        if self.as_ffprobe {
            for packet in self.queue.iter_mut().skip(before) {
                let whole = self.streams.get(packet.stream).is_some_and(|s| {
                    s.kind == StreamKind::Video
                        && !matches!(
                            s.codec,
                            Codec::H264 | Codec::Hevc | Codec::Mpeg1Video | Codec::Mpeg2Video
                        )
                });
                if whole {
                    packet.fed = None;
                }
            }
        }
        Ok(())
    }

    /// Lists what the demultiplexer finished, and bounds what is held.
    /// Most transport packets finish nothing: that case does no more than
    /// look.
    #[inline]
    fn drain(&mut self) -> Result<(), Error> {
        if !self.demux.sections_listed.is_empty() || !self.demux.ready.is_empty() {
            self.list_finished();
        }
        if self.carried.saturating_add(self.demux.buffered()) > self.max_buffered {
            return Err(Error::LimitExceeded {
                limit: LimitKind::BufferedBytes,
                offset: self.reader.position(),
            });
        }
        Ok(())
    }

    #[inline(never)]
    fn list_finished(&mut self) {
        // Each section of the Event Information Table is a packet of the
        // `epg` stream, header and CRC included: the file states no time
        // and no position for it, as `ffprobe` also reports. Each splice
        // information section of an SCTE 35 stream is a packet of it too,
        // timed by the last clock reference of its program (in the listing
        // as `ffprobe` gives it).
        while let Some((pid, section, clock)) = self.demux.sections_listed.pop_front() {
            if let Some(&index) = self.by_pid.get(&pid) {
                let time = clock.map(|t| self.ts.unwrap(pid, t));
                self.queue.push_back(Packet {
                    stream: index,
                    pos: None,
                    pes_offset: 0,
                    pts: time,
                    dts: time,
                    duration: None,
                    keyframe: true,
                    data: section,
                    corrupt: false,
                    fed: None,
                    split: false,
                });
            }
        }
        while let Some(pes) = self.demux.ready.pop_front() {
            if let Some(index) = self.index_of(&pes) {
                let before = self.queue.len();
                let (stream_id, corrupt) = (pes.header.stream_id, pes.corrupt);
                // In the listing as `ffprobe` gives it, a coded video
                // stream's units carry the stream id from the reading that
                // finished them ([`Au::fed`]).
                let as_ffprobe = self.as_ffprobe;
                let coded = as_ffprobe
                    && self
                        .streams
                        .get(index)
                        .is_some_and(|s| units_of(s.codec, as_ffprobe).is_some());
                self.pes(index, pes);
                // What this PES packet released of its stream takes its
                // damage, and the first of it its stream id.
                let mut first = !coded;
                for packet in self.queue.iter_mut().skip(before) {
                    if packet.stream == index {
                        packet.corrupt = corrupt;
                        if first {
                            packet.fed = Some(stream_id);
                            first = false;
                        }
                    }
                }
            }
        }
    }

    fn pes(&mut self, index: usize, pes: Pes) {
        let Some(stream) = self.streams.get(index) else {
            return;
        };
        let pts = pes.header.pts.map(|t| self.ts.unwrap(pes.pid, t));
        let dts = pes.header.dts.map(|t| self.ts.unwrap(pes.pid, t)).or(pts);
        if let Some(syntax) = stream.codec.frames() {
            self.audio(index, syntax, &pes, pts, dts);
            return;
        }
        if let Some(nal) = units_of(stream.codec, self.as_ffprobe) {
            // A piece of a PES packet ([`PES_PIECE`], in the listing as
            // `ffprobe` gives it; otherwise the whole PES packet): the
            // first unit that starts in a later piece takes the PES
            // packet's position and no time, and the first unit each
            // piece finishes takes the stream id.
            self.coded_video(
                index,
                nal,
                Piece {
                    payload: &pes.payload,
                    offset: pes.offset,
                    random_access: pes.random_access,
                    stream_id: pes.header.stream_id,
                },
                pts,
                dts,
            );
            return;
        }
        let keyframe = match stream.codec {
            Codec::Mpeg1Video | Codec::Mpeg2Video => video::mpeg2_keyframe(&pes.payload),
            // A stream whose codec is not named: nothing says where
            // decoding can start, since nothing reads the bytes. A
            // private stream (type 0x06) is read as binary data and
            // keeps the flag, as `ffprobe` also reports.
            Codec::Other if stream.stream_type != 0x06 => false,
            _ => stream.kind != StreamKind::Video || pes.random_access,
        };
        let packet = Packet {
            stream: index,
            pos: Some(pes.offset),
            pes_offset: pes.offset,
            pts,
            dts,
            duration: None,
            keyframe,
            data: pes.payload,
            corrupt: false,
            fed: None,
            split: false,
        };
        self.queue.push_back(packet);
    }

    /// Reads the access units of an H.264 or HEVC stream from one PES
    /// packet (ITU-T H.264 7.4.1.2.3, H.265 7.4.2.4.4). A unit is not
    /// bounded by the PES packet: real transport streams start a PES
    /// packet a few bytes before the access unit delimiter it carries, so
    /// the bytes a unit still needs are at the head of the next one.
    ///
    /// The first unit that starts in a PES packet takes its timestamps and
    /// position (ISO/IEC 13818-1 2.4.3.7). The units after it, in that PES
    /// packet or in ones that state no time, take no time of the file's
    /// own: they are held with it as a run, and when the next stated time
    /// shows how long the run is they divide the picture interval between
    /// them.
    ///
    /// In the listing as `ffprobe` gives it, the units after the first one
    /// of a PES packet take neither time nor position: the file states no
    /// time for them, and `ffprobe` reports none (on streams crafted with
    /// two pictures to a PES packet, with and without a stated clock). A
    /// unit is listed as soon as the start of the next one shows where it
    /// ends, as `ffprobe` lists it, which puts the first field of a frame
    /// before the PES packets of other streams that the second field waits
    /// behind.
    fn coded_video(
        &mut self,
        index: usize,
        nal: Nal,
        pes: Piece<'_>,
        pts: Option<i64>,
        dts: Option<i64>,
    ) {
        let as_ffprobe = self.as_ffprobe;
        let timed = pts.is_some() || dts.is_some();
        let Some(state) = self.frames.get_mut(index) else {
            return;
        };
        let here = Au {
            pts,
            dts,
            pos: Some(pes.offset),
            pes_offset: pes.offset,
            random_access: pes.random_access,
            fed: None,
        };
        // A unit that starts after the first one of this PES packet: the
        // run it joins gives it its times, or, as `ffprobe` gives it, it
        // has none.
        let later = if as_ffprobe {
            Au {
                pts: None,
                dts: None,
                pos: None,
                ..here
            }
        } else {
            Au { pos: None, ..here }
        };
        // The unit this PES packet goes on with, or one that starts here.
        let opens_here = state.open.is_none();
        let mut open = state.open.take().unwrap_or(Open {
            au: here,
            data: Vec::new(),
        });
        let base = open.data.len();
        self.carried = self.carried.saturating_sub(base);
        open.data.extend_from_slice(pes.payload);
        // The splitter carries its own place in the buffer: a start code,
        // or the bytes that say what its unit is, can straddle the join.
        let cuts = state.splitter.cuts(nal, &open.data);
        let mut done = Vec::new();
        let mut taken = 0usize;
        let mut first_here = opens_here;
        // Where `open.data` starts in the splitter's bytes, and where the
        // next unit starts in `open.data`.
        let mut shift = 0usize;
        let mut start = 0usize;
        let mut moved = false;
        for &cut in &cuts {
            let Some(at) = cut
                .checked_sub(shift)
                .filter(|at| *at >= start && *at <= open.data.len())
            else {
                break;
            };
            let unit = if moved {
                // Later units are copied out at their own size, and the
                // bytes they took leave the buffer once, after the loop:
                // a PES packet of many small units costs its length, not
                // its length for every unit.
                let unit = open.data.get(start..at).unwrap_or_default().to_vec();
                start = at;
                unit
            } else {
                // The first finished unit keeps the buffer and goes to the
                // caller without a copy; what is left moves to a buffer of
                // the same room.
                moved = true;
                let mut rest = Vec::with_capacity(open.data.capacity());
                rest.extend_from_slice(open.data.get(at..).unwrap_or_default());
                open.data.truncate(at);
                let unit = std::mem::replace(&mut open.data, rest);
                shift = at;
                unit
            };
            taken = cut;
            // The unit that starts here begins a run when its PES packet
            // states a time and no earlier unit of that packet took it
            // (runs are not kept in the listing as `ffprobe` gives it).
            let starts_run = !as_ffprobe && timed && !first_here;
            // A cut with no bytes before it lists no packet, but the unit
            // that starts there still begins a run.
            if !unit.is_empty() || starts_run {
                done.push((open.au, (!unit.is_empty()).then_some(unit), starts_run));
            }
            open.au = if first_here { later } else { here };
            first_here = true;
        }
        open.data.drain(..start.min(open.data.len()));
        state.splitter.drained(taken);
        self.carried = self.carried.saturating_add(open.data.len());
        state.open = Some(open);
        let mut fed = as_ffprobe.then_some(pes.stream_id);
        for (au, data, starts_run) in done {
            if let Some(data) = data {
                let au = Au {
                    fed: fed.take(),
                    ..au
                };
                self.finished(index, nal, au, data);
            }
            if starts_run {
                // Every unit of the run has been read: its length is
                // known, and so is the time that closes it.
                self.flush_run(index, dts);
            }
        }
    }

    /// Takes one finished access unit into the run being read, or, in the
    /// listing as `ffprobe` gives it, lists it.
    fn finished(&mut self, index: usize, nal: Nal, au: Au, data: Vec<u8>) {
        let max_refs = self.max_refs.get(index).copied().flatten();
        let keyframe = match nal {
            Nal::H264 => video::keyframe(Nal::H264, &data, max_refs),
            Nal::Hevc | Nal::Mpeg2 => video::keyframe(nal, &data, None),
        }
        .unwrap_or(au.random_access && au.pos.is_some());
        if !self.as_ffprobe {
            // The keyframe flag is read now, while the bytes are at hand;
            // the run keeps the unit until its times are known.
            let au = Au {
                random_access: keyframe,
                ..au
            };
            if let Some(state) = self.frames.get_mut(index) {
                self.carried = self.carried.saturating_add(data.len());
                state.run.push((au, data));
            }
            return;
        }
        self.queue.push_back(Packet {
            stream: index,
            pos: au.pos,
            pes_offset: au.pes_offset,
            pts: au.pts,
            dts: au.dts,
            duration: None,
            keyframe,
            data,
            corrupt: false,
            fed: au.fed,
            split: false,
        });
    }

    /// Lists the run of access units read since the last stated time: the
    /// first takes that stated time, the rest divide what is left until
    /// the time that closes the run (`closed_by`, the decoding time the
    /// next unit's PES packet states; `None` at the end of the file).
    fn flush_run(&mut self, index: usize, closed_by: Option<i64>) {
        let Some(state) = self.frames.get_mut(index) else {
            return;
        };
        let run = std::mem::take(&mut state.run);
        let units = run.len();
        let anchor = run.first().map(|(au, _)| (au.pts, au.dts));
        // What the units of the run divide: from the time the run starts
        // at to the one that closes it. Taking the smallest step the
        // whole stream has stated instead put the units of a run longer
        // than the shortest one in the wrong places, which a broadcast
        // capture whose runs differ in length shows.
        let gap = closed_by
            .zip(anchor.and_then(|(_, dts)| dts))
            .map(|(closes, starts)| closes - starts)
            .filter(|gap| *gap > 0);
        if let Some(step) = gap.and_then(|gap| step(gap, 1, units)) {
            state.note_period(step);
        }
        // A run no stated time closes -- the last of the file, one
        // before the first time the stream states, or one after a time
        // that goes backwards -- lasts as many picture periods as it has
        // units, the period being the shortest a closed run of the
        // stream has shown; before the first of those closes, its units
        // wait for one (`hold`).
        let period = state.period;
        let span = gap.or_else(|| period.and_then(|p| p.checked_mul(i64::try_from(units).ok()?)));
        // The units held for want of a period come before this run's:
        // they were read before it, and this run is what may have taught
        // the stream its period, so they are listed here, not at the
        // next run, which would put them after this one.
        self.release(index, period);
        self.relieve();
        // Whatever still waits, this run waits behind it: storage order
        // comes before the arithmetic. A period rounded to nothing is no
        // period, so a run can have a span to divide while the units
        // before it still have none.
        let hold = span.is_none()
            || self
                .frames
                .get(index)
                .is_some_and(|state| !state.pending.is_empty());
        let mut listed = Vec::with_capacity(units);
        for (k, (au, data)) in run.into_iter().enumerate() {
            self.carried = self.carried.saturating_sub(data.len());
            let shift = if k == 0 || hold {
                Some(0)
            } else {
                span.and_then(|s| step(s, k, units))
            };
            // A unit of a run after the first takes no time of the file's
            // own: it divides the span the stream's own stated times
            // give, as `ffprobe` divides the frame period between the
            // fields of an interlaced frame and between the pictures of
            // PES packets that state no time.
            let (pts, dts) = anchor.unwrap_or((au.pts, au.dts));
            listed.push((
                Packet {
                    stream: index,
                    pos: au.pos,
                    pes_offset: au.pes_offset,
                    pts: pts.zip(shift).map(|(t, s)| t + s),
                    dts: dts.zip(shift).map(|(t, s)| t + s),
                    duration: None,
                    keyframe: au.random_access,
                    data,
                    corrupt: false,
                    fed: None,
                    split: false,
                },
                k,
                units,
            ));
        }
        for (packet, k, units) in listed {
            if hold {
                self.carried = self.carried.saturating_add(packet.data.len());
                self.held = self.held.saturating_add(packet.data.len());
                if let Some(state) = self.frames.get_mut(index) {
                    state.pending.push((packet, k, units));
                }
            } else {
                self.queue.push_back(packet);
            }
        }
    }

    /// Lists what waits for a picture period, with the time it has, once
    /// too much of the budget is waiting. A stream whose stated times
    /// never step forward teaches no period at all, and waiting for one
    /// to the end of the file would hold every unit of it; a few such
    /// streams would exhaust `max_buffered_bytes` before the first
    /// packet was listed, on a file this reader used to list. The bound
    /// is on what waits across every stream, as that limit is, and the
    /// units are listed as the end of the file lists them.
    fn relieve(&mut self) {
        if self.held <= self.max_buffered / 4 {
            return;
        }
        for index in 0..self.frames.len() {
            let period = self.frames.get(index).and_then(|state| state.period);
            self.release(index, period.or(Some(0)));
        }
    }

    /// Lists the access units held for want of a picture period, with
    /// their shifts once the period is known (without them at the end of
    /// the file, where it never will be).
    fn release(&mut self, index: usize, period: Option<i64>) {
        let Some(state) = self.frames.get_mut(index) else {
            return;
        };
        if state.pending.is_empty() || period.is_none() {
            return;
        }
        for (mut packet, k, _units) in std::mem::take(&mut state.pending) {
            self.carried = self.carried.saturating_sub(packet.data.len());
            self.held = self.held.saturating_sub(packet.data.len());
            let shift = period
                .and_then(|p| p.checked_mul(i64::try_from(k).ok()?))
                .unwrap_or(0);
            packet.pts = packet.pts.map(|t| t + shift);
            packet.dts = packet.dts.map(|t| t + shift);
            self.queue.push_back(packet);
        }
    }

    /// Splits a PES payload into audio frames. A frame cut by the end of
    /// the PES packet continues in the next one; frames that start in the
    /// carried bytes take the time after the frame before them, and the
    /// first frame that starts in this PES packet takes its PTS and
    /// position.
    fn audio(
        &mut self,
        index: usize,
        syntax: frames::Syntax,
        pes: &Pes,
        pts: Option<i64>,
        dts: Option<i64>,
    ) {
        // What this PES packet states its times are apart; the frames it
        // times keep it, and so do the frames of their run. A distance
        // longer than a second is not one: the two times fell on
        // different sides of the 33-bit wrap, or the header is damaged.
        let offset = pts
            .zip(dts)
            .map(|(p, d)| p - d)
            .filter(|offset| offset.abs() <= 90_000)
            .unwrap_or(0);
        let stream_rate = self
            .streams
            .get(index)
            .and_then(|s| s.audio)
            .map(|a| a.sample_rate);
        // An E-AC-3 frame is listed once the next frame header shows that
        // no dependent substream follows it, and in the listing as
        // `ffprobe` gives it an AC-3 frame too, once the next shows that it
        // ended: the last frame of a PES packet waits for the next PES
        // packet.
        let as_ffprobe = self.as_ffprobe;
        let hold = self.streams.get(index).is_some_and(|s| {
            matches!(s.codec, Codec::Eac3 | Codec::Dts) || (as_ffprobe && s.codec == Codec::Ac3)
        });
        let Some(state) = self.frames.get_mut(index) else {
            return;
        };
        // A frame still short of its length takes the payload whole: its
        // header is not read again (linear in the bytes, however long the
        // frame).
        if state.need > 0 && state.leftover.len() + pes.payload.len() < state.need {
            state.leftover.extend_from_slice(&pes.payload);
            self.carried = self.carried.saturating_add(pes.payload.len());
            return;
        }
        let carried = state.leftover.len();
        self.carried = self.carried.saturating_sub(carried);
        // The carried frame's header was read in an earlier PES packet.
        let header_read = state.need > 0;
        state.need = 0;
        if carried == 0 {
            state.cut = None;
        }
        let mut data = std::mem::take(&mut state.leftover);
        data.extend_from_slice(&pes.payload);
        let mut at = 0usize;
        let mut first_in_pes = true;
        while at < data.len() {
            let Some(frame) = data.get(at..).and_then(|rest| frames::header(syntax, rest)) else {
                // The start of a header the end of the PES packet cuts: it
                // goes on in the next one, with this packet's time if it is
                // the first frame that starts here.
                if data
                    .get(at..)
                    .is_some_and(|rest| frames::may_start(syntax, rest))
                {
                    if at >= carried && first_in_pes {
                        state.cut = Some((pts, offset, pes.offset));
                    }
                    state.late = as_ffprobe
                        && syntax == frames::Syntax::Ac3
                        && state.started
                        && at >= carried
                        && data.len() - at < AC3_TIMED;
                    // Seven bytes of an AC-3 frame show `ffprobe` where it
                    // starts, though this reader needs eight to read its
                    // header: an independent frame there ends the one
                    // held before it, which is listed now (streams crafted
                    // with seven bytes of a frame at the end of a PES
                    // packet).
                    let independent = data.get(at..).is_some_and(|rest| {
                        rest.len() >= AC3_TIMED
                            && !matches!(*rest, [_, _, s0, _, _, b5, ..] if b5 >> 3 > 10 && s0 >> 6 == 1)
                    });
                    if as_ffprobe && hold && syntax == frames::Syntax::Ac3 && independent {
                        if let Some((done, _)) = state.held.take() {
                            self.carried = self.carried.saturating_sub(done.data.len());
                            self.queue.push_back(done);
                        }
                    }
                    break;
                }
                // Not a frame header here: look for the next one.
                at += 1;
                continue;
            };
            // A DTS extension substream after a core frame is part of its
            // access unit, as an E-AC-3 dependent substream is (the core
            // and the substream may be in PES packets of their own, as on
            // Blu-ray).
            let frame = if syntax == frames::Syntax::Dts {
                let substream = frame.detail == frames::DTS_SUBSTREAM;
                state.dts_core |= !substream;
                frames::Frame {
                    dependent: substream && state.dts_core,
                    ..frame
                }
            } else {
                frame
            };
            // The bytes before the stream's first frame header are the
            // tail of a frame that began before the file. `ffprobe` lists
            // them as a packet of their own with the PES packet's time and
            // position, one frame long, and the frames after them follow
            // it; so does this reader, whether or not the end of the PES
            // packet cuts the frame after them (a capture of the corpus
            // whose first AC-3 frame starts 290 bytes before the end of
            // its PES packet, and streams crafted so). MPEG audio is the
            // exception in the listing as `ffprobe` gives it: there they
            // are the start of the first frame's packet, which takes the
            // PES packet's time and position (two captures of the corpus,
            // MP2 and MP3, and streams crafted with 29 and 300 such bytes,
            // with the first frame whole and cut).
            if !state.started && at > 0 && carried == 0 {
                state.started = true;
                if self.as_ffprobe && syntax == frames::Syntax::MpegAudio {
                    state.lead = data.get(..at).unwrap_or_default().to_vec();
                    self.carried = self.carried.saturating_add(at);
                } else {
                    let rate = Some(frame.sample_rate)
                        .filter(|r| *r > 0)
                        .or(stream_rate)
                        .filter(|r| *r > 0);
                    let ticks = rate.map(|r| i64::from(frame.samples) * 90_000 / i64::from(r));
                    // E-AC-3 is the exception: `ffprobe` gives those lead
                    // bytes no duration and starts the first frame at
                    // their time, where an AC-3 lead packet lasts a frame
                    // and the frames follow it (streams crafted with 29,
                    // 100 and 300 lead bytes of each).
                    let eac3 = syntax == frames::Syntax::Ac3 && frame.detail == 1;
                    let spent = if eac3 { 0 } else { frame.samples };
                    state.anchor = pts
                        .zip(rate)
                        .map(|(time, r)| (time, u64::from(spent), r, frame.samples));
                    state.next = if eac3 {
                        pts
                    } else {
                        pts.zip(ticks).map(|(time, d)| time + d)
                    };
                    state.offset = offset;
                    first_in_pes = false;
                    self.queue.push_back(Packet {
                        stream: index,
                        pos: Some(pes.offset),
                        pes_offset: pes.offset,
                        pts,
                        dts: pts.map(|t| t - offset),
                        duration: if eac3 { None } else { ticks },
                        keyframe: true,
                        data: data.get(..at).unwrap_or_default().to_vec(),
                        corrupt: false,
                        fed: None,
                        split: false,
                    });
                }
            }
            let Some(bytes) = data.get(at..at + frame.len) else {
                // Cut by the end of the PES packet. If it is the first
                // frame that starts in this PES packet, the packet's PTS
                // and position are its own when it is complete.
                state.need = frame.len;
                if at >= carried && first_in_pes {
                    state.cut = Some((pts, offset, pes.offset));
                }
                // Not for a stream's first frame: `ffprobe` keeps the
                // first PES packet's time and position for it, however
                // little of it that packet holds (streams crafted with
                // the first frame cut at every byte count).
                state.late = as_ffprobe
                    && syntax == frames::Syntax::Ac3
                    && state.started
                    && at >= carried
                    && data.len() - at < AC3_TIMED;
                // Its header shows that the AC-3 frame (or E-AC-3 unit)
                // held before it ended: `ffprobe` lists that one now,
                // whole or cut as this frame is (a capture of the corpus
                // whose last PES packet the end of the file cuts). Only
                // in the listing as `ffprobe` gives it.
                if self.as_ffprobe && hold && !frame.dependent {
                    if let Some((done, _)) = state.held.take() {
                        self.carried = self.carried.saturating_sub(done.data.len());
                        self.queue.push_back(done);
                    }
                }
                break;
            };
            let payload = bytes.get(frame.header..).unwrap_or_default();
            if at == 0 && carried > 0 && frame.dependent {
                // The cut frame was a dependent substream: it takes no time
                // of its own, so the saved one is not for a later frame.
                state.cut = None;
            }
            if frame.dependent {
                // An E-AC-3 dependent substream belongs to the access unit
                // of the independent frame before it, in this PES packet
                // or an earlier one; a unit takes at most eight.
                if let Some((held, joined)) =
                    state.held.as_mut().filter(|(_, n)| *n < MAX_DEPENDENT)
                {
                    held.data.extend_from_slice(payload);
                    *joined += 1;
                    self.carried = self.carried.saturating_add(payload.len());
                    at += frame.len;
                    continue;
                }
            }
            state.started = true;
            let starts_here = at >= carried;
            // `given` is the time the file states for this frame, if any:
            // the PTS of the PES packet in which it starts. A frame
            // without one follows the frames before it.
            let (given, given_offset, pos) = if at == 0 && carried > 0 && state.late {
                // An AC-3 frame its PES packet held too little of: this
                // packet's time and position are its own, and the next
                // frame's too.
                state.late = false;
                state.cut = None;
                (pts, offset, Some(pes.offset))
            } else if at == 0 && carried > 0 && state.cut.is_some() {
                // The frame cut at the end of the previous PES packet, the
                // first that started there.
                let (time, cut_offset, pos) = state.cut.take().unwrap_or((None, 0, 0));
                (time, cut_offset, Some(pos))
            } else if starts_here && first_in_pes {
                first_in_pes = false;
                (pts, offset, Some(pes.offset))
            } else {
                (None, state.offset, None)
            };
            state.offset = given_offset;
            // LOAS frames without a configuration take the stream's rate.
            let rate = Some(frame.sample_rate)
                .filter(|r| *r > 0)
                .or(stream_rate)
                .filter(|r| *r > 0);
            let duration = rate.map(|r| i64::from(frame.samples) * 90_000 / i64::from(r));
            // A stated time anchors the run; a frame without one starts
            // the samples since the anchor after it. In the listing as
            // `ffprobe` gives it, it is placed as `ffprobe` places it for
            // the codec. Where a frame is not a whole number of ticks
            // (44.1 kHz), crafted streams show three rules: AAC
            // frames, in ADTS and in LOAS (a 22.05 kHz LATM stream of the
            // corpus), on the grid of [`grid_time`]; AC-3 and E-AC-3 frames
            // each one truncated frame duration after the one before (the
            // durations `ffprobe` lists, summed); MPEG audio frames, and
            // the codecs no stream showed otherwise, the samples since the
            // anchor rounded once.
            let time = match (given, rate) {
                (Some(t), Some(r)) => {
                    state.anchor = Some((t, 0, r, frame.samples));
                    Some(t)
                }
                (Some(t), None) => {
                    state.anchor = None;
                    Some(t)
                }
                (None, _) if !self.as_ffprobe => state
                    .anchor
                    .and_then(|(base, samples, r, _)| Some(base + ticks(samples, r)?))
                    .or(state.next),
                (None, _) => match syntax {
                    frames::Syntax::Ac3 => state.next,
                    frames::Syntax::Adts | frames::Syntax::Latm => state
                        .anchor
                        .and_then(|(base, samples, r, unit)| grid_time(base, samples, r, unit))
                        .or(state.next),
                    _ => state
                        .anchor
                        .and_then(|(base, samples, r, _)| {
                            let span =
                                round_div(i128::from(samples).checked_mul(90_000)?, i128::from(r))?;
                            i64::try_from(i128::from(base).checked_add(span)?).ok()
                        })
                        .or(state.next),
                },
            };
            if let Some((_, samples, _, _)) = state.anchor.as_mut() {
                *samples = samples.saturating_add(u64::from(frame.samples));
            }
            state.next = match (time, duration) {
                (Some(t), Some(d)) => Some(t + d),
                _ => None,
            };
            let mut bytes = std::mem::take(&mut state.lead);
            self.carried = self.carried.saturating_sub(bytes.len());
            bytes.extend_from_slice(payload);
            let packet = Packet {
                stream: index,
                pos,
                pes_offset: pes.offset,
                pts: time,
                dts: time.map(|t| t - given_offset),
                duration,
                keyframe: true,
                data: bytes,
                corrupt: false,
                fed: None,
                split: at == 0 && header_read,
            };
            if hold {
                self.carried = self.carried.saturating_add(packet.data.len());
                if let Some((done, _)) = state.held.replace((packet, 0)) {
                    self.carried = self.carried.saturating_sub(done.data.len());
                    self.queue.push_back(done);
                }
            } else {
                self.queue.push_back(packet);
            }
            at += frame.len;
        }
        // At most one frame is carried: the one the end of the PES packet
        // cut, whose length its header bounds.
        if let Some(rest) = data.get(at..) {
            state.leftover = rest.to_vec();
            self.carried = self.carried.saturating_add(rest.len());
        }
    }

    /// At the end of the file, a frame cut short is listed with the bytes
    /// there are, as `ffprobe` lists it; other carried bytes are dropped.
    fn flush_leftovers(&mut self) {
        if self.as_ffprobe {
            // Stream by stream, as `ffprobe` lists what the end of the
            // file finishes (an E-AC-3 stream's last frame before the last
            // picture of the video stream after it): the access unit the
            // end of the file leaves open; a unit held for the one after
            // it; a frame cut short.
            self.carried = 0;
            for index in 0..self.frames.len() {
                self.flush_open(index);
                self.flush_held(index);
            }
            return;
        }
        // The access unit the end of the file leaves open, as `ffprobe`
        // lists the last unit of a stream however it ends; then the run it
        // belongs to, with the picture interval there is; then the units
        // held for want of an interval the file never gave, listed with
        // the times of the packet they came from.
        for index in 0..self.frames.len() {
            self.flush_open(index);
            // The run that is still open, and then the units held for
            // a period the file never gave, which it holds too.
            let period = self.frames.get(index).and_then(|s| s.period);
            self.flush_run(index, None);
            self.release(index, period.or(Some(0)));
        }
        self.carried = 0;
        for index in 0..self.frames.len() {
            self.flush_held(index);
        }
    }

    /// The access unit of a coded video stream the end of the file leaves
    /// open, as `ffprobe` lists the last unit of a stream however it ends.
    fn flush_open(&mut self, index: usize) {
        let as_ffprobe = self.as_ffprobe;
        let nal = self
            .streams
            .get(index)
            .and_then(|s| units_of(s.codec, as_ffprobe));
        let open = self.frames.get_mut(index).and_then(|s| s.open.take());
        if let (Some(nal), Some(open)) = (nal, open) {
            if !open.data.is_empty() {
                self.finished(index, nal, open.au, open.data);
            }
        }
    }

    /// The unit a stream holds for the one after it, and the frame the end
    /// of the file cuts short.
    fn flush_held(&mut self, index: usize) {
        let syntax = self.streams.get(index).and_then(|s| s.codec.frames());
        let stream_rate = self
            .streams
            .get(index)
            .and_then(|s| s.audio)
            .map(|a| a.sample_rate);
        let as_ffprobe = self.as_ffprobe;
        let Some(state) = self.frames.get_mut(index) else {
            return;
        };
        let held = state.held.take();
        // The PES packet in which the cut frame started, when it is
        // the first frame that starts there: its position is the
        // frame's own, as it would be had the frame been whole.
        let cut = state.cut.take();
        state.need = 0;
        let leftover = std::mem::take(&mut state.leftover);
        let lead = std::mem::take(&mut state.lead);
        let frame = syntax.and_then(|syntax| frames::header(syntax, &leftover));
        // An E-AC-3 dependent substream or a DTS extension substream the
        // end of the file cuts is still part of the access unit before it,
        // which `ffprobe` lists with what there is of it (a Blu-ray
        // capture of the corpus cut in the extension substream of its
        // last DTS-HD frame).
        let dependent = frame.is_some_and(|f| {
            if syntax == Some(frames::Syntax::Dts) {
                f.detail == frames::DTS_SUBSTREAM && state.dts_core
            } else {
                f.dependent
            }
        });
        if let Some((mut unit, joined)) = held {
            if let (true, Some(f)) = (dependent && joined < MAX_DEPENDENT, frame) {
                unit.data
                    .extend_from_slice(leftover.get(f.header..).unwrap_or_default());
                self.queue.push_back(unit);
                return;
            }
            self.queue.push_back(unit);
        }
        let Some(frame) = frame else {
            return;
        };
        let time = cut.and_then(|(time, _, _)| time).or(state.next);
        let offset = cut.map_or(state.offset, |(_, offset, _)| offset);
        // In the listing as `ffprobe` gives it, the frame lasts what its
        // header says, cut or not.
        let rate = Some(frame.sample_rate)
            .filter(|r| *r > 0)
            .or(stream_rate)
            .filter(|r| *r > 0);
        let mut data = lead;
        data.extend_from_slice(leftover.get(frame.header..).unwrap_or_default());
        self.queue.push_back(Packet {
            stream: index,
            pos: cut.map(|(_, _, pos)| pos),
            pes_offset: cut.map_or(0, |(_, _, pos)| pos),
            pts: time,
            dts: time.map(|t| t - offset),
            duration: rate
                .filter(|_| as_ffprobe)
                .map(|r| i64::from(frame.samples) * 90_000 / i64::from(r)),
            keyframe: true,
            data,
            corrupt: false,
            fed: None,
            split: false,
        });
    }
}

impl<S: Source> Iterator for Packets<'_, S> {
    type Item = Result<Packet, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(packet) = self.queue.pop_front() {
            return Some(Ok(packet));
        }
        if self.done {
            return None;
        }
        if let Err(error) = self.advance() {
            self.done = true;
            return Some(Err(error));
        }
        self.queue.pop_front().map(Ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aac_frames_fall_on_a_grid_counted_from_zero() {
        // 1,024 samples at 44.1 kHz: 2,089.796 ticks a frame. From a PES
        // time of 43,830 (20.97 frames from zero, so frame 21 of the
        // grid), the next two frames start 2,090 and 4,179 ticks later;
        // rounding the span since the PES time gives 4,180.
        assert_eq!(grid_time(43_830, 1024, 44_100, 1024), Some(45_920));
        assert_eq!(grid_time(43_830, 2048, 44_100, 1024), Some(48_009));
        // From 50,100 (23.97 frames), the second is 4,180 later.
        assert_eq!(grid_time(50_100, 2048, 44_100, 1024), Some(54_280));
        // Whole ticks: the plain sum.
        assert_eq!(grid_time(90_000, 3 * 1024, 48_000, 1024), Some(95_760));
        assert_eq!(grid_time(0, 1024, 0, 1024), None);
    }

    #[test]
    fn the_narrow_grid_gives_what_the_wide_one_gives() {
        // The `i64` path, where it answers, answers as the `i128` one, and
        // where it overflows, the `i128` one decides.
        let bases = [
            -90_000_i64,
            -1,
            0,
            1,
            43_830,
            50_100,
            8_589_934_591,
            i64::MAX / 90_000,
            i64::MAX / 2,
            i64::MAX,
            i64::MIN,
        ];
        let samples = [0_u64, 1, 1024, 2048, 1 << 33, u64::from(u32::MAX), u64::MAX];
        let rates = [0_u32, 1, 7_350, 22_050, 44_100, 48_000, 96_000, u32::MAX];
        let units = [0_u32, 1, 960, 1024, 2048, u32::MAX];
        for &base in &bases {
            for &n in &samples {
                for &rate in &rates {
                    for &unit in &units {
                        assert_eq!(
                            grid_time(base, n, rate, unit),
                            grid_time_wide(base, n, rate, unit),
                            "{base} {n} {rate} {unit}"
                        );
                        if let Some(t) = grid_time_narrow(base, n, rate, unit) {
                            assert_eq!(Some(t), grid_time_wide(base, n, rate, unit));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn rounding_halves_up_for_either_sign() {
        assert_eq!(round_div(3, 2), Some(2));
        assert_eq!(round_div(-3, 2), Some(-1));
        assert_eq!(round_div(5, 4), Some(1));
        assert_eq!(round_div(1, 0), None);
        for (num, den) in [(3, 2), (-3, 2), (5, 4), (-7, 3), (1, 0), (i64::MAX, 3)] {
            assert_eq!(
                round_div_narrow(num, den).map(i128::from),
                round_div(i128::from(num), i128::from(den))
                    .filter(|_| num.checked_mul(2).is_some())
            );
        }
    }
}
