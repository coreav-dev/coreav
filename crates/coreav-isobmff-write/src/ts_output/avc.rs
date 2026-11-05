use super::{Limits, Work, limit, reserve, unsupported};
use crate::ExtractError;
use crate::matroska::Bits;
use coreav_isobmff::{AvcCopyFacts, LimitKind, PlanningBudget};

pub(super) struct Configuration<'a> {
    pub(super) facts: AvcCopyFacts,
    sps: &'a [u8],
    pps: &'a [u8],
    pps_id: u32,
}

pub(super) struct Conversion<'a> {
    pub(super) limits: &'a Limits,
    pub(super) planning: &'a PlanningBudget,
    pub(super) work: &'a mut Work,
    pub(super) nals: u64,
}

impl<'a> Configuration<'a> {
    pub(super) fn read(record: &'a [u8], planning: &PlanningBudget) -> Result<Self, ExtractError> {
        let sets = coreav_isobmff::avc_parameter_sets_with_planning(record, Some(planning))
            .map_err(|_| limit(LimitKind::PlanningBytes))?
            .ok_or_else(|| unsupported("incomplete AVC configuration"))?;
        let [sps, pps] = sets.as_slice() else {
            return Err(unsupported("MPEG-TS copy requires one SPS and one PPS"));
        };
        if sps
            .first()
            .is_none_or(|head| head & 0x9f != 7 || head & 0x60 == 0)
            || pps
                .first()
                .is_none_or(|head| head & 0x9f != 8 || head & 0x60 == 0)
        {
            return Err(unsupported("invalid AVC parameter-set NAL header"));
        }
        let canonical = crate::matroska::avc_record(&sets, Some(planning)).ok_or_else(|| {
            if planning.failed() {
                limit(LimitKind::PlanningBytes)
            } else {
                unsupported("unqualified canonical AVC configuration")
            }
        })?;
        if canonical != record {
            return Err(unsupported(
                "MPEG-TS copy requires a complete canonical AVC record",
            ));
        }
        let facts = coreav_isobmff::avc_copy_facts(record)
            .filter(|facts| {
                facts.chroma_format_idc == 1
                    && facts.bit_depth_luma_minus8 == 0
                    && facts.bit_depth_chroma_minus8 == 0
                    && facts.frame_mbs_only
            })
            .ok_or_else(|| unsupported("unqualified complete AVC transport headers"))?;
        let mut bits = Bits::new(pps.get(1..).unwrap_or_default());
        let pps_id = bits
            .ue()
            .filter(|id| *id <= 255)
            .ok_or_else(|| unsupported("invalid AVC picture parameter-set identity"))?;
        if bits.ue() != Some(facts.sps_id) {
            return Err(unsupported("AVC parameter-set identities disagree"));
        }
        Ok(Self {
            facts,
            sps,
            pps,
            pps_id,
        })
    }

