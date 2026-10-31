//! Picture properties `ffprobe` prints for a video stream that the codec
//! configuration determines: the pixel format, the reordering depth, the
//! chroma location, the field order, the coded size and the bit depth.
//! Read from the stored configuration records (ITU-T H.264 7.3.2.1.1 and
//! E.1.1, ITU-T H.265 7.3.2.2, the AV1 and VP codec ISOBMFF bindings,
//! SMPTE RDD 36 for ProRes frame headers) and, for codecs without one, from
//! the first frame (ITU-T T.81 and JFIF for JPEG, ITU-T H.263 5.1, the DV
//! header and VAUX packs of IEC 61834-2 and -4, SMPTE 314M and 370M);
//! untrusted bytes, so every read is bounded and a record that does not
//! parse gives nothing.

use coreav_isobmff::Color;

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
    /// The columns an H.264 picture crops on its left, in luma samples.
    pub h264_crop_left: Option<u32>,
    /// An H.264 stream's video signal type (E.2.1): `video_full_range_flag`
    /// and, when described, `colour_primaries`, `transfer_characteristics`
    /// and `matrix_coefficients`.
    pub h264_signal: Option<(bool, Option<[u32; 3]>)>,
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
    /// The colour the stream itself states, over the container's.
    pub color: Option<Color>,
    /// The picture size the stream states, which `ffprobe` prints over
    /// the sample entry's (DV).
    pub size: Option<(u32, u32)>,
    /// The bit rate the coding fixes (DV).
    pub bit_rate: Option<i64>,
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

    pub(super) fn ue(&mut self) -> Option<u32> {
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
pub(super) fn rbsp(nal: &[u8], header: usize) -> Vec<u8> {
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
    // An 8-bit monochrome stream is decoded as 4:2:0 (found black box).
    let shown = if chroma_format == 0 && bit_depth == 8 {
        1
    } else {
        chroma_format
    };
    let mut info = VideoInfo {
        pix_fmt: yuv(shown, bit_depth, full_range),
        coded_size: cropped,
        h264_corner: cropped.filter(|_| left == 0 && top == 0),
        h264_crop_left: left.checked_mul(unit_x),
        bits_per_raw_sample: Some(bit_depth),
        field_order: frame_mbs_only.then_some("progressive"),
        chroma_location: (shown > 0).then_some("left"),
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
        b.bits(3)?;
        let full_range = b.flag()?;
        let described = if b.flag()? {
            Some([b.bits(8)?, b.bits(8)?, b.bits(8)?])
        } else {
            None
        };
        info.h264_signal = Some((full_range, described));
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
        h264_crop_left: None,
        h264_signal: None,
        frame_rate: None,
        sar: None,
        profile: None,
        mpeg4_flags: None,
        color: None,
        size: None,
        bit_rate: None,
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
        h264_crop_left: None,
        h264_signal: None,
        frame_rate: None,
        sar: None,
        profile: None,
        mpeg4_flags: None,
        color: None,
        size: None,
        bit_rate: None,
    })
}

/// A `vpcC` record, shared by VP8 and VP9.
pub fn vp(vpcc: &[u8]) -> Option<VideoInfo> {
    let packed = *vpcc.get(6)?;
    let bits = u32::from(packed >> 4);
    if !matches!(bits, 8 | 10 | 12) {
        return None;
    }
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
        h264_crop_left: None,
        h264_signal: None,
        frame_rate: None,
        sar: None,
        profile: None,
        mpeg4_flags: None,
        color: None,
        size: None,
        bit_rate: None,
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
        h264_crop_left: None,
        h264_signal: None,
        frame_rate: None,
        sar: None,
        profile: None,
        mpeg4_flags: None,
        color: None,
        size: None,
        bit_rate: None,
    })
}

/// A JPEG frame (Motion JPEG): what its frame header and JFIF density say,
/// in full range with BT.601 colour, as a JPEG decoder reports it.
pub fn mjpeg(frame: &[u8]) -> Option<VideoInfo> {
    let picture = super::mkv::image::jpeg(frame)?;
    Some(VideoInfo {
        pix_fmt: picture.pix_fmt.map(str::to_owned),
        has_b_frames: Some(0),
        chroma_location: Some("center"),
        bits_per_raw_sample: picture.bits,
        sar: picture.aspect,
        profile: picture.profile.map(|name| (name.to_owned(), -99)),
        color: Some(Color {
            matrix: Some(5),
            full_range: Some(true),
            ..Color::default()
        }),
        ..VideoInfo::default()
    })
}

/// An H.263 stream (4:2:0, no B-frames) from its first picture header
/// (ITU-T H.263 5.1.1 to 5.1.3): the standard source formats have pixels
/// of 12:11 (4.1); an extended type (`PLUSPTYPE`) is not read.
pub fn h263(frame: &[u8]) -> VideoInfo {
    let sar = || -> Option<(u32, u32)> {
        let mut b = Bits::new(frame);
        if b.bits(22)? != 0x20 {
            return None;
        }
        b.bits(8)?;
        if b.bits(2)? != 0b10 {
            return None;
        }
        b.bits(3)?;
        matches!(b.bits(3)?, 1..=5).then_some((12, 11))
    };
    VideoInfo {
        pix_fmt: Some("yuv420p".to_owned()),
        has_b_frames: Some(0),
        chroma_location: Some("center"),
        sar: sar(),
        ..VideoInfo::default()
    }
}

