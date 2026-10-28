//! Slice headers (clauses 7.3.3 and 7.4.3), including reference picture
//! list modification, the prediction weight table and reference picture
//! marking.
#![forbid(unsafe_code)]

use crate::bits::BitReader;
use crate::error::{Error, Result, require};
use crate::params::{Pps, Sps};

/// slice_type modulo 5 (Table 7-6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SliceType {
    P,
    B,
    I,
    Sp,
    Si,
}

impl SliceType {
    /// Whether the slice is coded without inter prediction.
    pub(crate) fn is_intra(self) -> bool {
        matches!(self, SliceType::I | SliceType::Si)
    }

    /// Whether the slice may predict from two lists.
    pub(crate) fn is_b(self) -> bool {
        self == SliceType::B
    }
}

/// One entry of ref_pic_list_modification() (clause 7.3.3.1).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ListModification {
    /// modification_of_pic_nums_idc: 0 and 1 subtract and add a picture
    /// number difference, 2 names a long-term picture.
    pub(crate) idc: u32,
    /// abs_diff_pic_num_minus1 or long_term_pic_num.
    pub(crate) value: u32,
}

/// One memory management control operation (clause 7.3.3.3).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Mmco {
    pub(crate) op: u32,
    pub(crate) difference_of_pic_nums_minus1: u32,
    pub(crate) long_term_pic_num: u32,
    pub(crate) long_term_frame_idx: u32,
    pub(crate) max_long_term_frame_idx_plus1: u32,
}

/// pred_weight_table() (clause 7.4.3.2), with the inferred values filled in
/// where a flag is 0.
#[derive(Clone, Debug, Default)]
pub(crate) struct PredWeightTable {
    pub(crate) luma_log2_denom: u32,
    pub(crate) chroma_log2_denom: u32,
    /// Per list, per reference index: (weight, offset).
    pub(crate) luma: [Vec<(i32, i32)>; 2],
    /// Per list, per reference index, per component (Cb then Cr): (weight,
    /// offset).
    pub(crate) chroma: [Vec<[(i32, i32); 2]>; 2],
}

impl PredWeightTable {
    /// The luma weight and offset of `ref_idx` in `list`, or the default
    /// weighting if the table is shorter than the list.
    pub(crate) fn luma_weight(&self, list: usize, ref_idx: usize) -> (i32, i32) {
        self.luma
            .get(list)
            .and_then(|l| l.get(ref_idx))
            .copied()
            .unwrap_or((1 << self.luma_log2_denom, 0))
    }

    /// Bit `r` of entry `list`: whether reference `r` of the list has the
    /// default weights in all three planes, weight 2^logWD and offset 0.
    ///
    /// Such a weighting gives each sample back unchanged, by Equations
    /// 8-274 and 8-275: with logWD >= 1, (p * 2^logWD + 2^(logWD - 1)) >>
    /// logWD is p, and with logWD = 0, p * 1 + 0 is p. Encoders commonly set
    /// weighted_pred_flag and then leave most references at the default,
    /// so a block predicted from one of them can be predicted straight
    /// into the picture as if weighting were off.
    pub(crate) fn default_weights(&self) -> [u32; 2] {
        let mut out = [0u32; 2];
        for (list, mask) in out.iter_mut().enumerate() {
            for r in 0..32 {
                let luma = self.luma_weight(list, r) == (1 << self.luma_log2_denom, 0);
                let chroma = (0..2)
                    .all(|c| self.chroma_weight(list, r, c) == (1 << self.chroma_log2_denom, 0));
                if luma && chroma {
                    *mask |= 1 << r;
                }
            }
        }
        out
    }

    /// The same for component `c` (0 for Cb, 1 for Cr) of the chroma planes.
    pub(crate) fn chroma_weight(&self, list: usize, ref_idx: usize, c: usize) -> (i32, i32) {
        self.chroma
            .get(list)
            .and_then(|l| l.get(ref_idx))
            .and_then(|pair| pair.get(c))
            .copied()
            .unwrap_or((1 << self.chroma_log2_denom, 0))
    }
}

