//! What a transport stream's elementary streams state about themselves,
//! read from their first access units: the sequence parameter sets of
//! H.264 (ITU-T H.264 7.3.2.1.1, E.1.1) and H.265 (ITU-T H.265 7.3.2.2,
//! E.2.1), the MPEG-1 and MPEG-2 video sequence header and its extensions
//! (ISO/IEC 11172-2 2.4.2.3, ISO/IEC 13818-2 6.2.2 and 6.2.3.1), and the
//! frame headers of MPEG audio (ISO/IEC 11172-3 2.4.1.3, 13818-3) and
//! AC-3 / E-AC-3 (ATSC A/52 5.3 and E.1.2).
//!
//! Sans-IO and bounded: every read stops at the end of its bytes, and a
//! field that cannot be read is `None`, never a panic.

/// Most bytes of a parameter set read: far more than any real one holds.
const MAX_PARAMETER_SET: usize = 4096;

/// Reads bits most significant first, as the video standards write them.
pub struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    /// `n` bits (at most 32) as an unsigned number.
    pub fn bits(&mut self, n: u32) -> Option<u32> {
        if n > 32 {
            return None;
        }
        let mut value: u64 = 0;
        for _ in 0..n {
            let byte = *self.data.get(self.at / 8)?;
            let bit = (byte >> (7 - (self.at % 8))) & 1;
            value = (value << 1) | u64::from(bit);
            self.at += 1;
        }
        u32::try_from(value).ok()
    }

    pub fn flag(&mut self) -> Option<bool> {
        self.bits(1).map(|b| b == 1)
    }

    pub fn skip(&mut self, n: u32) -> Option<()> {
        let end = self.at.checked_add(usize::try_from(n).ok()?)?;
        if end > self.data.len().saturating_mul(8) {
            return None;
        }
        self.at = end;
        Some(())
    }

    /// An unsigned Exp-Golomb code (`ue(v)`), up to 32 bits of value.
    pub fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0u32;
        while !self.flag()? {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let rest = self.bits(zeros)?;
        let value = (1u64 << zeros) - 1 + u64::from(rest);
        u32::try_from(value).ok()
    }

    /// A signed Exp-Golomb code (`se(v)`).
    pub fn se(&mut self) -> Option<i64> {
        let k = i64::from(self.ue()?);
        Some(if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) })
    }
}

/// Removes the emulation prevention bytes (`00 00 03`) of a NAL unit,
/// keeping at most [`MAX_PARAMETER_SET`] bytes.
pub fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len().min(MAX_PARAMETER_SET));
    let mut zeros = 0usize;
    for &byte in nal {
        if out.len() >= MAX_PARAMETER_SET {
            break;
        }
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
        out.push(byte);
    }
    out
}

/// The NAL units of an Annex B byte stream: for each, where its
/// three-byte start code prefix (`00 00 01`) begins and its bytes (header
/// included, trailing zero bytes of the next start code left out).
pub fn annex_b(data: &[u8]) -> Vec<(usize, &[u8])> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while let Some(rest) = data.get(i..) {
        let Some(at) = rest.windows(3).position(|w| w == [0, 0, 1]) else {
            break;
        };
        starts.push(i + at);
        i += at + 3;
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(data.len());
        let mut body = data.get(start + 3..end).unwrap_or_default();
        while let [head @ .., 0] = body {
            body = head;
        }
        out.push((start, body));
    }
    out
}

/// The bytes `ffprobe` keeps as a coded video stream's extradata: from the
/// start code of its first parameter set to the start code of the first
/// NAL unit after its parameter sets.
pub fn annex_b_extradata(data: &[u8], hevc: bool) -> Option<usize> {
    let units = annex_b(data);
    let is_set = |nal: &[u8]| {
        let Some(&first) = nal.first() else {
            return false;
        };
        if hevc {
            matches!((first >> 1) & 0x3F, 32..=34)
        } else {
            matches!(first & 0x1F, 7 | 8)
        }
    };
    let first = units.iter().position(|(_, nal)| is_set(nal))?;
    let (start, _) = *units.get(first)?;
    let end = units
        .iter()
        .skip(first)
        .find(|(_, nal)| !is_set(nal))
        .map_or(data.len(), |(at, _)| *at);
    end.checked_sub(start)
}

/// The first NAL unit of `data` whose type `wanted` accepts, without its
/// emulation prevention bytes.
pub fn find_nal(data: &[u8], wanted: impl Fn(&[u8]) -> bool) -> Option<Vec<u8>> {
    annex_b(data)
        .into_iter()
        .find(|(_, nal)| wanted(nal))
        .map(|(_, nal)| rbsp(nal))
}

/// What the video usability information of a parameter set states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Vui {
    /// The sample aspect ratio, when stated and not unspecified.
    pub sar: Option<(u32, u32)>,
    /// `video_full_range_flag`, when the video signal type is stated.
    pub full_range: Option<bool>,
    /// Colour primaries, transfer characteristics and matrix
    /// coefficients, when the colour description is stated.
    pub colour: Option<(u8, u8, u8)>,
    /// `chroma_sample_loc_type_top_field`, when stated.
    pub chroma_location: Option<u32>,
    /// `num_units_in_tick` and `time_scale`, when stated.
    pub timing: Option<(u32, u32)>,
    /// `max_num_reorder_frames` (H.264), when the bitstream restriction is
    /// stated.
    pub reorder: Option<u32>,
}

/// What `ffprobe` reports of an H.264 or H.265 sequence parameter set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sps {
    pub chroma_format_idc: u32,
    pub bit_depth: u32,
    /// H.264: `frame_mbs_only_flag`.
    pub frame_mbs_only: bool,
    /// H.265: `sps_max_num_reorder_pics` of the highest sub-layer.
    pub reorder: Option<u32>,
    pub vui: Vui,
    /// H.264: what its slice headers need of it.
    pub slices: SliceSyntax,
}

