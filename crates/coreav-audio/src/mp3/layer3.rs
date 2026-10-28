//! Layer III granule decoding (ISO/IEC 11172-3 2.4.2.7 and 2.4.3.4, ISO/IEC
//! 13818-3 2.4.3.2): side information, scalefactors, requantization, short
//! block reordering, MS and intensity stereo, alias reduction and the IMDCT
//! with its windows and overlap.

use super::huffman::{self, MainBits};
use super::lanes::F4;
use super::synth::MAX_SLOTS;
use super::synth::Synthesis;
use super::tables;
use super::trig;
use super::{Header, Version};
use crate::Error;
use std::sync::OnceLock;

/// One granule and channel of side information.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GranuleInfo {
    pub(crate) part2_3_length: u32,
    pub(crate) big_values: u32,
    pub(crate) global_gain: u32,
    pub(crate) scalefac_compress: u32,
    pub(crate) window_switching: bool,
    pub(crate) block_type: u32,
    pub(crate) mixed: bool,
    pub(crate) table_select: [u32; 3],
    pub(crate) subblock_gain: [u32; 3],
    pub(crate) region0_count: u32,
    pub(crate) region1_count: u32,
    pub(crate) preflag: bool,
    pub(crate) scalefac_scale: bool,
    pub(crate) count1_table_b: bool,
}

/// The side information of a frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SideInfo {
    pub(crate) main_data_begin: u16,
    pub(crate) scfsi: [[bool; 4]; 2],
    pub(crate) granules: [[GranuleInfo; 2]; 2],
}

impl SideInfo {
    /// Reads the side information (Table 4 of the Fraunhofer document,
    /// ISO/IEC 11172-3 2.4.1.7).
    pub(crate) fn parse(data: &[u8], header: &Header) -> Result<Self, Error> {
        let mut bits = crate::bits::Bits::new(data);
        let mpeg1 = header.version == Version::Mpeg1;
        let channels = header.channels();
        let mut info = Self {
            main_data_begin: bits.read(if mpeg1 { 9 } else { 8 })? as u16,
            ..Self::default()
        };
        let private = match (mpeg1, channels) {
            (true, 1) => 5,
            (true, _) => 3,
            (false, 1) => 1,
            (false, _) => 2,
        };
        bits.read(private)?;
        if mpeg1 {
            for scfsi in info.scfsi.iter_mut().take(channels) {
                for band in scfsi.iter_mut() {
                    *band = bits.bit()?;
                }
            }
        }
        for granule in info.granules.iter_mut().take(header.granules()) {
            for gr in granule.iter_mut().take(channels) {
                gr.part2_3_length = bits.read(12)?;
                gr.big_values = bits.read(9)?;
                if gr.big_values > 288 {
                    return Err(Error::Invalid("big_values above 288"));
                }
                gr.global_gain = bits.read(8)?;
                gr.scalefac_compress = bits.read(if mpeg1 { 4 } else { 9 })?;
                gr.window_switching = bits.bit()?;
                if gr.window_switching {
                    gr.block_type = bits.read(2)?;
                    if gr.block_type == 0 {
                        return Err(Error::Invalid("window switching with block type 0"));
                    }
                    gr.mixed = bits.bit()?;
                    for table in gr.table_select.iter_mut().take(2) {
                        *table = bits.read(5)?;
                    }
                    for gain in &mut gr.subblock_gain {
                        *gain = bits.read(3)?;
                    }
                    // Implicit region counts (2.4.2.7): region1 holds the
                    // rest of the big values.
                    gr.region0_count = if gr.block_type == 2 && !gr.mixed {
                        8
                    } else {
                        7
                    };
                    gr.region1_count = 20 - gr.region0_count;
                } else {
                    for table in &mut gr.table_select {
                        *table = bits.read(5)?;
                    }
                    gr.region0_count = bits.read(4)?;
                    gr.region1_count = bits.read(3)?;
                }
                if mpeg1 {
                    gr.preflag = bits.bit()?;
                }
                gr.scalefac_scale = bits.bit()?;
                gr.count1_table_b = bits.bit()?;
            }
        }
        Ok(info)
    }
}

/// What persists from granule to granule for one channel: the IMDCT
/// overlap ([time][subband]) and the scalefactors (for `scfsi`).
#[derive(Clone)]
pub(crate) struct Channel {
    overlap: [[f32; 32]; 18],
    scalefac_l: [u8; 22],
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            overlap: [[0.0; 32]; 18],
            scalefac_l: [0; 22],
        }
    }
}

/// The working buffers of one granule.
#[derive(Clone)]
pub(crate) struct Granule {
    /// Requantized lines per channel.
    xr: [[f32; 576]; 2],
    /// Short block scalefactors per channel: [sfb][window].
    scalefac_s: [[[u8; 3]; 13]; 2],
    /// The long scalefactors of the granule per channel.
    scalefac_l: [[u8; 22]; 2],
    /// The bits of each LSF scalefactor (for the illegal intensity
    /// position): long and short bands, per channel.
    slen_l: [[u8; 22]; 2],
    slen_s: [[u8; 13]; 2],
    /// One past the last nonzero line per channel.
    nonzero: [usize; 2],
    /// The intensity_scale of the right channel (MPEG-2).
    intensity_scale: bool,
    /// Subband samples after the IMDCT per channel, for the whole frame:
    /// [subband][slot], granule 0 in slots 0..18 and granule 1 after it.
    subband: [[[f32; MAX_SLOTS]; 32]; 2],
}

