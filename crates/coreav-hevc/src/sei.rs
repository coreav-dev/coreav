use crate::{Error, Result};

/// A decoded-picture hash over the three coded 4:2:0 components (D.3.20).
/// `HeaderParser` retains these; `IntraPictureDecoder` verifies selected-picture samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureHash {
    /// Y, Cb and Cr MD5 digests.
    Md5([[u8; 16]; 3]),
    /// Y, Cb and Cr CRC values, in bitstream byte order.
    Crc([[u8; 2]; 3]),
    /// Y, Cb and Cr checksums, in bitstream byte order.
    Checksum([[u8; 4]; 3]),
}

/// Bounded SEI inspection without interpreting informational payload bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeiInfo {
    /// Number of messages, including unrecognized informational messages.
    pub messages: u32,
    /// Coded-picture hash if present with a defined hash type.
    pub picture_hash: Option<PictureHash>,
    /// Reserved hash types ignored under D.3.20, not hash coverage.
    pub reserved_hashes: u32,
}

fn extended_byte(data: &mut &[u8]) -> Result<u32> {
    let mut value = 0u32;
    loop {
        let (&byte, rest) = data
            .split_first()
            .ok_or(Error::InvalidData("truncated_sei"))?;
        *data = rest;
        value = value
            .checked_add(u32::from(byte))
            .ok_or(Error::InvalidData("sei_value"))?;
        if byte != 255 {
            return Ok(value);
        }
    }
}

fn components<const N: usize>(bytes: &[u8]) -> Result<[[u8; N]; 3]> {
    if bytes.len() != N * 3 {
        return Err(Error::InvalidData("picture_hash_size"));
    }
    let mut out = [[0; N]; 3];
    for (dest, source) in out.iter_mut().zip(bytes.chunks_exact(N)) {
        dest.copy_from_slice(source);
    }
    Ok(out)
}

pub(crate) fn parse(mut bytes: &[u8], suffix: bool) -> Result<SeiInfo> {
    let mut info = SeiInfo::default();
    let mut hash_seen = false;
    while bytes != [0x80] {
        let kind = extended_byte(&mut bytes)?;
        let size = extended_byte(&mut bytes)? as usize;
        let payload = bytes
            .get(..size)
            .ok_or(Error::InvalidData("truncated_sei_payload"))?;
        bytes = bytes
            .get(size..)
            .ok_or(Error::InvalidData("truncated_sei_payload"))?;
        info.messages = info
            .messages
            .checked_add(1)
            .ok_or(Error::Limit("sei_messages"))?;
        if kind == 132 {
            if !suffix || hash_seen {
                return Err(Error::InvalidData("picture_hash_placement"));
            }
            hash_seen = true;
            let (&hash_type, digest) = payload
                .split_first()
                .ok_or(Error::InvalidData("picture_hash_type"))?;
            info.picture_hash = match hash_type {
                0 => Some(PictureHash::Md5(components(digest)?)),
                1 => Some(PictureHash::Crc(components(digest)?)),
                2 => Some(PictureHash::Checksum(components(digest)?)),
                _ => {
                    info.reserved_hashes += 1;
                    None
                }
            };
        }
    }
    Ok(info)
}
