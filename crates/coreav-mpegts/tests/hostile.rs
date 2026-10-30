//! Hand-built transport streams: limits, damaged structures and the
//! listing's rules (docs/design/mpegts.md).
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod common;

use common::{Mux, adts, crc32};
use coreav_mpegts::{Allowlist, Codec, Error, LimitKind, Limits, SliceSource, Warning, probe};

fn audio_stream() -> Mux {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    mux
}

#[test]
fn audio_frames_take_the_pes_time_and_position() {
    let mut mux = audio_stream();
    // Three frames in the first PES packet, the third cut and finished in
    // the second, which then starts a frame of its own.
    let mut first = [adts(100), adts(120)].concat();
    let cut = adts(150);
    first.extend(&cut[..60]);
    mux.pes(0x101, 0xC0, 90_000, &first, true);
    let second_offset = mux.out.len() as u64;
    let second = [cut[60..].to_vec(), adts(110)].concat();
    mux.pes(0x101, 0xC0, 95_760, &second, true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(ts.streams.len(), 1);
    let audio = ts.streams[0].audio.unwrap();
    assert_eq!((audio.sample_rate, audio.channels), (48_000, 2));
    assert_eq!(ts.streams[0].config.as_deref(), Some(&[0x11, 0x90][..]));
    let packets: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(Result::unwrap)
        .collect();
    let summary: Vec<_> = packets
        .iter()
        .map(|p| (p.data.len(), p.pts, p.pos.is_some()))
        .collect();
    assert_eq!(
        summary,
        vec![
            (100, Some(90_000), true),
            (120, Some(91_920), false),
            // Started in the first PES packet: its time follows the frames
            // before it, and it has no position.
            (150, Some(93_840), false),
            (110, Some(95_760), true),
        ]
    );
    assert_eq!(packets[3].pos, Some(second_offset));
    // The stream ends after its last PES packet's first frame.
    assert_eq!(ts.streams[0].duration(), Some(95_760 + 1920 - 90_000));
}

#[test]
fn timestamps_near_the_wrap_are_placed_on_one_timeline() {
    let mut mux = audio_stream();
    let near = (1u64 << 33) - 9_000;
    mux.pes(0x101, 0xC0, near, &adts(100), true);
    mux.pes(0x101, 0xC0, 500, &adts(100), true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let pts: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap().pts)
        .collect();
    assert_eq!(pts, vec![Some(-9_000), Some(500)]);
}

#[test]
fn a_file_without_tables_lists_its_pes_pids() {
    // No Program Association Table: the PIDs that carry PES packets are
    // the streams, in the order of their first PES packets, each named by
    // its payload, in no program.
    let mut mux = Mux::default();
    mux.pes(0x102, 0xC0, 0, &adts(100), true);
    mux.pes(0x101, 0xC0, 0, &adts(120), true);
    mux.pes(0x102, 0xC0, 1_920, &adts(100), true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let streams: Vec<_> = ts.streams.iter().map(|s| (s.pid, s.codec)).collect();
    assert_eq!(streams, vec![(0x102, Codec::Aac), (0x101, Codec::Aac)]);
    assert!(ts.programs.is_empty());
    assert_eq!(ts.transport_stream_id, None);
    // A table without its Program Map Table is an error still, and so is
    // a file with neither tables nor PES packets.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pes(0x101, 0xC0, 0, &adts(100), true);
    let got = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    );
    assert_eq!(got, Err(Error::NoPrograms));
    // Three null packets.
    assert_eq!(
        probe(
            SliceSource::new(&[0x47u8, 0x1F, 0xFF, 0x10].repeat(47 * 3)),
            &Limits::default(),
            &Allowlist::default()
        ),
        Err(Error::NoPrograms)
    );
    assert_eq!(
        probe(
            SliceSource::new(&[0u8; 2000]),
            &Limits::default(),
            &Allowlist::default()
        ),
        Err(Error::NotTransportStream)
    );
}

#[test]
fn limits_bound_programs_streams_and_packets() {
    let mut mux = Mux::default();
    let programs: Vec<(u16, u16)> = (1..=200).map(|n| (n, 0x100 + n)).collect();
    mux.pat(&programs);
    let limits = Limits {
        max_programs: 10,
        ..Limits::default()
    };
    let got = probe(SliceSource::new(&mux.out), &limits, &Allowlist::default());
    assert!(matches!(
        got,
        Err(Error::LimitExceeded {
            limit: LimitKind::Programs,
            ..
        })
    ));
    // A PES packet of unbounded length that never ends.
    let mut mux = audio_stream();
    let mut pes = vec![0, 0, 1, 0xC0, 0, 0, 0x80, 0x80, 5, 0x21, 0, 1, 0, 1];
    pes.extend(adts(100));
    mux.unit(0x101, &pes);
    for _ in 0..200 {
        mux.packet(0x101, false, &[0x55; 184]);
    }
    let limits = Limits {
        max_packet_bytes: 16 << 10,
        ..Limits::default()
    };
    let got = probe(SliceSource::new(&mux.out), &limits, &Allowlist::default());
    assert!(matches!(
        got,
        Err(Error::LimitExceeded {
            limit: LimitKind::PacketBytes,
            ..
        })
    ));
}

#[test]
fn damaged_tables_are_skipped_with_warnings() {
    let mut mux = Mux::default();
    // A PAT whose CRC is wrong, then a good one.
    mux.pat(&[(1, 0x1000)]);
    // The section ends the packet: its last byte is the CRC's.
    let last = mux.out.len() - 1;
    mux.out[last] ^= 0xFF;
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    mux.pes(0x101, 0xC0, 0, &adts(100), true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(ts.streams.len(), 1);
    assert!(
        ts.warnings
            .iter()
            .any(|w| matches!(w, Warning::BadSection { pid: 0, .. }))
    );
}

#[test]
fn unknown_private_streams_are_sniffed_and_dropped_streams_listed() {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt_with(
        0x1000,
        1,
        &Mux::registration(b"CUEI"),
        &[(0x06, 0x101), (0x86, 0x102)],
    );
    mux.pes(0x101, 0xC0, 0, &[adts(100), adts(100)].concat(), true);
    // The default allowlist keeps every named codec, SCTE-35 included; a
    // caller that names fewer gets the rest as dropped streams, with their
    // place among the tables' streams.
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(ts.streams.len(), 2);
    assert_eq!(ts.streams[0].codec, Codec::Aac);
    assert_eq!(ts.streams[1].codec, Codec::Scte35);
    assert!(ts.dropped.is_empty());

    let audio_only = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::new(&[Codec::Aac]),
    )
    .unwrap();
    assert_eq!(audio_only.streams.len(), 1);
    assert_eq!(audio_only.streams[0].codec, Codec::Aac);
    assert_eq!(audio_only.streams[0].stream_index, 0);
    assert_eq!(audio_only.dropped.len(), 1);
    assert_eq!(audio_only.dropped[0].codec, Codec::Scte35);
    assert_eq!(audio_only.dropped[0].stream_index, 1);

    // Strict mode rejects the file instead of dropping the stream.
    assert_eq!(
        probe(
            SliceSource::new(&mux.out),
            &Limits::default(),
            &Allowlist::new(&[Codec::Aac]).strict(),
        ),
        Err(Error::CodecNotAllowed {
            pid: 0x102,
            codec: Codec::Scte35,
        })
    );
}

/// A stream whose type and descriptors name no codec this crate knows is
/// listed all the same, with `Codec::Other`: the tables declare it, and
/// `--allow` can neither name it nor drop it.
#[test]
fn a_stream_of_an_unknown_type_is_listed() {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    // 0x0D is DSM-CC sections and 0xA1 is nothing this crate names.
    mux.pmt(0x1000, 1, &[(0x0F, 0x101), (0x0D, 0x102), (0xA1, 0x103)]);
    mux.pes(0x101, 0xC0, 0, &adts(100), true);
    mux.pes(0x103, 0xBD, 0, &[0x11; 200], true);
    for allowlist in [
        Allowlist::default(),
        Allowlist::new(&[Codec::Aac]),
        Allowlist::new(&[Codec::Aac]).strict(),
    ] {
        let ts = probe(SliceSource::new(&mux.out), &Limits::default(), &allowlist).unwrap();
        assert_eq!(ts.streams.len(), 3, "{allowlist:?}");
        assert_eq!(ts.streams[1].codec, Codec::Other);
        assert_eq!(ts.streams[1].stream_type, 0x0D);
        assert_eq!(ts.streams[2].codec, Codec::Other);
        assert_eq!(ts.streams[2].stream_type, 0xA1);
        assert!(ts.dropped.is_empty(), "{allowlist:?}");
    }
}

#[test]
fn lost_sync_and_garbage_are_survived() {
    let mut mux = audio_stream();
    mux.pes(0x101, 0xC0, 0, &adts(100), true);
    let mut data = mux.out.clone();
    data.extend(std::iter::repeat_n(0x11, 77));
    let mut tail = Mux {
        counters: mux.counters.clone(),
        ..Mux::default()
    };
    tail.pes(0x101, 0xC0, 1920, &adts(100), true);
    data.extend(&tail.out);
    let ts = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let mut packets = ts.packets(SliceSource::new(&data), &Limits::default());
    let listed: Vec<_> = packets.by_ref().map(|p| p.unwrap().pts).collect();
    assert_eq!(listed, vec![Some(0), Some(1920)]);
    assert!(
        packets
            .warnings()
            .iter()
            .any(|w| matches!(w, Warning::LostSync { skipped: 77, .. }))
    );
    // A file that never resynchronises within the bound is an error.
    let mut data = mux.out.clone();
    data.extend(std::iter::repeat_n(0x11, 50_000));
    data.extend(&tail.out);
    let limits = Limits {
        max_resync_bytes: 1024,
        ..Limits::default()
    };
    // The probe has what it needs before the junk, and a tail it cannot
    // read only leaves durations unknown; the listing, which must read
    // through it, stops with the error.
    let ts = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    let last = ts.packets(SliceSource::new(&data), &limits).last().unwrap();
    assert!(
        matches!(
            last,
            Err(Error::LimitExceeded {
                limit: LimitKind::ResyncBytes,
                ..
            })
        ),
        "{last:?}"
    );
}

#[test]
fn duplicates_repeat_bytes_and_other_bytes_are_a_loss() {
    let mut mux = audio_stream();
    let mut pes = vec![0, 0, 1, 0xC0, 0, 0, 0x80, 0x80, 5, 0x21, 0, 1, 0, 1];
    pes.extend(adts(400));
    mux.unit(0x101, &pes);
    // Repeat the second packet exactly: a duplicate.
    let second = mux.out.len() - 188 * 2;
    let repeat = mux.out[second..second + 188].to_vec();
    let mut duplicated = mux.out.clone();
    duplicated.splice(second + 188..second + 188, repeat.clone());
    let ts = probe(
        SliceSource::new(&duplicated),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let mut listing = ts.packets(SliceSource::new(&duplicated), &Limits::default());
    let sizes: Vec<_> = listing.by_ref().map(|p| p.unwrap().data.len()).collect();
    assert_eq!(sizes, vec![400]);
    assert!(
        listing
            .warnings()
            .iter()
            .any(|w| matches!(w, Warning::Duplicate { .. }))
    );
    // The same counter on other bytes: kept, and reported as a loss.
    let mut changed = repeat;
    changed[100] ^= 0xFF;
    let mut lost = mux.out.clone();
    lost.splice(second + 188..second + 188, changed);
    let ts = probe(
        SliceSource::new(&lost),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let mut listing = ts.packets(SliceSource::new(&lost), &Limits::default());
    let _: Vec<_> = listing.by_ref().collect();
    assert!(
        !listing
            .warnings()
            .iter()
            .any(|w| matches!(w, Warning::Duplicate { .. }))
    );
    assert!(
        listing
            .warnings()
            .iter()
            .any(|w| matches!(w, Warning::Discontinuity { .. }))
    );
}

#[test]
fn streams_a_later_pmt_adds_are_listed() {
    let mut mux = audio_stream();
    for k in 0..3 {
        mux.pes(0x101, 0xC0, 90_000 + 1920 * k, &adts(100), true);
    }
    // Version 1 of the PMT adds a second AAC stream.
    let mut body = vec![0xE1, 0x00, 0xF0, 0x00];
    for pid in [0x101u16, 0x102] {
        body.extend([0x0F, 0xE0 | (pid >> 8) as u8, pid as u8, 0xF0, 0x00]);
    }
    let length = 5 + body.len() + 4;
    let mut section = vec![
        2,
        0xB0 | (length >> 8) as u8,
        length as u8,
        0,
        1,
        0xC3,
        0,
        0,
    ];
    section.extend(&body);
    let crc = crc32(&section);
    section.extend(crc.to_be_bytes());
    let mut payload = vec![0u8];
    payload.extend(section);
    mux.unit(0x1000, &payload);
    mux.pes(0x102, 0xC1, 96_000, &adts(120), true);
    mux.pes(0x101, 0xC0, 95_760, &adts(100), true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(
        ts.streams.iter().map(|s| s.pid).collect::<Vec<_>>(),
        vec![0x101, 0x102]
    );
    assert!(ts.warnings.contains(&Warning::PmtChanged { program: 1 }));
    let mut listing = ts.packets(SliceSource::new(&mux.out), &Limits::default());
    let streams: Vec<_> = listing.by_ref().map(|p| p.unwrap().stream).collect();
    assert_eq!(streams, vec![0, 0, 0, 1, 0]);
    assert_eq!(listing.streams().len(), 2);
}

#[test]
fn a_short_tail_read_is_harmless() {
    // 192-byte packets, and tails shorter than one packet.
    let mut mux = audio_stream();
    mux.pes(0x101, 0xC0, 90_000, &adts(100), true);
    let mut m2ts = Vec::new();
    for packet in mux.out.chunks(188) {
        m2ts.extend([0, 0, 0, 0]);
        m2ts.extend(packet);
    }
    for tail in 0..8 {
        let limits = Limits {
            max_tail_bytes: tail,
            ..Limits::default()
        };
        let ts = probe(SliceSource::new(&m2ts), &limits, &Allowlist::default()).unwrap();
        assert_eq!(ts.streams.len(), 1);
    }
}

/// A source whose `fail_at`-th read fails with an I/O error.
struct FailingAt<'a> {
    data: SliceSource<'a>,
    reads: usize,
    fail_at: usize,
}

impl coreav_mpegts::Source for FailingAt<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        self.data.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        self.reads += 1;
        if self.reads == self.fail_at {
            return Err(coreav_isobmff::Error::Io(
                std::io::ErrorKind::ConnectionReset,
            ));
        }
        self.data.read_exact_at(offset, buf)
    }
}

/// A read that fails is not a tail the reader cannot use: wherever
/// the failure falls, including the tail read for the durations, the probe
/// fails with it.
#[test]
fn a_read_failure_fails_the_probe_wherever_it_falls() {
    let mut mux = audio_stream();
    for (i, pts) in [90_000, 95_760, 101_520].into_iter().enumerate() {
        mux.pes(0x101, 0xC0, pts, &adts(100 + i), true);
    }
    // A tail shorter than the file, read in small windows, so that the tail
    // is read apart from the head.
    let limits = Limits {
        max_tail_bytes: 188 * 2,
        ..Limits::default()
    };
    let mut clean = FailingAt {
        data: SliceSource::new(&mux.out),
        reads: 0,
        fail_at: 0,
    };
    let ts = probe(&mut clean, &limits, &Allowlist::default()).unwrap();
    assert!(ts.streams[0].duration().is_some());
    for fail_at in 1..=clean.reads {
        let mut source = FailingAt {
            data: SliceSource::new(&mux.out),
            reads: 0,
            fail_at,
        };
        let result = probe(&mut source, &limits, &Allowlist::default());
        assert!(
            matches!(result, Err(Error::Io(std::io::ErrorKind::ConnectionReset))),
            "read {fail_at} of {}: {result:?}",
            clean.reads
        );
    }
}

#[test]
fn buffered_bytes_across_pids_are_bounded() {
    // Many PIDs each starting a PES packet of unbounded length.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    let streams: Vec<(u8, u16)> = (0..40).map(|i| (0x0F, 0x200 + i)).collect();
    mux.pmt(0x1000, 1, &streams);
    for round in 0..30 {
        for (_, pid) in &streams {
            if round == 0 {
                mux.packet(*pid, true, &[0, 0, 1, 0xC0, 0, 0, 0x80, 0, 0]);
            } else {
                mux.packet(*pid, false, &[0x55; 184]);
            }
        }
    }
    let limits = Limits {
        max_buffered_bytes: 64 << 10,
        ..Limits::default()
    };
    let got = probe(SliceSource::new(&mux.out), &limits, &Allowlist::default());
    assert!(
        matches!(
            got,
            Err(Error::LimitExceeded {
                limit: LimitKind::BufferedBytes,
                ..
            })
        ),
        "{got:?}"
    );
}

#[test]
fn a_stream_starts_at_its_first_pes_packet_before_the_tables() {
    // The elementary stream's packets come before the Program Map Table
    // that names its PID; they are the stream's all the same.
    let mut mux = Mux::default();
    mux.pes(0x101, 0xC0, 90_000, &adts(100), true);
    mux.pes(0x101, 0xC0, 91_920, &adts(100), true);
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    mux.pes(0x101, 0xC0, 93_840, &adts(100), true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(ts.streams.len(), 1);
    assert_eq!(ts.streams[0].start, Some(90_000));
}

#[test]
fn the_event_information_table_is_a_data_stream_where_it_is_read() {
    // An EIT section before the PAT, as a broadcast capture carries one:
    // `ffprobe` lists it first, so this reader does too.
    let mut mux = Mux::default();
    mux.section(0x12, 0x4E, 1, &[0u8; 8]);
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    mux.pes(0x101, 0xC0, 90_000, &adts(100), true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let listed: Vec<_> = ts
        .streams
        .iter()
        .map(|s| (s.pid, s.codec, s.pmt_index))
        .collect();
    assert_eq!(
        listed,
        vec![(0x12, Codec::Epg, None), (0x101, Codec::Aac, Some(0))]
    );
    // A section whose CRC-32 does not match makes no stream.
    let mut broken = Mux::default();
    broken.section(0x12, 0x4E, 1, &[0u8; 8]);
    let last = broken.out.len() - 1;
    broken.out[last] ^= 0xFF;
    broken.pat(&[(1, 0x1000)]);
    broken.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    broken.pes(0x101, 0xC0, 90_000, &adts(100), true);
    let ts = probe(
        SliceSource::new(&broken.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(
        ts.streams.iter().map(|s| s.pid).collect::<Vec<_>>(),
        vec![0x101]
    );
}

/// An H.264 access unit as a real encoder writes one: a delimiter, an SEI
/// message and a slice of `slice` bytes whose `first_mb_in_slice` is 0.
fn h264_access_unit(slice: usize) -> Vec<u8> {
    let mut au = vec![0, 0, 0, 1, 0x09, 0x10];
    au.extend([0, 0, 1, 0x06, 0x01, 0x04, 0x80]);
    au.extend([0, 0, 1, 0x21, 0x88]);
    au.resize(au.len() + slice, 0x42);
    au
}

#[test]
fn an_access_unit_a_pes_packet_boundary_cuts_keeps_its_last_bytes() {
    // Real transport streams start a PES packet a few bytes before the
    // delimiter of the access unit it carries, so a unit's last bytes are
    // at the head of the next PES packet (the standard says otherwise).
    let first = h264_access_unit(400);
    let second = h264_access_unit(500);
    let third = h264_access_unit(600);
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    let first_offset = mux.out.len() as u64;
    // The first PES packet stops 13 bytes into the second unit's tail.
    let split = first.len() - 13;
    mux.pes(0x101, 0xE0, 90_000, &first[..split], false);
    let second_offset = mux.out.len() as u64;
    mux.pes(
        0x101,
        0xE0,
        93_600,
        &[&first[split..], &second[..]].concat(),
        false,
    );
    let third_offset = mux.out.len() as u64;
    mux.pes(0x101, 0xE0, 97_200, &third, false);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let packets: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap())
        .map(|p| (p.data.len(), p.pts, p.pos))
        .collect();
    // Each unit is whole, and takes the time and position of the PES
    // packet it starts in, not of the one it ends in.
    assert_eq!(
        packets,
        vec![
            (first.len(), Some(90_000), Some(first_offset)),
            (second.len(), Some(93_600), Some(second_offset)),
            (third.len(), Some(97_200), Some(third_offset)),
        ]
    );
}

#[test]
fn the_access_units_of_one_pes_packet_divide_the_picture_interval() {
    // An interlaced stream delivers both fields of a frame in one PES
    // packet; the second takes no time of the file's own.
    let field = h264_access_unit(200);
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    for pts in [90_000u64, 93_600, 97_200] {
        mux.pes(0x101, 0xE0, pts, &[&field[..], &field[..]].concat(), false);
    }
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let times: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap())
        .map(|p| (p.data.len(), p.pts))
        .collect();
    assert_eq!(
        times,
        vec![
            (field.len(), Some(90_000)),
            (field.len(), Some(91_800)),
            (field.len(), Some(93_600)),
            (field.len(), Some(95_400)),
            (field.len(), Some(97_200)),
            (field.len(), Some(99_000)),
        ]
    );
}

#[test]
fn an_access_unit_is_found_wherever_the_pes_packet_cuts_it() {
    // The bytes that say what a NAL unit is can be past the end of the
    // PES packet that starts it: a payload may end with a start code, or
    // with a start code and one byte of the unit after it.
    let unit = h264_access_unit(60);
    for cut in 0..10usize {
        let stream = [&unit[..], &unit[..], &unit[..]].concat();
        let split = unit.len() * 2 - cut;
        let mut mux = Mux::default();
        mux.pat(&[(1, 0x1000)]);
        mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
        mux.pes(0x101, 0xE0, 90_000, &stream[..split], false);
        mux.pes(0x101, 0xE0, 93_600, &stream[split..], false);
        let ts = probe(
            SliceSource::new(&mux.out),
            &Limits::default(),
            &Allowlist::all(),
        )
        .unwrap();
        let sizes: Vec<_> = ts
            .packets(SliceSource::new(&mux.out), &Limits::default())
            .map(|p| p.unwrap().data.len())
            .collect();
        assert_eq!(sizes, vec![unit.len(); 3], "cut {cut} bytes before the end");
    }
}

#[test]
fn the_units_held_for_an_interval_keep_their_place() {
    // A stream that states a time twice and no more: the units of the
    // first packet wait for the picture interval, and the unit of the
    // last one must not overtake them.
    let unit = h264_access_unit(80);
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    mux.pes(0x101, 0xE0, 90_000, &[&unit[..], &unit[..]].concat(), false);
    mux.pes(0x101, 0xE0, 96_006, &unit, false);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let times: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap().pts)
        .collect();
    assert_eq!(times, vec![Some(90_000), Some(93_003), Some(96_006)]);
}

#[test]
fn a_picture_whose_pes_packet_states_no_time_divides_the_interval() {
    // Every other PES packet of a real capture states no time; the
    // picture it carries falls between the two that do.
    let picture = h264_access_unit(100);
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    for pts in [90_000u64, 93_003, 96_006] {
        mux.pes(0x101, 0xE0, pts, &picture, false);
        mux.unit(
            0x101,
            &[&[0, 0, 1, 0xE0, 0, 0, 0x80, 0, 0][..], &picture].concat(),
        );
    }
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let times: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap().pts)
        .collect();
    assert_eq!(
        times,
        vec![
            Some(90_000),
            Some(91_502),
            Some(93_003),
            Some(94_505),
            Some(96_006),
            Some(97_508),
        ]
    );
}

#[test]
fn eac3_dependent_substreams_join_their_access_unit_across_pes_packets() {
    // 256-byte frames, 48 kHz, 6 blocks: an independent one (strmtyp 0)
    // and a dependent one (strmtyp 1).
    let frame = |strmtyp: u8| {
        let mut f = vec![0x0B, 0x77, strmtyp << 6, 0x7F, 0x3F, 0x80];
        f.resize(256, 0);
        f
    };
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x87, 0x101)]);
    mux.pes(0x101, 0xC0, 90_000, &frame(0), true);
    mux.pes(0x101, 0xC0, 92_880, &[frame(1), frame(0)].concat(), true);
    mux.pes(0x101, 0xC0, 95_760, &frame(1), true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let packets: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap())
        .map(|p| (p.data.len(), p.pts))
        .collect();
    assert_eq!(packets, vec![(512, Some(90_000)), (512, Some(92_880))]);
}