/// A slice header.
#[derive(Clone, Debug)]
pub(crate) struct SliceHeader {
    pub(crate) nal_ref_idc: u8,
    pub(crate) idr: bool,
    pub(crate) first_mb: u32,
    pub(crate) slice_type: SliceType,
    pub(crate) pps_id: u32,
    pub(crate) frame_num: u32,
    pub(crate) field_pic: bool,
    pub(crate) idr_pic_id: u32,
    pub(crate) poc_lsb: u32,
    pub(crate) delta_poc_bottom: i32,
    pub(crate) delta_poc: [i32; 2],
    pub(crate) redundant_pic_cnt: u32,
    pub(crate) direct_spatial_mv_pred: bool,
    pub(crate) num_ref_idx_active: [u32; 2],
    pub(crate) list_mods: [Vec<ListModification>; 2],
    pub(crate) pred_weights: Option<PredWeightTable>,
    pub(crate) no_output_of_prior_pics: bool,
    pub(crate) long_term_reference: bool,
    pub(crate) adaptive_marking: bool,
    pub(crate) mmcos: Vec<Mmco>,
    pub(crate) cabac_init_idc: u32,
    pub(crate) slice_qp: i32,
    pub(crate) disable_deblocking_filter_idc: u32,
    pub(crate) filter_offset_a: i32,
    pub(crate) filter_offset_b: i32,
    /// Where slice_data() starts, in bits from the start of the RBSP.
    pub(crate) data_bit_offset: usize,
}

impl SliceHeader {
    /// Whether the marking resets every reference picture (operation 5).
    pub(crate) fn has_mmco5(&self) -> bool {
        self.mmcos.iter().any(|m| m.op == 5)
    }
}

/// Reads first_mb_in_slice, slice_type and pic_parameter_set_id, so that a
/// caller can find the picture parameter set a slice activates.
pub(crate) fn peek_pps_id(rbsp: &[u8]) -> Result<u32> {
    let mut r = BitReader::new(rbsp);
    r.ue()?;
    r.ue()?;
    r.ue()
}

/// ref_pic_list_modification() for one list (clause 7.3.3.1).
fn parse_list_modifications(r: &mut BitReader) -> Result<Vec<ListModification>> {
    let mut mods = Vec::new();
    if !r.flag()? {
        return Ok(mods);
    }
    loop {
        let idc = r.ue()?;
        match idc {
            0..=2 => {
                let value = r.ue()?;
                mods.push(ListModification { idc, value });
                // A list is at most 32 entries, and a modification can only
                // place one picture each.
                require(mods.len() <= 33, "too many reference list modifications")?;
            }
            3 => return Ok(mods),
            _ => return Err(Error::Invalid("modification_of_pic_nums_idc")),
        }
    }
}

/// pred_weight_table() (clause 7.3.3.2).
fn parse_pred_weights(
    r: &mut BitReader,
    sps: &Sps,
    slice_type: SliceType,
    active: [u32; 2],
) -> Result<PredWeightTable> {
    let mut t = PredWeightTable {
        luma_log2_denom: r.ue_max(7, "luma_log2_weight_denom")?,
        ..Default::default()
    };
    let chroma = sps.chroma_array_type() != 0;
    if chroma {
        t.chroma_log2_denom = r.ue_max(7, "chroma_log2_weight_denom")?;
    }
    let lists = if slice_type.is_b() { 2 } else { 1 };
    for list in 0..lists {
        let n = active.get(list).copied().unwrap_or(0);
        let (Some(luma), Some(chromas)) = (t.luma.get_mut(list), t.chroma.get_mut(list)) else {
            break;
        };
        for _ in 0..n {
            let mut weight = (1 << t.luma_log2_denom, 0);
            if r.flag()? {
                weight = (r.se()?, r.se()?);
                require(
                    (-128..=127).contains(&weight.0) && (-128..=127).contains(&weight.1),
                    "luma weight or offset",
                )?;
            }
            luma.push(weight);
            let mut pair = [(1 << t.chroma_log2_denom, 0); 2];
            if chroma && r.flag()? {
                for c in pair.iter_mut() {
                    *c = (r.se()?, r.se()?);
                    require(
                        (-128..=127).contains(&c.0) && (-128..=127).contains(&c.1),
                        "chroma weight or offset",
                    )?;
                }
            }
            chromas.push(pair);
        }
    }
    Ok(t)
}

/// dec_ref_pic_marking() for a picture that is not an IDR picture
/// (clause 7.3.3.3).
fn parse_mmcos(r: &mut BitReader) -> Result<Vec<Mmco>> {
    let mut mmcos = Vec::new();
    loop {
        let op = r.ue()?;
        if op == 0 {
            return Ok(mmcos);
        }
        require(op <= 6, "memory_management_control_operation")?;
        let mut m = Mmco {
            op,
            ..Default::default()
        };
        if op == 1 || op == 3 {
            m.difference_of_pic_nums_minus1 = r.ue()?;
        }
        if op == 2 {
            m.long_term_pic_num = r.ue()?;
        }
        if op == 3 || op == 6 {
            m.long_term_frame_idx = r.ue()?;
        }
        if op == 4 {
            m.max_long_term_frame_idx_plus1 = r.ue()?;
        }
        mmcos.push(m);
        // Each operation unmarks at most one picture of a buffer of 16, and
        // the buffer holds at most 16 of each kind.
        require(mmcos.len() <= 64, "too many marking operations")?;
    }
}

