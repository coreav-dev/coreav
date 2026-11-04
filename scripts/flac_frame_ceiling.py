"""Build-only encoded-frame replay for the complete frame-work erasure diagnostic."""

import argparse
from pathlib import Path

WRAPPER = r'''impl Stream {
    fn frame(&mut self, input: &[u8], out: &mut impl Write, limits: &Limits) -> Result<(), Error> {
        if let Some(bytes) = DIAGNOSTIC_FRAME_TAPE.with(|tape| tape.borrow_mut().take(limits)) {
            let count = bytes.len() as u32;
            out.write_all(&bytes).map_err(Error::Output)?;
            self.frames += 1;
            self.min_frame = if self.min_frame == 0 { count } else { self.min_frame.min(count) };
            self.max_frame = self.max_frame.max(count);
            return Ok(());
        }
        if DIAGNOSTIC_FRAME_TAPE.with(|tape| matches!(*tape.borrow(), DiagnosticFrameTape::Capture(_))) {
            let mut bytes = Vec::new();
            self.diagnostic_original_frame(input, &mut bytes, limits)?;
            DIAGNOSTIC_FRAME_TAPE.with(|tape| tape.borrow_mut().record(&bytes));
            out.write_all(&bytes).map_err(Error::Output)
        } else {
            self.diagnostic_original_frame(input, out, limits)
        }
    }

    fn diagnostic_original_frame(&mut self, input: &[u8], out: &mut impl Write, limits: &Limits) -> Result<(), Error> {'''

HOOK = r'''
enum DiagnosticFrameTape {
    Off,
    Capture(std::io::BufWriter<std::fs::File>),
    Replay { input: std::io::BufReader<std::fs::File>, remaining: u64 },
}

impl DiagnosticFrameTape {
    fn open() -> Self {
        match (std::env::var_os("COREAV_FRAME_CAPTURE"), std::env::var_os("COREAV_FRAME_REPLAY")) {
            (None, None) => Self::Off,
            (Some(path), None) => Self::Capture(std::io::BufWriter::new(
                std::fs::OpenOptions::new().write(true).create_new(true).open(path).unwrap())),
            (None, Some(path)) => {
                let file = std::fs::File::open(path).unwrap();
                let remaining = file.metadata().unwrap().len();
                Self::Replay { input: std::io::BufReader::new(file), remaining }
            }
            _ => panic!("capture and replay are mutually exclusive"),
        }
    }

    fn take(&mut self, limits: &Limits) -> Option<Vec<u8>> {
        use std::io::Read as _;
        let Self::Replay { input, remaining } = self else { return None };
        let mut count = [0u8; 4];
        input.read_exact(&mut count).unwrap();
        let count = u32::from_le_bytes(count) as usize;
        assert!(count != 0 && count as u64 <= limits.max_packet_bytes);
        let mut bytes = vec![0u8; count];
        input.read_exact(&mut bytes).unwrap();
        *remaining = remaining.checked_sub(4 + count as u64).unwrap();
        if *remaining == 0 { eprintln!("Frame decision tape fully consumed"); }
        Some(bytes)
    }

    fn record(&mut self, bytes: &[u8]) {
        use std::io::Write as _;
        let Self::Capture(output) = self else { return };
        output.write_all(&(bytes.len() as u32).to_le_bytes()).unwrap();
        output.write_all(bytes).unwrap();
        output.flush().unwrap();
    }
}

std::thread_local! {
    static DIAGNOSTIC_FRAME_TAPE: std::cell::RefCell<DiagnosticFrameTape> =
        std::cell::RefCell::new(DiagnosticFrameTape::open());
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--instrument', required=True, type=Path)
    args = parser.parse_args()
    original = args.instrument.read_text()
    marker = 'impl Stream {\n    fn frame(&mut self, input: &[u8], out: &mut impl Write, limits: &Limits) -> Result<(), Error> {'
    if original.count(marker) != 1:
        raise RuntimeError('Frame boundary differs from the diagnostic baseline')
    args.instrument.write_text(original.replace(marker, WRAPPER, 1) + HOOK)


if __name__ == '__main__':
    main()
