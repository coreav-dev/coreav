//! The test vectors of `tests/data/vectors.json` (`docs/design/vp9.md`,
//! milestone 1, gate G1): every frame's uncompressed header parses, and the
//! frames shown, with their sizes, are the entries of the vector's published
//! MD5 list.
//!
//! The vectors are not committed. `COREAV_VP9_VECTORS` names a directory
//! holding each vector and its `.md5` file, checked against the manifest's
//! SHA-256 values before the test runs (`.github/workflows/vp9-conformance.yml`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashSet;
use std::path::Path;

use coreav_vp9::{Decoder, Header, Limits};
use serde_json::Value;

fn manifest() -> Vec<(String, String)> {
    let text = include_str!("data/vectors.json");
    let json: Value = serde_json::from_str(text).unwrap();
    json["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["name"].as_str().unwrap().to_owned(),
                v["sha256"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn the_manifest_names_each_vector_once_in_order() {
    let names: Vec<String> = manifest().into_iter().map(|(name, _)| name).collect();
    assert_eq!(names.len(), 353);
    assert!(names.windows(2).all(|w| w[0] < w[1]));
    for name in &names {
        let profile = name.get(..5).unwrap_or_default();
        assert!(
            ["vp90-", "vp91-", "vp92-", "vp93-"].contains(&profile),
            "{name}"
        );
        assert!(name.ends_with(".webm") || name.ends_with(".ivf"), "{name}");
    }
    let hashes: HashSet<String> = manifest().into_iter().map(|(_, sha)| sha).collect();
    assert!(
        hashes
            .iter()
            .all(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
    );
}

/// The blocks of a WebM file, in storage order, read without a Matroska
/// library: the coverage of this crate's similarity check forbids path
/// dependencies on crates it does not cover.
///
/// The Segment, its Clusters and their BlockGroups are entered; every other
/// element is skipped by its size, so an unknown size (all ones) is entered
/// too. A SimpleBlock or Block is a track number, a 2-byte timestamp, a flags
/// byte and the frame; the vectors have one track and no lacing. A file whose
/// last element runs past its end ends before that element.
fn webm_packets(data: &[u8]) -> Vec<&[u8]> {
    /// An EBML variable-length integer at `at`: its bytes as stored (an
    /// element ID keeps its length marker), its value without the marker,
    /// whether every value bit is 1, and its length.
    fn vint(data: &[u8], at: usize) -> Option<(u64, u64, bool, usize)> {
        let first = *data.get(at)?;
        let len = first.leading_zeros() as usize + 1;
        if len > 8 {
            return None;
        }
        let bytes = data.get(at..at + len)?;
        let raw = bytes.iter().fold(0u64, |v, &b| v << 8 | u64::from(b));
        let value = raw & (u64::MAX >> (64 - 7 * len));
        Some((raw, value, value == (1u64 << (7 * len)) - 1, len))
    }
    /// Segment, Cluster and BlockGroup.
    const ENTERED: [u64; 3] = [0x1853_8067, 0x1F43_B675, 0xA0];
    /// SimpleBlock and Block.
    const BLOCKS: [u64; 2] = [0xA3, 0xA1];
    let mut packets = Vec::new();
    let mut at = 0;
    while let Some((id, _, _, id_len)) = vint(data, at) {
        let Some((_, size, unknown, size_len)) = vint(data, at + id_len) else {
            break;
        };
        let body = at + id_len + size_len;
        if ENTERED.contains(&id) || unknown {
            at = body;
            continue;
        }
        let Some(end) = usize::try_from(size)
            .ok()
            .and_then(|s| body.checked_add(s))
            .filter(|&e| e <= data.len())
        else {
            break;
        };
        if BLOCKS.contains(&id) {
            let (_, _, _, track_len) = vint(data, body).expect("a block's track number");
            assert_eq!(data[body + track_len + 2] & 0x06, 0, "a laced block");
            packets.push(&data[body + track_len + 3..end]);
        }
        at = end;
    }
    packets
}

/// The frames of an IVF file: a 32-byte header whose bytes 6 and 7 give its
/// length, then each frame's 4-byte size, 8-byte timestamp and bytes.
fn ivf_packets(data: &[u8]) -> Vec<&[u8]> {
    let mut at = usize::from(u16::from_le_bytes([data[6], data[7]]));
    let mut packets = Vec::new();
    while at < data.len() {
        let size = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        packets.push(&data[at + 12..at + 12 + size]);
        at += 12 + size;
    }
    packets
}

/// The size an entry of an MD5 list names (`...-352x288-0001.i420`, or
/// `d.1280x720_00001.yv12`), if it names one.
fn entry_size(entry: &str) -> Option<(u32, u32)> {
    let digits = |s: &str| -> String {
        s.chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };
    let x = entry.rfind('x')?;
    let width = digits(&entry[..x]);
    let height: String = entry[x + 1..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    Some((width.parse().ok()?, height.parse().ok()?))
}

/// The largest frames the specification allows: the vectors include frames
/// wider than [`Limits::default`] lets through.
const SPEC_LIMITS: Limits = Limits {
    max_width: 1 << 16,
    max_height: 1 << 16,
    max_pixels: 1 << 32,
    max_packet_bytes: 64 << 20,
};

/// Lists whose entries do not give the size of the frames they hash, with
/// the reason. Their frame counts are still checked.
const UNSIZED: [(&str, &str); 3] = [
    (
        "vp90-2-15-segkey_adpq.webm",
        "every entry names 1280x720, but the 150 MD5s are those FFmpeg 9.0.2's framemd5 gives for the stream's 852x480 frames",
    ),
    (
        "vp90-2-19-skip.webm",
        "its entries name frame numbers only (01.yuv, 02.yuv and so on)",
    ),
    (
        "vp90-2-19-skip-01.webm",
        "its entries name frame numbers only (01.yuv, 02.yuv and so on)",
    ),
];

/// The vector whose packets show several frames each.
const LAYERED: &str = "vp90-2-22-svc_1280x720_3.ivf";

/// What reading a vector's headers found wrong, or nothing.
///
/// A packet shows at most one frame, the last one it shows: in [`LAYERED`]
/// each packet is a superframe of three spatial layers, each with
/// `show_frame` set, and its list has one entry per packet, at the largest
/// layer's size. Every other vector shows at most one frame per packet.
fn check(dir: &Path, name: &str) -> Option<String> {
    let data = std::fs::read(dir.join(name)).unwrap();
    let list = std::fs::read_to_string(dir.join(format!("{name}.md5"))).unwrap();
    let expected: Vec<Option<(u32, u32)>> = list
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| entry_size(l.split_whitespace().nth(1).unwrap()))
        .collect();
    let packets = if data.starts_with(b"DKIF") {
        ivf_packets(&data)
    } else {
        webm_packets(&data)
    };
    let mut decoder = Decoder::new(SPEC_LIMITS);
    let mut shown = Vec::new();
    for (i, packet) in packets.iter().enumerate() {
        let headers = match decoder.read_headers(packet) {
            Ok(headers) => headers,
            Err(error) => return Some(format!("packet {i}: {error}")),
        };
        let mut in_packet = Vec::new();
        for header in headers {
            match header {
                Header::ShowExisting { width, height, .. } => in_packet.push((width, height)),
                Header::Frame(h) => {
                    if !h.trailing_bits_zero {
                        return Some(format!("packet {i}: a trailing bit is not 0"));
                    }
                    if h.show_frame {
                        in_packet.push((h.width, h.height));
                    }
                }
            }
        }
        if in_packet.len() > 1 && name != LAYERED {
            return Some(format!("packet {i} shows {} frames", in_packet.len()));
        }
        shown.extend(in_packet.last());
    }
    if shown.len() != expected.len() {
        return Some(format!(
            "{} frames shown, the list has {}",
            shown.len(),
            expected.len()
        ));
    }
    if UNSIZED
        .iter()
        .any(|(unsized_name, _)| *unsized_name == name)
    {
        return None;
    }
    let wrong = shown
        .iter()
        .zip(&expected)
        .position(|(s, e)| *e != Some(*s))?;
    Some(format!(
        "frame {wrong} is {:?}, the list says {:?}",
        shown[wrong], expected[wrong]
    ))
}

#[test]
#[ignore = "needs the test vectors in COREAV_VP9_VECTORS"]
fn every_vector_shows_the_frames_of_its_md5_list() {
    let dir = std::env::var_os("COREAV_VP9_VECTORS").expect("COREAV_VP9_VECTORS");
    let dir = Path::new(&dir);
    let vectors = manifest();
    let failures: Vec<String> = vectors
        .iter()
        .filter_map(|(name, _)| check(dir, name).map(|why| format!("{name}: {why}")))
        .collect();
    println!(
        "{} of {} vectors agree",
        vectors.len() - failures.len(),
        vectors.len()
    );
    for failure in &failures {
        println!("{failure}");
    }
    assert!(failures.is_empty(), "{} vectors disagree", failures.len());
}
