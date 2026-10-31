//! The FFT kernels of `coreav-opus`'s inverse MDCT (RFC 6716 4.3.7): the
//! passes of a mixed-radix (2, 3, 4, 5) decimation-in-time FFT and the
//! complex rotations before and after it, on complex values stored as
//! their real and imaginary parts apart.
//!
//! A pass's butterflies run side by side, one per lane: eight in the AVX
//! kernel, four in the portable one (one where a pass has fewer than four
//! side by side). Each value takes the same products and sums in the same
//! order in every lane width, with no fused multiply-add, so every kernel
//! gives the same bits.

/// The values a butterfly computes on: [`LANES`](Lane::WIDTH) complex
/// numbers side by side.
pub(crate) trait Lane: Copy {
    const WIDTH: usize;
    /// `WIDTH` values of 1.
    fn one() -> Self;
    /// The values from the first [`Self::WIDTH`] of `re` and `im`.
    fn load(re: &[f32], im: &[f32]) -> Option<Self>;
    fn store(self, re: &mut [f32], im: &mut [f32]);
    /// `(a.re b.re - a.im b.im, a.re b.im + a.im b.re)`, the products
    /// rounded before the sums.
    fn mul(self, o: Self) -> Self;
    fn add(self, o: Self) -> Self;
    fn sub(self, o: Self) -> Self;
    fn scale(self, s: f32) -> Self;
    /// `-i` times the value.
    fn times_minus_i(self) -> Self;
}

/// One complex number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Complex {
    re: f32,
    im: f32,
}

impl Lane for Complex {
    const WIDTH: usize = 1;

