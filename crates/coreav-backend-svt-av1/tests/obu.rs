//! Public framing-reader behavior on synthetic AV1 byte sequences.

use coreav_backend_svt_av1::obu::{Error, Limits, ObuKind, Obus};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const LIMITS: Limits = Limits {
    max_packet_bytes: 1024,
    max_obus: 32,
};

const KINDS: [(u8, ObuKind); 9] = [
    (1, ObuKind::SequenceHeader),
    (2, ObuKind::TemporalDelimiter),
    (3, ObuKind::FrameHeader),
    (4, ObuKind::TileGroup),
    (5, ObuKind::Metadata),
    (6, ObuKind::Frame),
    (7, ObuKind::RedundantFrameHeader),
    (8, ObuKind::TileList),
    (15, ObuKind::Padding),
];

fn assert_terminal_error(bytes: &[u8], error: Error) -> TestResult {
    let mut reader = Obus::new(bytes, LIMITS)?;
    assert_eq!(reader.next(), Some(Err(error)), "{bytes:02x?}");
    assert_eq!(reader.next(), None);
    assert_eq!(reader.next(), None);

    let mut packet = vec![0x12, 0];
    packet.extend_from_slice(bytes);
    packet.extend_from_slice(&[0x12, 0]);
    let mut reader = Obus::new(&packet, LIMITS)?;
    assert_eq!(
        reader.next().transpose()?.map(|o| o.kind()),
        Some(ObuKind::TemporalDelimiter)
    );
    assert_eq!(reader.next(), Some(Err(error)), "{packet:02x?}");
    assert_eq!(reader.next(), None);
    assert_eq!(reader.next(), None);
    Ok(())
}

#[test]
fn all_standard_kinds_preserve_borrowed_payloads() -> TestResult {
    let mut packet = Vec::new();
    for (number, kind) in KINDS {
        packet.push((number << 3) | 2);
        if kind == ObuKind::TemporalDelimiter {
            packet.push(0);
        } else {
            packet.extend_from_slice(&[3, 0, 0xff, 0x80]);
        }
    }
    let mut reader = Obus::new(&packet, LIMITS)?;
    let mut offset = 0;
    for (_, kind) in KINDS {
        let obu = reader.next().transpose()?.ok_or("missing OBU")?;
        assert_eq!(obu.kind(), kind);
        assert_eq!(obu.temporal_id(), 0);
        assert_eq!(obu.spatial_id(), 0);
        offset += 2;
        let length = if kind == ObuKind::TemporalDelimiter {
            0
        } else {
            3
        };
        assert_eq!(obu.payload(), &packet[offset..offset + length]);
        assert!(std::ptr::eq(
            obu.payload().as_ptr(),
            packet[offset..].as_ptr()
        ));
        offset += length;
    }
    assert_eq!(reader.next(), None);
    assert_eq!(reader.next(), None);
    Ok(())
}

#[test]
fn payload_syntax_and_sequence_order_are_not_claimed_valid() -> TestResult {
    for (number, _) in KINDS {
        let packet = [number << 3 | 2, 0];
        let mut reader = Obus::new(&packet, LIMITS)?;
        assert!(reader.next().transpose()?.is_some());
        assert_eq!(reader.next(), None);
    }
    Ok(())
}

#[test]
fn every_first_header_byte_is_classified() -> TestResult {
    let mut accepted = 0;
    for header in u8::MIN..=u8::MAX {
        let mut packet = vec![header];
        if header & 4 != 0 {
            packet.push(0);
        }
        packet.push(0);
        let number = (header >> 3) & 15;
        let supported = KINDS.iter().any(|(value, _)| *value == number);
        let extension_allowed = header & 4 == 0 || ![1, 2, 8].contains(&number);
        let valid = supported && header & 0x83 == 2 && extension_allowed;
        let mut reader = Obus::new(&packet, LIMITS)?;
        let result = reader.next().ok_or("missing header result")?;
        assert_eq!(result.is_ok(), valid, "header {header:02x}");
        assert_eq!(reader.next(), None, "header {header:02x}");
        assert_eq!(reader.next(), None, "header {header:02x}");
        accepted += usize::from(valid);
    }
    assert_eq!(accepted, 15);
    Ok(())
}

