//! Hand-built transport streams of structures the real captures of the
//! corpus set `v1-ts-audio` (`docs/corpus.md`) showed, each the shape of a
//! case the reader once read wrongly (docs/design/mpegts.md).
#![allow(
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod common;

use common::{Mux, adts, h264_pps, h264_sps};
use coreav_mpegts::{Allowlist, Limits, SliceSource, probe};

/// A picture size with the clock its sequence parameter set states, if
/// any: `num_units_in_tick` and `time_scale` (ITU-T H.264 E.2.1).
type Sets = (u32, u32, Option<(u32, u32)>);

/// A coded picture: a delimiter, the parameter sets when `sets` asks for
/// them, and an IDR slice of `slice` bytes.
fn picture(sets: Option<Sets>, slice: usize) -> Vec<u8> {
    let mut au = vec![0, 0, 0, 1, 0x09, 0x10];
    if let Some((width, height, timing)) = sets {
        au.extend([0, 0, 1]);
        au.extend(h264_sps(width, height, timing));
        au.extend([0, 0, 1]);
        au.extend(h264_pps());
    }
    au.extend([0, 0, 1, 0x65, 0x88]);
    au.resize(au.len() + slice, 0x42);
    au
}

#[test]
fn parameter_sets_of_the_pes_packet_the_end_of_the_file_completes() {
    // A camera's .m2ts of one access unit: the video PES packet states no
    // length (13818-1 2.4.3.7 allows 0 for video) and only the end of the
    // file completes it, so the reassembler held the parameter sets, and
    // with them the picture size, the profile and the level, until the
    // probe had given up on the stream.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x1011)]);
    // 60000/1001 fields a second, as a 1080i camera states it.
    mux.pes(
        0x1011,
        0xE0,
        99_009,
        &picture(Some((1440, 1088, Some((1001, 60_000)))), 3_000),
        false,
    );
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let video = ts.streams[0].video.unwrap();
    assert_eq!((video.width, video.height), (1440, 1088));
    assert!(matches!(
        ts.streams[0].profile,
        Some(coreav_isobmff::Profile::H264 {
            profile_idc: 66,
            level_idc: 30,
            ..
        })
    ));
    // The file times the stream once, so its own timestamps show no step
    // and the stream ends one tick of the clock its sequence parameter
    // set states after that time: 90000 * 1001 / 60000.
    assert_eq!(ts.streams[0].duration(), Some(1_501));
}

#[test]
fn a_stream_the_file_times_once_and_states_no_clock_for_has_no_duration() {
    // Without the timing information there is nothing to end on: the
    // reader reports no duration rather than guessing one.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x1011)]);
    mux.pes(
        0x1011,
        0xE0,
        99_009,
        &picture(Some((1440, 1088, None)), 3_000),
        false,
    );
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(ts.streams[0].video.unwrap().width, 1440);
    assert_eq!(ts.streams[0].duration(), None);
}

#[test]
fn audio_parameters_of_a_stream_whose_pes_packets_start_inside_a_frame() {
    // A broadcast capture's one-segment AAC: the muxer fills each PES
    // packet to a fixed size, so every payload but the stream's first
    // begins inside the frame the packet before it started, and no
    // payload begins with a frame header. Taking the header at the start
    // of a payload left the stream with no rate, no channels and no
    // profile at all.
    let frames: Vec<u8> = (0..8).flat_map(|i| adts(300 + i * 8)).collect();
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    // The first payload begins 40 bytes into a frame, and every one after
    // it 500 bytes on, which no frame boundary falls on.
    let mut at = 40;
    let mut pts = 90_000;
    while at < frames.len() {
        let end = (at + 500).min(frames.len());
        mux.pes(0x101, 0xC0, pts, &frames[at..end], true);
        at = end;
        pts += 1_920;
    }
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let audio = ts.streams[0].audio.unwrap();
    assert_eq!(
        (audio.sample_rate, audio.channels, audio.detail),
        (48_000, 2, 2)
    );
}

