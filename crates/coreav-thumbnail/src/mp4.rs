//! MP4, MOV and other ISO base media files: the video stream, its packets
//! and their presentation times after the edit list.

use std::collections::BTreeMap;

use coreav_decode::{Codec, CodecConfig, ConfigKind, StreamConfig};
use coreav_frame::{ChromaLocation, ColorInfo};
use coreav_isobmff::access::{AccessFormat, read_frame_access};
use coreav_isobmff::{
    Allowlist, Budget, Color, EditList, Limits, Movie, Packet, Rational, SampleDescription, Source,
    Track, TrackKind, probe,
};

use crate::select::Sample;
use crate::{
    Container, CutRule, Error, InBand, PacketRef, Plan, Presentation, PresentationWindow,
    ThumbnailOptions, Window,
};

/// The ISOBMFF allowlist: the default, and VP8 when the decode policy
/// allows it (the default leaves VP8 out, since the commands that copy
/// streams do not write it).
fn allowlist(options: &ThumbnailOptions) -> Allowlist {
    let mut codecs = Allowlist::default().codecs().to_vec();
    if options.policy.allowlist.allows(Codec::Vp8) {
        codecs.push(coreav_isobmff::Codec::Vp8);
    }
    Allowlist::new(&codecs)
}

pub(crate) fn plan<S: Source>(
    source: &mut S,
    options: &ThumbnailOptions,
    first: Option<usize>,
) -> Result<Plan, Error> {
    let limits = &options.isobmff_limits;
    let mut movie = probe(&mut *source, limits, &allowlist(options))?;
    if movie.needs_fragment_scan() {
        movie.scan_fragments(&mut *source, limits)?;
    }
    let track = movie
        .tracks
        .iter()
        .filter(|t| {
            t.kind == TrackKind::Video && options.stream.is_none_or(|s| s == t.stream_index)
        })
        // FFmpeg's default: the highest resolution, the first on a tie.
        .fold(None, |best: Option<&coreav_isobmff::Track>, t| {
            let area = |t: &coreav_isobmff::Track| {
                u32::from(t.description.width.unwrap_or(0))
                    * u32::from(t.description.height.unwrap_or(0))
            };
            match best {
                Some(b) if area(b) >= area(t) => Some(b),
                _ => Some(t),
            }
        })
        .ok_or(Error::NoVideo)?
        .clone();
    let description = &track.description;
    let codec = match description.codec {
        coreav_isobmff::Codec::H264 => Codec::H264,
        coreav_isobmff::Codec::Hevc => Codec::Hevc,
        coreav_isobmff::Codec::Av1 => Codec::Av1,
        coreav_isobmff::Codec::Vp9 => Codec::Vp9,
        coreav_isobmff::Codec::Vp8 => Codec::Vp8,
        coreav_isobmff::Codec::ProRes => Codec::ProRes,
        other => {
            return Err(Error::Unsupported(format!(
                "no decoder for {other:?} video"
            )));
        }
    };
    let config = if codec == Codec::ProRes {
        Some(CodecConfig {
            kind: ConfigKind::ProResFourcc,
            data: description.fourcc.0.to_vec(),
        })
    } else {
        description.codec_config.as_ref().and_then(|c| {
            let kind = match &c.kind.0 {
                b"avcC" => ConfigKind::Avcc,
                b"hvcC" => ConfigKind::Hvcc,
                b"av1C" => ConfigKind::Av1c,
                b"vpcC" => ConfigKind::Vpcc,
                _ => return None,
            };
            Some(CodecConfig {
                kind,
                data: c.data.clone(),
            })
        })
    };
    // One bound for every packet: the largest of the track's sample
    // descriptions', unknown when any is (`docs/design/decode.md`,
    // "Several sample descriptions").
    let max_num_ref_frames = track
        .later_descriptions
        .iter()
        .map(|d| coreav_isobmff::avc_max_num_ref_frames(&d.codec_config.as_ref()?.data))
        .fold(movie.max_num_ref_frames(track.index), |max, refs| {
            Some(max?.max(refs?))
        });
    let rule = match codec {
        Codec::H264 => movie
            .access_format(track.index)
            .map(|format| CutRule::H264 {
                format,
                max_num_ref_frames,
            }),
        Codec::Hevc => config
            .as_ref()
            .and_then(|c| c.data.get(21))
            .map(|b| CutRule::Hevc {
                length_size: (b & 3) + 1,
            }),
        Codec::Av1 if track.tables_valid && !track.external_data => Some(CutRule::Av1),
        _ => None,
    }
    .unwrap_or(CutRule::Container);
    // The parameter sets' colour is applied over this (`crate::plan`).
    let color = color_info(description.color);
    // The sample entry's size, as FFmpeg's demuxer passes it to the
    // decoder: FFmpeg's H.264 decoder crops a frame to it when the
    // parameter sets' cropped size is a line or column larger (an odd
    // size, which 4:2:0 cropping cannot express).
    let (width, height) = (
        description.width.map(u32::from),
        description.height.map(u32::from),
    );
    let stream = StreamConfig {
        codec,
        config,
        width,
        height,
        color,
    };
    let start = start_time(&movie);
    let timescale = u64::from(track.timescale.max(1));
    // A track with several sample descriptions is listed from its first
    // packet, where its descriptions' changes are found (`in_band`).
    let several = track.descriptions > 1;
    let window = match first {
        // Any error building the window (its work budget included) leaves
        // the plan to the full listing, which then gives its own result.
        None if options.seek && !several => {
            window(&movie, &track, &mut *source, options, start, timescale)
                .ok()
                .flatten()
        }
        _ => None,
    };
    let (mut listing, window, first_keyframe) = match window {
        Some((listing, bounds, first_keyframe)) => (listing, Some(bounds), Some(first_keyframe)),
        None => {
            let mut listing = Listing::new(&track);
            let mut budget = Budget::new(limits);
            let mut packets = movie.packets(track.index, &mut *source, limits, &mut budget)?;
            while let Some(packet) = packets.next() {
                if first.is_some_and(|n| listing.packets.len() >= n) {
                    break;
                }
                listing.push(packet?, timescale);
                if several {
                    listing.descriptions.push(packets.description());
                }
            }
            (listing, None, None)
        }
    };
    // An edit list CoreAV does not model leaves presentation times
    // unknown: use composition times, as if there were none.
    if listing.samples.iter().all(|s| s.time.is_none()) {
        for (s, p) in listing.samples.iter_mut().zip(&listing.packets) {
            s.time = seconds(Rational::from_int(i128::from(p.pts)), timescale);
            s.presented = true;
        }
    }
    let pixel_aspect = description
        .pixel_aspect
        .map(|(h, v)| (u64::from(h), u64::from(v)));
    let quarter_turns = match track.rotation {
        Some(degrees) => u8::try_from((degrees / 90) % 4).unwrap_or(0),
        None => 0,
    };
    let warnings = if listing.wrapped {
        vec!["composition offsets of a version 0 ctts read as signed".to_owned()]
    } else {
        Vec::new()
    };
    let presentation = presentation(&track, movie.timescale);
    let in_band = if several {
        in_band(&movie, &track, &mut *source, &listing, limits)?
    } else {
        InBand::default()
    };
    Ok(Plan {
        warnings,
        stream_index: track.stream_index,
        stream,
        quarter_turns,
        pixel_aspect,
        start,
        samples: listing.samples,
        packets: listing.packets,
        prefix: Vec::new(),
        in_band,
        rule,
        stated: listing.stated,
        window,
        first_keyframe,
        container: Container::Mp4,
        durations: listing.durations,
        time_base: Rational::new(1, timescale).unwrap_or(Rational::from_int(0)),
        presentation,
    })
}

