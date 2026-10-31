//! Picture sizes from parameter sets: the sequence parameter sets of H.264
//! and HEVC and the sequence header of AV1, read from the specifications
//! (ITU-T H.264 7.3.2.1.1 and 7.4.2.1.1; ITU-T H.265 7.3.2.2.1 and
//! 7.4.3.2.1; AV1 specification 5.5.1 and 6.4.1).
//!
//! Each function takes one NAL unit or OBU payload, so that the same code
//! reads the parameter sets of a codec configuration record and those found
//! in samples. Every read is bounded by the bytes given; a parameter set
//! that cannot be read gives `None`, never an error.

use crate::bitstream::{leb128, profile_tier_level, rbsp, uvlc};
use crate::bytes::{Bits, be_u16_at};

/// Most bytes of one parameter set that are read. Sizes come early in each
/// structure; scaling lists and the operating points are the only long
/// parts before them.
const MAX_PARAMETER_SET: usize = 4096;

/// A picture size in luma samples.
pub type Size = (u32, u32);

/// The larger of two sizes in each dimension: a bound for pictures of
/// either.
fn bound(a: Option<Size>, b: Size) -> Size {
    match a {
        None => b,
        Some((w, h)) => (w.max(b.0), h.max(b.1)),
    }
}

/// `SubWidthC` and `SubHeightC` for a `ChromaArrayType` (ITU-T H.264 table
/// 6-1, ITU-T H.265 table 6-1); 0 (monochrome or separate planes) is 1, 1.
fn chroma_subsampling(chroma_array_type: u32) -> (u64, u64) {
    match chroma_array_type {
        1 => (2, 2),
        2 => (2, 1),
        _ => (1, 1),
    }
}

/// Most luma samples of a right or bottom crop that the bound subtracts:
/// the padding to a whole macroblock (16) or coding tree block (at most
/// 64). Larger crops leave the coded size, which is what a decoder decodes.
const MAX_PADDING_CROP: u64 = 64;

/// Whether a crop or conformance window is valid as a whole: in both
/// dimensions, the samples it removes leave at least one. Decoders ignore an
/// invalid one entirely (`ffmpeg`, a black box here, decodes the coded size
/// in both dimensions when either is invalid).
fn crop_valid(
    (width, height): (u64, u64),
    (unit_x, unit_y): (u64, u64),
    (left, right, top, bottom): (u32, u32, u32, u32),
) -> bool {
    let fits = |full: u64, unit: u64, near: u32, far: u32| {
        u64::from(near)
            .checked_add(u64::from(far))
            .and_then(|n| n.checked_mul(unit))
            .is_some_and(|crop| crop < full)
    };
    fits(width, unit_x, left, right) && fits(height, unit_y, top, bottom)
}

/// The bound on one dimension of the decoded picture: the coded size, less
/// the far-side crop (right or bottom) when it is only padding and the crop
/// is valid as a whole ([`crop_valid`]). The near-side crop (left or top) is
/// never subtracted: decoders need not apply it exactly (`ffmpeg` rounds a
/// left crop down to an alignment), so the pictures they output can keep
/// those samples. `None` when the coded size is 0.
fn cropped(full: u64, unit: u64, far: u32, valid: bool) -> Option<u32> {
    let far_samples = unit.checked_mul(u64::from(far))?;
    let crop = if valid && far_samples < MAX_PADDING_CROP {
        far_samples
    } else {
        0
    };
    u32::try_from(full.checked_sub(crop)?)
        .ok()
        .filter(|size| *size > 0)
}

/// The picture a decoder puts out: the coded size less the whole
/// conformance crop, when the crop is valid ([`crop_valid`]); the coded
/// size when it is not, as a decoder that ignores an invalid crop gives.
/// Unlike [`cropped`] this is not a bound on what a decoder may output but
/// what the sequence says the picture is, which is the size a reader
/// reports for a stream whose container states none.
fn displayed(full: u64, unit: u64, near: u32, far: u32, valid: bool) -> Option<u32> {
    let crop = if valid {
        unit.checked_mul(u64::from(near).checked_add(u64::from(far))?)?
    } else {
        0
    };
    u32::try_from(full.checked_sub(crop)?)
        .ok()
        .filter(|size| *size > 0)
}

/// What a sequence parameter set says about the picture size: the coded
/// size, the crop units of its chroma format and the conformance crop
/// (left, right, top, bottom).
struct Geometry {
    size: (u64, u64),
    unit: (u64, u64),
    crop: (u32, u32, u32, u32),
}

impl Geometry {
    /// The bound on the picture size: the coded size less padding crops.
    fn bound(&self) -> Option<Size> {
        let valid = crop_valid(self.size, self.unit, self.crop);
        Some((
            cropped(self.size.0, self.unit.0, self.crop.1, valid)?,
            cropped(self.size.1, self.unit.1, self.crop.3, valid)?,
        ))
    }

