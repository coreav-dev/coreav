//! Video access units in byte stream format (ITU-T H.264 and H.265 annex
//! B): their NAL units, the configuration records their parameter sets
//! make (ISO/IEC 14496-15 5.3.3.1 and 8.3.3.1), and whether decoding can
//! start at them; and the MPEG-1 and MPEG-2 video sequence header (ISO/IEC
//! 11172-2 2.4.2.3, 13818-2 6.2.2.1). Sans-IO.

use coreav_isobmff::access::{annex_b_frame_access, next_start_code};

/// The NAL units of an access unit in byte stream format: the bytes
/// between start codes (`00 00 01`, or `00 00 00 01`), trailing zeros
/// removed.
pub(crate) fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while let Some(at) = next_start_code(data, i) {
        starts.push(at + 3);
        i = at + 3;
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map_or(data.len(), |next| next - 3);
        let mut unit = data.get(start..end).unwrap_or_default();
        while let Some((&0, rest)) = unit.split_last() {
            unit = rest;
        }
        if !unit.is_empty() {
            out.push(unit);
        }
    }
    out
}

/// The RBSP of a NAL unit: its bytes with emulation prevention removed.
fn rbsp(nal: &[u8], max: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len().min(max));
    let mut zeros = 0;
    for &b in nal.iter().take(max) {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Which video codec's NAL unit syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Nal {
    H264,
    Hevc,
    /// MPEG-1 and MPEG-2 video, which has no NAL units: its start codes
    /// (ISO/IEC 13818-2 6.2) delimit its pictures as theirs do, and the
    /// splitter ([`AccessUnits`]) cuts it into pictures the same way.
    Mpeg2,
}

/// Reads the bits of an RBSP, most significant first.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = self.data.get(self.at / 8)?;
        let bit = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(u32::from(bit))
    }

    fn read(&mut self, n: u32) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..n {
            value = (value << 1) | self.bit()?;
        }
        Some(value)
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        self.at = self.at.checked_add(n)?;
        (self.at <= self.data.len() * 8).then_some(())
    }

    /// An unsigned Exp-Golomb code (H.264 9.1, H.265 9.2).
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0u32;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let rest = self.read(zeros)?;
        ((1u64 << zeros) - 1 + u64::from(rest)).try_into().ok()
    }
}

/// From an H.264 SPS of the High profiles (7.3.2.1.1): `chroma_format_idc`,
/// `bit_depth_luma_minus8` and `bit_depth_chroma_minus8`; 4:2:0 at 8 bits
/// for the other profiles, which do not code them.
fn avc_format(sps: &[u8]) -> Option<(u32, u32, u32)> {
    let rbsp = rbsp(sps, 64);
    let mut bits = Bits {
        data: rbsp.get(1..)?,
        at: 0,
    };
    let profile = bits.read(8)?;
    bits.skip(16)?;
    bits.ue()?; // seq_parameter_set_id
    if !matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        return Some((1, 0, 0));
    }
    let chroma = bits.ue()?;
    if chroma == 3 {
        bits.skip(1)?;
    }
    Some((chroma, bits.ue()?, bits.ue()?))
}

/// What an `hvcC` record repeats from an HEVC SPS (7.3.2.2): the 12 bytes
/// of the general profile, tier and level, `sps_max_sub_layers_minus1`,
/// `sps_temporal_id_nesting_flag`, `chroma_format_idc` and the two bit
/// depths.
struct HevcFormat {
    general: Vec<u8>,
    sub_layers_minus1: u32,
    nesting: u32,
    chroma: u32,
    luma_minus8: u32,
    chroma_minus8: u32,
}

