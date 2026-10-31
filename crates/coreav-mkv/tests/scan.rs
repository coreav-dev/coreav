//! `Segment::scan_tracks` (`docs/design/policy-matroska.md`, decision 2):
//! spans, frame bytes and verified picture sizes on hand-built files.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod common;

use common::*;
use coreav_mkv::{Allowlist, Error, LimitKind, Limits, Scan, Segment, SliceSource, probe};

fn probe_bytes(data: &[u8]) -> Segment {
    probe(
        SliceSource::new(data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap()
}

fn scan(data: &[u8]) -> Scan {
    probe_bytes(data)
        .scan_tracks(SliceSource::new(data), &Limits::default())
        .unwrap()
}

fn one_track(track: Vec<u8>, blocks: &[Vec<u8>]) -> Vec<u8> {
    file(
        "matroska",
        &[info(None), el(TRACKS, &track), cluster(0, blocks)],
    )
}

/// A VP8 key frame of `width` × `height` (RFC 6386 9.1).
fn vp8_key(width: u16, height: u16) -> Vec<u8> {
    let mut frame = vec![0x50, 0x2a, 0x00, 0x9d, 0x01, 0x2a];
    frame.extend(width.to_le_bytes());
    frame.extend(height.to_le_bytes());
    frame.extend([0u8; 6]);
    frame
}

const VP8_INTER: [u8; 6] = [0x51, 0x2a, 0x00, 1, 2, 3];

/// Fields packed most significant bit first; `Ue` as Exp-Golomb.
#[derive(Clone, Copy)]
enum F {
    U(u64, u32),
    Ue(u32),
}

fn pack(fields: &[F]) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::new();
    let mut put = |value: u64, width: u32| {
        for i in (0..width).rev() {
            bits.push(value >> i & 1 == 1);
        }
    };
    for field in fields {
        match *field {
            F::U(value, width) => put(value, width),
            F::Ue(value) => {
                let code = u64::from(value) + 1;
                let len = 64 - code.leading_zeros();
                put(0, len - 1);
                put(code, len);
            }
        }
    }
    bits.push(true);
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    bits.chunks(8)
        .map(|c| {
            c.iter()
                .enumerate()
                .fold(0u8, |b, (i, bit)| b | (u8::from(*bit) << (7 - i)))
        })
        .collect()
}

/// An H.264 Main profile SPS NAL unit for `width_mbs` × `height_mbs`
/// macroblocks, progressive, not cropped.
fn avc_sps(width_mbs: u32, height_mbs: u32) -> Vec<u8> {
    let mut nal = vec![0x67];
    nal.extend(pack(&[
        F::U(77, 8),
        F::U(0, 8),
        F::U(40, 8),
        F::Ue(0),
        F::Ue(0),
        F::Ue(2),
        F::Ue(1),
        F::U(0, 1),
        F::Ue(width_mbs - 1),
        F::Ue(height_mbs - 1),
        F::U(1, 1),
        F::U(1, 1),
        F::U(0, 1),
        F::U(0, 1),
    ]));
    nal
}

/// An `avcC` record with 4-byte NAL lengths and this SPS.
fn avcc(sps: &[u8]) -> Vec<u8> {
    let mut record = vec![1, 77, 0, 40, 0xff, 0xe1];
    record.extend(u16::try_from(sps.len()).unwrap().to_be_bytes());
    record.extend(sps);
    record.push(0);
    record
}

/// A length-prefixed (4 bytes) frame of these NAL units.
fn nal_frame(nals: &[Vec<u8>]) -> Vec<u8> {
    nals.iter()
        .flat_map(|n| {
            let mut out = u32::try_from(n.len()).unwrap().to_be_bytes().to_vec();
            out.extend(n);
            out
        })
        .collect()
}

const SLICE: [u8; 4] = [0x65, 0x88, 0x84, 0x00];

#[test]
fn spans_and_bytes_follow_the_blocks() {
    // VP9 with a 40 ms DefaultDuration; Opus with a 6.5 ms CodecDelay, no
    // DefaultDuration and 20 ms frames (TOC 0xFC).
    let data = small_webm();
    let result = scan(&data);
    let (video, audio) = (result.tracks[0], result.tracks[1]);
    assert_eq!((video.frames, video.bytes), (2, 11));
    assert_eq!((video.start_ns, video.end_ns), (Some(0), Some(80_000_000)));
    assert_eq!(video.duration_ns(), Some(80_000_000));
    assert_eq!((audio.frames, audio.bytes), (2, 6));
    assert_eq!(
        (audio.start_ns, audio.end_ns),
        (Some(-6_500_000), Some(33_500_000))
    );
    assert_eq!(audio.duration_ns(), Some(40_000_000));
    assert_eq!(audio.frame_size, None);
}

/// A SimpleBlock of `frames` in Xiph lacing (RFC 9559 10.3.2), each frame
/// shorter than 255 bytes.
fn laced_block(track: u8, timestamp: i16, frames: &[&[u8]]) -> Vec<u8> {
    let mut body = vec![0x80 | track];
    body.extend(timestamp.to_be_bytes());
    body.push(0x80 | 0x02);
    body.push(u8::try_from(frames.len() - 1).unwrap());
    for frame in &frames[..frames.len() - 1] {
        body.push(u8::try_from(frame.len()).unwrap());
    }
    for frame in frames {
        body.extend_from_slice(frame);
    }
    el(SIMPLE_BLOCK, &body)
}

#[test]
fn opus_frames_are_timed_by_their_toc() {
    // No DefaultDuration: 20 ms CELT frames (TOC 0xF8), three in a lace,
    // then one alone; a 6.5 ms CodecDelay.
    let track = audio_track(
        1,
        "A_OPUS",
        48_000.0,
        2,
        &cat(&[
            uint(0x56AA, 6_500_000),
            el(0x63A2, b"OpusHead\x01\x02\x38\x01\x80\xbb\0\0\0\0\0"),
        ]),
    );
    let frame: &[u8] = &[0xf8, 1, 2];
    let data = one_track(
        track.clone(),
        &[
            laced_block(1, 0, &[frame, frame, frame]),
            simple_block(1, 60, true, frame),
        ],
    );
    let opus = scan(&data).tracks[0];
    assert_eq!((opus.frames, opus.untimed_frames), (4, 0));
    assert_eq!(opus.start_ns, Some(-6_500_000));
    assert_eq!(opus.end_ns, Some(73_500_000));
    assert_eq!(opus.duration_ns(), Some(80_000_000));
    // A lace whose first frame's duration is unknown (an empty frame):
    // the later frames have no time.
    let data = one_track(track, &[laced_block(1, 0, &[&[], frame, frame])]);
    let opus = scan(&data).tracks[0];
    assert_eq!(opus.untimed_frames, 2);
    assert_eq!(opus.duration_ns(), None);
}

#[test]
fn block_duration_ends_the_block() {
    let block = |timestamp: i16, frame: &[u8]| {
        let mut body = vec![0x81];
        body.extend(timestamp.to_be_bytes());
        body.push(0);
        body.extend_from_slice(frame);
        el(BLOCK_GROUP, &cat(&[el(0xA1, &body), uint(0x9B, 2_500)]))
    };
    let data = file(
        "matroska",
        &[
            info(None),
            el(
                TRACKS,
                &el(
                    TRACK_ENTRY,
                    &cat(&[uint(0xD7, 1), uint(0x83, 0x11), string(0x86, "S_TEXT/UTF8")]),
                ),
            ),
            cluster(1_000, &[block(0, b"a"), block(5_000, b"bc")]),
        ],
    );
    let track = scan(&data).tracks[0];
    assert_eq!(track.start_ns, Some(1_000_000_000));
    assert_eq!(track.end_ns, Some(8_500_000_000));
    assert_eq!(track.bytes, 3);
}

#[test]
fn vp8_key_frames_bound_the_size() {
    let data = one_track(
        video_track(1, "V_VP8", 320, 240, &[]),
        &[
            simple_block(1, 0, true, &vp8_key(320, 240)),
            simple_block(1, 40, false, &VP8_INTER),
            simple_block(1, 80, true, &vp8_key(1920, 1080)),
        ],
    );
    assert_eq!(scan(&data).tracks[0].frame_size, Some((1920, 1080)));
    // A key frame without its start code: no bound.
    let mut broken = vp8_key(320, 240);
    broken[3] = 0;
    let data = one_track(
        video_track(1, "V_VP8", 320, 240, &[]),
        &[simple_block(1, 0, true, &broken)],
    );
    assert_eq!(scan(&data).tracks[0].frame_size, None);
}

#[test]
fn h264_parameter_sets_in_codec_private_and_frames() {
    let private = el(0x63A2, &avcc(&avc_sps(20, 15)));
    let data = one_track(
        video_track(1, "V_MPEG4/ISO/AVC", 64, 64, &private),
        &[simple_block(1, 0, true, &nal_frame(&[SLICE.to_vec()]))],
    );
    assert_eq!(scan(&data).tracks[0].frame_size, Some((320, 240)));
    // A larger SPS in a later frame raises the bound.
    let data = one_track(
        video_track(1, "V_MPEG4/ISO/AVC", 64, 64, &private),
        &[
            simple_block(1, 0, true, &nal_frame(&[SLICE.to_vec()])),
            simple_block(1, 40, true, &nal_frame(&[avc_sps(120, 68), SLICE.to_vec()])),
        ],
    );
    assert_eq!(scan(&data).tracks[0].frame_size, Some((1920, 1088)));
    // A NAL length past the end of the frame: no bound.
    let data = one_track(
        video_track(1, "V_MPEG4/ISO/AVC", 64, 64, &private),
        &[simple_block(1, 0, true, &[0, 0, 0, 9, 0x65])],
    );
    assert_eq!(scan(&data).tracks[0].frame_size, None);
    // Without CodecPrivate (as Chrome writes H.264) the frames are a byte
    // stream with start codes (Annex B).
    let annex_b = |nals: &[Vec<u8>]| -> Vec<u8> {
        nals.iter()
            .flat_map(|n| [&[0, 0, 0, 1][..], n].concat())
            .collect()
    };
    let data = one_track(
        video_track(1, "V_MPEG4/ISO/AVC", 64, 64, &[]),
        &[
            simple_block(1, 0, true, &annex_b(&[avc_sps(20, 15), SLICE.to_vec()])),
            simple_block(1, 40, true, &annex_b(&[avc_sps(80, 45), SLICE.to_vec()])),
        ],
    );
    assert_eq!(scan(&data).tracks[0].frame_size, Some((1280, 720)));
    // A record too short to give the NAL unit length size: no bound.
    let data = one_track(
        video_track(1, "V_MPEG4/ISO/AVC", 64, 64, &el(0x63A2, &[1, 77, 0])),
        &[simple_block(1, 0, true, &nal_frame(&[SLICE.to_vec()]))],
    );
    assert_eq!(scan(&data).tracks[0].frame_size, None);
}

#[test]
fn stripped_headers_are_read_back_and_encrypted_frames_are_not_read() {
    let strip = |prefix: &[u8]| {
        el(
            0x6D80,
            &el(
                0x6240,
                &cat(&[
                    uint(0x5032, 1),
                    uint(0x5033, 0),
                    el(0x5034, &cat(&[uint(0x4254, 3), el(0x4255, prefix)])),
                ]),
            ),
        )
    };
    let key = vp8_key(1280, 720);
    let data = one_track(
        video_track(1, "V_VP8", 320, 240, &strip(&key[..4])),
        &[simple_block(1, 0, true, &key[4..])],
    );
    let track = scan(&data).tracks[0];
    assert_eq!(track.frame_size, Some((1280, 720)));
    assert_eq!(track.bytes, key.len() as u64);
    let encrypted = el(
        0x6D80,
        &el(
            0x6240,
            &cat(&[uint(0x5033, 1), el(0x5035, &uint(0x47E1, 5))]),
        ),
    );
    let data = one_track(
        video_track(1, "V_VP8", 320, 240, &encrypted),
        &[simple_block(1, 0, true, &key)],
    );
    let track = scan(&data).tracks[0];
    assert_eq!((track.frames, track.frame_size), (1, None));
    // zlib-compressed frames are listed with their inflated sizes, but
    // nothing is read from them.
    let zlib = el(
        0x6D80,
        &el(
            0x6240,
            &cat(&[uint(0x5033, 0), el(0x5034, &uint(0x4254, 0))]),
        ),
    );
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&key, 6);
    let data = one_track(
        video_track(1, "V_VP8", 320, 240, &zlib),
        &[simple_block(1, 0, true, &compressed)],
    );
    let track = scan(&data).tracks[0];
    assert!(track.listed);
    assert_eq!(
        (track.frames, track.bytes, track.frame_size),
        (1, key.len() as u64, None)
    );
    // A frame that is not a zlib stream leaves the track's size unknown:
    // not listed.
    let data = one_track(
        video_track(1, "V_VP8", 320, 240, &zlib),
        &[simple_block(1, 0, true, &key)],
    );
    let track = scan(&data).tracks[0];
    assert!(!track.listed);
    assert_eq!(track.frames, 1);
}