    /// The picture the sequence describes: the coded size less the whole
    /// conformance crop.
    fn display(&self) -> Option<Size> {
        let valid = crop_valid(self.size, self.unit, self.crop);
        Some((
            displayed(self.size.0, self.unit.0, self.crop.0, self.crop.1, valid)?,
            displayed(self.size.1, self.unit.1, self.crop.2, self.crop.3, valid)?,
        ))
    }
}

/// The bound on the picture size an H.264 sequence parameter set NAL unit
/// (header byte included, `nal_unit_type` 7) defines: the coded size less
/// padding crops ([`cropped`]).
pub(crate) fn avc_sps_size(nal: &[u8]) -> Option<Size> {
    avc_sps_geometry(nal)?.bound()
}

/// The picture size an H.264 sequence parameter set describes: the coded
/// size less the whole conformance crop ([`Geometry::display`]).
pub(crate) fn avc_sps_display_size(nal: &[u8]) -> Option<Size> {
    avc_sps_geometry(nal)?.display()
}

fn avc_sps_geometry(nal: &[u8]) -> Option<Geometry> {
    if nal.first()? & 0x1f != 7 {
        return None;
    }
    let sps = rbsp(nal.get(1..)?, MAX_PARAMETER_SET);
    let mut bits = Bits::new(&sps);
    let profile_idc = bits.bits(8)?;
    bits.skip(16)?;
    if bits.ue()? > 31 {
        return None;
    }
    let mut chroma_array_type = 1u32;
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        let chroma_format_idc = bits.ue()?;
        if chroma_format_idc > 3 {
            return None;
        }
        chroma_array_type = chroma_format_idc;
        if chroma_format_idc == 3 && bits.flag()? {
            // separate_colour_plane_flag
            chroma_array_type = 0;
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
        2 => {}
        _ => return None,
    }
    bits.ue()?;
    bits.skip(1)?;
    let width_mbs = u64::from(bits.ue()?).checked_add(1)?;
    let height_map_units = u64::from(bits.ue()?).checked_add(1)?;
    let frame_mbs_only = bits.flag()?;
    if !frame_mbs_only {
        bits.skip(1)?;
    }
    bits.skip(1)?;
    let crop = if bits.flag()? {
        (bits.ue()?, bits.ue()?, bits.ue()?, bits.ue()?)
    } else {
        (0, 0, 0, 0)
    };
    // 7.4.2.1.1: frame height in macroblocks, crop units.
    let field_factor = if frame_mbs_only { 1 } else { 2 };
    let width = width_mbs.checked_mul(16)?;
    let height = height_map_units
        .checked_mul(16)?
        .checked_mul(field_factor)?;
    let (sub_width, sub_height) = chroma_subsampling(chroma_array_type);
    let (unit_x, unit_y) = if chroma_array_type == 0 {
        (1, field_factor)
    } else {
        (sub_width, sub_height.checked_mul(field_factor)?)
    };
    Some(Geometry {
        size: (width, height),
        unit: (unit_x, unit_y),
        crop,
    })
}

/// The bound on the picture size an HEVC sequence parameter set NAL unit
/// (two-byte header included, `nal_unit_type` 33) defines: the coded size
/// less padding offsets of the conformance window ([`cropped`]).
pub(crate) fn hevc_sps_size(nal: &[u8]) -> Option<Size> {
    hevc_sps_geometry(nal)?.bound()
}

/// The picture size an HEVC sequence parameter set describes: the coded
/// size less the whole conformance window ([`Geometry::display`]).
pub(crate) fn hevc_sps_display_size(nal: &[u8]) -> Option<Size> {
    hevc_sps_geometry(nal)?.display()
}

fn hevc_sps_geometry(nal: &[u8]) -> Option<Geometry> {
    if (nal.first()? >> 1) & 0x3f != 33 {
        return None;
    }
    let sps = rbsp(nal.get(2..)?, MAX_PARAMETER_SET);
    let mut bits = Bits::new(&sps);
    bits.skip(4)?;
    let max_sub_layers_minus1 = bits.bits(3)?;
    if max_sub_layers_minus1 > 6 {
        return None;
    }
    bits.skip(1)?;
    profile_tier_level(&mut bits, max_sub_layers_minus1)?;
    if bits.ue()? > 15 {
        return None;
    }
    let chroma_format_idc = bits.ue()?;
    if chroma_format_idc > 3 {
        return None;
    }
    let mut chroma_array_type = chroma_format_idc;
    if chroma_format_idc == 3 && bits.flag()? {
        chroma_array_type = 0;
    }
    let width = u64::from(bits.ue()?);
    let height = u64::from(bits.ue()?);
    let window = if bits.flag()? {
        (bits.ue()?, bits.ue()?, bits.ue()?, bits.ue()?)
    } else {
        (0, 0, 0, 0)
    };
    let (unit_x, unit_y) = chroma_subsampling(chroma_array_type);
    Some(Geometry {
        size: (width, height),
        unit: (unit_x, unit_y),
        crop: window,
    })
}

