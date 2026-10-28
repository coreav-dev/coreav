//! Sequence and picture parameter sets (clauses 7.3.2.1, 7.3.2.2, 7.4.2.1
//! and 7.4.2.2), and the scaling matrices they select (clause 8.5.9).
#![forbid(unsafe_code)]

use crate::bits::BitReader;
use crate::error::{Error, Result, require};
use crate::tables as t;

/// Sequence parameter sets a stream may hold (clause 7.4.2.1.1).
pub(crate) const MAX_SPS: usize = 32;
/// Picture parameter sets a stream may hold (clause 7.4.2.2).
pub(crate) const MAX_PPS: usize = 256;

/// The parts of the video usability information the decoder uses: the two
/// hints that bound the decoded picture buffer (Annex E.2.1).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Vui {
    pub(crate) max_num_reorder_frames: Option<u32>,
    pub(crate) max_dec_frame_buffering: Option<u32>,
}

/// A sequence parameter set.
#[derive(Clone, Debug)]
pub(crate) struct Sps {
    pub(crate) profile_idc: u8,
    pub(crate) constraint_flags: u8,
    pub(crate) level_idc: u8,
    pub(crate) id: u32,
    pub(crate) chroma_format_idc: u32,
    pub(crate) separate_colour_plane: bool,
    pub(crate) bit_depth_luma: u32,
    pub(crate) bit_depth_chroma: u32,
    pub(crate) transform_bypass: bool,
    /// The sequence-level scaling lists in zig-zag order, present only when
    /// seq_scaling_matrix_present_flag is 1.
    pub(crate) scaling4x4: Option<[[u8; 16]; 6]>,
    pub(crate) scaling8x8: Option<[[u8; 64]; 6]>,
    pub(crate) log2_max_frame_num: u32,
    pub(crate) poc_type: u32,
    pub(crate) log2_max_poc_lsb: u32,
    pub(crate) delta_pic_order_always_zero: bool,
    pub(crate) offset_for_non_ref_pic: i32,
    pub(crate) offset_for_top_to_bottom_field: i32,
    pub(crate) offset_for_ref_frame: Vec<i32>,
    pub(crate) max_num_ref_frames: u32,
    pub(crate) gaps_in_frame_num_allowed: bool,
    pub(crate) width_mbs: u32,
    pub(crate) height_map_units: u32,
    pub(crate) frame_mbs_only: bool,
    pub(crate) mb_adaptive_frame_field: bool,
    pub(crate) direct_8x8_inference: bool,
    /// The frame cropping rectangle in luma samples: left, right, top,
    /// bottom.
    pub(crate) crop: [u32; 4],
    pub(crate) vui: Vui,
}

impl Sps {
    /// PicHeightInMbs for a frame (Equation 7-18 with field_pic_flag 0).
    pub(crate) fn frame_height_mbs(&self) -> u32 {
        (2 - u32::from(self.frame_mbs_only)) * self.height_map_units
    }

    /// MaxFrameNum (Equation 7-10).
    pub(crate) fn max_frame_num(&self) -> u32 {
        1 << self.log2_max_frame_num
    }

    /// ChromaArrayType (clause 7.4.2.1.1).
    pub(crate) fn chroma_array_type(&self) -> u32 {
        if self.separate_colour_plane {
            0
        } else {
            self.chroma_format_idc
        }
    }

    /// constraint_set`i`_flag.
    pub(crate) fn constraint_set(&self, i: u32) -> bool {
        self.constraint_flags & (0x80u8 >> i.min(7)) != 0
    }
}

/// One scaling list (clause 7.3.2.1.1.1). Returns whether the list falls
/// back to its default (useDefaultScalingMatrixFlag).
fn scaling_list(r: &mut BitReader, list: &mut [u8]) -> Result<bool> {
    let mut last: i32 = 8;
    let mut next: i32 = 8;
    let mut use_default = false;
    for (j, entry) in list.iter_mut().enumerate() {
        if next != 0 {
            let delta = r.se()?;
            require((-128..=127).contains(&delta), "delta_scale")?;
            next = (last + delta + 256) % 256;
            use_default = j == 0 && next == 0;
        }
        *entry = if next == 0 { last as u8 } else { next as u8 };
        last = i32::from(*entry);
    }
    Ok(use_default)
}

