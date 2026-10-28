//! Bar C1 of `docs/design/h264.md`: every in-scope JVT conformance
//! bitstream decodes bit-exactly.
//!
//! The suite is not vendored. Point the test at a directory that holds the
//! streams as their zips extract them
//! (`<dir>/<file>.264` and `.yuv`, or one directory per zip,
//! `<dir>/<stream>/<file>.264`):
//!
//! ```text
//! COREAV_H264_CONFORMANCE=/path/to/suite cargo test -p coreav-h264 \
//!     --release --test conformance -- --ignored --nocapture
//! ```
//!
//! `COREAV_H264_ONLY=name,name` runs only those streams,
//! `COREAV_H264_KERNELS=scalar` forces the reference kernels, and
//! `COREAV_H264_THREADS=1,2,8` decodes every stream once with each of
//! those thread counts (bar G1 of step 7; one thread where it is unset).

// Test code: the lints that keep production code panic-free are relaxed
// here, as clippy.toml does for unit tests.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use coreav_h264::{BitReader, Decoder, Limits, Mode, Picture, annex_b, rbsp};
use coreav_kernels_h264::{Auto, Kernels};

/// One stream of the manifest.
struct Stream {
    name: String,
    bitstream: String,
    yuv: String,
}

/// The in-scope streams of `tests/data/conformance-streams.json`.
fn manifest() -> Vec<Stream> {
    let text = include_str!("data/conformance-streams.json");
    let value: serde_json::Value = serde_json::from_str(text).expect("the manifest parses");
    let streams = value["streams"].as_array().expect("streams").clone();
    streams
        .iter()
        .filter(|s| s["in_scope"].as_bool().unwrap_or(false))
        .map(|s| Stream {
            name: s["name"].as_str().unwrap_or_default().to_owned(),
            bitstream: s["bitstream"].as_str().unwrap_or_default().to_owned(),
            yuv: s["yuv"].as_str().unwrap_or_default().to_owned(),
        })
        .collect()
}

/// The reference decoded file, read one picture at a time.
struct Reference {
    file: BufReader<File>,
    buffer: Vec<u8>,
    frames: usize,
}

impl Reference {
    fn open(path: &Path) -> Reference {
        let file = File::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
        Reference {
            file: BufReader::with_capacity(1 << 20, file),
            buffer: Vec::new(),
            frames: 0,
        }
    }

    /// Compares one decoded picture with the next one of the reference
    /// file, returning what differs.
    fn check(&mut self, picture: &Picture) -> Result<(), String> {
        let (cb, cr) = picture.chroma();
        let size = picture.width() * picture.height() + 2 * cb.width() * cb.height();
        self.buffer.resize(size, 0);
        let read = read_exact_or_less(&mut self.file, &mut self.buffer);
        if read < size {
            return Err(format!(
                "frame {}: the reference file holds {read} of {size} bytes",
                self.frames
            ));
        }
        let mut at = 0usize;
        for (plane, rows) in [
            picture.luma().rows().collect::<Vec<_>>(),
            cb.rows().collect(),
            cr.rows().collect(),
        ]
        .into_iter()
        .enumerate()
        {
            for (y, row) in rows.iter().enumerate() {
                let want = &self.buffer[at..at + row.len()];
                if want != *row {
                    let x = row
                        .iter()
                        .zip(want.iter())
                        .position(|(a, b)| a != b)
                        .unwrap_or(0);
                    return Err(format!(
                        "frame {}: plane {plane} row {y} sample {x}: {} is not the reference's {}",
                        self.frames, row[x], want[x]
                    ));
                }
                at += row.len();
            }
        }
        self.frames += 1;
        Ok(())
    }

    /// Whether the reference file ends where the decoder stopped.
    fn at_end(&mut self) -> bool {
        let mut byte = [0u8; 1];
        read_exact_or_less(&mut self.file, &mut byte) == 0
    }
}

fn read_exact_or_less(file: &mut BufReader<File>, out: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < out.len() {
        match file.read(&mut out[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) => panic!("read: {e}"),
        }
    }
    filled
}

/// Decodes one stream and compares every picture with the reference file.
fn decode_stream<K: Kernels>(
    kernels: K,
    threads: usize,
    bitstream: &Path,
    reference: &Path,
) -> Result<usize, String> {
    let mut data = Vec::new();
    File::open(bitstream)
        .map_err(|e| format!("open {}: {e}", bitstream.display()))?
        .read_to_end(&mut data)
        .map_err(|e| format!("read {}: {e}", bitstream.display()))?;
    let mut decoder = Decoder::with_kernels(kernels, Limits::default()).with_threads(threads);
    let mut reference = Reference::open(reference);
    for nal in annex_b(&data) {
        decoder
            .decode_nal(nal)
            .map_err(|e| format!("frame {}: {e}", reference.frames))?;
        for picture in decoder.take_pictures() {
            reference.check(&picture)?;
        }
    }
    decoder
        .flush()
        .map_err(|e| format!("frame {}: {e}", reference.frames))?;
    for picture in decoder.take_pictures() {
        reference.check(&picture)?;
    }
    if !reference.at_end() {
        return Err(format!(
            "the decoder made {} frames and the reference file holds more",
            reference.frames
        ));
    }
    Ok(reference.frames)
}

