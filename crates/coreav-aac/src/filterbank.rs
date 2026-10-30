//! The filterbank (ISO/IEC 14496-3 4.6.11): the IMDCT of every window,
//! windowing by window sequence and shape, and overlap-add with the frame
//! before.
//!
//! The IMDCT of M coefficients, x\[n\] = (2/N) sum X\[k\] cos(2 pi / N (n +
//! n0)(k + 1/2)) with N = 2M and n0 = (M + 1) / 2, is the DCT-IV C of the
//! coefficients unfolded: over n from 0 to 2M, x\[n\] is C\[n + M/2\]
//! below M/2, then minus C\[3M/2 - 1 - n\] below 3M/2, then minus C\[n -
//! 3M/2\], all over M. The DCT-IV is a complex FFT of M/2 points between
//! two twiddles: z\[j\] = (X\[2j\] + i X\[M - 1 - 2j\]) e^(-i pi (j + 1/8)
//! / M), Z its FFT, and w\[p\] = Z\[p\] e^(-i pi (p + 1/8) / M) gives
//! C\[2p\] = Re w\[p\] and C\[M - 1 - 2p\] = minus Im w\[p\]. The factor 1/M
//! is folded into the spectrum's scale.

// `as_chunks` (Rust 1.88) is within the workspace's rust-version, 1.89;
// clippy's version-aware lints keep to 1.85 (clippy.toml).
#![allow(clippy::incompatible_msrv)]

use crate::ics::{EIGHT_SHORT, Info, LONG_START, LONG_STOP};
use crate::tables::{
    FFT_ROOTS_480, FFT_ROOTS_512, IMDCT_TWIDDLE_120, IMDCT_TWIDDLE_128, IMDCT_TWIDDLE_960,
    IMDCT_TWIDDLE_1024, KBD_LONG_960, KBD_LONG_1024, KBD_SHORT_120, KBD_SHORT_128, SINE_LONG_960,
    SINE_LONG_1024, SINE_SHORT_120, SINE_SHORT_128,
};

/// Lanes of the FFT's butterflies: one value, or four that the compiler
/// keeps in one vector register.
trait Lane: Copy {
    fn store(self, v: &mut [f32], at: usize);
    fn splat(x: f32) -> Self;
    fn add(self, o: Self) -> Self;
    fn sub(self, o: Self) -> Self;
    fn mul(self, o: Self) -> Self;
    fn neg(self) -> Self;
}

impl Lane for f32 {
    #[inline(always)]
    fn store(self, v: &mut [f32], at: usize) {
        if let Some(slot) = v.get_mut(at) {
            *slot = self;
        }
    }
    #[inline(always)]
    fn splat(x: f32) -> Self {
        x
    }
    #[inline(always)]
    fn add(self, o: Self) -> Self {
        self + o
    }
    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        self - o
    }
    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        self * o
    }
    #[inline(always)]
    fn neg(self) -> Self {
        -self
    }
}

#[derive(Clone, Copy, Default)]
struct F4([f32; 4]);

impl F4 {
    #[inline(always)]
    fn zip(self, o: Self, f: impl Fn(f32, f32) -> f32) -> Self {
        let [a0, a1, a2, a3] = self.0;
        let [b0, b1, b2, b3] = o.0;
        Self([f(a0, b0), f(a1, b1), f(a2, b2), f(a3, b3)])
    }
}

impl Lane for F4 {
    #[inline(always)]
    fn store(self, v: &mut [f32], at: usize) {
        if let Some(slot) = v.get_mut(at..at + 4) {
            slot.copy_from_slice(&self.0);
        }
    }
    #[inline(always)]
    fn splat(x: f32) -> Self {
        Self([x; 4])
    }
    #[inline(always)]
    fn add(self, o: Self) -> Self {
        self.zip(o, |a, b| a + b)
    }
    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        self.zip(o, |a, b| a - b)
    }
    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        self.zip(o, |a, b| a * b)
    }
    #[inline(always)]
    fn neg(self) -> Self {
        let [a, b, c, d] = self.0;
        Self([-a, -b, -c, -d])
    }
}

/// Complex values in lanes.
#[derive(Clone, Copy)]
struct Cx<V> {
    re: V,
    im: V,
}

impl<V: Lane> Cx<V> {
    #[inline(always)]
    fn add(self, o: Self) -> Self {
        Self {
            re: self.re.add(o.re),
            im: self.im.add(o.im),
        }
    }
    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        Self {
            re: self.re.sub(o.re),
            im: self.im.sub(o.im),
        }
    }
    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        Self {
            re: self.re.mul(o.re).sub(self.im.mul(o.im)),
            im: self.re.mul(o.im).add(self.im.mul(o.re)),
        }
    }
    #[inline(always)]
    fn scale(self, s: f32) -> Self {
        let s = V::splat(s);
        Self {
            re: self.re.mul(s),
            im: self.im.mul(s),
        }
    }
    /// Multiplied by -i.
    #[inline(always)]
    fn neg_i(self) -> Self {
        Self {
            re: self.im,
            im: self.re.neg(),
        }
    }
}

