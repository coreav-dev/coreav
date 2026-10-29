//! What `ffprobe` reports of a video stream that its decoder learns from
//! the parameter sets, read here from the codec private data or the first
//! frame: ITU-T H.264 7.3.2.1.1 and Annex E, ITU-T H.265 7.3.2.2, the AV1
//! codec configuration record (AV1-ISOBMFF 2.3.3). Every reader is bounded
//! by its input and returns `None` on anything it cannot read.

/// What a sequence parameter set (or AV1 sequence header) says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Params {
    /// `chroma_format_idc`: 0 monochrome, 1 4:2:0, 2 4:2:2, 3 4:4:4.
    pub chroma_format: u32,
    /// Luma bit depth.
    pub bit_depth: u32,
    /// `video_full_range_flag`, when the signal type is stored.
    pub full_range: Option<bool>,
    /// Colour primaries, transfer and matrix code points, when stored.
    pub colour: Option<(u32, u32, u32)>,
    /// `chroma_sample_loc_type_top_field`, when stored.
    pub chroma_loc: Option<u32>,
    /// `max_num_reorder_frames` (H.264) or `sps_max_num_reorder_pics` of
    /// the highest sub-layer (H.265), when stored.
    pub reorder: Option<u32>,
    /// `frame_mbs_only_flag` (H.264).
    pub frame_mbs_only: bool,
    /// The picture size less the conformance crop.
    pub size: Option<(u32, u32)>,
    /// `aspect_ratio_idc` resolved to a sample aspect ratio, when stored.
    pub sar: Option<(u32, u32)>,
    /// The VUI's `num_units_in_tick` and `time_scale` (H.264), when
    /// stored and not zero.
    pub timing: Option<(u32, u32)>,
    /// `profile_idc` (H.264) and `constraint_set3_flag`.
    pub profile_idc: u32,
    pub constraint_set3: bool,
    /// What an H.264 slice header needs: `separate_colour_plane_flag`,
    /// `log2_max_frame_num_minus4 + 4`, `pic_order_cnt_type` and
    /// `log2_max_pic_order_cnt_lsb_minus4 + 4`.
    pub slice: Option<SliceSyntax>,
}

/// The H.264 sequence parameter set fields a slice header's layout
/// depends on, up to its picture order count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SliceSyntax {
    pub separate_colour_plane: bool,
    pub frame_num_bits: u32,
    pub poc_type: u32,
    pub poc_lsb_bits: u32,
    pub frame_mbs_only: bool,
}

/// Reads the bits of an RBSP, most significant first.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = self.data.get(self.at / 8)?;
        let bit = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(u32::from(bit))
    }

    fn flag(&mut self) -> Option<bool> {
        Some(self.bit()? == 1)
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..n {
            value = (value << 1) | self.bit()?;
        }
        Some(value)
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        self.at = self.at.checked_add(n)?;
        (self.at <= self.data.len().checked_mul(8)?).then_some(())
    }

    /// An unsigned Exp-Golomb code (H.264 9.1).
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0u32;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let rest = self.bits(zeros)?;
        ((1u64 << zeros) - 1 + u64::from(rest)).try_into().ok()
    }

    /// A signed Exp-Golomb code.
    fn se(&mut self) -> Option<i64> {
        let v = i64::from(self.ue()?);
        Some(if v % 2 == 1 { (v + 1) / 2 } else { -(v / 2) })
    }
}

/// The RBSP of a NAL unit: its payload with emulation prevention bytes
/// removed (H.264 7.4.1).
pub fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// The NAL units of `avcC` or `hvcC` parameter set arrays of type `kind`
/// (`None` for every type), in order.
pub fn avcc_nals(avcc: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let Some(&count) = avcc.get(5) else {
        return out;
    };
    let mut at = 6usize;
    let take = |at: &mut usize| -> Option<&[u8]> {
        let len = usize::from(u16::from_be_bytes([*avcc.get(*at)?, *avcc.get(*at + 1)?]));
        let nal = avcc.get(*at + 2..*at + 2 + len)?;
        *at += 2 + len;
        Some(nal)
    };
    for _ in 0..(count & 0x1F) {
        match take(&mut at) {
            Some(nal) => out.push(nal),
            None => return out,
        }
    }
    let Some(&pps) = avcc.get(at) else {
        return out;
    };
    at += 1;
    for _ in 0..pps {
        match take(&mut at) {
            Some(nal) => out.push(nal),
            None => return out,
        }
    }
    out
}

/// The NAL units of an `hvcC` record, in order.
pub fn hvcc_nals(hvcc: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let Some(&arrays) = hvcc.get(22) else {
        return out;
    };
    let mut at = 23usize;
    let u16_at = |at: usize| -> Option<usize> {
        Some(usize::from(u16::from_be_bytes([
            *hvcc.get(at)?,
            *hvcc.get(at + 1)?,
        ])))
    };
    for _ in 0..arrays {
        let Some(n) = u16_at(at + 1) else {
            return out;
        };
        at += 3;
        for _ in 0..n {
            let Some(len) = u16_at(at) else {
                return out;
            };
            let Some(nal) = hvcc.get(at + 2..at + 2 + len) else {
                return out;
            };
            out.push(nal);
            at += 2 + len;
        }
    }
    out
}

