use coreav_hevc::{AnnexB, Error, Result};
use std::io::Read;

pub const INPUT_LIMIT: u64 = 4 << 30;
pub const CHUNK: usize = 65536;

pub fn reconstruction_stats(stats: coreav_hevc::IntraStats) -> serde_json::Value {
    serde_json::json!({"ctus":stats.ctus,"coding_units":stats.coding_units,
        "transform_units":stats.transform_units,"nonzero_transform_units":stats.nonzero_transform_units,
        "transform_skips":stats.transform_skips,"transquant_bypass_units":stats.transquant_bypass_units,
        "pcm_units":stats.pcm_units,"dependent_segments":stats.dependent_segments,
        "tile_restarts":stats.tile_restarts,"wpp_restarts":stats.wpp_restarts,
        "qp_deltas":stats.qp_deltas,"nonzero_qp_deltas":stats.nonzero_qp_deltas,
        "scaling_list_blocks":stats.scaling_list_blocks,"sign_hidden_groups":stats.sign_hidden_groups,
        "dst_blocks":stats.dst_blocks,"bins":stats.bins,"picture_hashes":stats.picture_hashes,
        "reserved_hashes":stats.reserved_hashes,"memory_bytes":stats.memory_bytes,
        "deblock_edges":stats.deblock_edges,"deblock_changed_samples":stats.deblock_changed_samples,
        "sao_blocks":stats.sao_blocks,"sao_changed_samples":stats.sao_changed_samples,"sao_merges":stats.sao_merges,
        "luma_modes":stats.luma_modes.as_slice(),"chroma_modes":stats.chroma_modes.as_slice(),
        "dct_blocks":stats.dct_blocks.as_slice()})
}

pub struct Receipt {
    pub bytes: u64,
    pub capacity: usize,
}

pub fn read_nals(
    mut source: impl Read,
    nal_limit: usize,
    mut emit: impl FnMut(u64, &[u8]) -> Result<()>,
) -> Result<Receipt> {
    let capacity_limit = nal_limit
        .checked_add(CHUNK + 8)
        .ok_or(Error::Limit("inspection_input_capacity"))?;
    let mut data = Vec::new();
    let mut chunk = [0; CHUNK];
    let (mut total, mut base, mut peak) = (0u64, 0u64, 0usize);
    let (mut cursor, mut started) = (0usize, false);
    loop {
        let count = source
            .read(&mut chunk)
            .map_err(|_| Error::InvalidData("input_read"))?;
        total = total
            .checked_add(count as u64)
            .filter(|n| *n <= INPUT_LIMIT)
            .ok_or(Error::Limit("inspection_stream_bytes"))?;
        if data
            .len()
            .checked_add(count)
            .is_none_or(|n| n > capacity_limit)
        {
            return Err(Error::Limit("inspection_nal_bytes"));
        }
        data.try_reserve_exact(count)
            .map_err(|_| Error::Limit("inspection_input_allocation"))?;
        if data.capacity() > capacity_limit {
            return Err(Error::Limit("inspection_input_capacity"));
        }
        peak = peak.max(data.capacity());
        data.extend_from_slice(
            chunk
                .get(..count)
                .ok_or(Error::InvalidData("input_read_size"))?,
        );
        while let Some(found) = data
            .get(cursor..)
            .ok_or(Error::InvalidData("input_cursor"))?
            .windows(3)
            .position(|v| v == [0, 0, 1])
        {
            let prefix = cursor + found;
            if !started {
                started = true;
                cursor = prefix + 3;
                continue;
            }
            emit_nal(
                data.get(..prefix)
                    .ok_or(Error::InvalidData("input_cursor"))?,
                base,
                nal_limit,
                &mut emit,
            )?;
            data.drain(..prefix);
            base = base
                .checked_add(prefix as u64)
                .ok_or(Error::Limit("inspection_stream_bytes"))?;
            cursor = 3;
        }
        cursor = data.len().saturating_sub(2).max(cursor.min(data.len()));
        if count == 0 {
            if !data.is_empty() {
                emit_nal(&data, base, nal_limit, &mut emit)?;
            }
            return Ok(Receipt {
                bytes: total,
                capacity: peak,
            });
        }
    }
}

