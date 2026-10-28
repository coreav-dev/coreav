//! Arbitrary input through audio extraction: it either
//! fails with an error or writes an output that the parser accepts, whose
//! only track holds the source track's samples: the same bytes and decode
//! durations, in order.
#![no_main]

use coreav_isobmff::{Allowlist, Budget, BufferedSource, Limits, Packet, SliceSource, Source, probe};
use coreav_isobmff_write::{TrackChoice, extract_audio};
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
    // The first byte picks the track: the first audio track, or a stream.
    let (choice, input) = match data.split_first() {
        None => return,
        Some((0, rest)) => (TrackChoice::FirstAudio, rest),
        Some((n, rest)) => (TrackChoice::Stream(usize::from(*n) - 1), rest),
    };
    let mut out = Vec::new();
    let source = BufferedSource::new(SliceSource::new(input));
    let Ok(summary) = extract_audio(source, &limits, &Allowlist::default(), choice, &mut out)
    else {
        return;
    };
    assert_eq!(summary.bytes_written, out.len() as u64);
    let written = packets(&out, &Limits::default(), None);
    let source = packets(input, &limits, Some(summary.stream_index));
    assert_eq!(written.len(), summary.samples as usize);
    assert_eq!(written.len(), source.len());
    for (a, b) in source.iter().zip(&written) {
        assert_eq!(b.duration, a.duration);
        assert_eq!(bytes(&out, b), bytes(input, a));
    }
});
