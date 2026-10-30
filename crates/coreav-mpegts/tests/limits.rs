//! Every field of `Limits` is pinned from the outside: its default, the
//! input just inside it, and the input just past it, which is refused with
//! the error that names the limit (`docs/design/mpegts.md`, "Limits").
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod common;

use common::{Mux, adts};
use coreav_mpegts::{
    Allowlist, Error, LimitKind, Limits, SliceSource, TransportStream, Warning, probe,
};

const TS_PACKET: u64 = 188;

fn probe_with(data: &[u8], limits: &Limits) -> Result<TransportStream, Error> {
    probe(SliceSource::new(data), limits, &Allowlist::default())
}

fn assert_limit<T: std::fmt::Debug>(got: Result<T, Error>, kind: LimitKind) {
    assert!(
        matches!(got, Err(Error::LimitExceeded { limit, .. }) if limit == kind),
        "{got:?}"
    );
}

fn listing_error(ts: &TransportStream, data: &[u8], limits: &Limits) -> Option<Error> {
    ts.packets(SliceSource::new(data), limits)
        .find_map(Result::err)
}

fn filler(mux: &mut Mux, packets: usize) {
    for _ in 0..packets {
        mux.packet(0x1FFF, false, &[0xFF; 184]);
    }
}

fn audio_mux() -> Mux {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    mux
}

fn programs_mux(programs: u16) -> Mux {
    let mut mux = Mux::default();
    let listed: Vec<_> = (1..=programs).map(|n| (n, 0x100 + n)).collect();
    mux.pat(&listed);
    for n in 1..=programs {
        mux.pmt(0x100 + n, n, &[(0x0F, 0x1000 + n)]);
    }
    mux
}

fn streams_mux(per_program: &[u16]) -> Mux {
    let mut mux = Mux::default();
    let listed: Vec<_> = (0..per_program.len() as u16)
        .map(|n| (n + 1, 0x100 + n))
        .collect();
    mux.pat(&listed);
    for (n, count) in per_program.iter().enumerate() {
        let streams: Vec<_> = (0..*count)
            .map(|s| (0x0F, 0x1000 + 16 * n as u16 + s))
            .collect();
        mux.pmt(0x100 + n as u16, n as u16 + 1, &streams);
    }
    mux
}

#[test]
fn the_defaults_are_the_documented_limits() {
    assert_eq!(
        Limits::default(),
        Limits {
            max_probe_bytes: 16 * 1024 * 1024,
            max_tail_bytes: 4 * 1024 * 1024,
            tail_retries: 0,
            max_resync_bytes: 1024 * 1024,
            max_programs: 256,
            max_streams: 1_024,
            max_packet_bytes: 64 * 1024 * 1024,
            max_buffered_bytes: 256 * 1024 * 1024,
            max_listing_packets: 1 << 32,
        }
    );
}

#[test]
fn tables_past_max_probe_bytes_are_refused_with_the_limit() {
    let fillers = 6;
    let mut mux = Mux::default();
    filler(&mut mux, fillers);
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    let tables_end = (fillers as u64 + 2) * TS_PACKET;
    let limits = Limits {
        max_probe_bytes: tables_end,
        ..Limits::default()
    };
    let ts = probe_with(&mux.out, &limits).unwrap();
    assert_eq!(ts.streams.len(), 1);
    let limits = Limits {
        max_probe_bytes: tables_end - TS_PACKET,
        ..Limits::default()
    };
    assert_eq!(
        probe_with(&mux.out, &limits).err(),
        Some(Error::LimitExceeded {
            limit: LimitKind::ProbeBytes,
            offset: tables_end - TS_PACKET,
        })
    );
}

/// A file that names no program is refused for that when the probe read
/// all of it, and for the limit when the budget left packets unread.
#[test]
fn a_file_without_tables_read_whole_within_max_probe_bytes_names_no_program() {
    let mut mux = Mux::default();
    filler(&mut mux, 6);
    let len = mux.out.len() as u64;
    let limits = |max_probe_bytes| Limits {
        max_probe_bytes,
        ..Limits::default()
    };
    assert_eq!(
        probe_with(&mux.out, &limits(len)).err(),
        Some(Error::NoPrograms)
    );
    assert_eq!(
        probe_with(&mux.out, &limits(len - TS_PACKET)).err(),
        Some(Error::LimitExceeded {
            limit: LimitKind::ProbeBytes,
            offset: len - TS_PACKET,
        })
    );
}

