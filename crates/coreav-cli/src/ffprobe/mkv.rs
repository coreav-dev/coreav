//! Sections of a Matroska or WebM file from `coreav-mkv`'s `Segment`.
//!
//! Every value is derived black box from what the pinned `ffprobe` prints
//! (`docs/design/compat-cli.md`): the container's elements first (RFC
//! 9559), and where `ffprobe` reports what its decoder or parser learns,
//! the same fact read from the codec private data or the first frame
//! ([`video`], [`audio`], [`image`]). What cannot be derived that way is
//! unknown.
//!
//! The rules found black box, each on crafted files as well as the
//! fixtures and the development corpus:
//!
//! - Streams: every `TrackEntry` in order (tracks with a codec CoreAV does
//!   not name are listed with an unknown codec), then every attached file.
//!   An attached JPEG or PNG picture is a video stream with `attached_pic`
//!   (so is a GIF one, which CoreAV does not read and lists as an
//!   attachment of unknown codec); other attachments name their codec by
//!   a prefix of the media type. Attachments have a 1/90000 time base and the format's
//!   start and duration.
//! - Metadata: one ordered dictionary per section whose keys match in any
//!   case, where setting a key that exists moves the last entry into its
//!   place (so `ENCODER` from `Tags` replaces `MuxingApp`'s `encoder`).
//!   Format: `Title`, `MuxingApp`, `DateUTC`, then the `SimpleTag`s of the
//!   tags that name no track, chapter or attachment, under their
//!   `TargetType`. Streams: `Language` (default `eng`, not `und`;
//!   `LanguageBCP47` is not shown), `Name`, `alpha_mode`, `stereo_mode`,
//!   then their tags. A simple tag is named `NAME-lang` for a `TagLanguage`
//!   other than `und` (and is also set as `NAME`, first, when it stores
//!   `TagDefault` 1; a `TagDefault` left out is not 1 here), nested ones
//!   `PARENT/NAME`.
//! - Chapters: the first edition's top-level atoms with a UID and a start,
//!   in order of start, titled by their last display. One without
//!   `ChapterTimeEnd` ends at the next listed one's start or the format's
//!   end (microseconds), whichever is first, and not before its start.
//! - Side data: mastering display values print as the fraction closest to
//!   the stored float with terms up to 2^31 - 1 (a 4-byte float keeps its
//!   power-of-two denominator). H.265 chroma location comes from the VUI
//!   (4:2:0 only, else `left`); AV1 `film_grain` from the sequence header.
//!   Stereo 3D names each packing a `StereoMode` states (none for the
//!   anaglyph modes), with both views packed and the camera values
//!   unknown. A block's BlockAdditional side data is listed only for a
//!   `BlockMore` of `BlockAddID` 1 that holds its `BlockAdditional`,
//!   data its codec defines; another ID is not read without the track's
//!   mapping, and a `BlockMore` without that mandatory element carries no
//!   data.
//! - Timing: the stream time base is `TimestampScale`. A stream none of
//!   whose packets come before `ffprobe`'s analysis stops (one packet per
//!   audio stream; for video one, the reorder depth plus two for H.264
//!   with B-frames and eight without, the rate window without a
//!   `DefaultDuration` of a tick or more) has the format's start and
//!   duration. A frame lasts its `DefaultDuration` truncated to the tick
//!   (none when that is zero), laced ones too; audio frames otherwise last
//!   what their codec's header says (a Vorbis packet that is not an audio
//!   packet lasts what the previous one did). Decode times of reordered
//!   video come from the presentation times through a buffer of the
//!   reorder depth; an H.264 decoder without the VUI's reorder depth finds
//!   it in the picture order counts of the first frames. A
//!   `V_MS/VFW/FOURCC` MPEG-4 track with reordering lists its timestamps
//!   as decode times, without presentation times. Frame rates come from
//!   `DefaultDuration`, else from H.264 VUI timing (packet durations and
//!   `avg_frame_rate`), and `r_frame_rate` otherwise from the first
//!   timestamps ([`video_rates`], [`rate_window`], [`estimate_rate`]).
//!
//! Rejected: decoding frames to learn what `ffprobe`'s decoders report
//! (CoreAV's probes do not decode); printing the container's values where
//! the decoder's would differ (a wrong value is worse than an unknown one,
//! so where neither is known the key is unknown).

mod audio;
pub(super) mod image;
pub(super) mod video;

use super::model::{Unit, Value, s, time};
use super::names;
use super::{Chapter, Entries, Packet, PacketSource, Probed, Report, SideData, Stream, StreamKind};
use crate::compat::{gcd, ratio};
use crate::error::Failure;
use crate::names as cnames;
use coreav_isobmff::{Profile, Source};
use coreav_mkv::{Allowlist, Codec, Encoding, Limits, Segment, SimpleTag, Track, TrackKind};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};

/// `ffprobe`'s format name and long name for every Matroska and WebM file.
const FORMAT_NAME: &str = "matroska,webm";
const FORMAT_LONG_NAME: &str = "Matroska / WebM";

/// The time base of an attachment stream, which has no packets.
const ATTACHMENT_TIME_BASE: i64 = 90_000;

/// Packets read of each audio and video stream for its start time, its
/// first frame and the extent of `ffprobe`'s stream analysis.
const EARLY_PACKETS: usize = 48;

/// The largest terms of the frame rate `DefaultDuration` gives.
const FRAME_RATE_TERMS: u128 = 30_000;

/// Packets of a video stream without `DefaultDuration` that `ffprobe`
/// reads to estimate its frame rate; they are listed without a duration.
const RATE_PACKETS: usize = 41;

/// Bytes of a frame read for its header.
const FRAME_HEAD: u64 = 64;

/// Element visits allowed for the early packets.
const EARLY_WORK: u64 = 1 << 16;

/// Bytes of clusters, from the first packet, and packets the early scan
/// reads at most: a track with few packets must not make it read the file.
const EARLY_SPAN: u64 = 64 << 20;
const EARLY_TOTAL: usize = 1 << 14;

/// Probes `source` as Matroska.
pub fn probe<'a, S: Source + 'a>(
    mut source: S,
    size: u64,
    name: &str,
) -> Result<Probed<'a>, Failure> {
    let limits = Limits::default();
    let segment = coreav_mkv::probe_with_metadata(&mut source, &limits, &Allowlist::any())
        .map_err(Failure::Matroska)?;
    let early = early_packets(&segment, &mut source, &limits);
    // The tracks `ffprobe` lists, numbered in order.
    let mut next = 0usize;
    let infos: Vec<Info> = segment
        .tracks
        .iter()
        .zip(&early)
        .map(|(track, first)| {
            let index = listed(track).then(|| {
                next += 1;
                next - 1
            });
            Info::new(&segment, track, first, index)
        })
        .collect();
    let pictures: Vec<Option<image::Picture>> = segment
        .attachments
        .iter()
        .map(|a| attached_picture(a, &mut source))
        .collect();
    let mut report = report(&segment, &infos, &early, &pictures, size, name);
    report.setups = vec![super::Setup::default(); report.streams.len()];
    for (track, info) in segment.tracks.iter().zip(&infos) {
        if let Some(setup) = info.stream.and_then(|i| report.setups.get_mut(i)) {
            let video = track.video.unwrap_or_default();
            *setup = super::Setup {
                config: track.codec_private.clone(),
                width: video.pixel_width.and_then(|v| u32::try_from(v).ok()),
                height: video.pixel_height.and_then(|v| u32::try_from(v).ok()),
                unreadable: match track.encoding {
                    Encoding::None | Encoding::HeaderStripping(_) => None,
                    _ => Some("its frames are stored compressed or encrypted"),
                },
                skip: info.rate.map_or(0, |rate| {
                    let samples = (i128::from(track.codec_delay) * i128::from(rate) + 500_000_000)
                        .div_euclid(1_000_000_000);
                    i64::try_from(samples).unwrap_or(0)
                }),
            };
        }
    }
    Ok(Probed {
        report,
        packets: Box::new(MkvPackets {
            segment,
            source,
            infos,
        }),
    })
}

/// The first packets of a track, with the first bytes of each and its
/// place among the packets of every track.
#[derive(Clone, Debug, Default)]
struct Early {
    packets: Vec<(coreav_mkv::Packet, Vec<u8>, usize)>,
}

/// Reads the first packets of every track, best effort: until every audio
/// and video track has [`EARLY_PACKETS`] of them.
fn early_packets<S: Source>(segment: &Segment, source: &mut S, limits: &Limits) -> Vec<Early> {
    let mut out = vec![Early::default(); segment.tracks.len()];
    let peek = Limits {
        max_block_work: limits.max_block_work.min(EARLY_WORK),
        ..*limits
    };
    let av = |t: &Track| matches!(t.kind, TrackKind::Video | TrackKind::Audio);
    let mut pending = segment.tracks.iter().filter(|t| av(t)).count();
    let mut iter = segment.packets(&mut *source, &peek);
    let mut seq = 0usize;
    let mut start = None;
    while pending > 0 && seq < EARLY_TOTAL {
        let Some(Ok(packet)) = iter.next() else {
            break;
        };
        let first = *start.get_or_insert(packet.block_offset);
        if packet.block_offset.saturating_sub(first) > EARLY_SPAN {
            break;
        }
        seq += 1;
        let (Some(early), Some(track)) =
            (out.get_mut(packet.track), segment.tracks.get(packet.track))
        else {
            continue;
        };
        if early.packets.len() >= EARLY_PACKETS {
            continue;
        }
        let whole = matches!(
            track.codec,
            Codec::H264
                | Codec::Mpeg2Video
                | Codec::Mpeg1Video
                | Codec::Mpeg4
                | Codec::Dts
                | Codec::Mjpeg
        );
        let head = if early.packets.is_empty() && whole {
            first_frame(track, &packet, iter.source_mut())
        } else {
            frame_head(track, &packet, iter.source_mut())
        };
        early.packets.push((packet, head, seq));
        if early.packets.len() == EARLY_PACKETS && av(track) {
            pending -= 1;
        }
    }
    out
}

/// Whether `ffprobe` lists a track, found black box: video, audio,
/// subtitle and metadata tracks (`TrackType` 1, 2, 17 and 33); not the
/// complex, logo, button and control ones.
fn listed(track: &Track) -> bool {
    matches!(track.track_type, 1 | 2 | 17 | 33)
}

/// How many packets of a stream `ffprobe`'s analysis reads before it has
/// what it needs, found black box: one for audio and for video codecs
/// without reordering, and for H.264 and H.265 a number that grows with
/// the reorder depth. `None` for streams it reads none of (subtitles).
fn analysis_packets(track: &Track, info: &Info) -> Option<usize> {
    if track.codec == Codec::Other {
        return None;
    }
    match track.kind {
        TrackKind::Audio => Some(1),
        TrackKind::Video if !info.rates.declared => Some(info.rates.window.max(1)),
        TrackKind::Video => Some(match (track.codec, info.delay) {
            (Codec::H264, Some(0) | None) => 8,
            (Codec::H264, Some(d)) => d as usize + 2,
            (Codec::Hevc, Some(d)) => d as usize + 1,
            (Codec::Mpeg2Video | Codec::Mpeg1Video, Some(d)) => d as usize + 2,
            _ => 1,
        }),
        _ => None,
    }
}

/// The first bytes of a frame, the stripped header included; empty when
/// the frame cannot be read or is compressed or encrypted.
fn frame_head<S: Source>(track: &Track, packet: &coreav_mkv::Packet, source: &mut S) -> Vec<u8> {
    // Only the head is read: a long stripped header is not copied whole
    // for every packet.
    let keep = usize::try_from(FRAME_HEAD).unwrap_or(usize::MAX);
    let mut head = match &track.encoding {
        Encoding::HeaderStripping(prefix) => prefix.get(..keep).unwrap_or(prefix).to_vec(),
        Encoding::None => Vec::new(),
        _ => return Vec::new(),
    };
    // The frame's bytes fill the head up to its length.
    let room = FRAME_HEAD.saturating_sub(head.len() as u64);
    let n = packet.stored_size.min(room);
    let mut bytes = vec![0u8; usize::try_from(n).unwrap_or(0)];
    if source.read_exact_at(packet.offset, &mut bytes).is_err() {
        return Vec::new();
    }
    head.extend_from_slice(&bytes);
    head
}

/// A frame's stored bytes, without the stripped header, bounded; `None`
/// when the frame cannot be read or is compressed or encrypted.
fn stored_frame<S: Source>(
    track: &Track,
    packet: &coreav_mkv::Packet,
    source: &mut S,
) -> Option<Vec<u8>> {
    if !matches!(
        track.encoding,
        Encoding::HeaderStripping(_) | Encoding::None
    ) {
        return None;
    }
    let n = packet.stored_size.min(1 << 20);
    let mut bytes = vec![0u8; usize::try_from(n).ok()?];
    source.read_exact_at(packet.offset, &mut bytes).ok()?;
    Some(bytes)
}

/// The whole first frame of a track (for parameter sets carried in band),
/// bounded.
fn first_frame<S: Source>(track: &Track, packet: &coreav_mkv::Packet, source: &mut S) -> Vec<u8> {
    let mut head = match &track.encoding {
        Encoding::HeaderStripping(prefix) => prefix.get(..1 << 20).unwrap_or(prefix).to_vec(),
        Encoding::None => Vec::new(),
        _ => return Vec::new(),
    };
    let n = packet.stored_size.min(1 << 20);
    let mut bytes = vec![0u8; usize::try_from(n).unwrap_or(0)];
    if source.read_exact_at(packet.offset, &mut bytes).is_err() {
        return Vec::new();
    }
    head.extend_from_slice(&bytes);
    head
}

/// The stream time base of every track: `TimestampScale` nanoseconds.
fn time_base(segment: &Segment) -> (i64, i64) {
    let scale = segment.timestamp_scale.max(1);
    let g = gcd(scale, 1_000_000_000);
    (
        i64::try_from(scale / g).unwrap_or(1),
        i64::try_from(1_000_000_000 / g).unwrap_or(1_000),
    )
}

/// How an audio codec's packets are timed when the container does not
/// say.
#[derive(Clone, Debug)]
enum Parser {
    None,
    Opus,
    Vorbis(audio::Vorbis),
    /// A fixed number of samples per packet.
    Fixed(u32),
    Ac3,
    Dts,
    Mpeg,
    Flac,
    /// Bytes per sample frame (all channels).
    Pcm(u64),
}

impl Parser {
    /// Whether a packet's samples are read from its frame header, rather
    /// than known from the track alone.
    fn reads_frame(&self) -> bool {
        !matches!(self, Parser::None | Parser::Fixed(_) | Parser::Pcm(_))
    }
}

