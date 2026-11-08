use crate::{
    Error, Limits, NalHeader, Result,
    bits::Bits,
    params::{Pps, Sps},
    rps,
};
use sha2::{Digest, Sha256};

/// One long-term reference derived from slice and active SPS syntax.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LongTermRef {
    /// Low picture-order-count bits.
    pub poc_lsb: u32,
    /// Used by the current picture, rather than retained for a later picture.
    pub used: bool,
    /// Cumulative MSB cycle delta when explicitly signalled.
    pub delta_msb_cycle: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Weight {
    pub weight: i16,
    pub offset: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Weights {
    pub denominators: [u8; 2],
    pub lists: [[[Weight; 3]; 15]; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Collocated {
    pub from_l0: bool,
    pub index: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InterSlice {
    pub active: [u8; 2],
    pub modifications: [Option<[u8; 15]>; 2],
    pub mvd_l1_zero: bool,
    pub collocated: Option<Collocated>,
    pub weights: Option<Weights>,
    pub max_merge_candidates: u8,
}

/// Parsed slice segment syntax; entropy data remains unread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceHeader {
    /// Begins a new coded picture.
    pub first: bool,
    /// Inherits an earlier independent slice's header in this picture.
    pub dependent: bool,
    /// PPS identifier activated by this segment.
    pub pps_id: u32,
    /// Raster-scan CTU address.
    pub address: u32,
    /// B=0, P=1, I=2; absent for dependent segments.
    pub slice_type: Option<u32>,
    /// Low POC bits; zero for IDR, absent for dependent segments.
    pub poc_lsb: Option<u32>,
    /// Output flag; absent for dependent segments.
    pub output: Option<bool>,
    /// SliceQpY after the active PPS offset, absent for dependent segments.
    pub slice_qp_y: Option<i32>,
    /// Combined PPS and slice Cb/Cr QP offsets, absent for dependent segments.
    pub chroma_qp_offsets: Option<[i32; 2]>,
    /// CABAC initialization selection, including inferred false for I slices.
    pub cabac_init_flag: Option<bool>,
    /// Slice-level SAO flags; absent for dependent segments.
    pub sao: Option<[bool; 2]>,
    /// Slice-level deblocking disable flag; absent for dependent segments.
    pub deblock_disabled: Option<bool>,
    /// Effective beta/tC offsets in units of two, absent for dependent segments.
    pub deblock_offsets: Option<[i32; 2]>,
    /// Effective permission to filter across slices, absent for dependent segments.
    pub loop_filter_slices: Option<bool>,
    /// Number of entry point offsets.
    pub entry_points: u32,
    /// Byte-aligned entropy-data start, measured in unescaped RBSP bytes.
    pub rbsp_header_bytes: usize,
    /// IRAP flag controlling previously decoded pictures still awaiting output.
    pub no_output_of_prior_pics: bool,
    /// Short-term reference set, absent only through dependent inheritance.
    pub short_term_rps: rps::Rps,
    /// Fixed long-term reference storage; only `long_term_count` entries are active.
    pub long_term: [LongTermRef; 16],
    /// Active long-term reference entries.
    pub long_term_count: usize,
    pub(crate) reference_syntax: Option<(usize, [u8; 32])>,
    pub(crate) inter: Option<InterSlice>,
}

pub(crate) struct Prefix {
    pub first: bool,
    pub pps_id: u32,
    pub no_output_prior: bool,
}

pub(crate) struct ParsedSlice {
    pub header: SliceHeader,
    pub entry_lengths: Vec<u64>,
}

pub(crate) fn prefix(bits: &mut Bits<'_>, nal: NalHeader) -> Result<Prefix> {
    let first = bits.flag()?;
    let no_output_prior = (16..=23).contains(&nal.unit_type) && bits.flag()?;
    Ok(Prefix {
        first,
        pps_id: bits.ue(63)?,
        no_output_prior,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "header state and allocation boundary"
)]
pub(crate) fn parse(
    bits: &mut Bits<'_>,
    nal: NalHeader,
    prefix: Prefix,
    s: &Sps,
    p: &Pps,
    limits: &Limits,
    retain_entries: Option<usize>,
    reserve_entries: &mut dyn FnMut(usize) -> Result<()>,
) -> Result<ParsedSlice> {
    let mut entry_lengths = Vec::new();
    let mut h = SliceHeader {
        first: prefix.first,
        dependent: false,
        pps_id: prefix.pps_id,
        address: 0,
        slice_type: None,
        poc_lsb: None,
        output: None,
        slice_qp_y: None,
        chroma_qp_offsets: None,
        cabac_init_flag: None,
        sao: None,
        deblock_disabled: None,
        deblock_offsets: None,
        loop_filter_slices: None,
        entry_points: 0,
        rbsp_header_bytes: 0,
        no_output_of_prior_pics: prefix.no_output_prior,
        short_term_rps: rps::Rps::default(),
        long_term: [LongTermRef::default(); 16],
        long_term_count: 0,
        reference_syntax: None,
        inter: None,
    };
    if !h.first {
        h.dependent = p.dependent_segments && bits.flag()?;
        h.address = bits.u(ceil_log2(s.ctu_count))?;
        if h.address == 0 || h.address >= s.ctu_count {
            return Err(Error::InvalidData("slice_segment_address"));
        }
    }
    if !h.dependent {
        h.cabac_init_flag = Some(false);
        bits.skip(p.extra_header_bits as usize)?;
        let kind = bits.ue(2)?;
        if (16..=23).contains(&nal.unit_type) && kind != 2 {
            return Err(Error::InvalidData("irap_slice_type"));
        }
        h.slice_type = Some(kind);
        h.output = Some(!p.output_flag || bits.flag()?);
        let mut total_current = 0;
        let mut temporal_mvp = false;
        if nal.unit_type != 19 && nal.unit_type != 20 {
            h.poc_lsb = Some(bits.u(s.poc_bits)?);
            let mut reference_bits = bits.clone();
            let from_sps = bits.flag()?;
            let set = if !from_sps {
                rps::parse(
                    bits,
                    s.rps
                        .get(..s.rps_count)
                        .ok_or(Error::InvalidData("rps_count"))?,
                    s.rps_count,
                    true,
                )?
            } else {
                if s.rps_count == 0 {
                    return Err(Error::InvalidData("missing_sps_rps"));
                }
                let idx = if s.rps_count > 1 {
                    bits.u(ceil_log2(s.rps_count as u32))? as usize
                } else {
                    0
                };
                *s.rps
                    .get(idx)
                    .filter(|_| idx < s.rps_count)
                    .ok_or(Error::InvalidData("short_term_ref_pic_set_idx"))?
            };
            total_current = set.used_count();
            h.short_term_rps = set;
            if s.long_term_present {
                let from_sps = if s.long_term_count > 0 {
                    bits.ue(s.long_term_count as u32)?
                } else {
                    0
                };
                let extra = bits.ue(16u32
                    .checked_sub(from_sps)
                    .ok_or(Error::InvalidData("long_term_count"))?)?;
                let mut cycle = 0u32;
                for i in 0..from_sps + extra {
                    if i == 0 || i == from_sps {
                        cycle = 0;
                    }
                    let (poc_lsb, used) = if i < from_sps {
                        let idx = if s.long_term_count > 1 {
                            bits.u(ceil_log2(s.long_term_count as u32))? as usize
                        } else {
                            0
                        };
                        (
                            *s.long_term_poc
                                .get(idx)
                                .filter(|_| idx < s.long_term_count)
                                .ok_or(Error::InvalidData("lt_idx_sps"))?,
                            *s.long_term_used
                                .get(idx)
                                .ok_or(Error::InvalidData("lt_idx_sps"))?,
                        )
                    } else {
                        (bits.u(s.poc_bits)?, bits.flag()?)
                    };
                    total_current += u32::from(used);
                    let delta_msb_cycle = if bits.flag()? {
                        let delta = bits.ue(u32::MAX - 1)?;
                        cycle = cycle
                            .checked_add(delta)
                            .ok_or(Error::InvalidData("long_term_msb_cycle"))?;
                        Some(cycle)
                    } else {
                        None
                    };
                    *h.long_term
                        .get_mut(i as usize)
                        .ok_or(Error::InvalidData("long_term_count"))? = LongTermRef {
                        poc_lsb,
                        used,
                        delta_msb_cycle,
                    };
                    h.long_term_count += 1;
                }
            }
            temporal_mvp = s.temporal_mvp && bits.flag()?;
            let count = bits
                .position()
                .checked_sub(reference_bits.position())
                .ok_or(Error::InvalidData("reference_syntax_span"))?;
            let mut identity = Sha256::new();
            for _ in 0..count / 8 {
                identity.update([reference_bits.u(8)? as u8]);
            }
            let remaining = (count % 8) as u32;
            if remaining != 0 {
                identity.update([(reference_bits.u(remaining)? << (8 - remaining)) as u8]);
            }
            h.reference_syntax = Some((count, identity.finalize().into()));
            let dpb = *s
                .max_dpb
                .get(s.sub_layers as usize)
                .ok_or(Error::InvalidData("sps_dpb"))?;
            if h.short_term_rps.count + h.long_term_count >= dpb as usize {
                return Err(Error::InvalidData("slice_reference_count"));
            }
        } else {
            h.poc_lsb = Some(0);
        }
        let sao = if s.sao {
            [bits.flag()?, bits.flag()?]
        } else {
            [false; 2]
        };
        h.sao = Some(sao);
        if kind < 2 {
            if total_current == 0 {
                return Err(Error::InvalidData("empty_reference_list"));
            }
            let (mut l0, mut l1) = (p.ref_l0, p.ref_l1);
            if bits.flag()? {
                l0 = bits.ue(14)? + 1;
                if kind == 0 {
                    l1 = bits.ue(14)? + 1;
                }
            }
            let mut inter = InterSlice {
                active: [l0 as u8, if kind == 0 { l1 as u8 } else { 0 }],
                modifications: [None; 2],
                mvd_l1_zero: false,
                collocated: None,
                weights: None,
                max_merge_candidates: 5,
            };
            if p.lists_modification && total_current > 1 {
                for (modification, count) in inter.modifications.iter_mut().zip(inter.active) {
                    if count > 0 && bits.flag()? {
                        let mut entries = [0; 15];
                        for entry in entries.iter_mut().take(usize::from(count)) {
                            let index = bits.u(ceil_log2(total_current))?;
                            if index >= total_current {
                                return Err(Error::InvalidData("list_entry"));
                            }
                            *entry = index as u8;
                        }
                        *modification = Some(entries);
                    }
                }
            }
            if kind == 0 {
                inter.mvd_l1_zero = bits.flag()?;
            }
            if p.cabac_init {
                h.cabac_init_flag = Some(bits.flag()?);
            }
            if temporal_mvp {
                let from_l0 = kind != 0 || bits.flag()?;
                let count = if from_l0 { l0 } else { l1 };
                let index = if count > 1 { bits.ue(count - 1)? } else { 0 };
                inter.collocated = Some(Collocated {
                    from_l0,
                    index: index as u8,
                });
            }
            if kind == 1 && p.weighted_pred || kind == 0 && p.weighted_bipred {
                inter.weights = Some(weights(bits, l0, if kind == 0 { l1 } else { 0 })?);
            }
            inter.max_merge_candidates = 5 - bits.ue(4)? as u8;
            h.inter = Some(inter);
        }
        let qp = p.init_qp + bits.se(-87, 77)?;
        if qp < -6 * (s.bit_depth as i32 - 8) || qp > 51 {
            return Err(Error::InvalidData("slice_qp"));
        }
        h.slice_qp_y = Some(qp);
        let mut chroma_offsets = [p.cb_qp_offset, p.cr_qp_offset];
        if p.chroma_offsets {
            for offset in &mut chroma_offsets {
                *offset += bits.se(-12, 12)?;
                if !(-12..=12).contains(offset) {
                    return Err(Error::InvalidData("slice_chroma_qp_offset"));
                }
            }
        }
        h.chroma_qp_offsets = Some(chroma_offsets);
        let mut disabled = p.deblock_disabled;
        let mut offsets = p.deblock_offsets;
        if p.deblock_override && bits.flag()? {
            disabled = bits.flag()?;
            if !disabled {
                offsets = [bits.se(-6, 6)?, bits.se(-6, 6)?];
            }
        }
        h.deblock_disabled = Some(disabled);
        h.deblock_offsets = Some(offsets);
        let mut across = p.loop_filter_slices;
        if p.loop_filter_slices && (sao.iter().any(|x| *x) || !disabled) {
            across = bits.flag()?;
        }
        h.loop_filter_slices = Some(across);
    }
    if p.tiles || p.wpp {
        h.entry_points = bits.ue(s.ctu_count.saturating_sub(1))?;
        if h.entry_points > limits.max_ctus {
            return Err(Error::Limit("max_ctus"));
        }
        if h.entry_points > 0 {
            if let Some(memory_limit) = retain_entries {
                if (h.entry_points as usize)
                    .checked_mul(core::mem::size_of::<u64>())
                    .ok_or(Error::Limit("reconstruction_memory"))?
                    > memory_limit
                {
                    return Err(Error::Limit("reconstruction_memory"));
                }
                reserve_entries(
                    (h.entry_points as usize)
                        .checked_mul(core::mem::size_of::<u64>())
                        .ok_or(Error::Limit("reconstruction_memory"))?,
                )?;
                entry_lengths
                    .try_reserve_exact(h.entry_points as usize)
                    .map_err(|_| Error::Limit("entry_point_allocation"))?;
                reserve_entries(
                    entry_lengths
                        .capacity()
                        .checked_mul(core::mem::size_of::<u64>())
                        .ok_or(Error::Limit("reconstruction_memory"))?,
                )?;
            }
            let width = bits.ue(31)? + 1;
            let mut total = 0u64;
            for _ in 0..h.entry_points {
                let length = u64::from(bits.u(width)?) + 1;
                total += length;
                if retain_entries.is_some() {
                    entry_lengths.push(length);
                }
            }
            if total > limits.max_nal_bytes as u64 {
                return Err(Error::InvalidData("entry_point_offsets"));
            }
        }
    }
    if p.header_extension {
        let len = bits.ue(u32::MAX - 1)? as usize;
        if len > limits.max_nal_bytes {
            return Err(Error::Limit("slice_header_extension"));
        }
        bits.skip(
            len.checked_mul(8)
                .ok_or(Error::Limit("slice_header_extension"))?,
        )?;
    }
    bits.alignment()?;
    h.rbsp_header_bytes = bits.position() / 8;
    Ok(ParsedSlice {
        header: h,
        entry_lengths,
    })
}

pub(crate) fn ceil_log2(value: u32) -> u32 {
    32 - value.saturating_sub(1).leading_zeros()
}

fn weights(bits: &mut Bits<'_>, l0: u32, l1: u32) -> Result<Weights> {
    let denom = bits.ue(7)? as i32;
    let chroma_denom = denom + bits.se(-7, 7)?;
    if !(0..=7).contains(&chroma_denom) {
        return Err(Error::InvalidData("chroma_weight_denom"));
    }
    let luma_default = Weight {
        weight: 1 << denom,
        offset: 0,
    };
    let chroma_default = Weight {
        weight: 1 << chroma_denom,
        offset: 0,
    };
    let mut result = Weights {
        denominators: [denom as u8, chroma_denom as u8],
        lists: [[[luma_default, chroma_default, chroma_default]; 15]; 2],
    };
    let mut total_flags = 0;
    for (entries, count) in result.lists.iter_mut().zip([l0, l1]) {
        let mut luma = [false; 15];
        let mut chroma = [false; 15];
        for flag in luma.iter_mut().take(count as usize) {
            *flag = bits.flag()?;
            total_flags += u32::from(*flag);
        }
        for flag in chroma.iter_mut().take(count as usize) {
            *flag = bits.flag()?;
            total_flags += 2 * u32::from(*flag);
        }
        for ((entry, &y), &c) in entries
            .iter_mut()
            .zip(&luma)
            .zip(&chroma)
            .take(count as usize)
        {
            if y {
                let value = entry.first_mut().ok_or(Error::InvalidData("luma_weight"))?;
                value.weight += bits.se(-128, 127)? as i16;
                value.offset = bits.se(-128, 127)? as i16;
            }
            if c {
                for value in entry.iter_mut().skip(1) {
                    value.weight += bits.se(-128, 127)? as i16;
                    let delta_offset = bits.se(-512, 511)?;
                    value.offset = (128 + delta_offset
                        - ((128 * i32::from(value.weight)) >> chroma_denom))
                        .clamp(-128, 127) as i16;
                }
            }
        }
    }
    if total_flags > 24 {
        return Err(Error::InvalidData("weight_flag_count"));
    }
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::params::tests::Writer;

    #[test]
    fn weights_retain_both_lists_defaults_and_clipped_chroma_offsets() -> Result<()> {
        let mut writer = Writer::default();
        writer.ue(2);
        writer.se(-1);
        for flag in [true, false, true, false] {
            writer.flag(flag);
        }
        for value in [-3, -128, 1, 0, -3, 511] {
            writer.se(value);
        }
        writer.flag(false);
        writer.flag(true);
        for value in [0, -512, 127, 511] {
            writer.se(value);
        }
        let bytes = writer.bytes();
        let mut bits = Bits::new(&bytes);
        let parsed = weights(&mut bits, 2, 1)?;
        assert_eq!(bits.position(), writer.len());
        assert_eq!(parsed.denominators, [2, 1]);
        assert_eq!(
            parsed.lists[0][0],
            [
                Weight {
                    weight: 1,
                    offset: -128
                },
                Weight {
                    weight: 3,
                    offset: -64
                },
                Weight {
                    weight: -1,
                    offset: 127
                },
            ]
        );
        assert_eq!(
            parsed.lists[0][1],
            [
                Weight {
                    weight: 4,
                    offset: 0
                },
                Weight {
                    weight: 2,
                    offset: 0
                },
                Weight {
                    weight: 2,
                    offset: 0
                },
            ]
        );
        assert_eq!(
            parsed.lists[1][0],
            [
                Weight {
                    weight: 4,
                    offset: 0
                },
                Weight {
                    weight: 2,
                    offset: -128
                },
                Weight {
                    weight: 129,
                    offset: -128
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn weighted_prediction_accepts_24_flags_but_refuses_27() {
        for (l0, l1, valid) in [(4, 4, true), (5, 4, false)] {
            let mut writer = Writer::default();
            writer.ue(0);
            writer.se(0);
            for count in [l0, l1] {
                for _ in 0..count * 2 {
                    writer.flag(true);
                }
                for _ in 0..count * 6 {
                    writer.se(0);
                }
            }
            let bytes = writer.bytes();
            let result = weights(&mut Bits::new(&bytes), l0, l1);
            if valid {
                assert!(result.is_ok());
            } else {
                assert_eq!(result, Err(Error::InvalidData("weight_flag_count")));
            }
        }
    }

    #[test]
    fn weighted_prediction_refuses_invalid_chroma_denominators_and_truncation() {
        for (denom, delta) in [(0, -1), (7, 1)] {
            let mut writer = Writer::default();
            writer.ue(denom);
            writer.se(delta);
            let bytes = writer.bytes();
            assert_eq!(
                weights(&mut Bits::new(&bytes), 1, 0),
                Err(Error::InvalidData("chroma_weight_denom"))
            );
        }
        assert_eq!(
            weights(&mut Bits::new(&[]), 1, 0),
            Err(Error::InvalidData("truncated_header"))
        );
    }
}
