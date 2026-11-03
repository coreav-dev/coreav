//! Image output: the formats, the pluggable encoder trait and the built-in
//! royalty-free encoders (`docs/design/thumbnails.md`, "Formats").

use coreav_frame::{Picture, PixelFormat};
#[cfg(any(feature = "native-h264", test))]
use std::io;
use std::io::Write;

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
    let mut out = Vec::new();
    webp_into(rgb, &mut out)?;
    Ok(out)
}

fn webp_into(rgb: &Picture, out: impl Write) -> Result<(), Error> {
    if rgb.format() != PixelFormat::Rgb24 {
        return Err(Error::Encode("WebP input must be RGB".into()));
    }
    let mut data = Vec::new();
    rgb.write_packed(&mut data)?;
    let expected = u64::from(rgb.width()) * u64::from(rgb.height()) * 3;
    if u64::try_from(data.len()).ok() != Some(expected) {
        return Err(Error::Encode("WebP input size".into()));
    }
    image_webp::WebPEncoder::new(out)
        .encode(
            &data,
            rgb.width(),
            rgb.height(),
            image_webp::ColorType::Rgb8,
        )
        .map_err(|e| Error::Encode(format!("WebP: {e}")))?;
    Ok(())
}

impl BuiltinEncoder {
    #[cfg(any(feature = "native-h264", test))]
    pub(crate) fn encode_bounded(&self, rgb: &Picture, max: usize) -> Result<Vec<u8>, Error> {
        if rgb.width() > 1280
            || rgb.height() > 1280
            || u64::from(rgb.width()) * u64::from(rgb.height()) > 921_600
            || max > 4 << 20
        {
            return Err(output_limit());
        }
        let mut out = BoundedOutput::new(max)?;
        let result = match self.format {
            ImageFormat::Jpeg => jpeg::encode_into(rgb, self.quality, &mut out),
            ImageFormat::Png => png::encode_bounded(rgb, &mut out),
            ImageFormat::Webp => webp_into(rgb, &mut out),
        };
        if out.exceeded {
            return Err(output_limit());
        }
        result?;
        Ok(out.bytes)
    }
}

#[cfg(any(feature = "native-h264", test))]
pub(crate) struct BoundedOutput {
    bytes: Vec<u8>,
    max: usize,
    exceeded: bool,
}

#[cfg(any(feature = "native-h264", test))]
impl BoundedOutput {
    fn new(max: usize) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(max).map_err(|_| output_limit())?;
        Ok(Self {
            bytes,
            max,
            exceeded: false,
        })
    }

    pub(crate) fn remaining(&self) -> usize {
        self.max.saturating_sub(self.bytes.len())
    }
}

#[cfg(any(feature = "native-h264", test))]
impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining() {
            self.exceeded = true;
            return Err(io::Error::other("thumbnail output exceeds its byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(any(feature = "native-h264", test))]
pub(crate) fn output_limit() -> Error {
    Error::Decode(coreav_decode::Error::new(
        coreav_decode::ErrorKind::Limit,
        "thumbnail output exceeds its byte limit",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_encoders_keep_native_bytes() {
        for (width, height) in [(1, 1), (17, 9), (64, 33)] {
            let mut rgb = Picture::zeroed(width, height, PixelFormat::Rgb24).expect("picture");
            for y in 0..height as usize {
                for (x, byte) in rgb.row_mut(0, y).expect("row").iter_mut().enumerate() {
                    *byte = (x.wrapping_mul(79) ^ y.wrapping_mul(37)) as u8;
                }
            }
            for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Webp] {
                for quality in [1, 91, 100] {
                    let encoder = BuiltinEncoder { format, quality };
                    let native = encoder.encode(&rgb).expect("native image");
                    assert_eq!(
                        encoder
                            .encode_bounded(&rgb, 4 << 20)
                            .expect("bounded image"),
                        native,
                        "{width}x{height} {format:?} {quality}",
                    );
                    assert_eq!(
                        encoder
                            .encode_bounded(&rgb, native.len())
                            .expect("exact limit"),
                        native,
                    );
                    assert!(matches!(
                        encoder.encode_bounded(&rgb, native.len() - 1),
                        Err(Error::Decode(coreav_decode::Error {
                            kind: coreav_decode::ErrorKind::Limit,
                            ..
                        })),
                    ));
                }
            }
        }
    }

    #[test]
    fn output_writes_refuse_before_growth() {
        let mut out = BoundedOutput::new(8).expect("writer");
        out.write_all(&[1; 8]).expect("at limit");
        let capacity = out.bytes.capacity();
        assert!(out.write_all(&[2]).is_err());
        assert_eq!(out.bytes, [1; 8]);
        assert_eq!(out.bytes.capacity(), capacity);
        assert!(out.exceeded);
        assert!(BoundedOutput::new(usize::MAX).is_err());
    }
}
