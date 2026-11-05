use crate::{bits::Bits, nal, sei, *};

#[path = "../fuzz/fuzz_targets/intra_parameters.rs"]
mod intra_parameters;

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

struct PcmFixture {
    sets: [Vec<u8>; 3],
    nal: Vec<u8>,
    planes: [Vec<u16>; 3],
}

fn fixture_profile(writer: &mut params::tests::Writer, depth: u32) {
    writer.u(2, 0);
    writer.flag(false);
    writer.u(5, if depth == 10 { 2 } else { 1 });
    writer.u(32, 0);
    writer.u(4, 0);
    writer.u(32, 0);
    writer.u(12, 0);
    writer.u(8, 120);
}

fn pcm_fixture(depth: u32, pcm_depth: u32, filtered: bool) -> Result<PcmFixture> {
    let mut vps = params::tests::Writer::default();
    vps.u(4, 0);
    vps.flag(true);
    vps.flag(true);
    vps.u(6, 0);
    vps.u(3, 0);
    vps.flag(true);
    vps.u(16, 0xffff);
    fixture_profile(&mut vps, depth);
    vps.flag(false);
    for _ in 0..3 {
        vps.ue(0);
    }
    vps.u(6, 0);
    vps.ue(0);
    vps.flag(false);
    vps.flag(false);
    let mut sps = params::tests::Writer::default();
    sps.u(4, 0);
    sps.u(3, 0);
    sps.flag(true);
    fixture_profile(&mut sps, depth);
    for value in [0, 1, 8, 8] {
        sps.ue(value);
    }
    sps.flag(false);
    sps.ue(depth - 8);
    sps.ue(depth - 8);
    sps.ue(0);
    sps.flag(false);
    for value in [0, 0, 0, 0, 1, 0, 1, 0, 0] {
        sps.ue(value);
    }
    for flag in [false, false, false, true] {
        sps.flag(flag);
    }
    sps.u(4, pcm_depth - 1);
    sps.u(4, pcm_depth - 1);
    sps.ue(0);
    sps.ue(0);
    sps.flag(true);
    sps.ue(0);
    for _ in 0..5 {
        sps.flag(false);
    }
    let mut pps = params::tests::Writer::default();
    pps.ue(0);
    pps.ue(0);
    pps.flag(false);
    pps.flag(false);
    pps.u(3, 0);
    pps.flag(false);
    pps.flag(false);
    pps.ue(0);
    pps.ue(0);
    pps.se(0);
    for _ in 0..3 {
        pps.flag(false);
    }
    pps.se(0);
    pps.se(0);
    for _ in 0..7 {
        pps.flag(false);
    }
    pps.flag(true);
    pps.flag(false);
    pps.flag(!filtered);
    if filtered {
        pps.se(0);
        pps.se(0);
    }
    pps.flag(false);
    pps.flag(false);
    pps.ue(0);
    pps.flag(false);
    pps.flag(false);
    let mut slice = params::tests::Writer::default();
    slice.flag(true);
    slice.flag(false);
    slice.ue(0);
    slice.ue(2);
    slice.se(0);
    let mut rbsp = padded(slice);
    rbsp.extend([0x86, 0x80]);
    let raw: [Vec<u16>; 3] = core::array::from_fn(|component| {
        let length = if component == 0 { 64 } else { 16 };
        (0..length)
            .map(|i| ((i * 17 + component * 67) & ((1 << pcm_depth) - 1)) as u16)
            .collect()
    });
    let mut samples = params::tests::Writer::default();
    for plane in &raw {
        for value in plane {
            samples.u(pcm_depth, u32::from(*value));
        }
    }
    rbsp.extend(samples.bytes());
    rbsp.extend([0xfe, 0x80]);
    let planes = raw.map(|plane| {
        plane
            .into_iter()
            .map(|v| v << (depth - pcm_depth))
            .collect()
    });
    Ok(PcmFixture {
        sets: [
            escaped(32, &padded(vps)),
            escaped(33, &padded(sps)),
            escaped(34, &padded(pps)),
        ],
        nal: escaped(19, &rbsp),
        planes,
    })
}

