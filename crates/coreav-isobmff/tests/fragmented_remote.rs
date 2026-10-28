//! Remote index of fragmented files: routes, the byte and
//! request accounting of the range reader, and hostile segment indexes.
//! Files are built here from ISO/IEC 14496-12; no media is committed.

// Test helpers outside `#[test]` functions unwrap and panic like the tests.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use common::*;
use coreav_isobmff::{
    Allowlist, Budget, DownloadReason, Error, IndexRoute, IndexStatus, LimitKind, Limits, MAX_READ,
    Movie, RangePolicy, RangeReader, SliceSource, Source, plan_index, probe,
};

const AUDIO_SAMPLES: u32 = 10;
const AUDIO_SIZE: u32 = 300;

/// Layout of a generated two-track fragmented file.
#[derive(Clone)]
struct Layout {
    fragments: u32,
    video_samples: u32,
    video_size: u32,
    /// `None`: no segment index. `Some(version)`: one `sidx` per track over
    /// the same fragments, written before the first `moof`.
    sidx: Option<u8>,
    /// Split each subsegment reference into this many pieces at wrong
    /// offsets (a wrong but contiguous hint).
    misaligned: bool,
    trailer: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            fragments: 200,
            video_samples: 25,
            video_size: 2_000,
            sidx: Some(0),
            misaligned: false,
            trailer: true,
        }
    }
}

fn trak(track_id: u32, handler: &[u8; 4], entry: Vec<u8>) -> Vec<u8> {
    let mut tkhd = u32s(&[0, 0, track_id, 0, 0]);
    tkhd.extend_from_slice(&[0u8; 16]);
    tkhd.extend_from_slice(&u32s(&[
        0x10000,
        0,
        0,
        0,
        0x10000,
        0,
        0,
        0,
        0x4000_0000,
        0,
        0,
    ]));
    let mut mdhd = u32s(&[0, 0, TIMESCALE, 0]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(handler);
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&entry);
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(b"stsd", 0, 0, &stsd),
            full(b"stts", 0, 0, &u32s(&[0])),
            full(b"stsc", 0, 0, &u32s(&[0])),
            full(b"stsz", 0, 0, &u32s(&[0, 0])),
            full(b"stco", 0, 0, &u32s(&[0])),
        ]),
    );
    let mdia = bx(
        b"mdia",
        &cat(&[
            full(b"mdhd", 0, 0, &mdhd),
            full(b"hdlr", 0, 0, &hdlr),
            bx(b"minf", &stbl),
        ]),
    );
    bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia]))
}

fn head() -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"iso6\0\0\x02\0");
    let mut mvhd = u32s(&[0, 0, MOVIE_TIMESCALE, 0]);
    mvhd.extend_from_slice(&[0u8; 80]);
    let video = visual_entry(b"avc1", &bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]));
    let mvex = bx(
        b"mvex",
        &cat(&[
            full(b"trex", 0, 0, &u32s(&[1, 1, 512, 0, 0x0001_0000])),
            full(b"trex", 0, 0, &u32s(&[2, 1, 1024, AUDIO_SIZE, 0])),
        ]),
    );
    let moov = bx(
        b"moov",
        &cat(&[
            full(b"mvhd", 0, 0, &mvhd),
            trak(1, b"vide", video),
            trak(2, b"soun", aac_entry()),
            mvex,
        ]),
    );
    cat(&[ftyp, moov])
}

/// One fragment: `moof` with a video `traf` (per-sample sizes, first sample
/// sync) and an audio `traf` (defaults), then `mdat`.
fn fragment(layout: &Layout, index: u32) -> Vec<u8> {
    let traf = |track: u32, tfdt: u64, trun: Vec<u8>| {
        let tfhd = full(b"tfhd", 0, 0x02_0000, &u32s(&[track]));
        let tfdt = full(b"tfdt", 1, 0, &tfdt.to_be_bytes());
        bx(b"traf", &cat(&[tfhd, tfdt, trun]))
    };
    let build = |moof_len: u32| {
        let n = layout.video_samples;
        // Sizes vary per sample so that runs and moof sizes are realistic.
        let sizes: Vec<u32> = (0..n).map(|i| layout.video_size + (i % 7) * 13).collect();
        let video_bytes: u32 = sizes.iter().sum();
        let mut video_trun = u32s(&[n, moof_len + 8, 0x0200_0000]);
        video_trun.extend_from_slice(&u32s(&sizes));
        let video = traf(
            1,
            u64::from(index) * u64::from(n) * 512,
            full(b"trun", 0, 0x205, &video_trun),
        );
        let audio = traf(
            2,
            u64::from(index) * u64::from(AUDIO_SAMPLES) * 1024,
            full(
                b"trun",
                0,
                0x1,
                &u32s(&[AUDIO_SAMPLES, moof_len + 8 + video_bytes]),
            ),
        );
        let moof = bx(
            b"moof",
            &cat(&[full(b"mfhd", 0, 0, &u32s(&[index + 1])), video, audio]),
        );
        let media = video_bytes + AUDIO_SAMPLES * AUDIO_SIZE;
        (moof, media)
    };
    let (probe_moof, _) = build(0);
    let (moof, media) = build(probe_moof.len() as u32);
    let mdat = bx(b"mdat", &vec![(index % 251) as u8; media as usize]);
    cat(&[moof, mdat])
}

