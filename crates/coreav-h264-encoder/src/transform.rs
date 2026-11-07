#![forbid(unsafe_code)]

use crate::{Error, Result};
use std::sync::LazyLock;

pub(crate) const SCAN: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
const ADJUST: [[i64; 3]; 6] = [
    [10, 16, 13],
    [11, 18, 14],
    [13, 20, 16],
    [14, 23, 18],
    [16, 25, 20],
    [18, 29, 23],
];
const SHIFT: u32 = 40;
const INTRA_OFFSET: u64 = 5;
const INTER_OFFSET: u64 = 2;
const CHROMA_INTRA_OFFSET: u64 = 5;
const CHROMA_INTER_OFFSET: u64 = 5;
const OFFSET_DENOMINATOR: u64 = 12;
static QUANT: LazyLock<[Quant; 52]> =
    LazyLock::new(|| std::array::from_fn(|qp| Quant::new(qp as u8, INTER_OFFSET)));
static INTRA_QUANT: LazyLock<[Quant; 52]> =
    LazyLock::new(|| std::array::from_fn(|qp| Quant::new(qp as u8, INTRA_OFFSET)));
static CHROMA_QUANT: LazyLock<[Quant; 52]> =
    LazyLock::new(|| std::array::from_fn(|qp| Quant::new(qp as u8, CHROMA_INTER_OFFSET)));
static INTRA_CHROMA_QUANT: LazyLock<[Quant; 52]> =
    LazyLock::new(|| std::array::from_fn(|qp| Quant::new(qp as u8, CHROMA_INTRA_OFFSET)));

pub(crate) fn chroma_qp(qp: u8) -> u8 {
    const HIGH: [u8; 22] = [
        29, 30, 31, 32, 32, 33, 34, 34, 35, 35, 36, 36, 37, 37, 37, 38, 38, 38, 39, 39, 39, 39,
    ];
    if qp < 30 {
        qp
    } else {
        HIGH.get(usize::from(qp - 30)).copied().unwrap_or(39)
    }
}

pub(crate) fn norm(qp: u8, position: usize) -> Result<i64> {
    let row = (position / 4) % 2;
    let col = position % 2;
    let column = if row == col { row } else { 2 };
    ADJUST
        .get(usize::from(qp % 6))
        .and_then(|r| r.get(column))
        .copied()
        .ok_or(Error::Internal)
}

pub(crate) fn rate_weight(qp: u8) -> Result<u64> {
    let step = norm(qp, 0)?.unsigned_abs() << (qp / 6);
    Ok(step * step / 4)
}

fn rounded(numerator: i64, denominator: i64, offset: u64) -> Result<i32> {
    let magnitude = (numerator.unsigned_abs()
        + denominator.unsigned_abs() * offset / OFFSET_DENOMINATOR)
        / denominator.unsigned_abs();
    let level = i32::try_from(magnitude).map_err(|_| Error::Coefficient)?;
    Ok(if numerator < 0 { -level } else { level })
}

/// Forward core transform of one 4x4 residual of 8-bit samples, so every
/// coefficient is at most 36 * 255 in magnitude.
pub(crate) struct Forward([i32; 16]);

impl Forward {
    pub(crate) fn dc(&self) -> i32 {
        self.0.first().copied().unwrap_or(0)
    }

    pub(crate) fn coefficients(&self) -> &[i32; 16] {
        &self.0
    }
}

/// Quantizer constants of one QP by raster position: the zero bound, the
/// rounding bias and the reciprocal of the divisor, and the scaling factor.
pub(crate) struct Quant {
    offset: u64,
    zero: [u32; 16],
    bias: [u64; 16],
    reciprocal: [u64; 16],
    scale: [i32; 16],
}

impl Quant {
    fn new(qp: u8, offset: u64) -> Self {
        let scale = |p: usize| norm(qp, p).unwrap_or(0) << (qp / 6);
        let divisor = |p: usize| {
            ((if p / 4 % 2 == 0 { 4 } else { 5 }) * (if p % 2 == 0 { 4 } else { 5 }) * scale(p))
                .unsigned_abs()
        };
        Self {
            offset,
            zero: std::array::from_fn(|p| {
                let d = 2 * divisor(p);
                u32::try_from((d - 1 - d * offset / OFFSET_DENOMINATOR) / 128).unwrap_or(u32::MAX)
            }),
            bias: std::array::from_fn(|p| 2 * divisor(p) * offset / OFFSET_DENOMINATOR),
            reciprocal: std::array::from_fn(|p| {
                ((1u64 << SHIFT) + 2 * divisor(p) - 1)
                    .checked_div(2 * divisor(p))
                    .unwrap_or(0)
            }),
            scale: std::array::from_fn(|p| i32::try_from(scale(p)).unwrap_or(i32::MAX)),
        }
    }

