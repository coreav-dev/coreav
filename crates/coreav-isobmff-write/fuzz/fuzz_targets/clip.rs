//! Arbitrary input through the clip writer: it either
//! fails with an error or writes an output that the parser accepts, whose
//! tracks hold the source's kept samples: the same bytes and decode
//! durations, in order.
#![no_main]

use coreav_isobmff::{
    Allowlist, Budget, BufferedSource, Limits, Packet, Rational, SliceSource, Source, probe,
};
use coreav_isobmff_write::{AudioChoice, ClipOptions, ClipTrack, VideoChoice, clip_mp4};
use libfuzzer_sys::fuzz_target;

/// The first `count` packets of a track: the clip read no further, so a
/// damaged sample table or fragment past them may fail.
fn packets(
    data: &[u8],
    limits: &Limits,
    stream_index: Option<usize>,
    nth: usize,
    count: usize,
) -> Vec<Packet> {
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
        .take(count)
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

fn check(input: &[u8], output: &[u8], limits: &Limits, kept: &ClipTrack, nth: usize) {
    let samples = kept.samples as usize;
    let written = packets(output, &Limits::default(), None, nth, usize::MAX);
    assert_eq!(written.len(), samples);
    let first = kept.first_sample as usize;
    let source = packets(input, limits, Some(kept.stream_index), 0, first + samples);
    let run = &source[first..first + samples];
    for (a, b) in run.iter().zip(&written) {
        assert_eq!(b.duration, a.duration);
        assert_eq!(b.cts - b.dts, a.cts - a.dts);
        assert_eq!(bytes(output, b), bytes(input, a));
    }
}

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_sample_work: 1 << 20,
        max_table_bytes: 1 << 20,
        ..Limits::default()
    };
    // The first three bytes pick the audio, the start and the end, in
    // tenths of a second (an end of 0 means the end of the video).
    let Some((&[audio, start, end], input)) = data.split_first_chunk::<3>() else {
        return;
    };
    let options = ClipOptions {
        video: VideoChoice::First,
        audio: match audio % 3 {
            0 => AudioChoice::First,
            1 => AudioChoice::None,
            _ => AudioChoice::Stream(usize::from(audio / 3)),
        },
        start: Rational::new(i128::from(start), 10).expect("rational"),
        end: (end > 0).then(|| Rational::new(i128::from(end), 10).expect("rational")),
    };
    let mut out = Vec::new();
    let source = BufferedSource::new(SliceSource::new(input));
    let Ok(summary) = clip_mp4(source, &limits, &Allowlist::default(), &options, &mut out) else {
        return;
    };
    assert_eq!(summary.bytes_written, out.len() as u64);
    check(input, &out, &limits, &summary.video, 0);
    if let Some(audio) = &summary.audio {
        check(input, &out, &limits, audio, 1);
    }
});
