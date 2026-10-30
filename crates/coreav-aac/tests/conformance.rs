//! The conformance bar of `docs/design/audio-decode.md` ("AAC-LC bars",
//! 1): every AAC LC stream in MP4 of the ISO/IEC 14496-26 conformance
//! package (`al00` to `al19`, both frame lengths, every rate) decodes
//! within full accuracy of its reference waveform, every channel; every
//! HE-AAC stream of the package (`al_sbr_*`, `al960_sbr_*`) is refused by
//! name; nothing panics.
//!
//! Ignored by default; `.github/workflows/audio-conformance.yml` runs it
//! with `COREAV_AAC_VECTORS` naming a directory with `compressedMp4/` and
//! `referencesWav/` (the latter from the package's second DVD).

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use coreav_aac::{Config, Decoder, Error, Limits};
use coreav_isobmff::{self as mp4, Source};
use std::path::Path;

/// Full accuracy (the MP3 bar's): RMS of the difference at most 2^-15 /
/// sqrt(12), no difference above 2^-14, of full scale.
const MAX_RMS: f64 = 1.0 / 32768.0 / 3.464_101_615_137_754_6;
const MAX_ABS: f64 = 1.0 / 16384.0;
/// The references need not start at the decoder's first sample.
const MAX_SHIFT: i64 = 4096;
/// Noise blocks: per channel, their level over the whole stream within
/// 0.1 dB of the reference's.
const MAX_NOISE_DB: f64 = 0.1;

/// The `DecoderSpecificInfo` of an `esds` box payload (ISO/IEC 14496-1
/// 7.2.6.5 and 7.2.6.6): version and flags, then descriptors tagged 3
/// (`ES_Descriptor`), 4 and 5.
fn decoder_specific_info(esds: &[u8]) -> Option<Vec<u8>> {
    let mut at = 4;
    let descriptor = |at: &mut usize| -> Option<(u8, usize)> {
        let tag = *esds.get(*at)?;
        *at += 1;
        let mut len = 0usize;
        for _ in 0..4 {
            let b = *esds.get(*at)?;
            *at += 1;
            len = (len << 7) | usize::from(b & 0x7f);
            if b & 0x80 == 0 {
                break;
            }
        }
        Some((tag, len))
    };
    let (tag, _) = descriptor(&mut at)?;
    if tag != 3 {
        return None;
    }
    let flags = *esds.get(at + 2)?;
    at += 3;
    if flags & 0x80 != 0 {
        at += 2;
    }
    if flags & 0x40 != 0 {
        at += 1 + usize::from(*esds.get(at)?);
    }
    if flags & 0x20 != 0 {
        at += 2;
    }
    let (tag, _) = descriptor(&mut at)?;
    if tag != 4 {
        return None;
    }
    at += 13;
    let (tag, len) = descriptor(&mut at)?;
    if tag != 5 {
        return None;
    }
    esds.get(at..at + len).map(<[u8]>::to_vec)
}

enum Outcome {
    Decoded {
        channels: Vec<Vec<f32>>,
        damaged: usize,
        first_error: Option<Error>,
        noisy: Vec<bool>,
        /// Frames with intensity bands under an `ms_mask_present` of 2.
        all_ms: Vec<bool>,
        frame: usize,
    },
    Refused(Error),
}

