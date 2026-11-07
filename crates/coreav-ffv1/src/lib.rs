//! Lossless FFV1 version 3 encoding of 8-bit planar YCbCr pictures.
//!
//! The bitstream follows RFC 9043. Every frame and slice is independent.
//! Returned packets and caller-owned pictures are outside the encoder's
//! live allocation budget and must be bounded separately by the caller.

#![forbid(unsafe_code)]

mod range;

use coreav_frame::{Chroma, Picture, PixelFormat};
use range::{Buffer, Range};
use std::time::Instant;

/// Fixed input geometry and sample representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Planar YCbCr, depth 8, with 4:2:0, 4:2:2 or 4:4:4 chroma.
    pub format: PixelFormat,
    /// Positive sample aspect ratio, or unknown when absent.
    pub sample_aspect: Option<(u32, u32)>,
}

/// Dimension, work and encoder-local allocation limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Maximum admitted width.
    pub max_width: u32,
    /// Maximum admitted height.
    pub max_height: u32,
    /// Maximum luma pixels per frame.
    pub max_pixels: u64,
    /// Maximum bytes in one returned packet.
    pub max_packet_bytes: usize,
    /// Maximum live encoder-owned bytes, including capacity growth overlap.
    pub max_alloc_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_width: 16_384,
            max_height: 16_384,
            max_pixels: 16_777_216,
            max_packet_bytes: 64 * 1024 * 1024,
            max_alloc_bytes: 128 * 1024 * 1024,
        }
    }
}

/// An encoding refusal or failure. Failed calls never publish partial packets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Settings outside the implemented FFV1 subset.
    Unsupported(&'static str),
    /// The picture does not match the fixed settings.
    InvalidFrame,
    /// A dimension, packet, slice or live allocation bound was reached.
    Limit(&'static str),
    /// The allocator could not satisfy a fallible reservation.
    Allocation,
    /// The absolute deadline has elapsed.
    Timeout,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(reason) => write!(f, "unsupported FFV1 settings: {reason}"),
            Self::InvalidFrame => f.write_str("picture does not match FFV1 settings"),
            Self::Limit(reason) => write!(f, "FFV1 resource limit: {reason}"),
            Self::Allocation => f.write_str("FFV1 allocation failed"),
            Self::Timeout => f.write_str("FFV1 encoding deadline elapsed"),
        }
    }
}

impl std::error::Error for Error {}

const CONTEXTS: usize = 63;
const WORKSPACE: usize = 2 * CONTEXTS * 32 + 3 * 32;

/// A synchronous, all-intra FFV1 version 3 encoder.
pub struct Encoder {
    settings: Settings,
    limits: Limits,
    shifts: (u32, u32),
    grid: [u32; 2],
    configuration: Vec<u8>,
}

