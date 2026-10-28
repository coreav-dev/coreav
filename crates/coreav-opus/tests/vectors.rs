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
    for n in [1, 2, 3, 4, 7, 11] {
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
