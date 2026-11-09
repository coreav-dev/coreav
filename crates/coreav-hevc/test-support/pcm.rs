use super::{Result, writer::Writer};

pub(super) fn escaped(kind: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut nal = vec![kind << 1, 1];
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros == 2 && byte <= 3 {
            nal.push(3);
            zeros = 0;
        }
        nal.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    nal
}

pub(super) fn padded(w: Writer) -> Vec<u8> {
    w.rbsp()
}

pub(super) struct PcmFixture {
    pub(super) sets: [Vec<u8>; 3],
    pub(super) nal: Vec<u8>,
    pub(super) planes: [Vec<u16>; 3],
}

pub(super) fn fixture_profile(writer: &mut Writer, depth: u32) {
    writer.u(2, 0);
    writer.flag(false);
    writer.u(5, if depth == 10 { 2 } else { 1 });
    writer.u(32, 0);
    writer.u(4, 0);
    writer.u(32, 0);
    writer.u(12, 0);
    writer.u(8, 120);
}

pub(super) fn pcm_fixture(depth: u32, pcm_depth: u32, filtered: bool) -> Result<PcmFixture> {
    let mut vps = Writer::default();
    vps.u(4, 0);
    vps.flag(true);
    vps.flag(true);
    vps.u(6, 0);
    vps.u(3, 0);
    vps.flag(true);
    vps.u(16, 0xffff);
    fixture_profile(&mut vps, depth);
    vps.flag(false);
    for _ in 0..3 {
        vps.ue(0);
    }
    vps.u(6, 0);
    vps.ue(0);
    vps.flag(false);
    vps.flag(false);
    let mut sps = Writer::default();
    sps.u(4, 0);
    sps.u(3, 0);
    sps.flag(true);
    fixture_profile(&mut sps, depth);
    for value in [0, 1, 8, 8] {
        sps.ue(value);
    }
    sps.flag(false);
    sps.ue(depth - 8);
    sps.ue(depth - 8);
    sps.ue(0);
    sps.flag(false);
    for value in [0, 0, 0, 0, 1, 0, 1, 0, 0] {
        sps.ue(value);
    }
    for flag in [false, false, false, true] {
        sps.flag(flag);
    }
    sps.u(4, pcm_depth - 1);
    sps.u(4, pcm_depth - 1);
    sps.ue(0);
    sps.ue(0);
    sps.flag(true);
    sps.ue(0);
    for _ in 0..5 {
        sps.flag(false);
    }
    let mut pps = Writer::default();
    pps.ue(0);
    pps.ue(0);
    pps.flag(false);
    pps.flag(false);
    pps.u(3, 0);
    pps.flag(false);
    pps.flag(false);
    pps.ue(0);
    pps.ue(0);
    pps.se(0);
    for _ in 0..3 {
        pps.flag(false);
    }
    pps.se(0);
    pps.se(0);
    for _ in 0..7 {
        pps.flag(false);
    }
    pps.flag(true);
    pps.flag(false);
    pps.flag(!filtered);
    if filtered {
        pps.se(0);
        pps.se(0);
    }
    pps.flag(false);
    pps.flag(false);
    pps.ue(0);
    pps.flag(false);
    pps.flag(false);
    let mut slice = Writer::default();
    slice.flag(true);
    slice.flag(false);
    slice.ue(0);
    slice.ue(2);
    slice.se(0);
    let mut rbsp = padded(slice);
    rbsp.extend([0x86, 0x80]);
    let raw: [Vec<u16>; 3] = core::array::from_fn(|component| {
        let length = if component == 0 { 64 } else { 16 };
        (0..length)
            .map(|i| ((i * 17 + component * 67) & ((1 << pcm_depth) - 1)) as u16)
            .collect()
    });
    let mut samples = Writer::default();
    for plane in &raw {
        for value in plane {
            samples.u(pcm_depth, u32::from(*value));
        }
    }
    rbsp.extend(samples.bytes());
    rbsp.extend([0xfe, 0x80]);
    let planes = raw.map(|plane| {
        plane
            .into_iter()
            .map(|v| v << (depth - pcm_depth))
            .collect()
    });
    Ok(PcmFixture {
        sets: [
            escaped(32, &padded(vps)),
            escaped(33, &padded(sps)),
            escaped(34, &padded(pps)),
        ],
        nal: escaped(19, &rbsp),
        planes,
    })
}