/// The NAL units of a byte stream (Annex B): the bytes between start
/// codes.
pub fn annexb_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 3 <= data.len() {
        if data.get(i..i + 3) == Some(&[0, 0, 1]) {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (n, &start) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(data.len(), |next| next - 3);
        while end > start && data.get(end - 1) == Some(&0) {
            end -= 1;
        }
        if let Some(nal) = data.get(start..end) {
            out.push(nal);
        }
    }
    out
}

/// The `aspect_ratio_idc` table (H.264 Table E-1, H.265 Table E-1).
fn sar_table(idc: u32) -> Option<(u32, u32)> {
    const TABLE: [(u32, u32); 17] = [
        (0, 1),
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
    TABLE
        .get(usize::try_from(idc).ok()?)
        .copied()
        .filter(|s| s.0 != 0)
}

fn skip_scaling_list(bits: &mut Bits, size: usize) -> Option<()> {
    let (mut last, mut next) = (8i64, 8i64);
    for _ in 0..size {
        if next != 0 {
            let delta = bits.se()?;
            next = (last + delta + 256).rem_euclid(256);
        }
        if next != 0 {
            last = next;
        }
    }
    Some(())
}

fn skip_hrd(bits: &mut Bits) -> Option<()> {
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
    bits.skip(20)
}

/// Reads an H.264 sequence parameter set NAL unit (header byte included).
pub fn h264_sps(nal: &[u8]) -> Option<Params> {
    if nal.first()? & 0x1F != 7 {
        return None;
    }
    let data = rbsp(nal.get(1..)?);
    let mut bits = Bits::new(&data);
    let profile_idc = bits.bits(8)?;
    let constraints = bits.bits(8)?;
    bits.skip(8)?;
    bits.ue()?;
    let mut separate_colour_plane = false;
    let mut p = Params {
        chroma_format: 1,
        bit_depth: 8,
        profile_idc,
        constraint_set3: constraints & 0x10 != 0,
        ..Params::default()
    };
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        p.chroma_format = bits.ue()?;
        if p.chroma_format > 3 {
            return None;
        }
        if p.chroma_format == 3 {
            separate_colour_plane = bits.flag()?;
        }
        p.bit_depth = bits.ue()?.checked_add(8)?;
        bits.ue()?;
        bits.skip(1)?;
        if bits.flag()? {
            let lists = if p.chroma_format == 3 { 12 } else { 8 };
            for i in 0..lists {
                if bits.flag()? {
                    skip_scaling_list(&mut bits, if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    let frame_num_bits = bits.ue()?.checked_add(4)?;
    let poc_type = bits.ue()?;
    let mut poc_lsb_bits = 0;
    match poc_type {
        0 => {
            poc_lsb_bits = bits.ue()?.checked_add(4)?;
        }
        1 => {
            bits.skip(1)?;
            bits.se()?;
            bits.se()?;
            let n = bits.ue()?;
            if n > 255 {
                return None;
            }
            for _ in 0..n {
                bits.se()?;
            }
        }
        _ => {}
    }
    bits.ue()?;
    bits.skip(1)?;
    let width_mbs = bits.ue()?.checked_add(1)?;
    let height_units = bits.ue()?.checked_add(1)?;
    p.frame_mbs_only = bits.flag()?;
    if !p.frame_mbs_only {
        bits.skip(1)?;
    }
    if frame_num_bits <= 16 && poc_lsb_bits <= 16 {
        p.slice = Some(SliceSyntax {
            separate_colour_plane,
            frame_num_bits,
            poc_type,
            poc_lsb_bits,
            frame_mbs_only: p.frame_mbs_only,
        });
    }
    bits.skip(1)?;
    let height_mbs = height_units.checked_mul(if p.frame_mbs_only { 1 } else { 2 })?;
    let mut width = width_mbs.checked_mul(16)?;
    let mut height = height_mbs.checked_mul(16)?;
    if bits.flag()? {
        let (l, r, t, b) = (bits.ue()?, bits.ue()?, bits.ue()?, bits.ue()?);
        let (cx, cy) = match p.chroma_format {
            1 => (2, 2),
            2 => (2, 1),
            _ => (1, 1),
        };
        let cy = cy * if p.frame_mbs_only { 1 } else { 2 };
        width = width.checked_sub(l.checked_add(r)?.checked_mul(cx)?)?;
        height = height.checked_sub(t.checked_add(b)?.checked_mul(cy)?)?;
    }
    p.size = Some((width, height));
    if !bits.flag()? {
        return Some(p);
    }
    // vui_parameters (E.1.1); a VUI cut short keeps what was read.
    let _ = h264_vui(&mut bits, &mut p);
    Some(p)
}

fn h264_vui(bits: &mut Bits, p: &mut Params) -> Option<()> {
    if bits.flag()? {
        let idc = bits.bits(8)?;
        if idc == 255 {
            let (w, h) = (bits.bits(16)?, bits.bits(16)?);
            p.sar = Some((w, h)).filter(|s| s.0 != 0 && s.1 != 0);
        } else {
            p.sar = sar_table(idc);
        }
    }
    if bits.flag()? {
        bits.skip(1)?;
    }
    if bits.flag()? {
        bits.skip(3)?;
        p.full_range = Some(bits.flag()?);
        if bits.flag()? {
            p.colour = Some((bits.bits(8)?, bits.bits(8)?, bits.bits(8)?));
        }
    }
    if bits.flag()? {
        p.chroma_loc = Some(bits.ue()?);
        bits.ue()?;
    }
    if bits.flag()? {
        let (units, scale) = (bits.bits(32)?, bits.bits(32)?);
        p.timing = Some((units, scale)).filter(|t| t.0 > 0 && t.1 > 0);
        bits.skip(1)?;
    }
    let nal_hrd = bits.flag()?;
    if nal_hrd {
        skip_hrd(bits)?;
    }
    let vcl_hrd = bits.flag()?;
    if vcl_hrd {
        skip_hrd(bits)?;
    }
    if nal_hrd || vcl_hrd {
        bits.skip(1)?;
    }
    bits.skip(1)?;
    if bits.flag()? {
        bits.skip(1)?;
        for _ in 0..4 {
            bits.ue()?;
        }
        // At most 16 frames wait (H.264 A.3.1, H.265 A.4.2).
        p.reorder = Some(bits.ue()?).filter(|n| *n <= 16);
        bits.ue()?;
    }
    Some(())
}

/// The reorder depth an H.264 decoder finds in `frames` (each the start
/// of a frame of length-prefixed NAL units, `length_size` bytes each, in
/// decode order): for each picture, how many pictures decoded before it
/// since the last IDR picture come after it in output order (picture
/// order count, H.264 8.2.1), the largest. Only `pic_order_cnt_type` 0
/// is read (type 2 never reorders); `None` for anything else or a frame
/// whose first slice header cannot be read.
pub fn h264_poc_depth<'a>(
    syntax: &SliceSyntax,
    length_size: usize,
    frames: impl IntoIterator<Item = &'a [u8]>,
) -> Option<u32> {
    match syntax.poc_type {
        2 => return Some(0),
        0 => {}
        _ => return None,
    }
    let max_lsb = 1i64 << syntax.poc_lsb_bits;
    let (mut prev_msb, mut prev_lsb) = (0i64, 0i64);
    let mut period: Vec<i64> = Vec::new();
    let mut depth = 0u32;
    for frame in frames {
        let (nal_type, reference, lsb) = first_slice_poc(syntax, length_size, frame)?;
        let lsb = i64::from(lsb);
        if nal_type == 5 {
            prev_msb = 0;
            prev_lsb = 0;
            period.clear();
        }
        let msb = if lsb < prev_lsb && prev_lsb - lsb >= max_lsb / 2 {
            prev_msb + max_lsb
        } else if lsb > prev_lsb && lsb - prev_lsb > max_lsb / 2 {
            prev_msb - max_lsb
        } else {
            prev_msb
        };
        let poc = msb + lsb;
        if reference {
            prev_msb = msb;
            prev_lsb = lsb;
        }
        let later = period.iter().filter(|p| **p > poc).count();
        depth = depth.max(u32::try_from(later).ok()?);
        period.push(poc);
        if period.len() > 64 {
            period.remove(0);
        }
    }
    Some(depth.min(16))
}

/// The NAL unit type, `nal_ref_idc != 0` and `pic_order_cnt_lsb` of the
/// first slice of a frame (H.264 7.3.3).
fn first_slice_poc(
    syntax: &SliceSyntax,
    length_size: usize,
    frame: &[u8],
) -> Option<(u8, bool, u32)> {
    let mut at = 0usize;
    loop {
        let mut len = 0usize;
        for i in 0..length_size {
            len = (len << 8) | usize::from(*frame.get(at + i)?);
        }
        at += length_size;
        let header = *frame.get(at)?;
        let nal_type = header & 0x1F;
        if matches!(nal_type, 1 | 5) {
            let end = frame.len().min(at.checked_add(len)?);
            let data = rbsp(frame.get(at + 1..end)?);
            let mut bits = Bits::new(&data);
            bits.ue()?;
            bits.ue()?;
            bits.ue()?;
            if syntax.separate_colour_plane {
                bits.skip(2)?;
            }
            bits.skip(usize::try_from(syntax.frame_num_bits).ok()?)?;
            if !syntax.frame_mbs_only && bits.flag()? {
                // A field picture: CoreAV does not pair fields.
                return None;
            }
            if nal_type == 5 {
                bits.ue()?;
            }
            let lsb = bits.bits(syntax.poc_lsb_bits)?;
            return Some((nal_type, header & 0x60 != 0, lsb));
        }
        at = at.checked_add(len)?;
    }
}

/// The first H.264 SPS among `nals`.
pub fn first_h264_sps<'a>(nals: impl IntoIterator<Item = &'a [u8]>) -> Option<Params> {
    nals.into_iter()
        .filter(|n| n.first().is_some_and(|b| b & 0x1F == 7))
        .find_map(h264_sps)
}

fn skip_profile_tier_level(bits: &mut Bits, max_sub_layers_minus1: u32) -> Option<()> {
    bits.skip(88)?;
    bits.skip(8)?;
    let mut present = Vec::new();
    for _ in 0..max_sub_layers_minus1 {
        present.push((bits.flag()?, bits.flag()?));
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            bits.skip(2)?;
        }
    }
    for (profile, level) in present {
        if profile {
            bits.skip(88)?;
        }
        if level {
            bits.skip(8)?;
        }
    }
    Some(())
}

/// Reads an H.265 sequence parameter set NAL unit (two header bytes
/// included), up to the reorder depth.
pub fn hevc_sps(nal: &[u8]) -> Option<Params> {
    if (nal.first()? >> 1) & 0x3F != 33 {
        return None;
    }
    let data = rbsp(nal.get(2..)?);
    let mut bits = Bits::new(&data);
    bits.skip(4)?;
    let max_sub_layers_minus1 = bits.bits(3)?;
    if max_sub_layers_minus1 > 6 {
        return None;
    }
    bits.skip(1)?;
    skip_profile_tier_level(&mut bits, max_sub_layers_minus1)?;
    bits.ue()?;
    let mut p = Params {
        chroma_format: bits.ue()?,
        ..Params::default()
    };
    if p.chroma_format > 3 {
        return None;
    }
    if p.chroma_format == 3 {
        bits.skip(1)?;
    }
    let mut width = bits.ue()?;
    let mut height = bits.ue()?;
    if bits.flag()? {
        let (l, r, t, b) = (bits.ue()?, bits.ue()?, bits.ue()?, bits.ue()?);
        let (cx, cy) = match p.chroma_format {
            1 => (2, 2),
            2 => (2, 1),
            _ => (1, 1),
        };
        width = width.checked_sub(l.checked_add(r)?.checked_mul(cx)?)?;
        height = height.checked_sub(t.checked_add(b)?.checked_mul(cy)?)?;
    }
    p.size = Some((width, height));
    p.bit_depth = bits.ue()?.checked_add(8)?;
    bits.ue()?;
    bits.ue()?;
    let ordering_all = bits.flag()?;
    let first = if ordering_all {
        0
    } else {
        max_sub_layers_minus1
    };
    for _ in first..=max_sub_layers_minus1 {
        bits.ue()?;
        // At most 16 frames wait (H.264 A.3.1, H.265 A.4.2).
        p.reorder = Some(bits.ue()?).filter(|n| *n <= 16);
        bits.ue()?;
    }
    Some(p)
}

/// The first H.265 SPS among `nals`.
pub fn first_hevc_sps<'a>(nals: impl IntoIterator<Item = &'a [u8]>) -> Option<Params> {
    nals.into_iter()
        .filter(|n| n.first().is_some_and(|b| (b >> 1) & 0x3F == 33))
        .find_map(hevc_sps)
}

/// What an `av1C` record says: bit depth, monochrome and chroma
/// subsampling (`chroma_format` 0, 1 4:2:0, 2 4:2:2, 3 4:4:4).
pub fn av1c(record: &[u8]) -> Option<Params> {
    let byte = *record.get(2)?;
    let high = byte & 0x40 != 0;
    let twelve = byte & 0x20 != 0;
    let mono = byte & 0x10 != 0;
    let (sx, sy) = (byte & 0x08 != 0, byte & 0x04 != 0);
    let bit_depth = match (high, twelve) {
        (false, _) => 8,
        (true, false) => 10,
        (true, true) => 12,
    };
    let chroma_format = match (mono, sx, sy) {
        (true, ..) => 0,
        (false, true, true) => 1,
        (false, true, false) => 2,
        _ => 3,
    };
    Some(Params {
        chroma_format,
        bit_depth,
        ..Params::default()
    })
}

/// `film_grain_params_present_flag` of the sequence header OBU among an
/// `av1C` record's configuration OBUs (AV1 5.5); `None` when there is
/// none or it cannot be read.
pub fn av1_film_grain(record: &[u8]) -> Option<bool> {
    let mut at = 4usize;
    loop {
        let header = *record.get(at)?;
        let obu_type = (header >> 3) & 0x0F;
        let extension = header & 0x04 != 0;
        let has_size = header & 0x02 != 0;
        at += 1 + usize::from(extension);
        let size = if has_size {
            let mut value = 0usize;
            let mut i = 0u32;
            loop {
                let b = *record.get(at)?;
                at += 1;
                value |= usize::from(b & 0x7F).checked_shl(7 * i)?;
                i += 1;
                if b & 0x80 == 0 {
                    break;
                }
                if i >= 8 {
                    return None;
                }
            }
            value
        } else {
            record.len().checked_sub(at)?
        };
        let payload = record.get(at..at.checked_add(size)?)?;
        if obu_type == 1 {
            return av1_sequence_film_grain(payload);
        }
        at += size;
    }
}

/// Reads an AV1 sequence header OBU's payload up to
/// `film_grain_params_present_flag` (AV1 5.5.1 to 5.5.2).
fn av1_sequence_film_grain(data: &[u8]) -> Option<bool> {
    let mut b = Bits::new(data);
    let profile = b.bits(3)?;
    b.skip(1)?;
    let reduced = b.flag()?;
    let mut decoder_model = false;
    let mut delay_bits = 0usize;
    if reduced {
        b.skip(5)?;
    } else {
        if b.flag()? {
            b.skip(64)?;
            if b.flag()? {
                // num_ticks_per_picture_minus_1, uvlc.
                let mut zeros = 0u32;
                while !b.flag()? {
                    zeros += 1;
                    if zeros >= 32 {
                        return None;
                    }
                }
                b.skip(usize::try_from(zeros).ok()?)?;
            }
            decoder_model = b.flag()?;
            if decoder_model {
                delay_bits = usize::try_from(b.bits(5)?).ok()? + 1;
                b.skip(32 + 5 + 5)?;
            }
        }
        let initial_display_delay = b.flag()?;
        let points = b.bits(5)? + 1;
        for _ in 0..points {
            b.skip(12)?;
            if b.bits(5)? > 7 {
                b.skip(1)?;
            }
            if decoder_model && b.flag()? {
                b.skip(delay_bits.checked_mul(2)? + 1)?;
            }
            if initial_display_delay && b.flag()? {
                b.skip(4)?;
            }
        }
    }
    let width_bits = usize::try_from(b.bits(4)?).ok()? + 1;
    let height_bits = usize::try_from(b.bits(4)?).ok()? + 1;
    b.skip(width_bits + height_bits)?;
    if !reduced && b.flag()? {
        b.skip(7)?;
    }
    b.skip(3)?;
    if !reduced {
        b.skip(4)?;
        let order_hint = b.flag()?;
        if order_hint {
            b.skip(2)?;
        }
        let force_screen_content = if b.flag()? { 2 } else { b.bits(1)? };
        if force_screen_content > 0 && !b.flag()? {
            b.skip(1)?;
        }
        if order_hint {
            b.skip(3)?;
        }
    }
    b.skip(3)?;
    // color_config (5.5.2).
    let high = b.flag()?;
    let twelve = profile == 2 && high && b.flag()?;
    let mono = profile != 1 && b.flag()?;
    let (mut primaries, mut transfer, mut matrix) = (2, 2, 2);
    if b.flag()? {
        primaries = b.bits(8)?;
        transfer = b.bits(8)?;
        matrix = b.bits(8)?;
    }
    if mono {
        b.skip(1)?;
    } else {
        if primaries == 1 && transfer == 13 && matrix == 0 {
            // sRGB: full range, 4:4:4, nothing stored.
        } else {
            b.skip(1)?;
            let (sx, sy) = match profile {
                0 => (true, true),
                1 => (false, false),
                _ if twelve => {
                    let sx = b.flag()?;
                    (sx, sx && b.flag()?)
                }
                _ => (true, false),
            };
            if sx && sy {
                b.skip(2)?;
            }
        }
        b.skip(1)?;
    }
    b.flag()
}

/// What an MPEG-1 or MPEG-2 video sequence header and its extension say
/// (ISO/IEC 13818-2 6.2.2.1 and 6.2.2.3), and the first picture's coding
/// extension (6.2.3.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mpeg2 {
    /// `bit_rate` in bits per second (400 bit/s units), extension included.
    pub bit_rate: u64,
    /// `vbv_buffer_size` in bytes (16 kbit units), extension included.
    pub buffer_size: u64,
    /// `profile_and_level_indication`, from the sequence extension.
    pub profile_and_level: Option<u8>,
    /// `chroma_format` (1 4:2:0, 2 4:2:2, 3 4:4:4).
    pub chroma_format: u32,
    /// `low_delay`.
    pub low_delay: bool,
    /// The first picture: `progressive_frame` and `top_field_first`.
    pub picture: Option<(bool, bool)>,
    /// `bit_rate_value` is all ones (a variable bit rate, ISO/IEC 11172-2
    /// 2.4.3.2).
    pub variable: bool,
    /// Bytes from the sequence header to the next start code that is not
    /// one of its extensions: what a decoder takes as its configuration.
    pub header_len: usize,
}

