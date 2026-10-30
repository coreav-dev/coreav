//! Raw frames with their times for `ffmpeg -c:v libx264`
//! (`docs/design/transcode.md`, "Rate control and timestamps"): a Matroska
//! stream (RFC 9559) of one `V_UNCOMPRESSED` track, written front to back
//! to the encoder's standard input. Raw video (`-f rawvideo`) carries no
//! timestamps; in this stream each frame is a `SimpleBlock` in a `Cluster`
//! of its own whose timestamp is the frame's time, so FFmpeg hands
//! libx264 the frames' own times, as it does when it reads the source
//! itself.
//!
//! The track's `ColourSpace` is the frames' layout as a FourCC (`I420`,
//! `Y42B`, `444P`, and `Y3` with the chroma shifts and the depth for 10
//! bits), the planes packed without row padding, as for `-f rawvideo`; the
//! pinned FFmpeg reads every one of these back byte for byte, odd sizes
//! included (checked as a black box). `TimestampScale` is the source's own
//! time base in nanoseconds when that is a whole number (78,125 for
//! 1/12,800), so FFmpeg's stream keeps the source's time base; otherwise one
//! nanosecond, the times rounded to it.
//!
//! A stream of one frame also names the frame rate: the track's
//! `DefaultDuration`, one frame at the source's rate. FFmpeg finds a
//! stream's rate from its frames' times and hands it to libx264; from one
//! frame's time it finds none and takes the inverse of the time base
//! (12,800 frames a second at 1/12,800), and libx264 then encoded the frame
//! 7 dB below `ffmpeg -c:v libx264` on the source. A stream of more frames
//! names no rate: FFmpeg times the frames by a rate it is told, which
//! changed the encode of frames at irregular times (both checked as a black
//! box).

use coreav_frame::{Chroma, PixelFormat};

/// The largest frame in bytes the pinned FFmpeg reads from this stream:
/// its Matroska reader refuses an element longer than 2^28 bytes (checked
/// as a black box: a SimpleBlock of 2^28 + 32,772 bytes was refused, one
/// of 2^28 − 16,380 read), and a frame's Cluster holds the frame and a few
/// bytes of headers. 16,384 × 5,461 at 4:4:4, 8 bits, fits; 8K 4:2:0 at
/// 10 bits (99.5 MB) is far below.
pub const MAX_FRAME_BYTES: usize = (1 << 28) - 32;

/// How the frames' times become block timestamps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timescale {
    /// Nanoseconds per Segment Tick (`TimestampScale`).
    pub scale_ns: u64,
    /// Seconds per tick of the frames' times: `num / den`.
    num: u64,
    den: u64,
}

impl Timescale {
    /// For frame times in ticks of `num / den` seconds; `None` for a zero
    /// term.
    pub fn new(num: u64, den: u64) -> Option<Self> {
        if num == 0 || den == 0 {
            return None;
        }
        let ns = u128::from(num) * 1_000_000_000;
        let scale_ns = if ns % u128::from(den) == 0 {
            u64::try_from(ns / u128::from(den)).ok().filter(|s| *s > 0)
        } else {
            None
        }
        .unwrap_or(1);
        Some(Self { scale_ns, num, den })
    }

    /// The Segment Ticks of a time `ticks` after the first frame, rounded
    /// to the nearest; `None` when negative or out of range.
    pub fn ticks(&self, ticks: i64) -> Option<u64> {
        let ticks = u128::try_from(ticks).ok()?;
        let num = ticks
            .checked_mul(u128::from(self.num))?
            .checked_mul(1_000_000_000)?;
        let den = u128::from(self.den).checked_mul(u128::from(self.scale_ns))?;
        u64::try_from(num.checked_add(den / 2)? / den).ok()
    }
}

/// The `ColourSpace` FourCC of `format`, for the formats the encoder takes.
pub fn fourcc(format: PixelFormat) -> Option<[u8; 4]> {
    match format {
        PixelFormat::Yuv { chroma, depth: 8 } => match chroma {
            Chroma::Cs420 => Some(*b"I420"),
            Chroma::Cs422 => Some(*b"Y42B"),
            Chroma::Cs444 => Some(*b"444P"),
            Chroma::Mono => None,
        },
        PixelFormat::Yuv { chroma, depth: 10 } => {
            // "Y3", the chroma shifts (horizontal in the high nibble,
            // vertical in the low one), then the depth.
            let shifts = match chroma {
                Chroma::Cs420 => 0x0B,
                Chroma::Cs422 => 0x0A,
                Chroma::Cs444 => 0x00,
                Chroma::Mono => return None,
            };
            Some([b'Y', b'3', shifts, 10])
        }
        _ => None,
    }
}

/// One frame at `num / den` frames a second in nanoseconds, rounded to the
/// nearest; `None` when that is 0 or a term is.
pub fn frame_duration_ns(num: u32, den: u32) -> Option<u64> {
    let num = u64::from(num);
    if num == 0 {
        return None;
    }
    Some((u64::from(den) * 1_000_000_000 + num / 2) / num).filter(|ns| *ns > 0)
}

