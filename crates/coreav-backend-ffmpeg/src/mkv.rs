//! The Matroska stream the worker writes to FFmpeg's standard input (RFC
//! 9559): an EBML header, a Segment of unknown size with one video track,
//! then one Cluster per packet (`docs/design/decode.md`, "The FFmpeg
//! fallback").

use coreav_decode::{Codec, ConfigKind, StreamConfig};

/// Added to every pts, so that negative composition times stay positive
/// (Cluster timestamps are unsigned).
pub const PTS_OFFSET: i64 = 1 << 40;

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const CLUSTER: u32 = 0x1F43_B675;

/// The `CodecID` for `codec` (Matroska codec mappings).
pub fn codec_id(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "V_MPEG4/ISO/AVC",
        Codec::Hevc => "V_MPEGH/ISO/HEVC",
        Codec::Av1 => "V_AV1",
        Codec::Vp9 => "V_VP9",
        Codec::Vp8 => "V_VP8",
        Codec::ProRes => "V_PRORES",
        Codec::Mpeg4 => "V_MPEG4/ISO/ASP",
        Codec::Mpeg1Video => "V_MPEG1",
        Codec::Mpeg2Video => "V_MPEG2",
        Codec::Mjpeg => "V_MJPEG",
        Codec::Theora => "V_THEORA",
        Codec::Ffv1 => "V_FFV1",
    }
}

/// The EBML header, the start of the Segment, Info and Tracks.
pub fn header(stream: &StreamConfig) -> Vec<u8> {
    let mut out = Vec::new();
    let mut ebml = Vec::new();
    uint(&mut ebml, 0x4286, 1);
    uint(&mut ebml, 0x42F7, 1);
    uint(&mut ebml, 0x42F2, 4);
    uint(&mut ebml, 0x42F3, 8);
    element(&mut ebml, 0x4282, b"matroska");
    uint(&mut ebml, 0x4287, 4);
    uint(&mut ebml, 0x4285, 2);
    element(&mut out, EBML, &ebml);
    id(&mut out, SEGMENT);
    // Unknown size: a stream.
    out.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    let mut info = Vec::new();
    // One tick per nanosecond, so a block's timestamp is the packet's pts
    // as a number (plus PTS_OFFSET), whatever its time scale.
    uint(&mut info, 0x2A_D7B1, 1);
    element(&mut info, 0x4D80, b"coreav");
    element(&mut info, 0x5741, b"coreav");
    element(&mut out, INFO, &info);
    let mut entry = Vec::new();
    uint(&mut entry, 0xD7, 1);
    uint(&mut entry, 0x73C5, 1);
    uint(&mut entry, 0x83, 1);
    element(&mut entry, 0x86, codec_id(stream.codec).as_bytes());
    if let Some(private) = codec_private(stream) {
        element(&mut entry, 0x63A2, private);
    }
    let mut video = Vec::new();
    uint(
        &mut video,
        0xB0,
        u64::from(stream.width.unwrap_or(0).max(1)),
    );
    uint(
        &mut video,
        0xBA,
        u64::from(stream.height.unwrap_or(0).max(1)),
    );
    element(&mut entry, 0xE0, &video);
    let mut tracks = Vec::new();
    element(&mut tracks, 0xAE, &entry);
    element(&mut out, TRACKS, &tracks);
    out
}

/// The configuration bytes Matroska's mapping stores for the codec: the
/// ISOBMFF record for H.264, HEVC and AV1, the four-character code for
/// ProRes, the decoder specific information for MPEG-4 Part 2, Matroska's
/// own `CodecPrivate` as given; nothing for a `vpcC`, which Matroska does
/// not carry.
fn codec_private(stream: &StreamConfig) -> Option<&[u8]> {
    let config = stream.config.as_ref()?;
    match config.kind {
        ConfigKind::Vpcc => None,
        ConfigKind::Avcc
        | ConfigKind::Hvcc
        | ConfigKind::Av1c
        | ConfigKind::ProResFourcc
        | ConfigKind::Mpeg4Dsi
        | ConfigKind::MatroskaPrivate => Some(&config.data),
    }
}

/// One Cluster holding one SimpleBlock of track 1 at `timestamp`.
pub fn cluster(timestamp: u64, keyframe: bool, data: &[u8]) -> Vec<u8> {
    let mut block = Vec::with_capacity(data.len() + 4);
    // Track number 1 as a vint, relative timestamp 0, flags.
    block.extend_from_slice(&[0x81, 0, 0, if keyframe { 0x80 } else { 0 }]);
    block.extend_from_slice(data);
    let mut body = Vec::with_capacity(block.len() + 24);
    uint(&mut body, 0xE7, timestamp);
    element(&mut body, 0xA3, &block);
    let mut out = Vec::with_capacity(body.len() + 12);
    element(&mut out, CLUSTER, &body);
    out
}

fn id(out: &mut Vec<u8>, id: u32) {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count();
    out.extend(bytes.iter().skip(skip));
}

/// An EBML size as a variable-length integer of the fewest bytes that do
/// not make it all ones (RFC 8794 4.4).
fn size(out: &mut Vec<u8>, value: u64) {
    let mut length = 1u32;
    while length < 8 && value >= (1u64 << (7 * length)) - 1 {
        length += 1;
    }
    let marked = value | (1u64 << (7 * length));
    let bytes = marked.to_be_bytes();
    out.extend(bytes.iter().skip(8 - length as usize));
}

fn element(out: &mut Vec<u8>, element_id: u32, payload: &[u8]) {
    id(out, element_id);
    size(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

fn uint(out: &mut Vec<u8>, element_id: u32, value: u64) {
    let bytes = value.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count().min(7);
    let payload: Vec<u8> = bytes.iter().skip(skip).copied().collect();
    element(out, element_id, &payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_minimal_vints() {
        let mut v = Vec::new();
        size(&mut v, 0);
        size(&mut v, 126);
        size(&mut v, 127);
        size(&mut v, 300);
        assert_eq!(v, vec![0x80, 0xFE, 0x40, 0x7F, 0x41, 0x2C]);
    }

    #[test]
    fn cluster_layout() {
        let c = cluster(5, true, &[9, 9]);
        assert_eq!(
            c,
            vec![
                0x1F, 0x43, 0xB6, 0x75, 0x8B, 0xE7, 0x81, 5, 0xA3, 0x86, 0x81, 0, 0, 0x80, 9, 9
            ]
        );
    }
}
