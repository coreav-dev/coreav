use crate::{Error, Limits, Result};

/// The two-byte NAL header from H.265 clause 7.3.1.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NalHeader {
    /// Six-bit NAL unit type.
    pub unit_type: u8,
    /// Layer identifier; this parser admits only zero.
    pub layer_id: u8,
    /// Temporal identifier, after subtracting one from the syntax value.
    pub temporal_id: u8,
}

impl NalHeader {
    /// Validate a complete NAL header, without an Annex B start code.
    pub fn parse(nal: &[u8]) -> Result<Self> {
        let Some([first, second]) = nal.first_chunk::<2>() else {
            return Err(Error::InvalidData("truncated_nal_header"));
        };
        if first & 0x80 != 0 || second & 7 == 0 {
            return Err(Error::InvalidData("nal_header"));
        }
        let header = Self {
            unit_type: (first >> 1) & 63,
            layer_id: ((first & 1) << 5) | (second >> 3),
            temporal_id: (second & 7) - 1,
        };
        if header.layer_id != 0 {
            return Err(Error::Unsupported("nuh_layer_id"));
        }
        if ((16..=23).contains(&header.unit_type) || matches!(header.unit_type, 32 | 33 | 36 | 37))
            && header.temporal_id != 0
            || (2..=5).contains(&header.unit_type) && header.temporal_id == 0
        {
            return Err(Error::InvalidData("nal_temporal_id"));
        }
        Ok(header)
    }
}

pub(crate) fn rbsp(nal: &[u8], limits: &Limits) -> Result<Vec<u8>> {
    if nal.len() > limits.max_nal_bytes {
        return Err(Error::Limit("max_nal_bytes"));
    }
    let payload = nal
        .get(2..)
        .ok_or(Error::InvalidData("truncated_nal_header"))?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(payload.len())
        .map_err(|_| Error::Limit("rbsp_allocation"))?;
    for byte in Unescape::new(payload) {
        if let Some(byte) = byte? {
            result.push(byte);
        }
    }
    Ok(result)
}

pub(crate) struct Unescape<'a> {
    bytes: core::slice::Iter<'a, u8>,
    zeros: u8,
}

impl<'a> Unescape<'a> {
    pub(crate) fn new(payload: &'a [u8]) -> Self {
        Self {
            bytes: payload.iter(),
            zeros: 0,
        }
    }
}

impl Iterator for Unescape<'_> {
    type Item = Result<Option<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        let &byte = self.bytes.next()?;
        if self.zeros == 2 {
            if byte == 3 {
                if self.bytes.as_slice().first().is_some_and(|next| *next > 3) {
                    return Some(Err(Error::InvalidData("emulation_prevention")));
                }
                self.zeros = 0;
                return Some(Ok(None));
            }
            if byte < 3 {
                return Some(Err(Error::InvalidData("unescaped_start_code")));
            }
        }
        self.zeros = if byte == 0 { self.zeros + 1 } else { 0 };
        Some(Ok(Some(byte)))
    }
}

/// Iterate complete Annex B NAL units without copying their bytes.
pub struct AnnexB<'a> {
    data: &'a [u8],
    started: bool,
    failed: bool,
    empty_after_start: bool,
}

impl<'a> AnnexB<'a> {
    /// The input must be a complete byte stream, including its last NAL.
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            started: false,
            failed: false,
            empty_after_start: false,
        }
    }
}

fn start_code(data: &[u8]) -> Option<(usize, usize)> {
    let mut zeros = 0;
    for (pos, &byte) in data.iter().enumerate() {
        if byte == 1 && zeros >= 2 {
            return Some((pos - zeros, pos + 1));
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    None
}

impl<'a> Iterator for AnnexB<'a> {
    type Item = Result<&'a [u8]>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        if self.empty_after_start {
            self.failed = true;
            return Some(Err(Error::InvalidData("empty_nal")));
        }
        if self.data.is_empty() {
            return None;
        }
        if !self.started {
            let Some((begin, end)) = start_code(self.data) else {
                self.failed = true;
                return Some(Err(Error::InvalidData("annex_b_start_code")));
            };
            if self
                .data
                .get(..begin)
                .is_none_or(|prefix| prefix.iter().any(|b| *b != 0))
            {
                self.failed = true;
                return Some(Err(Error::InvalidData("annex_b_leading_bytes")));
            }
            self.data = self.data.get(end..).unwrap_or_default();
            self.started = true;
        }
        let (end, next) = start_code(self.data).unwrap_or((self.data.len(), self.data.len()));
        self.empty_after_start = end < self.data.len() && next == self.data.len();
        let mut nal = self.data.get(..end).unwrap_or_default();
        while nal.last() == Some(&0) {
            nal = nal.get(..nal.len() - 1).unwrap_or_default();
        }
        self.data = self.data.get(next..).unwrap_or_default();
        if nal.is_empty() {
            self.failed = true;
            return Some(Err(Error::InvalidData("empty_nal")));
        }
        Some(Ok(nal))
    }
}

/// Iterate NAL units framed with one to four big-endian length bytes.
pub struct LengthPrefixed<'a> {
    data: &'a [u8],
    length_bytes: u8,
    failed: bool,
}

impl<'a> LengthPrefixed<'a> {
    /// Reject a length width outside one through four.
    pub fn new(data: &'a [u8], length_bytes: u8) -> Result<Self> {
        if !(1..=4).contains(&length_bytes) {
            return Err(Error::InvalidData("nal_length_width"));
        }
        Ok(Self {
            data,
            length_bytes,
            failed: false,
        })
    }
}

impl<'a> Iterator for LengthPrefixed<'a> {
    type Item = Result<&'a [u8]>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.data.is_empty() {
            return None;
        }
        let result = (|| {
            let prefix = self
                .data
                .get(..usize::from(self.length_bytes))
                .ok_or(Error::InvalidData("truncated_nal_length"))?;
            let len = prefix
                .iter()
                .fold(0usize, |len, byte| (len << 8) | usize::from(*byte));
            if len == 0 {
                return Err(Error::InvalidData("empty_nal"));
            }
            let rest = self
                .data
                .get(prefix.len()..)
                .ok_or(Error::InvalidData("truncated_nal_length"))?;
            let nal = rest.get(..len).ok_or(Error::InvalidData("truncated_nal"))?;
            self.data = rest.get(len..).unwrap_or_default();
            Ok(nal)
        })();
        self.failed = result.is_err();
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::Unescape;
    use crate::Result;

    #[test]
    fn unescape_yields_one_event_per_payload_byte() {
        let events: Result<Vec<_>> = Unescape::new(&[0, 0, 3, 0, 0, 3, 1, 3]).collect();
        assert_eq!(
            events.unwrap(),
            [
                Some(0),
                Some(0),
                None,
                Some(0),
                Some(0),
                None,
                Some(1),
                Some(3)
            ]
        );
    }

    #[test]
    fn terminal_escape_byte_retains_existing_unescape_behavior() {
        let events: Result<Vec<_>> = Unescape::new(&[0, 0, 3]).collect();
        assert_eq!(events.unwrap(), [Some(0), Some(0), None]);
    }
}
