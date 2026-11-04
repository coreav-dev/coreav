#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use super::*;
use coreav_isobmff::SliceSource;

fn chunk(id: &[u8; 4], bytes: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    if bytes.len() % 2 != 0 {
        out.push(0);
    }
    out
}

fn fmt(bits: u16, code: u16, channels: u16) -> Vec<u8> {
    let mut out = Vec::new();
    let align = channels * (bits / 8);
    out.extend_from_slice(&code.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&48000u32.to_le_bytes());
    out.extend_from_slice(&(48000 * u32::from(align)).to_le_bytes());
    out.extend_from_slice(&align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out
}

fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"RIFF".to_vec();
    let size: usize = chunks.iter().map(Vec::len).sum();
    out.extend_from_slice(&((size + 4) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    for c in chunks {
        out.extend_from_slice(c);
    }
    out
}

fn report(bytes: &[u8]) -> Report {
    probe(
        &mut SliceSource::new(bytes),
        bytes.len() as u64,
        "test.wav",
        &Limits::default(),
    )
    .unwrap_or_else(|e| panic!("{}", e.to_json()))
}

fn field<'a>(report: &'a Report, key: &str) -> &'a Value {
    &report.streams[0]
        .entries
        .iter()
        .find(|(k, _)| *k == key)
        .unwrap()
        .1
}

#[test]
fn pcm_descriptor_preserves_the_data_span_without_reading_samples() {
    let mut info = b"INFO".to_vec();
    info.extend_from_slice(&chunk(b"INAM", b"title\0"));
    let bytes = riff(&[
        chunk(b"fmt ", &fmt(16, 1, 1)),
        chunk(b"JUNK", &[1, 2, 3]),
        chunk(b"data", &[0x51; 8]),
        chunk(b"LIST", &info),
    ]);
    struct HeaderOnly<'a>(&'a [u8]);
    impl Source for HeaderOnly<'_> {
        fn len(&mut self) -> Result<u64, Error> {
            Ok(self.0.len() as u64)
        }
        fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
            assert!(at + out.len() as u64 <= 56 || at >= 64);
            SliceSource::new(self.0).read_exact_at(at, out)
        }
    }
    let (report, pcm) = probe_pcm(
        &mut HeaderOnly(&bytes),
        bytes.len() as u64,
        "test.wav",
        &Limits::default(),
    )
    .unwrap_or_else(|e| panic!("{}", e.to_json()));
    assert_eq!(
        pcm,
        Pcm {
            encoding: PcmEncoding::S16,
            valid_bits: 16,
            channels: 1,
            rate: 48000,
            mask: 0,
            offset: 56,
            bytes: 8,
            declared_bytes: 8
        }
    );
    assert_eq!(field(&report, "duration_ts"), &Value::Int(4));
    assert_eq!(
        report.format_tags,
        vec![("title".to_owned(), "title".to_owned())]
    );
}

#[test]
fn pcm_descriptor_keeps_declared_extents_and_valid_precision_separate() {
    let mut format = fmt(32, 0xfffe, 2);
    format.extend_from_slice(&22u16.to_le_bytes());
    format.extend_from_slice(&24u16.to_le_bytes());
    format.extend_from_slice(&3u32.to_le_bytes());
    format.extend_from_slice(&[1, 0, 0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113]);
    let mut bytes = riff(&[chunk(b"fmt ", &format), chunk(b"data", &[0; 16])]);
    bytes.truncate(bytes.len() - 8);
    let (report, pcm) = probe_pcm(
        &mut SliceSource::new(&bytes),
        bytes.len() as u64,
        "test.wav",
        &Limits::default(),
    )
    .unwrap_or_else(|e| panic!("{}", e.to_json()));
    assert_eq!(pcm.encoding, PcmEncoding::S32);
    assert_eq!(pcm.valid_bits, 24);
    assert_eq!(pcm.declared_bytes, 16);
    assert_eq!(pcm.bytes, 8);
    assert_eq!(field(&report, "duration_ts"), &Value::Int(1));
}

