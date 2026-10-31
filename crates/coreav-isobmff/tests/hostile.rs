//! Hostile-input and pinned-limit tests.
// Test-only sources outside `#[test]` functions index their own buffers.
#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

mod common;

use common::fragments::{Fragmented, Track};
use common::*;
use coreav_isobmff::{
    Allowlist, Budget, BufferedSource, DurationSource, Error, LimitKind, Limits, MAX_READ, Movie,
    Rational, SliceSource, Source, probe,
};
use std::time::{Duration, Instant};

fn probe_bytes(data: &[u8]) -> Result<Movie, Error> {
    probe(
        SliceSource::new(data),
        &Limits::default(),
        &Allowlist::default(),
    )
}

/// Lists every packet of every track; returns the count or the first error.
fn drain(movie: &Movie, data: &[u8]) -> Result<u64, Error> {
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    let mut count = 0;
    for index in 0..movie.tracks.len() {
        for packet in movie.packets(index, SliceSource::new(data), &limits, &mut budget)? {
            packet?;
            count += 1;
        }
    }
    Ok(count)
}

#[test]
fn deep_nesting_is_rejected_quickly() {
    let levels = 10_000usize;
    let mut data = bx(b"ftyp", b"isom\0\0\x02\0");
    for i in 0..levels {
        let size = (8 * (levels - i)) as u32;
        data.extend_from_slice(&size.to_be_bytes());
        data.extend_from_slice(if i == 0 { b"moov" } else { b"trak" });
    }
    let start = Instant::now();
    let result = probe_bytes(&data);
    assert!(start.elapsed() < Duration::from_millis(100));
    assert!(
        matches!(
            result,
            Err(Error::InvalidBox { .. } | Error::LimitExceeded { .. })
        ),
        "{result:?}"
    );
}

#[test]
fn depth_limit_applies() {
    let movie_bytes = movie(&Spec::default());
    let limits = Limits {
        max_depth: 3,
        ..Limits::default()
    };
    let result = probe(
        SliceSource::new(&movie_bytes),
        &limits,
        &Allowlist::default(),
    );
    assert!(matches!(
        result,
        Err(Error::LimitExceeded {
            limit: LimitKind::Depth,
            ..
        })
    ));
}

#[test]
fn huge_sample_count_with_tiny_table_is_rejected() {
    let spec = Spec {
        stsz_count: Some(u32::MAX),
        ..Spec::default()
    };
    let data = movie(&spec);
    let start = Instant::now();
    let result = probe_bytes(&data);
    assert!(start.elapsed() < Duration::from_millis(100));
    assert!(
        matches!(result, Err(Error::InvalidBox { .. })),
        "{result:?}"
    );
}

#[test]
fn too_many_top_level_boxes() {
    let mut data = bx(b"ftyp", b"isom\0\0\x02\0");
    for _ in 0..10_001 {
        data.extend_from_slice(&bx(b"free", &[]));
    }
    assert!(matches!(
        probe_bytes(&data),
        Err(Error::LimitExceeded {
            limit: LimitKind::TopLevelBoxes,
            ..
        })
    ));
}

#[test]
fn total_box_limit() {
    let data = movie(&Spec::default());
    let limits = Limits {
        max_total_boxes: 5,
        ..Limits::default()
    };
    assert!(matches!(
        probe(SliceSource::new(&data), &limits, &Allowlist::default()),
        Err(Error::LimitExceeded {
            limit: LimitKind::TotalBoxes,
            ..
        })
    ));
}

#[test]
fn other_containers_are_not_parsed() {
    let avi = b"RIFF\x24\x00\x00\x00AVI LIST\x10\x00\x00\x00hdrlavih";
    assert_eq!(probe_bytes(avi).err(), Some(Error::NotIsobmff));
    assert_eq!(
        probe_bytes(b"\x1a\x45\xdf\xa3....").err(),
        Some(Error::NotIsobmff)
    );
    assert_eq!(probe_bytes(b"").err(), Some(Error::NotIsobmff));
}

