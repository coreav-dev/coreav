//! Matroska and WebM to MP4 round trips (`docs/design/mkv-remux.md`): every
//! output is probed by `coreav-isobmff`; each kept stream's frames must be
//! the source's frames (Annex B H.264 rewritten to NAL unit lengths), in
//! order, with the source's keyframes and presentation times, the movie box
//! first, and every stream or element left out reported.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

#[path = "../../coreav-mkv/tests/common/mod.rs"]
mod mkv;

use coreav_isobmff::{Budget, EditList, FourCc, Movie, Packet};
use coreav_isobmff_write::{
    MatroskaRemuxError, MatroskaRemuxOptions, MatroskaRemuxSummary, OnUnsupported, TrackDropReason,
    remux_matroska,
};
use coreav_mkv::{Allowlist, Codec, Encoding, SliceSource, Source};

use mkv::{
    BLOCK_GROUP, CLUSTER, INFO, TRACK_ENTRY, TRACKS, VP9_KEY_FRAME, cat, cluster, el, el_unknown,
    file, float, info, live_file, simple_block, sint, string, uint,
};

// ------------------------------------------------------------- builders

/// Why a VP8 stream is not copied.
const VP8_REASON: &str = "VP8 in MP4 does not play in Safari; keep the WebM, which plays in \
                          Chrome, Firefox and Safari, or re-encode to VP9, H.264 or AV1 outside \
                          CoreAV";

const CODEC_PRIVATE: u32 = 0x63A2;
const DEFAULT_DURATION: u32 = 0x23_E383;
const VIDEO: u32 = 0xE0;
const AUDIO: u32 = 0xE1;

/// A `TrackEntry` of `kind` (1 video, 2 audio, 17 subtitle) with `media`
/// (the `Video` or `Audio` element, or nothing) and `extra` children.
fn entry(number: u64, kind: u64, codec_id: &str, media: &[u8], extra: &[u8]) -> Vec<u8> {
    el(
        TRACK_ENTRY,
        &cat(&[
            uint(0xD7, number),
            uint(0x73C5, number * 1000),
            uint(0x83, kind),
            string(0x86, codec_id),
            media.to_vec(),
            extra.to_vec(),
        ]),
    )
}

fn video_el(width: u64, height: u64, extra: &[u8]) -> Vec<u8> {
    el(
        VIDEO,
        &cat(&[uint(0xB0, width), uint(0xBA, height), extra.to_vec()]),
    )
}

fn audio_el(rate: f64, channels: u64) -> Vec<u8> {
    el(AUDIO, &cat(&[float(0xB5, rate), uint(0x9F, channels)]))
}

fn tracks(entries: &[Vec<u8>]) -> Vec<u8> {
    el(TRACKS, &cat(entries))
}

/// `Info` with `TimestampScale` `scale`.
fn info_scale(scale: u64) -> Vec<u8> {
    el(
        INFO,
        &cat(&[uint(0x2A_D7B1, scale), string(0x4D80, "coreav-tests")]),
    )
}

/// An `OpusHead` (RFC 7845 5.1) with `pre_skip` and `channels`.
fn opus_head(channels: u8, pre_skip: u16) -> Vec<u8> {
    let mut head = b"OpusHead".to_vec();
    head.push(1);
    head.push(channels);
    head.extend(pre_skip.to_be_bytes());
    head.extend(48_000u32.to_le_bytes());
    head.extend([0, 0, 0]);
    head
}

/// A SimpleBlock with fixed-size lacing of `frames` (all the same size).
fn laced_block(track: u64, timestamp: i16, frames: &[Vec<u8>]) -> Vec<u8> {
    let mut body = vec![0x80 | track as u8];
    body.extend(timestamp.to_be_bytes());
    body.push(0x80 | 0x04);
    body.push(frames.len() as u8 - 1);
    for frame in frames {
        body.extend_from_slice(frame);
    }
    el(0xA3, &body)
}

/// A BlockGroup: a Block, then `children` (`BlockDuration`,
/// `ReferenceBlock`, `BlockAdditions`, ...).
fn block_group(track: u64, timestamp: i16, frame: &[u8], children: &[Vec<u8>]) -> Vec<u8> {
    let mut body = vec![0x80 | track as u8];
    body.extend(timestamp.to_be_bytes());
    body.push(0);
    body.extend_from_slice(frame);
    el(
        BLOCK_GROUP,
        &cat(&[vec![el(0xA1, &body)], children.to_vec()].concat()),
    )
}

/// Frame bytes that differ per frame.
fn frame(seed: usize, len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 + seed * 7 + 1) as u8).collect()
}

/// A high-profile H.264 sequence parameter set: profile_idc 100, 4:2:0,
/// 8-bit, level 3.0.
const SPS: [u8; 10] = [0x67, 100, 0, 30, 0xAC, 0xD9, 0x40, 0x50, 0x05, 0xBB];
const PPS: [u8; 4] = [0x68, 0xEB, 0xE3, 0xCB];

/// The `avcC` of [`SPS`] and [`PPS`].
fn avcc() -> Vec<u8> {
    let mut record = vec![1, 100, 0, 30, 0xFF, 0xE1];
    record.extend((SPS.len() as u16).to_be_bytes());
    record.extend(SPS);
    record.push(1);
    record.extend((PPS.len() as u16).to_be_bytes());
    record.extend(PPS);
    record.extend([0xFD, 0xF8, 0xF8, 0]);
    record
}

// ------------------------------------------------------------- checks

fn remux_with(
    source: &[u8],
    allowlist: &Allowlist,
    options: &MatroskaRemuxOptions,
) -> Result<(MatroskaRemuxSummary, Vec<u8>), MatroskaRemuxError> {
    let mut out = Vec::new();
    let summary = remux_matroska(
        SliceSource::new(source),
        &coreav_mkv::Limits::default(),
        &coreav_isobmff::Limits::default(),
        allowlist,
        options,
        &mut out,
    )?;
    assert_eq!(summary.bytes_written, out.len() as u64);
    Ok((summary, out))
}

fn remux(source: &[u8]) -> (MatroskaRemuxSummary, Vec<u8>) {
    remux_with(
        source,
        &Allowlist::default(),
        &MatroskaRemuxOptions::default(),
    )
    .unwrap()
}

/// Like [`remux`], dropping a video stream that cannot be copied.
fn remux_dropping_video(source: &[u8]) -> (MatroskaRemuxSummary, Vec<u8>) {
    let options = MatroskaRemuxOptions {
        drop_video: true,
        ..MatroskaRemuxOptions::default()
    };
    remux_with(source, &Allowlist::default(), &options).unwrap()
}

/// The remux with the default options fails because video stream
/// `stream_index` cannot be copied, for `reason`.
fn assert_video_not_copied(source: &[u8], stream_index: usize, reason: &str) {
    match remux_with(
        source,
        &Allowlist::default(),
        &MatroskaRemuxOptions::default(),
    ) {
        Err(MatroskaRemuxError::VideoNotCopied {
            stream_index: index,
            reason: why,
        }) => assert_eq!((index, why), (stream_index, reason)),
        other => panic!("{other:?}"),
    }
}