#[test]
fn a_stream_whose_first_frame_is_past_max_probe_bytes_has_no_parameters() {
    let mut mux = audio_mux();
    filler(&mut mux, 6);
    mux.pes(0x101, 0xC0, 90_000, &adts(100), true);
    let frame_end = mux.out.len() as u64;
    let limits = Limits {
        max_probe_bytes: frame_end,
        ..Limits::default()
    };
    let ts = probe_with(&mux.out, &limits).unwrap();
    assert!(ts.streams[0].audio.is_some());
    let limits = Limits {
        max_probe_bytes: frame_end - TS_PACKET,
        ..Limits::default()
    };
    let ts = probe_with(&mux.out, &limits).unwrap();
    assert!(ts.streams[0].audio.is_none());
}

#[test]
fn a_pat_with_more_programs_than_max_programs_is_refused() {
    let limits = Limits {
        max_programs: 4,
        ..Limits::default()
    };
    let ts = probe_with(&programs_mux(4).out, &limits).unwrap();
    assert_eq!(ts.programs.len(), 4);
    assert_limit(
        probe_with(&programs_mux(5).out, &limits),
        LimitKind::Programs,
    );
}

#[test]
fn services_past_max_programs_are_not_kept() {
    let service = |id: u16| {
        let descriptor = [0x48, 0x05, 0x01, 0x01, b'P', 0x01, b'N'];
        let mut entry = id.to_be_bytes().to_vec();
        entry.extend([0xFC, 0x80, descriptor.len() as u8]);
        entry.extend(descriptor);
        entry
    };
    let mut mux = Mux::default();
    let mut body = vec![0xFF, 0x01, 0xFF];
    for id in 1..=5 {
        body.extend(service(id));
    }
    mux.section(0x11, 0x42, 1, &body);
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    let limits = Limits {
        max_programs: 4,
        ..Limits::default()
    };
    let ts = probe_with(&mux.out, &limits).unwrap();
    assert_eq!(ts.services.len(), 4);
}

#[test]
fn a_pmt_with_more_streams_than_max_streams_is_refused() {
    let limits = Limits {
        max_streams: 4,
        ..Limits::default()
    };
    let ts = probe_with(&streams_mux(&[4]).out, &limits).unwrap();
    assert_eq!(ts.streams.len(), 4);
    assert_limit(
        probe_with(&streams_mux(&[5]).out, &limits),
        LimitKind::Streams,
    );
}

#[test]
fn max_streams_counts_the_streams_of_every_program() {
    let limits = Limits {
        max_streams: 4,
        ..Limits::default()
    };
    let ts = probe_with(&streams_mux(&[2, 2]).out, &limits).unwrap();
    assert_eq!(ts.streams.len(), 4);
    assert_limit(
        probe_with(&streams_mux(&[3, 2]).out, &limits),
        LimitKind::Streams,
    );
}

#[test]
fn pes_pids_past_max_streams_in_a_file_without_tables_are_not_listed() {
    let mut mux = Mux::default();
    for pid in 0x100..0x105u16 {
        mux.pes(pid, 0xC0, 90_000, &adts(100), true);
    }
    let limits = Limits {
        max_streams: 4,
        ..Limits::default()
    };
    let ts = probe_with(&mux.out, &limits).unwrap();
    assert_eq!(ts.streams.len(), 4);
    let ts = probe_with(&mux.out, &Limits::default()).unwrap();
    assert_eq!(ts.streams.len(), 5);
}

#[test]
fn a_pes_packet_longer_than_max_packet_bytes_is_refused() {
    let mut mux = audio_mux();
    mux.pes(0x101, 0xC0, 90_000, &adts(300), true);
    let pes_bytes = 14 + 300;
    let at_limit = Limits {
        max_packet_bytes: pes_bytes,
        ..Limits::default()
    };
    let ts = probe_with(&mux.out, &at_limit).unwrap();
    assert!(ts.streams[0].audio.is_some());
    assert_eq!(listing_error(&ts, &mux.out, &at_limit), None);

    let past = Limits {
        max_packet_bytes: pes_bytes - 1,
        ..Limits::default()
    };
    assert_limit(probe_with(&mux.out, &past), LimitKind::PacketBytes);
    let ts = probe_with(&mux.out, &Limits::default()).unwrap();
    let listed = listing_error(&ts, &mux.out, &past);
    assert_limit(Err::<(), _>(listed.unwrap()), LimitKind::PacketBytes);
}