impl Encoder {
    /// Validates the fixed format and generates its Configuration Record.
    pub fn new(settings: Settings, limits: Limits) -> Result<Self, Error> {
        let shifts = match settings.format {
            PixelFormat::Yuv {
                chroma: chroma @ (Chroma::Cs420 | Chroma::Cs422 | Chroma::Cs444),
                depth: 8,
            } => chroma.shifts(),
            _ => return Err(Error::Unsupported("8-bit planar YCbCr required")),
        };
        if settings.width == 0 || settings.height == 0 {
            return Err(Error::Unsupported("zero dimensions"));
        }
        if settings.width > limits.max_width
            || settings.height > limits.max_height
            || u64::from(settings.width) * u64::from(settings.height) > limits.max_pixels
        {
            return Err(Error::Limit("dimensions"));
        }
        if matches!(settings.sample_aspect, Some((0, _)) | Some((_, 0))) {
            return Err(Error::Unsupported("zero sample aspect component"));
        }
        let grid = if u64::from(settings.width) * u64::from(settings.height) > 101_376 {
            if settings.width < 2 || settings.height < 2 {
                return Err(Error::Unsupported(
                    "four slices require two rows and columns",
                ));
            }
            [(settings.width, shifts.0), (settings.height, shifts.1)].map(|(size, shift)| {
                (2..=size)
                    .find(|&count| {
                        size % (1 << shift) == 0
                            || (u64::from(size) * u64::from(count - 1) / u64::from(count))
                                % (1 << shift)
                                == 0
                    })
                    .unwrap_or(size)
            })
        } else {
            [1, 1]
        };
        let mut bytes = Buffer::new(usize::MAX, limits.max_alloc_bytes, WORKSPACE);
        let mut coder = Range::new(&mut bytes);
        let mut state = [128; 32];
        for value in [3, 4, 1, 0, 8] {
            coder.unsigned(&mut state, value)?;
        }
        coder.bit(state.first_mut().ok_or(Error::InvalidFrame)?, true)?;
        coder.unsigned(&mut state, shifts.0)?;
        coder.unsigned(&mut state, shifts.1)?;
        coder.bit(state.first_mut().ok_or(Error::InvalidFrame)?, false)?;
        for value in [grid[0] - 1, grid[1] - 1, 1] {
            coder.unsigned(&mut state, value)?;
        }
        for _ in 0..3 {
            let mut quant_state = [128; 32];
            for run in [0, 2, 123] {
                coder.unsigned(&mut quant_state, run)?;
            }
        }
        for _ in 0..2 {
            coder.unsigned(&mut [128; 32], 127)?;
        }
        coder.bit(state.first_mut().ok_or(Error::InvalidFrame)?, false)?;
        coder.unsigned(&mut state, 1)?;
        coder.unsigned(&mut state, 1)?;
        coder.finish()?;
        bytes.parity(0)?;
        Ok(Self {
            settings,
            limits,
            shifts,
            grid,
            configuration: bytes.into_vec(),
        })
    }

    /// Borrows the complete RFC 9043 Configuration Record, including CRC.
    pub fn configuration(&self) -> &[u8] {
        &self.configuration
    }

