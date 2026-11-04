"""Build-only selected-subframe replay for a whole-planning erasure diagnostic."""

import argparse
from pathlib import Path


WRAPPER = r'''impl Subframe {
    fn plan(input: &[i64], bits: u8, residual: &mut Vec<u32>) -> Self {
        if let Some(plan) = DIAGNOSTIC_SUBFRAME_TAPE.with(|tape| tape.borrow_mut().take(input, bits)) {
            return plan;
        }
        let plan = Self::diagnostic_original_plan(input, bits, residual);
        DIAGNOSTIC_SUBFRAME_TAPE.with(|tape| tape.borrow_mut().record(&plan));
        plan
    }

    fn diagnostic_original_plan(input: &[i64], bits: u8, residual: &mut Vec<u32>) -> Self {'''

HOOK = r'''
enum DiagnosticSubframeTape {
    Off,
    Capture(std::io::BufWriter<std::fs::File>),
    Replay { input: std::io::BufReader<std::fs::File>, remaining: u64 },
}

fn diagnostic_take<const N: usize>(input: &mut std::io::Cursor<Vec<u8>>) -> [u8; N] {
    use std::io::Read as _;
    let mut value = [0; N];
    input.read_exact(&mut value).unwrap();
    value
}

impl DiagnosticSubframeTape {
    fn open() -> Self {
        match (std::env::var_os("COREAV_SUBFRAME_CAPTURE"), std::env::var_os("COREAV_SUBFRAME_REPLAY")) {
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

    fn take(&mut self, samples: &[i64], input_bits: u8) -> Option<Subframe> {
        use std::io::Read as _;
        let Self::Replay { input, remaining } = self else { return None };
        let mut size = [0u8; 4];
        input.read_exact(&mut size).unwrap();
        let size = u32::from_le_bytes(size) as usize;
        assert!(size <= 65536);
        let mut packet = vec![0u8; size];
        input.read_exact(&mut packet).unwrap();
        *remaining = remaining.checked_sub(4 + size as u64).unwrap();
        let mut cursor = std::io::Cursor::new(packet);
        let [bits, wasted, kind, parameter, shift] = diagnostic_take(&mut cursor);
        assert!(bits != 0 && u16::from(bits) + u16::from(wasted) == u16::from(input_bits));
        let count = u32::from_le_bytes(diagnostic_take(&mut cursor)) as usize;
        assert!(count == samples.len());
        let cost = u64::from_le_bytes(diagnostic_take(&mut cursor));
        let [parameter_bits, order] = diagnostic_take(&mut cursor);
        assert!((4..=5).contains(&parameter_bits) && order <= 4);
        let rice_cost = u64::from_le_bytes(diagnostic_take(&mut cursor));
        let parameters_count = usize::from(u16::from_le_bytes(diagnostic_take(&mut cursor)));
        assert!(parameters_count <= 16);
        let predictor = match kind {
            0 => Predictor::Constant,
            1 => Predictor::Verbatim,
            2 => { assert!(parameter <= 4); Predictor::Fixed(parameter) },
            3 => {
                assert!((1..=8).contains(&parameter) && shift <= 15);
                let coefficients = (0..parameter).map(|_| i16::from_le_bytes(diagnostic_take(&mut cursor))).collect();
                Predictor::Linear { coefficients, shift }
            }
            _ => panic!("unknown predictor"),
        };
        let parameters = (0..parameters_count).map(|_| {
            let [value] = diagnostic_take(&mut cursor);
            if value & 128 == 0 {
                assert!(value <= 30);
                Partition::Rice(value)
            } else {
                assert!(value & 127 <= 31);
                Partition::Escape(value & 127)
            }
        }).collect();
        let residual_count = u32::from_le_bytes(diagnostic_take(&mut cursor)) as usize;
        assert!(residual_count <= count);
        let residual = (0..residual_count).map(|_| u32::from_le_bytes(diagnostic_take(&mut cursor))).collect();
        assert!(cursor.position() == size as u64);
        if *remaining == 0 {
            eprintln!("Subframe decision tape fully consumed");
        }
        Some(Subframe {
            bits, wasted, samples: samples.iter().map(|value| *value >> wasted).collect(),
            predictor, residual, cost,
            rice: Rice { parameter_bits, order, parameters, cost: rice_cost },
        })
    }

    fn record(&mut self, plan: &Subframe) {
        use std::io::Write as _;
        let Self::Capture(output) = self else { return };
        let (kind, parameter, shift) = match &plan.predictor {
            Predictor::Constant => (0, 0, 0),
            Predictor::Verbatim => (1, 0, 0),
            Predictor::Fixed(order) => (2, *order, 0),
            Predictor::Linear { coefficients, shift } => (3, coefficients.len() as u8, *shift),
        };
        let mut packet = vec![plan.bits, plan.wasted, kind, parameter, shift];
        packet.extend_from_slice(&(plan.samples.len() as u32).to_le_bytes());
        packet.extend_from_slice(&plan.cost.to_le_bytes());
        packet.extend_from_slice(&[plan.rice.parameter_bits, plan.rice.order]);
        packet.extend_from_slice(&plan.rice.cost.to_le_bytes());
        packet.extend_from_slice(&(plan.rice.parameters.len() as u16).to_le_bytes());
        if let Predictor::Linear { coefficients, .. } = &plan.predictor {
            for coefficient in coefficients { packet.extend_from_slice(&coefficient.to_le_bytes()); }
        }
        for value in &plan.rice.parameters {
            packet.push(match value { Partition::Rice(k) => *k, Partition::Escape(width) => 128 | *width });
        }
        packet.extend_from_slice(&(plan.residual.len() as u32).to_le_bytes());
        for value in &plan.residual { packet.extend_from_slice(&value.to_le_bytes()); }
        assert!(packet.len() <= 65536);
        output.write_all(&(packet.len() as u32).to_le_bytes()).unwrap();
        output.write_all(&packet).unwrap();
        output.flush().unwrap();
    }
}

std::thread_local! {
    static DIAGNOSTIC_SUBFRAME_TAPE: std::cell::RefCell<DiagnosticSubframeTape> =
        std::cell::RefCell::new(DiagnosticSubframeTape::open());
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--instrument', required=True, type=Path)
    args = parser.parse_args()
    original = args.instrument.read_text()
    marker = 'impl Subframe {\n    fn plan(input: &[i64], bits: u8, residual: &mut Vec<u32>) -> Self {'
    if original.count(marker) != 1:
        raise RuntimeError('Subframe planning boundary differs from the diagnostic baseline')
    args.instrument.write_text(original.replace(marker, WRAPPER, 1) + HOOK)


if __name__ == '__main__':
    main()