const SQRT3_2: f32 = 0.866_025_4;
const COS_2PI_5: f32 = 0.309_017;
const COS_4PI_5: f32 = -0.809_017;
const SIN_2PI_5: f32 = 0.951_056_5;
const SIN_4PI_5: f32 = 0.587_785_25;

/// The DFTs of 2 to 5 points, forward: y\[m\] = sum x\[q\] e^(-2 pi i m q / R).
#[inline(always)]
fn dft2<V: Lane>([a, b]: [Cx<V>; 2]) -> [Cx<V>; 2] {
    [a.add(b), a.sub(b)]
}

#[inline(always)]
fn dft3<V: Lane>([a, b, c]: [Cx<V>; 3]) -> [Cx<V>; 3] {
    let s = b.add(c);
    let t1 = a.sub(s.scale(0.5));
    let t2 = b.sub(c).scale(SQRT3_2).neg_i();
    [a.add(s), t1.add(t2), t1.sub(t2)]
}

#[inline(always)]
fn dft4<V: Lane>([a, b, c, d]: [Cx<V>; 4]) -> [Cx<V>; 4] {
    let s02 = a.add(c);
    let d02 = a.sub(c);
    let s13 = b.add(d);
    let d13 = b.sub(d).neg_i();
    [s02.add(s13), d02.add(d13), s02.sub(s13), d02.sub(d13)]
}

#[inline(always)]
fn dft5<V: Lane>([a, b, c, d, e]: [Cx<V>; 5]) -> [Cx<V>; 5] {
    let b1 = b.add(e);
    let b2 = c.add(d);
    let d1 = b.sub(e);
    let d2 = c.sub(d);
    let r1 = a.add(b1.scale(COS_2PI_5)).add(b2.scale(COS_4PI_5));
    let r2 = a.add(b1.scale(COS_4PI_5)).add(b2.scale(COS_2PI_5));
    let i1 = d1.scale(SIN_2PI_5).add(d2.scale(SIN_4PI_5)).neg_i();
    let i2 = d1.scale(SIN_4PI_5).sub(d2.scale(SIN_2PI_5)).neg_i();
    [
        a.add(b1).add(b2),
        r1.add(i1),
        r2.add(i2),
        r2.sub(i2),
        r1.sub(i1),
    ]
}

/// Four complex values: their real parts, then their imaginary parts.
type Block = [f32; 8];

#[inline(always)]
fn load(b: &Block) -> Cx<F4> {
    let [r0, r1, r2, r3, i0, i1, i2, i3] = *b;
    Cx {
        re: F4([r0, r1, r2, r3]),
        im: F4([i0, i1, i2, i3]),
    }
}

#[inline(always)]
fn store(y: Cx<F4>, b: &mut Block) {
    let ([r0, r1, r2, r3], [i0, i1, i2, i3]) = (y.re.0, y.im.0);
    *b = [r0, r1, r2, r3, i0, i1, i2, i3];
}

/// A radix-2 butterfly on four lanes, its second output multiplied by `w`.
/// The butterflies are functions the compiler keeps whole (not inlined
/// into their loops): it then vectorizes their four lanes, where in a loop
/// it may vectorize across iterations instead, gathering lanes.
#[inline(never)]
fn butterfly2(a: &mut Block, b: &mut Block, w: &Block) {
    let [y0, y1] = dft2([load(a), load(b)]);
    store(y0, a);
    store(y1.mul(load(w)), b);
}

/// A radix-4 butterfly on four lanes, outputs 1 to 3 multiplied by `w`.
#[inline(never)]
fn butterfly4(a: &mut Block, b: &mut Block, c: &mut Block, d: &mut Block, w: [&Block; 3]) {
    let [w1, w2, w3] = w;
    let [y0, y1, y2, y3] = dft4([load(a), load(b), load(c), load(d)]);
    store(y0, a);
    store(y1.mul(load(w1)), b);
    store(y2.mul(load(w2)), c);
    store(y3.mul(load(w3)), d);
}

/// An in-place pass of the FFT: in each run of `radix * span` blocks, the
/// `radix`-point DFTs of the values `span` blocks apart, output m
/// multiplied by the twiddle e^(-2 pi i m j / (radix span 4)) of its value
/// j in the run's first `span` blocks (`twiddles[j / 4][m - 1]`, in lanes).
struct Stage {
    radix: usize,
    span: usize,
    twiddles: Vec<[Block; 4]>,
}

impl Stage {
    fn run(&self, z: &mut [Block]) {
        match self.radix {
            2 => self.pass2(z),
            4 if self.span == 1 => self.pass4_span1(z),
            4 => self.pass4(z),
            3 => self.pass::<3>(z, dft3),
            _ => self.pass::<5>(z, dft5),
        }
    }