impl Default for Granule {
    fn default() -> Self {
        Self {
            xr: [[0.0; 576]; 2],
            scalefac_s: [[[0; 3]; 13]; 2],
            scalefac_l: [[0; 22]; 2],
            slen_l: [[0; 22]; 2],
            slen_s: [[0; 13]; 2],
            nonzero: [0; 2],
            intensity_scale: false,
            subband: [[[0.0; MAX_SLOTS]; 32]; 2],
        }
    }
}

/// The scalefactor band widths of a sample rate.
pub(crate) struct Bands {
    pub(crate) long: &'static [u16; 22],
    pub(crate) short: &'static [u16; 13],
}

pub(crate) fn bands(sample_rate: u32) -> Result<Bands, Error> {
    let index = tables::BAND_RATES
        .iter()
        .position(|&r| r == sample_rate)
        .ok_or(Error::Invalid("sample rate without scalefactor bands"))?;
    Ok(Bands {
        long: tables::LONG_BANDS
            .get(index)
            .ok_or(Error::Invalid("bands"))?,
        short: tables::SHORT_BANDS
            .get(index)
            .ok_or(Error::Invalid("bands"))?,
    })
}

/// Decodes every granule of a frame into `planes` (planar, `samples` per
/// channel).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_frame(
    header: &Header,
    info: &SideInfo,
    main: &[u8],
    available_from: usize,
    state: &mut [Channel; 2],
    synthesis: &mut [Synthesis; 2],
    granule: &mut Granule,
    planes: &mut [f32],
    samples: usize,
) -> Result<(), Error> {
    let bands = bands(header.sample_rate)?;
    let channels = header.channels();
    let mut bits = MainBits::new(main);
    for (gr, granule_info) in info.granules.iter().enumerate().take(header.granules()) {
        // A granule whose data starts before the bytes the decoder has
        // seen is silent: its lines are zero, and the filter banks go on.
        let missing = bits.position() < available_from;
        for ch in 0..channels {
            let gi = granule_info.get(ch).ok_or(Error::Invalid("channel"))?;
            let start = bits.position();
            let end = start.saturating_add(gi.part2_3_length as usize);
            if end > main.len().saturating_mul(8) {
                return Err(Error::Invalid("part2_3_length past the main data"));
            }
            if missing {
                if let Some(xr) = granule.xr.get_mut(ch) {
                    *xr = [0.0; 576];
                }
                if let Some(slot) = granule.nonzero.get_mut(ch) {
                    *slot = 0;
                }
                // A silent granule's scalefactors are zero: the next
                // granule's `scfsi` reuses them, never an older frame's.
                if let Some(channel) = state.get_mut(ch) {
                    channel.scalefac_l = [0; 22];
                }
                bits.seek(end);
                continue;
            }
            if header.version == Version::Mpeg1 {
                scalefactors_mpeg1(&mut bits, gi, info, gr, ch, state, granule)?;
            } else {
                scalefactors_lsf(&mut bits, gi, header, ch, granule)?;
            }
            let gains = gains(gi, header, &bands, ch, granule)?;
            let xr = granule.xr.get_mut(ch).ok_or(Error::Invalid("channel"))?;
            let mpeg1 = header.version == Version::Mpeg1;
            let nonzero = huffman::decode(&mut bits, gi, &bands, end, mpeg1, gains.runs(), xr)?;
            if let Some(slot) = granule.nonzero.get_mut(ch) {
                *slot = nonzero;
            }
            bits.seek(end);
        }
        if channels == 2 && !missing {
            stereo(header, granule_info, &bands, granule)?;
        }
        for ch in 0..channels {
            let gi = granule_info.get(ch).ok_or(Error::Invalid("channel"))?;
            let channel = state.get_mut(ch).ok_or(Error::Invalid("channel"))?;
            let xr = granule.xr.get_mut(ch).ok_or(Error::Invalid("channel"))?;
            let long_lines = long_part_lines(gi, header, &bands);
            if gi.block_type == 2 && gi.window_switching {
                reorder(xr, &bands, long_lines);
            }
            alias_reduce(xr, gi, long_lines);
            // The subbands of a mixed block's lower part take the normal
            // window, whatever the block type (Table 19 of the Fraunhofer
            // document: start and stop blocks have a mixed form too).
            let normal_below = if gi.window_switching && gi.mixed {
                mixed_long_lines(header, &bands) / 18
            } else {
                0
            };
            let subband = granule
                .subband
                .get_mut(ch)
                .ok_or(Error::Invalid("channel"))?;
            imdct(
                xr,
                gi,
                long_lines,
                normal_below,
                &mut channel.overlap,
                subband,
                gr * 18,
            );
        }
    }
    // The synthesis filter bank, over the whole frame at once.
    for (ch, (synthesis, subband)) in synthesis
        .iter_mut()
        .zip(granule.subband.iter())
        .enumerate()
        .take(channels)
    {
        let out = planes
            .get_mut(ch * samples..ch * samples + samples)
            .ok_or(Error::Invalid("output"))?;
        synthesis.filter(subband, samples / 32, out);
    }
    Ok(())
}

/// Lines of the long-block part of a granule: all 576 for long blocks,
/// none for short blocks, the first 8 (MPEG-1) or 6 (otherwise) long bands
/// of a mixed block.
fn long_part_lines(gi: &GranuleInfo, header: &Header, bands: &Bands) -> usize {
    if !(gi.window_switching && gi.block_type == 2) {
        return 576;
    }
    if !gi.mixed {
        return 0;
    }
    mixed_long_lines(header, bands)
}

