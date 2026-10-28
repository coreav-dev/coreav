//! Sections of an ISO base media file (MP4, MOV, M4A, 3GP) from
//! `coreav-isobmff`'s `Movie`.
//!
//! # Packet durations
//!
//! Measured on `ffprobe` 7.0.2 as a black box, with copies of real files
//! whose boxes or parameter sets were edited one at a time:
//!
//! - A sample of the movie box's tables keeps its `stts` duration, but for
//!   the cases below; a sample of a movie with fragments never does.
//! - A packet that keeps no sample duration gets one frame:
//!   - video: the frame rate H.264 parameter sets state (VUI timing,
//!     `time_scale / (2 × num_units_in_tick)`), rounded down to ticks, none
//!     (N/A) when that rate is 1,000 frames a second or more, and one tick
//!     in a time base coarser than 1/1,000 s; without one (H.265 included),
//!     the base rate `r_frame_rate`, which is one step when every sample
//!     lasts one. In fragments that rate is guessed from the first 20
//!     frames (40 in a time base coarser than 1/2,000 s), so the first 21
//!     (41) get the rate H.265 parameter sets state, else none (one tick
//!     in a time base coarser than 1/1,000 s);
//!   - audio: the decode step most of the first samples take, in whole
//!     audio samples at the configured rate, rounded down to ticks; none
//!     for any packet of a codec `ffprobe` cannot decode (AC-4, MPEG-H) or
//!     of an encrypted (`enca`) track, nor for the first AAC packet when no
//!     video track comes first, when 21 video packets or more are listed
//!     before it, or when it is a movie box sample of a movie with
//!     fragments (measured on crafted files; the cause is not established);
//!   - data and other text streams: none.
//! - Every sample of a video or audio track with a `ctts` box, even an
//!   all-zero one, and every sample of duration 0 get a frame.
//! - The last sample of the tables lasts until the stream's duration as
//!   `ffprobe` reports it (the smallest of the media header's, the `stts`
//!   sum and the edits' durations added up; for a data or text stream other
//!   than a timecode without an edit list, at least the longest audio or
//!   video stream's, else the movie's), counted from its
//!   presentation time, edits included; one that starts at or after that
//!   end gets a frame (raw audio grouped into packets keeps its group's
//!   duration).
//! - A track whose samples all last 0 ticks lists no packet.
//!
//! # Edit lists
//!
//! A track with one media edit (and at most a leading empty one) is
//! listed as `ffprobe` applies the edit (`docs/design/edit-lists.md`, "What
//! `ffprobe` lists", whose rules hold for these lists too):
//!
//! - video and other non-audio tracks: from the last sync sample at or
//!   before the first sample that lasts past the media time; times are
//!   moved so that the smallest composition time at or after the media
//!   time (else that sample's), not the media time, starts the
//!   presentation (`KD-EDIT-SNAP`); a sample whose composition time is
//!   outside the edit's window is marked discarded; a track with no sample
//!   past the media time lists nothing;
//! - audio: from the sample shown one second before the media time, or
//!   the first; the first packet's skip side data counts the samples from
//!   its start to the media time, or is the iTunes gapless delay when the
//!   file has one;
//! - an edit that ends before the media ends the listing at the first sync
//!   sample at or after its last presented sample (the second with a `ctts`
//!   box).
//!
//! A track whose samples are all in movie fragments keeps its decode times
//! (no snap), and its samples are never marked discarded.
//!
//! A track with several edits at rate 1 (overlapping and backward windows
//! included) in a movie without fragments is listed by that note's model,
//! rules 1 to 9: each media edit in turn, from its preroll sample to the
//! sync sample after its last presented one, its times anchored on the
//! first edit's snap and on each later edit's first sample's decode time,
//! so that a sample can be listed several times. The sync sample an edit's
//! listing ends on, when not presented, lasts one tick (without a `ctts`
//! box). Lists with dwells or other rates keep the library's listing (no
//! presentation times).
//!
//! Composition offsets of a version 0 `ctts` are read signed, as `ffprobe`
//! reads them (`KD-CTTS-UNSIGNED`): a negative one moves its sample's times
//! 2^32 ticks back and, as any negative offset, every decode time back.
//!
//! # Packet order and flags
//!
//! - Interleaving: each track's next packet is taken in turn, in stream
//!   order, against the best so far; within a second of each other (by
//!   decode times as listed, moved back by the smallest negative
//!   composition offset) the one stored first wins, else the one due
//!   first. The comparison is pairwise, so the result depends on the
//!   stream order, as `ffprobe`'s does.
//! - Audio of constant-size samples is grouped into packets of up to 1,024
//!   ticks of one chunk, as raw audio; a coded format's frames (AAC, AC-3
//!   and the like) are listed one by one even when all have one size.
//! - Every packet of a data track is a key packet; an empty sync sample
//!   table counts as none, every sample a sync sample.
//! - A packet of a sample whose sample description is encrypted (`encv`,
//!   `enca`) carries `Encryption info` side data. Only the first
//!   description's type is known; a fragment naming another (the clear
//!   lead-in of a DRM stream) is taken as clear.
//! - QuickTime compressed audio stored one sample per audio sample (IMA
//!   4:1, MACE, QDesign) is grouped in whole frames (the version 1 sound
//!   description's `samplesPerPacket` and `bytesPerFrame`, or the format's
//!   own), as many as fit in 1,024 samples, at least one.

use super::model::{Unit, Value, s, time};
use super::names;
use super::video::{self, VideoInfo};
use super::{Entries, Packet, PacketSource, Report, SideData, Stream, StreamKind};
use crate::compat::{self, ratio};
use crate::error::Failure;
use crate::names as cnames;
use coreav_isobmff::{Allowlist, Budget, Codec, Limits, Movie, Source, Track, TrackKind};
use std::cell::RefCell;
use std::collections::VecDeque;

/// How many packets of each track are read ahead of the listing.
const LOOKAHEAD: usize = 16;

/// How long, in ticks, the packets `ffprobe` groups samples of constant
/// size into last at most.
const GROUP_TICKS: u32 = 1024;

/// For an audio track of constant-size samples (linear PCM, and codecs such
/// as IMA ADPCM that QuickTime stores the same way) with no edit list or one
/// simple edit: how many samples make one packet, as `ffprobe` groups them
/// (up to [`GROUP_TICKS`] of one chunk; `KD-PCM`), and how far its
/// presentation and decode times lie from the media's, in its ticks.
/// `None` for every other track, listed sample by sample.
fn grouping(movie: &Movie, track: &Track, step: Option<i64>) -> Option<Group> {
    if track.kind != TrackKind::Audio || framed(track) {
        return None;
    }
    track.constant_sample_size()?;
    let step = u32::try_from(step?).ok().filter(|s| *s > 0)?;
    let frame = sound_frame(track);
    // Whole compressed frames, at least one.
    let samples = match frame {
        Some((per_frame, _)) => (GROUP_TICKS / step / per_frame)
            .max(1)
            .saturating_mul(per_frame),
        None => (GROUP_TICKS / step).max(1),
    };
    let (pts, dts) = shifts_of(movie, track)?;
    Some(Group {
        samples,
        pts,
        dts,
        frame,
    })
}

/// How [`grouping`] lists a track.
#[derive(Clone, Copy, Debug)]
struct Group {
    /// Samples per packet.
    samples: u32,
    /// Presentation time shift, in the track's ticks.
    pts: i64,
    /// Decode time shift.
    dts: i64,
    /// Samples and bytes of one compressed frame, when the samples are
    /// audio samples of a compressed QuickTime format.
    frame: Option<(u32, u32)>,
}

/// For QuickTime compressed audio whose samples are single audio samples
/// (`stsz` of 1): the samples and bytes of one compressed frame, from a
/// version 1 sound description, else the format's own (IMA 4:1, 64
/// samples in 34 bytes a channel; MACE 3:1 and 6:1, 6 samples in 2 and 1).
fn sound_frame(track: &Track) -> Option<(u32, u32)> {
    if track.constant_sample_size() != Some(1) {
        return None;
    }
    let d = &track.description;
    d.sound_packet.or_else(|| {
        let channels = d.channels.filter(|c| *c > 0)?;
        let (samples, bytes) = match &d.fourcc.0 {
            b"ima4" => (64, 34),
            b"MAC3" => (6, 2),
            b"MAC6" => (6, 1),
            _ => return None,
        };
        Some((samples, channels.checked_mul(bytes)?))
    })
}

/// A run of [`grouping`]'s compressed frames as one packet: where its
/// bytes are and how many, from the samples the run starts into its chunk
/// (`chunk`: the run offset the chunk starts at, as the tables count one
/// byte a sample, and where the previous run ended in those units).
fn frame_packet(
    run: &coreav_isobmff::SampleRun,
    (per_frame, frame_bytes): (u32, u32),
    chunk: &mut (u64, u64),
) -> Option<(u64, u32)> {
    // A run that does not continue the previous one starts a chunk.
    if run.offset != chunk.1 {
        chunk.0 = run.offset;
    }
    chunk.1 = run.offset.checked_add(u64::from(run.count))?;
    let into = run.offset.checked_sub(chunk.0)?;
    let per_frame = u64::from(per_frame.max(1));
    let offset = chunk
        .0
        .checked_add((into / per_frame).checked_mul(u64::from(frame_bytes))?)?;
    let frames = u64::from(run.count).div_ceil(per_frame);
    let size = u32::try_from(frames.checked_mul(u64::from(frame_bytes))?).ok()?;
    Some((offset, size))
}

/// Whether a track's samples are whole frames of a coded audio format,
/// which `ffprobe` lists one by one even when every frame has the same size
/// (an AAC track of silence, constant-rate AC-3).
fn framed(track: &Track) -> bool {
    matches!(
        track.description.codec,
        Codec::Aac | Codec::Mp3 | Codec::Opus | Codec::Flac
    ) || matches!(
        &track.description.fourcc.0,
        b"mp4a"
            | b"enca"
            | b"ac-3"
            | b"ec-3"
            | b"ac-4"
            | b"alac"
            | b"dtsc"
            | b"dtse"
            | b"dtsh"
            | b"dtsl"
            | b"samr"
            | b"sawb"
            | b"mha1"
            | b"mhm1"
            | b"Opus"
            | b"fLaC"
            | b".mp3"
    )
}

fn shifts_of(movie: &Movie, track: &Track) -> Option<(i64, i64)> {
    match track.edit_list {
        coreav_isobmff::EditList::None => Some((0, 0)),
        coreav_isobmff::EditList::Simple {
            empty, media_time, ..
        } => {
            let den = u128::from(movie.timescale.max(1));
            let ticks = u128::from(empty) * u128::from(track.timescale);
            let lead = i64::try_from((ticks + den / 2) / den).ok()?;
            let shift = lead.checked_sub(media_time.max(0))?;
            Some((shift, shift))
        }
        _ => None,
    }
}

/// A group of PCM samples as one packet.
fn pcm_packet(
    run: &coreav_isobmff::SampleRun,
    pts_shift: i64,
    dts_shift: i64,
) -> coreav_isobmff::Packet {
    let count = run.count.max(1);
    let cts = run.dts.saturating_add(run.composition_offset);
    let pts = cts.saturating_add(pts_shift);
    coreav_isobmff::Packet {
        sample: run.first_sample,
        offset: run.offset,
        size: run.sample_size.saturating_mul(count),
        dts: run.dts,
        cts,
        duration: run.sample_duration.saturating_mul(count),
        keyframe: run.keyframe,
        pts: Some(coreav_isobmff::Rational::from_int(i128::from(pts))),
        presentation_dts: Some(coreav_isobmff::Rational::from_int(i128::from(
            run.dts.saturating_add(dts_shift),
        ))),
        presented: true,
    }
}

/// `ffprobe`'s format name and long name for every ISO base media file.
const FORMAT_NAME: &str = "mov,mp4,m4a,3gp,3g2,mj2";
const FORMAT_LONG_NAME: &str = "QuickTime / MOV";

/// How many packets of each track are read for its start time: enough for
/// any reordering depth a stream declares.
const START_PACKETS: usize = 64;

/// A probed ISO base media file.
pub struct IsoProbed {
    /// Everything but the packets.
    pub report: Report,
    /// The movie the packets are listed from.
    pub movie: Movie,
    /// Per track, the samples the first packet skips.
    pub priming: Vec<i64>,
    /// Per track, what the packet listing takes from its stream's entries.
    pub ends: Vec<Listed>,
    /// Per track, how far decode times are moved back so that no
    /// composition offset is negative.
    pub shifts: Vec<i64>,
    /// Per track, the decode step of its first samples.
    pub steps: Vec<Option<i64>>,
    /// The cover pictures' streams: index, position and size of the one
    /// packet each, which `ffprobe` lists before every other.
    pub covers: Vec<(usize, u64, u64)>,
}

/// What the packet listing takes from a stream's entries.
#[derive(Clone, Copy, Debug, Default)]
pub struct Listed {
    /// `duration_ts`.
    pub duration: Option<i64>,
    /// `r_frame_rate`, a nonzero fraction.
    pub rate: Option<(u64, u64)>,
}

impl Listed {
    fn of(stream: &Stream, duration: Option<i64>) -> Self {
        let rate = stream
            .entries
            .iter()
            .find(|(k, _)| *k == "r_frame_rate")
            .and_then(|(_, v)| match v {
                Value::Str(text) => {
                    let (n, d) = text.split_once('/')?;
                    Some((n.parse().ok()?, d.parse().ok()?))
                }
                _ => None,
            })
            .filter(|(n, d): &(u64, u64)| *n > 0 && *d > 0);
        Self { duration, rate }
    }
}

