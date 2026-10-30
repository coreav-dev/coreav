//! The MP4 forms of the audio codecs a copy carries, and their configuration
//! boxes: FLAC (the FLAC-in-ISOBMFF mapping RFC 9639 10.3 refers to: `fLaC`
//! with `dfLa`), Apple Lossless (`alac` with the magic cookie, as Apple's
//! magic cookie description stores it), AC-3 and E-AC-3 (ETSI TS 102 366
//! Annex F: `ac-3` with `dac3`, `ec-3` with `dec3`). The boxes are built
//! from Matroska's `CodecPrivate` or, for AC-3 and E-AC-3, which have none,
//! from the first frame's headers.

use coreav_isobmff::{CodecConfig, FourCc};

/// FLAC's `STREAMINFO` body (RFC 9639 8.2): 34 bytes.
const STREAMINFO: usize = 34;

/// An AC-3 syncframe, and an E-AC-3 sample (the syncframes that deliver six
/// audio blocks of every substream, ETSI TS 102 366 F.2), hold 1,536
/// samples of audio.
pub(crate) const AC3_FRAME: u32 = 1536;

/// The configuration box of the MP4 sample entries a copy writes.
pub(crate) fn config_kind(entry: FourCc) -> Option<FourCc> {
    let kind: &[u8; 4] = match &entry.0 {
        b"mp4a" => b"esds",
        b"Opus" => b"dOps",
        b"fLaC" => b"dfLa",
        b"alac" => b"alac",
        b"ac-3" => b"dac3",
        b"ec-3" => b"dec3",
        _ => return None,
    };
    Some(FourCc(*kind))
}

/// The `samplesize` of an audio sample entry: the bits per sample of FLAC
/// (`STREAMINFO`, FLAC-in-ISOBMFF 3.3.1) and Apple Lossless (the cookie's
/// `bitDepth`), else 16.
pub(crate) fn sample_size(config: &CodecConfig) -> u16 {
    let bits = match &config.kind.0 {
        b"dfLa" => dfla_streaminfo(&config.data)
            .and_then(stream_info)
            .map(|s| s.bits),
        b"alac" => alac_cookie(&config.data)
            .and_then(alac_config)
            .map(|a| a.bits),
        _ => None,
    };
    bits.unwrap_or(16)
}

/// The sample rate an audio sample entry's 16.16 field holds for a stream
/// at `rate` Hz: the rate itself up to 65,535 Hz, else its greatest
/// division by a power of two that fits, else 65,535 (FLAC-in-ISOBMFF
/// 3.3.1; Apple Lossless follows the same rule). The configuration keeps
/// the real rate.
pub(crate) fn entry_rate(rate: u32) -> u32 {
    let mut fitted = rate;
    while fitted > 65_535 && fitted % 2 == 0 {
        fitted /= 2;
    }
    fitted.min(65_535)
}

/// What a FLAC `STREAMINFO` says of the decoded audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StreamInfo {
    pub(crate) rate: u32,
    pub(crate) channels: u16,
    pub(crate) bits: u16,
}

fn stream_info(body: &[u8]) -> Option<StreamInfo> {
    let field = u64::from_be_bytes(body.get(10..18)?.try_into().ok()?);
    let rate = u32::try_from(field >> 44).ok()?;
    let channels = u16::try_from((field >> 41) & 7).ok()? + 1;
    let bits = u16::try_from((field >> 36) & 0x1F).ok()? + 1;
    (rate > 0).then_some(StreamInfo {
        rate,
        channels,
        bits,
    })
}

/// The length of the metadata block whose 4-byte header starts at `at`.
fn block_length(blocks: &[u8], at: usize) -> Option<usize> {
    let header = blocks.get(at..at.checked_add(4)?)?;
    Some(
        usize::from(*header.get(1)?) << 16
            | usize::from(*header.get(2)?) << 8
            | usize::from(*header.get(3)?),
    )
}