fn mp4_packets(data: &[u8], movie: &Movie, index: usize) -> Vec<Packet> {
    let limits = coreav_isobmff::Limits::default();
    let mut budget = Budget::new(&limits);
    movie
        .packets(index, SliceSource::new(data), &limits, &mut budget)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn read(data: &[u8], offset: u64, size: u64) -> Vec<u8> {
    let mut buf = vec![0; size as usize];
    SliceSource::new(data)
        .read_exact_at(offset, &mut buf)
        .unwrap();
    buf
}

/// Top-level box types, in order.
fn top_level(data: &[u8]) -> Vec<[u8; 4]> {
    let mut at = 0;
    let mut kinds = Vec::new();
    while at + 8 <= data.len() {
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        kinds.push(data[at + 4..at + 8].try_into().unwrap());
        at += size.max(8);
    }
    kinds
}

/// An Annex B access unit with 4-byte NAL unit lengths instead of start
/// codes.
fn length_prefixed(annex_b: &[u8]) -> Vec<u8> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= annex_b.len() {
        if annex_b[i..i + 3] == [0, 0, 1] {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (n, &start) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(annex_b.len(), |next| next - 3);
        while end > start && annex_b[end - 1] == 0 {
            end -= 1;
        }
        out.extend(((end - start) as u32).to_be_bytes());
        out.extend_from_slice(&annex_b[start..end]);
    }
    out
}

/// The output has the movie box first, and each kept stream has the
/// source's frames, keyframes and presentation times (exact for video, to
/// half the codec's frame for audio), with decode times in order. Returns
/// the output movie.
fn assert_round_trip(source: &[u8], output: &[u8], summary: &MatroskaRemuxSummary) -> Movie {
    assert_eq!(top_level(output), [*b"ftyp", *b"moov", *b"mdat"]);
    let segment = coreav_mkv::probe(
        SliceSource::new(source),
        &coreav_mkv::Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let mut frames = Vec::new();
    for packet in segment.packets(SliceSource::new(source), &coreav_mkv::Limits::default()) {
        match packet {
            Ok(packet) => frames.push(packet),
            Err(coreav_mkv::Error::Truncated { .. }) => break,
            Err(error) => panic!("{error}"),
        }
    }
    let out = coreav_isobmff::probe(
        SliceSource::new(output),
        &coreav_isobmff::Limits::default(),
        &coreav_isobmff::Allowlist::default(),
    )
    .unwrap();
    assert_eq!(out.tracks.len(), summary.tracks.len());
    for (i, kept) in summary.tracks.iter().enumerate() {
        let track = segment
            .tracks
            .iter()
            .find(|t| t.stream_index == kept.stream_index)
            .unwrap();
        let o = &out.tracks[i];
        assert_eq!(o.track_id, kept.track_id);
        assert_eq!(o.timescale, kept.timescale);
        assert_ne!(o.edit_list, EditList::Unsupported);
        let from: Vec<_> = frames.iter().filter(|p| p.track == track.index).collect();
        let to = mp4_packets(output, &out, o.index);
        assert_eq!(to.len(), from.len());
        assert_eq!(kept.samples as usize, from.len());
        assert_eq!(
            kept.duration,
            to.iter().map(|p| u64::from(p.duration)).sum()
        );
        let prefix = match &track.encoding {
            Encoding::HeaderStripping(prefix) => prefix.clone(),
            _ => Vec::new(),
        };
        let tolerance = if track.audio.is_some() {
            let frame = if track.codec == Codec::Opus {
                1_250_000
            } else {
                12_000_000
            };
            1_000_000 + frame
        } else {
            1_000_000_000 / (2 * i128::from(o.timescale))
        };
        let mut last_dts = i64::MIN;
        for (a, b) in from.iter().zip(&to) {
            let mut bytes = prefix.clone();
            bytes.extend(read(source, a.offset, a.stored_size));
            if kept.annex_b {
                bytes = length_prefixed(&bytes);
            }
            assert_eq!(read(output, b.offset, u64::from(b.size)), bytes);
            assert_eq!(b.keyframe, a.keyframe);
            assert!(b.dts > last_dts && b.cts >= b.dts, "{b:?}");
            last_dts = b.dts;
            if let (Some(expected), Some(pts)) = (a.pts_ns(&segment), b.pts) {
                let ns = pts.num * 1_000_000_000 / (i128::from(pts.den) * i128::from(o.timescale));
                assert!(
                    (ns - expected).abs() <= tolerance,
                    "track {} sample {}: {ns} ns, source {expected} ns",
                    kept.stream_index,
                    b.sample
                );
            }
        }
    }
    out
}

fn reasons(summary: &MatroskaRemuxSummary) -> Vec<(usize, TrackDropReason)> {
    summary
        .dropped_tracks
        .iter()
        .map(|d| (d.stream_index, d.reason))
        .collect()
}

fn unsupported(reason: &'static str) -> TrackDropReason {
    TrackDropReason::Unsupported { reason }
}

// ------------------------------------------------------------- tests

#[test]
fn vp9_and_opus_from_webm() {
    let source = mkv::small_webm();
    let (summary, output) = remux(&source);
    let out = assert_round_trip(&source, &output, &summary);
    assert_eq!(summary.tracks.len(), 2);
    assert_eq!(summary.tracks[0].codec, Codec::Vp9);
    assert_eq!(summary.tracks[0].timescale, 1000);
    assert_eq!(summary.tracks[1].codec, Codec::Opus);
    assert_eq!(summary.tracks[1].timescale, 48_000);
    assert!(summary.dropped_tracks.is_empty());
    assert_eq!(summary.truncated_at, None);
    let video = &out.tracks[0].description;
    assert_eq!(video.fourcc, FourCc(*b"vp09"));
    assert_eq!((video.width, video.height), (Some(320), Some(240)));
    let vpcc = video.codec_config.as_ref().unwrap();
    assert_eq!(vpcc.kind, FourCc(*b"vpcC"));
    // Version 1; profile 0, level 0, 8-bit 4:2:0 (colocated with luma
    // unknown: vertical), BT.709 from the frame header's color space.
    assert_eq!(vpcc.data[..4], [1, 0, 0, 0]);
    assert_eq!(vpcc.data[4], 0, "profile");
    assert_eq!(vpcc.data[6] >> 4, 8, "bit depth");
    let audio = &out.tracks[1].description;
    assert_eq!(audio.fourcc, FourCc(*b"Opus"));
    assert_eq!(audio.channels, Some(2));
    let dops = audio.codec_config.as_ref().unwrap();
    assert_eq!(dops.kind, FourCc(*b"dOps"));
    // Version 0, 2 channels, pre-skip 312, 48 kHz, gain 0, family 0.
    assert_eq!(dops.data, [0, 2, 1, 0x38, 0, 0, 0xBB, 0x80, 0, 0, 0]);
    // CodecDelay (6.5 ms) is the media edit's start.
    let EditList::Simple { media_time, .. } = out.tracks[1].edit_list else {
        panic!("{:?}", out.tracks[1].edit_list);
    };
    assert_eq!(media_time, 312);
    // The same input gives the same output.
    assert_eq!(remux(&source).1, output);
}

/// Named streams are written in the order named, a stream named twice as
/// two tracks with the same frames, as `ffmpeg` writes its `-map`s.
#[test]
fn named_streams_keep_their_order_and_repeats() {
    let source = mkv::small_webm();
    let options = MatroskaRemuxOptions {
        streams: Some(vec![1, 0, 0]),
        ..MatroskaRemuxOptions::default()
    };
    let (summary, output) = remux_with(&source, &Allowlist::default(), &options).unwrap();
    assert_round_trip(&source, &output, &summary);
    assert_eq!(
        summary
            .tracks
            .iter()
            .map(|t| (t.stream_index, t.track_id))
            .collect::<Vec<_>>(),
        [(1, 1), (0, 2), (0, 3)]
    );
    assert!(summary.dropped_tracks.is_empty());
}

#[test]
fn h264_b_frames_get_decode_times() {
    let presentation = [0i16, 120, 40, 80, 240, 160, 200, 280];
    let blocks: Vec<Vec<u8>> = presentation
        .iter()
        .enumerate()
        .map(|(n, &t)| simple_block(1, t, n == 0, &frame(n, 30 + n)))
        .collect();
    let source = file(
        "matroska",
        &[
            info(None),
            tracks(&[entry(
                1,
                1,
                "V_MPEG4/ISO/AVC",
                &video_el(64, 48, &[]),
                &cat(&[
                    el(CODEC_PRIVATE, &avcc()),
                    uint(DEFAULT_DURATION, 40_000_000),
                ]),
            )]),
            cluster(1000, &blocks),
        ],
    );
    let (summary, output) = remux(&source);
    let out = assert_round_trip(&source, &output, &summary);
    let track = &out.tracks[0];
    assert_eq!(track.description.fourcc, FourCc(*b"avc1"));
    assert_eq!(
        track.description.codec_config.as_ref().unwrap().data,
        avcc()
    );
    let packets = mp4_packets(&output, &out, 0);
    // Decode times are the presentation times sorted and shifted back by
    // the largest reordering (40 ms); the composition offsets make up the
    // difference.
    let dts: Vec<i64> = packets.iter().map(|p| p.dts).collect();
    assert_eq!(dts, [0, 40, 80, 120, 160, 200, 240, 280]);
    let cts: Vec<i64> = packets.iter().map(|p| p.cts).collect();
    assert_eq!(cts, [40, 160, 80, 120, 280, 200, 240, 320]);
    // The track starts at the cluster's 1 s: an empty edit, then the media
    // from its first presented frame.
    let EditList::Simple {
        empty,
        media_time,
        segment,
    } = track.edit_list
    else {
        panic!("{:?}", track.edit_list);
    };
    // Every frame is presented: 8 frames of 40 ms from media time 40.
    assert_eq!((empty, media_time, segment), (1000, 40, 320));
    // The last frame lasts DefaultDuration.
    assert_eq!(packets[7].duration, 40);
}

#[test]
fn annex_b_h264_is_rewritten_to_nal_unit_lengths() {
    // Chrome's MediaRecorder: no CodecPrivate, parameter sets in band.
    let key = |n: usize| {
        cat(&[
            vec![0, 0, 0, 1],
            SPS.to_vec(),
            vec![0, 0, 0, 1],
            PPS.to_vec(),
            vec![0, 0, 1, 0x65],
            // An emulation prevention byte stays as it is.
            vec![0x88, 0, 0, 3, 1],
            frame(n, 20),
        ])
    };
    let delta = |n: usize| cat(&[vec![0, 0, 0, 1, 0x41], frame(n, 10 + n), vec![0]]);
    let blocks = vec![
        simple_block(1, 0, true, &key(0)),
        simple_block(1, 33, false, &delta(1)),
        simple_block(1, 66, false, &delta(2)),
        simple_block(1, 100, true, &key(3)),
    ];
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[entry(1, 1, "V_MPEG4/ISO/AVC", &video_el(64, 48, &[]), &[])]),
            cluster(0, &blocks),
        ],
    );
    let (summary, output) = remux(&source);
    assert!(summary.tracks[0].annex_b);
    let out = assert_round_trip(&source, &output, &summary);
    // The avcC holds the first keyframe's parameter sets, with the
    // high-profile chroma format and bit depths.
    let config = out.tracks[0].description.codec_config.as_ref().unwrap();
    assert_eq!(config.kind, FourCc(*b"avcC"));
    assert_eq!(config.data, avcc());
}