/// Lines of the lower part of a mixed block: the first 8 (MPEG-1) or 6
/// (otherwise) long bands, 36 lines (72 at 8 kHz).
fn mixed_long_lines(header: &Header, bands: &Bands) -> usize {
    let count = if header.version == Version::Mpeg1 {
        8
    } else {
        6
    };
    bands.long.iter().take(count).map(|&w| usize::from(w)).sum()
}

/// The MPEG-1 scalefactors (2.4.2.7, Table 16 of the Fraunhofer document).
fn scalefactors_mpeg1(
    bits: &mut MainBits,
    gi: &GranuleInfo,
    info: &SideInfo,
    gr: usize,
    ch: usize,
    state: &mut [Channel; 2],
    granule: &mut Granule,
) -> Result<(), Error> {
    const SLEN: [(u32, u32); 16] = [
        (0, 0),
        (0, 1),
        (0, 2),
        (0, 3),
        (3, 0),
        (1, 1),
        (1, 2),
        (1, 3),
        (2, 1),
        (2, 2),
        (2, 3),
        (3, 1),
        (3, 2),
        (3, 3),
        (4, 2),
        (4, 3),
    ];
    let (slen1, slen2) = SLEN
        .get(gi.scalefac_compress as usize)
        .copied()
        .unwrap_or((0, 0));
    let long = granule
        .scalefac_l
        .get_mut(ch)
        .ok_or(Error::Invalid("channel"))?;
    let short = granule
        .scalefac_s
        .get_mut(ch)
        .ok_or(Error::Invalid("channel"))?;
    let channel = state.get_mut(ch).ok_or(Error::Invalid("channel"))?;
    if gi.window_switching && gi.block_type == 2 {
        *long = [0; 22];
        *short = [[0; 3]; 13];
        let first_short = if gi.mixed {
            for slot in long.iter_mut().take(8) {
                *slot = bits.read(slen1) as u8;
            }
            3
        } else {
            0
        };
        for (sfb, windows) in short.iter_mut().enumerate().take(12).skip(first_short) {
            let n = if sfb < 6 { slen1 } else { slen2 };
            for value in windows.iter_mut() {
                *value = bits.read(n) as u8;
            }
        }
        channel.scalefac_l = *long;
    } else {
        *short = [[0; 3]; 13];
        let scfsi = info.scfsi.get(ch).copied().unwrap_or_default();
        const GROUPS: [(usize, usize); 4] = [(0, 6), (6, 11), (11, 16), (16, 21)];
        for (group, &(from, to)) in GROUPS.iter().enumerate() {
            let reuse = gr == 1 && scfsi.get(group).copied().unwrap_or(false);
            let n = if group < 2 { slen1 } else { slen2 };
            for sfb in from..to {
                let value = if reuse {
                    channel.scalefac_l.get(sfb).copied().unwrap_or(0)
                } else {
                    bits.read(n) as u8
                };
                if let Some(slot) = long.get_mut(sfb) {
                    *slot = value;
                }
            }
        }
        if let Some(slot) = long.get_mut(21) {
            *slot = 0;
        }
        channel.scalefac_l = *long;
    }
    Ok(())
}