#[test]
fn a_run_divides_the_step_that_closes_it() {
    // A capture whose PES packets state a time every four pictures and
    // then every five. The run's units divide the step from the time the
    // run starts at to the one that closes it; dividing the smallest step
    // the whole stream states instead -- four picture intervals here --
    // put the five-picture runs a fifth of a picture apart.
    let interval = 3_600u64;
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    let mut pts = 90_000;
    let mut expected = Vec::new();
    for pictures in [4usize, 5, 5, 4] {
        let payload: Vec<u8> = (0..pictures).flat_map(|_| picture(None, 120)).collect();
        mux.pes(0x101, 0xE0, pts, &payload, false);
        for k in 0..pictures {
            expected.push(Some(pts as i64 + (k as u64 * interval) as i64));
        }
        pts += interval * pictures as u64;
    }
    // One more PES packet to close the last run.
    mux.pes(0x101, 0xE0, pts, &picture(None, 120), false);
    expected.push(Some(pts as i64));
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
    assert_eq!(times, expected);
}

#[test]
fn a_pes_packet_that_states_no_decoding_time_gives_the_presentation_one() {
    // ISO/IEC 13818-1 2.7.5: a decoding time is encoded only when it
    // differs from the presentation time, so a PES packet that states
    // none -- or one equal to its PTS -- states that the two are the same
    // (KD-TS-DECODE-TIME).
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    mux.pes_times(
        0x101,
        0xE0,
        93_600,
        Some(90_000),
        &picture(None, 120),
        false,
    );
    mux.pes_times(
        0x101,
        0xE0,
        97_200,
        Some(97_200),
        &picture(None, 120),
        false,
    );
    mux.pes_times(0x101, 0xE0, 100_800, None, &picture(None, 120), false);
    mux.pes_times(0x101, 0xE0, 104_400, None, &picture(None, 120), false);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let times: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap())
        .map(|p| (p.pts, p.dts))
        .collect();
    assert_eq!(
        times,
        vec![
            (Some(93_600), Some(90_000)),
            (Some(97_200), Some(97_200)),
            (Some(100_800), Some(100_800)),
            (Some(104_400), Some(104_400)),
        ]
    );
}

#[test]
fn a_pid_no_table_declares_is_listed_after_the_streams_of_every_program() {
    // A broadcast capture carries PES packets on PIDs its tables never
    // name; they are the file's packets, and both readers list them. This
    // reader lists them after the streams the tables declare, whatever
    // order the file delivers them in (KD-TS-STREAM-ORDER).
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000), (2, 0x1001)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x100)]);
    // private_stream_2 (table 2-18): a six-byte header and opaque bytes.
    mux.unit(0x1FF, &[0, 0, 1, 0xBF, 0, 4, 1, 2, 3, 4]);
    mux.pes(0x100, 0xE0, 90_000, &picture(None, 120), false);
    mux.pmt(0x1001, 2, &[(0x0F, 0x200)]);
    mux.pes(0x200, 0xC0, 90_000, &adts(300), true);
    mux.pes(0x100, 0xE0, 93_600, &picture(None, 120), false);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    assert_eq!(
        ts.streams.iter().map(|s| s.pid).collect::<Vec<_>>(),
        vec![0x100, 0x200, 0x1FF]
    );
    assert_eq!(ts.streams[2].pmt_index, None);
}

#[test]
fn a_run_the_file_does_not_close_keeps_its_place() {
    // A capture that starts before its first stated time: the units
    // before it are a run no gap closes, and they are listed where they
    // are, ahead of the run that gives the stream its first period.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    let mut offsets = Vec::new();
    for (k, pts) in [None, Some(90_000u64), Some(93_600), Some(97_200)]
        .into_iter()
        .enumerate()
    {
        offsets.push(mux.out.len() as u64);
        let payload = [picture(None, 120), picture(None, 120)].concat();
        match pts {
            Some(pts) => mux.pes(0x101, 0xE0, pts, &payload, false),
            // A PES packet that states no time at all, before the first
            // one that does.
            None => mux.unit(
                0x101,
                &[&[0, 0, 1, 0xE0, 0, 0, 0x80, 0, 0][..], &payload].concat(),
            ),
        }
        let _ = k;
    }
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let listed: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap().pes_offset)
        .collect();
    let mut expected: Vec<u64> = Vec::new();
    for offset in &offsets {
        expected.push(*offset);
        expected.push(*offset);
    }
    // The last unit of the last PES packet is still open at the end of
    // the file; it is listed there too.
    assert_eq!(listed.len(), expected.len());
    assert_eq!(listed, expected);
}

