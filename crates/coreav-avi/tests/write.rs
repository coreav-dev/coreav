//! The written bytes, clock fields, resource bounds and callback contract.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

use coreav_avi::{Limits, MAX_FILE_BYTES, Packet, Track, plan};

fn pcm() -> Track {
    Track::Pcm {
        rate: 48000,
        channels: 1,
        bits: 8,
    }
}

fn packet(size: u32) -> Packet {
    Packet {
        track: 0,
        time: 0,
        duration: size,
        size,
        keyframe: true,
    }
}

fn number(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[test]
fn sizes_padding_index_offsets_and_sample_clocks_describe_the_bytes() {
    let tracks = [
        pcm(),
        Track::Mp3 {
            rate: 44100,
            channels: 2,
            frame_samples: 1152,
            bytes_per_second: 16000,
        },
    ];
    let packets = [
        packet(3),
        Packet {
            track: 1,
            time: 0,
            duration: 1,
            size: 5,
            keyframe: true,
        },
        Packet {
            time: 3,
            ..packet(2)
        },
    ];
    let plan = plan(&tracks, &packets, &Limits::default()).unwrap();
    let mut out = Vec::new();
    plan.write(&mut out, |i, to| {
        to.write_all(&vec![i as u8 + 1; packets[i].size as usize])
    })
    .unwrap();
    assert_eq!(&out[..4], b"RIFF");
    assert_eq!(number(&out, 4) as usize + 8, out.len());
    assert_eq!(plan.bytes() as usize, out.len());
    let movi = out.windows(4).position(|b| b == b"movi").unwrap();
    let index = out.windows(4).position(|b| b == b"idx1").unwrap();
    assert_eq!(number(&out, movi - 4) as usize + movi, index);
    assert_eq!(number(&out, index + 4), 48);
    for (i, size) in [3, 5, 2].into_iter().enumerate() {
        let entry = index + 8 + 16 * i;
        let position = movi + number(&out, entry + 8) as usize;
        assert_eq!(&out[entry..entry + 4], &out[position..position + 4]);
        assert_eq!(number(&out, entry + 12), size);
        assert_eq!(number(&out, position + 4), size);
        assert_eq!(
            &out[position + 8..position + 8 + size as usize],
            &vec![i as u8 + 1; size as usize]
        );
        if size % 2 != 0 {
            assert_eq!(out[position + 8 + size as usize], 0);
        }
    }
    assert_eq!(number(&out, index + 16), 4);
    let stream = out.windows(4).position(|b| b == b"strh").unwrap() + 8;
    assert_eq!(&out[stream..stream + 4], b"auds");
    assert_eq!(number(&out, stream + 20), 1);
    assert_eq!(number(&out, stream + 24), 48000);
    assert_eq!(number(&out, stream + 32), 5);
    assert_eq!(number(&out, stream + 44), 1);
}

#[test]
fn one_gib_is_refused_using_only_metadata() {
    let tracks = [pcm()];
    let overhead = plan(&tracks, &[packet(2)], &Limits::default())
        .unwrap()
        .bytes()
        - 2;
    let size = u32::try_from(MAX_FILE_BYTES - overhead).unwrap();
    let limits = Limits {
        max_packet_bytes: u32::MAX,
        ..Default::default()
    };
    assert!(
        plan(&tracks, &[packet(size)], &limits)
            .unwrap_err()
            .to_string()
            .contains("OpenDML")
    );
    assert_eq!(
        plan(&tracks, &[packet(size - 2)], &limits).unwrap().bytes(),
        MAX_FILE_BYTES - 2
    );
    assert!(plan(&tracks, &[packet(u32::MAX)], &limits).is_err());
}

#[test]
fn empty_video_slots_are_charged_to_the_file_limit_using_only_metadata() {
    let tracks = [Track::H264 {
        width: 16,
        height: 16,
        scale: 1,
        rate: 600,
        config: vec![1, 0, 0, 0, 0, 0, 0],
    }];
    let limits = Limits {
        max_packet_bytes: u32::MAX,
        ..Default::default()
    };
    let small = Packet {
        duration: 1,
        ..packet(2)
    };
    let overhead = plan(&tracks, &[small], &limits).unwrap().bytes() - 2;
    let near = Packet {
        size: u32::try_from(MAX_FILE_BYTES - overhead - 16 - 2).unwrap(),
        ..small
    };
    assert_eq!(
        plan(&tracks, &[near], &limits).unwrap().bytes(),
        MAX_FILE_BYTES - 16 - 2
    );
    for size in [
        near.size,
        u32::try_from(MAX_FILE_BYTES - 4 - 8 - 8).unwrap(),
    ] {
        let expanded = Packet {
            duration: 2,
            size,
            ..small
        };
        assert!(matches!(
            plan(&tracks, &[expanded], &limits),
            Err(coreav_avi::Error::Limit(
                "AVI output at or above 1 GiB needs unsupported OpenDML"
            ))
        ));
    }
}

#[test]
fn invalid_formats_gaps_and_work_budgets_fail_before_writing() {
    let packet = packet(4);
    for track in [
        Track::Pcm {
            rate: 0,
            channels: 1,
            bits: 8,
        },
        Track::Pcm {
            rate: 48000,
            channels: 0,
            bits: 8,
        },
        Track::Pcm {
            rate: 48000,
            channels: 1,
            bits: 12,
        },
        Track::Mp3 {
            rate: 44100,
            channels: 2,
            frame_samples: 1000,
            bytes_per_second: 1,
        },
    ] {
        assert!(plan(&[track], &[packet], &Limits::default()).is_err());
    }
    for p in [
        Packet { track: 1, ..packet },
        Packet { time: 1, ..packet },
        Packet {
            duration: 0,
            ..packet
        },
        Packet { size: 0, ..packet },
        Packet {
            duration: 1,
            ..packet
        },
    ] {
        assert!(plan(&[pcm()], &[p], &Limits::default()).is_err());
    }
    for limits in [
        Limits {
            max_packets: 0,
            ..Default::default()
        },
        Limits {
            max_tracks: 0,
            ..Default::default()
        },
        Limits {
            max_index_bytes: 15,
            ..Default::default()
        },
        Limits {
            max_packet_bytes: 3,
            ..Default::default()
        },
    ] {
        assert!(plan(&[pcm()], &[packet], &limits).is_err());
    }
}

#[test]
fn a_payload_cannot_be_short_long_or_an_ignored_write_failure() {
    let tracks = [pcm()];
    let packets = [packet(3)];
    let plan = plan(&tracks, &packets, &Limits::default()).unwrap();
    assert!(
        plan.write(Vec::new(), |_, to| to.write_all(&[1, 2]))
            .is_err()
    );
    assert!(
        plan.write(Vec::new(), |_, to| to.write_all(&[1, 2, 3, 4]))
            .is_err()
    );
    assert!(
        plan.write(Vec::new(), |_, _| Err(std::io::Error::other(
            "source failed"
        )))
        .is_err()
    );
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk failed"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(
        plan.write(Broken, |_, to| to.write_all(&[1, 2, 3]))
            .is_err()
    );
}

