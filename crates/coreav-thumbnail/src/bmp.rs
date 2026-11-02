//! Windows bitmaps with a 40-byte BITMAPINFOHEADER and 24-bit BI_RGB pixels.

use coreav_frame::{Picture, PixelFormat};

use crate::Error;

/// Encodes RGB samples as a bottom-up, DWORD-aligned Windows bitmap.
pub fn encode(picture: &Picture) -> Result<Vec<u8>, Error> {
    if picture.format() != PixelFormat::Rgb24 {
        return Err(Error::Encode("BMP input must be RGB24".into()));
    }
    let bad = || Error::Encode("BMP dimensions or file size exceed the format".into());
    let width = i32::try_from(picture.width()).map_err(|_| bad())?;
    let height = i32::try_from(picture.height()).map_err(|_| bad())?;
    let row = picture.width().checked_mul(3).ok_or_else(bad)?;
    let stride = row.checked_add(3).ok_or_else(bad)? & !3;
    let pixels = stride.checked_mul(picture.height()).ok_or_else(bad)?;
    let size = pixels.checked_add(54).ok_or_else(bad)?;
    let mut out = Vec::new();
    out.try_reserve_exact(usize::try_from(size).map_err(|_| bad())?)
        .map_err(|_| Error::Encode("out of memory for BMP".into()))?;
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&pixels.to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for y in (0..usize::try_from(picture.height()).map_err(|_| bad())?).rev() {
        let data = picture.row(0, y).ok_or_else(bad)?;
        for rgb in data.chunks_exact(3) {
            out.extend(rgb.iter().rev());
        }
        out.extend((row..stride).map(|_| 0));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_up_bgr_rows_keep_pixels_and_align_to_dwords() {
        for width in 1..=5 {
            let mut picture = Picture::zeroed(width, 3, PixelFormat::Rgb24).unwrap();
            for y in 0..3 {
                for (x, byte) in picture.row_mut(0, y).unwrap().iter_mut().enumerate() {
                    *byte = (y * 51 + x * 3) as u8;
                }
            }
            let bmp = encode(&picture).unwrap();
            let number = |offset| u32::from_le_bytes(bmp[offset..offset + 4].try_into().unwrap());
            let stride = (width as usize * 3 + 3) & !3;
            assert_eq!(&bmp[..2], b"BM");
            assert_eq!(number(2) as usize, bmp.len());
            assert_eq!(number(10), 54);
            assert_eq!(number(14), 40);
            assert_eq!(number(18), width);
            assert_eq!(number(22), 3);
            assert_eq!(&bmp[26..30], &[1, 0, 24, 0]);
            assert_eq!(number(30), 0);
            assert_eq!(number(34) as usize, stride * 3);
            for (y, row) in bmp[54..].chunks_exact(stride).rev().enumerate() {
                let actual: Vec<_> = row[..width as usize * 3]
                    .chunks_exact(3)
                    .flat_map(|p| p.iter().rev().copied())
                    .collect();
                assert_eq!(actual, picture.row(0, y).unwrap());
                assert!(row[width as usize * 3..].iter().all(|v| *v == 0));
            }
        }
        assert!(encode(&Picture::zeroed(1, 1, PixelFormat::Rgba).unwrap()).is_err());
    }
}