#[test]
fn annex_b_h264_without_parameter_sets_is_dropped() {
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[
                entry(1, 1, "V_MPEG4/ISO/AVC", &video_el(64, 48, &[]), &[]),
                entry(
                    2,
                    2,
                    "A_OPUS",
                    &audio_el(48_000.0, 1),
                    &el(CODEC_PRIVATE, &opus_head(1, 0)),
                ),
            ]),
            cluster(
                0,
                &[
                    simple_block(1, 0, true, &[0, 0, 0, 1, 0x65, 1, 2, 3]),
                    simple_block(2, 0, true, &[0xF8, 1]),
                ],
            ),
        ],
    );
    assert_video_not_copied(&source, 0, "the first H.264 keyframe has no parameter sets");
    let (summary, output) = remux_dropping_video(&source);
    assert_round_trip(&source, &output, &summary);
    assert_eq!(summary.tracks.len(), 1);
    assert_eq!(
        reasons(&summary),
        [(
            0,
            unsupported("the first H.264 keyframe has no parameter sets")
        )]
    );
}

#[test]
fn hevc_and_av1_keep_their_records() {
    let hvcc = cat(&[vec![
        1, 1, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 93, 0xF0, 0, 0xFC, 0xFD, 0xF8, 0xF8, 0, 0, 0x0F,
        0,
    ]]);
    let av1c = vec![0x81, 0x04, 0x0C, 0x00];
    for (codec_id, record, fourcc, kind) in [
        ("V_MPEGH/ISO/HEVC", hvcc, *b"hev1", *b"hvcC"),
        ("V_AV1", av1c, *b"av01", *b"av1C"),
    ] {
        let blocks: Vec<Vec<u8>> = (0..5)
            .map(|n| simple_block(1, n as i16 * 40, n == 0, &frame(n, 25)))
            .collect();
        let source = file(
            "matroska",
            &[
                info(None),
                tracks(&[entry(
                    1,
                    1,
                    codec_id,
                    &video_el(1920, 1080, &[]),
                    &el(CODEC_PRIVATE, &record),
                )]),
                cluster(0, &blocks),
            ],
        );
        let (summary, output) = remux(&source);
        let out = assert_round_trip(&source, &output, &summary);
        let description = &out.tracks[0].description;
        assert_eq!(description.fourcc, FourCc(fourcc), "{codec_id}");
        let config = description.codec_config.as_ref().unwrap();
        assert_eq!(config.kind, FourCc(kind));
        assert_eq!(config.data, record);
        // No reordering: no edit list.
        assert_eq!(out.tracks[0].edit_list, EditList::None);
    }
}

#[test]
fn aac_from_a_legacy_codec_id_with_laced_frames() {
    // Four 1,024-sample frames per block at 44.1 kHz, without
    // DefaultDuration: the later frames of each lace have no timestamp.
    let blocks: Vec<Vec<u8>> = (0..5)
        .map(|b| {
            let frames: Vec<Vec<u8>> = (0..4).map(|n| frame(b * 4 + n, 12)).collect();
            // 4,096 samples are 92.88 ms; stored to the millisecond.
            laced_block(1, (b as f64 * 4096.0 / 44.1).round() as i16, &frames)
        })
        .collect();
    let source = file(
        "matroska",
        &[
            info(None),
            tracks(&[entry(1, 2, "A_AAC/MPEG4/LC", &audio_el(44_100.0, 2), &[])]),
            cluster(0, &blocks),
        ],
    );
    let (summary, output) = remux(&source);
    let out = assert_round_trip(&source, &output, &summary);
    assert_eq!(summary.tracks[0].samples, 20);
    assert_eq!(summary.tracks[0].timescale, 44_100);
    let packets = mp4_packets(&output, &out, 0);
    assert!(packets.iter().all(|p| p.duration == 1024), "{packets:?}");
    let description = &out.tracks[0].description;
    assert_eq!(description.fourcc, FourCc(*b"mp4a"));
    assert_eq!(description.sample_rate, Some(44_100));
    assert_eq!(description.channels, Some(2));
    // AAC LC (2), 44.1 kHz (4), 2 channels.
    let esds = &description.codec_config.as_ref().unwrap().data;
    assert!(esds.windows(2).any(|w| w == [0x12, 0x10]), "{esds:02x?}");
}

#[test]
fn streams_mp4_cannot_carry_are_dropped_with_a_reason() {
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[
                entry(1, 1, "V_VP9", &video_el(320, 240, &[]), &[]),
                entry(
                    2,
                    2,
                    "A_VORBIS",
                    &audio_el(48_000.0, 2),
                    &el(CODEC_PRIVATE, &[2, 1, 1, 0, 0, 0]),
                ),
                entry(3, 17, "D_WEBVTT/SUBTITLES", &[], &[]),
            ]),
            cluster(
                0,
                &[
                    simple_block(1, 0, true, &VP9_KEY_FRAME),
                    simple_block(2, 0, true, &[1, 2, 3]),
                    simple_block(3, 0, true, b"hello"),
                    simple_block(1, 40, false, &[0b1000_0110, 0, 0]),
                ],
            ),
        ],
    );
    let (summary, output) = remux(&source);
    assert_round_trip(&source, &output, &summary);
    assert_eq!(summary.tracks.len(), 1);
    assert_eq!(
        reasons(&summary),
        [
            (
                1,
                unsupported("MP4 has no standard sample entry for Vorbis")
            ),
            (2, unsupported("subtitle streams are not copied to MP4")),
        ]
    );
    // Failing instead of dropping.
    let fail = MatroskaRemuxOptions {
        on_unsupported: OnUnsupported::Fail,
        ..MatroskaRemuxOptions::default()
    };
    assert!(matches!(
        remux_with(&source, &Allowlist::default(), &fail),
        Err(MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported {
            reason: "MP4 has no standard sample entry for Vorbis"
        }))
    ));
    // Naming streams: the others are not selected; a named stream that
    // cannot be copied fails.
    let only_video = MatroskaRemuxOptions {
        streams: Some(vec![0]),
        ..MatroskaRemuxOptions::default()
    };
    let (summary, _) = remux_with(&source, &Allowlist::default(), &only_video).unwrap();
    assert_eq!(
        reasons(&summary),
        [
            (1, TrackDropReason::NotSelected),
            (2, TrackDropReason::NotSelected)
        ]
    );
    let vorbis = MatroskaRemuxOptions {
        streams: Some(vec![1]),
        ..MatroskaRemuxOptions::default()
    };
    assert!(remux_with(&source, &Allowlist::default(), &vorbis).is_err());
    let missing = MatroskaRemuxOptions {
        streams: Some(vec![7]),
        ..MatroskaRemuxOptions::default()
    };
    assert!(remux_with(&source, &Allowlist::default(), &missing).is_err());
    // Codecs outside the allowlist are reported as such.
    let (summary, _) = remux_with(
        &source,
        &Allowlist::new(&[Codec::Vp9]),
        &MatroskaRemuxOptions::default(),
    )
    .unwrap();
    assert_eq!(
        reasons(&summary),
        [
            (1, TrackDropReason::CodecNotAllowed),
            (2, TrackDropReason::CodecNotAllowed)
        ]
    );
}