/// How far `ffprobe` moves a track's decode times back: the most negative
/// composition offset of a `ctts` (ISO/IEC 14496-12 8.6.1.3), read signed
/// whatever its version (`KD-CTTS`, `KD-CTTS-UNSIGNED`), so that no packet
/// is presented before it is decoded; 0 when no offset is negative.
fn dts_shift<S: Source>(movie: &Movie, track: &Track, source: &mut S, limits: &Limits) -> i64 {
    if track.ctts_signed().is_none() {
        return 0;
    }
    let mut budget = Budget::new(limits);
    let Ok(iter) = movie.packets(track.index, &mut *source, limits, &mut budget) else {
        return 0;
    };
    iter.map_while(Result::ok)
        .map(|p| {
            let p = signed_offsets(track, p);
            p.dts.saturating_sub(p.cts)
        })
        .max()
        .unwrap_or(0)
        .max(0)
}

/// A sample of a version 0 `ctts` read unsigned whose offset, read signed
/// as `ffprobe` reads it, is negative: its composition and presentation
/// times 2^32 ticks earlier (`KD-CTTS-UNSIGNED`).
fn signed_offsets(track: &Track, mut p: coreav_isobmff::Packet) -> coreav_isobmff::Packet {
    const WRAP: i64 = 1 << 32;
    if track.ctts_signed() == Some(false) && p.cts.saturating_sub(p.dts) >= WRAP / 2 {
        p.cts = p.cts.saturating_sub(WRAP);
        let by = coreav_isobmff::Rational::from_int(-i128::from(WRAP));
        p.pts = p.pts.and_then(|t| t.checked_add(by));
    }
    p
}

/// Probes `source` as ISO base media.
pub fn probe<S: Source>(source: &mut S, size: u64, name: &str) -> Result<IsoProbed, Failure> {
    let probed = crate::probe::run_source(&mut *source, size, &Allowlist::listing(), true)?;
    let movie = &probed.movie;
    let limits = Limits::default();
    let mut report = Report::default();
    // The iTunes gapless tag gives the AAC priming of a file without an
    // edit list, which `ffprobe` skips as the edit list's (ISO/IEC 14496-12
    // 8.6.6 does it for files with one).
    let gapless = movie
        .itunes_gapless(&mut *source)
        .ok()
        .flatten()
        .map(|g| i64::from(g.encoder_delay))
        .filter(|d| *d > 0);
    let mut priming = Vec::with_capacity(movie.tracks.len());
    let mut shifts = Vec::with_capacity(movie.tracks.len());
    let mut steps = Vec::with_capacity(movie.tracks.len());
    // Tags, dispositions, extradata sizes, chapters and protected entries,
    // as `ffprobe` reads them from the boxes (`iso_meta`).
    let meta = super::iso_meta::read(movie, &mut *source)?;
    for (i, original) in movie.tracks.iter().enumerate() {
        // A protected entry is described by the format it wraps, as
        // `ffprobe` reports it.
        let unwrapped = meta
            .protected
            .get(i)
            .and_then(Option::as_ref)
            .map(|p| unprotected(original, p));
        let track = unwrapped.as_ref().unwrap_or(original);
        let mut head = head(movie, track, &mut *source, &limits);
        let skip = if track.kind != TrackKind::Audio {
            0
        } else if let Some(first) = head.first_pts.filter(|p| *p < 0) {
            first.checked_neg().unwrap_or(0)
        } else if track.edit_list == coreav_isobmff::EditList::None {
            gapless.unwrap_or(0)
        } else {
            0
        };
        if skip > 0 {
            head.start = head.first_pts.and_then(|p| p.checked_add(skip));
        }
        // With one edit, presentation starts at the leading empty edit, in
        // the track's ticks.
        if let coreav_isobmff::EditList::Simple { empty, .. } = track.edit_list {
            if movie.timescale > 0 {
                let ticks = u128::from(empty) * u128::from(track.timescale);
                let den = u128::from(movie.timescale);
                head.start = i64::try_from((ticks + den / 2) / den).ok();
            }
        }
        priming.push(skip);
        shifts.push(dts_shift(movie, track, &mut *source, &limits));
        steps.push(head.steps.first().copied());
        let info = video_info(track, &head, &mut *source);
        report
            .time_bases
            .push((1, i64::from(track.timescale.max(1))));
        let mut stream = stream(movie, track, &head, info);
        if unwrapped.is_some() && meta.protection_headers {
            stream.side_data.push(SideData {
                typ: "encryption_initialization_data",
                entries: vec![("side_data_type", s("Encryption initialization data"))],
            });
        }
        report.streams.push(stream);
    }
    report.format_tags = meta.format_tags;
    report.chapters = meta.chapters;
    for (i, stream) in report.streams.iter_mut().enumerate() {
        if let Some(tags) = meta.stream_tags.get(i) {
            stream.tags.clone_from(tags);
        }
        if let Some(disposition) = meta.dispositions.get(i) {
            stream.disposition.clone_from(disposition);
        }
        if let Some(Some((scale, frame))) = meta.timecode_rates.get(i) {
            for (k, v) in &mut stream.entries {
                if *k == "avg_frame_rate" {
                    *v = s(format!("{scale}/{frame}"));
                }
            }
        }
        // A `fiel` box names the field order where the codec's own
        // configuration does not (H.265 written by `ffmpeg`, which adds
        // the box for an encoder that reports its order).
        if let Some(Some(order)) = meta.field_orders.get(i) {
            for (k, v) in &mut stream.entries {
                if *k == "field_order" && matches!(v, Value::Unset(_)) {
                    *v = s(*order);
                }
            }
        }
        if let Some(Some(layout)) = meta.pcm_layouts.get(i) {
            for (k, v) in &mut stream.entries {
                if *k == "channel_layout" {
                    *v = layout.map_or(Value::Unset("unknown"), s);
                }
            }
        }
        stream.entries.retain(|(k, _)| *k != "extradata_size");
        if let Some(Some(n)) = meta.extradata_sizes.get(i) {
            let at = stream
                .entries
                .iter()
                .position(|(k, _)| *k == "nb_read_packets")
                .map_or(stream.entries.len(), |p| p + 1);
            stream.entries.insert(at, ("extradata_size", int(*n)));
        }
    }
    let spans: Vec<(StreamKind, i64, Option<i64>, Option<i64>)> = report
        .streams
        .iter()
        .zip(&report.time_bases)
        .map(|(st, (_, ts))| {
            let get = |k: &str| {
                st.entries
                    .iter()
                    .find(|(key, _)| *key == k)
                    .and_then(|(_, v)| match v {
                        Value::Int(n) => Some(*n),
                        _ => None,
                    })
            };
            (st.kind, *ts, get("start_pts"), get("duration_ts"))
        })
        .collect();
    report.format = format(movie, &probed, name, &spans);
    let ends: Vec<Listed> = report
        .streams
        .iter()
        .zip(&spans)
        .map(|(st, (_, _, _, duration))| Listed::of(st, *duration))
        .collect();
    let covers = cover_streams(&mut report, &spans, &meta.covers);
    Ok(IsoProbed {
        report,
        movie: probed.movie,
        priming,
        ends,
        shifts,
        steps,
        covers,
    })
}

/// The time base of a cover picture's stream.
const COVER_TIME_BASE: i64 = 90_000;

/// Each cover picture whose JPEG or PNG header can be read becomes an
/// attached-picture stream after the tracks, as `ffprobe` lists it: the
/// picture's own properties, `id` 0x0, the format's start and duration in
/// 1/90000, and one untimed packet holding the picture. Returns each
/// stream's index, and its packet's position and size.
fn cover_streams(
    report: &mut Report,
    spans: &[(StreamKind, i64, Option<i64>, Option<i64>)],
    covers: &[super::iso_meta::Cover],
) -> Vec<(usize, u64, u64)> {
    let (start, duration) = span(spans);
    let ticks = |us: i64| {
        let n = i128::from(us) * i128::from(COVER_TIME_BASE);
        i64::try_from((2 * n + 1_000_000).div_euclid(2_000_000)).ok()
    };
    let mut out = Vec::new();
    for cover in covers {
        let picture = match cover.data.get(..4) {
            Some([0xff, 0xd8, ..]) => super::mkv::image::jpeg(&cover.data),
            Some([0x89, b'P', b'N', b'G']) => super::mkv::image::png(&cover.data),
            _ => None,
        };
        let Some(picture) = picture else {
            continue;
        };
        let index = report.streams.len();
        let mut stream = super::mkv::picture_stream(
            &picture,
            index,
            start.and_then(ticks),
            duration.and_then(ticks),
            Vec::new(),
        );
        for (key, value) in &mut stream.entries {
            if *key == "id" {
                *value = s("0x0");
            }
        }
        report.streams.push(stream);
        report.time_bases.push((1, COVER_TIME_BASE));
        let size = u64::try_from(cover.data.len()).unwrap_or(u64::MAX);
        out.push((index, cover.offset, size));
    }
    if !out.is_empty() {
        let streams = int(report.streams.len());
        for (key, value) in &mut report.format {
            if *key == "nb_streams" {
                *value = streams.clone();
            }
        }
    }
    out
}

/// A track whose protected sample entry is described as the format it
/// wraps: the original sample entry type, its codec, and the codec
/// configuration box found in the entry.
fn unprotected(track: &Track, wrapped: &super::iso_meta::Protected) -> Track {
    let mut t = track.clone();
    let d = &mut t.description;
    d.fourcc = coreav_isobmff::FourCc(wrapped.original);
    let config = wrapped.config.as_ref();
    let oti = config
        .filter(|(kind, _)| kind == b"esds")
        .and_then(|(_, data)| object_type(data));
    d.codec = match &wrapped.original {
        b"avc1" | b"avc3" => Codec::H264,
        b"hvc1" | b"hev1" => Codec::Hevc,
        b"av01" => Codec::Av1,
        b"vp09" => Codec::Vp9,
        b"Opus" => Codec::Opus,
        b"fLaC" => Codec::Flac,
        b"mp4a" if matches!(oti, Some(0x69 | 0x6b)) => Codec::Mp3,
        b"mp4a" if matches!(oti, None | Some(0x40 | 0x66..=0x68)) => Codec::Aac,
        _ => Codec::Other,
    };
    if let Some((kind, data)) = config {
        d.codec_config = Some(coreav_isobmff::CodecConfig {
            kind: coreav_isobmff::FourCc(*kind),
            data: data.clone(),
        });
        if let Some(record) =
            coreav_isobmff::describe_config_record(coreav_isobmff::FourCc(*kind), data)
        {
            d.profile = record.profile;
            if record.parameter_set {
                d.parameter_set_color = Some(record.color);
            }
        } else if kind == b"esds" {
            d.profile = esds_config(data).and_then(coreav_isobmff::describe_audio_specific_config);
        }
    }
    t
}

/// The channel layout `ffprobe` names for an AAC track, from its
/// `AudioSpecificConfig` (ISO/IEC 14496-3 1.6.2.1): its
/// `channelConfiguration`, or for 0 the program config element of its
/// `GASpecificConfig` (4.4.1.1). `ffprobe` names an element list only as
/// below, measured on every layout `ffmpeg` encodes with one: a front
/// channel pair with an LFE is `2.1`, with a back pair `quad`; lists with
/// a front centre after the pair (hexagonal, 3.1, 6.0 and the others) are
/// not named. `None` when the track is not AAC or its configuration
/// cannot be read.
fn aac_layout(d: &coreav_isobmff::SampleDescription) -> Option<Option<&'static str>> {
    if d.codec != Codec::Aac {
        return None;
    }
    let config = d
        .codec_config
        .as_ref()
        .filter(|c| c.kind.0 == *b"esds")
        .and_then(|c| esds_config(&c.data))?;
    let mut bits = video::Bits::new(config);
    let mut object_type = bits.bits(5)?;
    if object_type == 31 {
        object_type = 32 + bits.bits(6)?;
    }
    if bits.bits(4)? == 15 {
        bits.bits(24)?;
    }
    let config = bits.bits(4)?;
    if config != 0 {
        return Some(u8::try_from(config).ok().and_then(names::aac_layout));
    }
    // `GASpecificConfig` of the general audio object types.
    if !matches!(object_type, 1..=4 | 6 | 7 | 17 | 19..=23) {
        return Some(None);
    }
    bits.bits(1)?;
    if bits.bits(1)? == 1 {
        bits.bits(14)?;
    }
    bits.bits(1)?;
    // `program_config_element()`.
    bits.bits(4 + 2 + 4)?;
    let front = bits.bits(4)?;
    let side = bits.bits(4)?;
    let back = bits.bits(4)?;
    let lfe = bits.bits(2)?;
    bits.bits(3 + 4)?;
    for width in [4, 4, 3] {
        if bits.bits(1)? == 1 {
            bits.bits(width)?;
        }
    }
    let mut pairs = |n: u32| -> Option<Vec<bool>> {
        (0..n)
            .map(|_| {
                let pair = bits.bits(1)? == 1;
                bits.bits(4)?;
                Some(pair)
            })
            .collect()
    };
    let front = pairs(front)?;
    let side = pairs(side)?;
    let back = pairs(back)?;
    Some(
        match (front.as_slice(), side.as_slice(), back.as_slice(), lfe) {
            ([true], [], [], 1) => Some("2.1"),
            ([true], [], [true], 0) => Some("quad"),
            _ => None,
        },
    )
}

