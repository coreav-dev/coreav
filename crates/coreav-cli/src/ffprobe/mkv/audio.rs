//! What `ffprobe` learns of an audio packet from its first bytes: the
//! number of samples it decodes to (RFC 6716 3.1 for Opus, the Vorbis I
//! specification 4.3.1 for Vorbis, ATSC A/52 5.4 for AC-3 and E-AC-3,
//! ISO/IEC 11172-3 2.4.2.3 for MPEG audio, the FLAC format 9.1.1), and the
//! bit rate a frame header states. Every reader is bounded by its input.

/// Samples at 48 kHz in an Opus packet, from its TOC byte and, for code 3,
/// the frame count byte (RFC 6716 3.1 and 3.2).
pub fn opus_samples(packet: &[u8]) -> Option<u32> {
    let toc = *packet.first()?;
    let config = toc >> 3;
    let per_frame: u32 = match config {
        0..=11 => match config & 3 {
            0 => 480,
            1 => 960,
            2 => 1920,
            _ => 2880,
        },
        12..=15 => 480 << (config & 1),
        _ => 120 << (config & 3),
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => u32::from(*packet.get(1)? & 0x3F),
    };
    per_frame.checked_mul(frames)
}

/// Reads bits least significant first, as Vorbis packs them.
struct LsbBits<'a> {
    data: &'a [u8],
    at: usize,
}

impl LsbBits<'_> {
    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut value = 0u32;
        for i in 0..n {
            let byte = self.data.get(self.at / 8)?;
            let bit = (byte >> (self.at % 8)) & 1;
            value |= u32::from(bit) << i;
            self.at += 1;
        }
        Some(value)
    }
}

/// The bit at position `at` of `data`, least significant first.
fn bit_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from((data.get(at / 8)? >> (at % 8)) & 1))
}

/// `n` bits from position `at`, least significant first.
fn bits_at(data: &[u8], at: usize, n: u32) -> Option<u32> {
    LsbBits { data, at }.bits(n)
}

/// What a Vorbis stream's headers say about its packets' lengths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vorbis {
    /// The short and long block sizes.
    pub blocksizes: [u32; 2],
    /// Each mode's block flag.
    pub modes: Vec<bool>,
}

impl Vorbis {
    /// The block size of an audio packet.
    pub fn blocksize(&self, packet: &[u8]) -> Option<u32> {
        let first = *packet.first()?;
        if first & 1 != 0 {
            return None;
        }
        let count = u32::try_from(self.modes.len()).ok()?;
        let width = u32::BITS - count.saturating_sub(1).leading_zeros();
        let mode = bits_at(packet, 1, width)?;
        let long = *self.modes.get(usize::try_from(mode).ok()?)?;
        self.blocksizes.get(usize::from(long)).copied()
    }
}

/// The three headers of a Matroska `A_VORBIS` `CodecPrivate`: Xiph lacing
/// (the Matroska codec mappings).
pub fn xiph_headers(private: &[u8]) -> Option<Vec<&[u8]>> {
    let count = usize::from(*private.first()?) + 1;
    let mut sizes = Vec::with_capacity(count);
    let mut at = 1usize;
    for _ in 1..count {
        let mut size = 0usize;
        loop {
            let b = *private.get(at)?;
            at += 1;
            size = size.checked_add(usize::from(b))?;
            if b != 255 {
                break;
            }
        }
        sizes.push(size);
    }
    let mut out = Vec::with_capacity(count);
    for size in sizes {
        out.push(private.get(at..at.checked_add(size)?)?);
        at += size;
    }
    out.push(private.get(at..)?);
    Some(out)
}