#[test]
fn codecs_outside_the_allowlist_are_dropped_or_rejected() {
    // MagicYUV video in a QuickTime movie: the PixelSmash class.
    let spec = Spec {
        sample_entry: *b"M8RG",
        ..Spec::default()
    };
    let data = movie(&spec);
    // Default: the stream is dropped, reported, never exposed.
    let parsed = probe_bytes(&data).unwrap();
    assert!(parsed.tracks.is_empty());
    assert_eq!(parsed.dropped.len(), 1);
    let dropped = parsed.dropped[0];
    assert_eq!((dropped.stream_index, dropped.track_id), (0, 1));
    assert_eq!(dropped.fourcc.0, *b"M8RG");
    assert_eq!(dropped.reason, coreav_isobmff::DropReason::CodecNotAllowed);
    // Strict mode rejects the file.
    let strict = Allowlist::default().strict();
    match probe(SliceSource::new(&data), &Limits::default(), &strict) {
        Err(Error::CodecNotAllowed { track_id, fourcc }) => {
            assert_eq!(track_id, 1);
            assert_eq!(fourcc.0, *b"M8RG");
        }
        other => panic!("{other:?}"),
    }
    // A narrower allowlist drops H.264 too, or rejects it in strict mode.
    let h264 = movie(&Spec::default());
    let only_aac = Allowlist::new(&[coreav_isobmff::Codec::Aac]);
    let parsed = probe(SliceSource::new(&h264), &Limits::default(), &only_aac).unwrap();
    assert_eq!((parsed.tracks.len(), parsed.dropped.len()), (0, 1));
    assert!(matches!(
        probe(
            SliceSource::new(&h264),
            &Limits::default(),
            &only_aac.strict()
        ),
        Err(Error::CodecNotAllowed { .. })
    ));
}

#[test]
fn every_truncation_is_an_error_or_a_valid_result() {
    let data = movie(&Spec::default());
    for end in 0..data.len() {
        let prefix = &data[..end];
        if let Ok(movie) = probe_bytes(prefix) {
            let _ = drain(&movie, prefix);
        }
    }
}

#[test]
fn deterministic_mutations_never_panic() {
    let base = movie(&Spec::default());
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut accepted = 0;
    for _ in 0..4096 {
        let mut data = base.clone();
        for _ in 0..(1 + next() % 4) {
            let at = (next() % data.len() as u64) as usize;
            data[at] = match next() % 4 {
                0 => 0,
                1 => 0xff,
                2 => data[at].wrapping_add(1),
                _ => next() as u8,
            };
        }
        if let Ok(movie) = probe_bytes(&data) {
            accepted += 1;
            let _ = drain(&movie, &data);
        }
    }
    assert!(accepted > 0);
}

#[test]
fn inconsistent_tables_invalidate_only_the_track() {
    // stsz lists three sizes but stts and stsc describe four samples.
    let spec = Spec {
        sizes: vec![10, 20, 30],
        composition: vec![0, 0, 0],
        ..Spec::default()
    };
    let mut data = movie(&spec);
    // Patch stts sample count 3 -> 4.
    let needle = [0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 2, 0];
    let at = data
        .windows(needle.len())
        .position(|w| w == needle)
        .unwrap();
    data[at + 7] = 4;
    let movie = probe_bytes(&data).unwrap();
    assert!(!movie.tracks[0].tables_valid);
    assert_eq!(drain(&movie, &data).unwrap(), 0);
}

#[test]
fn packets_past_the_end_are_errors() {
    let data = movie(&Spec::default());
    let parsed = probe_bytes(&data).unwrap();
    assert_eq!(drain(&parsed, &data).unwrap(), 4);
    // Make the first sample size enormous: probing still succeeds (sizes are
    // checked per packet), listing fails without reading past the file.
    let mut bytes = data.clone();
    let needle = u32s(&[0, 4, 100, 50]);
    let at = bytes
        .windows(needle.len())
        .position(|w| w == needle.as_slice())
        .unwrap();
    bytes[at + 8..at + 12].copy_from_slice(&0x7fff_0000u32.to_be_bytes());
    let parsed = probe_bytes(&bytes).unwrap();
    assert!(drain(&parsed, &bytes).is_err());
}

