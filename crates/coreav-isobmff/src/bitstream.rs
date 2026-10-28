//! Color range, profile and level from codec configuration records.
//!
//! Only the bits needed to reach the wanted fields are read. Every read is
//! bounded by the record bytes; a malformed record gives `None` (unknown)
//! and never an error.

use crate::bytes::{Bits, be_u16_at};

/// Removes emulation prevention bytes (`0x000003`) from a NAL unit payload
/// (ITU-T H.265 7.4.2), keeping at most `max` bytes.
pub(crate) fn rbsp(nal: &[u8], max: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len().min(max));
    let mut zeros = 0usize;
    for &byte in nal {
        if out.len() >= max {
            break;
        }
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        zeros = if byte == 0 {
            zeros.saturating_add(1)
        } else {
            0
        };
        out.push(byte);
    }
    out
}

/// The video signal type a parameter set or sequence header describes:
/// color range and the ITU-T H.273 code points as stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Signal {
    pub(crate) full_range: Option<bool>,
    /// `colour_primaries`, `transfer_characteristics`, `matrix_coeffs`
    /// when the colour description is present.
    pub(crate) colour: Option<(u8, u8, u8)>,
}

/// `video_full_range_flag` of the first sequence parameter set in an
/// `hvcC` record (ISO/IEC 14496-15 8.3.3; ITU-T H.265 7.3.2.2 and E.2.1).
pub(crate) fn hevc_full_range(hvcc: &[u8]) -> Option<bool> {
    hevc_signal(hvcc)?.full_range
}

/// The video signal type of the first sequence parameter set in an `hvcC`
/// record; fields it does not store are unknown, and `None` means it cannot
/// be read.
pub(crate) fn hevc_signal(hvcc: &[u8]) -> Option<Signal> {
    // Skip the two-byte NAL unit header.
    hevc_sps_signal(&rbsp(hevc_first_sps(hvcc)?.get(2..)?, 4096))
}

pub(crate) fn profile_tier_level(bits: &mut Bits<'_>, max_sub_layers_minus1: u32) -> Option<()> {
    bits.skip(96)?;
    let mut profile_present = [false; 8];
    let mut level_present = [false; 8];
    for i in 0..max_sub_layers_minus1 as usize {
        *profile_present.get_mut(i)? = bits.flag()?;
        *level_present.get_mut(i)? = bits.flag()?;
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            bits.skip(2)?;
        }
    }
    for i in 0..max_sub_layers_minus1 as usize {
        if *profile_present.get(i)? {
            bits.skip(88)?;
        }
        if *level_present.get(i)? {
            bits.skip(8)?;
        }
    }
    Some(())
}

fn scaling_list_data(bits: &mut Bits<'_>) -> Option<()> {
    for size_id in 0u32..4 {
        let step = if size_id == 3 { 3 } else { 1 };
        let mut matrix_id = 0u32;
        while matrix_id < 6 {
            if !bits.flag()? {
                bits.ue()?;
            } else {
                let coef_num = 64u32.min(1 << (4 + (size_id << 1)));
                if size_id > 1 {
                    bits.se()?;
                }
                for _ in 0..coef_num {
                    bits.se()?;
                }
            }
            matrix_id += step;
        }
    }
    Some(())
}