#[test]
fn prores_frames_with_or_without_their_atom_header() {
    let header = |width: u16, height: u16| {
        let mut h = vec![0, 148, 0, 0];
        h.extend(b"apl0");
        h.extend(width.to_be_bytes());
        h.extend(height.to_be_bytes());
        h.extend([0u8; 20]);
        h
    };
    let with_atom = cat(&[
        vec![0, 0, 0, 40, b'i', b'c', b'p', b'f'],
        header(1920, 1080),
    ]);
    let track = || video_track(1, "V_PRORES", 640, 360, &el(0x63A2, b"apcn"));
    let data = one_track(track(), &[simple_block(1, 0, true, &with_atom)]);
    assert_eq!(scan(&data).tracks[0].frame_size, Some((1920, 1080)));
    let data = one_track(track(), &[simple_block(1, 0, true, &header(3840, 2160))]);
    assert_eq!(scan(&data).tracks[0].frame_size, Some((3840, 2160)));
}

/// A source whose `fail_at`-th read fails with an I/O error.
struct FailingAt<'a> {
    data: SliceSource<'a>,
    reads: usize,
    fail_at: usize,
}

impl coreav_mkv::Source for FailingAt<'_> {
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

/// A read that fails is not a frame without its atom header:
/// wherever the failure falls, the scan fails with it.
#[test]
fn a_read_failure_fails_the_scan_wherever_it_falls() {
    let mut frame = vec![0, 148, 0, 0];
    frame.extend(b"apl0");
    frame.extend(1920u16.to_be_bytes());
    frame.extend(1080u16.to_be_bytes());
    frame.extend([0u8; 20]);
    let track = video_track(1, "V_PRORES", 640, 360, &el(0x63A2, b"apcn"));
    let data = one_track(track, &[simple_block(1, 0, true, &frame)]);
    let segment = probe_bytes(&data);
    let mut clean = FailingAt {
        data: SliceSource::new(&data),
        reads: 0,
        fail_at: 0,
    };
    let scanned = segment.scan_tracks(&mut clean, &Limits::default()).unwrap();
    assert_eq!(scanned.tracks[0].frame_size, Some((1920, 1080)));
    for fail_at in 1..=clean.reads {
        let mut source = FailingAt {
            data: SliceSource::new(&data),
            reads: 0,
            fail_at,
        };
        let result = segment.scan_tracks(&mut source, &Limits::default());
        assert_eq!(
            result.err(),
            Some(Error::Io(std::io::ErrorKind::ConnectionReset)),
            "read {fail_at} of {}",
            clean.reads
        );
    }
}