/// The one-fragment movie with its `trun` sample count set to `count`.
fn with_trun_count(base: &[u8], count: u32) -> Vec<u8> {
    let mut data = base.to_vec();
    let at = data.windows(4).position(|w| w == b"trun").unwrap();
    // After the type: the version and flags word, then the sample count.
    data[at + 8..at + 12].copy_from_slice(&count.to_be_bytes());
    data
}

/// The same movie with the `trex` default sample size set to zero, so that
/// its samples are declared to hold no bytes at all.
fn with_zero_default_size(base: &[u8]) -> Vec<u8> {
    let mut data = base.to_vec();
    let at = data.windows(4).position(|w| w == b"trex").unwrap();
    // After the type: version and flags, track id, description index and
    // the default duration, then the default sample size.
    data[at + 20..at + 24].copy_from_slice(&0u32.to_be_bytes());
    data
}

/// The same movie with the `trex` default sample size set to zero and its
/// one `trun` rewritten to carry `sizes` per sample (flag 0x200), which the
/// zero default makes the only sizes its samples have.
fn with_per_sample_sizes(base: &[u8], sizes: &[u32]) -> Vec<u8> {
    let mut data = with_zero_default_size(base);
    let at = data.windows(4).position(|w| w == b"trun").unwrap();
    let count = u32::try_from(sizes.len()).unwrap();
    data[at + 8..at + 12].copy_from_slice(&count.to_be_bytes());
    // Flags 0x201: a data offset and a size per sample.
    data[at + 5..at + 8].copy_from_slice(&[0, 0x02, 0x01]);
    let entries: Vec<u8> = sizes.iter().flat_map(|s| s.to_be_bytes()).collect();
    let grow = u32::try_from(entries.len()).unwrap();
    let end = at + 16;
    data.splice(end..end, entries);
    // The `trun`, its `traf` and the `moof` all grow with the entries, and
    // the data offset, which is relative to the `moof`, moves with them.
    for kind in [b"trun", b"traf", b"moof"] {
        let pos = data.windows(4).position(|w| w == kind).unwrap() - 4;
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) + grow;
        data[pos..pos + 4].copy_from_slice(&size.to_be_bytes());
    }
    let at = data.windows(4).position(|w| w == b"trun").unwrap();
    let offset = u32::from_be_bytes(data[at + 12..at + 16].try_into().unwrap()) + grow;
    data[at + 12..at + 16].copy_from_slice(&offset.to_be_bytes());
    data
}

