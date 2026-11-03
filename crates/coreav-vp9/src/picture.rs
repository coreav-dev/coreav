//! The frame being decoded (`CurrFrame`) and the decoded frames the
//! decoder hands out.

use crate::header::{ColorConfig, ColorSpace};
use crate::memory::Memory;
use crate::{Error, LimitKind, Result};
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
        memory: &mut Memory,
    ) -> Result<Self> {
        let width = (mi_cols as usize).div_ceil(8) * 64;
        let height = (mi_rows as usize).div_ceil(8) * 64;
        let mut plane = |sub_x: bool, sub_y: bool| -> Result<Plane<P>> {
            let stride = width >> u32::from(sub_x);
            let len = stride
                .checked_mul(height >> u32::from(sub_y))
                .ok_or(Error::Limit(LimitKind::MemoryBytes))?;
            Ok(Plane {
                samples: memory.filled(len, P::default())?,
                stride,
            })
        };
        Ok(Planes {
            planes: [
                plane(false, false)?,
                plane(subsampling_x, subsampling_y)?,
                plane(subsampling_x, subsampling_y)?,
            ],
        })
    }

    /// The frame's visible samples (the output process, clause 8.9).
    pub(crate) fn picture(
        &self,
        width: u32,
        height: u32,
        color: ColorConfig,
        memory: &mut Memory,
    ) -> Result<Picture> {
        let ColorConfig {
            bit_depth,
            subsampling_x,
            subsampling_y,
            color_space,
            full_range,
        } = color;
        let mut visible = |i: usize, plane: &Plane<P>| -> Result<Samples> {
            let (w, h) = if i == 0 {
                (width, height)
            } else {
                (
                    (width + u32::from(subsampling_x)) >> u32::from(subsampling_x),
                    (height + u32::from(subsampling_y)) >> u32::from(subsampling_y),
                )
            };
            let len = (w as usize)
                .checked_mul(h as usize)
                .ok_or(Error::Limit(LimitKind::MemoryBytes))?;
            let mut samples = memory.empty(len)?;
            for row in plane.samples.chunks(plane.stride.max(1)).take(h as usize) {
                let row = row
                    .get(..w as usize)
                    .ok_or(Error::Invalid("visible plane row"))?;
                samples.extend_from_slice(row);
            }
            if samples.len() != len {
                return Err(Error::Invalid("visible plane rows"));
            }
            Ok(P::samples(samples))
        };
        let [y, u, v] = &self.planes;
        Ok(Picture {
            width,
            height,
            bit_depth,
            subsampling_x,
            subsampling_y,
            color_space,
            full_range,
            planes: [visible(0, y)?, visible(1, u)?, visible(2, v)?],
        })
    }
}

/// A sample type the decoder decodes into.
pub(crate) trait Sample: Pixel {
    fn samples(samples: Vec<Self>) -> Samples;
    fn plane(samples: &Samples) -> Option<&[Self]>;
}

impl Sample for u8 {
    fn samples(samples: Vec<Self>) -> Samples {
        Samples::Eight(samples)
    }
    fn plane(samples: &Samples) -> Option<&[Self]> {
        match samples {
            Samples::Eight(s) => Some(s),
            Samples::High(_) => None,
        }
    }
}

impl Sample for u16 {
    fn samples(samples: Vec<Self>) -> Samples {
        Samples::High(samples)
    }
    fn plane(samples: &Samples) -> Option<&[Self]> {
        match samples {
            Samples::High(s) => Some(s),
            Samples::Eight(_) => None,
        }
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
    /// The frame's VP9 `color_space`.
    pub color_space: ColorSpace,
    /// The frame's VP9 `color_range`.
    pub full_range: bool,
    /// The Y, U and V planes.
    pub planes: [Samples; 3],
}

impl Picture {
    pub(crate) fn charge(&self, memory: &mut Memory) -> Result<()> {
        for plane in &self.planes {
            match plane {
                Samples::Eight(s) => memory.charge::<u8>(s.capacity())?,
                Samples::High(s) => memory.charge::<u16>(s.capacity())?,
            }
        }
        Ok(())
    }

