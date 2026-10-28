//! QuickTime compressed movie boxes (`cmov`, docs/design/compressed-movie.md):
//! a movie read through its compressed box equals the movie it compresses,
//! and every limit and malformed part fails cleanly.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use common::*;
use coreav_isobmff::{Allowlist, Budget, Error, LimitKind, Limits, SliceSource, probe};

/// `data` with its movie box (last in the file) replaced by a compressed
/// movie box of `algorithm` holding `inner` (the original box by default),
/// compressed with zlib, and `declared` (its length by default).
fn compress(
    data: &[u8],
    algorithm: &[u8; 4],
    inner: Option<Vec<u8>>,
    declared: Option<u32>,
) -> Vec<u8> {
    compress_at(data, algorithm, inner, declared, 6)
}

/// [`compress`] at zlib `level` (0 stores the data).
fn compress_at(
    data: &[u8],
    algorithm: &[u8; 4],
    inner: Option<Vec<u8>>,
    declared: Option<u32>,
    level: u8,
) -> Vec<u8> {
    let at = data.windows(4).rposition(|w| w == b"moov").unwrap() - 4;
    let moov = inner.unwrap_or_else(|| data[at..].to_vec());
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&moov, level);
    let mut cmvd = declared.unwrap_or(moov.len() as u32).to_be_bytes().to_vec();
    cmvd.extend_from_slice(&packed);
    let cmov = bx(b"cmov", &cat(&[bx(b"dcom", algorithm), bx(b"cmvd", &cmvd)]));
    cat(&[data[..at].to_vec(), bx(b"moov", &cmov)])
}

fn read(data: &[u8], limits: &Limits) -> Result<coreav_isobmff::Movie, Error> {
    probe(SliceSource::new(data), limits, &Allowlist::default())
}

