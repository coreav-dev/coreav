//! Exact source timing fixtures, without native encoding or media decoding.

#![cfg(feature = "svt-av1")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use coreav_decode::ErrorKind;
use coreav_isobmff::Rational;
use coreav_thumbnail::StreamPacket;
use coreav_transcode::{av1::Timeline, encoder::FrameRate};

fn packets(times: &[i64], duration: Option<u64>) -> Vec<StreamPacket> {
    times
        .iter()
        .map(|&pts| StreamPacket {
            offset: 0,
            size: 0,
            pts,
            time: None,
            dts: None,
            duration,
            keyframe: false,
            presented: true,
        })
        .collect()
}

fn plan(packets: &[StreamPacket], clock: Rational) -> Result<Timeline, coreav_decode::Error> {
    Timeline::new(
        packets,
        clock,
        100,
        800,
        Instant::now() + Duration::from_secs(10),
    )
}

#[test]
fn exact_cadence_is_not_a_snapped_rate_or_a_source_clock_conversion() {
    for origin in [-90_000, 90_000] {
        let times = [origin + 9009, origin, origin + 6006, origin + 3003];
        let source = plan(
            &packets(&times, Some(3003)),
            Rational::new(1, 90_000).unwrap(),
        )
        .unwrap();
        assert_eq!(
            source.rate(),
            FrameRate {
                num: 30_000,
                den: 1001
            }
        );
        assert_eq!(source.frames(), 4);
        assert_eq!(source.last_duration().get(), 3003);
    }
    let source = plan(
        &packets(&[0, 1000, 2000], Some(1001)),
        Rational::new(1, 90_000).unwrap(),
    )
    .unwrap();
    assert_eq!(
        source.rate(),
        FrameRate {
            num: 270_000,
            den: 3001
        }
    );
    let variable = plan(
        &packets(&[-9, -8, -6, -5], Some(3)),
        Rational::new(1, 30).unwrap(),
    )
    .unwrap();
    assert_eq!(variable.rate(), FrameRate { num: 120, den: 7 });
    assert_eq!(variable.last_duration().get(), 3);
}

#[test]
fn unknown_last_duration_uses_only_a_real_preceding_interval() {
    let clock = Rational::new(1, 1000).unwrap();
    let source = plan(&packets(&[8, 11, 16], None), clock).unwrap();
    assert_eq!(source.last_duration().get(), 5);
    assert_eq!(source.rate(), FrameRate { num: 3000, den: 13 });
    assert_eq!(
        plan(&packets(&[8], None), clock).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    assert_eq!(
        plan(&packets(&[8], Some(2)), clock).unwrap().rate(),
        FrameRate { num: 500, den: 1 }
    );
}

#[test]
fn reference_only_packets_do_not_change_presented_cadence() {
    let mut source = packets(&[0, 2, 4], Some(2));
    let mut reference = *source.first().unwrap();
    reference.duration = Some(0);
    reference.presented = false;
    source.insert(1, reference);
    let timeline = plan(&source, Rational::new(1, 60).unwrap()).unwrap();
    assert_eq!(timeline.frames(), 3);
    assert_eq!(timeline.rate(), FrameRate { num: 30, den: 1 });
}

#[test]
fn duplicate_missing_invalid_and_unrepresentable_source_fields_are_refused() {
    let clock = Rational::new(1, 1000).unwrap();
    for source in [
        packets(&[], Some(1)),
        packets(&[0, 0], Some(1)),
        packets(&[0], Some(0)),
        packets(&[0], Some(u64::from(u32::MAX) + 1)),
        packets(&[i64::MIN, i64::MAX], None),
    ] {
        assert_eq!(
            plan(&source, clock).unwrap_err().kind,
            ErrorKind::Unsupported
        );
    }
    for clock in [
        Rational { num: 0, den: 1 },
        Rational { num: -1, den: 1 },
        Rational { num: 1, den: 0 },
        Rational {
            num: 1,
            den: u64::MAX,
        },
        Rational {
            num: i128::MAX,
            den: 1,
        },
    ] {
        assert_eq!(
            plan(&packets(&[0], Some(1)), clock).unwrap_err().kind,
            ErrorKind::Unsupported
        );
    }
}

#[test]
fn rational_factors_reduce_before_conversion_to_native_fields() {
    let timeline = plan(
        &packets(&[0], Some(1)),
        Rational {
            num: 1i128 << 80,
            den: 1u64 << 60,
        },
    )
    .unwrap();
    assert_eq!(
        timeline.rate(),
        FrameRate {
            num: 1,
            den: 1 << 20
        }
    );
    let timeline = plan(
        &packets(&[0, i64::from(u32::MAX)], Some(1)),
        Rational {
            num: 1,
            den: 1u64 << 31,
        },
    )
    .unwrap();
    assert_eq!(timeline.rate(), FrameRate { num: 1, den: 1 });
}

#[test]
fn count_capacity_and_deadline_limits_are_checked() {
    let source = packets(&[0, 1, 2, 3], Some(1));
    let clock = Rational::new(1, 30).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    for (frames, bytes) in [(0, 800), (3, 800), (4, 31), (4, 0)] {
        assert_eq!(
            Timeline::new(&source, clock, frames, bytes, deadline)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
    assert_eq!(
        Timeline::new(&source, clock, 4, 32, deadline)
            .unwrap()
            .frames(),
        4
    );
    assert_eq!(
        Timeline::new(&source, clock, 4, 32, Instant::now())
            .unwrap_err()
            .kind,
        ErrorKind::Timeout
    );
}
