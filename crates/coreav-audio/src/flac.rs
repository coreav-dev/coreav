//! FLAC decoding, written from RFC 9639.
//!
//! [`StreamInfo`] reads the streaminfo metadata block from the codec
//! configuration as Matroska (`fLaC` and the metadata blocks, RFC 9639
//! 10.2) and MP4 (the `dfLa` box, the FLAC-in-ISOBMFF mapping) carry it, or
//! from its 34-byte body. [`FlacDecoder`] decodes one frame per packet into
//! planar integer samples: frame header (9.1), subframes (9.2) with their
//! wasted bits, the five fixed predictors, linear predictors up to order
//! 32, partitioned Rice residuals with escapes, interchannel decorrelation
//! (4.2), and the frame footer's CRC-16 (9.3).
//!
//! Choices, beyond what the RFC fixes:
//! - A frame whose channel count, sample rate or bit depth differs from the
//!   streaminfo block is refused (`Error::Unsupported`): the output of a
//!   stream has one format (RFC 9639 9 lets a decoder stop there).
//! - A decoded sample outside its subframe's bit depth is an error, so all
//!   arithmetic stays within its proven range (Appendix A).
//! - A CRC-8 mismatch in the frame header is an error; a CRC-16 mismatch is
//!   reported in [`Frame::crc_ok`] and the samples are kept, as the bitstream
//!   parsed.
//! - Side subframes of 32-bit audio need 33 bits (4.2); they are decoded
//!   with 64-bit storage, the only case that needs it.

use crate::bits::Bits;
use crate::crc::{crc8, crc16};
use crate::limits::check_channels;
use crate::{Error, Limits, Pcm};

/// The streaminfo metadata block (RFC 9639 8.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    /// Smallest block size in samples, the last block excepted.
    pub min_block_size: u16,
    /// Largest block size in samples.
    pub max_block_size: u16,
    /// Smallest frame in bytes, 0 when unknown.
    pub min_frame_size: u32,
    /// Largest frame in bytes, 0 when unknown.
    pub max_frame_size: u32,
    /// Sample rate in Hz, never 0.
    pub sample_rate: u32,
    /// Channels, 1 to 8.
    pub channels: u8,
    /// Bits per sample, 4 to 32.
    pub bits_per_sample: u8,
    /// Samples per channel in the stream, 0 when unknown.
    pub total_samples: u64,
    /// MD5 of the decoded samples, all zero when unknown.
    pub md5: [u8; 16],
}

impl StreamInfo {
    /// Bytes of the block's body.
    pub const LEN: usize = 34;

    /// Reads the block's 34-byte body (bytes after it are ignored).
    pub fn parse(body: &[u8]) -> Result<Self, Error> {
        let body = body
            .get(..Self::LEN)
            .ok_or(Error::Invalid("streaminfo block too short"))?;
        let mut bits = Bits::new(body);
        let min_block_size = bits.read(16)? as u16;
        let max_block_size = bits.read(16)? as u16;
        let min_frame_size = bits.read(24)?;
        let max_frame_size = bits.read(24)?;
        let sample_rate = bits.read(20)?;
        let channels = bits.read(3)? as u8 + 1;
        let bits_per_sample = bits.read(5)? as u8 + 1;
        let total_samples = bits.read_u64(36)?;
        let mut md5 = [0u8; 16];
        for byte in &mut md5 {
            *byte = bits.read(8)? as u8;
        }
        if sample_rate == 0 {
            return Err(Error::Unsupported(
                "FLAC stream without a sample rate (non-audio)",
            ));
        }
        if bits_per_sample < 4 {
            return Err(Error::Invalid("bits per sample below 4"));
        }
        Ok(Self {
            min_block_size,
            max_block_size,
            min_frame_size,
            max_frame_size,
            sample_rate,
            channels,
            bits_per_sample,
            total_samples,
            md5,
        })
    }

