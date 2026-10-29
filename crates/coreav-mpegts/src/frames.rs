//! Audio frames inside a PES payload: the sync frames of ADTS AAC (ISO/IEC
//! 13818-7 6.2 and 14496-3 1.A.2), LOAS/LATM AAC (14496-3 1.7), MPEG audio
//! (ISO/IEC 11172-3 2.4.2.3 and 13818-3 2.4.2.3), AC-3 and E-AC-3 (ATSC
//! A/52 5.3 and annex E), and Opus access units (the Opus transport
//! multiplexing draft, ETSI TS 1xx xxx V0.1.3, section 6; RFC 6716 3.1 for
//! the packet's duration), and DTS core frames and extension substreams
//! (ETSI TS 102 114 5.3 and 7.5). Each frame is one packet of the listing.
//! Sans-IO.

/// What a frame header says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    /// Bytes of the whole frame, header included.
    pub len: usize,
    /// Bytes at its start that are not part of the packet's data (an Opus
    /// control header).
    pub header: usize,
    /// Audio samples per channel the frame decodes to.
    pub samples: u32,
    /// Samples per second.
    pub sample_rate: u32,
    /// Channels, when the header states them (0 for an ADTS
    /// `channel_configuration` of 0, whose channels are in the payload).
    pub channels: u8,
    /// Codec detail: the AAC audio object type, the MPEG audio layer, 1
    /// for an E-AC-3 frame (`bsid` 11 to 16, A/52 annex E) against 0 for
    /// AC-3, which is what tells the two apart, or
    /// [`DTS_SUBSTREAM`] for a DTS extension substream.
    pub detail: u8,
    /// An E-AC-3 dependent substream (`strmtyp` 1, A/52 annex E 2.3.1.1),
    /// part of the access unit of the independent frame before it.
    pub dependent: bool,
}

/// Which frame syntax a stream carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Syntax {
    Adts,
    MpegAudio,
    /// AC-3 and E-AC-3 share the sync word; the header tells them apart.
    Ac3,
    /// LOAS frames of LATM AAC (14496-3 1.7.2, `AudioSyncStream`).
    Latm,
    /// Opus access units.
    Opus,
    /// DTS: core frames and extension substreams share a stream; the
    /// header tells them apart ([`DTS_SUBSTREAM`]).
    Dts,
}

/// [`Frame::detail`] of a DTS extension substream; a core frame's is 0.
pub(crate) const DTS_SUBSTREAM: u8 = 1;

/// Whether `data`, shorter than a frame header of `syntax`, could be the
/// start of one: its bytes so far agree with the sync word. Such a tail of
/// a PES payload is kept for the next one, where the header may go on.
pub(crate) fn may_start(syntax: Syntax, data: &[u8]) -> bool {
    let (need, sync) = match syntax {
        Syntax::Adts => (
            7,
            (
                data.first() == Some(&0xFF),
                data.get(1).is_none_or(|b| b & 0xF6 == 0xF0),
            ),
        ),
        Syntax::MpegAudio => (
            4,
            (
                data.first() == Some(&0xFF),
                data.get(1).is_none_or(|b| b & 0xE0 == 0xE0),
            ),
        ),
        Syntax::Ac3 => (
            8,
            (
                data.first() == Some(&0x0B),
                data.get(1).is_none_or(|b| *b == 0x77),
            ),
        ),
        Syntax::Latm => (
            3,
            (
                data.first() == Some(&0x56),
                data.get(1).is_none_or(|b| b & 0xE0 == 0xE0),
            ),
        ),
        // A control header runs on while its size bytes are 0xFF: bounded
        // by the largest access unit.
        Syntax::Opus => (
            3 + MAX_OPUS_ACCESS_UNIT / 255,
            (
                data.first() == Some(&0x7F),
                data.get(1).is_none_or(|b| b & 0xE0 == 0xE0),
            ),
        ),
        // Either sync word, as far as the bytes go.
        Syntax::Dts => {
            let agrees = |sync: &[u8; 4]| sync.iter().zip(data).all(|(a, b)| a == b);
            (
                DTS_HEADER_BYTES,
                (agrees(&DTS_CORE_SYNC) || agrees(&DTS_SUBSTREAM_SYNC), true),
            )
        }
    };
    !data.is_empty() && data.len() < need && sync.0 && sync.1
}

/// Most frames of one PES payload looked at for one that states the
/// stream's rate.
const MAX_FRAMES_READ: usize = 64;

/// Most bytes of one PES payload searched for the first frame header:
/// the bytes before it are the tail of a frame that began in an earlier
/// PES packet, so one frame of the syntax bounds them. Each is the
/// longest the parser above accepts: an ADTS `aac_frame_length` of 13
/// bits; a LOAS `audioMuxLengthBytes` of 13 bits and its three-byte
/// header; an E-AC-3 `frmsiz` of 11 bits counting 16-bit words, which
/// is longer than any AC-3 frame (3,840 bytes); MPEG audio's longest,
/// which its bit rate table gives rather than a field -- MPEG-2.5
/// layer II at 160 kbit/s and 8 kHz, with its padding byte. An Opus
/// access unit runs to
/// [`MAX_OPUS_ACCESS_UNIT`] and takes the cap below instead.
fn longest_frame(syntax: Syntax) -> usize {
    match syntax {
        Syntax::Adts => 8_191,
        Syntax::Latm => 8_194,
        Syntax::Ac3 => 4_096,
        Syntax::MpegAudio => 2_881,
        Syntax::Opus | Syntax::Dts => MAX_SEARCH_BYTES,
    }
}