    #[inline(always)]
    fn one() -> Self {
        Self { re: 1.0, im: 0.0 }
    }

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
        Self {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
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

/// Four complex numbers, which the compiler keeps in vector registers.
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

    #[inline(always)]
    fn one() -> Self {
        Self {
            re: [1.0; 4],
            im: [0.0; 4],
        }
    }

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

/// The radix-2 butterfly.
#[inline(always)]
fn radix2<T: Lane>([a, b]: [T; 2], [w1]: [T; 1]) -> [T; 2] {
    let t = b.mul(w1);
    [a.add(t), a.sub(t)]
}

/// The radix-3 butterfly.
#[inline(always)]
fn radix3<T: Lane>([a, b, c]: [T; 3], [w1, w2]: [T; 2]) -> [T; 3] {
    // exp(-i 2 pi / 3) = -1/2 - i sqrt(3)/2.
    const S: f32 = 0.866_025_4;
    let (t1, t2) = (b.mul(w1), c.mul(w2));
    let (s, d) = (t1.add(t2), t1.sub(t2).scale(S).times_minus_i());
    let r = a.sub(s.scale(0.5));
    [a.add(s), r.add(d), r.sub(d)]
}

/// The radix-4 butterfly.
#[inline(always)]
fn radix4<T: Lane>([a, b, c, d]: [T; 4], [w1, w2, w3]: [T; 3]) -> [T; 4] {
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
fn radix5<T: Lane>([t0, b, c, d, e]: [T; 5], [w1, w2, w3, w4]: [T; 4]) -> [T; 5] {
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

/// A pass's twiddle factors for one value index `q` of its butterflies,
/// as their real and imaginary parts: butterfly `k` multiplies its value
/// `q` by `(re[k], im[k])`.
#[derive(Clone, Copy, Debug)]
pub struct Twiddles<'a> {
    /// The real parts.
    pub re: &'a [f32],
    /// The imaginary parts.
    pub im: &'a [f32],
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

/// Runs of [`Lane::WIDTH`] values of `re` and `im`.
#[inline(always)]
fn runs<'a, T: Lane>(
    re: &'a mut [f32],
    im: &'a mut [f32],
) -> core::iter::Zip<core::slice::ChunksExactMut<'a, f32>, core::slice::ChunksExactMut<'a, f32>> {
    re.chunks_exact_mut(T::WIDTH)
        .zip(im.chunks_exact_mut(T::WIDTH))
}

/// The twiddles of `t`, [`Lane::WIDTH`] at a time, for [`twiddle`]. The
/// loads are left to the loop body, inside the vector kernel's
/// `target_feature` function: an iterator adapter that loaded them would
/// be compiled, and called, outside it.
#[inline(always)]
fn twiddle_runs<T: Lane>(
    t: Twiddles<'_>,
) -> core::iter::Zip<core::slice::ChunksExact<'_, f32>, core::slice::ChunksExact<'_, f32>> {
    t.re.chunks_exact(T::WIDTH).zip(t.im.chunks_exact(T::WIDTH))
}

#[inline(always)]
fn twiddle<T: Lane>((r, i): (&[f32], &[f32])) -> T {
    T::load(r, i).unwrap_or(T::one())
}

#[inline(always)]
fn get<T: Lane>((r, i): &(&mut [f32], &mut [f32])) -> T {
    T::load(r, i).unwrap_or(T::one())
}

#[inline(always)]
fn put<T: Lane>((r, i): (&mut [f32], &mut [f32]), v: T) {
    v.store(r, i);
}

/// [`Fft::pass`] on lanes of type `T`, [`Lane::WIDTH`] butterflies at a
/// time; `m` is a multiple of the width.
#[inline(always)]
pub(crate) fn pass_lanes<T: Lane>(m: usize, w: &[Twiddles<'_>], re: &mut [f32], im: &mut [f32]) {
    let size = (w.len() + 1) * m;
    if size == m {
        return;
    }
    for (re, im) in re.chunks_exact_mut(size).zip(im.chunks_exact_mut(size)) {
        match *w {
            [t1] => {
                let [ar, br] = parts(re, m);
                let [ai, bi] = parts(im, m);
                for ((a, b), w1) in runs::<T>(ar, ai)
                    .zip(runs::<T>(br, bi))
                    .zip(twiddle_runs::<T>(t1))
                {
                    let [x, y] = radix2::<T>([get(&a), get(&b)], [twiddle(w1)]);
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
                    let [x, y, z] =
                        radix3::<T>([get(&a), get(&b), get(&c)], [twiddle(w1), twiddle(w2)]);
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
                    let [x, y, z, v] = radix4::<T>(
                        [get(&a), get(&b), get(&c), get(&d)],
                        [twiddle(w1), twiddle(w2), twiddle(w3)],
                    );
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
                    let [x, y, z, v, u] = radix5::<T>(
                        [get(&a), get(&b), get(&c), get(&d), get(&e)],
                        [twiddle(w1), twiddle(w2), twiddle(w3), twiddle(w4)],
                    );
                    put(a, x);
                    put(b, y);
                    put(c, z);
                    put(d, v);
                    put(e, u);
                }
            }
            _ => return,
        }
    }
}

/// [`Fft::rotate`] on lanes of type `T`, the values past the last full
/// lane one at a time.
#[inline(always)]
pub(crate) fn rotate_lanes<T: Lane>(re: &mut [f32], im: &mut [f32], w: Twiddles<'_>) {
    let n = re.len().min(im.len()).min(w.re.len()).min(w.im.len());
    let full = n / T::WIDTH * T::WIDTH;
    let (Some(head_re), Some(head_im)) = (re.get_mut(..full), im.get_mut(..full)) else {
        return;
    };
    let factors = Twiddles {
        re: w.re.get(..full).unwrap_or(&[]),
        im: w.im.get(..full).unwrap_or(&[]),
    };
    for (v, t) in runs::<T>(head_re, head_im).zip(twiddle_runs::<T>(factors)) {
        let x: T = get(&v);
        put(v, x.mul(twiddle(t)));
    }
    rotate_one(re, im, w, full, n);
}

/// [`Fft::rotate`] on the values `from..to`, one at a time.
#[inline(always)]
fn rotate_one(re: &mut [f32], im: &mut [f32], w: Twiddles<'_>, from: usize, to: usize) {
    for k in from..to {
        let (Some(r), Some(i)) = (re.get_mut(k..), im.get_mut(k..)) else {
            return;
        };
        if let (Some(v), Some(t)) = (
            Complex::load(r, i),
            Complex::load(w.re.get(k..).unwrap_or(&[]), w.im.get(k..).unwrap_or(&[])),
        ) {
            v.mul(t).store(r, i);
        }
    }
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

/// The four runs of `quads` values of `v` that start `eighth` apart.
#[inline(always)]
fn quarters(v: &[f32], eighth: usize, quads: usize) -> [&[f32]; 4] {
    core::array::from_fn(|q| v.get(q * eighth..q * eighth + quads).unwrap_or(&[]))
}

/// `v` four values at a time.
#[inline(always)]
fn fours(v: &[f32]) -> impl Iterator<Item = [f32; 4]> + '_ {
    v.chunks_exact(4)
        .map(|c| c.first_chunk::<4>().copied().unwrap_or_default())
}

/// See [`Fft::first_pass`].
fn first_pass(vr: &[f32], vi: &[f32], positions: &[usize], re: &mut [f32], im: &mut [f32]) {
    let eighth = positions.len();
    let quads = eighth / 4 * 4;
    let ([r0, r1, r2, r3], [i0, i1, i2, i3]) =
        (quarters(vr, eighth, quads), quarters(vi, eighth, quads));
    let reals = fours(r0).zip(fours(r1)).zip(fours(r2).zip(fours(r3)));
    let imags = fours(i0).zip(fours(i1)).zip(fours(i2).zip(fours(i3)));
    for ((((a, b), (c, d)), ((e, f), (g, h))), targets) in
        reals.zip(imags).zip(positions.chunks_exact(4))
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
    for (k, &pos) in positions.iter().enumerate().skip(quads) {
        let values = core::array::from_fn(|q| Complex {
            re: vr.get(q * eighth + k).copied().unwrap_or(0.0),
            im: vi.get(q * eighth + k).copied().unwrap_or(0.0),
        });
        for (q, v) in radix4_unit(values).into_iter().enumerate() {
            if let (Some(r), Some(i)) = (re.get_mut(pos + q), im.get_mut(pos + q)) {
                (*r, *i) = (v.re, v.im);
            }
        }
    }
}

/// The FFT kernels the CPU runs, chosen once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fft {
    #[cfg(target_arch = "x86_64")]
    avx: Option<crate::avx::Avx>,
}

impl Fft {
    /// The fastest kernels the CPU has.
    pub fn detect() -> Self {
        Self {
            #[cfg(target_arch = "x86_64")]
            avx: crate::avx::Avx::detect(),
        }
    }

    /// The portable kernels, the reference.
    pub fn portable() -> Self {
        Self {
            #[cfg(target_arch = "x86_64")]
            avx: None,
        }
    }

    /// One pass of radix `w.len() + 1` (2 to 5; any other leaves the
    /// values as they are) over blocks of `(w.len() + 1) m` values of `re`
    /// and `im`: in each block, butterfly `k` (below `m`) takes values
    /// `k`, `k + m`, ..., and multiplies value `q` (from 1) by twiddle `k`
    /// of `w[q - 1]` before its sums. A block that does not fit whole is left
    /// as it is, and so is every value when a twiddle slice holds fewer than
    /// `m` values.
    pub fn pass(&self, m: usize, w: &[Twiddles<'_>], re: &mut [f32], im: &mut [f32]) {
        if w.iter().any(|t| t.re.len() < m || t.im.len() < m) {
            return;
        }
        #[cfg(target_arch = "x86_64")]
        if let Some(avx) = self.avx {
            if m % 8 == 0 {
                avx.pass(m, w, re, im);
                return;
            }
        }
        if m % 4 == 0 {
            pass_lanes::<Quad>(m, w, re, im);
        } else {
            pass_lanes::<Complex>(m, w, re, im);
        }
    }

    /// Each value `k` of `re` and `im` times twiddle `k` of `w`, as far as
    /// all four slices go.
    pub fn rotate(&self, re: &mut [f32], im: &mut [f32], w: Twiddles<'_>) {
        #[cfg(target_arch = "x86_64")]
        if let Some(avx) = self.avx {
            avx.rotate(re, im, w);
            return;
        }
        rotate_lanes::<Quad>(re, im, w);
    }

    /// The first pass of an FFT whose innermost radix is 4, from values in
    /// natural order: with `e` the length of `positions`, butterfly `k`
    /// takes values `k`, `k + e`, `k + 2e` and `k + 3e` of `vr` and `vi`,
    /// with twiddles of 1, and writes its four outputs at `positions[k]`
    /// and the three after it in `re` and `im`. A value past the end of
    /// `vr` or `vi` is 0; an output past the end of `re` or `im` is not
    /// written.
    pub fn first_pass(
        &self,
        vr: &[f32],
        vi: &[f32],
        positions: &[usize],
        re: &mut [f32],
        im: &mut [f32],
    ) {
        first_pass(vr, vi, positions, re, im);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(seed: u32, n: usize) -> Vec<f32> {
        let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
            })
            .collect()
    }

    fn bits(v: &[f32]) -> Vec<u32> {
        v.iter().map(|x| x.to_bits()).collect()
    }

    /// Every radix and lane width gives the same bits as one butterfly at
    /// a time, on the pass shapes of CELT's FFT sizes and others.
    #[test]
    fn every_kernel_gives_the_same_bits() {
        for radix in 2..=5usize {
            for m in [1, 3, 4, 8, 12, 16, 24, 32, 48, 96] {
                let n = radix * m * 3;
                let w: Vec<(Vec<f32>, Vec<f32>)> = (1..radix)
                    .map(|q| (values(10 + q as u32, m), values(20 + q as u32, m)))
                    .collect();
                let tw: Vec<Twiddles<'_>> = w.iter().map(|(re, im)| Twiddles { re, im }).collect();
                let (re0, im0) = (values(1, n), values(2, n));
                let (mut r1, mut i1) = (re0.clone(), im0.clone());
                pass_lanes::<Complex>(m, &tw, &mut r1, &mut i1);
                for fft in [Fft::portable(), Fft::detect()] {
                    let (mut r2, mut i2) = (re0.clone(), im0.clone());
                    fft.pass(m, &tw, &mut r2, &mut i2);
                    assert_eq!(bits(&r1), bits(&r2), "radix {radix} m {m}");
                    assert_eq!(bits(&i1), bits(&i2), "radix {radix} m {m}");
                }
            }
        }
        for n in [1, 7, 8, 60, 61, 240, 480] {
            let (wr, wi) = (values(3, n), values(4, n));
            let w = Twiddles { re: &wr, im: &wi };
            let (re0, im0) = (values(5, n), values(6, n));
            let (mut r1, mut i1) = (re0.clone(), im0.clone());
            rotate_one(&mut r1, &mut i1, w, 0, n);
            for fft in [Fft::portable(), Fft::detect()] {
                let (mut r2, mut i2) = (re0.clone(), im0.clone());
                fft.rotate(&mut r2, &mut i2, w);
                assert_eq!(bits(&r1), bits(&r2), "rotate {n}");
                assert_eq!(bits(&i1), bits(&i2), "rotate {n}");
            }
        }
    }

    #[test]
    fn short_slices_are_left_alone() {
        let w = [Twiddles {
            re: &[1.0; 8],
            im: &[0.0; 8],
        }];
        for fft in [Fft::portable(), Fft::detect()] {
            let (mut re, mut im) = (vec![1.0f32; 15], vec![2.0f32; 15]);
            fft.pass(8, &w, &mut re, &mut im);
            assert_eq!((re, im), (vec![1.0; 15], vec![2.0; 15]));
        }
        for radix in 2..=5usize {
            for m in [4, 8, 16] {
                let (wr, wi) = (vec![0.5f32; m - 1], vec![0.25f32; m - 1]);
                let short: Vec<Twiddles<'_>> =
                    (1..radix).map(|_| Twiddles { re: &wr, im: &wi }).collect();
                let n = radix * m * 2;
                for fft in [Fft::portable(), Fft::detect()] {
                    let (mut re, mut im) = (values(7, n), values(8, n));
                    fft.pass(m, &short, &mut re, &mut im);
                    assert_eq!(bits(&re), bits(&values(7, n)), "radix {radix} m {m}");
                    assert_eq!(bits(&im), bits(&values(8, n)), "radix {radix} m {m}");
                }
            }
        }
    }
}
