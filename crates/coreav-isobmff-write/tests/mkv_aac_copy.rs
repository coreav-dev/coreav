//! Compatibility AAC clocks do not alter native Matroska reconstruction.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

#[path = "../../coreav-mkv/tests/common/mod.rs"]
mod mkv;

use coreav_isobmff_write::{
    MatroskaRemuxError, MatroskaRemuxOptions, check_matroska_aac_copy, remux_matroska,
    remux_matroska_compatible,
};
use coreav_mkv::{Allowlist, SliceSource, Source};
use mkv::{BLOCK_GROUP, TRACKS, audio_track, cat, cluster, el, file, info, simple_block, uint};

fn fixture(change: &str) -> Vec<u8> {
    let mut private = vec![0x12, 0x88, 0x56, 0xe5, 0];
    if change == "config-rate" {
        private[0] = 0x11;
    }
    if change == "short-frame" {
        private[1] |= 4;
    }
    if change == "implicit" {
        private.truncate(2);
    }
    let extra = cat(&[
        uint(
            0x56AA,
            if change == "delay" {
                33_000_000
            } else {
                32_000_000
            },
        ),
        uint(
            0x23_E383,
            if change == "default" {
                31_000_000
            } else {
                32_000_000
            },
        ),
        el(0x63A2, &private),
    ]);
    let mut blocks = Vec::new();
    for i in 0..5i16 {
        let timestamp = i * 32 + i16::from(change == "origin" || change == "gap" && i == 2);
        let mut block = simple_block(1, timestamp, true, &[0x21, 0x10, u8::try_from(i).unwrap()]);
        if change == "lacing" && i == 2 {
            block = el(0xA3, &[0x81, 0, 64, 0x84, 1, 0x21, 0x10]);
        }
        if change == "invisible" && i == 2 {
            block = el(0xA3, &[0x81, 0, 64, 0x88, 0x21, 0x10]);
        }
        if matches!(
            change,
            "padding" | "negative-padding" | "duration" | "state" | "additions"
        ) && i == 2
        {
            let changed = match change {
                "padding" => mkv::sint(0x75A2, 1_000_000),
                "negative-padding" => mkv::sint(0x75A2, -1_000_000),
                "duration" => uint(0x9B, 31),
                "state" => el(0xA4, &[1]),
                _ => el(0x75A1, &el(0xA6, &el(0xA5, &[1]))),
            };
            block = el(
                BLOCK_GROUP,
                &cat(&[el(0xA1, &[0x81, 0, 64, 0, 0x21, 0x10]), changed]),
            );
        }
        blocks.push(block);
    }
    let duration = if change == "endpoint" { 161.0 } else { 160.0 };
    file(
        "matroska",
        &[
            info(Some(duration)),
            el(TRACKS, &audio_track(1, "A_AAC", 32_000.0, 1, &extra)),
            cluster(0, &blocks),
        ],
    )
}

fn check(
    data: &[u8],
    demux: &coreav_mkv::Limits,
    mux: &coreav_isobmff::Limits,
) -> Result<(), MatroskaRemuxError> {
    check_matroska_aac_copy(
        SliceSource::new(data),
        demux,
        mux,
        &Allowlist::default(),
        &[0],
    )
}

#[test]
fn aligned_copy_and_native_remux_have_identical_output() {
    let data = fixture("");
    check(
        &data,
        &coreav_mkv::Limits::default(),
        &coreav_isobmff::Limits::default(),
    )
    .unwrap();
    let options = MatroskaRemuxOptions {
        streams: Some(vec![0]),
        ..MatroskaRemuxOptions::default()
    };
    let mut native = Vec::new();
    let mut compatible = Vec::new();
    remux_matroska(
        SliceSource::new(&data),
        &coreav_mkv::Limits::default(),
        &coreav_isobmff::Limits::default(),
        &Allowlist::default(),
        &options,
        &mut native,
    )
    .unwrap();
    remux_matroska_compatible(
        SliceSource::new(&data),
        &coreav_mkv::Limits::default(),
        &coreav_isobmff::Limits::default(),
        &Allowlist::default(),
        &options,
        &mut compatible,
    )
    .unwrap();
    assert!(!native.is_empty());
    assert_eq!(native, compatible);
}

