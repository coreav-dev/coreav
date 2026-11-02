//! Synthetic AAC configuration, packet-boundary, and drain-count fixtures.

use coreav_backend_apple::aac::{BatchLimits, Config, Error, PacketBatch, PacketDescription, Trim};

const LIMITS: BatchLimits = BatchLimits {
    max_buffer_bytes: 16,
    max_packet_bytes: 8,
    max_packets: 4,
};

fn description(start_offset: i64, byte_size: u32, variable_frames: u32) -> PacketDescription {
    PacketDescription {
        start_offset,
        byte_size,
        variable_frames,
    }
}

#[test]
fn all_four_acceptance_formats_keep_the_requested_rate_and_channel_order() -> Result<(), Error> {
    for (bytes, rate, channels) in [
        ([0x11, 0x88], 48_000, 1),
        ([0x11, 0x90], 48_000, 2),
        ([0x12, 0x08], 44_100, 1),
        ([0x12, 0x10], 44_100, 2),
    ] {
        let config = Config::parse(&bytes, rate, channels)?;
        assert_eq!(config.as_bytes(), &bytes);
        assert_eq!(config.sample_rate(), rate);
        assert_eq!(config.channels(), channels);
        assert_eq!(
            Config::parse(&bytes, rate + 1, channels),
            Err(Error::FormatMismatch)
        );
        assert_eq!(
            Config::parse(&bytes, rate, 3 - channels),
            Err(Error::FormatMismatch)
        );
    }
    Ok(())
}

#[test]
fn configuration_refuses_extensions_and_truncation() {
    for bytes in [
        &[][..],
        &[0x11][..],
        &[0x11, 0x90, 0][..],
        &[0x11, 0x90, 0x56, 0xe5, 0x80][..],
        &[0x2b, 0x90][..],
        &[0xeb, 0x90][..],
        &[0x11, 0x94][..],
        &[0x11, 0x92][..],
        &[0x11, 0x91][..],
        &[0x17, 0x90][..],
        &[0x17, 0x10][..],
        &[0x16, 0x90][..],
        &[0x11, 0x80][..],
        &[0x11, 0x98][..],
    ] {
        assert_eq!(Config::parse(bytes, 48_000, 2), Err(Error::Configuration));
    }
}

#[test]
fn only_one_two_byte_configuration_can_describe_the_requested_format() {
    let accepted: Vec<_> = (0..=u16::MAX)
        .filter(|word| Config::parse(&word.to_be_bytes(), 48_000, 2).is_ok())
        .collect();
    assert_eq!(accepted, [0x1190]);
}

#[test]
fn indexed_rates_are_preserved_without_resampling() -> Result<(), Error> {
    for (bytes, rate) in [
        ([0x10, 0x10], 96_000),
        ([0x10, 0x90], 88_200),
        ([0x11, 0x10], 64_000),
        ([0x11, 0x90], 48_000),
        ([0x12, 0x10], 44_100),
        ([0x12, 0x90], 32_000),
        ([0x13, 0x10], 24_000),
        ([0x13, 0x90], 22_050),
        ([0x14, 0x10], 16_000),
        ([0x14, 0x90], 12_000),
        ([0x15, 0x10], 11_025),
        ([0x15, 0x90], 8_000),
        ([0x16, 0x10], 7_350),
    ] {
        assert_eq!(Config::parse(&bytes, rate, 2)?.sample_rate(), rate);
    }
    Ok(())
}

#[test]
fn batches_own_only_packet_bytes_and_preserve_order() -> Result<(), Error> {
    let mut source = [90, 1, 2, 90, 3, 4, 5, 90];
    let batch = PacketBatch::copy_from(
        &source,
        &[description(1, 2, 0), description(4, 3, 1024)],
        LIMITS,
    )?;
    source.fill(0);
    assert_eq!(
        batch.packets().collect::<Vec<_>>(),
        [vec![1, 2], vec![3, 4, 5]]
    );
    assert_eq!(batch.nominal_frames(), 2048);
    Ok(())
}

