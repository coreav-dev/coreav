//! Arbitrary input through the HLS packager (docs/design/hls.md): it either
//! fails with an error or writes an index that opens, whose init and media
//! segments, concatenated per rendition, the parser accepts, with the
//! source track's samples from the first one a presented sample needs to
//! its last presented one: the same bytes, decode durations and
//! composition offsets, in order.
#![no_main]

use coreav_isobmff::{Allowlist, Budget, Limits, Packet, Rational, SliceSource, Source, probe};
use coreav_isobmff_write::AudioChoice;
use coreav_isobmff_write::hls::{HlsIndex, HlsOptions, build_index};
use libfuzzer_sys::fuzz_target;

fn packets(data: &[u8], limits: &Limits, stream_index: Option<usize>) -> Vec<Packet> {
    let movie = probe(SliceSource::new(data), limits, &Allowlist::default()).expect("probe");
    let track = match stream_index {
        Some(index) => movie.tracks.iter().find(|t| t.stream_index == index),
        None => movie.tracks.first(),
    }
    .expect("the track");
    let mut budget = Budget::new(limits);
    movie
        .packets(track.index, SliceSource::new(data), limits, &mut budget)
        .expect("packets")
        .map(|p| p.expect("every packet is readable"))
        .collect()
}

fn bytes(data: &[u8], packet: &Packet) -> Vec<u8> {
    let mut buf = vec![0; packet.size as usize];
    SliceSource::new(data)
        .read_exact_at(packet.offset, &mut buf)
        .expect("packet bytes");
    buf
}

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_sample_work: 1 << 20,
        max_table_bytes: 1 << 20,
        ..Limits::default()
    };
    // The first two bytes pick the audio and the target duration in tenths
    // of a second.
    let Some((&[audio, target], input)) = data.split_first_chunk::<2>() else {
        return;
    };
    let options = HlsOptions {
        audio: if audio % 2 == 0 {
            AudioChoice::First
        } else {
            AudioChoice::None
        },
        segment_duration: Rational::new(i128::from(target.max(1)), 10).expect("rational"),
        ..HlsOptions::default()
    };
    let Ok(index_bytes) = build_index(
        SliceSource::new(input),
        &limits,
        &Allowlist::default(),
        &options,
    ) else {
        return;
    };
    let index = HlsIndex::open(SliceSource::new(&index_bytes), &limits).expect("the index opens");
    let _ = index.master_playlist();
    for track in index.tracks() {
        let r = track.rendition;
        index.media_playlist(r).expect("playlist");
        let mut out = index.init_segment(r).expect("init segment");
        for n in 0..track.segments().len() as u32 {
            let segment = index
                .media_segment(
                    r,
                    n,
                    SliceSource::new(&index_bytes),
                    SliceSource::new(data),
                    &limits,
                )
                .expect("segment");
            segment
                .write(SliceSource::new(input), &mut out)
                .expect("segment bytes");
        }
        let written = packets(&out, &Limits::default(), None);
        let source = packets(input, &limits, Some(track.stream_index as usize));
        // The copied samples are a run of the source's; the ones left out
        // are unpresented.
        let first = (0..=source.len().saturating_sub(written.len()))
            .find(|&k| {
                source[..k].iter().all(|p| !p.presented)
                    && written.first().is_none_or(|w| {
                        source.get(k).is_some_and(|s| bytes(input, s) == bytes(&out, w))
                    })
            })
            .expect("the copied samples are a run of the source's");
        let source = &source[first..(first + written.len()).min(source.len())];
        assert_eq!(written.len(), source.len());
        for (a, b) in source.iter().zip(&written) {
            assert_eq!(b.duration, a.duration);
            assert_eq!(b.keyframe, a.keyframe);
            assert_eq!(b.cts - b.dts, a.cts - a.dts);
            assert_eq!(bytes(&out, b), bytes(input, a));
        }
    }
});
