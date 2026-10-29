//! A Matroska H.264 track written without a `CodecPrivate` carries its
//! parameter sets in the frames, as a byte stream (ITU-T H.264 Annex B).
//! A writer that muxed such a stream can flag every block a keyframe
//! although only some hold an IDR picture; the thumbnail then starts
//! decoding at a block that holds one (`docs/design/thumbnails.md`,
//! "H.264 without a configuration record"). The files are built with
//! `coreav-mkv`'s test helpers.
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    dead_code
)]

#[path = "../../coreav-mkv/tests/common/mod.rs"]
mod common;

use common::{
    CUES, TRACKS, cat, cluster, el, file, info, segment_data_offset, simple_block, uint,
    video_track,
};
use coreav_isobmff::{Rational, SliceSource};
use coreav_thumbnail::{ThumbnailOptions, oracle_choice};

/// Blocks of the stream, 40 ms apart.
const TICKS: u64 = 40;

/// A coded slice NAL unit: the header byte, then `first_mb_in_slice` 0 and
/// `slice_type` as Exp-Golomb codes, padded (ITU-T H.264 7.3.3).
fn slice(kind: u8, slice_type: u32) -> Vec<u8> {
    let code = slice_type + 1;
    let width = 32 - code.leading_zeros();
    let mut bits: Vec<u8> = vec![1];
    bits.extend(std::iter::repeat_n(0, (width - 1) as usize));
    bits.extend((0..width).rev().map(|i| ((code >> i) & 1) as u8));
    bits.push(1);
    while bits.len() % 8 != 0 {
        bits.push(0);
    }
    let mut out = vec![0x60 | kind];
    out.extend(
        bits.chunks(8)
            .map(|c| c.iter().fold(0u8, |v, b| (v << 1) | b)),
    );
    out
}

/// One frame as a byte stream: a start code before each NAL unit.
fn byte_stream(units: &[Vec<u8>]) -> Vec<u8> {
    units
        .iter()
        .flat_map(|u| [&[0u8, 0, 0, 1][..], u].concat())
        .collect()
}

/// A user data unregistered SEI message of `bytes` bytes (ITU-T H.264
/// D.1.6), whose payload holds no start code.
fn user_data(bytes: usize) -> Vec<u8> {
    let mut out = vec![0x06, 5];
    out.extend(std::iter::repeat_n(0xFF, bytes / 255));
    out.push((bytes % 255) as u8);
    out.extend(std::iter::repeat_n(0x42, bytes));
    out.push(0x80);
    out
}

/// An IDR picture with the parameter sets the writer left before it and
/// `sei` bytes of SEI messages, or a P picture with none of its own.
fn frame(idr: bool, sei: usize) -> Vec<u8> {
    if idr {
        let mut units = vec![
            vec![0x67, 0x42, 0xC0, 0x1E, 0x8C],
            vec![0x68, 0xCE, 0x3C, 0x80],
        ];
        if sei > 0 {
            units.push(user_data(sei));
        }
        units.push(slice(5, 7));
        byte_stream(&units)
    } else {
        byte_stream(&[slice(1, 0)])
    }
}

/// A stream of blocks for [`stream`].
struct Spec<'a> {
    frames: usize,
    /// Which blocks hold an IDR picture with its parameter sets.
    idr: &'a dyn Fn(usize) -> bool,
    /// Flag every block a keyframe, as a writer that took every frame of
    /// a byte stream for one; else flag only the blocks that hold an IDR
    /// picture, as an honest writer does.
    flag_every: bool,
    /// Bytes of SEI messages before each IDR picture's slice.
    sei: usize,
}