fn sidx(version: u8, track: u32, first_offset: u64, refs: &[(u32, u32)]) -> Vec<u8> {
    let mut p = u32s(&[track, TIMESCALE]);
    if version == 0 {
        p.extend_from_slice(&u32s(&[0, first_offset as u32]));
    } else {
        p.extend_from_slice(&0u64.to_be_bytes());
        p.extend_from_slice(&first_offset.to_be_bytes());
    }
    p.extend_from_slice(&[0, 0]);
    p.extend_from_slice(&(refs.len() as u16).to_be_bytes());
    for (size, duration) in refs {
        p.extend_from_slice(&u32s(&[*size, *duration, 0x9000_0000]));
    }
    full(b"sidx", version, 0, &p)
}

fn build(layout: &Layout) -> Vec<u8> {
    let fragments: Vec<Vec<u8>> = (0..layout.fragments).map(|i| fragment(layout, i)).collect();
    let duration = layout.video_samples * 512;
    let mut refs: Vec<(u32, u32)> = fragments
        .iter()
        .map(|f| (f.len() as u32, duration))
        .collect();
    if layout.misaligned {
        // Same total, every boundary after the second moved past a moof start.
        let last = refs.len() - 1;
        refs[1].0 += 100;
        refs[last].0 -= 100;
    }
    let mut out = head();
    if let Some(version) = layout.sidx {
        let second = sidx(version, 2, 0, &refs);
        out.extend(sidx(version, 1, second.len() as u64, &refs));
        out.extend(second);
    }
    for f in fragments {
        out.extend(f);
    }
    if layout.trailer {
        // A trailing non-fragment box, like `mfra`.
        out.extend(bx(b"free", &[0u8; 64]));
    }
    out
}

/// (track, sample, offset, size, dts, cts, keyframe) of every packet.
type Row = (usize, u64, u64, u32, i64, i64, bool);

fn packets<S: Source>(movie: &Movie, source: &mut S) -> Result<Vec<Row>, Error> {
    let limits = Limits::default();
    let mut rows = Vec::new();
    for track in 0..movie.tracks.len() {
        let mut budget = Budget::new(&limits);
        for p in movie.packets(track, &mut *source, &limits, &mut budget)? {
            let p = p?;
            rows.push((track, p.sample, p.offset, p.size, p.dts, p.cts, p.keyframe));
        }
    }
    Ok(rows)
}

fn local(data: &[u8]) -> Vec<Row> {
    let movie = probe(
        SliceSource::new(data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    packets(&movie, &mut SliceSource::new(data)).unwrap()
}

struct Remote<'a> {
    reader: RangeReader<SliceSource<'a>>,
    movie: Movie,
    probe_requests: u64,
    route: IndexRoute,
}

fn remote<'a>(data: &'a [u8], policy: &RangePolicy) -> Result<Remote<'a>, Error> {
    let mut reader = RangeReader::new(SliceSource::new(data), policy);
    let movie = probe(&mut reader, &Limits::default(), &Allowlist::default())?;
    let probe_requests = reader.requests();
    let route = plan_index(&movie, &mut reader, policy)?;
    Ok(Remote {
        reader,
        movie,
        probe_requests,
        route,
    })
}

