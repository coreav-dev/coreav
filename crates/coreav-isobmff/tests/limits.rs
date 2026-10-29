//! Each field of `Limits`, and the caps of `RangeReader`: the default value,
//! the largest input the limit lets through and the smallest one it refuses
//! with the error that names it.
// Test-only builders outside `#[test]` functions index their own buffers.
#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

mod common;

use common::fragments::{Fragmented, Track};
use common::{Spec, bx, cat, full, movie, u32s};
use coreav_isobmff::{
    Allowlist, Budget, Error, LimitKind, Limits, Movie, RangePolicy, RangeReader, SliceSource,
    Source, TrackDefaults, probe, subsegment_samples,
};

fn refused(limit: LimitKind) -> impl Fn(&Error) -> bool {
    move |error| matches!(error, Error::LimitExceeded { limit: kind, .. } if *kind == limit)
}

fn read(data: &[u8], limits: &Limits) -> Result<Movie, Error> {
    probe(SliceSource::new(data), limits, &Allowlist::default())
}

fn pins_boundary(data: &[u8], kind: LimitKind, limit: u64, with: impl Fn(&mut Limits, u64)) {
    let at = |value| {
        let mut limits = Limits::default();
        with(&mut limits, value);
        read(data, &limits)
    };
    assert!(at(limit).is_ok(), "{:?}", at(limit).err());
    let error = at(limit - 1).err();
    assert!(error.as_ref().is_some_and(refused(kind)), "{error:?}");
}

fn ftyp(padding: usize) -> Vec<u8> {
    bx(
        b"ftyp",
        &[b"isom\0\0\x02\0".as_slice(), &vec![b'x'; padding]].concat(),
    )
}

fn movie_box(children: &[Vec<u8>]) -> Vec<u8> {
    let mut header = u32s(&[0, 0, 1000, 0]);
    header.extend_from_slice(&[0u8; 80]);
    let mut payload = full(b"mvhd", 0, 0, &header);
    payload.extend(children.concat());
    bx(b"moov", &payload)
}

fn free_boxes(count: usize) -> Vec<u8> {
    (0..count).flat_map(|_| bx(b"free", &[])).collect()
}

fn movie_of_tracks(tracks: u32) -> Vec<u8> {
    let data = movie(&Spec::default());
    let moov = data.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let trak = data.windows(4).position(|w| w == b"trak").unwrap() - 4;
    let trak_len = u32::from_be_bytes(data[trak..trak + 4].try_into().unwrap()) as usize;
    let one = &data[trak..trak + trak_len];
    let mut moov_payload = data[moov + 8..trak].to_vec();
    for id in 1..=tracks {
        let mut copy = one.to_vec();
        let tkhd = copy.windows(4).position(|w| w == b"tkhd").unwrap();
        copy[tkhd + 16..tkhd + 20].copy_from_slice(&id.to_be_bytes());
        moov_payload.extend_from_slice(&copy);
    }
    cat(&[data[..moov].to_vec(), bx(b"moov", &moov_payload)])
}

fn list_all(data: &[u8], limits: &Limits) -> Result<u64, Error> {
    let movie = read(data, limits)?;
    let mut budget = Budget::new(limits);
    let mut count = 0;
    for index in 0..movie.tracks.len() {
        for packet in movie.packets(index, SliceSource::new(data), limits, &mut budget)? {
            packet?;
            count += 1;
        }
    }
    Ok(count)
}

fn list_fragments(fragments: u64, limits: &Limits) -> Result<u64, Error> {
    let mut source = Fragmented::prores(fragments);
    let mut movie = probe(&mut source, limits, &Allowlist::default())?;
    movie.scan_fragments(&mut source, limits)?;
    let mut budget = Budget::new(limits);
    let mut count = 0;
    for packet in movie.packets(0, &mut source, limits, &mut budget)? {
        packet?;
        count += 1;
    }
    Ok(count)
}

