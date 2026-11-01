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
#[path = "../../coreav-isobmff/tests/common/text_movie.rs"]
mod text_movie;
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

fn chapter_movie(sample: Vec<u8>) -> Fixture {
    let chapters = text_movie::TrackSpec::qt_text(2, vec![(sample, 1000)]);
    let video = text_movie::TrackSpec {
        chap: vec![chapters.id],
        ..text_movie::TrackSpec::video(1, &[1000, 1000])
    };
    fixture(text_movie::movie(&[video, chapters]))
}

#[test]
fn a_limit_the_chapter_track_exceeds_fails_the_probe() {
    let bytes = chapter_movie(text_movie::text_sample(&[b'a'; 200], &[]));
    assert_eq!(
        probe_limited(bytes, &Limits::default())
            .ok()
            .unwrap()
            .report
            .chapters
            .len(),
        1
    );
    let limits = Limits {
        max_packet_bytes: 100,
        ..Limits::default()
    };
    match probe_limited(bytes, &limits) {
        Err(Failure::Library(coreav_isobmff::Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::PacketBytes,
            ..
        })) => {}
        Err(error) => panic!("unexpected failure: {}", error.to_json()),
        Ok(probed) => panic!(
            "expected a chapter limit error, got {} chapters",
            probed.report.chapters.len()
        ),
    }
}

#[test]
fn a_chapter_track_over_the_text_limits_is_still_left_out() {
    let text_limits = coreav_text::TextLimits::default();
    let sample = text_movie::text_sample(
        b"chapter",
        &[text_movie::bx(
            b"free",
            &vec![0; text_limits.max_sample_bytes as usize],
        )],
    );
    let bytes = chapter_movie(sample);
    assert!(matches!(
        coreav_text::read_chapters(SliceSource::new(bytes), &Limits::default(), &text_limits),
        Err(coreav_text::TextError::LimitExceeded {
            limit: coreav_text::TextLimitKind::SampleBytes,
            ..
        })
    ));
    assert!(
        probe_limited(bytes, &Limits::default())
            .ok()
            .unwrap()
            .report
            .chapters
            .is_empty()
    );
}

fn late_negative_offsets(sample_100: u32) -> Vec<u8> {
    let mut sizes = vec![10; 128];
    sizes[100] = sample_100;
    common::movie(&common::Spec {
        sizes,
        composition: Vec::new(),
        ctts: Some(vec![(112, 0), (16, -1024)]),
        sync: Vec::new(),
        elst: Some(vec![(80, 0)]),
        ..common::Spec::default()
    })
}

fn decode_times(probed: &mut Probed<'_>) -> Result<Vec<Option<i64>>, Failure> {
    let payloads = vec![false; probed.report.streams.len()];
    let mut times = Vec::new();
    probed.packets.visit(&payloads, &mut |packet| {
        times.push(packet.dts);
        Ok(true)
    })?;
    Ok(times)
}

#[test]
fn a_limit_the_decode_time_scan_exceeds_fails_the_listing() {
    let bytes = fixture(late_negative_offsets(100));
    let mut probed = probe_limited(bytes, &Limits::default()).ok().unwrap();
    assert_eq!(
        decode_times(&mut probed).ok().unwrap(),
        [Some(-1024), Some(-512), Some(0)]
    );
    let limits = Limits {
        max_packet_bytes: 50,
        ..Limits::default()
    };
    let mut probed = probe_limited(bytes, &limits).ok().unwrap();
    assert_eq!(probed.report.streams.len(), 1);
    let mut times = Vec::new();
    let result = probed.packets.visit(&[false], &mut |packet| {
        times.push(packet.dts);
        Ok(true)
    });
    match result {
        Err(Failure::Library(coreav_isobmff::Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::PacketBytes,
            ..
        })) => {}
        Ok(()) => panic!("listed {times:?} with only a partial decode-time shift"),
        Err(error) => panic!("unexpected failure: {}", error.to_json()),
    }
    assert!(times.is_empty());
}

#[test]
fn a_sample_past_the_end_still_ends_the_decode_time_scan_quietly() {
    let mut bytes = late_negative_offsets(10);
    let at = bytes.windows(4).position(|w| w == b"stsz").unwrap() + 16 + 4 * 100;
    bytes[at..at + 4].copy_from_slice(&1_000_000u32.to_be_bytes());
    let mut probed = probe_limited(fixture(bytes), &Limits::default())
        .ok()
        .unwrap();
    assert_eq!(decode_times(&mut probed).ok().map(|t| t.len()), Some(3));
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
    assert!(matches!(
        probe_limited(movie(), &packets),
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