/// The MPEG-2 and 2.5 scalefactors (ISO/IEC 13818-3 2.4.3.2): four
/// partitions sized by `scalefac_compress`, with the intensity variant for
/// the right channel.
fn scalefactors_lsf(
    bits: &mut MainBits,
    gi: &GranuleInfo,
    header: &Header,
    ch: usize,
    granule: &mut Granule,
) -> Result<(), Error> {
    let block = if gi.window_switching && gi.block_type == 2 {
        if gi.mixed { 2 } else { 1 }
    } else {
        0
    };
    let sfc = gi.scalefac_compress;
    let intensity_right = header.intensity && ch == 1;
    // (slen, nr_of_sfb rows by block kind: long, short, mixed)
    let (slen, counts): ([u32; 4], [[usize; 4]; 3]) = if !intensity_right {
        if sfc < 400 {
            (
                [(sfc >> 4) / 5, (sfc >> 4) % 5, (sfc % 16) >> 2, sfc % 4],
                [[6, 5, 5, 5], [9, 9, 9, 9], [6, 9, 9, 9]],
            )
        } else if sfc < 500 {
            let s = sfc - 400;
            (
                [(s >> 2) / 5, (s >> 2) % 5, s % 4, 0],
                [[6, 5, 7, 3], [9, 9, 12, 6], [6, 9, 12, 6]],
            )
        } else {
            let s = sfc - 500;
            (
                [s / 3, s % 3, 0, 0],
                [[11, 10, 0, 0], [18, 18, 0, 0], [15, 18, 0, 0]],
            )
        }
    } else {
        granule.intensity_scale = sfc % 2 == 1;
        let s = sfc >> 1;
        if s < 180 {
            (
                [s / 36, (s % 36) / 6, (s % 36) % 6, 0],
                [[7, 7, 7, 0], [12, 12, 12, 0], [6, 15, 12, 0]],
            )
        } else if s < 244 {
            let t = s - 180;
            (
                [(t % 64) >> 4, (t % 16) >> 2, t % 4, 0],
                [[6, 6, 6, 3], [12, 9, 9, 6], [6, 12, 9, 6]],
            )
        } else {
            let t = s - 244;
            (
                [t / 3, t % 3, 0, 0],
                [[8, 8, 5, 0], [15, 12, 9, 0], [6, 18, 9, 0]],
            )
        }
    };
    let preflag = !intensity_right && sfc >= 500;
    let counts = counts.get(block).copied().unwrap_or_default();
    // The values in transmission order, with the bits each took.
    let mut values = [(0u8, 0u8); 54];
    let mut n = 0;
    for (&count, &len) in counts.iter().zip(&slen) {
        for _ in 0..count {
            if let Some(slot) = values.get_mut(n) {
                *slot = (bits.read(len) as u8, len as u8);
            }
            n += 1;
        }
    }
    let long = granule
        .scalefac_l
        .get_mut(ch)
        .ok_or(Error::Invalid("channel"))?;
    let short = granule
        .scalefac_s
        .get_mut(ch)
        .ok_or(Error::Invalid("channel"))?;
    let slen_l = granule
        .slen_l
        .get_mut(ch)
        .ok_or(Error::Invalid("channel"))?;
    let slen_s = granule
        .slen_s
        .get_mut(ch)
        .ok_or(Error::Invalid("channel"))?;
    *long = [0; 22];
    *short = [[0; 3]; 13];
    *slen_l = [0; 22];
    *slen_s = [0; 13];
    let mut it = values.iter().take(n);
    match block {
        0 => {
            for (slot, len) in long.iter_mut().zip(slen_l.iter_mut()).take(21) {
                let (v, l) = it.next().copied().unwrap_or((0, 0));
                *slot = v;
                *len = l;
            }
            if preflag {
                for (slot, &p) in long.iter_mut().zip(&tables::PRETAB) {
                    *slot = slot.saturating_add(p);
                }
            }
        }
        _ => {
            let first_short = if block == 2 {
                for (slot, len) in long.iter_mut().zip(slen_l.iter_mut()).take(6) {
                    let (v, l) = it.next().copied().unwrap_or((0, 0));
                    *slot = v;
                    *len = l;
                }
                3
            } else {
                0
            };
            for (windows, len) in short
                .iter_mut()
                .zip(slen_s.iter_mut())
                .take(12)
                .skip(first_short)
            {
                for value in windows.iter_mut() {
                    let (v, l) = it.next().copied().unwrap_or((0, 0));
                    *value = v;
                    *len = l;
                }
            }
        }
    }
    Ok(())
}

/// |x|^(4/3) for the values Huffman decoding can give (15 + 2^13 - 1), as
/// x times its cube root.
pub(crate) fn pow43() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..8207u32)
            .map(|i| {
                let x = f64::from(i);
                (x * cube_root(x)) as f32
            })
            .collect()
    })
}

/// The cube root of `x` (0 to 2^13 here), by Newton's method from a guess
/// within a factor of 2, to double precision, with arithmetic alone: the
/// standard library's `cbrt` and `powf` call the C math library
/// (`tables/gen_trig.py`).
fn cube_root(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    // 2^(e/3) for x's binary exponent e, then Newton steps; each step
    // doubles the correct digits, six reach double precision (the last two
    // may alternate between neighbours, which f32 cannot tell apart).
    let exponent = ((x.to_bits() >> 52) & 0x7FF) as i64 - 1023;
    let mut y = f64::from_bits(((exponent / 3 + 1023) as u64) << 52);
    for _ in 0..8 {
        y -= (y * y * y - x) / (3.0 * y * y);
    }
    y
}

/// 2^(q/4) for an exponent in quarter steps: 2^(q mod 4 / 4) times a
/// power of two made from its exponent bits (0 below the normal range).
#[inline(always)]
fn quarter_pow2(q: i32) -> f32 {
    const FRACTION: [f32; 4] = [1.0, 1.189_207_1, std::f32::consts::SQRT_2, 1.681_792_8];
    let whole = q.div_euclid(4);
    let fraction = FRACTION
        .get(q.rem_euclid(4) as usize)
        .copied()
        .unwrap_or(1.0);
    let power = match u32::try_from(whole + 127) {
        Ok(bits @ 1..=254) => f32::from_bits(bits << 23),
        Ok(0) | Err(_) => 0.0,
        Ok(_) => f32::MAX,
    };
    fraction * power
}

/// The gain of each line, as runs: `(end, gain)` with `end` one past the
/// run's last line, in line order, the last ending at 576 (ISO/IEC
/// 11172-3 2.4.3.4): 2^((global_gain - 210 - 8 subblock_gain) / 4)
/// 2^-(scalefac_multiplier (scalefac + preflag pretab)), with the
/// multiplier 0.5 or 1 by `scalefac_scale`. Huffman decoding multiplies
/// each |is|^(4/3) by its line's gain ([`huffman::decode`]).
pub(crate) struct Gains {
    runs: [(usize, f32); 64],
    len: usize,
}

impl Gains {
    pub(crate) fn runs(&self) -> &[(usize, f32)] {
        self.runs.get(..self.len).unwrap_or_default()
    }

    fn push(&mut self, end: usize, gain: f32) {
        if let Some(slot) = self.runs.get_mut(self.len) {
            *slot = (end, gain);
            self.len += 1;
        }
    }
}