#[test]
fn defaults_are_the_documented_values() {
    assert_eq!(
        Limits::default(),
        Limits {
            max_top_level_boxes: 10_000,
            max_depth: 32,
            max_total_boxes: 1_000_000,
            max_box_bytes: 1 << 20,
            max_retained_bytes: 16 << 20,
            max_table_bytes: 256 << 20,
            max_packet_bytes: 128 << 20,
            max_sample_work: 1 << 26,
            max_fragments: 1_000_000,
            max_tracks: 1_024,
            max_movie_bytes: 16 << 20,
            max_movie_ratio: 256,
        }
    );
}

#[test]
fn top_level_boxes_up_to_the_limit_are_read() {
    let free = 7;
    let data = cat(&[ftyp(0), movie_box(&[]), free_boxes(free)]);
    pins_boundary(&data, LimitKind::TopLevelBoxes, 2 + free as u64, |l, v| {
        l.max_top_level_boxes = v as u32;
    });
}

#[test]
fn nesting_up_to_the_depth_limit_is_read() {
    let mehd = full(b"mehd", 0, 0, &u32s(&[1000]));
    let data = cat(&[ftyp(0), movie_box(&[bx(b"mvex", &mehd)])]);
    pins_boundary(&data, LimitKind::Depth, 2, |l, v| {
        l.max_depth = v as u32;
    });
}

#[test]
fn boxes_up_to_the_total_limit_are_read() {
    let free = 9;
    let data = cat(&[ftyp(0), movie_box(&[]), free_boxes(free)]);
    pins_boundary(&data, LimitKind::TotalBoxes, 3 + free as u64, |l, v| {
        l.max_total_boxes = v as u32;
    });
}

#[test]
fn a_box_payload_up_to_the_box_limit_is_read() {
    let padding = 300;
    let data = cat(&[ftyp(padding), movie_box(&[])]);
    pins_boundary(&data, LimitKind::BoxBytes, 8 + padding as u64, |l, v| {
        l.max_box_bytes = v;
    });
}

#[test]
fn retained_bytes_up_to_the_limit_are_read() {
    let references = 5u16;
    let mut sidx = u32s(&[1, 1000, 0, 0]);
    sidx.extend_from_slice(&[0, 0]);
    sidx.extend_from_slice(&references.to_be_bytes());
    for _ in 0..references {
        sidx.extend_from_slice(&u32s(&[0, 0, 0]));
    }
    let data = cat(&[ftyp(0), movie_box(&[]), full(b"sidx", 0, 0, &sidx)]);
    let retained_per_reference = 24;
    pins_boundary(
        &data,
        LimitKind::RetainedBytes,
        u64::from(references) * retained_per_reference,
        |l, v| l.max_retained_bytes = v,
    );
}

#[test]
fn table_bytes_up_to_the_limit_are_read_across_all_tracks() {
    let samples = Spec::default().sizes.len() as u64;
    let per_track = 8 + 8 * samples + 12 + 4 * samples + 4 + 4;
    let data = movie_of_tracks(3);
    assert_eq!(read(&data, &Limits::default()).unwrap().tracks.len(), 3);
    pins_boundary(&data, LimitKind::TableBytes, 3 * per_track, |l, v| {
        l.max_table_bytes = v;
    });
}

#[test]
fn packets_up_to_the_packet_limit_are_listed_from_the_sample_tables() {
    let spec = Spec::default();
    let data = movie(&spec);
    let largest = u64::from(*spec.sizes.iter().max().unwrap());
    let at = |max_packet_bytes| {
        list_all(
            &data,
            &Limits {
                max_packet_bytes,
                ..Limits::default()
            },
        )
    };
    assert_eq!(at(largest).unwrap(), spec.sizes.len() as u64);
    let error = at(largest - 1).err();
    assert!(
        error.as_ref().is_some_and(refused(LimitKind::PacketBytes)),
        "{error:?}"
    );
}