/// The byte bar (5% of the object) and the request count on a
/// 200-fragment file: probe plus one window per subsegment, nothing else,
/// and the packets of a local listing.
#[test]
fn segment_index_route_stays_within_the_read_budget() {
    for version in [0, 1] {
        let layout = Layout {
            sidx: Some(version),
            ..Layout::default()
        };
        let data = build(&layout);
        let expected = local(&data);
        let policy = RangePolicy::default();
        let mut r = remote(&data, &policy).unwrap();
        let IndexRoute::SegmentIndex(plan) = r.route.clone() else {
            panic!("route {:?}", r.route);
        };
        assert_eq!(plan.subsegments, 200);
        // The first fragment's window was already fetched by probe.
        assert_eq!(plan.windows.len(), 199);
        assert!(plan.windows.iter().all(|w| w.len <= 4096 && w.len >= 512));
        r.reader.fetch(&plan.windows).unwrap();
        let rows = packets(&r.movie, &mut r.reader).unwrap();
        assert_eq!(rows, expected);
        assert_eq!(rows.len(), 200 * (25 + 10));

        let len = data.len() as u64;
        let bytes = r.reader.bytes_requested();
        let requests = r.reader.requests();
        println!(
            "sidx v{version}: {len} bytes, probe {} requests, total {requests} requests, \
             {bytes} bytes ({:.3}%)",
            r.probe_requests,
            bytes as f64 * 100.0 / len as f64
        );
        assert!(bytes * 20 <= len, "{bytes} of {len} bytes");
        assert_eq!(bytes, plan.requested + plan.window_bytes());
        // No miss during the walk: probe, at most one trailer header, windows.
        assert!(requests <= r.probe_requests + 1 + plan.windows.len() as u64);
        assert!(r.probe_requests <= 3, "probe took {}", r.probe_requests);
        assert!(r.reader.largest_read() <= MAX_READ);
    }
}

/// Windows fetched elsewhere (a concurrent client) and inserted give the same
/// packets and the same accounting.
#[test]
fn inserted_windows_count_like_fetched_ones() {
    let data = build(&Layout::default());
    let policy = RangePolicy::default();
    let mut r = remote(&data, &policy).unwrap();
    let IndexRoute::SegmentIndex(plan) = r.route.clone() else {
        panic!("route {:?}", r.route);
    };
    let before = r.reader.requests();
    for w in plan.windows.iter().rev() {
        let bytes = data[w.offset as usize..w.end() as usize].to_vec();
        r.reader.insert(w.offset, bytes).unwrap();
    }
    assert_eq!(r.reader.requests(), before + plan.windows.len() as u64);
    assert_eq!(packets(&r.movie, &mut r.reader).unwrap(), local(&data));
    assert_eq!(r.reader.requests(), before + plan.windows.len() as u64);
}

#[test]
fn heavily_fragmented_without_index_is_routed_to_download() {
    let data = build(&Layout {
        sidx: None,
        ..Layout::default()
    });
    let r = remote(&data, &RangePolicy::default()).unwrap();
    match r.route {
        IndexRoute::DownloadThenIndex(DownloadReason::TooManyUnindexedFragments {
            estimated_fragments,
            index: IndexStatus::Absent,
        }) => assert!((199..=201).contains(&estimated_fragments)),
        other => panic!("route {other:?}"),
    }
    // Detection costs no fragment walk.
    assert!(r.reader.requests() <= r.probe_requests + 1);
}

#[test]
fn few_unindexed_fragments_are_walked_under_a_request_cap() {
    let data = build(&Layout {
        sidx: None,
        fragments: 4,
        video_size: 40_000,
        ..Layout::default()
    });
    let mut r = remote(&data, &RangePolicy::default()).unwrap();
    assert!(
        matches!(
            r.route,
            IndexRoute::Sequential {
                estimated_fragments: 4 | 5,
                index: IndexStatus::Absent
            }
        ),
        "{:?}",
        r.route
    );
    assert_eq!(packets(&r.movie, &mut r.reader).unwrap(), local(&data));
    assert!(r.reader.bytes_requested() * 20 <= data.len() as u64);
}

/// A sequential route whose estimate was wrong stops at the request cap.
#[test]
fn a_wrong_estimate_hits_the_request_cap() {
    // The first fragment is large, the others small: the estimate is low.
    let mut data = head();
    let big = Layout {
        fragments: 1,
        video_size: 400_000,
        sidx: None,
        trailer: false,
        ..Layout::default()
    };
    data.extend(fragment(&big, 0));
    let small = Layout {
        video_size: 100,
        video_samples: 2,
        ..big.clone()
    };
    for i in 1..200 {
        data.extend(fragment(&small, i));
    }
    let mut r = remote(&data, &RangePolicy::default()).unwrap();
    assert!(
        matches!(r.route, IndexRoute::Sequential { .. }),
        "{:?}",
        r.route
    );
    let error = packets(&r.movie, &mut r.reader).unwrap_err();
    assert!(matches!(
        error,
        Error::LimitExceeded {
            limit: LimitKind::ReadRequests | LimitKind::ReadBytes,
            ..
        }
    ));
}