#[test]
fn malformed_packets_never_become_partial_success() {
    for descriptions in [
        vec![description(-1, 1, 0)],
        vec![description(i64::MAX, u32::MAX, 0)],
        vec![description(0, 0, 0)],
        vec![description(0, 1, 960)],
        vec![description(0, 1, u32::MAX)],
        vec![description(7, 2, 0)],
        vec![description(0, 3, 0), description(2, 2, 0)],
        vec![description(0, 1, 0), description(0, 1, 0)],
        vec![description(4, 1, 0), description(1, 1, 0)],
        vec![description(0, 1, 0), description(-1, 1, 0)],
    ] {
        assert!(PacketBatch::copy_from(&[1; 8], &descriptions, LIMITS).is_err());
    }
}

#[test]
fn each_batch_bound_is_enforced_at_its_exact_boundary() -> Result<(), Error> {
    let descriptions = [description(0, 2, 0), description(2, 2, 0)];
    let exact = BatchLimits {
        max_buffer_bytes: 4,
        max_packet_bytes: 2,
        max_packets: 2,
    };
    assert_eq!(
        PacketBatch::copy_from(&[1; 4], &descriptions, exact)?.nominal_frames(),
        2048
    );
    for limits in [
        BatchLimits {
            max_buffer_bytes: 3,
            ..exact
        },
        BatchLimits {
            max_packet_bytes: 1,
            ..exact
        },
        BatchLimits {
            max_packets: 1,
            ..exact
        },
    ] {
        assert_eq!(
            PacketBatch::copy_from(&[1; 4], &descriptions, limits).err(),
            Some(Error::Limit)
        );
    }
    assert_eq!(
        PacketBatch::copy_from(&[1; 8], &[description(0, 1, 0)], exact).err(),
        Some(Error::Limit)
    );
    Ok(())
}

#[test]
fn empty_output_is_explicit_and_has_no_unclaimed_bytes() -> Result<(), Error> {
    let limits = BatchLimits {
        max_buffer_bytes: 0,
        max_packet_bytes: 0,
        max_packets: 0,
    };
    let batch = PacketBatch::copy_from(&[], &[], limits)?;
    assert_eq!(batch.packets().len(), 0);
    assert_eq!(batch.nominal_frames(), 0);
    assert_eq!(
        PacketBatch::copy_from(&[1], &[], LIMITS).err(),
        Some(Error::PacketDescription)
    );
    assert_eq!(
        PacketBatch::copy_from(&[], &[description(0, 1, 0)], LIMITS).err(),
        Some(Error::PacketDescription)
    );
    Ok(())
}

#[test]
fn all_small_single_packet_extents_match_a_byte_iterator() -> Result<(), Error> {
    let source = [10, 11, 12, 13];
    for offset in -1_i64..=5 {
        for size in 0_u32..=5 {
            for frames in [0, 960, 1024] {
                let result =
                    PacketBatch::copy_from(&source, &[description(offset, size, frames)], LIMITS);
                let valid =
                    offset >= 0 && size > 0 && offset + i64::from(size) <= 4 && frames != 960;
                assert_eq!(result.is_ok(), valid);
                if valid {
                    let start = usize::try_from(offset).map_err(|_| Error::PacketDescription)?;
                    let length = usize::try_from(size).map_err(|_| Error::PacketDescription)?;
                    let expected: Vec<_> =
                        source.iter().copied().skip(start).take(length).collect();
                    assert_eq!(result?.packets().collect::<Vec<_>>(), [expected]);
                }
            }
        }
    }
    Ok(())
}

#[test]
fn drain_accounts_for_every_submitted_sample_frame() -> Result<(), Error> {
    let trim = Trim::validate(4, 1000, 2112, 984, 2112)?;
    assert_eq!(trim.leading_frames(), 2112);
    assert_eq!(trim.trailing_frames(), 984);
    assert!(Trim::validate(0, 0, 0, 0, 0).is_ok());
    assert!(Trim::validate(2, 2048, 0, 0, 0).is_ok());
    for (packets, submitted, leading, trailing) in [
        (3, 1000, 2112, 984),
        (5, 1000, 2112, 984),
        (4, 999, 2112, 984),
        (4, 1001, 2112, 984),
        (0, 1, 0, 0),
        (0, 0, 1, 0),
        (1, 0, 1024, 1),
        (u64::MAX, 0, 0, 0),
    ] {
        assert_eq!(
            Trim::validate(packets, submitted, leading, trailing, 4096),
            Err(Error::DrainCount)
        );
    }
    assert_eq!(Trim::validate(4, 1000, 2112, 984, 2111), Err(Error::Limit));
    assert_eq!(Trim::validate(1, 1, 0, 1023, 1022), Err(Error::Limit));
    Ok(())
}