fn gains(
    gi: &GranuleInfo,
    header: &Header,
    bands: &Bands,
    ch: usize,
    granule: &Granule,
) -> Result<Gains, Error> {
    let mut gains = Gains {
        runs: [(576, 0.0); 64],
        len: 0,
    };
    let long_sf = granule
        .scalefac_l
        .get(ch)
        .ok_or(Error::Invalid("channel"))?;
    let short_sf = granule
        .scalefac_s
        .get(ch)
        .ok_or(Error::Invalid("channel"))?;
    let shift = if gi.scalefac_scale { 4 } else { 2 };
    let gain = gi.global_gain as i32 - 210;
    // MPEG-1 adds the preemphasis here; MPEG-2 folded it into the
    // scalefactors.
    let pretab = header.version == Version::Mpeg1 && gi.preflag;
    let long_lines = long_part_lines(gi, header, bands);
    // Long bands, up to the long part's end.
    let mut line = 0usize;
    for (sfb, &width) in bands.long.iter().enumerate() {
        if line >= long_lines {
            break;
        }
        let sf = i32::from(long_sf.get(sfb).copied().unwrap_or(0))
            + if pretab {
                i32::from(tables::PRETAB.get(sfb).copied().unwrap_or(0))
            } else {
                0
            };
        line = (line + usize::from(width)).min(long_lines);
        gains.push(line, quarter_pow2(gain - shift * sf));
    }
    // Short bands: in transmission order, band by band, window by window.
    if long_lines < 576 {
        let first_short = if gi.mixed { 3 } else { 0 };
        for (sfb, &width) in bands.short.iter().enumerate().skip(first_short) {
            for window in 0..3 {
                let sf = i32::from(
                    short_sf
                        .get(sfb)
                        .and_then(|w| w.get(window))
                        .copied()
                        .unwrap_or(0),
                );
                let sbg = gi.subblock_gain.get(window).copied().unwrap_or(0) as i32;
                line = (line + usize::from(width)).min(576);
                gains.push(line, quarter_pow2(gain - 8 * sbg - shift * sf));
            }
        }
    }
    Ok(gains)
}

/// MS and intensity stereo (ISO/IEC 11172-3 2.4.3.4; 13818-3 2.4.3.2).
fn stereo(
    header: &Header,
    gi: &[GranuleInfo],
    bands: &Bands,
    granule: &mut Granule,
) -> Result<(), Error> {
    if !header.ms && !header.intensity {
        return Ok(());
    }
    if !header.intensity {
        // Middle/side only: lines past both channels' last nonzero line
        // stay zero.
        let lines = granule
            .nonzero
            .iter()
            .copied()
            .max()
            .unwrap_or(576)
            .min(576);
        let [left, right] = &mut granule.xr;
        let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
        let left = left.get_mut(..lines).unwrap_or_default();
        let right = right.get_mut(..lines).unwrap_or_default();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (m, s) = (*l, *r);
            *l = (m + s) * inv_sqrt2;
            *r = (m - s) * inv_sqrt2;
        }
        return Ok(());
    }
    let right_info = gi.get(1).ok_or(Error::Invalid("channel"))?;
    let short = right_info.window_switching && right_info.block_type == 2;
    let long_lines = long_part_lines(right_info, header, bands);
    let mpeg1 = header.version == Version::Mpeg1;
    // Per line: whether intensity stereo applies, and its factors.
    let mut is_factor = [None::<(f32, f32)>; 576];
    if header.intensity {
        let [left, right] = &granule.xr;
        let _ = left;
        // Bands (start, width, scalefactor, slen, window) in line order.
        let mut units: Vec<(usize, usize, u8, u8, usize)> = Vec::with_capacity(40);
        let mut line = 0usize;
        if long_lines > 0 {
            for (sfb, &width) in bands.long.iter().enumerate() {
                if line >= long_lines {
                    break;
                }
                let width = usize::from(width);
                let sf = granule
                    .scalefac_l
                    .get(1)
                    .and_then(|s| s.get(sfb))
                    .copied()
                    .unwrap_or(0);
                let slen = granule
                    .slen_l
                    .get(1)
                    .and_then(|s| s.get(sfb))
                    .copied()
                    .unwrap_or(0);
                units.push((line, width, sf, slen, 3));
                line += width;
            }
        }
        if short {
            let first_short = if right_info.mixed { 3 } else { 0 };
            for (sfb, &width) in bands.short.iter().enumerate().skip(first_short) {
                let width = usize::from(width);
                for window in 0..3 {
                    let sf = granule
                        .scalefac_s
                        .get(1)
                        .and_then(|s| s.get(sfb))
                        .and_then(|w| w.get(window))
                        .copied()
                        .unwrap_or(0);
                    let slen = granule
                        .slen_s
                        .get(1)
                        .and_then(|s| s.get(sfb))
                        .copied()
                        .unwrap_or(0);
                    units.push((line, width, sf, slen, window));
                    line += width;
                }
            }
        }
        // The intensity bound: above the last unit, per window for short
        // blocks (the long part counts as window 3), that holds a nonzero
        // right-channel line.
        let mut zero_from = [usize::MAX; 4];
        for (index, &(start, width, ..)) in units.iter().enumerate().rev() {
            let window = units.get(index).map_or(3, |u| u.4);
            if zero_from.get(window).copied() == Some(usize::MAX)
                && right
                    .get(start..start + width)
                    .unwrap_or_default()
                    .iter()
                    .any(|&v| v != 0.0)
            {
                if let Some(slot) = zero_from.get_mut(window) {
                    *slot = index + 1;
                }
            }
        }
        // A short window that is zero all the way down leaves the long part
        // of a mixed block to its own bound.
        let all_short_zero = short && (0..3).all(|w| zero_from.get(w).copied() == Some(usize::MAX));
        let mut last_valid_short = [(0u8, 0u8); 3];
        let mut last_valid_long = (0u8, 0u8);
        for (index, &(start, width, mut sf, mut slen, window)) in units.iter().enumerate() {
            let bound = zero_from.get(window).copied().unwrap_or(usize::MAX);
            let bound = if bound == usize::MAX { 0 } else { bound };
            let in_is = if window == 3 {
                // The long part: intensity coded only above its own bound
                // and when no short window has nonzero data.
                index >= bound && (!short || all_short_zero)
            } else {
                index >= bound
            };
            // The last band of each table carries no scalefactor; it takes
            // the previous band's position.
            let last_long = window == 3 && start + width >= 576;
            let last_short = window < 3 && start + width * (3 - window) >= 576;
            if last_long {
                (sf, slen) = last_valid_long;
            } else if last_short {
                (sf, slen) = last_valid_short.get(window).copied().unwrap_or((0, 0));
            } else if window == 3 {
                last_valid_long = (sf, slen);
            } else if let Some(slot) = last_valid_short.get_mut(window) {
                *slot = (sf, slen);
            }
            if !in_is {
                continue;
            }
            let factors = if mpeg1 {
                // An intensity position of 7 is illegal.
                match sf {
                    7.. => None,
                    6 => Some((1.0, 0.0)),
                    p => {
                        let ratio = trig::TAN_12.get(usize::from(p)).copied().unwrap_or(0.0);
                        Some(((ratio / (1.0 + ratio)) as f32, (1.0 / (1.0 + ratio)) as f32))
                    }
                }
            } else {
                let illegal = if slen == 0 { 0 } else { (1u32 << slen) - 1 };
                if slen > 0 && u32::from(sf) == illegal {
                    None
                } else {
                    // i0^n, i0 = 1/sqrt(2) for intensity_scale 1, else
                    // 2^(-1/4) (`trig.rs`).
                    let powers = trig::INTENSITY_POWERS
                        .get(usize::from(!granule.intensity_scale))
                        .copied()
                        .unwrap_or_default();
                    let power = |n: u8| powers.get(usize::from(n)).copied().unwrap_or(0.0) as f32;
                    if sf == 0 {
                        Some((1.0, 1.0))
                    } else if sf % 2 == 1 {
                        Some((power(sf.div_ceil(2)), 1.0))
                    } else {
                        Some((1.0, power(sf / 2)))
                    }
                }
            };
            if let Some(f) = factors {
                for slot in is_factor.get_mut(start..start + width).unwrap_or_default() {
                    *slot = Some(f);
                }
            }
        }
    }
    let [left, right] = &mut granule.xr;
    let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
    for ((l, r), factor) in left.iter_mut().zip(right.iter_mut()).zip(is_factor.iter()) {
        match factor {
            Some((kl, kr)) => {
                let m = *l;
                if mpeg1 {
                    *l = m * kl;
                    *r = m * kr;
                } else {
                    *r = m * kr;
                    *l = m * kl;
                }
            }
            None if header.ms => {
                let (m, s) = (*l, *r);
                *l = (m + s) * inv_sqrt2;
                *r = (m - s) * inv_sqrt2;
            }
            None => {}
        }
    }
    let _ = long_lines;
    Ok(())
}

