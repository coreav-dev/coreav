//! Colour description (ITU-T H.273 code points) and the Y'CbCr to R'G'B'
//! matrices.

/// Where chroma samples sit relative to luma samples (ITU-T H.273
/// `chroma_sample_loc_type`, the same numbering).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChromaLocation {
    /// 0: horizontally with the left luma sample, vertically between rows
    /// (MPEG-2, H.264 and H.265's default for 4:2:0).
    Left,
    /// 1: centred between luma samples in both directions (JPEG).
    Center,
    /// 2: with the top-left luma sample.
    TopLeft,
    /// 3: horizontally centred, with the top row.
    Top,
    /// 4: with the bottom-left luma sample.
    BottomLeft,
    /// 5: horizontally centred, with the bottom row.
    Bottom,
}

impl ChromaLocation {
    /// The location for an H.273 `chroma_sample_loc_type`.
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Left,
            1 => Self::Center,
            2 => Self::TopLeft,
            3 => Self::Top,
            4 => Self::BottomLeft,
            5 => Self::Bottom,
            _ => return None,
        })
    }

    /// Position of chroma sample 0 in luma sample units, horizontally and
    /// vertically, for a subsampled direction.
    pub(crate) fn offsets(self) -> (f64, f64) {
        match self {
            Self::Left => (0.0, 0.5),
            Self::Center => (0.5, 0.5),
            Self::TopLeft => (0.0, 0.0),
            Self::Top => (0.5, 0.0),
            Self::BottomLeft => (0.0, 1.0),
            Self::Bottom => (0.5, 1.0),
        }
    }
}

/// A colour description: ITU-T H.273 code points as stored, `None` where
/// nothing signals them, plus the range and the chroma location.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ColorInfo {
    /// `colour_primaries`.
    pub primaries: Option<u8>,
    /// `transfer_characteristics`.
    pub transfer: Option<u8>,
    /// `matrix_coefficients`.
    pub matrix: Option<u8>,
    /// Full range (`true`) or limited range (`false`).
    pub full_range: Option<bool>,
    /// Chroma sample location.
    pub chroma_location: Option<ChromaLocation>,
}

impl ColorInfo {
    /// Each field of `self`, or `other`'s where `self` has none.
    #[must_use]
    pub fn or(self, other: Self) -> Self {
        Self {
            primaries: self.primaries.or(other.primaries),
            transfer: self.transfer.or(other.transfer),
            matrix: self.matrix.or(other.matrix),
            full_range: self.full_range.or(other.full_range),
            chroma_location: self.chroma_location.or(other.chroma_location),
        }
    }
}

/// How Y'CbCr samples convert to R'G'B' (`docs/design/thumbnails.md`,
/// "Colour conversion").
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Matrix {
    /// R' = Y' + `cr_r`·Pr, G' = Y' − `cb_g`·Pb − `cr_g`·Pr, B' = Y' + `cb_b`·Pb.
    Kr {
        cr_r: f32,
        cb_g: f32,
        cr_g: f32,
        cb_b: f32,
    },
    /// YCgCo (H.273 matrix 8).
    YCgCo,
    /// The planes are G, B and R (H.273 matrix 0).
    Identity,
}

impl Matrix {
    /// The matrix for a `matrix_coefficients` code point. Unspecified (2),
    /// absent and code points without a fixed matrix are BT.601, as FFmpeg
    /// converts them; BT.2020 constant luminance (10) is converted as
    /// non-constant luminance.
    pub(crate) fn from_code(code: Option<u8>) -> Self {
        let (kr, kb) = match code {
            Some(0) => return Self::Identity,
            Some(8) => return Self::YCgCo,
            Some(1) => (0.2126, 0.0722),
            Some(4) => (0.30, 0.11),
            Some(7) => (0.212, 0.087),
            Some(9 | 10) => (0.2627, 0.0593),
            _ => (0.299, 0.114),
        };
        let kg = 1.0 - kr - kb;
        Self::Kr {
            cr_r: 2.0 * (1.0 - kr),
            cb_g: 2.0 * kb * (1.0 - kb) / kg,
            cr_g: 2.0 * kr * (1.0 - kr) / kg,
            cb_b: 2.0 * (1.0 - kb),
        }
    }
}

/// Scale and offset that map a sample of depth `depth` to the nominal
/// range: Y' in 0 to 1 and Pb, Pr in −0.5 to 0.5.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Levels {
    pub(crate) luma_offset: f32,
    pub(crate) luma_scale: f32,
    pub(crate) chroma_offset: f32,
    pub(crate) chroma_scale: f32,
}

impl Levels {
    pub(crate) fn new(depth: u8, full_range: bool) -> Self {
        let unit = f32::from(1u16 << (depth.clamp(8, 16) - 8));
        let max = (2f32).powi(i32::from(depth.clamp(8, 16))) - 1.0;
        if full_range {
            Self {
                luma_offset: 0.0,
                luma_scale: 1.0 / max,
                chroma_offset: 128.0 * unit,
                chroma_scale: 1.0 / max,
            }
        } else {
            Self {
                luma_offset: 16.0 * unit,
                luma_scale: 1.0 / (219.0 * unit),
                chroma_offset: 128.0 * unit,
                chroma_scale: 1.0 / (224.0 * unit),
            }
        }
    }
}