fn hevc_format(sps: &[u8]) -> Option<HevcFormat> {
    let rbsp = rbsp(sps, 512);
    let data = rbsp.get(2..)?;
    let mut bits = Bits { data, at: 0 };
    bits.skip(4)?; // sps_video_parameter_set_id
    let sub_layers_minus1 = bits.read(3)?;
    if sub_layers_minus1 > 6 {
        // 7 is not allowed (7.4.3.2.1), and would not fit the record.
        return None;
    }
    let nesting = bits.read(1)?;
    let general = data.get(1..13)?.to_vec();
    bits.skip(96)?;
    // profile_tier_level(1, sps_max_sub_layers_minus1) (7.3.3).
    let mut present = Vec::new();
    for _ in 0..sub_layers_minus1 {
        present.push((bits.read(1)?, bits.read(1)?));
    }
    if sub_layers_minus1 > 0 {
        bits.skip(2 * (8 - sub_layers_minus1 as usize))?;
    }
    for (profile, level) in present {
        bits.skip(88 * profile as usize + 8 * level as usize)?;
    }
    bits.ue()?; // sps_seq_parameter_set_id
    let chroma = bits.ue()?;
    if chroma == 3 {
        bits.skip(1)?;
    }
    bits.ue()?; // pic_width_in_luma_samples
    bits.ue()?; // pic_height_in_luma_samples
    if bits.read(1)? == 1 {
        for _ in 0..4 {
            bits.ue()?;
        }
    }
    Some(HevcFormat {
        general,
        sub_layers_minus1,
        nesting,
        chroma,
        luma_minus8: bits.ue()?,
        chroma_minus8: bits.ue()?,
    })
}

/// Parameter sets collected from access units, until a record can be
/// made.
#[derive(Clone, Debug, Default)]
pub(crate) struct ParameterSets {
    vps: Vec<Vec<u8>>,
    sps: Vec<Vec<u8>>,
    pps: Vec<Vec<u8>>,
    seen: std::collections::HashSet<Vec<u8>>,
    kept: usize,
}

/// Most bytes of parameter sets kept per stream, and most of each kind
/// (H.264 allows 32 SPS and 256 PPS ids; H.265 16, 16 and 64).
const MAX_PARAMETER_BYTES: usize = 64 << 10;
const MAX_PER_KIND: usize = 31;

impl ParameterSets {
    /// Keeps the parameter sets of an access unit not seen yet.
    pub(crate) fn add(&mut self, nal: Nal, data: &[u8]) {
        for unit in nal_units(data) {
            let Some(&head) = unit.first() else { continue };
            let list = match (nal, head) {
                (Nal::H264, h) if h & 0x1F == 7 => &mut self.sps,
                (Nal::H264, h) if h & 0x1F == 8 => &mut self.pps,
                (Nal::Hevc, h) if (h >> 1) & 0x3F == 32 => &mut self.vps,
                (Nal::Hevc, h) if (h >> 1) & 0x3F == 33 => &mut self.sps,
                (Nal::Hevc, h) if (h >> 1) & 0x3F == 34 => &mut self.pps,
                _ => continue,
            };
            if list.len() >= MAX_PER_KIND || self.kept + unit.len() > MAX_PARAMETER_BYTES {
                continue;
            }
            if self.seen.insert(unit.to_vec()) {
                list.push(unit.to_vec());
                self.kept += unit.len();
            }
        }
    }

    /// Whether a record can be made: an SPS and a PPS (and a VPS for HEVC).
    pub(crate) fn complete(&self, nal: Nal) -> bool {
        !self.sps.is_empty() && !self.pps.is_empty() && (nal == Nal::H264 || !self.vps.is_empty())
    }

    /// The `avcC` or `hvcC` payload these parameter sets make.
    pub(crate) fn record(&self, nal: Nal) -> Option<Vec<u8>> {
        if !self.complete(nal) {
            return None;
        }
        match nal {
            Nal::H264 => self.avcc(),
            Nal::Hevc => self.hvcc(),
            Nal::Mpeg2 => None,
        }
    }