/// What an H.264 sequence parameter set tells of the syntax of its slice
/// headers and picture timing messages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SliceSyntax {
    pub separate_colour_plane: bool,
    pub log2_max_frame_num: u32,
    pub poc_type: u32,
    pub log2_max_poc_lsb: u32,
    /// `pic_struct_present_flag`.
    pub pic_struct_present: bool,
    /// The lengths of `cpb_removal_delay` and `dpb_output_delay`, when an
    /// HRD is present (`CpbDpbDelaysPresentFlag`).
    pub delays: Option<(u32, u32)>,
}

/// The sample aspect ratios `aspect_ratio_idc` 1 to 16 stand for (ITU-T
/// H.264 table E-1, H.265 table E.1).
const SAR: [(u32, u32); 16] = [
    (1, 1),
    (12, 11),
    (10, 11),
    (16, 11),
    (40, 33),
    (24, 11),
    (20, 11),
    (32, 11),
    (80, 33),
    (18, 11),
    (15, 11),
    (64, 33),
    (160, 99),
    (4, 3),
    (3, 2),
    (2, 1),
];

/// `aspect_ratio_info` (E.1.1 / E.2.1), when the flag before it is set.
fn aspect_ratio(bits: &mut Bits<'_>) -> Option<Option<(u32, u32)>> {
    let idc = bits.bits(8)?;
    if idc == 255 {
        let w = bits.bits(16)?;
        let h = bits.bits(16)?;
        return Some((w > 0 && h > 0).then_some((w, h)));
    }
    Some(
        idc.checked_sub(1)
            .and_then(|i| SAR.get(usize::try_from(i).ok()?))
            .copied(),
    )
}

/// The start of a VUI shared by H.264 and H.265: aspect ratio, overscan,
/// video signal type and chroma location.
fn vui_head(bits: &mut Bits<'_>, vui: &mut Vui) -> Option<()> {
    if bits.flag()? {
        vui.sar = aspect_ratio(bits)?;
    }
    if bits.flag()? {
        bits.skip(1)?;
    }
    if bits.flag()? {
        bits.skip(3)?;
        vui.full_range = Some(bits.flag()?);
        if bits.flag()? {
            let p = u8::try_from(bits.bits(8)?).ok()?;
            let t = u8::try_from(bits.bits(8)?).ok()?;
            let m = u8::try_from(bits.bits(8)?).ok()?;
            vui.colour = Some((p, t, m));
        }
    }
    if bits.flag()? {
        vui.chroma_location = Some(bits.ue()?);
        bits.ue()?;
    }
    Some(())
}

/// `hrd_parameters()` of H.264 (E.1.2), skipped.
/// `hrd_parameters()` of H.264 (E.1.2): the lengths of
/// `cpb_removal_delay` and `dpb_output_delay`.
fn h264_hrd(bits: &mut Bits<'_>) -> Option<(u32, u32)> {
    let count = bits.ue()?.checked_add(1)?;
    if count > 32 {
        return None;
    }
    bits.skip(8)?;
    for _ in 0..count {
        bits.ue()?;
        bits.ue()?;
        bits.skip(1)?;
    }
    bits.skip(5)?;
    let cpb = bits.bits(5)? + 1;
    let dpb = bits.bits(5)? + 1;
    bits.skip(5)?;
    Some((cpb, dpb))
}