/// A DV frame: the sampling its header and VAUX source pack state (IEC
/// 61834-2: the DSF bit and APT of the header DIF block; SMPTE 314M and
/// 370M: the source type of pack 0x60) and the pixel aspect of its display
/// mode (pack 0x61, 4:3 or 16:9).
pub fn dv(frame: &[u8], frame_bytes: u32) -> Option<VideoInfo> {
    let header = frame.get(..80)?;
    if header.first()? >> 5 != 0 {
        return None;
    }
    let lines_625 = header.get(3)? >> 7 == 1;
    let apt = header.get(4)? & 0x07;
    let (mut stype, mut display) = (None, None);
    for block in 3..6 {
        let Some(vaux) = frame.get(block * 80 + 3..block * 80 + 78) else {
            break;
        };
        for pack in vaux.chunks_exact(5) {
            match *pack {
                [0x60, _, _, pc3, _] => {
                    stype.get_or_insert(pc3 & 0x1f);
                }
                [0x61, _, pc2, _, _] => {
                    display.get_or_insert(pc2 & 0x07);
                }
                _ => {}
            }
        }
    }
    let hd = matches!(stype, Some(0x14 | 0x15 | 0x18));
    let pix_fmt = if hd || stype == Some(0x04) {
        "yuv422p"
    } else if lines_625 && apt == 0 {
        "yuv420p"
    } else {
        "yuv411p"
    };
    let wide = display == Some(2);
    let sar = match (stype, lines_625, wide) {
        (Some(0x00 | 0x04), false, false) => Some((8, 9)),
        (Some(0x00 | 0x04), false, true) => Some((32, 27)),
        (Some(0x00 | 0x04), true, false) => Some((16, 15)),
        (Some(0x00 | 0x04), true, true) => Some((64, 45)),
        (Some(0x18), _, _) => Some((4, 3)),
        _ => None,
    };
    // A decoder reports the coded picture and the bit rate of the system's
    // nominal frame rate (525/60 or 625/50, doubled for 720p).
    let size = match (stype, lines_625) {
        (Some(0x00 | 0x04), false) => Some((720, 480)),
        (Some(0x00 | 0x04), true) => Some((720, 576)),
        (Some(0x18), _) => Some((960, 720)),
        _ => None,
    };
    let (num, den) = match (lines_625, stype == Some(0x18)) {
        (false, false) => (30_000, 1001),
        (false, true) => (60_000, 1001),
        (true, false) => (25, 1),
        (true, true) => (50, 1),
    };
    let bits = i64::from(frame_bytes) * 8 * num;
    Some(VideoInfo {
        pix_fmt: Some(pix_fmt.to_owned()),
        has_b_frames: Some(0),
        chroma_location: Some("topleft"),
        sar,
        size,
        bit_rate: Some((bits + den / 2) / den),
        ..VideoInfo::default()
    })
}

/// A PNG frame: the pixel format of its `IHDR`, in full range RGB.
pub fn png(frame: &[u8]) -> Option<VideoInfo> {
    let picture = super::mkv::image::png(frame)?;
    Some(VideoInfo {
        pix_fmt: picture.pix_fmt.map(str::to_owned),
        has_b_frames: Some(0),
        sar: picture.aspect,
        color: Some(Color {
            matrix: Some(0),
            full_range: Some(true),
            ..picture.color()
        }),
        ..VideoInfo::default()
    })
}

/// A codec whose decoder output is fixed (SVQ1 4:1:0; SVQ3 4:2:0 in full
/// range), or set by the sample entry's depth alone (QuickTime Animation,
/// 8BPS), with no reordering: what `ffprobe` reports, observed on files of
/// each.
pub fn fixed(codec: &str, depth: Option<u16>) -> Option<VideoInfo> {
    let (pix_fmt, bits, full_range) = match (codec, depth) {
        ("svq1", _) => ("yuv410p", None, false),
        ("svq3", _) => ("yuvj420p", Some(8), true),
        ("qtrle", Some(1)) => ("monow", None, false),
        ("qtrle", Some(2 | 4 | 8)) => ("pal8", None, false),
        ("qtrle", Some(16)) => ("rgb555le", None, false),
        ("qtrle", Some(24)) => ("rgb24", None, false),
        ("qtrle", Some(32)) => ("argb", None, false),
        ("8bps", Some(24)) => ("gbrp", None, false),
        _ => return None,
    };
    Some(VideoInfo {
        pix_fmt: Some(pix_fmt.to_owned()),
        has_b_frames: Some(0),
        bits_per_raw_sample: bits,
        color: full_range.then(|| Color {
            full_range: Some(true),
            ..Color::default()
        }),
        ..VideoInfo::default()
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
#[test]
fn vp_configuration_maps_depth_and_subsampling_without_a_jpeg_range_format() {
    for (packed, expected) in [
        (0x80, "yuv420p"),
        (0x82, "yuv420p"),
        (0x84, "yuv422p"),
        (0x86, "yuv444p"),
        (0xa0, "yuv420p10le"),
        (0xc4, "yuv422p12le"),
    ] {
        for range in [0, 1] {
            let info = vp(&[1, 0, 0, 0, 0, 0, packed | range, 1, 1, 1, 0, 0]).unwrap();
            assert_eq!(info.pix_fmt.as_deref(), Some(expected));
        }
    }
    for packed in [0x88, 0x90, 0xf0] {
        assert_eq!(vp(&[1, 0, 0, 0, 0, 0, packed, 1, 1, 1, 0, 0]), None);
    }
    assert_eq!(vp(&[1, 0, 0, 0]), None);
}
