#![no_main]

use coreav_backend_svt_av1::{frame, output, sequence};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 16_384 {
        return;
    }
    let expected = output::Expected {
        dimensions: frame::Expected {
            width: 64,
            height: 64,
        },
        primaries: 2,
        transfer: 2,
        matrix: 2,
        range: sequence::ColorRange::Studio,
        chroma_position: sequence::ChromaPosition::Unknown,
    };
    let limits = output::Limits {
        max_packet_bytes: 16_384,
        max_buffered_bytes: 32_768,
        max_obu_bytes: 16_384,
        max_temporal_unit_bytes: 32_768,
        max_sequence_bytes: 4096,
        max_total_bytes: 65_536,
        max_obus: 1024,
        max_frames: 256,
        max_frames_per_temporal_unit: 16,
        max_samples: 64,
        max_tiles: 64,
    };
    let mut output = output::Output::new(expected, limits);
    if output.begin_segment(64).is_err() {
        return;
    }
    let seed = [
        0x12, 0, 10, 13, 0, 0, 0, 63, 248, 1, 248, 1, 248, 4, 16, 0, 128,
    ];
    if output.push(&seed).is_err() || output.next_sample().is_err() {
        return;
    }
    let chunk_size = data.first().map_or(1, |b| usize::from(*b) + 1);
    let mut ordinal = 0;
    for chunk in data.chunks(chunk_size) {
        if output.push(chunk).is_err() {
            return;
        }
        loop {
            match output.next_sample() {
                Ok(Some(sample)) => {
                    assert_eq!(sample.ordinal, ordinal);
                    ordinal += 1;
                    assert!(!sample.bytes.is_empty());
                    assert!(sample.bytes.len() <= limits.max_temporal_unit_bytes);
                }
                Ok(None) => break,
                Err(_) => {
                    assert_eq!(output.next_sample(), Err(output::Error::Failed));
                    assert_eq!(output.push(&[]), Err(output::Error::Failed));
                    return;
                }
            }
        }
    }
    if let Ok(sample) = output.end_segment(ordinal + 1) {
        assert_eq!(sample.ordinal, ordinal);
    }
    let _ = output.configuration();
    let _ = output.sequence();
});
