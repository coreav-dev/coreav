//! Probe, then the scan of every track's spans, bytes and picture sizes
//! (`Segment::scan_tracks`): block listing, header stripping and the frame
//! readers of `coreav-isobmff` on Matroska frames.
#![no_main]

use coreav_mkv::{Allowlist, Limits, SliceSource, probe};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_block_work: 1 << 16,
        ..Limits::default()
    };
    let Ok(segment) = probe(SliceSource::new(data), &limits, &Allowlist::all()) else {
        return;
    };
    if let Ok(scan) = segment.scan_tracks(SliceSource::new(data), &limits) {
        assert_eq!(scan.tracks.len(), segment.tracks.len());
        for track in &scan.tracks {
            if let Some(duration) = track.duration_ns() {
                assert!(duration >= 0);
            }
        }
    }
});
