//! The coded data's view of random access (docs/design/random-access.md)
//! through a probed movie: `Movie::access_format` and
//! `Packets::frame_access` on H.264 samples built from the specification.

mod common;

use common::*;
use coreav_isobmff::access::{AccessFormat, FrameAccess};
use coreav_isobmff::{Allowlist, Budget, Limits, SliceSource, probe};

/// A length-prefixed NAL unit (4-byte lengths).
fn nal(bytes: &[u8]) -> Vec<u8> {
    let mut out = (bytes.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(bytes);
    out
}

/// A coded slice header: `nal_unit_type` `kind`, `first_mb_in_slice` 0 and
/// `slice_type` 0 (P) or 2 (I), each one Exp-Golomb code, padded.
fn slice(kind: u8, slice_type: u8) -> Vec<u8> {
    let bits: u8 = match slice_type {
        0 => 0b1100_0000,
        _ => 0b1011_0000,
    };
    vec![0x60 | kind, bits, 0, 0]
}

/// `parts` padded with a filler NAL unit (type 12) to `size` bytes.
fn sample(parts: &[Vec<u8>], size: usize) -> Vec<u8> {
    let mut out: Vec<u8> = parts.iter().flat_map(|p| nal(p)).collect();
    let filler = size - out.len() - 4;
    out.extend(nal(&[vec![0x0C], vec![0xFF; filler - 1]].concat()));
    out
}

#[test]
fn frames_of_a_movie_read_through_its_listing() {
    let spec = Spec::default();
    let mut data = movie(&spec);
    // The samples follow `ftyp` and the `mdat` header.
    let samples = [
        sample(&[slice(5, 2)], 100),
        sample(&[slice(1, 0)], 50),
        sample(&[slice(1, 2), slice(1, 2)], 60),
        sample(&[vec![0x06, 6, 1, 0b0010_0101, 0x80], slice(1, 0)], 70),
    ];
    let mut at = 28;
    for bytes in &samples {
        data[at..at + bytes.len()].copy_from_slice(bytes);
        at += bytes.len();
    }
    let limits = Limits::default();
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    let format = movie.access_format(0).unwrap();
    assert_eq!(format, AccessFormat::H264 { length_size: 4 });
    // The record holds no sequence parameter set.
    assert_eq!(movie.max_num_ref_frames(0), None);
    let mut budget = Budget::new(&limits);
    let mut packets = movie
        .packets(0, SliceSource::new(&data), &limits, &mut budget)
        .unwrap();
    let mut seen = Vec::new();
    while let Some(packet) = packets.next() {
        let packet = packet.unwrap();
        seen.push((
            packet.keyframe,
            packets.frame_access(format, &packet).unwrap(),
        ));
    }
    let frame = |idr, intra, recovery_frames| {
        Some(FrameAccess {
            idr,
            intra,
            recovery_frames,
            recovery_exact: recovery_frames.is_some(),
            // Every slice here has nal_ref_idc 3.
            reference: true,
        })
    };
    // The sync sample flag stays the table's: only the first sample.
    assert_eq!(
        seen,
        vec![
            (true, frame(true, true, None)),
            (false, frame(false, false, None)),
            (false, frame(false, true, None)),
            (false, frame(false, false, Some(3))),
        ]
    );
    // Not an H.264 track, or not a track: no format.
    assert_eq!(movie.access_format(1), None);
    let other = movie_with_entry(b"hvc1");
    let movie = probe(SliceSource::new(&other), &limits, &Allowlist::default()).unwrap();
    assert_eq!(movie.access_format(0), None);
}

fn movie_with_entry(kind: &[u8; 4]) -> Vec<u8> {
    movie(&Spec {
        sample_entry: *kind,
        ..Spec::default()
    })
}