    /// Reads the streaminfo block from a sequence of metadata blocks
    /// (RFC 9639 8.1), whose first block it must be.
    pub fn from_metadata_blocks(blocks: &[u8]) -> Result<Self, Error> {
        let header = blocks
            .get(..4)
            .ok_or(Error::Invalid("metadata block header too short"))?;
        let [kind, a, b, c] = <[u8; 4]>::try_from(header).map_err(|_| Error::Truncated)?;
        if kind & 0x7F != 0 {
            return Err(Error::Invalid("the first metadata block is not streaminfo"));
        }
        let len = u32::from_be_bytes([0, a, b, c]) as usize;
        if len < Self::LEN {
            return Err(Error::Invalid("streaminfo block too short"));
        }
        Self::parse(blocks.get(4..).unwrap_or_default())
    }

    /// Reads the configuration a container carries: Matroska's
    /// `CodecPrivate` (`fLaC` and the metadata blocks), the payload of an
    /// MP4 `dfLa` box (version and flags, then the metadata blocks), or a
    /// bare 34-byte streaminfo body (the raw and Ogg demuxers).
    pub fn from_config(config: &[u8]) -> Result<Self, Error> {
        if let Some(rest) = config.strip_prefix(b"fLaC") {
            return Self::from_metadata_blocks(rest);
        }
        if config.len() == Self::LEN {
            return Self::parse(config);
        }
        match config.split_first() {
            Some((0, rest)) if rest.len() >= 3 => {
                Self::from_metadata_blocks(rest.get(3..).unwrap_or_default())
            }
            Some((0, _)) | None => Err(Error::Invalid("FLAC configuration too short")),
            Some(_) => Err(Error::Unsupported("dfLa box version")),
        }
    }
}

/// How the channels of a frame are coded (RFC 9639 Table 16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Assignment {
    /// 1 to 8 channels coded independently.
    Independent(u8),
    /// Left and side.
    LeftSide,
    /// Side and right.
    SideRight,
    /// Mid and side.
    MidSide,
}

impl Assignment {
    fn channels(self) -> u8 {
        match self {
            Self::Independent(n) => n,
            _ => 2,
        }
    }

    /// Whether subframe `index` codes a side channel, one bit wider.
    fn is_side(self, index: usize) -> bool {
        matches!(
            (self, index),
            (Self::LeftSide, 1) | (Self::SideRight, 0) | (Self::MidSide, 1)
        )
    }
}

/// A frame header (RFC 9639 9.1).
#[derive(Clone, Copy, Debug)]
struct Header {
    variable: bool,
    block_size: u32,
    sample_rate: Option<u32>,
    assignment: Assignment,
    bits: Option<u32>,
    number: u64,
}

/// What [`FlacDecoder::decode`] read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Samples per channel.
    pub block_size: u32,
    /// The coded number: the frame number of a fixed block size stream, the
    /// first sample's number of a variable one.
    pub number: u64,
    /// Whether the stream has a variable block size.
    pub variable: bool,
    /// Bytes of the frame, footer included.
    pub bytes: usize,
    /// Whether the frame's CRC-16 matched.
    pub crc_ok: bool,
}

/// A FLAC decoder for one stream.
#[derive(Clone, Debug)]
pub struct FlacDecoder {
    info: StreamInfo,
    /// Scratch for a 33-bit side subframe.
    wide: Vec<i64>,
}

impl FlacDecoder {
    /// A decoder for the stream `info` describes.
    pub fn new(info: StreamInfo, limits: &Limits) -> Result<Self, Error> {
        check_channels(usize::from(info.channels), limits.max_channels)?;
        Ok(Self {
            info,
            wide: Vec::new(),
        })
    }