/// The `dfLa` payload (a version 0 full box, then the metadata blocks) for
/// Matroska's FLAC `CodecPrivate` (`fLaC`, then the blocks; RFC 9639
/// 10.2), and what its `STREAMINFO` says: the chain up to the block marked
/// last, which is marked last when the chain ends without one. `None` when
/// the chain does not parse or does not start with `STREAMINFO`.
pub(crate) fn dfla_from_codec_private(private: &[u8]) -> Option<(Vec<u8>, StreamInfo)> {
    let blocks = private.strip_prefix(b"fLaC")?;
    let mut start = 0usize;
    let mut end = 0usize;
    let mut last = false;
    while !last && end < blocks.len() {
        start = end;
        last = blocks.get(start)? & 0x80 != 0;
        end = start
            .checked_add(4)?
            .checked_add(block_length(blocks, start)?)?;
        if end > blocks.len() {
            return None;
        }
    }
    let mut dfla = vec![0; 4];
    dfla.extend_from_slice(blocks.get(..end)?);
    let info = dfla_streaminfo(&dfla).and_then(stream_info)?;
    if !last {
        *dfla.get_mut(4 + start)? |= 0x80;
    }
    Some((dfla, info))
}

/// The `STREAMINFO` body of a `dfLa` payload: version 0, then a first block
/// of type 0 and 34 bytes.
fn dfla_streaminfo(dfla: &[u8]) -> Option<&[u8]> {
    if dfla.get(..4)? != [0, 0, 0, 0] || dfla.get(4)? & 0x7F != 0 {
        return None;
    }
    if dfla.get(5..8)? != [0, 0, 34] {
        return None;
    }
    dfla.get(8..8 + STREAMINFO)
}

/// A `dfLa` payload with `STREAMINFO` alone, for sanitization: the other
/// blocks (Vorbis comments, pictures, application data) carry text and
/// images. Returns the payload and whether anything was left out.
pub(crate) fn dfla_streaminfo_only(dfla: &[u8]) -> Option<(Vec<u8>, bool)> {
    let body = dfla_streaminfo(dfla)?;
    let mut out = vec![0, 0, 0, 0, 0x80, 0, 0, 34];
    out.extend_from_slice(body);
    let changed = out != dfla;
    Some((out, changed))
}

/// The configuration of a copy that holds part of a source's samples, or
/// samples of several sources (a clip, a split, a join): the fields that
/// describe one whole stream get their "unknown" values. FLAC's
/// `STREAMINFO` loses its frame sizes, total samples and MD5 signature
/// (RFC 9639 8.2: 0 means unknown), Apple Lossless's cookie its
/// `maxFrameBytes` and `avgBitRate` (0 means unknown in the cookie
/// description). Other configurations are returned as stored.
pub(crate) fn excerpt_config(config: &CodecConfig) -> Vec<u8> {
    let mut data = config.data.clone();
    match &config.kind.0 {
        b"dfLa" if dfla_streaminfo(&data).is_some() => {
            // In the body (from byte 8): the frame sizes at 4..10, total
            // samples in the low four bits of 13 and in 14..18, then the
            // signature.
            if let Some(body) = data.get_mut(8..8 + STREAMINFO) {
                for (at, byte) in body.iter_mut().enumerate() {
                    match at {
                        4..10 | 14.. => *byte = 0,
                        13 => *byte &= 0xF0,
                        _ => {}
                    }
                }
            }
        }
        b"alac" => {
            let at = if data.get(..4) == Some(&[0, 0, 0, 0]) {
                4
            } else {
                0
            };
            if let Some(fields) = data.get_mut(at + 12..at + 20) {
                fields.fill(0);
            }
        }
        _ => {}
    }
    data
}

/// What must agree for streams to decode alike, to compare the inputs of a
/// join: FLAC's `STREAMINFO` and Apple Lossless's cookie without their
/// per-stream fields ([`excerpt_config`]), other configurations as stored.
pub(crate) fn decoding_config(config: &CodecConfig) -> Vec<u8> {
    let excerpt = excerpt_config(config);
    match &config.kind.0 {
        b"dfLa" => dfla_streaminfo(&excerpt).map_or_else(|| excerpt.clone(), <[u8]>::to_vec),
        _ => excerpt,
    }
}