fn scan_error(data: &[u8]) -> Error {
    probe_bytes(data)
        .scan_tracks(SliceSource::new(data), &Limits::default())
        .unwrap_err()
}

#[test]
fn a_truncated_file_fails_the_scan() {
    let mut data = one_track(
        video_track(1, "V_VP8", 320, 240, &uint(0x23_E383, 40_000_000)),
        &[
            simple_block(1, 0, true, &vp8_key(320, 240)),
            simple_block(1, 40, true, &vp8_key(640, 480)),
        ],
    );
    data.truncate(data.len() - 4);
    assert!(matches!(
        scan_error(&data),
        Error::Truncated { .. } | Error::InvalidElement { .. }
    ));
}

/// A file whose structure hides frames from the listing that a reader
/// may still find (docs/design/policy-matroska.md, decision 8).
#[test]
fn the_audit_refuses_what_the_listing_could_miss() {
    let track = || el(TRACKS, &video_track(1, "V_VP8", 320, 240, &[]));
    let key = |ts| simple_block(1, ts, true, &vp8_key(320, 240));
    let reason = |error: Error| match error {
        Error::InvalidElement { reason, .. } => reason,
        other => panic!("{other:?}"),
    };
    // A Segment whose declared size ends after the first Cluster.
    let mut short = file("webm", &[info(None), track(), cluster(0, &[key(0)])]);
    short.extend(cluster(40, &[key(0)]));
    assert_eq!(
        reason(scan_error(&short)),
        "data after the Segment's declared end"
    );
    // An EBML header among the Segment's children.
    let ebml = live_file(
        "webm",
        &[
            info(None),
            track(),
            cluster(0, &[key(0)]),
            ebml_header("webm"),
        ],
    );
    assert_eq!(
        reason(scan_error(&ebml)),
        "an EBML header inside the Segment"
    );
    // A second Tracks element.
    let tracks = file(
        "webm",
        &[
            info(None),
            track(),
            el(TRACKS, &audio_track(2, "A_OPUS", 48_000.0, 2, &[])),
            cluster(0, &[key(0)]),
        ],
    );
    assert_eq!(
        reason(scan_error(&tracks)),
        "a second Info or Tracks element"
    );
    // A BlockGroup with two Blocks.
    let block = |ts: i16| {
        let mut body = vec![0x81];
        body.extend(ts.to_be_bytes());
        body.push(0);
        body.extend(vp8_key(320, 240));
        el(0xA1, &body)
    };
    let groups = file(
        "webm",
        &[
            info(None),
            track(),
            cluster(0, &[el(BLOCK_GROUP, &cat(&[block(0), block(0)]))]),
        ],
    );
    assert_eq!(
        reason(scan_error(&groups)),
        "a BlockGroup with a second Block"
    );
    // A block of a track no TrackEntry defines.
    let undefined = file(
        "webm",
        &[
            info(None),
            track(),
            cluster(0, &[simple_block(2, 0, true, &[1])]),
        ],
    );
    assert_eq!(
        reason(scan_error(&undefined)),
        "a block of an undefined track"
    );
    // An element of unknown kind among the Segment's children.
    let unknown = file(
        "webm",
        &[
            info(None),
            track(),
            el(0x1A2B_3C4D, &[0]),
            cluster(0, &[key(0)]),
        ],
    );
    assert_eq!(
        reason(scan_error(&unknown)),
        "an unknown element in the Segment"
    );
    // A Void that claims more bytes than the file holds, before a Cluster.
    let mut void = live_file("webm", &[info(None), track()]);
    void.extend([0xEC, 0x08, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00]);
    void.extend(cluster(0, &[key(0)]));
    assert!(matches!(scan_error(&void), Error::Truncated { .. }));
    // An integer longer than 8 octets, where demuxers resynchronize: a
    // ReferenceBlock in a BlockGroup, and a CueTime in Cues.
    let group = el(BLOCK_GROUP, &cat(&[block(0), el(0xFB, &[0u8; 9])]));
    let long_ref = file("webm", &[info(None), track(), cluster(0, &[group])]);
    assert_eq!(
        reason(scan_error(&long_ref)),
        "an element whose length its type does not allow"
    );
    let cues = el(0x1C53_BB6B, &el(0xBB, &el(0xB3, &[0u8; 12])));
    let long_cue = file("webm", &[info(None), track(), cues, cluster(0, &[key(0)])]);
    assert_eq!(
        reason(scan_error(&long_cue)),
        "an element whose length its type does not allow"
    );
    // A string longer than the audit allows (`max_element_bytes`), where
    // demuxers that refuse long strings resynchronize: a TagString.
    let tags = el(
        0x1254_C367,
        &el(0x7373, &el(0x67C8, &el(0x4487, &[b'a'; 100]))),
    );
    let long_tag = file("webm", &[info(None), track(), tags, cluster(0, &[key(0)])]);
    let small = Limits {
        max_element_bytes: 64,
        ..Limits::default()
    };
    let error = probe_bytes(&long_tag)
        .scan_tracks(SliceSource::new(&long_tag), &small)
        .unwrap_err();
    assert_eq!(
        reason(error),
        "a string or binary longer than the audit allows"
    );
    assert_eq!(
        scan(&long_tag).tracks[0].frames,
        1,
        "within the default bound"
    );
    // The same files without the defect scan.
    let good = file(
        "webm",
        &[
            info(None),
            track(),
            cluster(0, &[key(0)]),
            cluster(40, &[key(0)]),
        ],
    );
    assert_eq!(scan(&good).tracks[0].frames, 2);
}