fn hevc_sps_signal(sps: &[u8]) -> Option<Signal> {
    let mut bits = Bits::new(sps);
    bits.skip(4)?;
    let max_sub_layers_minus1 = bits.bits(3)?;
    if max_sub_layers_minus1 > 6 {
        return None;
    }
    bits.skip(1)?;
    profile_tier_level(&mut bits, max_sub_layers_minus1)?;
    bits.ue()?;
    if bits.ue()? == 3 {
        bits.skip(1)?;
    }
    bits.ue()?;
    bits.ue()?;
    if bits.flag()? {
        for _ in 0..4 {
            bits.ue()?;
        }
    }
    bits.ue()?;
    bits.ue()?;
    let log2_max_poc_lsb = bits.ue()?.checked_add(4)?;
    if log2_max_poc_lsb > 16 {
        return None;
    }
    let ordering_all = bits.flag()?;
    let first = if ordering_all {
        0
    } else {
        max_sub_layers_minus1
    };
    for _ in first..=max_sub_layers_minus1 {
        bits.ue()?;
        bits.ue()?;
        bits.ue()?;
    }
    for _ in 0..6 {
        bits.ue()?;
    }
    if bits.flag()? && bits.flag()? {
        scaling_list_data(&mut bits)?;
    }
    bits.skip(2)?;
    if bits.flag()? {
        bits.skip(8)?;
        bits.ue()?;
        bits.ue()?;
        bits.skip(1)?;
    }
    let sets = bits.ue()?;
    if sets > 64 {
        return None;
    }
    // NumDeltaPocs of every short-term reference picture set (7.4.8).
    let mut delta_pocs: Vec<u32> = Vec::with_capacity(sets as usize);
    for index in 0..sets {
        let inter = index != 0 && bits.flag()?;
        let count = if inter {
            bits.skip(1)?;
            bits.ue()?;
            let reference = *delta_pocs.last()?;
            let mut count = 0u32;
            for _ in 0..=reference {
                let used = bits.flag()?;
                let use_delta = used || bits.flag()?;
                if use_delta {
                    count = count.checked_add(1)?;
                }
            }
            count
        } else {
            let negative = bits.ue()?;
            let positive = bits.ue()?;
            if negative > 16 || positive > 16 {
                return None;
            }
            for _ in 0..negative.checked_add(positive)? {
                bits.ue()?;
                bits.skip(1)?;
            }
            negative + positive
        };
        if count > 32 {
            return None;
        }
        delta_pocs.push(count);
    }
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
        // No VUI: the signal type is not stored.
        return Some(Signal::default());
    }
    if bits.flag()? && bits.bits(8)? == 255 {
        bits.skip(32)?;
    }
    if bits.flag()? {
        bits.skip(1)?;
    }
    video_signal_type(&mut bits)
}

/// `video_signal_type_present_flag` and what follows it, the same in ITU-T
/// H.264 E.1.1 and H.265 E.2.1: `video_format`, `video_full_range_flag`,
/// then the colour description.
/// `Some` with unknown fields when the flag is clear; `None` only when the
/// bits run out.
fn video_signal_type(bits: &mut Bits<'_>) -> Option<Signal> {
    if !bits.flag()? {
        return Some(Signal::default());
    }
    bits.skip(3)?;
    let full_range = bits.flag()?;
    let colour = if bits.flag()? {
        let primaries = u8::try_from(bits.bits(8)?).ok()?;
        let transfer = u8::try_from(bits.bits(8)?).ok()?;
        let matrix = u8::try_from(bits.bits(8)?).ok()?;
        Some((primaries, transfer, matrix))
    } else {
        None
    };
    Some(Signal {
        full_range: Some(full_range),
        colour,
    })
}

/// The video signal type of the first sequence parameter set in an `avcC`
/// record (ITU-T H.264 7.3.2.1.1 and E.1.1); fields it does not store are
/// unknown, and `None` means it cannot be read.
pub(crate) fn avc_signal(avcc: &[u8]) -> Option<Signal> {
    with_avc_sps(avcc, |bits| {
        to_max_num_ref_frames(bits)?;
        bits.ue()?;
        bits.skip(1)?;
        bits.ue()?;
        bits.ue()?;
        if !bits.flag()? {
            bits.skip(1)?;
        }
        bits.skip(1)?;
        if bits.flag()? {
            for _ in 0..4 {
                bits.ue()?;
            }
        }
        if !bits.flag()? {
            // No VUI: the signal type is not stored.
            return Some(Signal::default());
        }
        // vui_parameters (E.1.1): aspect ratio, overscan, then the signal type.
        if bits.flag()? && bits.bits(8)? == 255 {
            bits.skip(32)?;
        }
        if bits.flag()? {
            bits.skip(1)?;
        }
        video_signal_type(bits)
    })
}

