#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

use super::*;
use coreav_isobmff::{Error, SliceSource};

use super::summary_test_movie as common;

fn movie(samples: &[Vec<u8>]) -> (Vec<u8>, Movie) {
    let mut entry = vec![0; 78];
    entry[7] = 1;
    entry[24..26].copy_from_slice(&64u16.to_be_bytes());
    entry[26..28].copy_from_slice(&48u16.to_be_bytes());
    entry.extend(common::bx(b"hvcC", &video::hevc_test_record()));
    entry.extend(common::bx(b"fiel", &[1, 0]));
    let track = common::TrackSpec {
        entry: common::bx(b"hev1", &entry),
        samples: samples
            .iter()
            .cloned()
            .map(|sample| (sample, 100))
            .collect(),
        size: (64 << 16, 48 << 16),
        ..common::TrackSpec::video(1, &[])
    };
    let bytes = common::movie(&[track]);
    let mut source = SliceSource::new(&bytes);
    let movie =
        coreav_isobmff::probe(&mut source, &Limits::default(), &Allowlist::listing()).unwrap();
    (bytes, movie)
}

fn sample(kind: u8) -> Vec<u8> {
    vec![
        0,
        0,
        0,
        3,
        kind << 1,
        1,
        if (16..=21).contains(&kind) {
            0xa0
        } else {
            0xc0
        },
    ]
}

#[test]
fn every_sample_must_be_one_complete_selected_slice() {
    let samples = [sample(20), sample(1), sample(21), sample(8)];
    let (bytes, movie) = movie(&samples);
    assert!(
        hevc_summary_authority(
            &movie,
            &movie.tracks[0],
            &mut SliceSource::new(&bytes),
            &Limits::default()
        )
        .unwrap()
    );
    for index in 0..samples.len() {
        for kind in [2, 3, 4, 5, 32, 33, 34, 35, 39, 40, 63] {
            let mut changed = samples.to_vec();
            changed[index] = sample(kind);
            let (bytes, movie) = self::movie(&changed);
            assert!(
                !hevc_summary_authority(
                    &movie,
                    &movie.tracks[0],
                    &mut SliceSource::new(&bytes),
                    &Limits::default()
                )
                .unwrap()
            );
        }
        for bad in [
            vec![],
            vec![0, 0, 0, 0],
            vec![0, 0, 0, 9, 40, 1, 0xa0],
            vec![0, 0, 0, 2, 40, 1, 0xa0],
            [sample(20), sample(20)].concat(),
        ] {
            let mut changed = samples.to_vec();
            changed[index] = bad;
            let (bytes, movie) = self::movie(&changed);
            assert!(
                !hevc_summary_authority(
                    &movie,
                    &movie.tracks[0],
                    &mut SliceSource::new(&bytes),
                    &Limits::default()
                )
                .unwrap()
            );
        }
    }
    let (bytes, movie) = self::movie(&[sample(1)]);
    assert!(
        !hevc_summary_authority(
            &movie,
            &movie.tracks[0],
            &mut SliceSource::new(&bytes),
            &Limits::default()
        )
        .unwrap()
    );
}

struct Failing<'a>(SliceSource<'a>);
impl Source for Failing<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        self.0.len()
    }
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), Error> {
        if buffer.len() == 7 {
            return Err(Error::Io(std::io::ErrorKind::Other));
        }
        self.0.read_exact_at(offset, buffer)
    }
}

#[test]
fn census_io_errors_and_work_limits_never_certify_authority() {
    let (bytes, movie) = movie(&[sample(20), sample(1)]);
    assert!(matches!(
        hevc_summary_authority(
            &movie,
            &movie.tracks[0],
            &mut Failing(SliceSource::new(&bytes)),
            &Limits::default()
        ),
        Err(Error::Io(std::io::ErrorKind::Other))
    ));
    for cap in [0, 1, 2, 3] {
        let limits = Limits {
            max_sample_work: cap,
            ..Limits::default()
        };
        assert!(!matches!(
            hevc_summary_authority(
                &movie,
                &movie.tracks[0],
                &mut SliceSource::new(&bytes),
                &limits
            ),
            Ok(true)
        ));
    }
}

#[test]
fn parameter_sets_after_the_initial_probe_window_still_refuse() {
    let mut samples = vec![sample(1); 301];
    samples[0] = sample(20);
    let (bytes, movie) = self::movie(&samples);
    assert!(
        hevc_summary_authority(
            &movie,
            &movie.tracks[0],
            &mut SliceSource::new(&bytes),
            &Limits::default()
        )
        .unwrap()
    );
    samples[300] = sample(33);
    let (bytes, movie) = self::movie(&samples);
    assert!(
        !hevc_summary_authority(
            &movie,
            &movie.tracks[0],
            &mut SliceSource::new(&bytes),
            &Limits::default()
        )
        .unwrap()
    );
}