#[test]
fn a_file_with_nothing_to_copy_fails_with_the_first_reason() {
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[
                entry(1, 1, "V_VP8", &video_el(320, 240, &[]), &[]),
                entry(
                    2,
                    2,
                    "A_VORBIS",
                    &audio_el(48_000.0, 2),
                    &el(CODEC_PRIVATE, &[2, 1, 1, 0, 0, 0]),
                ),
            ]),
            cluster(
                0,
                &[simple_block(
                    1,
                    0,
                    true,
                    &[0x10, 0x02, 0, 0x9d, 0x01, 0x2a, 0x40, 0x01, 0xf0, 0],
                )],
            ),
        ],
    );
    // With or without dropping video, nothing is left to write: the video
    // stream's reason fails the remux.
    assert!(matches!(
        remux_with(
            &source,
            &Allowlist::default(),
            &MatroskaRemuxOptions::default()
        ),
        Err(MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported {
            reason: VP8_REASON
        }))
    ));
    let mut out = Vec::new();
    let result = remux_matroska(
        SliceSource::new(&source),
        &coreav_mkv::Limits::default(),
        &coreav_isobmff::Limits::default(),
        &Allowlist::default(),
        &MatroskaRemuxOptions {
            drop_video: true,
            ..MatroskaRemuxOptions::default()
        },
        &mut out,
    );
    assert!(matches!(
        result,
        Err(MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported {
            reason: VP8_REASON
        }))
    ));
    assert!(out.is_empty());
}

#[test]
fn encrypted_and_header_stripped_streams() {
    let encodings = |encoding: Vec<u8>| el(0x6D80, &el(0x6240, &encoding));
    let stripped = encodings(cat(&[
        uint(0x5032, 1),
        el(0x5034, &cat(&[uint(0x4254, 3), el(0x4255, &[0xFC])])),
    ]));
    let encrypted = encodings(cat(&[
        uint(0x5033, 1),
        el(0x5035, &cat(&[uint(0x47E1, 5), el(0x47E2, &[1; 16])])),
    ]));
    let opus = |number, extra: &[u8]| {
        entry(
            number,
            2,
            "A_OPUS",
            &audio_el(48_000.0, 2),
            &cat(&[el(CODEC_PRIVATE, &opus_head(2, 0)), extra.to_vec()]),
        )
    };
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[opus(1, &stripped), opus(2, &encrypted)]),
            cluster(
                0,
                &[
                    simple_block(1, 0, true, &[1, 2]),
                    simple_block(2, 0, true, &[1, 2]),
                    simple_block(1, 20, true, &[3, 4]),
                ],
            ),
        ],
    );
    let (summary, output) = remux(&source);
    let out = assert_round_trip(&source, &output, &summary);
    assert_eq!(
        reasons(&summary),
        [(1, unsupported("encrypted streams are not copied"))]
    );
    // The stripped header byte is back in front of each frame.
    let packets = mp4_packets(&output, &out, 0);
    assert_eq!(read(&output, packets[1].offset, 3), [0xFC, 3, 4]);
}

#[test]
fn block_additions_are_reported() {
    // A VP9 stream with an alpha plane in BlockAdditions, as Chrome
    // records transparent video.
    let addition = el(
        0x75A1,
        &el(0xA6, &cat(&[uint(0xEE, 1), el(0xA5, &[9, 9, 9])])),
    );
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[entry(
                1,
                1,
                "V_VP9",
                &video_el(320, 240, &uint(0x53C0, 1)),
                &[],
            )]),
            cluster(
                0,
                &[
                    block_group(1, 0, &VP9_KEY_FRAME, std::slice::from_ref(&addition)),
                    block_group(
                        1,
                        40,
                        &[0b1000_0110, 0, 0],
                        &[sint_reference(-40), addition.clone()],
                    ),
                ],
            ),
        ],
    );
    let (summary, output) = remux(&source);
    assert_round_trip(&source, &output, &summary);
    assert_eq!(summary.dropped_additions.len(), 2);
    let first = summary.dropped_additions[0];
    assert_eq!(first.stream_index, 0);
    assert_eq!(first.size, addition.len() as u64);
    assert_eq!(read(&source, first.offset, first.size), addition);
    assert_eq!(summary.dropped_additions_not_listed, 0);
}

fn sint_reference(value: i64) -> Vec<u8> {
    el(0xFB, &[value as u8])
}

#[test]
fn a_cut_off_recording_keeps_its_complete_frames() {
    // A live recording (unknown sizes) cut off inside its last block; 40 ms
    // Opus frames (TOC configuration 2).
    let blocks: Vec<Vec<u8>> = (0..6)
        .flat_map(|n| {
            [
                simple_block(1, n as i16 * 40, n % 3 == 0, &frame_vp9(n)),
                simple_block(2, n as i16 * 40, true, &cat(&[vec![0x10], frame(n, 40)])),
            ]
        })
        .collect();
    let whole = live_file(
        "webm",
        &[
            info(None),
            tracks(&[
                entry(1, 1, "V_VP9", &video_el(320, 240, &[]), &[]),
                entry(
                    2,
                    2,
                    "A_OPUS",
                    &audio_el(48_000.0, 2),
                    &el(CODEC_PRIVATE, &opus_head(2, 0)),
                ),
            ]),
            el_unknown(CLUSTER, &cat(&[vec![uint(0xE7, 0)], blocks].concat())),
        ],
    );
    let cut = &whole[..whole.len() - 20];
    let (summary, output) = remux(cut);
    assert!(summary.truncated_at.is_some());
    assert_round_trip(cut, &output, &summary);
    assert_eq!(summary.tracks[0].samples, 6);
    assert_eq!(summary.tracks[1].samples, 5);
    // Cut off before any frame: nothing to copy.
    let header_only = &whole[..whole.len() - blocks_len(&whole) + 4];
    // A VP9 stream without frames loses nothing; with or without
    // dropping video, the truncation is all that is left.
    assert!(matches!(
        remux_with(
            header_only,
            &Allowlist::default(),
            &MatroskaRemuxOptions::default()
        ),
        Err(MatroskaRemuxError::Source(
            coreav_mkv::Error::Truncated { .. }
        ))
    ));
    let options = MatroskaRemuxOptions {
        drop_video: true,
        ..MatroskaRemuxOptions::default()
    };
    assert!(matches!(
        remux_with(header_only, &Allowlist::default(), &options),
        Err(MatroskaRemuxError::Source(
            coreav_mkv::Error::Truncated { .. }
        ))
    ));
}

/// A VP9 frame: a key frame header for frames 0 and 3, else an inter frame.
fn frame_vp9(n: usize) -> Vec<u8> {
    if n % 3 == 0 {
        VP9_KEY_FRAME.to_vec()
    } else {
        cat(&[vec![0b1000_0110], frame(n, 10)])
    }
}

/// Bytes of a live file from the first block to the end.
fn blocks_len(file: &[u8]) -> usize {
    let at = file
        .windows(4)
        .position(|w| w == [0x1F, 0x43, 0xB6, 0x75])
        .unwrap();
    // Cluster ID, 8-octet unknown size, Timestamp element.
    file.len() - (at + 4 + 8 + 3)
}

#[test]
fn rotation_language_and_display_size() {
    let projection = el(0x7670, &float(0x7675, -90.0));
    let source = file(
        "matroska",
        &[
            info(None),
            tracks(&[
                entry(
                    1,
                    1,
                    "V_VP9",
                    &video_el(
                        320,
                        240,
                        &cat(&[uint(0x54B0, 640), uint(0x54BA, 240), projection]),
                    ),
                    &string(0x22_B59C, "ger"),
                ),
                entry(
                    2,
                    2,
                    "A_OPUS",
                    &audio_el(48_000.0, 2),
                    &cat(&[
                        el(CODEC_PRIVATE, &opus_head(2, 0)),
                        string(0x22_B59C, "fre"),
                    ]),
                ),
            ]),
            cluster(
                0,
                &[
                    simple_block(1, 0, true, &VP9_KEY_FRAME),
                    simple_block(2, 0, true, &[1]),
                ],
            ),
        ],
    );
    let (summary, output) = remux(&source);
    let out = assert_round_trip(&source, &output, &summary);
    let video = &out.tracks[0];
    assert_eq!(video.language, Some(*b"deu"));
    assert_eq!(out.tracks[1].language, Some(*b"fra"));
    // A roll of -90 degrees is a clockwise rotation by 90 degrees.
    assert_eq!(video.matrix[..2], [0, 0x0001_0000]);
    assert_eq!(video.matrix[3..5], [-0x0001_0000, 0]);
    // Display size 640x240: a pixel aspect ratio of 2:1.
    assert_eq!(video.presentation_size, (640 << 16, 240 << 16));
    assert_eq!(video.description.pixel_aspect, Some((2, 1)));
}

