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
    if picture.format() != PixelFormat::Rgb24 {
        return Err(Error::Encode("JPEG input must be RGB".into()));
    }
    let (w, h) = (
        u16::try_from(picture.width()).map_err(|_| Error::Encode("too wide for JPEG".into()))?,
        u16::try_from(picture.height()).map_err(|_| Error::Encode("too tall for JPEG".into()))?,
    );
    let mut rgb = Vec::new();
    picture.write_packed(&mut rgb)?;
    let mut out = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_2_2);
    encoder
        .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| Error::Encode(format!("JPEG: {e}")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