    fn pass2(&self, z: &mut [Block]) {
        let span = self.span;
        for run in z.chunks_exact_mut(2 * span) {
            let (a, b) = run.split_at_mut(span);
            for ((a, b), [w, ..]) in a.iter_mut().zip(b.iter_mut()).zip(&self.twiddles) {
                butterfly2(a, b, w);
            }
        }
    }

    fn pass4(&self, z: &mut [Block]) {
        let span = self.span;
        for run in z.chunks_exact_mut(4 * span) {
            let (a, rest) = run.split_at_mut(span);
            let (b, rest) = rest.split_at_mut(span);
            let (c, d) = rest.split_at_mut(span);
            for ((((a, b), c), d), [w1, w2, w3, _]) in a
                .iter_mut()
                .zip(b.iter_mut())
                .zip(c.iter_mut())
                .zip(d.iter_mut())
                .zip(&self.twiddles)
            {
                butterfly4(a, b, c, d, [w1, w2, w3]);
            }
        }
    }

    /// Span 1: every run is four adjacent blocks with the same twiddles.
    fn pass4_span1(&self, z: &mut [Block]) {
        let Some([w1, w2, w3, _]) = self.twiddles.first() else {
            return;
        };
        for [a, b, c, d] in z.as_chunks_mut::<4>().0 {
            butterfly4(a, b, c, d, [w1, w2, w3]);
        }
    }

    /// Radix 3 and 5 (frames of 960 only).
    fn pass<const R: usize>(&self, z: &mut [Block], dft: impl Fn([Cx<F4>; R]) -> [Cx<F4>; R]) {
        let span = self.span;
        let zero = Cx {
            re: F4::default(),
            im: F4::default(),
        };
        for run in z.chunks_exact_mut(R * span) {
            for (j, twiddles) in self.twiddles.iter().enumerate().take(span) {
                let x = core::array::from_fn(|q| run.get(q * span + j).map_or(zero, load));
                for (m, y) in dft(x).into_iter().enumerate() {
                    let y = match m.checked_sub(1).and_then(|t| twiddles.get(t)) {
                        Some(w) => y.mul(load(w)),
                        None => y,
                    };
                    if let Some(b) = run.get_mut(m * span + j) {
                        store(y, b);
                    }
                }
            }
        }
    }
}

/// A complex FFT of `size` points, forward (e^(-2 pi i k n / size)), by
/// decimation in frequency: passes of radix 2 to 5 in place, on blocks of
/// four values (four real parts, four imaginary parts), then a last pass
/// of radix 4 out of place. A pass combines values a span apart; the
/// radices are ordered so that every in-place span is a multiple of four,
/// and a pass's butterflies run four at a time in lanes, which the
/// compiler keeps in vector registers.
///
/// Decimation in frequency leaves output k of the in-place passes in
/// block `block[k mod size/4]`, at lane `k / (size / 4)`. The last pass
/// loads the blocks of four consecutive k mod size/4 and transposes them,
/// so its 4-point DFTs run in lanes and store four consecutive outputs at
/// a time: the output is in natural order, with no reordering pass.
struct Fft {
    size: usize,
    stages: Vec<Stage>,
    /// The block of each k below size / 4.
    block: Vec<u16>,
}

impl Fft {
    fn new(size: usize) -> Self {
        let (radices, roots, stride): (&'static [usize], &'static [[f32; 2]], usize) = match size {
            512 => (&[2, 4, 4, 4], &FFT_ROOTS_512, 1),
            64 => (&[4, 4], &FFT_ROOTS_512, 8),
            480 => (&[2, 4, 3, 5], &FFT_ROOTS_480, 1),
            _ => (&[3, 5], &FFT_ROOTS_480, 8),
        };
        let mut stages = Vec::with_capacity(radices.len());
        let mut len = size;
        for &radix in radices {
            let span = len / radix;
            // e^(-2 pi i m j / len) is root (m j size / len).
            let step = size / len * stride;
            let twiddles = (0..span / 4)
                .map(|jb| {
                    core::array::from_fn(|t| {
                        let mut block = [1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0];
                        let (re, im) = block.split_at_mut(4);
                        for (l, (re, im)) in re.iter_mut().zip(im.iter_mut()).enumerate() {
                            let m = t + 1;
                            if m < radix {
                                let [c, s] = roots
                                    .get(m * (4 * jb + l) * step)
                                    .copied()
                                    .unwrap_or([1.0, 0.0]);
                                *re = c;
                                *im = s;
                            }
                        }
                        block
                    })
                })
                .collect();
            stages.push(Stage {
                radix,
                span: span / 4,
                twiddles,
            });
            len = span;
        }
        // A pass's output m = k mod R goes to sub-run m, whose own
        // transform gives k / R; the last pass's span is 1.
        let block = (0..size / 4)
            .map(|k| {
                let (mut rest, mut len, mut at) = (k, size, 0);
                for &radix in radices {
                    let span = len / radix;
                    at += rest % radix * span;
                    rest /= radix;
                    len = span;
                }
                (at / 4) as u16
            })
            .collect();
        Self {
            size,
            stages,
            block,
        }
    }