/// `bits_per_raw_sample` of integer PCM of 24 and 32 bits, which
/// `ffprobe` states and does not for 16 bits or floating point.
fn pcm_raw_bits(track: &Track) -> Option<u32> {
    let pcm = track.description.pcm?;
    (!pcm.float && matches!(pcm.bits, 24 | 32)).then_some(u32::from(pcm.bits))
}

/// The `DecoderSpecificInfo` of an `esds` payload (ISO/IEC 14496-1
/// 7.2.6.7).
fn esds_config(esds: &[u8]) -> Option<&[u8]> {
    let mut at = 4;
    let descriptor = |at: &mut usize| -> Option<(u8, usize)> {
        let tag = *esds.get(*at)?;
        *at += 1;
        let mut len = 0usize;
        for _ in 0..4 {
            let b = *esds.get(*at)?;
            *at += 1;
            len = (len << 7) | usize::from(b & 0x7f);
            if b & 0x80 == 0 {
                break;
            }
        }
        Some((tag, len))
    };
    let (tag, _) = descriptor(&mut at)?;
    if tag != 3 {
        return None;
    }
    let flags = *esds.get(at + 2)?;
    at += 3;
    if flags & 0x80 != 0 {
        at += 2;
    }
    if flags & 0x40 != 0 {
        at += 1 + usize::from(*esds.get(at)?);
    }
    if flags & 0x20 != 0 {
        at += 2;
    }
    let (tag, _) = descriptor(&mut at)?;
    if tag != 4 {
        return None;
    }
    at += 13;
    let (tag, len) = descriptor(&mut at)?;
    (tag == 5).then(|| esds.get(at..at.checked_add(len)?))?
}

/// The `codec_name` of a track by its sample entry type alone, for the
/// codecs CoreAV does not identify.
fn codec_name_of(track: &Track) -> Option<String> {
    let d = &track.description;
    let oti = d
        .codec_config
        .as_ref()
        .filter(|c| c.kind.0 == *b"esds")
        .and_then(|c| object_type(&c.data));
    names::iso_codec(&d.fourcc.0, oti).map(str::to_owned)
}

/// What the first packets of a track show.
#[derive(Default)]
struct Head {
    /// The smallest presentation time of the presented ones, in ticks.
    start: Option<i64>,
    /// How many packets at most come before one they are shown after: the
    /// reordering depth.
    reorder: u32,
    /// Where the first packet is, and its size.
    first: Option<(u64, u32)>,
    /// The decode time steps between the first packets, in ticks.
    steps: Vec<i64>,
    /// The presentation time of the first packet.
    first_pts: Option<i64>,
    /// The decode times of the first packets, in ticks.
    decode_times: Vec<i64>,
}

fn head<S: Source>(movie: &Movie, track: &Track, source: &mut S, limits: &Limits) -> Head {
    let mut budget = Budget::new(limits);
    let Ok(iter) = movie.packets(track.index, &mut *source, limits, &mut budget) else {
        return Head::default();
    };
    let packets: Vec<coreav_isobmff::Packet> =
        iter.take(START_PACKETS).map_while(Result::ok).collect();
    let pts: Vec<i64> = packets
        .iter()
        .filter_map(|p| p.pts.and_then(|r| i64::try_from(r.round()).ok()))
        .collect();
    let mut reorder = 0;
    for (i, p) in pts.iter().enumerate() {
        let later = pts.iter().take(i).filter(|q| *q > p).count();
        reorder = reorder.max(u32::try_from(later).unwrap_or(u32::MAX));
    }
    Head {
        start: packets
            .iter()
            .filter(|p| p.presented)
            .filter_map(|p| p.pts.and_then(|r| i64::try_from(r.round()).ok()))
            .min(),
        reorder,
        first: packets.first().map(|p| (p.offset, p.size)),
        decode_times: packets.iter().map(|p| p.dts).collect(),
        first_pts: packets
            .first()
            .and_then(|p| p.pts)
            .and_then(|r| i64::try_from(r.round()).ok()),
        steps: packets
            .windows(2)
            .filter_map(|w| match w {
                [a, b] => Some(b.dts - a.dts),
                _ => None,
            })
            .filter(|d| *d > 0)
            .collect(),
    }
}

/// The picture properties of a video track's configuration.
fn video_info<S: Source>(track: &Track, head: &Head, source: &mut S) -> Option<VideoInfo> {
    let d = &track.description;
    if track.kind != TrackKind::Video {
        return None;
    }
    let full_range = compat::reported_color(track).full_range == Some(true);
    let config = d.codec_config.as_ref();
    let record = |kind: &[u8; 4]| {
        config
            .filter(|c| c.kind.0 == *kind)
            .map(|c| c.data.as_slice())
    };
    match d.codec {
        Codec::H264 => {
            let mut info = record(b"avcC").and_then(|r| video::avc(r, full_range))?;
            if info.has_b_frames.is_none() {
                info.has_b_frames = Some(head.reorder);
            }
            Some(info)
        }
        Codec::Hevc => record(b"hvcC").and_then(|r| video::hevc(r, full_range)),
        Codec::Av1 => record(b"av1C").and_then(video::av1),
        Codec::Vp9 => record(b"vpcC").and_then(video::vp9),
        Codec::Other if codec_name_of(track).as_deref() == Some("mpeg4") => {
            let dsi = config
                .filter(|c| c.kind.0 == *b"esds")
                .and_then(|c| esds_config(&c.data))?;
            let m = super::mkv::video::mpeg4(dsi)?;
            Some(VideoInfo {
                pix_fmt: Some("yuv420p".to_owned()),
                has_b_frames: m.low_delay.map(|l| u32::from(!l)).or(Some(0)),
                chroma_location: Some("left"),
                profile: m
                    .profile_and_level
                    .and_then(super::mkv::video::mpeg4_profile)
                    .map(|(name, level)| (name.to_owned(), i64::from(level))),
                mpeg4_flags: Some((m.quarter_sample, m.divx_packed)),
                sar: video::mpeg4_aspect(dsi),
                ..VideoInfo::default()
            })
        }
        Codec::ProRes => {
            let (offset, size) = head.first?;
            let mut frame = vec![0u8; usize::try_from(size.min(32)).ok()?];
            source.read_exact_at(offset, &mut frame).ok()?;
            video::prores(&frame)
        }
        _ => None,
    }
}

/// Whether a QuickTime chapter track is listed as binary data, as
/// `ffprobe` lists a track a `chap` reference names
/// (`KD-CHAPTER-TEXT`).
fn chapter_data(movie: &Movie, track: &Track) -> bool {
    matches!(
        track.description.codec,
        Codec::QuickTimeText | Codec::TimedText
    ) && movie.chapter_references.contains(&track.track_id)
}

/// A track `ffprobe` lists as a data stream of no known codec whatever
/// its handler: WebVTT in ISO base media (`wvtt`, ISO/IEC 14496-30).
fn as_data(track: &Track) -> bool {
    track.description.fourcc.0 == *b"wvtt"
}

/// The `objectTypeIndication` of an `esds` payload (ISO/IEC 14496-1
/// 7.2.6.5 and 7.2.6.6).
fn object_type(esds: &[u8]) -> Option<u8> {
    // Version and flags, then the ES descriptor.
    let mut at = 4;
    let descriptor = |at: &mut usize| -> Option<(u8, usize)> {
        let tag = *esds.get(*at)?;
        *at += 1;
        let mut len = 0usize;
        for _ in 0..4 {
            let b = *esds.get(*at)?;
            *at += 1;
            len = (len << 7) | usize::from(b & 0x7f);
            if b & 0x80 == 0 {
                break;
            }
        }
        Some((tag, len))
    };
    let (tag, _) = descriptor(&mut at)?;
    if tag != 3 {
        return None;
    }
    let flags = *esds.get(at + 2)?;
    at += 3;
    if flags & 0x80 != 0 {
        at += 2;
    }
    if flags & 0x40 != 0 {
        at += 1 + usize::from(*esds.get(at)?);
    }
    if flags & 0x20 != 0 {
        at += 2;
    }
    let (tag, _) = descriptor(&mut at)?;
    (tag == 4).then(|| esds.get(at).copied())?
}

/// `ffprobe`'s codec name for a track.
fn codec_name(movie: &Movie, track: &Track) -> Option<String> {
    let d = &track.description;
    if as_data(track) {
        return None;
    }
    if chapter_data(movie, track) {
        return Some("bin_data".to_owned());
    }
    match d.codec {
        Codec::Other | Codec::QuickTimeText => {
            let oti = d
                .codec_config
                .as_ref()
                .filter(|c| c.kind.0 == *b"esds")
                .and_then(|c| object_type(&c.data));
            names::iso_codec(&d.fourcc.0, oti)
                .or(if d.codec == Codec::QuickTimeText {
                    Some("mov_text")
                } else {
                    None
                })
                .map(str::to_owned)
        }
        _ => cnames::ffprobe_codec(d.codec, d.pcm),
    }
}

fn kind(track: &Track) -> StreamKind {
    match track.kind {
        TrackKind::Video => StreamKind::Video,
        TrackKind::Audio => StreamKind::Audio,
        TrackKind::Subtitle => StreamKind::Subtitle,
        TrackKind::Timecode | TrackKind::Data => StreamKind::Data,
    }
}

fn int(v: impl TryInto<i64>) -> Value {
    v.try_into().map_or(Value::Na, Value::Int)
}

