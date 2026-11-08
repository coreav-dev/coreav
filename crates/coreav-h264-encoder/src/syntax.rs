#![forbid(unsafe_code)]

use crate::{Deblocking, Encoder, Result, bits::Bits};

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
    let sar = e.settings.sample_aspect_ratio;
    w.flag(sar.present)?;
    if sar.present {
        let idc = match sar.as_ratio() {
            (1, 1) => 1,
            (12, 11) => 2,
            (10, 11) => 3,
            (16, 11) => 4,
            (40, 33) => 5,
            (24, 11) => 6,
            (20, 11) => 7,
            (32, 11) => 8,
            (80, 33) => 9,
            (18, 11) => 10,
            (15, 11) => 11,
            (64, 33) => 12,
            (160, 99) => 13,
            (4, 3) => 14,
            (3, 2) => 15,
            (2, 1) => 16,
            _ => 255,
        };
        w.u(8, idc)?;
        if idc == 255 {
            w.u(16, u64::from(sar.width))?;
            w.u(16, u64::from(sar.height))?;
        }
    }
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
    let (tick, scale) = e.settings.frame_rate.timing();
    w.u(32, u64::from(tick))?;
    w.u(32, scale)?;
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

#[derive(Clone, Copy)]
pub(crate) struct Slice {
    pub(crate) idr: bool,
    pub(crate) id: u32,
    pub(crate) frame_num: u8,
    pub(crate) qp: u8,
}

pub(crate) fn slice(w: &mut Bits, e: &Encoder, slice: Slice) -> Result<()> {
    w.ue(0)?;
    w.ue(if slice.idr { 7 } else { 5 })?;
    w.ue(0)?;
    w.u(4, u64::from(slice.frame_num))?;
    if slice.idr {
        w.ue(slice.id)?;
        w.flag(false)?;
        w.flag(false)?;
    } else {
        w.flag(false)?;
        w.flag(false)?;
        w.flag(false)?;
    }
    w.se(i32::from(slice.qp) - 26)?;
    match e.settings.deblocking {
        Deblocking::Disabled => w.ue(1),
        Deblocking::Enabled => {
            w.ue(0)?;
            w.se(0)?;
            w.se(0)
        }
    }
}
