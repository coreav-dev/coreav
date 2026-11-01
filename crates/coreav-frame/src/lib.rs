//! CoreAV's frame model and frame operations (`docs/design/decode.md`,
//! "Frame model"; `docs/design/thumbnails.md`, "Frame operations").
//!
//! A [`Frame`] is a presentation timestamp, a colour description and its
//! pixels, in CPU memory ([`Picture`]) or CUDA device memory
//! ([`CudaPicture`]). A [`Picture`] is checked against its format and size
//! when it is built, so the operations ([`Picture::crop`],
//! [`Picture::rotate`], [`Picture::resize`], [`Picture::resize_with`],
//! [`Picture::to_rgb`], [`Picture::resize_to_rgb`]) never read or write
//! outside a plane; bad arguments give a [`FrameError`], never a panic.
//!
//! ```
//! use coreav_frame::{ChromaUpsampling, ColorInfo, Filter, Picture, PixelFormat};
//!
//! let frame = Picture::zeroed(1920, 1080, PixelFormat::YUV420P)?;
//! let thumb = frame.resize_to_rgb(
//!     320,
//!     180,
//!     Filter::Bicubic,
//!     &ColorInfo::default(),
//!     ChromaUpsampling::FfmpegCompatible,
//! )?;
//! assert_eq!((thumb.width(), thumb.height()), (320, 180));
//! assert_eq!(thumb.format(), PixelFormat::Rgb24);
//! # Ok::<(), coreav_frame::FrameError>(())
//! ```

#![forbid(unsafe_code)]

mod color;
mod error;
mod format;
mod frame;
mod ops;
mod picture;
mod resample;

pub use color::{ChromaLocation, ColorInfo};
pub use error::FrameError;
pub use format::{Chroma, PixelFormat};
pub use frame::{
    CudaPicture, DeviceMemory, DevicePlane, Frame, Pixels, SharedMemory, SharedPicture,
};
pub use ops::{ChromaSiting, ChromaUpsampling, Rect};
pub use picture::{Picture, Plane};
pub use resample::Filter;
