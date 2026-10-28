//! The inverse MDCT of CELT's synthesis (RFC 6716 4.3.7), with its
//! low-overlap window, in `f32`.
//!
//! The transform is the textbook one, computed through a DCT-IV of half
//! its size, itself a complex FFT of a quarter of the size between a pre-
//! and a post-rotation; the FFT is a mixed-radix (2, 3, 4, 5) decimation in
//! time over a digit-reversed copy of its input. Every frame size's
//! rotations, FFT twiddles and permutation, and the window, are computed
//! once per process.
//!
//! The output is the reference decoder's: with `y` the inverse MDCT of the
//! `n` coefficients, `y[j] = sum X[k] cos(2 pi / 2n * (j + 1/2 + n/2) *
//! (k + 1/2))` for `j` in `0..2n`, the decoder keeps the `n + 120` samples
//! from `(n - 120) / 2` on, weighted by the window, whose first 120 add to
//! the previous block's last 120.

use std::sync::OnceLock;

/// The window's overlap, in samples (4.3.7).
pub(super) const OVERLAP: usize = 120;

/// A complex number.
#[derive(Clone, Copy, Debug, Default)]
struct Complex {
    re: f32,
    im: f32,
}

impl Complex {
    fn mul(self, o: Self) -> Self {
        Self {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }

    fn add(self, o: Self) -> Self {
        Self {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }

    /// exp(-i 2 pi k / n).
    fn root(k: usize, n: usize) -> Self {
        let a = -2.0 * core::f64::consts::PI * k as f64 / n as f64;
        Self {
            re: a.cos() as f32,
            im: a.sin() as f32,
        }
    }
}

/// A forward FFT of one size.
#[derive(Debug)]
struct Fft {
    /// The radices, outermost first.
    radices: Vec<usize>,
    /// Where each input lands in the digit-reversed order.
    position: Vec<usize>,
    /// exp(-i 2 pi k / n) for k in 0..n.
    roots: Vec<Complex>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let mut radices = Vec::new();
        let mut rest = n;
        for p in [4, 2, 3, 5] {
            while rest % p == 0 && rest > 1 {
                radices.push(p);
                rest /= p;
            }
        }
        // Input i = q1 + p1 (q2 + p2 (...)) goes to sum q_l * n / (p1 ... p_l).
        let position = (0..n)
            .map(|i| {
                let (mut i, mut span, mut at) = (i, n, 0);
                for &p in &radices {
                    span /= p;
                    at += (i % p) * span;
                    i /= p;
                }
                at
            })
            .collect();
        Self {
            radices,
            position,
            roots: (0..n).map(|k| Complex::root(k, n)).collect(),
        }
    }

    fn len(&self) -> usize {
        self.roots.len()
    }

    /// Transforms `buf`, already in digit-reversed order, in place.
    fn run(&self, buf: &mut [Complex]) {
        let n = self.len();
        let root = |k: usize| self.roots.get(k % n.max(1)).copied().unwrap_or_default();
        let mut m = 1;
        let mut t = [Complex::default(); 5];
        for &p in self.radices.iter().rev() {
            let size = p * m;
            let step = n / size;
            for block in buf.chunks_exact_mut(size) {
                for k in 0..m {
                    for (q, tq) in t.iter_mut().enumerate().take(p) {
                        let v = block.get(q * m + k).copied().unwrap_or_default();
                        *tq = v.mul(root(q * k * step));
                    }
                    for r in 0..p {
                        let mut sum = Complex::default();
                        for (q, &tq) in t.iter().enumerate().take(p) {
                            sum = sum.add(tq.mul(root((q * r % p) * (n / p))));
                        }
                        if let Some(o) = block.get_mut(k + r * m) {
                            *o = sum;
                        }
                    }
                }
            }
            m = size;
        }
    }
}

/// The inverse MDCT of one block size.
#[derive(Debug)]
struct Imdct {
    /// Coefficients per block.
    n: usize,
    fft: Fft,
    /// exp(-i pi (k + 1/4) / n), before the FFT.
    pre: Vec<Complex>,
    /// exp(-i pi k / n), after it.
    post: Vec<Complex>,
}

impl Imdct {
    fn new(n: usize) -> Self {
        let rotation = |k: f64| {
            let a = -core::f64::consts::PI * k / n as f64;
            Complex {
                re: a.cos() as f32,
                im: a.sin() as f32,
            }
        };
        Self {
            n,
            fft: Fft::new(n / 2),
            pre: (0..n / 2).map(|k| rotation(k as f64 + 0.25)).collect(),
            post: (0..n / 2).map(|k| rotation(k as f64)).collect(),
        }
    }
}

struct Tables {
    /// 120, 240, 480 and 960 coefficients.
    sizes: [Imdct; 4],
    window: [f32; OVERLAP],
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut window = [0f32; OVERLAP];
        for (i, w) in window.iter_mut().enumerate() {
            // sin(pi/2 sin^2(pi (i + 1/2) / 240)) (4.3.7).
            let s = (core::f64::consts::PI * (i as f64 + 0.5) / (2 * OVERLAP) as f64).sin();
            *w = (core::f64::consts::FRAC_PI_2 * s * s).sin() as f32;
        }
        Tables {
            sizes: [120, 240, 480, 960].map(Imdct::new),
            window,
        }
    })
}