/// Where the track's edit list presents its media, in media ticks: the
/// media edit of the simple model (its end rounded up to a whole tick), or
/// why the list is not of it.
fn presentation(track: &Track, movie_timescale: u32) -> Presentation {
    match track.edit_list {
        EditList::None => Presentation::All,
        EditList::Simple {
            media_time,
            segment,
            ..
        } => {
            let end = (segment != 0)
                .then(|| {
                    let ticks = u128::from(segment)
                        .checked_mul(u128::from(track.timescale))?
                        .div_ceil(u128::from(movie_timescale.max(1)));
                    i64::try_from(ticks).ok()?.checked_add(media_time)
                })
                .flatten();
            Presentation::Window(PresentationWindow {
                start: media_time,
                end,
            })
        }
        EditList::Multiple => Presentation::Unsupported(
            "an edit list of more than one media edit (docs/design/edit-lists.md)",
        ),
        EditList::Unsupported => Presentation::Unsupported("an edit list of an unsupported shape"),
    }
}

/// The packets a plan holds, and what selection needs of each.
struct Listing {
    samples: Vec<Sample>,
    packets: Vec<PacketRef>,
    stated: Vec<bool>,
    /// Decode durations in media ticks.
    durations: Vec<Option<u64>>,
    /// Some composition offsets were read signed (KD-CTTS-UNSIGNED).
    wrapped: bool,
    unsigned_offsets: bool,
    sync_table: bool,
    table_samples: u64,
    /// Per packet, its sample description index, for a track with several.
    descriptions: Vec<Option<u32>>,
}