    pub(crate) fn of(qp: u8) -> Result<&'static Self> {
        QUANT.get(usize::from(qp)).ok_or(Error::Internal)
    }

    pub(crate) fn intra(qp: u8) -> Result<&'static Self> {
        INTRA_QUANT.get(usize::from(qp)).ok_or(Error::Internal)
    }

    pub(crate) fn chroma_inter(qp: u8) -> Result<&'static Self> {
        CHROMA_QUANT.get(usize::from(qp)).ok_or(Error::Internal)
    }

    pub(crate) fn chroma_intra(qp: u8) -> Result<&'static Self> {
        INTRA_CHROMA_QUANT
            .get(usize::from(qp))
            .ok_or(Error::Internal)
    }

    pub(crate) fn chroma_dc(&self, sums: [i32; 4], qp: u8) -> Result<([i32; 4], [i32; 4])> {
        chroma_dc_offset(sums, qp, self.offset)
    }

    pub(crate) fn zero(&self, coefficients: &Forward, separate_dc: bool) -> bool {
        #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
        if let Some(simd) = crate::kernels::arithmetic() {
            return simd.zero(coefficients, self, separate_dc);
        }
        self.zero_scalar(coefficients, separate_dc)
    }

    pub(crate) fn zero_scalar(&self, coefficients: &Forward, separate_dc: bool) -> bool {
        coefficients
            .coefficients()
            .iter()
            .zip(&self.zero)
            .skip(usize::from(separate_dc))
            .all(|(c, &zero)| c.unsigned_abs() <= zero)
    }

    pub(crate) fn levels(&self, coefficients: &Forward, separate_dc: bool) -> [i32; 16] {
        #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
        if let Some(simd) = crate::kernels::arithmetic() {
            return simd.levels(coefficients, self, separate_dc);
        }
        self.levels_scalar(coefficients, separate_dc)
    }

    pub(crate) fn levels_scalar(&self, coefficients: &Forward, separate_dc: bool) -> [i32; 16] {
        let mut out = [0; 16];
        for (((level, &c), &bias), &reciprocal) in out
            .iter_mut()
            .zip(coefficients.coefficients())
            .zip(&self.bias)
            .zip(&self.reciprocal)
        {
            let magnitude =
                (((128 * u64::from(c.unsigned_abs()) + bias) * reciprocal) >> SHIFT) as i32;
            let sign = c >> 31;
            *level = (magnitude ^ sign) - sign;
        }
        if separate_dc && let Some(dc) = out.first_mut() {
            *dc = 0;
        }
        out
    }

    #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
    pub(crate) fn parameters(&self) -> (&[u32; 16], &[u64; 16], &[u64; 16], &[i32; 16]) {
        (&self.zero, &self.bias, &self.reciprocal, &self.scale)
    }

    pub(crate) fn scaled_scalar(&self, levels: &[i32; 16]) -> [i32; 16] {
        let mut out = [0; 16];
        for ((out, &level), &scale) in out.iter_mut().zip(levels).zip(&self.scale) {
            *out = level.wrapping_mul(scale);
        }
        out
    }

    fn scaled(&self, levels: &[i32; 16]) -> [i32; 16] {
        #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
        if let Some(simd) = crate::kernels::arithmetic() {
            return simd.scaled(levels, &self.scale);
        }
        self.scaled_scalar(levels)
    }

    #[inline]
    pub(crate) fn nearest_level(&self, coefficients: &Forward, p: usize) -> Result<i32> {
        let coefficient = *coefficients.0.get(p).ok_or(Error::Internal)?;
        let scale = *self.scale.get(p).ok_or(Error::Internal)?;
        let reciprocal = *self.reciprocal.get(p).ok_or(Error::Internal)?;
        let weight = (if p / 4 % 2 == 0 { 4 } else { 5 }) * (if p % 2 == 0 { 4 } else { 5 });
        let divisor = (weight * scale) as u64;
        let magnitude =
            ((128 * u64::from(coefficient.unsigned_abs()) + divisor) * reciprocal) >> SHIFT;
        Ok(magnitude as i32 * coefficient.signum())
    }

    pub(crate) fn reconstruct(
        &self,
        levels: &[i32; 16],
        dc: Option<i32>,
        samples: &mut [u8],
        at: usize,
        stride: usize,
    ) -> Result<()> {
        let mut coefficients = [0; 16];
        if levels.iter().any(|&level| level != 0) {
            coefficients = self.scaled(levels);
            if !coefficients.iter().all(|&c| fits(c)) {
                return Err(Error::Coefficient);
            }
        }
        if let (Some(dc), Some(first)) = (dc, coefficients.first_mut()) {
            *first = dc;
        }
        if coefficients.iter().all(|&c| c == 0) {
            return Ok(());
        }
        let residual = inverse(coefficients).ok_or(Error::Coefficient)?;
        let block = samples
            .get_mut(at..at + 3 * stride + 4)
            .ok_or(Error::Internal)?;
        for (row, values) in block.chunks_mut(stride).zip(residual.chunks_exact(4)) {
            for (sample, &r) in row.iter_mut().zip(values) {
                *sample = (i32::from(*sample) + ((r + 32) >> 6)).clamp(0, 255) as u8;
            }
        }
        Ok(())
    }
}

fn residual(source: &[u8], predicted: &[u8], at: usize, stride: usize) -> Result<[i32; 16]> {
    let mut out = [0; 16];
    for (j, row) in out.chunks_exact_mut(4).enumerate() {
        let start = at + j * stride;
        let s = source
            .get(start..)
            .and_then(<[u8]>::first_chunk::<4>)
            .ok_or(Error::Internal)?;
        let p = predicted
            .get(start..)
            .and_then(<[u8]>::first_chunk::<4>)
            .ok_or(Error::Internal)?;
        for ((r, &s), &p) in row.iter_mut().zip(s).zip(p) {
            *r = i32::from(s) - i32::from(p);
        }
    }
    Ok(out)
}

pub(crate) fn forward(
    source: &[u8],
    predicted: &[u8],
    at: usize,
    stride: usize,
) -> Result<Forward> {
    #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
    if let Some(simd) = crate::kernels::arithmetic() {
        return simd
            .forward(source, predicted, at, stride)
            .map(Forward)
            .ok_or(Error::Internal);
    }
    forward_scalar(source, predicted, at, stride)
}

pub(crate) fn forward_scalar(
    source: &[u8],
    predicted: &[u8],
    at: usize,
    stride: usize,
) -> Result<Forward> {
    Ok(Forward(transform::<false>(residual(
        source, predicted, at, stride,
    )?)))
}