#[test]
#[ignore = "needs the JVT conformance suite: set COREAV_H264_CONFORMANCE"]
fn every_in_scope_stream_is_bit_exact() {
    let Ok(dir) = std::env::var("COREAV_H264_CONFORMANCE") else {
        panic!("set COREAV_H264_CONFORMANCE to the directory holding the suite");
    };
    let dir = PathBuf::from(dir);
    let only: Vec<String> = std::env::var("COREAV_H264_ONLY")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let scalar = std::env::var("COREAV_H264_KERNELS").as_deref() == Ok("scalar");
    let kernels = if scalar {
        Auto::scalar()
    } else {
        Auto::detect()
    };
    let thread_counts: Vec<usize> = std::env::var("COREAV_H264_THREADS")
        .unwrap_or_else(|_| "1".to_owned())
        .split(',')
        .map(|n| {
            n.trim()
                .parse()
                .expect("COREAV_H264_THREADS: thread counts")
        })
        .collect();

    let streams = manifest();
    let mut failures = Vec::new();
    let mut passed = 0;
    let mut frames = 0;
    for stream in &streams {
        if !only.is_empty() && !only.contains(&stream.name) {
            continue;
        }
        // The manifest's paths are relative to the root of the stream's
        // zip; a suite may be extracted flat, or with one directory per
        // zip.
        let (bitstream, reference) = {
            let flat = (dir.join(&stream.bitstream), dir.join(&stream.yuv));
            if flat.0.exists() {
                flat
            } else {
                let per_zip = dir.join(&stream.name);
                (per_zip.join(&stream.bitstream), per_zip.join(&stream.yuv))
            }
        };
        if !bitstream.exists() || !reference.exists() {
            failures.push(format!("{}: missing from {}", stream.name, dir.display()));
            continue;
        }
        for &threads in &thread_counts {
            match decode_stream(kernels, threads, &bitstream, &reference) {
                Ok(n) => {
                    passed += 1;
                    frames += n;
                    println!("pass {} ({n} frames, {threads} threads)", stream.name);
                }
                Err(e) => {
                    println!("FAIL {} ({threads} threads): {e}", stream.name);
                    failures.push(format!("{} ({threads} threads): {e}", stream.name));
                }
            }
        }
    }
    println!(
        "{passed} of {} stream decodes bit-exact, {frames} frames, kernels: {}, threads: {:?}",
        passed + failures.len(),
        if scalar { "scalar" } else { "detected" },
        thread_counts
    );
    assert!(
        failures.is_empty(),
        "{} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A hash of every sample of a picture, so that a whole stream's pictures
/// can be compared without holding them.
fn digest(picture: &Picture) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let (cb, cr) = picture.chroma();
    for rows in [picture.luma().rows(), cb.rows(), cr.rows()] {
        for row in rows {
            for &sample in row {
                hash ^= u64::from(sample);
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
        }
        hash ^= 0xff;
    }
    hash
}

/// Every picture of `bitstream` in output order: the access unit it came
/// from (the index of the NAL unit that carried its first slice, through
/// [`Decoder::set_mark`]) and a digest of its samples.
fn digests(bitstream: &Path, mode: Mode) -> Result<Vec<(u64, u64)>, String> {
    let data = read(bitstream)?;
    let mut decoder = Decoder::new(Limits::default()).with_mode(mode);
    let mut out = Vec::new();
    for (index, nal) in annex_b(&data).enumerate() {
        decoder.set_mark(index as u64);
        decoder.decode_nal(nal).map_err(|e| e.to_string())?;
        out.extend(
            decoder
                .take_pictures()
                .iter()
                .map(|p| (p.mark(), digest(p))),
        );
    }
    decoder.flush().map_err(|e| e.to_string())?;
    out.extend(
        decoder
            .take_pictures()
            .iter()
            .map(|p| (p.mark(), digest(p))),
    );
    Ok(out)
}

fn read(bitstream: &Path) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    File::open(bitstream)
        .map_err(|e| format!("open {}: {e}", bitstream.display()))?
        .read_to_end(&mut data)
        .map_err(|e| format!("read {}: {e}", bitstream.display()))?;
    Ok(data)
}

/// The access units of `data` whose picture needs no other picture: every
/// slice of it is intra. The value is the index of the NAL unit that
/// carries the first slice, which is the mark [`digests`] attaches to the
/// picture decoded from it.
///
/// This reads the stream on its own, without the decoder, so that bar
/// C5's count comes from somewhere else than the thing it checks. A
/// picture starts at a slice whose `first_mb_in_slice` is 0 (the streams
/// in scope have no arbitrary slice order) and ends at the next one.
///
/// Two assumptions, both of which make this count too high rather than
/// too low, so a decoder that drops a keyframe can never pass: a
/// redundant coded picture (clause 7.4.1.2.5), which the decoder drops
/// and which this cannot see without parsing the whole slice header,
/// counts as a picture of its own, and so do the slices of a picture
/// whose first slice is missing.
fn keyframe_access_units(data: &[u8]) -> Vec<u64> {
    let mut out = Vec::new();
    let mut payload = Vec::new();
    // The picture being walked: where it started, and whether every slice
    // of it so far is intra.
    let mut current: Option<(u64, bool)> = None;
    for (index, nal) in annex_b(data).enumerate() {
        let Some(header) = nal.first() else { continue };
        let kind = header & 0x1f;
        if kind != 1 && kind != 5 {
            continue;
        }
        payload.clear();
        rbsp(&nal[1..], &mut payload);
        let mut r = BitReader::new(&payload);
        let (Ok(first_mb), Ok(slice_type)) = (r.ue(), r.ue()) else {
            continue;
        };
        // I and SI, with and without the "every slice of the picture"
        // offset of 5 (Table 7-6).
        let intra = matches!(slice_type % 5, 2 | 4);
        match current {
            Some((start, all_intra)) if first_mb != 0 => {
                current = Some((start, all_intra && intra));
            }
            _ => {
                if let Some((start, true)) = current {
                    out.push(start);
                }
                current = Some((index as u64, intra));
            }
        }
    }
    if let Some((start, true)) = current {
        out.push(start);
    }
    out
}

#[test]
#[ignore = "needs the JVT conformance suite: set COREAV_H264_CONFORMANCE"]
fn the_keyframe_mode_returns_what_full_decoding_returns() {
    let Ok(dir) = std::env::var("COREAV_H264_CONFORMANCE") else {
        panic!("set COREAV_H264_CONFORMANCE to the directory holding the suite");
    };
    let dir = PathBuf::from(dir);
    let only: Vec<String> = std::env::var("COREAV_H264_ONLY")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let mut failures = Vec::new();
    let mut checked = 0;
    let mut keyframes = 0;
    for stream in &manifest() {
        if !only.is_empty() && !only.contains(&stream.name) {
            continue;
        }
        let flat = dir.join(&stream.bitstream);
        let bitstream = if flat.exists() {
            flat
        } else {
            dir.join(&stream.name).join(&stream.bitstream)
        };
        if !bitstream.exists() {
            continue;
        }
        let (Ok(every), Ok(keys)) = (
            digests(&bitstream, Mode::EveryPicture),
            digests(&bitstream, Mode::KeyframesOnly),
        ) else {
            failures.push(format!("{}: did not decode", stream.name));
            continue;
        };
        // Bar C5, both ways. Each picture is tied to its access unit by
        // the mark the decoder carries through the reordering, so the two
        // runs are compared picture by picture rather than by position.
        let Ok(data) = read(&bitstream) else {
            failures.push(format!("{}: did not read", stream.name));
            continue;
        };
        let mut problems = Vec::new();
        // Nothing the mode returns is invented, and every picture it
        // returns is the picture full decoding returns for that access
        // unit, sample for sample.
        for (mark, key) in &keys {
            match every.iter().find(|(m, _)| m == mark) {
                Some((_, full)) if full == key => {}
                Some(_) => problems.push(format!(
                    "the picture of access unit {mark} differs from the one full decoding returns"
                )),
                None => problems.push(format!(
                    "access unit {mark} gives a picture full decoding does not return"
                )),
            }
        }
        // And none is missing or extra: the access units whose picture
        // needs no other picture, read from the stream itself, are
        // exactly the ones the mode returned, in that order.
        let wanted = keyframe_access_units(&data);
        let returned: Vec<u64> = keys.iter().map(|(m, _)| *m).collect();
        if returned != wanted {
            problems.push(format!(
                "the mode returned the access units {returned:?}, and the stream's \
                 pictures that need no other picture are {wanted:?}"
            ));
        }
        if !problems.is_empty() {
            failures.push(format!("{}: {}", stream.name, problems.join("; ")));
            continue;
        }
        checked += 1;
        keyframes += keys.len();
        println!(
            "pass {} ({} of {} pictures need no other picture)",
            stream.name,
            keys.len(),
            every.len()
        );
    }
    println!("{checked} streams, {keyframes} keyframes");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