/// slice_header() (clause 7.3.3). `nal_unit_type` and `nal_ref_idc` come
/// from the NAL unit header.
pub(crate) fn parse_slice_header(
    r: &mut BitReader,
    nal_unit_type: u8,
    nal_ref_idc: u8,
    sps: &Sps,
    pps: &Pps,
) -> Result<SliceHeader> {
    let idr = nal_unit_type == 5;
    let first_mb = r.ue()?;
    let raw_type = r.ue_max(9, "slice_type")?;
    let slice_type = match raw_type % 5 {
        0 => SliceType::P,
        1 => SliceType::B,
        2 => SliceType::I,
        3 => SliceType::Sp,
        _ => SliceType::Si,
    };
    let pps_id = r.ue()?;
    if sps.separate_colour_plane {
        r.u(2)?;
    }
    let frame_num = r.u(sps.log2_max_frame_num)?;
    let mut field_pic = false;
    if !sps.frame_mbs_only {
        field_pic = r.flag()?;
        if field_pic {
            // bottom_field_flag.
            r.u(1)?;
        }
    }
    let idr_pic_id = if idr { r.ue()? } else { 0 };
    let mut poc_lsb = 0;
    let mut delta_poc_bottom = 0;
    let mut delta_poc = [0; 2];
    if sps.poc_type == 0 {
        poc_lsb = r.u(sps.log2_max_poc_lsb)?;
        if pps.bottom_field_pic_order_in_frame_present && !field_pic {
            delta_poc_bottom = r.se()?;
        }
    }
    if sps.poc_type == 1 && !sps.delta_pic_order_always_zero {
        delta_poc = [
            r.se()?,
            if pps.bottom_field_pic_order_in_frame_present && !field_pic {
                r.se()?
            } else {
                0
            },
        ];
    }
    let redundant_pic_cnt = if pps.redundant_pic_cnt_present {
        r.ue()?
    } else {
        0
    };
    let direct_spatial_mv_pred = slice_type.is_b() && r.flag()?;
    let mut num_ref_idx_active = pps.num_ref_idx_default_active;
    if matches!(slice_type, SliceType::P | SliceType::Sp | SliceType::B) {
        if r.flag()? {
            num_ref_idx_active = [1 + r.ue()?, if slice_type.is_b() { 1 + r.ue()? } else { 0 }];
        }
        let max = if field_pic { 32 } else { 16 };
        require(
            num_ref_idx_active[0] <= max && (!slice_type.is_b() || num_ref_idx_active[1] <= max),
            "num_ref_idx_active",
        )?;
    }
    if !slice_type.is_b() {
        num_ref_idx_active[1] = 0;
    }
    if slice_type.is_intra() {
        num_ref_idx_active[0] = 0;
    }
    if nal_unit_type == 20 || nal_unit_type == 21 {
        return Err(Error::Unsupported("multiview or 3D-AVC slice extensions"));
    }
    let mut list_mods: [Vec<ListModification>; 2] = [Vec::new(), Vec::new()];
    if !slice_type.is_intra() {
        list_mods[0] = parse_list_modifications(r)?;
    }
    if slice_type.is_b() {
        list_mods[1] = parse_list_modifications(r)?;
    }
    let explicit_weights = (pps.weighted_pred
        && matches!(slice_type, SliceType::P | SliceType::Sp))
        || (pps.weighted_bipred_idc == 1 && slice_type.is_b());
    let pred_weights = if explicit_weights {
        Some(parse_pred_weights(r, sps, slice_type, num_ref_idx_active)?)
    } else {
        None
    };
    let mut no_output_of_prior_pics = false;
    let mut long_term_reference = false;
    let mut adaptive_marking = false;
    let mut mmcos = Vec::new();
    if nal_ref_idc != 0 {
        if idr {
            no_output_of_prior_pics = r.flag()?;
            long_term_reference = r.flag()?;
        } else {
            adaptive_marking = r.flag()?;
            if adaptive_marking {
                mmcos = parse_mmcos(r)?;
            }
        }
    }
    let cabac_init_idc = if pps.cabac && !slice_type.is_intra() {
        r.ue_max(2, "cabac_init_idc")?
    } else {
        0
    };
    // slice_qp_delta is whatever se(v) the bits hold; the sum is checked
    // against its range below, so it saturates rather than overflowing.
    let slice_qp = pps.pic_init_qp.saturating_add(r.se()?);
    let qp_min = -6 * (sps.bit_depth_luma as i32 - 8);
    require((qp_min..=51).contains(&slice_qp), "slice_qp_delta")?;
    if matches!(slice_type, SliceType::Sp | SliceType::Si) {
        if slice_type == SliceType::Sp {
            // sp_for_switch_flag.
            r.u(1)?;
        }
        r.se()?;
    }
    let mut disable_deblocking_filter_idc = 0;
    let mut filter_offset_a = 0;
    let mut filter_offset_b = 0;
    if pps.deblocking_filter_control_present {
        disable_deblocking_filter_idc = r.ue_max(2, "disable_deblocking_filter_idc")?;
        if disable_deblocking_filter_idc != 1 {
            filter_offset_a = r.se()?.saturating_mul(2);
            filter_offset_b = r.se()?.saturating_mul(2);
            require(
                (-12..=12).contains(&filter_offset_a) && (-12..=12).contains(&filter_offset_b),
                "deblocking filter offsets",
            )?;
        }
    }
    if pps.num_slice_groups > 1 && (3..=5).contains(&pps.slice_group_map_type) {
        return Err(Error::Unsupported("slice groups"));
    }
    Ok(SliceHeader {
        nal_ref_idc,
        idr,
        first_mb,
        slice_type,
        pps_id,
        frame_num,
        field_pic,
        idr_pic_id,
        poc_lsb,
        delta_poc_bottom,
        delta_poc,
        redundant_pic_cnt,
        direct_spatial_mv_pred,
        num_ref_idx_active,
        list_mods,
        pred_weights,
        no_output_of_prior_pics,
        long_term_reference,
        adaptive_marking,
        mmcos,
        cabac_init_idc,
        slice_qp,
        disable_deblocking_filter_idc,
        filter_offset_a,
        filter_offset_b,
        data_bit_offset: r.pos(),
    })
}

