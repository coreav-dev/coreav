//! A PNG writer (ISO/IEC 15948): 8-bit RGB, one IDAT of a zlib stream
//! (RFC 1950) compressed with `miniz_oxide`, each row with the filter of
//! the smallest sum of absolute differences (the heuristic the PNG
//! specification suggests, 12.8). Its input is CoreAV's own pixels.

use coreav_frame::{Picture, PixelFormat};

use crate::Error;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// The deflate level (`docs/design/thumbnails.md`, "Formats"): about as
/// fast as level 1, with files a few percent larger than level 6's.
const LEVEL: u8 = 3;

/// Encodes an [`PixelFormat::Rgb24`] picture as PNG.
pub fn encode(picture: &Picture) -> Result<Vec<u8>, Error> {
    if picture.format() != PixelFormat::Rgb24 {
        return Err(Error::Encode("PNG input must be RGB".into()));
    }
    let (w, h) = (picture.width(), picture.height());
    let row_bytes = picture.plane_rows(0).map_or(0, |(_, b)| b);
    let mut filtered = Vec::new();
    filtered
        .try_reserve(
            picture
                .packed_len()
                .saturating_add(picture.plane_rows(0).map_or(0, |r| r.0)),
        )
        .map_err(|_| Error::Encode("out of memory".into()))?;
    let zero = vec![0u8; row_bytes];
    let mut previous: &[u8] = &zero;
    let mut candidate = vec![0u8; row_bytes];
    let mut best = vec![0u8; row_bytes];
    for row in picture.rows(0) {
        let mut best_type = 0u8;
        let mut best_cost = u64::MAX;
        for filter in 0..5u8 {
            apply(filter, row, previous, &mut candidate);
            let cost: u64 = candidate
                .iter()
                .map(|b| u64::from(b.unsigned_abs_i8()))
                .sum();
            if cost < best_cost {
                best_cost = cost;
                best_type = filter;
                best.copy_from_slice(&candidate);
            }
        }
        filtered.push(best_type);
        filtered.extend_from_slice(&best);
        previous = row;
    }
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&filtered, LEVEL);
    let mut out = Vec::with_capacity(compressed.len() + 64);
    out.extend_from_slice(&SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &compressed);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

trait AbsI8 {
    fn unsigned_abs_i8(self) -> u8;
}

impl AbsI8 for u8 {
    /// The byte read as a signed difference, made positive.
    fn unsigned_abs_i8(self) -> u8 {
        i8::from_ne_bytes([self]).unsigned_abs()
    }
}

/// Filters `row` with filter type `filter` (12.2, 9.2) into `out`; three
/// bytes per pixel.
fn apply(filter: u8, row: &[u8], previous: &[u8], out: &mut [u8]) {
    for (i, ((o, x), b)) in out.iter_mut().zip(row).zip(previous).enumerate() {
        let a = i
            .checked_sub(3)
            .and_then(|j| row.get(j))
            .copied()
            .unwrap_or(0);
        let c = i
            .checked_sub(3)
            .and_then(|j| previous.get(j))
            .copied()
            .unwrap_or(0);
        let predictor = match filter {
            1 => a,
            2 => *b,
            3 => u8::try_from((u16::from(a) + u16::from(*b)) / 2).unwrap_or(0),
            4 => paeth(a, *b, c),
            _ => 0,
        };
        *o = x.wrapping_sub(predictor);
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let pa = (p - i16::from(a)).abs();
    let pb = (p - i16::from(b)).abs();
    let pc = (p - i16::from(c)).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(out.get(start..).unwrap_or(&[]));
    out.extend_from_slice(&crc.to_be_bytes());
}

/// CRC-32 of ISO 3309 as PNG uses it (annex D).
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_of_iend() {
        // The CRC every PNG ends with.
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn round_trips_through_inflate() {
        let mut p = Picture::zeroed(5, 3, PixelFormat::Rgb24).expect("picture");
        for y in 0..3 {
            for (x, b) in p.row_mut(0, y).expect("row").iter_mut().enumerate() {
                *b = (x * 17 + y * 31) as u8;
            }
        }
        let png = encode(&p).expect("png");
        assert!(png.starts_with(&SIGNATURE));
        // IDAT payload starts after signature (8), IHDR (25), length and type (8).
        let len = u32::from_be_bytes(png[33..37].try_into().expect("len")) as usize;
        let data = &png[41..41 + len];
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(data).expect("inflate");
        assert_eq!(raw.len(), 3 * (1 + 15));
        // Undo the filters and compare.
        let mut prev = vec![0u8; 15];
        for (y, line) in raw.chunks(16).enumerate() {
            let mut out = vec![0u8; 15];
            for i in 0..15 {
                let a = if i >= 3 { out[i - 3] } else { 0 };
                let b = prev[i];
                let c = if i >= 3 { prev[i - 3] } else { 0 };
                let pred = match line[0] {
                    1 => a,
                    2 => b,
                    3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                    4 => paeth(a, b, c),
                    _ => 0,
                };
                out[i] = line[1 + i].wrapping_add(pred);
            }
            assert_eq!(Some(out.as_slice()), p.row(0, y));
            prev = out;
        }
    }
}
