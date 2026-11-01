//! The inverse MDCT of CELT's synthesis (RFC 6716 4.3.7), with its
//! low-overlap window, in `f32`.
//!
//! The transform is the textbook one, computed through a DCT-IV of half
//! its size, itself a complex FFT of a quarter of the size between a pre-
//! and a post-rotation; the FFT is a mixed-radix (4, 2, 3, 5) decimation in
//! time over a digit-reversed copy of its input, each radix with its own
//! butterfly and each pass with its twiddles in the order it reads them.
//! Every frame size's rotations, twiddles and permutation, and the window,
//! are computed once per process, without the C math library (`trig`).
//!
//! The output is the reference decoder's: with `y` the inverse MDCT of the
//! `n` coefficients, `y[j] = sum X[k] cos(2 pi / 2n * (j + 1/2 + n/2) *
//! (k + 1/2))` for `j` in `0..2n`, the decoder keeps the `n + 120` samples
//! from `(n - 120) / 2` on, weighted by the window, whose first 120 add to
//! the previous block's last 120.

use std::sync::OnceLock;

/// The window's overlap, in samples (4.3.7).
pub(super) const OVERLAP: usize = 120;

use coreav_kernels_audio::Twiddles;

/// A complex number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Complex {
    re: f32,
    im: f32,
}

impl Complex {
    /// exp(-i 2 pi k / n).
    fn root(k: usize, n: usize) -> Self {
        let (s, c) = trig::sin_cos(-2.0 * core::f64::consts::PI * k as f64 / n as f64);
        Self {
            re: c as f32,
            im: s as f32,
        }
    }
}

/// Complex factors, as their real and imaginary parts.
#[derive(Debug)]
struct Roots {
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Roots {
    fn new(roots: impl Iterator<Item = Complex> + Clone) -> Self {
        Self {
            re: roots.clone().map(|c| c.re).collect(),
            im: roots.map(|c| c.im).collect(),
        }
    }

    fn view(&self) -> Twiddles<'_> {
        Twiddles {
            re: &self.re,
            im: &self.im,
        }
    }
}

/// One pass of the FFT: `radix`-point butterflies over blocks of
/// `radix * m` values, value `q` of butterfly `k` first multiplied by
/// `twiddles[q - 1]` at `k`.
#[derive(Debug)]
struct Pass {
    m: usize,
    twiddles: Vec<Roots>,
}

/// A forward FFT of one size, on values stored as their real and imaginary
/// parts apart; the butterflies are `coreav-kernels-audio`'s.
#[derive(Debug)]
struct Fft {
    /// Innermost first.
    passes: Vec<Pass>,
    /// Where each input lands in the digit-reversed order.
    position: Vec<usize>,
}

impl Fft {
    fn new(n: usize) -> Self {
        // Radix 4 innermost, so that every later pass of CELT's sizes has a
        // multiple of 4 butterflies side by side.
        let mut inner = Vec::new();
        let mut rest = n;
        for p in [4, 2, 3, 5] {
            while rest % p == 0 && rest > 1 {
                inner.push(p);
                rest /= p;
            }
        }
        let radices: Vec<usize> = inner.iter().rev().copied().collect();
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
        let mut passes = Vec::new();
        let mut m = 1;
        for &radix in &inner {
            let step = n / (radix * m);
            let twiddles = (1..radix)
                .map(|q| Roots::new((0..m).map(|k| Complex::root(q * k * step, n))))
                .collect();
            passes.push(Pass { m, twiddles });
            m *= radix;
        }
        Self { passes, position }
    }

    /// Transforms `re` and `im`, already in digit-reversed order, in place.
    #[cfg(test)]
    fn run(&self, re: &mut [f32], im: &mut [f32]) {
        self.passes_from(&coreav_kernels_audio::Fft::detect(), 0, re, im);
    }