fn decode(path: &Path) -> Outcome {
    let source = mp4::FileSource::open(path).expect("open");
    let limits = mp4::Limits::default();
    let movie =
        mp4::probe(source, &limits, &mp4::Allowlist::new(&[mp4::Codec::Aac])).expect("probe");
    let track = movie
        .tracks
        .iter()
        .find(|t| t.kind == mp4::TrackKind::Audio)
        .expect("an audio track");
    let esds = track.description.codec_config.as_ref().expect("esds");
    let asc = decoder_specific_info(&esds.data).expect("DecoderSpecificInfo");
    let config = match Config::parse(&asc, &Limits { max_channels: 128 }) {
        Ok(config) => config,
        Err(e) => return Outcome::Refused(e),
    };
    let mut decoder = Decoder::new(&config);
    let frame = config.frame_length();
    let mut channels = vec![Vec::new(); config.channels()];
    let mut out = vec![0f32; frame * config.channels()];
    let mut damaged = 0;
    let mut first_error = None;
    let mut noisy = Vec::new();
    let mut all_ms = Vec::new();
    let mut budget = mp4::Budget::new(&limits);
    let source = mp4::FileSource::open(path).expect("open");
    let mut packets = movie
        .packets(track.index, source, &limits, &mut budget)
        .expect("packets");
    let mut data = Vec::new();
    while let Some(packet) = packets.next() {
        let packet = packet.expect("packet");
        data.resize(packet.size as usize, 0);
        packets
            .source_mut()
            .read_exact_at(packet.offset, &mut data)
            .expect("read");
        match decoder.decode(&data, &mut out) {
            Ok(decoded) => {
                noisy.push(decoded.noise_bands > 0);
                all_ms.push(decoded.intensity_all_ms_bands > 0);
            }
            Err(Error::Unsupported(what)) => return Outcome::Refused(Error::Unsupported(what)),
            Err(e) => {
                damaged += 1;
                first_error.get_or_insert(e);
                noisy.push(false);
                all_ms.push(false);
                out.iter_mut().for_each(|s| *s = 0.0);
            }
        }
        for (c, samples) in channels.iter_mut().enumerate() {
            samples.extend_from_slice(&out[c * frame..(c + 1) * frame]);
        }
    }
    Outcome::Decoded {
        channels,
        damaged,
        first_error,
        noisy,
        all_ms,
        frame,
    }
}

/// The channels of a PCM WAV file, as f32 at full scale ±1.0.
fn read_wav(path: &Path) -> Vec<Vec<f32>> {
    let data = std::fs::read(path).expect("read wav");
    assert_eq!(&data[0..4], b"RIFF");
    let mut at = 12;
    let mut format = None;
    while at + 8 <= data.len() {
        let id = &data[at..at + 4];
        let len = u32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &data[at + 8..(at + 8 + len).min(data.len())];
        if id == b"fmt " {
            let channels = u16::from_le_bytes([body[2], body[3]]) as usize;
            let bits = u16::from_le_bytes([body[14], body[15]]) as usize;
            format = Some((channels, bits));
        } else if id == b"data" {
            let (channels, bits) = format.expect("fmt before data");
            let bytes = bits / 8;
            let mut out = vec![Vec::new(); channels];
            for (i, sample) in body.chunks_exact(bytes).enumerate() {
                let value = match bytes {
                    2 => i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0,
                    3 => {
                        (i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8) as f32
                            / 8_388_608.0
                    }
                    4 => i32::from_le_bytes(sample.try_into().unwrap()) as f32 / 2_147_483_648.0,
                    _ => panic!("{bits}-bit WAV"),
                };
                out[i % channels].push(value);
            }
            return out;
        }
        at += 8 + len + (len & 1);
    }
    panic!("no data chunk in {}", path.display());
}

/// A comparison of a decoded channel with its reference.
struct Comparison {
    rms: f64,
    max: f64,
    /// |10 log10(energy ours / energy reference)| over every noise block.
    noise_db: f64,
    /// Median over noise blocks of |20 log10(RMS ours / RMS reference)|,
    /// reported.
    noise_block_db: f64,
    noise_blocks: usize,
}

