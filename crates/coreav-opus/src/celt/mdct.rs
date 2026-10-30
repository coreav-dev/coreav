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

/// A complex number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Complex {
    re: f32,
    im: f32,
}

impl Complex {
    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        Self {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }

    /// exp(-i 2 pi k / n).
    fn root(k: usize, n: usize) -> Self {
        let (s, c) = trig::sin_cos(-2.0 * core::f64::consts::PI * k as f64 / n as f64);
        Self {
            re: c as f32,
            im: s as f32,
        }
    }
}

/// The values one butterfly computes on: one complex number, or [`Quad`]'s
/// four side by side.
trait Lane: Copy {
    /// The complex numbers in one value.
    const WIDTH: usize;
    const ONE: Self;
    /// The values from the first [`Self::WIDTH`] of `re` and `im`.
    fn load(re: &[f32], im: &[f32]) -> Option<Self>;
    fn store(self, re: &mut [f32], im: &mut [f32]);
    fn mul(self, o: Self) -> Self;
    fn add(self, o: Self) -> Self;
    fn sub(self, o: Self) -> Self;
    fn scale(self, s: f32) -> Self;
    /// `-i` times the value.
    fn times_minus_i(self) -> Self;
}

impl Lane for Complex {
    const WIDTH: usize = 1;
    const ONE: Self = Self { re: 1.0, im: 0.0 };

    #[inline(always)]
    fn load(re: &[f32], im: &[f32]) -> Option<Self> {
        Some(Self {
            re: *re.first()?,
            im: *im.first()?,
        })
    }

    #[inline(always)]
    fn store(self, re: &mut [f32], im: &mut [f32]) {
        if let (Some(r), Some(i)) = (re.first_mut(), im.first_mut()) {
            (*r, *i) = (self.re, self.im);
        }
    }

    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        Complex::mul(self, o)
    }

    #[inline(always)]
    fn add(self, o: Self) -> Self {
        Self {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }

    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        Self {
            re: self.re - o.re,
            im: self.im - o.im,
        }
    }

    #[inline(always)]
    fn scale(self, s: f32) -> Self {
        Self {
            re: self.re * s,
            im: self.im * s,
        }
    }

    #[inline(always)]
    fn times_minus_i(self) -> Self {
        Self {
            re: self.im,
            im: -self.re,
        }
    }
}

/// Four complex numbers, as their real and imaginary parts, which the
/// compiler keeps in vector registers.
#[derive(Clone, Copy, Debug)]
struct Quad {
    re: [f32; 4],
    im: [f32; 4],
}

/// `f` on each lane of `a` and `b`.
#[inline(always)]
fn lanes(mut a: [f32; 4], b: [f32; 4], f: impl Fn(f32, f32) -> f32) -> [f32; 4] {
    for (a, b) in a.iter_mut().zip(b) {
        *a = f(*a, b);
    }
    a
}

impl Lane for Quad {
    const WIDTH: usize = 4;
    const ONE: Self = Self {
        re: [1.0; 4],
        im: [0.0; 4],
    };

    #[inline(always)]
    fn load(re: &[f32], im: &[f32]) -> Option<Self> {
        Some(Self {
            re: *re.first_chunk()?,
            im: *im.first_chunk()?,
        })
    }

    #[inline(always)]
    fn store(self, re: &mut [f32], im: &mut [f32]) {
        if let (Some(r), Some(i)) = (re.first_chunk_mut(), im.first_chunk_mut()) {
            (*r, *i) = (self.re, self.im);
        }
    }

    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        let rr = lanes(self.re, o.re, |a, b| a * b);
        let ii = lanes(self.im, o.im, |a, b| a * b);
        let ri = lanes(self.re, o.im, |a, b| a * b);
        let ir = lanes(self.im, o.re, |a, b| a * b);
        Self {
            re: lanes(rr, ii, |a, b| a - b),
            im: lanes(ri, ir, |a, b| a + b),
        }
    }

    #[inline(always)]
    fn add(self, o: Self) -> Self {
        Self {
            re: lanes(self.re, o.re, |a, b| a + b),
            im: lanes(self.im, o.im, |a, b| a + b),
        }
    }

    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        Self {
            re: lanes(self.re, o.re, |a, b| a - b),
            im: lanes(self.im, o.im, |a, b| a - b),
        }
    }

    #[inline(always)]
    fn scale(self, s: f32) -> Self {
        Self {
            re: self.re.map(|v| v * s),
            im: self.im.map(|v| v * s),
        }
    }

    #[inline(always)]
    fn times_minus_i(self) -> Self {
        Self {
            re: self.im,
            im: self.re.map(|v| -v),
        }
    }
}