    /// The stream's streaminfo block.
    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// Decodes the frame at the start of `data` into `out`, as integers of
    /// the stream's bit depth.
    pub fn decode(&mut self, data: &[u8], out: &mut Pcm) -> Result<Frame, Error> {
        let (header, header_len) = parse_header(data)?;
        let channels = header.assignment.channels();
        if channels != self.info.channels {
            return Err(Error::Unsupported(
                "FLAC frame with another channel count than the stream",
            ));
        }
        if header
            .sample_rate
            .is_some_and(|rate| rate != self.info.sample_rate)
        {
            return Err(Error::Unsupported(
                "FLAC frame with another sample rate than the stream",
            ));
        }
        let bits_per_sample = u32::from(self.info.bits_per_sample);
        if header.bits.is_some_and(|bits| bits != bits_per_sample) {
            return Err(Error::Unsupported(
                "FLAC frame with another bit depth than the stream",
            ));
        }
        let block_size = header.block_size as usize;
        let samples = out.int_buffer(usize::from(channels), block_size, bits_per_sample);
        let mut bits = Bits::new(data.get(header_len..).unwrap_or_default());
        let mut wide_channel = None;
        for (index, plane) in samples.chunks_exact_mut(block_size.max(1)).enumerate() {
            let side = header.assignment.is_side(index);
            let depth = bits_per_sample + u32::from(side);
            if depth > 32 {
                self.wide.clear();
                self.wide.resize(block_size, 0);
                subframe_wide(&mut bits, &mut self.wide, depth)?;
                wide_channel = Some(index);
            } else {
                subframe(&mut bits, plane, depth)?;
            }
        }
        decorrelate(
            header.assignment,
            samples,
            block_size,
            wide_channel.map(|i| (i, self.wide.as_slice())),
        )?;
        bits.align();
        let body = header_len
            .checked_add(bits.byte_position())
            .ok_or(Error::Truncated)?;
        let footer = data
            .get(body..body.saturating_add(2))
            .ok_or(Error::Truncated)?;
        let stored = u16::from_be_bytes([
            footer.first().copied().unwrap_or(0),
            footer.get(1).copied().unwrap_or(0),
        ]);
        let crc_ok = crc16(data.get(..body).unwrap_or_default()) == stored;
        Ok(Frame {
            block_size: header.block_size,
            number: header.number,
            variable: header.variable,
            bytes: body + 2,
            crc_ok,
        })
    }
}

/// The block size of the frame at the start of `data`, from its header,
/// without decoding it; `None` when the header is not valid.
pub fn frame_samples(data: &[u8]) -> Option<u32> {
    parse_header(data).ok().map(|(header, _)| header.block_size)
}

