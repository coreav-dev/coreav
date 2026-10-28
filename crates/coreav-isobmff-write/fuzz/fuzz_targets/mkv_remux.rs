//! Arbitrary input through the Matroska-to-MP4 remux
//! (`docs/design/mkv-remux.md`): it either fails with an error or writes an
//! output that the MP4 parser accepts, with the movie box first, whose
//! streams hold every frame of the kept source streams in order, with the
//! same bytes (Annex B H.264 as 4-byte NAL unit lengths that cover each
//! sample exactly, each unit the bytes after a start code of the source
//! sample) and keyframes, and decode times in order.
#![no_main]

use coreav_isobmff::{Budget, Packet, SliceSource, Source, probe};
use coreav_isobmff_write::{MatroskaRemuxOptions, OnUnsupported, remux_matroska};
use coreav_mkv::{Allowlist, BufferedSource, Encoding, TrackKind};
use libfuzzer_sys::fuzz_target;

/// An element with a 1- or 2-octet size.
fn el(id: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    if payload.len() < 127 {
        out.push(0x80 | payload.len() as u8);
    } else {
        out.extend((0x4000 | payload.len() as u16).to_be_bytes());
    }
    out.extend_from_slice(payload);
    out
}

/// An EBML header, then a Segment of unknown size with `Info` and `Tracks`
/// (VP9, Opus, H.264 without `CodecPrivate`, AAC without `CodecPrivate`),
/// so that the input after it reaches the clusters.
fn header() -> Vec<u8> {
    let track = |number: u8, kind: u8, codec: &str, media: Vec<u8>| {
        el(
            &[0xAE],
            &[
                el(&[0xD7], &[number]),
                el(&[0x83], &[kind]),
                el(&[0x86], codec.as_bytes()),
                media,
            ]
            .concat(),
        )
    };
    let video = el(
        &[0xE0],
        &[el(&[0xB0], &[1, 0x40]), el(&[0xBA], &[0xF0])].concat(),
    );
    let audio = el(
        &[0xE1],
        &[el(&[0xB5], &48_000f32.to_be_bytes()), el(&[0x9F], &[2])].concat(),
    );
    let opus_head = el(
        &[0x63, 0xA2],
        b"OpusHead\x01\x02\x38\x01\x80\xbb\x00\x00\x00\x00\x00",
    );
    let tracks = [
        track(1, 1, "V_VP9", video.clone()),
        track(2, 2, "A_OPUS", [audio.clone(), opus_head].concat()),
        track(3, 1, "V_MPEG4/ISO/AVC", video),
        track(4, 2, "A_AAC/MPEG4/LC", audio),
    ]
    .concat();
    [
        el(
            &[0x1A, 0x45, 0xDF, 0xA3],
            &[
                el(&[0x42, 0x82], b"webm"),
                el(&[0x42, 0x87], &[4]),
                el(&[0x42, 0x85], &[2]),
            ]
            .concat(),
        ),
        vec![
            0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ],
        el(
            &[0x15, 0x49, 0xA9, 0x66],
            &el(&[0x2A, 0xD7, 0xB1], &[0x0F, 0x42, 0x40]),
        ),
        el(&[0x16, 0x54, 0xAE, 0x6B], &tracks),
    ]
    .concat()
}

fn mp4_packets(data: &[u8], movie: &coreav_isobmff::Movie, index: usize) -> Vec<Packet> {
    let limits = coreav_isobmff::Limits::default();
    let mut budget = Budget::new(&limits);
    movie
        .packets(index, SliceSource::new(data), &limits, &mut budget)
        .expect("packets")
        .map(|p| p.expect("every packet is readable"))
        .collect()
}

fn read(data: &[u8], offset: u64, size: u64) -> Vec<u8> {
    let mut buf = vec![0; size as usize];
    SliceSource::new(data)
        .read_exact_at(offset, &mut buf)
        .expect("bytes");
    buf
}