/// What the packets of a track need, and what the stream section shows of
/// the codec.
#[derive(Clone, Debug)]
struct Info {
    /// The stream index `ffprobe` gives the track; `None` for a track it
    /// does not list.
    stream: Option<usize>,
    kind: TrackKind,
    /// The decoder's reorder depth: decode times are rebuilt from the
    /// presentation times with this delay (`has_b_frames`).
    delay: Option<u32>,
    /// Decode times are the presentation times.
    dts_is_pts: bool,
    /// `DefaultDuration` in nanoseconds, or for a video track without it,
    /// the period of its `FrameRate`.
    default_ns: Option<u64>,
    /// The frame rates and packet durations of a video track.
    rates: Rates,
    parser: Parser,
    /// The sample rate packet lengths count in.
    rate: Option<u32>,
    /// Parameter sets of the codec private data or the first frame.
    params: Option<video::Params>,
    /// The parameter sets came from the first frame (no `CodecPrivate`).
    in_band: Option<usize>,
    /// The bit rate the first frame header states.
    frame_bit_rate: Option<u32>,
    /// The MPEG-1 or MPEG-2 video sequence header.
    mpeg2: Option<video::Mpeg2>,
    /// The MPEG-4 Part 2 headers.
    mpeg4: Option<video::Mpeg4>,
    /// A WebM WebVTT track, whose blocks carry two header lines.
    webvtt: bool,
    /// The header lines a WebVTT track's stripped header holds
    /// ([`StrippedLines`]), read once.
    webvtt_stripped: StrippedLines,
    /// FLAC `STREAMINFO`: bits per sample.
    flac_bits: Option<u32>,
    /// The first bytes of the first frame.
    first: Vec<u8>,
    /// The first frame of a Motion JPEG track.
    mjpeg: Option<image::Picture>,
}

impl Info {
    fn new(segment: &Segment, track: &Track, early: &Early, stream: Option<usize>) -> Self {
        let tb = time_base(segment);
        let frame_rate_ns = track
            .video
            .and_then(|v| v.frame_rate)
            .filter(|_| track.kind == TrackKind::Video)
            .map(|r| 1e9 / r)
            .filter(|ns| ns.is_finite() && *ns >= 1.0 && *ns < 1e15);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let default_ns = track
            .default_duration
            .or(frame_rate_ns.map(|ns| ns.round() as u64));
        let private = codec_config(track).unwrap_or_default();
        let first = early.packets.first().map(|(_, head, _)| head.as_slice());
        let (params, in_band) = params(track, early);
        let mpeg2 = match track.codec {
            Codec::Mpeg2Video | Codec::Mpeg1Video => {
                // The sequence header of the codec private data, else of
                // the first frame, which then also gives the configuration.
                let in_frame = first.and_then(video::mpeg2);
                let mut seq = video::mpeg2(private).or(in_frame);
                if let (Some(seq), Some(frame)) = (seq.as_mut(), in_frame) {
                    seq.picture = frame.picture;
                }
                seq
            }
            _ => None,
        };
        let mpeg4 = match track.codec {
            Codec::Mpeg4 => video::mpeg4(private).or_else(|| first.and_then(video::mpeg4)),
            _ => None,
        };
        let delay = match track.codec {
            Codec::H264 => {
                params.and_then(|p| h264_reorder(&p).or_else(|| poc_depth(&p, track, early)))
            }
            Codec::Hevc => params.and_then(|p| p.reorder),
            Codec::Mpeg2Video | Codec::Mpeg1Video => mpeg2.map(|m| u32::from(!m.low_delay)),
            Codec::Mpeg4 => mpeg4.and_then(|m| m.low_delay).map(|l| u32::from(!l)),
            _ => Some(0),
        };
        let rate = track.audio.and_then(|a| a.sample_rate());
        let parser = match track.codec {
            Codec::Opus => Parser::Opus,
            Codec::Vorbis => audio::vorbis(private).map_or(Parser::None, Parser::Vorbis),
            Codec::Aac => Parser::Fixed(1024),
            Codec::Ac3 | Codec::Eac3 => Parser::Ac3,
            Codec::Dts => Parser::Dts,
            Codec::Mp3 | Codec::Mp2 => Parser::Mpeg,
            Codec::Flac => Parser::Flac,
            Codec::Pcm => {
                let bits = track.audio.and_then(|a| a.bit_depth).unwrap_or(0);
                let channels = track.audio.map_or(1, |a| a.channels);
                (bits / 8)
                    .checked_mul(channels)
                    .map_or(Parser::None, Parser::Pcm)
            }
            _ => Parser::None,
        };
        let frame_bit_rate = match track.codec {
            Codec::Ac3 => first.and_then(audio::ac3).and_then(|(_, r)| r),
            Codec::Eac3 => first.and_then(audio::eac3_bit_rate),
            Codec::Dts => first.and_then(audio::dts).and_then(|(_, r, _)| r),
            Codec::Mp3 | Codec::Mp2 => first.and_then(audio::mpeg_audio).and_then(|(_, r)| r),
            _ => None,
        };
        let rates = if track.kind == TrackKind::Video {
            video_rates(
                track,
                default_ns,
                delay,
                params.as_ref(),
                early,
                tb,
                segment.timestamp_scale,
            )
        } else {
            Rates::default()
        };
        Self {
            stream,
            kind: track.kind,
            delay,
            dts_is_pts: track.codec.decodes_in_presentation_order(),
            default_ns,
            rates,
            parser,
            rate,
            params,
            in_band,
            frame_bit_rate,
            mpeg2,
            mpeg4,
            webvtt: track.codec_id.starts_with("D_WEBVTT/"),
            webvtt_stripped: StrippedLines::of(track),
            first: first.map(<[u8]>::to_vec).unwrap_or_default(),
            mjpeg: first
                .filter(|_| track.codec == Codec::Mjpeg)
                .and_then(image::jpeg),
            flac_bits: (track.codec == Codec::Flac)
                .then(|| flac_streaminfo(private))
                .flatten()
                .map(|(bits, _)| bits),
        }
    }

    /// Samples in a packet, from its first bytes.
    fn samples(&self, head: &[u8], size: u64) -> Option<u64> {
        match &self.parser {
            Parser::None => None,
            Parser::Opus => audio::opus_samples(head).map(u64::from),
            Parser::Vorbis(_) => None,
            Parser::Fixed(n) => Some(u64::from(*n)),
            Parser::Ac3 => audio::ac3(head).map(|(n, _)| u64::from(n)),
            Parser::Dts => audio::dts(head).map(|(n, _, _)| u64::from(n)),
            Parser::Mpeg => audio::mpeg_audio(head).map(|(n, _)| u64::from(n)),
            Parser::Flac => audio::flac_samples(head).map(u64::from),
            Parser::Pcm(frame) => (*frame > 0).then(|| size / frame),
        }
    }
}

/// What a decoder takes as the codec's configuration from `CodecPrivate`:
/// for a `V_MS/VFW/FOURCC` track, what follows its 40-byte
/// `BITMAPINFOHEADER`.
fn codec_config(track: &Track) -> Option<&[u8]> {
    let private = track.codec_private.as_deref()?;
    if vfw(track) {
        private.get(40..)
    } else {
        Some(private)
    }
}

fn vfw(track: &Track) -> bool {
    track.codec_id == "V_MS/VFW/FOURCC"
}

/// A `V_MS/VFW/FOURCC` MPEG-4 Part 2 track whose frames are reordered:
/// found black box, its blocks' timestamps are listed as decode times,
/// with no presentation time, and its stream has the format's duration.
fn vfw_reordered(track: &Track, info: &Info) -> bool {
    vfw(track) && track.codec == Codec::Mpeg4 && info.delay.is_some_and(|d| d > 0)
}

/// A FLAC `CodecPrivate` (`fLaC`, then metadata blocks, the first of
/// which is `STREAMINFO`): bits per sample and the size of `STREAMINFO`.
fn flac_streaminfo(private: &[u8]) -> Option<(u32, usize)> {
    if private.get(..4)? != b"fLaC" {
        return None;
    }
    let header = private.get(4..8)?;
    if header.first()? & 0x7F != 0 {
        return None;
    }
    let info = private.get(8..42)?;
    let bits = ((u32::from(*info.get(12)?) & 1) << 4 | u32::from(*info.get(13)?) >> 4) + 1;
    Some((bits, info.len()))
}

/// The reorder depth an H.264 decoder starts with: the VUI's
/// `max_num_reorder_frames`, or none for the profiles without B slices.
fn h264_reorder(p: &video::Params) -> Option<u32> {
    if let Some(n) = p.reorder {
        return Some(n);
    }
    // Baseline and the intra profiles (constraint_set3 with High
    // profiles) have no reordering.
    if p.profile_idc == 66 || (p.constraint_set3 && matches!(p.profile_idc, 100 | 110 | 122 | 244))
    {
        return Some(0);
    }
    None
}

/// The reorder depth of an H.264 track whose parameter sets do not state
/// it, as its decoder finds it in the first frames (found black box: the
/// picture order counts, not the container's timestamps, set it): only
/// for an `avcC` track whose frames can be read.
fn poc_depth(p: &video::Params, track: &Track, early: &Early) -> Option<u32> {
    let syntax = p.slice?;
    let private = track.codec_private.as_deref()?;
    if private.first() != Some(&1) || early.packets.is_empty() {
        return None;
    }
    let length_size = usize::from(private.get(4)? & 3) + 1;
    video::h264_poc_depth(
        &syntax,
        length_size,
        early.packets.iter().map(|(_, head, _)| head.as_slice()),
    )
}

/// The parameter sets of a track's codec private data, else of its first
/// frame; and, for the latter, the bytes a decoder would take as its
/// configuration (`extradata_size`).
fn params(track: &Track, early: &Early) -> (Option<video::Params>, Option<usize>) {
    let private = track.codec_private.as_deref().filter(|p| !p.is_empty());
    match track.codec {
        Codec::H264 => {
            if let Some(p) = private {
                let nals = if p.first() == Some(&1) {
                    video::avcc_nals(p)
                } else {
                    video::annexb_nals(p)
                };
                return (video::first_h264_sps(nals), None);
            }
            match early.packets.first() {
                Some((_, frame, _)) => {
                    let nals = video::annexb_nals(frame);
                    let sets: Vec<&[u8]> = nals
                        .iter()
                        .copied()
                        .filter(|n| n.first().is_some_and(|b| matches!(b & 0x1F, 7 | 8)))
                        .collect();
                    let mut state = (0, false);
                    super::ts::params::sets_before_slice(frame, false, &mut state);
                    let size = state.0;
                    (video::first_h264_sps(sets), Some(size).filter(|s| *s > 0))
                }
                None => (None, None),
            }
        }
        Codec::Hevc => {
            let Some(p) = private else {
                return (None, None);
            };
            let nals = if p.first() == Some(&1) {
                video::hvcc_nals(p)
            } else {
                video::annexb_nals(p)
            };
            let mut params = video::first_hevc_sps(nals);
            if let (Some(params), Some(&chroma), Some(&depth)) =
                (params.as_mut(), p.get(16), p.get(17))
            {
                params.chroma_format = u32::from(chroma & 3);
                params.bit_depth = u32::from(depth & 7) + 8;
            }
            (params, None)
        }
        Codec::Av1 => (private.and_then(video::av1c), None),
        _ => (None, None),
    }
}

fn int(v: impl TryInto<i64>) -> Value {
    v.try_into().map_or(Value::Na, Value::Int)
}

/// Bytes of metadata keys and values a report may hold across all its
/// sections: tags are copied per stream and per chapter, and nested tags
/// repeat their parents' names, so the file's own bound on what it keeps
/// is not one on what the report holds.
const METADATA_BYTES: u64 = 16 << 20;

/// An ordered dictionary as `ffprobe`'s metadata keeps one: keys match
/// whatever their ASCII case, and setting a key that exists moves the last
/// entry into its place and appends the new one. Entries past the
/// report's metadata budget are left out.
#[derive(Debug)]
struct Dict<'b> {
    entries: Vec<(String, String)>,
    /// Each key, lowercase, and its position.
    index: HashMap<String, usize>,
    budget: &'b Cell<u64>,
}

impl<'b> Dict<'b> {
    fn new(budget: &'b Cell<u64>) -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::new(),
            budget,
        }
    }

    /// Whether `bytes` more fit the budget, charging them when they do.
    fn charge(&self, bytes: usize) -> bool {
        let left = self.budget.get();
        match u64::try_from(bytes).ok().filter(|b| *b <= left) {
            Some(b) => {
                self.budget.set(left - b);
                true
            }
            None => false,
        }
    }

    fn set(&mut self, key: &str, value: &str) {
        if !self.charge(key.len().saturating_add(value.len())) {
            return;
        }
        let lower = key.to_ascii_lowercase();
        if let Some(i) = self.index.remove(&lower) {
            self.entries.swap_remove(i);
            if let Some((moved, _)) = self.entries.get(i) {
                self.index.insert(moved.to_ascii_lowercase(), i);
            }
        }
        self.index.insert(lower, self.entries.len());
        self.entries.push((key.to_owned(), value.to_owned()));
    }

    fn into_entries(self) -> Vec<(String, String)> {
        self.entries
    }
}

/// The tags of a segment by what they name, and the report's metadata
/// budget.
struct Meta<'s> {
    tags: &'s [coreav_mkv::Tag],
    by_track: HashMap<u64, Vec<usize>>,
    by_chapter: HashMap<u64, Vec<usize>>,
    by_attachment: HashMap<u64, Vec<usize>>,
    budget: Cell<u64>,
}

impl<'s> Meta<'s> {
    fn new(segment: &'s Segment) -> Self {
        let mut meta = Self {
            tags: &segment.tags,
            by_track: HashMap::new(),
            by_chapter: HashMap::new(),
            by_attachment: HashMap::new(),
            budget: Cell::new(METADATA_BYTES),
        };
        for (i, tag) in segment.tags.iter().enumerate() {
            for (uids, map) in [
                (&tag.track_uids, &mut meta.by_track),
                (&tag.chapter_uids, &mut meta.by_chapter),
                (&tag.attachment_uids, &mut meta.by_attachment),
            ] {
                let mut seen = HashSet::new();
                for uid in uids.iter().filter(|u| **u != 0) {
                    if seen.insert(*uid) {
                        map.entry(*uid).or_default().push(i);
                    }
                }
            }
        }
        meta
    }

    fn dict(&self) -> Dict<'_> {
        Dict::new(&self.budget)
    }

    /// Adds the simple tags of the tags `map` lists for `uid`.
    fn add(&self, dict: &mut Dict<'_>, map: &HashMap<u64, Vec<usize>>, uid: u64) {
        for i in map.get(&uid).into_iter().flatten() {
            if let Some(tag) = self.tags.get(*i) {
                add_simple_tags(dict, &tag.simple_tags, "");
            }
        }
    }
}