/// Reads the frame header at the start of `data`; returns it and its
/// length in bytes, CRC-8 included.
fn parse_header(data: &[u8]) -> Result<(Header, usize), Error> {
    let fixed = data.get(..4).ok_or(Error::Truncated)?;
    let [b0, b1, b2, b3] = <[u8; 4]>::try_from(fixed).map_err(|_| Error::Truncated)?;
    if b0 != 0xFF || b1 & 0xFE != 0xF8 {
        return Err(Error::Invalid("no FLAC frame sync code"));
    }
    let variable = b1 & 1 == 1;
    let size_bits = b2 >> 4;
    let rate_bits = b2 & 0x0F;
    let assignment = match b3 >> 4 {
        n @ 0..=7 => Assignment::Independent(n + 1),
        8 => Assignment::LeftSide,
        9 => Assignment::SideRight,
        10 => Assignment::MidSide,
        _ => return Err(Error::Invalid("reserved channel assignment")),
    };
    let bits = match (b3 >> 1) & 0x07 {
        0 => None,
        1 => Some(8),
        2 => Some(12),
        3 => return Err(Error::Invalid("reserved bit depth")),
        4 => Some(16),
        5 => Some(20),
        6 => Some(24),
        _ => Some(32),
    };
    // The coded number: UTF-8-like, up to 7 bytes (9.1.5).
    let mut at = 4;
    let first = *data.get(at).ok_or(Error::Truncated)?;
    at += 1;
    let extra = match first.leading_ones() {
        0 => 0,
        n @ 2..=7 => n - 1,
        _ => return Err(Error::Invalid("malformed coded number")),
    };
    // A first byte of n leading ones (n >= 2) keeps 7 - n value bits.
    let mut number = if extra == 0 {
        u64::from(first)
    } else {
        u64::from(first & (0x7F >> (extra + 1)))
    };
    for _ in 0..extra {
        let byte = *data.get(at).ok_or(Error::Truncated)?;
        at += 1;
        if byte & 0xC0 != 0x80 {
            return Err(Error::Invalid("malformed coded number"));
        }
        number = (number << 6) | u64::from(byte & 0x3F);
    }
    if !variable && number > u64::from(u32::MAX >> 1) {
        return Err(Error::Invalid("frame number above 31 bits"));
    }
    let block_size = match size_bits {
        0 => return Err(Error::Invalid("reserved block size")),
        1 => 192,
        2..=5 => 144 << size_bits,
        6 => {
            let byte = *data.get(at).ok_or(Error::Truncated)?;
            at += 1;
            u32::from(byte) + 1
        }
        7 => {
            let pair = data.get(at..at + 2).ok_or(Error::Truncated)?;
            at += 2;
            let value = u32::from(u16::from_be_bytes([
                pair.first().copied().unwrap_or(0),
                pair.get(1).copied().unwrap_or(0),
            ]));
            if value == 0xFFFF {
                return Err(Error::Invalid("block size 65536"));
            }
            value + 1
        }
        n => 1 << n,
    };
    let sample_rate = match rate_bits {
        0 => None,
        1 => Some(88_200),
        2 => Some(176_400),
        3 => Some(192_000),
        4 => Some(8_000),
        5 => Some(16_000),
        6 => Some(22_050),
        7 => Some(24_000),
        8 => Some(32_000),
        9 => Some(44_100),
        10 => Some(48_000),
        11 => Some(96_000),
        12 => {
            let byte = *data.get(at).ok_or(Error::Truncated)?;
            at += 1;
            Some(u32::from(byte) * 1000)
        }
        13 | 14 => {
            let pair = data.get(at..at + 2).ok_or(Error::Truncated)?;
            at += 2;
            let value = u32::from(u16::from_be_bytes([
                pair.first().copied().unwrap_or(0),
                pair.get(1).copied().unwrap_or(0),
            ]));
            Some(if rate_bits == 14 { value * 10 } else { value })
        }
        _ => return Err(Error::Invalid("forbidden sample rate bits")),
    };
    let crc = *data.get(at).ok_or(Error::Truncated)?;
    if crc8(data.get(..at).unwrap_or_default()) != crc {
        return Err(Error::Invalid("frame header CRC-8 mismatch"));
    }
    Ok((
        Header {
            variable,
            block_size,
            sample_rate,
            assignment,
            bits,
            number,
        },
        at + 1,
    ))
}

/// What a subframe header says (RFC 9639 9.2.1).
enum Kind {
    Constant,
    Verbatim,
    Fixed(usize),
    Lpc(usize),
}

/// Reads a subframe header; returns the kind and the wasted bits.
fn subframe_header(bits: &mut Bits, depth: u32) -> Result<(Kind, u32), Error> {
    if bits.bit()? {
        return Err(Error::Invalid("subframe padding bit set"));
    }
    let kind = match bits.read(6)? {
        0 => Kind::Constant,
        1 => Kind::Verbatim,
        n @ 8..=12 => Kind::Fixed((n - 8) as usize),
        n @ 32..=63 => Kind::Lpc((n - 31) as usize),
        _ => return Err(Error::Invalid("reserved subframe type")),
    };
    let wasted = if bits.bit()? {
        // k - 1 in unary; k must leave at least one bit.
        bits.unary(depth.saturating_sub(2))? + 1
    } else {
        0
    };
    Ok((kind, wasted))
}