fn unbounded_starts(pids: u16) -> Mux {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    let streams: Vec<_> = (0..pids).map(|i| (0x0F, 0x200 + i)).collect();
    mux.pmt(0x1000, 1, &streams);
    let mut start = vec![0, 0, 1, 0xC0, 0, 0, 0x80, 0, 0];
    start.resize(184, 0x55);
    for (_, pid) in &streams {
        mux.packet(*pid, true, &start);
    }
    mux
}

#[test]
fn more_bytes_held_across_pids_than_max_buffered_bytes_are_refused() {
    let mux = unbounded_starts(3);
    let held = 3 * 184;
    let at_limit = Limits {
        max_buffered_bytes: held,
        ..Limits::default()
    };
    let ts = probe_with(&mux.out, &at_limit).unwrap();
    assert_eq!(listing_error(&ts, &mux.out, &at_limit), None);

    let past = Limits {
        max_buffered_bytes: held - 1,
        ..Limits::default()
    };
    assert_limit(probe_with(&mux.out, &past), LimitKind::BufferedBytes);
    let ts = probe_with(&mux.out, &Limits::default()).unwrap();
    let listed = listing_error(&ts, &mux.out, &past);
    assert_limit(Err::<(), _>(listed.unwrap()), LimitKind::BufferedBytes);
}

#[test]
fn a_gap_without_a_sync_byte_longer_than_max_resync_bytes_is_refused() {
    let limits = Limits {
        max_resync_bytes: 300,
        ..Limits::default()
    };
    let with_gap = |gap: usize| {
        let mut mux = audio_mux();
        mux.pes(0x101, 0xC0, 90_000, &adts(100), true);
        mux.out.extend(std::iter::repeat_n(0x11, gap));
        mux.pes(0x101, 0xC0, 95_760, &adts(100), true);
        mux.out
    };

    let inside = with_gap(299);
    let ts = probe_with(&inside, &limits).unwrap();
    let mut listing = ts.packets(SliceSource::new(&inside), &limits);
    assert!(listing.all(|packet| packet.is_ok()));
    assert!(
        listing
            .warnings()
            .iter()
            .any(|w| matches!(w, Warning::LostSync { skipped: 299, .. }))
    );

    let past = with_gap(300);
    let ts = probe_with(&past, &limits).unwrap();
    let listed = listing_error(&ts, &past, &limits);
    assert_limit(Err::<(), _>(listed.unwrap()), LimitKind::ResyncBytes);
}

#[test]
fn bytes_before_the_first_packet_are_searched_up_to_max_resync_bytes() {
    let limits = Limits {
        max_resync_bytes: 300,
        ..Limits::default()
    };
    let with_prefix = |prefix: usize| {
        let mut data = vec![0x11; prefix];
        let mut mux = audio_mux();
        for i in 0..3u64 {
            mux.pes(0x101, 0xC0, 90_000 + 5_760 * i, &adts(100), true);
        }
        data.extend(mux.out);
        data
    };
    let ts = probe_with(&with_prefix(299), &limits).unwrap();
    assert_eq!(ts.first_packet, 299);
    assert_eq!(
        probe_with(&with_prefix(300), &limits).err(),
        Some(Error::NotTransportStream)
    );
}

#[test]
fn a_listing_of_more_packets_than_max_listing_packets_stops_with_an_error() {
    let mut mux = audio_mux();
    for i in 0..6u64 {
        mux.pes(0x101, 0xC0, 90_000 + 5_760 * i, &adts(100), true);
    }
    let total = mux.out.len() as u64 / TS_PACKET;
    let ts = probe_with(&mux.out, &Limits::default()).unwrap();

    let at_limit = Limits {
        max_listing_packets: total,
        ..Limits::default()
    };
    assert_eq!(listing_error(&ts, &mux.out, &at_limit), None);

    let past = Limits {
        max_listing_packets: total - 1,
        ..Limits::default()
    };
    assert_eq!(
        listing_error(&ts, &mux.out, &past),
        Some(Error::LimitExceeded {
            limit: LimitKind::ListingPackets,
            offset: (total - 1) * TS_PACKET,
        })
    );
}