    /// Encodes one independent frame, checking `deadline` at least every row.
    /// No picture, packet, or adaptive sample state is retained after return.
    pub fn encode(&mut self, picture: &Picture, deadline: Instant) -> Result<Vec<u8>, Error> {
        check_deadline(deadline)?;
        if picture.width() != self.settings.width
            || picture.height() != self.settings.height
            || picture.format() != self.settings.format
        {
            return Err(Error::InvalidFrame);
        }
        let base = WORKSPACE
            .checked_add(self.configuration.capacity())
            .ok_or(Error::Limit("live allocation"))?;
        let mut bytes = Buffer::new(
            self.limits.max_packet_bytes,
            self.limits.max_alloc_bytes,
            base,
        );
        for sy in 0..self.grid[1] {
            for sx in 0..self.grid[0] {
                let start = bytes.len();
                let mut coder = Range::new(&mut bytes);
                if sx == 0 && sy == 0 {
                    coder.bit(&mut 128, true)?;
                }
                let mut header = [128; 32];
                let (sar_num, sar_den) = self.settings.sample_aspect.unwrap_or((0, 0));
                for value in [sx, sy, 0, 0, 0, 0, 3, sar_num, sar_den] {
                    coder.unsigned(&mut header, value)?;
                }
                let mut banks = [[[128; 32]; CONTEXTS]; 2];
                for p in 0..3 {
                    let (hs, vs) = if p == 0 { (0, 0) } else { self.shifts };
                    let x0 = (u64::from(self.settings.width) * u64::from(sx)
                        / u64::from(self.grid[0])) as u32;
                    let y0 = (u64::from(self.settings.height) * u64::from(sy)
                        / u64::from(self.grid[1])) as u32;
                    let x1 = (u64::from(self.settings.width) * u64::from(sx + 1)
                        / u64::from(self.grid[0])) as u32;
                    let y1 = (u64::from(self.settings.height) * u64::from(sy + 1)
                        / u64::from(self.grid[1])) as u32;
                    let region = Region {
                        x: usize::try_from(x0 >> hs).map_err(|_| Error::Limit("dimensions"))?,
                        y: usize::try_from(y0 >> vs).map_err(|_| Error::Limit("dimensions"))?,
                        width: usize::try_from((x1 - x0).div_ceil(1 << hs))
                            .map_err(|_| Error::Limit("dimensions"))?,
                        height: usize::try_from((y1 - y0).div_ceil(1 << vs))
                            .map_err(|_| Error::Limit("dimensions"))?,
                    };
                    let bank = banks
                        .get_mut(usize::from(p != 0))
                        .ok_or(Error::InvalidFrame)?;
                    for y in 0..region.height {
                        check_deadline(deadline)?;
                        let row = picture.row(p, region.y + y).ok_or(Error::InvalidFrame)?;
                        let top = if y == 0 {
                            None
                        } else {
                            Some(
                                picture
                                    .row(p, region.y + y - 1)
                                    .ok_or(Error::InvalidFrame)?,
                            )
                        };
                        let left_top_border = if y < 2 {
                            0
                        } else {
                            i32::from(
                                *picture
                                    .row(p, region.y + y - 2)
                                    .and_then(|r| r.get(region.x))
                                    .ok_or(Error::InvalidFrame)?,
                            )
                        };
                        for x in 0..region.width {
                            let (prediction, context) =
                                neighbors(row, top, &region, x, left_top_border)?;
                            let sample =
                                i32::from(*row.get(region.x + x).ok_or(Error::InvalidFrame)?);
                            let difference = if context < 0 {
                                prediction - sample
                            } else {
                                sample - prediction
                            };
                            let residual = (difference + 128).rem_euclid(256) - 128;
                            let state = bank
                                .get_mut(context.unsigned_abs() as usize)
                                .ok_or(Error::InvalidFrame)?;
                            coder.signed(state, residual)?;
                        }
                    }
                }
                coder.bit(&mut 129, false)?;
                coder.finish()?;
                let size = bytes
                    .len()
                    .checked_sub(start)
                    .ok_or(Error::Limit("slice size"))?;
                if size > 0xff_ffff {
                    return Err(Error::Limit("24-bit slice size"));
                }
                let size = u32::try_from(size).map_err(|_| Error::Limit("slice size"))?;
                for shift in [16, 8, 0] {
                    bytes.push((size >> shift) as u8)?;
                }
                bytes.push(0)?;
                bytes.parity(start)?;
            }
        }
        check_deadline(deadline)?;
        Ok(bytes.into_vec())
    }
}

struct Region {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

fn neighbors(
    row: &[u8],
    top: Option<&[u8]>,
    region: &Region,
    x: usize,
    left_top_border: i32,
) -> Result<(i32, i32), Error> {
    let sample = |r: &[u8], column| {
        r.get(region.x + column)
            .copied()
            .map(i32::from)
            .ok_or(Error::InvalidFrame)
    };
    let t = match top {
        Some(r) => sample(r, x)?,
        None => 0,
    };
    let tr = match top {
        Some(r) => sample(r, (x + 1).min(region.width - 1))?,
        None => 0,
    };
    let l = if x != 0 {
        sample(row, x - 1)?
    } else {
        match top {
            Some(r) => sample(r, 0)?,
            None => 0,
        }
    };
    let tl = if x != 0 {
        match top {
            Some(r) => sample(r, x - 1)?,
            None => 0,
        }
    } else {
        left_top_border
    };
    let prediction = (l + t - tl).clamp(l.min(t), l.max(t));
    let context = quant(l - tl) + 5 * quant(tl - t) + 25 * quant(t - tr);
    Ok((prediction, context))
}

fn quant(difference: i32) -> i32 {
    let folded = (difference + 128).rem_euclid(256) - 128;
    let magnitude = match folded.unsigned_abs() {
        0 => 0,
        1..=3 => 1,
        _ => 2,
    };
    if folded < 0 { -magnitude } else { magnitude }
}

fn check_deadline(deadline: Instant) -> Result<(), Error> {
    if Instant::now() >= deadline {
        Err(Error::Timeout)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