/// A `trun` cannot declare samples the file does not back. Each
/// of the three things that can back one is checked on a file of its own,
/// and the fragment is refused where it is read, as a sample table its file
/// cannot back is. FFmpeg 7.0.2 refuses each of these files when it reads
/// the header, and reads the sized one at the end, as CoreAV does.
#[test]
fn a_trun_cannot_declare_samples_the_file_does_not_back() {
    let limits = Limits::default();
    let base = fragmented_movie(None, None);
    // The file the hostile ones are made from: four samples of the `trex`
    // default size, 400 bytes, exactly what its `mdat` holds.
    let mut movie = probe_bytes(&base).unwrap();
    movie
        .scan_fragments(SliceSource::new(&base), &limits)
        .unwrap();
    assert_eq!(movie.tracks[0].fragment_samples, 4);

    // `Ok` means the samples lie outside the file, which is reported as
    // `Truncated`, exactly as listing them one by one reports it; `Err`
    // names the reason the declaration itself is invalid.
    let hostile = [
        (
            "per-sample sizes that do not fit in the trun",
            with_trun_count(&with_per_sample_sizes(&base, &[100; 4]), u32::MAX),
            Err("trun entries exceed the box"),
        ),
        (
            "2^32 - 1 samples of 100 bytes",
            with_trun_count(&base, u32::MAX),
            Ok(()),
        ),
        (
            "five samples where four fit",
            with_trun_count(&base, 5),
            Ok(()),
        ),
        (
            "2^32 - 1 samples with no size at all",
            with_trun_count(&with_zero_default_size(&base), u32::MAX),
            Err("trun samples have no size"),
        ),
        (
            "four samples with no size at all",
            with_zero_default_size(&base),
            Err("trun samples have no size"),
        ),
    ];
    for (name, data, expected) in hostile {
        assert!(data.len() < 2048, "{name}");
        // Probe never reads a fragment, so it still succeeds
        // and still knows no duration.
        let mut movie = probe_bytes(&data).unwrap();
        assert_eq!(
            movie.tracks[0].duration_source,
            DurationSource::Unknown,
            "{name}"
        );
        assert_eq!(movie.tracks[0].fragment_samples, 0, "{name}");
        let scanned = movie.scan_fragments(SliceSource::new(&data), &limits);
        match (&scanned, expected) {
            (Err(Error::Truncated { .. }), Ok(())) => {}
            (Err(Error::InvalidBox { kind, reason, .. }), Err(want))
                if kind.0 == *b"moof" && *reason == want => {}
            _ => panic!("{name}: {scanned:?}"),
        }
        assert!(drain(&movie, &data).is_err(), "{name}");
    }

    // A sample with a size of its own is backed by it, whatever the default
    // says: four of 100 bytes are what this `mdat` holds.
    let sized = with_per_sample_sizes(&base, &[100; 4]);
    let mut movie = probe_bytes(&sized).unwrap();
    movie
        .scan_fragments(SliceSource::new(&sized), &limits)
        .unwrap();
    assert_eq!(movie.tracks[0].fragment_samples, 4);
    assert_eq!(drain(&movie, &sized).unwrap(), 4);

    // The measured residue of check 3 (`fragment.rs`): a sample that writes
    // a size of zero costs the four bytes of its entry, so the box bounds
    // the run and the file is read — as a `stsz` of zero sizes is, here and
    // in FFmpeg, whose fragment reader alone refuses this one. Pinned so
    // that it stays a decision.
    for sizes in [vec![0; 4], vec![0, 0, 0, 100]] {
        let zeros = with_per_sample_sizes(&base, &sizes);
        let mut movie = probe_bytes(&zeros).unwrap();
        movie
            .scan_fragments(SliceSource::new(&zeros), &limits)
            .unwrap();
        assert_eq!(movie.tracks[0].fragment_samples, 4);
        assert_eq!(drain(&movie, &zeros).unwrap(), 4);
    }
}

/// A track whose media is in another file gets no
/// packets, and the file is still read. Its `trun` offsets are offsets into
/// that file, so this file can neither check them nor report what they
/// declare: its fragments are not read at all, while the
/// samples of the track that is here must not be lost because of the track
/// that is not.
#[test]
fn fragments_of_a_track_with_external_media_are_not_read() {
    let limits = Limits::default();
    let data = external_fragmented_movie();
    let mut movie = probe_bytes(&data).unwrap();
    assert_eq!(movie.tracks.len(), 2);
    assert_eq!(
        (movie.tracks[0].external_data, movie.tracks[1].external_data),
        (false, true)
    );
    movie
        .scan_fragments(SliceSource::new(&data), &limits)
        .unwrap();
    // The local track's samples are counted and listed; the external
    // track's are neither, and its duration stays what the headers said.
    assert_eq!(movie.tracks[0].fragment_samples, 4);
    assert_eq!(movie.tracks[0].duration_source, DurationSource::Observed);
    assert_eq!(movie.tracks[1].fragment_samples, 0);
    assert_eq!(movie.tracks[1].duration_source, DurationSource::Unknown);
    assert_eq!(drain(&movie, &data).unwrap(), 4);

    // The one bit of the file that says "my media is elsewhere" cannot buy
    // a count this file would otherwise refuse: that track's samples are
    // not counted whatever it declares.
    let mut lying = data.clone();
    let at = lying
        .windows(4)
        .rposition(|w| w == b"trun")
        .expect("the external track's trun is the last one");
    lying[at + 8..at + 12].copy_from_slice(&u32::MAX.to_be_bytes());
    let mut movie = probe_bytes(&lying).unwrap();
    movie
        .scan_fragments(SliceSource::new(&lying), &limits)
        .unwrap();
    assert_eq!(movie.tracks[1].fragment_samples, 0);
    assert_eq!(movie.tracks[1].duration, None);
    // The movie lasts what the track that is here lasts: 2,048 ticks.
    assert_eq!(
        movie.duration_seconds(),
        Rational::new(2048, u64::from(TIMESCALE))
    );
}

