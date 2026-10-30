//! One pass over the blocks for what Matroska does not store per track
//! (`docs/design/policy-matroska.md`, decision 2): each track's
//! presentation span, its frame bytes, and the bound of the picture sizes
//! its parameter sets and frame headers define, read with the frame
//! readers of `coreav-isobmff` (`docs/design/frame-sizes.md`).

use crate::packets::{Packet, Packets};
use crate::segment::Segment;
use crate::track::{Codec, Encoding, Track, TrackKind};
use crate::{Error, LimitKind, Limits, Source};
use coreav_isobmff::frames::{self, FrameFormat, FrameSizes, RecordSize};

/// The longest Opus packet, in 48 kHz samples: 120 ms (RFC 6716 3.2.5).
const MAX_OPUS_SAMPLES: u32 = 5760;

/// The duration of an Opus packet in nanoseconds from its TOC byte and, for
/// code 3, its frame count byte (RFC 6716 3.1 and 3.2.5); `None` for an
/// empty packet or one a decoder rejects (no frames, above 120 ms).
fn opus_duration(head: &[u8]) -> Option<i128> {
    let toc = *head.first()?;
    let config = usize::from(toc >> 3);
    // Frame sizes in 48 kHz samples: SILK-only configurations 0 to 11,
    // hybrid 12 to 15, CELT-only 16 to 31.
    let frame: u32 = match config {
        0..=11 => [480, 960, 1920, 2880].get(config % 4).copied()?,
        12..=15 => [480, 960].get(config % 2).copied()?,
        _ => [120, 240, 480, 960].get(config % 4).copied()?,
    };
    let count = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => u32::from(*head.get(1)? & 0x3f),
    };
    let samples = frame
        .checked_mul(count)
        .filter(|s| (1..=MAX_OPUS_SAMPLES).contains(s))?;
    // Every frame size is a multiple of 120 samples, 2.5 ms.
    Some(i128::from(samples / 120) * 2_500_000)
}

/// The 8 bytes before a ProRes frame header in an ISO base media sample:
/// the frame size and the identifier `icpf` (SMPTE RDD 36 5.1).
const PRORES_ATOM: [u8; 8] = [0, 0, 0, 0, b'i', b'c', b'p', b'f'];

/// What the scan found for one track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackScan {
    /// Frames listed.
    pub frames: u64,
    /// Bytes of every frame, stripped headers included ([`Packet::size`]).
    pub bytes: u64,
    /// The earliest presentation time of a frame, in nanoseconds.
    pub start_ns: Option<i128>,
    /// The latest end of a frame, in nanoseconds: its presentation time
    /// plus its `BlockDuration` (which covers the whole block),
    /// `DefaultDuration`, or the duration its codec states (Opus, RFC 6716
    /// 3.1); a frame with none of them ends where it starts.
    pub end_ns: Option<i128>,
    /// Frames whose presentation time cannot be known: a later frame of a
    /// lace without `DefaultDuration` whose codec does not state the
    /// duration of the frames before it, or any frame of a track whose
    /// `TrackTimestampScale` is not 1.0 ([`Packet::pts_ns`]).
    pub untimed_frames: u64,
    /// The bound, in each dimension, of the picture sizes the codec
    /// configuration and every frame define; `None` when the codec carries
    /// no size the scan reads, the frames are encrypted, the configuration
    /// cannot be read, no frame states a size, or a frame cannot be read.
    pub frame_size: Option<(u32, u32)>,
    /// The track's frames are listed ([`Track::packets_available`]) with
    /// known sizes (a zlib frame that does not inflate within the limits
    /// has none); when not, the counts say nothing about the track.
    pub listed: bool,
}

impl TrackScan {
    /// The span from the earliest frame start to the latest frame end, in
    /// nanoseconds: 0 for a track without frames, `None` when a frame's
    /// time is unknown or the track's frames cannot be listed.
    pub fn duration_ns(&self) -> Option<i128> {
        if self.untimed_frames > 0 || !self.listed {
            return None;
        }
        match (self.start_ns, self.end_ns) {
            (Some(start), Some(end)) => Some(end.saturating_sub(start).max(0)),
            _ => Some(0),
        }
    }
}