#[test]
fn a_run_two_pes_packets_of_one_time_do_not_close_keeps_its_place() {
    // Two PES packets that state the same decoding time: the first run
    // has no positive step to divide, and must still be listed before
    // the units after it.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    let mut offsets = Vec::new();
    for pts in [90_000u64, 90_000, 93_600, 97_200] {
        offsets.push(mux.out.len() as u64);
        mux.pes(
            0x101,
            0xE0,
            pts,
            &[picture(None, 120), picture(None, 120)].concat(),
            false,
        );
    }
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let listed: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap().pes_offset)
        .collect();
    let expected: Vec<u64> = offsets.iter().flat_map(|o| [*o, *o]).collect();
    assert_eq!(listed, expected);
}

#[test]
fn a_run_of_one_unit_waits_while_an_earlier_run_does() {
    // The first run is held for want of a period; the run after it is
    // one unit, which needs no period of its own and must still be
    // listed behind the units read before it.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    let mut offsets = Vec::new();
    // A PES packet of two units that states no time, then two that state
    // the same one -- no step to divide -- then one that moves on.
    offsets.push(mux.out.len() as u64);
    let pair = [picture(None, 120), picture(None, 120)].concat();
    mux.unit(
        0x101,
        &[&[0, 0, 1, 0xE0, 0, 0, 0x80, 0, 0][..], &pair].concat(),
    );
    for pts in [90_000u64, 90_000, 93_600] {
        offsets.push(mux.out.len() as u64);
        mux.pes(0x101, 0xE0, pts, &picture(None, 120), false);
    }
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let listed: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap().pes_offset)
        .collect();
    assert_eq!(
        listed,
        vec![offsets[0], offsets[0], offsets[1], offsets[2], offsets[3]]
    );
}

#[test]
fn sound_keeps_the_decoding_time_its_pes_packet_states() {
    // A packaged stream whose audio PES packets state a decoding time
    // 600 ticks before the presentation time: every frame of the stream
    // decodes that far ahead, the frames the file times and the frames
    // it does not.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    for (k, pts) in [90_000u64, 93_840, 97_680].into_iter().enumerate() {
        let frames = [adts(200), adts(220)].concat();
        mux.pes_times(0x101, 0xC0, pts, Some(pts - 600), &frames, true);
        let _ = k;
    }
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let times: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap())
        .map(|p| (p.pts, p.dts))
        .collect();
    assert_eq!(
        times,
        vec![
            (Some(90_000), Some(89_400)),
            (Some(91_920), Some(91_320)),
            (Some(93_840), Some(93_240)),
            (Some(95_760), Some(95_160)),
            (Some(97_680), Some(97_080)),
            (Some(99_600), Some(99_000)),
        ]
    );
}

#[test]
fn a_run_waits_while_an_earlier_one_does_however_short_the_step() {
    // A step so short that it divides to nothing teaches the stream no
    // period, so the run before it is still waiting when the run that
    // closed it is listed: it waits too, or the two come out in the
    // wrong order.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    let mut offsets = vec![mux.out.len() as u64];
    let pair = [picture(None, 120), picture(None, 120)].concat();
    mux.unit(
        0x101,
        &[&[0, 0, 1, 0xE0, 0, 0, 0x80, 0, 0][..], &pair].concat(),
    );
    offsets.push(mux.out.len() as u64);
    let three = [picture(None, 120), picture(None, 120), picture(None, 120)].concat();
    // One tick between the two stated times, across three pictures.
    mux.pes(0x101, 0xE0, 90_000, &three, false);
    offsets.push(mux.out.len() as u64);
    mux.pes(0x101, 0xE0, 90_001, &picture(None, 120), false);
    offsets.push(mux.out.len() as u64);
    mux.pes(0x101, 0xE0, 93_600, &picture(None, 120), false);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let listed: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| p.unwrap().pes_offset)
        .collect();
    assert_eq!(
        listed,
        vec![
            offsets[0], offsets[0], offsets[1], offsets[1], offsets[1], offsets[2], offsets[3],
        ]
    );
}

/// Seven bytes that read as an ADTS header and are not one: 8 kHz, a
/// declared length of 8,000 bytes that no ordinary PES payload holds.
fn false_adts_sync() -> [u8; 7] {
    [0xFF, 0xF1, 0x2C, 0x83, 0xE8, 0x1F, 0xFC]
}

