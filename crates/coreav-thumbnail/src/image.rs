//! Image output: the formats, the pluggable encoder trait and the built-in
//! royalty-free encoders (`docs/design/thumbnails.md`, "Formats").

use coreav_frame::{Picture, PixelFormat};

use crate::{Error, jpeg, png};

/// An image format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    /// Baseline JPEG (JFIF), 4:2:0.
    #[default]
    Jpeg,
    /// PNG, 8-bit RGB.
    Png,
    /// Lossless WebP.
    Webp,
}

impl ImageFormat {
    /// A stable lower-case name: `jpeg`, `png`, `webp`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    /// The format named `name` (also `jpg`).
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "jpeg" | "jpg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }
}

/// Turns an RGB picture into image file bytes.
pub trait ImageEncoder {
    /// The format it writes.
    fn format(&self) -> ImageFormat;

    /// Encodes `rgb`, a [`PixelFormat::Rgb24`] picture.
    fn encode(&self, rgb: &Picture) -> Result<Vec<u8>, Error>;
}

/// The built-in encoders: JPEG through the `jpeg-encoder` crate, CoreAV's
/// own PNG writer, and `image-webp`'s lossless WebP encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinEncoder {
    /// The format.
    pub format: ImageFormat,
    /// JPEG quality, 1 to 100.
    pub quality: u8,
}

impl ImageEncoder for BuiltinEncoder {
    fn format(&self) -> ImageFormat {
        self.format
    }

    fn encode(&self, rgb: &Picture) -> Result<Vec<u8>, Error> {
        match self.format {
            ImageFormat::Jpeg => jpeg::encode(rgb, self.quality),
            ImageFormat::Png => png::encode(rgb),
            ImageFormat::Webp => webp(rgb),
        }
    }
}

fn webp(rgb: &Picture) -> Result<Vec<u8>, Error> {
    if rgb.format() != PixelFormat::Rgb24 {
        return Err(Error::Encode("WebP input must be RGB".into()));
    }
    let mut data = Vec::new();
    rgb.write_packed(&mut data)?;
    let expected = u64::from(rgb.width()) * u64::from(rgb.height()) * 3;
    if u64::try_from(data.len()).ok() != Some(expected) {
        return Err(Error::Encode("WebP input size".into()));
    }
    let mut out = Vec::new();
    image_webp::WebPEncoder::new(&mut out)
        .encode(
            &data,
            rgb.width(),
            rgb.height(),
            image_webp::ColorType::Rgb8,
        )
        .map_err(|e| Error::Encode(format!("WebP: {e}")))?;
    Ok(out)
}