impl Listing {
    fn new(track: &Track) -> Self {
        Self {
            samples: Vec::new(),
            packets: Vec::new(),
            stated: Vec::new(),
            durations: Vec::new(),
            wrapped: false,
            unsigned_offsets: track.ctts_signed() == Some(false),
            sync_table: track.sync_table(),
            table_samples: u64::from(track.sample_count),
            descriptions: Vec::new(),
        }
    }

    fn push(&mut self, mut p: Packet, timescale: u64) {
        // Known divergence KD-CTTS-UNSIGNED: a version 0 `ctts` of an ISO
        // file that stores negative offsets. The probe reports them as
        // stored (unsigned); presentation times near 2^32 ticks cannot place
        // a frame, so here they are read signed, as their writer meant,
        // with a warning (`docs/design/thumbnails.md`).
        if self.unsigned_offsets && p.cts.saturating_sub(p.dts) >= 1 << 31 {
            self.wrapped = true;
            p.cts = p.cts.saturating_sub(1 << 32);
            p.pts = p
                .pts
                .and_then(|t| t.checked_add(Rational::from_int(-(1i128 << 32))));
            p.presented = true;
        }
        let time = p.pts.and_then(|t| seconds(t, timescale));
        let end = p
            .pts
            .and_then(|t| t.checked_add(Rational::from_int(i128::from(p.duration))))
            .and_then(|t| seconds(t, timescale));
        self.samples.push(Sample {
            time,
            end,
            presented: p.presented,
            keyframe: p.keyframe,
        });
        self.packets.push(packet_ref(&p));
        self.durations.push(Some(u64::from(p.duration)));
        self.stated
            .push(self.sync_table || p.sample >= self.table_samples);
    }
}