/// Reads the block sizes from the identification header and the modes
/// from the end of the setup header, where they are the last field before
/// the framing bit (Vorbis I 4.2.4): each mode is a block flag, two 16-bit
/// zero fields and a mapping number, and a 6-bit count precedes them.
pub fn vorbis(private: &[u8]) -> Option<Vorbis> {
    let headers = xiph_headers(private)?;
    let id = headers.first()?;
    if id.get(..7)? != b"\x01vorbis" {
        return None;
    }
    let sizes = *id.get(28)?;
    let blocksizes = [1u32 << (sizes & 0x0F), 1u32 << (sizes >> 4)];
    let setup = headers.get(2)?;
    if setup.get(..7)? != b"\x05vorbis" {
        return None;
    }
    // The framing bit: the last bit set.
    let mut end = setup.len().checked_mul(8)?;
    loop {
        end = end.checked_sub(1)?;
        if bit_at(setup, end)? == 1 {
            break;
        }
    }
    let mut best = None;
    let mut modes = Vec::new();
    let mut at = end;
    while modes.len() < 64 {
        let Some(start) = at.checked_sub(41) else {
            break;
        };
        if bits_at(setup, start + 1, 16)? != 0 || bits_at(setup, start + 17, 16)? != 0 {
            break;
        }
        modes.push(bit_at(setup, start)? == 1);
        at = start;
        let Some(count_at) = at.checked_sub(6) else {
            break;
        };
        if usize::try_from(bits_at(setup, count_at, 6)?).ok()? + 1 == modes.len() {
            best = Some(modes.len());
        }
    }
    let n = best?;
    let mut modes: Vec<bool> = modes.into_iter().take(n).collect();
    modes.reverse();
    Some(Vorbis { blocksizes, modes })
}

/// AC-3 (`bsid` up to 10) or E-AC-3 (`bsid` 11 to 16) syncframe header:
/// samples per frame and bit rate in bits per second, when stated.
pub fn ac3(frame: &[u8]) -> Option<(u32, Option<u32>)> {
    if frame.get(..2)? != [0x0B, 0x77] {
        return None;
    }
    let bsid = frame.get(5)? >> 3;
    if bsid <= 10 {
        const RATES: [u32; 19] = [
            32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
        ];
        let code = usize::from((frame.get(4)? & 0x3F) >> 1);
        let rate = RATES.get(code)?.checked_mul(1000)?;
        let shift = u32::from(bsid.saturating_sub(8));
        return Some((1536, Some(rate >> shift)));
    }
    if bsid <= 16 {
        let byte = *frame.get(4)?;
        let fscod = byte >> 6;
        let blocks = match (fscod, (byte >> 4) & 3) {
            (3, _) | (_, 3) => 6,
            (_, n) => u32::from(n) + 1,
        };
        return Some((blocks * 256, None));
    }
    None
}

/// The channel layout an AC-3 or E-AC-3 syncframe header states
/// (`acmod` and `lfeon`, ATSC A/52 5.4.2.3 and E.1.2.2), as `ffprobe` names
/// it; `None` for the arrangements it has no name for.
pub fn ac3_layout(frame: &[u8]) -> Option<&'static str> {
    if frame.get(..2)? != [0x0B, 0x77] {
        return None;
    }
    let bsid = frame.get(5)? >> 3;
    let (acmod, lfe) = if bsid <= 10 {
        // acmod, then the mix levels its value implies, then lfeon.
        let byte = u32::from(*frame.get(6)?) << 8 | u32::from(*frame.get(7)?);
        let acmod = byte >> 13;
        let mut at = 3;
        if acmod & 1 != 0 && acmod != 1 {
            at += 2;
        }
        if acmod & 4 != 0 {
            at += 2;
        }
        if acmod == 2 {
            at += 2;
        }
        (acmod, (byte >> (15 - at)) & 1 == 1)
    } else if bsid <= 16 {
        let byte = *frame.get(4)?;
        (u32::from((byte >> 1) & 7), byte & 1 == 1)
    } else {
        return None;
    };
    Some(match (acmod, lfe) {
        (1, false) => "mono",
        (0 | 2, false) => "stereo",
        (3, false) => "3.0",
        (4, false) => "3.0(back)",
        (5, false) => "4.0",
        (6, false) => "quad(side)",
        (7, false) => "5.0(side)",
        (2, true) => "2.1",
        (3, true) => "3.1",
        (5, true) => "4.1",
        (7, true) => "5.1(side)",
        _ => return None,
    })
}