fn tail_mux(between: usize) -> Mux {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101), (0x0F, 0x102)]);
    mux.pes(0x101, 0xC0, 90_000, &adts(100), true);
    mux.pes(0x102, 0xC0, 90_000, &adts(100), true);
    mux.pes(0x101, 0xC0, 180_000, &adts(100), true);
    filler(&mut mux, between);
    mux.pes(0x102, 0xC0, 270_000, &adts(100), true);
    mux
}

fn last_times(mux: &Mux, limits: &Limits) -> (Option<i64>, Option<i64>) {
    let ts = probe_with(&mux.out, limits).unwrap();
    (ts.streams[0].last_pts, ts.streams[1].last_pts)
}

#[test]
fn the_last_timestamps_are_read_from_no_more_than_max_tail_bytes() {
    let mux = tail_mux(10);
    let tail = |packets: u64| Limits {
        max_tail_bytes: packets * TS_PACKET,
        ..Limits::default()
    };
    assert_eq!(last_times(&mux, &tail(1)), (None, Some(270_000)));
    assert_eq!(last_times(&mux, &tail(11)), (None, Some(270_000)));
    assert_eq!(last_times(&mux, &tail(12)), (Some(180_000), Some(270_000)));
}

#[test]
fn a_stream_the_tail_shows_no_end_of_is_looked_for_in_longer_tails_tail_retries_times() {
    let tail = |retries| Limits {
        max_tail_bytes: 4 * TS_PACKET,
        tail_retries: retries,
        ..Limits::default()
    };
    let near = tail_mux(5);
    assert_eq!(last_times(&near, &tail(0)), (None, Some(270_000)));
    assert_eq!(last_times(&near, &tail(1)), (Some(180_000), Some(270_000)));

    for between in [10, 12] {
        let far = tail_mux(between);
        assert_eq!(last_times(&far, &tail(1)), (None, Some(270_000)));
        assert_eq!(last_times(&far, &tail(2)), (Some(180_000), Some(270_000)));
    }
}

#[test]
fn pmts_before_the_pat_past_max_programs_are_not_read() {
    let mut mux = Mux::default();
    for decoy in 91..=94u16 {
        mux.pmt(0x100, decoy, &[(0x0F, 0x2000 + decoy)]);
    }
    mux.pmt(0x100, 2, &[(0x0F, 0x1002)]);
    mux.pat(&[(1, 0x100), (2, 0x100)]);
    mux.pmt(0x100, 1, &[(0x0F, 0x1001)]);
    mux.pmt(0x100, 2, &[(0x0F, 0x1002)]);
    let found_at = |max_programs| {
        let limits = Limits {
            max_programs,
            ..Limits::default()
        };
        let ts = probe_with(&mux.out, &limits).unwrap();
        ts.streams
            .iter()
            .find(|s| s.pid == 0x1002)
            .unwrap()
            .found_at
    };
    assert!(found_at(5) < found_at(4));
}

#[test]
fn audio_carried_across_pes_packets_counts_against_max_buffered_bytes() {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    let streams: Vec<_> = (0..4).map(|i| (0x0F, 0x101 + i)).collect();
    mux.pmt(0x1000, 1, &streams);
    let frame = adts(150);
    for (_, pid) in &streams {
        mux.pes(*pid, 0xC0, 90_000, &frame[..100], true);
    }
    let ts = probe_with(&mux.out, &Limits::default()).unwrap();
    let carried = Limits {
        max_buffered_bytes: 4 * 100,
        ..Limits::default()
    };
    assert_eq!(listing_error(&ts, &mux.out, &carried), None);

    let past = Limits {
        max_buffered_bytes: 4 * 100 - 1,
        ..Limits::default()
    };
    let listed = listing_error(&ts, &mux.out, &past);
    assert_limit(Err::<(), _>(listed.unwrap()), LimitKind::BufferedBytes);
}