    /// The passes from the `first` on.
    fn passes_from(
        &self,
        kernels: &coreav_kernels_audio::Fft,
        first: usize,
        re: &mut [f32],
        im: &mut [f32],
    ) {
        for pass in self.passes.iter().skip(first) {
            let views: [Twiddles<'_>; 4] = core::array::from_fn(|q| {
                pass.twiddles
                    .get(q)
                    .map_or(Twiddles { re: &[], im: &[] }, Roots::view)
            });
            let w = views.get(..pass.twiddles.len()).unwrap_or(&[]);
            kernels.pass(pass.m, w, re, im);
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
    pre: Roots,
    /// Where the FFT's inputs k, k + n/8, k + n/4 and k + 3n/8 go, for k
    /// below n/8: four neighbours from this position, the first pass's
    /// radix-4 butterfly (radix 4 is the innermost of every CELT size).
    first: Vec<usize>,
    /// exp(-i pi k / n), after it.
    post: Roots,
}

impl Imdct {
    fn new(n: usize) -> Self {
        let rotation = |k: f64| {
            let (s, c) = trig::sin_cos(-core::f64::consts::PI * k / n as f64);
            Complex {
                re: c as f32,
                im: s as f32,
            }
        };
        let fft = Fft::new(n / 2);
        let first = fft.position.iter().take(n / 8).copied().collect();
        Self {
            n,
            fft,
            pre: Roots::new((0..n / 2).map(|k| rotation(k as f64 + 0.25))),
            first,
            post: Roots::new((0..n / 2).map(|k| rotation(k as f64))),
        }
    }
}

struct Tables {
    /// 120, 240, 480 and 960 coefficients.
    sizes: [Imdct; 4],
    window: [f32; OVERLAP],
    /// The FFT kernels this CPU runs.
    kernels: coreav_kernels_audio::Fft,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut window = [0f32; OVERLAP];
        for (i, w) in window.iter_mut().enumerate() {
            // sin(pi/2 sin^2(pi (i + 1/2) / 240)) (4.3.7).
            let (s, _) =
                trig::sin_cos(core::f64::consts::PI * (i as f64 + 0.5) / (2 * OVERLAP) as f64);
            *w = trig::sin_cos(core::f64::consts::FRAC_PI_2 * s * s).0 as f32;
        }
        Tables {
            sizes: [120, 240, 480, 960].map(Imdct::new),
            window,
            kernels: coreav_kernels_audio::Fft::detect(),
        }
    })
}

/// The rising half of the window, which the post-filter's cross-fade and
/// the mode transitions use too.
pub(crate) fn window() -> &'static [f32; OVERLAP] {
    &tables().window
}

/// The inverse MDCT's working buffers, kept between calls so that they are
/// not cleared for each: every value is written before it is read.
#[derive(Clone, Debug)]
pub(super) struct Scratch {
    re: [f32; 480],
    im: [f32; 480],
    u: [f32; 960],
}

impl Default for Scratch {
    fn default() -> Self {
        Self {
            re: [0.0; 480],
            im: [0.0; 480],
            u: [0.0; 960],
        }
    }
}

