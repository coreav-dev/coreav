use crate::{BitDepth, BlockMut, Component, Error, IntraMode};

/// Main/Main10 intra prediction controls, after syntax and chroma derivation.
#[derive(Clone, Copy, Debug)]
pub struct IntraPrediction {
    /// Sample depth of the component.
    pub depth: BitDepth,
    /// Luma or 4:2:0 chroma filtering rules.
    pub component: Component,
    /// Resolved planar, DC or angular prediction mode.
    pub mode: IntraMode,
    /// The SPS `strong_intra_smoothing_enabled_flag`.
    pub strong_smoothing: bool,
}

fn get(samples: &[i32], index: usize) -> Result<i32, Error> {
    samples.get(index).copied().ok_or(Error::Buffer)
}

fn angle(mode: u8) -> i32 {
    match mode {
        2 | 34 => 32,
        3 | 33 => 26,
        4 | 32 => 21,
        5 | 31 => 17,
        6 | 30 => 13,
        7 | 29 => 9,
        8 | 28 => 5,
        9 | 27 => 2,
        11 | 25 => -2,
        12 | 24 => -5,
        13 | 23 => -9,
        14 | 22 => -13,
        15 | 21 => -17,
        16 | 20 => -21,
        17 | 19 => -26,
        18 => -32,
        _ => 0,
    }
}

fn inv_angle(mode: u8) -> i32 {
    match mode {
        11 | 25 => -4096,
        12 | 24 => -1638,
        13 | 23 => -910,
        14 | 22 => -630,
        15 | 21 => -482,
        16 | 20 => -390,
        17 | 19 => -315,
        18 => -256,
        _ => 0,
    }
}

fn angular_ref(p: &[i32], corner: i32, mode: u8, index: i32) -> Result<i32, Error> {
    let distance = if index < 0 {
        -((index * inv_angle(mode) + 128) >> 8)
    } else {
        index
    };
    let offset = if mode >= 18 {
        corner + distance
    } else {
        corner - distance
    };
    get(p, usize::try_from(offset).map_err(|_| Error::Buffer)?)
}

/// Predicts a square block using H.265 8.4.4.2, including substitution and filtering.
///
/// The first `4*N+1` references are ordered from `p[-1][2*N-1]` up the left
/// edge to `p[-1][-1]`, then right to `p[2*N-1][-1]`. `None` means unavailable
/// according to the caller's coding-order, slice, tile and constrained-intra rules.
/// Extra references are ignored. Available samples must fit `depth`.
/// Output padding is unchanged. Invalid inputs are rejected before output writes.
pub fn predict_intra(
    config: IntraPrediction,
    references: &[Option<u16>],
    mut output: BlockMut<'_, u16>,
) -> Result<(), Error> {
    let n = output.size.side();
    let count = 4 * n + 1;
    let references = references.get(..count).ok_or(Error::Buffer)?;
    if references
        .iter()
        .flatten()
        .any(|&v| v > config.depth.max_sample())
    {
        return Err(Error::Sample);
    }
    let mut p = [0_i32; crate::REFERENCE_SAMPLES];
    let mut previous = references
        .iter()
        .flatten()
        .next()
        .copied()
        .unwrap_or(1 << (config.depth.bits() - 1));
    for (reference, sample) in references.iter().zip(p.iter_mut()) {
        previous = reference.unwrap_or(previous);
        *sample = i32::from(previous);
    }
    let c = 2 * n;
    let mode = config.mode.number();
    let luma = config.component == Component::Luma;
    let distance = i32::from(mode)
        .abs_diff(26)
        .min(i32::from(mode).abs_diff(10));
    let threshold = match n {
        8 => 7,
        16 => 1,
        _ => 0,
    };
    if luma && mode != 1 && n != 4 && distance > threshold {
        let original = p;
        let corner = get(&original, c)?;
        let strong = config.strong_smoothing
            && n == 32
            && (corner + get(&original, 4 * n)? - 2 * get(&original, c + n)?).abs()
                < (1 << (config.depth.bits() - 5))
            && (corner + get(&original, 0)? - 2 * get(&original, n)?).abs()
                < (1 << (config.depth.bits() - 5));
        for i in 1..4 * n {
            let value = if strong {
                let (distance, endpoint) = if i < c {
                    (c - i, get(&original, 0)?)
                } else {
                    (i - c, get(&original, 4 * n)?)
                };
                ((64 - distance as i32) * corner + distance as i32 * endpoint + 32) >> 6
            } else {
                (get(&original, i - 1)? + 2 * get(&original, i)? + get(&original, i + 1)? + 2) >> 2
            };
            *p.get_mut(i).ok_or(Error::Buffer)? = value;
        }
    }
    let mut sum = n as i32;
    for i in 0..n {
        sum += get(&p, c - 1 - i)? + get(&p, c + 1 + i)?;
    }
    let dc = sum >> (output.size.log2() + 1);
    for y in 0..n {
        for x in 0..n {
            let top = get(&p, c + 1 + x)?;
            let left = get(&p, c - 1 - y)?;
            let value = match mode {
                0 => {
                    ((n - 1 - x) as i32 * left
                        + (x + 1) as i32 * get(&p, c + n + 1)?
                        + (n - 1 - y) as i32 * top
                        + (y + 1) as i32 * get(&p, c - n - 1)?
                        + n as i32)
                        >> (output.size.log2() + 1)
                }
                1 if luma && n < 32 && x == 0 && y == 0 => (left + 2 * dc + top + 2) >> 2,
                1 if luma && n < 32 && y == 0 => (top + 3 * dc + 2) >> 2,
                1 if luma && n < 32 && x == 0 => (left + 3 * dc + 2) >> 2,
                1 => dc,
                26 if luma && n < 32 && x == 0 => top + ((left - get(&p, c)?) >> 1),
                10 if luma && n < 32 && y == 0 => left + ((top - get(&p, c)?) >> 1),
                _ => {
                    let (position, along) = if mode >= 18 { (y, x) } else { (x, y) };
                    let delta = (position as i32 + 1) * angle(mode);
                    let index = along as i32 + (delta >> 5) + 1;
                    let fraction = delta & 31;
                    let first = angular_ref(&p, c as i32, mode, index)?;
                    if fraction == 0 {
                        first
                    } else {
                        ((32 - fraction) * first
                            + fraction * angular_ref(&p, c as i32, mode, index + 1)?
                            + 16)
                            >> 5
                    }
                }
            };
            output.put(x, y, config.depth.clip(i64::from(value)))?;
        }
    }
    Ok(())
}