/// The block size of a FLAC frame from its header (RFC 9639 9.1.1): the
/// samples it decodes to, or `None` when `head`, the frame's first bytes,
/// is not a frame header.
pub(crate) fn flac_block_size(head: &[u8]) -> Option<u32> {
    if *head.first()? != 0xFF || head.get(1)? & 0xFE != 0xF8 {
        return None;
    }
    let code = head.get(2)? >> 4;
    match code {
        1 => return Some(192),
        2..=5 => return Some(144 << code),
        8..=15 => return Some(1 << code),
        6 | 7 => {}
        _ => return None,
    }
    // The coded number (RFC 9639 9.1.5) starts at byte 4; its first byte's
    // leading ones count its bytes.
    let length = match head.get(4)?.leading_ones() {
        0 => 1,
        n @ 2..=7 => usize::try_from(n).ok()?,
        _ => return None,
    };
    let at = 4 + length;
    let stored = if code == 6 {
        u32::from(*head.get(at)?)
    } else {
        u32::from(u16::from_be_bytes([*head.get(at)?, *head.get(at + 1)?]))
    };
    Some(stored + 1)
}

/// What an Apple Lossless cookie (`ALACSpecificConfig`) says of the decoded
/// audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AlacConfig {
    pub(crate) frame_length: u32,
    pub(crate) bits: u16,
    pub(crate) channels: u16,
    pub(crate) rate: u32,
}

/// The cookie in an `alac` box payload: after the full box's version and
/// flags, or the whole payload when it has none (a cookie starts with
/// `frameLength`, never 0).
fn alac_cookie(payload: &[u8]) -> Option<&[u8]> {
    match payload.get(..4)? {
        [0, 0, 0, 0] => payload.get(4..),
        _ => Some(payload),
    }
}

fn alac_config(cookie: &[u8]) -> Option<AlacConfig> {
    let word = |at: usize| Some(u32::from_be_bytes(cookie.get(at..at + 4)?.try_into().ok()?));
    let config = AlacConfig {
        frame_length: word(0)?,
        bits: u16::from(*cookie.get(5)?),
        channels: u16::from(*cookie.get(9)?),
        rate: word(20)?,
    };
    let valid = *cookie.get(4)? == 0
        && config.frame_length > 0
        && (1..=32).contains(&config.bits)
        && config.channels > 0
        && config.rate > 0;
    valid.then_some(config)
}

/// The `alac` box payload for Matroska's Apple Lossless `CodecPrivate`, the
/// magic cookie (24 bytes, or 48 with the channel layout), with or without
/// the 12-byte atom header some writers keep: version 0 and flags, then the
/// cookie. `None` when the cookie does not parse.
pub(crate) fn alac_from_codec_private(private: &[u8]) -> Option<(Vec<u8>, AlacConfig)> {
    let cookie = match private.get(4..8) {
        Some(b"alac") => private.get(12..)?,
        _ => private,
    };
    let config = alac_config(cookie)?;
    let length = if cookie.len() < 48 { 24 } else { 48 };
    let mut payload = vec![0; 4];
    payload.extend_from_slice(cookie.get(..length)?);
    Some((payload, config))
}

/// The sample rates of AC-3's and E-AC-3's `fscod` (ETSI TS 102 366 4.4.1.3,
/// E.1.3.1.4), and of E-AC-3's `fscod2`.
const RATES: [u32; 3] = [48_000, 44_100, 32_000];
const HALF_RATES: [u32; 3] = [24_000, 22_050, 16_000];