#[test]
fn the_scan_is_bounded_by_the_work_limit() {
    let blocks: Vec<Vec<u8>> = (0..50)
        .map(|i| simple_block(1, i, true, &nal_frame(&vec![SLICE.to_vec(); 20])))
        .collect();
    let private = el(0x63A2, &avcc(&avc_sps(20, 15)));
    let data = one_track(video_track(1, "V_MPEG4/ISO/AVC", 64, 64, &private), &blocks);
    let segment = probe_bytes(&data);
    let limits = Limits {
        max_block_work: 200,
        ..Limits::default()
    };
    let result = segment.scan_tracks(SliceSource::new(&data), &limits);
    assert!(matches!(
        result,
        Err(Error::LimitExceeded {
            limit: LimitKind::BlockWork,
            ..
        })
    ));
}

/// The audit walks with the listing, but its verdict still comes
/// first: an error of the listing, or of the scan reading the frames,
/// gives way to an audit error found later in the file, as when the audit
/// ran first.
#[test]
fn an_audit_error_later_in_the_file_wins() {
    let track = || el(TRACKS, &video_track(1, "V_VP8", 320, 240, &[]));
    let key = |ts| simple_block(1, ts, true, &vp8_key(320, 240));
    let unknown = el(0x1A2B_3C4D, &[0]);
    // The listing fails at the first Cluster (a block before the Cluster
    // Timestamp); the audit, at the end of the Segment.
    let untimed = el(CLUSTER, &key(0));
    let listing = file("webm", &[info(None), track(), untimed.clone()]);
    assert!(matches!(
        scan_error(&listing),
        Error::InvalidElement {
            reason: "block before the Cluster Timestamp",
            ..
        }
    ));
    let both = file(
        "webm",
        &[
            info(None),
            track(),
            untimed,
            cluster(40, &[key(0)]),
            unknown.clone(),
        ],
    );
    assert!(matches!(
        scan_error(&both),
        Error::InvalidElement {
            reason: "an unknown element in the Segment",
            ..
        }
    ));
    // The scan's own frame reading runs out of work at the tenth block; the
    // audit fails after the last.
    let blocks: Vec<Vec<u8>> = (0..50)
        .map(|i| simple_block(1, i, true, &nal_frame(&vec![SLICE.to_vec(); 20])))
        .collect();
    let private = el(0x63A2, &avcc(&avc_sps(20, 15)));
    let entry = el(TRACKS, &video_track(1, "V_MPEG4/ISO/AVC", 64, 64, &private));
    let limits = Limits {
        max_block_work: 200,
        ..Limits::default()
    };
    for (tail, expected) in [
        (vec![], None),
        (unknown, Some("an unknown element in the Segment")),
    ] {
        let data = file(
            "matroska",
            &[info(None), entry.clone(), cluster(0, &blocks), tail],
        );
        let error = probe_bytes(&data)
            .scan_tracks(SliceSource::new(&data), &limits)
            .unwrap_err();
        match expected {
            None => assert!(matches!(
                error,
                Error::LimitExceeded {
                    limit: LimitKind::BlockWork,
                    ..
                }
            )),
            Some(expected) => assert!(
                matches!(error, Error::InvalidElement { reason, .. } if reason == expected),
                "{error:?}"
            ),
        }
    }
    // A file without Clusters is audited too.
    let empty = file("webm", &[info(None), track(), el(0x1A2B_3C4D, &[0])]);
    assert!(matches!(
        scan_error(&empty),
        Error::InvalidElement {
            reason: "an unknown element in the Segment",
            ..
        }
    ));
}

