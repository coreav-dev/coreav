use crate::{Error, Result};

pub(crate) const SCAN: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
const ADJUST: [[i64; 3]; 6] = [
    [10, 16, 13],
    [11, 18, 14],
    [13, 20, 16],
    [14, 23, 18],
    [16, 25, 20],
    [18, 29, 23],
];

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

fn norm(qp: u8, position: usize) -> Result<i64> {
    let row = (position / 4) % 2;
    let col = position % 2;
    let column = if row == col { row } else { 2 };
    ADJUST
        .get(usize::from(qp % 6))
        .and_then(|r| r.get(column))
        .copied()
        .ok_or(Error::Internal)
}

fn nearest(numerator: i64, denominator: i64) -> Result<i32> {
    let a = numerator.abs();
    let level = a / denominator + i64::from((a % denominator) * 2 > denominator);
    i32::try_from(if numerator < 0 { -level } else { level }).map_err(|_| Error::Coefficient)
}

pub(crate) fn forward(samples: &[i32; 16]) -> Result<[i64; 16]> {
    matrix::<false>(samples.map(i64::from))
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

fn matrix<const DC: bool>(input: [i64; 16]) -> Result<[i64; 16]> {
    let mut horizontal = [0; 16];
    for (src, dst) in input.chunks_exact(4).zip(horizontal.chunks_exact_mut(4)) {
        dst.copy_from_slice(&line::<DC>(src.try_into().map_err(|_| Error::Internal)?));
    }
    let mut out = [0; 16];
    for col in 0..4 {
        let mut column = [0; 4];
        for (row, v) in column.iter_mut().enumerate() {
            *v = *horizontal.get(row * 4 + col).ok_or(Error::Internal)?;
        }
        for (row, v) in line::<DC>(column).into_iter().enumerate() {
            *out.get_mut(row * 4 + col).ok_or(Error::Internal)? = v;
        }
    }
    Ok(out)
}

pub(crate) fn quantize(transformed: &[i64; 16], qp: u8, separate_dc: bool) -> Result<[i32; 16]> {
    let mut out = [0; 16];
    for (p, dst) in out.iter_mut().enumerate().skip(usize::from(separate_dc)) {
        let weight = (if p / 4 % 2 == 0 { 4 } else { 5 }) * (if p % 2 == 0 { 4 } else { 5 });
        *dst = nearest(
            64 * transformed.get(p).ok_or(Error::Internal)?,
            weight * norm(qp, p)? * (1i64 << (qp / 6)),
        )?;
    }
    Ok(out)
}

pub(crate) fn scaled(levels: &[i32; 16], qp: u8, dc: Option<i32>) -> Result<[i32; 16]> {
    let mut out = [0; 16];
    for (p, dst) in out.iter_mut().enumerate() {
        let n =
            i64::from(*levels.get(p).ok_or(Error::Internal)?) * norm(qp, p)? * (1i64 << (qp / 6));
        *dst = range(n)?;
    }
    if let Some(dc) = dc {
        *out.first_mut().ok_or(Error::Internal)? = dc;
    }
    check_inverse(&out)?;
    Ok(out)
}

pub(crate) fn luma_dc(sums: [i64; 16], qp: u8) -> Result<([i32; 16], [i32; 16])> {
    let transformed = matrix::<true>(sums)?;
    let mut levels = [0i32; 16];
    for (out, v) in levels.iter_mut().zip(transformed) {
        *out = nearest(v, norm(qp, 0)? * (1i64 << (qp / 6)))?;
    }
    let inv = matrix::<true>(levels.map(i64::from))?;
    let mut dc = [0i32; 16];
    for (out, v) in dc.iter_mut().zip(inv) {
        range(v)?;
        let n = v * norm(qp, 0)? * (1i64 << (qp / 6));
        *out = range((n + 2) >> 2)?;
    }
    Ok((levels, dc))
}

pub(crate) fn chroma_dc(sums: [i64; 4], qp: u8) -> Result<([i32; 4], [i32; 4])> {
    fn h([a, b, c, d]: [i64; 4]) -> [i64; 4] {
        [a + b + c + d, a - b + c - d, a + b - c - d, a - b - c + d]
    }
    let mut levels = [0i32; 4];
    for (out, v) in levels.iter_mut().zip(h(sums)) {
        *out = nearest(2 * v, norm(qp, 0)? * (1i64 << (qp / 6)))?;
    }
    let mut dc = [0i32; 4];
    for (out, v) in dc.iter_mut().zip(h(levels.map(i64::from))) {
        range(v)?;
        *out = range((v * norm(qp, 0)? * (1i64 << (qp / 6))) >> 1)?;
    }
    Ok((levels, dc))
}

fn range(v: i64) -> Result<i32> {
    if !(-32768..=32767).contains(&v) {
        return Err(Error::Coefficient);
    }
    Ok(v as i32)
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
        let line: [i32; 4] = input.try_into().map_err(|_| Error::Internal)?;
        output.copy_from_slice(&inverse_line(line)?);
    }
    for col in 0..4 {
        let mut line = [0; 4];
        for (r, out) in line.iter_mut().enumerate() {
            *out = *horizontal.get(r * 4 + col).ok_or(Error::Internal)?;
        }
        for x in inverse_line(line)? {
            if x > 32735 {
                return Err(Error::Coefficient);
            }
        }
    }
    Ok(())
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

    const BASIS: [[i64; 4]; 4] = [[1, 1, 1, 1], [2, 1, -1, -2], [1, -1, -1, 1], [1, -2, 2, -1]];
    const HADAMARD: [[i64; 4]; 4] = [[1, 1, 1, 1], [1, 1, -1, -1], [1, -1, -1, 1], [1, -1, 1, -1]];

    fn generic(input: [i64; 16], basis: &[[i64; 4]; 4]) -> [i64; 16] {
        std::array::from_fn(|p| {
            input
                .iter()
                .enumerate()
                .map(|(k, sample)| basis[p / 4][k / 4] * sample * basis[p % 4][k % 4])
                .sum()
        })
    }

    #[test]
    fn separable_transforms_preserve_every_basis_position_and_signed_extreme() {
        for position in 0..16 {
            for value in [i32::MIN, -32768, -255, -1, 0, 1, 255, 32767, i32::MAX] {
                let mut input = [0; 16];
                input[position] = value;
                assert_eq!(
                    forward(&input).unwrap(),
                    generic(input.map(i64::from), &BASIS)
                );
                assert_eq!(
                    matrix::<true>(input.map(i64::from)).unwrap(),
                    generic(input.map(i64::from), &HADAMARD)
                );
            }
        }
        for seed in 0..128i64 {
            let input = std::array::from_fn(|k| {
                (seed * 73 + k as i64 * 131 + k as i64 * seed * 17) % 511 - 255
            });
            assert_eq!(matrix::<false>(input).unwrap(), generic(input, &BASIS));
            assert_eq!(matrix::<true>(input).unwrap(), generic(input, &HADAMARD));
            for qp in 10..=51 {
                let transformed = generic(input, &HADAMARD);
                let levels = transformed
                    .map(|v| nearest(v, norm(qp, 0).unwrap() * (1i64 << (qp / 6))).unwrap());
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
}