#[test]
fn original_pcm_picture_is_published_once_after_complete_eof() -> Result<()> {
    for (depth, pcm_depth) in [(8, 8), (10, 10), (10, 8)] {
        let fixture = pcm_fixture(depth, pcm_depth, false)?;
        let mut decoder = IntraPictureDecoder::new(IntraLimits::default(), 0);
        let mut emitted = 0;
        for nal in fixture.sets.iter().chain([&fixture.nal]) {
            decoder.read_nal(nal, |_| {
                emitted += 1;
                Ok(())
            })?;
        }
        assert_eq!(emitted, 0);
        let summary = decoder.finish(|picture| {
            emitted += 1;
            assert_eq!(picture.index, 0);
            assert_eq!(
                (
                    picture.info.width,
                    picture.info.height,
                    picture.info.bit_depth
                ),
                (8, 8, depth)
            );
            for (plane, expected) in picture.planes.into_iter().zip(&fixture.planes) {
                assert_eq!(plane.samples, expected);
                assert_eq!(plane.stride, plane.width);
                assert_eq!(plane.samples.len(), plane.width * plane.height);
            }
            assert_eq!(
                (
                    picture.stats.ctus,
                    picture.stats.coding_units,
                    picture.stats.pcm_units
                ),
                (1, 1, 1)
            );
            assert_eq!(picture.stats.bins, 3);
            assert_eq!(picture.stats.transform_units, 0);
            assert_eq!(
                (picture.stats.picture_hashes, picture.stats.reserved_hashes),
                (0, 0)
            );
            assert!(picture.stats.memory_bytes <= IntraLimits::default().max_memory_bytes);
            Ok(())
        })?;
        assert_eq!((summary.pictures, emitted), (1, 1));
        decoder.finish(|_| {
            emitted += 1;
            Ok(())
        })?;
        assert_eq!(emitted, 1);
    }
    Ok(())
}

#[test]
fn selected_inter_picture_is_refused_before_sample_allocation() -> Result<()> {
    let mut decoder = IntraPictureDecoder::new(IntraLimits::default(), 1);
    for set in two_ctu_parameter_nals() {
        decoder.read_nal(&set, |_| Ok(()))?;
    }
    decoder.read_nal(&idr(), |_| Ok(()))?;
    let inter = reference_segment(
        1,
        true,
        1,
        TestReferences {
            short_term: Some((0, true)),
            ..TestReferences::default()
        },
    );
    let mut emitted = 0;
    assert_eq!(
        decoder.read_nal(&inter, |_| {
            emitted += 1;
            Ok(())
        }),
        Err(Error::Unsupported("selected_inter_picture"))
    );
    assert_eq!(emitted, 0);
    assert!(decoder.finish(|_| Ok(())).is_err());
    Ok(())
}

fn coded_hash_payload(kind: u8, hex: &str) -> Result<Vec<u8>> {
    let size = u8::try_from(hex.len() / 2 + 1).map_err(|_| Error::InvalidData("fixture_hash"))?;
    let mut bytes = vec![132, size, kind];
    for pair in hex.as_bytes().chunks_exact(2) {
        let text = core::str::from_utf8(pair).map_err(|_| Error::InvalidData("fixture_hash"))?;
        bytes.push(u8::from_str_radix(text, 16).map_err(|_| Error::InvalidData("fixture_hash"))?);
    }
    bytes.push(0x80);
    Ok(bytes)
}

#[test]
fn all_defined_suffix_algorithms_check_the_complete_coded_components() -> Result<()> {
    for (kind, depth, hex) in [
        (
            0,
            8,
            "79bf8613946d23ec06a6259409ef4a7c34d2be1f186c459c16c628a601d89a04631161904fcb145a907c48bc2b6d0ae4",
        ),
        (1, 8, "59ca77b4cc8d"),
        (2, 8, "00001fe00000082800000858"),
        (
            0,
            10,
            "d668aa9b26174848dde6aa5ec04375930e0b879cef1d71b5145d542804fd4be4e38097d76976f72f277de9f5c260cbe3",
        ),
        (1, 10, "7d4d82ff38f7"),
        (2, 10, "000020be0000084000000870"),
    ] {
        let fixture = pcm_fixture(depth, depth, false)?;
        for changed in [false, true] {
            let mut decoder = IntraPictureDecoder::new(IntraLimits::default(), 0);
            for nal in fixture.sets.iter().chain([&fixture.nal]) {
                decoder.read_nal(nal, |_| Ok(()))?;
            }
            let mut payload = coded_hash_payload(kind, hex)?;
            if changed {
                *payload
                    .get_mut(3)
                    .ok_or(Error::InvalidData("fixture_hash"))? ^= 1;
            }
            let mut emitted = 0;
            let result = decoder.read_nal(&escaped(40, &payload), |_| {
                emitted += 1;
                Ok(())
            });
            assert_eq!(emitted, 0);
            if changed {
                assert_eq!(result, Err(Error::InvalidData("picture_hash_mismatch")));
                assert!(decoder.finish(|_| Ok(())).is_err());
            } else {
                result?;
                decoder.finish(|picture| {
                    emitted += 1;
                    assert_eq!(
                        (picture.stats.picture_hashes, picture.stats.reserved_hashes),
                        (1, 0)
                    );
                    Ok(())
                })?;
                assert_eq!(emitted, 1);
            }
        }
    }
    let fixture = pcm_fixture(8, 8, false)?;
    let mut decoder = IntraPictureDecoder::new(IntraLimits::default(), 0);
    for nal in fixture.sets.iter().chain([&fixture.nal]) {
        decoder.read_nal(nal, |_| Ok(()))?;
    }
    decoder.read_nal(&escaped(40, &[132, 1, 255, 0x80]), |_| Ok(()))?;
    decoder.finish(|picture| {
        assert_eq!(
            (picture.stats.picture_hashes, picture.stats.reserved_hashes),
            (0, 1)
        );
        Ok(())
    })?;
    Ok(())
}