/// The largest frame size (`max_frame_width_minus_1 + 1`,
/// `max_frame_height_minus_1 + 1`) an AV1 sequence header OBU payload
/// allows; every frame of the coded video sequence is at most this large
/// (AV1 specification 6.8.2: `frame_width_minus_1` is at most
/// `max_frame_width_minus_1`, and the upscaled width likewise).
pub(crate) fn av1_sequence_size(payload: &[u8]) -> Option<Size> {
    let mut bits = Bits::new(payload);
    if bits.bits(3)? > 2 {
        return None;
    }
    bits.skip(1)?;
    let reduced = bits.flag()?;
    if reduced {
        bits.skip(5)?;
    } else {
        let mut decoder_model = false;
        let mut buffer_delay_len = 0u32;
        if bits.flag()? {
            // timing_info (5.5.3)
            bits.skip(64)?;
            if bits.flag()? {
                uvlc(&mut bits)?;
            }
            decoder_model = bits.flag()?;
            if decoder_model {
                // decoder_model_info (5.5.4)
                buffer_delay_len = bits.bits(5)?.checked_add(1)?;
                bits.skip(32 + 5 + 5)?;
            }
        }
        let initial_delay = bits.flag()?;
        let points = bits.bits(5)?.checked_add(1)?;
        for _ in 0..points {
            bits.skip(12)?;
            if bits.bits(5)? > 7 {
                bits.skip(1)?;
            }
            if decoder_model && bits.flag()? {
                // operating_parameters_info (5.5.5)
                bits.skip(buffer_delay_len.checked_mul(2)?.checked_add(1)?)?;
            }
            if initial_delay && bits.flag()? {
                bits.skip(4)?;
            }
        }
    }
    let width_bits = bits.bits(4)?.checked_add(1)?;
    let height_bits = bits.bits(4)?.checked_add(1)?;
    let width = bits.bits(width_bits)?.checked_add(1)?;
    let height = bits.bits(height_bits)?.checked_add(1)?;
    Some((width, height))
}

/// What a VP9 frame's uncompressed header (VP9 bitstream specification
/// 6.2) says about its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Vp9Size {
    /// The frame codes its size (`frame_size()`): key frames, intra-only
    /// frames and inter frames that find no reference of their size.
    Coded(Size),
    /// The size of a reference frame, itself coded earlier, or no new frame
    /// at all (`show_existing_frame`).
    Earlier,
}

/// `color_config()` (VP9 6.2.2): what it takes, for its length.
fn vp9_color_config(bits: &mut Bits<'_>, profile: u32) -> Option<()> {
    if profile >= 2 {
        bits.skip(1)?;
    }
    let color_space = bits.bits(3)?;
    if color_space != 7 {
        bits.skip(1)?;
        if profile == 1 || profile == 3 {
            bits.skip(3)?;
        }
    } else if profile == 1 || profile == 3 {
        bits.skip(1)?;
    }
    Some(())
}

/// `frame_size()` (VP9 6.2.5).
fn vp9_frame_size(bits: &mut Bits<'_>) -> Option<Size> {
    let width = bits.bits(16)?.checked_add(1)?;
    let height = bits.bits(16)?.checked_add(1)?;
    Some((width, height))
}

/// Reads a VP9 frame's uncompressed header far enough to know its size;
/// `None` when it is not a valid header (a wrong `frame_marker` or sync
/// code, or too few bytes).
pub(crate) fn vp9_size(frame: &[u8]) -> Option<Vp9Size> {
    let mut bits = Bits::new(frame);
    if bits.bits(2)? != 2 {
        return None;
    }
    let low = bits.bits(1)?;
    let high = bits.bits(1)?;
    let profile = (high << 1) | low;
    if profile == 3 && bits.bits(1)? != 0 {
        return None;
    }
    if bits.flag()? {
        // show_existing_frame
        return Some(Vp9Size::Earlier);
    }
    let key_frame = !bits.flag()?;
    let show_frame = bits.flag()?;
    let error_resilient = bits.flag()?;
    let sync = |bits: &mut Bits<'_>| -> Option<()> { (bits.bits(24)? == 0x49_8342).then_some(()) };
    if key_frame {
        sync(&mut bits)?;
        vp9_color_config(&mut bits, profile)?;
        return Some(Vp9Size::Coded(vp9_frame_size(&mut bits)?));
    }
    let intra_only = !show_frame && bits.flag()?;
    if !error_resilient {
        bits.skip(2)?;
    }
    if intra_only {
        sync(&mut bits)?;
        if profile > 0 {
            vp9_color_config(&mut bits, profile)?;
        }
        bits.skip(8)?;
        return Some(Vp9Size::Coded(vp9_frame_size(&mut bits)?));
    }
    // refresh_frame_flags, then ref_frame_idx and sign bias for 3 frames.
    bits.skip(8 + 3 * 4)?;
    // frame_size_with_refs (6.2.6)
    for _ in 0..3 {
        if bits.flag()? {
            return Some(Vp9Size::Earlier);
        }
    }
    Some(Vp9Size::Coded(vp9_frame_size(&mut bits)?))
}