/// A file of `spec.frames` blocks, one Cluster each and a Cue for every
/// one. Returns it and the byte offset of each block's frame data.
fn stream(spec: &Spec) -> (Vec<u8>, Vec<u64>) {
    let (frames, idr) = (spec.frames, spec.idr);
    let tracks = el(
        TRACKS,
        &video_track(1, "V_MPEG4/ISO/AVC", 320, 240, &uint(0x23_E383, 40_000_000)),
    );
    let head = cat(&[info(Some(frames as f64 * TICKS as f64)), tracks]);
    // Each Cluster holds its Timestamp and one SimpleBlock; `inner` is
    // where the block's frame data lies in the Cluster element, past both
    // elements' IDs and sizes and the block's four-byte header (the track
    // number, the timestamp and the flags).
    let mut clusters: Vec<Vec<u8>> = Vec::new();
    let mut inner: Vec<u64> = Vec::new();
    for i in 0..frames {
        let data = frame(idr(i), spec.sei);
        let timestamp = uint(0xE7, i as u64 * TICKS);
        let block = simple_block(1, 0, spec.flag_every || idr(i), &data);
        let element = cluster(i as u64 * TICKS, std::slice::from_ref(&block));
        let cluster_head = element.len() - timestamp.len() - block.len();
        let block_head = block.len() - data.len() - 4;
        inner.push((cluster_head + timestamp.len() + block_head + 4) as u64);
        clusters.push(element);
    }
    // A Cue holds the position of its Cluster from the Segment's data.
    let mut positions = Vec::new();
    let mut at = head.len() as u64;
    for c in &clusters {
        positions.push(at);
        at += c.len() as u64;
    }
    let cues = el(
        CUES,
        &cat(&positions
            .iter()
            .enumerate()
            .map(|(i, position)| {
                el(
                    0xBB,
                    &cat(&[
                        uint(0xB3, i as u64 * TICKS),
                        el(0xB7, &cat(&[uint(0xF7, 1), uint(0xF1, *position)])),
                    ]),
                )
            })
            .collect::<Vec<_>>()),
    );
    let data = file("matroska", &[head, cat(&clusters), cues]);
    let size = data.len() - segment_data_offset("matroska", 0);
    let segment = segment_data_offset("matroska", size) as u64;
    let offsets = (0..frames)
        .map(|i| segment + positions[i] + inner[i])
        .collect();
    (data, offsets)
}

/// The byte offsets `thumbnail` would decode from and present, or `None`
/// when it makes no image.
fn choice(data: &[u8], at: Rational, exact: bool) -> Option<(u64, u64)> {
    let mut options = ThumbnailOptions::at(at);
    options.exact = exact;
    oracle_choice(SliceSource::new(data), &options)
        .ok()
        .map(|c| (c.start, c.target))
}

fn ms(n: i128) -> Rational {
    Rational::new(n, 1000).unwrap()
}

#[test]
fn a_block_that_holds_no_idr_picture_is_no_cut_point() {
    // 12 blocks, every one flagged a keyframe, an IDR picture with its
    // parameter sets in every fourth.
    let every_fourth = |i: usize| i % 4 == 2;
    let (data, offsets) = stream(&Spec {
        frames: 12,
        idr: &every_fourth,
        flag_every: true,
        sei: 0,
    });
    // Keyframe mode: the IDR picture at or before the time, not the block
    // the flags claim.
    for (at, block) in [(80, 2), (120, 2), (240, 6), (300, 6), (400, 10)] {
        assert_eq!(
            choice(&data, ms(at), false),
            Some((offsets[block], offsets[block])),
            "keyframe mode at {at} ms"
        );
    }
    // Exact mode decodes from that IDR picture to the frame asked for.
    assert_eq!(choice(&data, ms(120), true), Some((offsets[2], offsets[3])));
    // Before the first IDR picture, the first one after it: its frame is
    // the one a decoder gives first, the pictures before it having no
    // parameter sets.
    for exact in [false, true] {
        assert_eq!(
            choice(&data, ms(0), exact),
            Some((offsets[2], offsets[2])),
            "at 0 ms, exact {exact}"
        );
    }
}

#[test]
fn a_frame_whose_slice_is_past_the_bytes_read_keeps_the_flag() {
    // 9,000 bytes of SEI messages before each IDR picture's slice: more
    // than the cut-point check reads, so it cannot tell whether the frame
    // starts decoding, and the block keeps the flag Matroska states.
    let every_fourth = |i: usize| i % 4 == 2;
    let spec = |flag_every, sei| Spec {
        frames: 12,
        idr: &every_fourth,
        flag_every,
        sei,
    };
    // Flagged honestly, the IDR pictures are chosen either way.
    for sei in [0, 9_000] {
        let (data, offsets) = stream(&spec(false, sei));
        assert_eq!(
            choice(&data, ms(120), false),
            Some((offsets[2], offsets[2])),
            "honest flags, {sei} bytes of SEI"
        );
        assert_eq!(
            choice(&data, ms(120), true),
            Some((offsets[2], offsets[3])),
            "honest flags, exact, {sei} bytes of SEI"
        );
    }
    // Flagged throughout, an unreadable frame keeps the flag while the P
    // pictures, whose slices are in the bytes read, are still rejected:
    // the search back from the time passes block 3 and stops at the IDR
    // picture of block 2. An image, not a refusal.
    let (data, offsets) = stream(&spec(true, 9_000));
    assert_eq!(
        choice(&data, ms(120), false),
        Some((offsets[2], offsets[2]))
    );
    assert_eq!(choice(&data, ms(120), true), Some((offsets[2], offsets[3])));
}

#[test]
fn a_stream_without_an_idr_picture_has_no_cut_point() {
    let (data, _) = stream(&Spec {
        frames: 6,
        idr: &|_| false,
        flag_every: true,
        sei: 0,
    });
    assert_eq!(choice(&data, ms(0), false), None);
    assert_eq!(choice(&data, ms(0), true), None);
}