/// A big-endian bit reader.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn read(&mut self, width: usize) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..width {
            let byte = self.data.get(self.at / 8)?;
            value = value << 1 | u32::from(byte >> (7 - self.at % 8) & 1);
            self.at += 1;
        }
        Some(value)
    }

    fn flag(&mut self) -> Option<bool> {
        self.read(1).map(|bit| bit == 1)
    }

    fn skip(&mut self, width: usize) -> Option<()> {
        self.at = self.at.checked_add(width)?;
        (self.at <= self.data.len().checked_mul(8)?).then_some(())
    }
}

/// Bits written most significant first, the last byte padded with zeros.
#[derive(Default)]
struct Packer {
    out: Vec<u8>,
    acc: u64,
    filled: u32,
}

impl Packer {
    fn put(&mut self, value: u32, width: u32) {
        self.acc = self.acc << width | u64::from(value) & ((1 << width) - 1);
        self.filled += width;
        while self.filled >= 8 {
            self.filled -= 8;
            self.out
                .push(u8::try_from(self.acc >> self.filled & 0xFF).unwrap_or(0));
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.filled > 0 {
            self.put(0, 8 - self.filled);
        }
        self.out
    }
}

/// The `dac3` payload (ETSI TS 102 366 F.4) for an AC-3 syncframe, the
/// stream's sample rate, and the syncframe's size in bytes (Table 4.13;
/// `None` for the reduced rates of `bsid` 9 and 10); `None` when `frame` does
/// not start with an AC-3 syncframe header.
pub(crate) fn dac3(frame: &[u8]) -> Option<(Vec<u8>, u32, Option<usize>)> {
    if frame.get(..2)? != [0x0B, 0x77] {
        return None;
    }
    let mut bits = Bits {
        data: frame,
        at: 32,
    };
    let fscod = bits.read(2)?;
    let frmsizecod = bits.read(6)?;
    let bsid = bits.read(5)?;
    let bsmod = bits.read(3)?;
    let acmod = bits.read(3)?;
    if bsid > 10 || frmsizecod > 37 {
        return None;
    }
    // cmixlev, surmixlev and dsurmod, as acmod has them (4.3.2).
    if acmod & 1 != 0 && acmod != 1 {
        bits.skip(2)?;
    }
    if acmod & 4 != 0 {
        bits.skip(2)?;
    }
    if acmod == 2 {
        bits.skip(2)?;
    }
    let lfeon = bits.read(1)?;
    // bsid 9 and 10 halve and quarter the rate.
    let base = *RATES.get(usize::try_from(fscod).ok()?)?;
    let rate = base >> bsid.saturating_sub(8);
    // 1,536 samples at the bit rate, in 16-bit words: 44.1 kHz rounds down,
    // and odd codes add a word (Table 4.13).
    const KBPS: [u32; 19] = [
        32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
    ];
    let kbps = *KBPS.get(usize::try_from(frmsizecod >> 1).ok()?)?;
    let words = kbps * 96_000 / base + if fscod == 1 { frmsizecod & 1 } else { 0 };
    let bytes = usize::try_from(words * 2).ok().filter(|_| bsid <= 8);
    let mut packer = Packer::default();
    for (value, width) in [
        (fscod, 2),
        (bsid, 5),
        (bsmod, 3),
        (acmod, 3),
        (lfeon, 1),
        (frmsizecod >> 1, 5),
        (0, 5),
    ] {
        packer.put(value, width);
    }
    Some((packer.finish(), rate, bytes))
}

/// One independent E-AC-3 substream as `dec3` describes it.
struct Substream {
    header: Eac3Header,
    dependents: u32,
    chan_loc: u32,
}

/// The `dec3` payload (ETSI TS 102 366 F.6) for one E-AC-3 sample, the
/// syncframes that deliver six audio blocks of every substream (F.2), and
/// the stream's sample rate. Fails when the frames are not E-AC-3 or do not
/// make exactly one sample.
pub(crate) fn dec3(sample: &[u8]) -> Result<(Vec<u8>, u32), &'static str> {
    const NOT: &str = "an E-AC-3 frame whose headers do not parse";
    let mut independent: Vec<Substream> = Vec::new();
    let mut blocks = 0u32;
    let mut cycles = 0u32;
    let mut next_id = 0u32;
    let mut at = 0usize;
    while at < sample.len() {
        let frame = sample.get(at..).ok_or(NOT)?;
        if frame.get(..2) != Some(&[0x0B, 0x77]) {
            return Err(NOT);
        }
        let header = eac3_header(&mut Bits {
            data: frame,
            at: 16,
        })
        .ok_or(NOT)?;
        at = at
            .checked_add(header.size)
            .filter(|end| *end <= sample.len())
            .ok_or(NOT)?;
        // Syncframes of fewer than six blocks repeat the substreams in
        // turn; the first turn describes them.
        if header.strmtyp == 1 {
            let owner = independent.last_mut().ok_or(NOT)?;
            if cycles > 1 {
                continue;
            }
            owner.dependents = owner.dependents.max(header.substreamid + 1);
            if let Some(chanmap) = header.chanmap {
                // chan_loc bits 0 to 7 are chanmap locations 5 to 12 and bit
                // 8 is location 14 (LFE2); chanmap's location 0 is its most
                // significant bit (E.1.3.1.8, Table F.6.1).
                for (bit, location) in (5..=12).chain([14]).enumerate() {
                    if chanmap >> (15 - location) & 1 == 1 {
                        owner.chan_loc |= 1 << bit;
                    }
                }
            }
            continue;
        }
        if header.substreamid == 0 {
            blocks += header.blocks;
            cycles += 1;
            next_id = 0;
        }
        if header.substreamid != next_id {
            return Err("E-AC-3 independent substreams out of order");
        }
        next_id += 1;
        if cycles == 1 {
            independent.push(Substream {
                header,
                dependents: 0,
                chan_loc: 0,
            });
        }
    }
    let Some(first) = independent.first() else {
        return Err(NOT);
    };
    if blocks != 6 || at != sample.len() {
        return Err("E-AC-3 frames of other than six audio blocks in one Matroska frame");
    }
    let rate = first.header.rate;
    let bits = u64::try_from(sample.len()).map_err(|_| NOT)? * 8;
    let data_rate = (bits * u64::from(rate) / (u64::from(AC3_FRAME) * 1000)).min(8191);
    let mut packer = Packer::default();
    packer.put(u32::try_from(data_rate).map_err(|_| NOT)?, 13);
    packer.put(u32::try_from(independent.len() - 1).map_err(|_| NOT)?, 3);
    for s in &independent {
        let h = &s.header;
        for (value, width) in [
            (h.fscod, 2),
            (h.bsid, 5),
            (0, 1),
            (0, 1),
            (h.bsmod, 3),
            (h.acmod, 3),
            (h.lfeon, 1),
            (0, 3),
            (s.dependents, 4),
        ] {
            packer.put(value, width);
        }
        if s.dependents > 0 {
            packer.put(s.chan_loc, 9);
        } else {
            packer.put(0, 1);
        }
    }
    Ok((packer.finish(), rate))
}