/// Adds simple tags to `dict`: a name, with `-` and the language when it
/// is not `und` (and before it the bare name too when the tag stores
/// `TagDefault` 1); nested tags are named `PARENT/CHILD`.
fn add_simple_tags(dict: &mut Dict<'_>, tags: &[SimpleTag], prefix: &str) {
    for tag in tags {
        // Every tag visited is charged its key, which is built only when
        // it fits the budget, whether or not it stores a value.
        let cost = prefix
            .len()
            .saturating_add(tag.name.len())
            .saturating_add(1);
        if !dict.charge(cost) {
            return;
        }
        let mut key = format!("{prefix}{}", tag.name);
        let lang = tag
            .language
            .as_deref()
            .filter(|l| !l.is_empty() && *l != "und");
        if let (Some(lang), Some(value)) = (lang, &tag.string) {
            // A tag stored as its language's default is also set under
            // its bare name, first.
            if tag.default_stored && tag.default {
                dict.set(&key, value);
            }
            key.push('-');
            key.push_str(lang);
        }
        if let Some(value) = &tag.string {
            dict.set(&key, value);
        }
        if !tag.children.is_empty() {
            add_simple_tags(dict, &tag.children, &format!("{prefix}{}/", tag.name));
        }
    }
}

/// Bytes of an attached image read for its header.
const PICTURE_HEAD: u64 = 1 << 18;

/// The picture an attachment holds, when `ffprobe` shows it as one: a JPEG
/// or PNG image whose header can be read.
fn attached_picture<S: Source>(
    attachment: &coreav_mkv::Attachment,
    source: &mut S,
) -> Option<image::Picture> {
    let codec = attachment_codec(&attachment.media_type)?;
    if !matches!(codec, "mjpeg" | "png") {
        return None;
    }
    let n = usize::try_from(attachment.size.min(PICTURE_HEAD)).ok()?;
    let mut data = vec![0u8; n];
    source
        .read_exact_at(attachment.data_offset, &mut data)
        .ok()?;
    match codec {
        "mjpeg" => image::jpeg(&data),
        _ => image::png(&data),
    }
}

fn report(
    segment: &Segment,
    infos: &[Info],
    early: &[Early],
    pictures: &[Option<image::Picture>],
    size: u64,
    name: &str,
) -> Report {
    let tb = time_base(segment);
    let mut report = Report::default();
    let meta = Meta::new(segment);
    // Where `ffprobe`'s analysis stops reading: once every audio and video
    // stream has the packets it needs. A stream none of whose packets come
    // before then has its start and duration filled from the format's.
    // A stream without a decoder keeps the analysis reading: every stream
    // with a packet in the early scan is seen then.
    let undecoded = segment
        .tracks
        .iter()
        .any(|t| listed(t) && t.codec == Codec::Other);
    let window = segment
        .tracks
        .iter()
        .zip(infos)
        .zip(early)
        .filter_map(|((track, info), e)| {
            let n = analysis_packets(track, info)?;
            let last = e.packets.get(n.saturating_sub(1)).or(e.packets.last())?;
            Some(last.2)
        })
        .max()
        .filter(|_| !undecoded);
    let starts: Vec<Option<i64>> = segment
        .tracks
        .iter()
        .zip(infos)
        .zip(early)
        .map(|((_, info), e)| {
            let first = e.packets.first()?;
            if window.is_some_and(|w| first.2 > w) {
                return None;
            }
            start_pts(e, Some(info))
        })
        .collect();
    let format_start = starts
        .iter()
        .flatten()
        .filter_map(|t| to_micros(*t, tb))
        .min();
    let duration_us = format_duration(segment);
    // Every track is kept (`Allowlist::any`), so none is dropped.
    for ((track, info), start) in segment.tracks.iter().zip(infos).zip(&starts) {
        let Some(index) = info.stream else {
            continue;
        };
        let timing = match start {
            Some(t) => Timing {
                start: Some(*t),
                duration: vfw_reordered(track, info)
                    .then(|| duration_us.map(|us| from_micros(us, tb)))
                    .flatten(),
            },
            None => Timing {
                start: format_start.map(|us| from_micros(us, tb)),
                duration: duration_us.map(|us| from_micros(us, tb)),
            },
        };
        report
            .streams
            .push(stream(&meta, track, info, index, timing, tb));
    }
    report.time_bases = vec![tb; report.streams.len()];
    for (attachment, picture) in segment.attachments.iter().zip(pictures) {
        let index = report.streams.len();
        report.streams.push(attachment_stream(
            &meta,
            attachment,
            *picture,
            index,
            format_start,
            duration_us,
        ));
        report.time_bases.push((1, ATTACHMENT_TIME_BASE));
    }
    report.format = format(report.streams.len(), format_start, duration_us, size, name);
    report.format_tags = format_tags(segment, &meta);
    report.chapters = chapters(segment, &meta);
    report
}

/// The segment's `Duration` in microseconds, as `ffprobe` computes it: in
/// double precision, truncated.
fn format_duration(segment: &Segment) -> Option<i64> {
    let ticks = segment.duration.filter(|d| d.is_finite() && *d > 0.0)?;
    #[allow(clippy::cast_precision_loss)]
    let us = ticks * segment.timestamp_scale as f64 / 1000.0;
    #[allow(clippy::cast_possible_truncation)]
    let us = us as i64;
    Some(us)
}

/// A stream's start and duration in its time base.
#[derive(Clone, Copy, Debug)]
struct Timing {
    start: Option<i64>,
    duration: Option<i64>,
}

/// Microseconds in ticks of `(num, den)`, rounded to the nearest.
fn from_micros(us: i64, (num, den): (i64, i64)) -> i64 {
    let n = i128::from(us) * i128::from(den);
    let d = i128::from(num.max(1)) * 1_000_000;
    let q = (2 * n + d).div_euclid(2 * d);
    i64::try_from(q).unwrap_or(0)
}

fn to_micros(ticks: i64, (num, den): (i64, i64)) -> Option<i64> {
    let v = i128::from(ticks)
        .checked_mul(i128::from(num))?
        .checked_mul(1_000_000)?;
    i64::try_from(v.div_euclid(i128::from(den.max(1)))).ok()
}

/// The start of a stream: the smallest presentation time among its first
/// packets, as many as its decoder may reorder.
fn start_pts(early: &Early, info: Option<&Info>) -> Option<i64> {
    let n = info.and_then(|i| i.delay).map_or(1, |d| d as usize + 1);
    early
        .packets
        .iter()
        .take(n)
        .filter_map(|(p, _, _)| p.pts)
        .min()
}

fn format_tags(segment: &Segment, meta: &Meta<'_>) -> Vec<(String, String)> {
    let mut dict = meta.dict();
    if let Some(title) = &segment.title {
        dict.set("title", title);
    }
    if let Some(app) = &segment.muxing_app {
        dict.set("encoder", app);
    }
    if let Some(date) = segment.date_utc.and_then(date_string) {
        dict.set("creation_time", &date);
    }
    // Tags that name no track, chapter or attachment (an edition is not
    // told apart), under their `TargetType`.
    let global = |t: &&coreav_mkv::Tag| {
        [&t.track_uids, &t.chapter_uids, &t.attachment_uids]
            .iter()
            .all(|uids| uids.iter().all(|uid| *uid == 0))
    };
    for tag in segment.tags.iter().filter(global) {
        let prefix = tag
            .target_type
            .as_deref()
            .map_or(String::new(), |t| format!("{t}/"));
        add_simple_tags(&mut dict, &tag.simple_tags, &prefix);
    }
    dict.into_entries()
}

/// `DateUTC` (nanoseconds since 2001-01-01) as `ffprobe` prints
/// `creation_time`: `YYYY-MM-DDTHH:MM:SS.uuuuuuZ`.
fn date_string(ns: i64) -> Option<String> {
    // 2001-01-01 is 978307200 s after 1970-01-01.
    let micros = i128::from(ns).div_euclid(1000) + 978_307_200_000_000;
    let secs = i64::try_from(micros.div_euclid(1_000_000)).ok()?;
    let frac = micros.rem_euclid(1_000_000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{frac:06}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    ))
}

/// The proleptic Gregorian date of a day count from 1970-01-01.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn format(
    streams: usize,
    start_us: Option<i64>,
    duration_us: Option<i64>,
    size: u64,
    name: &str,
) -> Entries {
    let mut e: Entries = Vec::new();
    e.push(("filename", s(name)));
    e.push(("nb_streams", int(streams)));
    e.push(("nb_programs", Value::Int(0)));
    e.push(("nb_stream_groups", Value::Int(0)));
    e.push(("format_name", s(FORMAT_NAME)));
    e.push(("format_long_name", s(FORMAT_LONG_NAME)));
    e.push((
        "start_time",
        start_us.map_or(Value::Na, |us| time(us, 1, 1_000_000)),
    ));
    e.push((
        "duration",
        duration_us.map_or(Value::Na, |us| time(us, 1, 1_000_000)),
    ));
    e.push((
        "size",
        Value::Unit(i64::try_from(size).unwrap_or(i64::MAX), Unit::Byte),
    ));
    e.push((
        "bit_rate",
        duration_us
            .and_then(|us| crate::compat::format_bit_rate(size, us))
            .map_or(Value::Na, |b| {
                Value::Unit(i64::try_from(b).unwrap_or(i64::MAX), Unit::BitRate)
            }),
    ));
    e.push(("probe_score", Value::Int(100)));
    e
}

/// The chapters `ffprobe` lists, found black box: the top-level atoms of
/// the first edition that have a UID and a start, in order of start (an
/// atom starting before the last one listed is left out), and that do not
/// end before they start; titled by their last display. An atom without
/// `ChapterTimeEnd` ends where the next listed one starts or where the
/// format ends, whichever comes first, and not before it starts.
fn chapters(segment: &Segment, meta: &Meta<'_>) -> Vec<Chapter> {
    let Some(edition) = segment.editions.first() else {
        return Vec::new();
    };
    let mut listed = Vec::new();
    let mut last_start = 0u64;
    for chapter in &edition.chapters {
        let (Some(uid), Some(start)) = (chapter.uid.filter(|u| *u != 0), chapter.start) else {
            continue;
        };
        if start < last_start || chapter.end.is_some_and(|end| end < start) {
            continue;
        }
        last_start = start;
        listed.push((uid, start, chapter));
    }
    // The format's duration in nanoseconds, from its microseconds.
    let duration = format_duration(segment)
        .and_then(|us| u64::try_from(us).ok())
        .and_then(|us| us.checked_mul(1000));
    let starts: Vec<u64> = listed.iter().map(|(_, start, _)| *start).collect();
    let mut out = Vec::new();
    for (i, (uid, start, chapter)) in listed.into_iter().enumerate() {
        let mut dict = meta.dict();
        if let Some((title, _)) = chapter.displays.last() {
            dict.set("title", title);
        }
        meta.add(&mut dict, &meta.by_chapter, uid);
        let end = chapter.end.unwrap_or_else(|| {
            let next = starts.get(i + 1).copied();
            let end = match (next, duration) {
                (Some(n), Some(d)) => n.min(d),
                (Some(n), None) => n,
                (None, Some(d)) => d,
                (None, None) => start,
            };
            end.max(start)
        });
        let ns = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
        // The UID as `ffprobe` prints it: a signed 64-bit number.
        let id = i64::from_ne_bytes(uid.to_ne_bytes());
        out.push(Chapter {
            entries: vec![
                ("id", Value::Int(id)),
                ("time_base", s("1/1000000000")),
                ("start", Value::Int(ns(start))),
                ("start_time", time(ns(start), 1, 1_000_000_000)),
                ("end", Value::Int(ns(end))),
                ("end_time", time(ns(end), 1, 1_000_000_000)),
            ],
            tags: dict.into_entries(),
        });
    }
    out
}

/// `ffprobe`'s `codec_name`.
fn codec_name(track: &Track) -> Option<String> {
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

fn kind(kind: TrackKind) -> (StreamKind, &'static str) {
    match kind {
        TrackKind::Video => (StreamKind::Video, "video"),
        TrackKind::Audio => (StreamKind::Audio, "audio"),
        TrackKind::Subtitle => (StreamKind::Subtitle, "subtitle"),
        // A metadata track.
        TrackKind::Data => (StreamKind::Data, "unknown"),
    }
}

/// The frame rate `DefaultDuration` gives: 10^9 / `DefaultDuration`,
/// reduced to the closest fraction with terms up to 30000... as `ffprobe`
/// prints it.
#[cfg(test)]
fn frame_rate(default_duration: u64) -> Option<String> {
    let (n, d) = best_rational(
        1_000_000_000,
        u128::from(default_duration),
        FRAME_RATE_TERMS,
    )?;
    Some(format!("{n}/{d}"))
}

/// The fraction closest to `num / den` whose terms are at most `max`:
/// continued fraction convergents, then the best semiconvergent.
fn best_rational(num: u128, den: u128, max: u128) -> Option<(u128, u128)> {
    if den == 0 {
        return None;
    }
    let g = {
        let (mut a, mut b) = (num, den);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a.max(1)
    };
    let (num, den) = (num / g, den / g);
    if num <= max && den <= max {
        return Some((num, den));
    }
    let (mut p0, mut q0, mut p1, mut q1) = (0u128, 1u128, 1u128, 0u128);
    let (mut n, mut d) = (num, den);
    loop {
        if d == 0 {
            break;
        }
        let a = n / d;
        let p2 = a.checked_mul(p1)?.checked_add(p0)?;
        let q2 = a.checked_mul(q1)?.checked_add(q0)?;
        if p2 > max || q2 > max {
            // The best semiconvergent with terms in range.
            let mut k = a;
            if let Some(n) = (max - p0).checked_div(p1) {
                k = k.min(n);
            }
            if let Some(n) = (max - q0).checked_div(q1) {
                k = k.min(n);
            }
            let (ps, qs) = (k * p1 + p0, k * q1 + q0);
            if qs > 0 && k > 0 && 2 * k >= a {
                // Closer of the semiconvergent and the last convergent.
                let err = |p: u128, q: u128| -> u128 {
                    let x = p * den;
                    let y = num * q;
                    (x.abs_diff(y)) * 1_000_000 / (q * den).max(1)
                };
                if err(ps, qs) < err(p1, q1) {
                    return Some((ps, qs));
                }
            }
            return Some((p1, q1));
        }
        (p0, q0, p1, q1) = (p1, q1, p2, q2);
        (n, d) = (d, n - a * d);
    }
    Some((p1, q1))
}

/// A frame rate estimated from timestamps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Estimate {
    /// A standard rate the timestamps fit: frames per second as a fraction.
    Standard(u64, u64),
    /// No standard rate fits: the rate of the largest tick count that
    /// divides every interval, and whether the intervals are all equal.
    Base(u64, u64, bool),
}