    /// Transforms `z` (`size / 4` blocks) into `out_re` and `out_im` in
    /// natural order; `z` is scratch.
    fn run(&self, z: &mut [Block], out_re: &mut [f32], out_im: &mut [f32]) {
        for stage in &self.stages {
            stage.run(z);
        }
        let quarter = self.size / 4;
        const ZERO: Block = [0.0; 8];
        let row = |b: u16| z.get(b as usize).unwrap_or(&ZERO);
        let block = |b: u16| *row(b);
        let groups = self.block.as_chunks::<4>();
        for (g, &[b0, b1, b2, b3]) in groups.0.iter().enumerate() {
            let rows = [row(b0), row(b1), row(b2), row(b3)];
            last4(rows, out_re, out_im, 4 * g, quarter);
        }
        // Sizes whose quarter is not a multiple of 4: the blocks left.
        let first = 4 * groups.0.len();
        for (k, &b) in groups.1.iter().enumerate() {
            let k = first + k;
            let Cx {
                re: F4([r0, r1, r2, r3]),
                im: F4([i0, i1, i2, i3]),
            } = load(&block(b));
            let x0 = Cx { re: r0, im: i0 };
            let x1 = Cx { re: r1, im: i1 };
            let x2 = Cx { re: r2, im: i2 };
            let x3 = Cx { re: r3, im: i3 };
            for (d, y) in dft4::<f32>([x0, x1, x2, x3]).into_iter().enumerate() {
                y.re.store(out_re, k + d * quarter);
                y.im.store(out_im, k + d * quarter);
            }
        }
    }
}

/// The last pass on four blocks: transposed, so lane t holds block t's
/// values, then 4-point DFTs in lanes, output d of lane t to `at + t + d
/// quarter`. Not inlined, as the butterflies.
#[inline(never)]
fn last4(rows: [&Block; 4], out_re: &mut [f32], out_im: &mut [f32], at: usize, quarter: usize) {
    let [
        &[a0, a1, a2, a3, a4, a5, a6, a7],
        &[b0, b1, b2, b3, b4, b5, b6, b7],
        &[c0, c1, c2, c3, c4, c5, c6, c7],
        &[d0, d1, d2, d3, d4, d5, d6, d7],
    ] = rows;
    let [r0, r1, r2, r3] = transpose([
        [a0, a1, a2, a3],
        [b0, b1, b2, b3],
        [c0, c1, c2, c3],
        [d0, d1, d2, d3],
    ]);
    let [i0, i1, i2, i3] = transpose([
        [a4, a5, a6, a7],
        [b4, b5, b6, b7],
        [c4, c5, c6, c7],
        [d4, d5, d6, d7],
    ]);
    let y = dft4([cx4(&r0, &i0), cx4(&r1, &i1), cx4(&r2, &i2), cx4(&r3, &i3)]);
    for (d, y) in y.into_iter().enumerate() {
        y.re.store(out_re, at + d * quarter);
        y.im.store(out_im, at + d * quarter);
    }
}

/// A value of lanes from a real and an imaginary lane array.
#[inline(always)]
fn cx4(re: &[f32; 4], im: &[f32; 4]) -> Cx<F4> {
    Cx {
        re: F4(*re),
        im: F4(*im),
    }
}

#[inline(always)]
fn transpose(rows: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let [
        [a0, a1, a2, a3],
        [b0, b1, b2, b3],
        [c0, c1, c2, c3],
        [d0, d1, d2, d3],
    ] = rows;
    [
        [a0, b0, c0, d0],
        [a1, b1, c1, d1],
        [a2, b2, c2, d2],
        [a3, b3, c3, d3],
    ]
}

/// Four values of the pre-twiddle: (X\[2j\] + i X\[m - 1 - 2j\]) times
/// the twiddle (`c`, `s`), from `x` (X\[2j..2j + 8\]) and `y` (X\[m - 8 -
/// 2j..m - 2j\]). Not inlined, as the butterflies.
#[inline(never)]
fn pre_twiddle(b: &mut Block, c: &[f32; 4], s: &[f32; 4], x: &[f32; 8], y: &[f32; 8]) {
    let [x0, _, x2, _, x4, _, x6, _] = *x;
    let [_, y3, _, y2, _, y1, _, y0] = *y;
    let v = Cx {
        re: F4([x0, x2, x4, x6]),
        im: F4([y0, y1, y2, y3]),
    };
    store(v.mul(cx4(c, s)), b);
}

/// Four values of the post-twiddle: w = Z twiddle, `even` its real parts,
/// `odd` its imaginary parts negated.
#[inline(never)]
fn post_twiddle(
    even: &mut [f32; 4],
    odd: &mut [f32; 4],
    z: (&[f32; 4], &[f32; 4]),
    twiddle: (&[f32; 4], &[f32; 4]),
) {
    let w = cx4(z.0, z.1).mul(cx4(twiddle.0, twiddle.1));
    *even = w.re.0;
    *odd = w.im.neg().0;
}