/// A stream the allowlist dropped is never exposed, so a fragment of it
/// this file cannot back costs that stream its timing rather than the file:
/// it cannot lend the movie a duration nothing backs, and it
/// cannot refuse a file whose kept tracks are sound.
#[test]
fn a_dropped_stream_cannot_lend_the_movie_a_duration() {
    let limits = Limits::default();
    // MagicYUV video beside H.264: dropped by the default allowlist.
    let data = two_track_fragmented_movie(b"M8RG", false);
    let honest = probe_and_scan(&data, &limits);
    assert_eq!(honest.tracks.len(), 1);
    assert_eq!(honest.dropped.len(), 1);
    assert_eq!(honest.dropped[0].fragment_duration, Some(4 * 512));
    let seconds = honest.duration_seconds();
    assert_eq!(seconds, Rational::new(2048, u64::from(TIMESCALE)));

    // The dropped stream's `trun` now declares 2^32 - 1 samples of the
    // `trex` default size, which this file cannot hold.
    let mut lying = data.clone();
    let at = lying
        .windows(4)
        .rposition(|w| w == b"trun")
        .expect("the dropped stream's trun is the last one");
    lying[at + 8..at + 12].copy_from_slice(&u32::MAX.to_be_bytes());
    let movie = probe_and_scan(&lying, &limits);
    assert_eq!(movie.dropped[0].fragment_duration, None);
    assert_eq!(movie.tracks[0].fragment_samples, 4);
    assert_eq!(movie.duration_seconds(), seconds);
    assert_eq!(drain(&movie, &lying).unwrap(), 4);
}

/// Probes `data` and reads its fragments, both of which must succeed.
fn probe_and_scan(data: &[u8], limits: &Limits) -> Movie {
    let mut movie = probe_bytes(data).unwrap();
    movie
        .scan_fragments(SliceSource::new(data), limits)
        .unwrap();
    movie
}

/// Probe cost does not grow with the number of fragments.
#[test]
fn probe_reads_do_not_grow_with_fragments() {
    let mut read = Vec::new();
    for fragments in [3, 300] {
        let mut source = Fragmented::prores(fragments);
        let movie = probe(&mut source, &Limits::default(), &Allowlist::default()).unwrap();
        assert_eq!(movie.tracks[0].duration_source, DurationSource::Unknown);
        read.push(source.read_bytes());
    }
    assert_eq!(read[0], read[1]);
    assert!(read[0] < 4096, "probe read {} bytes", read[0]);
}

#[test]
#[ignore = "lists 200,000 packets of a 2.2 GB virtual file; run with --ignored --release"]
fn twenty_thousand_fragments_stream_with_bounded_reads() {
    let fragments = 20_000;
    let mut buffered = BufferedSource::new(Fragmented::prores(fragments));
    let total = buffered.len().unwrap();
    assert!(total > 2_200_000_000);
    let limits = Limits::default();
    let mut movie = probe(&mut buffered, &limits, &Allowlist::default()).unwrap();
    movie.scan_fragments(&mut buffered, &limits).unwrap();
    assert_eq!(
        movie.tracks[0].fragment_samples,
        fragments * u64::from(Track::PRORES.samples)
    );
    let mut budget = Budget::new(&limits);
    let mut count = 0u64;
    let mut last_dts = -1;
    for packet in movie
        .packets(0, &mut buffered, &limits, &mut budget)
        .unwrap()
    {
        let packet = packet.unwrap();
        assert!(packet.dts > last_dts);
        last_dts = packet.dts;
        count += 1;
    }
    assert_eq!(count, fragments * u64::from(Track::PRORES.samples));
    assert!(buffered.largest_read() <= MAX_READ);
}

