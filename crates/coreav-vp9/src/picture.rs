//! The frame being decoded (`CurrFrame`) and the decoded frames the
//! decoder hands out.

use coreav_kernels_vp9::Pixel;

/// One plane of the frame being decoded: rows of `stride` samples that
/// cover the frame rounded up to whole superblocks, so every transform
/// block of the frame's superblocks fits.
#[derive(Debug)]
pub(crate) struct Plane<P> {
    pub(crate) samples: Vec<P>,
    pub(crate) stride: usize,
}

/// `CurrFrame`: the three planes of the frame being decoded.
#[derive(Debug)]
pub(crate) struct Planes<P> {
    pub(crate) planes: [Plane<P>; 3],
}

impl<P: Sample> Planes<P> {
    /// The planes of a frame of `mi_cols` by `mi_rows` 8x8 blocks.
    pub(crate) fn new(
        mi_cols: u32,
        mi_rows: u32,
        subsampling_x: bool,
        subsampling_y: bool,
    ) -> Self {
        let width = (mi_cols as usize).div_ceil(8) * 64;
        let height = (mi_rows as usize).div_ceil(8) * 64;
        let plane = |sub_x: bool, sub_y: bool| {
            let stride = width >> u32::from(sub_x);
            Plane {
                samples: vec![P::default(); stride * (height >> u32::from(sub_y))],
                stride,
            }
        };
        Planes {
            planes: [
                plane(false, false),
                plane(subsampling_x, subsampling_y),
                plane(subsampling_x, subsampling_y),
            ],
        }
    }

    /// The frame's visible samples (the output process, clause 8.9).
    pub(crate) fn picture(
        &self,
        width: u32,
        height: u32,
        bit_depth: u8,
        subsampling_x: bool,
        subsampling_y: bool,
    ) -> Picture {
        let visible = |i: usize, plane: &Plane<P>| {
            let (w, h) = if i == 0 {
                (width, height)
            } else {
                (
                    (width + u32::from(subsampling_x)) >> u32::from(subsampling_x),
                    (height + u32::from(subsampling_y)) >> u32::from(subsampling_y),
                )
            };
            let samples = plane
                .samples
                .chunks(plane.stride.max(1))
                .take(h as usize)
                .flat_map(|row| row.iter().take(w as usize).copied())
                .collect();
            P::samples(samples)
        };
        let [y, u, v] = &self.planes;
        Picture {
            width,
            height,
            bit_depth,
            subsampling_x,
            subsampling_y,
            planes: [visible(0, y), visible(1, u), visible(2, v)],
        }
    }
}

/// A sample type the decoder decodes into.
pub(crate) trait Sample: Pixel {
    fn samples(samples: Vec<Self>) -> Samples;
}

impl Sample for u8 {
    fn samples(samples: Vec<Self>) -> Samples {
        Samples::Eight(samples)
    }
}

impl Sample for u16 {
    fn samples(samples: Vec<Self>) -> Samples {
        Samples::High(samples)
    }
}

/// A decoded frame: its visible samples, plane by plane (Y, U, V), each
/// in rows without padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    /// `FrameWidth`.
    pub width: u32,
    /// `FrameHeight`.
    pub height: u32,
    /// `BitDepth`.
    pub bit_depth: u8,
    /// `subsampling_x`: the U and V planes are `(width + 1) >> 1` wide.
    pub subsampling_x: bool,
    /// `subsampling_y`: the U and V planes are `(height + 1) >> 1` high.
    pub subsampling_y: bool,
    /// The Y, U and V planes.
    pub planes: [Samples; 3],
}

/// One plane's samples: bytes at 8 bits, 16-bit values at 10 and 12.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Samples {
    /// 8-bit samples.
    Eight(Vec<u8>),
    /// 10 or 12-bit samples.
    High(Vec<u16>),
}