/// The IMDCT of one window length.
struct Imdct {
    m: usize,
    /// e^(-i pi (j + 1/8) / m), j below m / 2.
    cos: Vec<f32>,
    sin: Vec<f32>,
    fft: Fft,
}

impl Imdct {
    fn new(m: usize) -> Self {
        let twiddle: &'static [[f32; 2]] = match m {
            1024 => &IMDCT_TWIDDLE_1024,
            128 => &IMDCT_TWIDDLE_128,
            960 => &IMDCT_TWIDDLE_960,
            _ => &IMDCT_TWIDDLE_120,
        };
        Self {
            m,
            cos: twiddle.iter().map(|t| t[0]).collect(),
            sin: twiddle.iter().map(|t| t[1]).collect(),
            fft: Fft::new(m / 2),
        }
    }

    /// The DCT-IV C of `input` (m values), without normalization: C\[2p\]
    /// into `work.even[p]`, C\[m - 1 - 2p\] into `work.odd[p]`.
    fn dct4(&self, input: &[f32], work: &mut Work) {
        let m = self.m;
        let half = m / 2;
        let Work {
            z,
            zr,
            zi,
            even,
            odd,
            ..
        } = work;
        let (Some(z), Some(zr), Some(zi), Some(even), Some(odd)) = (
            z.get_mut(..half / 4),
            zr.get_mut(..half),
            zi.get_mut(..half),
            even.get_mut(..half),
            odd.get_mut(..half),
        ) else {
            return;
        };
        // The spectrum may be longer (frames of 960 in 1,024 values).
        let input = input.get(..m).unwrap_or(input);
        // z[j] = (X[2j] + i X[m - 1 - 2j]) e^(-i pi (j + 1/8) / m), four j
        // at a time (m is a multiple of 8).
        for (((b, c), s), (x, y)) in z
            .iter_mut()
            .zip(self.cos.as_chunks::<4>().0)
            .zip(self.sin.as_chunks::<4>().0)
            .zip(
                input
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .zip(input.as_rchunks::<8>().1.iter().rev()),
            )
        {
            pre_twiddle(b, c, s, x, y);
        }
        self.fft.run(z, zr, zi);
        // w[p] = Z[p] e^(-i pi (p + 1/8) / m): C[2p] = Re w[p] and
        // C[m - 1 - 2p] = -Im w[p].
        for ((((e, o), r), i), (c, s)) in even
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(odd.as_chunks_mut::<4>().0)
            .zip(zr.as_chunks::<4>().0)
            .zip(zi.as_chunks::<4>().0)
            .zip(
                self.cos
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(self.sin.as_chunks::<4>().0),
            )
        {
            post_twiddle(e, o, (r, i), (c, s));
        }
    }

    /// The IMDCT of `input` (m values) into `out` (2m values), without the
    /// factor 2/N.
    fn run(&self, input: &[f32], out: &mut [f32], work: &mut Work) {
        let m = self.m;
        let half = m / 2;
        self.dct4(input, work);
        let Work { dct, even, odd, .. } = work;
        let Some(dct) = dct.get_mut(..m) else {
            return;
        };
        // C[2p + 1] = C[m - 1 - 2(half - 1 - p)].
        for (pair, (&e, &o)) in dct
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(even.iter().take(half).zip(odd.iter().take(half).rev()))
        {
            *pair = [e, o];
        }
        let dct = &*dct;
        // With A = C[..m/2] and B = C[m/2..]: out = B, -B reversed,
        // -A reversed, -A.
        let (a, b) = dct.split_at(half);
        let Some(out) = out.get_mut(..2 * m) else {
            return;
        };
        let (first, second) = out.split_at_mut(m);
        let (q0, q1) = first.split_at_mut(half);
        let (q2, q3) = second.split_at_mut(half);
        q0.copy_from_slice(b);
        for (d, &x) in q1.iter_mut().zip(b.iter().rev()) {
            *d = -x;
        }
        for (d, &x) in q2.iter_mut().zip(a.iter().rev()) {
            *d = -x;
        }
        for (d, &x) in q3.iter_mut().zip(a) {
            *d = -x;
        }
    }
}

/// Scratch buffers of the filterbank.
struct Work {
    dct: Vec<f32>,
    z: Vec<Block>,
    zr: Vec<f32>,
    zi: Vec<f32>,
    even: Vec<f32>,
    odd: Vec<f32>,
    time: Vec<f32>,
    short: Vec<f32>,
}

/// The overlap of a channel: the second half of its last windowed IMDCT
/// output, and the last window shape.
#[derive(Clone, Debug)]
pub(crate) struct Overlap {
    samples: Vec<f32>,
    shape: u8,
}

impl Overlap {
    pub(crate) fn new(frame: usize) -> Self {
        Self {
            samples: vec![0.0; frame],
            shape: 0,
        }
    }

    /// The state after a reset: silence, the sine shape.
    pub(crate) fn reset(&mut self) {
        self.samples.iter_mut().for_each(|s| *s = 0.0);
        self.shape = 0;
    }
}