pub(crate) fn satd(source: &[u8], predicted: &[u8], at: usize, stride: usize) -> Result<u64> {
    Ok(transform::<true>(residual(source, predicted, at, stride)?)
        .into_iter()
        .map(|c| u64::from(c.unsigned_abs()))
        .sum::<u64>()
        / 2)
}

fn line<const DC: bool>([a, b, c, d]: [i32; 4]) -> [i32; 4] {
    if DC {
        let (p, q, r, s) = (a + b, c + d, a - b, c - d);
        [p + q, p - q, r - s, r + s]
    } else {
        let (p, q, r, s) = (a + d, b + c, a - d, b - c);
        [p + q, 2 * r + s, p - q, r - 2 * s]
    }
}

fn transform<const DC: bool>(input: [i32; 16]) -> [i32; 16] {
    let [
        a0,
        a1,
        a2,
        a3,
        b0,
        b1,
        b2,
        b3,
        c0,
        c1,
        c2,
        c3,
        d0,
        d1,
        d2,
        d3,
    ] = input;
    let [a0, a1, a2, a3] = line::<DC>([a0, a1, a2, a3]);
    let [b0, b1, b2, b3] = line::<DC>([b0, b1, b2, b3]);
    let [c0, c1, c2, c3] = line::<DC>([c0, c1, c2, c3]);
    let [d0, d1, d2, d3] = line::<DC>([d0, d1, d2, d3]);
    let [a0, b0, c0, d0] = line::<DC>([a0, b0, c0, d0]);
    let [a1, b1, c1, d1] = line::<DC>([a1, b1, c1, d1]);
    let [a2, b2, c2, d2] = line::<DC>([a2, b2, c2, d2]);
    let [a3, b3, c3, d3] = line::<DC>([a3, b3, c3, d3]);
    [
        a0, a1, a2, a3, b0, b1, b2, b3, c0, c1, c2, c3, d0, d1, d2, d3,
    ]
}

fn fits(v: i32) -> bool {
    (-32768..=32767).contains(&v)
}

fn inverse_line([a, b, c, d]: [i32; 4]) -> ([i32; 4], bool) {
    let (p, q, r, s) = (a + c, a - c, (b >> 1) - d, b + (d >> 1));
    let out = [p + s, q + r, q - r, p - s];
    (
        out,
        fits(p) && fits(q) && fits(r) && fits(s) && out.into_iter().all(fits),
    )
}

fn inverse(coefficients: [i32; 16]) -> Option<[i32; 16]> {
    #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
    if let Some(simd) = crate::kernels::arithmetic() {
        return simd.inverse(&coefficients);
    }
    inverse_scalar(coefficients)
}

pub(crate) fn inverse_scalar(coefficients: [i32; 16]) -> Option<[i32; 16]> {
    if let [dc, rest @ ..] = coefficients
        && rest.iter().all(|&c| c == 0)
    {
        return (fits(dc) && dc <= 32735).then_some([dc; 16]);
    }
    let [
        a0,
        a1,
        a2,
        a3,
        b0,
        b1,
        b2,
        b3,
        c0,
        c1,
        c2,
        c3,
        d0,
        d1,
        d2,
        d3,
    ] = coefficients;
    let ([a0, a1, a2, a3], ra) = inverse_line([a0, a1, a2, a3]);
    let ([b0, b1, b2, b3], rb) = inverse_line([b0, b1, b2, b3]);
    let ([c0, c1, c2, c3], rc) = inverse_line([c0, c1, c2, c3]);
    let ([d0, d1, d2, d3], rd) = inverse_line([d0, d1, d2, d3]);
    let ([a0, b0, c0, d0], r0) = inverse_line([a0, b0, c0, d0]);
    let ([a1, b1, c1, d1], r1) = inverse_line([a1, b1, c1, d1]);
    let ([a2, b2, c2, d2], r2) = inverse_line([a2, b2, c2, d2]);
    let ([a3, b3, c3, d3], r3) = inverse_line([a3, b3, c3, d3]);
    let out = [
        a0, a1, a2, a3, b0, b1, b2, b3, c0, c1, c2, c3, d0, d1, d2, d3,
    ];
    (ra && rb && rc && rd && r0 && r1 && r2 && r3 && out.iter().all(|&x| x <= 32735)).then_some(out)
}

/// Tests only inter-predicted chroma DC, as used by the quiet-skip path.
pub(crate) fn chroma_dc_quantizes_to_zero(sums: [i32; 4], qp: u8) -> Result<bool> {
    let [a, b, c, d] = sums.map(i64::from);
    let step = norm(qp, 0)? * (1i64 << (qp / 6));
    let limit = step - 1 - (step * CHROMA_INTER_OFFSET as i64 / OFFSET_DENOMINATOR as i64);
    Ok([a + b + c + d, a - b + c - d, a + b - c - d, a - b - c + d]
        .iter()
        .all(|v| 2 * v.abs() <= limit))
}

pub(crate) fn luma_dc(sums: [i32; 16], qp: u8) -> Result<([i32; 16], [i32; 16])> {
    let levels = luma_dc_levels(sums, qp, INTRA_OFFSET)?;
    Ok((levels, luma_dc_scaled(levels, qp)?))
}

pub(crate) fn luma_dc_nearest(sums: [i32; 16], qp: u8) -> Result<[i32; 16]> {
    luma_dc_levels(sums, qp, OFFSET_DENOMINATOR / 2)
}

fn luma_dc_levels(sums: [i32; 16], qp: u8, offset: u64) -> Result<[i32; 16]> {
    let step = norm(qp, 0)? * (1i64 << (qp / 6));
    let mut levels = [0i32; 16];
    for (out, v) in levels.iter_mut().zip(transform::<true>(sums)) {
        *out = rounded(i64::from(v), step, offset)?;
    }
    Ok(levels)
}

