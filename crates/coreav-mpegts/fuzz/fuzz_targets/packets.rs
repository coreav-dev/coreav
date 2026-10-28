//! Probe, then every packet: PES reassembly across transport packets,
//! continuity, audio frames split across PES packets, keyframes from the
//! coded data; in the default listing and in the listing as `ffprobe`
//! gives it (PES packets of coded video read in pieces).
#![no_main]

use coreav_mpegts::{Allowlist, Limits, SliceSource, probe};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_resync_bytes: 4096,
        max_packet_bytes: 1 << 20,
        ..Limits::default()
    };
    let Ok(ts) = probe(SliceSource::new(data), &limits, &Allowlist::all()) else {
        return;
    };
    for as_ffprobe in [false, true] {
        let mut listing = ts.packets(SliceSource::new(data), &limits);
        if as_ffprobe {
            listing = listing.listing_as_ffprobe();
        }
        while let Some(packet) = listing.next() {
            let Ok(packet) = packet else { break };
            assert!(packet.stream < listing.streams().len());
            assert!(packet.data.len() <= data.len());
            if let Some(pos) = packet.pos {
                assert!(pos < data.len() as u64);
            }
        }
    }
});
