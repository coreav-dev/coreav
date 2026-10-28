//! Pictures in CPU memory.

use crate::{FrameError, PixelFormat};

/// One plane: rows of samples, `stride` bytes apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plane {
    data: Vec<u8>,
    stride: usize,
}

impl Plane {
    /// A plane over `data` with rows `stride` bytes apart. Its size is
    /// checked when it becomes part of a [`Picture`].
    pub fn new(data: Vec<u8>, stride: usize) -> Self {
        Self { data, stride }
    }

    /// The plane's bytes, padding included.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Bytes from the start of one row to the start of the next.
    pub fn stride(&self) -> usize {
        self.stride
    }
}

/// A picture in CPU memory whose planes have been checked against its
/// format and size: every row of every plane lies inside the plane, so the
/// accessors and operations never read outside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    width: u32,
    height: u32,
    format: PixelFormat,
    planes: Vec<Plane>,
}

impl Picture {
    /// A picture over the given planes, checked: the width and height are
    /// at least 1, the format's depth is valid, there is one plane per
    /// format plane, each stride covers a row, and each plane holds
    /// `stride × (rows − 1) + row bytes`.
    pub fn new(
        width: u32,
        height: u32,
        format: PixelFormat,
        planes: Vec<Plane>,
    ) -> Result<Self, FrameError> {
        format.validate()?;
        if width == 0 || height == 0 {
            return Err(FrameError::InvalidGeometry("width or height of 0"));
        }
        if planes.len() != format.plane_count() {
            return Err(FrameError::InvalidPlane {
                plane: planes.len().min(format.plane_count()),
                reason: "wrong number of planes",
            });
        }
        for (index, plane) in planes.iter().enumerate() {
            let needed = plane_bytes(format, index, width, height, plane.stride)?;
            if plane.data.len() < needed {
                return Err(FrameError::InvalidPlane {
                    plane: index,
                    reason: "shorter than its rows",
                });
            }
        }
        Ok(Self {
            width,
            height,
            format,
            planes,
        })
    }