#[test]
fn an_eac3_access_unit_takes_at_most_eight_dependent_substreams() {
    let frame = |strmtyp: u8| {
        let mut f = vec![0x0B, 0x77, strmtyp << 6, 0x7F, 0x3F, 0x80];
        f.resize(256, 0);
        f
    };
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x87, 0x101)]);
    let mut payload = frame(0);
    for _ in 0..20 {
        payload.extend(frame(1));
    }
    mux.pes(0x101, 0xC0, 90_000, &payload, true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let sizes: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap().data.len())
        .collect();
    // A unit takes eight; a ninth starts a unit of its own, so memory is
    // bounded by nine frames whatever the stream holds.
    assert_eq!(sizes, vec![9 * 256, 9 * 256, 3 * 256]);
}

#[test]
fn a_frame_cut_by_its_pes_packet_keeps_that_packets_time() {
    let mut mux = audio_stream();
    let cut = adts(300);
    // The first PES packet: a whole frame, then the start of the next.
    mux.pes(
        0x101,
        0xC0,
        90_000,
        &[adts(100), cut[..50].to_vec()].concat(),
        true,
    );
    // The second starts with the rest of that frame, and its PTS (for the
    // frame that starts in it) is off the frames' own timeline.
    let second = mux.out.len() as u64;
    mux.pes(
        0x101,
        0xC0,
        99_000,
        &[cut[50..].to_vec(), adts(120)].concat(),
        true,
    );
    let _ = second;
    // A third, whose first frame is cut by its end and finished in a fourth.
    let third = mux.out.len() as u64;
    let long = adts(400);
    mux.pes(0x101, 0xC0, 120_000, &long[..100], true);
    mux.pes(
        0x101,
        0xC0,
        130_000,
        &[long[100..].to_vec(), adts(110)].concat(),
        true,
    );
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let packets: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap())
        .map(|p| (p.data.len(), p.pts, p.pos))
        .collect();
    assert_eq!(packets[3], (400, Some(120_000), Some(third)));
    assert_eq!(packets[4].1, Some(130_000));
}