#[test]
fn packets_up_to_the_packet_limit_are_listed_from_fragments() {
    let size = u64::from(Track::PRORES.size);
    let at = |max_packet_bytes| {
        list_fragments(
            2,
            &Limits {
                max_packet_bytes,
                ..Limits::default()
            },
        )
    };
    assert_eq!(at(size).unwrap(), 2 * u64::from(Track::PRORES.samples));
    let error = at(size - 1).err();
    assert!(
        error.as_ref().is_some_and(refused(LimitKind::PacketBytes)),
        "{error:?}"
    );
}

#[test]
fn packets_up_to_the_packet_limit_are_listed_by_runs_and_samples() {
    let spec = Spec::default();
    let data = movie(&spec);
    let largest = u64::from(*spec.sizes.iter().max().unwrap());
    let movie = read(&data, &Limits::default()).unwrap();
    let samples = spec.sizes.len() as u64;
    let walk = |max_packet_bytes, by_runs: bool| {
        let limits = Limits {
            max_packet_bytes,
            ..Limits::default()
        };
        let mut budget = Budget::new(&limits);
        let mut packets = movie.packets(0, SliceSource::new(&data), &limits, &mut budget)?;
        if !by_runs {
            return packets.for_each_sample(|_| Ok::<(), Error>(()));
        }
        let mut seen = 0;
        while let Some(run) = packets.next_run(u32::MAX) {
            seen += u64::from(run?.count);
        }
        Ok(seen)
    };
    for by_runs in [false, true] {
        assert_eq!(walk(largest, by_runs).unwrap(), samples, "runs: {by_runs}");
        let error = walk(largest - 1, by_runs).err();
        assert!(
            error.as_ref().is_some_and(refused(LimitKind::PacketBytes)),
            "runs: {by_runs}: {error:?}"
        );
    }
}

#[test]
fn listing_from_a_sample_stops_after_a_packet_over_the_limit_before_it() {
    let spec = Spec::default();
    let data = movie(&spec);
    let largest = u64::from(*spec.sizes.iter().max().unwrap());
    let last = spec.sizes.len() as u64 - 1;
    let movie = read(&data, &Limits::default()).unwrap();
    let from_last = |max_packet_bytes| {
        let limits = Limits {
            max_packet_bytes,
            ..Limits::default()
        };
        let mut budget = Budget::new(&limits);
        movie
            .packets_from(0, last, SliceSource::new(&data), &limits, &mut budget)
            .unwrap()
            .count()
    };
    assert_eq!(from_last(largest), 1);
    assert_eq!(from_last(largest - 1), 0);
}

#[test]
fn sample_visits_up_to_the_work_limit_are_listed_across_all_tracks() {
    let data = movie_of_tracks(2);
    let movie = read(&data, &Limits::default()).unwrap();
    let list = |max_sample_work, tracks: usize| {
        let limits = Limits {
            max_sample_work,
            ..Limits::default()
        };
        let mut budget = Budget::new(&limits);
        for index in 0..tracks {
            for packet in movie.packets(index, SliceSource::new(&data), &limits, &mut budget)? {
                packet?;
            }
        }
        Ok::<u64, Error>(budget.used())
    };
    let one = list(u64::MAX, 1).unwrap();
    let two = list(u64::MAX, 2).unwrap();
    assert!(one >= Spec::default().sizes.len() as u64, "{one}");
    assert_eq!(two, 2 * one);
    assert_eq!(list(one, 1).unwrap(), one);
    assert_eq!(list(two, 2).unwrap(), two);
    for (max, tracks) in [(one - 1, 1), (two - 1, 2)] {
        let error = list(max, tracks).err();
        assert!(
            error.as_ref().is_some_and(refused(LimitKind::SampleWork)),
            "{error:?}"
        );
    }
}

