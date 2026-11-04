use crate::{bits::Bits, nal, sei, *};

fn escaped(kind: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut nal = vec![kind << 1, 1];
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros == 2 && byte <= 3 {
            nal.push(3);
            zeros = 0;
        }
        nal.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    nal
}

fn padded(mut w: params::tests::Writer) -> Vec<u8> {
    w.flag(true);
    while w.len() % 8 != 0 {
        w.flag(false);
    }
    w.bytes()
}

fn synthetic_sets() -> HeaderParser {
    let mut w = params::tests::Writer::default();
    w.ue(0);
    w.ue(0);
    w.flag(false);
    w.flag(true);
    w.u(3, 0);
    w.flag(false);
    w.flag(false);
    w.ue(0);
    w.ue(0);
    w.se(0);
    for _ in 0..3 {
        w.flag(false);
    }
    w.se(0);
    w.se(0);
    for _ in 0..7 {
        w.flag(false);
    }
    w.flag(true);
    w.flag(true);
    w.flag(true);
    w.flag(false);
    w.flag(false);
    w.ue(0);
    w.flag(false);
    w.flag(false);
    let pps = padded(w);
    let mut parser = HeaderParser::new(Limits::default());
    parser
        .read_nal(&escaped(32, &params::tests::video(false)), |_| Ok(()))
        .unwrap();
    parser
        .read_nal(
            &escaped(33, &params::tests::sequence(8, false, false, false)),
            |_| Ok(()),
        )
        .unwrap();
    parser.read_nal(&escaped(34, &pps), |_| Ok(())).unwrap();
    parser
}

fn idr() -> Vec<u8> {
    let mut w = params::tests::Writer::default();
    w.flag(true);
    w.flag(false);
    w.ue(0);
    w.ue(2);
    w.flag(true);
    w.flag(false);
    w.flag(false);
    w.se(0);
    w.flag(false);
    let mut rbsp = padded(w);
    rbsp.push(0x80);
    escaped(19, &rbsp)
}