#[test]
fn a_false_sync_does_not_give_the_stream_a_rate() {
    // The bytes before a stream's first frame are the tail of a frame
    // that began before the file, and they may read as a header: the
    // payload confirms the real frame, which ends at the one after it,
    // and not the false one, whose length the payload cannot hold.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    let mut first = false_adts_sync().to_vec();
    first.extend([adts(300), adts(320), adts(300)].concat());
    mux.pes(0x101, 0xC0, 90_000, &first, true);
    mux.pes(0x101, 0xC0, 95_760, &[adts(300), adts(320)].concat(), true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let audio = ts.streams[0].audio.unwrap();
    assert_eq!((audio.sample_rate, audio.channels), (48_000, 2));
}

#[test]
fn frames_longer_than_the_pes_payloads_still_give_the_stream_a_rate() {
    // A muxer whose every frame straddles a PES boundary: no payload
    // ever holds a whole frame, so none confirms a place, and the first
    // header that reads is all there is to go on.
    let frames: Vec<u8> = (0..8).flat_map(|i| adts(600 + i * 8)).collect();
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    let mut at = 0;
    let mut pts = 90_000;
    while at < frames.len() {
        let end = (at + 400).min(frames.len());
        mux.pes(0x101, 0xC0, pts, &frames[at..end], true);
        at = end;
        pts += 1_920;
    }
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let audio = ts.streams[0].audio.unwrap();
    assert_eq!((audio.sample_rate, audio.channels), (48_000, 2));
}

#[test]
fn streams_whose_times_never_step_forward_are_listed_whole() {
    // The same, on more streams than the budget has quarters: what
    // waits is bounded across the file, not per stream.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    let pids: Vec<u16> = (0x101..0x109).collect();
    let streams: Vec<(u8, u16)> = pids.iter().map(|pid| (0x1B, *pid)).collect();
    mux.pmt(0x1000, 1, &streams);
    let mut offsets: std::collections::HashMap<u16, Vec<u64>> = std::collections::HashMap::new();
    for _ in 0..50 {
        for pid in &pids {
            offsets.entry(*pid).or_default().push(mux.out.len() as u64);
            mux.pes(*pid, 0xE0, 90_000, &picture(None, 1_400), false);
        }
    }
    let limits = Limits {
        max_buffered_bytes: 256 << 10,
        ..Limits::default()
    };
    let ts = probe(SliceSource::new(&mux.out), &limits, &Allowlist::all()).unwrap();
    let mut listed: std::collections::HashMap<u16, Vec<u64>> = std::collections::HashMap::new();
    for packet in ts.packets(SliceSource::new(&mux.out), &limits) {
        let packet = packet.unwrap();
        let pid = ts.streams[packet.stream].pid;
        listed.entry(pid).or_default().push(packet.pes_offset);
    }
    for pid in &pids {
        assert_eq!(listed.get(pid), offsets.get(pid), "pid {pid:#x}");
    }
}

#[test]
fn a_stream_whose_times_never_step_forward_is_listed_whole() {
    // Every PES packet states the same decoding time, so the stream
    // shows no picture period and its first run never closes. The units
    // wait for one, but not past the bytes the limits allow: a file
    // this reader listed must not turn into a limit.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    let mut offsets = Vec::new();
    for _ in 0..400 {
        offsets.push(mux.out.len() as u64);
        mux.pes(0x101, 0xE0, 90_000, &picture(None, 1_400), false);
    }
    let limits = Limits {
        max_buffered_bytes: 256 << 10,
        ..Limits::default()
    };
    let ts = probe(SliceSource::new(&mux.out), &limits, &Allowlist::all()).unwrap();
    let listed: Vec<u64> = ts
        .packets(SliceSource::new(&mux.out), &limits)
        .map(|p| p.unwrap().pes_offset)
        .collect();
    assert_eq!(listed, offsets);
}

#[test]
fn a_false_sync_that_ends_on_the_real_frame_does_not_win() {
    // The worst case of a false sync: its declared length lands exactly
    // on the real frame's header, so the payload confirms it. The two
    // state different sample rates, and the stream's is the one its own
    // frames agree on.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    // 8 kHz, 36 bytes: four bytes of a frame that began before the file,
    // then a header that reads and ends where the real one starts.
    let mut first = vec![0x11, 0x22, 0x33, 0x44];
    first.extend([0xFF, 0xF1, 0x2C, 0x80, 0x04, 0x80, 0xFC]);
    first.resize(4 + 36, 0x55);
    first.extend([adts(300), adts(320), adts(300)].concat());
    mux.pes(0x101, 0xC0, 90_000, &first, true);
    mux.pes(0x101, 0xC0, 95_760, &[adts(300), adts(320)].concat(), true);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let audio = ts.streams[0].audio.unwrap();
    assert_eq!((audio.sample_rate, audio.channels), (48_000, 2));
}

#[test]
fn units_held_for_a_period_take_it_when_a_later_run_shows_one() {
    // The first run has no step to divide -- the two PES packets state
    // one time -- so its units wait; the run after them shows the
    // picture period, and they take it. A reader that lost count of
    // what waits would list them with no shift at all.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    let pair = [picture(None, 120), picture(None, 120)].concat();
    mux.pes(0x101, 0xE0, 90_000, &pair, false);
    mux.pes(0x101, 0xE0, 90_000, &pair, false);
    mux.pes(0x101, 0xE0, 93_600, &pair, false);
    mux.pes(0x101, 0xE0, 97_200, &picture(None, 120), false);
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
            // The first run, placed once the period is known.
            Some(90_000),
            Some(91_800),
            // The second, which states the same time and divides the
            // step to the third.
            Some(90_000),
            Some(91_800),
            Some(93_600),
            Some(95_400),
            Some(97_200),
        ]
    );
}