/// The radix-2 butterfly; `w[0]` is not used.
#[inline(always)]
fn radix2<T: Lane>([a, b]: [T; 2], [_, w1]: [T; 2]) -> [T; 2] {
    let t = b.mul(w1);
    [a.add(t), a.sub(t)]
}

/// The radix-3 butterfly.
#[inline(always)]
fn radix3<T: Lane>([a, b, c]: [T; 3], [_, w1, w2]: [T; 3]) -> [T; 3] {
    // exp(-i 2 pi / 3) = -1/2 - i sqrt(3)/2.
    const S: f32 = 0.866_025_4;
    let (t1, t2) = (b.mul(w1), c.mul(w2));
    let (s, d) = (t1.add(t2), t1.sub(t2).scale(S).times_minus_i());
    let r = a.sub(s.scale(0.5));
    [a.add(s), r.add(d), r.sub(d)]
}

/// The radix-4 butterfly.
#[inline(always)]
fn radix4<T: Lane>([a, b, c, d]: [T; 4], [_, w1, w2, w3]: [T; 4]) -> [T; 4] {
    let (t1, t2, t3) = (b.mul(w1), c.mul(w2), d.mul(w3));
    let (s0, s1) = (a.add(t2), a.sub(t2));
    let (s2, s3) = (t1.add(t3), t1.sub(t3).times_minus_i());
    [s0.add(s2), s1.add(s3), s0.sub(s2), s1.sub(s3)]
}

/// The radix-4 butterfly with twiddles of 1.
#[inline(always)]
fn radix4_unit<T: Lane>([a, b, c, d]: [T; 4]) -> [T; 4] {
    let (s0, s1) = (a.add(c), a.sub(c));
    let (s2, s3) = (b.add(d), b.sub(d).times_minus_i());
    [s0.add(s2), s1.add(s3), s0.sub(s2), s1.sub(s3)]
}

/// The radix-5 butterfly.
#[inline(always)]
fn radix5<T: Lane>([t0, b, c, d, e]: [T; 5], [_, w1, w2, w3, w4]: [T; 5]) -> [T; 5] {
    // cos and sin of 2 pi / 5 and 4 pi / 5.
    const C1: f32 = 0.309_017;
    const C2: f32 = -0.809_017;
    const S1: f32 = 0.951_056_5;
    const S2: f32 = 0.587_785_24;
    let (t1, t2, t3, t4) = (b.mul(w1), c.mul(w2), d.mul(w3), e.mul(w4));
    let (a1, b1, a2, b2) = (t1.add(t4), t1.sub(t4), t2.add(t3), t2.sub(t3));
    let r1 = t0.add(a1.scale(C1)).add(a2.scale(C2));
    let r2 = t0.add(a1.scale(C2)).add(a2.scale(C1));
    let i1 = b1.scale(S1).add(b2.scale(S2)).times_minus_i();
    let i2 = b1.scale(S2).sub(b2.scale(S1)).times_minus_i();
    [
        t0.add(a1).add(a2),
        r1.add(i1),
        r2.add(i2),
        r2.sub(i2),
        r1.sub(i1),
    ]
}

/// Twiddle factors, as their real and imaginary parts.
#[derive(Debug)]
struct Twiddles {
    re: Vec<f32>,
    im: Vec<f32>,
}

/// One pass of the FFT: `radix`-point butterflies over blocks of
/// `radix * m` values, value `q` of butterfly `k` first multiplied by
/// `twiddles[q - 1]` at `k`.
#[derive(Debug)]
struct Pass {
    radix: usize,
    m: usize,
    twiddles: Vec<Twiddles>,
}

/// Runs of [`Lane::WIDTH`] values of `re` and `im`.
#[inline(always)]
fn runs<'a, T: Lane>(
    re: &'a mut [f32],
    im: &'a mut [f32],
) -> core::iter::Zip<core::slice::ChunksExactMut<'a, f32>, core::slice::ChunksExactMut<'a, f32>> {
    re.chunks_exact_mut(T::WIDTH)
        .zip(im.chunks_exact_mut(T::WIDTH))
}

/// The twiddles of `t`, [`Lane::WIDTH`] at a time.
#[inline(always)]
fn twiddle_runs<T: Lane>(t: &Twiddles) -> impl Iterator<Item = T> + '_ {
    t.re.chunks_exact(T::WIDTH)
        .zip(t.im.chunks_exact(T::WIDTH))
        .map(|(r, i)| T::load(r, i).unwrap_or(T::ONE))
}