fn emit_nal(
    data: &[u8],
    base: u64,
    limit: usize,
    emit: &mut impl FnMut(u64, &[u8]) -> Result<()>,
) -> Result<()> {
    let mut framed = AnnexB::new(data);
    let nal = framed.next().ok_or(Error::InvalidData("empty_nal"))??;
    if nal.len() > limit {
        return Err(Error::Limit("inspection_nal_bytes"));
    }
    if framed.next().is_some() {
        return Err(Error::InvalidData("input_framing"));
    }
    let offset = (nal.as_ptr() as usize)
        .checked_sub(data.as_ptr() as usize)
        .and_then(|n| base.checked_add(n as u64))
        .ok_or(Error::Limit("inspection_stream_bytes"))?;
    emit(offset, nal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstruction_counter_projection_preserves_the_intra_schema() -> Result<()> {
        let stats = coreav_hevc::IntraStats {
            ctus: 17,
            bins: 19,
            memory_bytes: 23,
            deblock_edges: [1, 2, 3],
            dct_blocks: [4, 5, 6, 7],
            ..coreav_hevc::IntraStats::default()
        };
        let value = reconstruction_stats(stats);
        let object = value.as_object().ok_or(Error::InvalidData("test_stats"))?;
        let mut expected = [
            "ctus",
            "coding_units",
            "transform_units",
            "nonzero_transform_units",
            "transform_skips",
            "transquant_bypass_units",
            "pcm_units",
            "dependent_segments",
            "tile_restarts",
            "wpp_restarts",
            "qp_deltas",
            "nonzero_qp_deltas",
            "scaling_list_blocks",
            "sign_hidden_groups",
            "dst_blocks",
            "bins",
            "picture_hashes",
            "reserved_hashes",
            "memory_bytes",
            "deblock_edges",
            "deblock_changed_samples",
            "sao_blocks",
            "sao_changed_samples",
            "sao_merges",
            "luma_modes",
            "chroma_modes",
            "dct_blocks",
        ];
        expected.sort_unstable();
        assert_eq!(
            object.keys().map(String::as_str).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(value.get("ctus"), Some(&serde_json::json!(17)));
        assert_eq!(value.get("bins"), Some(&serde_json::json!(19)));
        assert_eq!(value.get("memory_bytes"), Some(&serde_json::json!(23)));
        assert_eq!(
            value.get("deblock_edges"),
            Some(&serde_json::json!([1, 2, 3]))
        );
        assert_eq!(
            value.get("dct_blocks"),
            Some(&serde_json::json!([4, 5, 6, 7]))
        );
        assert_eq!(
            value
                .get("luma_modes")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(35)
        );
        assert_eq!(
            value
                .get("chroma_modes")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(35)
        );
        Ok(())
    }

    struct Short<'a> {
        bytes: &'a [u8],
        size: usize,
    }
    impl Read for Short<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = out.len().min(self.size).min(self.bytes.len());
            let (source, rest) = self.bytes.split_at(n);
            out.get_mut(..n)
                .ok_or(std::io::ErrorKind::InvalidInput)?
                .copy_from_slice(source);
            self.bytes = rest;
            Ok(n)
        }
    }

    #[test]
    fn every_chunk_boundary_agrees_with_the_complete_framer() -> Result<()> {
        let bytes = [
            0, 0, 0, 0, 1, 64, 1, 0, 0, 3, 1, 0, 0, 1, 66, 1, 128, 0, 0, 0, 1, 68, 1, 128, 0, 0,
        ];
        let expected = AnnexB::new(&bytes).collect::<Result<Vec<_>>>()?;
        for size in 1..=bytes.len() {
            let mut actual = Vec::new();
            let receipt = read_nals(
                Short {
                    bytes: &bytes,
                    size,
                },
                64,
                |offset, nal| {
                    let start = usize::try_from(offset).map_err(|_| Error::Limit("test_offset"))?;
                    assert_eq!(bytes.get(start..start + nal.len()), Some(nal));
                    actual.push(nal.to_vec());
                    Ok(())
                },
            )?;
            assert_eq!(actual, expected);
            assert_eq!(receipt.bytes, bytes.len() as u64);
        }
        for bytes in [
            &[0, 0, 1][..],
            &[4, 0, 0, 1, 64, 1],
            &[0, 0, 1, 64, 1, 0, 0, 1],
        ] {
            assert!(read_nals(bytes, 64, |_, _| Ok(())).is_err());
        }
        assert!(read_nals(&[0, 0, 1, 64, 1, 5][..], 2, |_, _| Ok(())).is_err());
        Ok(())
    }
}
