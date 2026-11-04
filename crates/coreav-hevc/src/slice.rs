use crate::{
    Error, Limits, NalHeader, Result,
    bits::Bits,
    params::{Pps, Sps},
    rps,
};

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
    /// Slice-level SAO flags; absent for dependent segments.
    pub sao: Option<[bool; 2]>,
    /// Slice-level deblocking disable flag; absent for dependent segments.
    pub deblock_disabled: Option<bool>,
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
}

pub(crate) struct Prefix {
    pub first: bool,
    pub pps_id: u32,
    pub no_output_prior: bool,
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

pub(crate) fn parse(
    bits: &mut Bits<'_>,
    nal: NalHeader,
    prefix: Prefix,
    s: &Sps,
    p: &Pps,
    limits: &Limits,
) -> Result<SliceHeader> {
    let mut h = SliceHeader {
        first: prefix.first,
        dependent: false,
        pps_id: prefix.pps_id,
        address: 0,
        slice_type: None,
        poc_lsb: None,
        output: None,
        sao: None,
        deblock_disabled: None,
        entry_points: 0,
        rbsp_header_bytes: 0,
        no_output_of_prior_pics: prefix.no_output_prior,
        short_term_rps: rps::Rps::default(),
        long_term: [LongTermRef::default(); 16],
        long_term_count: 0,
    };
    if !h.first {
        h.dependent = p.dependent_segments && bits.flag()?;
        h.address = bits.u(ceil_log2(s.ctu_count))?;
        if h.address == 0 || h.address >= s.ctu_count {
            return Err(Error::InvalidData("slice_segment_address"));
        }
    }
    if !h.dependent {
        bits.skip(p.extra_header_bits as usize)?;
        let kind = bits.ue(2)?;
        h.slice_type = Some(kind);
        h.output = Some(!p.output_flag || bits.flag()?);
        let mut total_current = 0;
        let mut temporal_mvp = false;
        if nal.unit_type != 19 && nal.unit_type != 20 {
            h.poc_lsb = Some(bits.u(s.poc_bits)?);
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
            if p.lists_modification && total_current > 1 {
                for count in [l0, if kind == 0 { l1 } else { 0 }] {
                    if count > 0 && bits.flag()? {
                        for _ in 0..count {
                            if bits.u(ceil_log2(total_current))? >= total_current {
                                return Err(Error::InvalidData("list_entry"));
                            }
                        }
                    }
                }
            }
            if kind == 0 {
                bits.flag()?;
            }
            if p.cabac_init {
                bits.flag()?;
            }
            if temporal_mvp {
                let from_l0 = kind != 0 || bits.flag()?;
                let count = if from_l0 { l0 } else { l1 };
                if count > 1 {
                    bits.ue(count - 1)?;
                }
            }
            if kind == 1 && p.weighted_pred || kind == 0 && p.weighted_bipred {
                weights(bits, l0, if kind == 0 { l1 } else { 0 })?;
            }
            bits.ue(4)?;
        }
        let qp = p.init_qp + bits.se(-87, 77)?;
        if qp < -6 * (s.bit_depth as i32 - 8) || qp > 51 {
            return Err(Error::InvalidData("slice_qp"));
        }
        if p.chroma_offsets
            && (!(-12..=12).contains(&(p.cb_qp_offset + bits.se(-12, 12)?))
                || !(-12..=12).contains(&(p.cr_qp_offset + bits.se(-12, 12)?)))
        {
            return Err(Error::InvalidData("slice_chroma_qp_offset"));
        }
        let mut disabled = p.deblock_disabled;
        if p.deblock_override && bits.flag()? {
            disabled = bits.flag()?;
            if !disabled {
                bits.se(-6, 6)?;
                bits.se(-6, 6)?;
            }
        }
        h.deblock_disabled = Some(disabled);
        if p.loop_filter_slices && (sao.iter().any(|x| *x) || !disabled) {
            bits.flag()?;
        }
    }
    if p.tiles || p.wpp {
        h.entry_points = bits.ue(s.ctu_count.saturating_sub(1))?;
        if h.entry_points > limits.max_ctus {
            return Err(Error::Limit("max_ctus"));
        }
        if h.entry_points > 0 {
            let width = bits.ue(31)? + 1;
            let mut total = 0u64;
            for _ in 0..h.entry_points {
                total += u64::from(bits.u(width)?) + 1;
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
    Ok(h)
}

pub(crate) fn ceil_log2(value: u32) -> u32 {
    32 - value.saturating_sub(1).leading_zeros()
}

fn weights(bits: &mut Bits<'_>, l0: u32, l1: u32) -> Result<()> {
    let denom = bits.ue(7)? as i32;
    let chroma_denom = denom + bits.se(-7, 7)?;
    if !(0..=7).contains(&chroma_denom) {
        return Err(Error::InvalidData("chroma_weight_denom"));
    }
    let mut total_flags = 0;
    for count in [l0, l1] {
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
        for (&y, &c) in luma.iter().zip(&chroma).take(count as usize) {
            if y {
                bits.se(-128, 127)?;
                bits.se(-128, 127)?;
            }
            if c {
                for _ in 0..2 {
                    bits.se(-128, 127)?;
                    bits.se(-512, 511)?;
                }
            }
        }
    }
    if total_flags > 24 {
        return Err(Error::InvalidData("weight_flag_count"));
    }
    Ok(())
}
