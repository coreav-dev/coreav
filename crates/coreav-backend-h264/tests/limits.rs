//! The four decode limits the boundary leaves to its backends
//! (`max_decoded_bytes`, `max_pending_bytes`, `max_reference_frames`,
//! `max_memory_bytes`; `docs/design/limits.md`), pinned at their boundary
//! on the native H.264 backend. The streams are built bit by bit, so no
//! `ffmpeg` is needed: each packet is the parameter sets and one IDR
//! picture of I_PCM macroblocks.
#![allow(clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

#[path = "../../coreav-h264/tests/common/streams.rs"]
mod streams;

use coreav_backend_h264::{NativeH264, NativeOptions};
use coreav_decode::{Backend, Codec, Error, ErrorKind, Limits, Packet, StreamConfig};
use streams::{idr_pcm_slice, nal, pcm_samples, pps, sps_with_max_refs};

/// Macroblocks across and down: a 32 by 32 picture.
const MBS: (u32, u32) = (2, 2);

/// The bytes of one decoded 32 by 32 4:2:0 frame.
const FRAME: u64 = 32 * 32 + 2 * 16 * 16;

fn access_unit(max_num_ref_frames: u32) -> Vec<u8> {
    let (w, h) = MBS;
    let mut out = nal(3, 7, sps_with_max_refs(w, h, max_num_ref_frames));
    out.extend(nal(3, 8, pps()));
    out.extend(nal(3, 5, idr_pcm_slice((w * h) as usize, pcm_samples)));
    out
}

fn kind<T>(result: Result<T, Error>) -> Option<ErrorKind> {
    result.err().map(|e| e.kind)
}

/// Decodes `pictures` access units on one thread, receiving the frames
/// ready after each packet when `receive_each` and else only after the
/// end of the stream; the frames received.
fn decode(
    limits: &Limits,
    max_num_ref_frames: u32,
    pictures: i64,
    receive_each: bool,
) -> Result<usize, Error> {
    let backend = NativeH264::with_options(NativeOptions {
        threads: Some(1),
        ..NativeOptions::default()
    });
    let mut decoder = backend.open(&StreamConfig::new(Codec::H264), limits)?;
    let unit = access_unit(max_num_ref_frames);
    let mut frames = 0;
    for pts in 0..pictures {
        decoder.send_packet(&Packet {
            data: &unit,
            pts: Some(pts),
            dts: Some(pts),
            keyframe: true,
            discard: false,
        })?;
        while receive_each && decoder.receive_frame()?.is_some() {
            frames += 1;
        }
    }
    decoder.send_eof()?;
    while decoder.receive_frame()?.is_some() {
        frames += 1;
    }
    Ok(frames)
}

#[test]
fn frames_of_exactly_max_decoded_bytes_decode_and_one_byte_less_is_refused() {
    let limits = |max_decoded_bytes| Limits {
        max_decoded_bytes,
        ..Limits::default()
    };
    assert_eq!(decode(&limits(3 * FRAME), 1, 3, true).ok(), Some(3));
    assert_eq!(
        kind(decode(&limits(3 * FRAME - 1), 1, 3, true)),
        Some(ErrorKind::Limit)
    );
}

/// Frames that wait to be received are refused past the limit, not waited
/// for: the in-process backends cannot block a caller that is not
/// receiving, while the sandboxed worker waits for room
/// (`docs/design/decode.md`, "Threads and backpressure").
#[test]
fn frames_waiting_up_to_max_pending_bytes_are_kept_and_one_byte_less_is_refused() {
    let limits = |max_pending_bytes| Limits {
        max_pending_bytes,
        ..Limits::default()
    };
    assert_eq!(decode(&limits(3 * FRAME), 1, 3, false).ok(), Some(3));
    assert_eq!(
        kind(decode(&limits(3 * FRAME - 1), 1, 3, false)),
        Some(ErrorKind::Limit)
    );
    // Received as they come, one frame waits at a time.
    assert_eq!(decode(&limits(FRAME), 1, 3, true).ok(), Some(3));
}

#[test]
fn a_stream_of_max_reference_frames_decodes_and_one_more_is_refused() {
    let limits = Limits {
        max_reference_frames: 4,
        ..Limits::default()
    };
    assert_eq!(decode(&limits, 4, 1, true).ok(), Some(1));
    assert_eq!(kind(decode(&limits, 5, 1, true)), Some(ErrorKind::Limit));
}

#[test]
fn a_stream_that_fits_max_memory_bytes_decodes_and_one_byte_less_is_refused() {
    let needed = NativeH264::memory_needed(32, 32, 1, &Limits::default());
    let limits = |max_memory_bytes| Limits {
        max_memory_bytes,
        ..Limits::default()
    };
    assert_eq!(decode(&limits(needed), 1, 1, true).ok(), Some(1));
    assert_eq!(
        kind(decode(&limits(needed - 1), 1, 1, true)),
        Some(ErrorKind::Limit)
    );
}