#[test]
fn small_objects_break_the_byte_bar_and_are_routed_to_download() {
    let data = build(&Layout {
        fragments: 50,
        video_samples: 2,
        video_size: 300,
        ..Layout::default()
    });
    let r = remote(&data, &RangePolicy::default()).unwrap();
    match r.route {
        IndexRoute::DownloadThenIndex(DownloadReason::ByteBudget { planned, allowed }) => {
            assert!(planned > allowed);
            assert_eq!(allowed, data.len() as u64 / 20);
        }
        other => panic!("route {other:?}"),
    }
}

#[test]
fn no_fragments_needs_no_plan() {
    let data = movie(&Spec::default());
    let r = remote(&data, &RangePolicy::default()).unwrap();
    assert_eq!(r.route, IndexRoute::MovieBox);
}

/// A contiguous index with boundaries off the fragment starts is a wrong
/// hint: extra reads, same packets.
#[test]
fn a_misaligned_index_costs_reads_not_results() {
    let data = build(&Layout {
        misaligned: true,
        fragments: 60,
        video_size: 4_000,
        ..Layout::default()
    });
    let mut r = remote(&data, &RangePolicy::default()).unwrap();
    let IndexRoute::SegmentIndex(plan) = r.route.clone() else {
        panic!("route {:?}", r.route);
    };
    r.reader.fetch(&plan.windows).unwrap();
    let before = r.reader.requests();
    assert_eq!(packets(&r.movie, &mut r.reader).unwrap(), local(&data));
    assert!(r.reader.requests() > before);
    assert!(r.reader.bytes_requested() * 20 <= data.len() as u64);
}

/// The file of `layout` with its two segment indexes replaced: each is
/// (extra bytes added to `first_offset`, references).
fn with_indexes(
    layout: &Layout,
    first: (u64, &[(u32, u32)]),
    second: (u64, &[(u32, u32)]),
) -> Vec<u8> {
    let data = build(layout);
    let head = head();
    let old = sidx(0, 1, 0, &vec![(0, 0); layout.fragments as usize]).len();
    let rest = &data[head.len() + 2 * old..];
    let second = sidx(0, 2, second.0, second.1);
    cat(&[
        head,
        sidx(0, 1, second.len() as u64 + first.0, first.1),
        second,
        rest.to_vec(),
    ])
}

fn with_refs(layout: &Layout, refs: &[(u32, u32)], flags: u32) -> Vec<u8> {
    let mut patched = refs.to_vec();
    patched[0].0 |= flags;
    with_indexes(layout, (0, &patched), (0, &patched))
}

fn fragment_refs(layout: &Layout) -> Vec<(u32, u32)> {
    (0..layout.fragments)
        .map(|i| (fragment(layout, i).len() as u32, 512))
        .collect()
}

fn status_of(data: &[u8]) -> IndexStatus {
    match remote(data, &RangePolicy::default()).unwrap().route {
        IndexRoute::DownloadThenIndex(DownloadReason::TooManyUnindexedFragments {
            index, ..
        })
        | IndexRoute::Sequential { index, .. } => index,
        other => panic!("route {other:?}"),
    }
}