/// Converts one pixel (samples at the source depth, as `f32`) to 8-bit
/// R'G'B'.
#[inline]
pub(crate) fn to_rgb8(matrix: Matrix, levels: Levels, a: f32, b: f32, c: f32) -> [u8; 3] {
    let (r, g, bl) = match matrix {
        Matrix::Kr {
            cr_r,
            cb_g,
            cr_g,
            cb_b,
        } => {
            let y = (a - levels.luma_offset) * levels.luma_scale;
            let pb = (b - levels.chroma_offset) * levels.chroma_scale;
            let pr = (c - levels.chroma_offset) * levels.chroma_scale;
            (y + cr_r * pr, y - cb_g * pb - cr_g * pr, y + cb_b * pb)
        }
        Matrix::YCgCo => {
            let y = (a - levels.luma_offset) * levels.luma_scale;
            let cg = (b - levels.chroma_offset) * levels.chroma_scale;
            let co = (c - levels.chroma_offset) * levels.chroma_scale;
            let t = y - cg;
            (t + co, y + cg, t - co)
        }
        // Planes G, B, R, each scaled like luma.
        Matrix::Identity => (
            (c - levels.luma_offset) * levels.luma_scale,
            (a - levels.luma_offset) * levels.luma_scale,
            (b - levels.luma_offset) * levels.luma_scale,
        ),
    };
    [byte(r), byte(g), byte(bl)]
}

/// Converts one output row to 8-bit R'G'B' (`out`, three bytes a pixel):
/// `luma` has a sample per pixel, `cb` and `cr` one per `step` pixels (1 or
/// 2), `mid` where a chroma sample is missing. The arithmetic of
/// [`to_rgb8`], operation for operation, so the result is the same, with
/// the matrix chosen once per row rather than per pixel and the chroma
/// sample of a pair of pixels read once.
pub(crate) fn row_to_rgb8(
    matrix: Matrix,
    levels: Levels,
    rows: (&[f32], &[f32], &[f32]),
    step: usize,
    mid: f32,
    out: &mut [u8],
) {
    match matrix {
        Matrix::Kr {
            cr_r,
            cb_g,
            cr_g,
            cb_b,
        } => fill(rows, step, mid, out, |a, b, c| {
            let y = (a - levels.luma_offset) * levels.luma_scale;
            let pb = (b - levels.chroma_offset) * levels.chroma_scale;
            let pr = (c - levels.chroma_offset) * levels.chroma_scale;
            [
                byte(y + cr_r * pr),
                byte(y - cb_g * pb - cr_g * pr),
                byte(y + cb_b * pb),
            ]
        }),
        Matrix::YCgCo | Matrix::Identity => fill(rows, step, mid, out, |a, b, c| {
            to_rgb8(matrix, levels, a, b, c)
        }),
    }
}

/// [`row_to_rgb8`] for one conversion `f`.
#[inline(always)]
fn fill(
    (luma, cb, cr): (&[f32], &[f32], &[f32]),
    step: usize,
    mid: f32,
    out: &mut [u8],
    f: impl Fn(f32, f32, f32) -> [u8; 3],
) {
    let pixels = luma.len().min(out.len() / 3);
    let chroma = pixels.div_ceil(step.max(1));
    if step == 2 && cb.len() >= chroma && cr.len() >= chroma {
        // Two pixels share a chroma sample.
        let mut pairs = out.chunks_exact_mut(6);
        let mut luma_pairs = luma.chunks_exact(2);
        for ((px, l), (b, c)) in (&mut pairs).zip(&mut luma_pairs).zip(cb.iter().zip(cr)) {
            if let ([l0, l1], (first, second)) = (l, px.split_at_mut(3)) {
                first.copy_from_slice(&f(*l0, *b, *c));
                second.copy_from_slice(&f(*l1, *b, *c));
            }
        }
        let last = pixels / 2;
        if let (Some(l), Some(b), Some(c)) =
            (luma_pairs.remainder().first(), cb.get(last), cr.get(last))
        {
            if let Some(px) = pairs.into_remainder().get_mut(..3) {
                px.copy_from_slice(&f(*l, *b, *c));
            }
        }
    } else if step == 1 && cb.len() >= pixels && cr.len() >= pixels {
        for ((px, l), (b, c)) in out.chunks_exact_mut(3).zip(luma).zip(cb.iter().zip(cr)) {
            px.copy_from_slice(&f(*l, *b, *c));
        }
    } else {
        let step = step.max(1);
        for (x, (px, l)) in out.chunks_exact_mut(3).zip(luma).enumerate() {
            let (b, c) = (
                cb.get(x / step).copied().unwrap_or(mid),
                cr.get(x / step).copied().unwrap_or(mid),
            );
            px.copy_from_slice(&f(*l, b, c));
        }
    }
}

/// A nominal value (0 to 1) as an 8-bit value, rounded to nearest and
/// clamped.
#[inline]
fn byte(value: f32) -> u8 {
    let scaled = (value * 255.0 + 0.5).clamp(0.0, 255.0);
    // In 0..=255 after the clamp; NaN becomes 0.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let out = scaled as u8;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bt601_red() {
        // Limited-range BT.601 red, as an untagged 8-bit stream: what FFmpeg
        // converts it to.
        let rgb = to_rgb8(
            Matrix::from_code(None),
            Levels::new(8, false),
            81.0,
            90.0,
            240.0,
        );
        assert_eq!(rgb, [254, 0, 0]);
        let rgb709 = to_rgb8(
            Matrix::from_code(Some(1)),
            Levels::new(8, false),
            81.0,
            90.0,
            240.0,
        );
        assert_eq!(rgb709, [255, 24, 0]);
    }

    #[test]
    fn ten_bit_white_and_black() {
        let levels = Levels::new(10, false);
        let m = Matrix::from_code(Some(1));
        assert_eq!(to_rgb8(m, levels, 940.0, 512.0, 512.0), [255, 255, 255]);
        assert_eq!(to_rgb8(m, levels, 64.0, 512.0, 512.0), [0, 0, 0]);
        let full = Levels::new(8, true);
        assert_eq!(to_rgb8(m, full, 255.0, 128.0, 128.0), [255, 255, 255]);
    }
}