/// Decodes one subframe of at most 32 bits per sample into `out`.
fn subframe(bits: &mut Bits, out: &mut [i32], depth: u32) -> Result<(), Error> {
    let (kind, wasted) = subframe_header(bits, depth)?;
    let width = depth - wasted;
    match kind {
        Kind::Constant => {
            let value = bits.signed(width)?;
            out.fill(value);
        }
        Kind::Verbatim => {
            for slot in out.iter_mut() {
                *slot = bits.signed(width)?;
            }
        }
        Kind::Fixed(order) => {
            warm_up(bits, out, order, width)?;
            residual(bits, out, order)?;
            fixed(out, order, width)?;
        }
        Kind::Lpc(order) => {
            warm_up(bits, out, order, width)?;
            let precision = bits.read(4)? + 1;
            if precision == 16 {
                return Err(Error::Invalid("forbidden coefficient precision"));
            }
            let shift = bits.signed(5)?;
            let shift =
                u32::try_from(shift).map_err(|_| Error::Invalid("negative prediction shift"))?;
            let mut coefficients = [0i32; 32];
            for slot in coefficients.iter_mut().take(order) {
                *slot = bits.signed(precision)?;
            }
            residual(bits, out, order)?;
            lpc(
                out,
                coefficients.get(..order).unwrap_or_default(),
                shift,
                width,
            )?;
        }
    }
    if wasted > 0 {
        // `width + wasted = depth <= 32` and every sample fits `width`
        // bits, so the shift cannot overflow.
        for sample in out.iter_mut() {
            *sample <<= wasted;
        }
    }
    Ok(())
}

/// Reads `order` unencoded warm-up samples.
fn warm_up(bits: &mut Bits, out: &mut [i32], order: usize, width: u32) -> Result<(), Error> {
    if order > out.len() {
        return Err(Error::Invalid("predictor order above the block size"));
    }
    for slot in out.iter_mut().take(order) {
        *slot = bits.signed(width)?;
    }
    Ok(())
}

/// Reads the coded residual of samples `order..` (RFC 9639 9.2.7).
fn residual(bits: &mut Bits, out: &mut [i32], order: usize) -> Result<(), Error> {
    let parameter_bits = match bits.read(2)? {
        0 => 4,
        1 => 5,
        _ => return Err(Error::Invalid("reserved residual coding method")),
    };
    let escape = (1 << parameter_bits) - 1;
    let partition_order = bits.read(4)?;
    let block_size = out.len();
    let partition_len = block_size >> partition_order;
    if partition_len << partition_order != block_size || partition_len < order {
        return Err(Error::Invalid("partition order does not fit the block"));
    }
    let mut start = order;
    for partition in 0..(1usize << partition_order) {
        let end = (partition + 1) * partition_len;
        let slots = out
            .get_mut(start..end)
            .ok_or(Error::Invalid("partition out of range"))?;
        start = end;
        let parameter = bits.read(parameter_bits)?;
        if parameter == escape {
            let width = bits.read(5)?;
            for slot in slots.iter_mut() {
                *slot = bits.signed(width)?;
            }
        } else {
            bits.rice(parameter, slots)?;
        }
    }
    Ok(())
}

/// Whether `value` fits `width` bits, two's complement.
#[inline(always)]
fn fits(value: i64, width: u32) -> bool {
    let half = 1i64 << (width - 1);
    (-half..half).contains(&value)
}

/// Adds the fixed prediction of `order` to the residuals at `order..`
/// (RFC 9639 Table 20).
fn fixed(out: &mut [i32], order: usize, width: u32) -> Result<(), Error> {
    let mut iter = out.iter_mut();
    let mut history = [0i64; 4];
    for slot in history.iter_mut().take(order) {
        *slot = iter.next().map_or(0, |s| i64::from(*s));
    }
    // history[0] is the oldest sample. The history holds the samples as
    // stored, 32-bit, so a stream whose samples leave their bit depth (an
    // error, reported below) cannot make the predictions grow: every sum
    // stays below 2^35.
    let [mut a, mut b, mut c, mut d] = history;
    let mut ok = true;
    match order {
        0 => {}
        1 => {
            for slot in iter {
                let v = i64::from(*slot) + a;
                ok &= fits(v, width);
                *slot = v as i32;
                a = i64::from(*slot);
            }
        }
        2 => {
            for slot in iter {
                let v = i64::from(*slot) + 2 * b - a;
                ok &= fits(v, width);
                *slot = v as i32;
                a = b;
                b = i64::from(*slot);
            }
        }
        3 => {
            for slot in iter {
                let v = i64::from(*slot) + 3 * c - 3 * b + a;
                ok &= fits(v, width);
                *slot = v as i32;
                a = b;
                b = c;
                c = i64::from(*slot);
            }
        }
        _ => {
            for slot in iter {
                let v = i64::from(*slot) + 4 * d - 6 * c + 4 * b - a;
                ok &= fits(v, width);
                *slot = v as i32;
                a = b;
                b = c;
                c = d;
                d = i64::from(*slot);
            }
        }
    }
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(
            "decoded sample outside the subframe's bit depth",
        ))
    }
}

