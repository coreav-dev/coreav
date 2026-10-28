//! The pinned memory bounds of the limits,
//! measured with the test-only counting allocator (`docs/design/limits.md`).
//!
//! Two bounds are pinned here: a 2.2 GB fragmented file is demuxed with
//! peak memory growth under 32 MiB, and a huge declared count with a tiny
//! table is rejected without allocating. The bars were fixed before the
//! run; a path that misses one is a limits bug.
// Test-only helpers outside `#[test]` functions index their own buffers.
#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

mod common;

use common::fragments::{Fragmented, Track};
use common::{Spec, bx, fragmented_movie, movie};
use coreav_counting_alloc::{Counters, CountingAllocator, measure, report};
use coreav_isobmff::{
    Allowlist, Budget, BufferedSource, Error, LimitKind, Limits, MAX_READ, SliceSource, Source,
    probe,
};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

/// Pinned bound on peak memory growth while a 2.2 GB fragmented file is
/// demuxed.
const DEMUX_PEAK: u64 = 32 << 20;

/// `docs/design/limits.md`: everything a rejecting call allocates, on an
/// input of a few kilobytes. Far below the 1 MiB `max_box_bytes`, and
/// about five orders of magnitude below the smallest table the declared
/// counts below would need.
const REJECT_PEAK: u64 = 64 << 10;

/// Fragments of the pinned file; the whole file is 2.2 GB.
const FRAGMENTS: u64 = 20_000;

/// Checks a measured region against a bar. A region that made no
/// allocator call at all would satisfy every bound here, so it is refused
/// first: that is what a bar looks like when the allocator is not
/// installed. A trough of zero then says the region never freed memory
/// allocated before it, so its peak is growth and nothing that was offset
/// against an earlier free.
fn under(bar: u64, what: &str, counters: &Counters) {
    report(what, counters);
    assert!(counters.calls() > 0, "{what}: measured nothing");
    assert!(counters.peak_bytes < bar, "{what}: {counters}");
    assert_eq!(counters.trough_bytes, 0, "{what}: {counters}");
}

/// Probes `source`, scans its fragments and lists every packet of every
/// track, returning the packets listed.
fn demux<S: Source>(source: &mut S, limits: &Limits) -> Result<u64, Error> {
    let mut movie = probe(&mut *source, limits, &Allowlist::default())?;
    movie.scan_fragments(&mut *source, limits)?;
    let mut budget = Budget::new(limits);
    let mut count = 0;
    for index in 0..movie.tracks.len() {
        for packet in movie.packets(index, &mut *source, limits, &mut budget)? {
            packet?;
            count += 1;
        }
    }
    Ok(count)
}

#[test]
#[ignore = "demuxes a 2.2 GB virtual file; run with --ignored --release"]
fn a_2_2_gb_fragmented_file_is_demuxed_under_32_mib() {
    let limits = Limits::default();
    let mut source = BufferedSource::new(Fragmented::prores(FRAGMENTS));
    let total = source.len().unwrap();
    println!("probe/fragmented-2.2gb: source {total} bytes");
    assert!(total > 2_200_000_000);

    let (mut movie, probed) =
        measure(|| probe(&mut source, &limits, &Allowlist::default()).unwrap());
    under(DEMUX_PEAK, "probe/fragmented-2.2gb", &probed);

    let (count, streamed) = measure(|| {
        movie.scan_fragments(&mut source, &limits).unwrap();
        let mut budget = Budget::new(&limits);
        let mut count = 0u64;
        for packet in movie.packets(0, &mut source, &limits, &mut budget).unwrap() {
            packet.unwrap();
            count += 1;
        }
        count
    });
    under(DEMUX_PEAK, "scan+packets/fragmented-2.2gb", &streamed);
    assert_eq!(count, FRAGMENTS * u64::from(Track::PRORES.samples));
    assert!(source.largest_read() <= MAX_READ);
}