#[test]
fn other_timestamp_scales() {
    for (scale, timescale) in [(100_000u64, 10_000u32), (333_333, 90_000)] {
        let blocks: Vec<Vec<u8>> = (0..4)
            .map(|n| simple_block(1, n as i16 * 300, n == 0, &frame_vp9(n * 3)))
            .collect();
        let source = file(
            "webm",
            &[
                info_scale(scale),
                tracks(&[entry(1, 1, "V_VP9", &video_el(320, 240, &[]), &[])]),
                cluster(0, &blocks),
            ],
        );
        let (summary, output) = remux(&source);
        assert_eq!(summary.tracks[0].timescale, timescale);
        assert_round_trip(&source, &output, &summary);
    }
}

#[test]
fn limits_bound_the_sample_records() {
    let source = mkv::small_webm();
    let run = |demux: coreav_mkv::Limits, mux: coreav_isobmff::Limits| {
        remux_matroska(
            SliceSource::new(&source),
            &demux,
            &mux,
            &Allowlist::default(),
            // A frame above the packet limit leaves the VP9 stream without
            // a readable first frame: drop it, so that the limit is reached.
            &MatroskaRemuxOptions {
                drop_video: true,
                ..MatroskaRemuxOptions::default()
            },
            &mut Vec::new(),
        )
    };
    assert!(matches!(
        run(
            coreav_mkv::Limits::default(),
            coreav_isobmff::Limits {
                max_table_bytes: 100,
                ..coreav_isobmff::Limits::default()
            }
        ),
        Err(MatroskaRemuxError::Output(
            coreav_isobmff::Error::LimitExceeded {
                limit: coreav_isobmff::LimitKind::TableBytes,
                ..
            }
        ))
    ));
    assert!(matches!(
        run(
            coreav_mkv::Limits {
                max_packet_bytes: 2,
                ..coreav_mkv::Limits::default()
            },
            coreav_isobmff::Limits::default()
        ),
        Err(MatroskaRemuxError::Source(
            coreav_mkv::Error::LimitExceeded { .. }
        ))
    ));
}

#[test]
fn write_errors_are_reported() {
    struct Full;
    impl std::io::Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::StorageFull.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let source = mkv::small_webm();
    let result = remux_matroska(
        SliceSource::new(&source),
        &coreav_mkv::Limits::default(),
        &coreav_isobmff::Limits::default(),
        &Allowlist::default(),
        &MatroskaRemuxOptions::default(),
        Full,
    );
    assert!(matches!(
        result,
        Err(MatroskaRemuxError::Write(std::io::ErrorKind::StorageFull))
    ));
}

#[test]
fn repeated_presentation_times_still_decode_in_order() {
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[entry(1, 1, "V_VP9", &video_el(320, 240, &[]), &[])]),
            cluster(
                0,
                &[
                    simple_block(1, 0, true, &VP9_KEY_FRAME),
                    simple_block(1, 0, false, &frame_vp9(1)),
                    simple_block(1, 40, false, &frame_vp9(2)),
                ],
            ),
        ],
    );
    let (summary, output) = remux(&source);
    let out = assert_round_trip(&source, &output, &summary);
    let packets = mp4_packets(&output, &out, 0);
    let times: Vec<(i64, i64)> = packets.iter().map(|p| (p.dts, p.cts)).collect();
    assert_eq!(times, [(0, 1), (1, 1), (40, 41)]);
}

/// An H.264 Annex B keyframe with `sps`, [`PPS`] and an IDR slice.
fn annex_b_key(sps: &[u8], n: usize) -> Vec<u8> {
    cat(&[
        vec![0, 0, 0, 1],
        sps.to_vec(),
        vec![0, 0, 0, 1],
        PPS.to_vec(),
        vec![0, 0, 1, 0x65],
        frame(n, 20),
    ])
}

#[test]
fn annex_b_parameter_set_changes_use_avc3() {
    // A resolution change: the second keyframe has another SPS.
    let mut other = SPS.to_vec();
    other[9] ^= 0x10;
    for (codec_private, second, fourcc) in [
        (Vec::new(), SPS.to_vec(), *b"avc1"),
        (Vec::new(), other.clone(), *b"avc3"),
        // An empty CodecPrivate is no CodecPrivate.
        (el(CODEC_PRIVATE, &[]), other, *b"avc3"),
    ] {
        let source = file(
            "webm",
            &[
                info(None),
                tracks(&[entry(
                    1,
                    1,
                    "V_MPEG4/ISO/AVC",
                    &video_el(64, 48, &[]),
                    &codec_private,
                )]),
                cluster(
                    0,
                    &[
                        simple_block(1, 0, true, &annex_b_key(&SPS, 0)),
                        simple_block(1, 40, true, &annex_b_key(&second, 1)),
                    ],
                ),
            ],
        );
        let (summary, output) = remux(&source);
        assert!(summary.tracks[0].annex_b);
        let out = assert_round_trip(&source, &output, &summary);
        assert_eq!(out.tracks[0].description.fourcc, FourCc(fourcc));
        // The avcC holds the first keyframe's parameter sets.
        assert_eq!(
            out.tracks[0]
                .description
                .codec_config
                .as_ref()
                .unwrap()
                .data,
            avcc()
        );
    }
}

#[test]
fn streams_with_problems_found_while_listing_are_dropped() {
    let opus = entry(
        2,
        2,
        "A_OPUS",
        &audio_el(48_000.0, 1),
        &el(CODEC_PRIVATE, &opus_head(1, 0)),
    );
    let blocks = |video: Vec<u8>| {
        vec![
            simple_block(1, 0, true, &video),
            simple_block(2, 0, true, &[0xF8, 1]),
        ]
    };
    for (video_track, video, reason) in [
        (
            entry(1, 1, "V_MPEG4/ISO/AVC", &video_el(64, 48, &[]), &[]),
            vec![0x65, 1, 2, 3],
            "an H.264 frame without CodecPrivate is not an Annex B byte stream",
        ),
        (
            entry(
                1,
                1,
                "V_MPEG4/ISO/AVC",
                &video_el(64, 48, &[]),
                &el(CODEC_PRIVATE, &[0, 1, 2]),
            ),
            vec![0, 0, 0, 1],
            "H.264 CodecPrivate is not an AVCDecoderConfigurationRecord",
        ),
        (
            entry(
                1,
                1,
                "V_AV1",
                &video_el(64, 48, &[]),
                &el(CODEC_PRIVATE, &[1, 2, 3, 4]),
            ),
            vec![0],
            "AV1 without an AV1CodecConfigurationRecord",
        ),
        (
            // A VP9 profile 1 key frame header with 4:4:0 subsampling.
            entry(1, 1, "V_VP9", &video_el(64, 48, &[]), &[]),
            vec![0b1010_0010, 0x49, 0x83, 0x42, 0b0100_0100, 0, 0, 0],
            "VP9 4:4:0 has no chroma subsampling value in vpcC",
        ),
    ] {
        let source = file(
            "webm",
            &[
                info(None),
                tracks(&[video_track, opus.clone()]),
                cluster(0, &blocks(video)),
            ],
        );
        assert_video_not_copied(&source, 0, reason);
        let (summary, output) = remux_dropping_video(&source);
        assert_round_trip(&source, &output, &summary);
        assert_eq!(reasons(&summary), [(0, unsupported(reason))], "{reason}");
        let fail = MatroskaRemuxOptions {
            on_unsupported: OnUnsupported::Fail,
            ..MatroskaRemuxOptions::default()
        };
        assert!(remux_with(&source, &Allowlist::default(), &fail).is_err());
    }
}