/// Adds the linear prediction to the residuals at `coefficients.len()..`
/// (RFC 9639 9.2.6). `coefficients[0]` multiplies the sample right before
/// the predicted one.
fn lpc(out: &mut [i32], coefficients: &[i32], shift: u32, width: u32) -> Result<(), Error> {
    // Oldest first, to line up with the window of past samples.
    let mut reversed = [0i64; 32];
    for (slot, &c) in reversed.iter_mut().zip(coefficients.iter().rev()) {
        *slot = i64::from(c);
    }
    macro_rules! orders {
        ($($n:literal)*) => {
            match coefficients.len() {
                $($n => lpc_order::<$n>(out, &reversed, shift, width),)*
                _ => Err(Error::Invalid("predictor order")),
            }
        };
    }
    orders!(1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32)
}

/// [`lpc`] for one order, so that the dot product is unrolled.
#[inline(always)]
fn lpc_order<const N: usize>(
    out: &mut [i32],
    reversed: &[i64; 32],
    shift: u32,
    width: u32,
) -> Result<(), Error> {
    let mut coefficients = [0i64; N];
    for (slot, &c) in coefficients.iter_mut().zip(reversed) {
        *slot = c;
    }
    let mut ok = true;
    for n in N..out.len() {
        let Some(window) = out.get_mut(n - N..=n) else {
            break;
        };
        let Some((slot, history)) = window.split_last_mut() else {
            break;
        };
        let Ok(history) = <&[i32; N]>::try_from(&*history) else {
            break;
        };
        // |sample| < 2^32 and |coefficient| < 2^15, so 32 products sum
        // below 2^52.
        let mut prediction = 0i64;
        for (&s, &c) in history.iter().zip(&coefficients) {
            prediction += i64::from(s) * c;
        }
        let v = i64::from(*slot) + (prediction >> shift);
        ok &= fits(v, width);
        *slot = v as i32;
    }
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(
            "decoded sample outside the subframe's bit depth",
        ))
    }
}

/// Decodes a subframe of 33 bits per sample (the side channel of 32-bit
/// audio) into `out`.
fn subframe_wide(bits: &mut Bits, out: &mut [i64], depth: u32) -> Result<(), Error> {
    let (kind, wasted) = subframe_header(bits, depth)?;
    let width = depth - wasted;
    let mut residuals = vec![0i32; out.len()];
    match kind {
        Kind::Constant => {
            let value = bits.signed_wide(width)?;
            out.fill(value);
        }
        Kind::Verbatim => {
            for slot in out.iter_mut() {
                *slot = bits.signed_wide(width)?;
            }
        }
        Kind::Fixed(order) | Kind::Lpc(order) => {
            if order > out.len() {
                return Err(Error::Invalid("predictor order above the block size"));
            }
            for slot in out.iter_mut().take(order) {
                *slot = bits.signed_wide(width)?;
            }
            let (coefficients, shift) = if let Kind::Lpc(_) = kind {
                let precision = bits.read(4)? + 1;
                if precision == 16 {
                    return Err(Error::Invalid("forbidden coefficient precision"));
                }
                let shift = u32::try_from(bits.signed(5)?)
                    .map_err(|_| Error::Invalid("negative prediction shift"))?;
                let mut coefficients = vec![0i64; order];
                for slot in &mut coefficients {
                    *slot = i64::from(bits.signed(precision)?);
                }
                (coefficients, shift)
            } else {
                let fixed: &[i64] = match order {
                    0 => &[],
                    1 => &[1],
                    2 => &[2, -1],
                    3 => &[3, -3, 1],
                    _ => &[4, -6, 4, -1],
                };
                (fixed.to_vec(), 0)
            };
            residual(bits, &mut residuals, order)?;
            for n in order..out.len() {
                let Some((history, rest)) = out.split_at_mut_checked(n) else {
                    break;
                };
                // |sample| < 2^33, |coefficient| < 2^15: below 2^53 for 32.
                let prediction: i64 = history
                    .iter()
                    .rev()
                    .zip(&coefficients)
                    .map(|(&s, &c)| s * c)
                    .sum();
                let r = residuals.get(n).copied().unwrap_or(0);
                let v = i64::from(r) + (prediction >> shift);
                if !fits(v, width) {
                    return Err(Error::Invalid(
                        "decoded sample outside the subframe's bit depth",
                    ));
                }
                if let Some(slot) = rest.first_mut() {
                    *slot = v;
                }
            }
        }
    }
    for sample in out.iter_mut() {
        *sample <<= wasted;
    }
    Ok(())
}

