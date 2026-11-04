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

fn synthetic_parameter_nals() -> [Vec<u8>; 3] {
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
    [
        escaped(32, &params::tests::video(false)),
        escaped(33, &params::tests::sequence(8, false, false, false)),
        escaped(34, &pps),
    ]
}

fn synthetic_sets() -> HeaderParser {
    let mut parser = HeaderParser::new(Limits::default());
    for nal in synthetic_parameter_nals() {
        parser.read_nal(&nal, |_| Ok(())).unwrap();
    }
    parser
}

fn idr() -> Vec<u8> {
    idr_segment(true)
}

fn idr_segment(first: bool) -> Vec<u8> {
    let mut w = params::tests::Writer::default();
    w.flag(first);
    w.flag(false);
    w.ue(0);
    if !first {
        w.u(1, 1);
    }
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

fn intra_first(kind: u8) -> Vec<u8> {
    let mut w = params::tests::Writer::default();
    w.flag(true);
    if (16..=21).contains(&kind) {
        w.flag(false);
    }
    w.ue(0);
    w.ue(2);
    w.flag(true);
    w.u(4, 1);
    w.flag(false);
    w.flag(false);
    for _ in 0..4 {
        w.ue(0);
    }
    w.flag(false);
    w.flag(false);
    w.flag(false);
    w.se(0);
    w.flag(false);
    let mut rbsp = padded(w);
    rbsp.push(0x80);
    escaped(kind, &rbsp)
}

#[derive(Clone, Copy, Default)]
enum TestLongTerm {
    #[default]
    None,
    FromSps(u32),
    Explicit(u32, bool),
}

#[derive(Clone, Copy, Default)]
struct TestReferences {
    short_term: Option<(u32, bool)>,
    long_term: TestLongTerm,
    temporal_mvp: bool,
}

fn reference_segment(kind: u8, first: bool, slice_type: u32, refs: TestReferences) -> Vec<u8> {
    let mut w = params::tests::Writer::default();
    w.flag(first);
    if (16..=21).contains(&kind) {
        w.flag(false);
    }
    w.ue(0);
    if !first {
        w.u(1, 1);
    }
    w.ue(slice_type);
    w.flag(true);
    w.u(4, 1);
    w.flag(false);
    w.flag(false);
    w.ue(u32::from(refs.short_term.is_some()));
    w.ue(0);
    if let Some((delta, used)) = refs.short_term {
        w.ue(delta);
        w.flag(used);
    }
    match refs.long_term {
        TestLongTerm::None => {
            w.ue(0);
            w.ue(0);
        }
        TestLongTerm::FromSps(index) => {
            w.ue(1);
            w.ue(0);
            w.u(1, index);
            w.flag(false);
        }
        TestLongTerm::Explicit(poc, used) => {
            w.ue(0);
            w.ue(1);
            w.u(4, poc);
            w.flag(used);
            w.flag(false);
        }
    }
    w.flag(refs.temporal_mvp);
    w.flag(false);
    w.flag(false);
    if slice_type < 2 {
        w.flag(false);
        w.ue(0);
    }
    w.se(0);
    w.flag(false);
    let mut rbsp = padded(w);
    rbsp.push(0x80);
    escaped(kind, &rbsp)
}

fn replaced_ue(rbsp: &[u8], offset: usize, value: u32) -> Vec<u8> {
    let stop = rbsp.len() * 8 - rbsp.last().unwrap().trailing_zeros() as usize - 1;
    let mut bits = Bits::new(rbsp);
    bits.skip(offset).unwrap();
    bits.ue(u32::MAX).unwrap();
    let after = bits.position();
    let mut writer = params::tests::Writer::default();
    for at in 0..offset {
        writer.flag(rbsp[at / 8] & (1 << (7 - at % 8)) != 0);
    }
    writer.ue(value);
    for at in after..stop {
        writer.flag(rbsp[at / 8] & (1 << (7 - at % 8)) != 0);
    }
    padded(writer)
}

fn two_ctu_parameter_nals() -> [Vec<u8>; 3] {
    let mut sets = synthetic_parameter_nals();
    let sps = nal::rbsp(&sets[1], &Limits::default()).unwrap();
    sets[1] = escaped(33, &replaced_ue(&sps, 8 + 96 + 1 + 3, 128));
    sets
}

#[test]
fn repeated_parameter_sets_between_slice_segments_preserve_the_picture() {
    let sets = two_ctu_parameter_nals();
    for repeated in &sets {
        let mut parser = HeaderParser::new(Limits::default());
        for set in &sets {
            parser.read_nal(set, |_| Ok(())).unwrap();
        }
        parser.read_nal(&idr_segment(true), |_| Ok(())).unwrap();
        parser.read_nal(repeated, |_| Ok(())).unwrap();
        let NalInfo::Slice { header, order, .. } =
            parser.read_nal(&idr_segment(false), |_| Ok(())).unwrap()
        else {
            panic!("expected a continuation slice")
        };
        assert!(!header.first);
        assert_eq!(header.address, 1);
        assert!(order.is_none());
        let summary = parser.finish(|_| Ok(())).unwrap();
        assert_eq!((summary.pictures, summary.output_pictures), (1, 1));
        assert_eq!(summary.independent_slices, 2);
    }
}

#[test]
fn unused_parameter_sets_between_slice_segments_preserve_the_picture() {
    let sets = two_ctu_parameter_nals();
    let mut unused = Vec::new();
    let mut vps = params::tests::video(false);
    vps[0] |= 1 << 4;
    let mut changed_vps = params::tests::video(true);
    changed_vps[0] |= 1 << 4;
    unused.push([escaped(32, &vps), escaped(32, &changed_vps)]);
    let sps = nal::rbsp(&sets[1], &Limits::default()).unwrap();
    unused.push([
        escaped(33, &replaced_ue(&sps, 8 + 96, 1)),
        escaped(
            33,
            &replaced_ue(&replaced_ue(&sps, 8 + 96 + 1 + 3, 192), 8 + 96, 1),
        ),
    ]);
    let pps = nal::rbsp(&sets[2], &Limits::default()).unwrap();
    unused.push([
        escaped(34, &replaced_ue(&pps, 0, 1)),
        escaped(34, &replaced_ue(&replaced_ue(&pps, 11, 2), 0, 1)),
    ]);
    for replacements in unused {
        let mut parser = HeaderParser::new(Limits::default());
        for set in &sets {
            parser.read_nal(set, |_| Ok(())).unwrap();
        }
        parser.read_nal(&idr_segment(true), |_| Ok(())).unwrap();
        for replacement in replacements {
            parser.read_nal(&replacement, |_| Ok(())).unwrap();
        }
        parser.read_nal(&idr_segment(false), |_| Ok(())).unwrap();
        let summary = parser.finish(|_| Ok(())).unwrap();
        assert_eq!((summary.pictures, summary.output_pictures), (1, 1));
        assert_eq!(summary.independent_slices, 2);
    }
}

#[test]
fn changed_active_parameter_sets_require_a_new_first_slice() {
    let sets = two_ctu_parameter_nals();
    let changed_sps = params::tests::sequence(8, false, false, true);
    let pps = nal::rbsp(&sets[2], &Limits::default()).unwrap();
    for changed in [
        escaped(32, &params::tests::video(true)),
        escaped(33, &replaced_ue(&changed_sps, 8 + 96 + 1 + 3, 128)),
        escaped(34, &replaced_ue(&pps, 11, 2)),
    ] {
        let mut parser = HeaderParser::new(Limits::default());
        for set in &sets {
            parser.read_nal(set, |_| Ok(())).unwrap();
        }
        parser.read_nal(&idr_segment(true), |_| Ok(())).unwrap();
        parser.read_nal(&changed, |_| Ok(())).unwrap();
        assert_eq!(
            parser.read_nal(&idr_segment(false), |_| Ok(())),
            Err(Error::InvalidData("missing_first_slice"))
        );
        assert_eq!(
            parser.finish(|_| Ok(())),
            Err(Error::InvalidData("parser_failed"))
        );
    }
}

#[test]
fn first_slice_cannot_change_the_sps_identity_inside_a_cvs() {
    for kind in [1, 21] {
        let mut unchanged = synthetic_sets();
        unchanged.read_nal(&idr(), |_| Ok(())).unwrap();
        unchanged.read_nal(&intra_first(kind), |_| Ok(())).unwrap();
        assert_eq!(unchanged.finish(|_| Ok(())).unwrap().pictures, 2);
        for different_id in [false, true] {
            let mut parser = synthetic_sets();
            parser.read_nal(&idr(), |_| Ok(())).unwrap();
            let changed = if different_id {
                replaced_ue(&params::tests::sequence(8, false, false, false), 8 + 96, 1)
            } else {
                params::tests::sequence(8, false, false, true)
            };
            parser.read_nal(&escaped(33, &changed), |_| Ok(())).unwrap();
            if different_id {
                let pps = nal::rbsp(&synthetic_parameter_nals()[2], &Limits::default()).unwrap();
                parser
                    .read_nal(&escaped(34, &replaced_ue(&pps, 1, 1)), |_| Ok(()))
                    .unwrap();
            }
            let mut emitted = 0;
            assert_eq!(
                parser.read_nal(&intra_first(kind), |_| {
                    emitted += 1;
                    Ok(())
                }),
                Err(Error::InvalidData("sps_change_within_cvs"))
            );
            assert_eq!(emitted, 0);
            assert_eq!(
                parser.finish(|_| Ok(())),
                Err(Error::InvalidData("parser_failed"))
            );
        }
    }
}

#[test]
fn a_new_cvs_can_activate_a_different_sps_identity() {
    for kind in [19, 21] {
        for different_id in [false, true] {
            let mut parser = synthetic_sets();
            parser.read_nal(&idr(), |_| Ok(())).unwrap();
            if kind == 21 {
                parser.read_nal(&escaped(36, &[]), |_| Ok(())).unwrap();
            }
            let changed = if different_id {
                replaced_ue(&params::tests::sequence(8, false, false, false), 8 + 96, 1)
            } else {
                params::tests::sequence(8, false, false, true)
            };
            parser.read_nal(&escaped(33, &changed), |_| Ok(())).unwrap();
            if different_id {
                let pps = nal::rbsp(&synthetic_parameter_nals()[2], &Limits::default()).unwrap();
                parser
                    .read_nal(&escaped(34, &replaced_ue(&pps, 1, 1)), |_| Ok(()))
                    .unwrap();
            }
            let next = if kind == 19 { idr() } else { intra_first(kind) };
            let NalInfo::Slice {
                order: Some(order), ..
            } = parser.read_nal(&next, |_| Ok(())).unwrap()
            else {
                panic!("expected the first slice of a new CVS")
            };
            assert!(order.no_rasl_output);
            let summary = parser.finish(|_| Ok(())).unwrap();
            assert_eq!(summary.pictures, 2);
            assert_eq!(summary.output_pictures, if kind == 21 { 1 } else { 2 });
        }
    }
}

#[test]
fn vps_change_obeys_new_picture_prior_output_flags() {
    for kind in [16, 19, 21] {
        for discard in [false, true] {
            let mut parser = synthetic_sets();
            let mut output = Vec::new();
            parser
                .read_nal(&idr(), |event| {
                    output.push(event);
                    Ok(())
                })
                .unwrap();
            assert!(output.is_empty());
            if kind == 21 {
                parser.read_nal(&escaped(36, &[]), |_| Ok(())).unwrap();
            }
            parser
                .read_nal(&escaped(32, &params::tests::video(true)), |_| Ok(()))
                .unwrap();
            let next = if kind == 19 { idr() } else { intra_first(kind) };
            let mut rbsp = nal::rbsp(&next, &Limits::default()).unwrap();
            if discard {
                rbsp[0] |= 1 << 6;
            }
            parser
                .read_nal(&escaped(kind, &rbsp), |event| {
                    output.push(event);
                    Ok(())
                })
                .unwrap();
            let summary = parser
                .finish(|event| {
                    output.push(event);
                    Ok(())
                })
                .unwrap();
            let indices = output
                .iter()
                .map(|event| event.picture_index)
                .collect::<Vec<_>>();
            if discard || kind == 21 {
                assert_eq!(indices, [1], "kind={kind}, discard={discard}");
                assert_eq!(summary.output_pictures, 1);
            } else {
                assert_eq!(indices, [0, 1]);
                assert_eq!(summary.output_pictures, 2);
            }
        }
    }
}

#[test]
fn vps_change_requires_an_irap_before_releasing_prior_output() {
    let mut parser = synthetic_sets();
    parser.read_nal(&idr(), |_| Ok(())).unwrap();
    parser
        .read_nal(&escaped(32, &params::tests::video(true)), |_| Ok(()))
        .unwrap();
    let mut output = Vec::new();
    assert_eq!(
        parser.read_nal(&intra_first(1), |event| {
            output.push(event);
            Ok(())
        }),
        Err(Error::InvalidData("CVSG requires CVS-start IRAP"))
    );
    assert!(output.is_empty());
    assert_eq!(
        parser.finish(|_| Ok(())),
        Err(Error::InvalidData("parser_failed"))
    );
}

#[test]
fn every_irap_slice_must_be_intra() {
    for first in [true, false] {
        let mut parser = HeaderParser::new(Limits::default());
        for set in two_ctu_parameter_nals() {
            parser.read_nal(&set, |_| Ok(())).unwrap();
        }
        if !first {
            parser
                .read_nal(
                    &reference_segment(21, true, 2, TestReferences::default()),
                    |_| Ok(()),
                )
                .unwrap();
        }
        let later = reference_segment(
            21,
            first,
            1,
            TestReferences {
                short_term: Some((0, true)),
                ..TestReferences::default()
            },
        );
        assert_eq!(
            parser.read_nal(&later, |_| Ok(())),
            Err(Error::InvalidData("irap_slice_type"))
        );
    }
}

#[test]
fn independent_segments_keep_the_same_reference_syntax() {
    for (first_refs, later_refs) in [
        (
            TestReferences::default(),
            TestReferences {
                short_term: Some((0, false)),
                ..TestReferences::default()
            },
        ),
        (
            TestReferences::default(),
            TestReferences {
                temporal_mvp: true,
                ..TestReferences::default()
            },
        ),
        (
            TestReferences::default(),
            TestReferences {
                long_term: TestLongTerm::Explicit(2, false),
                ..TestReferences::default()
            },
        ),
        (
            TestReferences {
                long_term: TestLongTerm::FromSps(1),
                ..TestReferences::default()
            },
            TestReferences {
                long_term: TestLongTerm::Explicit(2, false),
                ..TestReferences::default()
            },
        ),
    ] {
        let mut parser = HeaderParser::new(Limits::default());
        for set in two_ctu_parameter_nals() {
            parser.read_nal(&set, |_| Ok(())).unwrap();
        }
        parser
            .read_nal(&reference_segment(21, true, 2, first_refs), |_| Ok(()))
            .unwrap();
        assert_eq!(
            parser.read_nal(&reference_segment(21, false, 2, later_refs), |_| Ok(())),
            Err(Error::InvalidData("inconsistent_picture_headers"))
        );
        assert_eq!(
            parser.finish(|_| Ok(())),
            Err(Error::InvalidData("parser_failed"))
        );
    }
}

#[test]
fn matching_reference_syntax_survives_segment_offsets_and_non_irap_slice_types() {
    for refs in [
        TestReferences::default(),
        TestReferences {
            long_term: TestLongTerm::FromSps(1),
            ..TestReferences::default()
        },
        TestReferences {
            short_term: Some((0, false)),
            ..TestReferences::default()
        },
        TestReferences {
            temporal_mvp: true,
            ..TestReferences::default()
        },
    ] {
        let mut parser = HeaderParser::new(Limits::default());
        for set in two_ctu_parameter_nals() {
            parser.read_nal(&set, |_| Ok(())).unwrap();
        }
        for first in [true, false] {
            parser
                .read_nal(&reference_segment(21, first, 2, refs), |_| Ok(()))
                .unwrap();
        }
        let summary = parser.finish(|_| Ok(())).unwrap();
        assert_eq!((summary.pictures, summary.independent_slices), (1, 2));
    }
    let mut parser = HeaderParser::new(Limits::default());
    for set in two_ctu_parameter_nals() {
        parser.read_nal(&set, |_| Ok(())).unwrap();
    }
    parser.read_nal(&idr(), |_| Ok(())).unwrap();
    let refs = TestReferences {
        short_term: Some((0, true)),
        ..TestReferences::default()
    };
    parser
        .read_nal(&reference_segment(1, true, 2, refs), |_| Ok(()))
        .unwrap();
    parser
        .read_nal(&reference_segment(1, false, 1, refs), |_| Ok(()))
        .unwrap();
    let summary = parser.finish(|_| Ok(())).unwrap();
    assert_eq!((summary.pictures, summary.independent_slices), (2, 3));
}

#[test]
fn equal_derived_long_term_references_do_not_hide_different_syntax() {
    let mut parser = HeaderParser::new(Limits::default());
    for set in two_ctu_parameter_nals() {
        parser.read_nal(&set, |_| Ok(())).unwrap();
    }
    let first = reference_segment(
        21,
        true,
        2,
        TestReferences {
            long_term: TestLongTerm::FromSps(1),
            ..TestReferences::default()
        },
    );
    let NalInfo::Slice { header: first, .. } = parser.read_nal(&first, |_| Ok(())).unwrap() else {
        panic!("expected first slice")
    };
    let later = reference_segment(
        21,
        false,
        2,
        TestReferences {
            long_term: TestLongTerm::Explicit(2, false),
            ..TestReferences::default()
        },
    );
    let nal = NalHeader::parse(&later).unwrap();
    let rbsp = nal::rbsp(&later, &Limits::default()).unwrap();
    let mut bits = Bits::new(&rbsp);
    let prefix = slice::prefix(&mut bits, nal).unwrap();
    let sets = two_ctu_parameter_nals();
    let sps = nal::rbsp(&sets[1], &Limits::default()).unwrap();
    let sps = params::sps(&mut Bits::new(&sps), &Limits::default()).unwrap();
    let pps = nal::rbsp(&sets[2], &Limits::default()).unwrap();
    let pps = params::pps(&mut Bits::new(&pps), &Limits::default()).unwrap();
    let second = slice::parse(&mut bits, nal, prefix, &sps, &pps, &Limits::default()).unwrap();
    assert_eq!(first.long_term_count, second.long_term_count);
    assert_eq!(first.long_term, second.long_term);
    assert_eq!(
        parser.read_nal(&later, |_| Ok(())),
        Err(Error::InvalidData("inconsistent_picture_headers"))
    );
}

#[test]
fn prior_output_flag_is_consistent_in_independent_and_dependent_segments() {
    for dependent in [false, true] {
        let mut sets = two_ctu_parameter_nals();
        if dependent {
            let mut pps = nal::rbsp(&sets[2], &Limits::default()).unwrap();
            pps[0] |= 1 << 5;
            sets[2] = escaped(34, &pps);
        }
        let later = if dependent {
            let mut w = params::tests::Writer::default();
            w.flag(false);
            w.flag(false);
            w.ue(0);
            w.flag(true);
            w.u(1, 1);
            let mut rbsp = padded(w);
            rbsp.push(0x80);
            escaped(19, &rbsp)
        } else {
            idr_segment(false)
        };
        for changed in [false, true] {
            let mut parser = HeaderParser::new(Limits::default());
            for set in &sets {
                parser.read_nal(set, |_| Ok(())).unwrap();
            }
            parser.read_nal(&idr(), |_| Ok(())).unwrap();
            let mut rbsp = nal::rbsp(&later, &Limits::default()).unwrap();
            if changed {
                rbsp[0] |= 1 << 6;
            }
            let result = parser.read_nal(&escaped(19, &rbsp), |_| Ok(()));
            if changed {
                assert_eq!(
                    result,
                    Err(Error::InvalidData("inconsistent_picture_headers"))
                );
                assert_eq!(
                    parser.finish(|_| Ok(())),
                    Err(Error::InvalidData("parser_failed"))
                );
            } else {
                let NalInfo::Slice { header, .. } = result.unwrap() else {
                    panic!("expected continuation")
                };
                assert_eq!(header.dependent, dependent);
                assert_eq!(parser.finish(|_| Ok(())).unwrap().pictures, 1);
            }
        }
    }
}

#[test]
fn vps_change_cannot_turn_an_ordinary_cra_into_a_cvs_start() {
    let mut parser = synthetic_sets();
    parser.read_nal(&idr(), |_| Ok(())).unwrap();
    parser
        .read_nal(&escaped(32, &params::tests::video(true)), |_| Ok(()))
        .unwrap();
    let mut output = Vec::new();
    assert_eq!(
        parser.read_nal(&intra_first(21), |event| {
            output.push(event);
            Ok(())
        }),
        Err(Error::InvalidData("CVSG requires CVS-start IRAP"))
    );
    assert!(output.is_empty());
    assert_eq!(
        parser.finish(|_| Ok(())),
        Err(Error::InvalidData("parser_failed"))
    );
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
    assert_eq!(
        Bits::new(&[0, 0, 0, 0, 0x80, 0, 0, 0, 0])
            .ue(u32::MAX)
            .unwrap(),
        u32::MAX
    );
    assert!(
        Bits::new(&[0, 0, 0, 0, 0x80, 0, 0, 0, 0])
            .ue(u32::MAX - 1)
            .is_err()
    );
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