#[test]
fn what_waits_is_counted_down_as_it_is_listed() {
    // Two streams, each holding a run and then learning the period that
    // places it. The bytes of the first stream's run stop counting
    // against the bound once they are listed; a reader that kept
    // counting them would relieve the second stream's run early and
    // list it with no shift.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101), (0x1B, 0x102)]);
    let pair = [picture(None, 700), picture(None, 700)].concat();
    // Each stream holds its first run -- two PES packets of one time --
    // and the run after it gives the period that places it. The first
    // stream's packets are all delivered before the second's, so its
    // run is listed before the second's is held: a reader still
    // counting the listed bytes would relieve the second's early, with
    // no period and so with no shift.
    for pid in [0x101u16, 0x102] {
        mux.pes(pid, 0xE0, 90_000, &pair, false);
        mux.pes(pid, 0xE0, 90_000, &pair, false);
        // The second stream waits one run longer, which is where the
        // bound is tested while it still has no period.
        if pid == 0x102 {
            mux.pes(pid, 0xE0, 90_000, &pair, false);
        }
        mux.pes(pid, 0xE0, 93_600, &picture(None, 700), false);
        mux.pes(pid, 0xE0, 97_200, &picture(None, 700), false);
    }
    // A bound one run of two units is under and two are over.
    let limits = Limits {
        max_buffered_bytes: 8_000,
        ..Limits::default()
    };
    let ts = probe(SliceSource::new(&mux.out), &limits, &Allowlist::all()).unwrap();
    let mut times: Vec<(u16, Option<i64>)> = Vec::new();
    for packet in ts.packets(SliceSource::new(&mux.out), &limits) {
        let packet = packet.unwrap();
        times.push((ts.streams[packet.stream].pid, packet.pts));
    }
    let of = |pid: u16| -> Vec<Option<i64>> {
        times
            .iter()
            .filter(|(p, _)| *p == pid)
            .map(|(_, t)| *t)
            .collect()
    };
    let placed = [Some(90_000), Some(91_800)];
    assert_eq!(
        of(0x101),
        [&placed[..], &placed[..], &[Some(93_600), Some(97_200)][..]].concat()
    );
    assert_eq!(
        of(0x102),
        [
            &placed[..],
            &placed[..],
            &placed[..],
            &[Some(93_600), Some(97_200)][..]
        ]
        .concat()
    );
}

