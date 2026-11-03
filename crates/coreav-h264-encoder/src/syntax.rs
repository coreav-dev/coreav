use crate::{Encoder, Result, bits::Bits};

pub(crate) fn sps(w: &mut Bits, e: &Encoder) -> Result<()> {
    w.u(8, 66)?;
    w.u(8, 0xc0)?;
    w.u(8, 51)?;
    w.ue(0)?;
    w.ue(0)?;
    w.ue(2)?;
    w.ue(1)?;
    w.flag(false)?;
    w.ue((e.coded_width / 16 - 1) as u32)?;
    w.ue((e.coded_height / 16 - 1) as u32)?;
    w.flag(true)?;
    w.flag(true)?;
    let crop = e.coded_width != e.settings.width || e.coded_height != e.settings.height;
    w.flag(crop)?;
    if crop {
        w.ue(0)?;
        w.ue(((e.coded_width - e.settings.width) / 2) as u32)?;
        w.ue(0)?;
        w.ue(((e.coded_height - e.settings.height) / 2) as u32)?;
    }
    w.flag(true)?;
    w.flag(false)?;
    w.flag(false)?;
    w.flag(true)?;
    w.u(3, 5)?;
    w.flag(e.settings.colour.full_range)?;
    w.flag(true)?;
    w.u(8, u64::from(e.settings.colour.primaries))?;
    w.u(8, u64::from(e.settings.colour.transfer))?;
    w.u(8, u64::from(e.settings.colour.matrix))?;
    w.flag(true)?;
    w.ue(u32::from(e.settings.colour.chroma_location))?;
    w.ue(u32::from(e.settings.colour.chroma_location))?;
    w.flag(true)?;
    w.u(32, 1)?;
    w.u(32, 60)?;
    w.flag(true)?;
    w.flag(false)?;
    w.flag(false)?;
    w.flag(false)?;
    w.flag(false)
}

pub(crate) fn pps(w: &mut Bits) -> Result<()> {
    w.ue(0)?;
    w.ue(0)?;
    w.flag(false)?;
    w.flag(false)?;
    w.ue(0)?;
    w.ue(0)?;
    w.ue(0)?;
    w.flag(false)?;
    w.u(2, 0)?;
    w.se(0)?;
    w.se(0)?;
    w.se(0)?;
    w.flag(true)?;
    w.flag(true)?;
    w.flag(false)
}

pub(crate) fn slice(w: &mut Bits, e: &Encoder, id: u32) -> Result<()> {
    w.ue(0)?;
    w.ue(7)?;
    w.ue(0)?;
    w.u(4, 0)?;
    w.ue(id)?;
    w.flag(false)?;
    w.flag(false)?;
    w.se(i32::from(e.settings.qp) - 26)?;
    w.ue(1)
}
