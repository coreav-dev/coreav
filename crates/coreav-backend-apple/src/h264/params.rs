use super::bits::{Bits, rbsp};
use super::{Color, Dimensions, Error, Limits, header};

#[derive(Clone, Copy, Debug)]
pub(super) enum Poc {
    Lsb(u8),
    FrameNumber,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Sps {
    pub id: u32,
    pub constraints: u8,
    pub level: u8,
    pub frame_bits: u8,
    pub poc: Poc,
    pub macroblocks: u64,
    pub color: Option<Color>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Pps {
    pub id: u32,
    pub bottom_poc: bool,
}

pub(super) fn sps(nal: &[u8], expected: Dimensions, limits: Limits) -> Result<Sps, Error> {
    if header(nal)? & 31 != 7 {
        return Err(Error::Malformed);
    }
    let body = rbsp(nal)?;
    let mut r = Bits::new(&body)?;
    if r.bits(8)? != 100 {
        return Err(Error::Unsupported);
    }
    let constraints = r.bits(8)? as u8;
    if constraints & 0xe3 != 0 {
        return Err(Error::Malformed);
    }
    if constraints & 0x10 != 0 {
        return Err(Error::Unsupported);
    }
    let level = r.bits(8)? as u8;
    if !matches!(level, 10..=13 | 20..=22 | 30..=32 | 40..=42 | 50..=52 | 60..=62) {
        return Err(Error::Unsupported);
    }
    let id = r.bounded(31)?;
    if r.bounded(3)? != 1 || r.bounded(6)? != 0 || r.bounded(6)? != 0 || r.bit()? || r.bit()? {
        return Err(Error::Unsupported);
    }
    let frame_bits = r.bounded(12)? as u8 + 4;
    let poc = match r.bounded(2)? {
        0 => Poc::Lsb(r.bounded(12)? as u8 + 4),
        2 => Poc::FrameNumber,
        _ => return Err(Error::Unsupported),
    };
    let references = r.bounded(16)?;
    if r.bit()? {
        return Err(Error::Unsupported);
    }
    let width_mbs = r.ue()?.checked_add(1).ok_or(Error::Malformed)?;
    let height_mbs = r.ue()?.checked_add(1).ok_or(Error::Malformed)?;
    let coded_width = width_mbs.checked_mul(16).ok_or(Error::Malformed)?;
    let coded_height = height_mbs.checked_mul(16).ok_or(Error::Malformed)?;
    if !r.bit()? {
        return Err(Error::Unsupported);
    }
    r.bit()?;
    let (crop_x, crop_y) = if r.bit()? {
        let left = r.ue()?;
        let right = r.ue()?;
        let top = r.ue()?;
        let bottom = r.ue()?;
        (
            left.checked_add(right)
                .and_then(|n| n.checked_mul(2))
                .ok_or(Error::Malformed)?,
            top.checked_add(bottom)
                .and_then(|n| n.checked_mul(2))
                .ok_or(Error::Malformed)?,
        )
    } else {
        (0, 0)
    };
    let width = coded_width
        .checked_sub(crop_x)
        .filter(|&n| n != 0)
        .ok_or(Error::Malformed)?;
    let height = coded_height
        .checked_sub(crop_y)
        .filter(|&n| n != 0)
        .ok_or(Error::Malformed)?;
    if coded_width > limits.max_width || coded_height > limits.max_height {
        return Err(Error::Limit);
    }
    if Dimensions::new(width, height)? != expected {
        return Err(Error::FormatMismatch);
    }
    let color = if r.bit()? {
        vui(&mut r, references)?
    } else {
        None
    };
    r.finish()?;
    Ok(Sps {
        id,
        constraints,
        level,
        frame_bits,
        poc,
        macroblocks: u64::from(width_mbs) * u64::from(height_mbs),
        color,
    })
}

pub(super) fn pps(nal: &[u8], sps: Sps) -> Result<Pps, Error> {
    if header(nal)? & 31 != 8 {
        return Err(Error::Malformed);
    }
    let body = rbsp(nal)?;
    let mut r = Bits::new(&body)?;
    let id = r.bounded(255)?;
    if r.bounded(31)? != sps.id {
        return Err(Error::ParameterSet);
    }
    r.bit()?;
    let bottom_poc = r.bit()?;
    if r.bounded(7)? != 0 {
        return Err(Error::Unsupported);
    }
    r.bounded(31)?;
    r.bounded(31)?;
    r.bit()?;
    if r.bits(2)? == 3 {
        return Err(Error::Malformed);
    }
    r.signed(-26, 25)?;
    r.signed(-26, 25)?;
    r.signed(-12, 12)?;
    r.bit()?;
    r.bit()?;
    if r.bit()? {
        return Err(Error::Unsupported);
    }
    if r.more() {
        r.bit()?;
        if r.bit()? {
            return Err(Error::Unsupported);
        }
        r.signed(-12, 12)?;
    }
    r.finish()?;
    Ok(Pps { id, bottom_poc })
}

fn vui(r: &mut Bits<'_>, references: u32) -> Result<Option<Color>, Error> {
    let mut color = None;
    if r.bit()? {
        match r.bits(8)? {
            255 => {
                r.bits(16)?;
                r.bits(16)?;
            }
            0..=16 => {}
            _ => return Err(Error::Unsupported),
        }
    }
    if r.bit()? {
        r.bit()?;
    }
    if r.bit()? {
        if r.bits(3)? > 5 {
            return Err(Error::Malformed);
        }
        let full_range = r.bit()?;
        if r.bit()? {
            color = Some(Color {
                full_range,
                primaries: r.bits(8)? as u8,
                transfer: r.bits(8)? as u8,
                matrix: r.bits(8)? as u8,
            });
        }
    }
    if r.bit()? {
        r.bounded(5)?;
        r.bounded(5)?;
    }
    if r.bit()? {
        let tick = r.bits(32)?;
        let scale = r.bits(32)?;
        if tick == 0 || scale == 0 {
            return Err(Error::Malformed);
        }
        r.bit()?;
    }
    let nal_hrd = r.bit()?;
    if nal_hrd {
        hrd(r)?;
    }
    let vcl_hrd = r.bit()?;
    if vcl_hrd {
        hrd(r)?;
    }
    if nal_hrd || vcl_hrd {
        r.bit()?;
    }
    r.bit()?;
    if r.bit()? {
        r.bit()?;
        r.bounded(16)?;
        r.bounded(16)?;
        r.bounded(16)?;
        r.bounded(16)?;
        if r.ue()? != 0 {
            return Err(Error::Unsupported);
        }
        if r.bounded(16)? < references {
            return Err(Error::Malformed);
        }
    }
    Ok(color)
}

fn hrd(r: &mut Bits<'_>) -> Result<(), Error> {
    let count = r.bounded(31)? + 1;
    r.bits(4)?;
    r.bits(4)?;
    for _ in 0..count {
        r.ue()?;
        r.ue()?;
        r.bit()?;
    }
    for _ in 0..4 {
        r.bits(5)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Picture {
    idr: bool,
    reference: bool,
    frame_num: u32,
    idr_id: u32,
    poc_lsb: u32,
    bottom_delta: i32,
}

pub(super) struct Slice {
    pub first_mb: u32,
    pub picture: Picture,
    pub intra: bool,
}

pub(super) fn slice(nal: &[u8], sps: Sps, pps: Pps) -> Result<Slice, Error> {
    let head = header(nal)?;
    let idr = head & 31 == 5;
    let body = rbsp(nal)?;
    let mut r = Bits::new(&body)?;
    let first_mb = r.ue()?;
    if u64::from(first_mb) >= sps.macroblocks {
        return Err(Error::Malformed);
    }
    let kind = r.bounded(9)? % 5;
    if !matches!(kind, 0 | 2) {
        return Err(Error::Unsupported);
    }
    if idr && kind != 2 {
        return Err(Error::Malformed);
    }
    if r.bounded(255)? != pps.id {
        return Err(Error::ParameterSet);
    }
    let frame_num = r.bits(sps.frame_bits)?;
    if idr && frame_num != 0 {
        return Err(Error::Malformed);
    }
    let idr_id = if idr { r.bounded(65535)? } else { 0 };
    let (poc_lsb, bottom_delta) = match sps.poc {
        Poc::Lsb(width) => (r.bits(width)?, if pps.bottom_poc { r.se()? } else { 0 }),
        Poc::FrameNumber => (0, 0),
    };
    if !r.more() {
        return Err(Error::Malformed);
    }
    Ok(Slice {
        first_mb,
        intra: kind == 2,
        picture: Picture {
            idr,
            reference: head & 0x60 != 0,
            frame_num,
            idr_id,
            poc_lsb,
            bottom_delta,
        },
    })
}