/// The result of [`Segment::scan_tracks`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scan {
    /// One entry per track of [`Segment::tracks`], in the same order.
    pub tracks: Vec<TrackScan>,
}

/// How the scan reads one track's picture sizes.
struct SizeState {
    format: FrameFormat,
    sizes: FrameSizes,
    prores: bool,
}

/// How the scan reads a track's picture sizes: `None` when it reads none
/// for the track's codec, `Some(Err(()))` when the size cannot be verified
/// (encrypted frames, an unreadable configuration).
fn size_state(track: &Track) -> Option<Result<SizeState, ()>> {
    if track.kind != TrackKind::Video {
        return None;
    }
    let private = track.codec_private.as_deref();
    // H.264 and HEVC frames are length-prefixed after the record that
    // CodecPrivate holds; without one (as Chrome writes H.264), decoders
    // read them as a byte stream with start codes (Annex B).
    let (format, record) = match (track.codec, private) {
        (Codec::H264, None) => (Some(FrameFormat::H264AnnexB), None),
        (Codec::Hevc, None) => (Some(FrameFormat::HevcAnnexB), None),
        (Codec::H264, Some(p)) => (FrameFormat::from_record(b"avcC", p), Some(b"avcC")),
        (Codec::Hevc, Some(p)) => (FrameFormat::from_record(b"hvcC", p), Some(b"hvcC")),
        (Codec::Av1, _) => (Some(FrameFormat::Av1), Some(b"av1C")),
        (Codec::Vp8, _) => (Some(FrameFormat::Vp8), None),
        (Codec::Vp9, _) => (Some(FrameFormat::Vp9), None),
        (Codec::ProRes, _) => (Some(FrameFormat::ProRes), None),
        _ => return None,
    };
    // A record too short for its NAL unit length size gives no way to walk
    // the frames; encrypted or compressed frames are not the frames a
    // decoder reads.
    let Some(format) = format.filter(|_| !track.encoding.opaque()) else {
        return Some(Err(()));
    };
    let configured = match (record, private) {
        (Some(kind), Some(private)) => frames::record_size(kind, private),
        _ => RecordSize::Absent,
    };
    let configured = match configured {
        RecordSize::Absent => None,
        RecordSize::Bound(size) => Some(size),
        RecordSize::Unreadable => return Some(Err(())),
    };
    Some(Ok(SizeState {
        format,
        sizes: FrameSizes::new(configured),
        prores: track.codec == Codec::ProRes,
    }))
}

/// One frame seen through its track's header stripping: the stripped bytes
/// followed by the stored ones, at offsets from 0.
struct Frame<'a, S> {
    prefix: &'a [u8],
    source: &'a mut S,
    stored: u64,
    stored_size: u64,
}

impl<S: Source> Source for Frame<'_, S> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        Ok((self.prefix.len() as u64).saturating_add(self.stored_size))
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        let prefix_len = self.prefix.len() as u64;
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|end| *end <= prefix_len.saturating_add(self.stored_size))
            .ok_or(coreav_isobmff::Error::Truncated { offset })?;
        let from_prefix = usize::try_from(prefix_len.saturating_sub(offset).min(end - offset))
            .map_err(|_| coreav_isobmff::Error::Overflow { offset })?;
        let (head, tail) = buf.split_at_mut(from_prefix.min(buf.len()));
        if !head.is_empty() {
            let start =
                usize::try_from(offset).map_err(|_| coreav_isobmff::Error::Overflow { offset })?;
            let bytes = self
                .prefix
                .get(start..start + head.len())
                .ok_or(coreav_isobmff::Error::Truncated { offset })?;
            head.copy_from_slice(bytes);
        }
        if !tail.is_empty() {
            let within = offset.saturating_add(head.len() as u64) - prefix_len;
            self.source.read_exact_at(self.stored + within, tail)?;
        }
        Ok(())
    }
}

