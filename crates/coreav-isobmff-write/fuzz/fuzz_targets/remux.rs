//! Arbitrary input through the full-file remux (`docs/design/remux.md`): it
//! either fails with an error or writes an output that the parser accepts,
//! with the movie box first, whose streams hold every sample of the kept
//! source streams: the same bytes, decode durations and composition
//! offsets, in order.
#![no_main]

use coreav_isobmff::{
    Allowlist, Budget, BufferedSource, Limits, Packet, SliceSource, Source, probe,
};
use coreav_isobmff_write::{Metadata, OnUnsupported, RemuxOptions, remux_mp4};
use libfuzzer_sys::fuzz_target;

fn packets(data: &[u8], limits: &Limits, stream_index: Option<usize>, nth: usize) -> Vec<Packet> {
    let movie = probe(SliceSource::new(data), limits, &Allowlist::default()).expect("probe");
    let track = match stream_index {
        Some(index) => movie.tracks.iter().find(|t| t.stream_index == index),
        None => movie.tracks.get(nth),
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
    // The first byte picks the options: drop or fail on unsupported
    // streams, and every stream or only the stream it names.
    let Some((&choice, input)) = data.split_first() else {
        return;
    };
    let options = RemuxOptions {
        streams: (choice & 2 != 0).then(|| vec![usize::from(choice >> 2)]),
        on_unsupported: if choice & 1 == 0 {
            OnUnsupported::Drop
        } else {
            OnUnsupported::Fail
        },
        metadata: Metadata::Copy,
        left_out_covers: Vec::new(),
    };
    let mut out = Vec::new();
    let source = BufferedSource::new(SliceSource::new(input));
    let Ok(summary) = remux_mp4(source, &limits, &Allowlist::default(), &options, &mut out) else {
        return;
    };
    assert_eq!(summary.bytes_written, out.len() as u64);
    assert_eq!(&out[4..8], b"ftyp");
    let movie = probe(SliceSource::new(&out), &Limits::default(), &Allowlist::default())
        .expect("the output parses");
    assert!(!movie.fragmented);
    assert_eq!(movie.tracks.len(), summary.tracks.len());
    for (nth, kept) in summary.tracks.iter().enumerate() {
        let written = packets(&out, &Limits::default(), None, nth);
        let source = packets(input, &limits, Some(kept.stream_index), 0);
        assert_eq!(written.len(), kept.samples as usize);
        assert_eq!(written.len(), source.len());
        for (a, b) in source.iter().zip(&written) {
            assert_eq!(b.duration, a.duration);
            assert_eq!(b.cts - b.dts, a.cts - a.dts);
            assert_eq!(b.keyframe, a.keyframe);
            assert_eq!(bytes(&out, b), bytes(input, a));
        }
    }
});
