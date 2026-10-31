//! A thumbnail planned from a window chooses what one planned from every
//! packet chooses, errors included, also when building the window runs out
//! of the work budget (`docs/design/seek.md`). The movies are built with
//! `coreav-isobmff`'s test helpers.
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    dead_code
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

use common::{Spec, movie};
use coreav_isobmff::{Rational, SliceSource};
use coreav_thumbnail::{ThumbnailOptions, oracle_choice};

/// 3,000 samples of 512 ticks (two minutes at 25 per second), one `stts`
/// and one `ctts` entry and one chunk per sample, a sync sample every 50.
fn long_movie() -> Vec<u8> {
    let n = 3000u32;
    movie(&Spec {
        sizes: vec![64; n as usize],
        composition: Vec::new(),
        sync: (0..n).step_by(50).map(|i| i + 1).collect(),
        elst: None,
        stts: Some(vec![(1, 512); n as usize]),
        ctts: Some(
            (0..n)
                .map(|i| (1, [1024, 0, 512][i as usize % 3]))
                .collect(),
        ),
        chunks: Some(vec![1; n as usize]),
        ..Spec::default()
    })
}

fn choice(data: &[u8], at: i128, exact: bool, seek: bool, work: u64) -> String {
    let mut options = ThumbnailOptions::at(Rational::from_int(at));
    options.exact = exact;
    options.seek = seek;
    options.isobmff_limits.max_sample_work = work;
    format!("{:?}", oracle_choice(SliceSource::new(data), &options))
}

#[test]
fn a_window_chooses_as_every_packet_does_whatever_the_budget() {
    let data = long_movie();
    // Budgets from far too small, through the reviewed case (9,101: enough
    // for the listing, not for the window's lookups), to the default.
    for work in [
        100,
        3_500,
        9_101,
        20_000,
        coreav_isobmff::Limits::default().max_sample_work,
    ] {
        for at in [0, 1, 60, 100, 119, 120, 200] {
            for exact in [false, true] {
                let window = choice(&data, at, exact, true, work);
                let every = choice(&data, at, exact, false, work);
                assert_eq!(
                    window.replace("windowed: true", "windowed: false"),
                    every,
                    "{work} {at} {exact}"
                );
            }
        }
    }
    // With the default budget, the window is used.
    assert!(
        choice(
            &data,
            60,
            true,
            true,
            coreav_isobmff::Limits::default().max_sample_work
        )
        .contains("windowed: true")
    );
}

/// A source whose `fail_at`-th read fails with an I/O error.
struct FailingAt<'a> {
    data: SliceSource<'a>,
    reads: usize,
    fail_at: usize,
}

impl coreav_isobmff::Source for FailingAt<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        self.data.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        self.reads += 1;
        if self.reads == self.fail_at {
            return Err(coreav_isobmff::Error::Io(
                std::io::ErrorKind::ConnectionReset,
            ));
        }
        self.data.read_exact_at(offset, buf)
    }
}

/// A read that fails while a frame of the window is checked is not a
/// window that cannot choose: wherever the failure falls, planning
/// fails with it rather than falling back to the full listing.
#[test]
fn a_read_failure_fails_the_plan_wherever_it_falls() {
    let data = long_movie();
    let options = ThumbnailOptions::at(Rational::from_int(60));
    let mut clean = FailingAt {
        data: SliceSource::new(&data),
        reads: 0,
        fail_at: 0,
    };
    assert!(oracle_choice(&mut clean, &options).unwrap().windowed);
    for fail_at in 1..=clean.reads {
        let mut source = FailingAt {
            data: SliceSource::new(&data),
            reads: 0,
            fail_at,
        };
        let result = format!("{:?}", oracle_choice(&mut source, &options));
        assert!(
            result.contains("Io(ConnectionReset)"),
            "read {fail_at} of {}: {result}",
            clean.reads
        );
    }
}
