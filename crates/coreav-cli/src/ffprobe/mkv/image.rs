//! What `ffprobe` reports of an attached picture (a cover image in
//! `Attachments`), read from the image's header: ITU-T T.81 (JPEG) frame
//! headers, the PNG `IHDR` chunk (ISO/IEC 15948 11.2.2). Bounded by the
//! input; `None` on anything else.

/// An attached picture's codec and layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Picture {
    /// `ffprobe`'s codec name.
    pub codec: &'static str,
    pub width: u32,
    pub height: u32,
    /// The pixel format the decoder puts out.
    pub pix_fmt: Option<&'static str>,
    /// The JPEG process (`Baseline`, `Progressive`...).
    pub profile: Option<&'static str>,
    /// Sample precision, for JPEG.
    pub bits: Option<u32>,
    /// A PNG in the sRGB colour space.
    pub srgb: bool,
    /// The pixel aspect the image states: JFIF densities, PNG `pHYs`.
    pub aspect: Option<(u32, u32)>,
}

impl Picture {
    /// The colour the image states: sRGB's transfer and BT.709 primaries.
    pub fn color(&self) -> coreav_isobmff::Color {
        coreav_isobmff::Color {
            primaries: self.srgb.then_some(1),
            transfer: self.srgb.then_some(13),
            ..coreav_isobmff::Color::default()
        }
    }
}

/// Reads a JPEG's first frame header.
pub fn jpeg(data: &[u8]) -> Option<Picture> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2usize;
    loop {
        // Skip fill bytes before a marker.
        while data.get(at)? == &0xFF && data.get(at + 1)? == &0xFF {
            at += 1;
        }
        if data.get(at)? != &0xFF {
            return None;
        }
        let marker = *data.get(at + 1)?;
        at += 2;
        if matches!(marker, 0x01 | 0xD0..=0xD7) {
            continue;
        }
        let len = usize::from(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]));
        let segment = data.get(at + 2..at + len)?;
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let bits = u32::from(*segment.first()?);
            let height = u32::from(u16::from_be_bytes([*segment.get(1)?, *segment.get(2)?]));
            let width = u32::from(u16::from_be_bytes([*segment.get(3)?, *segment.get(4)?]));
            let components = *segment.get(5)?;
            let sampling = |c: usize| segment.get(6 + 3 * c + 1).copied();
            let pix_fmt = match (components, bits) {
                (1, 8) => Some("gray"),
                (3, 8) => {
                    // Luma sampling relative to the largest chroma sampling.
                    let (y, cb, cr) = (sampling(0)?, sampling(1)?, sampling(2)?);
                    let h = |b: u8| b >> 4;
                    let v = |b: u8| b & 0x0F;
                    let (hc, vc) = (h(cb).max(h(cr)), v(cb).max(v(cr)));
                    if hc == 0 || vc == 0 || h(y) % hc != 0 || v(y) % vc != 0 {
                        None
                    } else {
                        match (h(y) / hc, v(y) / vc) {
                            (2, 2) => Some("yuvj420p"),
                            (2, 1) => Some("yuvj422p"),
                            (1, 1) => Some("yuvj444p"),
                            (1, 2) => Some("yuvj440p"),
                            (4, 1) => Some("yuvj411p"),
                            _ => None,
                        }
                    }
                }
                _ => None,
            };
            let profile = match marker {
                0xC0 => Some("Baseline"),
                0xC1 => Some("Sequential"),
                0xC2 => Some("Progressive"),
                0xC3 => Some("Lossless"),
                _ => None,
            };
            return Some(Picture {
                codec: "mjpeg",
                width,
                height,
                pix_fmt,
                profile,
                bits: Some(bits),
                srgb: false,
                aspect: jfif_density(data),
            });
        }
        if marker == 0xDA {
            return None;
        }
        at += len;
    }
}