/// The frames of a VP9 sample: one frame, or the frames a superframe index
/// at its end lists (VP9 annex B), as `(offset, length)` within the sample.
/// `index` holds the sample's last bytes (up to 34, the largest index);
/// `None` when an index is malformed or its sizes do not add up to the
/// sample.
pub(crate) fn vp9_frames(sample_len: u64, index: &[u8]) -> Option<Vec<(u64, u64)>> {
    let marker = *index.last()?;
    if marker & 0xe0 != 0xc0 {
        return Some(vec![(0, sample_len)]);
    }
    let frames = usize::from(marker & 0x07) + 1;
    let bytes = usize::from((marker >> 3) & 0x03) + 1;
    let index_len = 2 + frames * bytes;
    let start = index.len().checked_sub(index_len)?;
    if *index.get(start)? != marker || u64::try_from(index_len).ok()? > sample_len {
        // Not an index: the last byte is frame data that looks like one.
        return Some(vec![(0, sample_len)]);
    }
    let mut out = Vec::with_capacity(frames);
    let mut at = 0u64;
    for i in 0..frames {
        let mut size = 0u64;
        for b in 0..bytes {
            let byte = *index.get(start + 1 + i * bytes + b)?;
            size |= u64::from(byte) << (8 * b);
        }
        out.push((at, size));
        at = at.checked_add(size)?;
    }
    let data_len = sample_len.checked_sub(u64::try_from(index_len).ok()?)?;
    (at <= data_len).then_some(out)
}

/// The size in a ProRes frame header (SMPTE RDD 36 5.1 and 5.2): the frame
/// starts with its size and the identifier `icpf`, then the frame header,
/// whose `horizontal_size` and `vertical_size` sit at bytes 16 to 19.
pub(crate) fn prores_size(frame: &[u8]) -> Option<Size> {
    if frame.get(4..8)? != b"icpf" {
        return None;
    }
    let width = u32::from(be_u16_at(frame, 16)?);
    let height = u32::from(be_u16_at(frame, 18)?);
    (width > 0 && height > 0).then_some((width, height))
}

/// The size in a VP8 frame (RFC 6386 9.1): `Some(Some(size))` for a key
/// frame, whose 3-byte frame tag (bit 0 clear) is followed by the start
/// code `9d 01 2a` and the 14-bit width and height (the 2 scale bits above
/// each ask for upscaling after decoding and do not change the decoded
/// size); `Some(None)` for an inter frame, which takes the size of the key
/// frame before it, or an empty frame (nothing to decode); `None` when a
/// key frame's header is cut short or lacks its start code.
pub(crate) fn vp8_size(frame: &[u8]) -> Option<Option<Size>> {
    let Some(&tag) = frame.first() else {
        return Some(None);
    };
    if frame.len() < 3 {
        return None;
    }
    if tag & 1 != 0 {
        return Some(None);
    }
    let [_, _, _, 0x9d, 0x01, 0x2a, w0, w1, h0, h1, ..] = *frame else {
        return None;
    };
    let width = u32::from(u16::from_le_bytes([w0, w1]) & 0x3fff);
    let height = u32::from(u16::from_le_bytes([h0, h1]) & 0x3fff);
    Some(Some((width, height)))
}

/// The bound of the picture sizes of every parameter set in a codec
/// configuration record: every SPS of an `avcC` (ISO/IEC 14496-15 5.3.3.1)
/// or `hvcC` (8.3.3.1) record, or the sequence header of an `av1C` record
/// (AV1 ISOBMFF binding 2.3). `None` when the record holds none, or when one
/// of them cannot be read: a decoder may use any of them.
pub(crate) fn configured_size(kind: &[u8; 4], record: &[u8]) -> Option<Size> {
    configured(kind, record).ok().flatten()
}

/// The picture size the first sequence parameter set of an `avcC` or
/// `hvcC` record describes: its coded size less the whole conformance
/// crop. This is what the sequence says the picture is, for a stream whose
/// container states no size (a transport stream); [`configured_size`] is a
/// different thing, a bound over every parameter set of the record on what
/// a decoder may put out.
pub(crate) fn configured_display_size(kind: &[u8; 4], record: &[u8]) -> Option<Size> {
    match kind {
        b"avcC" => first_parameter_set(record, 5, 0x1f, 6).and_then(avc_sps_display_size),
        b"hvcC" => hvcc_first_sps(record).and_then(hevc_sps_display_size),
        _ => None,
    }
}

/// The first parameter set of an `avcC` record: `count_at` holds the count
/// under `mask`, and the sets follow at `from`, each a 16-bit length and
/// its bytes.
fn first_parameter_set(record: &[u8], count_at: usize, mask: u8, from: usize) -> Option<&[u8]> {
    if record.first() != Some(&1) || record.get(count_at)? & mask == 0 {
        return None;
    }
    let len = usize::from(be_u16_at(record, from)?);
    let start = from.checked_add(2)?;
    record.get(start..start.checked_add(len)?)
}