/// The inverse MDCT of the `n` coefficients `coeffs[0]`, `coeffs[stride]`,
/// ... (`n` one of 120, 240, 480, 960), windowed: adds its first
/// [`OVERLAP`] samples to `out` and writes the next `n`.
pub(super) fn inverse(
    coeffs: &[f32],
    stride: usize,
    n: usize,
    out: &mut [f32],
    scratch: &mut Scratch,
) {
    let tables = tables();
    let Some(t) = tables.sizes.iter().find(|t| t.n == n) else {
        return;
    };
    let half = n / 2;
    let Some(coeffs) = coeffs.get(..(n - 1) * stride + 1) else {
        return;
    };
    // The DCT-IV of the coefficients, `u`, through the FFT: X[2k] and
    // X[n - 1 - 2k], rotated, make the FFT's input k. They are made in
    // order in `u`, real parts then imaginary, and the first pass of the
    // FFT reads them there.
    let Scratch { re, im, u } = scratch;
    let (re, im) = (
        re.get_mut(..half).unwrap_or(&mut []),
        im.get_mut(..half).unwrap_or(&mut []),
    );
    let size = u.len();
    let (vr, rest) = u.split_at_mut(half.min(size));
    let vi = rest.get_mut(..half).unwrap_or(&mut []);
    if stride == 1 {
        for ((r, i), (front, back)) in vr
            .iter_mut()
            .zip(vi.iter_mut())
            .zip(coeffs.chunks_exact(2).zip(coeffs.rchunks_exact(2)))
        {
            if let (&[x, _], &[_, y]) = (front, back) {
                (*r, *i) = (x, y);
            }
        }
    } else {
        let evens = coeffs.iter().step_by(2 * stride);
        let odds = coeffs.iter().rev().step_by(2 * stride);
        for ((r, i), (&x, &y)) in vr.iter_mut().zip(vi.iter_mut()).zip(evens.zip(odds)) {
            (*r, *i) = (x, y);
        }
    }
    let kernels = &tables.kernels;
    kernels.rotate(vr, vi, t.pre.view());
    // The first pass, whose radix-4 butterfly k reads input k + q n/8 as
    // its value q.
    kernels.first_pass(vr, vi, &t.first, re, im);
    t.fft.passes_from(kernels, 1, re, im);
    // y[j] = v[j] post[j], the DCT-IV u: u[2j] = re y[j] and u[n - 1 - 2j]
    // = -im y[j].
    kernels.rotate(re, im, t.post.view());

    // The output, from u extended by the DCT-IV's symmetries:
    // `y[j + (n - 120) / 2]` is u at `j + n - 60` for j < 60, -u at
    // `n + 59 - j` up to n + 60, then -u at `j - n - 60`. The window's
    // two ends read u[0..60] and u[n - 60..n], from y's first and last 30
    // values; the middle reads the rest, without the window.
    let window = &tables.window;
    let h = OVERLAP / 2;
    let q = h / 2;
    let Some(out) = out.get_mut(..n + OVERLAP) else {
        return;
    };
    let (head, rest) = out.split_at_mut(OVERLAP);
    let (body, tail) = rest.split_at_mut(n - OVERLAP);
    // j from 120 to n: -u[n + 59 - j], from n - 61 down: im y[30 + t] at
    // even offsets 2t, -re y[half - 31 - t] at odd ones.
    let (middle_re, middle_im) = (
        re.get(q..half - q).unwrap_or(&[]),
        im.get(q..half - q).unwrap_or(&[]),
    );
    for ((pair, &i), &r) in body
        .chunks_exact_mut(2)
        .zip(middle_im)
        .zip(middle_re.iter().rev())
    {
        if let [a, b] = pair {
            (*a, *b) = (i, -r);
        }
    }
    // u[0..60] and u[n - 60..n].
    let (mut low, mut high) = ([0f32; OVERLAP / 2], [0f32; OVERLAP / 2]);
    let (first_re, first_im) = (re.get(..q).unwrap_or(&[]), im.get(..q).unwrap_or(&[]));
    for (j, (&r, &i)) in first_re.iter().zip(first_im).enumerate() {
        set(&mut low, 2 * j, r);
        set(&mut high, h - 1 - 2 * j, -i);
    }
    let (last_re, last_im) = (
        re.get(half - q..).unwrap_or(&[]),
        im.get(half - q..).unwrap_or(&[]),
    );
    for (j, (&r, &i)) in last_re.iter().zip(last_im).enumerate() {
        set(&mut high, 2 * j, r);
        set(&mut low, h - 1 - 2 * j, -i);
    }
    let (rise, rise2) = window.split_at(h);
    let (head1, head2) = head.split_at_mut(h);
    let (tail1, tail2) = tail.split_at_mut(h);
    // j < 60: u[n - 60 + j]; then -u[n + 59 - j], down from n - 1.
    for ((o, &w), &v) in head1.iter_mut().zip(rise).zip(&high) {
        *o += w * v;
    }
    for ((o, &w), &v) in head2.iter_mut().zip(rise2).zip(high.iter().rev()) {
        *o -= w * v;
    }
    // j from n to n + 120, the window falling: -u[n + 59 - j] down to
    // u[0], then -u[j - n - 60] up from it.
    let fall = window.iter().rev();
    for ((o, &w), &v) in tail1.iter_mut().zip(fall.clone()).zip(low.iter().rev()) {
        *o = -w * v;
    }
    for ((o, &w), &v) in tail2.iter_mut().zip(fall.skip(h)).zip(&low) {
        *o = -w * v;
    }
}