#[test]
fn pcm_and_float_fields_use_storage_width() {
    for (code, bits, codec, sample_fmt) in [
        (1, 8, "pcm_u8", "u8"),
        (1, 16, "pcm_s16le", "s16"),
        (1, 24, "pcm_s24le", "s32"),
        (1, 32, "pcm_s32le", "s32"),
        (3, 32, "pcm_f32le", "flt"),
        (3, 64, "pcm_f64le", "dbl"),
    ] {
        let bytes = riff(&[
            chunk(b"fmt ", &fmt(bits, code, 2)),
            chunk(b"data", &vec![0; 480 * 2 * usize::from(bits / 8)]),
        ]);
        let r = report(&bytes);
        assert_eq!(field(&r, "codec_name"), &s(codec));
        assert_eq!(field(&r, "sample_fmt"), &s(sample_fmt));
        assert_eq!(field(&r, "duration_ts"), &Value::Int(480));
    }
}

#[test]
fn extensible_valid_bits_do_not_change_container_width() {
    for (bits, valid) in [(24, 20u16), (24, 24), (32, 24), (32, 32)] {
        let mut format = fmt(bits, 0xfffe, 2);
        format.extend_from_slice(&22u16.to_le_bytes());
        format.extend_from_slice(&valid.to_le_bytes());
        format.extend_from_slice(&3u32.to_le_bytes());
        format.extend_from_slice(&[1, 0, 0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113]);
        let bytes = riff(&[
            chunk(b"fmt ", &format),
            chunk(b"data", &vec![0; 480 * 2 * usize::from(bits / 8)]),
        ]);
        let r = report(&bytes);
        assert_eq!(
            field(&r, "codec_name"),
            &s(if bits == 24 { "pcm_s24le" } else { "pcm_s32le" })
        );
        assert_eq!(field(&r, "channel_layout"), &s("stereo"));
        assert_eq!(field(&r, "duration_ts"), &Value::Int(480));
    }
}

#[test]
fn partial_pcm_frames_keep_the_existing_fallback() {
    let mut bytes = riff(&[chunk(b"fmt ", &fmt(16, 1, 2)), chunk(b"data", &[0; 1920])]);
    bytes.truncate(bytes.len() - 1);
    assert!(matches!(
        probe(
            &mut SliceSource::new(&bytes),
            bytes.len() as u64,
            "test",
            &Limits::default()
        ),
        Err(Failure::Unsupported(_))
    ));
}

#[test]
fn info_tags_preserve_aliases_and_duplicate_order() {
    let mut info = b"INFO".to_vec();
    for (id, value) in [
        (b"INAM", b"first\0".as_slice()),
        (b"IART", b"artist\0".as_slice()),
        (b"IXXX", b"custom\0".as_slice()),
        (b"INAM", b"second\0".as_slice()),
    ] {
        info.extend_from_slice(&chunk(id, value));
    }
    let bytes = riff(&[
        chunk(b"fmt ", &fmt(16, 1, 2)),
        chunk(b"JUNK", &[1]),
        chunk(b"LIST", &info),
        chunk(b"data", &[0; 1920]),
    ]);
    assert_eq!(
        report(&bytes).format_tags,
        vec![
            ("IXXX".into(), "custom".into()),
            ("artist".into(), "artist".into()),
            ("title".into(), "second".into())
        ]
    );
}

#[test]
fn rf64_and_bw64_use_ds64_sizes_not_dummy_sample_count() {
    for magic in [b"RF64", b"BW64"] {
        let format = chunk(b"fmt ", &fmt(16, 1, 2));
        let mut ds = Vec::new();
        let size = 4 + 36 + format.len() as u64 + 8 + 1920;
        ds.extend_from_slice(&size.to_le_bytes());
        ds.extend_from_slice(&1920u64.to_le_bytes());
        ds.extend_from_slice(&12345u64.to_le_bytes());
        ds.extend_from_slice(&0u32.to_le_bytes());
        let mut bytes = magic.to_vec();
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(&chunk(b"ds64", &ds));
        bytes.extend_from_slice(&format);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0; 1920]);
        assert_eq!(field(&report(&bytes), "duration_ts"), &Value::Int(480));
    }
}

