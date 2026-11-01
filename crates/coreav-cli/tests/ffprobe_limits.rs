//! `ffprobe::probe_limited`: the limits it is given bound the probe and
//! the packet listing of each container, on small hand-built files.

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;
#[path = "../../coreav-mkv/tests/common/mod.rs"]
mod mkv;
#[path = "../../coreav-mpegts/tests/common/mod.rs"]
mod ts;

use coreav_cli::error::Failure;
use coreav_cli::ffprobe::{self, Probed};
use coreav_isobmff::{Limits, SliceSource};

type Fixture = &'static [u8];

fn fixture(bytes: Vec<u8>) -> Fixture {
    Box::leak(bytes.into_boxed_slice())
}

fn probe_limited(bytes: Fixture, limits: &Limits) -> Result<Probed<'static>, Failure> {
    let source = Box::leak(Box::new(SliceSource::new(bytes)));
    ffprobe::probe_limited(source, bytes.len() as u64, "limits", None, limits)
}

fn listed(probed: &mut Probed<'_>) -> Result<usize, Failure> {
    let payloads = vec![false; probed.report.streams.len()];
    let mut count = 0;
    probed.packets.visit(&payloads, &mut |_| {
        count += 1;
        Ok(true)
    })?;
    Ok(count)
}

fn movie() -> Fixture {
    fixture(common::av_movie_with(common::aac_entry(), false))
}

fn webm() -> Fixture {
    fixture(mkv::small_webm())
}

fn transport_stream() -> Fixture {
    let mut mux = ts::Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101), (0x0F, 0x102)]);
    for n in 0..12u64 {
        let pid = 0x101 + (n % 2) as u16;
        mux.pes(pid, 0xC0, 90_000 + n * 1920, &ts::adts(100), true);
    }
    fixture(mux.out)
}

#[test]
fn the_defaults_list_every_packet_of_each_container() {
    let limits = Limits::default();
    let count = |bytes| listed(&mut probe_limited(bytes, &limits).ok().unwrap()).ok();
    assert_eq!(count(movie()), Some(8));
    assert_eq!(count(webm()), Some(4));
    assert_eq!(count(transport_stream()), Some(12));
}

#[test]
fn iso_base_media_probes_and_lists_within_the_limits() {
    let tracks = Limits {
        max_tracks: 1,
        ..Limits::default()
    };
    assert!(matches!(
        probe_limited(movie(), &tracks),
        Err(Failure::Library(coreav_isobmff::Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::Tracks,
            ..
        }))
    ));
    let packets = Limits {
        max_packet_bytes: 50,
        ..Limits::default()
    };
    let mut probed = probe_limited(movie(), &packets).ok().unwrap();
    assert!(matches!(
        listed(&mut probed),
        Err(Failure::Library(coreav_isobmff::Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::PacketBytes,
            ..
        }))
    ));
}

#[test]
fn matroska_probes_and_lists_within_the_limits() {
    let tracks = Limits {
        max_tracks: 1,
        ..Limits::default()
    };
    assert!(matches!(
        probe_limited(webm(), &tracks),
        Err(Failure::Matroska(coreav_mkv::Error::LimitExceeded {
            limit: coreav_mkv::LimitKind::Tracks,
            ..
        }))
    ));
    let packets = Limits {
        max_packet_bytes: 5,
        ..Limits::default()
    };
    let mut probed = probe_limited(webm(), &packets).ok().unwrap();
    assert!(listed(&mut probed).ok().unwrap() < 4);
}

#[test]
fn transport_streams_probe_and_list_within_the_limits() {
    let tracks = Limits {
        max_tracks: 1,
        ..Limits::default()
    };
    assert!(matches!(
        probe_limited(transport_stream(), &tracks),
        Err(Failure::Mpegts(coreav_mpegts::Error::LimitExceeded {
            limit: coreav_mpegts::LimitKind::Streams,
            ..
        }))
    ));
    let work = Limits {
        max_sample_work: 6,
        ..Limits::default()
    };
    let mut probed = probe_limited(transport_stream(), &work).ok().unwrap();
    assert!(listed(&mut probed).ok().unwrap() < 12);
}