pub(crate) fn luma_dc_scaled(levels: [i32; 16], qp: u8) -> Result<[i32; 16]> {
    if !levels.iter().all(|&level| fits(level)) {
        return Err(Error::Coefficient);
    }
    let step = norm(qp, 0)? * (1i64 << (qp / 6));
    let mut dc = [0i32; 16];
    for (out, v) in dc.iter_mut().zip(transform::<true>(levels)) {
        let v = i64::from(range(i64::from(v))?);
        *out = range((v * step + 2) >> 2)?;
    }
    Ok(dc)
}

#[cfg(test)]
pub(crate) fn chroma_dc(sums: [i32; 4], qp: u8) -> Result<([i32; 4], [i32; 4])> {
    chroma_dc_offset(sums, qp, CHROMA_INTER_OFFSET)
}

fn chroma_dc_offset(sums: [i32; 4], qp: u8, offset: u64) -> Result<([i32; 4], [i32; 4])> {
    let levels = chroma_dc_levels(sums, qp, offset)?;
    Ok((levels, chroma_dc_scaled(levels, qp)?))
}

pub(crate) fn chroma_dc_nearest(sums: [i32; 4], qp: u8) -> Result<[i32; 4]> {
    chroma_dc_levels(sums, qp, OFFSET_DENOMINATOR / 2)
}

fn hadamard2([a, b, c, d]: [i64; 4]) -> [i64; 4] {
    [a + b + c + d, a - b + c - d, a + b - c - d, a - b - c + d]
}

fn chroma_dc_levels(sums: [i32; 4], qp: u8, offset: u64) -> Result<[i32; 4]> {
    let mut levels = [0i32; 4];
    for (out, v) in levels.iter_mut().zip(hadamard2(sums.map(i64::from))) {
        *out = rounded(2 * v, norm(qp, 0)? * (1i64 << (qp / 6)), offset)?;
    }
    Ok(levels)
}

pub(crate) fn chroma_dc_scaled(levels: [i32; 4], qp: u8) -> Result<[i32; 4]> {
    let mut dc = [0i32; 4];
    for (out, v) in dc.iter_mut().zip(hadamard2(levels.map(i64::from))) {
        range(v)?;
        *out = range((v * norm(qp, 0)? * (1i64 << (qp / 6))) >> 1)?;
    }
    Ok(dc)
}

fn range(v: i64) -> Result<i32> {
    if !(-32768..=32767).contains(&v) {
        return Err(Error::Coefficient);
    }
    Ok(v as i32)
}