#[test]
fn caller_limits_fail_before_extra_work() {
    let bytes = riff(&[chunk(b"fmt ", &fmt(16, 1, 2)), chunk(b"data", &[0; 1920])]);
    for limits in [
        Limits {
            max_tracks: 0,
            ..Limits::default()
        },
        Limits {
            max_total_boxes: 1,
            ..Limits::default()
        },
        Limits {
            max_top_level_boxes: 1,
            ..Limits::default()
        },
        Limits {
            max_retained_bytes: 1,
            ..Limits::default()
        },
        Limits {
            max_table_bytes: 1,
            ..Limits::default()
        },
        Limits {
            max_box_bytes: 15,
            ..Limits::default()
        },
        Limits {
            max_sample_work: 1,
            ..Limits::default()
        },
        Limits {
            max_depth: 0,
            ..Limits::default()
        },
    ] {
        let err = probe(
            &mut SliceSource::new(&bytes),
            bytes.len() as u64,
            "test",
            &limits,
        )
        .err()
        .unwrap();
        assert!(
            matches!(err, Failure::Library(Error::LimitExceeded { .. })),
            "{}",
            err.to_json()
        );
    }
}

#[test]
fn source_errors_are_not_optional_metadata() {
    struct Broken;
    impl Source for Broken {
        fn len(&mut self) -> Result<u64, Error> {
            Ok(100)
        }
        fn read_exact_at(&mut self, _: u64, _: &mut [u8]) -> Result<(), Error> {
            Err(Error::Io(std::io::ErrorKind::PermissionDenied))
        }
    }
    let err = probe(&mut Broken, 100, "test", &Limits::default())
        .err()
        .unwrap();
    assert!(matches!(
        err,
        Failure::Library(Error::Io(std::io::ErrorKind::PermissionDenied))
    ));
}

#[test]
fn malformed_metadata_fails_and_unknown_metadata_refuses() {
    let bytes = riff(&[chunk(b"fmt ", &[0; 15])]);
    assert!(matches!(
        probe(
            &mut SliceSource::new(&bytes),
            bytes.len() as u64,
            "test",
            &Limits::default()
        ),
        Err(Failure::Invalid(_))
    ));
    let bytes = riff(&[
        chunk(b"fmt ", &fmt(16, 1, 2)),
        chunk(b"bext", &[0; 16]),
        chunk(b"data", &[0; 1920]),
    ]);
    assert!(matches!(
        probe(
            &mut SliceSource::new(&bytes),
            bytes.len() as u64,
            "test",
            &Limits::default()
        ),
        Err(Failure::Unsupported(_))
    ));
}

#[test]
fn aligned_truncation_uses_available_sample_frames() {
    let mut bytes = riff(&[chunk(b"fmt ", &fmt(16, 1, 2)), chunk(b"data", &[0; 1920])]);
    bytes.truncate(bytes.len() - 8);
    assert_eq!(field(&report(&bytes), "duration_ts"), &Value::Int(478));
    let extent = bytes.len() as u32 - 8;
    bytes[4..8].copy_from_slice(&extent.to_le_bytes());
    assert_eq!(field(&report(&bytes), "duration_ts"), &Value::Int(478));
}

#[test]
fn compressed_wav_with_zero_sample_width_keeps_fallback() {
    let bytes = riff(&[chunk(b"fmt ", &fmt(0, 0x55, 2)), chunk(b"data", &[0; 8])]);
    assert!(matches!(
        probe(
            &mut SliceSource::new(&bytes),
            bytes.len() as u64,
            "test",
            &Limits::default()
        ),
        Err(Failure::Unsupported(_))
    ));
}

#[test]
fn all_guid_bytes_are_required_for_pcm() {
    let mut format = fmt(16, 0xfffe, 2);
    format.extend_from_slice(&22u16.to_le_bytes());
    format.extend_from_slice(&16u16.to_le_bytes());
    format.extend_from_slice(&3u32.to_le_bytes());
    format.extend_from_slice(&[1, 0, 0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 114]);
    let bytes = riff(&[chunk(b"fmt ", &format), chunk(b"data", &[0; 8])]);
    assert!(matches!(
        probe(
            &mut SliceSource::new(&bytes),
            bytes.len() as u64,
            "test",
            &Limits::default()
        ),
        Err(Failure::Unsupported(_))
    ));
}

#[test]
fn duration_does_not_read_sample_payloads() {
    struct Headers<'a>(&'a [u8]);
    impl Source for Headers<'_> {
        fn len(&mut self) -> Result<u64, Error> {
            Ok(self.0.len() as u64)
        }
        fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
            assert!(offset + out.len() as u64 <= 44);
            SliceSource::new(self.0).read_exact_at(offset, out)
        }
    }
    let bytes = riff(&[chunk(b"fmt ", &fmt(16, 1, 2)), chunk(b"data", &[0; 1920])]);
    let r = probe(
        &mut Headers(&bytes),
        bytes.len() as u64,
        "test",
        &Limits::default(),
    )
    .unwrap_or_else(|e| panic!("{}", e.to_json()));
    assert_eq!(field(&r, "duration_ts"), &Value::Int(480));
}