fuzz_target!(|data: &[u8]| {
    let demux = coreav_mkv::Limits {
        max_block_work: 1 << 20,
        ..coreav_mkv::Limits::default()
    };
    let mux = coreav_isobmff::Limits {
        max_table_bytes: 1 << 20,
        ..coreav_isobmff::Limits::default()
    };
    // The first byte picks the options and whether the input follows a
    // fixed header: drop or fail on unsupported streams, every stream or
    // only the one it names, and whether a lost video stream is dropped.
    let Some((&choice, rest)) = data.split_first() else {
        return;
    };
    let input = if choice & 0x80 != 0 {
        [header().as_slice(), rest].concat()
    } else {
        rest.to_vec()
    };
    let options = MatroskaRemuxOptions {
        streams: (choice & 2 != 0).then(|| vec![usize::from((choice >> 2) & 3)]),
        on_unsupported: if choice & 1 == 0 {
            OnUnsupported::Drop
        } else {
            OnUnsupported::Fail
        },
        drop_video: choice & 0x40 != 0,
    };
    let mut out = Vec::new();
    let source = BufferedSource::new(SliceSource::new(&input));
    let allowlist = Allowlist::default();
    let Ok(summary) = remux_matroska(source, &demux, &mux, &allowlist, &options, &mut out) else {
        return;
    };
    assert_eq!(summary.bytes_written, out.len() as u64);
    assert_eq!(&out[4..8], b"ftyp");
    let movie = probe(
        SliceSource::new(&out),
        &coreav_isobmff::Limits::default(),
        &coreav_isobmff::Allowlist::default(),
    )
    .expect("the output parses");
    assert_eq!(movie.tracks.len(), summary.tracks.len());
    let segment = coreav_mkv::probe(SliceSource::new(&input), &demux, &allowlist).expect("probe");
    let frames: Vec<coreav_mkv::Packet> = segment
        .packets(SliceSource::new(&input), &demux)
        .map_while(Result::ok)
        .collect();
    // Without dropping video, every video stream with frames is written.
    if !options.drop_video && options.streams.is_none() {
        for track in segment.tracks.iter().filter(|t| t.kind == TrackKind::Video) {
            if frames.iter().any(|p| p.track == track.index) {
                assert!(
                    summary
                        .tracks
                        .iter()
                        .any(|t| t.stream_index == track.stream_index)
                );
            }
        }
        assert!(!segment.dropped.iter().any(|d| d.kind == TrackKind::Video));
    }
    for (nth, kept) in summary.tracks.iter().enumerate() {
        let track = segment
            .tracks
            .iter()
            .find(|t| t.stream_index == kept.stream_index)
            .expect("the source track");
        let written = mp4_packets(&out, &movie, nth);
        let from: Vec<_> = frames.iter().filter(|p| p.track == track.index).collect();
        assert_eq!(written.len(), kept.samples as usize);
        assert_eq!(written.len(), from.len());
        let prefix = match &track.encoding {
            Encoding::HeaderStripping(prefix) => prefix.clone(),
            _ => Vec::new(),
        };
        let mut last = i64::MIN;
        for (a, b) in from.iter().zip(&written) {
            assert_eq!(b.keyframe, a.keyframe);
            assert!(b.dts > last && b.cts >= b.dts);
            last = b.dts;
            let bytes = read(&out, b.offset, u64::from(b.size));
            if kept.annex_b {
                // Each NAL unit follows a start code in the source sample,
                // in order, unchanged.
                let sample = read(&input, a.offset, a.stored_size);
                let mut at = 0usize;
                let mut cursor = 0usize;
                while at < bytes.len() {
                    let len =
                        u32::from_be_bytes(bytes[at..at + 4].try_into().expect("length")) as usize;
                    let unit = &bytes[at + 4..at + 4 + len];
                    let found = (cursor + 3..=sample.len().saturating_sub(len))
                        .find(|&p| sample[p - 3..p] == [0, 0, 1] && &sample[p..p + len] == unit)
                        .expect("each unit after a start code, in order");
                    cursor = found + len;
                    at += 4 + len;
                }
                assert_eq!(at, bytes.len());
            } else {
                let mut expected = prefix.clone();
                expected.extend(read(&input, a.offset, a.stored_size));
                assert_eq!(bytes, expected);
            }
        }
    }
});