#[test]
fn as_ffprobe_the_later_units_of_a_pes_packet_take_no_time() {
    // A capture whose PES packets carry four pictures and then five: the
    // first unit of each takes its PES packet's time (13818-1 2.4.3.7),
    // and the others take none, as `ffprobe` reports on streams crafted
    // with several pictures to a PES packet. The default listing divides
    // the step between stated times among them instead
    // (`a_run_divides_the_step_that_closes_it`).
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101)]);
    let mut pts = 90_000;
    let mut expected = Vec::new();
    for pictures in [4usize, 5, 5, 4] {
        let payload: Vec<u8> = (0..pictures).flat_map(|_| picture(None, 120)).collect();
        mux.pes(0x101, 0xE0, pts, &payload, false);
        expected.push(Some(pts as i64));
        expected.extend(std::iter::repeat_n(None, pictures - 1));
        pts += 3_600 * pictures as u64;
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
        .map(|p| p.unwrap().pts)
        .collect();
    assert_eq!(times, expected);
}

#[test]
fn as_ffprobe_a_unit_is_listed_once_the_next_one_starts() {
    // Two pictures in one PES packet of the first stream: the first is
    // finished, and listed, while that packet is read, before the second
    // stream's PES packet after it; the second waits for the first
    // stream's next PES packet, whose reading gives it the stream id.
    // The default listing holds the first with the second, as a run until
    // the next stated time, and so lists it after the second stream's
    // packet.
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101), (0x1B, 0x102)]);
    let pair = [picture(None, 120), picture(None, 120)].concat();
    mux.pes(0x101, 0xE0, 90_000, &pair, false);
    mux.pes(0x102, 0xE0, 90_000, &picture(None, 120), false);
    mux.pes(0x101, 0xE0, 97_200, &picture(None, 120), false);
    mux.pes(0x102, 0xE0, 93_600, &picture(None, 120), false);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let listed: Vec<(u16, Option<i64>, bool)> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .listing_as_ffprobe()
        .map(|p| {
            let p = p.unwrap();
            (ts.streams[p.stream].pid, p.pts, p.stream_id.is_some())
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            (0x101, Some(90_000), true),
            (0x101, None, true),
            (0x102, Some(90_000), true),
            // The end of the file finishes the last units.
            (0x101, Some(97_200), true),
            (0x102, Some(93_600), true),
        ]
    );
}

#[test]
fn as_ffprobe_a_long_pes_packet_is_read_in_pieces() {
    // A PES packet of two pictures, the first longer than a piece
    // (204,800 payload bytes): the second starts in the second piece, so
    // it takes the PES packet's position, and no time. The unit before
    // the long packet is finished by its first piece, so it is listed
    // before the audio frame whose PES packet arrives while the second
    // piece does.
    let (mux, start) = long_pes_packet();
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let listed: Vec<(u16, Option<i64>, Option<u64>)> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .listing_as_ffprobe()
        .map(|p| {
            let p = p.unwrap();
            (ts.streams[p.stream].pid, p.pts, p.pos)
        })
        .collect();
    let first = listed.first().and_then(|l| l.2);
    let audio = listed.iter().find(|l| l.0 == 0x102).and_then(|l| l.2);
    assert_eq!(
        listed,
        vec![
            (0x101, Some(90_000), first),
            (0x102, Some(91_000), audio),
            (0x101, Some(93_600), Some(start)),
            (0x101, None, Some(start)),
            (0x101, Some(100_800), listed.last().and_then(|l| l.2)),
        ]
    );
}

#[test]
fn every_picture_keeps_the_identifier_of_its_own_pes_packet() {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1b, 0x101)]);
    let pair = [picture(None, 120), picture(None, 120)].concat();
    mux.pes(0x101, 0xe0, 90_000, &pair, false);
    mux.pes(0x101, 0xe1, 97_200, &picture(None, 120), false);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let ids: Vec<_> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .listing_as_ffprobe()
        .map(|p| p.unwrap().stream_id)
        .collect();
    assert_eq!(ids, [Some(0xe0), Some(0xe0), Some(0xe1)]);
}

#[test]
fn a_long_pes_packet_is_read_whole_by_default() {
    // The stream of `as_ffprobe_a_long_pes_packet_is_read_in_pieces`,
    // listed by default: the long PES packet is read once the next one
    // starts, so the unit before it waits behind the audio frame, and
    // its second picture takes no position and its share of the step to
    // the next stated time.
    let (mux, start) = long_pes_packet();
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let listed: Vec<(u16, Option<i64>, Option<u64>)> = ts
        .packets(SliceSource::new(&mux.out), &Limits::default())
        .map(|p| {
            let p = p.unwrap();
            (ts.streams[p.stream].pid, p.pts, p.pos)
        })
        .collect();
    let first = listed.iter().find(|l| l.0 == 0x101).and_then(|l| l.2);
    let audio = listed.first().and_then(|l| l.2);
    assert_eq!(
        listed,
        vec![
            (0x102, Some(91_000), audio),
            (0x101, Some(90_000), first),
            (0x101, Some(93_600), Some(start)),
            (0x101, Some(97_200), None),
            (0x101, Some(100_800), listed.last().and_then(|l| l.2)),
        ]
    );
}