/// Reads the sequence header and the extensions that follow start codes
/// in `data` (a `CodecPrivate` or the first frame).
pub fn mpeg2(data: &[u8]) -> Option<Mpeg2> {
    let mut out: Option<Mpeg2> = None;
    let mut i = 0usize;
    let mut start = 0usize;
    while i + 4 <= data.len() {
        if data.get(i..i + 3) != Some(&[0, 0, 1]) {
            i += 1;
            continue;
        }
        let code = *data.get(i + 3)?;
        let body = data.get(i + 4..).unwrap_or_default();
        match code {
            0xB3 => {
                let b = body.get(..8)?;
                let byte = |k: usize| u64::from(b.get(k).copied().unwrap_or(0));
                let rate = byte(4) << 10 | byte(5) << 2 | byte(6) >> 6;
                let vbv = (byte(6) & 0x1F) << 5 | byte(7) >> 3;
                if out.is_some() {
                    i += 4;
                    continue;
                }
                out = Some(Mpeg2 {
                    bit_rate: rate,
                    buffer_size: vbv,
                    chroma_format: 1,
                    low_delay: false,
                    variable: rate == 0x3FFFF,
                    header_len: data.len() - i,
                    ..Mpeg2::default()
                });
                start = i;
            }
            0xB8 | 0x00 => {
                if let Some(seq) = out.as_mut() {
                    if seq.header_len == data.len() - start {
                        seq.header_len = i - start;
                    }
                }
            }
            0xB5 => {
                let id = body.first()? >> 4;
                if let (1, Some(seq)) = (id, out.as_mut()) {
                    let b = body.get(..6)?;
                    let byte = |k: usize| u64::from(b.get(k).copied().unwrap_or(0));
                    seq.profile_and_level = Some(((byte(0) & 0x0F) << 4 | byte(1) >> 4) as u8);
                    seq.chroma_format = u32::try_from((byte(1) >> 1) & 3).unwrap_or(1);
                    let rate_ext = (byte(2) & 0x1F) << 7 | byte(3) >> 1;
                    seq.bit_rate |= rate_ext << 18;
                    seq.buffer_size |= byte(4) << 10;
                    seq.low_delay = byte(5) & 0x80 != 0;
                }
                if let (8, Some(seq)) = (id, out.as_mut()) {
                    if seq.picture.is_none() {
                        let b3 = *body.get(3)?;
                        let b4 = *body.get(4)?;
                        let top_field_first = b3 & 0x80 != 0;
                        let progressive = b4 & 0x80 != 0;
                        seq.picture = Some((progressive, top_field_first));
                    }
                }
            }
            _ => {}
        }
        i += 4;
    }
    let mut seq = out?;
    seq.bit_rate = seq.bit_rate.saturating_mul(400);
    seq.buffer_size = seq.buffer_size.saturating_mul(16 * 1024);
    Some(seq)
}