impl Segment {
    /// Lists every frame of every kept track once ([`Segment::packets`])
    /// and returns, per track, its frame count, bytes, presentation span
    /// and the bound of the picture sizes its frames define (H.264, HEVC
    /// and AV1 parameter sets in `CodecPrivate` and in the frames, VP8 key
    /// frames, VP9 and ProRes frame headers). Coded data is not read.
    ///
    /// A structure audit checks that the listing can see every frame a
    /// reader could find (`docs/design/policy-matroska.md`, decision 8),
    /// in the same walk over the file as the listing: a file it cannot
    /// account for, or a truncated one, is an error, as is any error of the
    /// listing, because the values would miss frames. An audit error is
    /// returned rather than the listing's, wherever in the file it is.
    ///
    /// Bounded by `limits`: the audit and the packet listing each by
    /// `max_block_work`, and the NAL units, OBUs and frame headers read by a
    /// third `max_block_work`.
    pub fn scan_tracks<S: Source>(&self, source: S, limits: &Limits) -> Result<Scan, Error> {
        let mut packets = self
            .packets(source, limits)
            .with_inflated_sizes()
            .with_audit();
        self.scan_packets(&mut packets, limits)
            .map_err(|error| packets.audit_error(error))
    }

    /// The scan of [`Segment::scan_tracks`] over `packets`.
    fn scan_packets<S: Source>(
        &self,
        packets: &mut Packets<'_, S>,
        limits: &Limits,
    ) -> Result<Scan, Error> {
        let mut tracks: Vec<TrackScan> = self
            .tracks
            .iter()
            .map(|track| TrackScan {
                frames: 0,
                bytes: 0,
                start_ns: None,
                end_ns: None,
                untimed_frames: 0,
                frame_size: None,
                listed: track.packets_available(),
            })
            .collect();
        let mut states: Vec<Option<Result<SizeState, ()>>> =
            self.tracks.iter().map(size_state).collect();
        let mut work = 0u64;
        // Per track: the block of the lace being read and the time of its
        // next frame, when known.
        let mut laces: Vec<(u64, Option<i128>)> = vec![(u64::MAX, None); self.tracks.len()];
        while let Some(packet) = packets.next() {
            let packet = packet?;
            let (Some(track), Some(scan), Some(lace)) = (
                self.tracks.get(packet.track),
                tracks.get_mut(packet.track),
                laces.get_mut(packet.track),
            ) else {
                continue;
            };
            scan.frames += 1;
            scan.bytes = scan.bytes.saturating_add(packet.size());
            if track.encoding == Encoding::Zlib && packet.inflated_size.is_none() {
                // A zlib frame that did not inflate within the limits: its
                // size, and so the track's bytes, are unknown.
                scan.listed = false;
            }
            let codec_duration = if track.codec == Codec::Opus
                && track.default_duration.is_none()
                && !track.encoding.opaque()
            {
                let prefix: &[u8] = match &track.encoding {
                    Encoding::HeaderStripping(prefix) => prefix,
                    _ => &[],
                };
                let mut frame = Frame {
                    prefix,
                    source: packets.source_mut(),
                    stored: packet.offset,
                    stored_size: packet.stored_size,
                };
                let mut head = [0u8; 2];
                let len = usize::try_from(packet.size().min(2)).unwrap_or(0);
                if let Some(head) = head.get_mut(..len) {
                    frame.read_exact_at(0, head)?;
                }
                head.get(..len).and_then(opus_duration)
            } else {
                None
            };
            if packet.lace == 0 || lace.0 != packet.block_offset {
                *lace = (packet.block_offset, None);
            }
            let start = packet.pts_ns(self).or_else(|| {
                // A later frame of a lace without DefaultDuration starts where
                // the frames before it end, when their codec says.
                (packet.lace > 0 && track.timestamp_scale_one)
                    .then_some(lace.1)
                    .flatten()
            });
            let timed = start.and_then(|start| {
                times(self, track, &packet, start, codec_duration).map(|end| (start, end))
            });
            lace.1 = start.and_then(|start| {
                start.checked_add(track.default_duration.map(i128::from).or(codec_duration)?)
            });
            match timed {
                Some((start, end)) => {
                    scan.start_ns = Some(scan.start_ns.map_or(start, |s| s.min(start)));
                    scan.end_ns = Some(scan.end_ns.map_or(end, |e| e.max(end)));
                }
                None => scan.untimed_frames += 1,
            }
            let Some(Some(Ok(state))) = states.get_mut(packet.track) else {
                continue;
            };
            if state.sizes.is_unverifiable() {
                continue;
            }
            let mut prefix: &[u8] = match &track.encoding {
                Encoding::HeaderStripping(prefix) => prefix,
                _ => &[],
            };
            let mut frame = Frame {
                prefix,
                source: packets.source_mut(),
                stored: packet.offset,
                stored_size: packet.stored_size,
            };
            if state.prores {
                // A ProRes frame stored without the frame size and `icpf`
                // that start it in an ISO base media sample reads as if they
                // were there. A frame too short for the atom has none; a
                // source that fails to read fails the scan.
                let mut head = [0u8; 8];
                let atom = match frame.read_exact_at(0, &mut head) {
                    Ok(()) => head.get(4..8) == Some(b"icpf".as_slice()),
                    Err(error @ coreav_isobmff::Error::Io(_)) => return Err(Error::from(error)),
                    Err(_) => false,
                };
                if !atom && prefix.is_empty() {
                    prefix = &PRORES_ATOM;
                    frame.prefix = prefix;
                }
            }
            let len = frame.len()?;
            let mut exhausted = false;
            let max = limits.max_block_work;
            let read = frames::read_frame(
                state.format,
                &mut frame,
                0,
                len,
                &mut state.sizes,
                &mut || {
                    if work >= max {
                        exhausted = true;
                        return Err(coreav_isobmff::Error::LimitExceeded {
                            limit: coreav_isobmff::LimitKind::SampleWork,
                            offset: packet.offset,
                        });
                    }
                    work += 1;
                    Ok(())
                },
            );
            match read {
                Ok(()) => {}
                Err(_) if exhausted => {
                    return Err(Error::LimitExceeded {
                        limit: LimitKind::BlockWork,
                        offset: packet.offset,
                    });
                }
                Err(error) => return Err(Error::from(error)),
            }
        }
        for (scan, state) in tracks.iter_mut().zip(states) {
            scan.frame_size = match state {
                Some(Ok(state)) => state.sizes.bound(),
                _ => None,
            };
        }
        if !packets.warnings().is_empty() {
            // Blocks the listing skipped (an undefined track, for example):
            // the audit should have refused the file; do not trust it.
            return Err(Error::Unsupported {
                reason: "the packet listing skipped blocks",
            });
        }
        Ok(Scan { tracks })
    }
}