/// The default list of ScalingList4x4[i] (Table 7-2): intra for the first
/// three lists, inter for the rest.
fn default4x4(i: usize) -> [u8; 16] {
    if i < 3 {
        t::DEFAULT_4X4_INTRA
    } else {
        t::DEFAULT_4X4_INTER
    }
}

/// The default list of ScalingList8x8[i] (Table 7-2).
fn default8x8(i: usize) -> [u8; 64] {
    if i % 2 == 0 {
        t::DEFAULT_8X8_INTRA
    } else {
        t::DEFAULT_8X8_INTER
    }
}

/// Six 4x4 and six 8x8 scaling lists, in zig-zag order.
type ScalingLists = ([[u8; 16]; 6], [[u8; 64]; 6]);

/// Scaling lists with the fall-back rule of Table 7-2. `first4x4` and
/// `first8x8` are what a missing first list of each kind falls back to: the
/// default lists for rule A, the sequence-level lists for rule B.
fn scaling_lists(
    r: &mut BitReader,
    lists_8x8: usize,
    first4x4: [[u8; 16]; 2],
    first8x8: [[u8; 64]; 2],
) -> Result<ScalingLists> {
    let mut l4 = [[16u8; 16]; 6];
    let mut l8 = [[16u8; 64]; 6];
    let [intra4, inter4] = first4x4;
    let [intra8, inter8] = first8x8;
    // The fall-back of list i is list i - 1 for 4x4 and list i - 2 for 8x8,
    // carried along instead of indexed back.
    let mut prev4 = intra4;
    for (i, list) in l4.iter_mut().enumerate() {
        if r.flag()? {
            if scaling_list(r, list)? {
                *list = default4x4(i);
            }
        } else {
            *list = match i {
                0 => intra4,
                3 => inter4,
                _ => prev4,
            };
        }
        prev4 = *list;
    }
    let mut prev8 = [intra8, inter8];
    for (i, list) in l8.iter_mut().enumerate() {
        let present = i < lists_8x8 && r.flag()?;
        if present {
            if scaling_list(r, list)? {
                *list = default8x8(i);
            }
        } else {
            *list = match i {
                0 => intra8,
                1 => inter8,
                _ => prev8.get(i % 2).copied().unwrap_or(intra8),
            };
        }
        if let Some(slot) = prev8.get_mut(i % 2) {
            *slot = *list;
        }
    }
    Ok((l4, l8))
}

/// hrd_parameters() (Annex E.1.2), which the decoder only skips over.
fn skip_hrd(r: &mut BitReader) -> Result<()> {
    let cpb_cnt = r.ue()?.saturating_add(1);
    require(cpb_cnt <= 32, "cpb_cnt_minus1")?;
    r.u(8)?;
    for _ in 0..cpb_cnt {
        r.ue()?;
        r.ue()?;
        r.u(1)?;
    }
    r.u(20)?;
    Ok(())
}

/// vui_parameters() (Annex E.1.1). Only the bitstream restriction's two
/// buffering hints are kept.
fn parse_vui(r: &mut BitReader) -> Result<Vui> {
    let mut v = Vui::default();
    if r.flag()? {
        // aspect_ratio_idc, and Extended_SAR's two 16-bit fields.
        if r.u(8)? == 255 {
            r.u(32)?;
        }
    }
    if r.flag()? {
        r.u(1)?;
    }
    if r.flag()? {
        // video_format, video_full_range_flag, then the colour description.
        r.u(4)?;
        if r.flag()? {
            r.u(24)?;
        }
    }
    if r.flag()? {
        r.ue()?;
        r.ue()?;
    }
    if r.flag()? {
        r.u(32)?;
        r.u(32)?;
        r.u(1)?;
    }
    let nal_hrd = r.flag()?;
    if nal_hrd {
        skip_hrd(r)?;
    }
    let vcl_hrd = r.flag()?;
    if vcl_hrd {
        skip_hrd(r)?;
    }
    if nal_hrd || vcl_hrd {
        r.u(1)?;
    }
    r.u(1)?;
    if r.flag()? {
        // bitstream_restriction_flag.
        r.u(1)?;
        r.ue()?;
        r.ue()?;
        r.ue()?;
        r.ue()?;
        v.max_num_reorder_frames = Some(r.ue()?);
        v.max_dec_frame_buffering = Some(r.ue()?);
    }
    Ok(v)
}

