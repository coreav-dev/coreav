//! Top-level parser, box tree and every payload parser reached from `probe`,
//! then the on-demand fragment scan and the metadata derived from them.
#![no_main]

use coreav_isobmff::{Allowlist, Limits, Movie, SliceSource, parse_box_header, probe, unix_time};
use libfuzzer_sys::fuzz_target;

/// Derived values never panic, and the retained codec
/// configuration bytes stay within the limits.
fn check_metadata(movie: &Movie, limits: &Limits) {
    let _ = unix_time(movie.creation_time);
    let mut retained = 0u64;
    for track in &movie.tracks {
        let _ = track.bit_rate();
        let _ = unix_time(track.creation_time);
        let _ = unix_time(track.media_creation_time);
        if let Some(config) = &track.description.codec_config {
            let len = config.data.len() as u64;
            assert!(len <= limits.max_box_bytes);
            retained = retained.saturating_add(len);
        }
    }
    assert!(retained <= limits.max_retained_bytes);
}

fuzz_target!(|data: &[u8]| {
    let _ = parse_box_header(data, 0, data.len() as u64);
    let limits = Limits {
        max_sample_work: 1 << 20,
        max_retained_bytes: 1 << 16,
        ..Limits::default()
    };
    if let Ok(mut movie) = probe(SliceSource::new(data), &limits, &Allowlist::default()) {
        check_metadata(&movie, &limits);
        let _ = movie.scan_fragments(SliceSource::new(data), &limits);
        let _ = movie.duration_seconds();
        // The `iTunSMPB` tag reader never fails or panics.
        let _ = movie.itunes_gapless(SliceSource::new(data));
        check_metadata(&movie, &limits);
    }
});
