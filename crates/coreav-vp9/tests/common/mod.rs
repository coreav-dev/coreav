use serde_json::Value;

pub(crate) fn manifest() -> Vec<(String, String)> {
    let text = include_str!("../data/vectors.json");
    let json: Value = serde_json::from_str(text).unwrap();
    json["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["name"].as_str().unwrap().to_owned(),
                v["sha256"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The blocks of a WebM file, in storage order, read without a Matroska
/// library: the coverage of this crate's similarity check forbids path
/// dependencies on crates it does not cover.
///
/// The Segment, its Clusters and their BlockGroups are entered; every other
/// element is skipped by its size, so an unknown size (all ones) is entered
/// too. A SimpleBlock or Block is a track number, a 2-byte timestamp, a flags
/// byte and the frame; the vectors have one track and no lacing. A file whose
/// last element runs past its end ends before that element.
pub(crate) fn webm_packets(data: &[u8]) -> Vec<&[u8]> {
    /// An EBML variable-length integer at `at`: its bytes as stored (an
    /// element ID keeps its length marker), its value without the marker,
    /// whether every value bit is 1, and its length.
    fn vint(data: &[u8], at: usize) -> Option<(u64, u64, bool, usize)> {
        let first = *data.get(at)?;
        let len = first.leading_zeros() as usize + 1;
        if len > 8 {
            return None;
        }
        let bytes = data.get(at..at + len)?;
        let raw = bytes.iter().fold(0u64, |v, &b| v << 8 | u64::from(b));
        let value = raw & (u64::MAX >> (64 - 7 * len));
        Some((raw, value, value == (1u64 << (7 * len)) - 1, len))
    }
    /// Segment, Cluster and BlockGroup.
    const ENTERED: [u64; 3] = [0x1853_8067, 0x1F43_B675, 0xA0];
    /// SimpleBlock and Block.
    const BLOCKS: [u64; 2] = [0xA3, 0xA1];
    let mut packets = Vec::new();
    let mut at = 0;
    while let Some((id, _, _, id_len)) = vint(data, at) {
        let Some((_, size, unknown, size_len)) = vint(data, at + id_len) else {
            break;
        };
        let body = at + id_len + size_len;
        if ENTERED.contains(&id) || unknown {
            at = body;
            continue;
        }
        let Some(end) = usize::try_from(size)
            .ok()
            .and_then(|s| body.checked_add(s))
            .filter(|&e| e <= data.len())
        else {
            break;
        };
        if BLOCKS.contains(&id) {
            let (_, _, _, track_len) = vint(data, body).expect("a block's track number");
            assert_eq!(data[body + track_len + 2] & 0x06, 0, "a laced block");
            packets.push(&data[body + track_len + 3..end]);
        }
        at = end;
    }
    packets
}

/// The frames of an IVF file: a 32-byte header whose bytes 6 and 7 give its
/// length, then each frame's 4-byte size, 8-byte timestamp and bytes.
pub(crate) fn ivf_packets(data: &[u8]) -> Vec<&[u8]> {
    let mut at = usize::from(u16::from_le_bytes([data[6], data[7]]));
    let mut packets = Vec::new();
    while at < data.len() {
        let size = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        packets.push(&data[at + 12..at + 12 + size]);
        at += 12 + size;
    }
    packets
}
