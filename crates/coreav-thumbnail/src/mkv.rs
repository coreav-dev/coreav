//! Matroska and WebM: the video track and the packets around the time
//! asked for, read from the Cues' cluster when the file has Cues.

use coreav_decode::{Codec, CodecConfig, ConfigKind, StreamConfig};
use coreav_isobmff::Rational;
use coreav_isobmff::access::AccessFormat;
use coreav_mkv::{Allowlist, Encoding, Packet, Segment, Source, TrackKind, probe};

use crate::mp4::color_info;
use crate::select::{self, Sample};
use crate::{Container, CutRule, Error, PacketRef, Plan, Presentation, ThumbnailOptions};

/// Packets read from the start to find where the file starts.
const START_PACKETS: usize = 64;
const NANOS: u64 = 1_000_000_000;

pub(crate) fn plan<S: Source>(
    source: &mut S,
    options: &ThumbnailOptions,
    first: Option<usize>,
) -> Result<Plan, Error> {
    let limits = &options.matroska_limits;
    let allowlist = allowlist(options);
    let segment = probe(&mut *source, limits, &allowlist)?;
    let track = segment
        .tracks
        .iter()
        .filter(|t| {
            t.kind == TrackKind::Video && options.stream.is_none_or(|s| s == t.stream_index)
        })
        // FFmpeg's default: the highest resolution, the first on a tie.
        .fold(None, |best: Option<&coreav_mkv::Track>, t| {
            let area = |t: &coreav_mkv::Track| {
                let v = t.video.unwrap_or_default();
                v.pixel_width
                    .unwrap_or(0)
                    .saturating_mul(v.pixel_height.unwrap_or(0))
            };
            match best {
                Some(b) if area(b) >= area(t) => Some(b),
                _ => Some(t),
            }
        })
        .ok_or(Error::NoVideo)?;
    let codec = codec(track.codec).ok_or_else(|| {
        Error::Unsupported(format!("no decoder for {} video", track.codec.name()))
    })?;
    let prefix = match &track.encoding {
        Encoding::None => Vec::new(),
        Encoding::HeaderStripping(bytes) => bytes.clone(),
        Encoding::Encrypted => return Err(Error::Unsupported("encrypted video".into())),
        Encoding::Zlib | Encoding::Unsupported => {
            return Err(Error::Unsupported("compressed video frames".into()));
        }
    };
    let config = track.codec_private.as_ref().map(|data| CodecConfig {
        kind: match codec {
            Codec::H264 => ConfigKind::Avcc,
            Codec::Hevc => ConfigKind::Hvcc,
            Codec::Av1 => ConfigKind::Av1c,
            Codec::ProRes => ConfigKind::ProResFourcc,
            Codec::Mpeg4 => ConfigKind::Mpeg4Dsi,
            _ => ConfigKind::MatroskaPrivate,
        },
        data: data.clone(),
    });
    let rule = match (codec, &config) {
        (Codec::H264, Some(c)) => c
            .data
            .get(4)
            .map(|b| CutRule::H264 {
                format: AccessFormat::H264 {
                    length_size: (b & 3) + 1,
                },
                max_num_ref_frames: None,
            })
            .unwrap_or(CutRule::Container),
        // No `CodecPrivate`: the frames are a byte stream and carry their
        // own parameter sets (RFC 9559 codec mappings, `V_MPEG4/ISO/AVC`;
        // `docs/design/thumbnails.md`, "H.264 without a configuration
        // record"), so the block's keyframe flag is checked against them.
        (Codec::H264, None) => CutRule::H264ByteStream,
        (Codec::Hevc, Some(c)) => c
            .data
            .get(21)
            .map(|b| CutRule::Hevc {
                length_size: (b & 3) + 1,
            })
            .unwrap_or(CutRule::Container),
        (Codec::Av1, _) => CutRule::Av1,
        _ => CutRule::Container,
    };
    let video = track.video.unwrap_or_default();
    let (width, height) = (
        video.pixel_width.and_then(|v| u32::try_from(v).ok()),
        video.pixel_height.and_then(|v| u32::try_from(v).ok()),
    );
    let pixel_aspect = match (video.display_aspect(), width, height) {
        (Some((dw, dh)), Some(pw), Some(ph)) => {
            // Pixel width : height = (DisplayWidth × PixelHeight) :
            // (DisplayHeight × PixelWidth).
            let h = dw.checked_mul(u64::from(ph));
            let v = dh.checked_mul(u64::from(pw));
            h.zip(v)
        }
        _ => None,
    };
    let quarter_turns = video
        .rotation()
        .and_then(|d| u8::try_from((d / 90) % 4).ok())
        .unwrap_or(0);
    let stream = StreamConfig {
        codec,
        config,
        width,
        height,
        color: color_info(track.color),
    };
    let start = start_time(&segment, source, limits);
    let at = start
        .checked_add(options.at)
        .ok_or_else(|| Error::Unsupported("time out of range".into()))?;
    let (samples, packets, durations) = match first {
        Some(limit) => read(
            &segment,
            track.index,
            segment.packets(&mut *source, &options.matroska_limits),
            options,
            None,
            Some(limit),
        )?,
        None => packets(&segment, track.index, source, options, at, rule, &prefix)?,
    };
    let stated = vec![true; packets.len()];
    Ok(Plan {
        warnings: Vec::new(),
        stream_index: track.stream_index,
        stream,
        quarter_turns,
        pixel_aspect,
        start,
        samples,
        packets,
        prefix,
        rule,
        stated,
        window: None,
        first_keyframe: None,
        container: Container::Matroska,
        durations,
        // A Segment Tick is `TimestampScale` nanoseconds.
        time_base: Rational::new(i128::from(segment.timestamp_scale), NANOS)
            .unwrap_or(Rational::from_int(0)),
        presentation: Presentation::All,
    })
}