/// Compares `ours` with `reference` shifted by `shift` (reference\[i +
/// shift\] against ours\[i\]) over their overlap: samples outside noise
/// blocks by difference, ours clipped to the reference's range when
/// `clip` (integer PCM references); noise blocks (`noisy`\[k\] makes
/// blocks k and k + 1 random) by level, over the whole stream (gating) and
/// per block (reported).
fn compare(
    ours: &[f32],
    reference: &[f32],
    shift: i64,
    noisy: &[bool],
    frame: usize,
    clip: bool,
) -> Comparison {
    let mut sum = 0f64;
    let mut max = 0f64;
    let mut n = 0usize;
    let mut levels = Vec::new();
    let (mut noise_ours, mut noise_ref) = (0f64, 0f64);
    for (k, block) in ours.chunks(frame).enumerate() {
        let noise = noisy.get(k).copied().unwrap_or(false)
            || (k > 0 && noisy.get(k - 1).copied().unwrap_or(false));
        let (mut e_ours, mut e_ref, mut m) = (0f64, 0f64, 0usize);
        for (j, &v) in block.iter().enumerate() {
            let i = (k * frame + j) as i64 + shift;
            if i < 0 || i as usize >= reference.len() {
                continue;
            }
            let r = f64::from(reference[i as usize]);
            let v = if clip {
                v.clamp(-1.0, 1.0 - 1.0 / 8_388_608.0)
            } else {
                v
            };
            let v = f64::from(v);
            if noise {
                e_ours += v * v;
                e_ref += r * r;
                m += 1;
            } else {
                let d = v - r;
                sum += d * d;
                max = max.max(d.abs());
                n += 1;
            }
        }
        noise_ours += e_ours;
        noise_ref += e_ref;
        if noise && m == frame && e_ours > 0.0 && e_ref > 0.0 {
            levels.push((10.0 * (e_ours / e_ref).log10()).abs());
        }
    }
    levels.sort_by(f64::total_cmp);
    Comparison {
        rms: (sum / n.max(1) as f64).sqrt(),
        max,
        noise_db: if noise_ours > 0.0 && noise_ref > 0.0 {
            (10.0 * (noise_ours / noise_ref).log10()).abs()
        } else if noise_ours == noise_ref {
            0.0
        } else {
            f64::INFINITY
        },
        noise_block_db: levels.get(levels.len() / 2).copied().unwrap_or(0.0),
        noise_blocks: levels.len(),
    }
}

/// The shift within ±MAX_SHIFT with the smallest RMS difference between
/// `ours` and `reference` over 8,192 samples where the reference is
/// loudest; (shift, RMS).
fn best_shift(ours: &[f32], reference: &[f32]) -> (i64, f64) {
    const WINDOW: usize = 8192;
    let block_energy = |start: usize| -> f64 {
        reference
            .iter()
            .skip(start)
            .take(WINDOW)
            .map(|v| f64::from(*v).powi(2))
            .sum()
    };
    let start = (0..reference.len().saturating_sub(WINDOW))
        .step_by(WINDOW)
        .max_by(|&a, &b| block_energy(a).total_cmp(&block_energy(b)))
        .unwrap_or(0);
    let mut best = (0, f64::MAX);
    for shift in -MAX_SHIFT..=MAX_SHIFT {
        let mut sum = 0f64;
        let mut n = 0;
        for (j, &r) in reference.iter().enumerate().skip(start).take(WINDOW) {
            let i = j as i64 - shift;
            if i < 0 || i as usize >= ours.len() {
                continue;
            }
            let d = f64::from(ours[i as usize]) - f64::from(r);
            sum += d * d;
            n += 1;
        }
        if n > WINDOW / 2 && (sum / n as f64).sqrt() < best.1 {
            best = (shift, (sum / n as f64).sqrt());
        }
    }
    best
}

/// For streams that are mostly noise: the shift, in steps of 64 samples,
/// that best matches the log-energy envelopes (64-sample hops, 256-sample
/// windows); noise bands keep their energy, not their samples.
fn envelope_shift(ours: &[f32], reference: &[f32]) -> (i64, f64) {
    let envelope = |x: &[f32]| -> Vec<f64> {
        (0..x.len().saturating_sub(256) / 64)
            .map(|t| {
                (x[t * 64..t * 64 + 256]
                    .iter()
                    .map(|v| f64::from(*v).powi(2))
                    .sum::<f64>()
                    + 1e-12)
                    .ln()
            })
            .collect()
    };
    let (a, b) = (envelope(ours), envelope(reference));
    let mut best = (0, f64::MAX);
    for s in -(MAX_SHIFT / 64)..=(MAX_SHIFT / 64) {
        let (mut sum, mut n) = (0f64, 0usize);
        for (t, &e) in a.iter().enumerate() {
            let u = t as i64 + s;
            if u >= 0 && (u as usize) < b.len() {
                sum += (e - b[u as usize]).powi(2);
                n += 1;
            }
        }
        if n > a.len() / 2 && sum / (n as f64) < best.1 {
            best = (s * 64, sum / n as f64);
        }
    }
    best
}

