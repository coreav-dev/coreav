//! Picture properties `ffprobe` prints for a video stream that the codec
//! configuration determines: the pixel format, the reordering depth, the
//! chroma location, the field order, the coded size and the bit depth.
//! Read from the stored configuration records (ITU-T H.264 7.3.2.1.1 and
//! E.1.1, ITU-T H.265 7.3.2.2, the AV1 and VP codec ISOBMFF bindings,
//! SMPTE RDD 36 for ProRes frame headers); untrusted bytes, so every read
//! is bounded and a record that does not parse gives nothing.

/// What the configuration says, as `ffprobe` names it. `None` is unknown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VideoInfo {
    pub pix_fmt: Option<String>,
    pub has_b_frames: Option<u32>,
    pub chroma_location: Option<&'static str>,
    pub field_order: Option<&'static str>,
    pub bits_per_raw_sample: Option<u32>,
    pub coded_size: Option<(u32, u32)>,
    /// An H.264 picture's cropped size, when the cropping keeps its top
    /// left corner: the size a sample entry may narrow (`iso.rs`).
    pub h264_corner: Option<(u32, u32)>,
    /// The frame rate the stream's own clock states, as a fraction
    /// (ITU-T H.264 E.2.1, where a frame lasts two ticks; H.265 E.3.1,
    /// where a picture lasts one).
    pub frame_rate: Option<(u64, u64)>,
    /// The sample aspect ratio the stream signals (ITU-T H.264 table E-1).
    pub sar: Option<(u32, u32)>,
    /// The profile and level named from the stream's own header, for a
    /// codec the container gives none of (MPEG-4 Part 2).
    pub profile: Option<(String, i64)>,
    /// MPEG-4 Part 2's `quarter_sample` (unknown when the header stops
    /// before it) and whether DivX packs B-frames with their reference.
    pub mpeg4_flags: Option<(Option<bool>, bool)>,
}

/// `aspect_ratio_idc` 1 to 16 (ITU-T H.264 table E-1, H.265 table E.1).
const SAR_TABLE: [(u32, u32); 16] = [
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

/// A bit reader over an RBSP (emulation prevention removed).
pub(super) struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0 }
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - (self.pos % 8))) & 1;
        self.pos += 1;
        Some(u32::from(bit))
    }

    pub(super) fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }

    fn flag(&mut self) -> Option<bool> {
        self.bit().map(|b| b == 1)
    }

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

    fn se(&mut self) -> Option<i64> {
        let k = i64::from(self.ue()?);
        Some(if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) })
    }
}

