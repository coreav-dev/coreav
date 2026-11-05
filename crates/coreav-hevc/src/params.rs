use crate::bits::Bits;
use crate::rps::{self, Rps};
use crate::scaling::ScalingLists;
use crate::vui::{self, Hrd, Timing, Vui};
use crate::{Error, Limits, Result};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Vps {
    pub(crate) id: u32,
    pub(crate) sub_layers: u32,
    pub(crate) temporal_nesting: bool,
    pub(crate) max_dpb: [u32; 7],
    pub(crate) max_reorder: [u32; 7],
    pub(crate) max_latency: [u32; 7],
    pub(crate) timing: Option<Timing>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Pcm {
    pub(crate) luma_depth: u32,
    pub(crate) chroma_depth: u32,
    pub(crate) min_log2: u32,
    pub(crate) max_log2: u32,
    pub(crate) loop_filter_disabled: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Sps {
    pub(crate) id: u32,
    pub(crate) vps_id: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) visible_width: u32,
    pub(crate) visible_height: u32,
    pub(crate) conformance_window: [u32; 4],
    pub(crate) bit_depth: u32,
    pub(crate) profile_idc: u32,
    pub(crate) ctb_log2: u32,
    pub(crate) ctu_count: u32,
    pub(crate) poc_bits: u32,
    pub(crate) rps_count: usize,
    pub(crate) rps: [Rps; 64],
    pub(crate) long_term_present: bool,
    pub(crate) long_term_count: usize,
    pub(crate) long_term_used: [bool; 32],
    pub(crate) long_term_poc: [u32; 32],
    pub(crate) temporal_mvp: bool,
    pub(crate) sao: bool,
    pub(crate) sub_layers: u32,
    pub(crate) temporal_nesting: bool,
    pub(crate) max_dpb: [u32; 7],
    pub(crate) max_reorder: [u32; 7],
    pub(crate) max_latency: [u32; 7],
    pub(crate) min_cb_log2: u32,
    pub(crate) min_tb_log2: u32,
    pub(crate) max_tb_log2: u32,
    pub(crate) max_intra_transform_depth: u32,
    pub(crate) strong_smoothing: bool,
    pub(crate) pcm: Option<Pcm>,
    pub(crate) scaling_lists: Option<ScalingLists>,
    pub(crate) vui: Vui,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Pps {
    pub(crate) id: u32,
    pub(crate) sps_id: u32,
    pub(crate) dependent_segments: bool,
    pub(crate) output_flag: bool,
    pub(crate) extra_header_bits: u32,
    pub(crate) sign_data_hiding: bool,
    pub(crate) cabac_init: bool,
    pub(crate) ref_l0: u32,
    pub(crate) ref_l1: u32,
    pub(crate) weighted_pred: bool,
    pub(crate) weighted_bipred: bool,
    pub(crate) tiles: bool,
    pub(crate) wpp: bool,
    pub(crate) lists_modification: bool,
    pub(crate) deblock_override: bool,
    pub(crate) deblock_disabled: bool,
    pub(crate) deblock_offsets: [i32; 2],
    pub(crate) loop_filter_tiles: bool,
    pub(crate) loop_filter_slices: bool,
    pub(crate) chroma_offsets: bool,
    pub(crate) header_extension: bool,
    pub(crate) init_qp: i32,
    pub(crate) cb_qp_offset: i32,
    pub(crate) cr_qp_offset: i32,
    pub(crate) qp_delta_depth: u32,
    pub(crate) qp_delta_enabled: bool,
    pub(crate) transform_skip: bool,
    pub(crate) transquant_bypass: bool,
    pub(crate) parallel_merge_log2: u32,
    pub(crate) scaling_lists: Option<ScalingLists>,
    pub(crate) tile_columns: u32,
    pub(crate) tile_rows: u32,
    pub(crate) uniform_tiles: bool,
    pub(crate) column_width: [u32; 39],
    pub(crate) row_height: [u32; 43],
}

fn profile(bits: &mut Bits<'_>) -> Result<(u32, bool)> {
    if bits.u(2)? != 0 {
        return Err(Error::Unsupported("HEVC profile space"));
    }
    bits.flag()?;
    let id = bits.u(5)?;
    let compatibility = bits.u(32)?;
    let compatible = |p: u32| id == p || compatibility & (1u32 << (31 - p)) != 0;
    let main = compatible(1);
    let main10 = compatible(2);
    let still = compatible(3);
    if !main && !main10 && !still {
        return Err(Error::Unsupported("HEVC profile"));
    }
    bits.skip(4)?;
    if (4..=11).any(compatible) {
        bits.skip(9)?;
        if [5, 9, 10, 11].into_iter().any(compatible) {
            bits.flag()?;
            bits.skip(33)?;
        } else {
            bits.skip(34)?;
        }
    } else if main10 {
        bits.skip(7)?;
        bits.flag()?;
        bits.skip(35)?;
    } else {
        bits.skip(43)?;
    }
    if bits.flag()? {
        return Err(Error::Unsupported("HEVC INBLD capability"));
    }
    Ok((id, main10 && !main && !still))
}

fn profile_tier_level(bits: &mut Bits<'_>, sub_layers: u32) -> Result<(u32, bool)> {
    let result = profile(bits)?;
    bits.u(8)?;
    let mut profiles = [false; 7];
    let mut levels = [false; 7];
    for i in 0..sub_layers as usize {
        *profiles
            .get_mut(i)
            .ok_or(Error::InvalidData("sub-layer count"))? = bits.flag()?;
        *levels
            .get_mut(i)
            .ok_or(Error::InvalidData("sub-layer count"))? = bits.flag()?;
    }
    if sub_layers != 0 {
        for _ in sub_layers..8 {
            bits.u(2)?;
        }
    }
    for i in 0..sub_layers as usize {
        if profiles
            .get(i)
            .copied()
            .ok_or(Error::InvalidData("sub-layer count"))?
        {
            profile(bits)?;
        }
        if levels
            .get(i)
            .copied()
            .ok_or(Error::InvalidData("sub-layer count"))?
        {
            bits.u(8)?;
        }
    }
    Ok(result)
}

fn ordering(bits: &mut Bits<'_>, sub_layers: u32) -> Result<([u32; 7], [u32; 7], [u32; 7])> {
    let all = bits.flag()?;
    let mut dpb = [0; 7];
    let mut reorder = [0; 7];
    let mut latency = [0; 7];
    let start = if all { 0 } else { sub_layers as usize };
    let mut previous_dpb = 0;
    let mut previous_reorder = 0;
    for i in start..=sub_layers as usize {
        let size = bits.ue(15)? + 1;
        let count = bits.ue(size - 1)?;
        let delay = bits.ue(u32::MAX - 1)?;
        if size < previous_dpb || count < previous_reorder {
            return Err(Error::InvalidData("sub-layer ordering"));
        }
        *dpb.get_mut(i)
            .ok_or(Error::InvalidData("sub-layer count"))? = size;
        *reorder
            .get_mut(i)
            .ok_or(Error::InvalidData("sub-layer count"))? = count;
        *latency
            .get_mut(i)
            .ok_or(Error::InvalidData("sub-layer count"))? = delay;
        previous_dpb = size;
        previous_reorder = count;
    }
    if !all {
        let size = *dpb
            .get(start)
            .ok_or(Error::InvalidData("sub-layer count"))?;
        let count = *reorder
            .get(start)
            .ok_or(Error::InvalidData("sub-layer count"))?;
        let delay = *latency
            .get(start)
            .ok_or(Error::InvalidData("sub-layer count"))?;
        dpb.fill(size);
        reorder.fill(count);
        latency.fill(delay);
    }
    Ok((dpb, reorder, latency))
}

fn sub_layers(bits: &mut Bits<'_>) -> Result<(u32, bool)> {
    let count = bits.u(3)?;
    if count > 6 {
        return Err(Error::InvalidData("sub-layer count"));
    }
    let nesting = bits.flag()?;
    if count == 0 && !nesting {
        return Err(Error::InvalidData("temporal nesting"));
    }
    Ok((count, nesting))
}

fn end(bits: &mut Bits<'_>, ignored_extension: bool) -> Result<()> {
    if ignored_extension {
        while bits.more_rbsp_data() {
            bits.flag()?;
        }
    }
    bits.trailing()
}

fn extensions(bits: &mut Bits<'_>, sps: bool) -> Result<()> {
    let mut ignored = false;
    if bits.flag()? {
        if bits.flag()? {
            return Err(Error::Unsupported(if sps {
                "HEVC SPS range extension"
            } else {
                "HEVC PPS range extension"
            }));
        }
        if bits.flag()? {
            return Err(Error::Unsupported("HEVC multilayer extension"));
        }
        if bits.flag()? {
            return Err(Error::Unsupported("HEVC 3D extension"));
        }
        if bits.flag()? {
            return Err(Error::Unsupported("HEVC SCC extension"));
        }
        ignored = bits.u(4)? != 0;
    }
    end(bits, ignored)
}

pub(crate) fn vps(bits: &mut Bits<'_>, _limits: &Limits) -> Result<Vps> {
    let id = bits.u(4)?;
    if !bits.flag()? || !bits.flag()? {
        return Err(Error::Unsupported("HEVC external base layer"));
    }
    if bits.u(6)? != 0 {
        return Err(Error::Unsupported("HEVC multiple layers"));
    }
    let (sub_layers, temporal_nesting) = sub_layers(bits)?;
    bits.u(16)?;
    profile_tier_level(bits, sub_layers)?;
    let (max_dpb, max_reorder, max_latency) = ordering(bits, sub_layers)?;
    if bits.u(6)? != 0 {
        return Err(Error::Unsupported("HEVC multiple layers"));
    }
    let layer_sets = bits.ue(1023)?;
    for _ in 0..layer_sets {
        bits.flag()?;
    }
    let mut timing = None;
    if bits.flag()? {
        let num_units_in_tick = bits.u(32)?;
        let time_scale = bits.u(32)?;
        if num_units_in_tick == 0 || time_scale == 0 {
            return Err(Error::InvalidData("VPS timing"));
        }
        let ticks_per_poc = if bits.flag()? {
            Some(bits.ue(u32::MAX - 1)? + 1)
        } else {
            None
        };
        timing = Some(Timing {
            num_units_in_tick,
            time_scale,
            ticks_per_poc,
        });
        let count = bits.ue(layer_sets + 1)?;
        let mut state = Hrd::default();
        let mut seen = [false; 1024];
        for i in 0..count {
            let index = bits.ue(layer_sets)? as usize;
            let entry = seen
                .get_mut(index)
                .ok_or(Error::InvalidData("HRD layer set index"))?;
            if *entry {
                return Err(Error::InvalidData("duplicate HRD layer set"));
            }
            *entry = true;
            let common = i == 0 || bits.flag()?;
            vui::hrd(bits, common, sub_layers, &mut state)?;
        }
    }
    let extension = bits.flag()?;
    end(bits, extension)?;
    Ok(Vps {
        id,
        sub_layers,
        temporal_nesting,
        max_dpb,
        max_reorder,
        max_latency,
        timing,
    })
}

pub(crate) fn sps(bits: &mut Bits<'_>, limits: &Limits) -> Result<Sps> {
    let vps_id = bits.u(4)?;
    let (sub_layers, temporal_nesting) = sub_layers(bits)?;
    let (profile_idc, ten_bit) = profile_tier_level(bits, sub_layers)?;
    let id = bits.ue(15)?;
    let chroma = bits.ue(3)?;
    if chroma != 1 {
        return Err(Error::Unsupported("HEVC chroma format"));
    }
    let width = bits.ue(u32::MAX)?;
    let height = bits.ue(u32::MAX)?;
    if width == 0 || height == 0 {
        return Err(Error::InvalidData("SPS dimensions"));
    }
    if width > limits.max_width || height > limits.max_height {
        return Err(Error::Limit("HEVC dimensions"));
    }
    let mut visible_width = width;
    let mut visible_height = height;
    let mut conformance_window = [0; 4];
    if bits.flag()? {
        let left = bits.ue(u32::MAX)?;
        let right = bits.ue(u32::MAX)?;
        let top = bits.ue(u32::MAX)?;
        let bottom = bits.ue(u32::MAX)?;
        conformance_window = [left, right, top, bottom];
        let horizontal = (u64::from(left) + u64::from(right)) * 2;
        let vertical = (u64::from(top) + u64::from(bottom)) * 2;
        if horizontal >= u64::from(width) || vertical >= u64::from(height) {
            return Err(Error::InvalidData("SPS conformance window"));
        }
        visible_width -= horizontal as u32;
        visible_height -= vertical as u32;
    }
    let bit_depth = bits.ue(8)? + 8;
    let chroma_depth = bits.ue(8)? + 8;
    if bit_depth != chroma_depth {
        return Err(Error::Unsupported("unequal component depths"));
    }
    if !matches!(bit_depth, 8 | 10) || (bit_depth == 10 && !ten_bit) {
        return Err(Error::Unsupported("HEVC bit depth"));
    }
    let poc_bits = bits.ue(12)? + 4;
    let (max_dpb, max_reorder, max_latency) = ordering(bits, sub_layers)?;
    let min_cb_log2 = bits.ue(3)? + 3;
    let ctb_log2 = min_cb_log2 + bits.ue(6 - min_cb_log2)?;
    if ctb_log2 < 4 || width % (1 << min_cb_log2) != 0 || height % (1 << min_cb_log2) != 0 {
        return Err(Error::InvalidData("SPS coding block geometry"));
    }
    let ctu_count = width
        .div_ceil(1 << ctb_log2)
        .checked_mul(height.div_ceil(1 << ctb_log2))
        .ok_or(Error::Limit("HEVC CTU count"))?;
    if ctu_count > limits.max_ctus {
        return Err(Error::Limit("HEVC CTU count"));
    }
    let min_tb_log2 = bits.ue(min_cb_log2 - 3)? + 2;
    let max_tb_log2 = min_tb_log2 + bits.ue(ctb_log2.min(5) - min_tb_log2)?;
    bits.ue(ctb_log2 - min_tb_log2)?;
    let max_intra_transform_depth = bits.ue(ctb_log2 - min_tb_log2)?;
    let scaling_lists = if bits.flag()? {
        Some(if bits.flag()? {
            ScalingLists::parse(bits)?
        } else {
            ScalingLists::normative()
        })
    } else {
        None
    };
    bits.flag()?;
    let sao = bits.flag()?;
    let pcm = if bits.flag()? {
        let luma_depth = bits.u(4)? + 1;
        let chroma_depth = bits.u(4)? + 1;
        if luma_depth > bit_depth || chroma_depth > bit_depth {
            return Err(Error::InvalidData("PCM bit depth"));
        }
        let pcm_min = bits.ue(2)? + 3;
        if pcm_min < min_cb_log2.min(5) || pcm_min > ctb_log2.min(5) {
            return Err(Error::InvalidData("PCM block size"));
        }
        let max_log2 = pcm_min + bits.ue(ctb_log2.min(5) - pcm_min)?;
        let loop_filter_disabled = bits.flag()?;
        Some(Pcm {
            luma_depth,
            chroma_depth,
            min_log2: pcm_min,
            max_log2,
            loop_filter_disabled,
        })
    } else {
        None
    };
    let rps_count = bits.ue(64)? as usize;
    let mut rps = [Rps::default(); 64];
    let dpb = *max_dpb
        .get(sub_layers as usize)
        .ok_or(Error::InvalidData("sub-layer count"))?;
    for i in 0..rps_count {
        let parsed = rps::parse(bits, &rps, i, false)?;
        if parsed.count as u32 >= dpb {
            return Err(Error::InvalidData("SPS RPS picture count"));
        }
        *rps.get_mut(i).ok_or(Error::InvalidData("SPS RPS count"))? = parsed;
    }
    let long_term_present = bits.flag()?;
    let long_term_count = if long_term_present {
        bits.ue(32)? as usize
    } else {
        0
    };
    let mut long_term_used = [false; 32];
    let mut long_term_poc = [0; 32];
    for (used, poc) in long_term_used
        .iter_mut()
        .zip(long_term_poc.iter_mut())
        .take(long_term_count)
    {
        *poc = bits.u(poc_bits)?;
        *used = bits.flag()?;
    }
    let temporal_mvp = bits.flag()?;
    let strong_smoothing = bits.flag()?;
    let vui = if bits.flag()? {
        vui::parse(bits, sub_layers, visible_width, visible_height)?
    } else {
        Vui::default()
    };
    extensions(bits, true)?;
    Ok(Sps {
        id,
        vps_id,
        width,
        height,
        visible_width,
        visible_height,
        conformance_window,
        bit_depth,
        profile_idc,
        ctb_log2,
        ctu_count,
        poc_bits,
        rps_count,
        rps,
        long_term_present,
        long_term_count,
        long_term_used,
        long_term_poc,
        temporal_mvp,
        sao,
        sub_layers,
        temporal_nesting,
        max_dpb,
        max_reorder,
        max_latency,
        min_cb_log2,
        min_tb_log2,
        max_tb_log2,
        max_intra_transform_depth,
        strong_smoothing,
        pcm,
        scaling_lists,
        vui,
    })
}

pub(crate) fn pps(bits: &mut Bits<'_>, limits: &Limits) -> Result<Pps> {
    let id = bits.ue(63)?;
    let sps_id = bits.ue(15)?;
    let dependent_segments = bits.flag()?;
    let output_flag = bits.flag()?;
    let extra_header_bits = bits.u(3)?;
    let sign_data_hiding = bits.flag()?;
    let cabac_init = bits.flag()?;
    let ref_l0 = bits.ue(14)? + 1;
    let ref_l1 = bits.ue(14)? + 1;
    let init_qp = bits.se(-74, 25)? + 26;
    bits.flag()?;
    let transform_skip = bits.flag()?;
    let qp_delta_enabled = bits.flag()?;
    let qp_delta_depth = if qp_delta_enabled { bits.ue(3)? } else { 0 };
    let cb_qp_offset = bits.se(-12, 12)?;
    let cr_qp_offset = bits.se(-12, 12)?;
    let chroma_offsets = bits.flag()?;
    let weighted_pred = bits.flag()?;
    let weighted_bipred = bits.flag()?;
    let transquant_bypass = bits.flag()?;
    let tiles = bits.flag()?;
    let wpp = bits.flag()?;
    if tiles && wpp {
        return Err(Error::InvalidData("Main profile tiles with WPP"));
    }
    let mut tile_columns = 1;
    let mut tile_rows = 1;
    let mut uniform_tiles = true;
    let mut column_width = [0; 39];
    let mut row_height = [0; 43];
    let mut loop_filter_tiles = true;
    if tiles {
        tile_columns = bits.ue(u32::MAX - 1)? + 1;
        tile_rows = bits.ue(u32::MAX - 1)? + 1;
        if tile_columns > 40 || tile_rows > 44 {
            return Err(Error::Limit("HEVC tile grid"));
        }
        if tile_columns == 1 && tile_rows == 1 {
            return Err(Error::InvalidData("PPS tile count"));
        }
        uniform_tiles = bits.flag()?;
        if !uniform_tiles {
            for width in column_width.iter_mut().take(tile_columns as usize - 1) {
                *width = bits.ue(u32::MAX - 1)? + 1;
                if *width > limits.max_ctus {
                    return Err(Error::Limit("HEVC tile width"));
                }
            }
            for height in row_height.iter_mut().take(tile_rows as usize - 1) {
                *height = bits.ue(u32::MAX - 1)? + 1;
                if *height > limits.max_ctus {
                    return Err(Error::Limit("HEVC tile height"));
                }
            }
        }
        loop_filter_tiles = bits.flag()?;
    }
    let loop_filter_slices = bits.flag()?;
    let mut deblock_override = false;
    let mut deblock_disabled = false;
    let mut deblock_offsets = [0; 2];
    if bits.flag()? {
        deblock_override = bits.flag()?;
        deblock_disabled = bits.flag()?;
        if !deblock_disabled {
            deblock_offsets = [bits.se(-6, 6)?, bits.se(-6, 6)?];
        }
    }
    let scaling_lists = if bits.flag()? {
        Some(ScalingLists::parse(bits)?)
    } else {
        None
    };
    let lists_modification = bits.flag()?;
    let parallel_merge_log2 = bits.ue(4)? + 2;
    let header_extension = bits.flag()?;
    extensions(bits, false)?;
    Ok(Pps {
        id,
        sps_id,
        dependent_segments,
        output_flag,
        extra_header_bits,
        sign_data_hiding,
        cabac_init,
        ref_l0,
        ref_l1,
        weighted_pred,
        weighted_bipred,
        tiles,
        wpp,
        lists_modification,
        deblock_override,
        deblock_disabled,
        deblock_offsets,
        loop_filter_tiles,
        loop_filter_slices,
        chroma_offsets,
        header_extension,
        init_qp,
        cb_qp_offset,
        cr_qp_offset,
        qp_delta_depth,
        qp_delta_enabled,
        transform_skip,
        transquant_bypass,
        parallel_merge_log2,
        scaling_lists,
        tile_columns,
        tile_rows,
        uniform_tiles,
        column_width,
        row_height,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
pub(crate) mod tests {
    use super::*;

    #[derive(Default)]
    pub(crate) struct Writer {
        bits: Vec<bool>,
    }

    impl Writer {
        pub(crate) fn flag(&mut self, value: bool) {
            self.bits.push(value);
        }

        pub(crate) fn u(&mut self, n: u32, value: u32) {
            for shift in (0..n).rev() {
                self.flag(value & (1 << shift) != 0);
            }
        }

        pub(crate) fn ue(&mut self, value: u32) {
            let value = value + 1;
            let n = 32 - value.leading_zeros();
            for _ in 1..n {
                self.flag(false);
            }
            self.u(n, value);
        }

        pub(crate) fn se(&mut self, value: i32) {
            self.ue(if value > 0 {
                2 * value as u32 - 1
            } else {
                2 * (-value) as u32
            });
        }

        pub(crate) fn len(&self) -> usize {
            self.bits.len()
        }

        pub(crate) fn bytes(&self) -> Vec<u8> {
            self.bits
                .chunks(8)
                .map(|bits| {
                    bits.iter()
                        .enumerate()
                        .fold(0, |value, (i, bit)| value | (u8::from(*bit) << (7 - i)))
                })
                .collect()
        }

        fn rbsp(mut self) -> Vec<u8> {
            self.flag(true);
            while !self.len().is_multiple_of(8) {
                self.flag(false);
            }
            self.bytes()
        }
    }

    fn ptl(w: &mut Writer, depth: u32) {
        w.u(2, 0);
        w.flag(false);
        w.u(5, if depth == 10 { 2 } else { 1 });
        w.u(32, 0);
        w.u(4, 0);
        w.u(32, 0);
        w.u(12, 0);
        w.u(8, 120);
    }

    fn scaling(w: &mut Writer, explicit: bool) {
        for size in 0..4 {
            for _ in (0..6).step_by(if size == 3 { 3 } else { 1 }) {
                w.flag(explicit);
                if explicit {
                    if size > 1 {
                        w.se(0);
                    }
                    for _ in 0..if size == 0 { 16 } else { 64 } {
                        w.se(0);
                    }
                } else {
                    w.ue(0);
                }
            }
        }
    }

    pub(crate) fn sequence(
        depth: u32,
        field: bool,
        range_extension: bool,
        ignored_extension: bool,
    ) -> Vec<u8> {
        let mut w = Writer::default();
        w.u(4, 0);
        w.u(3, 0);
        w.flag(true);
        ptl(&mut w, depth);
        w.ue(0);
        w.ue(1);
        w.ue(64);
        w.ue(64);
        w.flag(true);
        for offset in [0, 0, 0, 4] {
            w.ue(offset);
        }
        w.ue(depth - 8);
        w.ue(depth - 8);
        w.ue(0);
        w.flag(false);
        w.ue(4);
        w.ue(2);
        w.ue(0);
        for value in [0, 3, 0, 3, 0, 0] {
            w.ue(value);
        }
        w.flag(true);
        w.flag(true);
        scaling(&mut w, false);
        w.flag(true);
        w.flag(true);
        w.flag(true);
        w.u(4, depth - 1);
        w.u(4, depth - 1);
        w.ue(0);
        w.ue(2);
        w.flag(true);
        w.ue(1);
        w.ue(1);
        w.ue(1);
        w.ue(0);
        w.flag(true);
        w.ue(1);
        w.flag(false);
        w.flag(true);
        w.ue(2);
        w.u(4, 0);
        w.flag(true);
        w.u(4, 2);
        w.flag(false);
        w.flag(true);
        w.flag(true);
        w.flag(true);
        for flag in [
            false, false, false, false, false, field, false, false, false, false,
        ] {
            w.flag(flag);
        }
        w.flag(range_extension || ignored_extension);
        if range_extension || ignored_extension {
            w.flag(range_extension);
            w.u(3, 0);
            w.u(4, 1);
            w.u(5, 0b10101);
        }
        w.rbsp()
    }

    fn picture(ignored_extension: bool) -> Vec<u8> {
        let mut w = Writer::default();
        w.ue(3);
        w.ue(0);
        w.flag(true);
        w.flag(true);
        w.u(3, 2);
        w.flag(true);
        w.flag(true);
        w.ue(1);
        w.ue(2);
        w.se(-2);
        w.flag(false);
        w.flag(true);
        w.flag(true);
        w.ue(1);
        w.se(-2);
        w.se(2);
        for flag in [true, true, true, true, true, false] {
            w.flag(flag);
        }
        w.ue(1);
        w.ue(1);
        w.flag(false);
        w.ue(0);
        w.ue(0);
        w.flag(true);
        w.flag(true);
        w.flag(true);
        w.flag(true);
        w.flag(false);
        w.se(-2);
        w.se(2);
        w.flag(true);
        scaling(&mut w, true);
        w.flag(true);
        w.ue(0);
        w.flag(true);
        w.flag(ignored_extension);
        if ignored_extension {
            w.u(4, 0);
            w.u(4, 1);
            w.u(3, 0b110);
        }
        w.rbsp()
    }

    pub(crate) fn video(ignored_extension: bool) -> Vec<u8> {
        let mut w = Writer::default();
        w.u(4, 0);
        w.flag(true);
        w.flag(true);
        w.u(6, 0);
        w.u(3, 0);
        w.flag(true);
        w.u(16, 0x1234);
        ptl(&mut w, 8);
        w.flag(false);
        w.ue(4);
        w.ue(2);
        w.ue(0);
        w.u(6, 0);
        w.ue(0);
        w.flag(true);
        w.u(32, 1);
        w.u(32, 60);
        w.flag(true);
        w.ue(0);
        w.ue(1);
        w.ue(0);
        w.flag(true);
        w.flag(true);
        w.flag(true);
        w.u(19, 0);
        w.u(12, 0);
        w.u(15, 0);
        w.flag(true);
        w.ue(0);
        w.ue(0);
        for _ in 0..2 {
            for _ in 0..4 {
                w.ue(0);
            }
            w.flag(false);
        }
        w.flag(ignored_extension);
        if ignored_extension {
            w.u(6, 0b101011);
        }
        w.rbsp()
    }

    #[test]
    fn parameter_sets_preserve_header_inputs_and_ignore_reserved_extension_data() {
        let limits = Limits::default();
        let bytes = video(true);
        let v = vps(&mut Bits::new(&bytes), &limits).unwrap();
        assert_eq!(v.id, 0);
        assert_eq!(v.max_dpb[0], 5);
        for depth in [8, 10] {
            let bytes = sequence(depth, false, false, true);
            let s = sps(&mut Bits::new(&bytes), &limits).unwrap();
            assert_eq!(
                (s.width, s.height, s.visible_width, s.visible_height),
                (64, 64, 64, 56)
            );
            assert_eq!(
                (s.bit_depth, s.ctb_log2, s.ctu_count, s.poc_bits),
                (depth, 6, 1, 4)
            );
            assert_eq!((s.rps_count, s.rps[0].used_count()), (1, 1));
            assert_eq!(s.long_term_count, 2);
            assert_eq!(&s.long_term_used[..2], &[true, false]);
            assert!(s.temporal_mvp && s.sao);
        }
        let bytes = picture(true);
        let p = pps(&mut Bits::new(&bytes), &limits).unwrap();
        assert_eq!(
            (p.id, p.sps_id, p.ref_l0, p.ref_l1, p.init_qp),
            (3, 0, 2, 3, 24)
        );
        assert_eq!((p.tile_columns, p.tile_rows), (2, 2));
        assert!(p.dependent_segments && p.output_flag && p.tiles && !p.wpp && p.header_extension);
    }

    #[test]
    fn every_parameter_set_byte_truncation_is_invalid() {
        let limits = Limits::default();
        for (bytes, kind) in [
            (video(false), 0),
            (sequence(10, false, false, false), 1),
            (picture(false), 2),
        ] {
            for length in 0..bytes.len() {
                let mut bits = Bits::new(&bytes[..length]);
                let result = match kind {
                    0 => vps(&mut bits, &limits).map(|_| ()),
                    1 => sps(&mut bits, &limits).map(|_| ()),
                    _ => pps(&mut bits, &limits).map(|_| ()),
                };
                assert!(
                    matches!(result, Err(Error::InvalidData(_))),
                    "kind {kind}, length {length}: {result:?}"
                );
            }
        }
    }

    #[test]
    fn scope_refusals_and_operational_limits_are_distinct() {
        let bytes = sequence(8, true, false, false);
        assert_eq!(
            sps(&mut Bits::new(&bytes), &Limits::default()).unwrap_err(),
            Error::Unsupported("HEVC field sequence")
        );
        let bytes = sequence(8, false, true, false);
        assert_eq!(
            sps(&mut Bits::new(&bytes), &Limits::default()).unwrap_err(),
            Error::Unsupported("HEVC SPS range extension")
        );
        let bytes = sequence(8, false, false, false);
        let limits = Limits {
            max_width: 32,
            ..Limits::default()
        };
        assert_eq!(
            sps(&mut Bits::new(&bytes), &limits).unwrap_err(),
            Error::Limit("HEVC dimensions")
        );
        let limits = Limits {
            max_ctus: 0,
            ..Limits::default()
        };
        assert_eq!(
            sps(&mut Bits::new(&bytes), &limits).unwrap_err(),
            Error::Limit("HEVC CTU count")
        );
    }
}
