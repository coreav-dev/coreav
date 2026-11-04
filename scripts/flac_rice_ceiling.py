"""Build-only instrumentation for byte-identical Rice-planning erasure experiments."""

import argparse
from pathlib import Path


HOOK = r'''
enum DiagnosticRiceTape {
    Off,
    Capture(std::io::BufWriter<std::fs::File>),
    Replay { input: std::io::BufReader<std::fs::File>, remaining: u64 },
}

impl DiagnosticRiceTape {
    fn open() -> Self {
        match (std::env::var_os("COREAV_RICE_CAPTURE"), std::env::var_os("COREAV_RICE_REPLAY")) {
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

    fn take(&mut self) -> Option<Rice> {
        use std::io::Read as _;
        let Self::Replay { input, remaining } = self else { return None };
        let mut header = [0u8; 12];
        input.read_exact(&mut header).unwrap();
        let parameter_bits = header[0];
        let order = header[1];
        let count = usize::from(u16::from_le_bytes(header[2..4].try_into().unwrap()));
        assert!((4..=5).contains(&parameter_bits) && order <= 4 && count == 1usize << order);
        let cost = u64::from_le_bytes(header[4..12].try_into().unwrap());
        let mut encoded = vec![0u8; count];
        input.read_exact(&mut encoded).unwrap();
        *remaining = remaining.checked_sub(12 + count as u64).unwrap();
        if *remaining == 0 {
            eprintln!("Rice decision tape fully consumed");
        }
        let parameters = encoded.into_iter().map(|value| {
            if value & 128 == 0 {
                assert!(value <= 30);
                Partition::Rice(value)
            } else {
                assert!(value & 127 <= 31);
                Partition::Escape(value & 127)
            }
        }).collect();
        Some(Rice { parameter_bits, order, parameters, cost })
    }

    fn record(&mut self, rice: &Rice) {
        use std::io::Write as _;
        let Self::Capture(output) = self else { return };
        let mut encoded = vec![rice.parameter_bits, rice.order];
        encoded.extend_from_slice(&(rice.parameters.len() as u16).to_le_bytes());
        encoded.extend_from_slice(&rice.cost.to_le_bytes());
        for value in &rice.parameters {
            encoded.push(match value {
                Partition::Rice(k) => *k,
                Partition::Escape(width) => 128 | *width,
            });
        }
        output.write_all(&encoded).unwrap();
        output.flush().unwrap();
    }
}

std::thread_local! {
    static DIAGNOSTIC_RICE_TAPE: std::cell::RefCell<DiagnosticRiceTape> =
        std::cell::RefCell::new(DiagnosticRiceTape::open());
}

impl Rice {
    fn plan(residual: &[u32], samples: usize, predictor: usize) -> Self {
        if let Some(rice) = DIAGNOSTIC_RICE_TAPE.with(|tape| tape.borrow_mut().take()) {
            return rice;
        }
        let rice = Self::diagnostic_original_plan(residual, samples, predictor);
        DIAGNOSTIC_RICE_TAPE.with(|tape| tape.borrow_mut().record(&rice));
        rice
    }
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--instrument", required=True, type=Path)
    args = parser.parse_args()
    original = args.instrument.read_text()
    marker = "impl Rice {\n    fn plan(residual: &[u32], samples: usize, predictor: usize) -> Self {"
    if original.count(marker) != 1:
        raise RuntimeError("Rice planning boundary differs from the diagnostic baseline")
    args.instrument.write_text(original.replace(marker, HOOK +
        "    fn diagnostic_original_plan(residual: &[u32], samples: usize, predictor: usize) -> Self {", 1))


if __name__ == "__main__":
    main()