fn stream(movie: &Movie, track: &Track, head: &Head, info: Option<VideoInfo>) -> Stream {
    let start = head.start;
    let info = info.unwrap_or_default();
    let d = &track.description;
    let video = track.kind == TrackKind::Video;
    let audio = track.kind == TrackKind::Audio;
    let codec_name = codec_name(movie, track);
    let mut e: Entries = Vec::with_capacity(48);
    e.push(("index", int(track.stream_index)));
    match &codec_name {
        Some(n) => {
            e.push(("codec_name", s(n.as_str())));
            e.push((
                "codec_long_name",
                names::long_name(n).map_or(Value::Unset("unknown"), s),
            ));
        }
        None => {
            e.push(("codec_name", Value::Unset("unknown")));
            e.push(("codec_long_name", Value::Unset("unknown")));
        }
    }
    e.push((
        "profile",
        d.profile
            .and_then(cnames::profile)
            .or_else(|| info.profile.as_ref().map(|(name, _)| name.clone()))
            .map_or(Value::Unset("unknown"), s),
    ));
    e.push((
        "codec_type",
        s(if chapter_data(movie, track) || as_data(track) {
            "data"
        } else {
            cnames::ffprobe_kind(track.kind)
        }),
    ));
    e.push(("codec_tag_string", s(compat::tag_string(d.fourcc))));
    e.push((
        "codec_tag",
        s(format!("0x{:08x}", u32::from_le_bytes(d.fourcc.0))),
    ));
    // The bit rate of a constant-rate coding: rate, channels and bits.
    let mut coded_rate: Option<i64> = None;
    if video {
        let w = d.width.map(u32::from);
        let h = d.height.map(u32::from);
        e.push(("width", w.map_or(Value::Na, int)));
        e.push(("height", h.map_or(Value::Na, int)));
        // An H.264 decoder takes the entry's size as its coded size when it
        // is no larger than the parameter set's cropped size and falls in
        // the same macroblocks, in both directions, and the cropping keeps
        // the top left corner; else the cropped size. Found on entries
        // patched to sizes around a 1920×1080 set (1905×1080 and 1920×1073
        // are taken, 1904×1080, 1920×1072 and 1921×1080 are not), and on
        // the development set (854×480 in a 864×480 set, 581×375 in
        // 582×376, but not 356×365 in 356×364).
        let fits = |entry: u32, set: u32| entry <= set && entry.div_ceil(16) == set.div_ceil(16);
        let coded = match (info.h264_corner, w, h) {
            (Some((sw, sh)), Some(w), Some(h)) if fits(w, sw) && fits(h, sh) => Some((w, h)),
            _ => info.coded_size,
        };
        let (cw, ch) = coded.map_or((w, h), |(cw, ch)| (Some(cw), Some(ch)));
        e.push(("coded_width", cw.map_or(Value::Na, int)));
        e.push(("coded_height", ch.map_or(Value::Na, int)));
        e.push(("closed_captions", Value::Int(0)));
        e.push(("film_grain", Value::Int(0)));
        // An H.264 stream that reorders frames, played through several
        // edits, is fed to the decoder edit after edit, its presentation
        // times going back at every edit, and `ffprobe` then reports the
        // decoder's deepest delay, 16 frames: on streams crafted with two
        // and three edits, contiguous, overlapping or around an empty one;
        // not with one edit, nor for H.265.
        let multiple = track.edit_list == coreav_isobmff::EditList::Multiple;
        let has_b_frames = match info.has_b_frames {
            Some(n) if n > 0 && multiple && d.codec == Codec::H264 => Some(16),
            other => other,
        };
        e.push(("has_b_frames", has_b_frames.map_or(Value::Na, int)));
        // The container's ratio (`pasp`), else the one the stream signals.
        let (sar, dar) = match (d.pixel_aspect.or(info.sar), w, h) {
            (Some((hs, vs)), Some(w), Some(h)) => (
                ratio(u64::from(hs), u64::from(vs)),
                ratio(
                    u64::from(w).saturating_mul(u64::from(hs)),
                    u64::from(h).saturating_mul(u64::from(vs)),
                ),
            ),
            _ => (None, None),
        };
        e.push(("sample_aspect_ratio", sar.map_or(Value::Na, s)));
        e.push(("display_aspect_ratio", dar.map_or(Value::Na, s)));
        e.push((
            "pix_fmt",
            info.pix_fmt.clone().map_or(Value::Unset("unknown"), s),
        ));
        e.push((
            "level",
            Value::Int(
                d.profile
                    .and_then(cnames::level)
                    .map(i64::from)
                    .or_else(|| info.profile.as_ref().map(|(_, level)| *level))
                    .unwrap_or(-99),
            ),
        ));
        let color = compat::reported_color(track);
        e.push((
            "color_range",
            color
                .full_range
                .map_or(Value::Unset("unknown"), |f| s(if f { "pc" } else { "tv" })),
        ));
        e.push((
            "color_space",
            color
                .matrix
                .and_then(cnames::matrix)
                .map_or(Value::Unset("unknown"), s),
        ));
        e.push((
            "color_transfer",
            color
                .transfer
                .and_then(cnames::transfer)
                .map_or(Value::Unset("unknown"), s),
        ));
        e.push((
            "color_primaries",
            color
                .primaries
                .and_then(cnames::primaries)
                .map_or(Value::Unset("unknown"), s),
        ));
        e.push((
            "chroma_location",
            info.chroma_location
                .filter(|l| *l != "unspecified")
                .map_or(Value::Unset("unspecified"), s),
        ));
        e.push((
            "field_order",
            info.field_order.map_or(Value::Unset("unknown"), s),
        ));
        e.push(("refs", Value::Int(1)));
        if let Some((quarter, packed)) = info.mpeg4_flags {
            let flag = |b: bool| s(if b { "true" } else { "false" });
            e.push((
                "quarter_sample",
                quarter.map_or(Value::Unset("unknown"), flag),
            ));
            e.push(("divx_packed", flag(packed)));
        }
        if d.codec == Codec::H264 {
            let length = d
                .codec_config
                .as_ref()
                .filter(|c| c.kind.0 == *b"avcC")
                .and_then(|c| c.data.get(4))
                .map(|b| (b & 3) + 1);
            e.push(("is_avc", s(if length.is_some() { "true" } else { "false" })));
            e.push(("nal_length_size", s(length.unwrap_or(0).to_string())));
        }
    } else if audio {
        let rate = d.configured_sample_rate.or(d.sample_rate);
        let channels = if d.implicit_ps_possible {
            Some(1)
        } else {
            d.channels
        };
        let bits = d.pcm.map(|p| p.bits);
        // IMA ADPCM codes 4 bits a sample.
        let coded_bits =
            bits.or_else(|| (codec_name.as_deref() == Some("adpcm_ima_qt")).then_some(4));
        coded_rate = coded_bits.zip(rate).zip(channels).and_then(|((b, r), c)| {
            i64::from(b)
                .checked_mul(i64::from(r))?
                .checked_mul(i64::from(c))
        });
        e.push((
            "sample_fmt",
            codec_name
                .as_deref()
                .and_then(|n| names::sample_fmt(n, bits))
                .map_or(Value::Unset("unknown"), s),
        ));
        e.push((
            "sample_rate",
            rate.map_or(Value::Na, |r| Value::Unit(i64::from(r), Unit::Hertz)),
        ));
        e.push(("channels", channels.map_or(Value::Na, int)));
        // AAC is named by its own configuration ([`aac_layout`]).
        let layout = aac_layout(d).unwrap_or_else(|| channels.and_then(names::default_layout));
        e.push(("channel_layout", layout.map_or(Value::Unset("unknown"), s)));
        e.push(("bits_per_sample", int(coded_bits.map_or(0, i64::from))));
        // Opus states its pre-skip (RFC 7845 5.1, in the `dOps` box of
        // the Opus in ISOBMFF encapsulation 4.3.2).
        let padding = d
            .codec_config
            .as_ref()
            .filter(|c| d.codec == Codec::Opus && c.kind.0 == *b"dOps")
            .and_then(|c| match *c.data.get(2..4)? {
                [hi, lo] => Some(i64::from(u16::from_be_bytes([hi, lo]))),
                _ => None,
            })
            .unwrap_or(0);
        e.push(("initial_padding", Value::Int(padding)));
    } else if let Some((w, h)) = compat::text_size(movie, track) {
        e.push(("width", int(w)));
        e.push(("height", int(h)));
    } else if track.kind == TrackKind::Subtitle && !chapter_data(movie, track) && !as_data(track) {
        // A subtitle stream has a size, unknown when the entry states none.
        e.push(("width", Value::Na));
        e.push(("height", Value::Na));
    }
    e.push(("id", s(format!("0x{:x}", track.track_id))));
    let frames = u64::from(track.sample_count).saturating_add(track.fragment_samples);
    let duration_ts = stream_duration(movie, track);
    let timescale = i64::from(track.timescale.max(1));
    if video {
        // Frames over the decode durations of every sample, edits aside.
        let avg = Some(track.sample_duration)
            .filter(|d| *d > 0)
            .or_else(|| duration_ts.and_then(|d| u64::try_from(d).ok()))
            .filter(|d| *d > 0)
            .and_then(|d| {
                let n = u64::try_from(timescale).ok()?.checked_mul(frames)?;
                rate(n, d)
            })
            .unwrap_or_else(|| "0/0".to_owned());
        let real =
            real_rate(&head.decode_times, track.timescale).unwrap_or_else(|| "0/0".to_owned());
        e.push(("r_frame_rate", s(real)));
        e.push(("avg_frame_rate", s(avg)));
    } else {
        e.push(("r_frame_rate", s("0/0")));
        e.push(("avg_frame_rate", s("0/0")));
    }
    e.push(("time_base", s(format!("1/{}", track.timescale))));
    e.push(("start_pts", start.map_or(Value::Na, Value::Int)));
    e.push((
        "start_time",
        start.map_or(Value::Na, |v| time(v, 1, timescale)),
    ));
    e.push(("duration_ts", duration_ts.map_or(Value::Na, Value::Int)));
    e.push((
        "duration",
        duration_ts.map_or(Value::Na, |v| time(v, 1, timescale)),
    ));
    // For audio whose edit starts inside the media (priming), the average
    // the container declares (`btrt`, else `esds`); else the bytes over the
    // stream's duration. Read from `ffprobe`'s output on the development
    // set, per stream kind and edit: video takes the bytes over the
    // duration nearly always, audio with an edit from media time 0 or none
    // too (30 of 30 streams), audio with an edit from a later media time
    // the declared average (54 of 57), as with several edits (the fixtures).
    let declared = d
        .declared_bit_rate
        .filter(|_| {
            audio
                && !movie.has_fragments()
                && match track.edit_list {
                    coreav_isobmff::EditList::Simple { media_time, .. } => media_time > 0,
                    coreav_isobmff::EditList::None => false,
                    _ => true,
                }
        })
        .map(|b| b.average)
        .filter(|b| *b > 0)
        .map(i64::from);
    // The track's own duration: a data track's filled span does not count.
    // In a fragmented movie with a segment index, `ffprobe` counts the
    // bytes of the first fragment only (found on the development set: the
    // index lets it stop reading there), over the whole duration.
    let first_only = movie.has_fragments() && !movie.segment_indexes.is_empty();
    let bytes = if first_only {
        track.first_fragment_bytes
    } else {
        track.sample_bytes
    };
    // Video over its whole media, edits aside (8 of 8 edited video streams
    // of the development set whose bit rate differed otherwise), and audio
    // whose edit starts inside the media when it declares no average (MP3
    // in QuickTime's `.mp3` entry: bytes over the 97,105 ticks of media,
    // not the 96,000 of the edit).
    let primed = matches!(
        track.edit_list,
        coreav_isobmff::EditList::Simple { media_time, .. } if media_time > 0
    );
    let span = if video || (audio && primed) {
        let samples = i64::try_from(track.sample_duration).ok().filter(|d| *d > 0);
        let header = i64::try_from(track.mdhd_duration).ok().filter(|d| *d > 0);
        match (samples, header) {
            (Some(s), Some(h)) if !movie.has_fragments() => Some(s.min(h)),
            (s, h) => s.or(h),
        }
    } else {
        None
    };
    let bit_rate = coded_rate.or(declared).or_else(|| {
        let own = span.or_else(|| media_duration(movie, track))?;
        let ticks = u128::try_from(own).ok().filter(|t| *t > 0)?;
        let bits = u128::from(bytes?)
            .checked_mul(8)?
            .checked_mul(u128::from(track.timescale))?;
        // The rate of a fragmented movie is rounded to the nearest (the
        // fixtures, with and without a segment index), that of a whole one
        // truncated.
        let rounded = if movie.has_fragments() { ticks / 2 } else { 0 };
        i64::try_from(bits.checked_add(rounded)? / ticks).ok()
    });
    e.push((
        "bit_rate",
        bit_rate
            .filter(|b| *b > 0)
            .map_or(Value::Na, |b| Value::Unit(b, Unit::BitRate)),
    ));
    e.push(("max_bit_rate", Value::Na));
    e.push((
        "bits_per_raw_sample",
        info.bits_per_raw_sample
            .or_else(|| pcm_raw_bits(track))
            .map_or(Value::Na, |b| s(b.to_string())),
    ));
    // The samples of the movie box's tables: `ffprobe` counts none of a
    // movie fragment's.
    e.push((
        "nb_frames",
        if track.sample_count > 0 {
            s(track.sample_count.to_string())
        } else {
            Value::Na
        },
    ));
    e.push(("nb_read_frames", Value::Na));
    e.push(("nb_read_packets", Value::Na));
    if let Some(size) = extradata_size(track) {
        e.push(("extradata_size", int(size)));
    }
    let mut tags = Vec::new();
    if let Some(lang) = cnames::language(track.language) {
        tags.push(("language".to_owned(), lang));
    }
    if let Some(handler) = &track.handler_name {
        tags.push(("handler_name".to_owned(), handler.clone()));
    }
    let mut side_data = Vec::new();
    // The Dolby Vision configuration record first, as `ffprobe` lists it.
    if let Some(dv) = d.dolby_vision.filter(|_| video) {
        side_data.push(SideData {
            typ: "dovi_configuration_record",
            entries: vec![
                ("side_data_type", s("DOVI configuration record")),
                ("dv_version_major", Value::Int(i64::from(dv.version_major))),
                ("dv_version_minor", Value::Int(i64::from(dv.version_minor))),
                ("dv_profile", Value::Int(i64::from(dv.profile))),
                ("dv_level", Value::Int(i64::from(dv.level))),
                ("rpu_present_flag", Value::Int(i64::from(dv.rpu_present))),
                ("el_present_flag", Value::Int(i64::from(dv.el_present))),
                ("bl_present_flag", Value::Int(i64::from(dv.bl_present))),
                (
                    "dv_bl_signal_compatibility_id",
                    Value::Int(i64::from(dv.bl_signal_compatibility_id)),
                ),
            ],
        });
    }
    if let Some(clockwise) = track.rotation.filter(|r| *r != 0 && video) {
        let ccw = (360 - i32::from(clockwise % 360)) % 360;
        let signed = if ccw >= 180 { ccw - 360 } else { ccw };
        side_data.push(SideData {
            typ: "display_matrix",
            entries: vec![
                ("side_data_type", s("Display Matrix")),
                ("displaymatrix", s(display_matrix(&track.matrix))),
                ("rotation", Value::Int(i64::from(signed))),
            ],
        });
    }
    Stream {
        kind: if chapter_data(movie, track) || as_data(track) {
            StreamKind::Data
        } else {
            kind(track)
        },
        attached_pic: false,
        entries: e,
        disposition: disposition(true),
        tags,
        side_data,
    }
}

/// A track's own duration in its ticks, as `ffprobe` reads it: without
/// an edit list the decode durations of every sample, no longer than the
/// media header says unless fragments follow; with one edit, no longer than the edit presents
/// ([`Track::duration`], which the stream oracle checks); otherwise the
/// probe's.
fn media_duration(movie: &Movie, track: &Track) -> Option<i64> {
    let edited = track.duration.and_then(crate::time::rounded);
    let header = i64::try_from(track.mdhd_duration).ok().filter(|d| *d > 0);
    let samples = i64::try_from(track.sample_duration).ok().filter(|d| *d > 0);
    match (track.edit_list, samples) {
        (coreav_isobmff::EditList::None, Some(samples)) if !movie.has_fragments() => {
            Some(header.map_or(samples, |h| h.min(samples)))
        }
        (coreav_isobmff::EditList::None, Some(samples)) => Some(samples),
        (coreav_isobmff::EditList::Simple { segment, .. }, Some(samples)) if segment > 0 => {
            Some(edited.map_or(samples, |e| e.min(samples)))
        }
        (_, None) => edited.filter(|d| *d > 0).or(header),
        // Several edits last as long as all of them, empty ones in
        // between included (`edit_multi_empty.mp4`: 4 s of media, 1 s
        // empty, 5 s of media last 10 s), rounded to the nearest tick.
        (coreav_isobmff::EditList::Multiple, _) if movie.timescale > 0 => {
            let total = track.edits.iter().try_fold(0u128, |sum, e| {
                sum.checked_add(u128::from(e.segment_duration))
            });
            let den = u128::from(movie.timescale);
            total
                .and_then(|t| t.checked_mul(u128::from(track.timescale)))
                .and_then(|t| i64::try_from((t + den / 2) / den).ok())
                .or(edited)
        }
        _ => edited,
    }
}