/// The standard frame rates a rate estimate snaps to: twelfths of a frame
/// per second up to 30, whole rates up to 60, 80, 120 and 240, and the
/// NTSC rates.
fn standard_rates() -> impl Iterator<Item = (u64, u64)> {
    (1..=360u64)
        .map(|k| (k, 12))
        .chain((31..=60).map(|k| (k, 1)))
        .chain([80, 120, 240].map(|k| (k, 1)))
        .chain([24, 30, 60, 12, 15, 48].map(|k| (k * 1000, 1001)))
}

/// Estimates a frame rate from decode times in ticks of `tb`, found black
/// box on crafted timestamp sequences: among the standard rates whose
/// frames cover at least three quarters of the intervals (the rate times
/// the span, over the intervals), the one on whose grid the times fall
/// most evenly (the variance, in frames, of each time's distance from the
/// first to its nearest frame; the lower rate on a tie), when that variance
/// is under 0.01;
/// else the base rate of the intervals.
pub(super) fn estimate_rate(times: &[i64], (num, den): (i64, i64)) -> Option<Estimate> {
    let first = *times.first()?;
    let last = *times.last()?;
    let deltas: Vec<i64> = times
        .windows(2)
        .filter_map(|w| w.get(1)?.checked_sub(*w.first()?))
        .filter(|d| *d > 0)
        .collect();
    if deltas.is_empty() {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let tick = num as f64 / den as f64;
    let rest = times.get(1..).unwrap_or_default();
    #[allow(clippy::cast_precision_loss)]
    let count = rest.len().max(1) as f64;
    #[allow(clippy::cast_precision_loss)]
    let span = last.saturating_sub(first) as f64 * tick;
    let base = deltas.iter().fold(0u64, |g, d| gcd(g, d.unsigned_abs()));
    let rate_num = u64::try_from(den).ok()?;
    let rate_den = base.checked_mul(u64::try_from(num).ok()?)?;
    let g = gcd(rate_num, rate_den);
    let constant = deltas.iter().all(|d| Some(d) == deltas.first());
    // Equal intervals are their own exact rate.
    if constant {
        return Some(Estimate::Base(rate_num / g, rate_den / g, true));
    }
    let mut best: Option<((u64, u64), f64)> = None;
    for (n, d) in standard_rates() {
        #[allow(clippy::cast_precision_loss)]
        let fps = n as f64 / d as f64;
        if fps * span < 0.75 * count {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let frames = |ticks: i64| ticks as f64 * tick * fps;
        let (sum, squares) = rest.iter().fold((0.0, 0.0), |(s, q), &t| {
            let x = frames(t.saturating_sub(first));
            let e = x - x.round();
            (s + e, q + e * e)
        });
        let mean = sum / count;
        let error = squares / count - mean * mean;
        // A tie (the exact grids of a rate and of its multiples) keeps the
        // lower rate.
        if best.is_none_or(|(_, e)| error < e - 1e-9) {
            best = Some(((n, d), error));
        }
    }
    if let Some(((n, d), error)) = best {
        if error < 0.01 {
            let g = gcd(n, d);
            return Some(Estimate::Standard(n / g, d / g));
        }
    }
    Some(Estimate::Base(rate_num / g, rate_den / g, false))
}

/// The frame rate of an estimate as a fraction.
pub(super) fn estimate_fraction(e: Estimate) -> (u64, u64) {
    match e {
        Estimate::Standard(n, d) | Estimate::Base(n, d, _) => (n, d),
    }
}

/// How many packets of a video stream `ffprobe` reads to estimate its
/// frame rate, found black box on crafted timestamp sequences: 21 when
/// the time base is half a millisecond or finer; else 41, or from the
/// 32nd on, the first whose time, less two frames of the rate estimated
/// up to it, is 5 seconds or more past the first.
fn rate_window(times: &[i64], tb: (i64, i64)) -> usize {
    let fine = tb.1 >= tb.0.saturating_mul(2000);
    let cap = if fine { 21 } else { RATE_PACKETS };
    if fine || times.len() < 32 {
        return times.len().min(cap);
    }
    #[allow(clippy::cast_precision_loss)]
    let tick = tb.0 as f64 / tb.1 as f64;
    let first = times.first().copied().unwrap_or(0);
    for n in 32..=cap.min(times.len()) {
        let (Some(window), Some(&last)) = (times.get(..n), times.get(n - 1)) else {
            break;
        };
        let Some((rn, rd)) = estimate_rate(window, tb).map(estimate_fraction) else {
            continue;
        };
        #[allow(clippy::cast_precision_loss)]
        let frame = rd as f64 / rn.max(1) as f64;
        #[allow(clippy::cast_precision_loss)]
        let elapsed = last.saturating_sub(first) as f64 * tick;
        if elapsed - 2.0 * frame >= 5.0 {
            return n;
        }
    }
    times.len().min(cap)
}

/// What `ffprobe` shows of a video stream's frame rate and the durations
/// of its packets.
#[derive(Clone, Debug, Default)]
struct Rates {
    /// `r_frame_rate` and `avg_frame_rate`.
    shown: Option<(String, String)>,
    /// Packets read for the rate estimate, listed without a duration
    /// when nothing else gives them one.
    window: usize,
    /// What a packet after the window lasts, in ticks, when nothing else
    /// says.
    estimated_ticks: Option<i64>,
    /// What every packet lasts by the H.264 VUI's timing, in ticks.
    vui_ticks: Option<i64>,
    /// `DefaultDuration` gives the packets a duration (a tick or more).
    declared: bool,
}

/// The standard rate nearest `rate`, when within 0.9% of it.
fn snap_rate(rate: f64) -> Option<(u64, u64)> {
    let mut best: Option<((u64, u64), f64)> = None;
    for (n, d) in standard_rates() {
        #[allow(clippy::cast_precision_loss)]
        let s = n as f64 / d as f64;
        let distance = (rate - s).abs() / s;
        if best.is_none_or(|(_, b)| distance < b) {
            best = Some(((n, d), distance));
        }
    }
    let ((n, d), distance) = best?;
    let g = gcd(n, d);
    (distance < 0.009).then_some((n / g, d / g))
}

/// The frame rates of a video stream and the durations of its packets,
/// found black box.
///
/// - `r_frame_rate` is the declared rate (10^9 / `DefaultDuration`, the
///   closest fraction with terms up to 30000), or where there is none or
///   it lasts a tick or less, the rate estimated from the decode times of
///   the window's packets ([`rate_window`], [`estimate_rate`]).
/// - `avg_frame_rate` is the declared rate; without one, for H.264 the
///   rate of a frame of its VUI timing (in whole ticks) snapped to a
///   standard rate within 0.9%; else the estimated rate where the
///   window's mean rate is within 3.5% of it (a base rate: where the
///   intervals are all equal), and `0/0` otherwise.
/// - A packet lasts `DefaultDuration` (truncated to the tick; none when
///   that is zero), else for H.264 two ticks of its VUI timing; else the
///   window's packets have no duration and later ones last a frame of
///   the estimated rate.
fn video_rates(
    track: &Track,
    default_ns: Option<u64>,
    delay: Option<u32>,
    params: Option<&video::Params>,
    early: &Early,
    tb: (i64, i64),
    scale: u64,
) -> Rates {
    let pts: Vec<Option<i64>> = early
        .packets
        .iter()
        .take(RATE_PACKETS)
        .map(|(p, _, _)| p.pts)
        .collect();
    let all: Vec<i64> = if track.codec.decodes_in_presentation_order() {
        pts.iter().flatten().copied().collect()
    } else {
        let mut buffer = vec![None; delay.map_or(1, |d| d as usize + 1)];
        pts.iter()
            .filter_map(|t| reorder(&mut buffer, *t))
            .collect()
    };
    let window = rate_window(&all, tb);
    let times = all.get(..window).unwrap_or(&all);
    // One timestamp has no interval: the rate of one tick, no average.
    let estimate = match times {
        [_] => u64::try_from(tb.1)
            .ok()
            .zip(u64::try_from(tb.0).ok())
            .map(|(n, d)| {
                let g = gcd(n, d);
                Estimate::Base(n / g, d / g, false)
            }),
        _ => estimate_rate(times, tb),
    };
    let frame_ticks = |(n, d): (u64, u64)| -> Option<i64> {
        let x = u128::from(d).checked_mul(u128::try_from(tb.1).ok()?)?;
        let y = u128::from(n).checked_mul(u128::try_from(tb.0).ok()?)?;
        i64::try_from(x.checked_div(y)?).ok()
    };
    let fraction = |(n, d): (u64, u64)| format!("{n}/{d}");
    let dd_ticks = default_ns.map(|ns| ns / scale.max(1));
    let mut rates = Rates {
        window: times.len(),
        declared: dd_ticks.is_some_and(|t| t > 0),
        estimated_ticks: estimate
            .map(estimate_fraction)
            .and_then(frame_ticks)
            .filter(|_| dd_ticks.is_none_or(|t| t == 0)),
        ..Rates::default()
    };
    if let Some(ns) = default_ns {
        let Some((n, d)) = best_rational(1_000_000_000, u128::from(ns), FRAME_RATE_TERMS) else {
            return rates;
        };
        let avg = fraction((u64::try_from(n).unwrap_or(0), u64::try_from(d).unwrap_or(0)));
        let r = match estimate {
            Some(e) if ns <= scale => fraction(estimate_fraction(e)),
            _ => avg.clone(),
        };
        rates.shown = Some((r, avg));
        return rates;
    }
    let Some(estimate) = estimate else {
        return rates;
    };
    let r = estimate_fraction(estimate);
    // H.264: a frame lasts two ticks of the VUI's timing.
    if let (Codec::H264, Some((units, time_scale))) = (track.codec, params.and_then(|p| p.timing)) {
        let ticks = (|| {
            let x = u128::from(units)
                .checked_mul(2)?
                .checked_mul(u128::try_from(tb.1).ok()?)?;
            let y = u128::from(time_scale).checked_mul(u128::try_from(tb.0).ok()?)?;
            u64::try_from(x.checked_div(y)?).ok().filter(|t| *t > 0)
        })();
        if let Some(t) = ticks {
            rates.vui_ticks = i64::try_from(t).ok();
            let num = u64::try_from(tb.1).unwrap_or(1);
            let den = t.saturating_mul(u64::try_from(tb.0).unwrap_or(1)).max(1);
            #[allow(clippy::cast_precision_loss)]
            let avg = snap_rate(num as f64 / den as f64).unwrap_or_else(|| {
                let g = gcd(num, den).max(1);
                (num / g, den / g)
            });
            rates.shown = Some((fraction(r), fraction(avg)));
            return rates;
        }
    }
    let avg = match estimate {
        Estimate::Standard(n, d) => {
            let (Some(&first), Some(&last)) = (times.first(), times.last()) else {
                return rates;
            };
            #[allow(clippy::cast_precision_loss)]
            let mean = (times.len() - 1) as f64 * tb.1 as f64
                / (last.saturating_sub(first) as f64 * tb.0 as f64);
            #[allow(clippy::cast_precision_loss)]
            let rate = n as f64 / d as f64;
            if (mean / rate - 1.0).abs() < 0.035 {
                fraction(r)
            } else {
                "0/0".to_owned()
            }
        }
        Estimate::Base(_, _, true) => fraction(r),
        Estimate::Base(_, _, false) => "0/0".to_owned(),
    };
    rates.shown = Some((fraction(r), avg));
    rates
}

fn stream(
    meta: &Meta<'_>,
    track: &Track,
    info: &Info,
    index: usize,
    timing: Timing,
    tb: (i64, i64),
) -> Stream {
    let start = timing.start;
    let (stream_kind, kind_name) = kind(track.kind);
    let name = codec_name(track);
    let mut e: Entries = Vec::with_capacity(48);
    e.push(("index", int(index)));
    match &name {
        Some(n) => {
            e.push(("codec_name", s(n.as_str())));
            e.push((
                "codec_long_name",
                long_name(n).map_or(Value::Unset("unknown"), s),
            ));
        }
        None => {
            e.push(("codec_name", Value::Unset("unknown")));
            e.push(("codec_long_name", Value::Unset("unknown")));
        }
    }
    e.push((
        "profile",
        profile(track, info).map_or(Value::Unset("unknown"), s),
    ));
    e.push(("codec_type", s(kind_name)));
    let tag = match track.codec {
        Codec::ProRes => prores_fourcc(track),
        _ => track.fourcc,
    };
    match tag {
        Some(fourcc) => {
            e.push((
                "codec_tag_string",
                s(crate::compat::tag_string(coreav_isobmff::FourCc(fourcc))),
            ));
            e.push((
                "codec_tag",
                s(format!("0x{:08x}", u32::from_le_bytes(fourcc))),
            ));
        }
        None => {
            e.push(("codec_tag_string", s("[0][0][0][0]")));
            e.push(("codec_tag", s("0x0000")));
        }
    }
    let mut side_data = Vec::new();
    match track.kind {
        TrackKind::Video => video_entries(&mut e, track, info, &mut side_data),
        TrackKind::Audio => audio_entries(&mut e, track, info, name.as_deref()),
        TrackKind::Subtitle => {
            let size = track.image_size;
            e.push(("width", size.map_or(Value::Na, |(w, _)| int(w))));
            e.push(("height", size.map_or(Value::Na, |(_, h)| int(h))));
        }
        TrackKind::Data => {}
    }
    e.push(("id", Value::Na));
    let (r, avg) = match (&info.rates.shown, track.kind) {
        (Some((r, avg)), _) => (s(r.as_str()), s(avg.as_str())),
        (None, TrackKind::Video) => (Value::Na, Value::Na),
        (None, _) => (s("0/0"), s("0/0")),
    };
    e.push(("r_frame_rate", r));
    e.push(("avg_frame_rate", avg));
    e.push(("time_base", s(format!("{}/{}", tb.0, tb.1))));
    e.push(("start_pts", start.map_or(Value::Na, Value::Int)));
    e.push((
        "start_time",
        start.map_or(Value::Na, |v| time(v, tb.0, tb.1)),
    ));
    e.push(("duration_ts", timing.duration.map_or(Value::Na, Value::Int)));
    e.push((
        "duration",
        timing.duration.map_or(Value::Na, |v| time(v, tb.0, tb.1)),
    ));
    let bit_rate = match track.codec {
        Codec::Pcm => track.audio.and_then(|a| {
            let rate = u64::from(a.sample_rate()?);
            rate.checked_mul(a.channels)?.checked_mul(a.bit_depth?)
        }),
        // MPEG-1 video states its bit rate in the sequence header.
        Codec::Mpeg1Video | Codec::Mpeg2Video => info
            .mpeg2
            .filter(|m| m.profile_and_level.is_none())
            .map(|m| m.bit_rate),
        _ => info.frame_bit_rate.map(u64::from),
    };
    e.push((
        "bit_rate",
        bit_rate.map_or(Value::Na, |b| {
            Value::Unit(i64::try_from(b).unwrap_or(i64::MAX), Unit::BitRate)
        }),
    ));
    e.push(("max_bit_rate", Value::Na));
    let raw_bits = match track.codec {
        Codec::H264 => info.params.map(|p| p.bit_depth),
        Codec::Flac => info.flac_bits,
        Codec::Alac => alac_bits(track),
        Codec::Pcm => track
            .audio
            .and_then(|a| a.bit_depth)
            .filter(|b| *b == 24)
            .and_then(|b| u32::try_from(b).ok()),
        Codec::ProRes => prores_fourcc(track).map(|f| {
            if matches!(&f, b"ap4h" | b"ap4x") {
                12
            } else {
                10
            }
        }),
        Codec::Mjpeg => info.mjpeg.and_then(|p| p.bits),
        _ => None,
    };
    e.push((
        "bits_per_raw_sample",
        raw_bits.map_or(Value::Na, |b| s(b.to_string())),
    ));
    e.push(("nb_frames", Value::Na));
    e.push(("nb_read_frames", Value::Na));
    e.push(("nb_read_packets", Value::Na));
    if let Some(n) = extradata_size(track, info) {
        e.push(("extradata_size", int(n)));
    }
    super::metadata::stream(&mut e, codec_config(track), &info.first, None);
    let mut tags = meta.dict();
    if let Some(lang) = track_language(track) {
        tags.set("language", &lang);
    }
    if let Some(title) = &track.name {
        tags.set("title", title);
    }
    if track.video.is_some_and(|v| v.alpha_mode != 0) {
        tags.set("alpha_mode", "1");
    }
    if let Some(name) = track
        .video
        .filter(|v| v.stereo_mode_present)
        .and_then(|v| stereo_name(v.stereo_mode))
    {
        tags.set("stereo_mode", name);
    }
    if let Some(uid) = track.uid {
        meta.add(&mut tags, &meta.by_track, uid);
    }
    Stream {
        kind: stream_kind,
        attached_pic: false,
        entries: e,
        disposition: disposition(track),
        tags: tags.into_entries(),
        side_data,
    }
}

/// The `Language` element as `ffprobe` reports it: the stored value or the
/// default `eng`, not `und`.
fn track_language(track: &Track) -> Option<String> {
    // An empty element takes the default too (RFC 8794 6.1);
    // `LanguageBCP47` is not shown.
    let lang = track
        .language_iso639
        .clone()
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "eng".to_owned());
    (lang != "und").then_some(lang)
}

fn long_name(codec: &str) -> Option<&'static str> {
    names::long_name(codec).or(match codec {
        "dvd_subtitle" => Some("DVD subtitles"),
        "ffv1" => Some("FFmpeg video codec #1"),
        _ => None,
    })
}