#[test]
fn accounted_peak_is_an_enforced_boundary_not_the_configured_maximum() -> Result<()> {
    let fixture = pcm_fixture(8, 8, false)?;
    let mut decoder = IntraPictureDecoder::new(IntraLimits::default(), 0);
    for nal in fixture.sets.iter().chain([&fixture.nal]) {
        decoder.read_nal(nal, |_| Ok(()))?;
    }
    let mut peak = 0;
    decoder.finish(|picture| {
        peak = picture.stats.memory_bytes;
        Ok(())
    })?;
    assert!(peak > 0 && peak < IntraLimits::default().max_memory_bytes);
    for bytes in [peak - 1, peak] {
        let limits = IntraLimits {
            max_memory_bytes: bytes,
            ..IntraLimits::default()
        };
        let mut decoder = IntraPictureDecoder::new(limits, 0);
        let mut emitted = 0;
        let result = (|| {
            for nal in fixture.sets.iter().chain([&fixture.nal]) {
                decoder.read_nal(nal, |_| {
                    emitted += 1;
                    Ok(())
                })?;
            }
            decoder.finish(|picture| {
                emitted += 1;
                assert_eq!(picture.stats.memory_bytes, peak);
                Ok(())
            })
        })();
        if bytes == peak {
            result?;
            assert_eq!(emitted, 1);
        } else {
            assert_eq!(result, Err(Error::Limit("reconstruction_memory")));
            assert_eq!(emitted, 0);
        }
    }
    Ok(())
}

#[test]
fn entropy_fuzz_bootstrap_matches_self_written_parameter_syntax() -> Result<()> {
    for (depth, parameters) in [(8, intra_parameters::EIGHT), (10, intra_parameters::TEN)] {
        let fixture = pcm_fixture(depth, depth, false)?;
        for (literal, written) in parameters.into_iter().zip(fixture.sets) {
            assert_eq!(literal, written);
        }
    }
    Ok(())
}

#[test]
fn selected_picture_budgets_and_callback_errors_are_terminal() -> Result<()> {
    let fixture = pcm_fixture(10, 10, false)?;
    for limits in [
        IntraLimits {
            max_bins: 2,
            ..IntraLimits::default()
        },
        IntraLimits {
            max_memory_bytes: 1,
            ..IntraLimits::default()
        },
    ] {
        let mut decoder = IntraPictureDecoder::new(limits, 0);
        let mut failure = None;
        let mut emitted = 0;
        for nal in fixture.sets.iter().chain([&fixture.nal]) {
            if let Err(error) = decoder.read_nal(nal, |_| {
                emitted += 1;
                Ok(())
            }) {
                failure = Some(error);
                break;
            }
        }
        assert!(matches!(failure, Some(Error::Limit(_))));
        assert_eq!(emitted, 0);
        assert!(decoder.finish(|_| Ok(())).is_err());
        assert!(decoder.read_nal(&fixture.nal, |_| Ok(())).is_err());
    }
    let mut decoder = IntraPictureDecoder::new(IntraLimits::default(), 0);
    for nal in fixture.sets.iter().chain([&fixture.nal]) {
        decoder.read_nal(nal, |_| Ok(()))?;
    }
    assert_eq!(
        decoder.finish(|_| Err(Error::Limit("fixture_output"))),
        Err(Error::Limit("fixture_output"))
    );
    assert!(decoder.finish(|_| Ok(())).is_err());
    Ok(())
}