/// Peak growth of a complete demux at each of the two fragment counts.
fn demux_peaks(counts: [u64; 2]) -> [u64; 2] {
    let limits = Limits::default();
    counts.map(|fragments| {
        let mut source = BufferedSource::new(Fragmented::prores(fragments));
        let (count, counters) = measure(|| demux(&mut source, &limits).unwrap());
        under(
            DEMUX_PEAK,
            &format!("demux/fragments-{fragments}"),
            &counters,
        );
        assert_eq!(count, fragments * u64::from(Track::PRORES.samples));
        counters.peak_bytes
    })
}

/// Memory does not grow with the number of fragments. One
/// `BufferedSource` window of slack (`MAX_READ`), nothing per fragment.
fn assert_flat(counts: [u64; 2]) {
    let [small, large] = demux_peaks(counts);
    assert!(
        large <= small + MAX_READ as u64,
        "{} fragments peaked at {large} B, {} at {small} B",
        counts[1],
        counts[0]
    );
}

#[test]
fn demux_memory_does_not_grow_with_fragments() {
    assert_flat([20, 200]);
}

#[test]
#[ignore = "demuxes a 2.2 GB virtual file; run with --ignored --release"]
fn demux_memory_is_flat_to_twenty_thousand_fragments() {
    assert_flat([200, FRAGMENTS]);
}

/// The default movie with the entry count of one table set to `count`,
/// leaving the table itself its four entries.
fn with_count(kind: &[u8; 4], count: u32) -> Vec<u8> {
    let mut data = movie(&Spec::default());
    let at = data.windows(4).position(|w| w == kind).unwrap();
    // After the type: the version and flags word, then `sample_size` in
    // `stsz` before its `sample_count`.
    let field = at + 8 + if kind == b"stsz" { 4 } else { 0 };
    data[field..field + 4].copy_from_slice(&count.to_be_bytes());
    data
}

/// The one-fragment movie with its `trun` sample count set to `count`.
fn with_run_count(count: u32) -> Vec<u8> {
    let mut data = fragmented_movie(None, None);
    let at = data.windows(4).position(|w| w == b"trun").unwrap();
    data[at + 8..at + 12].copy_from_slice(&count.to_be_bytes());
    data
}

fn probe_and_list(data: &[u8]) -> Result<u64, Error> {
    demux(&mut SliceSource::new(data), &Limits::default())
}

/// Measures `probe_and_list` on a file whose declared count is huge and on
/// one whose count is merely far too large, and returns what each
/// allocated. Allocation that followed the declared count could not give
/// the same numbers across a factor of 65,536.
///
/// `base` is the same file with its real count, which must parse: without
/// that check a file that failed for some other reason would pass every
/// bar here with two tiny, equal and meaningless numbers.
fn reject_at_two_counts(name: &str, base: &[u8], file: impl Fn(u32) -> Vec<u8>) {
    assert_eq!(
        probe_and_list(base).unwrap(),
        4,
        "{name}: the file must parse before its count is patched"
    );
    let counts = [u32::MAX, 65_537];
    let measured = counts.map(|count| {
        let data = file(count);
        let (result, counters) = measure(|| probe_and_list(&data));
        under(REJECT_PEAK, &format!("reject/{name}-{count}"), &counters);
        assert!(result.is_err(), "{name} at {count}: {result:?}");
        assert!(data.len() < 4096);
        counters
    });
    assert_eq!(
        measured[0], measured[1],
        "{name}: 2^32 - 1 allocated {}, 65,537 allocated {}",
        measured[0], measured[1]
    );
}

/// A sample count of 2^32 - 1 with a tiny size table is rejected without
/// allocating (a pinned limit test). Every count a file declares that could
/// drive an allocation is checked the same way.
#[test]
fn huge_counts_with_tiny_tables_are_rejected_without_allocating() {
    let base = movie(&Spec::default());
    for kind in [b"stsz", b"stco", b"stsc", b"stts", b"ctts", b"stss"] {
        let name = String::from_utf8_lossy(kind).into_owned();
        reject_at_two_counts(&name, &base, |count| with_count(kind, count));
    }
    reject_at_two_counts("trun", &fragmented_movie(None, None), with_run_count);
}

