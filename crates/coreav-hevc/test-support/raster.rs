use super::{Result, pcm, writer::Writer};

fn profile(writer: &mut Writer, rows: u32) {
    writer.u(2, 0);
    writer.flag(false);
    writer.u(5, 1);
    writer.u(32, 0);
    writer.u(4, 0);
    writer.u(32, 0);
    writer.u(12, 0);
    writer.u(8, if rows <= 256 { 120 } else { 150 });
}

pub(super) fn stream(predicted: bool) -> Result<Vec<Vec<u8>>> {
    stream_geometry(predicted, 32, 4)
}

pub(super) fn stream_geometry(predicted: bool, rows: u32, dpb: u32) -> Result<Vec<Vec<u8>>> {
    assert!((1..=512).contains(&rows));
    assert!((1..=16).contains(&dpb));
    let fixture = pcm::pcm_fixture(8, 8, false)?;
    let mut vps = Writer::default();
    vps.u(4, 0);
    vps.flag(true);
    vps.flag(true);
    vps.u(6, 0);
    vps.u(3, 0);
    vps.flag(true);
    vps.u(16, 0xffff);
    profile(&mut vps, rows);
    vps.flag(false);
    for value in [dpb - 1, dpb - 1, 0] {
        vps.ue(value);
    }
    vps.u(6, 0);
    vps.ue(0);
    vps.flag(false);
    vps.flag(false);
    let mut sps = Writer::default();
    sps.u(4, 0);
    sps.u(3, 0);
    sps.flag(true);
    profile(&mut sps, rows);
    for value in [0, 1, 16, rows * 16] {
        sps.ue(value);
    }
    sps.flag(false);
    sps.ue(0);
    sps.ue(0);
    sps.ue(0);
    sps.flag(false);
    for value in [dpb - 1, dpb - 1, 0, 1, 0, 0, 1, 0, 0] {
        sps.ue(value);
    }
    for flag in [false, false, false, true] {
        sps.flag(flag);
    }
    sps.u(4, 7);
    sps.u(4, 7);
    sps.ue(1);
    sps.ue(0);
    sps.flag(true);
    sps.ue(0);
    for _ in 0..5 {
        sps.flag(false);
    }
    let mut nals = vec![
        pcm::escaped(32, &vps.rbsp()),
        pcm::escaped(33, &sps.rbsp()),
        fixture.sets[2].clone(),
    ];
    for picture in 0..2 {
        let inter = predicted && picture != 0;
        for row in 0..rows {
            let mut header = Writer::default();
            header.flag(row == 0);
            if !inter {
                header.flag(false);
            }
            header.ue(0);
            if row != 0 {
                header.u(rows.next_power_of_two().trailing_zeros(), row);
            }
            header.ue(if inter { 1 } else { 2 });
            if inter {
                header.u(4, 1);
                header.flag(false);
                header.ue(1);
                header.ue(0);
                header.ue(0);
                header.flag(true);
                header.flag(false);
                header.ue(4);
            }
            header.se(0);
            let mut rbsp = header.rbsp();
            if inter {
                rbsp.extend([0xfe, 0xe0]);
            } else {
                rbsp.extend([0x86, 0x80]);
                rbsp.extend(core::iter::repeat_n(
                    (37 + picture * 5 + row) as u8,
                    16 * 16 * 3 / 2,
                ));
                rbsp.extend([0xfe, 0x80]);
            }
            nals.push(pcm::escaped(if inter { 1 } else { 19 }, &rbsp));
        }
    }
    Ok(nals)
}
