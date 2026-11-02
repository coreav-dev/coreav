//! The sole SHA-bound truncated-tail condition, never a general truncation policy.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Deserialize)]
struct Element {
    offset: u64,
    id: u32,
    header_len: u8,
    size: u64,
}

#[derive(Clone, Deserialize)]
struct Condition {
    id: String,
    sha256: String,
    file_size: u64,
    timestamp_scale: u64,
    track_number: u64,
    cluster: Element,
    timestamp: Element,
    cluster_timestamp: u64,
    block: Element,
    partial_pts: i64,
}

pub(super) struct Accepted {
    pub id: String,
    pub pts: i64,
}

pub(super) fn accept(
    path: &Path,
    offset: u64,
    track: u64,
    delay: u64,
    scale: u64,
) -> Result<Option<Accepted>, String> {
    let expected: Condition = serde_json::from_str(include_str!(
        "../../../docs/measurements/vp9-native/file-condition.json"
    ))
    .map_err(|e| e.to_string())?;
    if offset != expected.block.offset
        || track != expected.track_number
        || delay != 0
        || scale != expected.timestamp_scale
    {
        return Ok(None);
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if !expected.matches(&mut file, size, offset)? {
        return Ok(None);
    }
    Ok(Some(Accepted {
        id: expected.id,
        pts: expected.partial_pts,
    }))
}

impl Condition {
    fn matches(
        &self,
        file: &mut (impl Read + Seek),
        size: u64,
        offset: u64,
    ) -> Result<bool, String> {
        if size != self.file_size || offset != self.block.offset {
            return Ok(false);
        }
        for element in [&self.cluster, &self.timestamp, &self.block] {
            if element.id == 0 {
                return Ok(false);
            }
            let id = element.id.to_be_bytes();
            let id = id
                .get((element.id.leading_zeros() / 8) as usize..)
                .ok_or("element ID length")?;
            let Some(size_len) = usize::from(element.header_len).checked_sub(id.len()) else {
                return Ok(false);
            };
            if !(1..=8).contains(&size_len) || element.size >= (1u64 << (7 * size_len)) - 1 {
                return Ok(false);
            }
            let size_bytes = ((1u64 << (7 * size_len)) | element.size).to_be_bytes();
            let mut expected = id.to_vec();
            expected.extend_from_slice(
                size_bytes
                    .get(8 - size_len..)
                    .ok_or("element size length")?,
            );
            file.seek(SeekFrom::Start(element.offset))
                .map_err(|e| e.to_string())?;
            let mut bytes = [0u8; 12];
            let bytes = bytes.get_mut(..expected.len()).ok_or("header byte count")?;
            file.read_exact(bytes).map_err(|e| e.to_string())?;
            if bytes != expected {
                return Ok(false);
            }
        }
        let end = |e: &Element| e.offset + u64::from(e.header_len) + e.size;
        if end(&self.cluster) <= size
            || end(&self.block) <= size
            || end(&self.block) > end(&self.cluster)
            || self.timestamp.size > 8
            || self.track_number > 126
        {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(
            self.timestamp.offset + u64::from(self.timestamp.header_len),
        ))
        .map_err(|e| e.to_string())?;
        let mut timestamp = [0u8; 8];
        let timestamp = timestamp
            .get_mut(..self.timestamp.size as usize)
            .ok_or("timestamp byte count")?;
        file.read_exact(timestamp).map_err(|e| e.to_string())?;
        let timestamp = timestamp.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b));
        file.seek(SeekFrom::Start(
            self.block.offset + u64::from(self.block.header_len),
        ))
        .map_err(|e| e.to_string())?;
        let mut block = [0u8; 4];
        file.read_exact(&mut block).map_err(|e| e.to_string())?;
        let [track, high, low, flags] = block;
        if track != 0x80 | self.track_number as u8
            || flags & 6 != 0
            || timestamp != self.cluster_timestamp
            || i64::try_from(timestamp)
                .ok()
                .and_then(|t| t.checked_add(i64::from(i16::from_be_bytes([high, low]))))
                != Some(self.partial_pts)
        {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut hash = Sha256::new();
        let mut bytes = [0u8; 32 * 1024];
        loop {
            let count = file.read(&mut bytes).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            hash.update(bytes.get(..count).ok_or("hash byte count")?);
        }
        Ok(format!("{:x}", hash.finalize()) == self.sha256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fixture() -> Result<(Condition, Vec<u8>), String> {
        let mut bytes = vec![0u8; 64];
        bytes
            .get_mut(..5)
            .ok_or("cluster")?
            .copy_from_slice(&[0x1f, 0x43, 0xb6, 0x75, 0xe4]);
        bytes
            .get_mut(5..8)
            .ok_or("timestamp")?
            .copy_from_slice(&[0xe7, 0x81, 7]);
        bytes
            .get_mut(56..62)
            .ok_or("block")?
            .copy_from_slice(&[0xa3, 0x94, 0x81, 0, 2, 0]);
        let condition = Condition {
            id: "synthetic".into(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            file_size: 64,
            timestamp_scale: 1_000_000,
            track_number: 1,
            cluster: Element {
                offset: 0,
                id: 0x1f43b675,
                header_len: 5,
                size: 100,
            },
            timestamp: Element {
                offset: 5,
                id: 0xe7,
                header_len: 2,
                size: 1,
            },
            cluster_timestamp: 7,
            block: Element {
                offset: 56,
                id: 0xa3,
                header_len: 2,
                size: 20,
            },
            partial_pts: 9,
        };
        Ok((condition, bytes))
    }

    #[test]
    fn only_the_exact_bound_bytes_and_truncated_structure_match() -> Result<(), String> {
        let (condition, bytes) = fixture()?;
        assert!(condition.matches(&mut Cursor::new(&bytes), 64, 56)?);
        assert!(!condition.matches(&mut Cursor::new(&bytes), 63, 56)?);
        assert!(!condition.matches(&mut Cursor::new(&bytes), 64, 55)?);
        let mut changed = bytes.clone();
        *changed.last_mut().ok_or("last byte")? = 1;
        assert!(!condition.matches(&mut Cursor::new(changed), 64, 56)?);
        for field in 0..4 {
            let mut changed = condition.clone();
            match field {
                0 => changed.block.size += 1,
                1 => changed.cluster.size = 50,
                2 => changed.partial_pts += 1,
                _ => changed.cluster_timestamp += 1,
            }
            assert!(!changed.matches(&mut Cursor::new(&bytes), 64, 56)?);
        }
        Ok(())
    }
}