#[test]
fn every_extension_byte_checks_reserved_bits_and_layer_ids() -> TestResult {
    for (number, _) in KINDS {
        if [1, 2, 8].contains(&number) {
            continue;
        }
        for extension in u8::MIN..=u8::MAX {
            let packet = [number << 3 | 6, extension, 0];
            let mut reader = Obus::new(&packet, LIMITS)?;
            let result = reader.next().ok_or("missing extension result")?;
            if extension & 7 == 0 {
                let obu = result?;
                assert_eq!(obu.temporal_id(), extension >> 5);
                assert_eq!(obu.spatial_id(), (extension >> 3) & 3);
            } else {
                assert_eq!(result, Err(Error::ReservedExtensionBits));
            }
            assert_eq!(reader.next(), None);
        }
    }
    Ok(())
}

#[test]
fn hostile_headers_and_temporal_delimiters_are_terminal() -> TestResult {
    for (bytes, error) in [
        (&[0xb2, 0][..], Error::ForbiddenBit),
        (&[0x33, 0][..], Error::ReservedHeaderBit),
        (&[0x02, 0][..], Error::ReservedType),
        (&[0x4a, 0][..], Error::ReservedType),
        (&[0x72, 0][..], Error::ReservedType),
        (&[0x30][..], Error::MissingSizeField),
        (&[0x34][..], Error::MissingSizeField),
        (&[0x0e, 0, 0][..], Error::UnexpectedExtension),
        (&[0x16, 0, 0][..], Error::UnexpectedExtension),
        (&[0x46, 0, 0][..], Error::UnexpectedExtension),
        (&[0x36, 7, 0][..], Error::ReservedExtensionBits),
        (&[0x12, 1, 0][..], Error::NonemptyTemporalDelimiter),
        (&[0x12, 0x81, 0, 0][..], Error::NonemptyTemporalDelimiter),
    ] {
        assert_terminal_error(bytes, error)?;
    }
    Ok(())
}

#[test]
fn every_truncation_of_an_extended_obu_is_detected() -> TestResult {
    let packet = [0x36, 0xf8, 0x83, 0x80, 0, 0x11, 0x22, 0x33];
    assert_eq!(Obus::new(&packet[..0], LIMITS)?.next(), None);
    for end in 1..packet.len() {
        let error = match end {
            1 => Error::TruncatedExtension,
            2..=4 => Error::TruncatedSize,
            _ => Error::TruncatedPayload,
        };
        let mut reader = Obus::new(&packet[..end], LIMITS)?;
        assert_eq!(reader.next(), Some(Err(error)), "cut at {end}");
        assert_eq!(reader.next(), None);
        assert_eq!(reader.next(), None);
    }
    let mut reader = Obus::new(&packet, LIMITS)?;
    let obu = reader.next().transpose()?.ok_or("missing complete OBU")?;
    assert_eq!(obu.payload(), &[0x11, 0x22, 0x33]);
    assert_eq!(obu.temporal_id(), 7);
    assert_eq!(obu.spatial_id(), 3);
    assert_eq!(reader.next(), None);
    Ok(())
}

#[test]
fn missing_size_and_each_unterminated_size_prefix_fail() -> TestResult {
    for extension in [false, true] {
        for size_bytes in 0..8 {
            let mut packet = vec![if extension { 0x36 } else { 0x32 }];
            if extension {
                packet.push(0);
            }
            packet.extend(std::iter::repeat_n(0x80, size_bytes));
            let mut reader = Obus::new(&packet, LIMITS)?;
            assert_eq!(reader.next(), Some(Err(Error::TruncatedSize)));
            assert_eq!(reader.next(), None);
        }
    }
    Ok(())
}

#[test]
fn nonminimal_size_encodings_of_every_legal_length_are_accepted() -> TestResult {
    for length in 1..=8 {
        for payload_size in [0_u8, 1] {
            let mut packet = vec![0x32];
            for index in 0..length {
                let value = if index == 0 { payload_size } else { 0 };
                packet.push(value | if index + 1 == length { 0 } else { 0x80 });
            }
            packet.extend(std::iter::repeat_n(0xff, usize::from(payload_size)));
            packet.extend_from_slice(&[0x12, 0]);
            let mut reader = Obus::new(&packet, LIMITS)?;
            let obu = reader.next().transpose()?.ok_or("missing sized OBU")?;
            assert_eq!(obu.payload().len(), usize::from(payload_size));
            assert_eq!(
                reader.next().transpose()?.map(|o| o.kind()),
                Some(ObuKind::TemporalDelimiter)
            );
            assert_eq!(reader.next(), None);
        }
    }
    Ok(())
}