#[test]
fn unqualified_packet_facts_are_refused_in_both_passes() {
    let options = MatroskaRemuxOptions {
        streams: Some(vec![0]),
        ..MatroskaRemuxOptions::default()
    };
    for changed in [
        "config-rate",
        "short-frame",
        "implicit",
        "delay",
        "default",
        "origin",
        "gap",
        "lacing",
        "invisible",
        "padding",
        "negative-padding",
        "duration",
        "state",
        "additions",
        "endpoint",
    ] {
        let data = fixture(changed);
        assert!(
            matches!(
                check(
                    &data,
                    &coreav_mkv::Limits::default(),
                    &coreav_isobmff::Limits::default()
                ),
                Err(MatroskaRemuxError::Source(
                    coreav_mkv::Error::Unsupported { .. }
                ))
            ),
            "{changed}"
        );
        let mut output = Vec::new();
        assert!(
            remux_matroska_compatible(
                SliceSource::new(&data),
                &coreav_mkv::Limits::default(),
                &coreav_isobmff::Limits::default(),
                &Allowlist::default(),
                &options,
                &mut output
            )
            .is_err(),
            "{changed}"
        );
        assert!(output.is_empty(), "{changed}");
        if changed == "gap" {
            remux_matroska(
                SliceSource::new(&data),
                &coreav_mkv::Limits::default(),
                &coreav_isobmff::Limits::default(),
                &Allowlist::default(),
                &options,
                &mut output,
            )
            .unwrap();
            assert!(!output.is_empty());
        }
    }
}

#[test]
fn truncated_packets_do_not_become_timing_refusals() {
    for change in ["", "origin"] {
        let mut data = fixture(change);
        data.pop();
        assert!(matches!(
            check(
                &data,
                &coreav_mkv::Limits::default(),
                &coreav_isobmff::Limits::default(),
            ),
            Err(MatroskaRemuxError::Source(
                coreav_mkv::Error::Truncated { .. }
            ))
        ));
    }
}

#[test]
fn timing_refusal_does_not_hide_terminal_limits() {
    let data = fixture("gap");
    let mux = coreav_isobmff::Limits {
        max_table_bytes: 1,
        ..coreav_isobmff::Limits::default()
    };
    assert!(matches!(
        check(&data, &coreav_mkv::Limits::default(), &mux),
        Err(MatroskaRemuxError::Output(
            coreav_isobmff::Error::LimitExceeded { .. }
        ))
    ));
    let demux = coreav_mkv::Limits {
        max_block_work: 1,
        ..coreav_mkv::Limits::default()
    };
    assert!(matches!(
        check(&data, &demux, &coreav_isobmff::Limits::default()),
        Err(MatroskaRemuxError::Source(
            coreav_mkv::Error::LimitExceeded { .. }
        ))
    ));
}

struct LateReadFailure<'a> {
    data: &'a [u8],
    offset: u64,
}

impl Source for LateReadFailure<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        Ok(u64::try_from(self.data.len()).unwrap())
    }
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        if offset >= self.offset {
            return Err(coreav_isobmff::Error::Io(std::io::ErrorKind::Other));
        }
        SliceSource::new(self.data).read_exact_at(offset, buf)
    }
}

#[test]
fn timing_refusal_does_not_hide_a_later_source_error() {
    let data = fixture("origin");
    let demux = coreav_mkv::Limits::default();
    let segment =
        coreav_mkv::probe(SliceSource::new(&data), &demux, &Allowlist::default()).unwrap();
    let offset = segment
        .packets(SliceSource::new(&data), &demux)
        .last()
        .unwrap()
        .unwrap()
        .block_offset;
    let result = check_matroska_aac_copy(
        LateReadFailure {
            data: &data,
            offset,
        },
        &demux,
        &coreav_isobmff::Limits::default(),
        &Allowlist::default(),
        &[0],
    );
    assert!(matches!(
        result,
        Err(MatroskaRemuxError::Source(coreav_mkv::Error::Io(
            std::io::ErrorKind::Other
        )))
    ));
}