    /// A picture of zeros with rows packed without padding. Fails with
    /// [`FrameError::OutOfMemory`] rather than aborting when the memory
    /// cannot be reserved.
    pub fn zeroed(width: u32, height: u32, format: PixelFormat) -> Result<Self, FrameError> {
        format.validate()?;
        if width == 0 || height == 0 {
            return Err(FrameError::InvalidGeometry("width or height of 0"));
        }
        let mut planes = Vec::with_capacity(format.plane_count());
        for index in 0..format.plane_count() {
            let stride = format
                .row_bytes(index, width, height)
                .ok_or(FrameError::InvalidGeometry("row size overflows"))?;
            let bytes = plane_bytes(format, index, width, height, stride)?;
            planes.push(Plane::new(zeros(bytes)?, stride));
        }
        Self::new(width, height, format, planes)
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Pixel format.
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// The planes, in the format's order.
    pub fn planes(&self) -> &[Plane] {
        &self.planes
    }

    /// Rows and bytes per row (without padding) of plane `plane`.
    pub fn plane_rows(&self, plane: usize) -> Option<(usize, usize)> {
        let (_, rows) = self.format.plane_size(plane, self.width, self.height)?;
        let row_bytes = self.format.row_bytes(plane, self.width, self.height)?;
        Some((usize::try_from(rows).ok()?, row_bytes))
    }

    /// Row `y` of plane `plane`, without padding.
    pub fn row(&self, plane: usize, y: usize) -> Option<&[u8]> {
        let (rows, row_bytes) = self.plane_rows(plane)?;
        if y >= rows {
            return None;
        }
        let p = self.planes.get(plane)?;
        let start = y.checked_mul(p.stride)?;
        p.data.get(start..start.checked_add(row_bytes)?)
    }

    /// Row `y` of plane `plane`, mutable, without padding.
    pub fn row_mut(&mut self, plane: usize, y: usize) -> Option<&mut [u8]> {
        let (rows, row_bytes) = self.plane_rows(plane)?;
        if y >= rows {
            return None;
        }
        let p = self.planes.get_mut(plane)?;
        let start = y.checked_mul(p.stride)?;
        p.data.get_mut(start..start.checked_add(row_bytes)?)
    }

    /// Every row of plane `plane`, in order.
    pub fn rows(&self, plane: usize) -> impl Iterator<Item = &[u8]> {
        let rows = self.plane_rows(plane).map_or(0, |(rows, _)| rows);
        (0..rows).filter_map(move |y| self.row(plane, y))
    }

    /// Bytes of the planes in order with their rows packed, as FFmpeg's raw
    /// video and `framemd5` lay them out.
    pub fn packed_len(&self) -> usize {
        (0..self.planes.len())
            .filter_map(|p| self.plane_rows(p))
            .fold(0usize, |sum, (rows, bytes)| {
                sum.saturating_add(rows.saturating_mul(bytes))
            })
    }

    /// Appends the planes in order with their rows packed to `out`.
    pub fn write_packed(&self, out: &mut Vec<u8>) -> Result<(), FrameError> {
        out.try_reserve(self.packed_len())
            .map_err(|_| FrameError::OutOfMemory)?;
        for plane in 0..self.planes.len() {
            for row in self.rows(plane) {
                out.extend_from_slice(row);
            }
        }
        Ok(())
    }
}

/// Bytes plane `plane` needs with rows `stride` bytes apart.
fn plane_bytes(
    format: PixelFormat,
    plane: usize,
    width: u32,
    height: u32,
    stride: usize,
) -> Result<usize, FrameError> {
    let row = format
        .row_bytes(plane, width, height)
        .ok_or(FrameError::InvalidGeometry("row size overflows"))?;
    if stride < row {
        return Err(FrameError::InvalidPlane {
            plane,
            reason: "stride shorter than a row",
        });
    }
    let (_, rows) = format
        .plane_size(plane, width, height)
        .ok_or(FrameError::InvalidGeometry("no such plane"))?;
    let rows = usize::try_from(rows).map_err(|_| FrameError::InvalidGeometry("too many rows"))?;
    rows.checked_sub(1)
        .and_then(|r| r.checked_mul(stride))
        .and_then(|b| b.checked_add(row))
        .ok_or(FrameError::InvalidGeometry("plane size overflows"))
}

/// `len` zero bytes, or [`FrameError::OutOfMemory`].
pub(crate) fn zeros(len: usize) -> Result<Vec<u8>, FrameError> {
    let mut v = Vec::new();
    v.try_reserve_exact(len)
        .map_err(|_| FrameError::OutOfMemory)?;
    v.resize(len, 0);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Chroma;

    #[test]
    fn checks_planes() {
        let f = PixelFormat::YUV420P;
        let ok = Picture::new(
            3,
            3,
            f,
            vec![
                Plane::new(vec![0; 4 * 2 + 3], 4),
                Plane::new(vec![0; 4], 2),
                Plane::new(vec![0; 4], 2),
            ],
        );
        assert!(ok.is_ok());
        let short = Picture::new(
            3,
            3,
            f,
            vec![
                Plane::new(vec![0; 10], 4),
                Plane::new(vec![0; 4], 2),
                Plane::new(vec![0; 4], 2),
            ],
        );
        assert!(matches!(
            short,
            Err(FrameError::InvalidPlane { plane: 0, .. })
        ));
        let narrow = Picture::new(
            3,
            3,
            f,
            vec![
                Plane::new(vec![0; 100], 2),
                Plane::new(vec![0; 4], 2),
                Plane::new(vec![0; 4], 2),
            ],
        );
        assert!(narrow.is_err());
        assert!(Picture::new(0, 3, f, vec![]).is_err());
        assert!(Picture::new(3, 3, f, vec![Plane::new(vec![0; 100], 4)]).is_err());
    }

    #[test]
    fn packed_layout() {
        let f = PixelFormat::Yuv {
            chroma: Chroma::Cs420,
            depth: 10,
        };
        let p = Picture::zeroed(5, 3, f).expect("picture");
        // Luma 5 × 3 two-byte samples, chroma 3 × 2 each.
        assert_eq!(p.packed_len(), 5 * 3 * 2 + 2 * (3 * 2 * 2));
        let mut out = Vec::new();
        p.write_packed(&mut out).expect("packed");
        assert_eq!(out.len(), p.packed_len());
        assert_eq!(p.row(1, 2), None);
        assert_eq!(p.row(1, 1).map(<[u8]>::len), Some(6));
    }
}
