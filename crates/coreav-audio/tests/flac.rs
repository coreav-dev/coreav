//! FLAC decoding: the examples of RFC 9639 Appendix D, the conformance
//! vectors (`COREAV_FLAC_VECTORS`), and malformed frames.

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
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
