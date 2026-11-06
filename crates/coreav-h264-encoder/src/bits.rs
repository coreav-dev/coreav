#![forbid(unsafe_code)]

use crate::{Error, Result};

pub(crate) struct Bits {
    bytes: Option<Vec<u8>>,
    pub(crate) len: usize,
    limit: usize,
}

impl Bits {
    pub(crate) fn count() -> Self {
        Self {
            bytes: None,
            len: 0,
            limit: usize::MAX,
        }
    }

    pub(crate) fn new(limit: usize) -> Self {
        Self {
            bytes: Some(Vec::new()),
            len: 0,
            limit,
        }
    }

    pub(crate) fn u(&mut self, width: u32, value: u64) -> Result<()> {
        if width > 64 || (width < 64 && value >> width != 0) {
            return Err(Error::Syntax);
        }
        let end = self.len.checked_add(width as usize).ok_or(Error::Limit)?;
        if end.div_ceil(8) > self.limit {
            return Err(Error::Limit);
        }
        if let Some(bytes) = &mut self.bytes {
            bytes
                .try_reserve(end.div_ceil(8).saturating_sub(bytes.len()))
                .map_err(|_| Error::Allocation)?;
            let mut remaining = width;
            while remaining != 0 {
                let used = (self.len % 8) as u32;
                if used == 0 {
                    bytes.push(0);
                }
                let take = remaining.min(8 - used);
                let chunk = ((value >> (remaining - take)) & ((1 << take) - 1)) as u8;
                *bytes.last_mut().ok_or(Error::Internal)? |= chunk << (8 - used - take);
                self.len += take as usize;
                remaining -= take;
            }
        } else {
            self.len = end;
        }
        Ok(())
    }

    pub(crate) fn flag(&mut self, value: bool) -> Result<()> {
        self.u(1, u64::from(value))
    }

    pub(crate) fn ue(&mut self, value: u32) -> Result<()> {
        let number = u64::from(value) + 1;
        let width = 64 - number.leading_zeros();
        self.u(width - 1, 0)?;
        self.u(width, number)
    }

    pub(crate) fn se(&mut self, value: i32) -> Result<()> {
        let mapped = if value <= 0 {
            -2 * i64::from(value)
        } else {
            2 * i64::from(value) - 1
        };
        self.ue(u32::try_from(mapped).map_err(|_| Error::Syntax)?)
    }

    pub(crate) fn code(&mut self, packed: u32) -> Result<()> {
        let width = packed & 31;
        if width == 0 {
            return Err(Error::Syntax);
        }
        self.u(width, u64::from(packed >> 5))
    }

    pub(crate) fn nal(mut self, header: u8) -> Result<Vec<u8>> {
        self.flag(true)?;
        while self.len % 8 != 0 {
            self.flag(false)?;
        }
        let bytes = self.bytes.ok_or(Error::Internal)?;
        let mut output = Vec::new();
        output
            .try_reserve(bytes.len().saturating_mul(3) / 2 + 1)
            .map_err(|_| Error::Allocation)?;
        output.push(header);
        let mut zeros = 0;
        for b in bytes {
            if zeros == 2 && b <= 3 {
                output.push(3);
                zeros = 0;
            }
            output.push(b);
            zeros = if b == 0 { zeros + 1 } else { 0 };
            if output.len() > self.limit {
                return Err(Error::Limit);
            }
        }
        Ok(output)
    }
}
