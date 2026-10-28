//! Pixel formats and the geometry of their planes.

use crate::FrameError;

/// Chroma subsampling of a Y'CbCr format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Chroma {
    /// Luma only (monochrome): one plane.
    Mono,
    /// Chroma at half the width and half the height.
    Cs420,
    /// Chroma at half the width, full height.
    Cs422,
    /// Chroma at full size.
    Cs444,
}

impl Chroma {
    /// Horizontal and vertical subsampling as shifts: `(1, 1)` for 4:2:0.
    /// Monochrome has no chroma; it reports `(0, 0)`.
    pub fn shifts(self) -> (u32, u32) {
        match self {
            Self::Cs420 => (1, 1),
            Self::Cs422 => (1, 0),
            Self::Cs444 | Self::Mono => (0, 0),
        }
    }
}

/// How a picture's samples are laid out in memory.
///
/// Above 8 bits a sample takes two bytes, little-endian. In the planar
/// formats ([`PixelFormat::Yuv`], [`PixelFormat::Gbr`]) the value is in the
/// low bits, as software decoders write it; in [`PixelFormat::SemiPlanar`]
/// it is in the high bits, as NVDEC writes it (P010, P016).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// Planar Y'CbCr: Y, Cb and Cr planes (Y alone for [`Chroma::Mono`]).
    Yuv {
        /// Chroma subsampling.
        chroma: Chroma,
        /// Bits per sample, 8 to 16.
        depth: u8,
    },
    /// A Y plane, then one plane of interleaved Cb and Cr samples, 4:2:0
    /// (NV12 at 8 bits).
    SemiPlanar {
        /// Bits per sample, 8 to 16.
        depth: u8,
    },
    /// Planar G, B and R, for streams coded in RGB (`matrix_coefficients`
    /// 0, ITU-T H.273).
    Gbr {
        /// Bits per sample, 8 to 16.
        depth: u8,
    },
    /// Packed 8-bit R, G, B.
    Rgb24,
    /// Packed 8-bit R, G, B, A.
    Rgba,
}

impl PixelFormat {
    /// 8-bit 4:2:0, the format most decoders output.
    pub const YUV420P: Self = Self::Yuv {
        chroma: Chroma::Cs420,
        depth: 8,
    };

    /// Checks the depth.
    pub fn validate(self) -> Result<(), FrameError> {
        match self.depth() {
            8..=16 => Ok(()),
            _ => Err(FrameError::InvalidGeometry("depth outside 8 to 16 bits")),
        }
    }

    /// Bits per sample.
    pub fn depth(self) -> u8 {
        match self {
            Self::Yuv { depth, .. } | Self::SemiPlanar { depth } | Self::Gbr { depth } => depth,
            Self::Rgb24 | Self::Rgba => 8,
        }
    }

    /// Bytes per sample: 1 at 8 bits, else 2.
    pub fn sample_bytes(self) -> usize {
        if self.depth() > 8 { 2 } else { 1 }
    }

    /// Number of planes.
    pub fn plane_count(self) -> usize {
        match self {
            Self::Yuv {
                chroma: Chroma::Mono,
                ..
            } => 1,
            Self::Yuv { .. } | Self::Gbr { .. } => 3,
            Self::SemiPlanar { .. } => 2,
            Self::Rgb24 | Self::Rgba => 1,
        }
    }

    /// Chroma subsampling of the Y'CbCr formats; `None` for RGB formats.
    pub fn chroma(self) -> Option<Chroma> {
        match self {
            Self::Yuv { chroma, .. } => Some(chroma),
            Self::SemiPlanar { .. } => Some(Chroma::Cs420),
            Self::Gbr { .. } | Self::Rgb24 | Self::Rgba => None,
        }
    }

    /// Bytes per element of plane `plane`: one sample, or for an
    /// interleaved plane one pixel (two chroma samples, three or four
    /// colour bytes).
    pub fn element_bytes(self, plane: usize) -> usize {
        match (self, plane) {
            (Self::SemiPlanar { .. }, 1) => 2 * self.sample_bytes(),
            (Self::Rgb24, _) => 3,
            (Self::Rgba, _) => 4,
            _ => self.sample_bytes(),
        }
    }

    /// Size in elements (columns, rows) of plane `plane` of a `width` ×
    /// `height` picture; `None` past the last plane.
    pub fn plane_size(self, plane: usize, width: u32, height: u32) -> Option<(u32, u32)> {
        if plane >= self.plane_count() {
            return None;
        }
        if plane == 0 {
            return Some((width, height));
        }
        let (sx, sy) = match self {
            Self::Yuv { chroma, .. } => chroma.shifts(),
            Self::SemiPlanar { .. } => (1, 1),
            Self::Gbr { .. } | Self::Rgb24 | Self::Rgba => (0, 0),
        };
        Some((subsampled(width, sx), subsampled(height, sy)))
    }

    /// Bytes of one row of plane `plane`, without padding.
    pub fn row_bytes(self, plane: usize, width: u32, height: u32) -> Option<usize> {
        let (columns, _) = self.plane_size(plane, width, height)?;
        usize::try_from(columns)
            .ok()?
            .checked_mul(self.element_bytes(plane))
    }
}

/// `length` divided by `2^shift`, rounded up.
pub(crate) fn subsampled(length: u32, shift: u32) -> u32 {
    if shift == 0 {
        length
    } else {
        length.div_ceil(1 << shift)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_geometry() {
        let f = PixelFormat::YUV420P;
        assert_eq!(f.plane_size(0, 641, 361), Some((641, 361)));
        assert_eq!(f.plane_size(1, 641, 361), Some((321, 181)));
        assert_eq!(f.plane_size(3, 641, 361), None);
        let p010 = PixelFormat::SemiPlanar { depth: 10 };
        assert_eq!(p010.row_bytes(0, 1920, 1080), Some(3840));
        assert_eq!(p010.row_bytes(1, 1920, 1080), Some(3840));
        assert_eq!(p010.plane_size(1, 1920, 1080), Some((960, 540)));
        let y422 = PixelFormat::Yuv {
            chroma: Chroma::Cs422,
            depth: 10,
        };
        assert_eq!(y422.row_bytes(2, 33, 10), Some(34));
        assert_eq!(y422.plane_size(2, 33, 10), Some((17, 10)));
        assert_eq!(PixelFormat::Rgb24.row_bytes(0, 5, 1), Some(15));
        assert!(PixelFormat::Gbr { depth: 7 }.validate().is_err());
    }
}