/// Bar 4 of `docs/design/limits.md`: a fragmented file that runs past
/// `max_fragments` is refused while it is read, holding nothing per
/// fragment it already passed.
#[test]
fn a_fragment_flood_is_rejected_within_the_demux_bound() {
    let limits = Limits {
        max_fragments: 8,
        ..Limits::default()
    };
    let mut source = BufferedSource::new(Fragmented::prores(2_000));
    let (result, counters) = measure(|| demux(&mut source, &limits));
    under(DEMUX_PEAK, "reject/fragment-flood", &counters);
    assert!(
        matches!(
            result,
            Err(Error::LimitExceeded {
                limit: LimitKind::Fragments,
                ..
            })
        ),
        "{result:?}"
    );
}

/// A 10,000-level nested file is rejected. It is pinned in
/// time in `hostile.rs`; here it is pinned in memory, so that the depth
/// limit is not met by keeping a frame per level.
#[test]
fn deep_nesting_is_rejected_without_allocating() {
    let levels = 10_000usize;
    let mut data = bx(b"ftyp", b"isom\0\0\x02\0");
    for i in 0..levels {
        let size = (8 * (levels - i)) as u32;
        data.extend_from_slice(&size.to_be_bytes());
        data.extend_from_slice(if i == 0 { b"moov" } else { b"trak" });
    }
    let (result, counters) = measure(|| probe_and_list(&data));
    under(REJECT_PEAK, "reject/nesting-10000", &counters);
    assert!(result.is_err(), "{result:?}");
}

/// A file of 10,001 top-level boxes is refused at the limit, holding none
/// of them.
#[test]
fn too_many_top_level_boxes_are_rejected_without_allocating() {
    let mut data = bx(b"ftyp", b"isom\0\0\x02\0");
    for _ in 0..10_001 {
        data.extend_from_slice(&bx(b"free", &[]));
    }
    let (result, counters) = measure(|| probe_and_list(&data));
    under(REJECT_PEAK, "reject/top-level-10001", &counters);
    assert!(
        matches!(
            result,
            Err(Error::LimitExceeded {
                limit: LimitKind::TopLevelBoxes,
                ..
            })
        ),
        "{result:?}"
    );
}

/// A box that claims 4 GiB in a file of a few hundred bytes is refused
/// without reserving what it claims.
#[test]
fn a_box_claiming_four_gib_is_rejected_without_allocating() {
    let mut data = movie(&Spec::default());
    let at = data.windows(4).position(|w| w == b"moov").unwrap();
    data[at - 4..at].copy_from_slice(&0xffff_fff0u32.to_be_bytes());
    println!("reject/box-4gib: source {} bytes", data.len());
    let (result, counters) = measure(|| probe_and_list(&data));
    under(REJECT_PEAK, "reject/box-4gib", &counters);
    assert!(result.is_err(), "{result:?}");
}

/// Issue #289, where this test pinned the gap it closes: a `trun` sample
/// count is checked against the bytes that back it, so a scan can no
/// longer report what a kilobyte claims (2^32 − 1 samples of 100 bytes
/// were 5.45 years of video). Probe still succeeds on the file, because it
/// never reads a fragment; the scan that does read it
/// refuses it as truncated — the bytes the run names are not there, which
/// is how listing the packets one by one reports it too. The three checks
/// behind this have a test of their own
/// (`a_trun_cannot_declare_samples_the_file_does_not_back` in
/// `tests/hostile.rs`); the bytes allocated before refusing are a bar above
/// (`huge_counts_with_tiny_tables_are_rejected_without_allocating`).
#[test]
fn a_fragment_cannot_claim_more_samples_than_the_file_holds() {
    let data = with_run_count(u32::MAX);
    assert!(data.len() < 2048);
    let limits = Limits::default();
    let mut source = SliceSource::new(&data);
    let mut movie = probe(&mut source, &limits, &Allowlist::default()).unwrap();
    assert_eq!(movie.tracks[0].fragment_samples, 0);
    let scanned = movie.scan_fragments(&mut source, &limits);
    assert!(
        matches!(scanned, Err(Error::Truncated { .. })),
        "{scanned:?}"
    );
    assert_eq!(movie.tracks[0].fragment_samples, 0);
    assert!(probe_and_list(&data).is_err());
}