/// The end, in nanoseconds, of a frame that starts at `start`: the end of
/// its block's `BlockDuration`, else `start` plus `DefaultDuration` or the
/// duration its codec states (`codec_duration`), else `start`.
fn times(
    segment: &Segment,
    track: &Track,
    packet: &Packet,
    start: i128,
    codec_duration: Option<i128>,
) -> Option<i128> {
    let scale = i128::from(segment.timestamp_scale);
    let end = match packet.block_duration {
        // `BlockDuration` covers every frame of the block, from the block's
        // timestamp.
        Some(duration) => i128::from(packet.block_timestamp)
            .checked_add(i128::from(duration))?
            .checked_mul(scale)?
            .checked_sub(i128::from(track.codec_delay))?,
        None => start.checked_add(
            track
                .default_duration
                .map(i128::from)
                .or(codec_duration)
                .unwrap_or(0),
        )?,
    };
    Some(end.max(start))
}

#[cfg(test)]
mod tests {
    use super::opus_duration;

    #[test]
    fn opus_packet_durations() {
        // Configuration 31 (CELT, 20 ms), one frame; 2.5 ms CELT, two
        // frames; SILK 60 ms; code 3 with 6 frames of 20 ms.
        assert_eq!(opus_duration(&[31 << 3]), Some(20_000_000));
        assert_eq!(opus_duration(&[(16 << 3) | 1]), Some(5_000_000));
        assert_eq!(opus_duration(&[3 << 3]), Some(60_000_000));
        assert_eq!(opus_duration(&[(31 << 3) | 3, 6]), Some(120_000_000));
        // Above 120 ms, no frames, empty, or a code 3 packet cut short.
        assert_eq!(opus_duration(&[(31 << 3) | 3, 7]), None);
        assert_eq!(opus_duration(&[(31 << 3) | 3, 0]), None);
        assert_eq!(opus_duration(&[]), None);
        assert_eq!(opus_duration(&[(31 << 3) | 3]), None);
    }
}