#[test]
fn times_beyond_the_kept_range_drop_the_stream() {
    // TimestampScale 2^62 + 1 and a cluster at 2^62 ticks.
    let source = file(
        "webm",
        &[
            info_scale((1 << 62) + 1),
            tracks(&[
                entry(1, 1, "V_VP9", &video_el(320, 240, &[]), &[]),
                entry(
                    2,
                    2,
                    "A_OPUS",
                    &audio_el(48_000.0, 1),
                    &el(CODEC_PRIVATE, &opus_head(1, 0)),
                ),
            ]),
            cluster(
                1 << 62,
                &[
                    simple_block(1, 0, true, &VP9_KEY_FRAME),
                    simple_block(2, 0, true, &[0xF8]),
                ],
            ),
        ],
    );
    // Both streams fail: nothing is left to write, with or without
    // dropping video.
    assert!(matches!(
        remux_with(
            &source,
            &Allowlist::default(),
            &MatroskaRemuxOptions::default()
        ),
        Err(MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported {
            reason: "a time beyond the range this remux keeps (146 years)"
        }))
    ));
    let options = MatroskaRemuxOptions {
        drop_video: true,
        ..MatroskaRemuxOptions::default()
    };
    let result = remux_with(&source, &Allowlist::default(), &options);
    assert!(matches!(
        result,
        Err(MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported {
            reason: "a time beyond the range this remux keeps (146 years)"
        }))
    ));
}

#[test]
fn opus_frames_last_what_they_decode_to() {
    // Chrome-like: 60 ms frames (TOC configuration 3) at times that jitter
    // by a few milliseconds, then a 1 s pause, then DiscardPadding on the
    // last frame.
    let times = [0i16, 59, 122, 178, 241, 300, 1360, 1421];
    let mut blocks: Vec<Vec<u8>> = times[..7]
        .iter()
        .enumerate()
        .map(|(n, &t)| simple_block(1, t, true, &cat(&[vec![0x18], frame(n, 30)])))
        .collect();
    blocks.push(block_group(
        1,
        times[7],
        &cat(&[vec![0x18], frame(7, 30)]),
        &[sint(0x75A2, 20_000_000)],
    ));
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[entry(
                1,
                2,
                "A_OPUS",
                &audio_el(48_000.0, 2),
                &el(CODEC_PRIVATE, &opus_head(2, 0)),
            )]),
            cluster(0, &blocks),
        ],
    );
    let (summary, output) = remux(&source);
    let out = assert_round_trip(&source, &output, &summary);
    let durations: Vec<u32> = mp4_packets(&output, &out, 0)
        .iter()
        .map(|p| p.duration)
        .collect();
    // 2,880 samples each; the frame before the pause lasts up to the next
    // frame's time.
    assert_eq!(
        durations,
        [2880, 2880, 2880, 2880, 2880, 50_880, 2880, 2880]
    );
    // The last 20 ms (960 samples) are padding.
    let EditList::Simple {
        segment,
        media_time,
        ..
    } = out.tracks[0].edit_list
    else {
        panic!("{:?}", out.tracks[0].edit_list);
    };
    assert_eq!(media_time, 0);
    assert_eq!(u64::from(durations.iter().sum::<u32>()) - 960, segment * 48);
}

#[test]
fn a_sample_too_long_for_mp4_drops_its_stream() {
    // The last video frame lasts DefaultDuration, 2^60 ns: more ticks than
    // a 32-bit sample duration holds.
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[
                entry(
                    1,
                    1,
                    "V_VP9",
                    &video_el(320, 240, &[]),
                    &uint(DEFAULT_DURATION, 1 << 60),
                ),
                entry(
                    2,
                    2,
                    "A_OPUS",
                    &audio_el(48_000.0, 1),
                    &el(CODEC_PRIVATE, &opus_head(1, 0)),
                ),
            ]),
            cluster(
                0,
                &[
                    simple_block(1, 0, true, &VP9_KEY_FRAME),
                    simple_block(2, 0, true, &[0xF8]),
                ],
            ),
        ],
    );
    assert_video_not_copied(&source, 0, "a sample lasts too long for MP4");
    let (summary, output) = remux_dropping_video(&source);
    assert_round_trip(&source, &output, &summary);
    assert_eq!(
        reasons(&summary),
        [(0, unsupported("a sample lasts too long for MP4"))]
    );
}

#[test]
fn a_lost_video_stream_fails_unless_dropping_is_asked_for() {
    // Chrome's VP8 + Opus recording: by default no audio-only file.
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[
                entry(1, 1, "V_VP8", &video_el(320, 240, &[]), &[]),
                entry(
                    2,
                    2,
                    "A_OPUS",
                    &audio_el(48_000.0, 1),
                    &el(CODEC_PRIVATE, &opus_head(1, 0)),
                ),
            ]),
            cluster(
                0,
                &[
                    simple_block(
                        1,
                        0,
                        true,
                        &[0x10, 0x02, 0, 0x9d, 0x01, 0x2a, 0x40, 0x01, 0xf0, 0],
                    ),
                    simple_block(2, 0, true, &[0xF8]),
                ],
            ),
        ],
    );
    assert_video_not_copied(&source, 0, VP8_REASON);
    // Asked for: the Opus stream alone, the VP8 stream reported.
    let (summary, output) = remux_dropping_video(&source);
    assert_round_trip(&source, &output, &summary);
    assert_eq!(summary.tracks.len(), 1);
    assert_eq!(reasons(&summary), [(0, unsupported(VP8_REASON))]);
    // Naming the streams is asking too.
    let named = MatroskaRemuxOptions {
        streams: Some(vec![1]),
        ..MatroskaRemuxOptions::default()
    };
    let (summary, _) = remux_with(&source, &Allowlist::default(), &named).unwrap();
    assert_eq!(reasons(&summary), [(0, TrackDropReason::NotSelected)]);
    // A video codec outside the allowlist is lost the same way.
    let opus_only = Allowlist::new(&[Codec::Opus]);
    assert!(matches!(
        remux_with(&source, &opus_only, &MatroskaRemuxOptions::default()),
        Err(MatroskaRemuxError::VideoNotCopied {
            stream_index: 0,
            reason: "its codec is not on the allowlist"
        })
    ));
    let dropping = MatroskaRemuxOptions {
        drop_video: true,
        ..MatroskaRemuxOptions::default()
    };
    let (summary, _) = remux_with(&source, &opus_only, &dropping).unwrap();
    assert_eq!(reasons(&summary), [(0, TrackDropReason::CodecNotAllowed)]);
}

#[test]
fn a_video_stream_without_frames_is_dropped() {
    // A VP9 track whose recording holds no frame: nothing to lose.
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[
                entry(1, 1, "V_VP9", &video_el(320, 240, &[]), &[]),
                entry(
                    2,
                    2,
                    "A_OPUS",
                    &audio_el(48_000.0, 1),
                    &el(CODEC_PRIVATE, &opus_head(1, 0)),
                ),
            ]),
            cluster(
                0,
                &[
                    simple_block(2, 0, true, &[0xF8]),
                    simple_block(2, 20, true, &[0xF8]),
                ],
            ),
        ],
    );
    let (summary, output) = remux(&source);
    assert_round_trip(&source, &output, &summary);
    assert_eq!(summary.tracks.len(), 1);
    assert_eq!(
        reasons(&summary),
        [(0, unsupported("the stream has no frames"))]
    );
}

#[test]
fn laced_opus_frames_without_times_follow_their_durations() {
    // A lace of a 20 ms and a 60 ms packet (TOC configurations 1 and 3) at
    // 0 ms without DefaultDuration, then a block at 80 ms.
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[entry(
                1,
                2,
                "A_OPUS",
                &audio_el(48_000.0, 1),
                &el(CODEC_PRIVATE, &opus_head(1, 0)),
            )]),
            cluster(
                0,
                &[
                    laced_block(1, 0, &[vec![0x08, 1, 2], vec![0x18, 3, 4]]),
                    simple_block(1, 80, true, &[0x08, 5, 6]),
                ],
            ),
        ],
    );
    let (summary, output) = remux(&source);
    let out = assert_round_trip(&source, &output, &summary);
    let durations: Vec<u32> = mp4_packets(&output, &out, 0)
        .iter()
        .map(|p| p.duration)
        .collect();
    assert_eq!(durations, [960, 2880, 960]);
}

