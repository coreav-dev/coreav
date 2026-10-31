//! Probe followed by packet iteration of every track, one packet at a time,
//! in sample runs and sample by sample.
#![no_main]

use coreav_isobmff::{Allowlist, Budget, BufferedSource, Limits, SliceSource, probe};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_sample_work: 1 << 20,
        ..Limits::default()
    };
    let Ok(movie) = probe(SliceSource::new(data), &limits, &Allowlist::default()) else {
        return;
    };
    for index in 0..movie.tracks.len() {
        let mut budget = Budget::new(&limits);
        let source = BufferedSource::new(SliceSource::new(data));
        if let Ok(packets) = movie.packets(index, source, &limits, &mut budget) {
            for packet in packets {
                if packet.is_err() {
                    break;
                }
            }
        }
        let mut budget = Budget::new(&limits);
        let source = BufferedSource::new(SliceSource::new(data));
        if let Ok(mut packets) = movie.packets(index, source, &limits, &mut budget) {
            while let Some(Ok(run)) = packets.next_run(u32::MAX) {
                let _ = run.sample_offset(run.count.saturating_sub(1));
                let _ = run.sample_dts(run.count.saturating_sub(1));
            }
        }
        let mut budget = Budget::new(&limits);
        let source = BufferedSource::new(SliceSource::new(data));
        if let Ok(mut packets) = movie.packets(index, source, &limits, &mut budget) {
            let mut last = 0u64;
            let _ = packets.for_each_sample(|sample| -> Result<(), coreav_isobmff::Error> {
                last = last.wrapping_add(sample.offset ^ u64::from(sample.size));
                Ok(())
            });
            std::hint::black_box(last);
        }
    }
});