/// Reorders short-block lines from transmission order (band, window,
/// frequency) to window order within each subband: for subband `sb`,
/// window `w` and line `k` of the 6 the window has in it, position
/// `18 sb + 6 w + k` (ISO/IEC 11172-3 2.4.3.4, "Reordering").
fn reorder(xr: &mut [f32; 576], bands: &Bands, long_lines: usize) {
    let mut windows = [[0.0f32; 192]; 3];
    let first_short = if long_lines > 0 { 3 } else { 0 };
    let mut line = long_lines;
    let mut freq = bands
        .short
        .iter()
        .take(first_short)
        .map(|&w| usize::from(w))
        .sum::<usize>();
    for &width in bands.short.iter().skip(first_short) {
        let width = usize::from(width);
        for window in windows.iter_mut() {
            let src = xr.get(line..line + width).unwrap_or_default();
            if let Some(dst) = window.get_mut(freq..freq + width) {
                dst.copy_from_slice(src);
            }
            line += width;
        }
        freq += width;
    }
    let first_subband = long_lines / 18;
    for sb in first_subband..32 {
        for (w, window) in windows.iter().enumerate() {
            let src = window.get(sb * 6..sb * 6 + 6).unwrap_or_default();
            if let Some(dst) = xr.get_mut(sb * 18 + w * 6..sb * 18 + w * 6 + 6) {
                dst.copy_from_slice(src);
            }
        }
    }
}

/// The butterfly coefficients cs[i] = 1/sqrt(1 + c[i]^2), ca[i] = c[i]/sqrt(1
/// + c[i]^2) of Table 3-B.9.
fn butterflies() -> &'static [(f32, f32); 8] {
    static TABLE: OnceLock<[(f32, f32); 8]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = [(0.0, 0.0); 8];
        for (slot, &c) in out.iter_mut().zip(&tables::ALIAS) {
            let root = (1.0 + c * c).sqrt();
            *slot = ((1.0 / root) as f32, (c / root) as f32);
        }
        out
    })
}