#[inline(always)]
fn get<T: Lane>((r, i): &(&mut [f32], &mut [f32])) -> T {
    T::load(r, i).unwrap_or(T::ONE)
}

#[inline(always)]
fn put<T: Lane>((r, i): (&mut [f32], &mut [f32]), v: T) {
    v.store(r, i);
}

impl Pass {
    /// The pass on `re` and `im`, [`Lane::WIDTH`] butterflies at a time.
    #[inline(always)]
    fn run<T: Lane>(&self, re: &mut [f32], im: &mut [f32]) {
        let m = self.m;
        let size = self.radix * m;
        for (re, im) in re.chunks_exact_mut(size).zip(im.chunks_exact_mut(size)) {
            match self.twiddles.as_slice() {
                [t1] => {
                    let [ar, br] = parts(re, m);
                    let [ai, bi] = parts(im, m);
                    for ((a, b), w1) in runs::<T>(ar, ai)
                        .zip(runs::<T>(br, bi))
                        .zip(twiddle_runs::<T>(t1))
                    {
                        let [x, y] = radix2([get(&a), get(&b)], [T::ONE, w1]);
                        put(a, x);
                        put(b, y);
                    }
                }
                [t1, t2] => {
                    let [ar, br, cr] = parts(re, m);
                    let [ai, bi, ci] = parts(im, m);
                    for (((a, b), c), (w1, w2)) in runs::<T>(ar, ai)
                        .zip(runs::<T>(br, bi))
                        .zip(runs::<T>(cr, ci))
                        .zip(twiddle_runs::<T>(t1).zip(twiddle_runs::<T>(t2)))
                    {
                        let [x, y, z] = radix3([get(&a), get(&b), get(&c)], [T::ONE, w1, w2]);
                        put(a, x);
                        put(b, y);
                        put(c, z);
                    }
                }
                [t1, t2, t3] => {
                    let [ar, br, cr, dr] = parts(re, m);
                    let [ai, bi, ci, di] = parts(im, m);
                    let w = twiddle_runs::<T>(t1)
                        .zip(twiddle_runs::<T>(t2))
                        .zip(twiddle_runs::<T>(t3));
                    for ((((a, b), c), d), ((w1, w2), w3)) in runs::<T>(ar, ai)
                        .zip(runs::<T>(br, bi))
                        .zip(runs::<T>(cr, ci))
                        .zip(runs::<T>(dr, di))
                        .zip(w)
                    {
                        let [x, y, z, v] =
                            radix4([get(&a), get(&b), get(&c), get(&d)], [T::ONE, w1, w2, w3]);
                        put(a, x);
                        put(b, y);
                        put(c, z);
                        put(d, v);
                    }
                }
                [t1, t2, t3, t4] => {
                    let [ar, br, cr, dr, er] = parts(re, m);
                    let [ai, bi, ci, di, ei] = parts(im, m);
                    let w = twiddle_runs::<T>(t1)
                        .zip(twiddle_runs::<T>(t2))
                        .zip(twiddle_runs::<T>(t3))
                        .zip(twiddle_runs::<T>(t4));
                    for (((((a, b), c), d), e), (((w1, w2), w3), w4)) in runs::<T>(ar, ai)
                        .zip(runs::<T>(br, bi))
                        .zip(runs::<T>(cr, ci))
                        .zip(runs::<T>(dr, di))
                        .zip(runs::<T>(er, ei))
                        .zip(w)
                    {
                        let [x, y, z, v, u] = radix5(
                            [get(&a), get(&b), get(&c), get(&d), get(&e)],
                            [T::ONE, w1, w2, w3, w4],
                        );
                        put(a, x);
                        put(b, y);
                        put(c, z);
                        put(d, v);
                        put(e, u);
                    }
                }
                _ => {}
            }
        }
    }
}

/// `x` cut into `R` runs of `m`.
#[inline(always)]
fn parts<const R: usize>(x: &mut [f32], m: usize) -> [&mut [f32]; R] {
    let mut rest = x;
    core::array::from_fn(|_| {
        let all = core::mem::take(&mut rest);
        let (head, tail) = all.split_at_mut(m.min(all.len()));
        rest = tail;
        head
    })
}