#[test]
fn absent_target_filtered_picture_and_truncated_entropy_never_publish() -> Result<()> {
    for (target, filtered, truncated) in [(1, false, false), (0, true, false), (0, false, true)] {
        let mut fixture = pcm_fixture(8, 8, filtered)?;
        if truncated {
            fixture.nal.pop();
        }
        let mut decoder = IntraPictureDecoder::new(IntraLimits::default(), target);
        let mut emitted = 0;
        let result = (|| {
            for nal in fixture.sets.iter().chain([&fixture.nal]) {
                decoder.read_nal(nal, |_| {
                    emitted += 1;
                    Ok(())
                })?;
            }
            decoder.finish(|_| {
                emitted += 1;
                Ok(())
            })
        })();
        if filtered {
            assert_eq!(result, Err(Error::Unsupported("selected_filtered_picture")));
        } else {
            assert!(matches!(result, Err(Error::InvalidData(_))));
        }
        assert_eq!(emitted, 0);
        assert!(decoder.finish(|_| Ok(())).is_err());
    }
    Ok(())
}

#[test]
fn selection_uses_original_indices_without_decoding_unselected_entropy() -> Result<()> {
    let fixture = pcm_fixture(8, 8, false)?;
    let mut first = nal::rbsp(&fixture.nal, &Limits::default())?;
    first.truncate(2);
    let mut decoder = IntraPictureDecoder::new(IntraLimits::default(), 1);
    for nal in &fixture.sets {
        decoder.read_nal(nal, |_| Ok(()))?;
    }
    decoder.read_nal(&escaped(19, &first), |_| Ok(()))?;
    decoder.read_nal(&fixture.nal, |_| Ok(()))?;
    let mut emitted = 0;
    let summary = decoder.finish(|picture| {
        emitted += 1;
        assert_eq!(picture.index, 1);
        Ok(())
    })?;
    assert_eq!((summary.pictures, emitted), (2, 1));
    Ok(())
}

#[test]
fn a_later_error_does_not_revoke_a_callback_or_make_the_session_pass() -> Result<()> {
    let fixture = pcm_fixture(8, 8, false)?;
    let mut decoder = IntraPictureDecoder::new(IntraLimits::default(), 0);
    for nal in &fixture.sets {
        decoder.read_nal(nal, |_| Ok(()))?;
    }
    decoder.read_nal(&fixture.nal, |_| Ok(()))?;
    let mut emitted = 0;
    decoder.read_nal(&fixture.nal, |_| {
        emitted += 1;
        Ok(())
    })?;
    assert_eq!(emitted, 1);
    assert!(decoder.read_nal(&[0], |_| Ok(())).is_err());
    assert!(
        decoder
            .finish(|_| {
                emitted += 1;
                Ok(())
            })
            .is_err()
    );
    assert_eq!(emitted, 1);
    Ok(())
}

#[test]
fn selected_suffix_hash_cannot_be_ignored_before_a_picture_or_after_publication() -> Result<()> {
    let hash = escaped(40, &[132, 7, 1, 0, 0, 0, 0, 0, 0, 0x80]);
    let mut decoder = IntraPictureDecoder::new(IntraLimits::default(), 0);
    assert_eq!(
        decoder.read_nal(&hash, |_| Ok(())),
        Err(Error::InvalidData("suffix_sei_without_picture"))
    );
    assert!(decoder.finish(|_| Ok(())).is_err());
    let fixture = pcm_fixture(8, 8, false)?;
    let mut decoder = IntraPictureDecoder::new(IntraLimits::default(), 0);
    for nal in fixture.sets.iter().chain([&fixture.nal]) {
        decoder.read_nal(nal, |_| Ok(()))?;
    }
    let mut emitted = 0;
    decoder.read_nal(&escaped(36, &[]), |_| {
        emitted += 1;
        Ok(())
    })?;
    assert_eq!(emitted, 1);
    assert_eq!(
        decoder.read_nal(&hash, |_| Ok(())),
        Err(Error::InvalidData("suffix_sei_without_picture"))
    );
    assert!(decoder.finish(|_| Ok(())).is_err());
    Ok(())
}