pub(crate) fn scan(levels: &[i32; 16]) -> Result<[i32; 16]> {
    let mut out = [0; 16];
    for (dst, &p) in out.iter_mut().zip(&SCAN) {
        *dst = *levels.get(p).ok_or(Error::Internal)?;
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;
    use coreav_kernels_h264::{BlockMut, Kernels, Scalar};

    #[test]
    fn quantizer_tables_fit_the_vector_multiply_contract() -> Result<()> {
        for qp in 0..=51 {
            let quant = Quant::of(qp)?;
            for ((&bias, &reciprocal), &zero) in
                quant.bias.iter().zip(&quant.reciprocal).zip(&quant.zero)
            {
                assert!(bias + 128 * 9180 <= u64::from(u32::MAX));
                assert!(reciprocal <= u64::from(u32::MAX));
                assert!(
                    u128::from(bias + 128 * 9180) * u128::from(reciprocal) <= u128::from(u64::MAX)
                );
                assert!(zero <= i32::MAX as u32);
            }
        }
        Ok(())
    }

    #[test]
    fn vector_forward_matches_scalar_with_exact_extents_and_overlapping_rows() -> Result<()> {
        for stride in 0..32 {
            for at in 0..32 {
                let extent = at + 3 * stride + 4;
                let a: Vec<_> = (0..extent).map(|i| (i * 171 + 37) as u8).collect();
                let b: Vec<_> = (0..extent).map(|i| (i * 59 + 213) as u8).collect();
                assert_eq!(
                    forward(&a, &b, at, stride)?.coefficients(),
                    forward_scalar(&a, &b, at, stride)?.coefficients()
                );
                let short = a.get(..extent - 1).ok_or(Error::Internal)?;
                assert_eq!(forward(short, &b, at, stride).err(), Some(Error::Internal));
                let short = b.get(..extent - 1).ok_or(Error::Internal)?;
                assert_eq!(forward(&a, short, at, stride).err(), Some(Error::Internal));
            }
        }
        Ok(())
    }

    #[test]
    fn vector_inverse_matches_scalar_at_signed_intermediate_boundaries() {
        let mut seed = 0x56cadf31e974b280u64;
        for bound in [1, 255, 1023, 4095, 8191, 16383, 32767] {
            for _ in 0..16384 {
                let coefficients = std::array::from_fn(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    (seed % (2 * bound + 1) as u64) as i32 - bound
                });
                assert_eq!(inverse(coefficients), inverse_scalar(coefficients));
            }
        }
        for dc in -32768..=32767 {
            let mut coefficients = [0; 16];
            if let Some(first) = coefficients.first_mut() {
                *first = dc;
            }
            assert_eq!(inverse(coefficients), inverse_scalar(coefficients));
        }
    }

    const BASIS: [[i64; 4]; 4] = [[1, 1, 1, 1], [2, 1, -1, -2], [1, -1, -1, 1], [1, -2, 2, -1]];
    const HADAMARD: [[i64; 4]; 4] = [[1, 1, 1, 1], [1, 1, -1, -1], [1, -1, -1, 1], [1, -1, 1, -1]];
    const AT: usize = 3;
    const STRIDE: usize = 9;
    const LEN: usize = AT + 3 * STRIDE + 6;
    const MAX_COEFFICIENT: i32 = 36 * 255;

    /// The 64-bit, per-call checked path the fused kernel replaced.
    mod reference {
        use super::super::{CHROMA_INTER_OFFSET, Error, INTRA_OFFSET, Result, norm, range};

        pub(super) fn round(n: i64, d: i64, offset: u64) -> Result<i32> {
            let magnitude = (12 * n.abs() + d * offset as i64) / (12 * d);
            i32::try_from(if n < 0 { -magnitude } else { magnitude })
                .map_err(|_| Error::Coefficient)
        }

        fn line<const DC: bool>([a, b, c, d]: [i64; 4]) -> [i64; 4] {
            if DC {
                let (p, q, r, s) = (a + b, c + d, a - b, c - d);
                [p + q, p - q, r - s, r + s]
            } else {
                let (p, q, r, s) = (a + d, b + c, a - d, b - c);
                [p + q, 2 * r + s, p - q, r - 2 * s]
            }
        }

        pub(super) fn matrix<const DC: bool>(input: [i64; 16]) -> [i64; 16] {
            let mut horizontal = [0; 16];
            for (src, dst) in input.chunks_exact(4).zip(horizontal.chunks_exact_mut(4)) {
                dst.copy_from_slice(&line::<DC>(src.try_into().unwrap()));
            }
            let mut out = [0; 16];
            for col in 0..4 {
                let column = [0, 1, 2, 3].map(|row| horizontal[row * 4 + col]);
                for (row, v) in line::<DC>(column).into_iter().enumerate() {
                    out[row * 4 + col] = v;
                }
            }
            out
        }

        pub(super) fn quantize(
            transformed: &[i64; 16],
            qp: u8,
            separate_dc: bool,
            offset: u64,
        ) -> Result<[i32; 16]> {
            let mut out = [0; 16];
            for (p, dst) in out.iter_mut().enumerate().skip(usize::from(separate_dc)) {
                let weight =
                    (if p / 4 % 2 == 0 { 4 } else { 5 }) * (if p % 2 == 0 { 4 } else { 5 });
                *dst = round(
                    64 * transformed[p],
                    weight * norm(qp, p)? * (1i64 << (qp / 6)),
                    offset,
                )?;
            }
            Ok(out)
        }

        pub(super) fn quantizes_to_zero(
            transformed: &[i64; 16],
            qp: u8,
            separate_dc: bool,
            offset: u64,
        ) -> bool {
            transformed
                .iter()
                .enumerate()
                .skip(usize::from(separate_dc))
                .all(|(p, c)| {
                    let weight =
                        (if p / 4 % 2 == 0 { 4 } else { 5 }) * (if p % 2 == 0 { 4 } else { 5 });
                    round(
                        64 * c,
                        weight * norm(qp, p).unwrap() * (1i64 << (qp / 6)),
                        offset,
                    )
                    .unwrap()
                        == 0
                })
        }

        pub(super) fn scaled(levels: &[i32; 16], qp: u8, dc: Option<i32>) -> Result<[i32; 16]> {
            let mut out = [0; 16];
            if levels.iter().any(|&level| level != 0) {
                for (p, dst) in out.iter_mut().enumerate() {
                    *dst = range(i64::from(levels[p]) * norm(qp, p)? * (1i64 << (qp / 6)))?;
                }
            }
            if let Some(dc) = dc {
                out[0] = dc;
            }
            if out.iter().any(|&coefficient| coefficient != 0) {
                check_inverse(&out)?;
            }
            Ok(out)
        }

        fn inverse_line([a, b, c, d]: [i32; 4]) -> Result<[i32; 4]> {
            let v = [a + c, a - c, (b >> 1) - d, b + (d >> 1)];
            for &x in &v {
                range(i64::from(x))?;
            }
            let [p, q, r, s] = v;
            let out = [p + s, q + r, q - r, p - s];
            for &x in &out {
                range(i64::from(x))?;
            }
            Ok(out)
        }

        fn check_inverse(coefficients: &[i32; 16]) -> Result<()> {
            let mut horizontal = [0i32; 16];
            for (input, output) in coefficients
                .chunks_exact(4)
                .zip(horizontal.chunks_exact_mut(4))
            {
                output.copy_from_slice(&inverse_line(input.try_into().unwrap())?);
            }
            for col in 0..4 {
                for x in inverse_line([0, 1, 2, 3].map(|r| horizontal[r * 4 + col]))? {
                    if x > 32735 {
                        return Err(Error::Coefficient);
                    }
                }
            }
            Ok(())
        }

        pub(super) fn luma_dc(sums: [i64; 16], qp: u8) -> Result<([i32; 16], [i32; 16])> {
            let transformed = matrix::<true>(sums);
            let mut levels = [0i32; 16];
            for (out, v) in levels.iter_mut().zip(transformed) {
                *out = round(v, norm(qp, 0)? * (1i64 << (qp / 6)), INTRA_OFFSET)?;
            }
            let inv = matrix::<true>(levels.map(i64::from));
            let mut dc = [0i32; 16];
            for (out, v) in dc.iter_mut().zip(inv) {
                range(v)?;
                let n = v * norm(qp, 0)? * (1i64 << (qp / 6));
                *out = range((n + 2) >> 2)?;
            }
            Ok((levels, dc))
        }

        pub(super) fn chroma_dc(sums: [i64; 4], qp: u8) -> Result<([i32; 4], [i32; 4])> {
            chroma_dc_with_offset(sums, qp, CHROMA_INTER_OFFSET)
        }

        pub(super) fn chroma_dc_with_offset(
            sums: [i64; 4],
            qp: u8,
            offset: u64,
        ) -> Result<([i32; 4], [i32; 4])> {
            fn h([a, b, c, d]: [i64; 4]) -> [i64; 4] {
                [a + b + c + d, a - b + c - d, a + b - c - d, a - b - c + d]
            }
            let mut levels = [0i32; 4];
            for (out, v) in levels.iter_mut().zip(h(sums)) {
                *out = round(2 * v, norm(qp, 0)? * (1i64 << (qp / 6)), offset)?;
            }
            let mut dc = [0i32; 4];
            for (out, v) in dc.iter_mut().zip(h(levels.map(i64::from))) {
                range(v)?;
                *out = range((v * norm(qp, 0)? * (1i64 << (qp / 6))) >> 1)?;
            }
            Ok((levels, dc))
        }
    }

    fn generic(input: [i64; 16], basis: &[[i64; 4]; 4]) -> [i64; 16] {
        std::array::from_fn(|p| {
            input
                .iter()
                .enumerate()
                .map(|(k, sample)| basis[p / 4][k / 4] * sample * basis[p % 4][k % 4])
                .sum()
        })
    }

    fn place(block: [u8; 16]) -> Vec<u8> {
        let mut out = vec![0xa5; LEN];
        for (j, row) in block.chunks_exact(4).enumerate() {
            out[AT + j * STRIDE..AT + j * STRIDE + 4].copy_from_slice(row);
        }
        out
    }

    /// Source and prediction pairs: equal blocks, flat extremes, every basis
    /// pattern at full swing in both signs, single-sample impulses and noise.
    fn pairs() -> Vec<([u8; 16], [u8; 16])> {
        let mut out = vec![
            ([0; 16], [0; 16]),
            ([255; 16], [255; 16]),
            ([255; 16], [0; 16]),
            ([0; 16], [255; 16]),
        ];
        for basis in [BASIS, HADAMARD] {
            for u in 0..4 {
                for v in 0..4 {
                    let high: [u8; 16] = std::array::from_fn(|k| {
                        if basis[u][k / 4] * basis[v][k % 4] > 0 {
                            255
                        } else {
                            0
                        }
                    });
                    out.push((high, high.map(|s| 255 - s)));
                    out.push((high.map(|s| 255 - s), high));
                }
            }
        }
        for p in 0..16 {
            let mut impulse = [0; 16];
            impulse[p] = 255;
            out.push((impulse, [0; 16]));
            out.push(([0; 16], impulse));
        }
        let mut seed = 0x2545f4914f6cdd1du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        };
        for small in [false, true] {
            for _ in 0..128 {
                let source: [u8; 16] = std::array::from_fn(|_| next());
                let prediction: [u8; 16] = std::array::from_fn(|k| {
                    if small {
                        source[k].saturating_add(next() % 9).saturating_sub(4)
                    } else {
                        next()
                    }
                });
                out.push((source, prediction));
            }
        }
        out
    }

    fn difference(source: &[u8; 16], prediction: &[u8; 16]) -> [i64; 16] {
        std::array::from_fn(|k| i64::from(source[k]) - i64::from(prediction[k]))
    }

    #[test]
    fn forward_and_satd_match_the_reference_for_every_pattern() {
        for (source, prediction) in pairs() {
            let residual = difference(&source, &prediction);
            let forward = forward(&place(source), &place(prediction), AT, STRIDE).unwrap();
            assert_eq!(forward.0.map(i64::from), generic(residual, &BASIS));
            assert_eq!(forward.dc(), forward.0[0]);
            assert_eq!(
                satd(&place(source), &place(prediction), AT, STRIDE).unwrap(),
                generic(residual, &HADAMARD)
                    .into_iter()
                    .map(i64::unsigned_abs)
                    .sum::<u64>()
                    / 2
            );
        }
        let short = vec![0; AT + 3 * STRIDE + 3];
        assert_eq!(
            forward(&short, &place([0; 16]), AT, STRIDE).err(),
            Some(Error::Internal)
        );
        assert_eq!(
            satd(&place([0; 16]), &short, AT, STRIDE).err(),
            Some(Error::Internal)
        );
    }

    #[test]
    fn offset_kernels_match_division_and_zero_decisions_for_both_prediction_types() {
        for qp in 0..=51 {
            for offset in [0, 2, 3, 4, 5, 6] {
                let quant = Quant::new(qp, offset);
                for coefficient in -MAX_COEFFICIENT..=MAX_COEFFICIENT {
                    let input = Forward([coefficient; 16]);
                    for separate_dc in [false, true] {
                        let expected =
                            reference::quantize(&input.0.map(i64::from), qp, separate_dc, offset)
                                .unwrap();
                        assert_eq!(quant.levels(&input, separate_dc), expected);
                        assert_eq!(
                            quant.zero(&input, separate_dc),
                            expected.into_iter().all(|level| level == 0),
                        );
                    }
                }
            }
            for quant in [Quant::of(qp).unwrap(), Quant::intra(qp).unwrap()] {
                for numerator in [-130_560, -1, 0, 1, 130_560] {
                    let divisor = norm(qp, 0).unwrap() << (qp / 6);
                    assert_eq!(
                        rounded(numerator, divisor, quant.offset),
                        reference::round(numerator, divisor, quant.offset),
                    );
                }
            }
        }
    }

    #[test]
    fn intra_chroma_dc_and_prediction_offsets_match_direct_division() {
        let mut split_witnesses = [false; 52];
        for qp in 0..=51 {
            assert!((0..=MAX_COEFFICIENT).any(|value| {
                let block = Forward([value; 16]);
                Quant::intra(qp).unwrap().levels(&block, false)
                    != Quant::of(qp).unwrap().levels(&block, false)
            }));
            for value in -4080..=4080 {
                for sums in [[value, 0, 0, 0], [value, -value, value, -value]] {
                    let intra = Quant::chroma_intra(qp).unwrap().chroma_dc(sums, qp);
                    let inter = Quant::chroma_inter(qp).unwrap().chroma_dc(sums, qp);
                    assert_eq!(
                        intra,
                        reference::chroma_dc_with_offset(
                            sums.map(i64::from),
                            qp,
                            CHROMA_INTRA_OFFSET
                        ),
                        "intra QP {qp}, {sums:?}",
                    );
                    assert_eq!(
                        inter,
                        reference::chroma_dc_with_offset(
                            sums.map(i64::from),
                            qp,
                            CHROMA_INTER_OFFSET
                        ),
                        "inter QP {qp}, {sums:?}",
                    );
                    if let (Ok((intra_levels, _)), Ok((inter_levels, _))) = (intra, inter) {
                        split_witnesses[usize::from(qp)] |= intra_levels != inter_levels;
                        assert_eq!(
                            chroma_dc_quantizes_to_zero(sums, qp).unwrap(),
                            inter_levels.into_iter().all(|level| level == 0),
                        );
                    }
                }
            }
        }
        assert!(
            split_witnesses
                .into_iter()
                .all(|found| found == (CHROMA_INTRA_OFFSET != CHROMA_INTER_OFFSET))
        );
    }

    #[test]
    fn nearest_targets_match_direct_half_bias_and_dc_scaling_rejects_extreme_levels() {
        for qp in 0..=51 {
            for c in -9180..=9180 {
                let forward = Forward([c; 16]);
                for separate_dc in [false, true] {
                    assert_eq!(
                        std::array::from_fn(|p| if separate_dc && p == 0 {
                            0
                        } else {
                            Quant::of(qp).unwrap().nearest_level(&forward, p).unwrap()
                        }),
                        reference::quantize(&[i64::from(c); 16], qp, separate_dc, 6).unwrap(),
                        "{qp} {c} {separate_dc}"
                    );
                }
            }
            for sums in [
                [0; 16],
                [4080; 16],
                [-4080; 16],
                std::array::from_fn(|p| p as i32 * 511 - 4080),
            ] {
                let (levels, dc) = luma_dc(sums, qp).unwrap();
                assert_eq!(luma_dc_scaled(levels, qp), Ok(dc));
            }
            for sums in [[0; 4], [4080; 4], [-4080; 4], [4080, -4080, 4080, -4080]] {
                let (levels, dc) = chroma_dc(sums, qp).unwrap();
                assert_eq!(chroma_dc_scaled(levels, qp), Ok(dc));
            }
        }
        assert_eq!(luma_dc_scaled([i32::MAX; 16], 28), Err(Error::Coefficient));
        assert_eq!(luma_dc_scaled([i32::MIN; 16], 28), Err(Error::Coefficient));
        assert_eq!(chroma_dc_scaled([i32::MAX; 4], 28), Err(Error::Coefficient));
        assert_eq!(
            Quant::of(28).unwrap().nearest_level(&Forward([0; 16]), 16),
            Err(Error::Internal)
        );
    }

    #[test]
    fn levels_and_zero_match_exact_division_for_every_coefficient_and_qp() {
        for qp in 0..=51u8 {
            let quant = Quant::of(qp).unwrap();
            for c in -MAX_COEFFICIENT..=MAX_COEFFICIENT {
                let block = Forward(std::array::from_fn(|p| if p % 3 == 0 { -c } else { c }));
                let wide = block.0.map(i64::from);
                for separate_dc in [false, true] {
                    assert_eq!(
                        quant.levels(&block, separate_dc),
                        reference::quantize(&wide, qp, separate_dc, quant.offset).unwrap(),
                        "{qp} {c} {separate_dc}"
                    );
                    assert_eq!(
                        quant.zero(&block, separate_dc),
                        reference::quantizes_to_zero(&wide, qp, separate_dc, quant.offset),
                        "{qp} {c} {separate_dc}"
                    );
                }
            }
        }
        assert!(Quant::of(52).is_err());
    }

    #[test]
    fn fused_reconstruction_matches_the_reference_including_refusals() {
        let mut outcomes = [[0usize; 2]; 52];
        for (source, prediction) in pairs() {
            let residual = difference(&source, &prediction);
            for qp in 0..=51u8 {
                let quant = Quant::of(qp).unwrap();
                let coefficients = forward(&place(source), &place(prediction), AT, STRIDE).unwrap();
                for separate_dc in [false, true] {
                    let levels = quant.levels(&coefficients, separate_dc);
                    let expected = reference::quantize(
                        &reference::matrix::<false>(residual),
                        qp,
                        separate_dc,
                        quant.offset,
                    )
                    .unwrap();
                    assert_eq!(levels, expected);
                    let dcs: &[Option<i32>] = if separate_dc {
                        &[
                            Some(0),
                            Some(-32768),
                            Some(-33),
                            Some(31),
                            Some(32),
                            Some(1000),
                            Some(32735),
                            Some(32736),
                            Some(32767),
                        ]
                    } else {
                        &[None]
                    };
                    for &dc in dcs {
                        let mut fused = place(prediction);
                        let got = quant.reconstruct(&levels, dc, &mut fused, AT, STRIDE);
                        let mut old = place(prediction);
                        let want = reference::scaled(&expected, qp, dc).map(|coefficients| {
                            Scalar.idct4x4_add(
                                BlockMut::new(&mut old, AT, STRIDE, 4, 4).unwrap(),
                                &coefficients,
                            );
                        });
                        assert_eq!(
                            got, want,
                            "{qp} {separate_dc} {dc:?} {source:?} {prediction:?}"
                        );
                        assert_eq!(
                            fused, old,
                            "{qp} {separate_dc} {dc:?} {source:?} {prediction:?}"
                        );
                        outcomes[usize::from(qp)][usize::from(got.is_err())] += 1;
                    }
                }
            }
        }
        assert!(outcomes[0][1] > 0 && outcomes[51][1] > 0);
        assert!(outcomes.iter().all(|[accepted, _]| *accepted > 0));
        let mut short = vec![0; AT + 3 * STRIDE + 3];
        assert_eq!(
            Quant::of(28)
                .unwrap()
                .reconstruct(&[1; 16], None, &mut short, AT, STRIDE),
            Err(Error::Internal)
        );
    }

    #[test]
    fn dc_transforms_match_the_wider_reference_at_every_qp() {
        let extremes = [-4080, -4079, -2040, -1, 0, 1, 2039, 4079, 4080];
        let mut seed = 0x9e3779b97f4a7c15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 8161) as i32 - 4080
        };
        let mut luma: Vec<[i32; 16]> = extremes.iter().map(|&v| [v; 16]).collect();
        luma.extend(
            extremes
                .iter()
                .map(|&v| std::array::from_fn(|k| if k % 2 == 0 { v } else { -v })),
        );
        luma.extend((0..256).map(|_| std::array::from_fn(|_| next())));
        let mut chroma: Vec<[i32; 4]> = Vec::new();
        for a in extremes {
            for b in extremes {
                chroma.push([a, b, -a, b]);
                chroma.push([a, a, b, b]);
            }
        }
        for qp in 0..=51u8 {
            for sums in &luma {
                assert_eq!(
                    luma_dc(*sums, qp),
                    reference::luma_dc(sums.map(i64::from), qp),
                    "{qp} {sums:?}"
                );
            }
            for sums in &chroma {
                assert_eq!(
                    chroma_dc(*sums, qp),
                    reference::chroma_dc(sums.map(i64::from), qp),
                    "{qp} {sums:?}"
                );
            }
        }
    }

    #[test]
    fn transforms_match_the_direct_matrices_and_dc_quantizer() {
        for position in 0..16 {
            for value in [-65280, -32768, -255, -1, 0, 1, 255, 32767, 65280] {
                let mut input = [0; 16];
                input[position] = value;
                assert_eq!(
                    transform::<false>(input).map(i64::from),
                    generic(input.map(i64::from), &BASIS)
                );
                assert_eq!(
                    transform::<true>(input).map(i64::from),
                    generic(input.map(i64::from), &HADAMARD)
                );
            }
        }
        for seed in 0..128i32 {
            let input = std::array::from_fn(|k| {
                (seed * 73 + k as i32 * 131 + k as i32 * seed * 17) % 511 - 255
            });
            assert_eq!(
                transform::<false>(input).map(i64::from),
                generic(input.map(i64::from), &BASIS)
            );
            assert_eq!(
                transform::<true>(input).map(i64::from),
                generic(input.map(i64::from), &HADAMARD)
            );
            for qp in 10..=51 {
                let transformed = generic(input.map(i64::from), &HADAMARD);
                let levels = transformed.map(|v| {
                    reference::round(v, norm(qp, 0).unwrap() * (1i64 << (qp / 6)), INTRA_OFFSET)
                        .unwrap()
                });
                let inv = generic(levels.map(i64::from), &HADAMARD);
                let expected = (|| {
                    let mut dc = [0; 16];
                    for (out, v) in dc.iter_mut().zip(inv) {
                        range(v)?;
                        *out = range((v * norm(qp, 0)? * (1i64 << (qp / 6)) + 2) >> 2)?;
                    }
                    Ok((levels, dc))
                })();
                assert_eq!(luma_dc(input, qp), expected);
            }
        }
    }

    #[test]
    fn zero_checks_match_the_quantizers_at_every_rounding_boundary() {
        for qp in 0..=51u8 {
            let quant = Quant::of(qp).unwrap();
            for separate_dc in [false, true] {
                let mut blocks = Vec::new();
                for p in 0..16 {
                    let weight =
                        (if p / 4 % 2 == 0 { 4 } else { 5 }) * (if p % 2 == 0 { 4 } else { 5 });
                    let boundary =
                        (weight * norm(qp, p).unwrap() * (1i64 << (qp / 6)) / 128) as i32;
                    for value in boundary - 2..=boundary + 2 {
                        for sign in [1, -1] {
                            let mut block = [0; 16];
                            block[p] = (sign * value).clamp(-MAX_COEFFICIENT, MAX_COEFFICIENT);
                            blocks.push(block);
                        }
                    }
                }
                for seed in 0..64i32 {
                    let scale = 1 + seed % 7 * (1 << (qp / 6));
                    blocks.push(std::array::from_fn(|k| {
                        (((seed * 37 + k as i32 * 11 + k as i32 * seed) % 9 - 4) * scale)
                            .clamp(-MAX_COEFFICIENT, MAX_COEFFICIENT)
                    }));
                }
                for block in blocks {
                    let block = Forward(block);
                    assert_eq!(
                        quant.zero(&block, separate_dc),
                        quant.levels(&block, separate_dc).iter().all(|&l| l == 0),
                        "{qp} {separate_dc} {:?}",
                        block.0
                    );
                }
            }
            let boundary = (norm(qp, 0).unwrap() * (1i64 << (qp / 6)) / 4) as i32;
            for a in boundary - 2..=boundary + 2 {
                for sums in [[a, 0, 0, 0], [a, a, 0, 0], [a, -a, a, -a], [-a, 1, 2, -3]] {
                    assert_eq!(
                        chroma_dc_quantizes_to_zero(sums, qp).unwrap(),
                        chroma_dc(sums, qp).unwrap().0.iter().all(|&l| l == 0),
                        "{qp} {sums:?}"
                    );
                }
            }
        }
    }
}