/// A stream's duration in its ticks as `ffprobe` reports it: a track's own
/// ([`media_duration`]), except that a data or subtitle track without an
/// edit list lasts at least as long as the longest track of the file, in its
/// own ticks rounded to the nearest (found on GoPro metadata tracks, text
/// tracks with an empty media header and tracks whose samples last zero
/// ticks; timecode tracks and tracks with an edit list keep their own).
fn stream_duration(movie: &Movie, track: &Track) -> Option<i64> {
    let own = media_duration(movie, track).filter(|d| *d > 0);
    if !matches!(track.kind, TrackKind::Data | TrackKind::Subtitle)
        || track.edit_list != coreav_isobmff::EditList::None
    {
        return own;
    }
    let longest = movie
        .tracks
        .iter()
        .filter_map(|t| {
            Some((
                i128::from(media_duration(movie, t)?),
                i128::from(t.timescale.max(1)),
            ))
        })
        .max_by(|(a, x), (b, y)| (a * y).cmp(&(b * x)));
    let filled = longest.and_then(|(ticks, scale)| {
        let n = ticks.checked_mul(i128::from(track.timescale))?;
        i64::try_from((2 * n + scale).div_euclid(2 * scale)).ok()
    });
    match (own, filled.filter(|d| *d > 0)) {
        (Some(o), Some(f)) => Some(o.max(f)),
        (o, f) => o.or(f),
    }
}

/// How many decode times of a track's first packets `ffprobe`'s frame
/// rate estimate fits, found on the development set (21 agreed best of 21,
/// 41, 64, 128 and 256).
const RATE_WINDOW: usize = 21;

/// `r_frame_rate`, `ffprobe`'s guess at the base frame rate: the estimate
/// of the first decode times that the Matroska builder uses too
/// (`mkv::estimate_rate`: the standard rate whose grid the times fit, else
/// the rate every interval is a whole number of frames of). The rate a
/// stream's own timing states plays no part: taking it where there is one
/// agreed with `ffprobe` on 206 of the 224 development files, the estimate
/// alone on 212.
fn real_rate(times: &[i64], timescale: u32) -> Option<String> {
    let window = times.get(..RATE_WINDOW.min(times.len())).unwrap_or(times);
    super::mkv::estimate_rate(window, (1, i64::from(timescale.max(1))))
        .map(super::mkv::estimate_fraction)
        .and_then(|(n, d)| rate(n, d))
}

/// `N/D` in lowest terms, as `ffprobe` prints a rate; `0/0` for nothing.
pub fn rate(n: u64, d: u64) -> Option<String> {
    if n == 0 || d == 0 {
        return None;
    }
    let g = compat::gcd(n, d);
    Some(format!("{}/{}", n / g, d / g))
}

/// The disposition flags, in `ffprobe`'s order.
pub fn disposition(default: bool) -> Entries {
    [
        "default",
        "dub",
        "original",
        "comment",
        "lyrics",
        "karaoke",
        "forced",
        "hearing_impaired",
        "visual_impaired",
        "clean_effects",
        "attached_pic",
        "timed_thumbnails",
        "non_diegetic",
        "captions",
        "descriptions",
        "metadata",
        "dependent",
        "still_image",
    ]
    .into_iter()
    .map(|k| (k, Value::Int(i64::from(k == "default" && default))))
    .collect()
}

/// A display matrix as `ffprobe` prints one: three rows of three
/// right-aligned numbers, each row headed by its offset.
fn display_matrix(m: &[i32; 9]) -> String {
    let mut out = String::from("\n");
    for row in 0..3 {
        out.push_str(&format!("{row:08x}: "));
        for col in 0..3 {
            let v = m.get(row * 3 + col).copied().unwrap_or(0);
            out.push_str(&format!("{v:12}"));
        }
        out.push('\n');
    }
    out
}

/// The size of the codec configuration a decoder gets.
fn extradata_size(track: &Track) -> Option<usize> {
    let config = track.description.codec_config.as_ref()?;
    match &config.kind.0 {
        b"avcC" | b"hvcC" | b"av1C" => Some(config.data.len()),
        _ => None,
    }
}

/// The format's start and duration in microseconds, as `ffprobe` counts
/// them from the streams: each stream's start and duration rounded to
/// microseconds on their own, then the earliest start and the latest end,
/// of the audio and video streams when there are any, else of every
/// stream.
fn span(streams: &[(StreamKind, i64, Option<i64>, Option<i64>)]) -> (Option<i64>, Option<i64>) {
    let av = streams
        .iter()
        .any(|(k, ..)| matches!(k, StreamKind::Video | StreamKind::Audio));
    let us = |ticks: i64, timescale: i64| -> Option<i64> {
        let v = i128::from(ticks).checked_mul(1_000_000)?;
        let den = i128::from(timescale);
        let half = den / 2;
        let q = if v >= 0 {
            (v + half) / den
        } else {
            (v - half) / den
        };
        i64::try_from(q).ok()
    };
    let mut first: Option<i64> = None;
    let mut last: Option<i64> = None;
    for (kind, timescale, start, duration) in streams {
        let other = av && !matches!(kind, StreamKind::Video | StreamKind::Audio);
        let Some(start) = start.and_then(|s| us(s, *timescale)) else {
            continue;
        };
        if !other {
            first = Some(first.map_or(start, |f| f.min(start)));
        }
        // A subtitle stream's end counts (`text_shifted.mp4`), a data
        // stream's does not (GoPro's `gpmd` a millisecond past the video).
        let ends = !other || *kind == StreamKind::Subtitle;
        if let Some(end) = duration
            .and_then(|d| us(d, *timescale))
            .and_then(|d| start.checked_add(d))
            .filter(|_| ends)
        {
            last = Some(last.map_or(end, |l| l.max(end)));
        }
    }
    let duration = first
        .zip(last)
        .and_then(|(f, l)| l.checked_sub(f))
        .filter(|d| *d >= 0);
    (first, duration)
}

fn format(
    movie: &Movie,
    probed: &crate::probe::Probed,
    name: &str,
    streams: &[(StreamKind, i64, Option<i64>, Option<i64>)],
) -> Entries {
    let (start, micros) = span(streams);
    vec![
        ("filename", s(name)),
        (
            "nb_streams",
            int(movie.tracks.len().saturating_add(movie.dropped.len())),
        ),
        ("nb_programs", Value::Int(0)),
        ("nb_stream_groups", Value::Int(0)),
        ("format_name", s(FORMAT_NAME)),
        ("format_long_name", s(FORMAT_LONG_NAME)),
        (
            "start_time",
            start.map_or(Value::Na, |us| time(us, 1, 1_000_000)),
        ),
        (
            "duration",
            micros.map_or(Value::Na, |us| time(us, 1, 1_000_000)),
        ),
        ("size", Value::Unit(int_u64(probed.size), Unit::Byte)),
        (
            "bit_rate",
            micros
                .and_then(|us| compat::format_bit_rate(probed.size, us))
                .map_or(Value::Na, |b| Value::Unit(int_u64(b), Unit::BitRate)),
        ),
        ("probe_score", Value::Int(100)),
    ]
}

fn int_u64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// The packets of every track, merged by file position.
pub fn packets<'a, S: Source + 'a>(
    probed: IsoProbedPackets,
    mut source: S,
) -> Box<dyn PacketSource + 'a> {
    let limits = Limits::default();
    let plans = probed
        .movie
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            // An audio frame: the decode step most of the first samples
            // take (a first sample can hold the encoder's priming too), as
            // whole audio samples, in the track's ticks rounded down.
            let frame = if t.kind == TrackKind::Audio {
                let mut steps = head(&probed.movie, t, &mut source, &limits).steps;
                steps.sort_unstable();
                steps
                    .get(steps.len() / 2)
                    .copied()
                    .map(|step| audio_frame(t, step))
            } else {
                None
            };
            plan(
                &probed.movie,
                t,
                probed.ends.get(i).copied().unwrap_or_default(),
                probed.steps.get(i).copied().flatten(),
                frame,
            )
        })
        .collect();
    let gapless = probed
        .movie
        .itunes_gapless(&mut source)
        .ok()
        .flatten()
        .map(|g| i64::from(g.encoder_delay))
        .filter(|d| *d > 0);
    let edits = probed
        .movie
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let shift = probed.shifts.get(i).copied().unwrap_or(0);
            let step = probed.steps.get(i).copied().flatten();
            if grouping(&probed.movie, t, step).is_some() {
                return None;
            }
            edit_plan(&probed.movie, t, shift, gapless, &mut source, &limits)
        })
        .collect();
    let mut work = Work {
        held: MAX_SPLICED_SAMPLES,
        steps: limits.max_sample_work,
    };
    let spliced = probed
        .movie
        .tracks
        .iter()
        .map(|t| spliced(&probed.movie, t, &mut source, &limits, &mut work))
        .collect();
    Box::new(IsoPackets {
        source: RefCell::new(source),
        movie: probed.movie,
        priming: probed.priming,
        plans,
        edits,
        spliced,
        shifts: probed.shifts,
        steps: probed.steps,
        covers: probed.covers,
    })
}

/// A track whose edit list has several entries, listed as `ffprobe` lists
/// it (`docs/design/edit-lists.md`, "What `ffprobe` lists"): each media
/// edit in turn, from its preroll sample to the sync sample after its last
/// presented one, so that a sample can be listed several times.
struct Spliced {
    /// The track's samples, in decode order.
    samples: Vec<Held>,
    /// The media edits' listings, in turn.
    segments: Vec<Segment>,
}

/// One media edit's listing.
#[derive(Clone, Copy, Debug)]
struct Segment {
    /// The first sample listed (decode order).
    first: usize,
    /// The last one.
    last: usize,
    /// What the composition and decode times of its samples move by.
    offset: i64,
    /// The media times it presents, `[start, end)`.
    window: (i64, i64),
    /// Whether a sample is presented when its interval overlaps the window
    /// (the first edit of an audio track) rather than when it starts in it.
    overlap: bool,
}

impl Segment {
    fn presents(&self, cts: i64, duration: u32) -> bool {
        let (start, end) = self.window;
        if self.overlap {
            cts.saturating_add(i64::from(duration)) > start && cts < end
        } else {
            cts >= start && cts < end
        }
    }
}

/// A sample held for a [`Spliced`] listing.
#[derive(Clone, Copy, Debug)]
struct Held {
    sample: u64,
    offset: u64,
    size: u32,
    dts: i64,
    cts: i64,
    duration: u32,
    keyframe: bool,
}

impl Held {
    fn of(p: &coreav_isobmff::Packet) -> Self {
        Self {
            sample: p.sample,
            offset: p.offset,
            size: p.size,
            dts: p.dts,
            cts: p.cts,
            duration: p.duration,
            keyframe: p.keyframe,
        }
    }

    /// The packet, at its edit's times.
    fn packet(&self, segment: &Segment) -> coreav_isobmff::Packet {
        let by = i128::from(segment.offset);
        coreav_isobmff::Packet {
            sample: self.sample,
            offset: self.offset,
            size: self.size,
            dts: self.dts,
            cts: self.cts,
            duration: self.duration,
            keyframe: self.keyframe,
            pts: Some(coreav_isobmff::Rational::from_int(
                i128::from(self.cts) + by,
            )),
            presentation_dts: Some(coreav_isobmff::Rational::from_int(
                i128::from(self.dts) + by,
            )),
            presented: segment.presents(self.cts, self.duration),
        }
    }
}

/// How many samples the tracks with several edits may hold for their
/// listings, all together (48 bytes each).
const MAX_SPLICED_SAMPLES: usize = 1_000_000;

/// What building and listing the tracks with several edits may spend, all
/// together: one step per sample searched or listed. Past it, a track keeps
/// the library's listing.
#[derive(Debug)]
struct Work {
    /// Samples the listings may still hold.
    held: usize,
    /// Steps left.
    steps: u64,
}

impl Work {
    fn spend(&mut self, steps: u64) -> Result<(), Exhausted> {
        self.steps = self.steps.checked_sub(steps).ok_or(Exhausted)?;
        Ok(())
    }
}

/// [`Work`] ran out.
#[derive(Debug)]
struct Exhausted;

/// Where composition times can lie from decode times in a held track, for
/// searching it by decode time, which never decreases.
#[derive(Clone, Copy, Debug)]
struct Reach {
    /// The smallest and largest composition offsets.
    min: i64,
    max: i64,
    /// The longest sample.
    duration: i64,
}

impl Reach {
    fn of(samples: &[Held]) -> Self {
        let mut reach = Self {
            min: 0,
            max: 0,
            duration: 0,
        };
        for p in samples {
            let offset = p.cts.saturating_sub(p.dts);
            reach.min = reach.min.min(offset);
            reach.max = reach.max.max(offset);
            reach.duration = reach.duration.max(i64::from(p.duration));
        }
        reach
    }