/// The EBML header, the start of a Segment of unknown size, its Info and
/// its one track: `width` × `height` frames of `fourcc`, each
/// `frame_ns` long by default when that is given.
pub fn header(
    width: u32,
    height: u32,
    fourcc: [u8; 4],
    timescale: &Timescale,
    frame_ns: Option<u64>,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut ebml = Vec::new();
    uint(&mut ebml, 0x4286, 1); // EBMLVersion
    uint(&mut ebml, 0x42F7, 1); // EBMLReadVersion
    uint(&mut ebml, 0x42F2, 4); // EBMLMaxIDLength
    uint(&mut ebml, 0x42F3, 8); // EBMLMaxSizeLength
    element(&mut ebml, 0x4282, b"matroska"); // DocType
    uint(&mut ebml, 0x4287, 4); // DocTypeVersion
    uint(&mut ebml, 0x4285, 2); // DocTypeReadVersion
    element(&mut out, 0x1A45_DFA3, &ebml);
    id(&mut out, 0x1853_8067); // Segment, of unknown size: a stream
    out.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    let mut info = Vec::new();
    uint(&mut info, 0x2A_D7B1, timescale.scale_ns); // TimestampScale
    element(&mut info, 0x4D80, b"coreav"); // MuxingApp
    element(&mut info, 0x5741, b"coreav"); // WritingApp
    element(&mut out, 0x1549_A966, &info);
    let mut video = Vec::new();
    uint(&mut video, 0xB0, u64::from(width)); // PixelWidth
    uint(&mut video, 0xBA, u64::from(height)); // PixelHeight
    element(&mut video, 0x2E_B524, &fourcc); // ColourSpace
    let mut entry = Vec::new();
    uint(&mut entry, 0xD7, 1); // TrackNumber
    uint(&mut entry, 0x73C5, 1); // TrackUID
    uint(&mut entry, 0x83, 1); // TrackType: video
    uint(&mut entry, 0x9C, 0); // FlagLacing
    element(&mut entry, 0x86, b"V_UNCOMPRESSED"); // CodecID
    if let Some(ns) = frame_ns {
        uint(&mut entry, 0x23_E383, ns); // DefaultDuration
    }
    element(&mut entry, 0xE0, &video);
    let mut tracks = Vec::new();
    element(&mut tracks, 0xAE, &entry);
    element(&mut out, 0x1654_AE6B, &tracks);
    out
}

/// What comes before a frame of `frame_bytes` bytes: a Cluster with its
/// Timestamp, and the head of its one SimpleBlock (track 1, relative
/// timestamp 0, a keyframe). The frame's bytes follow it.
pub fn cluster_head(timestamp: u64, frame_bytes: u64) -> Vec<u8> {
    let mut timestamp_element = Vec::new();
    uint(&mut timestamp_element, 0xE7, timestamp);
    let block_bytes = frame_bytes.saturating_add(4);
    let mut block_head = Vec::with_capacity(16);
    id(&mut block_head, 0xA3);
    size(&mut block_head, block_bytes);
    block_head.extend_from_slice(&[0x81, 0x00, 0x00, 0x80]);
    let payload = (timestamp_element.len() as u64)
        .saturating_add(block_head.len() as u64)
        .saturating_add(frame_bytes);
    let mut out = Vec::with_capacity(32);
    id(&mut out, 0x1F43_B675);
    size(&mut out, payload);
    out.extend_from_slice(&timestamp_element);
    out.extend_from_slice(&block_head);
    out
}

fn id(out: &mut Vec<u8>, element_id: u32) {
    let bytes = element_id.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count();
    out.extend_from_slice(bytes.get(skip..).unwrap_or_default());
}

/// An EBML variable-size integer, in the fewest bytes (at most 8).
fn size(out: &mut Vec<u8>, value: u64) {
    let length = (1..=8u32)
        .find(|l| value < (1u64 << (7 * l)) - 1)
        .unwrap_or(8);
    let marked = value | (1u64 << (7 * length));
    let bytes = marked.to_be_bytes();
    let start = 8usize.saturating_sub(length as usize);
    out.extend_from_slice(bytes.get(start..).unwrap_or_default());
}

