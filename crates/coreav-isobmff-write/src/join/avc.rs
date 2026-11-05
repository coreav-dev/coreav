use coreav_isobmff::{Error, LimitKind};
use std::io::{self, Write};

pub(super) fn length_size(record: &[u8]) -> Result<usize, Error> {
    let invalid = || Error::Unsupported {
        reason: "an AVC configuration without complete sample framing",
    };
    if record.first() != Some(&1) {
        return Err(invalid());
    }
    let size = usize::from(record.get(4).ok_or_else(invalid)? & 3) + 1;
    let sets = coreav_isobmff::avc_parameter_sets(record).ok_or_else(invalid)?;
    if size == 3
        || ![7, 8].iter().all(|kind| {
            sets.iter()
                .any(|set| set.first().is_some_and(|header| header & 31 == *kind))
        })
    {
        return Err(invalid());
    }
    Ok(size)
}

pub(super) struct Sample<'a, W> {
    out: &'a mut W,
    width: usize,
    prefix: usize,
    length: u32,
    header: bool,
    payload: u32,
    remaining: u64,
    work: &'a mut u64,
    limit: u64,
    pub(super) error: Option<Error>,
}

impl<'a, W: Write> Sample<'a, W> {
    pub(super) fn new(
        out: &'a mut W,
        width: usize,
        size: u32,
        work: &'a mut u64,
        limit: u64,
    ) -> Self {
        Self {
            out,
            width,
            prefix: 0,
            length: 0,
            header: false,
            payload: 0,
            remaining: u64::from(size),
            work,
            limit,
            error: None,
        }
    }

    pub(super) fn finish(&self) -> Result<(), Error> {
        if self.remaining != 0 || self.prefix != 0 || self.header || self.payload != 0 {
            return Err(Self::invalid());
        }
        Ok(())
    }

    fn invalid() -> Error {
        Error::Unsupported {
            reason: "an AVC sample with invalid NAL framing",
        }
    }

    fn inspect(&mut self, mut bytes: &[u8]) -> Result<(), Error> {
        while !bytes.is_empty() {
            if self.payload > 0 {
                let skip = bytes.len().min(self.payload as usize);
                bytes = bytes.get(skip..).ok_or_else(Self::invalid)?;
                self.payload -= u32::try_from(skip).map_err(|_| Self::invalid())?;
                self.remaining = self.remaining.saturating_sub(skip as u64);
                continue;
            }
            let (&byte, tail) = bytes.split_first().ok_or_else(Self::invalid)?;
            bytes = tail;
            self.remaining = self.remaining.checked_sub(1).ok_or_else(Self::invalid)?;
            if self.header {
                if matches!(byte & 31, 7 | 8 | 13 | 15) {
                    return Err(Error::Unsupported {
                        reason: "AVC parameter sets carried inside avc1 samples",
                    });
                }
                if byte & 0x80 != 0 || byte & 31 == 0 {
                    return Err(Self::invalid());
                }
                self.payload = self.length.saturating_sub(1);
                self.header = false;
                self.length = 0;
                continue;
            }
            self.length = (self.length << 8) | u32::from(byte);
            self.prefix += 1;
            if self.prefix == self.width {
                if self.length == 0 || u64::from(self.length) > self.remaining {
                    return Err(Self::invalid());
                }
                *self.work = self.work.saturating_add(1);
                if *self.work > self.limit {
                    return Err(Error::LimitExceeded {
                        limit: LimitKind::SampleWork,
                        offset: 0,
                    });
                }
                self.prefix = 0;
                self.header = true;
            }
        }
        Ok(())
    }
}

impl<W: Write> Write for Sample<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Err(error) = self.inspect(bytes) {
            self.error = Some(error);
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        self.out.write_all(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(nal: &[u8], width: usize) -> Vec<u8> {
        let mut bytes = (nal.len() as u32).to_be_bytes()[4 - width..].to_vec();
        bytes.extend_from_slice(nal);
        bytes
    }

    #[test]
    fn every_prefix_and_payload_split_preserves_the_complete_sample() {
        for width in [1, 2, 4] {
            let bytes = [sample(&[0x65, 3, 4], width), sample(&[0x06, 8, 9], width)].concat();
            for split in 0..=bytes.len() {
                let mut out = Vec::new();
                let mut work = 0;
                let mut observer = Sample::new(&mut out, width, bytes.len() as u32, &mut work, 2);
                observer.write_all(&bytes[..split]).unwrap();
                observer.write_all(&bytes[split..]).unwrap();
                observer.finish().unwrap();
                assert_eq!(out, bytes);
                assert_eq!(work, 2);
            }
        }
    }

    #[test]
    fn parameter_sets_are_rejected_even_across_a_copy_window() {
        for kind in [7, 8, 13, 15] {
            let bytes = [sample(&vec![0x61; (64 << 10) - 6], 4), sample(&[kind, 1], 4)].concat();
            let mut out = Vec::new();
            let mut work = 0;
            let mut observer = Sample::new(&mut out, 4, bytes.len() as u32, &mut work, 2);
            assert!(observer.write_all(&bytes[..64 << 10]).is_ok());
            assert!(observer.write_all(&bytes[64 << 10..]).is_err());
            assert!(matches!(observer.error, Some(Error::Unsupported { reason: "AVC parameter sets carried inside avc1 samples" })));
        }
    }

    #[test]
    fn malformed_extents_and_work_exhaustion_fail_closed() {
        for bytes in [vec![0, 0, 0, 0], vec![0, 0, 0, 2, 0x65], vec![0, 0, 0]] {
            let mut out = Vec::new();
            let mut work = 0;
            let mut observer = Sample::new(&mut out, 4, bytes.len() as u32, &mut work, 10);
            let written = observer.write_all(&bytes);
            assert!(written.is_err() || observer.finish().is_err());
        }
        let bytes = sample(&[0x65, 0], 4);
        let mut out = Vec::new();
        let mut work = 0;
        let mut observer = Sample::new(&mut out, 4, bytes.len() as u32, &mut work, 0);
        assert!(observer.write_all(&bytes).is_err());
        assert!(matches!(observer.error, Some(Error::LimitExceeded { limit: LimitKind::SampleWork, .. })));
    }
}