/// The cap on that search for a syntax whose frames are longer than any
/// payload a reader need look through.
const MAX_SEARCH_BYTES: usize = 64 << 10;

/// The first whole frame of a PES payload that states a sample rate, with
/// where it starts. A LOAS frame carries the stream's configuration only
/// now and then (`useSameStreamMux` leaves it out of the others), and the
/// first frame of a PES packet is commonly one of the others.
///
/// The search starts where a header reads, not at the start of the
/// payload: a PES packet commonly begins inside a frame that began in the
/// one before it, which is how the whole of a broadcast capture's
/// 1-segment AAC is written.
///
/// A sync word is a few bits (eleven for LOAS), so arbitrary bytes hold
/// one every few kilobytes. The search therefore prefers a place the
/// payload confirms -- the frame fits in it and ends at another header
/// or at its end, and where both frames state a sample rate the two
/// agree -- and takes the first header that merely reads only where the
/// payload confirms none. A muxer whose frames are longer than its PES
/// payloads confirms nothing anywhere, and a stream of its kind must
/// still be read; a payload that confirms one place is the common one,
/// and there a false sync loses to the real frame: its declared length
/// is one the payload usually cannot hold, and where it does land on
/// the real frame it names another rate.
pub(crate) fn first_with_rate(syntax: Syntax, data: &[u8]) -> Option<(usize, Frame)> {
    // The frame a candidate heads must fit in the payload and end at
    // another header or at the payload's end, and where both state a
    // sample rate the two must agree: a false sync whose declared length
    // happens to land on the real frame states the rate of nothing, and
    // the real frame states the stream's.
    let confirms = |at: usize, frame: &Frame| {
        let Some(next) = at.checked_add(frame.len).filter(|next| *next <= data.len()) else {
            return false;
        };
        if frame.len == 0 {
            return false;
        }
        if next == data.len() {
            return true;
        }
        let Some(after) = header(syntax, data.get(next..).unwrap_or_default()) else {
            return false;
        };
        !(frame.sample_rate > 0 && after.sample_rate > 0 && frame.sample_rate != after.sample_rate)
    };
    // One pass, one header read per byte looked at: the place the
    // payload confirms if there is one, else the first that reads.
    let bound = data.len().min(longest_frame(syntax));
    let (mut at, mut frame) = {
        let mut reads = None;
        let mut confirmed = None;
        for at in 0..bound {
            let Some(frame) = header(syntax, data.get(at..).unwrap_or_default()) else {
                continue;
            };
            if reads.is_none() {
                reads = Some((at, frame));
            }
            if confirms(at, &frame) {
                confirmed = Some((at, frame));
                break;
            }
        }
        confirmed.or(reads)?
    };
    for _ in 0..MAX_FRAMES_READ {
        if frame.sample_rate > 0 {
            return Some((at, frame));
        }
        if frame.len == 0 || at.checked_add(frame.len)? > data.len() {
            return None;
        }
        at = at.checked_add(frame.len)?;
        frame = header(syntax, data.get(at..)?)?;
    }
    None
}

/// Reads the frame header at the start of `data`; `None` when it is not
/// one (or is cut short).
pub(crate) fn header(syntax: Syntax, data: &[u8]) -> Option<Frame> {
    match syntax {
        Syntax::Adts => adts(data),
        Syntax::MpegAudio => mpeg_audio(data),
        Syntax::Ac3 => ac3(data),
        Syntax::Latm => latm(data),
        Syntax::Opus => opus(data),
        Syntax::Dts => dts(data),
    }
}

/// The sync words of a DTS core frame and of an extension substream (ETSI
/// TS 102 114 5.3.1 and 7.5.2).
const DTS_CORE_SYNC: [u8; 4] = [0x7F, 0xFE, 0x80, 0x01];
const DTS_SUBSTREAM_SYNC: [u8; 4] = [0x64, 0x58, 0x20, 0x25];
/// Bytes that hold every field [`dts`] reads of either header: the core
/// header's first fields (88 bits after the sync word), the substream
/// header's sizes and its static fields up to the first asset size.
const DTS_HEADER_BYTES: usize = 16;

/// Core sample rates by `SFREQ` (TS 102 114 table 5-5); 0 is not allowed.
const DTS_CORE_RATES: [u32; 16] = [
    0, 8_000, 16_000, 32_000, 0, 0, 11_025, 22_050, 44_100, 0, 0, 12_000, 24_000, 48_000, 0, 0,
];
/// Channels by `AMODE` (table 5-4), the low-frequency effects channel
/// apart.
const DTS_CORE_CHANNELS: [u8; 16] = [1, 2, 2, 2, 2, 3, 3, 4, 4, 5, 6, 6, 6, 7, 8, 8];