/// The first sequence parameter set of an `hvcC` record (`nal_unit_type`
/// 33), whose arrays each hold one kind of parameter set.
fn hvcc_first_sps(record: &[u8]) -> Option<&[u8]> {
    if record.first() != Some(&1) {
        return None;
    }
    let arrays = *record.get(22)?;
    let mut at = 23usize;
    for _ in 0..arrays {
        let nal_type = *record.get(at)? & 0x3f;
        let count = be_u16_at(record, at.checked_add(1)?)?;
        at = at.checked_add(3)?;
        for _ in 0..count {
            let len = usize::from(be_u16_at(record, at)?);
            let start = at.checked_add(2)?;
            let end = start.checked_add(len)?;
            if nal_type == 33 {
                return record.get(start..end);
            }
            at = end;
        }
    }
    None
}

/// Whether the first sequence parameter set of an `hvcC` record has a
/// conformance window that leaves no picture: ITU-T H.265 7.4.3.2.1 requires
/// its left and right offsets to leave part of the width and its top and
/// bottom offsets part of the height ([`crop_valid`]). `false` when the
/// record holds no such set or it cannot be read.
pub(crate) fn hvcc_window_empty(record: &[u8]) -> bool {
    hvcc_first_sps(record)
        .and_then(hevc_sps_geometry)
        .is_some_and(|g| !crop_valid(g.size, g.unit, g.crop))
}

/// [`configured_size`], telling a record without parameter sets
/// (`Ok(None)`) from one this crate cannot read (`Err`): a version other
/// than 1 (`avcC`, `hvcC`; a decoder may then frame samples differently),
/// a structure that runs past the record, or a parameter set that does not
/// parse.
pub(crate) fn configured(kind: &[u8; 4], record: &[u8]) -> Result<Option<Size>, ()> {
    match kind {
        b"avcC" => avcc_size(record),
        b"hvcC" => hvcc_size(record),
        b"av1C" => av1c_size(record),
        _ => Ok(None),
    }
}

fn avcc_size(avcc: &[u8]) -> Result<Option<Size>, ()> {
    if avcc.first() != Some(&1) {
        return Err(());
    }
    let count = *avcc.get(5).ok_or(())? & 0x1f;
    let mut at = 6usize;
    let mut size = None;
    for _ in 0..count {
        let len = usize::from(be_u16_at(avcc, at).ok_or(())?);
        let start = at.checked_add(2).ok_or(())?;
        let end = start.checked_add(len).ok_or(())?;
        size = Some(bound(
            size,
            avc_sps_size(avcc.get(start..end).ok_or(())?).ok_or(())?,
        ));
        at = end;
    }
    Ok(size)
}

fn hvcc_size(hvcc: &[u8]) -> Result<Option<Size>, ()> {
    if hvcc.first() != Some(&1) {
        return Err(());
    }
    let arrays = *hvcc.get(22).ok_or(())?;
    let mut at = 23usize;
    let mut size = None;
    for _ in 0..arrays {
        let nal_type = *hvcc.get(at).ok_or(())? & 0x3f;
        let count = be_u16_at(hvcc, at.checked_add(1).ok_or(())?).ok_or(())?;
        at = at.checked_add(3).ok_or(())?;
        for _ in 0..count {
            let len = usize::from(be_u16_at(hvcc, at).ok_or(())?);
            let start = at.checked_add(2).ok_or(())?;
            let end = start.checked_add(len).ok_or(())?;
            let nal = hvcc.get(start..end).ok_or(())?;
            if nal_type == 33 {
                size = Some(bound(size, hevc_sps_size(nal).ok_or(())?));
            }
            at = end;
        }
    }
    Ok(size)
}

fn av1c_size(av1c: &[u8]) -> Result<Option<Size>, ()> {
    if av1c.len() < 4 {
        return Err(());
    }
    if av1c.len() == 4 {
        return Ok(None);
    }
    match av1c_sequence_header(av1c) {
        Some(payload) => av1_sequence_size(payload).map(Some).ok_or(()),
        // OBUs other than a sequence header (metadata): no size from them.
        None if av1c.get(4).is_some_and(|h| (h >> 3) & 0x0f != 1) => Ok(None),
        None => Err(()),
    }
}