/// Reads an H.264 sequence parameter set NAL unit (header byte included,
/// emulation prevention removed).
pub fn h264_sps(nal: &[u8]) -> Option<Sps> {
    let mut bits = Bits::new(nal.get(1..)?);
    let profile_idc = bits.bits(8)?;
    bits.skip(16)?;
    bits.ue()?;
    let mut sps = Sps {
        chroma_format_idc: 1,
        bit_depth: 8,
        ..Sps::default()
    };
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        sps.chroma_format_idc = bits.ue()?;
        if sps.chroma_format_idc == 3 {
            sps.slices.separate_colour_plane = bits.flag()?;
        }
        sps.bit_depth = bits.ue()?.checked_add(8)?;
        bits.ue()?;
        bits.skip(1)?;
        if bits.flag()? {
            let lists = if sps.chroma_format_idc == 3 { 12 } else { 8 };
            for i in 0..lists {
                if bits.flag()? {
                    let size = if i < 6 { 16 } else { 64 };
                    let mut last = 8i64;
                    let mut next = 8i64;
                    for _ in 0..size {
                        if next != 0 {
                            next = (last + bits.se()? + 256) % 256;
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }
    sps.slices.log2_max_frame_num = bits.ue()?.checked_add(4)?;
    sps.slices.poc_type = bits.ue()?;
    match sps.slices.poc_type {
        0 => {
            sps.slices.log2_max_poc_lsb = bits.ue()?.checked_add(4)?;
        }
        1 => {
            bits.skip(1)?;
            bits.se()?;
            bits.se()?;
            let cycle = bits.ue()?;
            if cycle > 255 {
                return None;
            }
            for _ in 0..cycle {
                bits.se()?;
            }
        }
        _ => {}
    }
    bits.ue()?;
    bits.skip(1)?;
    bits.ue()?;
    bits.ue()?;
    sps.frame_mbs_only = bits.flag()?;
    if !sps.frame_mbs_only {
        bits.skip(1)?;
    }
    bits.skip(1)?;
    if bits.flag()? {
        for _ in 0..4 {
            bits.ue()?;
        }
    }
    if !bits.flag()? {
        return Some(sps);
    }
    let vui = &mut sps.vui;
    vui_head(&mut bits, vui)?;
    if bits.flag()? {
        let units = bits.bits(32)?;
        let scale = bits.bits(32)?;
        bits.skip(1)?;
        vui.timing = (units > 0 && scale > 0).then_some((units, scale));
    }
    if bits.flag()? {
        sps.slices.delays = Some(h264_hrd(&mut bits)?);
    }
    if bits.flag()? {
        let vcl = h264_hrd(&mut bits)?;
        sps.slices.delays = sps.slices.delays.or(Some(vcl));
    }
    if sps.slices.delays.is_some() {
        bits.skip(1)?;
    }
    sps.slices.pic_struct_present = bits.flag()?;
    let vui = &mut sps.vui;
    if bits.flag()? {
        bits.skip(1)?;
        for _ in 0..4 {
            bits.ue()?;
        }
        vui.reorder = Some(bits.ue()?);
        bits.ue()?;
    }
    Some(sps)
}

/// `profile_tier_level(1, max_sub_layers_minus1)` (H.265 7.3.3), skipped.
fn hevc_profile_tier_level(bits: &mut Bits<'_>, max_sub_layers_minus1: u32) -> Option<()> {
    bits.skip(96)?;
    let mut present = [(false, false); 8];
    for slot in present.iter_mut().take(max_sub_layers_minus1 as usize) {
        *slot = (bits.flag()?, bits.flag()?);
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            bits.skip(2)?;
        }
    }
    for (profile, level) in present.iter().take(max_sub_layers_minus1 as usize) {
        if *profile {
            bits.skip(88)?;
        }
        if *level {
            bits.skip(8)?;
        }
    }
    Some(())
}

/// `scaling_list_data()` (H.265 7.3.4), skipped.
fn hevc_scaling_lists(bits: &mut Bits<'_>) -> Option<()> {
    for size_id in 0u32..4 {
        let step = if size_id == 3 { 3 } else { 1 };
        let mut matrix_id = 0u32;
        while matrix_id < 6 {
            if bits.flag()? {
                let coefficients = 64u32.min(1 << (4 + (size_id << 1)));
                if size_id > 1 {
                    bits.se()?;
                }
                for _ in 0..coefficients {
                    bits.se()?;
                }
            } else {
                bits.ue()?;
            }
            matrix_id += step;
        }
    }
    Some(())
}

/// The short-term reference picture sets of an H.265 SPS (7.3.7),
/// skipped.
fn hevc_short_term_sets(bits: &mut Bits<'_>) -> Option<()> {
    let sets = bits.ue()?;
    if sets > 64 {
        return None;
    }
    let mut counts: Vec<u32> = Vec::with_capacity(sets as usize);
    for index in 0..sets {
        let predicted = index != 0 && bits.flag()?;
        let count = if predicted {
            bits.skip(1)?;
            bits.ue()?;
            let reference = *counts.last()?;
            let mut count = 0u32;
            for _ in 0..=reference {
                let used = bits.flag()?;
                if used || bits.flag()? {
                    count += 1;
                }
            }
            count
        } else {
            let negative = bits.ue()?;
            let positive = bits.ue()?;
            if negative > 16 || positive > 16 {
                return None;
            }
            for _ in 0..negative + positive {
                bits.ue()?;
                bits.skip(1)?;
            }
            negative + positive
        };
        if count > 32 {
            return None;
        }
        counts.push(count);
    }
    Some(())
}

/// Reads an H.265 sequence parameter set NAL unit (two header bytes
/// included, emulation prevention removed).
pub fn hevc_sps(nal: &[u8]) -> Option<Sps> {
    let mut bits = Bits::new(nal.get(2..)?);
    bits.skip(4)?;
    let max_sub_layers_minus1 = bits.bits(3)?;
    if max_sub_layers_minus1 > 6 {
        return None;
    }
    bits.skip(1)?;
    hevc_profile_tier_level(&mut bits, max_sub_layers_minus1)?;
    bits.ue()?;
    let mut sps = Sps {
        chroma_format_idc: bits.ue()?,
        frame_mbs_only: true,
        ..Sps::default()
    };
    if sps.chroma_format_idc == 3 {
        bits.skip(1)?;
    }
    bits.ue()?;
    bits.ue()?;
    if bits.flag()? {
        for _ in 0..4 {
            bits.ue()?;
        }
    }
    sps.bit_depth = bits.ue()?.checked_add(8)?;
    bits.ue()?;
    let log2_max_poc_lsb = bits.ue()?.checked_add(4)?;
    if log2_max_poc_lsb > 16 {
        return None;
    }
    let all_layers = bits.flag()?;
    let first = if all_layers { 0 } else { max_sub_layers_minus1 };
    for _ in first..=max_sub_layers_minus1 {
        bits.ue()?;
        sps.reorder = Some(bits.ue()?);
        bits.ue()?;
    }
    for _ in 0..6 {
        bits.ue()?;
    }
    if bits.flag()? && bits.flag()? {
        hevc_scaling_lists(&mut bits)?;
    }
    bits.skip(2)?;
    if bits.flag()? {
        bits.skip(8)?;
        bits.ue()?;
        bits.ue()?;
        bits.skip(1)?;
    }
    hevc_short_term_sets(&mut bits)?;
    if bits.flag()? {
        let long_term = bits.ue()?;
        if long_term > 32 {
            return None;
        }
        for _ in 0..long_term {
            bits.skip(log2_max_poc_lsb)?;
            bits.skip(1)?;
        }
    }
    bits.skip(2)?;
    if !bits.flag()? {
        return Some(sps);
    }
    let vui = &mut sps.vui;
    vui_head(&mut bits, vui)?;
    // neutral_chroma_indication, field_seq, frame_field_info_present.
    bits.skip(3)?;
    if bits.flag()? {
        for _ in 0..4 {
            bits.ue()?;
        }
    }
    if bits.flag()? {
        let units = bits.bits(32)?;
        let scale = bits.bits(32)?;
        vui.timing = (units > 0 && scale > 0).then_some((units, scale));
    }
    Some(sps)
}

/// `bottom_field_pic_order_in_frame_present_flag` of an H.264 picture
/// parameter set NAL unit (7.3.2.2).
fn h264_pps_bottom_poc(nal: &[u8]) -> Option<bool> {
    let mut bits = Bits::new(nal.get(1..)?);
    bits.ue()?;
    bits.ue()?;
    bits.skip(1)?;
    bits.flag()
}

/// `pic_struct` of the picture timing message (D.1.3, D.2.3) of an H.264
/// SEI NAL unit, if it has one.
fn h264_pic_struct(nal: &[u8], syntax: &SliceSyntax) -> Option<u32> {
    let mut rest = nal.get(1..)?;
    loop {
        let mut kind = 0u32;
        loop {
            let (&byte, tail) = rest.split_first()?;
            rest = tail;
            kind += u32::from(byte);
            if byte != 0xFF {
                break;
            }
        }
        let mut size = 0usize;
        loop {
            let (&byte, tail) = rest.split_first()?;
            rest = tail;
            size += usize::from(byte);
            if byte != 0xFF {
                break;
            }
        }
        let payload = rest.get(..size)?;
        rest = rest.get(size..)?;
        if kind == 1 {
            if !syntax.pic_struct_present {
                return None;
            }
            let mut bits = Bits::new(payload);
            if let Some((cpb, dpb)) = syntax.delays {
                bits.skip(cpb)?;
                bits.skip(dpb)?;
            }
            return bits.bits(4);
        }
    }
}

/// Most bytes of an access unit [`h264_picture`] looks through for its
/// first slice: the parameter sets and messages before it are far
/// shorter.
const MAX_PICTURE_HEAD: usize = 64 * 1024;

/// What the first picture of an H.264 access unit states of its timing:
/// whether it is a field (and which), and the `pic_struct` of its picture
/// timing message, when the sequence carries one (D.2.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Picture {
    /// `Some(bottom_field_flag)` for a field picture.
    pub field: Option<bool>,
    pub pic_struct: Option<u32>,
}

impl Picture {
    /// How many ticks of the sequence's clock (`num_units_in_tick /
    /// time_scale`, E.2.1) the picture is shown for: a field one, a frame
    /// two, and as table D-1 has it for the `pic_struct` values that
    /// repeat a field or a frame.
    pub fn ticks(self) -> u32 {
        if self.field.is_some() {
            return 1;
        }
        match self.pic_struct {
            Some(1 | 2) => 1,
            Some(5 | 6) => 3,
            Some(7) => 4,
            Some(8) => 6,
            _ => 2,
        }
    }
}

/// The NAL units of `data` up to and including the first slice of a
/// picture (types 1 and 5), each with its header byte.
fn units_to_first_slice(data: &[u8]) -> Vec<&[u8]> {
    let data = data.get(..MAX_PICTURE_HEAD).unwrap_or(data);
    // Where the NAL unit after the next start code (`00 00 01`) at or
    // after `from` begins. The units are found one at a time, so a large
    // picture is not searched past its first slice header.
    let next = |from: usize| -> Option<usize> {
        let rest = data.get(from..)?;
        rest.windows(3)
            .position(|w| w == [0, 0, 1])
            .map(|at| from + at + 3)
    };
    let mut out = Vec::new();
    let mut at = next(0);
    while let Some(start) = at {
        let kind = data.get(start).map(|b| b & 0x1F);
        if matches!(kind, Some(1 | 5)) {
            // The slice header is all a reader of the picture needs.
            let slice = data.get(start..).unwrap_or_default();
            out.push(slice.get(..64).unwrap_or(slice));
            break;
        }
        at = next(start);
        let end = at.map_or(data.len(), |n| n - 3);
        let mut body = data.get(start..end).unwrap_or_default();
        while let [head @ .., 0] = body {
            body = head;
        }
        out.push(body);
    }
    out
}

/// Reads [`Picture`] from an H.264 access unit, `None` when it has no
/// slice header that can be read.
pub fn h264_picture(unit: &[u8], sps: &Sps) -> Option<Picture> {
    let syntax = sps.slices;
    let units = units_to_first_slice(unit);
    let kind = |nal: &[u8]| nal.first().map(|b| b & 0x1F);
    let pic_struct = units
        .iter()
        .filter(|nal| kind(nal) == Some(6))
        .find_map(|nal| h264_pic_struct(&rbsp(nal), &syntax));
    let slice = units.iter().find(|nal| matches!(kind(nal), Some(1 | 5)))?;
    let slice = rbsp(slice.get(..64).unwrap_or(slice));
    let mut bits = Bits::new(slice.get(1..)?);
    bits.ue()?;
    bits.ue()?;
    bits.ue()?;
    if syntax.separate_colour_plane {
        bits.skip(2)?;
    }
    bits.skip(syntax.log2_max_frame_num)?;
    let field = if !sps.frame_mbs_only && bits.flag()? {
        Some(bits.flag()?)
    } else {
        None
    };
    Some(Picture { field, pic_struct })
}

/// The field order `ffprobe` reports for an H.264 stream whose sequence
/// allows field coding, from the first picture of an access unit: a field
/// names its parity; a frame takes the picture structure its timing
/// message states, else the order of its two fields' picture order counts
/// (8.2.1), equal counts making it progressive.
pub fn h264_field_order(unit: &[u8], sps: &Sps) -> Option<&'static str> {
    let syntax = sps.slices;
    let units = annex_b(unit);
    let kind = |nal: &[u8]| nal.first().map(|b| b & 0x1F);
    let bottom_poc = units
        .iter()
        .find(|(_, nal)| kind(nal) == Some(8))
        .and_then(|(_, nal)| h264_pps_bottom_poc(&rbsp(nal)))?;
    let pic_struct = units
        .iter()
        .filter(|(_, nal)| kind(nal) == Some(6))
        .find_map(|(_, nal)| h264_pic_struct(&rbsp(nal), &syntax));
    let (_, slice) = units
        .iter()
        .find(|(_, nal)| matches!(kind(nal), Some(1 | 5)))?;
    let idr = kind(slice) == Some(5);
    let slice = rbsp(slice);
    let mut bits = Bits::new(slice.get(1..)?);
    bits.ue()?;
    bits.ue()?;
    bits.ue()?;
    if syntax.separate_colour_plane {
        bits.skip(2)?;
    }
    bits.skip(syntax.log2_max_frame_num)?;
    if !sps.frame_mbs_only && bits.flag()? {
        return Some(if bits.flag()? { "bb" } else { "tt" });
    }
    match pic_struct {
        Some(0) => return Some("progressive"),
        Some(3 | 5) => return Some("tt"),
        Some(4 | 6) => return Some("bb"),
        Some(_) => return None,
        None => {}
    }
    if idr {
        bits.ue()?;
    }
    match (syntax.poc_type, bottom_poc) {
        // The two fields of a frame share their order count (8.2.1.1,
        // 8.2.1.3).
        (0, false) | (2, _) => return Some("progressive"),
        (0, true) => {}
        _ => return None,
    }
    bits.skip(syntax.log2_max_poc_lsb)?;
    let delta = bits.se()?;
    Some(match delta {
        0 => "progressive",
        d if d > 0 => "tt",
        _ => "bb",
    })
}

/// What the MPEG-1 or MPEG-2 video sequence header, its extensions and
/// the first picture coding extension state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mpeg2 {
    pub aspect_ratio_information: u8,
    pub frame_rate_code: u8,
    /// `bit_rate_value`, with `bit_rate_extension` above it (MPEG-2).
    pub bit_rate: u32,
    /// `vbv_buffer_size_value`, with its extension above it (MPEG-2).
    pub vbv_buffer_size: u32,
    /// The sequence extension, when there is one (MPEG-2).
    pub extension: Option<SequenceExtension>,
    /// The first picture coding extension: `picture_structure`,
    /// `top_field_first`, `progressive_frame`.
    pub picture: Option<(u8, bool, bool)>,
    /// The first picture header's `vbv_delay` (13818-2 6.2.3), `0xFFFF`
    /// for a stream of variable rate (6.3.9).
    pub vbv_delay: Option<u16>,
}