/// What an MPEG-4 Part 2 visual object sequence and video object layer
/// header say (ISO/IEC 14496-2 6.2.2 and 6.2.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mpeg4 {
    /// `profile_and_level_indication` of the visual object sequence.
    pub profile_and_level: Option<u8>,
    /// `low_delay`, when the layer has `vol_control_parameters`.
    pub low_delay: Option<bool>,
    /// `quarter_sample`, when the layer header was read that far.
    pub quarter_sample: Option<bool>,
    /// A DivX user data string ending in `p`: B-frames packed with their
    /// reference.
    pub divx_packed: bool,
    /// The `video_signal_type` of the visual object header (6.2.2): the
    /// range, and the colour description when it has one.
    pub color: Option<coreav_isobmff::Color>,
}

/// Reads the VOS and VOL headers that follow start codes in `data`.
pub fn mpeg4(data: &[u8]) -> Option<Mpeg4> {
    let mut out = Mpeg4::default();
    let mut found = false;
    let mut i = 0usize;
    while i + 4 <= data.len() {
        if data.get(i..i + 3) != Some(&[0, 0, 1]) {
            i += 1;
            continue;
        }
        let code = *data.get(i + 3)?;
        let body = data.get(i + 4..).unwrap_or_default();
        match code {
            0xB0 => out.profile_and_level = body.first().copied(),
            0xB5 => out.color = mpeg4_signal(body),
            0xB2 => {
                let end = body
                    .windows(3)
                    .position(|w| w == [0, 0, 1])
                    .unwrap_or(body.len());
                let text = body.get(..end).unwrap_or_default();
                if text.starts_with(b"DivX") && text.last() == Some(&b'p') {
                    out.divx_packed = true;
                }
            }
            0x20..=0x2F if !found => {
                found = true;
                let (low_delay, quarter) = mpeg4_vol(body);
                out.low_delay = low_delay;
                out.quarter_sample = quarter;
            }
            0x00..=0x1F | 0xB1 | 0xB3 | 0xB5..=0xB6 => {}
            _ => {}
        }
        if code == 0xB6 {
            break;
        }
        i += 4;
    }
    found.then_some(out)
}

