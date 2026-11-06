#![doc = "Packed decoder output reuses allocation capacity without extra budget."]
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

#[path = "../../coreav-h264/tests/common/streams.rs"]
mod streams;

use coreav_backend_h264::{NativeH264, NativeOptions};
use coreav_counting_alloc::{CountingAllocator, measure};
use coreav_decode::{Backend, Codec, Limits, Packet, StreamConfig};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

#[test]
fn steady_output_reuses_planes_within_the_existing_budget() {
    let backend = NativeH264::with_options(NativeOptions {
        threads: Some(1),
        ..NativeOptions::default()
    });
    let mut limits = Limits {
        max_pending_bytes: 256 * 256 * 3 / 2,
        ..Limits::default()
    };
    limits.max_memory_bytes = NativeH264::memory_needed(256, 256, 1, &limits);
    let mut decoder = backend
        .open(&StreamConfig::new(Codec::H264), &limits)
        .unwrap();
    let mut unit = streams::nal(3, 7, streams::sps_with_max_refs(16, 16, 1));
    unit.extend(streams::nal(3, 8, streams::pps()));
    unit.extend(streams::nal(
        3,
        5,
        streams::idr_pcm_slice(256, |addr| [1 + (addr % 200) as u8; 384]),
    ));
    decoder
        .send_packet(&Packet {
            data: &unit,
            pts: Some(-1),
            dts: Some(-1),
            keyframe: true,
            discard: false,
        })
        .unwrap();
    let mut receive = |pts| {
        decoder
            .send_packet(&Packet {
                data: &unit,
                pts: Some(pts),
                dts: Some(pts),
                keyframe: true,
                discard: false,
            })
            .unwrap();
        let frame = decoder.receive_frame().unwrap().unwrap();
        assert_eq!(frame.pts, Some(pts - 1));
        frame.pixels.into_cpu().unwrap()
    };
    for pts in 0..8 {
        drop(receive(pts));
    }
    let ((), counters) = measure(|| {
        for pts in 8..40 {
            let picture = receive(pts);
            assert_eq!(&picture.row(0, 0).unwrap()[..16], &[1; 16]);
        }
    });
    coreav_counting_alloc::report("packed-output-reuse", &counters);
    assert!(
        counters.bytes < 32 * 256 * 256 * 3 / 2,
        "steady decoding allocated at least one packed frame per output: {counters}"
    );
}