/// Profiles whose sequence parameter set carries the chroma format, the bit
/// depths and the scaling matrices (clause 7.3.2.1.1).
fn has_chroma_format(profile_idc: u8) -> bool {
    matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    )
}

/// seq_parameter_set_data() (clause 7.3.2.1).
pub(crate) fn parse_sps(rbsp: &[u8]) -> Result<Sps> {
    let mut r = BitReader::new(rbsp);
    let profile_idc = r.u(8)? as u8;
    let constraint_flags = r.u(8)? as u8;
    let level_idc = r.u(8)? as u8;
    let id = r.ue_max(MAX_SPS as u32 - 1, "seq_parameter_set_id")?;
    let mut sps = Sps {
        profile_idc,
        constraint_flags,
        level_idc,
        id,
        chroma_format_idc: 1,
        separate_colour_plane: false,
        bit_depth_luma: 8,
        bit_depth_chroma: 8,
        transform_bypass: false,
        scaling4x4: None,
        scaling8x8: None,
        log2_max_frame_num: 4,
        poc_type: 0,
        log2_max_poc_lsb: 4,
        delta_pic_order_always_zero: false,
        offset_for_non_ref_pic: 0,
        offset_for_top_to_bottom_field: 0,
        offset_for_ref_frame: Vec::new(),
        max_num_ref_frames: 0,
        gaps_in_frame_num_allowed: false,
        width_mbs: 0,
        height_map_units: 0,
        frame_mbs_only: true,
        mb_adaptive_frame_field: false,
        direct_8x8_inference: false,
        crop: [0; 4],
        vui: Vui::default(),
    };
    if has_chroma_format(profile_idc) {
        sps.chroma_format_idc = r.ue_max(3, "chroma_format_idc")?;
        if sps.chroma_format_idc == 3 {
            sps.separate_colour_plane = r.flag()?;
        }
        sps.bit_depth_luma = 8 + r.ue_max(6, "bit_depth_luma_minus8")?;
        sps.bit_depth_chroma = 8 + r.ue_max(6, "bit_depth_chroma_minus8")?;
        sps.transform_bypass = r.flag()?;
        if r.flag()? {
            let lists_8x8 = if sps.chroma_format_idc == 3 { 6 } else { 2 };
            // Fall-back rule A: a missing first list is the default list.
            let (l4, l8) = scaling_lists(
                &mut r,
                lists_8x8,
                [t::DEFAULT_4X4_INTRA, t::DEFAULT_4X4_INTER],
                [t::DEFAULT_8X8_INTRA, t::DEFAULT_8X8_INTER],
            )?;
            sps.scaling4x4 = Some(l4);
            sps.scaling8x8 = Some(l8);
        }
    }
    sps.log2_max_frame_num = 4 + r.ue_max(12, "log2_max_frame_num_minus4")?;
    sps.poc_type = r.ue_max(2, "pic_order_cnt_type")?;
    match sps.poc_type {
        0 => sps.log2_max_poc_lsb = 4 + r.ue_max(12, "log2_max_pic_order_cnt_lsb_minus4")?,
        1 => {
            sps.delta_pic_order_always_zero = r.flag()?;
            sps.offset_for_non_ref_pic = r.se()?;
            sps.offset_for_top_to_bottom_field = r.se()?;
            let n = r.ue_max(255, "num_ref_frames_in_pic_order_cnt_cycle")?;
            sps.offset_for_ref_frame.reserve(n as usize);
            let mut total: i64 = 0;
            for _ in 0..n {
                let offset = r.se()?;
                // The offsets are summed over a cycle and then over
                // cycles; a stream whose cycle alone leaves the range of a
                // picture order count cannot be decoded (clause 8.2.1.2).
                total += i64::from(offset);
                require(
                    i32::try_from(total).is_ok(),
                    "ExpectedDeltaPerPicOrderCntCycle out of range",
                )?;
                sps.offset_for_ref_frame.push(offset);
            }
        }
        _ => {}
    }
    sps.max_num_ref_frames = r.ue_max(16, "max_num_ref_frames")?;
    sps.gaps_in_frame_num_allowed = r.flag()?;
    // Level 6.2 allows 8192 luma samples across, which is 512 macroblocks;
    // 1024 leaves room without letting a size overflow the picture limit.
    sps.width_mbs = 1 + r.ue_max(1023, "pic_width_in_mbs_minus1")?;
    sps.height_map_units = 1 + r.ue_max(1023, "pic_height_in_map_units_minus1")?;
    sps.frame_mbs_only = r.flag()?;
    if !sps.frame_mbs_only {
        sps.mb_adaptive_frame_field = r.flag()?;
    }
    sps.direct_8x8_inference = r.flag()?;
    if r.flag()? {
        sps.crop = parse_crop(&mut r, &sps)?;
    }
    if r.flag()? {
        // A malformed video usability information does not stop decoding:
        // only the buffering hints come from it.
        sps.vui = parse_vui(&mut r).unwrap_or_default();
    }
    Ok(sps)
}