/// The filterbank of a stream's frame length.
pub(crate) struct Filterbank {
    frame: usize,
    long: Imdct,
    short: Imdct,
    /// Rising halves of the short windows, sine then KBD.
    short_windows: [&'static [f32]; 2],
    /// The window over the first half of a long block's output, by shape
    /// (sine, KBD): of ONLY_LONG and LONG_START (the long window's rising
    /// half), then of LONG_STOP (zeros, the short window's rising half,
    /// ones).
    left: [Vec<f32>; 4],
    /// Over the second half: of ONLY_LONG and LONG_STOP (the long window's
    /// falling half), then of LONG_START (ones, the short window's falling
    /// half, zeros).
    right: [Vec<f32>; 4],
    work: Work,
}

impl Clone for Filterbank {
    fn clone(&self) -> Self {
        Self::new(self.frame)
    }
}

impl Filterbank {
    pub(crate) fn new(frame: usize) -> Self {
        let short = frame / 8;
        let flat = (frame - short) / 2;
        let (long_windows, short_windows): ([&'static [f32]; 2], [&'static [f32]; 2]) =
            if frame == 960 {
                (
                    [&SINE_LONG_960, &KBD_LONG_960],
                    [&SINE_SHORT_120, &KBD_SHORT_120],
                )
            } else {
                (
                    [&SINE_LONG_1024, &KBD_LONG_1024],
                    [&SINE_SHORT_128, &KBD_SHORT_128],
                )
            };
        let stop = |w: &[f32]| -> Vec<f32> {
            let mut v = vec![0.0; flat];
            v.extend_from_slice(w);
            v.resize(frame, 1.0);
            v
        };
        let start = |w: &[f32]| -> Vec<f32> {
            let mut v = vec![1.0; flat];
            v.extend(w.iter().rev());
            v.resize(frame, 0.0);
            v
        };
        let [long_sine, long_kbd] = long_windows;
        let [short_sine, short_kbd] = short_windows;
        Self {
            frame,
            long: Imdct::new(frame),
            short: Imdct::new(short),
            short_windows,
            left: [
                long_sine.to_vec(),
                long_kbd.to_vec(),
                stop(short_sine),
                stop(short_kbd),
            ],
            right: [
                long_sine.iter().rev().copied().collect(),
                long_kbd.iter().rev().copied().collect(),
                start(short_sine),
                start(short_kbd),
            ],
            work: Work {
                dct: vec![0.0; frame],
                z: vec![[0.0; 8]; frame / 8],
                zr: vec![0.0; frame / 2],
                zi: vec![0.0; frame / 2],
                even: vec![0.0; frame / 2],
                odd: vec![0.0; frame / 2],
                time: vec![0.0; 2 * frame],
                short: vec![0.0; 2 * short],
            },
        }
    }

    /// Transforms one channel's spectrum (`info`'s window sequence and
    /// shape) and writes the frame's samples to `out`, updating `state`.
    pub(crate) fn synthesize(
        &mut self,
        info: &Info,
        spec: &[f32],
        state: &mut Overlap,
        out: &mut [f32],
    ) {
        let prev = state.shape as usize & 1;
        let cur = info.window_shape as usize & 1;
        if info.window_sequence == EIGHT_SHORT {
            self.synthesize_short(spec, state, out, prev, cur);
        } else {
            let left = self.left.get(
                prev + if info.window_sequence == LONG_STOP {
                    2
                } else {
                    0
                },
            );
            let right = self.right.get(
                cur + if info.window_sequence == LONG_START {
                    2
                } else {
                    0
                },
            );
            if let (Some(left), Some(right)) = (left, right) {
                self.long.dct4(spec, &mut self.work);
                long_block(&self.work.even, &self.work.odd, left, right, state, out);
            }
        }
        state.shape = info.window_shape;
    }

