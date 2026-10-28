//! QuickTime compressed audio whose sample tables count single audio
//! samples (`stsz` of 1, a version 1 sound description giving the samples
//! and bytes of one compressed frame): the bytes are the frames from each
//! chunk's offset, so a last chunk whose frames fit the file iterates,
//! although the tables' one byte a sample would run past its end, and one
//! whose frames do not fit stops as truncated.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use common::*;
use coreav_isobmff::{Allowlist, Budget, Error, Limits, SliceSource, probe};

const PER_PACKET: u32 = 2048;
const FRAME_BYTES: u32 = 370;

/// A version 1 sound description of `QDM2`: 2048 samples in 370 bytes.
fn qdm2_entry() -> Vec<u8> {
    let mut body = vec![0u8; 6];
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&1u16.to_be_bytes()); // version 1
    body.extend_from_slice(&[0u8; 6]);
    body.extend_from_slice(&2u16.to_be_bytes());
    body.extend_from_slice(&16u16.to_be_bytes());
    body.extend_from_slice(&(-2i16).to_be_bytes());
    body.extend_from_slice(&[0u8; 2]);
    body.extend_from_slice(&(44_100u32 << 16).to_be_bytes());
    body.extend_from_slice(&u32s(&[PER_PACKET, FRAME_BYTES / 2, FRAME_BYTES, 2]));
    bx(b"QDM2", &body)
}

/// A movie whose one audio track has a chunk of `samples` audio samples
/// for each entry of `chunks`, the movie box first and the frames of the
/// chunks back to back in `mdat` at the end, short of `cut` bytes.
fn movie(chunks: &[u32], cut: usize) -> Vec<u8> {
    let frames: Vec<u32> = chunks.iter().map(|n| n.div_ceil(PER_PACKET)).collect();
    let samples: u32 = chunks.iter().sum();
    let ftyp = bx(b"ftyp", b"qt  \0\0\x02\0qt  ");
    let build = |first: u32| {
        let mut offsets = Vec::new();
        let mut at = first;
        for f in &frames {
            offsets.push(at);
            at += f * FRAME_BYTES;
        }
        let mut mvhd = u32s(&[0, 0, 44_100, samples]);
        mvhd.extend_from_slice(&[0u8; 80]);
        let mut tkhd = u32s(&[0, 0, 1, 0, samples]);
        tkhd.extend_from_slice(&[0u8; 52]);
        let mut mdhd = u32s(&[0, 0, 44_100, samples]);
        mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
        let mut hdlr = u32s(&[0]);
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0u8; 13]);
        let stsc: Vec<u32> = chunks
            .iter()
            .enumerate()
            .flat_map(|(i, n)| [i as u32 + 1, *n, 1])
            .collect();
        let stbl = bx(
            b"stbl",
            &cat(&[
                full(b"stsd", 0, 0, &cat(&[u32s(&[1]), qdm2_entry()])),
                full(b"stts", 0, 0, &u32s(&[1, samples, 1])),
                full(
                    b"stsc",
                    0,
                    0,
                    &cat(&[u32s(&[chunks.len() as u32]), u32s(&stsc)]),
                ),
                full(b"stsz", 0, 0, &u32s(&[1, samples])),
                full(
                    b"stco",
                    0,
                    0,
                    &cat(&[u32s(&[offsets.len() as u32]), u32s(&offsets)]),
                ),
            ]),
        );
        let dref = full(b"dref", 0, 0, &cat(&[u32s(&[1]), full(b"url ", 0, 1, &[])]));
        let minf = bx(
            b"minf",
            &cat(&[full(b"smhd", 0, 0, &[0u8; 4]), bx(b"dinf", &dref), stbl]),
        );
        let mdia = bx(
            b"mdia",
            &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
        );
        bx(
            b"moov",
            &cat(&[
                full(b"mvhd", 0, 0, &mvhd),
                bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia])),
            ]),
        )
    };
    // The offsets have a fixed width, so the movie box's length does not
    // depend on them.
    let first = (ftyp.len() + build(0).len() + 8) as u32;
    let total = (frames.iter().sum::<u32>() * FRAME_BYTES) as usize - cut;
    cat(&[ftyp, build(first), bx(b"mdat", &vec![0xAB; total])])
}

/// Samples listed one by one, by runs, by batches of runs and by visits,
/// and from the middle: each the samples and the error that ended it.
fn listings(data: &[u8]) -> [(u64, Option<Error>); 5] {
    let limits = Limits::default();
    let movie = probe(SliceSource::new(data), &limits, &Allowlist::listing()).unwrap();
    assert_eq!(
        movie.tracks[0].description.sound_packet,
        Some((PER_PACKET, FRAME_BYTES))
    );
    let mut budget = Budget::new(&limits);
    let mut packets = movie
        .packets(0, SliceSource::new(data), &limits, &mut budget)
        .unwrap();
    let mut one = (0, None);
    for packet in &mut packets {
        match packet {
            Ok(_) => one.0 += 1,
            Err(error) => one.1 = Some(error),
        }
    }
    let mut budget = Budget::new(&limits);
    let mut packets = movie
        .packets(0, SliceSource::new(data), &limits, &mut budget)
        .unwrap();
    let mut runs = (0, None);
    while let Some(run) = packets.next_run(1000) {
        match run {
            Ok(run) => runs.0 += u64::from(run.count),
            Err(error) => runs.1 = Some(error),
        }
    }
    let mut budget = Budget::new(&limits);
    let mut packets = movie
        .packets(0, SliceSource::new(data), &limits, &mut budget)
        .unwrap();
    let mut batches = (0, None);
    let mut out = Vec::new();
    loop {
        out.clear();
        match packets.next_runs(1000, &mut out, 4) {
            Ok(0) => break,
            Ok(_) => batches.0 += out.iter().map(|r| u64::from(r.count)).sum::<u64>(),
            Err(error) => {
                batches.1 = Some(error);
                break;
            }
        }
    }
    let mut budget = Budget::new(&limits);
    let mut packets = movie
        .packets(0, SliceSource::new(data), &limits, &mut budget)
        .unwrap();
    let mut visits = (0, None);
    match packets.for_each_sample(|_| Ok::<(), Error>(())) {
        Ok(n) => visits.0 = n,
        Err(error) => visits.1 = Some(error),
    }
    let mut budget = Budget::new(&limits);
    let from = movie
        .packets_from(0, 5000, SliceSource::new(data), &limits, &mut budget)
        .unwrap();
    let mut middle = (0, None);
    for packet in from {
        match packet {
            Ok(_) => middle.0 += 1,
            Err(error) => middle.1 = Some(error),
        }
    }
    [one, runs, batches, visits, middle]
}

#[test]
fn frames_inside_the_file_iterate() {
    // 10,000 samples a chunk: 5 frames, 1,850 bytes, where the tables'
    // one byte a sample ends 8,150 bytes past the file.
    let data = movie(&[10_000, 10_000], 0);
    let listed = listings(&data);
    for (i, (count, error)) in listed.iter().enumerate() {
        assert_eq!(*error, None, "listing {i}");
        assert_eq!(*count, if i == 4 { 15_000 } else { 20_000 }, "listing {i}");
    }
}

#[test]
fn frames_past_the_end_stop_as_truncated() {
    let data = movie(&[10_000, 10_000], 1);
    for (i, (_, error)) in listings(&data).iter().enumerate() {
        if i == 4 {
            // Seeking stops before the samples it would list.
            continue;
        }
        assert!(
            matches!(error, Some(Error::Truncated { .. })),
            "listing {i}: {error:?}"
        );
    }
}