/// A DTS core frame (TS 102 114 5.3.1) or an extension substream (7.5.2).
///
/// A core frame lasts `(NBLKS + 1) * 32` samples at the rate of `SFREQ`
/// and is `FSIZE + 1` bytes long. An extension substream is
/// `nuExtSSFsize + 1` bytes long and, when its header carries the static
/// fields, lasts `nuExSSFrameDuration` samples of the reference clock;
/// without them it states no duration (0 samples and no rate). Whether a
/// substream extends the core frame before it or is an access unit of its
/// own is the stream's, not the header's: the listing decides
/// (`packets.rs`).
///
/// Kept out of [`header`]: inlined, its frame setup costs every call of
/// the byte-by-byte search for the other syntaxes (the `probe-ts-no-frames`
/// guard case counted 8% more instructions).
#[inline(never)]
fn dts(data: &[u8]) -> Option<Frame> {
    let sync = data.get(..4)?;
    let mut bits = Bits {
        data: data.get(4..)?,
        at: 0,
    };
    if sync == DTS_CORE_SYNC {
        // FTYPE, SHORT, CPF.
        bits.read(1 + 5 + 1)?;
        let blocks = bits.read(7)? + 1;
        let len = usize::try_from(bits.read(14)?).ok()? + 1;
        let amode = usize::try_from(bits.read(6)?).ok()?;
        let rate = *DTS_CORE_RATES.get(usize::try_from(bits.read(4)?).ok()?)?;
        // RATE, the fixed bit, DYNF, TIMEF, AUXF, HDCD, EXT_AUDIO_ID,
        // EXT_AUDIO, ASPF.
        bits.read(5 + 1 + 1 + 1 + 1 + 1 + 3 + 1 + 1)?;
        let lfe = bits.read(2)?;
        // Table 5-2 allows `NBLKS` of 5 to 127 and `FSIZE` of 95 to 16,383;
        // `LFF` 3 is not allowed.
        if blocks < 6 || len < 96 || rate == 0 || lfe == 3 {
            return None;
        }
        let channels = DTS_CORE_CHANNELS.get(amode).copied().unwrap_or(0) + u8::from(lfe != 0);
        return Some(Frame {
            len,
            header: 0,
            dependent: false,
            samples: blocks * 32,
            sample_rate: rate,
            channels,
            detail: 0,
        });
    }
    if sync != DTS_SUBSTREAM_SYNC {
        return None;
    }
    // UserDefinedBits, nExtSSIndex.
    bits.read(8 + 2)?;
    let wide = bits.read(1)? == 1;
    let (header_bits, size_bits) = if wide { (12, 20) } else { (8, 16) };
    let header_len = bits.read(header_bits)? + 1;
    let len = usize::try_from(bits.read(size_bits)?).ok()? + 1;
    // The header is part of the substream and holds at least the fields
    // read so far.
    if usize::try_from(header_len).ok()? > len || header_len < 6 {
        return None;
    }
    let (samples, sample_rate) = if bits.read(1)? == 1 {
        let clock = match bits.read(2)? {
            0 => 32_000,
            1 => 44_100,
            2 => 48_000,
            _ => return None,
        };
        (512 * (bits.read(3)? + 1), clock)
    } else {
        (0, 0)
    };
    Some(Frame {
        len,
        header: 0,
        dependent: false,
        samples,
        sample_rate,
        channels: 0,
        detail: DTS_SUBSTREAM,
    })
}

/// A LOAS frame: the 11-bit sync word 0x2B7 and a 13-bit length, then an
/// `AudioMuxElement`. The sample rate and channels come from the
/// `StreamMuxConfig` when the frame carries one (0 otherwise: the stream's
/// earlier configuration applies).
fn latm(data: &[u8]) -> Option<Frame> {
    let [b0, b1, b2, ..] = *data else {
        return None;
    };
    if b0 != 0x56 || b1 & 0xE0 != 0xE0 {
        return None;
    }
    let len = 3 + ((usize::from(b1 & 0x1F) << 8) | usize::from(b2));
    let config = data.get(3..len.min(data.len())).and_then(latm_config);
    Some(Frame {
        len,
        header: 0,
        dependent: false,
        samples: 1024,
        sample_rate: config.map_or(0, |c| c.0),
        channels: config.map_or(0, |c| c.1),
        detail: config.map_or(0, |c| c.2),
    })
}

/// The sample rate, channels and audio object type of the
/// `AudioSpecificConfig` in an `AudioMuxElement` that starts with a
/// `StreamMuxConfig` of `audioMuxVersion` 0 (14496-3 1.7.3).
pub(crate) fn latm_config(element: &[u8]) -> Option<(u32, u8, u8)> {
    let mut bits = Bits {
        data: element,
        at: 0,
    };
    if bits.read(1)? != 0 {
        return None; // useSameStreamMux: no configuration here.
    }
    if bits.read(1)? != 0 {
        return None; // audioMuxVersion 1 is not read.
    }
    // allStreamsSameTimeFraming, numSubFrames, numProgram, numLayer.
    bits.read(1 + 6 + 4 + 3)?;
    let mut object = bits.read(5)?;
    if object == 31 {
        object = 32 + bits.read(6)?;
    }
    let index = bits.read(4)?;
    let rate = if index == 15 {
        bits.read(24)?
    } else {
        *AAC_RATES.get(usize::try_from(index).ok()?)?
    };
    let channels = bits.read(4)?;
    Some((
        rate,
        u8::try_from(channels).ok()?,
        u8::try_from(object).ok()?,
    ))
}