    /// ISO/IEC 14496-15 5.3.3.1.
    fn avcc(&self) -> Option<Vec<u8>> {
        let sps = self.sps.first()?;
        let [_, profile, compatibility, level, ..] = **sps else {
            return None;
        };
        let mut out = vec![
            1,
            profile,
            compatibility,
            level,
            0xFF,
            0xE0 | count(&self.sps)?,
        ];
        for s in &self.sps {
            out.extend(u16::try_from(s.len()).ok()?.to_be_bytes());
            out.extend(s);
        }
        out.push(count(&self.pps)?);
        for p in &self.pps {
            out.extend(u16::try_from(p.len()).ok()?.to_be_bytes());
            out.extend(p);
        }
        if matches!(profile, 100 | 110 | 122 | 144) {
            let (chroma, luma, chroma_depth) = avc_format(sps)?;
            out.extend([
                0xFC | u8::try_from(chroma & 3).ok()?,
                0xF8 | u8::try_from(luma & 7).ok()?,
                0xF8 | u8::try_from(chroma_depth & 7).ok()?,
                0,
            ]);
        }
        Some(out)
    }

    /// ISO/IEC 14496-15 8.3.3.1.
    fn hvcc(&self) -> Option<Vec<u8>> {
        let format = hevc_format(self.sps.first()?)?;
        let mut out = vec![1];
        out.extend(&format.general);
        out.extend([
            0xF0,
            0x00,
            0xFC,
            0xFC | u8::try_from(format.chroma & 3).ok()?,
            0xF8 | u8::try_from(format.luma_minus8 & 7).ok()?,
            0xF8 | u8::try_from(format.chroma_minus8 & 7).ok()?,
            0x00,
            0x00,
            u8::try_from(((format.sub_layers_minus1 + 1) << 3) | (format.nesting << 2) | 3).ok()?,
        ]);
        out.push(3);
        for (kind, list) in [(32u8, &self.vps), (33, &self.sps), (34, &self.pps)] {
            out.push(0x80 | kind);
            out.extend(u16::try_from(list.len()).ok()?.to_be_bytes());
            for unit in list {
                out.extend(u16::try_from(unit.len()).ok()?.to_be_bytes());
                out.extend(unit);
            }
        }
        Some(out)
    }
}

fn count(list: &[Vec<u8>]) -> Option<u8> {
    u8::try_from(list.len()).ok().filter(|n| *n < 32)
}

/// The bytes of a coded slice read for its header, as the isobmff walk reads
/// them.
const SLICE_HEADER_BYTES: usize = 32;

/// Whether this access unit is a keyframe as the coded data defines one,
/// the rule `ffprobe` follows (`tools/oracle/SPEC.md`, "Keyframes"): for
/// H.264, an IDR picture, a recovery point SEI message (whatever its
/// count), or only I slices when at most one reference frame is kept; for HEVC, an IRAP
/// picture (`nal_unit_type` 16 to 23). `None` when the NAL units cannot
/// be read.
pub(crate) fn keyframe(nal: Nal, data: &[u8], max_num_ref_frames: Option<u32>) -> Option<bool> {
    match nal {
        Nal::Mpeg2 => Some(mpeg2_keyframe(data)),
        Nal::Hevc => Some(nal_units(data).iter().any(|u| {
            u.first()
                .is_some_and(|h| (16..=23).contains(&(h >> 1 & 0x3F)))
        })),
        Nal::H264 => {
            if next_start_code(data, 0).is_none() {
                return Some(false);
            }
            // The units up to the first coded slice decide whether it is an
            // IDR picture or a recovery point (7.4.1.2.3, 7.4.1.2.4): the
            // slices after it, often nearly all of the unit's bytes, are
            // read only for the intra rule.
            let whole = max_num_ref_frames.is_some_and(|refs| refs <= 1);
            let mut end = data.len();
            if !whole {
                let mut i = 0;
                while let Some(at) = next_start_code(data, i) {
                    i = at + 3;
                    if data.get(i).is_some_and(|h| matches!(h & 0x1F, 1 | 2 | 5)) {
                        let header_end = i.saturating_add(SLICE_HEADER_BYTES);
                        let window = data.get(..header_end.saturating_add(2).min(data.len()))?;
                        match next_start_code(window, i) {
                            // A unit that starts within those bytes is cut
                            // off before it, not through its header.
                            Some(next) if next <= header_end => end = next,
                            _ => {
                                end = end.min(header_end);
                                // A unit's trailing zeros are dropped before
                                // its header is read: never cut it where they
                                // would be.
                                while end < data.len() && data.get(end - 1) == Some(&0) {
                                    end += 1;
                                }
                            }
                        }
                        break;
                    }
                }
            }
            let access = annex_b_frame_access(data.get(..end)?, end == data.len())?;
            Some(
                access.idr
                    || access.recovery_frames.is_some()
                    || (access.intra && max_num_ref_frames.is_some_and(|refs| refs <= 1)),
            )
        }
    }
}