#[test]
fn size_integer_and_encoding_bounds_are_distinct() -> TestResult {
    for bytes in [
        &[0x32, 0xff, 0xff, 0xff, 0xff, 0x0f][..],
        &[0x32, 0xff, 0xff, 0xff, 0xff, 0x8f, 0x80, 0x80, 0][..],
    ] {
        assert_terminal_error(bytes, Error::TruncatedPayload)?;
    }
    for bytes in [
        &[0x32, 0x80, 0x80, 0x80, 0x80, 0x10][..],
        &[0x32, 0x80, 0x80, 0x80, 0x80, 0x80, 1][..],
        &[0x32, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 1][..],
        &[0x32, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 1][..],
    ] {
        assert_terminal_error(bytes, Error::SizeOverflow)?;
    }
    assert_terminal_error(
        &[0x32, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80],
        Error::SizeTooLong,
    )?;
    assert_terminal_error(
        &[0x32, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0],
        Error::SizeTooLong,
    )?;
    Ok(())
}

#[test]
fn multibyte_sizes_define_the_exact_payload_extent() -> TestResult {
    let mut packet = vec![0x32, 0x80, 1];
    packet.extend(std::iter::repeat_n(0x12, 128));
    packet.extend_from_slice(&[0x12, 0]);
    let mut reader = Obus::new(&packet, LIMITS)?;
    assert_eq!(
        reader.next().transpose()?.map(|o| o.payload().len()),
        Some(128)
    );
    assert_eq!(
        reader.next().transpose()?.map(|o| o.kind()),
        Some(ObuKind::TemporalDelimiter)
    );
    assert_eq!(reader.next(), None);
    Ok(())
}

#[test]
fn packet_and_obu_limits_accept_exactly_the_bound() -> TestResult {
    let packet = [0x12, 0, 0x7a, 0];
    let exact = Limits {
        max_packet_bytes: packet.len(),
        max_obus: 2,
    };
    assert_eq!(
        Obus::new(&packet, exact)?
            .collect::<Result<Vec<_>, _>>()?
            .len(),
        2
    );
    assert!(matches!(
        Obus::new(
            &packet,
            Limits {
                max_packet_bytes: packet.len() - 1,
                ..exact
            }
        ),
        Err(Error::PacketTooLarge)
    ));
    let mut reader = Obus::new(
        &packet,
        Limits {
            max_obus: 1,
            ..exact
        },
    )?;
    assert!(reader.next().transpose()?.is_some());
    assert_eq!(reader.next(), Some(Err(Error::TooManyObus)));
    assert_eq!(reader.next(), None);
    assert_eq!(reader.next(), None);
    let zero = Limits {
        max_packet_bytes: 0,
        max_obus: 0,
    };
    assert_eq!(Obus::new(&[], zero)?.next(), None);
    assert!(matches!(
        Obus::new(&[0x12], zero),
        Err(Error::PacketTooLarge)
    ));
    let mut reader = Obus::new(
        &[0x12, 0],
        Limits {
            max_packet_bytes: 2,
            max_obus: 0,
        },
    )?;
    assert_eq!(reader.next(), Some(Err(Error::TooManyObus)));
    assert_eq!(reader.next(), None);
    Ok(())
}

#[test]
fn caller_must_check_the_final_result_to_accept_the_packet() -> TestResult {
    let mut reader = Obus::new(&[0x12, 0, 0x32], LIMITS)?;
    assert!(reader.next().transpose()?.is_some());
    assert_eq!(reader.next(), Some(Err(Error::TruncatedSize)));
    assert_eq!(reader.next(), None);
    Ok(())
}

#[test]
fn errors_implement_the_standard_error_contract() {
    let errors = [
        Error::PacketTooLarge,
        Error::TooManyObus,
        Error::ForbiddenBit,
        Error::ReservedHeaderBit,
        Error::ReservedType,
        Error::MissingSizeField,
        Error::UnexpectedExtension,
        Error::TruncatedExtension,
        Error::ReservedExtensionBits,
        Error::TruncatedSize,
        Error::SizeTooLong,
        Error::SizeOverflow,
        Error::TruncatedPayload,
        Error::NonemptyTemporalDelimiter,
    ];
    for error in errors {
        let dynamic: &dyn std::error::Error = &error;
        assert!(!dynamic.to_string().is_empty());
        assert!(dynamic.source().is_none());
    }
}
