import argparse
from pathlib import Path


WORD_BITS = '''struct Bits {
    bytes: Vec<u8>,
    partial: u64,
    used: u8,
}

impl Bits {
    fn new() -> Self {
        Self { bytes: Vec::new(), partial: 0, used: 0 }
    }

    fn put(&mut self, mut count: u8, value: u64) {
        while count > 0 {
            let take = count.min(64 - self.used);
            count -= take;
            let value = if take == 64 { value } else { (value >> count) & ((1u64 << take) - 1) };
            self.partial |= value << (64 - self.used - take);
            self.used += take;
            if self.used == 64 {
                self.bytes.extend_from_slice(&self.partial.to_be_bytes());
                self.partial = 0;
                self.used = 0;
            }
        }
    }

    fn unary(&mut self, mut zeros: u64) {
        if self.used != 0 && zeros > 0 {
            let take = zeros.min(u64::from(64 - self.used)) as u8;
            self.put(take, 0);
            zeros -= u64::from(take);
        }
        self.bytes.resize(self.bytes.len() + (zeros / 8) as usize, 0);
        self.put((zeros % 8) as u8, 0);
        self.put(1, 1);
    }

    fn align(&mut self) {
        if self.used != 0 {
            self.bytes.extend(self.partial.to_be_bytes().into_iter().take(usize::from(self.used.div_ceil(8))));
            self.partial = 0;
            self.used = 0;
        }
    }
}

'''

CONSIDER = '''    #[cfg(test)]
    fn consider(&mut self, predictor: Predictor, overhead: u64, residual: &mut Vec<u32>) {
        self.consider_limited(predictor, overhead, residual, u64::MAX);
    }

    fn consider_limited(&mut self, predictor: Predictor, overhead: u64, residual: &mut Vec<u32>, limit: u64) {
        let order = match &predictor {
            Predictor::Fixed(order) => usize::from(*order),
            Predictor::Linear { coefficients, .. } => coefficients.len(),
            _ => return,
        };
        let extra = if matches!(predictor, Predictor::Linear { .. }) { 9 + 15 * order as u64 } else { 0 };
        let base = overhead + order as u64 * u64::from(self.bits) + extra;
        let threshold = self.cost.min(limit.saturating_add(1));
        if base + 10 >= threshold {
            return;
        }
        if predictor.residual_limited(&self.samples, residual, threshold - base - 10).is_none() {
            return;
        }
        let rice = Rice::plan(residual, self.samples.len(), order);
        let cost = base + rice.cost;
        if cost < self.cost {
            self.predictor = predictor;
            std::mem::swap(&mut self.residual, residual);
            self.rice = rice;
            self.cost = cost;
        }
    }

'''

WORD_TEST = '''
    #[test]
    fn feasibility_word_bits_match_scalar_bits() {
        let mut output = Bits::new();
        let mut scalar = Vec::new();
        let mut state = 17u64;
        for repetition in 0..20 {
            for count in [0, 1, 7, 8, 9, 15, 16, 24, 31, 32, 63, 64] {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                output.put(count, state);
                scalar.extend((0..count).rev().map(|bit| ((state >> bit) & 1) as u8));
                let zeros = [0, 1, 7, 8, 63, 64, 129, 1000][repetition % 8];
                output.unary(zeros);
                scalar.extend(std::iter::repeat_n(0, zeros as usize));
                scalar.push(1);
                assert_eq!(output.bytes.len() * 8 + usize::from(output.used), scalar.len());
            }
        }
        output.align();
        scalar.resize(scalar.len().div_ceil(8) * 8, 0);
        let expected: Vec<u8> = scalar.chunks_exact(8).map(|bits| bits.iter().fold(0, |a, b| (a << 1) | b)).collect();
        assert_eq!(output.bytes, expected);
    }
'''

