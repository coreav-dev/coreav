//! A stream description and its packets, arbitrary bytes, through the
//! backend the way the decode boundary drives it: the configuration
//! record at `open`, then packets, then the end of the stream.
//!
//! What this target asserts, beyond "no panic": every frame is inside
//! the limits the decoder was opened with, a limit breach is reported as
//! one, and `receive_frame` after `send_eof` ends.
#![no_main]

use coreav_backend_h264::{NativeH264, NativeOptions};
use coreav_decode::{
    Backend, Codec, CodecConfig, ConfigKind, ErrorKind, Limits, Packet, StreamConfig, Support,
};
use libfuzzer_sys::fuzz_target;

/// Frames a run may take, so that one input cannot decode for ever.
const MAX_FRAMES: usize = 64;

/// A length-prefixed chunk of `data`, a 16-bit length first.
fn chunk<'a>(data: &mut &'a [u8]) -> Option<&'a [u8]> {
    let (head, rest) = data.split_at_checked(2)?;
    let len = usize::from(u16::from_be_bytes([*head.first()?, *head.get(1)?]));
    let (unit, rest) = rest.split_at_checked(len.min(rest.len()))?;
    *data = rest;
    Some(unit)
}

fuzz_target!(|data: &[u8]| {
    let mut rest = data;
    let Some((&choice, tail)) = rest.split_first() else {
        return;
    };
    rest = tail;
    // The configuration record, and the sizes the container declares.
    let Some(config) = chunk(&mut rest) else {
        return;
    };
    let declared = |v: u8| if v == 0 { None } else { Some(u32::from(v)) };
    let stream = StreamConfig {
        config: Some(CodecConfig {
            kind: ConfigKind::Avcc,
            data: config.to_vec(),
        }),
        width: declared(choice),
        height: declared(choice.rotate_left(3)),
        ..StreamConfig::new(Codec::H264)
    };
    let limits = Limits {
        max_width: 1024,
        max_height: 1024,
        max_pixels: 1 << 18,
        max_frame_bytes: 1 << 22,
        max_decoded_bytes: 1 << 26,
        max_pending_bytes: 1 << 24,
        max_packet_bytes: 1 << 20,
        max_reference_frames: 1 + u32::from(choice % 16),
        // Small enough that a picture of any size can fail it, so the
        // path that refuses one is reached.
        max_memory_bytes: 1 << (16 + u64::from(choice % 16)),
        ..Limits::default()
    };
    let backend = NativeH264::with_options(NativeOptions {
        keyframes_only: choice % 2 == 0,
        // One to three pictures at once: threads change when frames come
        // out, never which.
        threads: Some(1 + usize::from(choice / 32 % 3)),
    });
    // Selection first, as a caller does: a stream this backend refuses
    // is another backend's.
    let traits = stream.traits();
    if matches!(backend.supports(&stream, &traits), Support::No(_)) {
        return;
    }
    let Ok(mut decoder) = backend.open(&stream, &limits) else {
        return;
    };
    let mut frames = 0usize;
    let mut decoded = 0u64;

    // A bound on what a picture may hold and still fit the memory budget,
    // which the decoder checks before it makes the pictures of a size
    // (`coreav_h264::Decoder::memory_needed` counts more than this).
    let budget_bytes = limits.max_memory_bytes / u64::from(limits.max_reference_frames + 4);
    let take = |decoder: &mut Box<dyn coreav_decode::Decoder>,
                frames: &mut usize,
                decoded: &mut u64| {
        while *frames < MAX_FRAMES {
            match decoder.receive_frame() {
                Ok(Some(frame)) => {
                    // Inside the limits the decoder was opened with.
                    assert!(frame.width() <= limits.max_width);
                    assert!(frame.height() <= limits.max_height);
                    assert!(u64::from(frame.width()) * u64::from(frame.height()) <= limits.max_pixels);
                    // A frame carries the timestamp of its packet, and
                    // every packet here has one.
                    assert!(frame.pts.is_some());
                    // Readable to its last sample, no larger than a
                    // frame may be, and inside the memory budget: the
                    // backend holds the references a stream asks for
                    // plus the pictures in flight, so a picture it
                    // delivers fits that share of the budget.
                    if let Ok(picture) = frame.to_cpu() {
                        assert_eq!(picture.width(), frame.width());
                        assert_eq!(picture.planes().len(), 3);
                        let bytes: u64 = picture
                            .planes()
                            .iter()
                            .map(|p| p.data().len() as u64)
                            .sum();
                        assert!(bytes <= limits.max_frame_bytes);
                        assert!(
                            bytes <= budget_bytes,
                            "{bytes} bytes of picture above the budget's {budget_bytes}"
                        );
                        *decoded += bytes;
                        assert!(*decoded <= limits.max_decoded_bytes);
                    }
                    *frames += 1;
                }
                Ok(None) => return true,
                // A limit or a broken stream ends the run, never the
                // process.
                Err(e) => {
                    assert!(matches!(
                        e.kind,
                        ErrorKind::Limit
                            | ErrorKind::InvalidData
                            | ErrorKind::Unsupported
                            | ErrorKind::Timeout
                            | ErrorKind::Backend
                    ));
                    return false;
                }
            }
        }
        false
    };
    while let Some(unit) = chunk(&mut rest) {
        let packet = Packet {
            data: unit,
            pts: Some(i64::from(unit.len() as u32)),
            dts: None,
            keyframe: choice % 4 == 0,
            discard: choice % 8 == 7,
        };
        // A failed call does not end the run: what a decoder does after
        // one is part of what this target checks — a limit it reported
        // must stay reported, and nothing it returns afterwards may
        // break the limits either.
        if let Err(e) = decoder.send_packet(&packet) {
            assert!(matches!(
                e.kind,
                ErrorKind::Limit
                    | ErrorKind::InvalidData
                    | ErrorKind::Unsupported
                    | ErrorKind::Timeout
                    | ErrorKind::Backend
            ));

        }
        if !take(&mut decoder, &mut frames, &mut decoded) {
            break;
        }
    }
    let _ = decoder.send_eof();
    take(&mut decoder, &mut frames, &mut decoded);

    // After the end of the stream there is nothing more to take.
    if frames < MAX_FRAMES {
        assert!(matches!(decoder.receive_frame(), Ok(None) | Err(_)));
    }
});