/// What a decoder configured with the track's first sample description
/// needs before the packets of a track with several (ISO/IEC 14496-12
/// 8.5.2 and 8.7.4; `docs/design/decode.md`, "Several sample
/// descriptions"): the parameter sets of a description's `avcC` (ISO/IEC
/// 14496-15 5.3.3.1) before each packet whose record differs from the
/// packet's before it, which must then hold an IDR picture, where a new
/// sequence parameter set can take effect (ITU-T H.264 7.4.1.2.1), and
/// before each sync sample whose record differs from the first
/// description's, so that decoding can start there. Refused: a description
/// of another codec, another codec's record changing, a record with another
/// NAL unit length than the first's, and a change at a packet that holds no
/// IDR picture. The NAL units read to find the IDR pictures, over every
/// change, count against `max_sample_work`.
fn in_band<S: Source>(
    movie: &Movie,
    track: &Track,
    source: &mut S,
    listing: &Listing,
    limits: &Limits,
) -> Result<InBand, Error> {
    fn record(d: &SampleDescription) -> Option<&[u8]> {
        d.codec_config.as_ref().map(|c| c.data.as_slice())
    }
    let first = &track.description;
    let refuse = |why: String| Error::Unsupported(format!("the video track {why}"));
    let mut in_band = InBand::default();
    let mut slots = BTreeMap::new();
    let mut last = record(first);
    let mut work = 0u64;
    let packets = listing.packets.iter().zip(&listing.descriptions);
    for (index, (packet, &number)) in packets.enumerate() {
        let entry = match number {
            Some(1) => Some(first),
            Some(n) => n
                .checked_sub(2)
                .and_then(|i| track.later_descriptions.get(usize::try_from(i).ok()?)),
            None => None,
        }
        .ok_or_else(|| refuse(format!("names no sample description at packet {index}")))?;
        if entry.codec != first.codec {
            return Err(refuse(format!(
                "changes from {} to {} at packet {index}, another codec",
                first.fourcc, entry.fourcc
            )));
        }
        let here = record(entry);
        let changed = here != last;
        last = here;
        if !changed && !(packet.keyframe && here != record(first)) {
            continue;
        }
        let Some(format @ AccessFormat::H264 { length_size }) = movie.access_format(track.index)
        else {
            return Err(refuse(format!(
                "changes its {} configuration record with its sample description, \
                 which CoreAV decodes only for H.264 with a readable first record",
                first.fourcc
            )));
        };
        if here.and_then(|r| r.get(4)).map(|b| (b & 3) + 1) != Some(length_size) {
            return Err(refuse(format!(
                "changes at packet {index} to a configuration record with another NAL unit \
                 length, or to none"
            )));
        }
        if changed {
            let frame = read_frame_access(format, source, packet.offset, packet.size, &mut || {
                work += 1;
                if work > limits.max_sample_work {
                    Err(coreav_isobmff::Error::LimitExceeded {
                        limit: coreav_isobmff::LimitKind::SampleWork,
                        offset: packet.offset,
                    })
                } else {
                    Ok(())
                }
            })?;
            if !frame.is_some_and(|f| f.idr) {
                return Err(refuse(format!(
                    "changes its parameter sets at packet {index}, which holds no IDR picture \
                     (ITU-T H.264 7.4.1.2.1)"
                )));
            }
        }
        let slot = match slots.get(&number) {
            Some(&slot) => slot,
            None => {
                let sets = here
                    .and_then(coreav_isobmff::avc_parameter_sets)
                    .ok_or_else(|| {
                        refuse(format!(
                            "changes at packet {index} to a configuration record that cannot be read"
                        ))
                    })?;
                let width = usize::from(length_size);
                let mut bytes = Vec::new();
                for unit in sets {
                    let len = (unit.len() as u64).to_be_bytes();
                    let (high, low) = len.split_at(len.len().saturating_sub(width));
                    if high.iter().any(|&b| b != 0) {
                        return Err(refuse(format!(
                            "has a parameter set longer than its NAL unit length allows at packet {index}"
                        )));
                    }
                    bytes.extend_from_slice(low);
                    bytes.extend_from_slice(unit);
                }
                let slot = in_band.sets.len();
                in_band.sets.push(bytes);
                slots.insert(number, slot);
                slot
            }
        };
        in_band.before.insert(index, slot);
    }
    Ok(in_band)
}

fn packet_ref(p: &Packet) -> PacketRef {
    PacketRef {
        offset: p.offset,
        size: u64::from(p.size),
        pts: p.cts,
        dts: Some(p.dts),
        keyframe: p.keyframe,
    }
}

/// Packets after the one `at` falls in that a window keeps, past the
/// reordering the composition offsets allow.
const AFTER: u32 = 32;