/// The sequence extension (ISO/IEC 13818-2 6.2.2.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SequenceExtension {
    pub progressive_sequence: bool,
    pub chroma_format: u8,
    pub low_delay: bool,
    pub frame_rate_extension: (u8, u8),
}

/// The start codes of `data` and what follows each.
fn start_codes(data: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut i = 0usize;
    std::iter::from_fn(move || {
        let rest = data.get(i..)?;
        let at = rest.windows(3).position(|w| w == [0, 0, 1])?;
        let code = *rest.get(at + 3)?;
        let body = rest.get(at + 4..).unwrap_or_default();
        i += at + 4;
        Some((code, body))
    })
}

/// Reads the sequence header of an MPEG-1 or MPEG-2 video access unit,
/// and the extensions after it.
pub fn mpeg2(data: &[u8]) -> Option<Mpeg2> {
    let mut seen = None;
    for (code, body) in start_codes(data) {
        match code {
            0xB3 if seen.is_none() => {
                let mut bits = Bits::new(body);
                bits.skip(24)?;
                let aspect = u8::try_from(bits.bits(4)?).ok()?;
                let rate = u8::try_from(bits.bits(4)?).ok()?;
                let bit_rate = bits.bits(18)?;
                bits.skip(1)?;
                let vbv = bits.bits(10)?;
                seen = Some(Mpeg2 {
                    aspect_ratio_information: aspect,
                    frame_rate_code: rate,
                    bit_rate,
                    vbv_buffer_size: vbv,
                    ..Mpeg2::default()
                });
            }
            0x00 => {
                if let Some(seq) = seen.as_mut().filter(|s| s.vbv_delay.is_none()) {
                    let mut bits = Bits::new(body);
                    bits.skip(13)?;
                    seq.vbv_delay = u16::try_from(bits.bits(16)?).ok();
                }
            }
            0xB5 => {
                let Some(seq) = seen.as_mut() else {
                    continue;
                };
                let mut bits = Bits::new(body);
                match bits.bits(4)? {
                    1 if seq.extension.is_none() => {
                        bits.skip(8)?;
                        let progressive = bits.flag()?;
                        let chroma = u8::try_from(bits.bits(2)?).ok()?;
                        bits.skip(4)?;
                        let rate_ext = bits.bits(12)?;
                        bits.skip(1)?;
                        let vbv_ext = bits.bits(8)?;
                        let low_delay = bits.flag()?;
                        let n = u8::try_from(bits.bits(2)?).ok()?;
                        let d = u8::try_from(bits.bits(5)?).ok()?;
                        seq.bit_rate |= rate_ext << 18;
                        seq.vbv_buffer_size |= vbv_ext << 10;
                        seq.extension = Some(SequenceExtension {
                            progressive_sequence: progressive,
                            chroma_format: chroma,
                            low_delay,
                            frame_rate_extension: (n, d),
                        });
                    }
                    8 if seq.picture.is_none() => {
                        bits.skip(16)?;
                        bits.skip(2)?;
                        let structure = u8::try_from(bits.bits(2)?).ok()?;
                        let top_first = bits.flag()?;
                        bits.skip(6)?;
                        bits.skip(1)?;
                        let progressive = bits.flag()?;
                        seq.picture = Some((structure, top_first, progressive));
                        return seen;
                    }
                    _ => {}
                }
            }
            // A slice: the picture coding extension, if any, is behind.
            0x01..=0xAF if seen.is_some() => return seen,
            _ => {}
        }
    }
    seen
}

