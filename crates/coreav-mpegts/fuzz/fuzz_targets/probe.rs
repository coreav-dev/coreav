//! The probe: packet size detection, resynchronisation, PAT and PMT
//! sections, PES headers, parameter sets and audio frame headers, and the
//! tail read.
#![no_main]

use coreav_mpegts::{Allowlist, Limits, SliceSource, probe};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_resync_bytes: 4096,
        ..Limits::default()
    };
    if let Ok(ts) = probe(SliceSource::new(data), &limits, &Allowlist::all()) {
        assert!(ts.first_packet < data.len() as u64);
        for stream in &ts.streams {
            if let Some(duration) = stream.duration() {
                assert!(duration >= 0);
            }
        }
    }
});