#[test]
fn a_frame_is_split_when_its_header_ends_before_its_pes_packet_does() {
    // Cut inside the 7-byte ADTS header, the frame is read again from its
    // start once whole; cut after it, its header was read in the first PES
    // packet and the frame is split.
    for (cut, split) in [(3, false), (6, false), (7, true), (50, true)] {
        let mut mux = audio_stream();
        let frame = adts(300);
        mux.pes(
            0x101,
            0xC0,
            90_000,
            &[adts(100), frame[..cut].to_vec()].concat(),
            true,
        );
        mux.pes(
            0x101,
            0xC0,
            99_000,
            &[frame[cut..].to_vec(), adts(120)].concat(),
            true,
        );
        let ts = probe(
            SliceSource::new(&mux.out),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap();
        let packets: Vec<_> = ts
            .packets(SliceSource::new(&mux.out), &Limits::default())
            .map(|p| p.unwrap())
            .map(|p| (p.data.len(), p.split))
            .collect();
        assert_eq!(
            packets,
            vec![(100, false), (300, split), (120, false)],
            "cut at {cut}"
        );
    }
}

/// Splits two frames of `stream_type` across two PES packets at every byte
/// of the second frame's first `head` bytes, and checks the listing finds
/// the three frames whole.
fn split_everywhere(stream_type: u8, descriptors: &[u8], frames: [Vec<u8>; 3], head: usize) {
    for cut in 1..head {
        let mut mux = Mux::default();
        mux.pat(&[(1, 0x1000)]);
        let mut body = vec![
            0xE1,
            0x00,
            0xF0,
            0x00,
            stream_type,
            0xE1,
            0x01,
            0xF0,
            descriptors.len() as u8,
        ];
        body.extend(descriptors);
        mux.section(0x1000, 2, 1, &body);
        let first = [frames[0].clone(), frames[1][..cut].to_vec()].concat();
        let second = [frames[1][cut..].to_vec(), frames[2].clone()].concat();
        mux.pes(0x101, 0xC0, 90_000, &first, true);
        mux.pes(0x101, 0xC0, 99_000, &second, true);
        let ts = probe(
            SliceSource::new(&mux.out),
            &Limits::default(),
            &Allowlist::all(),
        )
        .unwrap();
        let sizes: Vec<_> = ts
            .packets(SliceSource::new(&mux.out), &Limits::default())
            .map(|p| p.unwrap().data.len())
            .collect();
        let expected: Vec<_> = frames.iter().map(Vec::len).collect();
        assert_eq!(
            sizes.len(),
            3,
            "type {stream_type:#x}, cut {cut}: {sizes:?}"
        );
        if stream_type != 0x06 {
            assert_eq!(sizes, expected, "type {stream_type:#x}, cut {cut}");
        }
    }
}

#[test]
fn a_frame_header_split_across_pes_packets_is_kept() {
    // ADTS.
    split_everywhere(0x0F, &[], [adts(100), adts(110), adts(120)], 7);
    // MPEG-1 layer II, 192 kbit/s, 48 kHz: 576-byte frames.
    let mp2 = || {
        let mut f = vec![0xFF, 0xFD, 0xA4, 0xC0];
        f.resize(576, 0x11);
        f
    };
    split_everywhere(0x03, &[], [mp2(), mp2(), mp2()], 4);
    // AC-3, 48 kHz, frmsizecod 0: 128-byte frames.
    let ac3 = || {
        let mut f = vec![0x0B, 0x77, 0, 0, 0x00, 0x40, 0x40, 0x00];
        f.resize(128, 0);
        f
    };
    split_everywhere(0x81, &[], [ac3(), ac3(), ac3()], 8);
    // E-AC-3, 256-byte frames.
    let eac3 = || {
        let mut f = vec![0x0B, 0x77, 0x00, 0x7F, 0x3F, 0x80];
        f.resize(256, 0);
        f
    };
    split_everywhere(0x87, &[], [eac3(), eac3(), eac3()], 8);
    // LOAS with a StreamMuxConfig (AAC LC, 48 kHz, stereo).
    let loas = |len: usize| {
        let mut f = vec![
            0x56,
            0xE0 | ((len - 3) >> 8) as u8,
            (len - 3) as u8,
            0,
            0,
            0x11,
            0x90,
        ];
        f.resize(len, 0);
        f
    };
    split_everywhere(0x11, &[], [loas(60), loas(70), loas(80)], 3);
    // Opus access units with a control header (payload 300, a CELT 20 ms
    // packet); the listing's packets are the Opus packets.
    let opus = || {
        let mut f = vec![0x7F, 0xE0, 0xFF, 0x2D, 0xF8];
        f.resize(4 + 300, 0);
        f
    };
    split_everywhere(
        0x06,
        &[0x05, 0x04, b'O', b'p', b'u', b's'],
        [opus(), opus(), opus()],
        4,
    );
}

#[test]
fn a_program_remapped_in_a_later_pat_section_is_read_again() {
    let mut mux = Mux::default();
    // A PAT of two sections: program 1 in section 0, program 2 in 1.
    let pat = |mux: &mut Mux, version: u8, second_pmt: u16| {
        for (number, (program, pmt)) in [(1u16, 0x1000u16), (2, second_pmt)].into_iter().enumerate()
        {
            let body = [
                (program >> 8) as u8,
                program as u8,
                0xE0 | (pmt >> 8) as u8,
                pmt as u8,
            ];
            let length = 5 + body.len() + 4;
            let mut s = vec![
                0,
                0xB0,
                length as u8,
                0,
                1,
                0xC1 | (version << 1),
                number as u8,
                1,
            ];
            s.extend(body);
            let crc = crc32(&s);
            s.extend(crc.to_be_bytes());
            let mut payload = vec![0u8];
            payload.extend(s);
            mux.unit(0, &payload);
        }
    };
    pat(&mut mux, 0, 0x1001);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    mux.pmt(0x1001, 2, &[(0x0F, 0x102)]);
    mux.pes(0x101, 0xC0, 90_000, &adts(100), true);
    mux.pes(0x102, 0xC1, 90_000, &adts(100), true);
    // Version 1 moves program 2's PMT, which names a new stream.
    pat(&mut mux, 1, 0x1002);
    mux.pmt(0x1002, 2, &[(0x0F, 0x103)]);
    mux.pes(0x103, 0xC2, 92_000, &adts(100), true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let program = ts.programs.iter().find(|p| p.number == 2).unwrap();
    assert_eq!(program.pmt_pid, 0x1002);
    assert!(
        ts.streams.iter().any(|s| s.pid == 0x103),
        "{:?}",
        ts.streams
    );
}

#[test]
fn an_unbounded_resync_limit_does_not_overflow() {
    let mut mux = audio_stream();
    mux.pes(0x101, 0xC0, 90_000, &adts(100), true);
    let mut data = mux.out.clone();
    data.extend([0u8; 188]);
    let limits = Limits {
        max_resync_bytes: u64::MAX,
        ..Limits::default()
    };
    let ts = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    let listed: Vec<_> = ts.packets(SliceSource::new(&data), &limits).collect();
    assert_eq!(listed.len(), 1);
}

#[test]
fn as_ffprobe_the_second_unit_of_a_pes_packet_takes_no_time_of_the_files_own() {
    // An interlaced stream delivers both fields of a frame in one PES
    // packet; the second takes no time of the file's own (13818-1
    // 2.4.3.7 gives the PES packet's times to the first unit that starts
    // in it). Placing a second field is the caller's: it is a matter of
    // the picture's own syntax, not of the multiplex. The default listing
    // gives it its share of the picture interval instead
    // (`the_access_units_of_one_pes_packet_divide_the_picture_interval`).
    let field = h264_access_unit(200);
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    for pts in [90_000u64, 93_600, 97_200] {
        mux.pes(0x101, 0xE0, pts, &[&field[..], &field[..]].concat(), false);
    }
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let times: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .listing_as_ffprobe()
        .map(|p| p.unwrap())
        .map(|p| (p.data.len(), p.pts, p.pos.is_some()))
        .collect();
    assert_eq!(
        times,
        vec![
            (field.len(), Some(90_000), true),
            (field.len(), None, false),
            (field.len(), Some(93_600), true),
            (field.len(), None, false),
            (field.len(), Some(97_200), true),
            (field.len(), None, false),
        ]
    );
}

#[test]
fn as_ffprobe_the_units_of_a_pes_packet_keep_their_place() {
    // A stream that states a time twice and no more: the second unit of
    // the first packet has no time, and the unit of the last packet does
    // not overtake it (`the_units_held_for_an_interval_keep_their_place`
    // by default).
    let unit = h264_access_unit(80);
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    mux.pes(0x101, 0xE0, 90_000, &[&unit[..], &unit[..]].concat(), false);
    mux.pes(0x101, 0xE0, 96_006, &unit, false);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let times: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .listing_as_ffprobe()
        .map(|p| p.unwrap().pts)
        .collect();
    assert_eq!(times, vec![Some(90_000), None, Some(96_006)]);
}

#[test]
fn as_ffprobe_a_picture_whose_pes_packet_states_no_time_has_none() {
    // Every other PES packet of a real capture states no time; the
    // picture it carries keeps the position of its PES packet and takes
    // no time (`ffprobe` reports none for a frame there, and places only
    // a second field, which is the caller's to read). By default it
    // divides the interval
    // (`a_picture_whose_pes_packet_states_no_time_divides_the_interval`).
    let picture = h264_access_unit(100);
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    for pts in [90_000u64, 93_003, 96_006] {
        mux.pes(0x101, 0xE0, pts, &picture, false);
        mux.unit(
            0x101,
            &[&[0, 0, 1, 0xE0, 0, 0, 0x80, 0, 0][..], &picture].concat(),
        );
    }
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let times: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .listing_as_ffprobe()
        .map(|p| p.unwrap())
        .map(|p| (p.pts, p.pos.is_some()))
        .collect();
    assert_eq!(
        times,
        vec![
            (Some(90_000), true),
            (None, true),
            (Some(93_003), true),
            (None, true),
            (Some(96_006), true),
            (None, true),
        ]
    );
}

/// An ADTS stream at 44.1 kHz (a frame of 1,024 samples lasts 2,089.796
/// ticks) whose one PES packet, at 43,830, carries three frames.
fn aac_at_44100() -> Vec<u8> {
    let frame = || {
        let mut f = adts(200);
        // sampling_frequency_index 4 (ISO/IEC 14496-3 table 1.18).
        f[2] = 0x50;
        f
    };
    let mut mux = audio_stream();
    mux.pes(
        0x101,
        0xC0,
        43_830,
        &[frame(), frame(), frame()].concat(),
        true,
    );
    mux.out
}

#[test]
fn aac_frames_a_pes_packet_does_not_time_take_the_samples_since_it() {
    // By default the samples since the PES time, rounded once.
    let data = aac_at_44100();
    let ts = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let times: Vec<_> = ts
        .packets(SliceSource::new(&data), &Limits::default())
        .map(|p| p.unwrap().pts)
        .collect();
    assert_eq!(times, vec![Some(43_830), Some(45_920), Some(48_010)]);
}

#[test]
fn as_ffprobe_aac_frames_fall_on_a_grid_counted_from_zero() {
    // As `ffprobe` places them: the third frame is 4,179 ticks after the
    // PES time, not 4,180 (the grid of `grid_time`).
    let data = aac_at_44100();
    let ts = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let times: Vec<_> = ts
        .packets(SliceSource::new(&data), &Limits::default())
        .listing_as_ffprobe()
        .map(|p| p.unwrap().pts)
        .collect();
    assert_eq!(times, vec![Some(43_830), Some(45_920), Some(48_009)]);
}

/// An E-AC-3 stream at 44.1 kHz (a frame of 1,536 samples lasts 3,134.69
/// ticks) and an AAC stream: the first E-AC-3 PES packet holds a frame
/// and the start of the next, the AAC PES packet comes next, and the
/// second E-AC-3 PES packet finishes the cut frame.
fn eac3_cut_before_another_stream() -> Vec<u8> {
    // 256 bytes, 44.1 kHz (fscod 1), 6 blocks, an independent frame.
    let mut frame = vec![0x0B, 0x77, 0x00, 0x7F, 0x7F, 0x80];
    frame.resize(256, 0);
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x87, 0x101), (0x0F, 0x102)]);
    mux.pes(
        0x101,
        0xC0,
        90_000,
        &[&frame[..], &frame[..100]].concat(),
        true,
    );
    mux.pes(0x102, 0xC1, 90_000, &adts(300), true);
    mux.pes(0x101, 0xC0, 99_000, &frame[100..], true);
    mux.out
}