/// The frame rate `frame_rate_code` stands for (13818-2 table 6-4), as a
/// fraction.
pub fn mpeg2_frame_rate(code: u8) -> Option<(u32, u32)> {
    Some(match code {
        1 => (24_000, 1001),
        2 => (24, 1),
        3 => (25, 1),
        4 => (30_000, 1001),
        5 => (30, 1),
        6 => (50, 1),
        7 => (60_000, 1001),
        8 => (60, 1),
        _ => return None,
    })
}

/// The extradata of an MPEG-1 or MPEG-2 video stream: from the sequence
/// header's start code to the start code after its extensions.
pub fn mpeg2_extradata(data: &[u8]) -> Option<usize> {
    let start = data.windows(4).position(|w| w == [0, 0, 1, 0xB3])?;
    let mut at = start + 4;
    loop {
        let rest = data.get(at..)?;
        let Some(next) = rest.windows(3).position(|w| w == [0, 0, 1]) else {
            return Some(data.len() - start);
        };
        let code = rest.get(next + 3).copied();
        if code != Some(0xB5) {
            return Some(at + next - start);
        }
        at += next + 4;
    }
}

/// The bit rate in an MPEG audio frame header (ISO/IEC 11172-3 table
/// 2.4.2.3, ISO/IEC 13818-3 table B.1), bits per second; `None` for the
/// free format and invalid values.
pub fn mpeg_audio_bit_rate(header: &[u8]) -> Option<u32> {
    let [0xFF, b1, b2, ..] = *header else {
        return None;
    };
    if b1 & 0xE0 != 0xE0 {
        return None;
    }
    let version = (b1 >> 3) & 3; // 3: MPEG-1, 2: MPEG-2, 0: MPEG-2.5
    let layer = (b1 >> 1) & 3; // 3: I, 2: II, 1: III
    let index = usize::from(b2 >> 4);
    const V1L1: [u32; 15] = [
        0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
    ];
    const V1L2: [u32; 15] = [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
    ];
    const V1L3: [u32; 15] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ];
    const V2L1: [u32; 15] = [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
    ];
    const V2L23: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
    let table = match (version, layer) {
        (3, 3) => &V1L1,
        (3, 2) => &V1L2,
        (3, 1) => &V1L3,
        (0 | 2, 3) => &V2L1,
        (0 | 2, 1 | 2) => &V2L23,
        _ => return None,
    };
    let kbps = *table.get(index)?;
    (kbps > 0).then_some(kbps * 1000)
}