/// The `video_signal_type` of a visual object header of the video type
/// (ISO/IEC 14496-2 6.2.2).
fn mpeg4_signal(body: &[u8]) -> Option<coreav_isobmff::Color> {
    let mut bits = Bits::new(body);
    if bits.flag()? {
        bits.skip(7)?;
    }
    if bits.bits(4)? != 1 || !bits.flag()? {
        return None;
    }
    bits.skip(3)?;
    let full_range = bits.flag()?;
    let mut color = coreav_isobmff::Color {
        full_range: Some(full_range),
        ..coreav_isobmff::Color::default()
    };
    if bits.flag()? {
        color.primaries = Some(u16::try_from(bits.bits(8)?).ok()?);
        color.transfer = Some(u16::try_from(bits.bits(8)?).ok()?);
        color.matrix = Some(u16::try_from(bits.bits(8)?).ok()?);
    }
    Some(color)
}

/// `low_delay` and `quarter_sample` of a video object layer header.
fn mpeg4_vol(body: &[u8]) -> (Option<bool>, Option<bool>) {
    let mut low_delay = None;
    let mut read = || -> Option<bool> {
        let mut bits = Bits::new(body);
        bits.skip(1)?;
        bits.skip(8)?;
        let mut verid = 1;
        if bits.flag()? {
            verid = bits.bits(4)?;
            bits.skip(3)?;
        }
        if bits.bits(4)? == 15 {
            bits.skip(16)?;
        }
        if bits.flag()? {
            bits.skip(2)?;
            low_delay = Some(bits.flag()?);
            if bits.flag()? {
                bits.skip(15 + 1 + 15 + 1 + 15 + 1 + 3 + 11 + 1 + 15 + 1)?;
            }
        }
        let shape = bits.bits(2)?;
        if shape == 3 && verid != 1 {
            bits.skip(4)?;
        }
        bits.skip(1)?;
        let resolution = bits.bits(16)?;
        bits.skip(1)?;
        if bits.flag()? {
            let width = (u32::BITS - resolution.saturating_sub(1).leading_zeros()).max(1);
            bits.skip(usize::try_from(width).ok()?)?;
        }
        if shape != 0 {
            return None;
        }
        bits.skip(1 + 13 + 1 + 13 + 1)?;
        bits.skip(1)?;
        bits.skip(1)?;
        let sprite = if verid == 1 {
            bits.bits(1)?
        } else {
            bits.bits(2)?
        };
        // Static sprites (1) store their size and position; global motion
        // compensation (2) only the warping fields (ISO/IEC 14496-2
        // 6.2.3).
        match sprite {
            0 => {}
            1 => bits.skip(4 * 14 + 6 + 2 + 1 + 1)?,
            2 => bits.skip(6 + 2 + 1)?,
            _ => return None,
        }
        if bits.flag()? {
            bits.skip(8)?;
        }
        if bits.flag()? {
            for _ in 0..2 {
                if bits.flag()? {
                    for _ in 0..64 {
                        if bits.bits(8)? == 0 {
                            break;
                        }
                    }
                }
            }
        }
        if verid == 1 {
            return Some(false);
        }
        bits.flag()
    };
    let quarter = read();
    (low_delay, quarter)
}

