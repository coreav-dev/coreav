//! Arbitrary input through the splitter (docs/design/split.md): it either
//! fails with an error or writes segments that the parser accepts and that
//! together hold every sample of each kept track exactly once, in order: the
//! same bytes, decode durations and composition offsets.
#![no_main]

use coreav_isobmff::{
    Allowlist, Budget, BufferedSource, Limits, Packet, Rational, SliceSource, Source, TrackKind,
    probe,
};
use coreav_isobmff_write::{AudioChoice, SplitAt, SplitOptions, VideoChoice, split_mp4};
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

/// Every packet of a track, `None` when some packet cannot be read.
fn packets(data: &[u8], limits: &Limits, stream_index: Option<usize>, kind: TrackKind) -> Option<Vec<Packet>> {
    let movie = probe(SliceSource::new(data), limits, &Allowlist::default()).expect("probe");
    let track = match stream_index {
        Some(index) => movie.tracks.iter().find(|t| t.stream_index == index),
        None => movie.tracks.iter().find(|t| t.kind == kind),
    }?;
    let mut budget = Budget::new(limits);
    movie
        .packets(track.index, SliceSource::new(data), limits, &mut budget)
        .expect("packets")
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
    // The first three bytes pick the audio and the cuts: every `step` tenths
    // of a second, or at `step` and `second` tenths.
    let Some((&[audio, step, second], input)) = data.split_first_chunk::<3>() else {
        return;
    };
    let tenths = |n: u8| Rational::new(i128::from(n), 10).expect("rational");
    let at = if second == 0 {
        SplitAt::Every(tenths(step.max(1)))
    } else {
        SplitAt::Times(vec![tenths(step), tenths(step.saturating_add(second))])
    };
    let options = SplitOptions {
        video: VideoChoice::First,
        audio: match audio % 3 {
            0 => AudioChoice::First,
            1 => AudioChoice::None,
            _ => AudioChoice::Stream(usize::from(audio / 3)),
        },
        at,
    };
    let collected = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
    let source = BufferedSource::new(SliceSource::new(input));
    let result = split_mp4(source, &limits, &Allowlist::default(), &options, |index| {
        let mut all = collected.borrow_mut();
        assert_eq!(index, all.len());
        all.push(Vec::new());
        Ok(Sink(Rc::clone(&collected)))
    });
    let Ok(summary) = result else {
        return;
    };
    let outputs = collected.borrow().clone();
    assert_eq!(summary.segments.len(), outputs.len());
    for kind in [TrackKind::Video, TrackKind::Audio] {
        let stream = summary.segments.iter().find_map(|s| match kind {
            TrackKind::Video => Some(s.video),
            _ => s.audio,
        });
        let Some(stream) = stream else { continue };
        // The source's packets: the split read every one of them.
        let source = packets(input, &limits, Some(stream.stream_index), kind).expect("source packets");
        let mut next = 0usize;
        for (segment, output) in summary.segments.iter().zip(&outputs) {
            assert_eq!(segment.bytes_written, output.len() as u64);
            let kept = match kind {
                TrackKind::Video => Some(segment.video),
                _ => segment.audio,
            };
            let Some(kept) = kept else { continue };
            assert_eq!(kept.first_sample, next as u64);
            let written = packets(output, &Limits::default(), None, kind).expect("segment packets");
            assert_eq!(written.len(), kept.samples as usize);
            for (s, o) in source[next..next + written.len()].iter().zip(&written) {
                assert_eq!(o.duration, s.duration);
                assert_eq!(o.cts - o.dts, s.cts - s.dts);
                assert_eq!(bytes(output, o), bytes(input, s));
            }
            next += written.len();
        }
        assert_eq!(next, source.len(), "every sample exactly once");
    }
});

/// A writer appending to the last collected output.
struct Sink(Rc<RefCell<Vec<Vec<u8>>>>);

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(last) = self.0.borrow_mut().last_mut() {
            last.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