/// The bit rate of an E-AC-3 syncframe: its size over its duration.
pub fn eac3_bit_rate(frame: &[u8]) -> Option<u32> {
    if frame.get(..2)? != [0x0B, 0x77] || !(11..=16).contains(&(frame.get(5)? >> 3)) {
        return None;
    }
    let size = (u32::from(frame.get(2)? & 0x07) << 8 | u32::from(*frame.get(3)?)) + 1;
    let byte = *frame.get(4)?;
    let fscod = byte >> 6;
    let (rate, blocks) = match fscod {
        3 => (
            [24_000, 22_050, 16_000]
                .get(usize::from((byte >> 4) & 3))
                .copied()?,
            6,
        ),
        _ => (
            [48_000u32, 44_100, 32_000]
                .get(usize::from(fscod))
                .copied()?,
            [1u32, 2, 3, 6].get(usize::from((byte >> 4) & 3)).copied()?,
        ),
    };
    let bits = u64::from(size) * 2 * 8 * u64::from(rate);
    u32::try_from(bits / (u64::from(blocks) * 256)).ok()
}

/// A DTS core frame header (ETSI TS 102 114 5.3.1): samples per frame,
/// bit rate, and whether only the core is present (no extension
/// substream after it).
pub fn dts(frame: &[u8]) -> Option<(u32, Option<u32>, bool)> {
    if frame.get(..4)? != [0x7F, 0xFE, 0x80, 0x01] {
        return None;
    }
    let b = |i: usize| u32::from(frame.get(i).copied().unwrap_or(0));
    let header = b(4) << 24 | b(5) << 16 | b(6) << 8 | b(7);
    // FTYPE(1) SHORT(5) CPF(1) NBLKS(7) FSIZE(14) AMODE(6 from bit 4)...
    let blocks = ((header >> 18) & 0x7F) + 1;
    let fsize = ((header >> 4) & 0x3FFF) + 1;
    let tail = b(8) << 24 | b(9) << 16 | b(10) << 8 | b(11);
    // AMODE ends at bit 4 of byte 8 (2 bits in `header`, 4 in `tail`),
    // then SFREQ(4), RATE(5).
    let rate_index = (tail >> 21) & 0x1F;
    const RATES: [u32; 29] = [
        32_000, 56_000, 64_000, 96_000, 112_000, 128_000, 192_000, 224_000, 256_000, 320_000,
        384_000, 448_000, 512_000, 576_000, 640_000, 768_000, 960_000, 1_024_000, 1_152_000,
        1_280_000, 1_344_000, 1_408_000, 1_411_200, 1_472_000, 1_536_000, 1_920_000, 2_048_000,
        3_072_000, 3_840_000,
    ];
    let rate = RATES.get(usize::try_from(rate_index).ok()?).copied();
    let core_only = frame.len() <= usize::try_from(fsize).ok()?
        || frame.get(usize::try_from(fsize).ok()?..usize::try_from(fsize).ok()? + 4)
            != Some(&[0x64, 0x58, 0x20, 0x25]);
    Some((blocks * 32, rate, core_only))
}