/// Whether a VCL NAL unit starts a picture: `first_mb_in_slice` is 0
/// (ITU-T H.264 7.3.3, a `ue(v)` whose first bit is then 1), or
/// `first_slice_segment_in_pic_flag` is 1 (H.265 7.3.6.1, the first bit
/// after the two-byte header).
fn first_in_picture(nal: Nal, unit: &[u8]) -> bool {
    match nal {
        Nal::H264 => unit.get(1).is_some_and(|b| b & 0x80 != 0),
        Nal::Hevc => unit.get(2).is_some_and(|b| b & 0x80 != 0),
        Nal::Mpeg2 => false,
    }
}

/// Where the access units of a byte stream begin (ITU-T H.264 7.4.1.2.3,
/// H.265 7.4.2.4.4), kept across the PES packet boundaries that cut them.
///
/// ISO/IEC 13818-1 2.4.3.7 says a PES packet starts at an access unit, but
/// real transport streams do not: a PES packet often starts a few bytes
/// before the access unit delimiter of the unit it carries, so the last
/// bytes of the unit before it are in it. Reading each PES payload on its
/// own would cut the units there; this state carries over instead, so a
/// caller appends each payload to the bytes it still holds and asks where
/// the units end.
#[derive(Debug, Default)]
pub(crate) struct AccessUnits {
    /// Whether a coded picture has been read since the last boundary.
    coded: bool,
    /// MPEG-2 video: the last picture read is the first field of a frame
    /// whose second field is still to come, and the picture being read is
    /// such a second field.
    first_field: bool,
    second_field: bool,
    /// Where the next scan starts in the caller's buffer. A start code
    /// whose kind could not be read -- the bytes that say it are past the
    /// end of what the caller holds -- is left here to be read again when
    /// they arrive, since nothing after it may be read first.
    at: usize,
}

impl AccessUnits {
    /// Takes back `bytes` the caller removed from the front of its buffer.
    pub(crate) fn drained(&mut self, bytes: usize) {
        self.at = self.at.saturating_sub(bytes);
    }

