//! FLAC decoding: the examples of RFC 9639 Appendix D, the conformance
//! vectors (`COREAV_FLAC_VECTORS`), and malformed frames.

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#[path = "support/flac_ranges.rs"]
mod flac_ranges;
mod support;

use support::{decode_raw, from_dump, md5};

const EXAMPLE_1: &str = "
   00000000: 664c 6143 8000 0022 1000 1000  fLaC...\"....
   0000000c: 0000 0f00 000f 0ac4 42f0 0000  ........B...
   00000018: 0001 3e84 b418 07dc 6903 0758  ..>.....i..X
   00000024: 6a3d ad1a 2e0f fff8 6918 0000  j=......i...
   00000030: bf03 58fd 0312 8baa 9a         ..X......
";

const EXAMPLE_2: &str = "
   00000000: 664c 6143 0000 0022 0010 0010  fLaC...\"....
   0000000c: 0000 1700 0044 0ac4 42f0 0000  .....D..B...
   00000018: 0013 d5b0 5649 75e9 8b8d 8b93  ....VIu.....
   00000024: 0422 757b 8103 0300 0012 0000  .\"u{........
   00000030: 0000 0000 0000 0000 0000 0000  ............
   0000003c: 0000 0010 0400 003a 2000 0000  .......: ...
   00000048: 7265 6665 7265 6e63 6520 6c69  reference li
   00000054: 6246 4c41 4320 312e 332e 3320  bFLAC 1.3.3
   00000060: 3230 3139 3038 3034 0100 0000  20190804....
   0000006c: 0e00 0000 5449 544c 453d d7a9  ....TITLE=..
   00000078: d79c d795 d79d 8100 0006 0000  ............
   00000084: 0000 0000 fff8 6998 000f 9912  ......i.....
   00000090: 0867 0162 3d14 4299 8f5d f70d  .g.b=.B..]..
   0000009c: 6fe0 0c17 caeb 2100 0ee7 a77a  o.....!....z
   000000a8: 24a1 590c 1217 b603 097b 784f  $.Y......{xO
   000000b4: aa9a 33d2 85e0 70ad 5b1b 4851  ..3...p.[.HQ
   000000c0: b401 0d99 d2cd 1a68 f1e6 b810  .......h....
   000000cc: fff8 6918 0102 a402 c382 c40b  ..i.........
   000000d8: c14a 03ee 48dd 03b6 7c13 30    .J..H...|.0
";

const EXAMPLE_3: &str = "
   00000000: 664c 6143 8000 0022 1000 1000  fLaC...\"....
   0000000c: 0000 1f00 001f 07d0 0070 0000  .........p..
   00000018: 0018 f8f9 e396 f5cb cfc6 dc80  ............
   00000024: 7f99 7790 6b32 fff8 6802 0017  ..w.k2..h...
   00000030: e944 004f 6f31 3d10 47d2 27cb  .D.Oo1=.G.'.
   0000003c: 6d09 0831 452b dc28 2222 8057  m..1E+.(\"\".W
   00000048: a3                             .
";

#[test]
fn md5_of_rfc_1321() {
    assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(
        hex(&md5(b"message digest")),
        "f96b697d7cb7938d525a2f31aaf161d0"
    );
    let long = b"12345678901234567890123456789012345678901234567890123456789012345678901234567890";
    assert_eq!(hex(&md5(long)), "57edf4a22be3c955ac49da2e2107b67a");
}

#[test]
fn complete_validation_checks_reference_samples_and_integrity() {
    use coreav_audio::flac::{StreamInfo, validate_stream};
    use coreav_audio::{Error, LimitKind, Limits};
    for (example, offset, count) in [(EXAMPLE_1, 42, 1), (EXAMPLE_2, 136, 2), (EXAMPLE_3, 42, 1)] {
        let bytes = from_dump(example);
        let info = StreamInfo::parse(&bytes[8..42]).unwrap();
        let payload = &bytes[offset..];
        assert_eq!(
            validate_stream(info, payload, &Limits::default()).unwrap(),
            count
        );
        let mut damaged = payload.to_vec();
        *damaged.last_mut().unwrap() ^= 1;
        assert!(validate_stream(info, &damaged, &Limits::default()).is_err());
        assert!(validate_stream(info, &payload[..payload.len() - 1], &Limits::default()).is_err());
        let mut trailing = payload.to_vec();
        trailing.extend_from_slice(b"trailing");
        assert!(validate_stream(info, &trailing, &Limits::default()).is_err());
        let mut changed = info;
        changed.md5[0] ^= 1;
        assert!(validate_stream(changed, payload, &Limits::default()).is_err());
        changed = info;
        changed.total_samples += 1;
        assert!(validate_stream(changed, payload, &Limits::default()).is_err());
        assert!(matches!(
            validate_stream(
                info,
                payload,
                &Limits {
                    max_packet_bytes: payload.len() as u64 - 1,
                    ..Limits::default()
                }
            ),
            Err(Error::LimitExceeded(LimitKind::PacketBytes))
        ));
        assert!(matches!(
            validate_stream(
                info,
                payload,
                &Limits {
                    max_output_samples: 0,
                    ..Limits::default()
                }
            ),
            Err(Error::LimitExceeded(LimitKind::OutputSamples))
        ));
        let mut limits = Limits::default();
        limits.isobmff.max_retained_bytes = 0;
        assert!(matches!(
            validate_stream(info, payload, &limits),
            Err(Error::Isobmff(_))
        ));
    }
}

#[test]
fn prepared_stream_charges_both_walks_and_packing_before_start() {
    use coreav_audio::flac::{StreamInfo, prepare_stream};
    use coreav_audio::{Error, LimitKind, Limits, OutputFormat, SampleFormat, Sink};
    struct Capture {
        starts: usize,
        bytes: Vec<u8>,
        fail: bool,
    }
    impl Sink for Capture {
        fn start(&mut self, _: &OutputFormat) -> Result<(), Error> {
            self.starts += 1;
            Ok(())
        }
        fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
            if self.fail {
                return Err(Error::Output(std::io::Error::from(
                    std::io::ErrorKind::BrokenPipe,
                )));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(())
        }
    }
    let bytes = from_dump(EXAMPLE_2);
    let info = StreamInfo::parse(&bytes[8..42]).unwrap();
    let payload = &bytes[136..];
    for format in [SampleFormat::S16, SampleFormat::F32] {
        let scalar = info.total_samples * u64::from(info.channels);
        let block = u64::from(info.max_block_size);
        let scratch = block * (u64::from(info.channels) * (4 + format.bytes() as u64) + 8) + 1024;
        let mut limits = Limits::default();
        limits.isobmff.max_sample_work = scalar * 128;
        limits.isobmff.max_retained_bytes = scratch;
        let mut out = Capture {
            starts: 0,
            bytes: vec![],
            fail: false,
        };
        prepare_stream(info, payload, format, &limits)
            .unwrap()
            .write(&mut out)
            .unwrap();
        assert_eq!(out.starts, 1);
        assert_eq!(out.bytes.len() as u64, scalar * format.bytes() as u64);
        let mut limited = limits;
        limited.isobmff.max_sample_work -= 1;
        assert!(matches!(
            prepare_stream(info, payload, format, &limited),
            Err(Error::LimitExceeded(LimitKind::OutputSamples))
        ));
        limited = limits;
        limited.max_output_samples = scalar - 1;
        assert!(matches!(
            prepare_stream(info, payload, format, &limited),
            Err(Error::LimitExceeded(LimitKind::OutputSamples))
        ));
        limited = limits;
        limited.max_packet_bytes = payload.len() as u64 - 1;
        assert!(matches!(
            prepare_stream(info, payload, format, &limited),
            Err(Error::LimitExceeded(LimitKind::PacketBytes))
        ));
        limited = limits;
        limited.isobmff.max_retained_bytes -= 1;
        assert!(matches!(
            prepare_stream(info, payload, format, &limited),
            Err(Error::Isobmff(coreav_isobmff::Error::LimitExceeded {
                limit: coreav_isobmff::LimitKind::RetainedBytes,
                ..
            }))
        ));
        let mut damaged = payload.to_vec();
        *damaged.last_mut().unwrap() ^= 1;
        assert!(prepare_stream(info, &damaged, format, &limits).is_err());
        let mut zero_md5 = info;
        zero_md5.md5 = [0; 16];
        let mut again = Capture {
            starts: 0,
            bytes: vec![],
            fail: false,
        };
        prepare_stream(zero_md5, payload, format, &limits)
            .unwrap()
            .write(&mut again)
            .unwrap();
        assert_eq!(again.bytes, out.bytes);
        let mut failing = Capture {
            starts: 0,
            bytes: vec![],
            fail: true,
        };
        assert!(
            matches!(prepare_stream(info, payload, format, &limits).unwrap().write(&mut failing), Err(Error::Output(error)) if error.kind() == std::io::ErrorKind::BrokenPipe)
        );
        assert_eq!(failing.starts, 1);
    }
}

#[test]
fn invalid_reconstructed_ranges_fail_decoder_validator_and_preparation() {
    use coreav_audio::flac::{FlacDecoder, StreamInfo, prepare_stream, validate_stream};
    use coreav_audio::{Error, Limits, Pcm, SampleFormat};
    for bits in [16, 24] {
        let half = 1i64 << (bits - 1);
        for (assignment, values, fixed) in [
            (0, [half, 0], true),
            (8, [half - 1, -2 * half], false),
            (9, [2 * half - 1, half - 1], false),
            (10, [half - 1, 2 * half - 1], false),
        ] {
            for late in [false, true] {
                let bytes = flac_ranges::file(bits, assignment, values, fixed, late);
                let info = StreamInfo::parse(&bytes[8..42]).unwrap();
                let payload = &bytes[42..];
                let limits = Limits::default();
                let mut decoder = FlacDecoder::new(info, &limits).unwrap();
                let mut pcm = Pcm::new();
                let offset = if late {
                    decoder.decode(payload, &mut pcm).unwrap().bytes
                } else {
                    0
                };
                assert!(matches!(
                    decoder.decode(&payload[offset..], &mut pcm),
                    Err(Error::Invalid(_))
                ));
                assert!(matches!(
                    validate_stream(info, payload, &limits),
                    Err(Error::Invalid(_))
                ));
                for packing in [SampleFormat::S16, SampleFormat::F32] {
                    assert!(matches!(
                        prepare_stream(info, payload, packing, &limits),
                        Err(Error::Invalid(_))
                    ));
                }
            }
        }
    }
}

fn constant_frame(number: u8, variable: bool, block: u8) -> Vec<u8> {
    let header = vec![
        0xff,
        0xf8 | u8::from(variable),
        0x69,
        0x08,
        number,
        block - 1,
    ];
    checked_frame(header, &[0, 0, 0])
}

fn checked_frame(mut bytes: Vec<u8>, subframes: &[u8]) -> Vec<u8> {
    let mut crc8 = 0u8;
    for &byte in &bytes {
        crc8 ^= byte;
        for _ in 0..8 {
            crc8 = (crc8 << 1) ^ if crc8 & 0x80 != 0 { 7 } else { 0 };
        }
    }
    bytes.push(crc8);
    bytes.extend_from_slice(subframes);
    let mut crc16 = 0u16;
    for &byte in &bytes {
        crc16 ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc16 = (crc16 << 1) ^ if crc16 & 0x8000 != 0 { 0x8005 } else { 0 };
        }
    }
    bytes.extend_from_slice(&crc16.to_be_bytes());
    bytes
}

#[test]
fn complete_validation_rejects_crc_correct_reserved_fields_and_padding() {
    use coreav_audio::Limits;
    use coreav_audio::flac::{StreamInfo, validate_stream};
    let mut info = StreamInfo {
        min_block_size: 16,
        max_block_size: 16,
        min_frame_size: 0,
        max_frame_size: 0,
        sample_rate: 44_100,
        channels: 1,
        bits_per_sample: 16,
        total_samples: 16,
        md5: [0; 16],
    };
    for header in [
        vec![0xff, 0xf8, 0x69, 0x09, 0, 15],
        vec![0xff, 0xf8, 0x69, 0x08, 0xc0, 0x80, 15],
    ] {
        let payload = checked_frame(header, &[0, 0, 0]);
        assert!(validate_stream(info, &payload, &Limits::default()).is_err());
    }
    info.bits_per_sample = 20;
    let valid = checked_frame(vec![0xff, 0xf8, 0x69, 0x0a, 0, 15], &[0, 0, 0, 0]);
    assert_eq!(
        validate_stream(info, &valid, &Limits::default()).unwrap(),
        1
    );
    let damaged = checked_frame(vec![0xff, 0xf8, 0x69, 0x0a, 0, 15], &[0, 0, 0, 1]);
    assert!(validate_stream(info, &damaged, &Limits::default()).is_err());
}

#[test]
fn complete_validation_requires_zero_origin_and_contiguous_numbers() {
    use coreav_audio::Limits;
    use coreav_audio::flac::{StreamInfo, validate_stream};
    let mut info = StreamInfo {
        min_block_size: 16,
        max_block_size: 16,
        min_frame_size: 0,
        max_frame_size: 0,
        sample_rate: 44_100,
        channels: 1,
        bits_per_sample: 16,
        total_samples: 32,
        md5: [0; 16],
    };
    for (sequence, valid) in [
        (vec![(0, false, 16), (1, false, 16)], true),
        (vec![(0, true, 16), (16, true, 16)], true),
        (vec![(1, false, 16), (2, false, 16)], false),
        (vec![(16, true, 16), (32, true, 16)], false),
        (vec![(0, false, 16), (0, false, 16)], false),
        (vec![(0, false, 16), (2, false, 16)], false),
        (vec![(0, false, 16), (16, true, 16)], false),
        (vec![(0, false, 16), (1, false, 8)], true),
    ] {
        info.total_samples = sequence.iter().map(|(_, _, block)| u64::from(*block)).sum();
        let payload: Vec<u8> = sequence
            .into_iter()
            .flat_map(|(number, variable, block)| constant_frame(number, variable, block))
            .collect();
        assert_eq!(
            validate_stream(info, &payload, &Limits::default()).is_ok(),
            valid
        );
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn example_1_two_verbatim_samples() {
    let decoded = decode_raw(&from_dump(EXAMPLE_1)).unwrap();
    assert_eq!(decoded.first, [25588, 10416]);
    assert_eq!(decoded.md5, decoded.info.md5);
    assert_eq!(decoded.crc_failures, 0);
}

#[test]
fn example_2_fixed_predictor_and_side_channel() {
    let decoded = decode_raw(&from_dump(EXAMPLE_2)).unwrap();
    assert_eq!(decoded.frames, 2);
    assert_eq!(decoded.samples, 19);
    // D.2.9 lists the samples interleaved: 0x2884 = 10372, 0x17B6 = 6070.
    assert_eq!(&decoded.first[..2], &[10372, 6070]);
    assert_eq!(decoded.md5, decoded.info.md5);
    assert_eq!(decoded.crc_failures, 0);
}

#[test]
fn example_3_linear_predictor_and_partitions() {
    let decoded = decode_raw(&from_dump(EXAMPLE_3)).unwrap();
    assert_eq!(decoded.samples, 24);
    assert_eq!(decoded.md5, decoded.info.md5);
    assert_eq!(decoded.crc_failures, 0);
}

/// The IETF CELLAR test files (`ietf-wg-cellar/flac-test-files`), in the
/// directory `COREAV_FLAC_VECTORS` names: every file of `subset` must decode
/// to the MD5 of its streaminfo block; `uncommon` files decode the same
/// way or are refused with `Unsupported`; `faulty` files fail or decode,
/// never panic. Files without the `fLaC` signature need a raw-stream
/// demuxer and are skipped. Run by the audio-conformance workflow.
#[test]
#[ignore = "needs COREAV_FLAC_VECTORS"]
fn conformance_vectors() {
    let root = std::path::PathBuf::from(
        std::env::var("COREAV_FLAC_VECTORS").expect("COREAV_FLAC_VECTORS"),
    );
    let mut failures = Vec::new();
    let mut passed = 0;
    for set in ["subset", "uncommon", "faulty"] {
        let mut files: Vec<_> = std::fs::read_dir(root.join(set))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "flac"))
            .collect();
        files.sort();
        for path in files {
            let name = format!("{set}/{}", path.file_name().unwrap().to_string_lossy());
            let data = std::fs::read(&path).unwrap();
            if !data.starts_with(b"fLaC") {
                // uncommon/10 and 11 start at a frame or with junk: finding
                // frames in a raw stream is the raw FLAC demuxer's.
                println!("skipped {name}: no fLaC signature");
                continue;
            }
            let result = std::panic::catch_unwind(|| decode_raw(&data));
            match (set, result) {
                (_, Err(_)) => failures.push(format!("{name}: panicked")),
                ("faulty", Ok(_)) => passed += 1,
                (_, Ok(Ok(d))) if d.info.md5 == [0; 16] || d.md5 == d.info.md5 => {
                    println!("ok   {name} ({} samples)", d.samples);
                    passed += 1;
                }
                (_, Ok(Ok(_))) => failures.push(format!("{name}: MD5 mismatch")),
                ("uncommon", Ok(Err(coreav_audio::Error::Unsupported(why)))) => {
                    println!("refused {name}: {why}");
                    passed += 1;
                }
                (_, Ok(Err(e))) => failures.push(format!("{name}: {e}")),
            }
        }
    }
    println!("{passed} passed");
    assert!(failures.is_empty(), "{failures:#?}");
}
