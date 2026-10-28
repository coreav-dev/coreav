//! Probe, then every packet (clusters of known and unknown size, SimpleBlocks,
//! BlockGroups, lacing), then the seek index (Cues or a cluster scan) and a
//! seek through it.
#![no_main]

use coreav_mkv::{Allowlist, Limits, SliceSource, probe};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_block_work: 1 << 16,
        ..Limits::default()
    };
    let Ok(segment) = probe(SliceSource::new(data), &limits, &Allowlist::any()) else {
        return;
    };
    for packet in segment.packets(SliceSource::new(data), &limits) {
        let Ok(packet) = packet else { break };
        assert!(packet.offset.saturating_add(packet.stored_size) <= data.len() as u64);
        assert!(packet.track < segment.tracks.len());
        let _ = packet.pts_ns(&segment);
    }
    if let Ok(index) = segment.index(SliceSource::new(data), &limits) {
        if let Some(cluster) = index.cluster_for(None, 1_000) {
            for packet in segment.packets_from(SliceSource::new(data), &limits, cluster) {
                if packet.is_err() {
                    break;
                }
            }
        }
    }
});