/// The RBSP of a NAL unit: its header bytes skipped and emulation
/// prevention bytes removed.
fn rbsp(nal: &[u8], header: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal.iter().skip(header) {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

fn u16_at(data: &[u8], at: usize) -> Option<usize> {
    let hi = *data.get(at)?;
    let lo = *data.get(at + 1)?;
    Some(usize::from(hi) << 8 | usize::from(lo))
}

/// The pixel format name of a YUV layout.
fn yuv(chroma_format: u32, bits: u32, full_range: bool) -> Option<String> {
    let base = match chroma_format {
        0 => "gray",
        1 => "yuv420p",
        2 => "yuv422p",
        3 => "yuv444p",
        _ => return None,
    };
    Some(match (bits, chroma_format) {
        (8, 0) => "gray".to_owned(),
        (8, _) if full_range => base.replacen("yuv", "yuvj", 1),
        (8, _) => base.to_owned(),
        (b @ (9 | 10 | 12 | 14 | 16), _) => format!("{base}{b}le"),
        _ => return None,
    })
}

const CHROMA_LOCATIONS: [&str; 6] = ["left", "center", "topleft", "top", "bottomleft", "bottom"];

/// An `avcC` record's first sequence parameter set.
pub fn avc(avcc: &[u8], full_range: bool) -> Option<VideoInfo> {
    let count = usize::from(*avcc.get(5)? & 0x1f);
    if count == 0 {
        return None;
    }
    let len = u16_at(avcc, 6)?;
    let nal = avcc.get(8..8 + len)?;
    avc_sps(nal, full_range)
}

/// An H.264 sequence parameter set NAL unit, header included.
pub fn avc_sps(nal: &[u8], full_range: bool) -> Option<VideoInfo> {
    let data = rbsp(nal, 1);
    let mut b = Bits::new(&data);
    let profile_idc = b.bits(8)?;
    b.bits(16)?;
    b.ue()?;
    let (mut chroma_format, mut bit_depth) = (1, 8);
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format = b.ue()?;
        if chroma_format == 3 {
            b.bit()?;
        }
        bit_depth = b.ue()?.checked_add(8)?;
        b.ue()?;
        b.bit()?;
        if b.flag()? {
            let lists = if chroma_format == 3 { 12 } else { 8 };
            for i in 0..lists {
                if b.flag()? {
                    let size = if i < 6 { 16 } else { 64 };
                    let (mut last, mut next) = (8i64, 8i64);
                    for _ in 0..size {
                        if next != 0 {
                            let delta = b.se()?;
                            next = (last + delta + 256) % 256;
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }
    b.ue()?;
    match b.ue()? {
        0 => {
            b.ue()?;
        }
        1 => {
            b.bit()?;
            b.se()?;
            b.se()?;
            let cycle = b.ue()?;
            if cycle > 255 {
                return None;
            }
            for _ in 0..cycle {
                b.se()?;
            }
        }
        _ => {}
    }
    b.ue()?;
    b.bit()?;
    let width_mbs = b.ue()?;
    let height_units = b.ue()?;
    let frame_mbs_only = b.flag()?;
    if !frame_mbs_only {
        b.bit()?;
    }
    b.bit()?;
    let mut crop = [0u32; 4];
    if b.flag()? {
        for c in &mut crop {
            *c = b.ue()?;
        }
    }
    // The decoded picture's size, in whole macroblocks and cropped
    // (7.4.2.1.1).
    let fields = if frame_mbs_only { 1 } else { 2 };
    let (unit_x, unit_y) = match chroma_format {
        1 => (2, 2 * fields),
        2 => (2, fields),
        _ => (1, fields),
    };
    let [left, right, top, bottom] = crop;
    let whole = width_mbs
        .checked_add(1)
        .and_then(|w| w.checked_mul(16))
        .zip(
            height_units
                .checked_add(1)
                .and_then(|h| h.checked_mul(16 * fields)),
        );
    let cropped = whole.and_then(|(w, h)| {
        let x = left.checked_add(right)?.checked_mul(unit_x)?;
        let y = top.checked_add(bottom)?.checked_mul(unit_y)?;
        w.checked_sub(x).zip(h.checked_sub(y))
    });
    let mut info = VideoInfo {
        pix_fmt: yuv(chroma_format, bit_depth, full_range),
        coded_size: cropped,
        h264_corner: cropped.filter(|_| left == 0 && top == 0),
        bits_per_raw_sample: Some(bit_depth),
        field_order: frame_mbs_only.then_some("progressive"),
        chroma_location: (chroma_format == 1).then_some("left"),
        ..VideoInfo::default()
    };
    if !b.flag()? {
        return Some(info);
    }
    // VUI (E.1.1).
    if b.flag()? {
        let idc = b.bits(8)?;
        info.sar = if idc == 255 {
            let w = b.bits(16)?;
            let h = b.bits(16)?;
            (w > 0 && h > 0).then_some((w, h))
        } else {
            usize::try_from(idc)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| SAR_TABLE.get(i))
                .copied()
        };
    }
    if b.flag()? {
        b.bit()?;
    }
    if b.flag()? {
        b.bits(4)?;
        if b.flag()? {
            b.bits(24)?;
        }
    }
    if b.flag()? {
        let top = b.ue()?;
        b.ue()?;
        if chroma_format == 1 {
            info.chroma_location = CHROMA_LOCATIONS.get(usize::try_from(top).ok()?).copied();
        }
    }
    if b.flag()? {
        let units = b.bits(32)?;
        let scale = b.bits(32)?;
        b.bit()?;
        if units > 0 && scale > 0 {
            info.frame_rate = Some((u64::from(scale), u64::from(units) * 2));
        }
    }
    let nal_hrd = b.flag()?;
    if nal_hrd {
        hrd(&mut b)?;
    }
    let vcl_hrd = b.flag()?;
    if vcl_hrd {
        hrd(&mut b)?;
    }
    if nal_hrd || vcl_hrd {
        b.bit()?;
    }
    b.bit()?;
    if b.flag()? {
        b.bit()?;
        b.ue()?;
        b.ue()?;
        b.ue()?;
        b.ue()?;
        info.has_b_frames = Some(b.ue()?);
        b.ue()?;
    }
    Some(info)
}

fn hrd(b: &mut Bits<'_>) -> Option<()> {
    let count = b.ue()?.checked_add(1)?;
    if count > 32 {
        return None;
    }
    b.bits(8)?;
    for _ in 0..count {
        b.ue()?;
        b.ue()?;
        b.bit()?;
    }
    b.bits(20)?;
    Some(())
}

/// An `hvcC` record's first sequence parameter set.
pub fn hevc(hvcc: &[u8], full_range: bool) -> Option<VideoInfo> {
    let arrays = usize::from(*hvcc.get(22)?);
    let mut at = 23;
    for _ in 0..arrays {
        let kind = *hvcc.get(at)? & 0x3f;
        let count = u16_at(hvcc, at + 1)?;
        at += 3;
        for _ in 0..count {
            let len = u16_at(hvcc, at)?;
            let nal = hvcc.get(at + 2..at + 2 + len)?;
            if kind == 33 {
                return hevc_sps(nal, full_range);
            }
            at += 2 + len;
        }
    }
    None
}

/// An H.265 sequence parameter set NAL unit, header included.
pub fn hevc_sps(nal: &[u8], full_range: bool) -> Option<VideoInfo> {
    let data = rbsp(nal, 2);
    let mut b = Bits::new(&data);
    b.bits(4)?;
    let max_sub_layers_minus1 = b.bits(3)?;
    b.bit()?;
    // profile_tier_level (7.3.3).
    b.bits(8)?;
    b.bits(32)?;
    b.bits(32)?;
    b.bits(16)?;
    b.bits(8)?;
    let mut present = Vec::new();
    for _ in 0..max_sub_layers_minus1 {
        present.push((b.flag()?, b.flag()?));
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            b.bits(2)?;
        }
    }
    for (profile, level) in present {
        if profile {
            b.bits(32)?;
            b.bits(32)?;
            b.bits(24)?;
        }
        if level {
            b.bits(8)?;
        }
    }
    b.ue()?;
    let chroma_format = b.ue()?;
    if chroma_format == 3 {
        b.bit()?;
    }
    let width = b.ue()?;
    let height = b.ue()?;
    if b.flag()? {
        for _ in 0..4 {
            b.ue()?;
        }
    }
    let bit_depth = b.ue()?.checked_add(8)?;
    b.ue()?;
    let log2_max_poc_lsb = b.ue()?.checked_add(4)?;
    if log2_max_poc_lsb > 16 {
        return None;
    }
    let ordering = b.flag()?;
    let first = if ordering { 0 } else { max_sub_layers_minus1 };
    let mut reorder = 0;
    for _ in first..=max_sub_layers_minus1 {
        b.ue()?;
        reorder = b.ue()?;
        b.ue()?;
    }
    let mut info = VideoInfo {
        pix_fmt: yuv(chroma_format, bit_depth, full_range),
        has_b_frames: Some(reorder),
        chroma_location: (chroma_format == 1).then_some("left"),
        field_order: None,
        bits_per_raw_sample: None,
        coded_size: Some((width, height)),
        h264_corner: None,
        frame_rate: None,
        sar: None,
        profile: None,
        mpeg4_flags: None,
    };
    // What follows only adds the VUI's fields; a set that ends early
    // keeps what was read.
    let _ = hevc_vui(&mut b, log2_max_poc_lsb, chroma_format, &mut info);
    Some(info)
}

/// The rest of an H.265 sequence parameter set up to and through the
/// timing of its VUI (7.3.2.2, E.2.1).
fn hevc_vui(
    b: &mut Bits<'_>,
    log2_max_poc_lsb: u32,
    chroma_format: u32,
    info: &mut VideoInfo,
) -> Option<()> {
    for _ in 0..6 {
        b.ue()?;
    }
    if b.flag()? && b.flag()? {
        hevc_scaling_list_data(b)?;
    }
    b.bits(2)?;
    if b.flag()? {
        b.bits(8)?;
        b.ue()?;
        b.ue()?;
        b.bit()?;
    }
    let sets = b.ue()?;
    if sets > 64 {
        return None;
    }
    let mut counts: Vec<u32> = Vec::new();
    for index in 0..sets {
        let inter = index != 0 && b.flag()?;
        let count = if inter {
            b.bit()?;
            b.ue()?;
            let reference = *counts.last()?;
            let mut count = 0u32;
            for _ in 0..=reference {
                let used = b.flag()?;
                if used || b.flag()? {
                    count = count.checked_add(1)?;
                }
            }
            count
        } else {
            let negative = b.ue()?;
            let positive = b.ue()?;
            if negative > 16 || positive > 16 {
                return None;
            }
            for _ in 0..negative + positive {
                b.ue()?;
                b.bit()?;
            }
            negative + positive
        };
        if count > 32 {
            return None;
        }
        counts.push(count);
    }
    if b.flag()? {
        let long_term = b.ue()?;
        if long_term > 32 {
            return None;
        }
        for _ in 0..long_term {
            b.bits(log2_max_poc_lsb)?;
            b.bit()?;
        }
    }
    b.bits(2)?;
    if !b.flag()? {
        return Some(());
    }
    if b.flag()? {
        let idc = b.bits(8)?;
        info.sar = if idc == 255 {
            let w = b.bits(16)?;
            let h = b.bits(16)?;
            (w > 0 && h > 0).then_some((w, h))
        } else {
            usize::try_from(idc)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| SAR_TABLE.get(i))
                .copied()
        };
    }
    if b.flag()? {
        b.bit()?;
    }
    if b.flag()? {
        b.bits(4)?;
        if b.flag()? {
            b.bits(24)?;
        }
    }
    if b.flag()? {
        let top = b.ue()?;
        b.ue()?;
        if chroma_format == 1 {
            info.chroma_location = CHROMA_LOCATIONS.get(usize::try_from(top).ok()?).copied();
        }
    }
    b.bits(3)?;
    if b.flag()? {
        for _ in 0..4 {
            b.ue()?;
        }
    }
    if b.flag()? {
        let units = b.bits(32)?;
        let scale = b.bits(32)?;
        if units > 0 && scale > 0 {
            info.frame_rate = Some((u64::from(scale), u64::from(units)));
        }
    }
    Some(())
}

/// `scaling_list_data` (H.265 7.3.4).
fn hevc_scaling_list_data(b: &mut Bits<'_>) -> Option<()> {
    for size in 0..4u32 {
        let step = if size == 3 { 3 } else { 1 };
        let mut matrix = 0;
        while matrix < 6 {
            if b.flag()? {
                let coefficients = 64.min(1u32 << (4 + (size << 1)));
                if size > 1 {
                    b.se()?;
                }
                for _ in 0..coefficients {
                    b.se()?;
                }
            } else {
                b.ue()?;
            }
            matrix += step;
        }
    }
    Some(())
}

/// An `av1C` record.
pub fn av1(av1c: &[u8]) -> Option<VideoInfo> {
    let flags = *av1c.get(2)?;
    let high = flags & 0x40 != 0;
    let twelve = flags & 0x20 != 0;
    let mono = flags & 0x10 != 0;
    let (x, y) = (flags & 0x08 != 0, flags & 0x04 != 0);
    let bits = match (high, twelve) {
        (false, _) => 8,
        (true, false) => 10,
        (true, true) => 12,
    };
    let chroma_format = match (mono, x, y) {
        (true, _, _) => 0,
        (false, true, true) => 1,
        (false, true, false) => 2,
        (false, false, _) => 3,
    };
    let location = match flags & 0x03 {
        1 => "left",
        2 => "topleft",
        _ => "unspecified",
    };
    Some(VideoInfo {
        pix_fmt: yuv(chroma_format, bits, false),
        has_b_frames: Some(0),
        chroma_location: Some(location),
        // Named by the entry's `fiel` box alone (`iso_meta`).
        field_order: None,
        bits_per_raw_sample: None,
        coded_size: None,
        h264_corner: None,
        frame_rate: None,
        sar: None,
        profile: None,
        mpeg4_flags: None,
    })
}

/// A `vpcC` record (version 1).
pub fn vp9(vpcc: &[u8]) -> Option<VideoInfo> {
    let packed = *vpcc.get(6)?;
    let bits = u32::from(packed >> 4);
    let chroma_format = match (packed >> 1) & 0x07 {
        0 | 1 => 1,
        2 => 2,
        3 => 3,
        _ => return None,
    };
    Some(VideoInfo {
        pix_fmt: yuv(chroma_format, bits, false),
        has_b_frames: Some(0),
        chroma_location: Some("unspecified"),
        // Named by the entry's `fiel` box alone (`iso_meta`).
        field_order: None,
        bits_per_raw_sample: None,
        coded_size: None,
        h264_corner: None,
        frame_rate: None,
        sar: None,
        profile: None,
        mpeg4_flags: None,
    })
}

/// The pixel aspect ratio an MPEG-4 Part 2 video object layer header
/// states (ISO/IEC 14496-2 6.2.3, table 6-12): the first layer start code
/// (0x20 to 0x2F) in `data`, then its `aspect_ratio_info`.
pub fn mpeg4_aspect(data: &[u8]) -> Option<(u32, u32)> {
    let start = data
        .windows(4)
        .position(|w| matches!(w, [0, 0, 1, 0x20..=0x2F]))?;
    let mut b = Bits::new(data.get(start + 4..)?);
    b.bit()?;
    b.bits(8)?;
    if b.flag()? {
        b.bits(7)?;
    }
    Some(match b.bits(4)? {
        1 => (1, 1),
        2 => (12, 11),
        3 => (10, 11),
        4 => (16, 11),
        5 => (40, 33),
        15 => {
            let w = b.bits(8)?;
            let h = b.bits(8)?;
            if w == 0 || h == 0 {
                return None;
            }
            (w, h)
        }
        _ => return None,
    })
}

/// A ProRes frame header (SMPTE RDD 36 5.1): the start of a sample.
pub fn prores(frame: &[u8]) -> Option<VideoInfo> {
    if frame.get(4..8)? != b"icpf" {
        return None;
    }
    let width = u32::try_from(u16_at(frame, 16)?).ok()?;
    let height = u32::try_from(u16_at(frame, 18)?).ok()?;
    let packed = *frame.get(20)?;
    let chroma = (packed >> 6) & 0x03;
    let interlace = (packed >> 2) & 0x03;
    let alpha = *frame.get(25)? & 0x0f;
    let (pix_fmt, bits) = match (chroma, alpha) {
        (2, 0) => ("yuv422p10le", 10),
        (2, _) => ("yuva422p10le", 10),
        (3, 0) => ("yuv444p12le", 12),
        (3, _) => ("yuva444p12le", 12),
        _ => return None,
    };
    Some(VideoInfo {
        pix_fmt: Some(pix_fmt.to_owned()),
        has_b_frames: Some(0),
        chroma_location: Some("unspecified"),
        field_order: Some(match interlace {
            0 => "progressive",
            1 => "tt",
            2 => "bb",
            _ => return None,
        }),
        bits_per_raw_sample: Some(bits),
        coded_size: Some((width, height)),
        h264_corner: None,
        frame_rate: None,
        sar: None,
        profile: None,
        mpeg4_flags: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb() {
        // 1, 010, 011, 00100 → 0, 1, 2, 3.
        let data = [0b1010_0110, 0b0100_0000];
        let mut b = Bits::new(&data);
        assert_eq!(b.ue(), Some(0));
        assert_eq!(b.ue(), Some(1));
        assert_eq!(b.ue(), Some(2));
        assert_eq!(b.ue(), Some(3));
    }

    #[test]
    fn emulation_prevention_is_removed() {
        assert_eq!(rbsp(&[0x67, 0, 0, 3, 1], 1), vec![0, 0, 1]);
    }

    #[test]
    fn pixel_formats() {
        assert_eq!(yuv(1, 8, false).as_deref(), Some("yuv420p"));
        assert_eq!(yuv(1, 8, true).as_deref(), Some("yuvj420p"));
        assert_eq!(yuv(1, 10, true).as_deref(), Some("yuv420p10le"));
        assert_eq!(yuv(2, 10, false).as_deref(), Some("yuv422p10le"));
    }

    #[test]
    fn mpeg4_aspect_ratio() {
        // VOL start code, random_accessible_vol 0, type 1, no identifier,
        // aspect_ratio_info 1.
        assert_eq!(mpeg4_aspect(&[0, 0, 1, 0x20, 0x00, 0x84]), Some((1, 1)));
        assert_eq!(mpeg4_aspect(&[0, 0, 1, 0xB0]), None);
    }

    #[test]
    fn truncated_records_give_nothing() {
        assert_eq!(avc(&[1, 100, 0, 30, 0xff, 0xe1], false), None);
        assert_eq!(hevc(&[1; 10], false), None);
        assert_eq!(av1(&[0x81]), None);
    }
}