/// The fields of an E-AC-3 syncframe header that `dec3` and the sample's
/// extent need.
struct Eac3Header {
    strmtyp: u32,
    substreamid: u32,
    size: usize,
    fscod: u32,
    rate: u32,
    blocks: u32,
    acmod: u32,
    lfeon: u32,
    bsid: u32,
    chanmap: Option<u32>,
    bsmod: u32,
}

/// Reads `bsi()` (ETSI TS 102 366 E.1.2.2) up to `bsmod`, which is 0 when
/// `infomdate` is 0 (F.6.2.8).
fn eac3_header(bits: &mut Bits<'_>) -> Option<Eac3Header> {
    let strmtyp = bits.read(2)?;
    let substreamid = bits.read(3)?;
    let size = (usize::try_from(bits.read(11)?).ok()? + 1) * 2;
    let fscod = bits.read(2)?;
    let (rate, numblkscod) = if fscod == 3 {
        (*HALF_RATES.get(usize::try_from(bits.read(2)?).ok()?)?, 3)
    } else {
        (*RATES.get(usize::try_from(fscod).ok()?)?, bits.read(2)?)
    };
    let blocks = *[1, 2, 3, 6].get(usize::try_from(numblkscod).ok()?)?;
    let acmod = bits.read(3)?;
    let lfeon = bits.read(1)?;
    let bsid = bits.read(5)?;
    if strmtyp == 3 || !(11..=16).contains(&bsid) {
        return None;
    }
    bits.skip(5)?;
    if bits.flag()? {
        bits.skip(8)?;
    }
    if acmod == 0 {
        bits.skip(5)?;
        if bits.flag()? {
            bits.skip(8)?;
        }
    }
    let mut chanmap = None;
    if strmtyp == 1 && bits.flag()? {
        chanmap = Some(bits.read(16)?);
    }
    if bits.flag()? {
        mixing_metadata(bits, strmtyp, acmod, lfeon, numblkscod, blocks)?;
    }
    let bsmod = if bits.flag()? { bits.read(3)? } else { 0 };
    Some(Eac3Header {
        strmtyp,
        substreamid,
        size,
        fscod,
        rate,
        blocks,
        acmod,
        lfeon,
        bsid,
        chanmap,
        bsmod,
    })
}