/// The profile name of an MPEG-4 Part 2 `profile_and_level_indication`,
/// and its level, for the profiles `ffprobe` names that CoreAV knows.
pub fn mpeg4_profile(pli: u8) -> Option<(&'static str, u8)> {
    Some(match pli {
        0x01..=0x06 => ("Simple Profile", pli),
        0x08 => ("Simple Profile", 0),
        0xF0..=0xF5 => ("Advanced Simple Profile", pli & 0x0F),
        _ => return None,
    })
}

/// The colour configuration of a VP9 key frame's uncompressed header (VP9
/// bitstream specification 6.2 and 7.2): `color_space` and `color_range`.
pub fn vp9_colour(frame: &[u8]) -> Option<(u8, bool)> {
    let mut bits = Bits::new(frame);
    if bits.bits(2)? != 2 {
        return None;
    }
    let low = bits.bits(1)?;
    let high = bits.bits(1)?;
    let profile = high << 1 | low;
    if profile == 3 {
        bits.skip(1)?;
    }
    if bits.flag()? {
        return None;
    }
    // frame_type 0 is a key frame.
    if bits.flag()? {
        return None;
    }
    bits.skip(2)?;
    if bits.bits(24)? != 0x49_8342 {
        return None;
    }
    if profile >= 2 {
        bits.skip(1)?;
    }
    let space = u8::try_from(bits.bits(3)?).ok()?;
    let full = if space == 7 { true } else { bits.flag()? };
    Some((space, full))
}