/// `num_units_in_tick` and `time_scale` of the first sequence parameter
/// set in an `avcC` record (ITU-T H.264 E.2.1): the clock the stream
/// states for itself, from which a field lasts `num_units_in_tick /
/// time_scale` seconds and a frame twice that. `None` when the set has no
/// timing information or cannot be read.
pub(crate) fn avc_timing(avcc: &[u8]) -> Option<(u32, u32)> {
    with_avc_sps(avcc, |bits| {
        to_max_num_ref_frames(bits)?;
        bits.ue()?;
        bits.skip(1)?;
        bits.ue()?;
        bits.ue()?;
        if !bits.flag()? {
            bits.skip(1)?;
        }
        bits.skip(1)?;
        if bits.flag()? {
            for _ in 0..4 {
                bits.ue()?;
            }
        }
        if !bits.flag()? {
            // No VUI: the stream states no clock.
            return None;
        }
        // vui_parameters (E.1.1), to timing_info: aspect ratio, overscan,
        // the signal type, then the chroma sample location.
        if bits.flag()? && bits.bits(8)? == 255 {
            bits.skip(32)?;
        }
        if bits.flag()? {
            bits.skip(1)?;
        }
        if bits.flag()? {
            bits.skip(4)?;
            if bits.flag()? {
                bits.skip(24)?;
            }
        }
        if bits.flag()? {
            bits.ue()?;
            bits.ue()?;
        }
        if !bits.flag()? {
            return None;
        }
        let units = bits.bits(32)?;
        let scale = bits.bits(32)?;
        (units > 0 && scale > 0).then_some((units, scale))
    })
}

/// `max_num_ref_frames` of the first sequence parameter set in an `avcC`
/// record (ITU-T H.264 7.4.2.1.1): the most frames a P or B frame may
/// reference. `None` when the set cannot be read.
pub(crate) fn avc_max_num_ref_frames(avcc: &[u8]) -> Option<u32> {
    with_avc_sps(avcc, |bits| {
        to_max_num_ref_frames(bits)?;
        bits.ue()
    })
}