/// Skips the mixing metadata of `bsi()` (the `mixmdate` branch).
fn mixing_metadata(
    bits: &mut Bits<'_>,
    strmtyp: u32,
    acmod: u32,
    lfeon: u32,
    numblkscod: u32,
    blocks: u32,
) -> Option<()> {
    if acmod > 2 {
        bits.skip(2)?;
    }
    if acmod & 1 != 0 && acmod > 2 {
        bits.skip(6)?;
    }
    if acmod & 4 != 0 {
        bits.skip(6)?;
    }
    if lfeon == 1 && bits.flag()? {
        bits.skip(5)?;
    }
    if strmtyp != 0 {
        return Some(());
    }
    if bits.flag()? {
        bits.skip(6)?;
    }
    if acmod == 0 && bits.flag()? {
        bits.skip(6)?;
    }
    if bits.flag()? {
        bits.skip(6)?;
    }
    match bits.read(2)? {
        1 => bits.skip(5)?,
        2 => bits.skip(12)?,
        // `mixdeflen` gives the mixing data's length in bytes, less 2.
        3 => {
            let length = usize::try_from(bits.read(5)?).ok()? + 2;
            bits.skip(length * 8)?;
        }
        _ => {}
    }
    if acmod < 2 {
        if bits.flag()? {
            bits.skip(14)?;
        }
        if acmod == 0 && bits.flag()? {
            bits.skip(14)?;
        }
    }
    if bits.flag()? {
        if numblkscod == 0 {
            bits.skip(5)?;
        } else {
            for _ in 0..blocks {
                if bits.flag()? {
                    bits.skip(5)?;
                }
            }
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(fields: &[(u32, u32)]) -> Vec<u8> {
        let mut packer = Packer::default();
        for &(value, width) in fields {
            packer.put(value, width);
        }
        packer.finish()
    }

    fn streaminfo(rate: u32, channels: u32, bits: u32) -> Vec<u8> {
        pack(&[
            (4096, 16),
            (4096, 16),
            (14, 24),
            (12_288, 24),
            (rate, 20),
            (channels - 1, 3),
            (bits - 1, 5),
            (0, 4),
            (44_100, 32),
            (0xABCD_ABCD, 32),
            (0xABCD_ABCD, 32),
            (0xABCD_ABCD, 32),
            (0xABCD_ABCD, 32),
        ])
    }

    #[test]
    fn flac_codec_private_becomes_dfla() {
        let mut private = b"fLaC".to_vec();
        private.extend_from_slice(&[0, 0, 0, 34]);
        private.extend_from_slice(&streaminfo(96_000, 2, 24));
        private.extend_from_slice(&[4, 0, 0, 2, b'h', b'i']);
        let (dfla, info) = dfla_from_codec_private(&private).unwrap();
        assert_eq!(
            info,
            StreamInfo {
                rate: 96_000,
                channels: 2,
                bits: 24
            }
        );
        assert_eq!(&dfla[..4], &[0, 0, 0, 0]);
        // The chain ended without a last block: its last block is marked.
        assert_eq!(dfla[4 + 4 + 34], 0x84);
        let config = CodecConfig {
            kind: FourCc(*b"dfLa"),
            data: dfla.clone(),
        };
        assert_eq!(sample_size(&config), 24);
        let (alone, changed) = dfla_streaminfo_only(&dfla).unwrap();
        assert!(changed);
        assert_eq!(alone.len(), 8 + 34);
        assert_eq!(alone[4], 0x80);
        let excerpt = excerpt_config(&config);
        assert_eq!(excerpt.len(), dfla.len());
        let body = &excerpt[8..42];
        assert_eq!(&body[..4], &streaminfo(96_000, 2, 24)[..4]);
        assert_eq!(&body[4..10], &[0; 6]);
        assert_eq!(
            &body[13..],
            &streaminfo(96_000, 2, 24)[13..14]
                .iter()
                .map(|b| b & 0xF0)
                .chain([0; 20])
                .collect::<Vec<_>>()[..]
        );
        assert_eq!(stream_info(body), Some(info));
        assert_eq!(decoding_config(&config), body);
        assert!(dfla_from_codec_private(b"fLaC\x00\x00\x00\x40").is_none());
        assert!(dfla_from_codec_private(&private[4..]).is_none());
    }

    #[test]
    fn entry_rates_fit_sixteen_bits() {
        assert_eq!(entry_rate(44_100), 44_100);
        assert_eq!(entry_rate(96_000), 48_000);
        assert_eq!(entry_rate(192_000), 48_000);
        assert_eq!(entry_rate(176_400), 44_100);
        assert_eq!(entry_rate(70_001), 65_535);
    }

    #[test]
    fn flac_block_sizes() {
        assert_eq!(flac_block_size(&[0xFF, 0xF8, 0xC9, 0x18, 0x00]), Some(4096));
        assert_eq!(flac_block_size(&[0xFF, 0xF8, 0x19, 0x18, 0x00]), Some(192));
        assert_eq!(flac_block_size(&[0xFF, 0xF8, 0x29, 0x18, 0x00]), Some(576));
        // An 8-bit block size after a two-byte coded number.
        assert_eq!(
            flac_block_size(&[0xFF, 0xF9, 0x69, 0x18, 0xC2, 0x80, 99]),
            Some(100)
        );
        assert_eq!(
            flac_block_size(&[0xFF, 0xF8, 0x79, 0x18, 0x01, 0x0F, 0xFF]),
            Some(4096)
        );
        assert_eq!(flac_block_size(&[0xFF, 0xF8, 0x09, 0x18, 0x00]), None);
        assert_eq!(flac_block_size(&[0x0B, 0x77, 0xC9]), None);
    }

    fn cookie() -> Vec<u8> {
        pack(&[
            (4096, 32),
            (0, 8),
            (16, 8),
            (40, 8),
            (10, 8),
            (14, 8),
            (2, 8),
            (255, 16),
            (1234, 32),
            (700_000, 32),
            (44_100, 32),
        ])
    }

    #[test]
    fn alac_cookies() {
        let (payload, config) = alac_from_codec_private(&cookie()).unwrap();
        assert_eq!(&payload[..4], &[0, 0, 0, 0]);
        assert_eq!(&payload[4..], &cookie()[..]);
        assert_eq!(
            config,
            AlacConfig {
                frame_length: 4096,
                bits: 16,
                channels: 2,
                rate: 44_100
            }
        );
        let mut atom = 36u32.to_be_bytes().to_vec();
        atom.extend_from_slice(b"alac");
        atom.extend_from_slice(&[0; 4]);
        atom.extend_from_slice(&cookie());
        assert_eq!(alac_from_codec_private(&atom).unwrap().0, payload);
        let config = CodecConfig {
            kind: FourCc(*b"alac"),
            data: payload,
        };
        assert_eq!(sample_size(&config), 16);
        let excerpt = excerpt_config(&config);
        assert_eq!(&excerpt[16..24], &[0; 8]);
        assert_eq!(&excerpt[24..], &44_100u32.to_be_bytes());
        let mut bad = cookie();
        bad[4] = 1;
        assert!(alac_from_codec_private(&bad).is_none());
    }

    #[test]
    fn ac3_syncframe_configures_dac3() {
        // 48 kHz, 384 kbit/s (frmsizecod 28), bsid 8, bsmod 0, 3/2 with
        // LFE: cmixlev and surmixlev are present.
        let frame = pack(&[
            (0x0B77, 16),
            (0, 16),
            (0, 2),
            (28, 6),
            (8, 5),
            (0, 3),
            (7, 3),
            (1, 2),
            (1, 2),
            (1, 1),
        ]);
        let (payload, rate, bytes) = dac3(&frame).unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(bytes, Some(1536));
        assert_eq!(
            payload,
            pack(&[(0, 2), (8, 5), (0, 3), (7, 3), (1, 1), (14, 5), (0, 5)])
        );
        assert!(dac3(&frame[1..]).is_none());
    }

    fn eac3_frame(
        strmtyp: u32,
        id: u32,
        numblkscod: u32,
        acmod: u32,
        extra: &[(u32, u32)],
        words: usize,
    ) -> Vec<u8> {
        let mut fields = vec![
            (0x0B77, 16),
            (strmtyp, 2),
            (id, 3),
            (u32::try_from(words - 1).unwrap(), 11),
            (0, 2),
            (numblkscod, 2),
            (acmod, 3),
            (1, 1),
            (16, 5),
            (31, 5),
            (0, 1),
        ];
        fields.extend_from_slice(extra);
        let mut frame = pack(&fields);
        frame.resize(words * 2, 0);
        frame
    }

    #[test]
    fn eac3_sample_configures_dec3() {
        // One independent substream of six blocks, 5.1, no mixing
        // metadata, informational metadata with bsmod 2.
        let frame = eac3_frame(0, 0, 3, 7, &[(0, 1), (1, 1), (2, 3)], 384);
        let (payload, rate) = dec3(&frame).unwrap();
        assert_eq!(rate, 48_000);
        // 768 bytes per 1,536 samples at 48 kHz: 192 kbit/s.
        let one = [(0, 2), (16, 5), (0, 2), (2, 3), (7, 3), (1, 1), (0, 3)];
        let mut expected = vec![(192, 13), (0, 3)];
        expected.extend_from_slice(&one);
        expected.extend_from_slice(&[(0, 4), (0, 1)]);
        assert_eq!(payload, pack(&expected));

        // A dependent substream whose channel map holds Lrs/Rrs (location
        // 6) follows: 7.1.
        let mut sample = frame.clone();
        sample.extend(eac3_frame(
            1,
            0,
            3,
            2,
            &[(1, 1), (1 << (15 - 6), 16), (0, 1), (0, 1)],
            128,
        ));
        let (payload, _) = dec3(&sample).unwrap();
        let mut expected = vec![(256, 13), (0, 3)];
        expected.extend_from_slice(&one);
        expected.extend_from_slice(&[(1, 4), (0b10, 9)]);
        assert_eq!(payload, pack(&expected));

        // Two three-block syncframes of substream 0 make a sample; one does
        // not, nor does a cut frame.
        let half = eac3_frame(0, 0, 2, 2, &[(0, 1), (0, 1)], 192);
        let mut two = half.clone();
        two.extend_from_slice(&half);
        assert!(dec3(&two).is_ok());
        assert!(dec3(&half).is_err());
        assert!(dec3(&frame[..100]).is_err());
    }
}