#[test]
fn h264_has_a_bitmap_header_and_a_frame_clock() {
    let tracks = [Track::H264 {
        width: 16,
        height: 16,
        scale: 1001,
        rate: 30000,
        config: vec![1, 0, 0, 0, 0, 0, 0],
    }];
    let packets = [Packet {
        duration: 1,
        size: 3,
        ..packet(3)
    }];
    let mut out = Vec::new();
    plan(&tracks, &packets, &Limits::default())
        .unwrap()
        .write(&mut out, |_, to| to.write_all(&[1, 2, 3]))
        .unwrap();
    let avih = out.windows(4).position(|b| b == b"avih").unwrap() + 8;
    assert_eq!(number(&out, avih), 33367);
    let stream = out.windows(4).position(|b| b == b"strh").unwrap() + 8;
    assert_eq!(&out[stream..stream + 8], b"vidsavc1");
    assert_eq!(number(&out, stream + 20), 1001);
    assert_eq!(number(&out, stream + 24), 30000);
    assert_eq!(number(&out, stream + 32), 1);
    assert!(out.windows(4).any(|b| b == b"00dc"));
}

#[test]
fn video_clock_slots_include_empty_chunks_in_offsets_lengths_and_budgets() {
    let tracks = [
        Track::H264 {
            width: 16,
            height: 16,
            scale: 1,
            rate: 600,
            config: vec![1, 0, 0, 0, 0, 0, 0],
        },
        pcm(),
    ];
    let packets = [
        Packet {
            duration: 24,
            size: 3,
            ..packet(3)
        },
        Packet {
            track: 1,
            ..packet(1)
        },
        Packet {
            time: 24,
            duration: 24,
            size: 2,
            keyframe: false,
            ..packet(2)
        },
    ];
    let plan = plan(&tracks, &packets, &Limits::default()).unwrap();
    let mut out = Vec::new();
    let mut calls = Vec::new();
    plan.write(&mut out, |i, to| {
        calls.push(i);
        to.write_all(&vec![1; packets[i].size as usize])
    })
    .unwrap();
    assert_eq!(calls, [0, 1, 2]);
    let movi = out.windows(4).position(|b| b == b"movi").unwrap();
    let index = out.windows(4).position(|b| b == b"idx1").unwrap();
    assert_eq!(number(&out, index + 4), 49 * 16);
    let stream = out.windows(4).position(|b| b == b"strh").unwrap() + 8;
    assert_eq!(number(&out, stream + 32), 48);
    let mut empty = 0;
    for entry in out[index + 8..].chunks_exact(16) {
        let at = movi + number(entry, 8) as usize;
        assert_eq!(&out[at..at + 4], &entry[..4]);
        assert_eq!(number(&out, at + 4), number(entry, 12));
        if number(entry, 12) == 0 {
            empty += 1;
            assert_eq!(number(entry, 4), 0);
        }
    }
    assert_eq!(empty, 46);
    assert_eq!(number(&out, index + 8 + 48 * 16 + 12), 0);
    for limits in [
        Limits {
            max_packets: 48,
            ..Default::default()
        },
        Limits {
            max_index_bytes: 49 * 16 - 1,
            ..Default::default()
        },
    ] {
        assert!(coreav_avi::plan(&tracks, &packets, &limits).is_err());
    }
    let huge = [Packet {
        duration: u32::MAX,
        ..packets[0]
    }];
    assert!(coreav_avi::plan(&tracks[..1], &huge, &Limits::default()).is_err());
}