/// The audit and the listing walk the file together: through a window
/// that keeps the last mebibyte, as the WebAssembly build reads, the scan
/// reads a file about once, not once per walk.
#[test]
fn the_scan_reads_the_file_about_once() {
    struct Counting<'a>(SliceSource<'a>, u64);
    impl coreav_mkv::Source for Counting<'_> {
        fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
            self.0.len()
        }
        fn read_exact_at(
            &mut self,
            offset: u64,
            buf: &mut [u8],
        ) -> Result<(), coreav_isobmff::Error> {
            self.1 += buf.len() as u64;
            self.0.read_exact_at(offset, buf)
        }
    }
    // Clusters of small frames, over several windows.
    let clusters: Vec<Vec<u8>> = (0..64)
        .map(|c| {
            let blocks: Vec<Vec<u8>> = (0..64)
                .map(|i| {
                    simple_block(
                        1,
                        i * 10,
                        i == 0,
                        &[vec![0x51, 0x2a, 0x00], vec![7; 2000]].concat(),
                    )
                })
                .collect();
            cluster(c * 1000, &blocks)
        })
        .collect();
    let mut children = vec![
        info(None),
        el(TRACKS, &video_track(1, "V_VP8", 320, 240, &[])),
    ];
    children.extend(clusters);
    let data = file("webm", &children);
    let segment = probe_bytes(&data);
    let mut counting = Counting(SliceSource::new(&data), 0);
    let scan = segment
        .scan_tracks(
            coreav_mkv::BufferedSource::new(&mut counting),
            &Limits::default(),
        )
        .unwrap();
    assert_eq!(scan.tracks[0].frames, 64 * 64);
    let read = counting.1 as f64 / data.len() as f64;
    assert!(read < 1.1, "read {read:.2} times the file");
}