/// The `AudioSpecificConfig` of an `AudioMuxElement` that starts with a
/// `StreamMuxConfig` of `audioMuxVersion` 0 (14496-3 1.7.3), copied out
/// of the bit stream to whole bytes, the last one padded with zero bits.
///
/// In this version of the syntax nothing states the configuration's
/// length, so it ends where its own syntax does (1.6.2.1): the object
/// type, the sampling frequency and the channel configuration, the
/// extension frequency and core object type of explicit hierarchical SBR
/// signalling (object types 5 and 29), and the `GASpecificConfig` of the
/// general audio object types (4.4.1) with the `epConfig` of the
/// error-resilient ones. `None` for a configuration whose length this
/// does not read: another object type, a program config element
/// (channel configuration 0), or error protection (`epConfig` 2 or 3).
pub(crate) fn latm_audio_specific_config(element: &[u8]) -> Option<Vec<u8>> {
    let mut bits = Bits {
        data: element,
        at: 0,
    };
    if bits.read(2)? != 0 {
        return None; // useSameStreamMux, or audioMuxVersion 1.
    }
    bits.read(1 + 6 + 4 + 3)?;
    let start = bits.at;
    let object_type = |bits: &mut Bits<'_>| -> Option<u32> {
        let object = bits.read(5)?;
        if object == 31 {
            return Some(32 + bits.read(6)?);
        }
        Some(object)
    };
    let frequency = |bits: &mut Bits<'_>| -> Option<()> {
        if bits.read(4)? == 15 {
            bits.read(24)?;
        }
        Some(())
    };
    let mut object = object_type(&mut bits)?;
    frequency(&mut bits)?;
    let channels = bits.read(4)?;
    if matches!(object, 5 | 29) {
        frequency(&mut bits)?;
        object = object_type(&mut bits)?;
        if object == 22 {
            bits.read(4)?; // extensionChannelConfiguration
        }
    }
    if !matches!(object, 1..=4 | 6 | 7 | 17 | 19..=23) || channels == 0 {
        return None;
    }
    // GASpecificConfig: frameLengthFlag, dependsOnCoreCoder (and the
    // 14-bit coreCoderDelay), extensionFlag.
    bits.read(1)?;
    if bits.read(1)? == 1 {
        bits.read(14)?;
    }
    let extension = bits.read(1)? == 1;
    if matches!(object, 6 | 20) {
        bits.read(3)?; // layerNr
    }
    if extension {
        if object == 22 {
            bits.read(5 + 11)?; // numOfSubFrame, layer_length
        }
        if matches!(object, 17 | 19 | 20 | 23) {
            bits.read(3)?; // the three resilience flags
        }
        bits.read(1)?; // extensionFlag3
    }
    if matches!(object, 17 | 19..=23) && bits.read(2)? >= 2 {
        return None; // ErrorProtectionSpecificConfig follows.
    }
    let len = bits.at - start;
    let mut copy = Bits {
        data: element,
        at: start,
    };
    let mut out = Vec::with_capacity(len.div_ceil(8));
    let mut left = len;
    while left > 0 {
        let n = left.min(8);
        let byte = copy.read(u32::try_from(n).ok()?)? << (8 - n);
        out.push(u8::try_from(byte).ok()?);
        left -= n;
    }
    Some(out)
}

/// Reads bits most significant first.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: u32) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..n {
            let byte = self.data.get(self.at / 8)?;
            value = (value << 1) | u32::from((byte >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Some(value)
    }
}

/// Most bytes of an Opus access unit: 255 streams of the longest Opus
/// packet (RFC 6716 3.4: at most 1,275 bytes per 20 ms frame, six frames)
/// fit with room to spare.
const MAX_OPUS_ACCESS_UNIT: usize = 1 << 21;

/// An Opus access unit: an optional control header (the 11-bit prefix
/// 0x3FF, the trim and extension flags, the payload size in 0xFF-continued
/// bytes, the trims and the extension), then the Opus packet. Without a
/// control header the access unit is the rest of the PES payload.
fn opus(data: &[u8]) -> Option<Frame> {
    let [b0, b1, ..] = *data else {
        return None;
    };
    let (header, payload) = if b0 == 0x7F && b1 & 0xE0 == 0xE0 {
        let mut at = 2usize;
        let mut size = 0usize;
        loop {
            let byte = *data.get(at)?;
            at += 1;
            size += usize::from(byte);
            if size > MAX_OPUS_ACCESS_UNIT {
                // Larger than any Opus access unit: not a control header.
                return None;
            }
            if byte != 0xFF {
                break;
            }
        }
        if b1 & 0x10 != 0 {
            at += 2; // start_trim
        }
        if b1 & 0x08 != 0 {
            at += 2; // end_trim
        }
        if b1 & 0x04 != 0 {
            at += 1 + usize::from(*data.get(at)?); // control extension
        }
        (at, size)
    } else {
        (0, data.len())
    };
    let toc = *data.get(header)?;
    let samples = opus_samples(toc, data.get(header + 1).copied())?;
    Some(Frame {
        len: header + payload,
        header,
        dependent: false,
        samples,
        sample_rate: 48_000,
        channels: 0,
        detail: 0,
    })
}