    /// Where each access unit after the one `data` starts with begins: the
    /// offsets of the start codes that begin one. An access unit ends at
    /// the first of: an access unit delimiter, a parameter set or a prefix
    /// SEI message after its coded picture, or the first slice of the next
    /// picture.
    ///
    /// The scan carries over the calls: a caller appends to `data` and
    /// asks again, and this reads only what it has not read, back far
    /// enough for a start code, or the bytes that say what its unit is, to
    /// straddle the join.
    pub(crate) fn cuts(&mut self, nal: Nal, data: &[u8]) -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = self.at;
        while let Some(at) = next_start_code(data, i) {
            i = at + 3;
            // The start code's leading zero byte is part of it, so a unit's
            // bytes end where the next one's start code begins. MPEG-2
            // video has no such byte: a zero before a start code is
            // stuffing of the picture before it (13818-2 5.3), and
            // `ffprobe` counts it there.
            let code = at
                - usize::from(
                    nal != Nal::Mpeg2 && at > 0 && data.get(at.wrapping_sub(1)) == Some(&0),
                );
            let Some(&first) = data.get(i) else {
                // Not even the header byte of the unit is here: read this
                // start code again when the rest of it arrives.
                self.at = at;
                return out;
            };
            if nal == Nal::Mpeg2 {
                if !self.mpeg2(data, i, first) {
                    self.at = at;
                    return out;
                }
                // A slice is a coded picture; a sequence header, a group
                // of pictures header or a picture header after one begins
                // the next picture, unless it is the second field of the
                // frame the last picture began.
                let picture = matches!(first, 0x00 | 0xB3 | 0xB8);
                if self.coded && picture && !(first == 0x00 && self.second_field) {
                    out.push(code);
                    self.coded = false;
                }
                self.coded |= (0x01..=0xAF).contains(&first);
                continue;
            }
            let (vcl, before_picture) = match nal {
                Nal::H264 => {
                    let kind = first & 0x1F;
                    (
                        (1..=5).contains(&kind),
                        matches!(kind, 6 | 7 | 8 | 9 | 14 | 15 | 17 | 18),
                    )
                }
                Nal::Hevc => {
                    let kind = (first >> 1) & 0x3F;
                    (kind <= 31, matches!(kind, 32..=35 | 39 | 41..=44))
                }
                Nal::Mpeg2 => (false, false),
            };
            // A VCL unit is judged on the first bits of its slice header,
            // one byte past the header for H.264 and two for HEVC: when
            // they are not here yet, read the start code again rather
            // than take their absence for a no.
            let header = match nal {
                Nal::H264 => 2,
                Nal::Hevc => 3,
                Nal::Mpeg2 => 0,
            };
            if vcl && i.saturating_add(header) > data.len() {
                self.at = at;
                return out;
            }
            let new_unit = self.coded
                && (before_picture
                    || (vcl && first_in_picture(nal, data.get(i..).unwrap_or_default())));
            if new_unit {
                out.push(code);
                self.coded = false;
            }
            self.coded |= vcl;
        }
        // Every byte is read but the last two, which a start code could
        // begin in.
        self.at = data.len().saturating_sub(2);
        out
    }

    /// Follows the fields of MPEG-2 video at the start code whose value
    /// `code` is at `data[i]`; `false` when the bytes that say what it
    /// is are not all here yet. A picture coding extension (13818-2
    /// 6.2.3.1, `extension_start_code_identifier` 8) gives its picture's
    /// `picture_structure` in the low two bits of its third byte: 3 is a
    /// frame, 1 and 2 a field. Fields come in pairs (6.1.1.2), so a field
    /// picture after a first field is the second of its frame, and the
    /// two are one access unit, as `ffprobe` lists them (a capture of the
    /// corpus whose two fields are each in a PES packet of its own).
    fn mpeg2(&mut self, data: &[u8], i: usize, code: u8) -> bool {
        match code {
            0x00 => {
                self.second_field = self.first_field;
                self.first_field = false;
            }
            0xB3 | 0xB8 => {
                self.first_field = false;
                self.second_field = false;
            }
            0xB5 => {
                let Some(ext) = data.get(i.saturating_add(1)..i.saturating_add(4)) else {
                    return false;
                };
                if let [id, _, structure] = *ext {
                    if id >> 4 == 8 && structure & 3 != 3 && !self.second_field {
                        self.first_field = true;
                    }
                }
            }
            _ => {}
        }
        true
    }
}

/// The picture size of an MPEG-1 or MPEG-2 video sequence header in a PES
/// payload.
pub(crate) fn mpeg2_size(data: &[u8]) -> Option<(u32, u32)> {
    let at = data.windows(4).position(|w| w == [0, 0, 1, 0xB3])?;
    let [_, _, _, _, a, b, c, ..] = *data.get(at..)? else {
        return None;
    };
    let width = (u32::from(a) << 4) | u32::from(b >> 4);
    let height = (u32::from(b & 0x0F) << 8) | u32::from(c);
    Some((width, height))
}

/// `profile_and_level_indication` of an MPEG-2 video sequence extension
/// (13818-2 6.2.2.3) in a PES payload: (escape bit and profile, level).
pub(crate) fn mpeg2_profile(data: &[u8]) -> Option<(u8, u8)> {
    let mut i = 0usize;
    while let Some(at) = data
        .get(i..)
        .and_then(|d| d.windows(4).position(|w| w == [0, 0, 1, 0xB5]))
    {
        let ext = i + at + 4;
        if let [a, b, ..] = *data.get(ext..)? {
            if a >> 4 == 1 {
                // sequence_extension: extension_start_code_identifier 1,
                // then the 8-bit profile_and_level_indication.
                let indication = (a << 4) | (b >> 4);
                return Some((indication >> 4, indication & 0x0F));
            }
        }
        i = ext;
    }
    None
}

