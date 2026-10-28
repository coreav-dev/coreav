//! Bounds-checked big-endian reading over byte slices.

use crate::{Error, FourCc};

/// A cursor over a byte slice. Every read checks the remaining length and
/// reports failures at the absolute file offset of the slice start.
#[derive(Clone, Debug)]
pub(crate) struct Bytes<'a> {
    data: &'a [u8],
    /// Absolute file offset of `data[0]`, for error reports.
    base: u64,
}

impl<'a> Bytes<'a> {
    pub(crate) fn new(data: &'a [u8], base: u64) -> Self {
        Self { data, base }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len()
    }

    pub(crate) fn offset(&self) -> u64 {
        self.base
    }

    pub(crate) fn rest(&self) -> &'a [u8] {
        self.data
    }

    fn truncated(&self) -> Error {
        Error::Truncated { offset: self.base }
    }

    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        let (head, tail) = self.data.split_at_checked(count).ok_or(self.truncated())?;
        self.data = tail;
        self.base = self.base.saturating_add(count as u64);
        Ok(head)
    }

    pub(crate) fn skip(&mut self, count: usize) -> Result<(), Error> {
        self.take(count).map(|_| ())
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let bytes = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(u8::from_be_bytes(self.array()?))
    }

    pub(crate) fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub(crate) fn u24(&mut self) -> Result<u32, Error> {
        let [a, b, c] = self.array::<3>()?;
        Ok(u32::from_be_bytes([0, a, b, c]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    pub(crate) fn i64(&mut self) -> Result<i64, Error> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    pub(crate) fn fourcc(&mut self) -> Result<FourCc, Error> {
        Ok(FourCc(self.array()?))
    }

    /// Reads a full-box version and flags.
    pub(crate) fn version_flags(&mut self) -> Result<(u8, u32), Error> {
        let version = self.u8()?;
        let flags = self.u24()?;
        Ok((version, flags))
    }
}

/// Reads a big-endian `u32` at `index` of a slice, if it fits.
pub(crate) fn be_u32_at(data: &[u8], index: usize) -> Option<u32> {
    let end = index.checked_add(4)?;
    let bytes: [u8; 4] = data.get(index..end)?.try_into().ok()?;
    Some(u32::from_be_bytes(bytes))
}

/// Reads a big-endian `u64` at `index` of a slice, if it fits.
pub(crate) fn be_u64_at(data: &[u8], index: usize) -> Option<u64> {
    let end = index.checked_add(8)?;
    let bytes: [u8; 8] = data.get(index..end)?.try_into().ok()?;
    Some(u64::from_be_bytes(bytes))
}

/// Reads a big-endian `u16` at `index` of a slice, if it fits.
pub(crate) fn be_u16_at(data: &[u8], index: usize) -> Option<u16> {
    let end = index.checked_add(2)?;
    let bytes: [u8; 2] = data.get(index..end)?.try_into().ok()?;
    Some(u16::from_be_bytes(bytes))
}

/// A most-significant-bit-first bit reader for codec headers.
#[derive(Clone, Debug)]
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub(crate) fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.position / 8)?;
        let shift = 7 - (self.position % 8);
        self.position = self.position.checked_add(1)?;
        Some(u32::from((byte >> shift) & 1))
    }

    pub(crate) fn bits(&mut self, count: u32) -> Option<u32> {
        if count > 32 {
            return None;
        }
        let mut value: u32 = 0;
        for _ in 0..count {
            value = (value << 1) | self.bit()?;
        }
        Some(value)
    }

    pub(crate) fn flag(&mut self) -> Option<bool> {
        Some(self.bit()? == 1)
    }

    pub(crate) fn skip(&mut self, count: u32) -> Option<()> {
        for _ in 0..count {
            self.bit()?;
        }
        Some(())
    }

    /// Unsigned Exp-Golomb code (ITU-T H.265 9.2), limited to 32-bit values.
    pub(crate) fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0u32;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let rest = self.bits(zeros)?;
        let base = (1u64 << zeros) - 1;
        u32::try_from(base + u64::from(rest)).ok()
    }

    /// Signed Exp-Golomb code; only the magnitude is used by callers.
    pub(crate) fn se(&mut self) -> Option<i64> {
        let code = i64::from(self.ue()?);
        Some(if code % 2 == 1 {
            (code + 1) / 2
        } else {
            -(code / 2)
        })
    }
}