/// The 48 kHz samples of an Opus packet from its TOC byte and, for code 3,
/// its frame count byte (RFC 6716 3.1 and 3.2.5).
fn opus_samples(toc: u8, count: Option<u8>) -> Option<u32> {
    let config = toc >> 3;
    // Frame sizes in 48 kHz samples: SILK 10/20/40/60 ms, hybrid 10/20
    // ms, CELT 2.5/5/10/20 ms.
    let frame = match config {
        0..=11 => *[480, 960, 1920, 2880].get(usize::from(config % 4))?,
        12..=15 => *[480, 960].get(usize::from(config % 2))?,
        _ => *[120, 240, 480, 960].get(usize::from(config % 4))?,
    };
    let frames = match toc & 0x03 {
        0 => 1,
        1 | 2 => 2,
        _ => u32::from(count? & 0x3F),
    };
    Some(frame * frames)
}

const AAC_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

fn adts(data: &[u8]) -> Option<Frame> {
    let [b0, b1, b2, b3, b4, b5, b6, ..] = *data else {
        return None;
    };
    if b0 != 0xFF || b1 & 0xF6 != 0xF0 {
        return None;
    }
    let profile = b2 >> 6;
    let rate = *AAC_RATES.get(usize::from((b2 >> 2) & 0x0F))?;
    let channels = ((b2 & 1) << 2) | (b3 >> 6);
    let len = (usize::from(b3 & 0x03) << 11) | (usize::from(b4) << 3) | usize::from(b5 >> 5);
    let header = if b1 & 1 == 0 { 9 } else { 7 };
    if len < header {
        return None;
    }
    let blocks = u32::from(b6 & 0x03) + 1;
    Some(Frame {
        len,
        header: 0,
        dependent: false,
        samples: 1024 * blocks,
        sample_rate: rate,
        channels,
        detail: profile + 1,
    })
}

