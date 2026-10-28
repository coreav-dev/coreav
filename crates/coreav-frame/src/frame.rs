//! Decoded frames: CPU and CUDA pixels as first-class variants.

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
}

impl Frame {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        match &self.pixels {
            Pixels::Cpu(p) => p.width(),
            Pixels::Cuda(p) => p.width,
        }
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        match &self.pixels {
            Pixels::Cpu(p) => p.height(),
            Pixels::Cuda(p) => p.height,
        }
    }

    /// Pixel format.
    pub fn format(&self) -> PixelFormat {
        match &self.pixels {
            Pixels::Cpu(p) => p.format(),
            Pixels::Cuda(p) => p.format,
        }
    }

    /// The pixels in CPU memory: borrowed when they are there, copied from
    /// the device otherwise.
    pub fn to_cpu(&self) -> Result<Cow<'_, Picture>, FrameError> {
        match &self.pixels {
            Pixels::Cpu(p) => Ok(Cow::Borrowed(p)),
            Pixels::Cuda(p) => p.memory.download().map(Cow::Owned),
        }
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