/// What an AC-3 or E-AC-3 frame header states: the bit rate, `acmod`,
/// `lfeon`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ac3 {
    pub bit_rate: u32,
    pub acmod: u8,
    pub lfe: bool,
}

/// Reads an AC-3 (A/52 5.3.1) or E-AC-3 (A/52 E.1.2) frame header.
pub fn ac3(header: &[u8]) -> Option<Ac3> {
    let [0x0B, 0x77, ..] = *header else {
        return None;
    };
    let bsid = header.get(5)? >> 3;
    let mut bits = Bits::new(header.get(2..)?);
    if bsid <= 10 {
        bits.skip(16)?;
        let fscod = bits.bits(2)?;
        let frmsizecod = bits.bits(6)?;
        bits.skip(8)?;
        let acmod = u8::try_from(bits.bits(3)?).ok()?;
        if acmod & 1 != 0 && acmod != 1 {
            bits.skip(2)?;
        }
        if acmod & 4 != 0 {
            bits.skip(2)?;
        }
        if acmod == 2 {
            bits.skip(2)?;
        }
        let lfe = bits.flag()?;
        const RATES: [u32; 19] = [
            32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
        ];
        let kbps = *RATES.get(usize::try_from(frmsizecod / 2).ok()?)?;
        let shift = bsid.saturating_sub(8);
        if fscod == 3 {
            return None;
        }
        return Some(Ac3 {
            bit_rate: (kbps * 1000) >> shift,
            acmod,
            lfe,
        });
    }
    if bsid > 16 {
        return None;
    }
    bits.skip(2)?; // strmtyp
    bits.skip(3)?; // substreamid
    let words = bits.bits(11)?.checked_add(1)?;
    let fscod = bits.bits(2)?;
    let (rate, blocks) = if fscod == 3 {
        let fscod2 = bits.bits(2)?;
        let rate = match fscod2 {
            0 => 24_000,
            1 => 22_050,
            2 => 16_000,
            _ => return None,
        };
        (rate, 6)
    } else {
        let numblkscod = bits.bits(2)?;
        let rate = match fscod {
            0 => 48_000,
            1 => 44_100,
            _ => 32_000,
        };
        (
            rate,
            [1, 2, 3, 6]
                .get(usize::try_from(numblkscod).ok()?)
                .copied()?,
        )
    };
    let acmod = u8::try_from(bits.bits(3)?).ok()?;
    let lfe = bits.flag()?;
    let bytes = u64::from(words) * 2;
    let bit_rate = bytes * 8 * rate / (256 * blocks);
    Some(Ac3 {
        bit_rate: u32::try_from(bit_rate).ok()?,
        acmod,
        lfe,
    })
}