#[test]
fn slice_quantization_is_retained_without_changing_header_admission() {
    for delta in [-26, 0, 25, 26] {
        let mut w = params::tests::Writer::default();
        w.flag(true);
        w.flag(false);
        w.ue(0);
        w.ue(2);
        w.flag(true);
        w.flag(false);
        w.flag(false);
        w.se(delta);
        w.flag(false);
        let mut rbsp = padded(w);
        rbsp.push(0x80);
        let mut parser = synthetic_sets();
        let result = parser.read_nal(&escaped(19, &rbsp), |_| Ok(()));
        if delta == 26 {
            assert_eq!(result, Err(Error::InvalidData("slice_qp")));
            continue;
        }
        let NalInfo::Slice { header, .. } = result.unwrap() else {
            panic!("expected slice")
        };
        assert_eq!(header.slice_qp_y, Some(26 + delta));
        assert_eq!(header.chroma_qp_offsets, Some([0; 2]));
        assert_eq!(header.cabac_init_flag, Some(false));
    }
}

#[test]
fn selected_stream_record_inherits_syntax_but_preserves_raw_dependent_header() -> Result<()> {
    let mut sets = two_ctu_parameter_nals();
    let pps_nal = sets.get_mut(2).ok_or(Error::InvalidData("fixture_pps"))?;
    let mut pps = nal::rbsp(pps_nal, &Limits::default())?;
    *pps.first_mut().ok_or(Error::InvalidData("fixture_pps"))? |= 1 << 5;
    *pps_nal = escaped(34, &pps);
    let mut core = parser::StreamCore::new(Limits::default());
    for set in &sets {
        core.read_nal(set, |_| Ok(()))?;
    }
    let mut first_syntax = None;
    let first = idr_segment(true);
    core.read_selected(
        &first,
        |_| Ok(()),
        Some(0),
        usize::MAX,
        |input| {
            assert_eq!(input.index, 0);
            assert_eq!(input.original, first);
            assert_eq!(input.picture.width, input.sps.width);
            assert_eq!(input.pps.id, input.header.pps_id);
            assert_eq!(input.layout.at_raster(1)?.tile_scan, 1);
            assert_eq!(input.syntax.kind, 2);
            assert_eq!(input.syntax.independent_address, 0);
            assert_eq!(input.syntax.qp_y, 26);
            assert!(input.scratch_bytes >= input.rbsp.len());
            assert!(input.entry_lengths.is_empty());
            first_syntax = Some(input.syntax);
            Ok(())
        },
    )?;
    let mut writer = params::tests::Writer::default();
    writer.flag(false);
    writer.flag(false);
    writer.ue(0);
    writer.flag(true);
    writer.u(1, 1);
    let mut rbsp = padded(writer);
    rbsp.push(0x80);
    let mut observed = 0;
    let result = core.read_selected(
        &escaped(19, &rbsp),
        |_| Ok(()),
        Some(0),
        usize::MAX,
        |input| {
            observed += 1;
            assert!(input.header.dependent);
            assert_eq!(input.header.slice_qp_y, None);
            assert_eq!(input.header.slice_type, None);
            assert_eq!(Some(input.syntax), first_syntax);
            Ok(())
        },
    )?;
    assert!(matches!(
        result,
        NalInfo::Slice {
            header: SliceHeader {
                dependent: true,
                ..
            },
            ..
        }
    ));
    assert_eq!(observed, 1);
    assert_eq!(core.finish(|_| Ok(()))?.dependent_slices, 1);
    Ok(())
}

#[test]
fn selected_observer_failure_and_retained_scratch_limit_end_the_core() -> Result<()> {
    let mut core = parser::StreamCore::new(Limits::default());
    for set in synthetic_parameter_nals() {
        core.read_nal(&set, |_| Ok(()))?;
    }
    let refusal = Error::Limit("fixture_observer");
    assert_eq!(
        core.read_selected(&idr(), |_| Ok(()), Some(0), usize::MAX, |_| Err(refusal)),
        Err(refusal)
    );
    assert_eq!(
        core.finish(|_| Ok(())),
        Err(Error::InvalidData("parser_failed"))
    );
    let mut core = parser::StreamCore::new(Limits::default());
    let sets = synthetic_parameter_nals();
    let first = sets.first().ok_or(Error::InvalidData("fixture_vps"))?;
    assert_eq!(
        core.read_selected(first, |_| Ok(()), Some(0), first.len() - 3, |_| Ok(())),
        Err(Error::Limit("reconstruction_memory"))
    );
    assert_eq!(
        core.read_nal(first, |_| Ok(())),
        Err(Error::InvalidData("parser_failed"))
    );
    Ok(())
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
    let second = slice::parse(&mut bits, nal, prefix, &sps, &pps, &Limits::default(), None)
        .unwrap()
        .header;
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