/// The frame cropping rectangle in luma samples (clause 7.4.2.1.1,
/// Equations 7-19 to 7-22).
fn parse_crop(r: &mut BitReader, sps: &Sps) -> Result<[u32; 4]> {
    let raw = [r.ue()?, r.ue()?, r.ue()?, r.ue()?];
    let frame = u32::from(sps.frame_mbs_only);
    let (unit_x, unit_y) = match sps.chroma_array_type() {
        0 => (1, 2 - frame),
        1 => (2, 2 * (2 - frame)),
        2 => (2, 2 - frame),
        _ => (1, 2 - frame),
    };
    let [left, right, top, bottom] = raw;
    let crop = [
        left.saturating_mul(unit_x),
        right.saturating_mul(unit_x),
        top.saturating_mul(unit_y),
        bottom.saturating_mul(unit_y),
    ];
    let width = sps.width_mbs.saturating_mul(16);
    let height = sps.frame_height_mbs().saturating_mul(16);
    require(
        crop[0].saturating_add(crop[1]) < width && crop[2].saturating_add(crop[3]) < height,
        "frame cropping outside the picture",
    )?;
    Ok(crop)
}

/// A picture parameter set, resolved against the sequence parameter set it
/// names.
#[derive(Clone, Debug)]
pub(crate) struct Pps {
    pub(crate) sps_id: u32,
    pub(crate) cabac: bool,
    pub(crate) bottom_field_pic_order_in_frame_present: bool,
    pub(crate) num_slice_groups: u32,
    pub(crate) slice_group_map_type: u32,
    pub(crate) num_ref_idx_default_active: [u32; 2],
    pub(crate) weighted_pred: bool,
    pub(crate) weighted_bipred_idc: u32,
    pub(crate) pic_init_qp: i32,
    pub(crate) chroma_qp_index_offset: [i32; 2],
    pub(crate) deblocking_filter_control_present: bool,
    pub(crate) constrained_intra_pred: bool,
    pub(crate) redundant_pic_cnt_present: bool,
    pub(crate) transform_8x8_mode: bool,
    /// The scaling lists this picture parameter set decodes with, in zig-zag
    /// order (flat 16s where neither it nor its sequence parameter set has a
    /// matrix).
    pub(crate) scaling4x4: [[u8; 16]; 6],
    pub(crate) scaling8x8: [[u8; 64]; 6],
}