/// Alias reduction between the subbands of the long part.
fn alias_reduce(xr: &mut [f32; 576], gi: &GranuleInfo, long_lines: usize) {
    if gi.window_switching && gi.block_type == 2 && !gi.mixed {
        return;
    }
    let subbands = long_lines / 18;
    let coefficients = butterflies();
    for sb in 1..subbands {
        let Some((low, high)) = xr.split_at_mut_checked(18 * sb) else {
            break;
        };
        for (i, &(cs, ca)) in coefficients.iter().enumerate() {
            let (Some(a), Some(b)) = (low.get_mut(18 * sb - 1 - i), high.get_mut(i)) else {
                continue;
            };
            let (x, y) = (*a, *b);
            *a = x * cs - y * ca;
            *b = y * cs + x * ca;
        }
    }
}

/// The IMDCT windows: normal, start, short, stop (block types 0 to 3).
fn windows() -> &'static [[f32; 36]; 4] {
    static TABLE: OnceLock<[[f32; 36]; 4]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let long = |i: usize| trig::LONG_WINDOW.get(i).copied().unwrap_or(0.0);
        let short = |i: usize| trig::SHORT_WINDOW.get(i).copied().unwrap_or(0.0);
        let mut w = [[0.0f32; 36]; 4];
        let [normal, start, short_window, stop] = &mut w;
        for (i, ((n, s), e)) in normal
            .iter_mut()
            .zip(start.iter_mut())
            .zip(stop.iter_mut())
            .enumerate()
        {
            *n = long(i) as f32;
            *s = match i {
                0..=17 => long(i),
                18..=23 => 1.0,
                24..=29 => short(i - 18),
                _ => 0.0,
            } as f32;
            *e = match i {
                0..=5 => 0.0,
                6..=11 => short(i - 6),
                12..=17 => 1.0,
                _ => long(i),
            } as f32;
        }
        for (i, slot) in short_window.iter_mut().take(12).enumerate() {
            *slot = short(i) as f32;
        }
        w
    })
}

/// The short IMDCT cosines cos(pi/24 (2i + 7)(2k + 1)), transposed and
/// halved by the symmetries of [`imdct12`]: for input line k, the cosines
/// of outputs 0..3 and 6..9 (`trig.rs`).
fn short_cosines() -> &'static [([f32; 3], [f32; 3]); 6] {
    static TABLE: OnceLock<[([f32; 3], [f32; 3]); 6]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut short = [([0.0f32; 3], [0.0f32; 3]); 6];
        for ((low, high), (l64, h64)) in short.iter_mut().zip(trig::SHORT_COSINES.iter()) {
            for (slot, &v) in low.iter_mut().zip(l64) {
                *slot = v as f32;
            }
            for (slot, &v) in high.iter_mut().zip(h64) {
                *slot = v as f32;
            }
        }
        short
    })
}

/// The long IMDCT (n = 36): x[i] = sum_k X[k] cos(pi/72 (2i + 19)(2k + 1))
/// is the 18-point DCT-IV Y[n] (`imdct.rs`) at n = i + 9, which the
/// kernel's symmetries fold back into 0..18: cos(pi/72 (2n + 1)(2k + 1))
/// changes sign at n -> 35 - n and at n -> n + 36. So x[i] = Y[i + 9] for
/// i < 9, -Y[26 - i] for i < 27 and -Y[i - 27] above.
#[inline(always)]
fn imdct36(input: &[F4; 18]) -> [F4; 36] {
    let y = super::imdct::dct4_18(input);
    let mut out = [F4::ZERO; 36];
    let (low, high) = y.split_at(9);
    let (first, rest) = out.split_at_mut(9);
    let (middle, last) = rest.split_at_mut(18);
    for (o, &v) in first.iter_mut().zip(high) {
        *o = v;
    }
    for (o, &v) in middle.iter_mut().zip(y.iter().rev()) {
        *o = -v;
    }
    for (o, &v) in last.iter_mut().zip(low) {
        *o = -v;
    }
    out
}

/// The short IMDCT (n = 12): with a = 2i + 7, x[5 - i] = -x[i] and
/// x[11 - i] = x[6 + i].
#[inline(always)]
fn imdct12(input: &[F4], cos: &[([f32; 3], [f32; 3]); 6]) -> [F4; 12] {
    let (mut a, mut b) = ([F4::ZERO; 3], [F4::ZERO; 3]);
    for (&x, (low, high)) in input.iter().zip(cos.iter()) {
        for (s, &c) in a.iter_mut().zip(low) {
            *s = *s + x * c;
        }
        for (s, &c) in b.iter_mut().zip(high) {
            *s = *s + x * c;
        }
    }
    let mut out = [F4::ZERO; 12];
    for (i, (&va, &vb)) in a.iter().zip(&b).enumerate() {
        for (at, v) in [(i, va), (5 - i, -va), (6 + i, vb), (11 - i, vb)] {
            if let Some(slot) = out.get_mut(at) {
                *slot = v;
            }
        }
    }
    out
}

/// Four lanes of a [time][subband] row.
#[inline(always)]
fn lanes(row: &[f32; 32], sb: usize) -> F4 {
    match row.get(sb..sb + 4) {
        Some(&[a, b, c, d]) => F4([a, b, c, d]),
        _ => F4::ZERO,
    }
}

#[inline(always)]
fn store(row: &mut [f32; 32], sb: usize, value: F4) {
    if let Some(slot) = row.get_mut(sb..sb + 4) {
        slot.copy_from_slice(&value.0);
    }
}