fn element(out: &mut Vec<u8>, element_id: u32, payload: &[u8]) {
    id(out, element_id);
    size(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

fn uint(out: &mut Vec<u8>, element_id: u32, value: u64) {
    let bytes = value.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count().min(7);
    element(out, element_id, bytes.get(skip..).unwrap_or_default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_mkv::{Allowlist, Limits, Parsed, SliceSource, parse_element_header, probe};

    #[test]
    fn timescales() {
        let t = Timescale::new(1, 12_800).unwrap();
        assert_eq!(t.scale_ns, 78_125);
        assert_eq!(t.ticks(512), Some(512));
        let ms = Timescale::new(1, 1000).unwrap();
        assert_eq!(ms.scale_ns, 1_000_000);
        assert_eq!(ms.ticks(33), Some(33));
        // 1/90,000 s is not a whole number of nanoseconds: 1 ns ticks.
        let t = Timescale::new(1, 90_000).unwrap();
        assert_eq!(t.scale_ns, 1);
        assert_eq!(t.ticks(3003), Some(33_366_667));
        assert_eq!(t.ticks(-1), None);
        assert_eq!(Timescale::new(0, 1), None);
        assert_eq!(Timescale::new(1, 0), None);
    }

    #[test]
    fn frame_durations() {
        assert_eq!(frame_duration_ns(25, 1), Some(40_000_000));
        assert_eq!(frame_duration_ns(30_000, 1001), Some(33_366_667));
        assert_eq!(frame_duration_ns(0, 1), None);
        assert_eq!(frame_duration_ns(25, 0), None);
        assert_eq!(frame_duration_ns(u32::MAX, 1), None);
    }

    /// One element's header at `at`: (id, header length, size).
    fn element_at(stream: &[u8], at: usize) -> (u32, usize, Option<u64>) {
        match parse_element_header(&stream[at..], at as u64, 8).unwrap() {
            Parsed::Done(h) => (h.id, usize::from(h.header_len), h.size),
            Parsed::NeedMore(_) => panic!("short element at {at}"),
        }
    }

    #[test]
    fn the_stream_reads_back() {
        // 4 x 2 frames of 4:2:0: 12 bytes each.
        let t = Timescale::new(1, 12_800).unwrap();
        let mut stream = header(
            4,
            2,
            fourcc(PixelFormat::YUV420P).unwrap(),
            &t,
            frame_duration_ns(25, 1),
        );
        let frames: Vec<Vec<u8>> = (0..3u8).map(|n| vec![n; 12]).collect();
        for (frame, time) in frames.iter().zip([0i64, 512, 2048]) {
            stream.extend(cluster_head(t.ticks(time).unwrap(), 12));
            stream.extend_from_slice(frame);
        }
        // CoreAV's own demuxer reads the header; it does not take
        // V_UNCOMPRESSED, so the track is listed as dropped.
        let mut source = SliceSource::new(&stream);
        let segment = probe(&mut source, &Limits::default(), &Allowlist::all()).unwrap();
        assert_eq!(segment.timestamp_scale, 78_125);
        assert_eq!(segment.dropped.len(), 1);
        assert_eq!(segment.dropped[0].codec_id, "V_UNCOMPRESSED");
        let default_duration = [0x23, 0xE3, 0x83, 0x84, 0x02, 0x62, 0x5A, 0x00];
        assert!(stream.windows(8).any(|w| w == default_duration));
        // The clusters, walked element by element.
        let (ebml, len, size) = element_at(&stream, 0);
        assert_eq!(ebml, 0x1A45_DFA3);
        let (segment_id, segment_len, unknown) = element_at(&stream, len + size.unwrap() as usize);
        assert_eq!((segment_id, unknown), (0x1853_8067, None));
        let mut at = len + size.unwrap() as usize + segment_len;
        let mut seen = Vec::new();
        while at < stream.len() {
            let (id, len, size) = element_at(&stream, at);
            let size = size.unwrap() as usize;
            if id == 0x1F43_B675 {
                let (t_id, t_len, t_size) = element_at(&stream, at + len);
                assert_eq!(t_id, 0xE7);
                let t_size = t_size.unwrap() as usize;
                let time = stream[at + len + t_len..at + len + t_len + t_size]
                    .iter()
                    .fold(0u64, |v, b| (v << 8) | u64::from(*b));
                let block = at + len + t_len + t_size;
                let (b_id, b_len, b_size) = element_at(&stream, block);
                assert_eq!((b_id, b_size), (0xA3, Some(16)));
                assert_eq!(
                    &stream[block + b_len..block + b_len + 4],
                    &[0x81, 0, 0, 0x80]
                );
                let data = &stream[block + b_len + 4..block + b_len + 16];
                seen.push((time, data.to_vec()));
                assert_eq!(block + b_len + 16, at + len + size);
            }
            at += len + size;
        }
        assert_eq!(at, stream.len());
        assert_eq!(
            seen,
            vec![
                (0, frames[0].clone()),
                (512, frames[1].clone()),
                (2048, frames[2].clone())
            ]
        );
    }

    #[test]
    fn sizes_take_the_fewest_bytes() {
        let mut out = Vec::new();
        size(&mut out, 0);
        size(&mut out, 126);
        size(&mut out, 127);
        size(&mut out, 1_658_880);
        assert_eq!(out, [0x80, 0xFE, 0x40, 0x7F, 0x39, 0x50, 0x00]);
        assert_eq!(
            fourcc(PixelFormat::Yuv {
                chroma: Chroma::Cs420,
                depth: 10
            }),
            Some(*b"Y3\x0b\x0a")
        );
        assert_eq!(
            fourcc(PixelFormat::Yuv {
                chroma: Chroma::Mono,
                depth: 8
            }),
            None
        );
    }
}