/// pic_parameter_set_rbsp() (clause 7.3.2.2). `sps_of` gives the sequence
/// parameter set the picture parameter set names, which must already be
/// known.
pub(crate) fn parse_pps(rbsp: &[u8], sps_of: impl Fn(u32) -> Option<Sps>) -> Result<Pps> {
    let mut r = BitReader::new(rbsp);
    r.ue_max(MAX_PPS as u32 - 1, "pic_parameter_set_id")?;
    let sps_id = r.ue_max(MAX_SPS as u32 - 1, "seq_parameter_set_id")?;
    let sps = sps_of(sps_id).ok_or(Error::Invalid(
        "a picture parameter set names a missing sequence parameter set",
    ))?;
    let cabac = r.flag()?;
    let bottom_field_pic_order_in_frame_present = r.flag()?;
    let num_slice_groups = 1 + r.ue_max(7, "num_slice_groups_minus1")?;
    let mut slice_group_map_type = 0;
    if num_slice_groups > 1 {
        slice_group_map_type = r.ue_max(6, "slice_group_map_type")?;
        skip_slice_groups(&mut r, slice_group_map_type, num_slice_groups)?;
    }
    let l0 = 1 + r.ue_max(31, "num_ref_idx_l0_default_active_minus1")?;
    let l1 = 1 + r.ue_max(31, "num_ref_idx_l1_default_active_minus1")?;
    let weighted_pred = r.flag()?;
    let weighted_bipred_idc = r.u(2)?;
    require(weighted_bipred_idc <= 2, "weighted_bipred_idc")?;
    // pic_init_qp_minus26 and pic_init_qs_minus26 are -(26 + QpBdOffsetY)
    // to 25, and QpBdOffsetY is 0 for 8-bit (clause 7.4.2.2).
    let qp_delta = r.se()?;
    require((-26..=25).contains(&qp_delta), "pic_init_qp_minus26")?;
    let pic_init_qp = 26 + qp_delta;
    let qs_delta = r.se()?;
    require((-26..=25).contains(&qs_delta), "pic_init_qs_minus26")?;
    let chroma_qp_index_offset = r.se()?;
    require(
        (-12..=12).contains(&chroma_qp_index_offset),
        "chroma_qp_index_offset",
    )?;
    let deblocking_filter_control_present = r.flag()?;
    let constrained_intra_pred = r.flag()?;
    let redundant_pic_cnt_present = r.flag()?;
    let mut transform_8x8_mode = false;
    let mut second_chroma_qp_index_offset = chroma_qp_index_offset;
    let mut scaling4x4 = sps.scaling4x4.unwrap_or([[16; 16]; 6]);
    let mut scaling8x8 = sps.scaling8x8.unwrap_or([[16; 64]; 6]);
    if r.more_rbsp_data() {
        transform_8x8_mode = r.flag()?;
        if r.flag()? {
            let lists_8x8 = match (transform_8x8_mode, sps.chroma_format_idc) {
                (false, _) => 0,
                (true, 3) => 6,
                (true, _) => 2,
            };
            let (first4, first8) = match (&sps.scaling4x4, &sps.scaling8x8) {
                // Fall-back rule B: the sequence-level lists.
                (Some(s4), Some(s8)) => (
                    [first_or_flat(s4, 0), first_or_flat(s4, 3)],
                    [first_or_flat(s8, 0), first_or_flat(s8, 1)],
                ),
                // Fall-back rule A: the default lists.
                _ => (
                    [t::DEFAULT_4X4_INTRA, t::DEFAULT_4X4_INTER],
                    [t::DEFAULT_8X8_INTRA, t::DEFAULT_8X8_INTER],
                ),
            };
            let (l4, l8) = scaling_lists(&mut r, lists_8x8, first4, first8)?;
            scaling4x4 = l4;
            scaling8x8 = l8;
        }
        second_chroma_qp_index_offset = r.se()?;
        require(
            (-12..=12).contains(&second_chroma_qp_index_offset),
            "second_chroma_qp_index_offset",
        )?;
    }
    Ok(Pps {
        sps_id,
        cabac,
        bottom_field_pic_order_in_frame_present,
        num_slice_groups,
        slice_group_map_type,
        num_ref_idx_default_active: [l0, l1],
        weighted_pred,
        weighted_bipred_idc,
        pic_init_qp,
        chroma_qp_index_offset: [chroma_qp_index_offset, second_chroma_qp_index_offset],
        deblocking_filter_control_present,
        constrained_intra_pred,
        redundant_pic_cnt_present,
        transform_8x8_mode,
        scaling4x4,
        scaling8x8,
    })
}

/// List `i` of a scaling matrix, or a flat list if it is not there (which
/// the array's own length rules out).
fn first_or_flat<const N: usize>(lists: &[[u8; N]; 6], i: usize) -> [u8; N] {
    lists.get(i).copied().unwrap_or([16; N])
}