/// Gives `read` the bits of the first sequence parameter set of an `avcC`
/// record (ISO/IEC 14496-15 5.3.3.1), emulation prevention removed.
fn with_avc_sps<T>(avcc: &[u8], read: impl FnOnce(&mut Bits<'_>) -> Option<T>) -> Option<T> {
    if *avcc.get(5)? & 0x1f == 0 {
        return None;
    }
    let len = usize::from(be_u16_at(avcc, 6)?);
    let nal = avcc.get(8..8usize.checked_add(len)?)?;
    if nal.first()? & 0x1f != 7 {
        return None;
    }
    let sps = rbsp(nal.get(1..)?, 4096);
    read(&mut Bits::new(&sps))
}

/// Reads a sequence parameter set (ITU-T H.264 7.3.2.1.1) up to
/// `max_num_ref_frames`.
fn to_max_num_ref_frames(bits: &mut Bits<'_>) -> Option<()> {
    let profile_idc = bits.bits(8)?;
    bits.skip(16)?;
    bits.ue()?;
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        let chroma_format_idc = bits.ue()?;
        if chroma_format_idc == 3 {
            bits.skip(1)?;
        }
        bits.ue()?;
        bits.ue()?;
        bits.skip(1)?;
        if bits.flag()? {
            let lists = if chroma_format_idc == 3 { 12 } else { 8 };
            for i in 0..lists {
                if bits.flag()? {
                    // scaling_list (7.3.2.1.1.1): 16 or 64 coefficients.
                    let size = if i < 6 { 16 } else { 64 };
                    let (mut last, mut next) = (8i64, 8i64);
                    for _ in 0..size {
                        if next != 0 {
                            next = (last + bits.se()? + 256).rem_euclid(256);
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }
    bits.ue()?;
    match bits.ue()? {
        0 => {
            bits.ue()?;
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
    Some(())
}

/// Profile, constraint flags and level of the first sequence parameter set
/// in an `avcC` record (ISO/IEC 14496-15 5.3.3.1; ITU-T H.264 7.3.2.1.1).
pub(crate) fn avc_sps_profile(avcc: &[u8]) -> Option<(u8, u8, u8)> {
    if *avcc.get(5)? & 0x1f == 0 {
        return None;
    }
    let len = usize::from(be_u16_at(avcc, 6)?);
    let nal = avcc.get(8..8usize.checked_add(len)?)?;
    if nal.first()? & 0x1f != 7 {
        return None;
    }
    let sps = rbsp(nal.get(1..)?, 3);
    let [profile, flags, level] = *sps.as_slice() else {
        return None;
    };
    Some((profile, flags, level))
}

/// General profile space, tier, profile, compatibility flags, constraint
/// flags (48 bits) and level of a profile_tier_level structure (ITU-T H.265
/// 7.3.3).
pub(crate) type HevcGeneral = (u8, bool, u8, u32, u64, u8);

/// The general profile, tier and level of the first sequence parameter set
/// in an `hvcC` record (ITU-T H.265 7.3.2.2.1 and 7.3.3).
pub(crate) fn hevc_sps_profile(hvcc: &[u8]) -> Option<HevcGeneral> {
    let sps = hevc_first_sps(hvcc)?;
    let sps = rbsp(sps.get(2..)?, 16);
    let mut bits = Bits::new(&sps);
    bits.skip(8)?;
    let space = u8::try_from(bits.bits(2)?).ok()?;
    let tier = bits.flag()?;
    let profile = u8::try_from(bits.bits(5)?).ok()?;
    let compatibility = bits.bits(32)?;
    let constraint = (u64::from(bits.bits(16)?) << 32) | u64::from(bits.bits(32)?);
    let level = u8::try_from(bits.bits(8)?).ok()?;
    Some((space, tier, profile, compatibility, constraint, level))
}

/// Whether an `avcC`, `hvcC` or `av1C` record carries the parameter set
/// its signal type is read from (a record may legally carry none).
pub(crate) fn has_parameter_set(kind: &[u8; 4], record: &[u8]) -> bool {
    match kind {
        b"avcC" => record.get(5).is_some_and(|n| n & 0x1f > 0),
        b"hvcC" => hevc_first_sps(record).is_some(),
        b"av1C" => av1_sequence_header(record).is_some(),
        _ => false,
    }
}

/// The first sequence parameter set NAL unit of an `hvcC` record.
fn hevc_first_sps(hvcc: &[u8]) -> Option<&[u8]> {
    let arrays = *hvcc.get(22)?;
    let mut at = 23usize;
    for _ in 0..arrays {
        let nal_type = *hvcc.get(at)? & 0x3f;
        let count = be_u16_at(hvcc, at.checked_add(1)?)?;
        at = at.checked_add(3)?;
        for _ in 0..count {
            let len = usize::from(be_u16_at(hvcc, at)?);
            let start = at.checked_add(2)?;
            let end = start.checked_add(len)?;
            let nal = hvcc.get(start..end)?;
            if nal_type == 33 {
                return Some(nal);
            }
            at = end;
        }
    }
    None
}

/// `seq_profile`, `seq_level_idx[0]` and `seq_tier[0]` of the sequence
/// header OBU in an `av1C` record (AV1 specification 5.5.1).
pub(crate) fn av1_sequence_profile(av1c: &[u8]) -> Option<(u8, u8, bool)> {
    let payload = av1_sequence_header(av1c)?;
    let mut bits = Bits::new(payload);
    let profile = u8::try_from(bits.bits(3)?).ok()?;
    bits.skip(1)?;
    if bits.flag()? {
        // reduced_still_picture_header: one operating point, tier 0.
        return Some((profile, u8::try_from(bits.bits(5)?).ok()?, false));
    }
    if bits.flag()? {
        // timing_info, then decoder_model_info when present.
        bits.skip(64)?;
        if bits.flag()? {
            uvlc(&mut bits)?;
        }
        if bits.flag()? {
            bits.skip(5 + 32 + 5 + 5)?;
        }
    }
    // initial_display_delay_present_flag, operating_points_cnt_minus_1 and
    // operating_point_idc[0]; the level of point 0 follows.
    bits.skip(1 + 5 + 12)?;
    let level = u8::try_from(bits.bits(5)?).ok()?;
    let tier = level > 7 && bits.flag()?;
    Some((profile, level, tier))
}

/// The payload of the first sequence header OBU in an `av1C` record.
fn av1_sequence_header(av1c: &[u8]) -> Option<&[u8]> {
    let mut at = 4usize;
    while at < av1c.len() {
        let header = *av1c.get(at)?;
        let obu_type = (header >> 3) & 0x0f;
        let extension = header & 0x04 != 0;
        let has_size = header & 0x02 != 0;
        at = at.checked_add(1 + usize::from(extension))?;
        let size = if has_size {
            usize::try_from(leb128(av1c, &mut at)?).ok()?
        } else {
            av1c.len().checked_sub(at)?
        };
        let end = at.checked_add(size)?;
        let payload = av1c.get(at..end)?;
        if obu_type == 1 {
            return Some(payload);
        }
        at = end;
    }
    None
}

/// Reads a LEB128 value (AV1 specification 4.10.5).
pub(crate) fn leb128(data: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for i in 0..8u32 {
        let byte = *data.get(*at)?;
        *at = at.checked_add(1)?;
        value |= u64::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// `color_range` of the sequence header OBU in an `av1C` record (AV1 ISOBMFF
/// binding 2.3; AV1 specification 5.5.1 and 5.5.2).
pub(crate) fn av1_full_range(av1c: &[u8]) -> Option<bool> {
    av1_signal(av1c)?.full_range
}

/// The color configuration of the sequence header OBU in an `av1C` record.
pub(crate) fn av1_signal(av1c: &[u8]) -> Option<Signal> {
    av1_sequence_signal(av1_sequence_header(av1c)?)
}

pub(crate) fn uvlc(bits: &mut Bits<'_>) -> Option<()> {
    let mut zeros = 0u32;
    while !bits.flag()? {
        zeros += 1;
        if zeros >= 32 {
            return Some(());
        }
    }
    bits.skip(zeros)
}

fn av1_sequence_signal(payload: &[u8]) -> Option<Signal> {
    let mut bits = Bits::new(payload);
    let profile = bits.bits(3)?;
    bits.skip(1)?;
    let reduced = bits.flag()?;
    if reduced {
        bits.skip(5)?;
    } else {
        let mut decoder_model = false;
        let mut buffer_delay_len = 0u32;
        if bits.flag()? {
            bits.skip(64)?;
            if bits.flag()? {
                uvlc(&mut bits)?;
            }
            decoder_model = bits.flag()?;
            if decoder_model {
                buffer_delay_len = bits.bits(5)? + 1;
                bits.skip(32 + 5 + 5)?;
            }
        }
        let initial_delay = bits.flag()?;
        let points = bits.bits(5)? + 1;
        for _ in 0..points {
            bits.skip(12)?;
            if bits.bits(5)? > 7 {
                bits.skip(1)?;
            }
            if decoder_model && bits.flag()? {
                bits.skip(buffer_delay_len * 2 + 1)?;
            }
            if initial_delay && bits.flag()? {
                bits.skip(4)?;
            }
        }
    }
    let width_bits = bits.bits(4)? + 1;
    let height_bits = bits.bits(4)? + 1;
    bits.skip(width_bits + height_bits)?;
    if !reduced && bits.flag()? {
        bits.skip(7)?;
    }
    bits.skip(3)?;
    if !reduced {
        bits.skip(4)?;
        let order_hint = bits.flag()?;
        if order_hint {
            bits.skip(2)?;
        }
        let force_screen_content = if bits.flag()? { 2 } else { bits.bits(1)? };
        if force_screen_content > 0 && !bits.flag()? {
            bits.skip(1)?;
        }
        if order_hint {
            bits.skip(3)?;
        }
    }
    bits.skip(3)?;
    // color_config (5.5.2)
    let high_bitdepth = bits.flag()?;
    if profile == 2 && high_bitdepth {
        bits.skip(1)?;
    }
    let mono = if profile == 1 { false } else { bits.flag()? };
    let described = bits.flag()?;
    let (primaries, transfer, matrix) = if described {
        (bits.bits(8)?, bits.bits(8)?, bits.bits(8)?)
    } else {
        (2, 2, 2)
    };
    let colour = if described {
        Some((
            u8::try_from(primaries).ok()?,
            u8::try_from(transfer).ok()?,
            u8::try_from(matrix).ok()?,
        ))
    } else {
        None
    };
    let full_range = if !mono && primaries == 1 && transfer == 13 && matrix == 0 {
        true
    } else {
        bits.flag()?
    };
    Some(Signal {
        full_range: Some(full_range),
        colour,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ue(bits: &mut Vec<u8>, value: u32) {
        let code = value + 1;
        let width = 32 - code.leading_zeros();
        bits.extend(std::iter::repeat_n(0, (width - 1) as usize));
        bits.extend((0..width).rev().map(|i| ((code >> i) & 1) as u8));
    }

    /// An `avcC` record of one sequence parameter set: `profile` (66 or
    /// 100, the latter with a scaling list), 320x240, `refs` reference
    /// frames and a VUI with a colour description when `colour`.
    fn avcc(profile: u8, refs: u32, colour: bool) -> Vec<u8> {
        avcc_timed(profile, refs, colour, None)
    }

    /// The same, with the timing information of E.2.1 when `timing` gives
    /// one (`num_units_in_tick`, `time_scale`).
    fn avcc_timed(profile: u8, refs: u32, colour: bool, timing: Option<(u32, u32)>) -> Vec<u8> {
        let mut bits = Vec::new();
        for value in [profile, 0, 30] {
            bits.extend((0..8).rev().map(|i| (value >> i) & 1));
        }
        ue(&mut bits, 0);
        if profile == 100 {
            // chroma_format_idc 1, 8-bit, no bypass, one 4x4 scaling list
            // of 16 delta 0 coefficients.
            for value in [1, 0, 0] {
                ue(&mut bits, value);
            }
            bits.extend([0, 1, 1]);
            for _ in 0..16 {
                ue(&mut bits, 0);
            }
            bits.extend([0; 7]);
        }
        // log2_max_frame_num - 4, pic_order_cnt_type 0 and its log2 - 4.
        for value in [0, 0, 0, refs] {
            ue(&mut bits, value);
        }
        bits.push(0);
        ue(&mut bits, 19);
        ue(&mut bits, 14);
        bits.extend([1, 1, 0]);
        if colour || timing.is_some() {
            // VUI: no aspect ratio or overscan.
            bits.extend([1, 0, 0]);
            if colour {
                // A full-range signal type with BT.709 code points.
                bits.extend([1, 1, 0, 1, 1, 1]);
                for value in [1u8, 1, 1] {
                    bits.extend((0..8).rev().map(|i| (value >> i) & 1));
                }
            } else {
                bits.push(0);
            }
            bits.push(0); // chroma_loc_info_present_flag
            if let Some((units, scale)) = timing {
                bits.push(1);
                for value in [units, scale] {
                    bits.extend((0..32).rev().map(|i| ((value >> i) & 1) as u8));
                }
                bits.push(1); // fixed_frame_rate_flag
            } else {
                bits.push(0);
            }
            // No HRD, no picture structure, no bitstream restriction.
            bits.extend([0, 0, 0, 0]);
        } else {
            bits.push(0);
        }
        bits.push(1);
        while bits.len() % 8 != 0 {
            bits.push(0);
        }
        let mut nal = vec![0x67];
        // With the emulation prevention of 7.4.1.1: a 32-bit clock value
        // such as 1001 writes zero bytes that the reader would otherwise
        // read as part of a start code.
        let mut zeros = 0;
        for byte in bits
            .chunks(8)
            .map(|c| c.iter().fold(0u8, |v, b| (v << 1) | b))
        {
            if zeros == 2 && byte <= 3 {
                nal.push(3);
                zeros = 0;
            }
            zeros = if byte == 0 { zeros + 1 } else { 0 };
            nal.push(byte);
        }
        let mut record = vec![1, profile, 0, 30, 0xFF, 0xE1];
        record.extend((nal.len() as u16).to_be_bytes());
        record.extend(nal);
        record
    }

    #[test]
    fn reference_frames_of_the_sequence_parameter_set() {
        for profile in [66, 100] {
            for refs in [0, 1, 4, 16] {
                assert_eq!(
                    avc_max_num_ref_frames(&avcc(profile, refs, false)),
                    Some(refs)
                );
            }
            // The signal type of the same records still reads.
            assert_eq!(
                avc_signal(&avcc(profile, 1, false)),
                Some(Signal::default())
            );
            let colour = avc_signal(&avcc(profile, 1, true)).unwrap();
            assert_eq!(
                (colour.full_range, colour.colour),
                (Some(true), Some((1, 1, 1)))
            );
            let record = avcc(profile, 4, true);
            for end in 0..record.len() {
                let _ = avc_max_num_ref_frames(&record[..end]);
                let _ = avc_signal(&record[..end]);
            }
        }
    }

    #[test]
    fn timing_information_of_the_sequence_parameter_set() {
        for profile in [66, 100] {
            // A stream that states no clock, with and without a colour
            // description before the place the clock would be.
            assert_eq!(avc_timing(&avcc(profile, 1, false)), None);
            assert_eq!(avc_timing(&avcc(profile, 1, true)), None);
            for colour in [false, true] {
                for timing in [(1001, 60_000), (1, 50), (1001, 48_000)] {
                    let record = avcc_timed(profile, 1, colour, Some(timing));
                    assert_eq!(avc_timing(&record), Some(timing));
                    // The fields before it still read.
                    assert_eq!(avc_max_num_ref_frames(&record), Some(1));
                    assert!(avc_signal(&record).is_some());
                    for end in 0..record.len() {
                        let _ = avc_timing(&record[..end]);
                    }
                }
            }
            // A clock of zero is no clock.
            assert_eq!(
                avc_timing(&avcc_timed(profile, 1, false, Some((0, 1)))),
                None
            );
            assert_eq!(
                avc_timing(&avcc_timed(profile, 1, false, Some((1, 0)))),
                None
            );
        }
    }

    #[test]
    fn emulation_prevention_is_removed() {
        assert_eq!(rbsp(&[0, 0, 3, 1, 0, 0, 3], 16), vec![0, 0, 1, 0, 0]);
    }

    #[test]
    fn every_prefix_of_a_record_is_safe() {
        let mut record = vec![0u8; 23];
        record[22] = 1;
        record.extend_from_slice(&[33, 0, 1, 0, 40]);
        record.extend((0..40u8).map(|i| i.wrapping_mul(37)));
        for end in 0..record.len() {
            let _ = hevc_full_range(&record[..end]);
            let _ = av1_full_range(&record[..end]);
            let _ = hevc_sps_profile(&record[..end]);
            let _ = av1_sequence_profile(&record[..end]);
            let _ = avc_sps_profile(&record[..end]);
        }
    }
}
