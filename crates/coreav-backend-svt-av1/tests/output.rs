//! Packet-independent temporal-unit and segmented-job validation.

#[path = "frames.rs"]
mod fixtures;

use coreav_backend_svt_av1::output;
use fixtures::{frame, sequence};

use fixtures::{Coded, Seq, TestResult, existing, packet};
use output::{Error, Expected, Limits, Output, Sample};

fn limits() -> Limits {
    Limits {
        max_packet_bytes: 4096,
        max_buffered_bytes: 8192,
        max_obu_bytes: 4096,
        max_temporal_unit_bytes: 8192,
        max_sequence_bytes: 4096,
        max_total_bytes: 1 << 20,
        max_obus: 10_000,
        max_frames: 10_000,
        max_frames_per_temporal_unit: 16,
        max_samples: 1000,
        max_tiles: 4096,
    }
}

fn expected(seq: Seq) -> Expected {
    Expected {
        dimensions: seq.expected(),
        primaries: 2,
        transfer: 2,
        matrix: 2,
        range: sequence::ColorRange::Studio,
        chroma_position: sequence::ChromaPosition::Unknown,
    }
}

fn tu(obus: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = packet(2, &[]);
    for obu in obus {
        bytes.extend_from_slice(obu);
    }
    bytes
}

fn drain(reader: &mut Output, samples: &mut Vec<Sample>) -> Result<(), Error> {
    while let Some(sample) = reader.next_sample()? {
        samples.push(sample);
    }
    Ok(())
}

fn stream(seq: Seq) -> Vec<u8> {
    let mut bytes = tu(&[seq.packet(), Coded::default().packet(seq)]);
    bytes.extend_from_slice(&tu(&[
        Coded {
            kind: 1,
            show: false,
            refresh: 2,
            hint: 2,
            ..Coded::default()
        }
        .packet(seq),
        Coded {
            kind: 1,
            refresh: 4,
            hint: 1,
            ..Coded::default()
        }
        .packet(seq),
    ]));
    bytes.extend_from_slice(&tu(&[existing(1)]));
    bytes
}

