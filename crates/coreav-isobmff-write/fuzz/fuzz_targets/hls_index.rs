//! Arbitrary bytes as an HLS index (docs/design/hls.md): opening it, its
//! playlists, init segments and media segments either fail with an error
//! or succeed, and every media segment copies only ranges inside the
//! source length the index records.
#![no_main]

use coreav_isobmff::{Limits, SliceSource};
use coreav_isobmff_write::hls::HlsIndex;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_table_bytes: 1 << 20,
        ..Limits::default()
    };
    let Ok(index) = HlsIndex::open(SliceSource::new(data), &limits) else {
        return;
    };
    let _ = index.master_playlist();
    for track in index.tracks() {
        let _ = index.media_playlist(track.rendition);
        let _ = index.init_segment(track.rendition);
        for n in 0..track.segments().len().min(64) as u32 {
            let Ok(segment) = index.media_segment(
                track.rendition,
                n,
                SliceSource::new(data),
                SliceSource::new(data),
                &limits,
            )
            else {
                continue;
            };
            let mut header_len = 0u64;
            for range in segment.ranges() {
                assert!(range.len > 0 && range.end() <= index.source_len());
                header_len += range.len;
            }
            assert!(segment.len() >= header_len);
            // A source of the recorded length, when small enough to hold.
            if index.source_len() <= 1 << 16 {
                let source = vec![0u8; index.source_len() as usize];
                let mut out = Vec::new();
                let written = segment.write(SliceSource::new(&source), &mut out).expect("write");
                assert_eq!(written, out.len() as u64);
            }
        }
    }
});