/// Bar 1's comparison of decoded channels with the reference channels of
/// a conformance stream: the shift, then (worst RMS, worst largest
/// difference, worst noise level, most noise blocks, worst per-block
/// noise level) over the reference channels, each matched to the decoded
/// channel with the smallest RMS difference.
fn against_reference(
    channels: &[Vec<f32>],
    reference: &[Vec<f32>],
    noisy: &[bool],
    frame: usize,
) -> (i64, (f64, f64, f64, usize, f64)) {
    let loudest = (0..reference.len())
        .max_by(|&a, &b| {
            let e = |c: usize| {
                reference[c]
                    .iter()
                    .take(1 << 16)
                    .map(|v| f64::from(*v).powi(2))
                    .sum::<f64>()
            };
            e(a).total_cmp(&e(b))
        })
        .unwrap_or(0);
    // Channels are matched to reference channels by the smallest RMS
    // difference at the stream's shift (the references' order is the
    // package's, not WAV's).
    let mostly_noise = noisy.iter().filter(|&&n| n).count() * 2 > noisy.len();
    let shift = channels
        .iter()
        .map(|c| {
            if mostly_noise {
                envelope_shift(c, &reference[loudest])
            } else {
                best_shift(c, &reference[loudest])
            }
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(0, |(s, _)| s);
    let mut worst = (0f64, 0f64, 0f64, 0usize, 0f64);
    let mut used = vec![false; channels.len()];
    for want in reference {
        let mut best: Option<(Comparison, usize)> = None;
        for (c, got) in channels.iter().enumerate() {
            if used[c] {
                continue;
            }
            let cmp = compare(got, want, shift, noisy, frame, true);
            if best.as_ref().is_none_or(|b| cmp.rms < b.0.rms) {
                best = Some((cmp, c));
            }
        }
        let Some((cmp, c)) = best else { break };
        used[c] = true;
        worst = (
            worst.0.max(cmp.rms),
            worst.1.max(cmp.max),
            worst.2.max(cmp.noise_db),
            worst.3.max(cmp.noise_blocks),
            worst.4.max(cmp.noise_block_db),
        );
    }
    (shift, worst)
}

/// The reference channels of stream `name`: one file, or one per element
/// (`_f00`, `_s00`, `_b00`, `_l00`, ...), in name order.
fn references(dir: &Path, name: &str) -> Vec<Vec<f32>> {
    let single = dir.join(format!("{name}.wav"));
    if single.exists() {
        return read_wav(&single);
    }
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("references")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| {
            f.strip_prefix(&format!("{name}_"))
                .and_then(|rest| rest.strip_suffix(".wav"))
                .is_some_and(|rest| {
                    rest.len() == 3 && rest[1..].chars().all(|c| c.is_ascii_digit())
                })
        })
        .collect();
    files.sort();
    files.iter().flat_map(|f| read_wav(&dir.join(f))).collect()
}

#[test]
#[ignore = "needs the ISO/IEC 14496-26 conformance package (COREAV_AAC_VECTORS)"]
fn conformance_streams() {
    let root = std::env::var("COREAV_AAC_VECTORS").expect("COREAV_AAC_VECTORS");
    let root = Path::new(&root);
    let mut names: Vec<String> = std::fs::read_dir(root.join("compressedMp4"))
        .expect("compressedMp4")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.starts_with("al") && f.ends_with(".mp4"))
        .collect();
    names.sort();
    let (mut counted, mut failed, mut refused_he) = (0, Vec::new(), 0);
    for file in &names {
        let name = file.trim_end_matches(".mp4");
        let he = name.starts_with("al_sbr") || name.starts_with("al960_sbr");
        let outcome = decode(&root.join("compressedMp4").join(file));
        if he {
            match outcome {
                Outcome::Refused(Error::Unsupported(what)) if what.contains("HE-AAC") => {
                    refused_he += 1
                }
                Outcome::Refused(e) => {
                    failed.push(format!("{name}: refused as {e}, not as HE-AAC"))
                }
                Outcome::Decoded { .. } => {
                    failed.push(format!("{name}: HE-AAC decoded, not refused"))
                }
            }
            continue;
        }
        let Outcome::Decoded {
            channels,
            damaged,
            first_error,
            noisy,
            frame,
            ..
        } = outcome
        else {
            if let Outcome::Refused(e) = outcome {
                failed.push(format!("{name}: refused: {e}"));
            }
            continue;
        };
        counted += 1;
        let reference = references(&root.join("referencesWav"), name);
        if reference.len() != channels.len() {
            failed.push(format!(
                "{name}: {} channels, reference {}",
                channels.len(),
                reference.len()
            ));
            continue;
        }
        let (shift, worst) = against_reference(&channels, &reference, &noisy, frame);
        let pass =
            worst.0 <= MAX_RMS && worst.1 <= MAX_ABS && worst.2 <= MAX_NOISE_DB && damaged == 0;
        println!(
            "{name}: {} ch, shift {}, rms {:.3e}, max {:.3e}, noise {:.4} dB over {} blocks (per block {:.2} dB), damaged {damaged} {first_error:?}: {}",
            channels.len(),
            shift,
            worst.0,
            worst.1,
            worst.2,
            worst.3,
            worst.4,
            if pass { "ok" } else { "FAIL" }
        );
        if !pass {
            failed.push(format!(
                "{name}: rms {:.3e}, max {:.3e}, damaged {damaged}",
                worst.0, worst.1
            ));
        }
    }
    println!(
        "{counted} LC streams counted, {refused_he} HE-AAC streams refused, {} failures",
        failed.len()
    );
    for f in &failed {
        println!("  {f}");
    }
    assert!(failed.is_empty());
}