/// A forward FFT of one size, on values stored as their real and imaginary
/// parts apart, so that a pass's butterflies run four at a time.
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
                .map(|q| {
                    let roots: Vec<Complex> =
                        (0..m).map(|k| Complex::root(q * k * step, n)).collect();
                    Twiddles {
                        re: roots.iter().map(|c| c.re).collect(),
                        im: roots.iter().map(|c| c.im).collect(),
                    }
                })
                .collect();
            passes.push(Pass { radix, m, twiddles });
            m *= radix;
        }
        Self { passes, position }
    }

    /// Transforms `re` and `im`, already in digit-reversed order, in place.
    #[cfg(test)]
    fn run(&self, re: &mut [f32], im: &mut [f32]) {
        self.passes_from(0, re, im);
    }

    /// The passes from the `first` on.
    fn passes_from(&self, first: usize, re: &mut [f32], im: &mut [f32]) {
        for pass in self.passes.iter().skip(first) {
            if pass.m == 1 && pass.radix == 4 {
                // Twiddles of 1, on four neighbours.
                for (r, i) in re.chunks_exact_mut(4).zip(im.chunks_exact_mut(4)) {
                    if let (Some(r), Some(i)) = (r.first_chunk_mut::<4>(), i.first_chunk_mut::<4>())
                    {
                        let out = radix4_unit(core::array::from_fn(|q| Complex {
                            re: r.get(q).copied().unwrap_or_default(),
                            im: i.get(q).copied().unwrap_or_default(),
                        }));
                        for ((r, i), v) in r.iter_mut().zip(i.iter_mut()).zip(out) {
                            (*r, *i) = (v.re, v.im);
                        }
                    }
                }
            } else if pass.m % 4 == 0 {
                pass.run::<Quad>(re, im);
            } else {
                pass.run::<Complex>(re, im);
            }
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
    pre: Twiddles,
    /// Where the FFT's inputs k, k + n/8, k + n/4 and k + 3n/8 go, for k
    /// below n/8: four neighbours from this position, the first pass's
    /// radix-4 butterfly (radix 4 is the innermost of every CELT size).
    first: Vec<usize>,
    /// exp(-i pi k / n), after it.
    post: Twiddles,
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
        let pre: Vec<Complex> = (0..n / 2).map(|k| rotation(k as f64 + 0.25)).collect();
        Self {
            n,
            fft,
            pre: Twiddles {
                re: pre.iter().map(|c| c.re).collect(),
                im: pre.iter().map(|c| c.im).collect(),
            },
            first,
            post: Twiddles {
                re: (0..n / 2).map(|k| rotation(k as f64).re).collect(),
                im: (0..n / 2).map(|k| rotation(k as f64).im).collect(),
            },
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
            let (s, _) =
                trig::sin_cos(core::f64::consts::PI * (i as f64 + 0.5) / (2 * OVERLAP) as f64);
            *w = trig::sin_cos(core::f64::consts::FRAC_PI_2 * s * s).0 as f32;
        }
        Tables {
            sizes: [120, 240, 480, 960].map(Imdct::new),
            window,
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

/// The four runs of `quads` values of `v` that start `eighth` apart.
fn quarters(v: &[f32], eighth: usize, quads: usize) -> [&[f32]; 4] {
    core::array::from_fn(|q| v.get(q * eighth..q * eighth + quads).unwrap_or(&[]))
}

/// `v` four values at a time.
fn fours(v: &[f32]) -> impl Iterator<Item = [f32; 4]> + '_ {
    v.chunks_exact(4)
        .map(|c| c.first_chunk::<4>().copied().unwrap_or_default())
}

/// The columns of four rows of four.
#[inline(always)]
fn transpose(
    [
        [x0, x1, x2, x3],
        [y0, y1, y2, y3],
        [z0, z1, z2, z3],
        [w0, w1, w2, w3],
    ]: [[f32; 4]; 4],
) -> [[f32; 4]; 4] {
    [
        [x0, y0, z0, w0],
        [x1, y1, z1, w1],
        [x2, y2, z2, w2],
        [x3, y3, z3, w3],
    ]
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
    for ((r, i), (&pr, &pi)) in vr
        .iter_mut()
        .zip(vi.iter_mut())
        .zip(t.pre.re.iter().zip(&t.pre.im))
    {
        (*r, *i) = (*r * pr - *i * pi, *r * pi + *i * pr);
    }
    // The first pass: input k + q n/8 is value q of the butterfly at
    // first[k], four butterflies at a time, each one's outputs turned
    // into neighbours.
    let eighth = half / 4;
    let quads = eighth / 4 * 4;
    let ([r0, r1, r2, r3], [i0, i1, i2, i3]) =
        (quarters(vr, eighth, quads), quarters(vi, eighth, quads));
    let reals = fours(r0).zip(fours(r1)).zip(fours(r2).zip(fours(r3)));
    let imags = fours(i0).zip(fours(i1)).zip(fours(i2).zip(fours(i3)));
    for ((((a, b), (c, d)), ((e, f), (g, h))), targets) in
        reals.zip(imags).zip(t.first.chunks_exact(4))
    {
        let quad = |re, im| Quad { re, im };
        let [a, b, c, d] = radix4_unit([quad(a, e), quad(b, f), quad(c, g), quad(d, h)]);
        let real = transpose([a.re, b.re, c.re, d.re]);
        let imag = transpose([a.im, b.im, c.im, d.im]);
        for ((&pos, r4), i4) in targets.iter().zip(real).zip(imag) {
            if let (Some(r), Some(i)) = (
                re.get_mut(pos..).and_then(<[f32]>::first_chunk_mut::<4>),
                im.get_mut(pos..).and_then(<[f32]>::first_chunk_mut::<4>),
            ) {
                (*r, *i) = (r4, i4);
            }
        }
    }
    // The butterflies left over, one at a time (n/8 is 15 or 30 for the
    // two shortest sizes).
    for (k, &pos) in t.first.iter().enumerate().skip(quads) {
        let input = core::array::from_fn(|q| Complex {
            re: vr.get(k + q * eighth).copied().unwrap_or(0.0),
            im: vi.get(k + q * eighth).copied().unwrap_or(0.0),
        });
        for (q, v) in radix4_unit(input).into_iter().enumerate() {
            if let (Some(r), Some(i)) = (re.get_mut(pos + q), im.get_mut(pos + q)) {
                (*r, *i) = (v.re, v.im);
            }
        }
    }
    t.fft.passes_from(1, re, im);
    // y[j] = v[j] post[j]; u[2j] = re y[j] and u[n - 1 - 2j] = -im y[j]: the
    // pair u[2j], u[2j + 1] takes y[j] and y[half - 1 - j].
    for ((r, i), (&pr, &pi)) in re
        .iter_mut()
        .zip(im.iter_mut())
        .zip(t.post.re.iter().zip(&t.post.im))
    {
        (*r, *i) = (*r * pr - *i * pi, *r * pi + *i * pr);
    }
    let u = u.get_mut(..n).unwrap_or(&mut []);
    for ((eight, y), z) in u
        .chunks_exact_mut(8)
        .zip(re.chunks_exact(4))
        .zip(im.rchunks_exact(4))
    {
        if let ([a0, b0, a1, b1, a2, b2, a3, b3], &[y0, y1, y2, y3], &[z0, z1, z2, z3]) =
            (eight, y, z)
        {
            (*a0, *b0, *a1, *b1) = (y0, -z3, y1, -z2);
            (*a2, *b2, *a3, *b3) = (y2, -z1, y3, -z0);
        }
    }

    // The window's support: `y[j + (n - 120) / 2]` is `u` at `j + n - 60`,
    // extended by the DCT-IV's symmetries: u[n - 60 + j] for j < 60,
    // -u[n + 59 - j] up to n + 60, then -u[j - n - 60].
    let window = &tables.window;
    let h = OVERLAP / 2;
    let Some(out) = out.get_mut(..n + OVERLAP) else {
        return;
    };
    let (head, rest) = out.split_at_mut(OVERLAP);
    let (body, tail) = rest.split_at_mut(n - OVERLAP);
    let (rise, rise2) = window.split_at(h);
    let (head1, head2) = head.split_at_mut(h);
    let (tail1, tail2) = tail.split_at_mut(h);
    let part = |r: core::ops::Range<usize>| u.get(r).unwrap_or(&[]);
    // j < 60: u[n - 60 + j]; then -u[n + 59 - j], down from n - 1.
    for ((o, &w), &v) in head1.iter_mut().zip(rise).zip(part(n - h..n)) {
        *o += w * v;
    }
    for ((o, &w), &v) in head2.iter_mut().zip(rise2).zip(part(n - h..n).iter().rev()) {
        *o -= w * v;
    }
    // j from 120 to n: -u[n + 59 - j], down from n - 61 to 60.
    for (o, &v) in body.iter_mut().zip(part(h..n - h).iter().rev()) {
        *o = -v;
    }
    // j from n to n + 120, the window falling: -u[n + 59 - j] down to
    // u[0], then -u[j - n - 60] up from it.
    let fall = window.iter().rev();
    for ((o, &w), &v) in tail1
        .iter_mut()
        .zip(fall.clone())
        .zip(part(0..h).iter().rev())
    {
        *o = -w * v;
    }
    for ((o, &w), &v) in tail2.iter_mut().zip(fall.skip(h)).zip(part(0..h)) {
        *o = -w * v;
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