    pub(crate) fn copy(&self, memory: &mut Memory) -> Result<Self> {
        let mut plane = |s: &Samples| -> Result<Samples> {
            Ok(match s {
                Samples::Eight(s) => Samples::Eight(memory.copy(s)?),
                Samples::High(s) => Samples::High(memory.copy(s)?),
            })
        };
        let [y, u, v] = &self.planes;
        Ok(Self {
            planes: [plane(y)?, plane(u)?, plane(v)?],
            width: self.width,
            height: self.height,
            bit_depth: self.bit_depth,
            subsampling_x: self.subsampling_x,
            subsampling_y: self.subsampling_y,
            color_space: self.color_space,
            full_range: self.full_range,
        })
    }
}

/// One plane's samples: bytes at 8 bits, 16-bit values at 10 and 12.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Samples {
    /// 8-bit samples.
    Eight(Vec<u8>),
    /// 10 or 12-bit samples.
    High(Vec<u16>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_high_depth_planes_charge_superblock_padding_and_visible_samples() {
        let working = (128 * 64 + 2 * 64 * 32) * 2;
        let visible = (65 * 33 + 2 * 33 * 17) * 2;
        let color = ColorConfig {
            bit_depth: 10,
            subsampling_x: true,
            subsampling_y: true,
            color_space: ColorSpace::Bt709,
            full_range: false,
        };
        let mut memory = Memory::new(working + visible - 1);
        let frame = Planes::<u16>::new(9, 5, true, true, &mut memory).unwrap();
        assert!(matches!(
            frame.picture(65, 33, color, &mut memory),
            Err(Error::Limit(LimitKind::MemoryBytes))
        ));
        let mut memory = Memory::new(working + visible);
        let frame = Planes::<u16>::new(9, 5, true, true, &mut memory).unwrap();
        let picture = frame.picture(65, 33, color, &mut memory).unwrap();
        let Samples::High(y) = &picture.planes[0] else {
            panic!()
        };
        assert_eq!(y.len(), 65 * 33);
    }

    #[test]
    fn bulk_visible_rows_preserve_samples_order_and_capacity_charges() {
        fn check<P: Sample>(width: u32, height: u32, color: ColorConfig, value: fn(usize) -> P) {
            let mut memory = Memory::new(u64::MAX);
            let mut frame = Planes::<P>::new(
                width.div_ceil(8),
                height.div_ceil(8),
                color.subsampling_x,
                color.subsampling_y,
                &mut memory,
            )
            .unwrap();
            for (i, plane) in frame.planes.iter_mut().enumerate() {
                for (j, sample) in plane.samples.iter_mut().enumerate() {
                    *sample = value(i * 17071 + j);
                }
            }
            let before = memory.used();
            let picture = frame.picture(width, height, color, &mut memory).unwrap();
            let mut charged = 0;
            for (i, samples) in picture.planes.iter().enumerate() {
                let actual = P::plane(samples).unwrap();
                let (w, h) = if i == 0 {
                    (width, height)
                } else {
                    (
                        width.div_ceil(1 << u32::from(color.subsampling_x)),
                        height.div_ceil(1 << u32::from(color.subsampling_y)),
                    )
                };
                let stride = frame.planes[i].stride;
                let expected: Vec<P> = (0..h as usize)
                    .flat_map(|r| (0..w as usize).map(move |c| value(i * 17071 + r * stride + c)))
                    .collect();
                assert_eq!(actual, expected);
                charged += match samples {
                    Samples::Eight(s) => s.capacity() as u64,
                    Samples::High(s) => (s.capacity() * 2) as u64,
                };
            }
            assert_eq!(memory.used() - before, charged);
        }
        for (width, height) in [(1, 1), (3, 5), (63, 65), (65, 33)] {
            for subsampling_x in [false, true] {
                for subsampling_y in [false, true] {
                    let color = ColorConfig {
                        bit_depth: 10,
                        subsampling_x,
                        subsampling_y,
                        color_space: ColorSpace::Bt709,
                        full_range: true,
                    };
                    check(width, height, color, |i| i as u8);
                    check(width, height, color, |i| i as u16);
                }
            }
        }
    }
}