/// The sampling rates of `sampling_frequency_index` (ISO/IEC 14496-3
/// 1.6.3.4, table 1.18).
pub const AAC_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// Whether an AAC `raw_data_block()` (ISO/IEC 14496-3 4.4.2.1) ends with
/// a fill element that carries SBR data (`extension_type` EXT_SBR_DATA
/// or EXT_SBR_DATA_CRC, 4.4.2.7 and table 4.121) just before its
/// `ID_END`. The block is read from its end, the only end a reader that
/// does not decode the spectral data before it can find: `ID_END` (`111`)
/// then up to seven zero bits of byte alignment, and before it a fill
/// element (`110`) whose `count` (with `esc_count`) is exactly the bytes
/// between its header and `ID_END`, starting with the SBR extension type.
/// A frame of plain AAC shows no such element (none in 140 frames of
/// seven AAC-LC captures of the corpus), and an HE-AAC frame shows it
/// whenever it carries SBR data.
pub fn aac_ends_with_sbr(block: &[u8]) -> bool {
    let bit = |at: usize| -> Option<u8> {
        let byte = *block.get(at / 8)?;
        Some((byte >> (7 - (at % 8))) & 1)
    };
    let read = |at: usize, n: usize| -> Option<u32> {
        let mut v = 0u32;
        for k in 0..n {
            v = (v << 1) | u32::from(bit(at.checked_add(k)?)?);
        }
        Some(v)
    };
    let total = block.len().saturating_mul(8);
    for pad in 0..8usize {
        let Some(end) = total.checked_sub(pad) else {
            break;
        };
        if (end..total).any(|at| bit(at) != Some(0)) {
            // Alignment bits are zero; a one ends the search.
            break;
        }
        let Some(body_end) = end.checked_sub(3) else {
            continue;
        };
        if read(body_end, 3) != Some(0b111) {
            continue;
        }
        // A fill element of `count` bytes: 3 + 4 bits of header, 8 more
        // when `count` is 15 (`esc_count`).
        for count in 1..=MAX_FILL_BYTES {
            for header in [7usize, 15] {
                let Some(start) = body_end
                    .checked_sub(count * 8)
                    .and_then(|s| s.checked_sub(header))
                else {
                    continue;
                };
                if read(start, 3) != Some(0b110) {
                    continue;
                }
                let stated = match (header, read(start + 3, 4)) {
                    (7, Some(c)) if c < 15 => Some(c as usize),
                    (15, Some(15)) => read(start + 7, 8).map(|e| 15 + e as usize - 1),
                    _ => None,
                };
                if stated == Some(count) && matches!(read(start + header, 4), Some(0b1101 | 0b1110))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// The most bytes of a fill element: `count` 15 plus `esc_count` 255,
/// less one (14496-3 4.4.2.7).
const MAX_FILL_BYTES: usize = 269;

/// What the four-byte header of a Blu-ray LPCM audio frame states: the
/// bytes of samples after it (16 bits), the channel assignment and the
/// sampling frequency (4 bits each), the bits per sample (2 bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lpcm {
    pub sample_rate: u32,
    /// The channels the assignment names.
    pub channels: u32,
    /// The channels stored, an even number: a mono or three-channel
    /// frame carries an empty channel more.
    pub stored: u32,
    pub bits: u32,
    pub layout: &'static str,
}

/// Reads the header of a Blu-ray LPCM frame, for the channel
/// assignments, frequencies and sample sizes `ffprobe` was seen to read
/// on streams crafted with its own encoder; `None` for the others.
pub fn bluray_lpcm(header: &[u8]) -> Option<Lpcm> {
    let [_, _, b2, b3, ..] = *header else {
        return None;
    };
    let (channels, stored, layout) = match b2 >> 4 {
        1 => (1, 2, "mono"),
        3 => (2, 2, "stereo"),
        4 => (3, 4, "3.0"),
        6 => (4, 4, "4.0"),
        7 => (4, 4, "quad(side)"),
        9 => (6, 6, "5.1(side)"),
        10 => (7, 8, "7.0"),
        11 => (8, 8, "7.1"),
        _ => return None,
    };
    let sample_rate = match b2 & 0x0F {
        1 => 48_000,
        4 => 96_000,
        5 => 192_000,
        _ => return None,
    };
    let bits = match b3 >> 6 {
        1 => 16,
        3 => 24,
        _ => return None,
    };
    Some(Lpcm {
        sample_rate,
        channels,
        stored,
        bits,
        layout,
    })
}

impl Lpcm {
    /// The bit rate `ffprobe` reports: every stored channel's samples.
    pub fn bit_rate(self) -> u64 {
        u64::from(self.sample_rate) * u64::from(self.stored) * u64::from(self.bits)
    }

    /// The duration of a frame of `size` bytes, header included, in 90
    /// kHz ticks, rounded down.
    pub fn duration(self, size: usize) -> Option<i64> {
        let samples = u64::try_from(size.checked_sub(4)?).ok()?;
        let per_sample = u64::from(self.stored) * u64::from(self.bits / 8);
        let ticks = samples
            .checked_mul(90_000)?
            .checked_div(per_sample.checked_mul(u64::from(self.sample_rate))?)?;
        i64::try_from(ticks).ok()
    }
}

/// The channel layout `ffprobe` names for an AC-3 `acmod` and LFE flag
/// (A/52 table 5.8); `None` for the combinations whose name is not known.
pub fn ac3_layout(acmod: u8, lfe: bool) -> Option<&'static str> {
    Some(match (acmod, lfe) {
        (1, false) => "mono",
        (2, false) => "stereo",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb_codes() {
        // 1, 010, 011, 00100: 0, 1, 2, 3.
        let data = [0b1010_0110, 0b0100_0000];
        let mut bits = Bits::new(&data);
        assert_eq!(bits.ue(), Some(0));
        assert_eq!(bits.ue(), Some(1));
        assert_eq!(bits.ue(), Some(2));
        assert_eq!(bits.ue(), Some(3));
        assert_eq!(bits.ue(), None);
        let mut bits = Bits::new(&[0b0100_1100]);
        assert_eq!(bits.se(), Some(1));
        assert_eq!(bits.se(), Some(-1));
    }

    #[test]
    fn emulation_prevention_is_removed() {
        assert_eq!(rbsp(&[0, 0, 3, 1, 0, 0, 3]), vec![0, 0, 1, 0, 0]);
    }

    #[test]
    fn extradata_runs_from_the_first_parameter_set_to_the_next_unit() {
        let mut data = vec![0, 0, 0, 1, 0x09, 0xF0];
        data.extend([0, 0, 0, 1, 0x67, 1, 2, 3]);
        data.extend([0, 0, 0, 1, 0x68, 4]);
        data.extend([0, 0, 1, 0x06, 5]);
        data.extend([0, 0, 1, 0x65, 6]);
        // From the SPS's three-byte prefix to the SEI's.
        assert_eq!(annex_b_extradata(&data, false), Some(3 + 4 + 4 + 2));
        assert_eq!(annex_b_extradata(&[0, 0, 1, 0x65], false), None);
    }

    #[test]
    fn mpeg2_sequence_and_extension() {
        // 320x240, aspect 1, 25 fps, bit rate 0x3FFFF, VBV 3, then a
        // sequence extension (Main@Main, progressive, 4:2:0, not low
        // delay) and a GOP header.
        let mut data = vec![
            0, 0, 1, 0xB3, 0x14, 0x00, 0xF0, 0x13, 0xFF, 0xFF, 0xE0, 0x18,
        ];
        data.extend([0, 0, 1, 0xB5, 0x14, 0x8A, 0x00, 0x01, 0x00, 0x00]);
        data.extend([0, 0, 1, 0xB8, 0, 0, 0]);
        let seq = mpeg2(&data).unwrap();
        assert_eq!(seq.aspect_ratio_information, 1);
        assert_eq!(seq.frame_rate_code, 3);
        assert_eq!(seq.bit_rate, 0x3FFFF);
        assert_eq!(seq.vbv_buffer_size, 3);
        let ext = seq.extension.unwrap();
        assert!(ext.progressive_sequence);
        assert_eq!(ext.chroma_format, 1);
        assert!(!ext.low_delay);
        assert_eq!(mpeg2_extradata(&data), Some(22));
    }

    #[test]
    fn audio_bit_rates() {
        // MPEG-1 layer II, 384 kb/s, 44.1 kHz.
        assert_eq!(
            mpeg_audio_bit_rate(&[0xFF, 0xFD, 0xE0, 0xC4]),
            Some(384_000)
        );
        // AC-3, 48 kHz, frmsizecod 30 (448 kb/s), bsid 8, 3/2 with LFE.
        let ac3_header = [0x0B, 0x77, 0, 0, 0x1E, 0x40, 0xE0, 0x40];
        let parsed = ac3(&ac3_header).unwrap();
        assert_eq!(parsed.bit_rate, 448_000);
        assert_eq!(parsed.acmod, 7);
        assert!(!parsed.lfe);
        assert_eq!(ac3_layout(7, true), Some("5.1(side)"));
        assert_eq!(ac3_layout(1, false), Some("mono"));
    }

    #[test]
    fn sbr_is_found_at_the_end_of_a_block() {
        // A block of some element bits, then a fill element of two bytes
        // carrying EXT_SBR_DATA, then ID_END and alignment.
        // 1010 1010 | 110 0010 1 101 0000 0 000 111 0 0 ...
        let bits = "10101010".to_owned() + "110" + "0010" + "1101" + "0000" + "00000000" + "111";
        let mut padded = bits.clone();
        while padded.len() % 8 != 0 {
            padded.push('0');
        }
        let block: Vec<u8> = padded
            .as_bytes()
            .chunks(8)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 2).unwrap())
            .collect();
        assert!(aac_ends_with_sbr(&block));
        // The same with a fill extension (EXT_FILL_DATA) instead.
        let fill = bits.replacen("1101", "0001", 1);
        let mut padded = fill;
        while padded.len() % 8 != 0 {
            padded.push('0');
        }
        let block: Vec<u8> = padded
            .as_bytes()
            .chunks(8)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 2).unwrap())
            .collect();
        assert!(!aac_ends_with_sbr(&block));
        assert!(!aac_ends_with_sbr(&[]));
    }

    #[test]
    fn bluray_lpcm_headers() {
        // 960 bytes, stereo, 48 kHz, 16 bits: 240 samples, 450 ticks.
        let stereo = bluray_lpcm(&[0x03, 0xC0, 0x31, 0x40]).unwrap();
        assert_eq!((stereo.channels, stereo.layout), (2, "stereo"));
        assert_eq!(stereo.bit_rate(), 1_536_000);
        assert_eq!(stereo.duration(964), Some(450));
        // Mono is stored as two channels.
        let mono = bluray_lpcm(&[0x03, 0xC0, 0x11, 0x40]).unwrap();
        assert_eq!((mono.channels, mono.stored), (1, 2));
        assert_eq!(mono.duration(964), Some(450));
        // 2,160 bytes, stereo, 96 kHz, 24 bits: 337.5 ticks.
        let hd = bluray_lpcm(&[0x08, 0x70, 0x34, 0xC0]).unwrap();
        assert_eq!(hd.bit_rate(), 4_608_000);
        assert_eq!(hd.duration(2164), Some(337));
        assert_eq!(bluray_lpcm(&[0x03, 0xC0, 0x51, 0x40]), None);
    }
}