#[test]
fn an_eac3_frame_is_listed_once_the_next_one_is_whole() {
    // By default the first E-AC-3 frame waits until the frame after it is
    // whole, behind the AAC frame, and the cut frame starts the samples
    // of one frame, rounded, after the PES time.
    let data = eac3_cut_before_another_stream();
    let ts = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let listed: Vec<_> = ts
        .packets(SliceSource::new(&data), &Limits::default())
        .map(|p| p.unwrap())
        .map(|p| (ts.streams[p.stream].pid, p.pts))
        .collect();
    assert_eq!(
        listed,
        vec![
            (0x102, Some(90_000)),
            (0x101, Some(90_000)),
            (0x101, Some(93_135)),
        ]
    );
}

#[test]
fn as_ffprobe_an_eac3_frame_is_listed_once_the_next_header_is_read() {
    // As `ffprobe` lists them: the first E-AC-3 frame is listed as soon as
    // the header of the next one is read, though the end of the PES
    // packet cuts that one, so before the AAC frame; and the cut frame
    // starts one truncated frame duration (3,134 ticks) after the first.
    let data = eac3_cut_before_another_stream();
    let ts = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let listed: Vec<_> = ts
        .packets(SliceSource::new(&data), &Limits::default())
        .listing_as_ffprobe()
        .map(|p| p.unwrap())
        .map(|p| (ts.streams[p.stream].pid, p.pts))
        .collect();
    assert_eq!(
        listed,
        vec![
            (0x101, Some(90_000)),
            (0x102, Some(90_000)),
            (0x101, Some(93_134)),
        ]
    );
}