/// The Matroska allowlist: the default, plus the long-tail video codecs
/// the decode policy allows.
fn allowlist(options: &ThumbnailOptions) -> Allowlist {
    let mut codecs = Allowlist::default().codecs().to_vec();
    for (decode, mkv) in [
        (Codec::Mpeg4, coreav_mkv::Codec::Mpeg4),
        (Codec::Mpeg1Video, coreav_mkv::Codec::Mpeg1Video),
        (Codec::Mpeg2Video, coreav_mkv::Codec::Mpeg2Video),
        (Codec::Mjpeg, coreav_mkv::Codec::Mjpeg),
        (Codec::Theora, coreav_mkv::Codec::Theora),
        (Codec::Ffv1, coreav_mkv::Codec::Ffv1),
    ] {
        if options.policy.allowlist.allows(decode) {
            codecs.push(mkv);
        }
    }
    Allowlist::new(&codecs)
}

fn codec(codec: coreav_mkv::Codec) -> Option<Codec> {
    use coreav_mkv::Codec as M;
    Some(match codec {
        M::H264 => Codec::H264,
        M::Hevc => Codec::Hevc,
        M::Av1 => Codec::Av1,
        M::Vp8 => Codec::Vp8,
        M::Vp9 => Codec::Vp9,
        M::ProRes => Codec::ProRes,
        M::Mpeg4 => Codec::Mpeg4,
        M::Mpeg1Video => Codec::Mpeg1Video,
        M::Mpeg2Video => Codec::Mpeg2Video,
        M::Mjpeg => Codec::Mjpeg,
        M::Theora => Codec::Theora,
        M::Ffv1 => Codec::Ffv1,
        _ => return None,
    })
}

fn nanos(ns: i128) -> Option<Rational> {
    Rational::new(ns, NANOS)
}

/// The earliest presentation time among the first packets: where `ffmpeg
/// -ss` counts from; 0 when none is known.
fn start_time<S: Source>(
    segment: &Segment,
    source: &mut S,
    limits: &coreav_mkv::Limits,
) -> Rational {
    segment
        .packets(&mut *source, limits)
        .take(START_PACKETS)
        .map_while(Result::ok)
        .filter_map(|p| p.pts_ns(segment))
        .min()
        .and_then(nanos)
        .unwrap_or(Rational::from_int(0))
}