fn profile(track: &Track, info: &Info) -> Option<String> {
    if let Some(m) = info.mpeg4 {
        return m
            .profile_and_level
            .and_then(video::mpeg4_profile)
            .map(|(name, _)| name.to_owned());
    }
    if track.codec == Codec::Dts {
        // The core alone; an extension substream names other profiles.
        return audio::dts(&info.first)
            .filter(|(_, _, core_only)| *core_only)
            .map(|_| "DTS".to_owned());
    }
    if track.codec == Codec::ProRes {
        return prores_fourcc(track)
            .and_then(|f| prores_profile(&f))
            .map(str::to_owned);
    }
    if let Some(p) = info.mjpeg {
        return p.profile.map(str::to_owned);
    }
    if let Some(pl) = info.mpeg2.and_then(|m| m.profile_and_level) {
        if pl & 0x80 != 0 {
            return match pl & 0x0F {
                0x02 | 0x05 => Some("4:2:2".to_owned()),
                _ => None,
            };
        }
        return Some(
            match (pl >> 4) & 7 {
                1 => "High",
                2 => "Spatially Scalable",
                3 => "SNR Scalable",
                4 => "Main",
                5 => "Simple",
                _ => return None,
            }
            .to_owned(),
        );
    }
    let profile = track.profile.and_then(cnames::profile);
    match track.codec {
        Codec::Vp8 => track.frame_profile.map(|v| v.to_string()),
        Codec::Aac | Codec::H264 | Codec::Hevc | Codec::Av1 | Codec::Vp9 | Codec::ProRes => profile,
        Codec::Mpeg2Video => None,
        _ => None,
    }
}

fn video_entries(e: &mut Entries, track: &Track, info: &Info, side_data: &mut Vec<SideData>) {
    let v = track.video.unwrap_or_default();
    let params = info.params;
    let mut size = v.pixel_width.zip(v.pixel_height);
    if track.codec == Codec::H264 {
        if let Some((w, h)) = params.and_then(|p| p.size) {
            size = Some((u64::from(w), u64::from(h)));
        }
    }
    let (w, h) = size.map_or((Value::Na, Value::Na), |(w, h)| (int(w), int(h)));
    e.push(("width", w.clone()));
    e.push(("height", h.clone()));
    // Without a decoder, `ffprobe` has no coded size, captions, film grain
    // or reference count to show.
    let decoder = track.codec != Codec::Other;
    if info.mpeg2.is_some() {
        // The MPEG-1 and MPEG-2 decoders leave the coded size unset.
        e.push(("coded_width", Value::Int(0)));
        e.push(("coded_height", Value::Int(0)));
    } else if decoder {
        e.push(("coded_width", w));
        e.push(("coded_height", h));
    }
    if decoder {
        e.push(("closed_captions", Value::Int(0)));
        // AV1: the sequence header's film grain flag.
        let grain = (track.codec == Codec::Av1)
            .then(|| {
                track
                    .codec_private
                    .as_deref()
                    .and_then(video::av1_film_grain)
            })
            .flatten();
        e.push(("film_grain", Value::Int(i64::from(grain.unwrap_or(false)))));
    }
    e.push((
        "has_b_frames",
        info.delay.map_or(Value::Na, |d| Value::Int(i64::from(d))),
    ));
    let (sar, dar) = match size {
        Some((w, h)) => {
            let shown = (
                w.saturating_sub(v.crop[2]).saturating_sub(v.crop[3]),
                h.saturating_sub(v.crop[0]).saturating_sub(v.crop[1]),
            );
            let (mut dw, mut dh) = v.display_aspect().unwrap_or(shown);
            // A frame packing two views declares the display size of one.
            match v.stereo_mode {
                1 | 11 => dw = dw.saturating_mul(2),
                2 | 3 => dh = dh.saturating_mul(2),
                _ => {}
            }
            (
                ratio(dw.saturating_mul(h), dh.saturating_mul(w)),
                ratio(dw, dh),
            )
        }
        None => (None, None),
    };
    e.push(("sample_aspect_ratio", sar.map_or(Value::Na, s)));
    e.push(("display_aspect_ratio", dar.map_or(Value::Na, s)));
    let color = signal(track, info);
    let full_range = color.full_range;
    let pix = match (info.mpeg2, info.mjpeg, info.mpeg4) {
        (Some(m), ..) => video::pix_fmt(m.chroma_format, 8, false),
        (_, Some(p), _) => p.pix_fmt,
        (.., Some(_)) => Some("yuv420p"),
        _ => pix_fmt(track, info, full_range),
    };
    e.push(("pix_fmt", pix.map_or(Value::Unset("unknown"), s)));
    let level = match (info.mpeg2.and_then(|m| m.profile_and_level), info.mpeg4) {
        (Some(pl), _) if pl & 0x80 == 0 => Some(pl & 0x0F),
        (Some(_), _) => None,
        (None, Some(m)) => m
            .profile_and_level
            .and_then(video::mpeg4_profile)
            .map(|(_, level)| level),
        (None, None) => level(track),
    };
    e.push(("level", Value::Int(level.map_or(-99, i64::from))));
    let range = match track.codec {
        Codec::Vp9 | Codec::Av1 | Codec::Hevc => Some(full_range.unwrap_or(false)),
        // The MPEG-1 and MPEG-2 decoders always say limited range.
        Codec::Mpeg2Video | Codec::Mpeg1Video => Some(false),
        Codec::Mjpeg if info.mjpeg.is_some() => Some(true),
        _ => full_range,
    };
    e.push((
        "color_range",
        range.map_or(Value::Unset("unknown"), |f| s(if f { "pc" } else { "tv" })),
    ));
    let matrix = match (color.matrix, info.mjpeg) {
        (None, Some(_)) => Some("bt470bg"),
        (m, _) => m.and_then(cnames::matrix),
    };
    e.push(("color_space", matrix.map_or(Value::Unset("unknown"), s)));
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
        chroma_location(track, info).map_or(Value::Unset("unspecified"), s),
    ));
    let order = field_order(&v).or(match track.codec {
        // The VP8 decoder and an H.264 decoder of frame-only pictures say
        // progressive.
        Codec::Vp8 => Some("progressive"),
        Codec::H264 => params.filter(|p| p.frame_mbs_only).map(|_| "progressive"),
        Codec::Mpeg2Video | Codec::Mpeg1Video => {
            info.mpeg2
                .and_then(|m| m.picture)
                .map(|(progressive, tff)| match (progressive, tff) {
                    (true, _) => "progressive",
                    (false, true) => "tt",
                    (false, false) => "bb",
                })
        }
        _ => None,
    });
    e.push(("field_order", order.map_or(Value::Unset("unknown"), s)));
    if decoder {
        e.push(("refs", Value::Int(1)));
    }
    if track.codec == Codec::H264 {
        let length = track
            .codec_private
            .as_deref()
            .filter(|p| p.first() == Some(&1))
            .and_then(|p| p.get(4))
            .map(|b| (b & 3) + 1);
        e.push(("is_avc", s(if length.is_some() { "true" } else { "false" })));
        e.push(("nal_length_size", s(length.unwrap_or(0).to_string())));
    }
    if let Some(m) = info.mpeg4 {
        let flag = |b: bool| s(if b { "true" } else { "false" });
        e.push((
            "quarter_sample",
            m.quarter_sample.map_or(Value::Unset("unknown"), flag),
        ));
        e.push(("divx_packed", flag(m.divx_packed)));
    }
    if let Some(m) = info.mpeg2 {
        side_data.push(SideData {
            typ: "cpb_properties",
            entries: vec![
                ("side_data_type", s("CPB properties")),
                ("max_bitrate", int(if m.variable { 0 } else { m.bit_rate })),
                ("min_bitrate", Value::Int(0)),
                ("avg_bitrate", Value::Int(0)),
                ("buffer_size", int(m.buffer_size)),
                ("vbv_delay", Value::Int(-1)),
            ],
        });
    }
    if v.stereo_mode_present {
        if let Some((typ, inverted)) = stereo(v.stereo_mode) {
            side_data.push(SideData {
                typ: "stereo_3d",
                entries: vec![
                    ("side_data_type", s("Stereo 3D")),
                    ("type", s(typ)),
                    ("inverted", Value::Int(i64::from(inverted))),
                    // Matroska states no view, primary eye, camera
                    // baseline, disparity or field of view: both views
                    // are packed in the frame and the rest is unknown.
                    ("view", s("packed")),
                    ("primary_eye", s("none")),
                    ("baseline", Value::Int(0)),
                    ("horizontal_disparity_adjustment", s("0/1")),
                    ("horizontal_field_of_view", s("0/1")),
                ],
            });
        }
    }
    if let Some((max_content, max_average)) = v.light_level {
        side_data.push(SideData {
            typ: "content_light_level_metadata",
            entries: vec![
                ("side_data_type", s("Content light level metadata")),
                ("max_content", int(max_content)),
                ("max_average", int(max_average)),
            ],
        });
    }
    if let Some(entries) = v.mastering.and_then(mastering_entries) {
        side_data.push(SideData {
            typ: "mastering_display_metadata",
            entries,
        });
    }
    if v.projection_present && v.projection_type == 1 {
        side_data.push(SideData {
            typ: "spherical_mapping",
            entries: vec![
                ("side_data_type", s("Spherical Mapping")),
                ("projection", s("equirectangular")),
                ("yaw", Value::Int(0)),
                ("pitch", Value::Int(0)),
                ("roll", Value::Int(0)),
            ],
        });
    }
    if let Some(clockwise) = v.rotation().filter(|r| *r != 0) {
        let ccw = (360 - i32::from(clockwise % 360)) % 360;
        let signed = if ccw >= 180 { ccw - 360 } else { ccw };
        side_data.push(SideData {
            typ: "display_matrix",
            entries: vec![
                ("side_data_type", s("Display Matrix")),
                ("displaymatrix", s(display_matrix(signed))),
                ("rotation", Value::Int(i64::from(signed))),
            ],
        });
    }
}

/// The mastering display side data of a `MasteringMetadata` element whose
/// every value is stored, each value printed as the fraction closest to it
/// with terms up to 2^31 - 1 ([`d2q`]).
fn mastering_entries(values: [Option<f64>; 10]) -> Option<Entries> {
    const KEYS: [&str; 10] = [
        "red_x",
        "red_y",
        "green_x",
        "green_y",
        "blue_x",
        "blue_y",
        "white_point_x",
        "white_point_y",
        "max_luminance",
        "min_luminance",
    ];
    let mut out: Entries = vec![("side_data_type", s("Mastering display metadata"))];
    let mut fractions = Vec::with_capacity(10);
    for value in values {
        let (num, den) = d2q(value?)?;
        fractions.push(format!("{num}/{den}"));
    }
    // Printed as red, green, blue, white point, then minimum and maximum.
    let order = [0, 1, 2, 3, 4, 5, 6, 7, 9, 8];
    for i in order {
        let (Some(key), Some(value)) = (KEYS.get(i), fractions.get(i)) else {
            return None;
        };
        out.push((key, s(value.as_str())));
    }
    Some(out)
}