/// An MPEG-2 video picture of `kind` (1 I, 2 P, 3 B) and `structure` (3 a
/// frame, 1 and 2 a field), with a slice of `slice` bytes; the first
/// carries a sequence header.
fn mpeg2_picture(kind: u8, structure: u8, slice: usize, sequence: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if sequence {
        out.extend([
            0, 0, 1, 0xB3, 0x14, 0x00, 0xF0, 0x13, 0xFF, 0xFF, 0xE0, 0x18,
        ]);
    }
    out.extend([0, 0, 1, 0x00, 0x00, (kind << 3) | 0x07, 0xFF, 0xF8]);
    out.extend([0, 0, 1, 0xB5, 0x8F, 0xFF, 0xF0 | structure, 0x80]);
    out.extend([0, 0, 1, 0x01]);
    out.resize(out.len() + slice, 0x55);
    out
}

fn mpeg2_listing(mux: &Mux, as_ffprobe: bool) -> Vec<(usize, Option<i64>, Option<u64>, bool)> {
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let packets = ts.packets(SliceSource::new(&mux.out), &Limits::default());
    let packets: Vec<_> = if as_ffprobe {
        packets.listing_as_ffprobe().collect()
    } else {
        packets.collect()
    };
    packets
        .into_iter()
        .map(|p| p.unwrap())
        .map(|p| (p.data.len(), p.pts, p.pos, p.keyframe))
        .collect()
}

