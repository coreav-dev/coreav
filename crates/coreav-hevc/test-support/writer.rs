#[derive(Default)]
pub(crate) struct Writer {
    bits: Vec<bool>,
}

impl Writer {
    pub(crate) fn flag(&mut self, value: bool) {
        self.bits.push(value);
    }

    pub(crate) fn u(&mut self, n: u32, value: u32) {
        for shift in (0..n).rev() {
            self.flag(value & (1 << shift) != 0);
        }
    }

    pub(crate) fn ue(&mut self, value: u32) {
        let value = value + 1;
        let n = 32 - value.leading_zeros();
        for _ in 1..n {
            self.flag(false);
        }
        self.u(n, value);
    }

    pub(crate) fn se(&mut self, value: i32) {
        self.ue(if value > 0 {
            2 * value as u32 - 1
        } else {
            2 * (-value) as u32
        });
    }

    pub(crate) fn len(&self) -> usize {
        self.bits.len()
    }

    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.bits
            .chunks(8)
            .map(|bits| {
                bits.iter()
                    .enumerate()
                    .fold(0, |value, (i, bit)| value | (u8::from(*bit) << (7 - i)))
            })
            .collect()
    }

    pub(crate) fn rbsp(mut self) -> Vec<u8> {
        self.flag(true);
        while !self.len().is_multiple_of(8) {
            self.flag(false);
        }
        self.bytes()
    }
}