/// Inside a BlockGroup, the listing reads the Block's header before the
/// elements after it but reports their errors first, as when it read it
/// after them; the scan reports the audit's.
#[test]
fn errors_inside_a_block_group_keep_their_order() {
    let track = el(TRACKS, &video_track(1, "V_VP8", 320, 240, &[]));
    // A Block whose header announces Xiph lacing and ends there, then a
    // BlockDuration of 9 octets.
    let bad_block = el(0xA1, &[0x81, 0, 0, 0x02]);
    let long_duration = el(0x9B, &[0u8; 9]);
    let group = el(BLOCK_GROUP, &cat(&[bad_block.clone(), long_duration]));
    let data = file("webm", &[info(None), track.clone(), cluster(0, &[group])]);
    let segment = probe_bytes(&data);
    let listed: Vec<_> = segment
        .packets(SliceSource::new(&data), &Limits::default())
        .collect();
    assert!(
        matches!(
            listed.as_slice(),
            [Err(Error::LimitExceeded {
                limit: LimitKind::ElementBytes,
                ..
            })]
        ),
        "{listed:?}"
    );
    assert!(matches!(
        scan_error(&data),
        Error::InvalidElement {
            reason: "an element whose length its type does not allow",
            ..
        }
    ));
    // Alone, the Block's error is the listing's.
    let group = el(BLOCK_GROUP, &bad_block);
    let data = file("webm", &[info(None), track, cluster(0, &[group])]);
    let listed: Vec<_> = probe_bytes(&data)
        .packets(SliceSource::new(&data), &Limits::default())
        .collect();
    assert!(matches!(listed.as_slice(), [Err(_)]), "{listed:?}");
}

