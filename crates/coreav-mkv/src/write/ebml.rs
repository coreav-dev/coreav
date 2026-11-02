use crate::Error;
use std::io::Write;

pub(super) fn header(id: u32, size: u64) -> Result<Vec<u8>, Error> {
    let mut out = id_bytes(id);
    let width = (1..=8u32)
        .find(|n| size < (1u64 << (7 * n)) - 1)
        .ok_or(Error::Overflow { offset: 0 })?;
    let bytes = (size | (1u64 << (7 * width))).to_be_bytes();
    out.extend_from_slice(bytes.get(8 - width as usize..).unwrap_or_default());
    Ok(out)
}

pub(super) fn id_bytes(id: u32) -> Vec<u8> {
    let bytes = id.to_be_bytes();
    let start = bytes.iter().take_while(|b| **b == 0).count();
    bytes.get(start..).unwrap_or_default().to_vec()
}

pub(super) fn element(id: u32, payload: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = header(id, payload.len() as u64)?;
    out.extend_from_slice(payload);
    Ok(out)
}

pub(super) fn uint(id: u32, value: u64) -> Result<Vec<u8>, Error> {
    let bytes = value.to_be_bytes();
    let start = bytes.iter().take_while(|b| **b == 0).count().min(7);
    element(id, bytes.get(start..).unwrap_or_default())
}

pub(super) fn sint(id: u32, value: i64) -> Result<Vec<u8>, Error> {
    element(id, &value.to_be_bytes())
}

pub(super) fn write(out: &mut impl Write, bytes: &[u8]) -> Result<(), Error> {
    out.write_all(bytes).map_err(|e| Error::Io(e.kind()))
}