#[test]
fn as_ffprobe_mpeg2_video_is_listed_picture_by_picture() {
    // A PES packet of an I-picture, a B-picture and the start of a
    // P-picture, then one of the rest of it and a B-picture: `ffprobe`
    // lists the four pictures, the first of each PES packet with its time
    // and position, the P-picture none (it starts in the first). By
    // default each PES packet is a packet.
    let i = mpeg2_picture(1, 3, 300, true);
    let b = mpeg2_picture(3, 3, 100, false);
    let p = mpeg2_picture(2, 3, 200, false);
    let b2 = mpeg2_picture(3, 3, 120, false);
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x02, 0x100)]);
    let first = mux.out.len() as u64;
    mux.pes(0x100, 0xE0, 93_003, &[&i[..], &b, &p[..20]].concat(), false);
    let second = mux.out.len() as u64;
    mux.pes(0x100, 0xE0, 99_009, &[&p[20..], &b2[..]].concat(), false);
    assert_eq!(
        mpeg2_listing(&mux, true),
        vec![
            (i.len(), Some(93_003), Some(first), true),
            (b.len(), None, None, false),
            (p.len(), None, None, false),
            (b2.len(), Some(99_009), Some(second), false),
        ]
    );
    assert_eq!(mpeg2_listing(&mux, false).len(), 2);
}

#[test]
fn as_ffprobe_the_two_fields_of_a_frame_are_one_picture() {
    // Each field in a PES packet of its own, each with a time: the second
    // field is part of its frame's packet, and its PES packet's time is
    // nobody's, as the first unit starting in it is the next frame's.
    let top = mpeg2_picture(1, 1, 200, true);
    let bottom = mpeg2_picture(1, 2, 150, false);
    let next = mpeg2_picture(2, 1, 100, false);
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x02, 0x100)]);
    let first = mux.out.len() as u64;
    mux.pes(0x100, 0xE0, 90_000, &top, false);
    mux.pes(0x100, 0xE0, 91_877, &bottom, false);
    let third = mux.out.len() as u64;
    mux.pes(0x100, 0xE0, 93_754, &next, false);
    assert_eq!(
        mpeg2_listing(&mux, true),
        vec![
            (top.len() + bottom.len(), Some(90_000), Some(first), true),
            (next.len(), Some(93_754), Some(third), false),
        ]
    );
}

/// An MPEG-1 layer II frame of 576 bytes (192 kbit/s at 48 kHz).
fn mp2_frame() -> Vec<u8> {
    let mut f = vec![0xFF, 0xFD, 0xA4, 0xC4];
    f.resize(576, 0x55);
    f
}

/// An AC-3 frame of 768 bytes (192 kbit/s at 48 kHz, 2/0).
fn ac3_frame() -> Vec<u8> {
    let mut f = vec![0x0B, 0x77, 0, 0, 0x14, 0x40, 0x40, 0x00];
    f.resize(768, 0x55);
    f
}

fn audio_listing(mux: &Mux, as_ffprobe: bool) -> Vec<(usize, Option<i64>, Option<u64>)> {
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let packets = ts.packets(SliceSource::new(&mux.out), &Limits::default());
    let packets: Vec<_> = if as_ffprobe {
        packets.listing_as_ffprobe().collect()
    } else {
        packets.collect()
    };
    packets
        .into_iter()
        .map(|p| p.unwrap())
        .map(|p| (p.data.len(), p.pts, p.pos))
        .collect()
}

#[test]
fn as_ffprobe_the_bytes_before_the_first_mpeg_audio_frame_join_it() {
    // 29 bytes of a frame that began before the file, then frames: by
    // default they are a packet of their own; as `ffprobe` lists MPEG
    // audio, the start of the first frame's packet, whether the end of
    // the PES packet cuts that frame or not.
    let lead = [0x22u8; 29];
    for cut in [None, Some(300)] {
        let frames = [mp2_frame(), mp2_frame()].concat();
        let all = [&lead[..], &frames].concat();
        let split = cut.map_or(all.len(), |c| lead.len() + c);
        let mut mux = Mux::default();
        mux.pat(&[(1, 0x1000)]);
        mux.pmt(0x1000, 1, &[(0x03, 0x101)]);
        let first = mux.out.len() as u64;
        mux.pes(0x101, 0xC0, 90_000, &all[..split], true);
        let second = mux.out.len() as u64;
        if split < all.len() {
            mux.pes(0x101, 0xC0, 92_160, &all[split..], true);
        }
        let expected_second = if split < all.len() {
            (576, Some(92_160), Some(second))
        } else {
            (576, Some(92_160), None)
        };
        assert_eq!(
            audio_listing(&mux, true),
            vec![(29 + 576, Some(90_000), Some(first)), expected_second],
            "{cut:?}"
        );
        assert_eq!(audio_listing(&mux, false).len(), 3, "{cut:?}");
    }
}

#[test]
fn the_bytes_before_a_first_frame_its_pes_packet_cuts_are_a_packet() {
    // 100 bytes of an AC-3 frame that began before the file, then a frame
    // the end of the PES packet cuts: the 100 bytes are listed with the
    // PES packet's time, and the cut frame follows them by one frame.
    let lead = [0x22u8; 100];
    let frame = ac3_frame();
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x81, 0x101)]);
    let first = mux.out.len() as u64;
    mux.pes(
        0x101,
        0xBD,
        87_120,
        &[&lead[..], &frame[..300]].concat(),
        true,
    );
    let second = mux.out.len() as u64;
    mux.pes(
        0x101,
        0xBD,
        92_880,
        &[&frame[300..], &ac3_frame()[..]].concat(),
        true,
    );
    for as_ffprobe in [false, true] {
        assert_eq!(
            audio_listing(&mux, as_ffprobe),
            vec![
                (100, Some(87_120), Some(first)),
                (768, Some(90_000), None),
                (768, Some(92_880), Some(second)),
            ],
            "{as_ffprobe}"
        );
    }
}