/// The slice group map of a picture parameter set (clause 7.3.2.2). The
/// decoder does not decode slice groups; the syntax is read so that what
/// follows it is read at the right position, and the slice refuses later.
fn skip_slice_groups(r: &mut BitReader, map_type: u32, groups: u32) -> Result<()> {
    match map_type {
        0 => {
            for _ in 0..groups {
                r.ue()?;
            }
        }
        2 => {
            for _ in 0..groups.saturating_sub(1) {
                r.ue()?;
                r.ue()?;
            }
        }
        3..=5 => {
            r.u(1)?;
            r.ue()?;
        }
        6 => {
            let units = 1 + r.ue_max((1 << 20) - 1, "pic_size_in_map_units_minus1")?;
            let bits = 32 - groups.saturating_sub(1).leading_zeros();
            for _ in 0..units {
                r.u(bits)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// normAdjust4x4 (Equation 8-315), the three values of each qP % 6 row.
const NORM_ADJUST_4X4: [[i32; 3]; 6] = [
    [10, 16, 13],
    [11, 18, 14],
    [13, 20, 16],
    [14, 23, 18],
    [16, 25, 20],
    [18, 29, 23],
];

/// normAdjust8x8 (Equation 8-318), the six values of each qP % 6 row.
const NORM_ADJUST_8X8: [[i32; 6]; 6] = [
    [20, 18, 32, 19, 25, 24],
    [22, 19, 35, 21, 28, 26],
    [26, 23, 42, 24, 33, 31],
    [28, 25, 45, 26, 35, 33],
    [32, 28, 51, 30, 40, 38],
    [36, 32, 58, 34, 46, 43],
];

/// normAdjust4x4(m, (i, j)) (Equation 8-315).
fn norm_adjust4(row: [i32; 3], i: usize, j: usize) -> i32 {
    let [a, b, c] = row;
    match (i % 2, j % 2) {
        (0, 0) => a,
        (1, 1) => b,
        _ => c,
    }
}

/// normAdjust8x8(m, (i, j)) (Equation 8-318).
fn norm_adjust8(row: [i32; 6], i: usize, j: usize) -> i32 {
    let [a, b, c, d, e, f] = row;
    if i % 4 == 0 && j % 4 == 0 {
        a
    } else if i % 2 == 1 && j % 2 == 1 {
        b
    } else if i % 4 == 2 && j % 4 == 2 {
        c
    } else if (i % 4 == 0 && j % 2 == 1) || (i % 2 == 1 && j % 4 == 0) {
        d
    } else if (i % 4 == 0 && j % 4 == 2) || (i % 4 == 2 && j % 4 == 0) {
        e
    } else {
        f
    }
}

/// LevelScale4x4 and LevelScale8x8 (clause 8.5.9) of one picture parameter
/// set, in raster order, for every qP % 6.
pub(crate) struct LevelScale {
    /// Indexed by list (Y, Cb, Cr intra, then Y, Cb, Cr inter), qP % 6 and
    /// raster position.
    pub(crate) l4: [[[i32; 16]; 6]; 6],
    /// Indexed by list (0 intra luma, 1 inter luma), qP % 6 and raster
    /// position.
    pub(crate) l8: [[[i32; 64]; 6]; 2],
}

impl LevelScale {
    /// The level scales of `pps`.
    pub(crate) fn new(pps: &Pps) -> Box<LevelScale> {
        LevelScale::new_body(pps)
    }

    /// [`LevelScale::new`]'s body, which each instance of the hot paths
    /// compiles ([`crate::tier::Choice::level_scale`]).
    #[inline(always)]
    pub(crate) fn new_body(pps: &Pps) -> Box<LevelScale> {
        let mut ls = Box::new(LevelScale {
            l4: [[[0; 16]; 6]; 6],
            l8: [[[0; 64]; 6]; 2],
        });
        for (list, scales) in ls.l4.iter_mut().enumerate() {
            // weightScale4x4: the scaling list out of zig-zag order.
            let mut weight = [0i32; 16];
            let zigzag = t::ZIGZAG_4X4
                .iter()
                .zip(pps.scaling4x4.get(list).into_iter().flatten());
            for (&pos, &v) in zigzag {
                if let Some(w) = weight.get_mut(usize::from(pos)) {
                    *w = i32::from(v);
                }
            }
            for (m, out) in scales.iter_mut().enumerate() {
                let row = NORM_ADJUST_4X4.get(m).copied().unwrap_or([0; 3]);
                for (pos, (o, w)) in out.iter_mut().zip(weight).enumerate() {
                    *o = w * norm_adjust4(row, pos / 4, pos % 4);
                }
            }
        }
        for (list, scales) in ls.l8.iter_mut().enumerate() {
            let mut weight = [0i32; 64];
            let zigzag = t::ZIGZAG_8X8
                .iter()
                .zip(pps.scaling8x8.get(list).into_iter().flatten());
            for (&pos, &v) in zigzag {
                if let Some(w) = weight.get_mut(usize::from(pos)) {
                    *w = i32::from(v);
                }
            }
            for (m, out) in scales.iter_mut().enumerate() {
                let row = NORM_ADJUST_8X8.get(m).copied().unwrap_or([0; 6]);
                for (pos, (o, w)) in out.iter_mut().zip(weight).enumerate() {
                    *o = w * norm_adjust8(row, pos / 8, pos % 8);
                }
            }
        }
        ls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 176x144 baseline sequence parameter set (profile_idc 66, level
    /// 1.0, 11 by 9 macroblocks, log2_max_pic_order_cnt_lsb 6), as a
    /// decoder receives it: the RBSP after the NAL unit header.
    const SPS_QCIF: &[u8] = &[0x42, 0xc0, 0x0a, 0xed, 0x05, 0x89, 0xc8];

    #[test]
    fn a_baseline_sequence_parameter_set() {
        let sps = parse_sps(SPS_QCIF).expect("sps");
        assert_eq!(sps.profile_idc, 66);
        assert_eq!(sps.level_idc, 10);
        assert_eq!(sps.width_mbs, 11);
        assert_eq!(sps.height_map_units, 9);
        assert_eq!(sps.frame_height_mbs(), 9);
        assert_eq!(sps.log2_max_frame_num, 4);
        assert_eq!(sps.log2_max_poc_lsb, 6);
        assert_eq!(sps.max_num_ref_frames, 1);
        assert!(sps.direct_8x8_inference);
        assert_eq!(sps.crop, [0; 4]);
        assert!(sps.frame_mbs_only);
        // A baseline profile has no chroma format in its parameter set.
        assert_eq!(sps.chroma_format_idc, 1);
        assert_eq!(sps.bit_depth_luma, 8);
        assert!(sps.scaling4x4.is_none());
    }

    #[test]
    fn a_truncated_parameter_set_is_an_error() {
        for n in 0..SPS_QCIF.len() - 3 {
            let short = SPS_QCIF.get(..n).expect("prefix");
            // Either an error, or a parameter set whose fields all read as
            // zero-padding; never a panic.
            let _ = parse_sps(short);
        }
        assert!(parse_sps(&[]).is_err());
    }

    /// A picture parameter set with flat scaling lists.
    fn flat_pps() -> Pps {
        Pps {
            sps_id: 0,
            cabac: true,
            bottom_field_pic_order_in_frame_present: false,
            num_slice_groups: 1,
            slice_group_map_type: 0,
            num_ref_idx_default_active: [1, 1],
            weighted_pred: false,
            weighted_bipred_idc: 0,
            pic_init_qp: 26,
            chroma_qp_index_offset: [0, 0],
            deblocking_filter_control_present: false,
            constrained_intra_pred: false,
            redundant_pic_cnt_present: false,
            transform_8x8_mode: false,
            scaling4x4: [[16; 16]; 6],
            scaling8x8: [[16; 64]; 6],
        }
    }

    #[test]
    fn flat_scaling_lists_give_the_plain_norm_adjust() {
        let pps = flat_pps();
        let ls = LevelScale::new(&pps);
        // Position (0, 0) of qP % 6 == 0 is 16 * normAdjust4x4(0, (0,0)).
        assert_eq!(ls.l4[0][0][0], 16 * 10);
        // Position (1, 1) uses the second value, (0, 1) the third.
        assert_eq!(ls.l4[0][0][5], 16 * 16);
        assert_eq!(ls.l4[0][0][1], 16 * 13);
        assert_eq!(ls.l8[0][5][0], 16 * 36);
    }

    #[cfg(all(feature = "fast", target_arch = "x86_64"))]
    #[test]
    fn each_instance_derives_the_same_level_scales() {
        // Differential test: LevelScale in the v3 instance against the
        // baseline's, for flat and random scaling lists.
        use crate::tier::Choice;
        if crate::fast::V3::detect().is_none() {
            return;
        }
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut pps = flat_pps();
        for case in 0..64 {
            if case > 0 {
                for list in pps.scaling4x4.iter_mut() {
                    list.iter_mut().for_each(|v| *v = 1 + next() as u8 % 255);
                }
                for list in pps.scaling8x8.iter_mut() {
                    list.iter_mut().for_each(|v| *v = 1 + next() as u8 % 255);
                }
            }
            let chosen = Choice::detect(true).level_scale(&pps);
            let base = LevelScale::new(&pps);
            assert_eq!(chosen.l4, base.l4, "case {case}");
            assert_eq!(chosen.l8, base.l8, "case {case}");
        }
    }
}
