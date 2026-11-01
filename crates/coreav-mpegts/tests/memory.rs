//! The probe and the packet listing hold nothing once they return, measured
//! with the test-only counting allocator: a fuzz target that runs
//! them on input after input must not grow, so a listing run many times
//! peaks where one run peaks and leaves no live bytes, also when it ends
//! on an error.
// Test-only helpers outside `#[test]` functions index their own buffers.
#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

mod common;

use common::{Mux, adts, h264_pps, h264_sps};
use coreav_counting_alloc::{Counters, CountingAllocator, measure, report};
use coreav_mpegts::{Allowlist, Limits, SliceSource, probe};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

/// The `packets` fuzz target's limits.
fn fuzz_limits() -> Limits {
    Limits {
        max_resync_bytes: 4096,
        max_packet_bytes: 1 << 20,
        ..Limits::default()
    }
}

/// What the `packets` fuzz target does with one input: the probe, then
/// every packet until the first error. The packets listed, and whether the
/// listing ended on an error.
fn listing(data: &[u8], limits: &Limits) -> (usize, bool) {
    let ts = probe(SliceSource::new(data), limits, &Allowlist::all()).unwrap();
    let mut count = 0;
    for packet in ts.packets(SliceSource::new(data), limits) {
        let Ok(_packet) = packet else {
            return (count, true);
        };
        count += 1;
    }
    (count, false)
}

/// An H.264 stream in PES packets of no stated length, and AAC beside it.
fn stream() -> Vec<u8> {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x100), (0x0F, 0x101)]);
    for i in 0..40u64 {
        let mut unit = vec![0, 0, 0, 1, 0x09, 0x10];
        if i % 10 == 0 {
            unit.extend([0, 0, 0, 1]);
            unit.extend(h264_sps(320, 240, Some((1, 50))));
            unit.extend([0, 0, 0, 1]);
            unit.extend(h264_pps());
        }
        unit.extend([0, 0, 1, if i % 10 == 0 { 0x65 } else { 0x41 }, 0x88]);
        unit.resize(unit.len() + 2_000 + 37 * i as usize, 0x42);
        mux.pes(0x100, 0xE0, 90_000 + 3_600 * i, &unit, false);
        mux.pes(
            0x101,
            0xC0,
            90_000 + 1_920 * i,
            &adts(200 + i as usize),
            true,
        );
    }
    mux.out
}

/// One listing, then the same listing `runs` times in one region; and
/// whether it ends on an error.
fn once_and_many(data: &[u8], limits: &Limits, runs: usize) -> (Counters, Counters, bool) {
    // A first run outside the regions, so that nothing set up once per
    // process counts.
    let (listed, failed) = listing(data, limits);
    assert!(listed > 0);
    let (again, one) = measure(|| listing(data, limits));
    assert_eq!(again, (listed, failed));
    let (total, many) = measure(|| (0..runs).map(|_| listing(data, limits).0).sum::<usize>());
    assert_eq!(total, listed * runs);
    (one, many, failed)
}

fn holds_nothing(what: &str, one: &Counters, many: &Counters) {
    report(&format!("{what}/one"), one);
    report(&format!("{what}/many"), many);
    assert!(one.calls() > 0, "{what}: measured nothing");
    // Nothing is left behind, and nothing freed was allocated before.
    for counters in [one, many] {
        assert_eq!(counters.live_bytes, 0, "{what}: {counters}");
        assert_eq!(counters.trough_bytes, 0, "{what}: {counters}");
    }
    // Running again does not raise the high-water mark.
    assert_eq!(many.peak_bytes, one.peak_bytes, "{what}: {one} then {many}");
}

#[test]
fn a_listing_run_again_and_again_holds_nothing() {
    let (one, many, failed) = once_and_many(&stream(), &fuzz_limits(), 200);
    assert!(!failed);
    holds_nothing("listing", &one, &many);
}

#[test]
fn a_listing_that_ends_on_an_error_holds_nothing() {
    let data = stream();
    // A PES packet over the bound ends the listing with an error.
    let limits = Limits {
        max_packet_bytes: 3_000,
        ..fuzz_limits()
    };
    let (one, many, failed) = once_and_many(&data, &limits, 200);
    assert!(failed);
    holds_nothing("listing-error", &one, &many);
    // Cut mid-packet, past the tables, and garbage in place of packets
    // from there: the listing resynchronises within its bound, then fails.
    let mut cut = data[..data.len() / 2 + 77].to_vec();
    cut.extend((0..8_000u32).map(|i| (i * 7 + 3) as u8 & 0x3F));
    let (one, many, failed) = once_and_many(&cut, &fuzz_limits(), 200);
    assert!(failed);
    holds_nothing("listing-cut", &one, &many);
}

#[test]
fn the_room_pes_buffers_start_with_stays_within_the_buffered_bytes_together() {
    // 64 streams whose first PES packets each state the longest length
    // and come before the tables, probed with the defaults and listed
    // with one stream and 1 MiB held at once: the buffers those packets
    // start must not reserve more, together, than the listing may hold.
    let pids: Vec<u16> = (0x100..0x140).collect();
    let mut mux = Mux::default();
    for &pid in &pids {
        let mut payload = vec![0, 0, 1, 0xC0, 0xFF, 0xFF, 0x80, 0x00, 0x00];
        payload.resize(184, 0x55);
        mux.packet(pid, true, &payload);
    }
    mux.pat(&[(1, 0x1000)]);
    let streams: Vec<(u8, u16)> = pids.iter().map(|&pid| (0x0F, pid)).collect();
    mux.pmt(0x1000, 1, &streams);
    let data = mux.out;
    let ts = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    assert_eq!(ts.streams.len(), pids.len());
    let max_buffered: u64 = 1 << 20;
    let limits = Limits {
        max_streams: 1,
        max_buffered_bytes: max_buffered,
        ..Limits::default()
    };
    let (_, counters) = measure(|| ts.packets(SliceSource::new(&data), &limits).count());
    assert!(
        counters.peak_bytes < max_buffered + (512 << 10),
        "peak {} with {max_buffered} bytes held at most",
        counters.peak_bytes
    );
}