/// The video track's packets around `at`: from the Cues' cluster at or
/// before it (or the first cluster), until a keyframe presented after `at`
/// and the frames it leads (`select`'s stop rule).
fn packets<S: Source>(
    segment: &Segment,
    track: usize,
    source: &mut S,
    options: &ThumbnailOptions,
    at: Rational,
    rule: CutRule,
    prefix: &[u8],
) -> Result<Listed, Error> {
    let limits = &options.matroska_limits;
    let number = segment.tracks.get(track).map(|t| t.number);
    let cluster = if segment.cues.is_some() {
        let index = segment.index(&mut *source, limits)?;
        let ticks = at
            .num
            .checked_mul(i128::from(NANOS))
            .map(|n| n / i128::from(at.den.max(1)) / i128::from(segment.timestamp_scale.max(1)))
            .and_then(|t| u64::try_from(t.max(0)).ok())
            .unwrap_or(0);
        index.cluster_for(number, ticks)
    } else {
        None
    };
    if let Some(cluster) = cluster {
        let (samples, packets, durations) = read(
            segment,
            track,
            segment.packets_from(&mut *source, limits, cluster),
            options,
            Some(at),
            None,
        )?;
        // The Cues led to no keyframe at or before the time (they point
        // past it, at a cluster without one, or at blocks whose coded data
        // does not start decoding, as a writer that flagged every block
        // leaves them): read from the start.
        if cut_point_before(source, &samples, &packets, at, rule, prefix)? {
            return Ok((samples, packets, durations));
        }
    }
    let (samples, packets, durations) = read(
        segment,
        track,
        segment.packets(&mut *source, limits),
        options,
        Some(at),
        None,
    )?;
    if cut_point_before(source, &samples, &packets, at, rule, prefix)? {
        return Ok((samples, packets, durations));
    }
    // No block at or before the time starts decoding: the stop rule would
    // end the listing before the first that does, which `select` falls
    // back to, so every block is listed.
    read(
        segment,
        track,
        segment.packets(&mut *source, limits),
        options,
        None,
        None,
    )
}