#[test]
fn structured_purpose_retains_the_old_report_without_a_census_read() {
    let (bytes, _) = movie(&[sample(20), sample(1)]);
    let size = bytes.len() as u64;
    let mut source = Failing(SliceSource::new(&bytes));
    let structured = probe(
        &mut source,
        size,
        "generated.mp4",
        &Limits::default(),
        super::super::Purpose::Structured,
    )
    .map_err(|error| error.to_json())
    .unwrap();
    assert!(matches!(
        structured.report.summary,
        super::super::SummaryEvidence::Unsupported(_)
    ));
    let mut source = Failing(SliceSource::new(&bytes));
    assert!(matches!(
        probe(
            &mut source,
            size,
            "generated.mp4",
            &Limits::default(),
            super::super::Purpose::Summary
        ),
        Err(Failure::Library(Error::Io(std::io::ErrorKind::Other)))
    ));
}

#[test]
fn only_video_handlers_can_qualify_hev1_summaries() {
    for handler in [
        b"vide", b"sbtl", b"subt", b"text", b"soun", b"meta", b"abcd",
    ] {
        let (mut bytes, _) = movie(&[sample(20), sample(1)]);
        let at = bytes.windows(4).position(|bytes| bytes == b"hdlr").unwrap();
        bytes[at + 12..at + 16].copy_from_slice(handler);
        let probed = probe(
            &mut SliceSource::new(&bytes),
            bytes.len() as u64,
            "generated.mp4",
            &Limits::default(),
            super::super::Purpose::Summary,
        )
        .map_err(|error| error.to_json())
        .unwrap();
        assert_eq!(
            matches!(
                probed.report.summary,
                super::super::SummaryEvidence::Complete { .. }
            ),
            handler == b"vide",
            "{handler:?}"
        );
    }
}

struct CensusReads<'a> {
    source: SliceSource<'a>,
    reads: Vec<(u64, usize)>,
    fail: Option<u64>,
}

impl Source for CensusReads<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        self.source.len()
    }
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), Error> {
        self.reads.push((offset, buffer.len()));
        if self.fail == Some(offset) {
            return Err(Error::Io(std::io::ErrorKind::Other));
        }
        self.source.read_exact_at(offset, buffer)
    }
}

#[test]
fn terminal_escape_lookahead_is_conditional_bounded_and_propagates_io_errors() {
    for terminal in [false, true] {
        for continuation in [0, 1, 2, 3, 4] {
            let mut nal = vec![0x55; video::HEVC_SUMMARY_PREFIX_BYTES];
            nal[..3].copy_from_slice(&[40, 1, 0xa0]);
            if terminal {
                nal[9..12].copy_from_slice(&[0, 0, 3]);
            } else {
                nal[10..12].copy_from_slice(&[0, 0]);
            }
            nal.extend([continuation, 0x80]);
            let mut sample = (nal.len() as u32).to_be_bytes().to_vec();
            sample.extend(nal);
            let (bytes, movie) = movie(&[sample.clone()]);
            let offset = {
                let mut source = SliceSource::new(&bytes);
                let mut budget = Budget::new(&Limits::default());
                let mut packets = movie
                    .packets(0, &mut source, &Limits::default(), &mut budget)
                    .unwrap();
                packets.next().unwrap().unwrap().offset
            };
            let mut source = CensusReads {
                source: SliceSource::new(&bytes),
                reads: Vec::new(),
                fail: None,
            };
            assert_eq!(
                hevc_summary_authority(&movie, &movie.tracks[0], &mut source, &Limits::default())
                    .unwrap(),
                !terminal || continuation <= 3
            );
            let reads: Vec<_> = source
                .reads
                .iter()
                .copied()
                .filter(|(at, _)| *at >= offset && *at < offset + sample.len() as u64)
                .collect();
            assert_eq!(
                reads,
                if terminal {
                    vec![(offset, 16), (offset + 16, 1)]
                } else {
                    vec![(offset, 16)]
                }
            );
            source.fail = Some(offset + 16);
            let result =
                hevc_summary_authority(&movie, &movie.tracks[0], &mut source, &Limits::default());
            if terminal {
                assert!(matches!(result, Err(Error::Io(std::io::ErrorKind::Other))));
            } else {
                assert!(result.unwrap());
            }
        }
    }
}