/// Sets `v[i]`, if it exists.
fn set(v: &mut [f32], i: usize, value: f32) {
    if let Some(x) = v.get_mut(i) {
        *x = value;
    }
}

mod trig {
    //! Sine and cosine in `f64` for the tables, without the C math library
    //! (`docs/design/audio-decode.md`, MP3 "Tables": linking it costs every
    //! process its loading).

    use core::f64::consts::{FRAC_PI_2, FRAC_PI_4};

    /// `(sin x, cos x)` to within a few units of the last place for
    /// `|x| < 2^20`: reduced by quarter turns to `[-pi/4, pi/4]`, then
    /// Taylor series whose next terms are below 2^-60.
    pub(super) fn sin_cos(x: f64) -> (f64, f64) {
        let turns = x / FRAC_PI_2;
        let mut q = turns as i64;
        // Round to the nearest quarter turn.
        let frac = turns - q as f64;
        if frac > 0.5 {
            q += 1;
        } else if frac < -0.5 {
            q -= 1;
        }
        let r = x - q as f64 * FRAC_PI_2;
        debug_assert!(r.abs() <= FRAC_PI_4 + 1e-12);
        let r2 = r * r;
        let (mut s, mut c) = (0.0f64, 0.0f64);
        let (mut ts, mut tc) = (r, 1.0f64);
        for n in 0..12 {
            s += ts;
            c += tc;
            let k = f64::from(2 * n + 2);
            ts *= -r2 / (k * (k + 1.0));
            tc *= -r2 / ((k - 1.0) * k);
        }
        match q.rem_euclid(4) {
            0 => (s, c),
            1 => (c, -s),
            2 => (-s, -c),
            _ => (-c, s),
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn sin_cos_matches_std() {
            for i in -2000..2000 {
                let x = f64::from(i) * 0.0123;
                let (s, c) = super::sin_cos(x);
                assert!((s - x.sin()).abs() < 1e-14, "{x}");
                assert!((c - x.cos()).abs() < 1e-14, "{x}");
            }
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
            let (mut re, mut im) = (vec![0f32; n], vec![0f32; n]);
            for (i, &v) in input.iter().enumerate() {
                re[fft.position[i]] = v.re;
                im[fft.position[i]] = v.im;
            }
            fft.run(&mut re, &mut im);
            let buf: Vec<Complex> = re
                .iter()
                .zip(&im)
                .map(|(&re, &im)| Complex { re, im })
                .collect();
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
    fn inverse_reads_every_stride() {
        for (n, stride) in [(120, 8), (240, 4), (480, 2)] {
            let coeffs: Vec<f32> = (0..n * stride)
                .map(|k| ((k * 37 % 101) as f32 - 50.0) / 50.0)
                .collect();
            let own: Vec<f32> = coeffs.iter().step_by(stride).copied().collect();
            let mut want = vec![0f32; n + OVERLAP];
            inverse(&own, 1, n, &mut want, &mut Scratch::default());
            let mut got = vec![0f32; n + OVERLAP];
            inverse(&coeffs, stride, n, &mut got, &mut Scratch::default());
            assert_eq!(got, want, "n {n} stride {stride}");
        }
    }

    #[test]
    fn inverse_matches_the_definition() {
        for n in [120, 240, 480, 960] {
            let coeffs: Vec<f32> = (0..n)
                .map(|k| ((k * 37 % 101) as f32 - 50.0) / 50.0)
                .collect();
            let mut out = vec![0f32; n + OVERLAP];
            inverse(&coeffs, 1, n, &mut out, &mut Scratch::default());
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