/// A source that fails to read is reported as such: the audit does not
/// read on past it and put a structural error in its place.
#[test]
fn a_read_failure_is_reported_as_one() {
    // Fails the listing's first read of a block header (64 octets), which
    // the audit never asks for.
    struct Failing<'a>(SliceSource<'a>);
    impl coreav_mkv::Source for Failing<'_> {
        fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
            self.0.len()
        }
        fn read_exact_at(
            &mut self,
            offset: u64,
            buf: &mut [u8],
        ) -> Result<(), coreav_isobmff::Error> {
            if buf.len() == 64 {
                return Err(coreav_isobmff::Error::Io(
                    std::io::ErrorKind::ConnectionReset,
                ));
            }
            self.0.read_exact_at(offset, buf)
        }
    }
    let frame = [vp8_key(320, 240), vec![0; 100]].concat();
    // An element of unknown kind at the end fails the audit.
    let data = file(
        "webm",
        &[
            info(None),
            el(TRACKS, &video_track(1, "V_VP8", 320, 240, &[])),
            cluster(0, &[simple_block(1, 0, true, &frame)]),
            el(0x1A2B_3C4D, &[0]),
        ],
    );
    let error = probe_bytes(&data)
        .scan_tracks(Failing(SliceSource::new(&data)), &Limits::default())
        .unwrap_err();
    assert_eq!(error, Error::Io(std::io::ErrorKind::ConnectionReset));
    assert!(matches!(
        scan_error(&data),
        Error::InvalidElement {
            reason: "an unknown element in the Segment",
            ..
        }
    ));
}