/// The fraction closest to a non-negative `v` whose terms are at most
/// 2^31 - 1, found black box on crafted values: the exact binary value
/// of the float (so a value stored as a 4-byte float keeps its power of
/// two denominator), then its continued fraction's last convergent in
/// range or a closer semiconvergent; `1/0` past the range. `None` for a
/// negative or non-finite value, which CoreAV does not know how `ffprobe`
/// prints.
fn d2q(v: f64) -> Option<(u64, u64)> {
    const MAX: u128 = i32::MAX as u128;
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    if v > MAX as f64 {
        return Some((1, 0));
    }
    // v = m * 2^-shift exactly, the shift kept within 64 bits of
    // denominator (dropping bits below 2^-64).
    let bits = v.to_bits();
    let exp = i32::try_from((bits >> 52) & 0x7FF).ok()?;
    let frac = u128::from(bits & ((1 << 52) - 1));
    let (mut m, e) = if exp == 0 {
        (frac, -1074)
    } else {
        (frac | (1 << 52), exp - 1075)
    };
    if e >= 0 {
        let n = m.checked_shl(e.unsigned_abs())?;
        return (n <= MAX)
            .then(|| Some((u64::try_from(n).ok()?, 1)))
            .flatten();
    }
    let mut shift = e.unsigned_abs();
    if shift > 64 {
        m = m.checked_shr(shift - 64).unwrap_or(0);
        shift = 64;
    }
    let (num, den) = (m, 1u128 << shift);
    if num == 0 {
        return Some((0, 1));
    }
    // |p/q - num/den| compared exactly: |p*den - num*q| / q.
    let err = |p: u128, q: u128| -> Option<u128> {
        Some((p.checked_mul(den)?).abs_diff(num.checked_mul(q)?))
    };
    let closer = |(p, q): (u128, u128), (r, t): (u128, u128)| -> Option<bool> {
        Some(err(p, q)?.checked_mul(t)? < err(r, t)?.checked_mul(q)?)
    };
    let (mut p0, mut q0, mut p1, mut q1) = (0u128, 1u128, 1u128, 0u128);
    let (mut n, mut d) = (num, den);
    let best = loop {
        if d == 0 {
            break (p1, q1);
        }
        let a = n / d;
        let p2 = a.checked_mul(p1)?.checked_add(p0)?;
        let q2 = a.checked_mul(q1)?.checked_add(q0)?;
        if p2 > MAX || q2 > MAX {
            let mut k = a;
            if let Some(n) = (MAX - p0).checked_div(p1) {
                k = k.min(n);
            }
            if let Some(n) = (MAX - q0).checked_div(q1) {
                k = k.min(n);
            }
            let semi = (k * p1 + p0, k * q1 + q0);
            if k > 0 && q1 > 0 && closer(semi, (p1, q1))? {
                break semi;
            }
            break (p1, q1);
        }
        (p0, q0, p1, q1) = (p1, q1, p2, q2);
        (n, d) = (d, n - a * d);
    };
    Some((u64::try_from(best.0).ok()?, u64::try_from(best.1).ok()?))
}

/// The name of a `StereoMode` (RFC 9559 5.1.4.1.28.3) in the
/// `stereo_mode` tag; none for a single view.
fn stereo_name(mode: u64) -> Option<&'static str> {
    Some(match mode {
        1 => "left_right",
        2 => "bottom_top",
        3 => "top_bottom",
        4 => "checkerboard_rl",
        5 => "checkerboard_lr",
        6 => "row_interleaved_rl",
        7 => "row_interleaved_lr",
        8 => "col_interleaved_rl",
        9 => "col_interleaved_lr",
        10 => "anaglyph_cyan_red",
        11 => "right_left",
        12 => "anaglyph_green_magenta",
        13 => "block_lr",
        14 => "block_rl",
        _ => return None,
    })
}

/// The packing a `StereoMode` states (RFC 9559 5.1.4.1.28.3), as
/// `ffprobe` names it, and whether the right eye comes first; none for
/// the anaglyph modes, which pack no separate views.
fn stereo(mode: u64) -> Option<(&'static str, bool)> {
    Some(match mode {
        0 => ("2D", false),
        1 => ("side by side", false),
        2 => ("top and bottom", true),
        3 => ("top and bottom", false),
        4 => ("checkerboard", true),
        5 => ("checkerboard", false),
        6 => ("interleaved lines", true),
        7 => ("interleaved lines", false),
        8 => ("interleaved columns", true),
        9 => ("interleaved columns", false),
        11 => ("side by side", true),
        13 => ("frame alternate", false),
        14 => ("frame alternate", true),
        _ => return None,
    })
}

/// The display matrix of a counter-clockwise rotation by a quarter turn,
/// as `ffprobe` prints one: three rows of three right-aligned numbers,
/// each row headed by its index.
fn display_matrix(ccw: i32) -> String {
    let (sin, cos) = match ccw.rem_euclid(360) {
        90 => (1, 0),
        180 => (0, -1),
        270 => (-1, 0),
        _ => (0, 1),
    };
    let m = [
        cos * 65536,
        -sin * 65536,
        0,
        sin * 65536,
        cos * 65536,
        0,
        0,
        0,
        1 << 30,
    ];
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

fn level(track: &Track) -> Option<u8> {
    match track.profile {
        Some(Profile::Vp9 { .. }) | None => None,
        Some(p) => cnames::level(p),
    }
}

/// The colour description a stream reports, by codec, found black box:
/// the H.264 decoder's sequence parameter set values where it states them
/// and the `Colour` element's otherwise; the H.265 and AV1 decoders' own
/// values only; the VP9 decoder's matrix and range with the element's
/// primaries and transfer; the element's for the others.
fn signal(track: &Track, info: &Info) -> coreav_isobmff::Color {
    let container = track.colour;
    let code = |v: u32, known: &[u32]| known.contains(&v).then(|| u16::try_from(v).ok()).flatten();
    let primaries = [1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 22];
    let transfers = [1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18];
    let matrices = [0, 1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14];
    match track.codec {
        Codec::H264 => {
            let p = info.params.unwrap_or_default();
            let mut out = container;
            if let Some(full) = p.full_range {
                out.full_range = Some(full);
            }
            if let Some((pri, trc, mat)) = p.colour {
                out.primaries = code(pri, &primaries);
                out.transfer = code(trc, &transfers);
                out.matrix = code(mat, &matrices);
            }
            out
        }
        Codec::Hevc | Codec::Av1 => {
            let kind = if track.codec == Codec::Hevc {
                *b"hvcC"
            } else {
                *b"av1C"
            };
            track
                .codec_private
                .as_deref()
                .filter(|p| p.len() > 4)
                .and_then(|p| {
                    coreav_isobmff::describe_config_record(coreav_isobmff::FourCc(kind), p)
                })
                .filter(|d| d.parameter_set)
                .map_or(track.color, |d| d.color)
        }
        Codec::Vp9 => {
            let Some((space, full)) = video::vp9_colour(&info.first) else {
                return track.color;
            };
            coreav_isobmff::Color {
                matrix: match space {
                    1 => Some(5),
                    2 => Some(1),
                    3 => Some(6),
                    4 => Some(7),
                    5 => Some(9),
                    7 => Some(0),
                    _ => None,
                },
                full_range: Some(full),
                ..container
            }
        }
        _ => container,
    }
}

fn pix_fmt(track: &Track, info: &Info, full_range: Option<bool>) -> Option<&'static str> {
    let params = info.params;
    let matrix = signal(track, info).matrix;
    // 4:4:4 with the identity matrix is planar GBR.
    let gbr = |p: &video::Params| p.chroma_format == 3 && matrix == Some(0);
    match track.codec {
        Codec::Vp9 if track.chroma_subsampling == Some((0, 0)) && matrix == Some(0) => {
            match track.profile {
                Some(Profile::Vp9 { bit_depth, .. }) if bit_depth > 8 => match bit_depth {
                    10 => Some("gbrp10le"),
                    12 => Some("gbrp12le"),
                    _ => None,
                },
                _ => Some("gbrp"),
            }
        }
        Codec::Theora => {
            // The pixel format field of the identification header.
            let header = audio::xiph_headers(track.codec_private.as_deref()?)?;
            let id = header.first()?;
            if id.get(..7)? != b"\x80theora" {
                return None;
            }
            match (id.get(41)? >> 3) & 3 {
                0 => Some("yuv420p"),
                2 => Some("yuv422p"),
                3 => Some("yuv444p"),
                _ => None,
            }
        }
        Codec::H264 | Codec::Hevc | Codec::Av1 if params.as_ref().is_some_and(gbr) => {
            let p = params?;
            match p.bit_depth {
                8 => Some("gbrp"),
                10 => Some("gbrp10le"),
                12 => Some("gbrp12le"),
                _ => None,
            }
        }
        Codec::H264 => {
            let p = params?;
            video::pix_fmt(p.chroma_format, p.bit_depth, full_range == Some(true))
        }
        Codec::Hevc | Codec::Av1 => {
            let p = params?;
            video::pix_fmt(p.chroma_format, p.bit_depth, false)
        }
        Codec::ProRes => {
            let fourcc = prores_fourcc(track)?;
            if matches!(&fourcc, b"ap4h" | b"ap4x") {
                // alpha_info, the low bits of byte 17 of the frame header.
                let alpha = info.first.get(17).is_some_and(|b| b & 0x0F != 0);
                Some(if alpha { "yuva444p12le" } else { "yuv444p12le" })
            } else {
                Some("yuv422p10le")
            }
        }
        Codec::Vp8 => Some("yuv420p"),
        Codec::Vp9 => {
            let depth = match track.profile {
                Some(Profile::Vp9 { bit_depth, .. }) if bit_depth > 8 => u32::from(bit_depth),
                _ => 8,
            };
            let chroma = match track.chroma_subsampling {
                Some((1, 1)) | None => 1,
                Some((1, 0)) => 2,
                Some((0, 0)) => 3,
                _ => return None,
            };
            video::pix_fmt(chroma, depth, false)
        }
        _ => None,
    }
}

fn chroma_location(track: &Track, info: &Info) -> Option<&'static str> {
    let params = info.params;
    if info.mjpeg.is_some() {
        return Some("center");
    }
    let v = track.video?;
    match v.chroma_siting {
        (Some(1), Some(1)) => return Some("topleft"),
        (Some(2), Some(1)) => return Some("top"),
        (Some(1), Some(2)) => return Some("left"),
        (Some(2), Some(2)) => return Some("center"),
        _ => {}
    }
    match track.codec {
        Codec::H264 => {
            let p = params?;
            match p.chroma_loc {
                Some(loc) => video::chroma_location(loc),
                None => Some("left"),
            }
        }
        // 4:2:0 only: the VUI's location of the first SPS, else left.
        Codec::Hevc => {
            let p = track.codec_private.as_deref()?;
            let nals = if p.first() == Some(&1) {
                video::hvcc_nals(p)
            } else {
                video::annexb_nals(p)
            };
            nals.into_iter()
                .filter(|n| n.first().is_some_and(|b| (b >> 1) & 0x3F == 33))
                .find_map(|nal| super::video::hevc_sps(nal, false))
                .and_then(|sps| sps.chroma_location)
        }
        // MPEG-1 sites chroma in the middle, MPEG-2 to the left.
        Codec::Mpeg2Video | Codec::Mpeg1Video => {
            info.mpeg2.filter(|m| m.chroma_format == 1).map(|m| {
                if m.profile_and_level.is_none() {
                    "center"
                } else {
                    "left"
                }
            })
        }
        Codec::Mpeg4 => info.mpeg4.map(|_| "left"),
        Codec::Theora => Some("center"),
        _ => None,
    }
}

fn field_order(v: &coreav_mkv::Video) -> Option<&'static str> {
    match v.interlaced {
        2 => Some("progressive"),
        1 => match v.field_order {
            1 => Some("tt"),
            6 => Some("bb"),
            9 => Some("tb"),
            14 => Some("bt"),
            _ => None,
        },
        _ => None,
    }
}

fn audio_entries(e: &mut Entries, track: &Track, info: &Info, name: Option<&str>) {
    let a = track.audio.unwrap_or_default();
    let bits = match track.codec {
        Codec::Flac => info.flac_bits,
        Codec::Alac => alac_bits(track),
        _ => a.bit_depth.and_then(|b| u32::try_from(b).ok()),
    }
    .and_then(|b| u16::try_from(b).ok());
    let sample_fmt = match track.codec {
        // The planar decoders.
        Codec::Mp2 => Some("s16p"),
        Codec::Alac => match bits {
            Some(16) => Some("s16p"),
            Some(20 | 24 | 32) => Some("s32p"),
            _ => None,
        },
        _ => name.and_then(|n| names::sample_fmt(n, bits)),
    };
    e.push(("sample_fmt", sample_fmt.map_or(Value::Unset("unknown"), s)));
    e.push((
        "sample_rate",
        a.output_sample_rate()
            .map_or(Value::Na, |r| Value::Unit(i64::from(r), Unit::Hertz)),
    ));
    e.push(("channels", int(a.channels)));
    let layout = channel_layout(track, info, a.channels);
    e.push(("channel_layout", layout.map_or(Value::Unset("unknown"), s)));
    let bits_per_sample = match track.codec {
        Codec::Pcm => a.bit_depth.unwrap_or(0),
        _ => 0,
    };
    e.push(("bits_per_sample", int(bits_per_sample)));
    let padding = info
        .rate
        .map(|r| (u128::from(track.codec_delay) * u128::from(r) + 500_000_000) / 1_000_000_000)
        .unwrap_or(0);
    e.push(("initial_padding", int(padding)));
}

/// The channel layout a decoder reports, by codec, found black box: what
/// the AC-3 header or the AAC configuration states, the Vorbis channel
/// orders for Vorbis, Opus (mapping family 1) and FLAC, and none for PCM,
/// whose decoders leave the order unspecified.
fn channel_layout(track: &Track, info: &Info, channels: u64) -> Option<&'static str> {
    let vorbis_order = |n: u64| -> Option<&'static str> {
        Some(match n {
            1 => "mono",
            2 => "stereo",
            3 => "3.0",
            4 => "quad",
            5 => "5.0",
            6 => "5.1",
            7 => "6.1",
            8 => "7.1",
            _ => return None,
        })
    };
    match track.codec {
        Codec::Pcm | Codec::Other => None,
        Codec::Ac3 | Codec::Eac3 => audio::ac3_layout(&info.first),
        Codec::Aac => match track.codec_private.as_deref() {
            // channelConfiguration: 4 bits after the object type (5) and
            // the sampling frequency index (4).
            Some(asc) if asc.len() >= 2 => {
                let config = (asc.get(1).copied().unwrap_or(0) >> 3) & 0x0F;
                names::aac_layout(config).filter(|_| config <= 6)
            }
            _ => u32::try_from(channels).ok().and_then(names::default_layout),
        },
        Codec::Opus => {
            let family = track
                .codec_private
                .as_deref()
                .and_then(|h| h.get(18))
                .copied();
            match family {
                Some(0) | None if channels <= 2 => vorbis_order(channels),
                Some(1) => vorbis_order(channels),
                _ => None,
            }
        }
        Codec::Vorbis | Codec::Flac => vorbis_order(channels),
        _ => u32::try_from(channels).ok().and_then(names::default_layout),
    }
}

