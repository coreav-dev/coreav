//! Builders for hand-made Matroska files (RFC 8794, RFC 9559), shared by the
//! tests of this crate and of `coreav-cli`.
#![allow(dead_code, clippy::cast_possible_truncation)]

/// The ID bytes of an element ID with its marker bits.
pub fn id_bytes(id: u32) -> Vec<u8> {
    let len = (4 - id.leading_zeros() / 8) as usize;
    id.to_be_bytes()[4 - len..].to_vec()
}

/// The shortest data size encoding of `size`.
pub fn size_bytes(size: u64) -> Vec<u8> {
    for len in 1..=8u32 {
        if size < (1u64 << (7 * len)) - 1 {
            let value = (1u64 << (7 * len)) | size;
            return value.to_be_bytes()[(8 - len) as usize..].to_vec();
        }
    }
    panic!("size too large")
}

/// An element with a known size.
pub fn el(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = id_bytes(id);
    out.extend(size_bytes(payload.len() as u64));
    out.extend_from_slice(payload);
    out
}

/// An element with the 8-octet unknown size.
pub fn el_unknown(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = id_bytes(id);
    out.extend([0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    out.extend_from_slice(payload);
    out
}

pub fn uint(id: u32, value: u64) -> Vec<u8> {
    let len = (8 - value.leading_zeros() as usize / 8).max(1);
    el(id, &value.to_be_bytes()[8 - len..])
}

pub fn sint(id: u32, value: i64) -> Vec<u8> {
    el(id, &value.to_be_bytes())
}

pub fn float(id: u32, value: f64) -> Vec<u8> {
    el(id, &value.to_be_bytes())
}

pub fn string(id: u32, value: &str) -> Vec<u8> {
    el(id, value.as_bytes())
}

pub fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

pub const EBML: u32 = 0x1A45_DFA3;
pub const SEGMENT: u32 = 0x1853_8067;
pub const SEEK_HEAD: u32 = 0x114D_9B74;
pub const INFO: u32 = 0x1549_A966;
pub const TRACKS: u32 = 0x1654_AE6B;
pub const CLUSTER: u32 = 0x1F43_B675;
pub const CUES: u32 = 0x1C53_BB6B;
pub const ATTACHMENTS: u32 = 0x1941_A469;
pub const VOID: u32 = 0xEC;
pub const TRACK_ENTRY: u32 = 0xAE;
pub const SIMPLE_BLOCK: u32 = 0xA3;
pub const BLOCK_GROUP: u32 = 0xA0;

/// The EBML header with `doc_type`.
pub fn ebml_header(doc_type: &str) -> Vec<u8> {
    el(
        EBML,
        &cat(&[
            uint(0x4286, 1),
            uint(0x42F7, 1),
            uint(0x42F2, 4),
            uint(0x42F3, 8),
            string(0x4282, doc_type),
            uint(0x4287, 4),
            uint(0x4285, 2),
        ]),
    )
}

/// `Info` with the default `TimestampScale` and a duration in ticks.
pub fn info(duration: Option<f64>) -> Vec<u8> {
    let mut body = uint(0x2A_D7B1, 1_000_000);
    if let Some(duration) = duration {
        body.extend(float(0x4489, duration));
    }
    body.extend(string(0x4D80, "coreav-tests"));
    body.extend(string(0x5741, "coreav-tests"));
    el(INFO, &body)
}

/// A video `TrackEntry`.
pub fn video_track(number: u64, codec_id: &str, width: u64, height: u64, extra: &[u8]) -> Vec<u8> {
    el(
        TRACK_ENTRY,
        &cat(&[
            uint(0xD7, number),
            uint(0x73C5, number * 1000),
            uint(0x83, 1),
            string(0x86, codec_id),
            el(0xE0, &cat(&[uint(0xB0, width), uint(0xBA, height)])),
            extra.to_vec(),
        ]),
    )
}

/// An audio `TrackEntry`.
pub fn audio_track(number: u64, codec_id: &str, rate: f64, channels: u64, extra: &[u8]) -> Vec<u8> {
    el(
        TRACK_ENTRY,
        &cat(&[
            uint(0xD7, number),
            uint(0x73C5, number * 1000),
            uint(0x83, 2),
            string(0x86, codec_id),
            el(0xE1, &cat(&[float(0xB5, rate), uint(0x9F, channels)])),
            extra.to_vec(),
        ]),
    )
}

/// A SimpleBlock body (no lacing) for `track` (below 127).
pub fn simple_block(track: u64, timestamp: i16, keyframe: bool, frame: &[u8]) -> Vec<u8> {
    let mut body = vec![0x80 | track as u8];
    body.extend(timestamp.to_be_bytes());
    body.push(if keyframe { 0x80 } else { 0 });
    body.extend_from_slice(frame);
    el(SIMPLE_BLOCK, &body)
}

/// A Cluster with a known size.
pub fn cluster(timestamp: u64, blocks: &[Vec<u8>]) -> Vec<u8> {
    el(
        CLUSTER,
        &cat(&[vec![uint(0xE7, timestamp)], blocks.to_vec()].concat()),
    )
}

/// A whole file: EBML header, then a Segment of `children`.
pub fn file(doc_type: &str, children: &[Vec<u8>]) -> Vec<u8> {
    cat(&[ebml_header(doc_type), el(SEGMENT, &cat(children))])
}

/// A whole file whose Segment has an unknown size.
pub fn live_file(doc_type: &str, children: &[Vec<u8>]) -> Vec<u8> {
    cat(&[ebml_header(doc_type), el_unknown(SEGMENT, &cat(children))])
}

/// The offset of the Segment's data in a file made by [`file`].
pub fn segment_data_offset(doc_type: &str, segment_size: usize) -> usize {
    ebml_header(doc_type).len() + id_bytes(SEGMENT).len() + size_bytes(segment_size as u64).len()
}

/// A VP9 key frame header: profile 0, BT.709, studio range.
pub const VP9_KEY_FRAME: [u8; 8] = [0b1000_0010, 0x49, 0x83, 0x42, 0b0100_0000, 0, 0, 0];

/// A two-track WebM file (VP9 video, Opus audio) with one Cluster and Cues.
pub fn small_webm() -> Vec<u8> {
    let tracks = el(
        TRACKS,
        &cat(&[
            video_track(1, "V_VP9", 320, 240, &uint(0x23_E383, 40_000_000)),
            audio_track(
                2,
                "A_OPUS",
                48_000.0,
                2,
                &cat(&[
                    uint(0x56AA, 6_500_000),
                    el(0x63A2, b"OpusHead\x01\x02\x38\x01\x80\xbb\0\0\0\0\0"),
                ]),
            ),
        ]),
    );
    let blocks = vec![
        simple_block(1, 0, true, &VP9_KEY_FRAME),
        simple_block(2, 0, true, &[0xfc, 1, 2]),
        simple_block(2, 20, true, &[0xfc, 3, 4]),
        simple_block(1, 40, false, &[0b1000_0110, 0, 0]),
    ];
    let clusters = cluster(0, &blocks);
    // SeekHead with 8-octet positions, so its size does not depend on them.
    let seek_head = |cues: u64| {
        el(
            SEEK_HEAD,
            &el(
                0x4DBB,
                &cat(&[el(0x53AB, &id_bytes(CUES)), el(0x53AC, &cues.to_be_bytes())]),
            ),
        )
    };
    let head_len = seek_head(0).len() + info(Some(80.0)).len() + tracks.len();
    let cues = el(
        CUES,
        &el(
            0xBB,
            &cat(&[
                uint(0xB3, 0),
                el(0xB7, &cat(&[uint(0xF7, 1), uint(0xF1, head_len as u64)])),
            ]),
        ),
    );
    let head = cat(&[
        seek_head((head_len + clusters.len()) as u64),
        info(Some(80.0)),
        tracks,
    ]);
    file("webm", &[head, clusters, cues])
}