#[test]
fn a_compressed_movie_reads_as_the_movie_it_holds() {
    let plain = movie(&Spec::default());
    let packed = compress(&plain, b"zlib", None, None);
    let limits = Limits::default();
    let expected = read(&plain, &limits).unwrap();
    let movie = read(&packed, &limits).unwrap();
    assert!(movie.compressed && !expected.compressed);
    assert_eq!(movie.timescale, expected.timescale);
    assert_eq!(movie.mvhd_duration, expected.mvhd_duration);
    assert_eq!(movie.tracks.len(), 1);
    let (track, want) = (&movie.tracks[0], &expected.tracks[0]);
    assert_eq!(track.description.codec, want.description.codec);
    assert_eq!(track.sample_count, want.sample_count);
    assert_eq!(track.edit_list, want.edit_list);
    // The samples are where the file has them: the same packets.
    let packets = |m: &coreav_isobmff::Movie, data: &[u8]| {
        let mut budget = Budget::new(&limits);
        m.packets(0, SliceSource::new(data), &limits, &mut budget)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(packets(&movie, &packed), packets(&expected, &plain));
}

#[test]
fn limits_bound_the_inflated_movie() {
    let plain = movie(&Spec::default());
    let packed = compress(&plain, b"zlib", None, None);
    let small = Limits {
        max_movie_bytes: 64,
        ..Limits::default()
    };
    assert!(matches!(
        read(&packed, &small),
        Err(Error::LimitExceeded {
            limit: LimitKind::MovieBytes,
            ..
        })
    ));
    let tight = Limits {
        max_movie_ratio: 1,
        ..Limits::default()
    };
    assert!(matches!(
        read(&packed, &tight),
        Err(Error::LimitExceeded {
            limit: LimitKind::MovieRatio,
            ..
        })
    ));
    // A movie box padded with zeros compresses beyond the default ratio.
    let at = plain.windows(4).rposition(|w| w == b"moov").unwrap() - 4;
    let mut padded = plain[at..].to_vec();
    padded.extend(bx(b"free", &vec![0u8; 1 << 20]));
    let size = padded.len() as u32;
    padded[..4].copy_from_slice(&size.to_be_bytes());
    let bomb = compress(&plain, b"zlib", Some(padded), None);
    assert!(matches!(
        read(&bomb, &Limits::default()),
        Err(Error::LimitExceeded {
            limit: LimitKind::MovieRatio,
            ..
        })
    ));
}

#[test]
fn malformed_compressed_movies_are_rejected() {
    let plain = movie(&Spec::default());
    let limits = Limits::default();
    let at = plain.windows(4).rposition(|w| w == b"moov").unwrap() - 4;
    let size = (plain.len() - at) as u32;
    // A declared size one byte off either way.
    for declared in [size - 1, size + 1] {
        let packed = compress(&plain, b"zlib", None, Some(declared));
        assert!(matches!(
            read(&packed, &limits),
            Err(Error::InvalidBox { .. })
        ));
    }
    // Another algorithm.
    let packed = compress(&plain, b"adec", None, None);
    assert_eq!(
        read(&packed, &limits).unwrap_err(),
        Error::Unsupported {
            reason: "compressed movie algorithm other than zlib"
        }
    );
    // Data that is not zlib, or does not inflate to one movie box.
    let mut garbage = compress(&plain, b"zlib", None, None);
    let end = garbage.len();
    garbage[end - 20..end - 10].fill(0x5A);
    assert!(read(&garbage, &limits).is_err());
    let not_moov = compress(&plain, b"zlib", Some(bx(b"free", &[0u8; 64])), None);
    assert!(matches!(
        read(&not_moov, &limits),
        Err(Error::InvalidBox { .. })
    ));
    // A compressed movie inside a compressed movie.
    let inner = compress(&plain, b"zlib", None, None);
    let nested = compress(&plain, b"zlib", Some(inner[at..].to_vec()), None);
    assert!(matches!(
        read(&nested, &limits),
        Err(Error::InvalidBox { .. })
    ));
    // No dcom, or no cmvd.
    let base = plain[..at].to_vec();
    for cmov in [
        bx(b"cmov", &bx(b"cmvd", &[0, 0, 0, 0])),
        bx(b"cmov", &bx(b"dcom", b"zlib")),
    ] {
        let file = cat(&[base.clone(), bx(b"moov", &cmov)]);
        assert!(matches!(
            read(&file, &limits),
            Err(Error::InvalidBox { .. })
        ));
    }
}

#[test]
fn every_prefix_and_byte_of_a_compressed_movie_is_safe() {
    let packed = compress(&movie(&Spec::default()), b"zlib", None, None);
    let limits = Limits::default();
    for end in 0..packed.len() {
        let _ = read(&packed[..end], &limits);
    }
    for i in 0..packed.len() {
        let mut changed = packed.clone();
        changed[i] ^= 0xFF;
        let _ = read(&changed, &limits);
    }
}

/// The plain movie box of `data` (last in the file) with `extra` appended.
fn moov_with(data: &[u8], extra: &[u8]) -> Vec<u8> {
    let at = data.windows(4).rposition(|w| w == b"moov").unwrap() - 4;
    let mut moov = data[at..].to_vec();
    moov.extend_from_slice(extra);
    let size = moov.len() as u32;
    moov[..4].copy_from_slice(&size.to_be_bytes());
    moov
}

#[test]
fn data_read_in_chunks_and_errors_at_the_compressed_box() {
    let plain = movie(&Spec::default());
    let limits = Limits::default();
    // Stored data of 200 KB of varied bytes spans several read chunks.
    let noise: Vec<u8> = (0..200_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    let big = moov_with(&plain, &bx(b"free", &noise));
    let stored = compress_at(&plain, b"zlib", Some(big), None, 0);
    assert!(read(&stored, &limits).unwrap().compressed);
    // Compressed data longer than any zlib stream of its size.
    let at = plain.windows(4).rposition(|w| w == b"moov").unwrap() - 4;
    let mut long = compress(&plain, b"zlib", None, None);
    long.extend(vec![0u8; 4096]);
    let cmvd_at = long.windows(4).position(|w| w == b"cmvd").unwrap() - 4;
    for (offset, extra) in [(at, 4096u32), (at + 8, 4096), (cmvd_at, 4096)] {
        let size = u32::from_be_bytes(long[offset..offset + 4].try_into().unwrap()) + extra;
        long[offset..offset + 4].copy_from_slice(&size.to_be_bytes());
    }
    assert!(
        matches!(read(&long, &limits), Err(Error::InvalidBox { offset, .. }) if offset == cmvd_at as u64)
    );
    // An error in the inflated box reports the cmvd box: a box past its
    // movie box's end.
    let broken = moov_with(&plain, &[0, 0, 0x10, 0, b'f', b'r', b'e', b'e']);
    let packed = compress(&plain, b"zlib", Some(broken), None);
    let cmvd_at = packed.windows(4).position(|w| w == b"cmvd").unwrap() as u64 - 4;
    let error = read(&packed, &limits).unwrap_err();
    assert!(
        matches!(error, Error::Truncated { offset } | Error::InvalidBox { offset, .. } if offset == cmvd_at),
        "{error:?}"
    );
}

#[test]
fn counters_carry_into_the_inflated_box() {
    let plain = movie(&Spec::default());
    let packed = compress(&plain, b"zlib", None, None);
    // The fewest boxes each needs: the compressed movie needs the plain
    // one's and its outer moov, cmov, dcom and cmvd, so the inflated walk
    // carries the file walk's count on.
    let fits = |data: &[u8], n: u32| {
        read(
            data,
            &Limits {
                max_total_boxes: n,
                ..Limits::default()
            },
        )
        .is_ok()
    };
    let least = |data: &[u8]| (1..200).find(|n| fits(data, *n)).unwrap();
    assert_eq!(least(&packed), least(&plain) + 4);
}

#[test]
fn structure_rules_of_the_movie_box() {
    let plain = movie(&Spec::default());
    let limits = Limits::default();
    let at = plain.windows(4).rposition(|w| w == b"moov").unwrap() - 4;
    let packed = compress(&plain, b"zlib", None, None);
    let cmov = &packed[at + 8..];
    // A movie header beside a compressed movie: read as before, cmov
    // ignored.
    let with_header = moov_with(&plain, cmov);
    let file = cat(&[plain[..at].to_vec(), with_header]);
    let movie = read(&file, &limits).unwrap();
    assert!(!movie.compressed);
    assert_eq!(movie.tracks.len(), 1);
    // Tracks beside a compressed movie, without a movie header.
    let moov = &plain[at + 8..];
    let mvhd_len = u32::from_be_bytes(moov[..4].try_into().unwrap()) as usize;
    let tracks = bx(b"moov", &cat(&[moov[mvhd_len..].to_vec(), cmov.to_vec()]));
    let file = cat(&[plain[..at].to_vec(), tracks]);
    assert!(matches!(
        read(&file, &limits),
        Err(Error::InvalidBox {
            reason: "compressed movie beside tracks",
            ..
        })
    ));
}

#[test]
fn gapless_metadata_is_read_from_the_inflated_box() {
    let plain = movie(&Spec::default());
    let full = |body: &[u8]| [&[0u8; 4][..], body].concat();
    let value = b" 00000000 00000840 000003DC 00000000004D8BE4 00000000 00000000";
    let item = bx(
        b"----",
        &cat(&[
            bx(b"mean", &full(b"com.apple.iTunes")),
            bx(b"name", &full(b"iTunSMPB")),
            bx(b"data", &[&[0, 0, 0, 1, 0, 0, 0, 0][..], value].concat()),
        ]),
    );
    let udta = bx(
        b"udta",
        &bx(
            b"meta",
            &cat(&[vec![0; 4], bx(b"hdlr", &[0; 25]), bx(b"ilst", &item)]),
        ),
    );
    let with_tags = moov_with(&plain, &udta);
    let at = plain.windows(4).rposition(|w| w == b"moov").unwrap() - 4;
    let tagged = cat(&[plain[..at].to_vec(), with_tags.clone()]);
    let packed = compress(&plain, b"zlib", Some(with_tags), None);
    let limits = Limits::default();
    let want = read(&tagged, &limits)
        .unwrap()
        .itunes_gapless(SliceSource::new(&tagged))
        .unwrap();
    assert!(want.is_some());
    let movie = read(&packed, &limits).unwrap();
    // Read during probe: an empty source is never touched.
    assert_eq!(movie.itunes_gapless(SliceSource::new(&[])).unwrap(), want);
}