#[test]
fn a_stream_of_which_nothing_is_presented_is_dropped() {
    // One 20 ms Opus packet, all of it DiscardPadding.
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[
                entry(1, 1, "V_VP9", &video_el(320, 240, &[]), &[]),
                entry(
                    2,
                    2,
                    "A_OPUS",
                    &audio_el(48_000.0, 1),
                    &el(CODEC_PRIVATE, &opus_head(1, 0)),
                ),
            ]),
            cluster(
                0,
                &[
                    simple_block(1, 0, true, &VP9_KEY_FRAME),
                    block_group(2, 0, &[0x08, 1], &[sint(0x75A2, 20_000_000)]),
                ],
            ),
        ],
    );
    let (summary, output) = remux(&source);
    assert_round_trip(&source, &output, &summary);
    assert_eq!(
        reasons(&summary),
        [(1, unsupported("nothing of the stream is presented"))]
    );
}

#[test]
fn a_composition_offset_too_large_for_mp4_drops_its_stream() {
    // TimestampScale 1 ns (timescale 10^9): reordered H.264 frames 5 s
    // apart need a 10 s composition offset, beyond 32 bits of ticks.
    let video = entry(
        1,
        1,
        "V_MPEG4/ISO/AVC",
        &video_el(64, 48, &[]),
        &el(CODEC_PRIVATE, &avcc()),
    );
    let opus = entry(
        2,
        2,
        "A_OPUS",
        &audio_el(48_000.0, 1),
        &el(CODEC_PRIVATE, &opus_head(1, 0)),
    );
    let source = file(
        "matroska",
        &[
            info_scale(1),
            tracks(&[video, opus]),
            cluster(
                0,
                &[
                    simple_block(1, 0, true, &frame(0, 10)),
                    simple_block(2, 0, true, &[0x08]),
                ],
            ),
            cluster(10_000_000_000, &[simple_block(1, 0, false, &frame(1, 10))]),
            cluster(5_000_000_000, &[simple_block(1, 0, false, &frame(2, 10))]),
        ],
    );
    assert_video_not_copied(&source, 0, "a composition offset too large for MP4");
    let (summary, output) = remux_dropping_video(&source);
    assert_round_trip(&source, &output, &summary);
    assert_eq!(
        reasons(&summary),
        [(0, unsupported("a composition offset too large for MP4"))]
    );
}

#[test]
fn block_additions_of_dropped_streams_do_not_count() {
    // Track 1 (VP9 whose first frame header cannot be read, dropped after
    // the listing) has 300 BlockAdditions; track 2 (kept) has 2 after them.
    let addition = el(0x75A1, &el(0xA6, &cat(&[uint(0xEE, 1), el(0xA5, &[9])])));
    let mut blocks: Vec<Vec<u8>> = (0..300)
        .map(|n| block_group(1, n as i16, &[0, 0, 0], std::slice::from_ref(&addition)))
        .collect();
    blocks.push(block_group(
        2,
        300,
        &VP9_KEY_FRAME,
        std::slice::from_ref(&addition),
    ));
    blocks.push(block_group(
        2,
        340,
        &[0b1000_0110, 0, 0],
        &[sint_reference(-40), addition.clone()],
    ));
    let source = file(
        "webm",
        &[
            info(None),
            tracks(&[
                entry(1, 1, "V_VP9", &video_el(320, 240, &[]), &[]),
                entry(2, 1, "V_VP9", &video_el(320, 240, &[]), &[]),
            ]),
            cluster(0, &blocks),
        ],
    );
    let (summary, output) = remux_dropping_video(&source);
    assert_round_trip(&source, &output, &summary);
    assert_eq!(summary.tracks.len(), 1);
    assert_eq!(summary.dropped_additions.len(), 2);
    assert!(
        summary
            .dropped_additions
            .iter()
            .all(|a| a.stream_index == 1)
    );
    assert_eq!(summary.dropped_additions_not_listed, 0);
}

/// Bit fields `(value, width)` packed most significant bit first, padded
/// with zero bits to a whole byte.
fn pack(fields: &[(u32, u32)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut used = 0u32;
    for &(value, width) in fields {
        for bit in (0..width).rev() {
            if used % 8 == 0 {
                out.push(0);
            }
            *out.last_mut().unwrap() |= (((value >> bit) & 1) as u8) << (7 - used % 8);
            used += 1;
        }
    }
    out
}

/// A FLAC `STREAMINFO` (RFC 9639 8.2) of 1,152-sample blocks.
fn streaminfo(rate: u32, channels: u32, bits: u32) -> Vec<u8> {
    let mut info = pack(&[
        (1152, 16),
        (1152, 16),
        (20, 24),
        (40, 24),
        (rate, 20),
        (channels - 1, 3),
        (bits - 1, 5),
        (0, 4),
        (1152 * 6, 32),
    ]);
    info.extend([0x5A; 16]);
    info
}

/// The Apple Lossless magic cookie: 4,096-sample frames, 16 bits, `channels`
/// channels at 44.1 kHz.
fn alac_cookie(channels: u32) -> Vec<u8> {
    pack(&[
        (4096, 32),
        (0, 8),
        (16, 8),
        (40, 8),
        (10, 8),
        (14, 8),
        (channels, 8),
        (255, 16),
        (40, 32),
        (64_000, 32),
        (44_100, 32),
    ])
}

/// A stereo Apple Lossless packet: see [`alac_element_packet`].
fn alac_packet(stated: Option<u32>, seed: usize) -> Vec<u8> {
    alac_element_packet(1, stated, seed)
}

/// An Apple Lossless packet of one channel element: its header (`element`,
/// 0 for one channel or 1 for a pair, instance 0, 12 zero bits, the flag
/// for a stated sample count, shift 0, no escape), the count when stated,
/// then filler.
fn alac_element_packet(element: u32, stated: Option<u32>, seed: usize) -> Vec<u8> {
    let mut fields = vec![
        (element, 3),
        (0, 4),
        (0, 12),
        (u32::from(stated.is_some()), 1),
        (0, 2),
        (0, 1),
    ];
    fields.extend(stated.map(|n| (n, 32)));
    cat(&[pack(&fields), frame(seed, 40)])
}

/// A Matroska file of one stereo Apple Lossless track holding `packets`,
/// each lasting the samples paired with it, at their times in
/// milliseconds.
fn alac_source(packets: &[(Vec<u8>, u32)]) -> Vec<u8> {
    alac_source_of(2, packets)
}

/// [`alac_source`] with `channels` channels.
fn alac_source_of(channels: u32, packets: &[(Vec<u8>, u32)]) -> Vec<u8> {
    let mut at = 0u32;
    let blocks: Vec<Vec<u8>> = packets
        .iter()
        .map(|(packet, samples)| {
            let ms = (f64::from(at) / 44.1).round() as i16;
            at += samples;
            simple_block(1, ms, true, packet)
        })
        .collect();
    file(
        "matroska",
        &[
            info(None),
            tracks(&[entry(
                1,
                2,
                "A_ALAC",
                &audio_el(44_100.0, u64::from(channels)),
                &el(CODEC_PRIVATE, &alac_cookie(channels)),
            )]),
            cluster(0, &blocks),
        ],
    )
}

/// Each Apple Lossless packet lasts the samples its header states, else
/// the cookie's `frameLength`: a short packet in the middle and a short
/// last one keep their own lengths.
#[test]
fn alac_packets_last_the_samples_their_headers_state() {
    let source = alac_source(&[
        (alac_packet(None, 0), 4096),
        (alac_packet(Some(100), 1), 100),
        (alac_packet(None, 2), 4096),
        (alac_packet(Some(2184), 3), 2184),
    ]);
    let (summary, output) = remux(&source);
    assert!(summary.dropped_tracks.is_empty(), "{summary:?}");
    let out = assert_round_trip(&source, &output, &summary);
    let times: Vec<(i64, u32)> = mp4_packets(&output, &out, 0)
        .iter()
        .map(|p| (p.dts, p.duration))
        .collect();
    assert_eq!(times, [(0, 4096), (4096, 100), (4196, 4096), (8292, 2184)]);
}

/// A stated count wins over the source's millisecond times, which cannot
/// tell a short packet from none: 16 samples then 4,096 at times 0 and 0,
/// and 16 samples (0.36 ms) between two packets of 4,096. A mono stream's
/// packets hold one single channel element.
#[test]
fn alac_stated_counts_win_over_millisecond_times() {
    let cases = [
        (
            alac_source(&[(alac_packet(Some(16), 0), 16), (alac_packet(None, 1), 4096)]),
            vec![(0, 16), (16, 4096)],
        ),
        (
            alac_source(&[
                (alac_packet(None, 0), 4096),
                (alac_packet(Some(16), 1), 16),
                (alac_packet(None, 2), 4096),
            ]),
            vec![(0, 4096), (4096, 16), (4112, 4096)],
        ),
        (
            alac_source_of(
                1,
                &[
                    (alac_element_packet(0, None, 0), 4096),
                    (alac_element_packet(0, Some(16), 1), 16),
                    (alac_element_packet(0, None, 2), 4096),
                ],
            ),
            vec![(0, 4096), (4096, 16), (4112, 4096)],
        ),
    ];
    for (n, (source, expected)) in cases.into_iter().enumerate() {
        let (summary, output) = remux(&source);
        assert!(summary.dropped_tracks.is_empty(), "case {n}: {summary:?}");
        let out = assert_round_trip(&source, &output, &summary);
        let times: Vec<(i64, u32)> = mp4_packets(&output, &out, 0)
            .iter()
            .map(|p| (p.dts, p.duration))
            .collect();
        assert_eq!(times, expected, "case {n}");
    }
}

/// More than two channels take more than one channel element a packet, and
/// only the first element's header is read: the stream is refused.
#[test]
fn alac_of_more_than_two_channels_is_refused() {
    let fail = MatroskaRemuxOptions {
        on_unsupported: OnUnsupported::Fail,
        ..MatroskaRemuxOptions::default()
    };
    for channels in [3, 6] {
        let source = alac_source_of(channels, &[(alac_packet(None, 0), 4096)]);
        assert!(
            matches!(
                remux_with(&source, &Allowlist::default(), &fail),
                Err(MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported {
                    reason: "Apple Lossless from Matroska with more than two channels: \
                             a packet's later channel elements are not read"
                }))
            ),
            "{channels} channels"
        );
    }
}

