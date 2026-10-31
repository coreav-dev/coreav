//! The RFC 8251 test vectors (`docs/design/audio-decode.md`, "Opus bars",
//! bar 1) through what this crate has so far: every packet of the twelve
//! `.bit` files is a valid Opus packet, and their durations add up to the
//! length of both reference outputs (`.dec` and `m.dec`, 48 kHz stereo,
//! 16-bit). The decoder's comparisons join this test when it lands.
//!
//! Run by `.github/workflows/audio-conformance.yml`, or by hand:
//! `COREAV_OPUS_VECTORS=<dir> cargo test -p coreav-opus --test vectors --
//! --ignored`, with `<dir>` the unpacked archive that RFC 8251 section 11
//! names (its files checked against the SHA-1 hashes printed there).

#![allow(clippy::expect_used, clippy::panic)]

use coreav_opus::Packet;
use std::path::PathBuf;

/// The packets of a `.bit` file: each a 4-byte big-endian length, the
/// encoder's final range (4 bytes, big-endian), then the packet; the layout
/// read from the files themselves.
fn packets(data: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let (len, tail) = rest.split_first_chunk::<4>().expect("length");
        let (range, tail) = tail.split_first_chunk::<4>().expect("range");
        let len = u32::from_be_bytes(*len) as usize;
        let (packet, tail) = tail.split_at(len);
        out.push((u32::from_be_bytes(*range), packet));
        rest = tail;
    }
    out
}

#[test]
#[ignore = "needs COREAV_OPUS_VECTORS"]
fn rfc8251_vectors_packets() {
    let dir = PathBuf::from(std::env::var_os("COREAV_OPUS_VECTORS").expect("COREAV_OPUS_VECTORS"));
    let mut total = 0;
    for n in 1..=12 {
        let name = format!("testvector{n:02}");
        let bit = std::fs::read(dir.join(format!("{name}.bit"))).expect(".bit");
        let mut samples = 0u64;
        let list = packets(&bit);
        for (i, (_, data)) in list.iter().enumerate() {
            let packet = Packet::parse(data).unwrap_or_else(|e| panic!("{name} packet {i}: {e}"));
            samples += u64::from(packet.samples());
        }
        for suffix in ["", "m"] {
            let dec = dir.join(format!("{name}{suffix}.dec"));
            let bytes = std::fs::metadata(&dec).expect(".dec").len();
            assert_eq!(bytes, samples * 4, "{name}{suffix}.dec: 48 kHz stereo s16");
        }
        println!("{name}: {} packets, {samples} samples", list.len());
        total += list.len();
    }
    println!("{total} packets");
}