/// The payload of the sequence header OBU in an `av1C` record: at most one,
/// and the first OBU when present (AV1 ISOBMFF binding 2.3.4).
fn av1c_sequence_header(av1c: &[u8]) -> Option<&[u8]> {
    let mut at = 4usize;
    let header = *av1c.get(at)?;
    if (header >> 3) & 0x0f != 1 {
        return None;
    }
    let extension = header & 0x04 != 0;
    let has_size = header & 0x02 != 0;
    at = at.checked_add(1 + usize::from(extension))?;
    let size = if has_size {
        usize::try_from(leb128(av1c, &mut at)?).ok()?
    } else {
        av1c.len().checked_sub(at)?
    };
    av1c.get(at..at.checked_add(size)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Packs `(value, width)` fields most significant bit first; `ue` and
    /// `se` values are written as Exp-Golomb codes.
    #[derive(Clone, Copy)]
    enum F {
        U(u64, u32),
        Ue(u32),
    }

    fn pack(fields: &[F]) -> Vec<u8> {
        let mut bits: Vec<bool> = Vec::new();
        let mut put = |value: u64, width: u32| {
            for i in (0..width).rev() {
                bits.push(value >> i & 1 == 1);
            }
        };
        for field in fields {
            match *field {
                F::U(value, width) => put(value, width),
                F::Ue(value) => {
                    let code = u64::from(value) + 1;
                    let len = 64 - code.leading_zeros();
                    put(0, len - 1);
                    put(code, len);
                }
            }
        }
        // rbsp_stop_one_bit and alignment.
        bits.push(true);
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        bits.chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |b, (i, bit)| b | (u8::from(*bit) << (7 - i)))
            })
            .collect()
    }

    /// An H.264 SPS NAL unit: Main profile (no chroma fields), 4:2:0,
    /// `width_mbs` × `height_units`, with optional cropping.
    fn avc_sps(
        width_mbs: u32,
        height_units: u32,
        frame_mbs_only: bool,
        crop: Option<[u32; 4]>,
    ) -> Vec<u8> {
        let mut fields = vec![
            F::U(77, 8),
            F::U(0, 8),
            F::U(40, 8),
            F::Ue(0),
            F::Ue(0),
            F::Ue(2),
            F::Ue(1),
            F::U(0, 1),
            F::Ue(width_mbs - 1),
            F::Ue(height_units - 1),
            F::U(u64::from(frame_mbs_only), 1),
        ];
        if !frame_mbs_only {
            fields.push(F::U(0, 1));
        }
        fields.push(F::U(1, 1));
        match crop {
            Some(c) => {
                fields.push(F::U(1, 1));
                fields.extend(c.iter().map(|v| F::Ue(*v)));
            }
            None => fields.push(F::U(0, 1)),
        }
        fields.push(F::U(0, 1));
        let mut nal = vec![0x67];
        nal.extend(pack(&fields));
        nal
    }

    #[test]
    fn h264_sizes() {
        // 1920×1080: 120×68 macroblocks cropped by 8 lines (4 crop units).
        assert_eq!(
            avc_sps_size(&avc_sps(120, 68, true, Some([0, 0, 0, 4]))),
            Some((1920, 1080))
        );
        assert_eq!(avc_sps_size(&avc_sps(20, 15, true, None)), Some((320, 240)));
        // Field coding: map units are pairs of macroblock rows.
        assert_eq!(
            avc_sps_size(&avc_sps(45, 18, false, None)),
            Some((720, 576))
        );
        // A crop that leaves nothing is invalid as a whole: the coded size.
        assert_eq!(
            avc_sps_size(&avc_sps(1, 1, true, Some([0, 8, 0, 0]))),
            Some((16, 16))
        );
        assert_eq!(
            avc_sps_size(&avc_sps(80, 45, true, Some([620, 20, 0, 0]))),
            Some((1280, 720))
        );
        // Not an SPS; truncated.
        assert_eq!(avc_sps_size(&[0x68, 0xce]), None);
        let full = avc_sps(20, 15, true, None);
        for end in 0..full.len() - 1 {
            assert_eq!(avc_sps_size(&full[..end]), None, "{end}");
        }
    }

    #[test]
    fn display_sizes_take_the_whole_conformance_crop() {
        // 1280x720 coded 1920x1088 and cropped, as broadcast H.264 is: the
        // bound keeps the coded size (the crop is more than padding), the
        // display size is what the sequence says the picture is.
        let sps = avc_sps(120, 68, true, Some([0, 320, 0, 184]));
        assert_eq!(avc_sps_size(&sps), Some((1920, 1088)));
        assert_eq!(avc_sps_display_size(&sps), Some((1280, 720)));
        // A near-side crop counts too, where the bound never takes it.
        let near = avc_sps(80, 45, true, Some([600, 20, 0, 0]));
        assert_eq!(avc_sps_size(&near), Some((1240, 720)));
        assert_eq!(avc_sps_display_size(&near), Some((40, 720)));
        // A crop that leaves nothing is ignored whole, as by a decoder.
        let invalid = avc_sps(1, 1, true, Some([0, 8, 0, 0]));
        assert_eq!(avc_sps_display_size(&invalid), Some((16, 16)));
        // Padding only: the two agree.
        let padded = avc_sps(120, 68, true, Some([0, 0, 0, 4]));
        assert_eq!(avc_sps_display_size(&padded), Some((1920, 1080)));
        // Through a configuration record, from its first parameter set.
        let mut avcc = vec![1, 77, 0, 40, 0xFF, 0xE1];
        avcc.extend(u16::try_from(sps.len()).unwrap().to_be_bytes());
        avcc.extend(&sps);
        assert_eq!(configured_display_size(b"avcC", &avcc), Some((1280, 720)));
        assert_eq!(configured_size(b"avcC", &avcc), Some((1920, 1088)));
        assert_eq!(configured_display_size(b"avcC", &avcc[..6]), None);
        let window = hevc_sps(1920, 1088, Some([0, 0, 0, 4]));
        let mut hvcc = vec![1u8; 23];
        hvcc[0] = 1;
        hvcc[22] = 1;
        hvcc.push(33);
        hvcc.extend(1u16.to_be_bytes());
        hvcc.extend(u16::try_from(window.len()).unwrap().to_be_bytes());
        hvcc.extend(&window);
        assert_eq!(configured_display_size(b"hvcC", &hvcc), Some((1920, 1080)));
    }

    #[test]
    fn h264_emulation_prevention_is_removed() {
        // A width of 4096 macroblocks writes many zero bits: the encoder
        // would insert 0x03 after two zero bytes.
        let nal = avc_sps(4096, 15, true, None);
        let mut escaped = Vec::new();
        let mut zeros = 0;
        for &byte in &nal {
            if zeros >= 2 && byte <= 3 {
                escaped.push(3);
                zeros = 0;
            }
            zeros = if byte == 0 { zeros + 1 } else { 0 };
            escaped.push(byte);
        }
        assert_eq!(avc_sps_size(&escaped), Some((65_536, 240)));
    }

    fn hevc_sps(width: u32, height: u32, window: Option<[u32; 4]>) -> Vec<u8> {
        let mut fields = vec![F::U(0, 4), F::U(0, 3), F::U(1, 1)];
        // profile_tier_level with no sub-layers: 96 bits.
        fields.push(F::U(0, 32));
        fields.push(F::U(0, 32));
        fields.push(F::U(0, 32));
        fields.extend([F::Ue(0), F::Ue(1), F::Ue(width), F::Ue(height)]);
        match window {
            Some(w) => {
                fields.push(F::U(1, 1));
                fields.extend(w.iter().map(|v| F::Ue(*v)));
            }
            None => fields.push(F::U(0, 1)),
        }
        let mut nal = vec![33 << 1, 1];
        nal.extend(pack(&fields));
        nal
    }

    #[test]
    fn hevc_sizes() {
        // 1920×1088 coded, the window removes 4 chroma rows (8 luma rows).
        assert_eq!(
            hevc_sps_size(&hevc_sps(1920, 1088, Some([0, 0, 0, 4]))),
            Some((1920, 1080))
        );
        assert_eq!(
            hevc_sps_size(&hevc_sps(7680, 4320, None)),
            Some((7680, 4320))
        );
        assert_eq!(hevc_sps_size(&hevc_sps(0, 4320, None)), None);
        // An invalid window (left + right at least the width) is ignored
        // as a whole, the far crop included.
        assert_eq!(
            hevc_sps_size(&hevc_sps(1976, 1080, Some([980, 28, 0, 0]))),
            Some((1976, 1080))
        );
        assert_eq!(
            hevc_sps_size(&hevc_sps(1280, 720, Some([0, 0, 350, 15]))),
            Some((1280, 720))
        );
        assert_eq!(hevc_sps_size(&[34 << 1, 1, 0]), None);
    }

    fn av1_header(width: u32, height: u32) -> Vec<u8> {
        pack(&[
            F::U(0, 3),
            F::U(0, 1),
            F::U(0, 1),
            F::U(0, 1),
            F::U(0, 1),
            F::U(0, 5),
            F::U(0, 12),
            F::U(8, 5),
            F::U(0, 1),
            F::U(15, 4),
            F::U(15, 4),
            F::U(u64::from(width - 1), 16),
            F::U(u64::from(height - 1), 16),
        ])
    }

    #[test]
    fn av1_sizes() {
        assert_eq!(
            av1_sequence_size(&av1_header(7680, 4320)),
            Some((7680, 4320))
        );
        assert_eq!(av1_sequence_size(&av1_header(320, 240)), Some((320, 240)));
        // seq_profile above 2 is reserved.
        assert_eq!(av1_sequence_size(&[0xe0]), None);
        // In an av1C record: the sequence header is the first OBU.
        let payload = av1_header(1280, 720);
        let mut av1c = vec![0x81, 0, 0x0c, 0];
        av1c.push(1 << 3 | 0x02);
        av1c.push(u8::try_from(payload.len()).unwrap());
        av1c.extend(&payload);
        assert_eq!(configured_size(b"av1C", &av1c), Some((1280, 720)));
        assert_eq!(configured_size(b"av1C", &av1c[..4]), None);
    }

    fn vp9_key(width: u32, height: u32) -> Vec<u8> {
        pack(&[
            F::U(2, 2),
            F::U(0, 1),
            F::U(0, 1),
            F::U(0, 1),
            F::U(0, 1),
            F::U(1, 1),
            F::U(0, 1),
            F::U(0x49_8342, 24),
            F::U(1, 3),
            F::U(0, 1),
            F::U(u64::from(width - 1), 16),
            F::U(u64::from(height - 1), 16),
        ])
    }

    #[test]
    fn vp9_frames_and_sizes() {
        assert_eq!(
            vp9_size(&vp9_key(1280, 720)),
            Some(Vp9Size::Coded((1280, 720)))
        );
        // An inter frame that takes its size from the first reference.
        let inter = pack(&[
            F::U(2, 2),
            F::U(0, 2),
            F::U(0, 1),
            F::U(1, 1),
            F::U(1, 1),
            F::U(0, 1),
            F::U(0, 2),
            F::U(0, 8),
            F::U(0, 12),
            F::U(1, 1),
        ]);
        assert_eq!(vp9_size(&inter), Some(Vp9Size::Earlier));
        // An inter frame that finds no reference codes its size.
        let resized = pack(&[
            F::U(2, 2),
            F::U(0, 2),
            F::U(0, 1),
            F::U(1, 1),
            F::U(1, 1),
            F::U(0, 1),
            F::U(0, 2),
            F::U(0, 8),
            F::U(0, 12),
            F::U(0, 3),
            F::U(7679, 16),
            F::U(4319, 16),
        ]);
        assert_eq!(vp9_size(&resized), Some(Vp9Size::Coded((7680, 4320))));
        // show_existing_frame
        assert_eq!(
            vp9_size(&pack(&[F::U(2, 2), F::U(0, 2), F::U(1, 1), F::U(0, 3)])),
            Some(Vp9Size::Earlier)
        );
        // Wrong marker or sync code.
        assert_eq!(vp9_size(&[0x00, 0, 0, 0]), None);
        let mut bad = vp9_key(64, 64);
        bad[1] ^= 0x10;
        assert_eq!(vp9_size(&bad), None);
        // Superframe index: two frames of 10 and 20 bytes, 1 byte sizes.
        let marker = 0xc0 | 1;
        let index = [marker, 10, 20, marker];
        assert_eq!(vp9_frames(34, &index), Some(vec![(0, 10), (10, 20)]));
        // Sizes past the data.
        assert_eq!(vp9_frames(30, &[marker, 10, 20, marker]), None);
        // No index.
        assert_eq!(vp9_frames(50, &[1, 2, 3]), Some(vec![(0, 50)]));
    }

    #[test]
    fn vp8_sizes() {
        // Key frame, 1920x1080 with scale bits set on the width.
        let key = [0x50, 0x2a, 0x00, 0x9d, 0x01, 0x2a, 0x80, 0x47, 0x38, 0x04];
        assert_eq!(vp8_size(&key), Some(Some((1920, 1080))));
        // Inter frame and empty frame: no size.
        assert_eq!(vp8_size(&[0x51, 0x2a, 0x00, 0xff]), Some(None));
        assert_eq!(vp8_size(&[]), Some(None));
        // A key frame without its start code, or cut short.
        assert_eq!(
            vp8_size(&[0x50, 0x2a, 0x00, 0x9d, 0x01, 0x2b, 0, 0, 0, 0]),
            None
        );
        for len in 1..key.len() {
            assert_eq!(vp8_size(&key[..len]), None, "{len}");
        }
    }

    #[test]
    fn prores_sizes() {
        let mut frame = vec![0, 0, 0x10, 0, b'i', b'c', b'p', b'f', 0, 148, 0, 0];
        frame.extend(b"apl0");
        frame.extend(1920u16.to_be_bytes());
        frame.extend(1080u16.to_be_bytes());
        assert_eq!(prores_size(&frame), Some((1920, 1080)));
        frame[4] = b'x';
        assert_eq!(prores_size(&frame), None);
        assert_eq!(prores_size(&frame[..10]), None);
    }

    #[test]
    fn records_bound_every_parameter_set() {
        let small = avc_sps(20, 15, true, None);
        let large = avc_sps(480, 270, true, None);
        let mut avcc = vec![1, 77, 0, 40, 0xff, 0xe2];
        for sps in [&small, &large] {
            avcc.extend(u16::try_from(sps.len()).unwrap().to_be_bytes());
            avcc.extend(sps.iter());
        }
        avcc.push(0);
        assert_eq!(configured_size(b"avcC", &avcc), Some((7680, 4320)));
        // A record without parameter sets, or with one that cannot be read.
        assert_eq!(
            configured_size(b"avcC", &[1, 77, 0, 40, 0xff, 0xe0, 0]),
            None
        );
        let mut broken = vec![1, 77, 0, 40, 0xff, 0xe2];
        broken.extend(u16::try_from(small.len()).unwrap().to_be_bytes());
        broken.extend(&small);
        broken.extend([0, 2, 0x67, 0x4d]);
        assert_eq!(configured_size(b"avcC", &broken), None);
        assert_eq!(configured(b"avcC", &broken), Err(()));
        assert_eq!(
            configured(b"avcC", &[1, 77, 0, 40, 0xff, 0xe0, 0]),
            Ok(None)
        );
        // A version other than 1.
        let mut version0 = avcc.clone();
        version0[0] = 0;
        assert_eq!(configured(b"avcC", &version0), Err(()));
        // hvcC: 22 header bytes, then one array of SPS NAL units.
        let sps = hevc_sps(3840, 2160, None);
        let mut hvcc = vec![0u8; 22];
        hvcc[0] = 1;
        hvcc.extend([1, 0x80 | 33, 0, 1]);
        hvcc.extend(u16::try_from(sps.len()).unwrap().to_be_bytes());
        hvcc.extend(&sps);
        assert_eq!(configured_size(b"hvcC", &hvcc), Some((3840, 2160)));
        assert_eq!(configured_size(b"vpcC", &hvcc), None);
    }
}