/// Whether a block of the listing that `select` offers as a keyframe at
/// or before `at` (`select::offered_by`, the rule `select::keyframe`
/// applies) starts decoding (`crate::starts_decoding`). The blocks are
/// checked from the last in decode order back, so that a listing which
/// holds one costs one check; the order decides which block's error, such
/// as a work limit, is met first, and a block before the one found is
/// never read.
fn cut_point_before<S: Source>(
    source: &mut S,
    samples: &[Sample],
    packets: &[PacketRef],
    at: Rational,
    rule: CutRule,
    prefix: &[u8],
) -> Result<bool, Error> {
    let candidates = samples
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, s)| select::offered_by(s, at))
        .map(|(i, _)| i);
    for index in candidates {
        let Some(packet) = packets.get(index) else {
            continue;
        };
        // Matroska states every block's keyframe flag, so an unreadable
        // frame keeps it (`crate::cut_point`).
        if crate::starts_decoding(source, rule, prefix, packet, true)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The track's packets of `listing`, at most `limit` of them. With `stop`,
/// the listing ends at the first keyframe presented after that time and
/// the frames it leads (`select`'s stop rule); without one it runs to the
/// end of the listing.
fn read<S: Source>(
    segment: &Segment,
    track: usize,
    listing: coreav_mkv::Packets<'_, S>,
    options: &ThumbnailOptions,
    stop: Option<Rational>,
    limit: Option<usize>,
) -> Result<Listed, Error> {
    let scale = i128::from(segment.timestamp_scale);
    let default_duration = segment.tracks.get(track).and_then(|t| t.default_duration);
    let mut samples = Vec::new();
    let mut packets = Vec::new();
    let mut durations = Vec::new();
    // A keyframe presented after `at`, once seen: (its time).
    let mut later_key: Option<Rational> = None;
    for packet in listing {
        // A file cut short (a partial upload): its complete blocks are
        // listed, then the listing ends.
        let p: Packet = match packet {
            Ok(p) => p,
            Err(coreav_mkv::Error::Truncated { .. }) if !packets.is_empty() => break,
            Err(e) => return Err(e.into()),
        };
        if p.track != track {
            continue;
        }
        if limit.is_some_and(|n| packets.len() >= n) {
            break;
        }
        let ns = p.pts_ns(segment);
        let time = ns.and_then(nanos);
        if let Some(k) = later_key {
            // Stop at the first packet after it that it does not lead.
            let leads = time.is_some_and(|t| t.checked_lt(k).unwrap_or(false));
            if !leads || !options.exact {
                break;
            }
        }
        let duration = p
            .block_duration
            .and_then(|d| i128::from(d).checked_mul(scale))
            .or_else(|| default_duration.map(i128::from));
        let end = ns
            .zip(duration)
            .and_then(|(n, d)| n.checked_add(d))
            .and_then(nanos);
        let pts = p.pts.unwrap_or(p.block_timestamp);
        samples.push(Sample {
            time,
            end,
            presented: !p.invisible,
            keyframe: p.keyframe,
        });
        packets.push(PacketRef {
            offset: p.offset,
            size: p.stored_size,
            pts,
            dts: p.dts,
            keyframe: p.keyframe,
        });
        // In Segment Ticks: `BlockDuration` as stored, else `DefaultDuration`
        // (nanoseconds) in ticks, rounded.
        durations.push(p.block_duration.or_else(|| {
            let scale = segment.timestamp_scale.max(1);
            default_duration.map(|d| d.saturating_add(scale / 2) / scale)
        }));
        if p.keyframe
            && later_key.is_none()
            && stop.is_some_and(|at| time.is_some_and(|t| at.checked_lt(t).unwrap_or(false)))
        {
            later_key = time;
        }
    }
    Ok((samples, packets, durations))
}

/// The packets a listing gives: what selection needs of each, where its
/// bytes are, and its duration in Segment Ticks when known.
type Listed = (Vec<Sample>, Vec<PacketRef>, Vec<Option<u64>>);

#[cfg(test)]
mod tests {
    use coreav_isobmff::SliceSource;

    use super::*;
    use crate::select::Choice;

    fn r(n: i128) -> Rational {
        Rational::from_int(n)
    }

    /// A Cues listing is kept when it holds a block `select::keyframe`
    /// would give, so `cut_point_before` must find a block exactly when
    /// keyframe mode chooses one: a listing it accepts without one gives
    /// the frame at the time instead of a keyframe (coreav-dev/coreav#16, finding 5).
    #[test]
    fn cut_point_before_offers_what_keyframe_mode_gives() {
        let at = r(2);
        let key = |time: Option<i128>, presented: bool| Sample {
            time: time.map(r),
            end: time.map(|t| r(t + 1)),
            presented,
            keyframe: true,
        };
        let cases = [
            ("presented before the time", key(Some(1), true), true),
            ("presented at the time", key(Some(2), true), true),
            ("invisible, flagged a keyframe", key(Some(1), false), false),
            ("without a time", key(None, true), false),
            ("presented after the time", key(Some(3), true), false),
        ];
        for (name, keyframe, offered) in cases {
            // The keyframe, then the frame at the time, which exact mode
            // gives when keyframe mode falls back to it.
            let samples = [
                keyframe,
                Sample {
                    time: Some(at),
                    end: Some(r(3)),
                    presented: true,
                    keyframe: false,
                },
            ];
            let packet = PacketRef {
                offset: 0,
                size: 0,
                pts: 0,
                dts: None,
                keyframe: true,
            };
            let mut source = SliceSource::new(&[]);
            let mkv = cut_point_before(
                &mut source,
                &samples,
                &[packet; 2],
                at,
                CutRule::Container,
                &[],
            )
            .expect("cut_point_before");
            let given = select::keyframe(&samples, at, false, &mut |_| Ok(true)).ok()
                == Some(Choice {
                    start: 0,
                    target: 0,
                });
            assert_eq!((mkv, given), (offered, offered), "{name}");
        }
    }

    /// The listing check visits the blocks offered at or before the time
    /// from the last in decode order back, so a block it need not read is
    /// not read: here the block at 40 ms, whose filler units exceed the
    /// work limit, comes before the one at 0 ms in decode order, and
    /// `--exact` at 41 ms decodes from the block at 0 ms.
    #[test]
    fn cut_point_before_reads_from_the_last_block_in_decode_order() {
        let ms = |n: i128| Rational::new(n, 1000).expect("ms");
        // An IDR slice of four-byte NAL units, and a filler NAL unit.
        let idr = [0, 0, 0, 5, 0x65, 0xb8, 0, 0, 0];
        let filler = [0, 0, 0, 2, 0x0c, 0x80];
        let mut data = Vec::new();
        let mut packets = Vec::new();
        let mut samples = Vec::new();
        for (time, keyframe, fillers) in [(40, true, 100_001), (0, true, 0), (60, false, 0)] {
            let offset = u64::try_from(data.len()).expect("offset");
            for _ in 0..fillers {
                data.extend_from_slice(&filler);
            }
            data.extend_from_slice(&idr);
            packets.push(PacketRef {
                offset,
                size: u64::try_from(data.len()).expect("size") - offset,
                pts: 0,
                dts: None,
                keyframe,
            });
            samples.push(Sample {
                time: Some(ms(time)),
                end: Some(ms(time + 40)),
                presented: true,
                keyframe,
            });
        }
        let rule = CutRule::H264 {
            format: AccessFormat::H264 { length_size: 4 },
            max_num_ref_frames: None,
        };
        let mut source = SliceSource::new(&data);
        let found = cut_point_before(&mut source, &samples, &packets, ms(41), rule, &[]);
        assert!(matches!(found, Ok(true)), "{found:?}");
    }
}