/// Decodes a vector to `channels` channels as 16-bit samples.
fn decode_vector(bit: &[u8], channels: u8) -> Vec<i16> {
    let mut decoder = coreav_opus::Decoder::new(channels).expect("decoder");
    let mut pcm = Vec::new();
    for (i, (range, data)) in packets(bit).into_iter().enumerate() {
        decoder
            .decode(data, &mut pcm)
            .unwrap_or_else(|e| panic!("packet {i}: {e}"));
        if data.len() > 1 {
            assert_eq!(decoder.final_range(), range, "packet {i}");
        }
    }
    pcm.iter()
        .map(|&x| (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
        .collect()
}

/// The SNR of `ours` against the reference, in dB.
fn snr(reference: &[i16], ours: &[i16]) -> f64 {
    let (mut signal, mut noise) = (0f64, 0f64);
    for (&r, &o) in reference.iter().zip(ours) {
        signal += f64::from(r).powi(2);
        noise += (f64::from(r) - f64::from(o)).powi(2);
    }
    10.0 * (signal / noise.max(1.0)).log10()
}

/// The vectors of the modes decoded so far, SILK-only (02 to 04) and
/// CELT-only (01, 07, 11), decoded to stereo and to mono, every packet
/// ending in the encoder's range state, and compared with the reference
/// output by `opus_compare` (RFC 6716 section 6.1) when
/// `COREAV_OPUS_COMPARE` names it: Opus bar 1 asks a quality above 90
/// (`docs/design/audio-decode.md`). The reference outputs are the `.dec`
/// set, which the RFC's code with RFC 8251's changes reproduces (the `m.dec`
/// set is another decoder's; the two are the same for the SILK-only
/// vectors, 10 and 12).
#[test]
#[ignore = "needs COREAV_OPUS_VECTORS"]
fn rfc8251_vectors_decode() {
    let dir = PathBuf::from(std::env::var_os("COREAV_OPUS_VECTORS").expect("COREAV_OPUS_VECTORS"));
    let compare = std::env::var_os("COREAV_OPUS_COMPARE");
    let tmp = std::env::temp_dir();
    for n in 1..=12 {
        let silk = (2..=4).contains(&n);
        let name = format!("testvector{n:02}");
        let bit = std::fs::read(dir.join(format!("{name}.bit"))).expect(".bit");
        let reference: Vec<i16> = std::fs::read(dir.join(format!("{name}.dec")))
            .expect(".dec")
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        for channels in [2u8, 1] {
            let ours = decode_vector(&bit, channels);
            if channels == 2 && silk {
                // SILK's reconstruction and resampler are the reference's:
                // the output is the reference output, sample for sample.
                assert!(ours == reference, "{name}: not the reference output");
            }
            let stereo_snr = if channels == 2 {
                snr(&reference, &ours)
            } else {
                f64::NAN
            };
            let out = tmp.join(format!("coreav-{name}-{channels}.sw"));
            let bytes: Vec<u8> = ours.iter().flat_map(|s| s.to_le_bytes()).collect();
            std::fs::write(&out, bytes).expect("write");
            let mut line = format!("{name} {channels} ch: SNR {stereo_snr:.1} dB");
            if let Some(tool) = &compare {
                let mut cmd = std::process::Command::new(tool);
                if channels == 2 {
                    cmd.arg("-s");
                }
                let result = cmd
                    .args(["-r", "48000"])
                    .arg(dir.join(format!("{name}.dec")))
                    .arg(&out)
                    .output()
                    .expect("opus_compare");
                let text = String::from_utf8_lossy(&result.stderr).into_owned()
                    + &String::from_utf8_lossy(&result.stdout);
                line += &format!(", opus_compare: {}", text.trim().replace('\n', " | "));
                assert!(result.status.success(), "{line}");
                let quality: f64 = text
                    .split("quality metric: ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|q| q.parse().ok())
                    .expect("opus_compare's quality");
                assert!(quality > 90.0, "{line}");
            }
            println!("{line}");
        }
    }
}

/// FNV-1a over the bits of every sample the decoder gives for a vector.
fn output_hash(bit: &[u8], channels: u8) -> u64 {
    let mut decoder = coreav_opus::Decoder::new(channels).expect("decoder");
    let mut pcm = Vec::new();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for (i, (_, data)) in packets(bit).into_iter().enumerate() {
        pcm.clear();
        decoder
            .decode(data, &mut pcm)
            .unwrap_or_else(|e| panic!("packet {i}: {e}"));
        for x in &pcm {
            hash = (hash ^ u64::from(x.to_bits())).wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

/// The decoder's output, every sample's bits, on each vector in stereo and
/// in mono, as the agreement bar measured it (`docs/design/audio-decode.md`,
/// "Opus bars"): speed work keeps it.
#[test]
#[ignore = "needs COREAV_OPUS_VECTORS"]
fn rfc8251_vectors_output_unchanged() {
    let dir = PathBuf::from(std::env::var_os("COREAV_OPUS_VECTORS").expect("COREAV_OPUS_VECTORS"));
    let mut got = Vec::new();
    for n in 1..=12 {
        let bit = std::fs::read(dir.join(format!("testvector{n:02}.bit"))).expect(".bit");
        got.push([output_hash(&bit, 2), output_hash(&bit, 1)]);
    }
    for [stereo, mono] in &got {
        println!("[{stereo:#018x}, {mono:#018x}],");
    }
    assert_eq!(got, OUTPUT_HASHES);
}

/// [stereo, mono] per vector, 01 to 12.
const OUTPUT_HASHES: [[u64; 2]; 12] = [
    [0x582a_a15b_2333_049f, 0x0a9a_6f51_551f_7819],
    [0xb561_5650_dce3_4825, 0xfcc0_ab57_f730_01a5],
    [0x2e69_2762_25b6_5725, 0xb992_3177_b721_8125],
    [0xcf3a_a774_0a82_1025, 0x6023_98d7_fea2_f1a5],
    [0x203b_c907_0a56_ba4d, 0x1bbb_81a0_1d14_bfa0],
    [0x2e5d_47fd_ae93_c800, 0x29b1_ed13_7017_5d08],
    [0x36ee_9248_c389_36af, 0x154a_e84d_c55b_2840],
    [0x1577_e5ab_ed67_19a5, 0x0830_d6e5_9ee1_102f],
    [0xf5a3_111c_1d4e_bd76, 0x8440_85df_4d21_6379],
    [0xb7fb_4cf7_d137_d813, 0xb6cd_3eb1_b363_2238],
    [0x7a3c_bbd0_6be6_150d, 0xf3e6_0f45_6afd_9ede],
    [0xb98b_9250_c19f_ae05, 0x149f_389d_ca24_63b3],
];