/// The colour description of an MPEG-2 video sequence display extension
/// (13818-2 6.2.2.4) in a PES payload: colour primaries, transfer
/// characteristics and matrix coefficients, as ITU-T H.273 code points.
/// `None` when the sequence has no display extension or it carries no
/// colour description.
///
/// The extension holds `extension_start_code_identifier` (4 bits, 0b0010),
/// `video_format` (3) and `colour_description` (1); when that flag is set,
/// the colour primaries, transfer characteristics and matrix coefficients
/// follow, one byte each.
pub(crate) fn mpeg2_colour(data: &[u8]) -> Option<(u16, u16, u16)> {
    let mut i = 0usize;
    while let Some(at) = data
        .get(i..)
        .and_then(|d| d.windows(4).position(|w| w == [0, 0, 1, 0xB5]))
    {
        let ext = i.saturating_add(at).saturating_add(4);
        if let [a, primaries, transfer, matrix, ..] = *data.get(ext..)? {
            if a >> 4 == 2 && a & 1 == 1 {
                return Some((u16::from(primaries), u16::from(transfer), u16::from(matrix)));
            }
        }
        i = ext;
    }
    None
}

/// The video size a Blu-ray presentation graphics stream states in its
/// presentation composition segment (Blu-ray BD-ROM part 3-1 9.14.4.2.2):
/// `segment_type` 0x16, a 16-bit length, then `video_width` and
/// `video_height`. A subtitle of that stream is composed on a picture of
/// that size, which is why `ffprobe` reports it as the stream's size.
pub(crate) fn pgs_size(data: &[u8]) -> Option<(u32, u32)> {
    let mut at = 0usize;
    // Segments follow one another in the payload; the composition one
    // need not be first.
    while let Some(&[kind, l0, l1]) = data.get(at..at.checked_add(3)?).and_then(|h| {
        let header: Option<&[u8; 3]> = h.try_into().ok();
        header
    }) {
        let length = usize::from(u16::from_be_bytes([l0, l1]));
        let body = data.get(at.checked_add(3)?..at.checked_add(3)?.checked_add(length)?)?;
        if kind == 0x16 {
            let [w0, w1, h0, h1, ..] = *body else {
                return None;
            };
            let (width, height) = (u16::from_be_bytes([w0, w1]), u16::from_be_bytes([h0, h1]));
            return (width > 0 && height > 0).then_some((u32::from(width), u32::from(height)));
        }
        at = at.checked_add(3)?.checked_add(length)?;
    }
    None
}