/// Bit rates in kbit/s by version (MPEG-1, MPEG-2 and 2.5), layer and index.
const MPEG1_RATES: [[u16; 15]; 3] = [
    [
        0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
    ],
    [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
    ],
    [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ],
];
const MPEG2_RATES: [[u16; 15]; 2] = [
    [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
    ],
    [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
];

fn mpeg_audio(data: &[u8]) -> Option<Frame> {
    let [b0, b1, b2, b3, ..] = *data else {
        return None;
    };
    if b0 != 0xFF || b1 & 0xE0 != 0xE0 {
        return None;
    }
    let version = (b1 >> 3) & 0x03; // 0: 2.5, 2: MPEG-2, 3: MPEG-1
    let layer = match (b1 >> 1) & 0x03 {
        3 => 1u8,
        2 => 2,
        1 => 3,
        _ => return None,
    };
    if version == 1 {
        return None;
    }
    let index = usize::from(b2 >> 4);
    let base_rate: u32 = match (b2 >> 2) & 0x03 {
        0 => 44_100,
        1 => 48_000,
        2 => 32_000,
        _ => return None,
    };
    let sample_rate = match version {
        3 => base_rate,
        2 => base_rate / 2,
        _ => base_rate / 4,
    };
    let kbps = if version == 3 {
        MPEG1_RATES.get(usize::from(layer - 1))?.get(index)
    } else {
        MPEG2_RATES.get(usize::from(layer != 1))?.get(index)
    };
    // Index 0 is the free format, whose frames cannot be delimited from
    // the header; index 15 is not allowed.
    let bits = u32::from(*kbps.filter(|k| **k != 0)?) * 1000;
    let padding = u32::from((b2 >> 1) & 1);
    let (samples, len) = match (layer, version) {
        (1, _) => (384, (12 * bits / sample_rate + padding) * 4),
        (2, _) | (3, 3) => (1152, 144 * bits / sample_rate + padding),
        _ => (576, 72 * bits / sample_rate + padding),
    };
    Some(Frame {
        len: usize::try_from(len).ok()?,
        header: 0,
        dependent: false,
        samples,
        sample_rate,
        channels: if b3 >> 6 == 3 { 1 } else { 2 },
        detail: layer,
    })
}

/// AC-3 frame sizes in 16-bit words by `frmsizecod` / 2 and `fscod`
/// (48, 44.1 and 32 kHz; ATSC A/52 table 5.18).
const AC3_WORDS: [[u16; 3]; 19] = [
    [64, 69, 96],
    [80, 87, 120],
    [96, 104, 144],
    [112, 121, 168],
    [128, 139, 192],
    [160, 174, 240],
    [192, 208, 288],
    [224, 243, 336],
    [256, 278, 384],
    [320, 348, 480],
    [384, 417, 576],
    [448, 487, 672],
    [512, 557, 768],
    [640, 696, 960],
    [768, 835, 1152],
    [896, 975, 1344],
    [1024, 1114, 1536],
    [1152, 1253, 1728],
    [1280, 1393, 1920],
];
const AC3_RATES: [u32; 3] = [48_000, 44_100, 32_000];
/// Channels of each `acmod` before the LFE channel (table 5.8).
const AC3_CHANNELS: [u8; 8] = [2, 1, 2, 3, 3, 4, 4, 5];

fn ac3(data: &[u8]) -> Option<Frame> {
    let [0x0B, 0x77, _, _, b4, b5, b6, b7, ..] = *data else {
        return None;
    };
    let bsid = b5 >> 3;
    if bsid <= 10 {
        // AC-3 (A/52 5.4.1): fscod, frmsizecod; bsid, bsmod; acmod, ...
        let fscod = usize::from(b4 >> 6);
        let code = usize::from(b4 & 0x3F);
        let words = *AC3_WORDS.get(code / 2)?.get(fscod)?;
        // 44.1 kHz frames with an odd frmsizecod carry one more word.
        let words = u32::from(words) + u32::from(fscod == 1 && code % 2 == 1);
        let acmod = b6 >> 5;
        // A bsid of 9 or 10 marks a stream at half or a quarter of the
        // sample rate fscod names; 8 and below are at that rate.
        let shift = bsid.saturating_sub(8);
        Some(Frame {
            len: usize::try_from(words * 2).ok()?,
            header: 0,
            dependent: false,
            samples: 1536,
            sample_rate: *AC3_RATES.get(fscod)? >> shift,
            channels: AC3_CHANNELS.get(usize::from(acmod)).copied()? + lfe_ac3(b6, b7, acmod),
            detail: 0,
        })
    } else if (11..=16).contains(&bsid) {
        // E-AC-3 (annex E 2.3.1): strmtyp, substreamid, frmsiz; fscod,
        // numblkscod, acmod, lfeon.
        let [_, _, s0, s1, ..] = *data else {
            return None;
        };
        let frmsiz = (usize::from(s0 & 0x07) << 8) | usize::from(s1);
        let fscod = b4 >> 6;
        let (sample_rate, blocks) = if fscod == 3 {
            (*AC3_RATES.get(usize::from((b4 >> 4) & 0x03))? / 2, 6)
        } else {
            (
                *AC3_RATES.get(usize::from(fscod))?,
                *[1u32, 2, 3, 6].get(usize::from((b4 >> 4) & 0x03))?,
            )
        };
        let acmod = (b4 >> 1) & 0x07;
        Some(Frame {
            len: (frmsiz + 1) * 2,
            header: 0,
            dependent: s0 >> 6 == 1,
            samples: 256 * blocks,
            sample_rate,
            channels: AC3_CHANNELS.get(usize::from(acmod)).copied()? + (b4 & 1),
            detail: 1,
        })
    } else {
        None
    }
}

/// The `lfeon` bit of an AC-3 header, which follows `acmod` and the mix
/// levels that `acmod` brings (A/52 5.4.2).
fn lfe_ac3(b6: u8, b7: u8, acmod: u8) -> u8 {
    let mut bit = 3; // bits of b6 after acmod
    if acmod & 1 != 0 && acmod != 1 {
        bit += 2; // cmixlev
    }
    if acmod & 4 != 0 {
        bit += 2; // surmixlev
    }
    if acmod == 2 {
        bit += 2; // dsurmod
    }
    let word = (u16::from(b6) << 8) | u16::from(b7);
    // `bit` counts from the top of b6 after its first 3 bits.
    u8::from(word >> (15 - bit) & 1 == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adts_header() {
        // AAC LC, 48 kHz, 2 channels, 371 bytes, no CRC.
        let len = 371usize;
        let h = [
            0xFF,
            0xF1,
            0x4C,
            0x80 | ((len >> 11) as u8 & 3),
            (len >> 3) as u8,
            ((len as u8) << 5) | 0x1F,
            0xFC,
        ];
        let f = header(Syntax::Adts, &h).unwrap();
        assert_eq!(
            (f.len, f.samples, f.sample_rate, f.channels, f.detail),
            (371, 1024, 48_000, 2, 2)
        );
        assert!(header(Syntax::Adts, &h[..5]).is_none());
    }

    #[test]
    fn mpeg_audio_headers() {
        // MPEG-1 layer III, 128 kbit/s, 44.1 kHz, no padding: 417 bytes.
        let f = header(Syntax::MpegAudio, &[0xFF, 0xFB, 0x90, 0x44]).unwrap();
        assert_eq!(
            (f.len, f.samples, f.sample_rate, f.channels, f.detail),
            (417, 1152, 44_100, 2, 3)
        );
        // MPEG-1 layer II, 192 kbit/s, 48 kHz: 576 bytes.
        let f = header(Syntax::MpegAudio, &[0xFF, 0xFD, 0xA4, 0xC0]).unwrap();
        assert_eq!((f.len, f.samples, f.channels, f.detail), (576, 1152, 1, 2));
        // MPEG-2 layer III, 64 kbit/s, 24 kHz: 576 samples, 192 bytes.
        let f = header(Syntax::MpegAudio, &[0xFF, 0xF3, 0x84, 0x00]).unwrap();
        assert_eq!((f.len, f.samples, f.sample_rate), (192, 576, 24_000));
        // Free format and the reserved version are refused.
        assert!(header(Syntax::MpegAudio, &[0xFF, 0xFB, 0x00, 0x44]).is_none());
        assert!(header(Syntax::MpegAudio, &[0xFF, 0xEB, 0x90, 0x44]).is_none());
    }

    #[test]
    fn the_search_for_a_first_frame_stops_after_one_frame_of_the_syntax() {
        // The bytes before a payload's first frame header are the tail
        // of a frame that began in the PES packet before it, so one
        // frame of the syntax bounds the search: a header further in is
        // not this stream's first frame.
        // Two ADTS frames of AAC LC at 48 kHz, 300 and 320 bytes.
        let frame = |len: usize| {
            let mut f = vec![
                0xFF,
                0xF1,
                0x4C,
                0x80 | ((len >> 11) as u8 & 3),
                (len >> 3) as u8,
                ((len as u8) << 5) | 0x1F,
                0xFC,
            ];
            f.resize(len, 0x21);
            f
        };
        let frames = [frame(300), frame(320)].concat();
        for (lead, found) in [
            (0usize, Some(0usize)),
            (100, Some(100)),
            (8_000, Some(8_000)),
            (8_191, None),
            (20_000, None),
        ] {
            let mut payload = vec![0x11u8; lead];
            payload.extend_from_slice(&frames);
            let at = first_with_rate(Syntax::Adts, &payload).map(|(at, _)| at);
            assert_eq!(at, found, "lead {lead}");
        }
        // The bound is the syntax's own longest frame, not one number,
        // and no frame the parser accepts is longer than its own.
        assert_eq!(
            [
                longest_frame(Syntax::Adts),
                longest_frame(Syntax::Latm),
                longest_frame(Syntax::Ac3),
                longest_frame(Syntax::MpegAudio),
                longest_frame(Syntax::Opus),
            ],
            [8_191, 8_194, 4_096, 2_881, MAX_SEARCH_BYTES]
        );
    }

    #[test]
    fn no_frame_a_parser_accepts_is_longer_than_its_bound() {
        // Every value of the fields a length is built from: AC-3's
        // fscod and frmsizecod, E-AC-3's frmsiz, and MPEG audio's
        // version, layer, bit rate, sampling rate and padding.
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                for frame in [
                    header(Syntax::Ac3, &[0x0B, 0x77, 0, 0, a, b, 0, 0]),
                    // bsid 11 (b5 >> 3) makes it E-AC-3, whose frmsiz
                    // is the low three bits of b2 and all of b3.
                    header(Syntax::Ac3, &[0x0B, 0x77, a, b, 0, 0x58, 0, 0]),
                ]
                .into_iter()
                .flatten()
                {
                    assert!(frame.len <= longest_frame(Syntax::Ac3), "{a:#x} {b:#x}");
                }
                if let Some(frame) = header(Syntax::MpegAudio, &[0xFF, a, b, 0]) {
                    assert!(
                        frame.len <= longest_frame(Syntax::MpegAudio),
                        "{a:#x} {b:#x}"
                    );
                }
            }
        }
        // ADTS and LOAS state their length outright, in 13 bits.
        let adts = [0xFF, 0xF1, 0x4C, 0x83, 0xFF, 0xFF, 0xFC];
        assert_eq!(
            header(Syntax::Adts, &adts).map(|f| f.len),
            Some(longest_frame(Syntax::Adts))
        );
        let loas = [0x56, 0xFF, 0xFF];
        assert_eq!(
            header(Syntax::Latm, &loas).map(|f| f.len),
            Some(longest_frame(Syntax::Latm))
        );
    }

    #[test]
    fn opus_access_units() {
        // Control header, payload of 300 bytes; a CELT 20 ms packet.
        let mut au = vec![0x7F, 0xE0, 0xFF, 0x2D, 0xF8];
        au.resize(4 + 300, 0);
        let f = header(Syntax::Opus, &au).unwrap();
        assert_eq!((f.header, f.len, f.samples), (4, 304, 960));
        // With a start trim, and code 3 with 3 frames of 10 ms SILK.
        let au = [0x7F, 0xF0, 0x02, 0x00, 0x10, 0x03, 0x03];
        let f = header(Syntax::Opus, &au).unwrap();
        assert_eq!((f.header, f.len, f.samples), (5, 7, 3 * 480));
        // No control header: the rest of the payload.
        let f = header(Syntax::Opus, &[0xF8, 1, 2]).unwrap();
        assert_eq!((f.header, f.len, f.samples), (0, 3, 960));
    }

    #[test]
    fn loas_frames() {
        // useSameStreamMux 0, version 0, 0+0+0+0 fields, AAC LC (2),
        // 48 kHz (3), 2 channels.
        let element = [0b0000_0000, 0b0000_0000, 0b0001_0001, 0b1001_0000];
        let mut frame = vec![0x56, 0xE0, element.len() as u8];
        frame.extend(element);
        let f = header(Syntax::Latm, &frame).unwrap();
        assert_eq!(
            (f.len, f.sample_rate, f.channels, f.detail),
            (7, 48_000, 2, 2)
        );
    }

    #[test]
    fn latm_configuration_bytes() {
        // AAC LC, 48 kHz, 2 channels: the 16 bits of the configuration.
        let element = [0b0000_0000, 0b0000_0000, 0b0001_0001, 0b1001_0000];
        assert_eq!(latm_audio_specific_config(&element), Some(vec![0x11, 0x90]));
        // Explicit SBR (object type 5) at 22.05 kHz, mono, 44.1 kHz out,
        // over an LC core: 25 bits, four bytes.
        let element = [0x20, 0x00, 0x2B, 0x8A, 0x08, 0x0F, 0xF3];
        assert_eq!(
            latm_audio_specific_config(&element),
            Some(vec![0x2B, 0x8A, 0x08, 0x00])
        );
        // useSameStreamMux: no configuration; channel configuration 0: a
        // program config element, whose length is not read.
        assert_eq!(latm_audio_specific_config(&[0x80, 0, 0, 0]), None);
        assert_eq!(latm_audio_specific_config(&[0x00, 0x00, 0x11, 0x80]), None);
    }

    #[test]
    fn eac3_dependent_substreams_and_low_rate_ac3() {
        // strmtyp 1: a dependent substream.
        let f = header(Syntax::Ac3, &[0x0B, 0x77, 0x41, 0x7F, 0x3F, 0x80, 0, 0]).unwrap();
        assert!(f.dependent);
        let f = header(Syntax::Ac3, &[0x0B, 0x77, 0x01, 0x7F, 0x3F, 0x80, 0, 0]).unwrap();
        assert!(!f.dependent);
        // AC-3 with bsid 9: half of 48 kHz.
        let f = header(Syntax::Ac3, &[0x0B, 0x77, 0, 0, 0x1E, 9 << 3, 0x40, 0x00]).unwrap();
        assert_eq!((f.sample_rate, f.samples), (24_000, 1536));
        // An Opus control header claiming more than any access unit.
        let mut au = vec![0x7F, 0xE0];
        au.extend(std::iter::repeat_n(0xFF, 9000));
        assert!(header(Syntax::Opus, &au).is_none());
    }

    #[test]
    fn ac3_and_eac3_headers() {
        // AC-3, 48 kHz, frmsizecod 30 (448 kbit/s): 1792 bytes; bsid 8,
        // acmod 7 (3/2) with lfeon 1.
        let f = header(Syntax::Ac3, &[0x0B, 0x77, 0, 0, 0x1E, 0x40, 0xE1, 0x00]).unwrap();
        assert_eq!(
            (f.len, f.samples, f.sample_rate, f.channels),
            (1792, 1536, 48_000, 6)
        );
        // 44.1 kHz with an odd frmsizecod carries one more word.
        let f = header(Syntax::Ac3, &[0x0B, 0x77, 0, 0, 0x5F, 0x40, 0x40, 0x00]).unwrap();
        assert_eq!((f.len, f.sample_rate, f.channels), (1952, 44_100, 2));
        // E-AC-3, frmsiz 383 (768 bytes), 48 kHz, 6 blocks, acmod 7, lfeon.
        let f = header(Syntax::Ac3, &[0x0B, 0x77, 0x01, 0x7F, 0x3F, 0x80, 0, 0]).unwrap();
        assert_eq!(
            (f.len, f.samples, f.sample_rate, f.channels, f.detail),
            (768, 1536, 48_000, 6, 1)
        );
        assert!(header(Syntax::Ac3, &[0x0B, 0x78, 0, 0, 0, 0, 0, 0]).is_none());
    }

    /// Packs `(value, bits)` fields most significant bit first.
    fn pack(fields: &[(u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut at = 0usize;
        for &(value, n) in fields {
            for i in (0..n).rev() {
                if at % 8 == 0 {
                    out.push(0);
                }
                if (value >> i) & 1 == 1 {
                    *out.last_mut().unwrap() |= 0x80 >> (at % 8);
                }
                at += 1;
            }
        }
        out
    }

    #[test]
    fn dts_core_frames_and_substreams() {
        // A core frame: 16 blocks of 32 samples, 1,024 bytes, C L R Ls Rs
        // (AMODE 9) and LFE at 48 kHz.
        let mut core = DTS_CORE_SYNC.to_vec();
        core.extend(pack(&[
            (1, 1),
            (31, 5),
            (0, 1),
            (15, 7),
            (1023, 14),
            (9, 6),
            (13, 4),
            (15, 5),
            (0, 1 + 1 + 1 + 1 + 1 + 3 + 1 + 1),
            (1, 2),
            (0, 8),
        ]));
        let f = header(Syntax::Dts, &core).unwrap();
        assert_eq!(
            (f.len, f.samples, f.sample_rate, f.channels, f.detail),
            (1024, 512, 48_000, 6, 0)
        );
        // A substream of 116 bytes whose static fields give 512 samples of
        // the 48 kHz reference clock.
        let mut substream = DTS_SUBSTREAM_SYNC.to_vec();
        substream.extend(pack(&[
            (0, 8),
            (0, 2),
            (0, 1),
            (31, 8),
            (115, 16),
            (1, 1),
            (2, 2),
            (0, 3),
            (0, 8),
        ]));
        let f = header(Syntax::Dts, &substream).unwrap();
        assert_eq!(
            (f.len, f.samples, f.sample_rate, f.detail),
            (116, 512, 48_000, DTS_SUBSTREAM)
        );
        // A core frame shorter than the standard allows is not one, and
        // the start of either sync word may go on in the next payload.
        let mut short = core.clone();
        short[6] = 0;
        short[7] &= 0x0F;
        assert!(header(Syntax::Dts, &short).is_none());
        assert!(may_start(Syntax::Dts, &[0x7F, 0xFE]));
        assert!(may_start(Syntax::Dts, &[0x64, 0x58, 0x20]));
        assert!(!may_start(Syntax::Dts, &[0x7F, 0xFF]));
    }
}