/// MPEG audio frame header: samples per frame and bit rate.
pub fn mpeg_audio(frame: &[u8]) -> Option<(u32, Option<u32>)> {
    let h = u32::from_be_bytes([*frame.first()?, *frame.get(1)?, *frame.get(2)?, 0]);
    if h >> 21 != 0x7FF {
        return None;
    }
    let version = (h >> 19) & 3; // 3 MPEG-1, 2 MPEG-2, 0 MPEG-2.5
    let layer = (h >> 17) & 3; // 3 layer I, 2 layer II, 1 layer III
    let index = usize::try_from((h >> 12) & 0xF).ok()?;
    if version == 1 || layer == 0 || index == 0 || index == 15 {
        return None;
    }
    let mpeg1 = version == 3;
    let samples = match (layer, mpeg1) {
        (3, _) => 384,
        (2, _) | (1, true) => 1152,
        _ => 576,
    };
    let table: [u32; 15] = match (mpeg1, layer) {
        (true, 3) => [
            0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
        ],
        (true, 2) => [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
        ],
        (true, _) => [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ],
        (false, 3) => [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
        ],
        (false, _) => [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    };
    Some((samples, Some(table.get(index)?.checked_mul(1000)?)))
}

/// The block size of a FLAC frame, from its header.
pub fn flac_samples(frame: &[u8]) -> Option<u32> {
    if frame.first()? != &0xFF || frame.get(1)? & 0xFE != 0xF8 {
        return None;
    }
    let code = frame.get(2)? >> 4;
    match code {
        1 => return Some(192),
        2..=5 => return Some(576 << (code - 2)),
        8..=15 => return Some(256 << (code - 8)),
        6 | 7 => {}
        _ => return None,
    }
    // The coded frame or sample number (UTF-8-like) follows byte 3.
    let lead = *frame.get(4)?;
    let len = match lead.leading_ones() {
        0 => 1,
        n @ 2..=7 => usize::try_from(n).ok()?,
        _ => return None,
    };
    let at = 4 + len;
    if code == 6 {
        Some(u32::from(*frame.get(at)?) + 1)
    } else {
        Some(u32::from(u16::from_be_bytes([*frame.get(at)?, *frame.get(at + 1)?])) + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_tocs() {
        // CELT 20 ms, one frame.
        assert_eq!(opus_samples(&[0xF8]), Some(960));
        // SILK 60 ms, two frames.
        assert_eq!(opus_samples(&[0x19]), Some(5760));
        // CELT 2.5 ms, code 3 with 4 frames.
        assert_eq!(opus_samples(&[0x83, 0x04]), Some(480));
        assert_eq!(opus_samples(&[]), None);
    }

    #[test]
    fn xiph_lacing() {
        let private = [2, 3, 1, b'a', b'b', b'c', b'd', b'e', b'f'];
        let headers = xiph_headers(&private).unwrap();
        assert_eq!(headers, vec![&b"abc"[..], &b"d"[..], &b"ef"[..]]);
        assert_eq!(xiph_headers(&[2, 255]), None);
    }

    #[test]
    fn frame_headers() {
        // AC-3, 48 kHz, frmsizecod 28 (384 kb/s), bsid 8.
        assert_eq!(
            ac3(&[0x0B, 0x77, 0, 0, 0x1C, 0x40]),
            Some((1536, Some(384_000)))
        );
        // MPEG-1 layer III, 128 kb/s.
        assert_eq!(mpeg_audio(&[0xFF, 0xFB, 0x90]), Some((1152, Some(128_000))));
        // FLAC, block size code 12 (4096).
        assert_eq!(flac_samples(&[0xFF, 0xF8, 0xC9, 0x08, 0x00]), Some(4096));
        // FLAC, block size code 7: 16-bit size minus one after the number.
        assert_eq!(
            flac_samples(&[0xFF, 0xF8, 0x79, 0x08, 0x00, 0x11, 0xFF]),
            Some(4608)
        );
    }

    #[test]
    fn syncframe_layouts_and_rates() {
        // E-AC-3, 384-byte frames of six blocks at 48 kHz, mono.
        let eac3 = [0x0B, 0x77, 0x00, 0xBF, 0x32, 0x87, 0xC0, 0x00];
        assert_eq!(eac3_bit_rate(&eac3), Some(96_000));
        assert_eq!(ac3_layout(&eac3), Some("mono"));
        // AC-3, acmod 2 with dsurmod, no LFE.
        let ac3 = [0x0B, 0x77, 0xFE, 0x95, 0x14, 0x40, 0x43, 0xE1];
        assert_eq!(ac3_layout(&ac3), Some("stereo"));
        // A DTS core of 16 blocks at 1411.2 kb/s.
        let dts_core = [
            0x7F, 0xFE, 0x80, 0x01, 0xFC, 0x3C, 0x75, 0xB0, 0xB6, 0xC0, 0x01, 0x38,
        ];
        assert_eq!(dts(&dts_core), Some((512, Some(1_411_200), true)));
    }
}