/// Whether the first picture of an MPEG-1 or MPEG-2 video PES payload is
/// an I picture (`picture_coding_type` 1, 13818-2 6.3.9).
pub(crate) fn mpeg2_keyframe(data: &[u8]) -> bool {
    let mut i = 0usize;
    while let Some(at) = data
        .get(i..)
        .and_then(|d| d.windows(4).position(|w| w.starts_with(&[0, 0, 1])))
    {
        let code = i + at + 3;
        if data.get(code) == Some(&0x00) {
            // picture_start_code: temporal_reference (10 bits), then
            // picture_coding_type (3 bits).
            return data.get(code + 2).is_some_and(|b| (b >> 3) & 0x07 == 1);
        }
        i = code + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slice_that_starts_within_the_first_slices_header_bytes_keeps_the_key_frame() {
        let mut data = vec![0, 0, 1, 0x65, 0xB8];
        data.extend([0x55; 25]);
        data.extend([0, 0, 1, 0x65, 0x01, 0x00, 0xC0, 0xFF]);
        for refs in [Some(1), Some(4), None] {
            assert_eq!(
                keyframe(Nal::H264, &data, refs),
                Some(true),
                "max_num_ref_frames {refs:?}"
            );
        }
    }

    #[test]
    fn nal_units_between_start_codes() {
        let data = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 0,
        ];
        let units = nal_units(&data);
        assert_eq!(units, vec![&[0x67, 1, 2][..], &[0x68, 3], &[0x65, 4]]);
        assert!(nal_units(&[1, 2, 3]).is_empty());
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn hvcc_repeats_the_sps_format() {
        // The parameter sets of an x265 Main 10 stream, 4:2:0 at 10 bits.
        let mut au = Vec::new();
        for nal in [
            "40010c01ffff02200000030090000003000003003c959809",
            "42010102200000030090000003000003003ca00a080f1365959a4932bc05a02000000300200000030321",
            "4401c172b46240",
        ] {
            au.extend([0, 0, 0, 1]);
            au.extend(hex(nal));
        }
        let mut sets = ParameterSets::default();
        sets.add(Nal::Hevc, &au);
        let record = sets.record(Nal::Hevc).unwrap();
        // Profile idc 2 (Main 10), then chroma 1 and both depths 10 bits.
        assert_eq!(record[1] & 0x1F, 2);
        assert_eq!(&record[16..19], &[0xFD, 0xFA, 0xFA]);
        // One temporal layer, nested, 4-byte lengths.
        assert_eq!(record[21], (1 << 3) | (1 << 2) | 3);
        let described =
            coreav_isobmff::describe_config_record(coreav_isobmff::FourCc(*b"hvcC"), &record)
                .unwrap();
        assert!(!described.malformed);
        assert_eq!(described.parameter_set_size, Some((320, 240)));
    }

    #[test]
    fn parameter_sets_are_bounded_and_deduplicated() {
        let mut sets = ParameterSets::default();
        let mut au = Vec::new();
        for i in 0..5000u16 {
            au.extend([0, 0, 1, 0x67, 100, 0, 30]);
            au.extend(i.to_be_bytes());
        }
        sets.add(Nal::H264, &au);
        sets.add(Nal::H264, &au);
        assert_eq!(sets.sps.len(), MAX_PER_KIND);
        assert!(sets.record(Nal::H264).is_none());
    }

    /// An I-picture of MPEG-2 video: its header, a picture coding
    /// extension of `structure` (3 a frame, 1 and 2 a field), a slice and
    /// a zero byte of stuffing.
    fn mpeg2_picture(structure: u8) -> Vec<u8> {
        let mut out = vec![0, 0, 1, 0x00, 0x00, 0x0F, 0xFF, 0xF8];
        out.extend([0, 0, 1, 0xB5, 0x8F, 0xFF, 0xF0 | structure, 0x80]);
        out.extend([0, 0, 1, 0x01, 0x12, 0x34, 0x56, 0x00]);
        out
    }

    #[test]
    fn mpeg2_video_is_cut_into_pictures_with_fields_paired() {
        // A sequence header and a group of pictures before a frame, a
        // frame, a top and a bottom field, and a frame.
        let mut data = vec![0, 0, 1, 0xB3, 0x14, 0x00, 0xF0, 0x13];
        data.extend([0, 0, 1, 0xB8, 0x00, 0x08, 0x00, 0x00]);
        data.extend(mpeg2_picture(3));
        let second = data.len();
        data.extend(mpeg2_picture(3));
        let fields = data.len();
        data.extend(mpeg2_picture(1));
        data.extend(mpeg2_picture(2));
        let last = data.len();
        data.extend(mpeg2_picture(3));
        let mut units = AccessUnits::default();
        // A cut falls on the start code itself: the zero byte before it
        // is the stuffing of the picture before.
        assert_eq!(units.cuts(Nal::Mpeg2, &data), vec![second, fields, last]);
        // Fed a byte at a time, a picture coding extension whose
        // `picture_structure` has not arrived is read again later.
        let mut bytewise = AccessUnits::default();
        let mut cuts = Vec::new();
        for end in 1..=data.len() {
            cuts.extend(bytewise.cuts(Nal::Mpeg2, &data[..end]));
        }
        assert_eq!(cuts, vec![second, fields, last]);
        // Two fields of the same frame are one unit whatever their
        // parities; a third field begins the next.
        let mut three = mpeg2_picture(2);
        three.extend(mpeg2_picture(1));
        let next = three.len();
        three.extend(mpeg2_picture(2));
        let mut units = AccessUnits::default();
        assert_eq!(units.cuts(Nal::Mpeg2, &three), vec![next]);
        // An I-picture is a keyframe; a B-picture is not.
        assert_eq!(keyframe(Nal::Mpeg2, &data, None), Some(true));
        assert_eq!(
            keyframe(Nal::Mpeg2, &[0, 0, 1, 0x00, 0x00, 0x1F, 0xFF, 0xF8], None),
            Some(false)
        );
    }

    #[test]
    fn access_units_carry_their_state_across_calls() {
        // A delimiter, a slice, then the next delimiter, fed in two parts
        // that cut the second delimiter's start code.
        let mut data = vec![0, 0, 0, 1, 0x09, 0x10, 0, 0, 1, 0x21, 0x88];
        data.resize(60, 0x42);
        data.extend([0, 0, 0, 1, 0x09, 0x10]);
        let mut units = AccessUnits::default();
        assert_eq!(units.cuts(Nal::H264, &data[..61]), Vec::<usize>::new());
        // The second call reads on from where the first stopped.
        assert_eq!(units.cuts(Nal::H264, &data), vec![60]);
        // A buffer that ends in a start code, or before the header byte
        // of the unit after it, is read again when the rest arrives. A
        // delimiter needs its header byte alone, so one byte short is
        // already enough to place it.
        for short in 2..=5 {
            let mut again = AccessUnits::default();
            let end = data.len() - short;
            assert_eq!(
                again.cuts(Nal::H264, &data[..end]),
                Vec::<usize>::new(),
                "{short}"
            );
            assert_eq!(again.cuts(Nal::H264, &data), vec![60], "{short}");
        }
        let mut whole = AccessUnits::default();
        assert_eq!(whole.cuts(Nal::H264, &data[..data.len() - 1]), vec![60]);
        // A slice needs the first bits of its slice header too.
        let mut slice = vec![0, 0, 0, 1, 0x09, 0x10, 0, 0, 1, 0x21, 0x88];
        slice.resize(40, 0x42);
        slice.extend([0, 0, 1, 0x21, 0x88]);
        let mut late = AccessUnits::default();
        assert_eq!(
            late.cuts(Nal::H264, &slice[..slice.len() - 1]),
            Vec::<usize>::new()
        );
        assert_eq!(late.cuts(Nal::H264, &slice), vec![40]);
        // Bytes the caller takes off the front move the place with them.
        let mut moved = AccessUnits::default();
        assert_eq!(moved.cuts(Nal::H264, &data[..61]), Vec::<usize>::new());
        moved.drained(30);
        assert_eq!(moved.cuts(Nal::H264, &data[30..]), vec![30]);
    }

    #[test]
    fn presentation_graphics_state_their_picture() {
        let payload = hex("16001307800438100000800000010000000003800384");
        assert_eq!(pgs_size(&payload), Some((1920, 1080)));
        // The composition segment need not be first.
        let mut later = hex("17000a01000380038403800024");
        later.extend(&payload);
        assert_eq!(pgs_size(&later), Some((1920, 1080)));
        assert_eq!(pgs_size(&payload[..6]), None);
        assert_eq!(pgs_size(&[]), None);
        // A segment that runs past the payload.
        assert_eq!(pgs_size(&hex("16ffff07800438")), None);
    }

    #[test]
    fn rbsp_drops_emulation_prevention() {
        assert_eq!(rbsp(&[0, 0, 3, 1, 0, 0, 3], 64), vec![0, 0, 1, 0, 0]);
    }

    #[test]
    fn mpeg2_sequence_header() {
        let data = [0xFF, 0, 0, 1, 0xB3, 0x2D, 0x01, 0xE0, 0x24];
        assert_eq!(mpeg2_size(&data), Some((720, 480)));
        assert_eq!(mpeg2_size(&data[..6]), None);
        // A picture header with picture_coding_type 1.
        assert!(mpeg2_keyframe(&[0, 0, 1, 0xB3, 0, 0, 1, 0x00, 0x00, 0x08]));
        assert!(!mpeg2_keyframe(&[0, 0, 1, 0x00, 0x00, 0x10]));
    }
}
