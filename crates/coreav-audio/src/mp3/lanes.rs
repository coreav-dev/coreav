//! Four `f32` lanes with element-wise arithmetic, written so that the
//! compiler maps them onto vector registers (SSE2 on x86-64, NEON on
//! AArch64). The IMDCT uses one lane per subband (`layer3.rs`).

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Default)]
pub(crate) struct F4(pub(crate) [f32; 4]);

impl F4 {
    pub(crate) const ZERO: F4 = F4([0.0; 4]);
}

impl Add for F4 {
    type Output = F4;
    #[inline(always)]
    fn add(self, rhs: F4) -> F4 {
        let ([a, b, c, d], [e, f, g, h]) = (self.0, rhs.0);
        F4([a + e, b + f, c + g, d + h])
    }
}

impl Sub for F4 {
    type Output = F4;
    #[inline(always)]
    fn sub(self, rhs: F4) -> F4 {
        let ([a, b, c, d], [e, f, g, h]) = (self.0, rhs.0);
        F4([a - e, b - f, c - g, d - h])
    }
}

impl Mul for F4 {
    type Output = F4;
    #[inline(always)]
    fn mul(self, rhs: F4) -> F4 {
        let ([a, b, c, d], [e, f, g, h]) = (self.0, rhs.0);
        F4([a * e, b * f, c * g, d * h])
    }
}

impl Mul<f32> for F4 {
    type Output = F4;
    #[inline(always)]
    fn mul(self, rhs: f32) -> F4 {
        let [a, b, c, d] = self.0;
        F4([a * rhs, b * rhs, c * rhs, d * rhs])
    }
}

impl Neg for F4 {
    type Output = F4;
    #[inline(always)]
    fn neg(self) -> F4 {
        let [a, b, c, d] = self.0;
        F4([-a, -b, -c, -d])
    }
}