#[cfg(test)]
mod tests {
    use super::PredWeightTable;

    fn table(luma: Vec<(i32, i32)>, chroma: Vec<[(i32, i32); 2]>) -> PredWeightTable {
        PredWeightTable {
            luma_log2_denom: 5,
            chroma_log2_denom: 3,
            luma: [luma, Vec::new()],
            chroma: [chroma, Vec::new()],
        }
    }

    #[test]
    fn default_weights_are_the_references_that_weighting_leaves_unchanged() {
        let t = table(
            vec![(32, 0), (32, 2), (40, 0), (32, 0)],
            vec![
                [(8, 0), (8, 0)],
                [(8, 0), (8, 0)],
                [(8, 0), (8, 0)],
                [(8, 0), (8, 1)],
            ],
        );
        let mask = t.default_weights();
        // Reference 0 is the default in all planes; 1 has a luma offset, 2 a
        // luma weight, 3 a Cr offset.
        assert_eq!(mask[0] & 0b1111, 0b0001);
        // Past the table, and in a list the table leaves empty, the defaults
        // apply (clause 7.4.3.2).
        assert_eq!(mask[0] >> 4, u32::MAX >> 4);
        assert_eq!(mask[1], u32::MAX);
    }

    #[test]
    fn a_default_weight_gives_every_sample_back() {
        // Equations 8-274 and 8-275 with w = 2^logWD and o = 0, through the
        // kernels' own weighting, scalar and vector: every sample value at
        // every denominator comes back unchanged.
        use coreav_kernels_h264::{Auto, Block, BlockMut, Combine, Kernels};
        let samples: Vec<u8> = (0..=255).collect();
        for kernels in [Auto::scalar(), Auto::detect()] {
            for log_wd in 0..=7 {
                for list in 0..2 {
                    let mut out = vec![0u8; 256];
                    let dst = BlockMut::new(&mut out, 0, 16, 16, 16).expect("view");
                    let p = Block::new(&samples, 0, 16, 16, 16).expect("view");
                    let w = 1 << log_wd;
                    let mode = Combine::Uni {
                        list,
                        log_wd,
                        w,
                        o: 0,
                    };
                    kernels.combine(dst, p, p, mode);
                    assert_eq!(out, samples, "logWD {log_wd}, list {list}");
                }
            }
        }
    }
}
