use coreav_hevc::{AnnexB, Error, Result};
use std::io::Read;

pub const INPUT_LIMIT: u64 = 4 << 30;
pub const CHUNK: usize = 65536;

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
