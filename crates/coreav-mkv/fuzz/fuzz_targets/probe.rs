//! The EBML header, the element header parser, and every metadata element
//! the probe reads (SeekHead, Info, Tracks, Attachments, and through
//! `probe_with_metadata`, a superset of `probe`, Tags and Chapters),
//! including the first frame headers of VP8, VP9, H.264 and AV1 tracks.
#![no_main]

use coreav_mkv::{Allowlist, Limits, SliceSource, parse_element_header, probe_with_metadata};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = parse_element_header(data, 0, 8);
    let limits = Limits {
        max_retained_bytes: 1 << 16,
        ..Limits::default()
    };
    if let Ok(segment) = probe_with_metadata(SliceSource::new(data), &limits, &Allowlist::all()) {
        let mut retained = 0usize;
        for track in &segment.tracks {
            retained += track.codec_private.as_ref().map_or(0, Vec::len);
            if let Some(video) = track.video {
                let _ = video.rotation();
                let _ = video.display_aspect();
            }
            if let Some(audio) = track.audio {
                let _ = audio.sample_rate();
            }
        }
        assert!(retained as u64 <= limits.max_retained_bytes);
        let _ = segment.duration_seconds();
        let _ = segment.ticks_per_second();
    }
});