/// The pixel format name `ffprobe` prints for a YUV layout.
pub fn pix_fmt(chroma_format: u32, bit_depth: u32, jpeg: bool) -> Option<&'static str> {
    Some(match (chroma_format, bit_depth, jpeg) {
        (0, 8, _) => "gray",
        (0, 10, _) => "gray10le",
        (0, 12, _) => "gray12le",
        (1, 8, false) => "yuv420p",
        (1, 8, true) => "yuvj420p",
        (2, 8, false) => "yuv422p",
        (2, 8, true) => "yuvj422p",
        (3, 8, false) => "yuv444p",
        (3, 8, true) => "yuvj444p",
        (1, 10, _) => "yuv420p10le",
        (2, 10, _) => "yuv422p10le",
        (3, 10, _) => "yuv444p10le",
        (1, 12, _) => "yuv420p12le",
        (2, 12, _) => "yuv422p12le",
        (3, 12, _) => "yuv444p12le",
        _ => return None,
    })
}

/// The chroma location name for an H.273 `chroma_sample_loc_type`.
pub fn chroma_location(loc: u32) -> Option<&'static str> {
    Some(match loc {
        0 => "left",
        1 => "center",
        2 => "topleft",
        3 => "top",
        4 => "bottomleft",
        5 => "bottom",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emulation_prevention_is_removed() {
        assert_eq!(rbsp(&[0, 0, 3, 1, 0, 0, 3]), vec![0, 0, 1, 0, 0]);
    }

    #[test]
    fn an_x264_sps() {
        // High profile, level 1.3, 320x240, VUI with bitstream restriction.
        let avcc = [
            0x01, 0x64, 0x00, 0x0d, 0xff, 0xe1, 0x00, 0x19, 0x67, 0x64, 0x00, 0x0d, 0xac, 0xd9,
            0x41, 0x41, 0xfb, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03, 0x03,
            0x20, 0xf1, 0x42, 0x99, 0x60, 0x01, 0x00, 0x06, 0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0,
            0xfd, 0xf8, 0xf8, 0x00,
        ];
        let nals = avcc_nals(&avcc);
        assert_eq!(nals.len(), 2);
        let p = first_h264_sps(nals).unwrap();
        assert_eq!(p.size, Some((320, 240)));
        assert_eq!((p.chroma_format, p.bit_depth), (1, 8));
        assert_eq!(p.reorder, Some(2));
        assert_eq!(p.sar, Some((1, 1)));
    }

    #[test]
    fn start_codes_split_a_byte_stream() {
        let data = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0];
        assert_eq!(annexb_nals(&data), vec![&[0x67, 1, 2][..], &[0x68, 3][..]]);
    }

    #[test]
    fn pixel_formats() {
        assert_eq!(pix_fmt(1, 8, true), Some("yuvj420p"));
        assert_eq!(pix_fmt(2, 10, false), Some("yuv422p10le"));
        assert_eq!(pix_fmt(1, 9, false), None);
        assert_eq!(
            av1c(&[0x81, 0x08, 0x0c, 0]).map(|p| (p.chroma_format, p.bit_depth)),
            Some((1, 8))
        );
    }

    #[test]
    fn mpeg_video_headers() {
        // MPEG-2: sequence header, sequence extension (Main, 4:2:0, not
        // low delay), then a GOP.
        let seq = [
            0x00, 0x00, 0x01, 0xB3, 0x0B, 0x00, 0x90, 0x13, 0xFF, 0xFF, 0xE0, 0x18, 0x00, 0x00,
            0x01, 0xB5, 0x14, 0x8A, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0xB8, 0x00,
        ];
        let m = mpeg2(&seq).unwrap();
        assert_eq!(m.profile_and_level, Some(0x48));
        assert_eq!((m.chroma_format, m.low_delay, m.variable), (1, false, true));
        assert_eq!(m.buffer_size, 49_152);
        assert_eq!(m.header_len, 22);
        // MPEG-4 Part 2: Advanced Simple Profile level 1, then a layer.
        let vol = [
            0x00, 0x00, 0x01, 0xB0, 0xF1, 0x00, 0x00, 0x01, 0xB5, 0xA9, 0x13, 0x00, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x01, 0x20, 0x08, 0xD4, 0x8D, 0x08, 0x00, 0xCD, 0x05, 0x84, 0x12,
            0x14, 0x10, 0x3F,
        ];
        let m = mpeg4(&vol).unwrap();
        assert_eq!(
            m.profile_and_level.and_then(mpeg4_profile),
            Some(("Advanced Simple Profile", 1))
        );
        assert_eq!(m.low_delay, Some(false));
        assert!(!m.divx_packed);
    }
}
