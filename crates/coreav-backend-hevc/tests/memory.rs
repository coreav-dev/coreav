//! Counts every native decoder thread on generated pictures, without an external oracle.
#![cfg(feature = "native-hevc")]

use coreav_backend_hevc::{NativeHevc, NativeOptions};
use coreav_counting_alloc::{CountingAllocator, measure_process, report};
use coreav_decode::{Codec, Decoder, Error, ErrorKind, Limits, Packet, Policy, StreamConfig};
use coreav_frame::Pixels;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[path = "../../coreav-hevc/test-support/pcm.rs"]
mod pcm;
#[path = "../../coreav-hevc/test-support/raster.rs"]
mod raster;
#[path = "../../coreav-hevc/test-support/writer.rs"]
mod writer;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const FRAME_BYTES: u64 = 16 * 512 * 3 / 2;
const HEAP_BOUND: u64 = 32 * 1024 * 1024;
const STACK_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Default, PartialEq, Eq)]
struct Snapshot {
    frames: [Option<(Option<i64>, u64)>; 2],
    error: Option<(ErrorKind, u64)>,
}

fn digest(bytes: &[u8], seed: u64) -> u64 {
    bytes.iter().fold(seed, |value, byte| {
        value.wrapping_mul(257).wrapping_add(u64::from(*byte))
    })
}

fn annex_b(nals: &[Vec<u8>]) -> Vec<u8> {
    let mut packet = Vec::new();
    for nal in nals {
        packet.extend_from_slice(&[0, 0, 0, 1]);
        packet.extend_from_slice(nal);
    }
    packet
}

fn drain(
    decoder: &mut dyn Decoder,
    snapshot: &mut Snapshot,
    geometry: (u32, u32, u8),
) -> std::result::Result<(), Error> {
    while let Some(frame) = decoder.receive_frame()? {
        let Pixels::Cpu(picture) = frame.pixels else {
            return Err(Error::new(
                ErrorKind::Backend,
                "generated HEVC frame is not on CPU",
            ));
        };
        if (picture.width(), picture.height(), picture.format().depth()) != geometry {
            return Err(Error::new(
                ErrorKind::Backend,
                "generated HEVC frame geometry changed",
            ));
        }
        let hash = picture
            .planes()
            .iter()
            .fold(0, |hash, plane| digest(plane.data(), hash));
        let entry = snapshot
            .frames
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or_else(|| Error::new(ErrorKind::Backend, "extra generated HEVC frame"))?;
        *entry = Some((frame.pts, hash));
    }
    Ok(())
}

fn decode(
    packets: &[Vec<u8>],
    threads: usize,
    limits: &Limits,
    eager: bool,
    geometry: (u32, u32, u8),
) -> Snapshot {
    let mut snapshot = Snapshot::default();
    let backend = NativeHevc::with_options(NativeOptions {
        threads: Some(threads),
    });
    let result = (|| {
        let mut decoder = coreav_decode::open(
            &StreamConfig::new(Codec::Hevc),
            &Policy::default(),
            limits,
            &[&backend],
        )?
        .decoder;
        let result = (|| {
            for (index, data) in packets.iter().enumerate() {
                decoder.send_packet(&Packet {
                    data,
                    pts: Some(index as i64),
                    dts: Some(index as i64),
                    keyframe: true,
                    discard: false,
                })?;
                if eager {
                    drain(&mut *decoder, &mut snapshot, geometry)?;
                }
            }
            decoder.send_eof()?;
            drain(&mut *decoder, &mut snapshot, geometry)
        })();
        if result.is_err() {
            let _ = drain(&mut *decoder, &mut snapshot, geometry);
        }
        result
    })();
    if let Err(error) = result {
        snapshot.error = Some((error.kind, digest(error.message.as_bytes(), 0)));
    }
    snapshot
}

#[test]
fn generated_raster_native_open_bounds_all_thread_heap_and_preserves_cold_admission() -> Result<()>
{
    let tiny = pcm::pcm_fixture(8, 8, false)?;
    let packets = [annex_b(
        &tiny.sets.into_iter().chain([tiny.nal]).collect::<Vec<_>>(),
    )];
    let tiny_limits = Limits {
        max_memory_bytes: HEAP_BOUND,
        ..Limits::default()
    };
    let (actual, counters) =
        measure_process(|| decode(&packets, 1, &tiny_limits, false, (8, 8, 8)));
    let hash = tiny.planes.iter().flatten().fold(0u64, |value, sample| {
        value.wrapping_mul(257).wrapping_add(u64::from(*sample))
    });
    assert_eq!(
        actual,
        Snapshot {
            frames: [Some((Some(0), hash)), None],
            error: None
        }
    );
    report("hevc-generated-tiny-cold", &counters);
    assert!(counters.calls() > 0);
    assert_eq!(counters.trough_bytes, 0);
    assert_eq!(counters.live_bytes, 0);
    assert!(counters.peak_bytes <= HEAP_BOUND, "{counters}");
    for predicted in [false, true] {
        let nals = raster::stream(predicted)?;
        let packets = [annex_b(&nals[..35]), annex_b(&nals[35..])];
        let limits = Limits {
            max_packet_bytes: packets.iter().map(Vec::len).max().unwrap_or(0) as u64,
            max_frame_bytes: FRAME_BYTES,
            max_pending_bytes: 4 * FRAME_BYTES,
            max_memory_bytes: HEAP_BOUND,
            ..Limits::default()
        };
        let serial = decode(&packets, 1, &limits, false, (16, 512, 8));
        assert!(serial.error.is_none(), "{serial:?}");
        assert!(serial.frames.iter().all(Option::is_some));
        let (mut low, mut high) = (0, HEAP_BOUND);
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if decode(
                &packets,
                1,
                &Limits {
                    max_memory_bytes: middle,
                    ..limits.clone()
                },
                false,
                (16, 512, 8),
            )
            .error
            .is_none()
            {
                high = middle;
            } else {
                low = middle;
            }
        }
        for memory in [low, high, high + 1] {
            let bounded = Limits {
                max_memory_bytes: memory,
                ..limits.clone()
            };
            let expected = decode(&packets, 1, &bounded, false, (16, 512, 8));
            for threads in [2, 4, 8] {
                assert_eq!(
                    decode(&packets, threads, &bounded, false, (16, 512, 8)),
                    expected,
                    "predicted={predicted} memory={memory} threads={threads}"
                );
            }
        }
        for threads in [1usize, 2, 4, 8] {
            for eager in [false, true] {
                let stacks = (threads as u64 - 1) * STACK_BYTES;
                let bounded = Limits {
                    max_memory_bytes: HEAP_BOUND + stacks,
                    ..limits.clone()
                };
                let (actual, counters) =
                    measure_process(|| decode(&packets, threads, &bounded, eager, (16, 512, 8)));
                report(
                    &format!("hevc-raster-predicted-{predicted}-threads-{threads}-eager-{eager}"),
                    &counters,
                );
                assert_eq!(actual, serial);
                assert!(counters.calls() > 0);
                assert_eq!(counters.trough_bytes, 0);
                assert_eq!(counters.live_bytes, 0);
                assert!(
                    counters.peak_bytes + stacks <= bounded.max_memory_bytes,
                    "{counters}"
                );
            }
        }
    }
    Ok(())
}
