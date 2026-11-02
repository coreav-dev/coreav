use super::Error;

pub(super) fn rbsp(nal: &[u8]) -> Result<Vec<u8>, Error> {
    let (_, payload) = nal.split_first().ok_or(Error::Malformed)?;
    if payload.is_empty() || payload.last() == Some(&0) {
        return Err(Error::Malformed);
    }
    let mut out = Vec::new();
    out.try_reserve_exact(payload.len())
        .map_err(|_| Error::Allocation)?;
    let mut zeros = 0;
    let mut bytes = payload.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        if zeros == 2 {
            if byte < 3 {
                return Err(Error::Malformed);
            }
            if byte == 3 {
                if bytes.peek().is_some_and(|&next| next > 3) {
                    return Err(Error::Malformed);
                }
                zeros = 0;
                continue;
            }
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    Ok(out)
}

pub(super) struct Bits<'a> {
    data: &'a [u8],
    position: usize,
    end: usize,
}

impl<'a> Bits<'a> {
    pub(super) fn new(data: &'a [u8]) -> Result<Self, Error> {
        Ok(Self {
            data,
            position: 0,
            end: data.len().checked_mul(8).ok_or(Error::Limit)?,
        })
    }

    pub(super) fn bit(&mut self) -> Result<bool, Error> {
        let byte = self.data.get(self.position / 8).ok_or(Error::Malformed)?;
        let value = (byte >> (7 - self.position % 8)) & 1 != 0;
        self.position += 1;
        Ok(value)
    }

    pub(super) fn bits(&mut self, count: u8) -> Result<u32, Error> {
        if count > 32 {
            return Err(Error::Malformed);
        }
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | u32::from(self.bit()?);
        }
        Ok(value)
    }

    pub(super) fn ue(&mut self) -> Result<u32, Error> {
        let mut zeros = 0_u8;
        while !self.bit()? {
            zeros += 1;
            if zeros == 32 {
                return Err(Error::Malformed);
            }
        }
        Ok(((1_u32 << zeros) - 1) + self.bits(zeros)?)
    }

    pub(super) fn bounded(&mut self, max: u32) -> Result<u32, Error> {
        let value = self.ue()?;
        if value > max {
            return Err(Error::Malformed);
        }
        Ok(value)
    }

    pub(super) fn se(&mut self) -> Result<i32, Error> {
        let code = i64::from(self.ue()?);
        let value = if code % 2 == 0 {
            -code / 2
        } else {
            (code + 1) / 2
        };
        i32::try_from(value).map_err(|_| Error::Malformed)
    }

    pub(super) fn signed(&mut self, min: i32, max: i32) -> Result<(), Error> {
        if !(min..=max).contains(&self.se()?) {
            return Err(Error::Malformed);
        }
        Ok(())
    }

    pub(super) fn more(&self) -> bool {
        if self.position >= self.end {
            return false;
        }
        let mut copy = Self {
            data: self.data,
            position: self.position,
            end: self.end,
        };
        if copy.bit() != Ok(true) {
            return true;
        }
        while copy.position < copy.end {
            if copy.bit() != Ok(false) {
                return true;
            }
        }
        false
    }

    pub(super) fn finish(&mut self) -> Result<(), Error> {
        if !self.bit()? {
            return Err(Error::Malformed);
        }
        while self.position % 8 != 0 {
            if self.bit()? {
                return Err(Error::Malformed);
            }
        }
        if self.position != self.end {
            return Err(Error::Malformed);
        }
        Ok(())
    }
}