/// The rising half of the window, which the post-filter's cross-fade and
/// the mode transitions use too.
pub(super) fn window() -> &'static [f32; OVERLAP] {
    &tables().window
}

/// The inverse MDCT of the `n` coefficients `coeffs[0]`, `coeffs[stride]`,
/// ... (`n` one of 120, 240, 480, 960), windowed: adds its first
/// [`OVERLAP`] samples to `out` and writes the next `n`.
pub(super) fn inverse(coeffs: &[f32], stride: usize, n: usize, out: &mut [f32]) {
    let tables = tables();
    let Some(t) = tables.sizes.iter().find(|t| t.n == n) else {
        return;
    };
    let half = n / 2;
    let coeff = |k: usize| coeffs.get(k * stride).copied().unwrap_or(0.0);

    // The DCT-IV of the coefficients, `u`, through the FFT.
    let mut buf = [Complex::default(); 480];
    let buf = buf.get_mut(..half).unwrap_or(&mut []);
    for (k, (&pos, &pre)) in t.fft.position.iter().zip(&t.pre).enumerate() {
        let v = Complex {
            re: coeff(2 * k),
            im: coeff(n - 1 - 2 * k),
        };
        if let Some(b) = buf.get_mut(pos) {
            *b = v.mul(pre);
        }
    }
    t.fft.run(buf);
    let mut u = [0f32; 960];
    for (j, (&v, &post)) in buf.iter().zip(&t.post).enumerate() {
        let y = v.mul(post);
        if let Some(e) = u.get_mut(2 * j) {
            *e = y.re;
        }
        if let Some(o) = u.get_mut(n - 1 - 2 * j) {
            *o = -y.im;
        }
    }

    // The window's support: `y[j + (n - 120) / 2]` is `u` at `j + n -
    // 60`, extended by the DCT-IV's symmetries.
    let at = |m: usize| -> f32 {
        if m < n {
            u.get(m).copied().unwrap_or(0.0)
        } else if m < 2 * n {
            -u.get(2 * n - 1 - m).copied().unwrap_or(0.0)
        } else {
            -u.get(m - 2 * n).copied().unwrap_or(0.0)
        }
    };
    let window = &tables.window;
    let offset = n - OVERLAP / 2;
    for (j, o) in out.iter_mut().enumerate().take(n + OVERLAP) {
        let y = at(j + offset);
        if j < OVERLAP {
            *o += window.get(j).copied().unwrap_or(0.0) * y;
        } else if j < n {
            *o = y;
        } else {
            *o = window.get(n + OVERLAP - 1 - j).copied().unwrap_or(0.0) * y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_matches_the_dft() {
        for n in [60, 120, 240, 480] {
            let fft = Fft::new(n);
            let input: Vec<Complex> = (0..n)
                .map(|i| Complex {
                    re: ((i * 7 % 13) as f32 - 6.0) / 7.0,
                    im: ((i * 5 % 11) as f32 - 5.0) / 9.0,
                })
                .collect();
            let mut buf = vec![Complex::default(); n];
            for (i, &v) in input.iter().enumerate() {
                buf[fft.position[i]] = v;
            }
            fft.run(&mut buf);
            for (k, out) in buf.iter().enumerate() {
                let (mut re, mut im) = (0f64, 0f64);
                for (i, v) in input.iter().enumerate() {
                    let a = -2.0 * core::f64::consts::PI * (i * k % n) as f64 / n as f64;
                    re += f64::from(v.re) * a.cos() - f64::from(v.im) * a.sin();
                    im += f64::from(v.re) * a.sin() + f64::from(v.im) * a.cos();
                }
                assert!((re - f64::from(out.re)).abs() < 1e-3, "n {n} k {k}");
                assert!((im - f64::from(out.im)).abs() < 1e-3, "n {n} k {k}");
            }
        }
    }

    #[test]
    fn inverse_matches_the_definition() {
        for n in [120, 240, 480, 960] {
            let coeffs: Vec<f32> = (0..n)
                .map(|k| ((k * 37 % 101) as f32 - 50.0) / 50.0)
                .collect();
            let mut out = vec![0f32; n + OVERLAP];
            inverse(&coeffs, 1, n, &mut out);
            let w = window();
            for (j, &o) in out.iter().enumerate() {
                let t = (j + (n - OVERLAP) / 2) as f64;
                let y: f64 = coeffs
                    .iter()
                    .enumerate()
                    .map(|(k, &x)| {
                        let a = core::f64::consts::PI / n as f64
                            * (t + 0.5 + n as f64 / 2.0)
                            * (k as f64 + 0.5);
                        f64::from(x) * a.cos()
                    })
                    .sum();
                let g = if j < OVERLAP {
                    f64::from(w[j])
                } else if j < n {
                    1.0
                } else {
                    f64::from(w[n + OVERLAP - 1 - j])
                };
                assert!(
                    (g * y - f64::from(o)).abs() < 2e-3,
                    "n {n} j {j}: {} {o}",
                    g * y
                );
            }
        }
    }
}