SCREEN_TEST = '''
    #[test]
    fn feasibility_folded_bit_lengths_bound_every_code() {
        for value in (0u32..=65536).chain([u32::MAX - 1, u32::MAX]) {
            let lower = u64::from(32 - value.leading_zeros());
            for k in 0u8..=30 {
                assert!(lower <= u64::from(k + 1) + u64::from(value >> k));
            }
            for width in lower..=31 {
                assert!(lower <= width);
            }
        }
    }

    #[test]
    fn feasibility_plan_budget_preserves_every_competitive_result() {
        let mut state = 31u32;
        for count in [1, 2, 9, 17, 64, 257, 4096] {
            for mode in 0..4 {
                let input: Vec<i64> = (0..count).map(|index| {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    match mode { 0 => 0, 1 => index as i64 * 17, 2 => i64::from(state as i32 >> 16), _ => i64::from(state as i32 >> 8) }
                }).collect();
                let full = Subframe::plan(&input, 24, &mut Vec::new());
                let cost = full.cost;
                let mut full_bits = Bits::new();
                full.write(&mut full_bits);
                full_bits.align();
                for limit in [0, cost.saturating_sub(1), cost, cost + 1, u64::MAX] {
                    let limited = Subframe::plan_limited(&input, 24, &mut Vec::new(), limit);
                    if cost <= limit {
                        assert_eq!(limited.cost, cost);
                        let mut bits = Bits::new();
                        limited.write(&mut bits);
                        bits.align();
                        assert_eq!(bits.bytes, full_bits.bytes);
                    } else {
                        assert!(limited.cost > limit);
                    }
                }
            }
        }
    }
'''


def replace(text, old, new, count=1):
    if text.count(old) != count:
        raise ValueError("unexpected frozen source marker")
    return text.replace(old, new)


def transform(text, variant):
    if variant in ("word", "combined"):
        start, end = text.index("struct Bits {"), text.index("enum Predictor {")
        text = text[:start] + WORD_BITS + text[end:]
        text = text[:-2] + WORD_TEST + text[-2:]
    if variant in ("predictor", "stereo", "combined"):
        text = replace(text, "fn fixed_residual<const ORDER: usize>(samples: &[i64], residual: &mut Vec<u32>) -> bool {", "fn fixed_residual<const ORDER: usize>(samples: &[i64], residual: &mut Vec<u32>, limit: u64) -> bool {\n    let mut lower = 0u64;")
        text = replace(text, "        residual.push(value);\n    }\n    true", "        lower += u64::from(32 - value.leading_zeros());\n        if lower >= limit { return false; }\n        residual.push(value);\n    }\n    true")
        old = "    fn residual(&self, samples: &[i64], residual: &mut Vec<u32>) -> Option<usize> {"
        new = "    #[cfg(test)]\n" + old + "\n        self.residual_limited(samples, residual, u64::MAX)\n    }\n\n    fn residual_limited(&self, samples: &[i64], residual: &mut Vec<u32>, limit: u64) -> Option<usize> {"
        text = replace(text, old, new)
        for order in range(5):
            text = replace(text, f"fixed_residual::<{order}>(samples, residual)", f"fixed_residual::<{order}>(samples, residual, limit)")
        text = replace(text, "                for window in samples.windows(order + 1) {", "                let mut lower = 0u64;\n                for window in samples.windows(order + 1) {")
        text = replace(text, "                    residual.push(value);", "                    lower += u64::from(32 - value.leading_zeros());\n                    if lower >= limit { return None; }\n                    residual.push(value);")
        start = text.index("    fn consider(&mut self,")
        end = text.index("    fn write(self,", start)
        text = text[:start] + CONSIDER + text[end:]
        old = "    fn plan(input: &[i64], bits: u8, residual: &mut Vec<u32>) -> Self {"
        new = old + "\n        Self::plan_limited(input, bits, residual, u64::MAX)\n    }\n\n    fn plan_limited(input: &[i64], bits: u8, residual: &mut Vec<u32>, limit: u64) -> Self {"
        text = replace(text, old, new)
        text = replace(text, "best.consider(Predictor::Fixed(order), overhead, residual);", "best.consider_limited(Predictor::Fixed(order), overhead, residual, limit);")
        text = replace(text, "            best.consider(\n", "            best.consider_limited(\n")
        text = replace(text, "                overhead,\n                residual,\n", "                overhead,\n                residual,\n                limit,\n")
        text = text[:-2] + SCREEN_TEST + text[-2:]
    if variant in ("stereo", "combined"):
        text = replace(text, "            let mid = Subframe::plan(&mid, bits, &mut self.residual_scratch);", "            let budget = match subframes.as_slice() {\n                [left, right] => (left.cost + right.cost).min(left.cost + side.cost).min(side.cost + right.cost).saturating_sub(side.cost),\n                _ => u64::MAX,\n            };\n            let mid = Subframe::plan_limited(&mid, bits, &mut self.residual_scratch, budget);")
    return text


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("variant", choices=("word", "predictor", "stereo", "combined"))
    parser.add_argument("encoder", type=Path)
    args = parser.parse_args()
    args.encoder.write_text(transform(args.encoder.read_text(), args.variant))