#[test]
fn small_virtual_fragmented_file() {
    let mut buffered = BufferedSource::new(Fragmented::prores(3));
    let limits = Limits::default();
    // Probe does not read fragments; the duration is unknown.
    let mut movie = probe(&mut buffered, &limits, &Allowlist::default()).unwrap();
    let track = &movie.tracks[0];
    assert_eq!(track.fragment_samples, 0);
    assert_eq!(track.duration, None);
    assert_eq!(track.duration_source, DurationSource::Unknown);
    assert_eq!(movie.duration_seconds(), None);
    // The on-demand scan gives the observed span, and is repeatable.
    for _ in 0..2 {
        movie.scan_fragments(&mut buffered, &limits).unwrap();
        let track = &movie.tracks[0];
        assert_eq!(track.fragment_samples, 30);
        assert_eq!(track.sample_duration, 30 * 512);
        assert_eq!(track.duration, Some(Rational::from_int(30 * 512)));
        assert_eq!(track.duration_source, DurationSource::Observed);
    }
    let mut budget = Budget::new(&limits);
    let packets: Vec<_> = movie
        .packets(0, &mut buffered, &limits, &mut budget)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(packets.len(), 30);
    assert_eq!(packets[10].dts, 5120);
    assert!(
        packets
            .iter()
            .all(|p| p.size == Track::PRORES.size && p.keyframe)
    );
    // The fragment limit applies where fragments are read: scan and packets.
    let tight = Limits {
        max_fragments: 2,
        ..Limits::default()
    };
    let mut movie = probe(&mut buffered, &tight, &Allowlist::default()).unwrap();
    assert!(matches!(
        movie.scan_fragments(&mut buffered, &tight),
        Err(Error::LimitExceeded {
            limit: LimitKind::Fragments,
            ..
        })
    ));
    let mut budget = Budget::new(&tight);
    let listed: Result<Vec<_>, _> = movie
        .packets(0, &mut buffered, &tight, &mut budget)
        .unwrap()
        .collect();
    assert!(matches!(
        listed,
        Err(Error::LimitExceeded {
            limit: LimitKind::Fragments,
            ..
        })
    ));
}

#[test]
fn repeated_malformed_configuration_is_one_warning() {
    // 50,000 empty `hvcC` children in one allowed `avc1` entry (400 KB, inside
    // the 1 MiB box limit): each is malformed, but the warning is kept once.
    let children: Vec<u8> = (0..50_000).flat_map(|_| bx(b"hvcC", &[])).collect();
    let spec = Spec::default();
    let mut data = movie(&spec);
    let entry = visual_entry(b"avc1", &bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]));
    let mut big = visual_entry(
        b"avc1",
        &[bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]), children].concat(),
    );
    // Splice the large entry in place of the small one and fix every
    // enclosing box size (stsd, stbl, minf, mdia, trak, moov).
    let at = data
        .windows(entry.len())
        .position(|w| w == entry.as_slice())
        .unwrap();
    let grow = (big.len() - entry.len()) as u32;
    data.splice(at..at + entry.len(), big.drain(..));
    for kind in [b"stsd", b"stbl", b"minf", b"mdia", b"trak", b"moov"] {
        let pos = data.windows(4).position(|w| w == kind).unwrap() - 4;
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) + grow;
        data[pos..pos + 4].copy_from_slice(&size.to_be_bytes());
    }
    let movie = probe_bytes(&data).unwrap();
    let warnings = &movie.tracks[0].warnings;
    let malformed = warnings
        .iter()
        .filter(|w| matches!(w, coreav_isobmff::Warning::MalformedCodecConfig(_)))
        .count();
    assert_eq!(malformed, 1, "{warnings:?}");
}
