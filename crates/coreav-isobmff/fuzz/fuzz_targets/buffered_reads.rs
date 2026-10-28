//! Arbitrary reads through `BufferedSource` (docs/design/read-window.md):
//! the input is the file, and its first 8 bytes seed a sequence of reads
//! (forward, backward, small, large, past the end, released) through
//! `read_exact_at` and `read_append`. Every read that succeeds returns the
//! file's bytes, never what an earlier fill left in the window. When the
//! inner source is honest, a read succeeds exactly when the file holds it;
//! in odd seeds it claims more bytes than it has and fails the reads that
//! reach past them, after writing what it has.
#![no_main]

use coreav_isobmff::{BufferedSource, Error, Source};
use libfuzzer_sys::fuzz_target;

struct Inner<'a> {
    data: &'a [u8],
    claimed: u64,
}

impl Source for Inner<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.claimed)
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(self.data.len());
        let end = start.saturating_add(buf.len()).min(self.data.len());
        let have = &self.data[start..end];
        buf[..have.len()].copy_from_slice(have);
        if have.len() < buf.len() || offset > self.data.len() as u64 {
            return Err(Error::Truncated { offset });
        }
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    let mut state = data
        .iter()
        .take(8)
        .fold(0x9e37_79b9_7f4a_7c15u64, |s, b| s.rotate_left(8) ^ u64::from(*b));
    let mut next = |bound: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % bound.max(1)
    };
    let len = data.len() as u64;
    let honest = next(2) == 0;
    let claimed = if honest { len } else { len + 1 + next(100_000) };
    let mut source = BufferedSource::new(Inner { data, claimed });
    let mut at = 0u64;
    for _ in 0..64 {
        at = match next(8) {
            0 => next(claimed + 100),
            1 => at.saturating_sub(next(100_000)),
            2 => {
                source.release();
                at
            }
            _ => at + next(5000),
        };
        let size = match next(16) {
            0 => next(300 << 10),
            1 => next(2 << 20),
            _ => next(600),
        } as usize;
        // An empty read succeeds wherever it starts.
        let truth = match size {
            0 => Some(&[][..]),
            _ => usize::try_from(at)
                .ok()
                .and_then(|start| data.get(start..start.checked_add(size)?)),
        };
        let result = if next(2) == 0 {
            let mut buf = vec![0u8; size];
            source.read_exact_at(at, &mut buf).map(|()| buf)
        } else {
            let mut buf = vec![7u8];
            source
                .read_append(at, size, &mut buf)
                .map(|()| buf.split_off(1))
        };
        match (result, truth) {
            (Ok(bytes), Some(truth)) => assert_eq!(bytes, truth),
            (Ok(_), None) => panic!("a read past the file's bytes succeeded"),
            (Err(_), Some(_)) => assert!(!honest, "a read of the file's bytes failed"),
            (Err(_), None) => {}
        }
    }
});