#[test]
fn as_ffprobe_the_bytes_before_a_header_the_pes_packet_cuts_are_a_packet() {
    // 100 bytes of a frame that began before the file, then too little of
    // the next frame's header for this reader (one to seven bytes) or
    // none of it: the listing as `ffprobe` gives it lists those bytes as
    // a packet of their own all the same, and the frame after them takes
    // the next PES packet's time and position. The default listing drops
    // them, which it did before.
    for held in [0usize, 1, 6, 7] {
        let frames = [ac3_frame(), ac3_frame()].concat();
        let lead = [0x22u8; 100];
        let mut mux = Mux::default();
        mux.pat(&[(1, 0x1000)]);
        mux.pmt(0x1000, 1, &[(0x81, 0x101)]);
        let first = mux.out.len() as u64;
        mux.pes(
            0x101,
            0xBD,
            90_000,
            &[&lead[..], &frames[..held]].concat(),
            true,
        );
        let second = mux.out.len() as u64;
        mux.pes(0x101, 0xBD, 92_880, &frames[held..], true);
        let listed = audio_listing(&mux, true);
        assert_eq!(
            listed.first().copied(),
            Some((100, Some(90_000), Some(first))),
            "{held}"
        );
        // The frames after them, timed by the rule for a frame its PES
        // packet cuts (the test below), and none lost.
        assert_eq!(
            listed.iter().map(|(len, _, _)| *len).collect::<Vec<_>>(),
            vec![100, 768, 768],
            "{held}"
        );
        // Fewer than seven bytes of it: the frame takes the next PES
        // packet's position, as it takes its time; with seven it follows
        // the lead bytes with none (the test below).
        assert_eq!(
            listed[1].2,
            if held < 7 { Some(second) } else { None },
            "{held}"
        );
        // The frames alone by default, the lead bytes dropped.
        assert_eq!(
            audio_listing(&mux, false)
                .into_iter()
                .map(|(len, _, _)| len)
                .collect::<Vec<_>>(),
            vec![768, 768],
            "{held}"
        );
    }
}

#[test]
fn as_ffprobe_an_ac3_frame_with_six_bytes_in_its_pes_packet_starts_in_the_next() {
    // A frame of which its PES packet holds six bytes takes the next PES
    // packet's time and position, as the first frame that starts there
    // does too; with seven bytes it follows the frame before it.
    for (held, late) in [(1, true), (6, true), (7, false), (16, false)] {
        let frames = [ac3_frame(), ac3_frame(), ac3_frame()].concat();
        let mut mux = Mux::default();
        mux.pat(&[(1, 0x1000)]);
        mux.pmt(0x1000, 1, &[(0x81, 0x101)]);
        let first = mux.out.len() as u64;
        mux.pes(0x101, 0xBD, 90_000, &frames[..768 + held], true);
        let second = mux.out.len() as u64;
        mux.pes(0x101, 0xBD, 95_760, &frames[768 + held..], true);
        let cut = if late {
            (768, Some(95_760), Some(second))
        } else {
            (768, Some(92_880), None)
        };
        assert_eq!(
            audio_listing(&mux, true),
            vec![
                (768, Some(90_000), Some(first)),
                cut,
                (768, Some(95_760), Some(second)),
            ],
            "{held}"
        );
    }
}

/// An E-AC-3 independent frame of 256 bytes (one block of 256 samples at
/// 48 kHz).
fn eac3_frame() -> Vec<u8> {
    let mut f = vec![0x0B, 0x77, 0x00, 0x7F, 0x3F, 0x80];
    f.resize(256, 0x55);
    f
}

/// A DTS core frame of 1,024 bytes (512 samples at 48 kHz).
fn dts_frame() -> Vec<u8> {
    let mut f = vec![
        0x7F, 0xFE, 0x80, 0x01, 0xFC, 0x3C, 0x3F, 0xF0, 0xB5, 0xE0, 0x01, 0x38, 0x00, 0x03, 0xEF,
        0x7F,
    ];
    f.resize(1024, 0x55);
    f
}

/// A stream of `stream_type` whose PES packets hold `payloads`, timed from
/// 90,000 in steps of 5,000, and the position of each.
fn audio_mux(stream_type: u8, payloads: &[&[u8]]) -> (Mux, Vec<u64>) {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(stream_type, 0x101)]);
    let stream_id = if matches!(stream_type, 0x03 | 0x0F) {
        0xC0
    } else {
        0xBD
    };
    let mut at = Vec::new();
    for (k, payload) in payloads.iter().enumerate() {
        at.push(mux.out.len() as u64);
        mux.pes(0x101, stream_id, 90_000 + 5_000 * k as u64, payload, true);
    }
    (mux, at)
}

#[test]
fn as_ffprobe_a_frame_seven_bytes_after_lead_bytes_follows_them() {
    // 100 bytes before the first frame header and seven bytes of the
    // frame, enough for `ffprobe` to place it: the frame follows the
    // packet of those bytes, a frame later for AC-3 and at their time for
    // E-AC-3, whose lead packet takes none.
    let lead = [0x22u8; 100];
    for (stream_type, frame, after) in [(0x81, ac3_frame(), 92_880), (0x87, eac3_frame(), 90_000)] {
        let frames = [frame.clone(), frame.clone()].concat();
        let (mux, at) = audio_mux(
            stream_type,
            &[&[&lead[..], &frames[..7]].concat(), &frames[7..]],
        );
        assert_eq!(
            audio_listing(&mux, true),
            vec![
                (100, Some(90_000), Some(at[0])),
                (frame.len(), Some(after), None),
                (frame.len(), Some(95_000), Some(at[1])),
            ],
            "{stream_type:#x}"
        );
    }
}

#[test]
fn as_ffprobe_the_bytes_before_an_adts_header_wait_for_it() {
    // 100 bytes before the first ADTS header, then part of the frame: with
    // too little of the header for it to read, the frame takes the next
    // PES packet's time and position, as the first frame that starts there
    // does too; with the header whole, it follows the lead packet.
    let lead = [0x22u8; 100];
    let frames = [adts(300), adts(300)].concat();
    for (held, second) in [(3, None), (7, Some(91_920))] {
        let (mux, at) = audio_mux(
            0x0F,
            &[&[&lead[..], &frames[..held]].concat(), &frames[held..]],
        );
        let second = second.map_or((300, Some(95_000), Some(at[1])), |t| (300, Some(t), None));
        assert_eq!(
            audio_listing(&mux, true),
            vec![
                (100, Some(90_000), Some(at[0])),
                second,
                (300, Some(95_000), Some(at[1])),
            ],
            "{held}"
        );
    }
}

#[test]
fn as_ffprobe_lead_bytes_in_pes_packets_of_their_own_by_syntax() {
    // Two PES packets of 50 bytes each before the first frame header: one
    // packet of them for AC-3, the start of the first frame for MPEG
    // audio, nothing for DTS, each with the first PES packet's time and
    // position; the third PES packet's time goes to the first frame that
    // starts in it after that.
    let lead = [0x22u8; 50];
    let ac3 = [ac3_frame(), ac3_frame()].concat();
    let (mux, at) = audio_mux(0x81, &[&lead, &lead, &ac3]);
    assert_eq!(
        audio_listing(&mux, true),
        vec![
            (100, Some(90_000), Some(at[0])),
            (768, Some(100_000), Some(at[2])),
            (768, Some(102_880), None),
        ]
    );
    let mp2 = [mp2_frame(), mp2_frame()].concat();
    let (mux, at) = audio_mux(0x03, &[&lead, &lead, &mp2]);
    assert_eq!(
        audio_listing(&mux, true),
        vec![
            (676, Some(90_000), Some(at[0])),
            (576, Some(100_000), Some(at[2])),
        ]
    );
    let dts = dts_frame().repeat(2);
    let (mux, at) = audio_mux(0x82, &[&lead, &lead, &dts, &dts]);
    assert_eq!(
        audio_listing(&mux, true)[..2],
        [
            (1024, Some(90_000), Some(at[0])),
            (1024, Some(100_000), Some(at[2])),
        ]
    );
    // MPEG audio whose first header the end of the file cuts: the bytes
    // before it and those of it, one packet.
    let (mux, at) = audio_mux(0x03, &[&lead, &mp2_frame()[..3]]);
    assert_eq!(
        audio_listing(&mux, true),
        vec![(53, Some(90_000), Some(at[0]))]
    );
    // A file that ends before any header: one packet of what there is.
    let (mux, at) = audio_mux(0x81, &[&lead, &lead]);
    assert_eq!(
        audio_listing(&mux, true),
        vec![(100, Some(90_000), Some(at[0]))]
    );
}