/// A picture, then a PES packet of two pictures whose first is longer
/// than a piece of the listing as `ffprobe` gives it (204,800 payload
/// bytes), with an audio PES packet between its 1,200th and 1,201st
/// transport packets, then one more picture; and where the long PES
/// packet starts.
fn long_pes_packet() -> (Mux, u64) {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x101), (0x0F, 0x102)]);
    mux.pes(0x101, 0xE0, 90_000, &picture(None, 120), false);
    let start = mux.out.len() as u64;
    let mut pes = vec![0, 0, 1, 0xE0, 0, 0, 0x80, 0x80, 5];
    pes.extend(common::timestamp(93_600, 2));
    pes.extend([picture(None, 250_000), picture(None, 120)].concat());
    let (head, tail) = pes.split_at(184 * 1200);
    for (i, chunk) in head.chunks(184).enumerate() {
        mux.packet(0x101, i == 0, chunk);
    }
    mux.pes(0x102, 0xC0, 91_000, &adts(300), true);
    for chunk in tail.chunks(184) {
        mux.packet(0x101, false, chunk);
    }
    mux.pes(0x101, 0xE0, 100_800, &picture(None, 120), false);
    (mux, start)
}

/// The PIDs of a probe's streams in the order `ffprobe` numbers them
/// (`Stream::with_first_table`, then `Stream::found_at`).
fn numbered(ts: &coreav_mpegts::TransportStream) -> Vec<u16> {
    let mut streams: Vec<_> = ts.streams.iter().collect();
    streams.sort_by_key(|s| (!s.with_first_table, s.found_at, s.stream_index));
    streams.iter().map(|s| s.pid).collect()
}

#[test]
fn streams_are_numbered_as_the_file_shows_them_after_the_first_table() {
    // `ffprobe` reads up to the first Program Map Table after the PAT and
    // takes its streams, in its order; it then reads the file again from
    // the start and takes every other stream where it first shows: a PES
    // packet (padding aside), or a Program Map Table section, even one
    // that comes before the PAT (crafted files and the development set).
    let mut mux = Mux::default();
    // Program 2's audio, before any table.
    mux.pes(0x201, 0xC0, 90_000, &adts(300), true);
    // Program 3's table, before the PAT.
    mux.pmt(0x1002, 3, &[(0x0F, 0x300)]);
    // Padding on program 2's second audio PID, and on a PID no table
    // names: neither places a stream, nor lists one.
    mux.unit(0x202, &[0, 0, 1, 0xBE, 0, 4, 0xFF, 0xFF, 0xFF, 0xFF]);
    mux.unit(0x1FE, &[0, 0, 1, 0xBE, 0, 4, 0xFF, 0xFF, 0xFF, 0xFF]);
    mux.pat(&[(1, 0x1000), (2, 0x1001), (3, 0x1002)]);
    mux.pmt(0x1000, 1, &[(0x1B, 0x100), (0x0F, 0x101)]);
    mux.pes(
        0x100,
        0xE0,
        90_000,
        &picture(Some((64, 64, None)), 120),
        false,
    );
    mux.pmt(0x1001, 2, &[(0x0F, 0x200), (0x0F, 0x201), (0x0F, 0x202)]);
    mux.pmt(0x1002, 3, &[(0x0F, 0x300)]);
    for pid in [0x101, 0x202, 0x300, 0x200, 0x201] {
        mux.pes(pid, 0xC0, 90_000, &adts(300), true);
    }
    mux.pes(0x100, 0xE0, 93_600, &picture(None, 120), false);
    let ts = probe(
        SliceSource::new(&mux.out),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    assert_eq!(
        numbered(&ts),
        vec![0x100, 0x101, 0x201, 0x300, 0x200, 0x202]
    );
    let first: Vec<bool> = ts.streams.iter().map(|s| s.with_first_table).collect();
    assert_eq!(first, vec![true, true, false, false, false, false]);
}