#[test]
fn unusable_indexes_are_hints_not_errors() {
    let layout = Layout::default();
    let refs = fragment_refs(&layout);
    // Hierarchical: the first reference points to another index.
    assert_eq!(
        status_of(&with_refs(&layout, &refs, 0x8000_0000)),
        IndexStatus::Hierarchical
    );
    // Two fragments per subsegment.
    let pairs: Vec<(u32, u32)> = refs
        .chunks(2)
        .map(|c| (c.iter().map(|r| r.0).sum(), 1024))
        .collect();
    assert_eq!(
        status_of(&with_indexes(&layout, (0, &pairs), (0, &pairs))),
        IndexStatus::SeveralFragmentsPerSubsegment
    );
    // Covers only the first fragments: what follows is a `moof`.
    assert_eq!(
        status_of(&with_refs(&layout, &refs[..10], 0)),
        IndexStatus::Incomplete
    );
    // A gap: the second index starts one fragment after the first ends.
    let skip: u64 = refs[..101].iter().map(|r| u64::from(r.0)).sum();
    assert_eq!(
        status_of(&with_indexes(
            &layout,
            (0, &refs[..100]),
            (skip, &refs[101..])
        )),
        IndexStatus::NotContiguous
    );
    // Overlapping references of different sizes.
    let mut shifted = refs.clone();
    shifted[0].0 += 10;
    shifted[1].0 -= 10;
    assert_eq!(
        status_of(&with_indexes(&layout, (0, &refs), (0, &shifted))),
        IndexStatus::NotContiguous
    );
    // The hint ends inside a media data box.
    let mut short = refs.clone();
    short.pop();
    let last = short.len() - 1;
    short[last].0 -= 7;
    assert_eq!(
        status_of(&with_refs(&layout, &short, 0)),
        IndexStatus::NotContiguous
    );
    // Wrong start: both indexes start 16 bytes into the first fragment.
    let mut late = refs.clone();
    late.pop();
    assert_eq!(
        status_of(&with_indexes(&layout, (16, &late), (16, &late))),
        IndexStatus::WrongStart
    );
}

/// A root index whose only reference names the leaf index before the first
/// fragment: probe has read the leaf, so the leaf's references are used.
#[test]
fn a_root_index_over_a_leading_leaf_index_is_resolved() {
    let layout = Layout::default();
    let data = build(&layout);
    let head = head();
    let old = sidx(0, 1, 0, &vec![(0, 0); layout.fragments as usize]).len();
    let rest = &data[head.len() + 2 * old..];
    let leaf = sidx(1, 1, 0, &fragment_refs(&layout));
    let root = sidx(1, 1, 0, &[(0x8000_0000 | leaf.len() as u32, 0)]);
    let file = cat(&[head, root, leaf, rest.to_vec()]);
    let mut r = remote(&file, &RangePolicy::default()).unwrap();
    let IndexRoute::SegmentIndex(plan) = r.route.clone() else {
        panic!("route {:?}", r.route);
    };
    assert_eq!(plan.subsegments, 200);
    r.reader.fetch(&plan.windows).unwrap();
    assert_eq!(packets(&r.movie, &mut r.reader).unwrap(), local(&file));
}

#[test]
fn references_outside_the_object_reject_the_file() {
    let layout = Layout::default();
    let mut refs = fragment_refs(&layout);
    refs.last_mut().unwrap().0 += 1_000;
    let data = with_refs(&layout, &refs, 0);
    assert!(matches!(
        remote(&data, &RangePolicy::default()),
        Err(Error::Truncated { .. })
    ));
    refs.last_mut().unwrap().0 = 0x7fff_ffff;
    let data = with_refs(&layout, &refs, 0);
    assert!(remote(&data, &RangePolicy::default()).is_err());
}

/// 60,000 references, each valid, over a 60 MB object: the plan would
/// exceed the speculative budget, which is checked before any window is
/// allocated or fetched.
#[test]
fn huge_reference_counts_are_bounded_by_the_speculative_budget() {
    let layout = Layout {
        fragments: 1,
        trailer: false,
        ..Layout::default()
    };
    let first = fragment(&layout, 0);
    let count = 60_000usize;
    let filler = 1_000usize;
    let mut refs = vec![(first.len() as u32, 512)];
    refs.extend(std::iter::repeat_n((filler as u32, 512), count - 1));
    let second = sidx(1, 2, 0, &refs);
    let mut data = cat(&[
        head(),
        sidx(1, 1, second.len() as u64, &refs),
        second,
        first,
    ]);
    data.resize(data.len() + filler * (count - 1), 0);
    let r = remote(&data, &RangePolicy::default()).unwrap();
    match r.route {
        IndexRoute::DownloadThenIndex(DownloadReason::SpeculativeBudget { planned, allowed }) => {
            assert!(planned > allowed)
        }
        other => panic!("route {other:?}"),
    }
    assert!(r.reader.requests() <= r.probe_requests + 1);
}

#[test]
fn a_reference_count_larger_than_the_box_is_rejected() {
    let layout = Layout {
        fragments: 3,
        ..Layout::default()
    };
    let mut data = build(&layout);
    // reference_count of the first `sidx`: header 8, version and flags 4,
    // reference_ID 4, timescale 4, two 32-bit times 8, reserved 2.
    let at = head().len() + 8 + 4 + 4 + 4 + 8 + 2;
    data[at..at + 2].copy_from_slice(&u16::MAX.to_be_bytes());
    assert!(matches!(
        remote(&data, &RangePolicy::default()),
        Err(Error::InvalidBox { .. })
    ));
}