#[test]
fn fragments_up_to_the_limit_are_scanned() {
    let fragments = 6;
    let at = |max_fragments| {
        let limits = Limits {
            max_fragments,
            ..Limits::default()
        };
        let mut source = Fragmented::prores(fragments);
        let mut movie = probe(&mut source, &limits, &Allowlist::default())?;
        movie.scan_fragments(&mut source, &limits)?;
        Ok::<u64, Error>(movie.tracks[0].fragment_samples)
    };
    assert_eq!(
        at(fragments as u32).unwrap(),
        fragments * u64::from(Track::PRORES.samples)
    );
    let error = at(fragments as u32 - 1).err();
    assert!(
        error.as_ref().is_some_and(refused(LimitKind::Fragments)),
        "{error:?}"
    );
}

#[test]
fn fragments_up_to_the_limit_are_listed() {
    let fragments = 6;
    let at = |max_fragments| {
        let limits = Limits {
            max_fragments,
            ..Limits::default()
        };
        let mut source = Fragmented::prores(fragments);
        let movie = probe(&mut source, &limits, &Allowlist::default())?;
        let mut budget = Budget::new(&limits);
        let mut count = 0;
        for packet in movie.packets(0, &mut source, &limits, &mut budget)? {
            packet?;
            count += 1;
        }
        Ok::<u64, Error>(count)
    };
    assert_eq!(
        at(fragments as u32).unwrap(),
        fragments * u64::from(Track::PRORES.samples)
    );
    let error = at(fragments as u32 - 1).err();
    assert!(
        error.as_ref().is_some_and(refused(LimitKind::Fragments)),
        "{error:?}"
    );
}

#[test]
fn a_movie_with_fragments_is_refused_when_the_limit_is_zero() {
    let at = |max_fragments| {
        let limits = Limits {
            max_fragments,
            ..Limits::default()
        };
        probe(Fragmented::prores(1), &limits, &Allowlist::default())
    };
    assert!(at(1).is_ok());
    let error = at(0).err();
    assert!(
        error.as_ref().is_some_and(refused(LimitKind::Fragments)),
        "{error:?}"
    );
}

#[test]
fn fragments_up_to_the_limit_are_listed_from_a_subsegment() {
    let fragments = 6;
    let mut source = Fragmented::prores(fragments);
    let file_len = source.len().unwrap();
    let mut file = vec![0u8; file_len as usize];
    source.read_exact_at(0, &mut file).unwrap();
    let first = file.windows(4).position(|w| w == b"moof").unwrap() as u64 - 4;
    let defaults = TrackDefaults {
        track_id: 1,
        duration: Track::PRORES.duration,
        size: Track::PRORES.size,
        flags: 0,
    };
    let mut at = |max_fragments| {
        let limits = Limits {
            max_fragments,
            ..Limits::default()
        };
        let mut budget = Budget::new(&limits);
        subsegment_samples(
            &mut source,
            first,
            file_len - first,
            file_len,
            defaults,
            &limits,
            &mut budget,
        )
    };
    let listed = at(fragments as u32).unwrap();
    let samples: u64 = listed.runs.iter().map(|run| u64::from(run.count)).sum();
    assert_eq!(samples, fragments * u64::from(Track::PRORES.samples));
    let error = at(fragments as u32 - 1).err();
    assert!(
        error.as_ref().is_some_and(refused(LimitKind::Fragments)),
        "{error:?}"
    );
}

#[test]
fn tracks_up_to_the_limit_are_read() {
    let data = movie_of_tracks(4);
    pins_boundary(&data, LimitKind::Tracks, 4, |l, v| {
        l.max_tracks = v as u32;
    });
}

fn compressed(moov: &[u8]) -> Vec<u8> {
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(moov, 6);
    let mut cmvd = (moov.len() as u32).to_be_bytes().to_vec();
    cmvd.extend_from_slice(&packed);
    cat(&[
        ftyp(0),
        bx(
            b"moov",
            &bx(b"cmov", &cat(&[bx(b"dcom", b"zlib"), bx(b"cmvd", &cmvd)])),
        ),
    ])
}