/// Reads a PNG's `IHDR`.
pub fn png(data: &[u8]) -> Option<Picture> {
    if data.get(..8)? != b"\x89PNG\r\n\x1a\n" || data.get(12..16)? != b"IHDR" {
        return None;
    }
    let field = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes([
            *data.get(at)?,
            *data.get(at + 1)?,
            *data.get(at + 2)?,
            *data.get(at + 3)?,
        ]))
    };
    let (width, height) = (field(16)?, field(20)?);
    let (depth, colour) = (*data.get(24)?, *data.get(25)?);
    let pix_fmt = match (colour, depth) {
        (0, 1) => Some("monob"),
        (0, 8) => Some("gray"),
        (0, 16) => Some("gray16be"),
        (2, 8) => Some("rgb24"),
        (2, 16) => Some("rgb48be"),
        (3, 1 | 2 | 4 | 8) => Some("pal8"),
        (4, 8) => Some("ya8"),
        (4, 16) => Some("ya16be"),
        (6, 8) => Some("rgba"),
        (6, 16) => Some("rgba64be"),
        _ => None,
    };
    Some(Picture {
        codec: "png",
        width,
        height,
        pix_fmt,
        profile: None,
        bits: None,
        srgb: png_chunk(data, b"sRGB").is_some(),
        aspect: png_chunk(data, b"pHYs").and_then(|p| {
            let x = u32::from_be_bytes(p.get(..4)?.try_into().ok()?);
            let y = u32::from_be_bytes(p.get(4..8)?.try_into().ok()?);
            let g = u32::try_from(crate::compat::gcd(u64::from(x), u64::from(y))).ok()?;
            (x > 0 && y > 0).then(|| (x / g, y / g))
        }),
    })
}

/// The pixel aspect a JFIF header (`APP0`, JFIF 1.02) states by its
/// densities, when both are set.
pub fn jfif_density(data: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2usize;
    while data.get(at) == Some(&0xFF) {
        let marker = *data.get(at + 1)?;
        if matches!(marker, 0xD9 | 0xDA) {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([*data.get(at + 2)?, *data.get(at + 3)?]));
        let segment = data.get(at + 4..at + 2 + len)?;
        if marker == 0xE0 && segment.starts_with(b"JFIF\0") {
            let x = u32::from(u16::from_be_bytes([*segment.get(8)?, *segment.get(9)?]));
            let y = u32::from(u16::from_be_bytes([*segment.get(10)?, *segment.get(11)?]));
            if x == 0 || y == 0 {
                return None;
            }
            let g = crate::compat::gcd(u64::from(x), u64::from(y));
            let g = u32::try_from(g).ok()?;
            return Some((x / g, y / g));
        }
        at += 2 + len;
    }
    None
}

/// The data of a PNG's first chunk of type `kind` before its image data
/// (ISO/IEC 15948 5.3: `sRGB` 11.3.3.5, `pHYs` 11.3.5.3).
fn png_chunk<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut at = 8usize;
    loop {
        let len =
            usize::try_from(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?)).ok()?;
        let name = data.get(at + 4..at + 8)?;
        if name == b"IDAT" || name == b"IEND" {
            return None;
        }
        if name == kind {
            return data.get(at + 8..at.checked_add(8 + len)?);
        }
        at = at.checked_add(12 + len)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jpeg_frame_header() {
        let data = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, // APP0
            0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x30, 0x00, 0x40, 0x03, // SOF0 48x64
            0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01,
        ];
        let p = jpeg(&data).unwrap();
        assert_eq!((p.width, p.height), (64, 48));
        assert_eq!(p.pix_fmt, Some("yuvj420p"));
        assert_eq!(p.profile, Some("Baseline"));
        assert_eq!(jpeg(&[0xFF, 0xD8, 0xFF]), None);
    }

    #[test]
    fn a_png_header() {
        let mut data = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        data.extend_from_slice(&[0, 0, 0, 64, 0, 0, 0, 48, 8, 6, 0, 0, 0]);
        let p = png(&data).unwrap();
        assert_eq!((p.width, p.height, p.pix_fmt), (64, 48, Some("rgba")));
    }
}