/// IMDCT, windowing, overlap and frequency inversion into `subband`
/// ([subband][slot], from slot `first`), four subbands at a time, one per
/// lane. Lanes of
/// short blocks get a zero long window and lanes of long blocks a zero
/// short one, so a group that mixes them (the boundary of a mixed block)
/// takes both transforms.
fn imdct(
    xr: &[f32; 576],
    gi: &GranuleInfo,
    long_lines: usize,
    normal_below: usize,
    overlap: &mut [[f32; 32]; 18],
    subband: &mut [[f32; MAX_SLOTS]; 32],
    first: usize,
) {
    const ZERO: [f32; 36] = [0.0; 36];
    let short_cos = short_cosines();
    let windows = windows();
    let short_window = windows.get(2).unwrap_or(&ZERO);
    let block_type = if gi.window_switching {
        gi.block_type as usize
    } else {
        0
    };
    for (group, lines) in xr.chunks_exact(72).enumerate() {
        let sb = group * 4;
        let mut out = [F4::ZERO; 36];
        // A group with no lines contributes nothing but the overlap.
        if lines.iter().any(|&x| x != 0.0) {
            // Line k of the four subbands.
            let mut input = [F4::ZERO; 18];
            let [a, b, c, d] =
                [0, 1, 2, 3].map(|l| lines.get(l * 18..l * 18 + 18).unwrap_or_default());
            for ((slot, (&a, &b)), (&c, &d)) in
                input.iter_mut().zip(a.iter().zip(b)).zip(c.iter().zip(d))
            {
                *slot = F4([a, b, c, d]);
            }
            // A long block takes the normal window in the lower part of a
            // mixed block, else the block type's.
            let long = [0, 1, 2, 3].map(|l| {
                let at = sb + l;
                if at * 18 >= long_lines {
                    None
                } else if block_type == 2 || at < normal_below {
                    windows.first()
                } else {
                    windows.get(block_type)
                }
            });
            if long.iter().any(Option::is_some) {
                let [w0, w1, w2, w3] = long.map(|w| w.unwrap_or(&ZERO));
                let x = imdct36(&input);
                for (i, (o, v)) in out.iter_mut().zip(x).enumerate() {
                    let window = F4([
                        w0.get(i).copied().unwrap_or(0.0),
                        w1.get(i).copied().unwrap_or(0.0),
                        w2.get(i).copied().unwrap_or(0.0),
                        w3.get(i).copied().unwrap_or(0.0),
                    ]);
                    *o = v * window;
                }
            }
            if long.iter().any(Option::is_none) {
                let mask = F4(long.map(|w| if w.is_none() { 1.0 } else { 0.0 }));
                for w in 0..3 {
                    let x = imdct12(input.get(w * 6..w * 6 + 6).unwrap_or_default(), short_cos);
                    let dst = out.get_mut(6 + 6 * w..18 + 6 * w).unwrap_or_default();
                    for ((o, &x), &win) in dst.iter_mut().zip(x.iter()).zip(short_window.iter()) {
                        *o = *o + x * mask * win;
                    }
                }
            }
        }
        // Frequency inversion: odd time samples of odd subbands.
        let invert = F4([1.0, -1.0, 1.0, -1.0]);
        let (now, next) = out.split_at(18);
        let rows = subband.get_mut(sb..sb + 4).unwrap_or_default();
        for (t, (prev, (&o, &n))) in overlap.iter_mut().zip(now.iter().zip(next)).enumerate() {
            let mut value = o + lanes(prev, sb);
            if t % 2 == 1 {
                value = value * invert;
            }
            for (row, &v) in rows.iter_mut().zip(value.0.iter()) {
                if let Some(slot) = row.get_mut(first + t) {
                    *slot = v;
                }
            }
            store(prev, sb, n);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;

    /// x times its cube root equals x^(4/3) in f32, bit for bit, for every
    /// value Huffman decoding gives.
    #[test]
    fn pow43_matches_the_power() {
        for (i, &got) in pow43().iter().enumerate() {
            let want = (i as f64).powf(4.0 / 3.0) as f32;
            assert_eq!(got.to_bits(), want.to_bits(), "{i}: {got} {want}");
        }
    }

    /// The generated constants are their formulas.
    #[test]
    fn trigonometric_constants_match_their_formulas() {
        let pi = std::f64::consts::PI;
        for (i, &v) in trig::LONG_WINDOW.iter().enumerate() {
            assert!((v - (pi / 36.0 * (i as f64 + 0.5)).sin()).abs() < 1e-15);
        }
        for (i, &v) in trig::SHORT_WINDOW.iter().enumerate() {
            assert!((v - (pi / 12.0 * (i as f64 + 0.5)).sin()).abs() < 1e-15);
        }
        for (k, (low, high)) in trig::SHORT_COSINES.iter().enumerate() {
            for i in 0..3 {
                let c = |i: f64| (pi / 24.0 * (2.0 * i + 7.0) * (2.0 * k as f64 + 1.0)).cos();
                assert!((low[i] - c(i as f64)).abs() < 1e-15);
                assert!((high[i] - c(6.0 + i as f64)).abs() < 1e-15);
            }
        }
        for (p, &v) in trig::TAN_12.iter().enumerate() {
            assert!((v - (p as f64 * pi / 12.0).tan()).abs() < 1e-15);
        }
        for n in 0..16 {
            assert!((trig::INTENSITY_POWERS[0][n] - 0.5f64.sqrt().powi(n as i32)).abs() < 1e-15);
            assert!((trig::INTENSITY_POWERS[1][n] - 0.5f64.powf(n as f64 / 4.0)).abs() < 1e-15);
        }
    }
}