#[test]
fn complete_header_activation_end_markers_and_drain_are_distinct() {
    let mut parser = synthetic_sets();
    let mut outputs = Vec::new();
    let parsed = parser
        .read_nal(&idr(), |event| {
            outputs.push(event);
            Ok(())
        })
        .unwrap();
    let NalInfo::Slice {
        header,
        picture,
        order,
    } = parsed
    else {
        panic!("expected a slice")
    };
    assert_eq!(header.slice_type, Some(2));
    assert_eq!(
        (picture.width, picture.visible_height, picture.bit_depth),
        (64, 56, 8)
    );
    assert_eq!(order.unwrap().poc, 0);
    parser.read_nal(&[72, 1], |_| Ok(())).unwrap();
    parser
        .read_nal(&idr(), |event| {
            outputs.push(event);
            Ok(())
        })
        .unwrap();
    parser.read_nal(&[74, 1], |_| Ok(())).unwrap();
    let summary = parser
        .finish(|event| {
            outputs.push(event);
            Ok(())
        })
        .unwrap();
    assert_eq!((summary.pictures, summary.output_pictures), (2, 2));
    assert_eq!(
        outputs.iter().map(|e| e.picture_index).collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(
        parser.finish(|_| panic!("must not emit twice")).unwrap(),
        summary
    );
    assert!(parser.read_nal(&idr(), |_| Ok(())).is_err());
}

#[test]
fn truncated_slice_headers_never_enter_the_picture_or_output_count() {
    let nal = idr();
    for end in 0..nal.len() {
        let mut parser = synthetic_sets();
        assert!(
            parser
                .read_nal(&nal[..end], |_| panic!(
                    "no output from a truncated first slice"
                ))
                .is_err()
        );
        assert!(parser.finish(|_| Ok(())).is_err());
    }
    let mut parser = synthetic_sets();
    assert!(
        parser
            .read_nal(&nal, |_| Err(Error::Limit("consumer")))
            .is_ok()
    );
    assert_eq!(
        parser.finish(|_| Err(Error::Limit("consumer"))),
        Err(Error::Limit("consumer"))
    );
    assert_eq!(
        parser.finish(|_| Ok(())),
        Err(Error::InvalidData("parser_failed"))
    );
}

#[test]
fn nal_headers_keep_layer_and_temporal_identifiers_separate() {
    assert_eq!(
        NalHeader::parse(&[2, 1]).unwrap(),
        NalHeader {
            unit_type: 1,
            layer_id: 0,
            temporal_id: 0
        }
    );
    assert_eq!(NalHeader::parse(&[68, 3]).unwrap().temporal_id, 2);
    assert_eq!(
        NalHeader::parse(&[0x80, 1]),
        Err(Error::InvalidData("nal_header"))
    );
    assert_eq!(
        NalHeader::parse(&[0, 0]),
        Err(Error::InvalidData("nal_header"))
    );
    assert_eq!(
        NalHeader::parse(&[0, 9]),
        Err(Error::Unsupported("nuh_layer_id"))
    );
    assert!(matches!(NalHeader::parse(&[0]), Err(Error::InvalidData(_))));
    assert!(matches!(
        NalHeader::parse(&[4, 1]),
        Err(Error::InvalidData(_))
    ));
    assert!(NalHeader::parse(&[4, 2]).is_ok());
}

#[test]
fn framing_keeps_payload_boundaries_and_refuses_incomplete_lengths() {
    let stream = [0, 0, 0, 1, 64, 1, 0x80, 0, 0, 1, 66, 1, 0x80, 0, 0];
    let nals = AnnexB::new(&stream).collect::<Result<Vec<_>>>().unwrap();
    assert_eq!(nals, [&[64, 1, 0x80][..], &[66, 1, 0x80][..]]);
    let framed = [0, 0, 0, 3, 64, 1, 0x80];
    assert_eq!(
        LengthPrefixed::new(&framed, 4)
            .unwrap()
            .next()
            .unwrap()
            .unwrap(),
        &[64, 1, 0x80]
    );
    assert!(LengthPrefixed::new(&framed, 0).is_err());
    for truncated in [&framed[..3], &framed[..6]] {
        assert!(
            LengthPrefixed::new(truncated, 4)
                .unwrap()
                .next()
                .unwrap()
                .is_err()
        );
    }
    assert!(
        AnnexB::new(&[0, 0, 1, 64, 1, 0x80, 0, 0, 1])
            .last()
            .unwrap()
            .is_err()
    );
    assert!(
        AnnexB::new(&[7, 0, 0, 1, 64, 1, 0x80])
            .next()
            .unwrap()
            .is_err()
    );
}

#[test]
fn rbsp_removes_only_valid_emulation_prevention() {
    assert_eq!(
        nal::rbsp(&[64, 1, 0, 0, 3, 0, 0, 3, 1, 0x80], &Limits::default()).unwrap(),
        [0, 0, 0, 0, 1, 0x80]
    );
    assert!(nal::rbsp(&[64, 1, 0, 0, 3, 4], &Limits::default()).is_err());
    assert!(nal::rbsp(&[64, 1, 0, 0, 2], &Limits::default()).is_err());
    assert!(matches!(
        nal::rbsp(
            &[64, 1, 0x80],
            &Limits {
                max_nal_bytes: 2,
                ..Limits::default()
            }
        ),
        Err(Error::Limit(_))
    ));
}

#[test]
fn bounded_golomb_alignment_and_truncation() {
    let mut bits = Bits::new(&[0b10100110]);
    assert_eq!(bits.ue(0).unwrap(), 0);
    assert_eq!(bits.ue(1).unwrap(), 1);
    assert_eq!(bits.se(-1, 1).unwrap(), -1);
    assert!(bits.flag().is_ok());
    assert!(bits.flag().is_err());
    assert!(Bits::new(&[0, 0, 0, 0, 0]).ue(u32::MAX).is_err());
    assert!(Bits::new(&[0x80]).trailing().is_ok());
    assert!(Bits::new(&[0x81]).trailing().is_err());
    assert!(Bits::new(&[0x80, 0]).trailing().is_err());
    assert!(Bits::new(&[0x40]).alignment().is_err());
}

#[test]
fn sei_hash_types_and_ignored_payloads_have_separate_accounting() {
    let mut message = vec![132, 49, 0];
    message.extend([17; 48]);
    message.push(0x80);
    let parsed = sei::parse(&message, true).unwrap();
    assert_eq!(parsed.picture_hash, Some(PictureHash::Md5([[17; 16]; 3])));
    assert_eq!(parsed.reserved_hashes, 0);
    assert!(sei::parse(&message, false).is_err());
    message[1] = 48;
    assert!(sei::parse(&message, true).is_err());
    assert_eq!(
        sei::parse(&[132, 1, 99, 0x80], true)
            .unwrap()
            .reserved_hashes,
        1
    );
    assert_eq!(sei::parse(&[5, 2, 0, 1, 0x80], false).unwrap().messages, 1);
    for bytes in [&[132, 1][..], &[255][..], &[5, 5, 0x80][..]] {
        assert!(sei::parse(bytes, true).is_err());
    }
}

#[test]
fn errors_are_terminal_and_empty_streams_do_not_pass() {
    let mut parser = HeaderParser::new(Limits::default());
    assert!(parser.finish(|_| Ok(())).is_err());
    assert_eq!(
        parser.read_nal(&[0], |_| Ok(())),
        Err(Error::InvalidData("truncated_nal_header"))
    );
    assert_eq!(
        parser.read_nal(&[70, 1, 0x10], |_| Ok(())),
        Err(Error::InvalidData("parser_failed"))
    );
    assert!(parser.finish(|_| Ok(())).is_err());
}
