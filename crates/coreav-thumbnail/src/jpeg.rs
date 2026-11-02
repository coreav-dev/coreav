//! JPEG thumbnails through the `jpeg-encoder` crate
//! (`docs/design/thumbnails.md`, "JPEG encoder"): baseline, 8-bit Y'CbCr
//! 4:2:0 with JFIF's full-range BT.601, the example tables of T.81 Annex K
//! scaled by the quality. Its input is CoreAV's own RGB pixels, never
//! untrusted bytes.

use coreav_frame::{Picture, PixelFormat};

use crate::Error;

/// Encodes an [`PixelFormat::Rgb24`] picture as a baseline JFIF JPEG at
/// `quality` (1 to 100).
pub fn encode(picture: &Picture, quality: u8) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    encode_into(picture, quality, &mut out)?;
    Ok(out)
}

pub(crate) fn encode_into(
    picture: &Picture,
    quality: u8,
    out: impl std::io::Write,
) -> Result<(), Error> {
    if picture.format() != PixelFormat::Rgb24 {
        return Err(Error::Encode("JPEG input must be RGB".into()));
    }
    let (w, h) = (
        u16::try_from(picture.width()).map_err(|_| Error::Encode("too wide for JPEG".into()))?,
        u16::try_from(picture.height()).map_err(|_| Error::Encode("too tall for JPEG".into()))?,
    );
    let mut rgb = Vec::new();
    picture.write_packed(&mut rgb)?;
    let mut encoder = jpeg_encoder::Encoder::new(out, quality.clamp(1, 100));
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_2_2);
    encoder
        .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| Error::Encode(format!("JPEG: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn portable_output_does_not_depend_on_cpu_dispatch() {
        let mut picture = Picture::zeroed(8, 8, PixelFormat::Rgb24).expect("picture");
        for y in 0..8 {
            for (x, pixel) in picture
                .row_mut(0, y)
                .expect("row")
                .chunks_exact_mut(3)
                .enumerate()
            {
                pixel.copy_from_slice(&[
                    (37 * x + 13 * y) as u8,
                    (11 * x + 53 * y + 7) as u8,
                    (71 * x + 19 * y + 101) as u8,
                ]);
            }
        }
        let encoded = encode(&picture, 100).expect("JPEG");
        assert_eq!(encoded.len(), 898);
        assert_eq!(
            format!("{:x}", Sha256::digest(&encoded)),
            "539b88e1b5f6aa0bd095d9e0edb0c5f9c403020c1efc8d7caf6d7b18c3c091f3",
        );
        let bounded = crate::BuiltinEncoder {
            format: crate::ImageFormat::Jpeg,
            quality: 100,
        }
        .encode_bounded(&picture, 4 << 20)
        .expect("bounded JPEG");
        assert_eq!(bounded, encoded);
    }

    #[test]
    fn encodes_markers_and_sizes() {
        for (w, h) in [(1, 1), (17, 9), (320, 180), (333, 187)] {
            let p = Picture::zeroed(w, h, PixelFormat::Rgb24).expect("picture");
            let jpeg = encode(&p, 90).expect("jpeg");
            assert!(jpeg.starts_with(&[0xFF, 0xD8]), "{w}x{h}");
            assert!(jpeg.ends_with(&[0xFF, 0xD9]), "{w}x{h}");
            // The frame header (SOF0) gives the size.
            let sof = jpeg
                .windows(2)
                .position(|m| m == [0xFF, 0xC0])
                .expect("SOF0");
            let size = |at: usize| u32::from(u16::from_be_bytes([jpeg[at], jpeg[at + 1]]));
            assert_eq!((size(sof + 7), size(sof + 5)), (w, h));
        }
    }

    #[test]
    fn refuses_what_jpeg_cannot_hold() {
        let p = Picture::zeroed(70_000, 1, PixelFormat::Rgb24).expect("picture");
        assert!(encode(&p, 90).is_err());
        let rgba = Picture::zeroed(4, 4, PixelFormat::Rgba).expect("picture");
        assert!(encode(&rgba, 90).is_err());
    }
}