#[test]
fn arbitrary_native_chunk_boundaries_preserve_complete_tus_and_display_order() -> TestResult {
    let seq = Seq::default();
    let bytes = stream(seq);
    let mut baseline = Vec::new();
    for chunk_size in 1..=bytes.len() {
        let mut reader = Output::new(expected(seq), limits());
        reader.begin_segment(10)?;
        let mut samples = Vec::new();
        for chunk in bytes.chunks(chunk_size) {
            reader.push(chunk)?;
            drain(&mut reader, &mut samples)?;
        }
        samples.push(reader.end_segment(3)?);
        assert_eq!(
            samples.iter().map(|s| s.ordinal).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(
            samples
                .iter()
                .map(|s| s.picture_identity)
                .collect::<Vec<_>>(),
            [0, 2, 1]
        );
        assert_eq!(
            samples.iter().map(|s| s.sync).collect::<Vec<_>>(),
            [true, false, false]
        );
        assert_eq!(
            samples.iter().map(|s| s.show_existing).collect::<Vec<_>>(),
            [false, false, true]
        );
        assert_eq!(reader.configuration(), Some(seq.packet().as_slice()));
        assert_eq!(
            reader.sequence().ok_or("sequence")?.av1c_header().bytes(),
            [0x81, 7, 12, 0]
        );
        if baseline.is_empty() {
            baseline = samples;
        } else {
            assert_eq!(samples, baseline);
        }
    }
    let mut first = seq.packet();
    first.extend_from_slice(&Coded::default().packet(seq));
    assert_eq!(baseline.first().ok_or("first sample")?.bytes, first);
    Ok(())
}

#[test]
fn segments_keep_global_ordinals_and_configuration_but_not_references() -> TestResult {
    let seq = Seq::default();
    let bytes = tu(&[seq.packet(), Coded::default().packet(seq)]);
    let mut reader = Output::new(expected(seq), limits());
    for ordinal in 0..3 {
        reader.begin_segment(3)?;
        reader.push(&bytes)?;
        assert!(reader.next_sample()?.is_none());
        let sample = reader.end_segment(1)?;
        assert_eq!(sample.ordinal, ordinal);
        assert!(sample.sync);
    }
    reader.begin_segment(1)?;
    reader.push(&tu(&[seq.packet(), existing(0)]))?;
    assert_eq!(
        reader.next_sample(),
        Err(Error::Frame(frame::Error::Reference))
    );
    assert_eq!(reader.begin_segment(1), Err(Error::Failed));
    Ok(())
}

#[test]
fn fixed_session_caps_are_bounded_by_the_remaining_job_samples() -> TestResult {
    let seq = Seq::default();
    let bytes = tu(&[seq.packet(), Coded::default().packet(seq)]);
    for capacity in [3_u64, 150] {
        for pictures in [1_u64, 2, 3, 4, 5, 6, 7, 10] {
            let mut reader = Output::new(
                expected(seq),
                Limits {
                    max_samples: pictures,
                    ..limits()
                },
            );
            let mut samples = Vec::new();
            let mut remaining = pictures;
            while remaining != 0 {
                reader.begin_segment(capacity)?;
                let count = capacity.min(remaining);
                for _ in 0..count {
                    reader.push(&bytes)?;
                    drain(&mut reader, &mut samples)?;
                }
                samples.push(reader.end_segment(count)?);
                remaining -= count;
            }
            assert_eq!(samples.len() as u64, pictures);
            assert_eq!(
                samples
                    .iter()
                    .map(|sample| sample.ordinal)
                    .collect::<Vec<_>>(),
                (0..pictures).collect::<Vec<_>>()
            );
            assert_eq!(reader.begin_segment(capacity), Err(Error::Limit));
            assert_eq!(reader.next_sample(), Err(Error::Failed));
        }
    }
    Ok(())
}

#[test]
fn session_and_remaining_job_caps_both_reject_an_extra_sample() -> TestResult {
    let seq = Seq::default();
    let bytes = tu(&[seq.packet(), Coded::default().packet(seq)]);
    for preceding in [0, 3] {
        let mut reader = Output::new(
            expected(seq),
            Limits {
                max_samples: 4,
                ..limits()
            },
        );
        if preceding != 0 {
            reader.begin_segment(3)?;
            for _ in 0..preceding {
                reader.push(&bytes)?;
                drain(&mut reader, &mut Vec::new())?;
            }
            reader.end_segment(preceding)?;
        }
        reader.begin_segment(3)?;
        let count = 3.min(4 - preceding) + 1;
        for _ in 0..count {
            reader.push(&bytes)?;
            drain(&mut reader, &mut Vec::new())?;
        }
        assert_eq!(reader.end_segment(count), Err(Error::PictureCount));
        assert_eq!(reader.next_sample(), Err(Error::Failed));
    }
    Ok(())
}

#[test]
fn identical_parsed_configuration_with_different_obu_bytes_is_rejected() -> TestResult {
    let seq = Seq::default();
    let bytes = tu(&[seq.packet(), Coded::default().packet(seq)]);
    let mut reader = Output::new(expected(seq), limits());
    reader.begin_segment(1)?;
    reader.push(&bytes)?;
    reader.next_sample()?;
    reader.end_segment(1)?;
    let mut payload = seq.syntax().bytes();
    payload.push(0);
    assert_eq!(
        fixtures::parse_sequence(&packet(1, &payload))?,
        seq.parse()?
    );
    reader.begin_segment(1)?;
    reader.push(&tu(&[packet(1, &payload), Coded::default().packet(seq)]))?;
    assert_eq!(reader.next_sample(), Err(Error::ConfigurationChanged));
    Ok(())
}

#[test]
fn incomplete_eof_hidden_tails_and_wrong_submission_counts_fail() -> TestResult {
    let seq = Seq::default();
    let bytes = stream(seq);
    for n in 0..bytes.len() {
        let mut reader = Output::new(expected(seq), limits());
        reader.begin_segment(3)?;
        reader.push(bytes.get(..n).ok_or("prefix")?)?;
        let mut samples = Vec::new();
        assert!(
            drain(&mut reader, &mut samples)
                .and_then(|()| reader.end_segment(3).map(|_| ()))
                .is_err()
        );
    }
    for count in [0, 2, 4] {
        let mut reader = Output::new(expected(seq), limits());
        reader.begin_segment(3)?;
        reader.push(&bytes)?;
        drain(&mut reader, &mut Vec::new())?;
        assert_eq!(reader.end_segment(count), Err(Error::PictureCount));
    }
    let mut reader = Output::new(expected(seq), limits());
    reader.begin_segment(3)?;
    let mut bytes = tu(&[seq.packet(), Coded::default().packet(seq)]);
    bytes.extend_from_slice(&tu(&[
        Coded {
            kind: 1,
            show: false,
            refresh: 2,
            ..Coded::default()
        }
        .packet(seq),
        Coded {
            kind: 1,
            refresh: 4,
            ..Coded::default()
        }
        .packet(seq),
    ]));
    reader.push(&bytes)?;
    drain(&mut reader, &mut Vec::new())?;
    assert_eq!(
        reader.end_segment(2),
        Err(Error::Frame(frame::Error::HiddenFrameLost))
    );
    Ok(())
}

#[test]
fn each_tu_needs_one_display_and_each_segment_starts_with_a_shown_keyframe() -> TestResult {
    let seq = Seq::default();
    for bytes in [
        tu(&[seq.packet()]),
        tu(&[
            seq.packet(),
            Coded::default().packet(seq),
            Coded::default().packet(seq),
        ]),
    ] {
        let mut reader = Output::new(expected(seq), limits());
        reader.begin_segment(2)?;
        reader.push(&bytes)?;
        let result = reader.next_sample().and_then(|_| reader.end_segment(1));
        assert_eq!(result, Err(Error::DisplayCount));
    }
    let mut reader = Output::new(expected(seq), limits());
    reader.begin_segment(1)?;
    reader.push(&tu(&[
        seq.packet(),
        Coded {
            kind: 2,
            refresh: 1,
            ..Coded::default()
        }
        .packet(seq),
    ]))?;
    assert!(reader.next_sample()?.is_none());
    assert_eq!(reader.end_segment(1), Err(Error::RandomAccess));
    let mut reader = Output::new(expected(seq), limits());
    reader.begin_segment(1)?;
    reader.push(&seq.packet())?;
    assert_eq!(reader.next_sample(), Err(Error::Ordering));
    Ok(())
}

#[test]
fn unknown_metadata_layered_obus_and_changed_color_fail_closed() -> TestResult {
    let seq = Seq::default();
    for kind in [5, 7, 8] {
        let mut reader = Output::new(expected(seq), limits());
        reader.begin_segment(1)?;
        reader.push(&tu(&[seq.packet(), packet(kind, &[0x80])]))?;
        assert_eq!(reader.next_sample(), Err(Error::UnsupportedObu));
    }
    let mut layer = vec![0x36, 0];
    layer.extend_from_slice(Coded::default().packet(seq).get(1..).ok_or("frame")?);
    let mut reader = Output::new(expected(seq), limits());
    reader.begin_segment(1)?;
    reader.push(&tu(&[seq.packet(), layer]))?;
    assert_eq!(reader.next_sample(), Err(Error::UnsupportedObu));
    let mut w = seq.syntax();
    w.change("range", 1)?;
    let mut reader = Output::new(expected(seq), limits());
    reader.begin_segment(1)?;
    reader.push(&tu(&[packet(1, &w.bytes()), Coded::default().packet(seq)]))?;
    assert_eq!(reader.next_sample(), Err(Error::Metadata));
    Ok(())
}

#[test]
fn resource_limits_charge_ignored_obus_hidden_frames_and_partial_lengths() -> TestResult {
    let seq = Seq::default();
    let bytes = stream(seq);
    let cases = [
        Limits {
            max_packet_bytes: bytes.len() - 1,
            ..limits()
        },
        Limits {
            max_buffered_bytes: bytes.len() - 1,
            ..limits()
        },
        Limits {
            max_obu_bytes: seq.packet().len() - 1,
            ..limits()
        },
        Limits {
            max_sequence_bytes: seq.packet().len() - 1,
            ..limits()
        },
        Limits {
            max_total_bytes: bytes.len() as u64 - 1,
            ..limits()
        },
        Limits {
            max_temporal_unit_bytes: seq.packet().len(),
            ..limits()
        },
        Limits {
            max_obus: 3,
            ..limits()
        },
        Limits {
            max_frames: 3,
            ..limits()
        },
        Limits {
            max_frames_per_temporal_unit: 1,
            ..limits()
        },
    ];
    for bound in cases {
        let mut reader = Output::new(expected(seq), bound);
        reader.begin_segment(3)?;
        let result = reader
            .push(&bytes)
            .and_then(|()| drain(&mut reader, &mut Vec::new()));
        assert_eq!(result, Err(Error::Limit), "{bound:?}");
        assert_eq!(reader.next_sample(), Err(Error::Failed));
    }
    let mut reader = Output::new(
        expected(seq),
        Limits {
            max_obus: 4,
            ..limits()
        },
    );
    reader.begin_segment(1)?;
    reader.push(&tu(&[
        seq.packet(),
        packet(15, &[]),
        packet(15, &[]),
        Coded::default().packet(seq),
    ]))?;
    assert_eq!(reader.next_sample(), Err(Error::Limit));
    let mut reader = Output::new(expected(seq), limits());
    reader.begin_segment(1)?;
    reader.push(&[0x32, 0xff, 0xff, 0x7f])?;
    assert_eq!(reader.next_sample(), Err(Error::Limit));
    Ok(())
}

#[test]
fn padding_is_not_sample_data_and_late_errors_cannot_release_more_samples() -> TestResult {
    let seq = Seq::default();
    let key = Coded::default().packet(seq);
    let mut bytes = tu(&[
        packet(15, &[1, 2, 3]),
        seq.packet(),
        key.clone(),
        packet(15, &[4, 5]),
    ]);
    bytes.extend_from_slice(&packet(2, &[]));
    bytes.extend_from_slice(&[0x80]);
    let mut reader = Output::new(expected(seq), limits());
    reader.begin_segment(2)?;
    reader.push(&bytes)?;
    let sample = reader.next_sample()?.ok_or("sample")?;
    let mut expected = seq.packet();
    expected.extend_from_slice(&key);
    assert_eq!(sample.bytes, expected);
    assert!(reader.next_sample().is_err());
    assert_eq!(reader.next_sample(), Err(Error::Failed));
    assert_eq!(reader.end_segment(1), Err(Error::Failed));
    Ok(())
}

#[test]
fn undrained_packets_cannot_trigger_repeated_buffer_compaction() -> TestResult {
    let seq = Seq::default();
    let bytes = stream(seq);
    let mut reader = Output::new(expected(seq), limits());
    reader.begin_segment(3)?;
    reader.push(&bytes)?;
    assert!(reader.next_sample()?.is_some());
    assert_eq!(reader.push(&[]), Err(Error::DrainRequired));
    assert_eq!(reader.next_sample(), Err(Error::Failed));
    Ok(())
}
