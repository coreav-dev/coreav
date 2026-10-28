use coreav_mkv::{probe, Allowlist, BufferedSource, Limits, SliceSource, Source};
use std::io::Write;

struct Counting<'a> { inner: SliceSource<'a>, bytes: u64 }
impl Source for Counting<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> { self.inner.len() }
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        self.bytes += buf.len() as u64;
        self.inner.read_exact_at(offset, buf)
    }
}

/// Per file: the scan's result whole and cut in half under four work
/// limits (W1), then the bytes the scan reads through BufferedSource (W3).
fn main() {
    let out = std::io::stdout();
    let mut out = out.lock();
    for path in std::env::args().skip(1) {
        let Ok(full) = std::fs::read(&path) else { continue };
        for (cut, data) in [("full", &full[..]), ("half", &full[..full.len() / 2])] {
            for work in [u64::MAX, 1 << 16, 40, 7] {
                let mut limits = Limits::default();
                if work != u64::MAX {
                    limits.max_block_work = work;
                }
                let Ok(segment) = probe(SliceSource::new(data), &limits, &Allowlist::all()) else {
                    writeln!(out, "{path}\t{cut}\t{work}\tprobe failed").unwrap();
                    continue;
                };
                let scan = segment.scan_tracks(SliceSource::new(data), &limits);
                writeln!(out, "{path}\t{cut}\t{work}\t{scan:?}").unwrap();
            }
        }
        let limits = Limits::default();
        if let Ok(segment) = probe(SliceSource::new(&full), &limits, &Allowlist::all()) {
            let mut counting = Counting { inner: SliceSource::new(&full), bytes: 0 };
            let _ = segment.scan_tracks(BufferedSource::new(&mut counting), &limits);
            writeln!(out, "{path}\tbytes\t{}\t{}", full.len(), counting.bytes).unwrap();
        }
    }
}