/// `bitDepth` of an `ALACSpecificConfig` (the Apple Lossless format: 4
/// bytes of frame length and one of compatible version precede it).
fn alac_bits(track: &Track) -> Option<u32> {
    let config = track.codec_private.as_deref()?;
    config.get(5).map(|b| u32::from(*b))
}

/// The four-character code a `V_PRORES` `CodecPrivate` holds.
fn prores_fourcc(track: &Track) -> Option<[u8; 4]> {
    let private = track.codec_private.as_deref()?;
    <[u8; 4]>::try_from(private.get(..4)?).ok()
}

/// The ProRes profile a four-character code names.
fn prores_profile(fourcc: &[u8; 4]) -> Option<&'static str> {
    Some(match fourcc {
        b"apco" => "Proxy",
        b"apcs" => "LT",
        b"apcn" => "Standard",
        b"apch" => "HQ",
        b"ap4h" => "4444",
        b"ap4x" => "XQ",
        _ => return None,
    })
}

/// The size of the configuration a decoder gets: the codec private data,
/// or for H.264 without it, the parameter sets of the first frame.
fn extradata_size(track: &Track, info: &Info) -> Option<usize> {
    match track.codec {
        Codec::Vp9 | Codec::Vp8 | Codec::Ac3 | Codec::Eac3 | Codec::Mp3 | Codec::Mp2 => None,
        Codec::H264 if track.codec_private.is_none() => info.in_band,
        Codec::H264
            if track
                .codec_private
                .as_ref()
                .is_some_and(|c| c.first() != Some(&1)) =>
        {
            track
                .codec_private
                .as_deref()
                .and_then(|c| super::ts::params::annex_b_extradata(c, false))
        }
        Codec::Flac => track
            .codec_private
            .as_deref()
            .and_then(flac_streaminfo)
            .map(|(_, n)| n),
        // The decoder takes the configuration in its `alac` atom.
        Codec::Alac => track.codec_private.as_ref().map(|p| p.len() + 12),
        Codec::Mjpeg => None,
        Codec::Mpeg2Video | Codec::Mpeg1Video if track.codec_private.is_none() => {
            info.mpeg2.map(|m| m.header_len)
        }
        // Without a configuration, the parser takes the first frame's
        // headers before its first VOP.
        Codec::Mpeg4 if codec_config(track).is_none_or(<[u8]>::is_empty) => info
            .first
            .windows(4)
            .position(|w| w == [0, 0, 1, 0xB6])
            .filter(|n| *n > 0),
        _ if vfw(track) => codec_config(track).map(<[u8]>::len).filter(|n| *n > 0),
        _ => track
            .codec_private
            .as_ref()
            .map(Vec::len)
            .filter(|n| *n > 0),
    }
}

/// The disposition flags, in `ffprobe`'s order.
fn disposition(track: &Track) -> Entries {
    [
        ("default", track.default),
        ("dub", false),
        ("original", track.original),
        ("comment", track.commentary),
        ("lyrics", false),
        ("karaoke", false),
        ("forced", track.forced),
        ("hearing_impaired", track.hearing_impaired),
        ("visual_impaired", track.visual_impaired),
        ("clean_effects", false),
        ("attached_pic", false),
        ("timed_thumbnails", false),
        ("non_diegetic", false),
        ("captions", false),
        // `FlagTextDescriptions` is not shown, black box.
        ("descriptions", false),
        ("metadata", false),
        ("dependent", false),
        ("still_image", false),
        ("multilayer", false),
    ]
    .into_iter()
    .map(|(k, v)| (k, Value::Int(i64::from(v))))
    .collect()
}

fn empty_disposition() -> Entries {
    disposition_flags(&[])
}

fn disposition_flags(set: &[&str]) -> Entries {
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
        "multilayer",
    ]
    .into_iter()
    .map(|k| (k, Value::Int(i64::from(set.contains(&k)))))
    .collect()
}

/// The codec `ffprobe` names an attached file by, from its media type.
/// The media types are matched by prefix, black box: `font/ttf` and
/// `font/otf` name no codec, `application/x-font-opentype` names TrueType.
fn attachment_codec(media_type: &str) -> Option<&'static str> {
    const TYPES: [(&str, &str); 6] = [
        ("image/jpeg", "mjpeg"),
        ("image/png", "png"),
        ("image/gif", "gif"),
        ("application/x-truetype-font", "ttf"),
        ("application/x-font", "ttf"),
        ("application/vnd.ms-opentype", "otf"),
    ];
    TYPES
        .iter()
        .find(|(prefix, _)| media_type.starts_with(prefix))
        .map(|(_, codec)| *codec)
}

fn attachment_stream(
    meta: &Meta<'_>,
    attachment: &coreav_mkv::Attachment,
    picture: Option<image::Picture>,
    index: usize,
    start_us: Option<i64>,
    duration_us: Option<i64>,
) -> Stream {
    let codec = attachment_codec(&attachment.media_type)
        .filter(|c| !matches!(*c, "mjpeg" | "png" | "gif") || picture.is_some());
    let ticks = |us: i64| from_micros(us, (1, ATTACHMENT_TIME_BASE));
    let start = start_us.map(ticks);
    let duration = duration_us.map(ticks);
    let mut tags = meta.dict();
    tags.set("filename", &attachment.name);
    tags.set("mimetype", &attachment.media_type);
    if let Some(description) = &attachment.description {
        tags.set("title", description);
    }
    if let Some(uid) = attachment.uid {
        meta.add(&mut tags, &meta.by_attachment, uid);
    }
    let tags = tags.into_entries();
    if let Some(picture) = picture {
        return picture_stream(&picture, index, start, duration, tags);
    }
    let mut e: Entries = vec![("index", int(index))];
    match codec {
        Some(c) => {
            e.push(("codec_name", s(c)));
            e.push((
                "codec_long_name",
                long_name(c).map_or(Value::Unset("unknown"), s),
            ));
        }
        None => {
            e.push(("codec_name", Value::Unset("unknown")));
            e.push(("codec_long_name", Value::Unset("unknown")));
        }
    }
    e.extend([
        ("profile", Value::Unset("unknown")),
        ("codec_type", s("attachment")),
        ("codec_tag_string", s("[0][0][0][0]")),
        ("codec_tag", s("0x0000")),
        ("id", Value::Na),
        ("r_frame_rate", s("0/0")),
        ("avg_frame_rate", s("0/0")),
        ("time_base", s(format!("1/{ATTACHMENT_TIME_BASE}"))),
        ("start_pts", start.map_or(Value::Na, Value::Int)),
        (
            "start_time",
            start.map_or(Value::Na, |v| time(v, 1, ATTACHMENT_TIME_BASE)),
        ),
        ("duration_ts", duration.map_or(Value::Na, Value::Int)),
        (
            "duration",
            duration.map_or(Value::Na, |v| time(v, 1, ATTACHMENT_TIME_BASE)),
        ),
        ("bit_rate", Value::Na),
        ("max_bit_rate", Value::Na),
        ("bits_per_raw_sample", Value::Na),
        ("nb_frames", Value::Na),
        ("nb_read_frames", Value::Na),
        ("nb_read_packets", Value::Na),
    ]);
    if attachment.size > 0 {
        e.push(("extradata_size", int(attachment.size)));
    }
    Stream {
        kind: StreamKind::Attachment,
        attached_pic: false,
        entries: e,
        disposition: empty_disposition(),
        tags,
        side_data: Vec::new(),
    }
}

/// An attached picture, which `ffprobe` shows as a video stream.
pub(super) fn picture_stream(
    picture: &image::Picture,
    index: usize,
    start: Option<i64>,
    duration: Option<i64>,
    tags: Vec<(String, String)>,
) -> Stream {
    let jpeg = picture.codec == "mjpeg";
    let (w, h) = (u64::from(picture.width), u64::from(picture.height));
    let mut e: Entries = vec![
        ("index", int(index)),
        ("codec_name", s(picture.codec)),
        (
            "codec_long_name",
            long_name(picture.codec).map_or(Value::Unset("unknown"), s),
        ),
        (
            "profile",
            picture.profile.map_or(Value::Unset("unknown"), s),
        ),
        ("codec_type", s("video")),
        ("codec_tag_string", s("[0][0][0][0]")),
        ("codec_tag", s("0x0000")),
        ("width", int(w)),
        ("height", int(h)),
        ("coded_width", int(w)),
        ("coded_height", int(h)),
        ("closed_captions", Value::Int(0)),
        ("film_grain", Value::Int(0)),
        ("has_b_frames", Value::Int(0)),
        (
            "sample_aspect_ratio",
            picture
                .aspect
                .and_then(|(x, y)| ratio(u64::from(x), u64::from(y)))
                .map_or(Value::Na, s),
        ),
        (
            "display_aspect_ratio",
            picture
                .aspect
                .and_then(|(x, y)| ratio(w * u64::from(x), h * u64::from(y)))
                .map_or(Value::Na, s),
        ),
        (
            "pix_fmt",
            picture.pix_fmt.map_or(Value::Unset("unknown"), s),
        ),
        ("level", Value::Int(-99)),
        ("color_range", s("pc")),
        ("color_space", s(if jpeg { "bt470bg" } else { "gbr" })),
        (
            "color_transfer",
            if picture.srgb {
                s("iec61966-2-1")
            } else {
                Value::Unset("unknown")
            },
        ),
        (
            "color_primaries",
            if picture.srgb {
                s("bt709")
            } else {
                Value::Unset("unknown")
            },
        ),
        (
            "chroma_location",
            if jpeg {
                s("center")
            } else {
                Value::Unset("unspecified")
            },
        ),
        ("field_order", Value::Unset("unknown")),
        ("refs", Value::Int(1)),
        ("id", Value::Na),
        ("r_frame_rate", s(format!("{ATTACHMENT_TIME_BASE}/1"))),
        ("avg_frame_rate", s("0/0")),
        ("time_base", s(format!("1/{ATTACHMENT_TIME_BASE}"))),
        ("start_pts", start.map_or(Value::Na, Value::Int)),
        (
            "start_time",
            start.map_or(Value::Na, |v| time(v, 1, ATTACHMENT_TIME_BASE)),
        ),
        ("duration_ts", duration.map_or(Value::Na, Value::Int)),
        (
            "duration",
            duration.map_or(Value::Na, |v| time(v, 1, ATTACHMENT_TIME_BASE)),
        ),
        ("bit_rate", Value::Na),
        ("max_bit_rate", Value::Na),
        (
            "bits_per_raw_sample",
            picture.bits.map_or(Value::Na, |b| s(b.to_string())),
        ),
        ("nb_frames", Value::Na),
        ("nb_read_frames", Value::Na),
        ("nb_read_packets", Value::Na),
    ];
    e.shrink_to_fit();
    Stream {
        kind: StreamKind::Video,
        attached_pic: true,
        entries: e,
        disposition: disposition_flags(&["attached_pic"]),
        tags,
        side_data: Vec::new(),
    }
}

/// The packets of every track in storage order.
struct MkvPackets<S> {
    segment: Segment,
    source: S,
    infos: Vec<Info>,
}

/// Per track state of the listing.
#[derive(Clone, Debug)]
struct State {
    /// The presentation times waiting to become decode times, smallest
    /// first; `None` is a time not yet known.
    buffer: Vec<Option<i64>>,
    /// The end of the last packet, for a later frame of a lace.
    next: Option<i64>,
    /// The previous Vorbis block size, and the samples of the previous
    /// Vorbis packet.
    previous_block: Option<u32>,
    previous_samples: Option<u64>,
    /// The presentation time of the first frame of the current block, and
    /// the samples of the block's frames so far.
    lace_start: Option<i64>,
    lace_samples: Option<u64>,
    /// Packets listed so far.
    count: usize,
}