/// Deterministic mutations of the segment indexes and the first fragment:
/// never a panic; when the remote path succeeds, its packets are the local
/// packets.
#[test]
fn mutated_indexes_never_panic_and_never_change_packets() {
    let layout = Layout {
        fragments: 12,
        ..Layout::default()
    };
    let data = build(&layout);
    let start = head().len();
    let end = (start + 400).min(data.len());
    let mut state = 0x2545_f491_4f6c_dd1du64;
    for _ in 0..1_500 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let mut mutated = data.clone();
        let at = start + (state as usize % (end - start));
        mutated[at] = (state >> 32) as u8;
        let local_rows = probe(
            SliceSource::new(&mutated),
            &Limits::default(),
            &Allowlist::default(),
        )
        .and_then(|m| packets(&m, &mut SliceSource::new(&mutated)));
        let Ok(mut r) = remote(&mutated, &RangePolicy::default()) else {
            continue;
        };
        if let IndexRoute::SegmentIndex(plan) = &r.route {
            let windows = plan.windows.clone();
            if r.reader.fetch(&windows).is_err() {
                continue;
            }
        }
        if let (Ok(rows), Ok(expected)) = (packets(&r.movie, &mut r.reader), &local_rows) {
            assert_eq!(&rows, expected);
        }
    }
}

/// One fragment whose `moof` is larger than any window: the second read of
/// its rest is predicted, so the byte bar routes it before the walk.
#[test]
fn a_moof_larger_than_the_window_is_predicted() {
    let layout = Layout {
        fragments: 1,
        video_samples: 4_000,
        video_size: 10,
        ..Layout::default()
    };
    let data = build(&layout);
    let r = remote(&data, &RangePolicy::default()).unwrap();
    assert!(
        matches!(
            r.route,
            IndexRoute::DownloadThenIndex(DownloadReason::ByteBudget { .. })
        ),
        "{:?}",
        r.route
    );
    // With a larger object the same plan fits and the walk stays in budget.
    let bigger = Layout {
        video_size: 2_000,
        ..layout
    };
    let data = build(&bigger);
    let mut r = remote(&data, &RangePolicy::default()).unwrap();
    let IndexRoute::SegmentIndex(plan) = r.route.clone() else {
        panic!("route {:?}", r.route);
    };
    r.reader.fetch(&plan.windows).unwrap();
    assert_eq!(packets(&r.movie, &mut r.reader).unwrap(), local(&data));
}

/// Near the byte bar with every `moof` larger than its window: the walk
/// reads only the rest of each `moof`, so an approved plan stays in budget
/// (review finding on pull request 17).
#[test]
fn large_fragment_headers_near_the_budget_stay_in_budget() {
    let mut indexed = 0;
    for (fragments, samples, size) in [(20, 2_000, 60), (20, 2_000, 50), (40, 1_000, 120)] {
        let layout = Layout {
            fragments,
            video_samples: samples,
            video_size: size,
            ..Layout::default()
        };
        let data = build(&layout);
        let mut r = remote(&data, &RangePolicy::default()).unwrap();
        match r.route.clone() {
            IndexRoute::SegmentIndex(plan) => {
                indexed += 1;
                r.reader.fetch(&plan.windows).unwrap();
                assert_eq!(packets(&r.movie, &mut r.reader).unwrap(), local(&data));
                assert!(r.reader.bytes_requested() <= plan.byte_budget);
            }
            IndexRoute::DownloadThenIndex(DownloadReason::ByteBudget { .. }) => {}
            other => panic!("route {other:?}"),
        }
    }
    assert!(indexed >= 2, "only {indexed} plans");
}

/// A reader that cannot keep the planned windows gets a typed route instead
/// of a plan whose windows would be read twice.
#[test]
fn a_small_cache_is_routed_before_the_walk() {
    let data = build(&Layout::default());
    let policy = RangePolicy {
        max_held_bytes: 4096,
        ..RangePolicy::default()
    };
    let r = remote(&data, &policy).unwrap();
    assert!(
        matches!(
            r.route,
            IndexRoute::DownloadThenIndex(DownloadReason::HeldBudget { .. })
        ),
        "{:?}",
        r.route
    );
}