#[test]
fn info_reads_preserve_source_io_and_limit_failures() {
    struct Broken<'a> {
        bytes: &'a [u8],
        error: Error,
    }
    impl Source for Broken<'_> {
        fn len(&mut self) -> Result<u64, Error> {
            Ok(self.bytes.len() as u64)
        }
        fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
            if offset >= 56 {
                return Err(self.error.clone());
            }
            SliceSource::new(self.bytes).read_exact_at(offset, out)
        }
    }
    let mut info = b"INFO".to_vec();
    info.extend_from_slice(&chunk(b"INAM", b"title\0"));
    let bytes = riff(&[
        chunk(b"fmt ", &fmt(16, 1, 2)),
        chunk(b"LIST", &info),
        chunk(b"data", &[0; 8]),
    ]);
    for error in [
        Error::Io(std::io::ErrorKind::PermissionDenied),
        Error::LimitExceeded {
            limit: LimitKind::ReadBytes,
            offset: 56,
        },
    ] {
        let mut source = Broken {
            bytes: &bytes,
            error: error.clone(),
        };
        let err = probe(&mut source, bytes.len() as u64, "test", &Limits::default())
            .err()
            .unwrap();
        assert!(matches!(err, Failure::Library(actual) if actual == error));
    }
}

#[test]
fn ds64_table_duplicate_ids_are_consumed_in_order() {
    let format = chunk(b"fmt ", &fmt(16, 1, 2));
    let mut ds = Vec::new();
    let size = 4 + 60 + format.len() as u64 + 10 + 12 + 8 + 8;
    ds.extend_from_slice(&size.to_le_bytes());
    ds.extend_from_slice(&8u64.to_le_bytes());
    ds.extend_from_slice(&2u64.to_le_bytes());
    ds.extend_from_slice(&2u32.to_le_bytes());
    for len in [2u64, 4] {
        ds.extend_from_slice(b"JUNK");
        ds.extend_from_slice(&len.to_le_bytes());
    }
    let mut bytes = b"RF64".to_vec();
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(&chunk(b"ds64", &ds));
    bytes.extend_from_slice(&format);
    for len in [2, 4] {
        bytes.extend_from_slice(b"JUNK");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&vec![0; len]);
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&[0; 8]);
    assert_eq!(field(&report(&bytes), "duration_ts"), &Value::Int(2));
}

#[test]
fn malformed_info_extent_is_not_silently_omitted() {
    let mut info = b"INFOINAM".to_vec();
    info.extend_from_slice(&100u32.to_le_bytes());
    info.push(0);
    let bytes = riff(&[
        chunk(b"fmt ", &fmt(16, 1, 2)),
        chunk(b"LIST", &info),
        chunk(b"data", &[0; 8]),
    ]);
    assert!(matches!(
        probe(
            &mut SliceSource::new(&bytes),
            bytes.len() as u64,
            "test",
            &Limits::default()
        ),
        Err(Failure::Invalid(_))
    ));
}

#[test]
fn named_and_custom_masks_keep_their_speaker_positions() {
    for (mask, expected) in [
        (0x10f, "4.1"),
        (0x707, "6.0"),
        (0x6c3, "6.0(front)"),
        (0x137, "hexagonal"),
        (0x6cb, "6.1(front)"),
        (0x637, "7.0"),
        (0x6c7, "7.0(front)"),
        (0xff, "7.1(wide)"),
        (0x6cf, "7.1(wide-side)"),
        (0x737, "octagonal"),
        (0x2d033, "cube"),
        (0x563f, "7.1.2"),
        (0x560f, "5.1.2"),
        (0x2d60f, "5.1.4"),
        (0x2d63f, "7.1.4"),
        (0x2d6ff, "9.1.4"),
        (1, "1 channels (FL)"),
    ] {
        assert_eq!(
            layout(mask).unwrap_or_else(|e| panic!("{}", e.to_json())),
            s(expected)
        );
    }
}
