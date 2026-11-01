//! Decoded frames: CPU, CUDA and shared-memory pixels as first-class
//! variants.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use crate::{ColorInfo, FrameError, Picture, PixelFormat};

/// A decoded frame: when it is presented, how its colours are described,
/// and its pixels.
#[derive(Clone, Debug)]
pub struct Frame {
    /// Presentation timestamp of the packet that holds the frame's
    /// picture, in that packet's time scale.
    pub pts: Option<i64>,
    /// Colour description.
    pub color: ColorInfo,
    /// The pixels.
    pub pixels: Pixels,
}

/// Where a frame's pixels are.
#[derive(Clone, Debug)]
pub enum Pixels {
    /// In CPU memory.
    Cpu(Picture),
    /// In CUDA device memory.
    Cuda(CudaPicture),
    /// In memory the process that decoded it shares, and may still write.
    Shared(SharedPicture),
}

impl Frame {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        match &self.pixels {
            Pixels::Cpu(p) => p.width(),
            Pixels::Cuda(p) => p.width,
            Pixels::Shared(p) => p.width,
        }
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        match &self.pixels {
            Pixels::Cpu(p) => p.height(),
            Pixels::Cuda(p) => p.height,
            Pixels::Shared(p) => p.height,
        }
    }

    /// Pixel format.
    pub fn format(&self) -> PixelFormat {
        match &self.pixels {
            Pixels::Cpu(p) => p.format(),
            Pixels::Cuda(p) => p.format,
            Pixels::Shared(p) => p.format,
        }
    }

    /// The pixels in CPU memory: borrowed when they are there, copied from
    /// the device or the shared memory otherwise.
    pub fn to_cpu(&self) -> Result<Cow<'_, Picture>, FrameError> {
        match &self.pixels {
            Pixels::Cpu(p) => Ok(Cow::Borrowed(p)),
            Pixels::Cuda(p) => p.memory.download().map(Cow::Owned),
            Pixels::Shared(p) => p.download().map(Cow::Owned),
        }
    }
}

impl Pixels {
    /// The pixels in CPU memory: taken when they are there, copied from the
    /// device or the shared memory otherwise.
    pub fn into_cpu(self) -> Result<Picture, FrameError> {
        match self {
            Self::Cpu(p) => Ok(p),
            Self::Cuda(p) => p.memory.download(),
            Self::Shared(p) => p.download(),
        }
    }
}

impl From<Picture> for Pixels {
    fn from(picture: Picture) -> Self {
        Self::Cpu(picture)
    }
}

impl From<SharedPicture> for Pixels {
    fn from(picture: SharedPicture) -> Self {
        Self::Shared(picture)
    }
}

/// A picture in CUDA device memory. CoreAV never dereferences the
/// addresses; the backend that owns the memory copies it out
/// ([`DeviceMemory::download`]) and frees it when the last clone drops.
#[derive(Clone)]
pub struct CudaPicture {
    /// CUDA device ordinal.
    pub device: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format; NVDEC writes [`PixelFormat::SemiPlanar`].
    pub format: PixelFormat,
    /// Each plane's device address and pitch, for zero-copy consumers
    /// (DLPack).
    pub planes: Vec<DevicePlane>,
    /// The owner of the memory.
    pub memory: Arc<dyn DeviceMemory>,
}

impl fmt::Debug for CudaPicture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CudaPicture")
            .field("device", &self.device)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("planes", &self.planes)
            .finish_non_exhaustive()
    }
}

/// One plane in device memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevicePlane {
    /// Device address of the first row.
    pub address: u64,
    /// Bytes from one row to the next.
    pub pitch: usize,
}

/// The owner of a [`CudaPicture`]'s memory, implemented by the backend
/// that decoded it.
pub trait DeviceMemory: Send + Sync {
    /// Copies the picture to CPU memory.
    fn download(&self) -> Result<Picture, FrameError>;
}

/// A picture in memory shared with the process that decoded it, which may
/// write it at any time: a slot of the sandboxed worker's shared memory.
/// CoreAV reads it only by copying bytes out
/// ([`SharedMemory::read_packed`]), never through a reference; the owner
/// gets the memory back when the last clone drops.
#[derive(Clone)]
pub struct SharedPicture {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: PixelFormat,
    /// The owner of the memory.
    pub memory: Arc<dyn SharedMemory>,
}

impl fmt::Debug for SharedPicture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedPicture")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl SharedPicture {
    /// Rows and bytes per row (without padding) of each plane.
    fn plane_rows(&self) -> impl Iterator<Item = Option<(usize, usize)>> {
        (0..self.format.plane_count()).map(|plane| {
            let (_, rows) = self.format.plane_size(plane, self.width, self.height)?;
            let row_bytes = self.format.row_bytes(plane, self.width, self.height)?;
            Some((usize::try_from(rows).ok()?, row_bytes))
        })
    }

    /// Bytes of the planes in order with their rows packed
    /// ([`Picture::packed_len`]); `None` when the count overflows.
    pub fn packed_len(&self) -> Option<usize> {
        self.plane_rows().try_fold(0usize, |sum, plane| {
            let (rows, row_bytes) = plane?;
            sum.checked_add(rows.checked_mul(row_bytes)?)
        })
    }

    /// Copies the picture to memory this process owns, its rows packed.
    pub fn download(&self) -> Result<Picture, FrameError> {
        let mut planes = Vec::with_capacity(self.format.plane_count());
        let mut at = 0usize;
        for (index, plane) in self.plane_rows().enumerate() {
            let (rows, row_bytes) =
                plane.ok_or(FrameError::InvalidGeometry("plane size overflows"))?;
            let len = rows
                .checked_mul(row_bytes)
                .ok_or(FrameError::InvalidGeometry("plane size overflows"))?;
            let mut data = crate::picture::zeros(len)?;
            if self.memory.read_packed(at, &mut data) != Some(len) {
                return Err(FrameError::InvalidPlane {
                    plane: index,
                    reason: "outside its shared memory",
                });
            }
            at = at
                .checked_add(len)
                .ok_or(FrameError::InvalidGeometry("plane size overflows"))?;
            planes.push(crate::Plane::new(data, row_bytes));
        }
        Picture::new(self.width, self.height, self.format, planes)
    }
}

/// The owner of a [`SharedPicture`]'s memory, implemented by the process
/// that receives it.
pub trait SharedMemory: Send + Sync {
    /// Copies bytes from `at` of the picture's planes, packed in order as
    /// [`Picture::write_packed`] lays them out, into `out`: as many as fit
    /// and remain, which is 0 from the end on; `None` when they are not
    /// inside the memory.
    fn read_packed(&self, at: usize, out: &mut [u8]) -> Option<usize>;
}