    pub(super) fn convert(
        &self,
        bytes: &[u8],
        keyframe: bool,
        first: bool,
        conversion: &mut Conversion<'_>,
    ) -> Result<Vec<u8>, ExtractError> {
        let limits = conversion.limits;
        let planning = conversion.planning;
        let work = &mut conversion.work;
        let total = &mut conversion.nals;
        if bytes.len() > limits.max_access_unit_bytes {
            return Err(limit(LimitKind::PacketBytes));
        }
        let mut at = 0usize;
        let mut count = 0usize;
        let mut sei = None;
        let mut vcl = None;
        let mut idr = false;
        while at < bytes.len() {
            work.charge(1)?;
            count = count
                .checked_add(1)
                .filter(|count| *count <= limits.max_nals_per_sample)
                .ok_or_else(|| limit(LimitKind::SampleWork))?;
            *total = total
                .checked_add(1)
                .filter(|count| *count <= limits.max_nals)
                .ok_or_else(|| limit(LimitKind::SampleWork))?;
            let len = bytes
                .get(
                    at..at
                        .checked_add(4)
                        .ok_or_else(|| limit(LimitKind::PacketBytes))?,
                )
                .and_then(|head| <[u8; 4]>::try_from(head).ok())
                .map(u32::from_be_bytes)
                .and_then(|len| usize::try_from(len).ok())
                .filter(|len| *len > 0)
                .ok_or_else(|| unsupported("invalid length-prefixed AVC NAL"))?;
            at = at
                .checked_add(4)
                .ok_or_else(|| limit(LimitKind::PacketBytes))?;
            let end = at
                .checked_add(len)
                .ok_or_else(|| limit(LimitKind::PacketBytes))?;
            let nal = bytes
                .get(at..end)
                .ok_or_else(|| unsupported("truncated AVC NAL"))?;
            let head = *nal.first().ok_or_else(|| unsupported("empty AVC NAL"))?;
            if head & 0x80 != 0 {
                return Err(unsupported("AVC forbidden NAL bit"));
            }
            match head & 31 {
                6 if head == 6 && sei.is_none() && vcl.is_none() => {
                    user_data_sei(nal)?;
                    sei = Some(nal);
                }
                kind @ (1 | 5) if vcl.is_none() => {
                    idr = kind == 5;
                    if idr && head & 0x60 == 0 {
                        return Err(unsupported("unreferenced AVC IDR picture"));
                    }
                    let mut bits = Bits::new(nal.get(1..).unwrap_or_default());
                    if bits.ue() != Some(0)
                        || !bits.ue().is_some_and(|kind| matches!(kind, 0..=2 | 5..=7))
                        || bits.ue() != Some(self.pps_id)
                    {
                        return Err(unsupported("unqualified AVC slice header"));
                    }
                    vcl = Some(nal);
                }
                _ => return Err(unsupported("unqualified AVC NAL layout for MPEG-TS copy")),
            }
            at = end;
        }
        let vcl = vcl.ok_or_else(|| unsupported("AVC sample without a coded picture"))?;
        if idr != keyframe || (first && !idr) || (sei.is_some() && !idr) {
            return Err(unsupported("unqualified AVC random-access layout"));
        }
        let mut size = 6usize;
        if let Some(sei) = sei {
            size = size
                .checked_add(4)
                .and_then(|size| size.checked_add(sei.len()))
                .ok_or_else(|| limit(LimitKind::PacketBytes))?;
        }
        if idr {
            for parameter in [self.sps, self.pps] {
                size = size
                    .checked_add(4)
                    .and_then(|size| size.checked_add(parameter.len()))
                    .ok_or_else(|| limit(LimitKind::PacketBytes))?;
            }
        }
        let prefix: &[u8] = if idr || sei.is_some() {
            &[0, 0, 1]
        } else {
            &[0, 0, 0, 1]
        };
        size = size
            .checked_add(prefix.len())
            .and_then(|size| size.checked_add(vcl.len()))
            .filter(|size| *size <= limits.max_access_unit_bytes)
            .ok_or_else(|| limit(LimitKind::PacketBytes))?;
        let mut converted = Vec::new();
        reserve(planning, &mut converted, size)?;
        converted.extend_from_slice(&[0, 0, 0, 1, 9, 0xf0]);
        if let Some(sei) = sei {
            converted.extend_from_slice(&[0, 0, 0, 1]);
            converted.extend_from_slice(sei);
        }
        if idr {
            for parameter in [self.sps, self.pps] {
                converted.extend_from_slice(&[0, 0, 0, 1]);
                converted.extend_from_slice(parameter);
            }
        }
        converted.extend_from_slice(prefix);
        converted.extend_from_slice(vcl);
        Ok(converted)
    }
}

fn user_data_sei(nal: &[u8]) -> Result<(), ExtractError> {
    if nal
        .windows(3)
        .any(|bytes| bytes.get(..2) == Some(&[0, 0]) && bytes.get(2).is_some_and(|byte| *byte <= 3))
    {
        return Err(unsupported("unqualified escaped AVC SEI"));
    }
    let mut at = 1usize;
    let mut value = 0usize;
    loop {
        let byte = usize::from(
            *nal.get(at)
                .ok_or_else(|| unsupported("short AVC SEI type"))?,
        );
        at += 1;
        value = value
            .checked_add(byte)
            .ok_or_else(|| unsupported("AVC SEI type overflow"))?;
        if byte != 255 {
            break;
        }
    }
    if value != 5 {
        return Err(unsupported("AVC SEI other than unregistered user data"));
    }
    let mut size = 0usize;
    loop {
        let byte = usize::from(
            *nal.get(at)
                .ok_or_else(|| unsupported("short AVC SEI size"))?,
        );
        at += 1;
        size = size
            .checked_add(byte)
            .ok_or_else(|| unsupported("AVC SEI size overflow"))?;
        if byte != 255 {
            break;
        }
    }
    if size < 16
        || at.checked_add(size).and_then(|end| end.checked_add(1)) != Some(nal.len())
        || nal.last() != Some(&0x80)
    {
        return Err(unsupported("unqualified complete AVC user-data SEI"));
    }
    Ok(())
}
