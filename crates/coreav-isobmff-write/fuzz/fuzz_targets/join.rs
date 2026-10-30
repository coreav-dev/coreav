//! Arbitrary input through the joiner (docs/design/join.md): the input is
//! cut into two files; joining them either fails with an error or writes an
//! output that the parser accepts, whose tracks hold the first file's
//! samples and then the second's, each once and in order: the same bytes and
//! composition offsets, and the same decode durations but for each file's
//! last video sample, which may be extended.
#![no_main]

use coreav_isobmff::{Allowlist, Budget, Limits, Packet, SliceSource, Source, TrackKind, probe};
use coreav_isobmff_write::{JoinAudio, JoinOptions, VideoChoice, join_mp4};
use libfuzzer_sys::fuzz_target;

/// Every packet of the first track of `kind`, `None` when some packet
/// cannot be read.
fn packets(data: &[u8], limits: &Limits, kind: TrackKind, stream: Option<usize>) -> Option<Vec<Packet>> {
    let movie = probe(SliceSource::new(data), limits, &Allowlist::default()).ok()?;
    let track = match stream {
        Some(index) => movie.tracks.iter().find(|t| t.stream_index == index),
        None => movie.tracks.iter().find(|t| t.kind == kind),
    }?;
    let mut budget = Budget::new(limits);
    movie
        .packets(track.index, SliceSource::new(data), limits, &mut budget)
        .ok()?
        .collect::<Result<Vec<_>, _>>()
        .ok()
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
    // The first byte picks the audio, the next two where the input is cut.
    let Some((&[audio, high, low], rest)) = data.split_first_chunk::<3>() else {
        return;
    };
    let cut = (usize::from(high) << 8 | usize::from(low)).min(rest.len());
    let (a, b) = rest.split_at(cut);
    let inputs: Vec<&[u8]> = if audio & 2 == 0 { vec![a, b] } else { vec![a, a] };
    let options = JoinOptions {
        video: VideoChoice::First,
        audio: if audio & 1 == 0 {
            JoinAudio::First
        } else {
            JoinAudio::None
        },
    };
    let mut out = Vec::new();
    let Ok(summary) = join_mp4(
        inputs.iter().map(|i| SliceSource::new(i)).collect(),
        &limits,
        &Allowlist::default(),
        &options,
        &mut out,
    ) else {
        return;
    };
    assert_eq!(summary.bytes_written, out.len() as u64);
    for kind in [TrackKind::Video, TrackKind::Audio] {
        if kind == TrackKind::Audio && summary.inputs.iter().all(|i| i.audio.is_none()) {
            continue;
        }
        let written = packets(&out, &Limits::default(), kind, None).expect("output packets");
        let mut next = 0usize;
        for (input, joined) in inputs.iter().zip(&summary.inputs) {
            let kept = if kind == TrackKind::Video {
                joined.video
            } else {
                joined.audio.expect("audio")
            };
            let source = packets(input, &limits, kind, Some(kept.stream_index)).expect("input packets");
            assert_eq!(source.len(), kept.samples as usize);
            for (k, (s, o)) in source.iter().zip(&written[next..]).enumerate() {
                assert_eq!(bytes(&out, o), bytes(input, s));
                assert_eq!(o.cts - o.dts, s.cts - s.dts);
                if kind == TrackKind::Video && k + 1 == source.len() {
                    assert_eq!(u64::from(o.duration), u64::from(s.duration) + joined.extended);
                } else {
                    assert_eq!(o.duration, s.duration);
                }
            }
            next += source.len();
        }
        assert_eq!(next, written.len(), "every sample once");
    }
});