#[test]
fn as_ffprobe_a_carried_byte_that_starts_no_header_joins_the_lead_bytes() {
    // A first PES packet that ends in 0xFF, which may start an ADTS
    // header, and a next one whose first byte shows it does not: that
    // byte is among the lead bytes, and the frames after them in the next
    // PES packet take its time once, then follow one another.
    // The same with that byte alone in the first PES packet: it still
    // takes that packet's time and position.
    let rest = [&[0x22u8][..], &adts(300), &adts(300)].concat();
    for lead_len in [100, 0] {
        let lead = [vec![0x22u8; lead_len], vec![0xFF]].concat();
        let (mux, at) = audio_mux(0x0F, &[&lead, &rest]);
        assert_eq!(
            audio_listing(&mux, true),
            vec![
                (lead_len + 2, Some(90_000), Some(at[0])),
                (300, Some(95_000), Some(at[1])),
                (300, Some(96_920), None),
            ],
            "{lead_len}"
        );
    }
}

#[test]
fn as_ffprobe_bytes_before_a_header_are_bounded_by_a_frame() {
    // 51 KB without a header before the first AC-3 frame, more than any
    // frame holds: dropped up to that frame rather than held, so a buffer
    // limit of 16 KB is not met, none of them is listed, and the frames
    // are.
    let junk = [0x22u8; 1_000];
    let frames = [ac3_frame(), ac3_frame()].concat();
    let mut payloads: Vec<&[u8]> = vec![&junk; 51];
    payloads.push(&frames);
    let (mux, at) = audio_mux(0x81, &payloads);
    let limits = Limits {
        max_buffered_bytes: 16 << 10,
        ..Limits::default()
    };
    let ts = probe(SliceSource::new(&mux.out), &limits, &Allowlist::all()).unwrap();
    let listed: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &limits)
        .listing_as_ffprobe()
        .map(|p| p.map(|p| (p.data.len(), p.pts, p.pos)))
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        listed,
        vec![
            (768, Some(345_000), Some(at[51])),
            (768, Some(347_880), None),
        ]
    );
}

#[test]
fn a_dependent_substream_the_end_of_the_file_cuts_joins_its_access_unit() {
    // An E-AC-3 independent frame, then a dependent substream of which the
    // file holds 100 bytes: one access unit, with what there is.
    let frame = |strmtyp: u8| {
        let mut f = vec![0x0B, 0x77, strmtyp << 6, 0x7F, 0x3F, 0x80];
        f.resize(256, 0);
        f
    };
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x87, 0x101)]);
    mux.pes(0x101, 0xC0, 90_000, &frame(0), true);
    mux.pes(0x101, 0xC0, 92_880, &frame(1)[..100], false);
    for as_ffprobe in [false, true] {
        let sizes: Vec<_> = audio_listing(&mux, as_ffprobe)
            .into_iter()
            .map(|(size, ..)| size)
            .collect();
        assert_eq!(sizes, vec![356], "{as_ffprobe}");
    }
}

/// A transport packet of `pid` that carries only a PCR of `base`.
fn pcr_packet(mux: &mut Mux, pid: u16, base: u64) {
    let counter = mux.counters.entry(pid).or_insert(0);
    // Adaptation field only: the counter does not step (13818-1 2.4.3.3).
    let mut p = vec![0x47, (pid >> 8) as u8, pid as u8, 0x20 | *counter];
    p.extend([183, 0x10]);
    p.extend([
        (base >> 25) as u8,
        (base >> 17) as u8,
        (base >> 9) as u8,
        (base >> 1) as u8,
        ((base as u8 & 1) << 7) | 0x7E,
        0,
    ]);
    p.resize(188, 0xFF);
    mux.out.extend(p);
}

/// A splice information section (SCTE 35 9.2) with a `splice_null`, and
/// its CRC-32 spoilt when `damaged`.
fn splice_null(damaged: bool) -> Vec<u8> {
    let mut s = vec![
        0xFC, 0x30, 0x11, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0, 0, 0,
    ];
    let crc = crc32(&s) ^ u32::from(damaged);
    s.extend(crc.to_be_bytes());
    s
}

#[test]
fn as_ffprobe_splice_sections_are_packets_timed_by_the_programs_clock() {
    // An SCTE 35 stream: a section before the first PCR has no time, one
    // after it the PCR's, a PCR of a PID no program names counts for
    // nothing, and a section whose CRC fails is not listed. The default
    // listing lists no section.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt_with(
        0x1000,
        1,
        &Mux::registration(b"CUEI"),
        &[(0x0F, 0x100), (0x86, 0x102)],
    );
    let section = |mux: &mut Mux, damaged: bool| {
        mux.unit(0x102, &[&[0u8][..], &splice_null(damaged)].concat());
    };
    section(&mut mux, false);
    pcr_packet(&mut mux, 0x100, 63_000);
    section(&mut mux, false);
    pcr_packet(&mut mux, 0x200, 5_000_000);
    section(&mut mux, true);
    section(&mut mux, false);
    mux.pes(0x100, 0xC0, 90_000, &adts(100), true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let listed: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .listing_as_ffprobe()
        .map(|p| p.unwrap())
        .filter(|p| ts.streams[p.stream].codec == Codec::Scte35)
        .map(|p| (p.data.len(), p.pts, p.dts, p.pos))
        .collect();
    assert_eq!(
        listed,
        vec![
            (20, None, None, None),
            (20, Some(63_000), Some(63_000), None),
            (20, Some(63_000), Some(63_000), None),
        ]
    );
    let default = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap())
        .filter(|p| ts.streams[p.stream].codec == Codec::Scte35)
        .count();
    assert_eq!(default, 0);
}

#[test]
fn as_ffprobe_a_type_0x86_stream_whose_program_registers_nothing_has_no_packets() {
    // `ffprobe` reads type 0x86 as splice information only from the
    // program's registration; a registration in the stream's own loop,
    // `CUEI` or another identifier, leaves the type unnamed and its
    // sections unlisted.
    for stream_descriptors in [
        Vec::new(),
        Mux::registration(b"CUEI"),
        Mux::registration(b"ABCD"),
    ] {
        let mut mux = Mux::default();
        mux.pat(&[(1, 0x1000)]);
        mux.pmt_streams(
            0x1000,
            1,
            &[],
            &[(0x0F, 0x100, Vec::new()), (0x86, 0x102, stream_descriptors)],
        );
        mux.unit(0x102, &[&[0u8][..], &splice_null(false)].concat());
        pcr_packet(&mut mux, 0x100, 63_000);
        mux.unit(0x102, &[&[0u8][..], &splice_null(false)].concat());
        mux.pes(0x100, 0xC0, 90_000, &adts(100), true);
        let ts = probe(
            SliceSource::new(&mux.out),
            &Limits::default(),
            &Allowlist::all(),
        )
        .unwrap();
        assert_eq!(ts.streams[1].codec, Codec::Other);
        let listed = ts
            .packets(SliceSource::new(&mux.out), &Limits::default())
            .listing_as_ffprobe()
            .map(|p| p.unwrap())
            .filter(|p| p.stream == 1)
            .count();
        assert_eq!(listed, 0);
    }
}