    /// The first sample composed at or after `m`.
    fn first_from(
        &self,
        samples: &[Held],
        m: i64,
        work: &mut Work,
    ) -> Result<Option<usize>, Exhausted> {
        let lo = samples.partition_point(|p| p.dts.saturating_add(self.max) < m);
        for (i, p) in samples.iter().enumerate().skip(lo) {
            work.spend(1)?;
            if p.cts >= m {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    /// The smallest composition time at or after `m`, from sample `f` on.
    fn snap(
        &self,
        samples: &[Held],
        f: usize,
        m: i64,
        work: &mut Work,
    ) -> Result<Option<i64>, Exhausted> {
        let mut best: Option<i64> = None;
        for p in samples.iter().skip(f) {
            if best.is_some_and(|b| p.dts.saturating_add(self.min) >= b) {
                break;
            }
            work.spend(1)?;
            if p.cts >= m {
                best = Some(best.map_or(p.cts, |b| b.min(p.cts)));
            }
        }
        Ok(best)
    }

    /// The first sample lasting past `from`.
    fn first_past(
        &self,
        samples: &[Held],
        from: i64,
        work: &mut Work,
    ) -> Result<Option<usize>, Exhausted> {
        let lo = samples.partition_point(|p| {
            p.dts.saturating_add(self.max).saturating_add(self.duration) <= from
        });
        for (i, p) in samples.iter().enumerate().skip(lo) {
            work.spend(1)?;
            if p.cts.saturating_add(i64::from(p.duration)) > from {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    /// The last sample `segment` presents.
    fn last_presented(
        &self,
        samples: &[Held],
        segment: &Segment,
        work: &mut Work,
    ) -> Result<Option<usize>, Exhausted> {
        let (start, end) = segment.window;
        let hi = samples.partition_point(|p| p.dts.saturating_add(self.min) < end);
        let reach = self
            .max
            .saturating_add(if segment.overlap { self.duration } else { 0 });
        for (i, p) in samples.iter().enumerate().take(hi).rev() {
            if p.dts.saturating_add(reach) < start {
                break;
            }
            work.spend(1)?;
            if segment.presents(p.cts, p.duration) {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }
}

/// A track's [`Spliced`] listing; `None` for a track without several
/// edits, in a movie with fragments, or with an entry at a rate other than
/// 1 (`ffprobe`'s dwells are not modelled).
fn spliced<S: Source>(
    movie: &Movie,
    track: &Track,
    source: &mut S,
    limits: &Limits,
    work: &mut Work,
) -> Option<Spliced> {
    let mut steps = Work {
        held: work.held,
        steps: work.steps,
    };
    let listing = spliced_within(movie, track, source, limits, &mut steps).ok()??;
    // Only a listing built in full spends what it held and searched; one
    // that ran out leaves the rest to the other tracks.
    work.held = work.held.saturating_sub(listing.samples.len());
    work.steps = steps.steps;
    Some(listing)
}

fn spliced_within<S: Source>(
    movie: &Movie,
    track: &Track,
    source: &mut S,
    limits: &Limits,
    work: &mut Work,
) -> Result<Option<Spliced>, Exhausted> {
    if !matches!(
        track.edit_list,
        coreav_isobmff::EditList::Multiple | coreav_isobmff::EditList::Unsupported
    ) || movie.has_fragments()
        || movie.timescale == 0
        || track
            .edits
            .iter()
            .any(|e| e.media_time < -1 || (e.media_time >= 0 && e.media_rate != 0x0001_0000))
    {
        return Ok(None);
    }
    let mut budget = Budget::new(limits);
    let Ok(iter) = movie.packets(track.index, &mut *source, limits, &mut budget) else {
        return Ok(None);
    };
    let mut samples = Vec::new();
    for p in iter {
        if samples.len() >= work.held {
            return Err(Exhausted);
        }
        work.spend(1)?;
        let Ok(p) = p else {
            return Ok(None);
        };
        samples.push(Held::of(&signed_offsets(track, p)));
    }
    if samples.is_empty() {
        return Ok(None);
    }
    let reach = Reach::of(&samples);
    let audio = track.kind == TrackKind::Audio;
    let ctts = track.ctts_version().is_some();
    let timescale = i64::from(track.timescale.max(1));
    let den = u128::from(movie.timescale);
    // Each entry's duration in media ticks, rounded to the nearest, halves
    // up; its start, the sum of those before it.
    let mut start: i64 = 0;
    let mut media_seen = false;
    let mut segments = Vec::new();
    // `C`: how far the first edit's anchor sample decodes before its start.
    let mut anchor: Option<i64> = None;
    for e in &track.edits {
        let ticks = (u128::from(e.segment_duration) * u128::from(track.timescale) + den / 2) / den;
        let Ok(duration) = i64::try_from(ticks) else {
            return Ok(None);
        };
        let entry_start = start;
        let Some(next) = start.checked_add(duration) else {
            return Ok(None);
        };
        start = next;
        // Empty edits before the first media edit only move the start; one
        // after a media edit presents the media from time -1.
        if e.media_time == -1 && !media_seen {
            continue;
        }
        let first_edit = !media_seen;
        media_seen = true;
        let m = e.media_time;
        let Some(end) = m.checked_add(duration) else {
            return Ok(None);
        };
        // `F`: the first sample in decode order composed at or after the
        // media time.
        let Some(f) = reach.first_from(&samples, m, work)? else {
            continue;
        };
        let Some(f_sample) = samples.get(f).copied() else {
            return Ok(None);
        };
        let offset = if first_edit {
            let snap = if audio {
                m
            } else {
                reach.snap(&samples, f, m, work)?.unwrap_or(f_sample.cts)
            };
            anchor = f_sample.dts.checked_sub(snap);
            entry_start.checked_sub(snap)
        } else {
            entry_start
                .checked_add(anchor.unwrap_or(0))
                .and_then(|t| t.checked_sub(f_sample.dts))
        };
        let Some(offset) = offset else {
            return Ok(None);
        };
        // The preroll sample.
        let first = if audio {
            reach
                .first_past(&samples, m.saturating_sub(timescale), work)?
                .unwrap_or(0)
                .min(f)
        } else {
            let mut at = 0;
            for (i, p) in samples.iter().enumerate().take(f.saturating_add(1)).rev() {
                work.spend(1)?;
                if p.keyframe {
                    at = i;
                    break;
                }
            }
            at
        };
        let mut segment = Segment {
            first,
            last: samples.len().saturating_sub(1),
            offset,
            window: (m, end),
            overlap: audio && first_edit,
        };
        // To the first sync sample at or after the last presented one, the
        // second with composition offsets.
        let presented_last = reach.last_presented(&samples, &segment, work)?.unwrap_or(f);
        let mut syncs = 0;
        for (i, p) in samples.iter().enumerate().skip(presented_last) {
            work.spend(1)?;
            if p.keyframe {
                syncs += 1;
                if syncs >= if ctts { 2 } else { 1 } {
                    segment.last = i;
                    break;
                }
            }
        }
        // Each sample the edit lists is one more step.
        let listed = segment.last.saturating_sub(segment.first).saturating_add(1);
        work.spend(u64::try_from(listed).unwrap_or(u64::MAX))?;
        segments.push(segment);
    }
    Ok(Some(Spliced { samples, segments }))
}

/// Which samples of a track with a simple edit list `ffprobe` lists, and
/// how it times them (module documentation, "Edit lists").
#[derive(Clone, Copy, Debug, Default)]
struct EditPlan {
    /// The first sample listed, in decode order.
    start: u64,
    /// The last sample listed, when the listing ends before the track.
    stop: Option<u64>,
    /// How much earlier every time is than the exact mapping of the edit's
    /// media time puts it, in the track's ticks.
    shift: i64,
    /// The first packet's skip side data.
    skip: Option<i64>,
    /// Nothing is listed: no sample lasts past the media time.
    nothing: bool,
    /// For video, the media window `[start, end)` whose composition times
    /// are presented (`ffprobe` marks the rest discarded); `None` for
    /// audio, presented where its interval overlaps the window.
    window: Option<(i64, Option<i64>)>,
}

/// A track's [`EditPlan`], `None` without a simple edit list. Reads the
/// track's samples from the start until the first listed one is known,
/// and to the end when the edit ends before the media.
fn edit_plan<S: Source>(
    movie: &Movie,
    track: &Track,
    dts_shift: i64,
    gapless: Option<i64>,
    source: &mut S,
    limits: &Limits,
) -> Option<EditPlan> {
    let coreav_isobmff::EditList::Simple {
        media_time,
        segment,
        ..
    } = track.edit_list
    else {
        return None;
    };
    let m = media_time.max(0);
    let audio = track.kind == TrackKind::Audio;
    let timescale = i64::from(track.timescale.max(1));
    // Where the edit's window ends, in media ticks; `window_end` when that
    // is before the media ends.
    let edit_end = (segment > 0 && movie.timescale > 0)
        .then(|| {
            let den = u128::from(movie.timescale);
            let ticks = (u128::from(segment) * u128::from(track.timescale) + den / 2) / den;
            i64::try_from(ticks).ok().and_then(|t| m.checked_add(t))
        })
        .flatten();
    // (A text or data track's last sample can start at the end exactly.)
    let short = !matches!(track.kind, TrackKind::Video | TrackKind::Audio);
    let window_end = edit_end.filter(|end| {
        u64::try_from(*end)
            .is_ok_and(|e| e < track.sample_duration || (short && e == track.sample_duration))
    });
    let mut budget = Budget::new(limits);
    let iter = movie
        .packets(track.index, &mut *source, limits, &mut budget)
        .ok()?;
    let ctts = track.ctts_version().is_some();
    let mut plan = EditPlan::default();
    let mut last_sync: u64 = 0;
    // Video: the first sample that lasts past the media time, and the
    // smallest composition time at or after it (`snap`).
    let mut first: Option<u64> = None;
    let mut snap: Option<i64> = None;
    let mut start_cts: Option<i64> = None;
    // The last presented sample, and the sync samples since.
    let mut last_presented: Option<u64> = None;
    let mut syncs_after: u32 = 0;
    for p in iter {
        let p = signed_offsets(track, p.ok()?);
        let n = p.sample;
        let end = p.cts.saturating_add(i64::from(p.duration));
        if audio {
            // One second before the media time, or the first sample.
            if start_cts.is_none() && (end > m.saturating_sub(timescale) || p.cts >= m) {
                plan.start = n;
                start_cts = Some(p.cts);
            }
        } else {
            if p.keyframe && first.is_none() {
                last_sync = n;
            }
            // From the sync sample before the first one that lasts past the
            // media time.
            if first.is_none() && end > m {
                first = Some(n);
                plan.start = last_sync;
                start_cts = Some(p.cts);
            }
            if p.cts >= m {
                snap = Some(snap.map_or(p.cts, |s| s.min(p.cts)));
            }
        }
        let started = if audio {
            start_cts.is_some()
        } else {
            snap.is_some()
        };
        let Some(window_end) = window_end else {
            // No end to find: done once no later sample can start earlier
            // than the snap.
            if started && (audio || snap.is_some_and(|s| p.dts.saturating_sub(dts_shift) > s)) {
                break;
            }
            continue;
        };
        let presented = if audio {
            end > m && p.cts < window_end
        } else {
            p.cts >= m && p.cts < window_end
        };
        // The listing goes on to the first sync sample at or after the last
        // presented one, the second with composition offsets.
        if presented {
            last_presented = Some(n);
            syncs_after = 0;
            plan.stop = None;
        }
        if last_presented.is_some() && p.keyframe && plan.stop.is_none() {
            syncs_after = syncs_after.saturating_add(1);
            if syncs_after >= if ctts { 2 } else { 1 } {
                plan.stop = Some(n);
            }
        }
        if p.dts.saturating_sub(dts_shift) >= window_end && plan.stop.is_some() {
            break;
        }
    }
    if audio {
        let c = start_cts?;
        plan.skip = (m > c).then(|| gapless.unwrap_or(m - c));
    } else {
        // Without a composition time at or after the media time, the
        // sample lasting past it starts the presentation.
        match snap.or(start_cts) {
            // Fragments keep their decode times: no snap, no discards.
            Some(_) if track.sample_count == 0 => {}
            Some(snap) => {
                plan.shift = snap - m;
                plan.window = Some((m, edit_end));
            }
            None => plan.nothing = true,
        }
    }
    Some(plan)
}

/// How `ffprobe` times one track's packets (module documentation, "Packet
/// durations").
#[derive(Clone, Copy, Debug)]
struct Plan {
    /// A video or audio track with composition offsets (`ctts`): no packet keeps its
    /// sample's duration.
    ctts: bool,
    /// The duration a packet gets when it keeps no sample duration.
    fill: Fill,
    /// How many packets of the track's fragments come before `fill` is
    /// known: those have no duration.
    lead: u64,
    /// Their duration: none, or one tick in a time base coarser than
    /// 1/1,000 s.
    lead_fill: Option<i64>,
    /// The stream's duration in its ticks, as `ffprobe` counts it: the last
    /// packet of the sample tables lasts from its presentation time until
    /// then.
    end: Option<i64>,
    /// Whether the first sample description is encrypted (`encv`, `enca`):
    /// the packets of its samples carry encryption side data.
    encrypted: bool,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            ctts: false,
            fill: Fill::Stored,
            lead: 0,
            lead_fill: None,
            end: None,
            encrypted: false,
        }
    }
}

/// The duration of a packet `ffprobe` gives no sample duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fill {
    /// One frame, in the track's ticks; `None` when the rate is unusable.
    Frame(Option<i64>),
    /// The sample's own (audio: one frame of the codec, which the samples
    /// hold).
    Stored,
    /// None.
    Na,
}

/// Where a video track's frame rate comes from.
enum Rate {
    /// H.264 parameter sets state it: known from the first packet.
    Stated(Option<i64>),
    /// Guessed from the timestamps (`r_frame_rate`); until then, in
    /// fragments, the rate H.265 parameter sets state, if any.
    Guessed {
        /// The guessed rate's frame.
        rate: Option<i64>,
        /// The stated rate's frame.
        stated: Option<i64>,
    },
}

/// The frame duration `ffprobe` gives a video packet whose duration it does
/// not take from the sample tables, in the track's ticks, rounded down:
/// from the frame rate H.264 parameter sets state (ITU-T H.264 E.2.1)
/// when they state one, none when that rate is 1,000 frames a second or
/// more; else from the guessed base rate (`r_frame_rate`), which an H.265
/// rate (H.265 E.3.1) only stands in for until it is guessed.
fn frame_rate(track: &Track, guessed: Option<(u64, u64)>) -> Rate {
    // From the configuration record, whatever the sample entry (an
    // encrypted `encv` one holds the original's).
    let config = track.description.codec_config.as_ref();
    let record = |kind: &[u8; 4]| config.filter(|c| c.kind.0 == *kind);
    let stated = record(b"avcC")
        .and_then(|c| video::avc(&c.data, false))
        .and_then(|i| i.frame_rate);
    let hevc = record(b"hvcC")
        .and_then(|c| video::hevc(&c.data, false))
        .and_then(|i| i.frame_rate);
    let ticks = |num: u64, den: u64| -> Option<i64> {
        let t = u128::from(track.timescale).checked_mul(u128::from(den))? / u128::from(num.max(1));
        i64::try_from(t).ok().filter(|t| *t > 0)
    };
    if let Some((num, den)) = stated {
        // At 1,000 frames a second or more, the rate is not used.
        if u128::from(num) >= u128::from(den).saturating_mul(1000) {
            return Rate::Stated(None);
        }
        return Rate::Stated(ticks(num, den));
    }
    Rate::Guessed {
        rate: guessed.and_then(|(num, den)| ticks(num, den)),
        stated: hevc.and_then(|(num, den)| ticks(num, den)),
    }
}

/// A track's duration as `ffprobe` reports it (KD-EDIT-DURATION): the
/// smallest of the media header's duration, the sum of the sample
/// durations, and the edits' durations added up in the track's ticks.
fn listed_duration(movie: &Movie, track: &Track) -> Option<i64> {
    let mut best: Option<u64> = None;
    let mut take = |v: u64| {
        if v > 0 {
            best = Some(best.map_or(v, |b| b.min(v)));
        }
    };
    take(track.mdhd_duration);
    take(track.sample_duration);
    if !track.edits.is_empty() && movie.timescale > 0 {
        let den = u128::from(movie.timescale);
        let sum: u128 = track
            .edits
            .iter()
            .map(|e| (u128::from(e.segment_duration) * u128::from(track.timescale) + den / 2) / den)
            .sum();
        take(u64::try_from(sum).unwrap_or(u64::MAX));
    }
    let own = best.and_then(|b| i64::try_from(b).ok());
    // A data or text stream (not a timecode) without an edit list lasts at
    // least as long as the longest audio or video stream, else the movie.
    if matches!(track.kind, TrackKind::Data | TrackKind::Subtitle) && track.edits.is_empty() {
        let longest = movie
            .tracks
            .iter()
            .filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Video))
            .filter_map(|t| {
                let d = u128::try_from(listed_duration(movie, t)?).ok()?;
                Some((d, u128::from(t.timescale.max(1))))
            })
            .max_by(|(a, ta), (b, tb)| (a * tb).cmp(&(b * ta)))
            .or_else(|| {
                (movie.timescale > 0)
                    .then(|| (u128::from(movie.mvhd_duration), u128::from(movie.timescale)))
            });
        let whole = longest.and_then(|(d, den)| {
            let ticks = d.checked_mul(u128::from(track.timescale))?;
            i64::try_from((ticks + den / 2) / den)
                .ok()
                .filter(|d| *d > 0)
        });
        return own.max(whole);
    }
    own
}

fn plan(
    movie: &Movie,
    track: &Track,
    listed: Listed,
    step: Option<i64>,
    audio_frame: Option<i64>,
) -> Plan {
    let protected = matches!(&track.description.fourcc.0, b"encv" | b"enca");
    // With one sample description, every sample uses it.
    let encrypted = protected && track.descriptions == 1;
    let coarse = track.timescale < 1000;
    let (fill, lead, stated) = match track.kind {
        TrackKind::Video => match frame_rate(track, listed.rate) {
            // In a time base coarser than 1/1,000 s, one tick.
            Rate::Stated(d) => (Fill::Frame(if coarse { Some(1) } else { d }), 0, None),
            // The rate is guessed from the first 20 frames' timestamps (40
            // in a time base coarser than 1/2,000 s), read before the first
            // is listed.
            Rate::Guessed { rate: d, stated } => {
                // Samples that all last one step: the rate is one frame a
                // step, whatever the guess.
                let samples = u64::from(track.sample_count).saturating_add(track.fragment_samples);
                let constant = step
                    .and_then(|s| u64::try_from(s).ok())
                    .filter(|s| *s > 0 && s.checked_mul(samples) == Some(track.sample_duration));
                let d = constant.and_then(|s| i64::try_from(s).ok()).or(d);
                let lead = if track.timescale < 2000 { 41 } else { 21 };
                (Fill::Frame(d), lead, stated)
            }
        },
        // Never known for an encrypted track, or a codec `ffprobe` cannot
        // decode.
        TrackKind::Audio
            if encrypted
                || codec_name(movie, track).is_none()
                || matches!(&track.description.fourcc.0, b"ac-4" | b"mha1" | b"mhm1") =>
        {
            (Fill::Na, 0, None)
        }
        // AAC's frame size is known once the first packet is decoded.
        TrackKind::Audio => {
            let aac = track.description.codec == Codec::Aac
                || matches!(&track.description.fourcc.0, b"mp4a" | b"enca");
            (
                Fill::Frame(audio_frame.filter(|s| *s > 0)),
                u64::from(aac),
                None,
            )
        }
        // WebVTT in ISO files is a data stream to `ffprobe`.
        TrackKind::Subtitle
            if codec_name(movie, track).is_some() && track.description.fourcc.0 != *b"wvtt" =>
        {
            (Fill::Stored, 0, None)
        }
        TrackKind::Subtitle => (Fill::Na, 0, None),
        TrackKind::Data | TrackKind::Timecode => (Fill::Na, 0, None),
    };
    Plan {
        ctts: matches!(track.kind, TrackKind::Video | TrackKind::Audio)
            && track.ctts_version().is_some(),
        fill,
        lead,
        lead_fill: if coarse && track.kind == TrackKind::Video {
            Some(1)
        } else {
            stated
        },
        end: listed_duration(movie, track).or(listed.duration),
        encrypted: protected,
    }
}

/// A decode step of `step` ticks as `ffprobe` times an audio frame: the
/// whole audio samples it holds at the configured sample rate, back in the
/// track's ticks, rounded down.
fn audio_frame(track: &Track, step: i64) -> i64 {
    let rate = track
        .description
        .configured_sample_rate
        .or(track.description.sample_rate)
        .map(i128::from)
        .filter(|r| *r > 0);
    let ts = i128::from(track.timescale);
    match rate {
        Some(rate) if ts > 0 && rate != ts => {
            let samples = (i128::from(step) * rate + ts / 2) / ts;
            i64::try_from(samples * ts / rate).unwrap_or(step)
        }
        _ => step,
    }
}

/// The last packet of a track's sample tables lasts until the stream's end;
/// one that starts at or after it gets no sample duration (raw audio
/// grouped into packets keeps its group's).
fn trim_last(queue: &mut Queue, plan: Plan, tables: u64, raw: bool) {
    let Some((p, d, _)) = queue.back_mut() else {
        return;
    };
    if plan.ctts || p.sample >= tables || d.is_some() {
        return;
    }
    let pts = p.pts.and_then(|r| i64::try_from(r.round()).ok());
    let left = plan
        .end
        .zip(pts)
        .map(|(end, t)| end.saturating_sub(t))
        .filter(|left| *left > 0);
    *d = Some(match (left, plan.fill) {
        (Some(left), _) => Some(left),
        (None, Fill::Frame(frame)) if !raw => frame,
        (None, _) => Some(i64::from(p.duration)),
    });
}

/// What [`packets`] lists from.
pub struct IsoProbedPackets {
    /// The movie.
    pub movie: Movie,
    /// See [`IsoProbed::priming`].
    pub priming: Vec<i64>,
    /// See [`IsoProbed::ends`].
    pub ends: Vec<Listed>,
    /// See [`IsoProbed::shifts`].
    pub shifts: Vec<i64>,
    /// See [`IsoProbed::steps`].
    pub steps: Vec<Option<i64>>,
    /// See [`IsoProbed::covers`].
    pub covers: Vec<(usize, u64, u64)>,
}

/// A track's next packets, each with the duration it is listed with when
/// that is not its sample's, and the sample description of a fragment's.
type Queue = VecDeque<(coreav_isobmff::Packet, Option<Option<i64>>, Option<u32>)>;

struct IsoPackets<S> {
    source: RefCell<S>,
    movie: Movie,
    /// Per track, the samples the first packet skips.
    priming: Vec<i64>,
    /// Per track, how `ffprobe` times its packets.
    plans: Vec<Plan>,
    /// Per track with a simple edit list, which samples are listed.
    edits: Vec<Option<EditPlan>>,
    /// Per track with several edits, its listing.
    spliced: Vec<Option<Spliced>>,
    /// Per track, how far decode times are moved back.
    shifts: Vec<i64>,
    /// Per track, the decode step of its first samples.
    steps: Vec<Option<i64>>,
    /// The cover pictures' packets.
    covers: Vec<(usize, u64, u64)>,
}

/// One source shared by the iterators of every track, which are advanced
/// one at a time.
struct Shared<'c, S>(&'c RefCell<S>);

impl<S: Source> Source for Shared<'_, S> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        self.0
            .try_borrow_mut()
            .map_err(|_| coreav_isobmff::Error::Io(std::io::ErrorKind::WouldBlock))?
            .len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        self.0
            .try_borrow_mut()
            .map_err(|_| coreav_isobmff::Error::Io(std::io::ErrorKind::WouldBlock))?
            .read_exact_at(offset, buf)
    }
}