impl<S: Source> PacketSource for MkvPackets<S> {
    fn visit(
        &mut self,
        payloads: &[bool],
        f: &mut dyn FnMut(Packet) -> Result<bool, Failure>,
    ) -> Result<(), Failure> {
        let limits = Limits::default();
        let segment = &self.segment;
        let tb = time_base(segment);
        let mut states: Vec<State> = self
            .infos
            .iter()
            .map(|info| State {
                buffer: vec![None; info.delay.map_or(1, |d| d as usize + 1)],
                next: None,
                previous_block: None,
                previous_samples: None,
                lace_start: None,
                lace_samples: None,
                count: 0,
            })
            .collect();
        let mut iter = segment
            .packets(&mut self.source, &limits)
            .with_inflated_sizes();
        while let Some(item) = iter.next() {
            let p = match item {
                Ok(p) => p,
                Err(coreav_mkv::Error::Io(kind)) => {
                    return Err(Failure::Matroska(coreav_mkv::Error::Io(kind)));
                }
                Err(_) => return Ok(()),
            };
            let (Some(track), Some(info), Some(state)) = (
                segment.tracks.get(p.track),
                self.infos.get(p.track),
                states.get_mut(p.track),
            ) else {
                continue;
            };
            // A WebVTT block's stored bytes; the head of others.
            let stored = if info.webvtt {
                stored_frame(track, &p, iter.source_mut())
            } else {
                None
            };
            let head = if info.webvtt {
                Vec::new()
            } else if track.codec == Codec::TrueHd {
                frame_head(track, &p, iter.source_mut())
            } else if !info.parser.reads_frame() {
                Vec::new()
            } else {
                frame_head(track, &p, iter.source_mut())
            };
            let size = if info.webvtt {
                stored
                    .as_deref()
                    .and_then(|frame| info.webvtt_stripped.payload(frame))
                    .map_or(p.size(), |n| n as u64)
            } else if track.codec == Codec::ProRes {
                // The demuxer puts back the frame's 8-byte `icpf` header.
                p.size().saturating_add(8)
            } else {
                p.size()
            };
            let mut samples = info.samples(&head, size);
            if let Parser::Vorbis(v) = &info.parser {
                if let Some(block) = v.blocksize(&head) {
                    let previous = state.previous_block.unwrap_or(block);
                    samples = Some(u64::from(previous / 4 + block / 4));
                    state.previous_block = Some(block);
                    state.previous_samples = samples;
                } else {
                    // A packet that is not an audio packet lasts what the
                    // previous one did, and leaves the block size as it
                    // was (found black box).
                    samples = state.previous_samples;
                }
            }
            let parsed = samples.zip(info.rate).and_then(|(n, rate)| {
                let ticks = u128::from(n) * u128::try_from(tb.1).ok()?
                    / (u128::from(rate) * u128::try_from(tb.0).ok()?).max(1);
                i64::try_from(ticks).ok()
            });
            // A block lasts its frames' `DefaultDuration`s, truncated to
            // the tick; each frame of a lace gets its share of that, from
            // its start to the next frame's, each truncated.
            let scale = u128::from(segment.timestamp_scale.max(1));
            let frames = u128::from(p.frames_in_block.max(1));
            let lace = u128::from(p.lace);
            let block_ticks = info
                .default_ns
                .and_then(|dd| u128::from(dd).checked_mul(frames).map(|n| n / scale));
            let share = |i: u128| block_ticks.and_then(|b| b.checked_mul(i).map(|n| n / frames));
            // Each frame lasts its `DefaultDuration` truncated to the
            // tick; one shorter than a tick is no duration. An audio frame
            // of a lace lasts from its start to the next frame's.
            let lace_duration = if info.kind == TrackKind::Audio && frames > 1 {
                share(lace + 1)
                    .zip(share(lace))
                    .map(|(end, start)| end - start)
            } else {
                info.default_ns.map(|dd| u128::from(dd) / scale)
            }
            .filter(|t| *t > 0)
            .and_then(|t| i64::try_from(t).ok());
            let block_duration = p.block_duration.and_then(|d| i64::try_from(d).ok());
            // An audio packet lasts what its frame header says, else what
            // the track or its codec says (found black box: a
            // `DefaultDuration` that disagrees with the frames is ignored);
            // a video packet what its block or the track says, or after the
            // rate analysis, a frame of the estimated rate.
            let duration = match info.kind {
                TrackKind::Audio if info.parser.reads_frame() => {
                    parsed.or(lace_duration).or(block_duration)
                }
                TrackKind::Audio => lace_duration.or(parsed).or(block_duration),
                TrackKind::Video => block_duration
                    .or(lace_duration)
                    .or(info.rates.vui_ticks)
                    .or(info
                        .rates
                        .estimated_ticks
                        .filter(|_| state.count >= info.rates.window)),
                _ => block_duration.or(lace_duration),
            };
            state.count += 1;
            if p.lace == 0 {
                state.lace_start = p.pts;
                state.lace_samples = Some(0);
            }
            let pts = if p.lace == 0 {
                p.pts
            } else if let Some(offset) = share(lace) {
                state
                    .lace_start
                    .zip(i64::try_from(offset).ok())
                    .and_then(|(t, o)| t.checked_add(o))
            } else {
                match (state.lace_start, state.lace_samples, info.rate) {
                    (Some(start), Some(n), Some(rate)) => {
                        rounded_ticks(n, rate, tb).and_then(|t| start.checked_add(t))
                    }
                    _ => state.next,
                }
            };
            state.lace_samples = state
                .lace_samples
                .zip(samples)
                .map(|(a, b)| a.saturating_add(b));
            state.next = pts.zip(duration).and_then(|(t, d)| t.checked_add(d));
            let (pts, dts) = if vfw_reordered(track, info) {
                (None, pts)
            } else if info.dts_is_pts || info.kind != TrackKind::Video {
                (pts, pts)
            } else if info.delay.is_some() {
                let first = state.count == 1;
                let mpeg1 = info.mpeg2.is_some_and(|m| m.profile_and_level.is_none());
                let dts = match reorder(&mut state.buffer, pts) {
                    // MPEG-1 video starts reordering at its third picture.
                    _ if mpeg1 && state.count <= 2 => pts,
                    // The MPEG-2 and MPEG-4 video parsers start the decode
                    // times a frame before the first picture.
                    None if first && (info.mpeg2.is_some() || info.mpeg4.is_some()) => {
                        pts.zip(duration).and_then(|(t, d)| t.checked_sub(d))
                    }
                    other => other,
                };
                (pts, dts)
            } else {
                (pts, None)
            };
            let mut side_data = Vec::new();
            if let (Some(padding), Some(rate)) = (p.discard_padding, info.rate) {
                let samples = (i128::from(padding) * i128::from(rate) + 500_000_000)
                    .div_euclid(1_000_000_000);
                side_data.push(SideData {
                    typ: "skip_samples",
                    entries: vec![
                        ("side_data_type", s("Skip Samples")),
                        ("skip_samples", Value::Int(0)),
                        ("discard_padding", int(samples)),
                        ("skip_reason", Value::Int(0)),
                        ("discard_reason", Value::Int(0)),
                    ],
                });
            }
            if p.codec_addition {
                side_data.push(SideData {
                    typ: "matroska_blockadditional",
                    entries: vec![
                        ("side_data_type", s("Matroska BlockAdditional")),
                        ("block_additional_id", Value::Int(1)),
                    ],
                });
            }
            let Some(stream) = info.stream else {
                continue;
            };
            let packet = Packet {
                stream,
                pts,
                dts,
                duration,
                size,
                pos: Some(p.block_offset),
                keyframe: match (info.kind, track.codec) {
                    // A TrueHD access unit with a major sync.
                    (_, Codec::TrueHd) => head.get(4..8) == Some(&[0xF8, 0x72, 0x6F, 0xBA]),
                    (TrackKind::Video, _) => p.keyframe,
                    _ => true,
                },
                discard: false,
                corrupt: false,
                side_data,
                data: if payloads.get(stream).copied().unwrap_or(false) {
                    let mut data = match &track.encoding {
                        Encoding::HeaderStripping(prefix) => prefix.clone(),
                        _ => Vec::new(),
                    };
                    data.extend(super::frames::read(
                        iter.source_mut(),
                        p.offset,
                        p.stored_size,
                    )?);
                    data
                } else {
                    Vec::new()
                },
            };
            if !f(packet)? {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// `samples` at `rate` in ticks of `tb`, rounded to the nearest.
fn rounded_ticks(samples: u64, rate: u32, (num, den): (i64, i64)) -> Option<i64> {
    let n = u128::from(samples) * u128::try_from(den).ok()?;
    let d = u128::from(rate) * u128::try_from(num).ok()?;
    if d == 0 {
        return None;
    }
    let q = n.checked_mul(2)?.checked_add(d)? / d.checked_mul(2)?;
    i64::try_from(q).ok()
}

/// What a track's stripped header (`ContentCompression` algorithm 3)
/// holds of a WebVTT block's two header lines, so that each block is
/// measured without joining the header to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StrippedLines {
    /// No line ends in it (or there is none).
    #[default]
    None,
    /// The first line ends in it.
    One,
    /// Both lines end in it, the given number of bytes before its end.
    Both(usize),
}

impl StrippedLines {
    fn of(track: &Track) -> Self {
        let Encoding::HeaderStripping(prefix) = &track.encoding else {
            return Self::None;
        };
        let mut ends = prefix
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i);
        match (ends.next(), ends.next()) {
            (Some(_), Some(second)) => Self::Both(prefix.len().saturating_sub(second + 1)),
            (Some(_), None) => Self::One,
            _ => Self::None,
        }
    }

    /// The payload's size of a block whose stored bytes are `frame`.
    fn payload(self, frame: &[u8]) -> Option<usize> {
        match self {
            Self::Both(after) => after.checked_add(frame.len()),
            Self::One => {
                let end = frame.iter().position(|&b| b == b'\n')?;
                frame.len().checked_sub(end + 1)
            }
            Self::None => webvtt_payload(frame),
        }
    }
}

/// The payload of a WebM WebVTT block (`D_WEBVTT/...`): what follows the
/// cue identifier line and the cue settings line.
fn webvtt_payload(frame: &[u8]) -> Option<usize> {
    let first = frame.iter().position(|&b| b == b'\n')?;
    let rest = frame.get(first + 1..)?;
    let second = rest.iter().position(|&b| b == b'\n')?;
    Some(rest.len() - second - 1)
}

/// Decode times rebuilt from presentation times: the time enters a buffer
/// of the reorder depth plus one, kept sorted, and the smallest comes out.
fn reorder(buffer: &mut [Option<i64>], pts: Option<i64>) -> Option<i64> {
    let first = buffer.first_mut()?;
    *first = pts;
    for i in 0..buffer.len().saturating_sub(1) {
        let (Some(a), Some(b)) = (buffer.get(i).copied(), buffer.get(i + 1).copied()) else {
            break;
        };
        // `None` sorts first.
        if a > b {
            buffer.swap(i, i + 1);
        } else {
            break;
        }
    }
    buffer.first().copied().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_times_from_presentation_times() {
        let mut buffer = vec![None; 3];
        let out: Vec<Option<i64>> = [0, 160, 80, 40, 120]
            .into_iter()
            .map(|pts| reorder(&mut buffer, Some(pts)))
            .collect();
        assert_eq!(out, vec![None, None, Some(0), Some(40), Some(80)]);
    }

    #[test]
    fn frame_rates_from_default_durations() {
        for (dd, rate) in [
            (40_000_000, "25/1"),
            (33_366_666, "30000/1001"),
            (16_683_333, "19001/317"),
            (33_284_516, "7511/250"),
            (41_715_740, "26321/1098"),
            (33_365_554, "27933/932"),
            (41_000_407, "2439/100"),
            (41_708_333, "24000/1001"),
            (33_000_000, "1000/33"),
            (33_426_183, "359/12"),
            (1_000_000, "1000/1"),
        ] {
            assert_eq!(frame_rate(dd).as_deref(), Some(rate), "{dd}");
        }
    }

    #[test]
    fn metadata_replaces_by_moving_the_last_entry() {
        let budget = Cell::new(METADATA_BYTES);
        let mut dict = Dict::new(&budget);
        for (k, v) in [
            ("encoder", "a"),
            ("creation_time", "b"),
            ("X", "c"),
            ("ENCODER", "d"),
        ] {
            dict.set(k, v);
        }
        let entries = dict.into_entries();
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["X", "creation_time", "ENCODER"]);
    }

    #[test]
    fn dates() {
        assert_eq!(
            date_string(0).as_deref(),
            Some("2001-01-01T00:00:00.000000Z")
        );
        assert_eq!(
            date_string(800_230_932_000_000_000).as_deref(),
            Some("2026-05-11T22:22:12.000000Z")
        );
    }

    #[test]
    fn frame_rates_from_timestamps() {
        let ms = |fps: f64, n: usize, start: i64| -> Vec<i64> {
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            (0..n)
                .map(|i| start + (i as f64 * 1000.0 / fps).round() as i64)
                .collect()
        };
        let tb = (1, 1000);
        assert_eq!(
            estimate_rate(&ms(30.0, 41, 533), tb),
            Some(Estimate::Standard(30, 1))
        );
        assert_eq!(
            estimate_rate(&ms(30_000.0 / 1001.0, 41, 0), tb),
            Some(Estimate::Standard(30_000, 1001))
        );
        assert_eq!(
            estimate_rate(&ms(24.0, 41, 0), tb),
            Some(Estimate::Standard(24, 1))
        );
        // No standard rate fits: the base rate of the intervals.
        assert_eq!(
            estimate_rate(&ms(70.0, 41, 0), tb),
            Some(Estimate::Base(1000, 1, false))
        );
        assert_eq!(
            estimate_rate(&ms(200.0, 41, 0), tb),
            Some(Estimate::Base(200, 1, true))
        );
        assert_eq!(estimate_rate(&[5], tb), None);
        // Found black box on crafted VP8 files: a 30 fps grid with a frame
        // 1 ms after every fifth still estimates 30; equal intervals give
        // their own rate; a rate covering too few intervals is passed over.
        let mut dup = ms(30.0, 41, 0);
        for i in (0..41).step_by(5) {
            dup.push(dup[i] + 1);
        }
        dup.sort_unstable();
        dup.truncate(41);
        assert_eq!(estimate_rate(&dup, tb), Some(Estimate::Standard(30, 1)));
        let equal: Vec<i64> = (0..41).map(|i| i * 155).collect();
        assert_eq!(
            estimate_rate(&equal, tb),
            Some(Estimate::Base(200, 31, true))
        );
        let pairs: Vec<i64> = (0..16).flat_map(|i| [i * 1000, i * 1000 + 1]).collect();
        assert_eq!(estimate_rate(&pairs, tb), Some(Estimate::Standard(2, 1)));
        // The window: 41, at least 32, else the first time 5 s and two
        // frames past the first; 21 for a time base finer than 0.5 ms.
        let slow: Vec<i64> = (0..48).map(|i| i * 150).collect();
        assert_eq!(rate_window(&slow, tb), 37);
        assert_eq!(rate_window(&ms(30.0, 48, 0), tb), 41);
        let two: Vec<i64> = (0..48).map(|i| i * 500).collect();
        assert_eq!(rate_window(&two, tb), 32);
        assert_eq!(rate_window(&ms(30.0, 48, 0), (1, 10_000)), 21);
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn fractions_of_floats() {
        // Found black box on crafted mastering metadata.
        for (v, want) in [
            (f64::from(0.68f32), (11_408_507, 16_777_216)),
            (f64::from(0.005f32), (5_368_709, 1_073_741_824)),
            (0.68, (17, 25)),
            (0.123_456_789, (177_723_509, 1_439_560_436)),
            (0.333_333_333_333, (1, 3)),
            (0.707_106_781_186_547_6, (1_479_444_199, 2_092_250_051)),
            (0.999_999_99, (199_999_997, 199_999_999)),
            (0.618_033_988_749_894_9, (530_125_705, 857_761_409)),
            (1_234.567_89, (123_456_789, 100_000)),
            (0.000_123_4, (617, 5_000_000)),
            (3.141_592_653_589_79, (789_453_460, 251_290_841)),
            (1e-10, (0, 1)),
            (2e-10, (0, 1)),
            (0.499_999_999_9, (1, 2)),
            (4e9, (1, 0)),
            (1e-5, (1, 100_000)),
            (1000.0, (1000, 1)),
        ] {
            assert_eq!(d2q(v), Some(want), "{v}");
        }
    }

    #[test]
    fn small_helpers() {
        assert_eq!(webvtt_payload(b"\n\nhello"), Some(5));
        assert_eq!(webvtt_payload(b"id\nline:0\nab"), Some(2));
        assert_eq!(webvtt_payload(b"no newline"), None);
        assert_eq!(
            display_matrix(90),
            "\n00000000:            0      -65536           0\n\
             00000001:        65536           0           0\n\
             00000002:            0           0  1073741824\n"
        );
        assert_eq!(attachment_codec("application/x-font-opentype"), Some("ttf"));
        assert_eq!(attachment_codec("application/vnd.ms-opentype"), Some("otf"));
        assert_eq!(attachment_codec("font/ttf"), None);
        assert_eq!(stereo_name(1), Some("left_right"));
        assert_eq!(stereo_name(0), None);
        assert_eq!(from_micros(-21_000, (1, 1000)), -21);
        assert_eq!(from_micros(2_047_000, (1, 90_000)), 184_230);
    }
}
