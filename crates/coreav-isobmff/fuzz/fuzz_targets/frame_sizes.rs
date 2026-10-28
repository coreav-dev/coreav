//! Arbitrary bytes through the picture size readers
//! (docs/design/frame-sizes.md): as a file, probed and every track's
//! samples scanned; as an `avcC`, `hvcC` or `av1C` record; or as one frame
//! of each format through `frames::read_frame` (length-prefixed and Annex B
//! NAL units, AV1 OBUs, VP8, VP9 and ProRes headers), with a work limit.
//! Every result is a size or `None`, never a panic, and a scanned size is
//! never smaller than the configuration's parameter set size.
#![no_main]

use coreav_isobmff::frames::{self, FrameFormat, FrameSizes};
use coreav_isobmff::{Allowlist, Budget, FourCc, Limits, SliceSource, describe_config_record, probe};
use libfuzzer_sys::fuzz_target;

const FORMATS: [FrameFormat; 9] = [
    FrameFormat::H264 { length_size: 4 },
    FrameFormat::H264 { length_size: 1 },
    FrameFormat::Hevc { length_size: 2 },
    FrameFormat::H264AnnexB,
    FrameFormat::HevcAnnexB,
    FrameFormat::Av1,
    FrameFormat::Vp8,
    FrameFormat::Vp9,
    FrameFormat::ProRes,
];

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let kind = match mode % 5 {
        1 => *b"avcC",
        2 => *b"hvcC",
        3 => *b"av1C",
        4 => {
            let format = FORMATS[usize::from(mode / 5) % FORMATS.len()];
            let mut sizes = FrameSizes::new(None);
            let mut work = 0u32;
            let len = rest.len() as u64;
            let _ = frames::read_frame(format, &mut SliceSource::new(rest), 0, len, &mut sizes, &mut || {
                work += 1;
                if work > 10_000 {
                    Err(coreav_isobmff::Error::Overflow { offset: 0 })
                } else {
                    Ok(())
                }
            });
            if sizes.bound().is_some() {
                assert!(!sizes.is_unverifiable(), "an unverifiable frame gives no bound");
            }
            return;
        }
        _ => {
            let limits = Limits {
                max_sample_work: 1 << 20,
                ..Limits::default()
            };
            let Ok(movie) = probe(SliceSource::new(rest), &limits, &Allowlist::default()) else {
                return;
            };
            let mut budget = Budget::new(&limits);
            for track in &movie.tracks {
                let Ok(scanned) =
                    movie.scan_frame_sizes(track.index, SliceSource::new(rest), &limits, &mut budget)
                else {
                    continue;
                };
                if let (Some((sw, sh)), Some((cw, ch))) =
                    (scanned, track.description.parameter_set_size)
                {
                    assert!(sw >= cw && sh >= ch, "the scan includes the configuration");
                }
            }
            return;
        }
    };
    if let Some(record) = describe_config_record(FourCc(kind), rest) {
        if let Some((w, h)) = record.parameter_set_size {
            assert!(w > 0 && h > 0);
        }
    }
});