/// Undoes interchannel decorrelation (RFC 9639 4.2). `wide` is the
/// 33-bit side subframe of 32-bit audio, which `samples` does not hold.
fn decorrelate(
    assignment: Assignment,
    samples: &mut [i32],
    block_size: usize,
    wide: Option<(usize, &[i64])>,
) -> Result<(), Error> {
    if let Assignment::Independent(_) = assignment {
        return Ok(());
    }
    let (first, second) = samples
        .split_at_mut_checked(block_size)
        .ok_or(Error::Invalid("stereo frame without two channels"))?;
    match (assignment, wide) {
        (Assignment::LeftSide, None) => {
            for (left, side) in first.iter().zip(second.iter_mut()) {
                *side = clamp(i64::from(*left) - i64::from(*side));
            }
        }
        (Assignment::SideRight, None) => {
            for (side, right) in first.iter_mut().zip(second.iter()) {
                *side = clamp(i64::from(*side) + i64::from(*right));
            }
        }
        (Assignment::MidSide, None) => {
            for (mid, side) in first.iter_mut().zip(second.iter_mut()) {
                let s = i64::from(*side);
                let m = (i64::from(*mid) << 1) | (s & 1);
                *mid = clamp((m + s) >> 1);
                *side = clamp((m - s) >> 1);
            }
        }
        (Assignment::LeftSide, Some((_, wide))) => {
            for ((left, side), &s) in first.iter().zip(second.iter_mut()).zip(wide) {
                *side = clamp(i64::from(*left) - s);
            }
        }
        (Assignment::SideRight, Some((_, wide))) => {
            for ((side, right), &s) in first.iter_mut().zip(second.iter()).zip(wide) {
                *side = clamp(s + i64::from(*right));
            }
        }
        (Assignment::MidSide, Some((_, wide))) => {
            for ((mid, side), &s) in first.iter_mut().zip(second.iter_mut()).zip(wide) {
                let m = (i64::from(*mid) << 1) | (s & 1);
                *mid = clamp((m + s) >> 1);
                *side = clamp((m - s) >> 1);
            }
        }
        (Assignment::Independent(_), _) => {}
    }
    Ok(())
}

/// Narrows a decorrelated sample; only an invalid stream goes past 32 bits.
#[inline(always)]
fn clamp(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Residuals that drive the samples far outside their bit depth fail
    /// the frame without the predictions overflowing (a debug build checks
    /// every operation).
    #[test]
    fn runaway_fixed_predictions_fail_without_overflow() {
        for order in 1..=4 {
            let mut out = vec![(1 << 30) - 1; 4096];
            assert!(fixed(&mut out, order, 16).is_err(), "order {order}");
            let mut out = vec![-(1 << 30); 4096];
            assert!(fixed(&mut out, order, 32).is_err(), "order {order}");
        }
        let mut out = vec![(1 << 30) - 1; 4096];
        let coefficients = [(1 << 14) - 1; 32];
        assert!(lpc(&mut out, &coefficients, 0, 16).is_err());
    }
}