/// The packets around `at` (seconds from the file's start), found in the
/// sample tables (`docs/design/seek.md`): from two sync samples before the
/// last packet presented wholly before `at`, to `AFTER` packets past the
/// first that cannot be presented before it. With the bounds on the
/// packets left out, and the stream's first keyframe. `None` where the
/// tables cannot bound them (movie fragments, edit lists other than one
/// edit, offsets read signed by KD-CTTS-UNSIGNED, unknown times) or a
/// packet cannot be read: then every packet is listed.
fn window<S: Source>(
    movie: &Movie,
    track: &Track,
    source: &mut S,
    options: &ThumbnailOptions,
    start: Rational,
    timescale: u64,
) -> Result<Option<(Listing, Window, PacketRef)>, Error> {
    if movie.fragmented
        || movie.first_fragment().is_some()
        || !matches!(track.edit_list, EditList::None | EditList::Simple { .. })
    {
        return Ok(None);
    }
    let limits = &options.isobmff_limits;
    let mut budget = Budget::new(limits);
    let Some(seeker) = movie.seeker(track.index, &mut budget)? else {
        return Ok(None);
    };
    let samples = seeker.sample_count();
    let (low, high) = seeker.composition_offsets();
    if track.ctts_signed() == Some(false) && high >= 1 << 31 {
        return Ok(None);
    }
    let Some(first_sync) = seeker.first_sync() else {
        return Ok(None);
    };
    let first_keyframe = match movie
        .packets_from(
            track.index,
            u64::from(first_sync),
            &mut *source,
            limits,
            &mut budget,
        )?
        .next()
    {
        Some(Ok(p)) => p,
        _ => return Ok(None),
    };
    // One edit or none: every presentation time is the composition time
    // plus the same shift, in media ticks.
    let Some(shift) = first_keyframe
        .pts
        .and_then(|pts| pts.checked_add(Rational::from_int(-i128::from(first_keyframe.cts))))
    else {
        return Ok(None);
    };
    let ticks = |t: i64| Rational::from_int(i128::from(t)).checked_add(shift);
    // `at` as a composition time.
    let Some(at) = start
        .checked_add(options.at)
        .and_then(|at| Rational::new(at.num.checked_mul(i128::from(timescale))?, at.den))
        .and_then(|at| at.checked_add(Rational::new(-shift.num, shift.den)?))
    else {
        return Ok(None);
    };
    let floor = |r: Rational| i64::try_from(r.num.div_euclid(i128::from(r.den))).ok();
    let (Some(at_low), Some(at_high)) = (
        at.checked_add(Rational::from_int(-i128::from(low)))
            .and_then(floor),
        at.checked_add(Rational::from_int(-i128::from(high)))
            .and_then(floor),
    ) else {
        return Ok(None);
    };
    // Packets from `after` on have a decode time past `at - low`, so none
    // is presented at or before `at`.
    let after = seeker.samples_through(at_low);
    let end = after.saturating_add(AFTER).min(samples);
    // Packets before `whole` have a decode time below `at - high`, so each
    // is presented before `at`; decoding starts two sync samples before the
    // last of them (`docs/design/seek.md`, "Why the choice is the same").
    let whole = seeker.samples_through(at_high.saturating_sub(1));
    let begin = whole
        .checked_sub(1)
        .and_then(|last| seeker.sync_at_or_before(last))
        .and_then(|sync| sync.checked_sub(1))
        .and_then(|before| seeker.sync_at_or_before(before))
        .unwrap_or(0);
    if end <= begin {
        return Ok(None);
    }
    let mut listing = Listing::new(track);
    let packets = movie.packets_from(
        track.index,
        u64::from(begin),
        &mut *source,
        limits,
        &mut budget,
    )?;
    for packet in packets.take(usize::try_from(end - begin).unwrap_or(usize::MAX)) {
        match packet {
            Ok(p) => listing.push(p, timescale),
            Err(_) => return Ok(None),
        }
    }
    if listing.packets.len() != usize::try_from(end - begin).unwrap_or(usize::MAX) {
        return Ok(None);
    }
    let bound = |sample: u32, offset: i64| {
        seeker
            .dts(sample)
            .and_then(|dts| dts.checked_add(offset))
            .and_then(ticks)
            .and_then(|t| seconds(t, timescale))
    };
    let window = Window {
        before: match begin {
            0 => None,
            _ => match bound(begin, high) {
                Some(b) => Some(b),
                None => return Ok(None),
            },
        },
        after: if end < samples {
            match bound(end, low) {
                Some(a) => Some(a),
                None => return Ok(None),
            }
        } else {
            None
        },
    };
    Ok(Some((listing, window, packet_ref(&first_keyframe))))
}

/// `ticks` of `timescale` as seconds.
fn seconds(ticks: Rational, timescale: u64) -> Option<Rational> {
    Rational::new(ticks.num, ticks.den.checked_mul(timescale)?)
}

/// The file's start, as `ffmpeg -ss` counts from: the earliest stream start
/// on the presentation timeline (its leading empty edit, or without an
/// edit list its first sample's composition time; the same spans as the
/// probe's presentation duration). 0 when no stream's timing is known.
fn start_time(movie: &Movie) -> Rational {
    movie
        .presentation_spans()
        .iter()
        .map(|s| s.start)
        .fold(None, |min: Option<Rational>, t| match min {
            Some(m) if !t.checked_lt(m).unwrap_or(false) => Some(m),
            _ => Some(t),
        })
        .unwrap_or(Rational::from_int(0))
}

/// A container colour description as the frame model's.
pub(crate) fn color_info(c: Color) -> ColorInfo {
    let code = |v: Option<u16>| v.and_then(|v| u8::try_from(v).ok());
    ColorInfo {
        primaries: code(c.primaries),
        transfer: code(c.transfer),
        matrix: code(c.matrix),
        full_range: c.full_range,
        chroma_location: None::<ChromaLocation>,
    }
}
