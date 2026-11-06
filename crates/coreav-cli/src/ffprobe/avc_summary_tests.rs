#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

use super::*;
use coreav_isobmff::{Error, SliceSource};

use super::summary_test_movie as common;

struct SampleReads<'a> {
    inner: SliceSource<'a>,
    sample: (u64, usize),
    reads: usize,
}

impl Source for SampleReads<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        self.inner.len()
    }

    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), Error> {
        if (offset, buffer.len()) == self.sample {
            self.reads += 1;
        }
        self.inner.read_exact_at(offset, buffer)
    }
}

#[test]
fn avc_authority_reads_only_summary_eligible_samples() {
    let config = [
        1, 66, 0, 30, 0xff, 0xe1, 0, 8, 0x67, 66, 0, 30, 0xf4, 0x23, 0xd0, 4, 1, 0, 4, 0x68, 0xce,
        0x3c, 0x80,
    ];
    let mut entry = vec![0; 78];
    entry[7] = 1;
    entry[24..26].copy_from_slice(&64u16.to_be_bytes());
    entry[26..28].copy_from_slice(&48u16.to_be_bytes());
    entry.extend(common::bx(b"avcC", &config));
    entry.extend(common::bx(b"fiel", &[1, 0]));
    let track = common::TrackSpec {
        entry: common::bx(b"avc1", &entry),
        samples: vec![(vec![0, 0, 0, 2, 0x65, 0xb8], 100)],
        size: (64 << 16, 48 << 16),
        ..common::TrackSpec::video(1, &[])
    };
    let plain = common::movie(&[track]);
    let mut fragmented = plain.clone();
    let moov = fragmented.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let size = u32::from_be_bytes(fragmented[moov..moov + 4].try_into().unwrap());
    let trex = [1u32, 1, 100, 6, 0]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect::<Vec<_>>();
    let mvex = common::bx(b"mvex", &common::full(b"trex", 0, 0, &trex));
    fragmented[moov..moov + 4].copy_from_slice(&(size + mvex.len() as u32).to_be_bytes());
    fragmented.extend(mvex);
    fragmented.extend(common::bx(
        b"moof",
        &[
            common::full(b"mfhd", 0, 0, &1u32.to_be_bytes()),
            common::bx(
                b"traf",
                &common::full(b"tfhd", 0, 0x030000, &1u32.to_be_bytes()),
            ),
        ]
        .concat(),
    ));

    for (bytes, eligible) in [(&plain, true), (&fragmented, false)] {
        let limits = Limits::default();
        let movie =
            coreav_isobmff::probe(&mut SliceSource::new(bytes), &limits, &Allowlist::listing())
                .unwrap();
        assert_eq!(movie.has_fragments(), !eligible);
        let first = head(
            &movie,
            &movie.tracks[0],
            &mut SliceSource::new(bytes),
            &limits,
        )
        .unwrap()
        .first
        .unwrap();
        for purpose in [
            super::super::Purpose::Summary,
            super::super::Purpose::Structured,
        ] {
            let mut source = SampleReads {
                inner: SliceSource::new(bytes),
                sample: (first.0, first.1 as usize),
                reads: 0,
            };
            let probed = probe(&mut source, bytes.len() as u64, "avc.mp4", &limits, purpose)
                .unwrap_or_else(|error| panic!("{}", error.to_json()));
            assert_eq!(source.reads, usize::from(eligible));
            assert_eq!(
                matches!(
                    probed.report.summary,
                    super::super::SummaryEvidence::Complete { .. }
                ),
                eligible
            );
        }
    }
}