/// Bar 2 of "AAC-LC bars": the decoder on every packet of each MP4 input
/// against `ffmpeg -ignore_editlist 1 -i IN -map 0:a:0 -f f32le -`, sample 0
/// to sample 0. `COREAV_AAC_AGREEMENT` names a file of lines `INPUT<TAB>
/// REFERENCE` (the reference's interleaved f32le samples);
/// `COREAV_AAC_AGREEMENT_OUT` the file this writes one JSON object to per
/// input. The bars are judged by `scripts/aac_agreement.py`.
#[test]
#[ignore = "needs inputs and their ffmpeg decodes (COREAV_AAC_AGREEMENT)"]
fn ffmpeg_agreement() {
    let list = std::env::var("COREAV_AAC_AGREEMENT").expect("COREAV_AAC_AGREEMENT");
    let out = std::env::var("COREAV_AAC_AGREEMENT_OUT").expect("COREAV_AAC_AGREEMENT_OUT");
    let mut lines = String::new();
    for line in std::fs::read_to_string(&list).expect("list").lines() {
        let (input, reference) = line.split_once('\t').expect("INPUT<TAB>REFERENCE");
        let name = json_string(input);
        let pce = {
            let source = mp4::FileSource::open(Path::new(input)).expect("open");
            let limits = mp4::Limits::default();
            let movie = mp4::probe(source, &limits, &mp4::Allowlist::new(&[mp4::Codec::Aac]))
                .expect("probe");
            let track = movie
                .tracks
                .iter()
                .find(|t| t.kind == mp4::TrackKind::Audio)
                .expect("an audio track");
            let esds = track.description.codec_config.as_ref().expect("esds");
            decoder_specific_info(&esds.data)
                .and_then(|asc| Config::parse(&asc, &Limits { max_channels: 128 }).ok())
                .is_some_and(|c| c.channel_configuration() == 0)
        };
        let (channels, damaged, noisy, all_ms, frame) = match decode(Path::new(input)) {
            Outcome::Decoded {
                channels,
                damaged,
                noisy,
                all_ms,
                frame,
                ..
            } => (channels, damaged, noisy, all_ms, frame),
            Outcome::Refused(e) => {
                let reason = json_string(&e.to_string());
                lines += &format!(
                    "{{\"input\": {name}, \"status\": \"refused\", \"reason\": {reason}}}\n"
                );
                continue;
            }
        };
        let bytes = std::fs::read(reference).expect("reference");
        let count = channels.len();
        let interleaved: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let theirs: Vec<Vec<f32>> = (0..count)
            .map(|c| interleaved.iter().skip(c).step_by(count).copied().collect())
            .collect();
        let level_only: Vec<bool> = noisy.iter().zip(&all_ms).map(|(n, a)| n | a).collect();
        let mut used = vec![false; count];
        let mut matched = Vec::new();
        let mut results = Vec::new();
        for (c, ours) in channels.iter().enumerate() {
            let candidates: Vec<usize> = if pce {
                (0..count).filter(|&t| !used[t]).collect()
            } else {
                vec![c]
            };
            let Some((cmp, t)) = candidates
                .into_iter()
                .map(|t| (compare(ours, &theirs[t], 0, &level_only, frame, false), t))
                .min_by(|a, b| a.0.rms.total_cmp(&b.0.rms))
            else {
                break;
            };
            used[t] = true;
            matched.push(t.to_string());
            results.push(format!(
                "{{\"rms\": {:e}, \"max\": {:e}, \"noise_db\": {}, \"noise_blocks\": {}, \"noise_block_db\": {:.4}}}",
                cmp.rms,
                cmp.max,
                if cmp.noise_db.is_finite() {
                    format!("{:.4}", cmp.noise_db)
                } else {
                    "null".to_owned()
                },
                cmp.noise_blocks,
                cmp.noise_block_db
            ));
        }
        // A conformance stream: both decoders against its reference, as
        // bar 1 measures CoreAV.
        let references_dir = Path::new(input)
            .parent()
            .and_then(Path::parent)
            .map(|root| root.join("referencesWav"))
            .filter(|_| input.contains("/compressedMp4/"));
        let against = references_dir.map_or_else(String::new, |dir| {
            let stem = Path::new(input)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let reference = references(&dir, &stem);
            let (_, ours) = against_reference(&channels, &reference, &noisy, frame);
            let (_, ffmpeg) = against_reference(&theirs, &reference, &noisy, frame);
            format!(
                ", \"reference\": {{\"coreav\": [{:e}, {:e}], \"ffmpeg\": [{:e}, {:e}]}}",
                ours.0, ours.1, ffmpeg.0, ffmpeg.1
            )
        });
        lines += &format!(
            "{{\"input\": {name}, \"status\": \"decoded\", \"channels\": {count}, \"frame\": {frame}, \"packets\": {}, \"length\": {}, \"reference_length\": {}, \"reference_values\": {}, \"damaged\": {damaged}, \"noise_frames\": {}, \"intensity_all_ms_frames\": {}, \"pce\": {pce}, \"matched\": [{}], \"per_channel\": [{}]{against}}}\n",
            noisy.len(),
            channels.first().map_or(0, Vec::len),
            theirs.first().map_or(0, Vec::len),
            interleaved.len(),
            noisy.iter().filter(|&&n| n).count(),
            all_ms.iter().filter(|&&a| a).count(),
            matched.join(", "),
            results.join(", ")
        );
    }
    std::fs::write(out, lines).expect("write results");
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            c if (c as u32) < 0x20 => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out + "\""
}
