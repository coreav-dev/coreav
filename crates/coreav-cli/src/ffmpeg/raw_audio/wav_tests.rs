#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;

struct Generated {
    pcm: Pcm,
    reads: usize,
    largest: usize,
    fail_after: Option<usize>,
}

impl Source for Generated {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        Ok(self.pcm.offset + self.pcm.bytes + 50)
    }

    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        assert!(at >= self.pcm.offset);
        assert!(at + out.len() as u64 <= self.pcm.offset + self.pcm.bytes);
        assert!(out.len() <= 65_536);
        if self.fail_after == Some(self.reads) {
            return Err(coreav_isobmff::Error::Io(
                std::io::ErrorKind::PermissionDenied,
            ));
        }
        self.reads += 1;
        self.largest = self.largest.max(out.len());
        out.fill(128);
        Ok(())
    }
}

fn source() -> Generated {
    Generated {
        pcm: Pcm {
            encoding: PcmEncoding::U8,
            valid_bits: 8,
            channels: 1,
            rate: 48000,
            mask: 0,
            offset: 123,
            bytes: 1_000_003,
            declared_bytes: 1_000_003,
        },
        reads: 0,
        largest: 0,
        fail_after: None,
    }
}

#[test]
fn source_reads_only_its_span_in_bounded_chunks() {
    for format in [SampleFormat::S16, SampleFormat::F32] {
        let mut source = source();
        let pcm = source.pcm;
        let mut written = 0;
        let mut largest = 0;
        let result = feed(&mut source, pcm, format, |bytes| {
            assert!(bytes.iter().all(|byte| *byte == 0));
            written += bytes.len();
            largest = largest.max(bytes.len());
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!(written as u64, pcm.bytes * format.bytes() as u64);
        assert!(source.reads > 1);
        assert!(source.largest <= 65_536);
        assert!(largest <= 65_536);
    }
}

#[test]
fn a_late_source_failure_retains_its_io_classification() {
    let mut source = source();
    source.fail_after = Some(1);
    let pcm = source.pcm;
    let mut writes = 0;
    let result = feed(&mut source, pcm, SampleFormat::F32, |_| {
        writes += 1;
        Ok(())
    });
    assert_eq!(writes, 1);
    assert!(matches!(
        result,
        Err(DecodeError::Wave(Failure::Library(
            coreav_isobmff::Error::Io(std::io::ErrorKind::PermissionDenied)
        )))
    ));
}

#[test]
fn source_and_sink_failures_never_become_packing_refusals() {
    let mut source = source();
    let pcm = source.pcm;
    let result = feed(&mut source, pcm, SampleFormat::S16, |_| {
        Err(Error::Output(std::io::ErrorKind::BrokenPipe.into()))
    });
    assert!(result.err().unwrap().terminal().is_io());
    assert_eq!(source.reads, 1);
}