impl<S: Source> PacketSource for IsoPackets<S> {
    fn visit(&mut self, f: &mut dyn FnMut(Packet) -> Result<bool, Failure>) -> Result<(), Failure> {
        // A cover picture's packet comes first, untimed.
        for &(stream, pos, size) in &self.covers {
            let packet = Packet {
                stream,
                size,
                pos: Some(pos),
                keyframe: true,
                ..Packet::default()
            };
            if !f(packet)? {
                return Ok(());
            }
        }
        let limits = Limits::default();
        let movie = &self.movie;
        let tracks = movie.tracks.len();
        let mut budgets: Vec<Budget> = movie.tracks.iter().map(|_| Budget::new(&limits)).collect();
        let mut iters = Vec::with_capacity(tracks);
        for (track, budget) in movie.tracks.iter().zip(budgets.iter_mut()) {
            // From the first sample `ffprobe` lists.
            let start = self
                .edits
                .get(track.index)
                .copied()
                .flatten()
                .map_or(0, |e| e.start);
            let iter = movie
                .packets_from(track.index, start, Shared(&self.source), &limits, budget)
                .map_err(Failure::library)?;
            iters.push(iter);
        }
        // The tracks whose samples are grouped into packets, and how their
        // times move from media to presentation.
        let grouped: Vec<Option<Group>> = movie
            .tracks
            .iter()
            .zip(&self.steps)
            .map(|(t, step)| grouping(movie, t, *step))
            .collect();
        // Each track's next packets, a few ahead, so that the one shown
        // last is known when the track ends, and each packet's duration
        // when it is trimmed to the end of the presentation.
        let mut queues: Vec<Queue> = (0..tracks).map(|_| VecDeque::new()).collect();
        // A chapter track is read for the chapters, not listed; nor is a
        // track whose samples all last 0 ticks.
        let mut done: Vec<bool> = movie
            .tracks
            .iter()
            .map(|t| {
                chapter_data(movie, t)
                    || (t.sample_count > 0 && t.sample_duration == 0 && !movie.has_fragments())
                    || self
                        .edits
                        .get(t.index)
                        .copied()
                        .flatten()
                        .is_some_and(|e| e.nothing)
            })
            .collect();
        let pts_of = |p: &coreav_isobmff::Packet| p.pts.and_then(|r| i64::try_from(r.round()).ok());
        // Per track, where the chunk of the last compressed frames starts.
        let mut chunks: Vec<(u64, u64)> = vec![(0, u64::MAX); tracks];
        // Per track with several edits, the segment and sample listed next.
        let mut cursors: Vec<(usize, usize)> = vec![(0, 0); tracks];
        let refill = |i: usize,
                      queues: &mut Vec<Queue>,
                      done: &mut Vec<bool>,
                      iters: &mut Vec<coreav_isobmff::Packets<'_, Shared<'_, S>>>,
                      chunks: &mut Vec<(u64, u64)>,
                      cursors: &mut Vec<(usize, usize)>|
         -> Result<(), Failure> {
            let (Some(queue), Some(finished), Some(iter), Some(chunk)) = (
                queues.get_mut(i),
                done.get_mut(i),
                iters.get_mut(i),
                chunks.get_mut(i),
            ) else {
                return Ok(());
            };
            while !*finished && queue.len() < LOOKAHEAD {
                // Several edits: each media edit's samples in turn.
                if let Some(spliced) = self.spliced.get(i).and_then(Option::as_ref) {
                    let Some(cursor) = cursors.get_mut(i) else {
                        return Ok(());
                    };
                    let next = loop {
                        let Some(segment) = spliced.segments.get(cursor.0) else {
                            break None;
                        };
                        let at = cursor.1.max(segment.first);
                        if at > segment.last {
                            *cursor = (cursor.0.saturating_add(1), 0);
                            continue;
                        }
                        cursor.1 = at.saturating_add(1);
                        let Some(p) = spliced.samples.get(at).map(|h| h.packet(segment)) else {
                            break None;
                        };
                        // The sync sample an edit's listing ends on, when
                        // not presented, lasts one tick.
                        let ctts = movie
                            .tracks
                            .get(i)
                            .is_some_and(|t| t.ctts_version().is_some());
                        let ends = at == segment.last && !p.presented && !ctts;
                        break Some((p, ends.then_some(Some(1)), None));
                    };
                    match next {
                        Some(p) => queue.push_back(p),
                        None => {
                            *finished = true;
                            let plan = self.plans.get(i).copied().unwrap_or_default();
                            let tables =
                                movie.tracks.get(i).map_or(0, |t| u64::from(t.sample_count));
                            trim_last(queue, plan, tables, false);
                        }
                    }
                    continue;
                }
                let next = match grouped.get(i).copied().flatten() {
                    // Constant-size audio samples of one chunk make one
                    // packet of up to GROUP_TICKS, as `ffprobe` lists them.
                    Some(group) => match iter.next_run(group.samples) {
                        Some(Ok(run)) => {
                            let mut p = pcm_packet(&run, group.pts, group.dts);
                            if let Some(frame) = group.frame {
                                if let Some((offset, size)) = frame_packet(&run, frame, chunk) {
                                    p.offset = offset;
                                    p.size = size;
                                }
                            }
                            Some(p)
                        }
                        Some(Err(e)) => return Err(Failure::library(e)),
                        None => None,
                    },
                    None => iter.next().transpose().map_err(Failure::library)?,
                };
                let description = iter.fragment_description();
                let next = match (next, movie.tracks.get(i)) {
                    (Some(p), Some(track)) => Some(signed_offsets(track, p)),
                    (next, _) => next,
                };
                // A simple edit list's listing: up to its last sample, every
                // time moved by the snap of its media time.
                let edit = self.edits.get(i).copied().flatten();
                let next = next
                    .filter(|p| {
                        edit.and_then(|e| e.stop)
                            .is_none_or(|stop| p.sample <= stop)
                    })
                    .map(|mut p| {
                        if let Some(shift) = edit.map(|e| e.shift).filter(|s| *s != 0) {
                            let by = coreav_isobmff::Rational::from_int(-i128::from(shift));
                            p.pts = p.pts.and_then(|t| t.checked_add(by));
                            p.presentation_dts = p.presentation_dts.and_then(|t| t.checked_add(by));
                        }
                        // Video is presented where its composition time is
                        // in the window.
                        if let Some((start, end)) = edit.and_then(|e| e.window) {
                            p.presented = p.cts >= start && end.is_none_or(|end| p.cts < end);
                        }
                        p
                    });
                match next {
                    Some(p) => queue.push_back((p, None, description)),
                    None => {
                        *finished = true;
                        let plan = self.plans.get(i).copied().unwrap_or_default();
                        let tables = movie.tracks.get(i).map_or(0, |t| u64::from(t.sample_count));
                        let raw = grouped.get(i).copied().flatten().is_some();
                        trim_last(queue, plan, tables, raw);
                    }
                }
            }
            Ok(())
        };
        for i in 0..tracks {
            refill(
                i,
                &mut queues,
                &mut done,
                &mut iters,
                &mut chunks,
                &mut cursors,
            )?;
        }
        // How each track's keyframes are told (tools/oracle/SPEC.md,
        // "Keyframes"): H.264 and AV1 from the coded data, as `ffprobe`
        // flags them; other codecs from the sync sample table.
        let access: Vec<_> = movie
            .tracks
            .iter()
            .map(|t| movie.access_format(t.index))
            .collect();
        let single_reference: Vec<bool> = movie
            .tracks
            .iter()
            .map(|t| movie.max_num_ref_frames(t.index).is_some_and(|r| r <= 1))
            .collect();
        let mut av1_reduced: Vec<Option<bool>> = movie
            .tracks
            .iter()
            .map(|t| {
                t.description
                    .codec_config
                    .as_ref()
                    .filter(|c| c.kind.0 == *b"av1C")
                    .and_then(|c| coreav_isobmff::access::av1_config_reduced(&c.data))
            })
            .collect();
        let mut first = vec![true; tracks];
        let mut listed_count = vec![0u64; tracks];
        // Video packets listed so far, and whether there is a video track:
        // an AAC track's first packet has no duration when no video comes
        // first, or 21 video packets or more do, or when it is a movie box
        // sample of a movie with fragments (measured, not explained).
        let mut video_listed: u64 = 0;
        let has_video = movie.tracks.iter().any(|t| t.kind == TrackKind::Video);
        loop {
            // The packet stored first, unless one of another track is due
            // more than a second earlier, as `ffprobe` interleaves a file
            // whose tracks are stored apart.
            // The decode time as listed (moved back by the track's smallest
            // composition offset), in microseconds rounded to the nearest.
            let micros = |i: usize, p: &coreav_isobmff::Packet| -> i128 {
                let ts = i128::from(movie.tracks.get(i).map_or(1, |t| t.timescale.max(1)));
                let dts = p.presentation_dts.map_or(i128::from(p.dts), |r| r.round());
                let shift = i128::from(self.shifts.get(i).copied().unwrap_or(0));
                let v = dts.saturating_sub(shift).saturating_mul(1_000_000);
                let half = ts / 2;
                if v >= 0 {
                    v.saturating_add(half) / ts
                } else {
                    v.saturating_sub(half) / ts
                }
            };
            let live = || {
                queues
                    .iter()
                    .enumerate()
                    .filter_map(|(i, q)| q.front().map(|(p, _, _)| (i, *p)))
            };
            // Each track's next packet in turn against the best so far: the
            // one stored first when they are due within a second of each
            // other, else the one due first.
            let next = live()
                .reduce(|(i, best), (j, p)| {
                    let (a, b) = (micros(i, &best), micros(j, &p));
                    let later = if a.abs_diff(b) <= 1_000_000 {
                        p.offset < best.offset
                    } else {
                        b < a
                    };
                    if later { (j, p) } else { (i, best) }
                })
                .map(|(i, _)| i);
            let Some(i) = next else {
                return Ok(());
            };
            let Some((p, trimmed, description)) = queues.get_mut(i).and_then(VecDeque::pop_front)
            else {
                return Ok(());
            };
            refill(
                i,
                &mut queues,
                &mut done,
                &mut iters,
                &mut chunks,
                &mut cursors,
            )?;
            let Some(track) = movie.tracks.get(i) else {
                return Ok(());
            };
            // A data track's samples are all key, and an empty sync sample
            // table is no table.
            let mut keyframe = p.keyframe
                || matches!(track.kind, TrackKind::Data | TrackKind::Timecode)
                || (track.sync_entries() == Some(0) && p.sample < u64::from(track.sample_count));
            if let Some(iter) = iters.get_mut(i) {
                if track.description.codec == Codec::Av1 {
                    if let Some(reduced) = av1_reduced.get_mut(i) {
                        if let Ok(Some(start)) = iter.av1_frame_start_at(p.offset, p.size, reduced)
                        {
                            if let Some(key) = start.key_frame {
                                keyframe = key;
                            }
                        }
                    }
                }
                if let Some(Some(format)) = access.get(i) {
                    if let Ok(Some(frame)) = iter.frame_access(*format, &p) {
                        keyframe = frame.idr
                            || frame.recovery_frames.is_some()
                            || (frame.intra && single_reference.get(i).copied().unwrap_or(false));
                    }
                }
            }
            let pts = pts_of(&p);
            let plan = self.plans.get(i).copied().unwrap_or_default();
            let listed = listed_count.get(i).copied().unwrap_or(0);
            if let Some(n) = listed_count.get_mut(i) {
                *n = n.saturating_add(1);
            }
            let stored = Some(i64::from(p.duration));
            // In a movie with fragments, the movie box's samples are timed as
            // the fragments' are.
            let fragment = p.sample >= u64::from(track.sample_count) || movie.has_fragments();
            let fill = match plan.fill {
                Fill::Frame(frame) => frame,
                Fill::Stored => stored,
                Fill::Na => None,
            };
            let audio = track.kind == TrackKind::Audio;
            let leading = listed < plan.lead
                && if audio {
                    !has_video
                        || video_listed >= 21
                        || (movie.has_fragments() && p.sample < u64::from(track.sample_count))
                } else {
                    fragment
                };
            if track.kind == TrackKind::Video {
                video_listed = video_listed.saturating_add(1);
            }
            let duration = if fragment || plan.ctts {
                if leading { plan.lead_fill } else { fill }
            } else {
                match trimmed {
                    Some(trimmed) => trimmed,
                    // A sample of no duration has none of its own.
                    None if p.duration == 0 => fill,
                    None => stored,
                }
            };
            let mut side_data = Vec::new();
            // A sample of the first, encrypted, sample description.
            if plan.encrypted && description.unwrap_or(1) == 1 {
                side_data.push(SideData {
                    typ: "encryption_info",
                    entries: vec![("side_data_type", s("Encryption info"))],
                });
            }
            // Audio presented from inside its first packet: the samples
            // before the start are skipped, as `ffprobe` marks the packet.
            if first.get(i).copied().unwrap_or(false) && track.kind == TrackKind::Audio {
                let skip = match self.edits.get(i).copied().flatten() {
                    Some(edit) => edit.skip,
                    None => self.priming.get(i).copied(),
                };
                if let Some(skip) = skip.filter(|p| *p > 0) {
                    side_data.push(SideData {
                        typ: "skip_samples",
                        entries: vec![
                            ("side_data_type", s("Skip Samples")),
                            ("skip_samples", Value::Int(skip)),
                            ("discard_padding", Value::Int(0)),
                            ("skip_reason", Value::Int(0)),
                            ("discard_reason", Value::Int(0)),
                        ],
                    });
                }
            }
            if let Some(f) = first.get_mut(i) {
                *f = false;
            }
            let packet = Packet {
                stream: track.stream_index,
                pts,
                dts: p
                    .presentation_dts
                    .and_then(|r| i64::try_from(r.round()).ok())
                    .and_then(|d| d.checked_sub(self.shifts.get(i).copied().unwrap_or(0))),
                duration,
                size: u64::from(p.size),
                pos: Some(p.offset),
                keyframe,
                // Only the samples of the tables are marked.
                discard: !p.presented && !fragment,
                corrupt: false,
                side_data,
            };
            if !f(packet)? {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(offset: u64, count: u32) -> coreav_isobmff::SampleRun {
        coreav_isobmff::SampleRun {
            first_sample: 0,
            count,
            offset,
            sample_size: 1,
            dts: 0,
            sample_duration: 1,
            composition_offset: 0,
            keyframe: true,
        }
    }

    #[test]
    fn compressed_frames_are_placed_by_whole_frames_into_their_chunk() {
        // IMA 4:1 mono: 64 samples in 34 bytes; a chunk at 1,000 of 1,024
        // and 384 samples, the next at 5,000.
        let mut chunk = (0, u64::MAX);
        let frame = (64, 34);
        assert_eq!(
            frame_packet(&run(1000, 1024), frame, &mut chunk),
            Some((1000, 544))
        );
        assert_eq!(
            frame_packet(&run(2024, 384), frame, &mut chunk),
            Some((1544, 204))
        );
        assert_eq!(
            frame_packet(&run(5000, 1024), frame, &mut chunk),
            Some((5000, 544))
        );
        // A last run of a partial frame still takes a whole frame's bytes.
        assert_eq!(
            frame_packet(&run(6024, 10), frame, &mut chunk),
            Some((5544, 34))
        );
    }

    fn held(dts: i64, cts: i64, keyframe: bool) -> Held {
        Held {
            sample: 0,
            offset: 0,
            size: 1,
            dts,
            cts,
            duration: 10,
            keyframe,
        }
    }

    #[test]
    fn held_samples_are_searched_by_decode_time() {
        // I P B B P B B, 10 ticks each, offsets up to 30.
        let samples = [
            held(0, 10, true),
            held(10, 40, false),
            held(20, 20, false),
            held(30, 30, false),
            held(40, 70, false),
            held(50, 50, false),
            held(60, 60, false),
        ];
        let reach = Reach::of(&samples);
        let mut work = Work {
            held: 100,
            steps: 100,
        };
        assert_eq!(reach.first_from(&samples, 25, &mut work).unwrap(), Some(1));
        assert_eq!(reach.snap(&samples, 1, 25, &mut work).unwrap(), Some(30));
        assert_eq!(reach.first_past(&samples, 35, &mut work).unwrap(), Some(1));
        let segment = Segment {
            first: 0,
            last: 6,
            offset: 0,
            window: (20, 45),
            overlap: false,
        };
        assert_eq!(
            reach.last_presented(&samples, &segment, &mut work).unwrap(),
            Some(3)
        );
        // Out of steps, a search fails rather than runs on.
        let mut spent = Work {
            held: 100,
            steps: 0,
        };
        assert!(reach.first_from(&samples, 65, &mut spent).is_err());
    }

    #[test]
    fn segments_present_by_start_or_by_overlap() {
        let video = Segment {
            first: 0,
            last: 0,
            offset: 0,
            window: (100, 200),
            overlap: false,
        };
        assert!(!video.presents(90, 20));
        assert!(video.presents(100, 20));
        assert!(!video.presents(200, 20));
        let audio = Segment {
            overlap: true,
            ..video
        };
        assert!(audio.presents(90, 20));
        assert!(!audio.presents(80, 20));
    }

    #[test]
    fn the_listing_reads_the_printed_base_rate() {
        let stream = Stream {
            kind: StreamKind::Video,
            attached_pic: false,
            entries: vec![("r_frame_rate", s("30000/1001"))],
            disposition: Vec::new(),
            tags: Vec::new(),
            side_data: Vec::new(),
        };
        let listed = Listed::of(&stream, Some(7));
        assert_eq!(listed.rate, Some((30_000, 1001)));
        assert_eq!(listed.duration, Some(7));
        let unknown = Stream {
            entries: vec![("r_frame_rate", s("0/0"))],
            ..stream
        };
        assert_eq!(Listed::of(&unknown, None).rate, None);
    }
}