    /// Eight short windows: each IMDCT windowed and added at its place in
    /// the block, the block's first half added to the overlap.
    fn synthesize_short(
        &mut self,
        spec: &[f32],
        state: &mut Overlap,
        out: &mut [f32],
        prev: usize,
        cur: usize,
    ) {
        let m = self.frame;
        let s = m / 8;
        let flat = (m - s) / 2;
        let mut time = core::mem::take(&mut self.work.time);
        time.iter_mut().for_each(|t| *t = 0.0);
        let mut short = core::mem::take(&mut self.work.short);
        for j in 0..8 {
            let input = spec.get(j * s..(j + 1) * s).unwrap_or_default();
            self.short.run(input, &mut short, &mut self.work);
            let left = self
                .short_windows
                .get(if j == 0 { prev } else { cur })
                .copied()
                .unwrap_or_default();
            let right = self.short_windows.get(cur).copied().unwrap_or_default();
            let at = flat + j * s;
            if let Some(dst) = time.get_mut(at..at + 2 * s) {
                let (first, second) = dst.split_at_mut(s);
                for ((d, &x), &w) in first.iter_mut().zip(&short).zip(left) {
                    *d += x * w;
                }
                for ((d, &x), &w) in second
                    .iter_mut()
                    .zip(short.get(s..).unwrap_or_default())
                    .zip(right.iter().rev())
                {
                    *d += x * w;
                }
            }
        }
        self.work.short = short;
        for ((o, &t), &p) in out.iter_mut().zip(&time).zip(&state.samples).take(m) {
            *o = p + t;
        }
        if let Some(rest) = time.get(m..2 * m) {
            state.samples.copy_from_slice(rest);
        }
        self.work.time = time;
    }
}

/// A long block from its DCT-IV C (m values, `even[p]` = C\[2p\], `odd[p]`
/// = C\[m - 1 - 2p\]). With A = C\[..m/2\] and B = C\[m/2..\], the IMDCT
/// output is B, -B reversed, -A reversed, -A. Its first half, windowed by
/// `left`, is added to the overlap into `out`; its second half, windowed by
/// `right`, is the new overlap. Taken two samples at a time, B\[2t\] =
/// `even[m/4 + t]`, B\[2t + 1\] = `odd[m/4 - 1 - t]`, A\[2t\] = `even[t]` and
/// A\[2t + 1\] = `odd[m/2 - 1 - t]`.
fn long_block(
    even: &[f32],
    odd: &[f32],
    left: &[f32],
    right: &[f32],
    state: &mut Overlap,
    out: &mut [f32],
) {
    let m = state.samples.len();
    let half = m / 2;
    let quarter = m / 4;
    let (Some(out), Some(left), Some(right), Some(even), Some(odd)) = (
        out.get_mut(..m),
        left.get(..m),
        right.get(..m),
        even.get(..half),
        odd.get(..half),
    ) else {
        return;
    };
    fn pairs(v: &[f32], half: usize) -> (&[[f32; 2]], &[[f32; 2]]) {
        let (a, b) = v.split_at(half.min(v.len()));
        (a.as_chunks().0, b.as_rchunks().1)
    }
    let (e0, e1) = even.split_at(quarter);
    let (o0, o1) = odd.split_at(quarter);
    // B two at a time: (even[m/4 + t], odd[m/4 - 1 - t]).
    let b = || e1.iter().zip(o0.iter().rev());
    // A two at a time: (even[t], odd[m/2 - 1 - t]).
    let a = || e0.iter().zip(o1.iter().rev());
    let ov = &mut state.samples;
    {
        let (p0, p1) = pairs(ov, half);
        let (l0, l1) = pairs(left, half);
        let (o_0, o_1) = out.split_at_mut(half);
        let (o_0, o_1) = (o_0.as_chunks_mut::<2>().0, o_1.as_rchunks_mut::<2>().1);
        // out[i] = ov[i] + B[i] left[i], i below m/2.
        for (((o, &[p0, p1]), &[w0, w1]), (&x, &y)) in o_0.iter_mut().zip(p0).zip(l0).zip(b()) {
            *o = [p0 + x * w0, p1 + y * w1];
        }
        // out[m - 1 - i] = ov[m - 1 - i] - B[i] left[m - 1 - i].
        for (((o, &[p0, p1]), &[w0, w1]), (&x, &y)) in o_1
            .iter_mut()
            .rev()
            .zip(p1.iter().rev())
            .zip(l1.iter().rev())
            .zip(b())
        {
            *o = [p0 - y * w0, p1 - x * w1];
        }
    }
    let (s0, s1) = ov.split_at_mut(half);
    let (s0, s1) = (s0.as_rchunks_mut::<2>().1, s1.as_chunks_mut::<2>().0);
    let (r0, r1) = pairs(right, half);
    // ov[m/2 - 1 - i] = -A[i] right[m/2 - 1 - i].
    for ((s, &[w0, w1]), (&x, &y)) in s0.iter_mut().rev().zip(r0.iter().rev()).zip(a()) {
        *s = [-(y * w0), -(x * w1)];
    }
    // ov[m/2 + i] = -A[i] right[m/2 + i].
    for ((s, &[w0, w1]), (&x, &y)) in s1.iter_mut().zip(r1).zip(a()) {
        *s = [-(x * w0), -(y * w1)];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct_dft(x: &[(f32, f32)]) -> Vec<(f32, f32)> {
        let n = x.len();
        (0..n)
            .map(|k| {
                let mut acc = (0f64, 0f64);
                for (j, &(re, im)) in x.iter().enumerate() {
                    let a = -2.0 * std::f64::consts::PI * (j * k % n) as f64 / n as f64;
                    let (s, c) = a.sin_cos();
                    acc.0 += re as f64 * c - im as f64 * s;
                    acc.1 += re as f64 * s + im as f64 * c;
                }
                (acc.0 as f32, acc.1 as f32)
            })
            .collect()
    }

    fn signal(n: usize) -> Vec<(f32, f32)> {
        let mut state = 12345u32;
        (0..n)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let re = (state >> 8) as f32 / (1 << 24) as f32 - 0.5;
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let im = (state >> 8) as f32 / (1 << 24) as f32 - 0.5;
                (re, im)
            })
            .collect()
    }

