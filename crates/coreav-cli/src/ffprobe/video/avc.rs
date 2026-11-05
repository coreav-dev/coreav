use super::{AvcSps, Bits, avc_sps_read, strict_rbsp};

fn parameter_rbsp(nal: &[u8], kind: u8) -> Option<Vec<u8>> {
    let header = *nal.first()?;
    if header & 0x9f != kind || header & 0x60 == 0 {
        return None;
    }
    strict_rbsp(nal, 1)
}

fn pps(nal: &[u8], sps: &AvcSps) -> Option<u32> {
    let data = parameter_rbsp(nal, 8)?;
    let mut b = Bits::new(&data);
    let id = b.ue()?;
    if id > 255 || b.ue()? != sps.id {
        return None;
    }
    if b.flag()? && matches!(sps.profile, 66 | 88) {
        return None;
    }
    b.bit()?;
    if b.ue()? != 0 || b.ue()? > 31 || b.ue()? > 31 {
        return None;
    }
    b.bit()?;
    if b.bits(2)? > 2
        || !(-(26 + 6 * i64::from(sps.depth - 8))..=25).contains(&b.se()?)
        || !(-26..=25).contains(&b.se()?)
        || !(-12..=12).contains(&b.se()?)
    {
        return None;
    }
    b.bits(3)?;
    if !b.trailing() {
        let transform = b.flag()?;
        if transform && matches!(sps.profile, 66 | 77 | 88) {
            return None;
        }
        if b.flag()? {
            let count = 6 + if transform {
                if sps.chroma == 3 { 6 } else { 2 }
            } else {
                0
            };
            for i in 0..count {
                if b.flag()? {
                    let (mut last, mut next) = (8i64, 8i64);
                    for _ in 0..if i < 6 { 16 } else { 64 } {
                        if next != 0 {
                            let delta = b.se()?;
                            if !(-128..=127).contains(&delta) {
                                return None;
                            }
                            next = (last + delta + 256) % 256;
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
        if !(-12..=12).contains(&b.se()?) {
            return None;
        }
    }
    b.trailing().then_some(id)
}

pub(super) struct AvcConfig<'a> {
    sps: &'a [u8],
    pps: &'a [u8],
    pps_id: u32,
    width: usize,
}

pub(super) fn configuration(avcc: &[u8]) -> Option<AvcConfig<'_>> {
    if avcc.first() != Some(&1) || avcc.get(5) != Some(&0xe1) {
        return None;
    }
    let sets = coreav_isobmff::avc_parameter_sets(avcc)?;
    let [sps_nal, pps_nal] = sets.as_slice() else {
        return None;
    };
    let sps_data = parameter_rbsp(sps_nal, 7)?;
    if avcc.get(1..4)? != sps_data.get(..3)? {
        return None;
    }
    let (_, sps) = avc_sps_read(sps_nal, false)?;
    let sps = sps?;
    let pps_id = pps(pps_nal, &sps)?;
    let end = 6usize
        .checked_add(2 + sps_nal.len())?
        .checked_add(3 + pps_nal.len())?;
    let suffix = avcc.get(end..)?;
    if !suffix.is_empty()
        && (!matches!(avcc.get(1), Some(100 | 110 | 122 | 144))
            || suffix
                != [
                    0xfc | u8::try_from(sps.chroma).ok()?,
                    0xf8 | u8::try_from(sps.depth - 8).ok()?,
                    0xf8 | u8::try_from(sps.depth - 8).ok()?,
                    0,
                ])
    {
        return None;
    }
    let length_byte = *avcc.get(4)?;
    if length_byte & 0xfc != 0xfc || length_byte & 3 == 2 {
        return None;
    }
    Some(AvcConfig {
        sps: sps_nal,
        pps: pps_nal,
        pps_id,
        width: usize::from((length_byte & 3) + 1),
    })
}

pub(crate) fn summary(avcc: &[u8], sample: &[u8]) -> Option<()> {
    let config = configuration(avcc)?;
    let mut at = 0usize;
    let mut slices = 0usize;
    while at < sample.len() {
        let mut length = 0usize;
        for &byte in sample.get(at..at.checked_add(config.width)?)? {
            length = length.checked_mul(256)?.checked_add(usize::from(byte))?;
        }
        at = at.checked_add(config.width)?;
        let nal = sample.get(at..at.checked_add(length)?)?;
        let header = *nal.first()?;
        if header & 0x80 != 0 {
            return None;
        }
        match header & 31 {
            1 | 5 => {
                if header & 31 == 5 && header & 0x60 == 0 {
                    return None;
                }
                let prefix = super::rbsp(nal.get(..nal.len().min(32))?, 1);
                let mut b = Bits::new(&prefix);
                b.ue()?;
                if b.ue()? > 9 || b.ue()? != config.pps_id {
                    return None;
                }
                slices += 1;
            }
            6 | 9 | 12 if header & 0x60 == 0 => {}
            7 if nal == config.sps => {}
            8 if nal == config.pps => {}
            _ => return None,
        }
        at = at.checked_add(length)?;
    }
    (slices > 0).then_some(())
}