#[test]
fn an_inflated_movie_up_to_the_movie_limit_is_read() {
    let moov = movie_box(&[bx(b"free", &[0u8; 400])]);
    let data = compressed(&moov);
    pins_boundary(&data, LimitKind::MovieBytes, moov.len() as u64, |l, v| {
        l.max_movie_bytes = v;
    });
}

#[test]
fn an_inflated_movie_up_to_the_ratio_limit_is_read() {
    let sized = |padding: usize| {
        let moov = movie_box(&[bx(b"free", &vec![0u8; padding])]);
        let data = compressed(&moov);
        let cmvd = data.windows(4).position(|w| w == b"cmvd").unwrap() - 4;
        let box_len = u32::from_be_bytes(data[cmvd..cmvd + 4].try_into().unwrap()) as u64;
        (data, moov.len() as u64, box_len - 8 - 4)
    };
    let (data, inflated, packed) = (2000..40_000)
        .map(sized)
        .find(|(_, inflated, packed)| inflated % packed == 1 && inflated / packed > 2)
        .unwrap();
    let ratio = inflated.div_ceil(packed);
    pins_boundary(&data, LimitKind::MovieRatio, ratio, |l, v| {
        l.max_movie_ratio = v as u32;
    });
}

fn remote(data: &[u8]) -> RangeReader<SliceSource<'_>> {
    RangeReader::new(SliceSource::new(data), &RangePolicy::default())
}

#[test]
fn a_range_reader_has_no_cap_until_one_is_set() {
    let data = vec![0u8; 4096];
    let mut reader = remote(&data);
    for at in 0..2000 {
        reader.insert(at, vec![0u8; 2]).unwrap();
    }
    assert_eq!((reader.requests(), reader.bytes_requested()), (2000, 4000));
}

#[test]
fn requested_bytes_up_to_the_byte_cap_are_fetched() {
    let data = vec![0u8; 4096];
    let mut reader = remote(&data);
    reader.set_max_bytes(Some(100));
    reader.insert(0, vec![0u8; 60]).unwrap();
    reader.insert(60, vec![0u8; 40]).unwrap();
    assert_eq!(reader.bytes_requested(), 100);
    let error = reader.insert(100, vec![0u8; 1]).err();
    assert!(
        error.as_ref().is_some_and(refused(LimitKind::ReadBytes)),
        "{error:?}"
    );
    assert_eq!((reader.requests(), reader.bytes_requested()), (2, 100));
    reader.set_max_bytes(None);
    reader.insert(100, vec![0u8; 1]).unwrap();
}

#[test]
fn requests_up_to_the_request_cap_are_sent() {
    let data = vec![0u8; 4096];
    let mut reader = remote(&data);
    reader.set_max_requests(Some(3));
    for at in 0..3 {
        reader.insert(at, vec![0u8; 1]).unwrap();
    }
    let error = reader.insert(3, vec![0u8; 1]).err();
    assert!(
        error.as_ref().is_some_and(refused(LimitKind::ReadRequests)),
        "{error:?}"
    );
    assert_eq!(reader.requests(), 3);
    reader.set_max_requests(None);
    reader.insert(3, vec![0u8; 1]).unwrap();
}

#[test]
fn reads_through_a_range_reader_stop_at_the_byte_cap_before_the_request() {
    let data = vec![7u8; 100_000];
    let mut reader = remote(&data);
    reader.set_max_bytes(Some(4096));
    reader.read_exact_at(0, &mut [0u8; 4096]).unwrap();
    assert_eq!(reader.bytes_requested(), 4096);
    let error = reader.read_exact_at(50_000, &mut [0u8; 1]).err();
    assert!(
        error.as_ref().is_some_and(refused(LimitKind::ReadBytes)),
        "{error:?}"
    );
    assert_eq!(reader.bytes_requested(), 4096);
}