    #[test]
    fn fft_matches_the_dft() {
        for n in [512, 64, 480, 60] {
            let x = signal(n);
            let want = direct_dft(&x);
            let mut z: Vec<Block> = x
                .chunks(4)
                .map(|c| {
                    let mut b = [0f32; 8];
                    for (l, v) in c.iter().enumerate() {
                        b[l] = v.0;
                        b[4 + l] = v.1;
                    }
                    b
                })
                .collect();
            let (mut zr, mut zi) = (vec![0f32; n], vec![0f32; n]);
            Fft::new(n).run(&mut z, &mut zr, &mut zi);
            let err = want
                .iter()
                .zip(zr.iter().zip(&zi))
                .map(|(w, (r, i))| (r - w.0).abs().max((i - w.1).abs()))
                .fold(0f32, f32::max);
            assert!(err < 1e-3, "n {n}: error {err}");
        }
    }

    /// A long block as the standard writes it: the IMDCT output (2m
    /// values), windowed by the sequence's window, its first half added to
    /// the overlap.
    fn long_reference(
        fb: &mut Filterbank,
        sequence: u8,
        prev: usize,
        cur: usize,
        spec: &[f32],
        overlap: &[f32],
    ) -> (Vec<f32>, Vec<f32>) {
        let m = fb.frame;
        let (s, flat) = (m / 8, (m - m / 8) / 2);
        let mut time = vec![0f32; 2 * m];
        let imdct = Imdct::new(m);
        imdct.run(&spec[..m], &mut time, &mut fb.work);
        let (long, short) = if m == 960 {
            (
                [&SINE_LONG_960[..], &KBD_LONG_960[..]],
                [&SINE_SHORT_120[..], &KBD_SHORT_120[..]],
            )
        } else {
            (
                [&SINE_LONG_1024[..], &KBD_LONG_1024[..]],
                [&SINE_SHORT_128[..], &KBD_SHORT_128[..]],
            )
        };
        let mut window = vec![0f32; 2 * m];
        for n in 0..m {
            window[n] = if sequence == LONG_STOP {
                if n < flat {
                    0.0
                } else if n < flat + s {
                    short[prev][n - flat]
                } else {
                    1.0
                }
            } else {
                long[prev][n]
            };
            window[m + n] = if sequence == LONG_START {
                if n < flat {
                    1.0
                } else if n < flat + s {
                    short[cur][s - 1 - (n - flat)]
                } else {
                    0.0
                }
            } else {
                long[cur][m - 1 - n]
            };
        }
        let out = (0..m).map(|n| overlap[n] + time[n] * window[n]).collect();
        let next = (m..2 * m).map(|n| time[n] * window[n]).collect();
        (out, next)
    }

    #[test]
    fn long_blocks_match_the_window_and_overlap() {
        for m in [1024, 960] {
            long_blocks_of(m);
        }
    }

    fn long_blocks_of(m: usize) {
        // The decoder's spectrum holds 1,024 values for both frame lengths.
        let spec: Vec<f32> = signal(1024).iter().map(|c| c.0).collect();
        let overlap: Vec<f32> = signal(2 * m).iter().skip(m).map(|c| c.1).collect();
        for sequence in [0, LONG_START, LONG_STOP] {
            for (prev, cur) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let mut fb = Filterbank::new(m);
                let (want_out, want_next) =
                    long_reference(&mut fb, sequence, prev, cur, &spec, &overlap);
                let mut state = Overlap {
                    samples: overlap.clone(),
                    shape: prev as u8,
                };
                let info = Info {
                    window_sequence: sequence,
                    window_shape: cur as u8,
                    ..Info::default()
                };
                let mut out = vec![0f32; m];
                fb.synthesize(&info, &spec, &mut state, &mut out);
                assert_eq!(out, want_out, "sequence {sequence}, shapes {prev} {cur}");
                assert_eq!(
                    state.samples, want_next,
                    "sequence {sequence}, shapes {prev} {cur}"
                );
            }
        }
    }

    #[test]
    fn imdct_matches_its_definition() {
        for m in [1024, 128, 960, 120] {
            let input: Vec<f32> = signal(m).iter().map(|c| c.0).collect();
            let imdct = Imdct::new(m);
            let mut fb = Filterbank::new(if m >= 960 { m } else { m * 8 });
            let mut out = vec![0f32; 2 * m];
            imdct.run(&input, &mut out, &mut fb.work);
            let n = 2 * m;
            let n0 = (m as f64 + 1.0) / 2.0;
            let mut err = 0f64;
            for (i, &got) in out.iter().enumerate() {
                let mut acc = 0f64;
                for (k, &x) in input.iter().enumerate() {
                    let a =
                        2.0 * std::f64::consts::PI / n as f64 * (i as f64 + n0) * (k as f64 + 0.5);
                    acc += x as f64 * a.cos();
                }
                err = err.max((acc - got as f64).abs());
            }
            assert!(err < 2e-3 * (m as f64).sqrt(), "m {m}: error {err}");
        }
    }
}
