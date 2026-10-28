//! MP3 decoding: the ISO/IEC 11172-4 Layer III compliance bitstreams
//! (`COREAV_MP3_VECTORS`), and malformed frames.

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use coreav_audio::mp3::{Header, Mp3Decoder};
use coreav_audio::{Error, Limits, Pcm, Plane};

/// Splits a raw Layer III stream into frames by their headers' lengths
/// (free format: up to the next sync word that parses).
fn frames(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut free_len = None;
    while at + 4 <= data.len() {
        let Ok(header) = Header::parse(&data[at..]) else {
            at += 1;
            continue;
        };
        let len = match header.frame_bytes() {
            Some(n) => n,
            None => {
                *free_len.get_or_insert_with(|| {
                    (at + 4..data.len())
                        .find(|&n| {
                            Header::parse(&data[n..])
                                .is_ok_and(|h| h.sample_rate == header.sample_rate)
                        })
                        .map_or(data.len() - at, |n| n - at)
                        - usize::from(header.padding)
                }) + usize::from(header.padding)
            }
        };
        let end = (at + len).min(data.len());
        out.push(&data[at..end]);
        at = end;
    }
    out
}

/// Decodes a raw stream; returns the samples of each channel.
fn decode(data: &[u8], stereo: bool) -> Result<Vec<Vec<f32>>, Error> {
    let mut decoder = Mp3Decoder::new(stereo.then_some(2), &Limits::default())?;
    let mut pcm = Pcm::new();
    let mut channels: Vec<Vec<f32>> = Vec::new();
    for frame in frames(data) {
        decoder.decode(frame, &mut pcm)?;
        channels.resize(pcm.channels(), Vec::new());
        for (c, out) in channels.iter_mut().enumerate() {
            match pcm.plane(c).unwrap() {
                Plane::Float(s) => out.extend_from_slice(s),
                Plane::Int { .. } => panic!("integer samples"),
            }
        }
    }
    Ok(channels)
}

#[test]
fn rejects_layers_one_and_two_and_junk() {
    let mut pcm = Pcm::new();
    let mut decoder = Mp3Decoder::new(None, &Limits::default()).unwrap();
    // Layer II, 44.1 kHz.
    assert!(matches!(
        decoder.decode(&[0xFF, 0xFD, 0x90, 0x00, 0, 0, 0, 0], &mut pcm),
        Err(Error::Unsupported(_))
    ));
    assert!(decoder.decode(&[0u8; 400], &mut pcm).is_err());
    assert!(decoder.decode(&[0xFF, 0xFB], &mut pcm).is_err());
}

/// The ISO/IEC 11172-4 Layer III compliance bitstreams, in the directory
/// `COREAV_MP3_VECTORS` names (the `layer3` package: `*.bit`, and
/// `compl.hex`, the reference output of `compl.bit` in 24-bit hex). Every
/// stream decodes without an error; `compl.bit` meets the full accuracy
/// criteria of 11172-4: the RMS of the difference below 2^-15 / sqrt(12),
/// and no difference above 2^-14, relative to full scale.
#[test]
#[ignore = "needs COREAV_MP3_VECTORS"]
fn compliance_bitstreams() {
    let root =
        std::path::PathBuf::from(std::env::var("COREAV_MP3_VECTORS").expect("COREAV_MP3_VECTORS"));
    let mut names: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "bit"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 11, "{names:?}");
    for path in &names {
        let data = std::fs::read(path).unwrap();
        // he_mode.bit changes between one channel and two: its frames are
        // mapped to its first frame's one.
        let channels = decode(&data, false).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        println!(
            "{}: {} channels, {} samples",
            path.display(),
            channels.len(),
            channels[0].len()
        );
    }
    let ours = decode(&std::fs::read(root.join("compl.bit")).unwrap(), false).unwrap();
    let reference: Vec<f64> = std::fs::read_to_string(root.join("compl.hex"))
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v = i32::from_str_radix(l.trim(), 16).unwrap();
            let v = if v & 0x80_0000 != 0 {
                v - 0x100_0000
            } else {
                v
            };
            f64::from(v) / f64::from(1 << 23)
        })
        .collect();
    assert_eq!(ours.len(), 1);
    // The file's last frame is cut off after 23 of its 192 bytes; the
    // reference stops before it.
    assert_eq!(ours[0].len(), reference.len() + 1152);
    let (mut sum, mut max) = (0.0f64, 0.0f64);
    for (&a, &b) in ours[0].iter().zip(&reference) {
        let d = f64::from(a) - b;
        sum += d * d;
        max = max.max(d.abs());
    }
    let rms = (sum / reference.len() as f64).sqrt();
    let rms_bar = 2f64.powi(-15) / 12f64.sqrt();
    let max_bar = 2f64.powi(-14);
    println!("compl.bit: rms {rms:.3e} (bar {rms_bar:.3e}), max {max:.3e} (bar {max_bar:.3e})");
    assert!(rms < rms_bar && max <= max_bar);
}

#[test]
#[ignore = "debugging aid: COREAV_MP3_DUMP=IN:OUT writes IN decoded as raw f32"]
fn dump() {
    let spec = std::env::var("COREAV_MP3_DUMP").unwrap();
    let (input, output) = spec.split_once(':').unwrap();
    let channels = decode(
        &std::fs::read(input).unwrap(),
        std::env::var("COREAV_MP3_STEREO").is_ok(),
    )
    .unwrap();
    let mut out = Vec::new();
    for i in 0..channels[0].len() {
        for c in &channels {
            out.extend_from_slice(&c[i].to_le_bytes());
        }
    }
    std::fs::write(output, out).unwrap();
}