/// A packet whose header is not a channel element's, or states no samples
/// or more than the cookie's `frameLength`, is refused rather than given
/// a guessed length.
#[test]
fn alac_packets_with_a_malformed_header_are_refused() {
    let fail = MatroskaRemuxOptions {
        on_unsupported: OnUnsupported::Fail,
        ..MatroskaRemuxOptions::default()
    };
    let malformed = [
        // A nonzero bit among the 12 that must be zero.
        cat(&[pack(&[(1, 3), (0, 4), (1, 12), (0, 4)]), frame(0, 40)]),
        // Shift 3.
        cat(&[
            pack(&[(1, 3), (0, 4), (0, 12), (0, 1), (3, 2), (0, 1)]),
            frame(0, 40),
        ]),
        // A data stream element first (tag 4).
        cat(&[pack(&[(4, 3), (0, 4), (0, 12), (0, 4)]), frame(0, 40)]),
        // A single channel element in a stereo stream.
        alac_element_packet(0, None, 0),
        alac_packet(Some(0), 0),
        alac_packet(Some(4097), 0),
        // A stated count cut short.
        pack(&[(1, 3), (0, 4), (0, 12), (1, 1), (0, 2), (0, 1), (100, 16)]),
    ];
    for (n, packet) in malformed.into_iter().enumerate() {
        let source = alac_source(&[(alac_packet(None, 0), 4096), (packet, 4096)]);
        assert!(
            matches!(
                remux_with(&source, &Allowlist::default(), &fail),
                Err(MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported {
                    reason: "an Apple Lossless packet without a valid element header"
                }))
            ),
            "case {n}"
        );
    }
}

/// Six frames of each codec, one per block, at its frame duration: each
/// copied unchanged into its MP4 sample entry with the configuration box
/// its binding names, at the source's times.
#[test]
fn flac_alac_ac3_and_eac3_get_their_sample_entries() {
    // FLAC (RFC 9639 10.1): `fLaC` and the last metadata block,
    // STREAMINFO; frames of 1,152 samples (block size code 3), stereo,
    // 16 bits, at 96 kHz.
    let mut flac_private = b"fLaC".to_vec();
    flac_private.extend([0x80, 0, 0, 34]);
    flac_private.extend(streaminfo(96_000, 2, 16));
    let flac = |n: usize| cat(&[vec![0xFF, 0xF8, 0x30, 0x18, n as u8, 0], frame(n, 20)]);
    let cookie = alac_cookie(2);
    // An AC-3 syncframe (ETSI TS 102 366 4.3): 48 kHz, 32 kbit/s (64
    // words), bsid 8, bsmod 0, stereo without LFE.
    let ac3 = |n: usize| {
        let mut f = pack(&[
            (0x0B77, 16),
            (0, 16),
            (0, 2),
            (0, 6),
            (8, 5),
            (0, 3),
            (2, 3),
            (0, 2),
            (0, 1),
        ]);
        f.extend(frame(n, 128 - f.len()));
        f
    };
    // An E-AC-3 frame (E.1.2): an independent substream of six blocks at
    // 48 kHz, stereo, bsid 16, 128 words.
    let eac3 = |n: usize| {
        let mut f = pack(&[
            (0x0B77, 16),
            (0, 2),
            (0, 3),
            (127, 11),
            (0, 2),
            (3, 2),
            (2, 3),
            (0, 1),
            (16, 5),
            (31, 5),
            (0, 1),
            (0, 1),
            (0, 1),
            (0, 1),
        ]);
        f.extend(frame(n, 256 - f.len()));
        f
    };
    // (codec ID, CodecPrivate, Audio rate, frame, frame in ms, sample
    // entry, configuration box and payload, timescale, frame samples).
    type Case<'a> = (
        &'a str,
        Vec<u8>,
        f64,
        &'a dyn Fn(usize) -> Vec<u8>,
        f64,
        [u8; 4],
        [u8; 4],
        Vec<u8>,
        u32,
        u32,
    );
    let alac = |n: usize| alac_packet(None, n);
    let cases: [Case<'_>; 4] = [
        (
            "A_FLAC",
            flac_private.clone(),
            96_000.0,
            &flac,
            12.0,
            *b"fLaC",
            *b"dfLa",
            cat(&[vec![0, 0, 0, 0], flac_private[4..].to_vec()]),
            96_000,
            1152,
        ),
        (
            "A_ALAC",
            cookie.clone(),
            44_100.0,
            &alac,
            4096.0 / 44.1,
            *b"alac",
            *b"alac",
            cat(&[vec![0, 0, 0, 0], cookie.clone()]),
            44_100,
            4096,
        ),
        (
            "A_AC3",
            Vec::new(),
            48_000.0,
            &ac3,
            32.0,
            *b"ac-3",
            *b"dac3",
            // fscod 0, bsid 8, bsmod 0, acmod 2, lfeon 0, bit_rate_code 0
            // (F.4).
            vec![0x10, 0x10, 0x00],
            48_000,
            1536,
        ),
        (
            "A_EAC3",
            Vec::new(),
            48_000.0,
            &eac3,
            32.0,
            *b"ec-3",
            *b"dec3",
            // data_rate 64 kbit/s, one independent substream: fscod 0,
            // bsid 16, bsmod 0, acmod 2, lfeon 0, no dependent substream
            // (F.6).
            vec![0x02, 0x00, 0x20, 0x04, 0x00],
            48_000,
            1536,
        ),
    ];
    for (codec_id, private, rate, make, ms, fourcc, kind, payload, timescale, samples) in cases {
        let blocks: Vec<Vec<u8>> = (0..6)
            .map(|n| simple_block(1, (n as f64 * ms).round() as i16, true, &make(n)))
            .collect();
        let extra = if private.is_empty() {
            Vec::new()
        } else {
            el(CODEC_PRIVATE, &private)
        };
        let source = file(
            "matroska",
            &[
                info(None),
                tracks(&[entry(1, 2, codec_id, &audio_el(rate, 2), &extra)]),
                cluster(0, &blocks),
            ],
        );
        let (summary, output) = remux(&source);
        assert!(summary.dropped_tracks.is_empty(), "{codec_id}: {summary:?}");
        let out = assert_round_trip(&source, &output, &summary);
        let track = &out.tracks[0];
        assert_eq!(track.timescale, timescale, "{codec_id}");
        let d = &track.description;
        assert_eq!(d.fourcc, FourCc(fourcc), "{codec_id}");
        assert_eq!(d.channels, Some(2), "{codec_id}");
        let config = d.codec_config.as_ref().unwrap();
        assert_eq!(
            (config.kind, &config.data),
            (FourCc(kind), &payload),
            "{codec_id}"
        );
        let packets = mp4_packets(&output, &out, 0);
        let times: Vec<i64> = packets.iter().map(|p| p.dts).collect();
        let expected: Vec<i64> = (0..6).map(|n| n * i64::from(samples)).collect();
        assert_eq!(times, expected, "{codec_id}");
        assert!(packets.iter().all(|p| p.keyframe), "{codec_id}");
    }
}
