//! Hand-built byte streams: a bit writer, parameter sets and slice headers
//! written from the syntax tables of ITU-T H.264 7.3.

use super::*;

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    bit: u8,
}

impl Writer {
    fn u(&mut self, n: u32, value: u64) -> &mut Self {
        for i in (0..n).rev() {
            if self.bit == 0 {
                self.bytes.push(0);
            }
            let b = ((value >> i) & 1) as u8;
            *self.bytes.last_mut().unwrap() |= b << (7 - self.bit);
            self.bit = (self.bit + 1) % 8;
        }
        self
    }

    fn flag(&mut self, f: bool) -> &mut Self {
        self.u(1, u64::from(f))
    }

    fn ue(&mut self, value: u64) -> &mut Self {
        let v = value + 1;
        let len = 64 - v.leading_zeros();
        self.u(len - 1, 0).u(len, v)
    }

    fn se(&mut self, value: i64) -> &mut Self {
        let k = if value > 0 {
            2 * value as u64 - 1
        } else {
            2 * (-value) as u64
        };
        self.ue(k)
    }

    /// rbsp_trailing_bits, then the NAL unit with emulation prevention.
    fn nal(&mut self, header: u8) -> Vec<u8> {
        self.u(1, 1);
        while self.bit != 0 {
            self.u(1, 0);
        }
        let mut out = vec![header];
        let mut zeros = 0;
        for &b in &self.bytes {
            if zeros >= 2 && b <= 3 {
                out.push(3);
                zeros = 0;
            }
            zeros = if b == 0 { zeros + 1 } else { 0 };
            out.push(b);
        }
        out
    }
}

#[derive(Clone)]
struct SpsSpec {
    profile: u8,
    id: u64,
    chroma: u64,
    depth: u64,
    scaling: bool,
    log2_frame_num: u64,
    poc_type: u64,
    log2_lsb: u64,
    /// Type 1: offset_for_non_ref_pic, offset_for_top_to_bottom_field and
    /// the cycle.
    cycle: (i64, i64, Vec<i64>),
    width_mbs: u64,
    height_mbs: u64,
    frame_mbs_only: bool,
    crop: Option<[u64; 4]>,
    sar: Option<(u8, u16, u16)>,
}

impl Default for SpsSpec {
    fn default() -> Self {
        Self {
            profile: 100,
            id: 0,
            chroma: 1,
            depth: 8,
            scaling: false,
            log2_frame_num: 4,
            poc_type: 0,
            log2_lsb: 6,
            cycle: (0, 0, vec![]),
            width_mbs: 4,
            height_mbs: 3,
            frame_mbs_only: true,
            crop: None,
            sar: None,
        }
    }
}

fn sps(s: &SpsSpec) -> Vec<u8> {
    let mut w = Writer::default();
    w.u(8, u64::from(s.profile)).u(8, 0).u(8, 30).ue(s.id);
    if HIGH_PROFILES.contains(&s.profile) {
        w.ue(s.chroma);
        if s.chroma == 3 {
            w.flag(false);
        }
        w.ue(s.depth - 8)
            .ue(s.depth - 8)
            .flag(false)
            .flag(s.scaling);
        if s.scaling {
            // List 0 present with a run of deltas, list 6 present ending
            // early (delta to 0), the others absent.
            for i in 0..8 {
                let present = i == 0 || i == 6;
                w.flag(present);
                if i == 0 {
                    for _ in 0..16 {
                        w.se(1);
                    }
                } else if i == 6 {
                    w.se(-8);
                }
            }
        }
    }
    w.ue(s.log2_frame_num - 4).ue(s.poc_type);
    match s.poc_type {
        0 => {
            w.ue(s.log2_lsb - 4);
        }
        1 => {
            w.flag(false)
                .se(s.cycle.0)
                .se(s.cycle.1)
                .ue(s.cycle.2.len() as u64);
            for o in &s.cycle.2 {
                w.se(*o);
            }
        }
        _ => {}
    }
    w.ue(4).flag(false).ue(s.width_mbs - 1).ue(s.height_mbs - 1);
    w.flag(s.frame_mbs_only);
    if !s.frame_mbs_only {
        w.flag(true);
    }
    w.flag(true).flag(s.crop.is_some());
    if let Some(c) = s.crop {
        for v in c {
            w.ue(v);
        }
    }
    w.flag(s.sar.is_some());
    if let Some((idc, sw, sh)) = s.sar {
        w.flag(true).u(8, u64::from(idc));
        if idc == 255 {
            w.u(16, u64::from(sw)).u(16, u64::from(sh));
        }
        // The rest of the VUI, which the reader does not read.
        w.flag(false)
            .flag(false)
            .flag(false)
            .flag(false)
            .flag(false);
        w.flag(false).flag(false).flag(false);
    }
    w.nal(0x67)
}

#[derive(Clone, Default)]
struct PpsSpec {
    id: u64,
    sps_id: u64,
    bottom_field_pic_order: bool,
    slice_groups: Option<u64>,
    weighted_pred: bool,
    /// weighted_bipred_idc 1.
    bipred: bool,
    redundant: bool,
}

fn pps(p: &PpsSpec) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(p.id)
        .ue(p.sps_id)
        .flag(true)
        .flag(p.bottom_field_pic_order);
    match p.slice_groups {
        None => {
            w.ue(0);
        }
        Some(map_type) => {
            w.ue(2).ue(map_type);
            match map_type {
                0 => {
                    for _ in 0..3 {
                        w.ue(5);
                    }
                }
                2 => {
                    for _ in 0..2 {
                        w.ue(0).ue(3);
                    }
                }
                3..=5 => {
                    w.flag(true).ue(1);
                }
                6 => {
                    w.ue(11);
                    for i in 0..12 {
                        w.u(2, i % 3);
                    }
                }
                _ => {}
            }
        }
    }
    w.ue(2)
        .ue(0)
        .flag(p.weighted_pred)
        .u(2, u64::from(p.bipred));
    w.se(0)
        .se(0)
        .se(-2)
        .flag(true)
        .flag(false)
        .flag(p.redundant);
    // transform_8x8_mode_flag and the rest, which the reader does not read.
    w.flag(true).flag(false).se(-2);
    w.nal(0x68)
}

#[derive(Clone)]
struct SliceSpec {
    first_mb: u64,
    /// 0 P, 1 B, 2 I.
    kind: u64,
    pps_id: u64,
    frame_num: u64,
    idr: bool,
    idr_pic_id: u64,
    ref_idc: u8,
    poc_lsb: u64,
    delta_bottom: Option<i64>,
    delta: [i64; 2],
    field: Option<bool>,
    /// num_ref_idx_l0_active_minus1 and, in B slices, l1.
    refs_minus1: u64,
    modification: bool,
    weights: bool,
    mmco5: bool,
    no_output_of_prior_pics: bool,
}

impl Default for SliceSpec {
    fn default() -> Self {
        Self {
            first_mb: 0,
            kind: 2,
            pps_id: 0,
            frame_num: 0,
            idr: true,
            idr_pic_id: 0,
            ref_idc: 3,
            poc_lsb: 0,
            delta_bottom: None,
            delta: [0, 0],
            field: None,
            refs_minus1: 1,
            modification: false,
            weights: false,
            mmco5: false,
            no_output_of_prior_pics: false,
        }
    }
}

fn slice(s: &SliceSpec, sps: &SpsSpec, pps: &PpsSpec) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(s.first_mb).ue(s.kind + 5).ue(s.pps_id);
    w.u(sps.log2_frame_num as u32, s.frame_num);
    if !sps.frame_mbs_only {
        w.flag(s.field.is_some());
        if let Some(bottom) = s.field {
            w.flag(bottom);
        }
    }
    if s.idr {
        w.ue(s.idr_pic_id);
    }
    if sps.poc_type == 0 {
        w.u(sps.log2_lsb as u32, s.poc_lsb);
        if pps.bottom_field_pic_order && s.field.is_none() {
            w.se(s.delta_bottom.unwrap_or(0));
        }
    }
    if sps.poc_type == 1 {
        w.se(s.delta[0]);
        if pps.bottom_field_pic_order && s.field.is_none() {
            w.se(s.delta[1]);
        }
    }
    if pps.redundant {
        w.ue(0);
    }
    if s.kind == 1 {
        w.flag(true);
    }
    if s.kind != 2 {
        // num_ref_idx_active_override_flag, two references a list by default.
        w.flag(true).ue(s.refs_minus1);
        if s.kind == 1 {
            w.ue(s.refs_minus1);
        }
        for _ in 0..if s.kind == 1 { 2 } else { 1 } {
            w.flag(s.modification);
            if s.modification {
                w.ue(0).ue(0).ue(2).ue(1).ue(3);
            }
        }
    }
    if (pps.weighted_pred && s.kind == 0) || (pps.bipred && s.kind == 1) {
        assert!(s.weights);
        w.ue(5).ue(5);
        for _list in 0..if s.kind == 1 { 2 } else { 1 } {
            for i in 0..=s.refs_minus1 {
                w.flag(i == 0);
                if i == 0 {
                    w.se(-3).se(7);
                }
                w.flag(i == 1);
                if i == 1 {
                    w.se(1).se(-1).se(2).se(-2);
                }
            }
        }
    }
    if s.ref_idc != 0 {
        if s.idr {
            w.flag(s.no_output_of_prior_pics).flag(false);
        } else {
            w.flag(true);
            w.ue(1).ue(0).ue(3).ue(1).ue(2);
            if s.mmco5 {
                w.ue(5);
            }
            w.ue(4).ue(1).ue(6).ue(0).ue(0);
        }
    }
    // The rest of the slice header and the slice data.
    w.ue(0).se(3).u(32, 0xDEAD_BEEF);
    let header = (s.ref_idc << 5) | if s.idr { 5 } else { 1 };
    w.nal(header)
}

/// A picture of the default stream: IDR when `lsb` is `None`.
fn picture(frame_num: u64, lsb: Option<u64>, reference: bool, kind: u64) -> SliceSpec {
    SliceSpec {
        kind,
        frame_num,
        idr: lsb.is_none(),
        ref_idc: if reference { 2 } else { 0 },
        poc_lsb: lsb.unwrap_or(0),
        ..SliceSpec::default()
    }
}

fn annexb(units: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0, 0];
    for (i, u) in units.iter().enumerate() {
        if i % 2 == 0 {
            out.push(0);
        }
        out.extend_from_slice(&[0, 0, 1]);
        out.extend_from_slice(u);
        if i % 3 == 0 {
            out.extend_from_slice(&[0, 0]);
        }
    }
    out
}

fn read_with(limits: Limits, stream: &[u8], chunk: usize) -> Result<Vec<EncodedPicture>, Error> {
    let mut reader = AnnexBReader::new(limits);
    for part in stream.chunks(chunk.max(1)) {
        reader.push(part)?;
    }
    reader.finish()?;
    Ok(std::iter::from_fn(|| reader.pop()).collect())
}

fn read(stream: &[u8]) -> Result<Vec<EncodedPicture>, Error> {
    read_with(Limits::default(), stream, usize::MAX)
}

fn order(pictures: &[EncodedPicture]) -> Vec<u64> {
    pictures.iter().map(|p| p.presentation).collect()
}

fn units(sample: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![];
    let mut rest = sample;
    while !rest.is_empty() {
        let n = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        out.push(rest[4..4 + n].to_vec());
        rest = &rest[4 + n..];
    }
    out
}

/// SPS, PPS, then the pictures, each preceded by an access unit delimiter.
fn stream_of(s: &SpsSpec, p: &PpsSpec, pictures: &[SliceSpec]) -> Vec<Vec<u8>> {
    let mut out = vec![sps(s), pps(p)];
    for pic in pictures {
        out.push(vec![0x09, 0xF0]);
        out.push(slice(pic, s, p));
    }
    out
}

#[test]
fn ipbb_in_two_periods() {
    let s = SpsSpec::default();
    let p = PpsSpec::default();
    let pics = [
        picture(0, None, true, 2),
        picture(1, Some(6), true, 0),
        picture(2, Some(2), false, 1),
        picture(2, Some(4), false, 1),
        SliceSpec {
            idr_pic_id: 1,
            ..picture(0, None, true, 2)
        },
        picture(1, Some(4), true, 0),
        picture(2, Some(2), false, 1),
    ];
    let stream = annexb(&stream_of(&s, &p, &pics));
    let out = read(&stream).unwrap();
    assert_eq!(order(&out), [0, 3, 1, 2, 4, 6, 5]);
    let idr: Vec<bool> = out.iter().map(|p| p.idr).collect();
    assert_eq!(idr, [true, false, false, false, true, false, false]);
    // The first sample holds its delimiter and slice, not the parameter sets.
    let first = units(&out[0].data);
    assert_eq!(first, vec![vec![0x09, 0xF0], slice(&pics[0], &s, &p)]);
    // Every chunking gives the same samples.
    for chunk in 1..12 {
        assert_eq!(read_with(Limits::default(), &stream, chunk).unwrap(), out);
    }
}

#[test]
fn config_and_avcc() {
    let s = SpsSpec {
        width_mbs: 120,
        height_mbs: 68,
        crop: Some([0, 0, 0, 4]),
        sar: Some((14, 0, 0)),
        scaling: true,
        ..SpsSpec::default()
    };
    let p = PpsSpec::default();
    let (sps_nal, pps_nal) = (sps(&s), pps(&p));
    let stream = annexb(&stream_of(&s, &p, &[SliceSpec::default()]));
    let mut reader = AnnexBReader::new(Limits::default());
    reader.push(&stream).unwrap();
    reader.finish().unwrap();
    let config = reader.config().unwrap().clone();
    assert_eq!((config.width, config.height), (1920, 1080));
    assert_eq!(config.sample_aspect_ratio, Some((4, 3)));
    assert_eq!(
        (
            config.profile_idc,
            config.level_idc,
            config.chroma_format_idc
        ),
        (100, 30, 1)
    );
    let mut avcc = vec![1, 100, 0, 30, 0xFF, 0xE1];
    avcc.extend_from_slice(&(sps_nal.len() as u16).to_be_bytes());
    avcc.extend_from_slice(&sps_nal);
    avcc.push(1);
    avcc.extend_from_slice(&(pps_nal.len() as u16).to_be_bytes());
    avcc.extend_from_slice(&pps_nal);
    avcc.extend_from_slice(&[0xFD, 0xF8, 0xF8, 0]);
    assert_eq!(config.avcc, avcc);

    // Baseline: no High fields; an extended SAR; 4:2:0 crop units.
    let s = SpsSpec {
        profile: 66,
        crop: Some([1, 2, 1, 0]),
        sar: Some((255, 64, 45)),
        ..SpsSpec::default()
    };
    let stream = annexb(&stream_of(&s, &p, &[SliceSpec::default()]));
    let mut reader = AnnexBReader::new(Limits::default());
    reader.push(&stream).unwrap();
    reader.finish().unwrap();
    let config = reader.config().unwrap();
    assert_eq!((config.width, config.height), (64 - 6, 48 - 2));
    assert_eq!(config.sample_aspect_ratio, Some((64, 45)));
    assert_eq!(
        config.avcc.len(),
        6 + 2 + sps(&s).len() + 1 + 2 + pps_nal.len()
    );

    // High 4:4:4 at 10 bits: crop units of one sample.
    let s = SpsSpec {
        profile: 244,
        chroma: 3,
        depth: 10,
        crop: Some([1, 2, 1, 0]),
        ..SpsSpec::default()
    };
    let stream = annexb(&stream_of(&s, &p, &[SliceSpec::default()]));
    let mut reader = AnnexBReader::new(Limits::default());
    reader.push(&stream).unwrap();
    reader.finish().unwrap();
    let config = reader.config().unwrap();
    assert_eq!((config.width, config.height), (64 - 3, 48 - 1));
    assert_eq!((config.bit_depth_luma, config.bit_depth_chroma), (10, 10));
    assert_eq!(config.avcc[config.avcc.len() - 4..], [0xFF, 0xFA, 0xFA, 0]);
}

#[test]
fn poc_type0_wraps() {
    let s = SpsSpec {
        log2_lsb: 4,
        ..SpsSpec::default()
    };
    let p = PpsSpec::default();
    let mut pics = vec![picture(0, None, true, 2)];
    for n in 1..40u64 {
        pics.push(picture(n % 16, Some((2 * n) % 16), true, 0));
    }
    let out = read(&annexb(&stream_of(&s, &p, &pics))).unwrap();
    assert_eq!(order(&out), (0..40).collect::<Vec<_>>());
}

#[test]
fn poc_type2() {
    let s = SpsSpec {
        poc_type: 2,
        ..SpsSpec::default()
    };
    let p = PpsSpec::default();
    // Reference and non-reference pictures, frame_num wrapping at 16.
    let mut pics = vec![picture(0, None, true, 2)];
    let mut frame_num = 1;
    for n in 1..40u64 {
        let reference = n % 3 != 0;
        pics.push(picture(frame_num % 16, Some(0), reference, 0));
        if reference {
            frame_num += 1;
        }
    }
    let out = read(&annexb(&stream_of(&s, &p, &pics))).unwrap();
    assert_eq!(order(&out), (0..40).collect::<Vec<_>>());
}

#[test]
fn poc_type1() {
    // Two reference frames a cycle, at +4 and +2; non-reference pictures
    // at −4, so I P b P b... with the b pictures presented between.
    let s = SpsSpec {
        poc_type: 1,
        cycle: (-4, 0, vec![4, 2]),
        ..SpsSpec::default()
    };
    let p = PpsSpec::default();
    // Decode order: I0(poc 0) P1(4) b(2)... POCs of P at frame_num n:
    // expected = sum of offsets up to absFrameNum.
    let pics = [
        picture(0, None, true, 2),
        picture(1, Some(0), true, 0),  // abs 1: 4
        picture(2, Some(0), false, 1), // abs 1: 4 − 4 = 0? see below
        picture(2, Some(0), true, 0),  // abs 2: 6
        picture(3, Some(0), true, 0),  // abs 3: 10
        picture(4, Some(0), false, 1), // abs 3: 10 − 4 = 6? duplicate
    ];
    // The non-reference b after P1 has absFrameNum 2 − 1 = 1, so 4 − 4 = 0,
    // the IDR's count: a duplicate, which is an error.
    assert_eq!(
        read(&annexb(&stream_of(&s, &p, &pics[..3]))),
        Err(Invalid("two pictures of one period with one order count"))
    );
    let s = SpsSpec {
        poc_type: 1,
        cycle: (-1, 0, vec![4, 2]),
        ..SpsSpec::default()
    };
    // I 0, P 4, b 3, P 6, P 10, b 9: presented I b P P b P.
    let out = read(&annexb(&stream_of(&s, &p, &pics))).unwrap();
    assert_eq!(order(&out), [0, 2, 1, 3, 5, 4]);
    // delta_pic_order_cnt[0] moves a picture.
    let mut moved = pics.to_vec();
    moved[1].delta = [-5, 0];
    let out = read(&annexb(&stream_of(&s, &p, &moved))).unwrap();
    assert_eq!(order(&out), [1, 0, 2, 3, 5, 4]);
}

#[test]
fn mmco5_starts_a_period() {
    let s = SpsSpec {
        log2_lsb: 4,
        ..SpsSpec::default()
    };
    let p = PpsSpec::default();
    let pics = [
        picture(0, None, true, 2),
        picture(1, Some(4), true, 0),
        picture(2, Some(2), false, 1),
        SliceSpec {
            mmco5: true,
            modification: true,
            ..picture(2, Some(8), true, 0)
        },
        // After the reset: lsb 14 against 0 is −2, before the mmco5 picture.
        picture(1, Some(14), false, 1),
        picture(1, Some(2), true, 0),
    ];
    let out = read(&annexb(&stream_of(&s, &p, &pics))).unwrap();
    assert_eq!(order(&out), [0, 2, 1, 4, 3, 5]);
    assert!(out.iter().skip(1).all(|p| !p.idr));
}

#[test]
fn no_output_of_prior_pics() {
    let s = SpsSpec::default();
    let p = PpsSpec::default();
    let first = SliceSpec {
        no_output_of_prior_pics: true,
        ..SliceSpec::default()
    };
    assert!(read(&annexb(&stream_of(&s, &p, std::slice::from_ref(&first)))).is_ok());
    let pics = [SliceSpec::default(), first];
    assert_eq!(
        read(&annexb(&stream_of(&s, &p, &pics))),
        Err(Error::Unsupported(
            "an IDR picture with no_output_of_prior_pics_flag 1"
        ))
    );
}

#[test]
fn weighted_prediction_and_slice_groups() {
    let s = SpsSpec::default();
    for groups in [None, Some(0), Some(1), Some(2), Some(3), Some(6)] {
        let p = PpsSpec {
            weighted_pred: true,
            slice_groups: groups,
            redundant: true,
            bottom_field_pic_order: true,
            ..PpsSpec::default()
        };
        let pics = [
            SliceSpec {
                delta_bottom: Some(-1),
                ..picture(0, None, true, 2)
            },
            SliceSpec {
                weights: true,
                modification: true,
                ..picture(1, Some(8), true, 0)
            },
            SliceSpec {
                weights: true,
                mmco5: true,
                ..picture(2, Some(4), true, 0)
            },
            SliceSpec {
                weights: true,
                ..picture(1, Some(2), true, 0)
            },
        ];
        let out = read(&annexb(&stream_of(&s, &p, &pics))).unwrap();
        assert_eq!(order(&out), [0, 1, 2, 3], "{groups:?}");
    }
}

#[test]
fn bipred_weights() {
    // weighted_bipred_idc 1: B slices carry a table for each list.
    let s = SpsSpec::default();
    let p = PpsSpec {
        weighted_pred: true,
        bipred: true,
        ..PpsSpec::default()
    };
    let weighted = |pic: SliceSpec| SliceSpec {
        weights: true,
        modification: true,
        ..pic
    };
    let pics = [
        picture(0, None, true, 2),
        weighted(picture(1, Some(8), true, 0)),
        weighted(picture(2, Some(4), true, 1)),
        weighted(picture(3, Some(2), false, 1)),
        SliceSpec {
            refs_minus1: 3,
            ..weighted(picture(3, Some(6), false, 1))
        },
        // mmco 5 after both tables: read only if the tables were skipped
        // exactly.
        SliceSpec {
            mmco5: true,
            ..weighted(picture(3, Some(12), true, 1))
        },
        weighted(picture(1, Some(2), true, 0)),
    ];
    let out = read(&annexb(&stream_of(&s, &p, &pics))).unwrap();
    assert_eq!(order(&out), [0, 4, 2, 1, 3, 5, 6]);
}

#[test]
fn mmco5_with_poc_types_1_and_2() {
    let p = PpsSpec::default();
    // Type 1, cycle +4 +2, non-reference pictures −1: I 0, P 4, b 3, then
    // P(mmco 5) 6 reset to 0. frame_num is then 0: b at frame_num 1 is
    // absFrameNum 0, so −1, before the reset picture; P at 1 is 4.
    let s = SpsSpec {
        poc_type: 1,
        cycle: (-1, 0, vec![4, 2]),
        ..SpsSpec::default()
    };
    let pics = [
        picture(0, None, true, 2),
        picture(1, Some(0), true, 0),
        picture(2, Some(0), false, 1),
        SliceSpec {
            mmco5: true,
            ..picture(2, Some(0), true, 0)
        },
        picture(1, Some(0), false, 1),
        picture(1, Some(0), true, 0),
    ];
    let out = read(&annexb(&stream_of(&s, &p, &pics))).unwrap();
    assert_eq!(order(&out), [0, 2, 1, 4, 3, 5]);
    // Type 2: output order is decode order; frame_num restarts after it.
    let s = SpsSpec {
        poc_type: 2,
        ..SpsSpec::default()
    };
    let pics = [
        picture(0, None, true, 2),
        picture(1, Some(0), true, 0),
        SliceSpec {
            mmco5: true,
            ..picture(2, Some(0), true, 0)
        },
        picture(1, Some(0), false, 0),
        picture(1, Some(0), true, 0),
    ];
    let out = read(&annexb(&stream_of(&s, &p, &pics))).unwrap();
    assert_eq!(order(&out), [0, 1, 2, 3, 4]);
    assert_eq!(out.iter().filter(|p| p.idr).count(), 1);
}

#[test]
fn strict_slice_headers() {
    let s = SpsSpec::default();
    let p = PpsSpec::default();
    let idr_p = SliceSpec {
        kind: 0,
        ..SliceSpec::default()
    };
    assert_eq!(
        read(&annexb(&stream_of(&s, &p, &[idr_p]))),
        Err(Invalid("an IDR picture with a P or B slice"))
    );
    for (kind, refs_minus1, ok) in [(0, 15, true), (0, 16, false), (1, 15, true), (1, 16, false)] {
        let pics = [
            picture(0, None, true, 2),
            SliceSpec {
                refs_minus1,
                ..picture(1, Some(2), true, kind)
            },
        ];
        let result = read(&annexb(&stream_of(&s, &p, &pics)));
        if ok {
            assert!(result.is_ok());
        } else {
            assert_eq!(result, Err(Invalid("num_ref_idx_active_minus1 above 15")));
        }
    }
}

#[test]
fn parameter_sets_fit_avcc() {
    let s = SpsSpec::default();
    let p = PpsSpec::default();
    let pic = slice(&SliceSpec::default(), &s, &p);
    // Bytes after rbsp_trailing_bits are not read; they still count.
    let padded = |mut nal: Vec<u8>, total: usize| {
        nal.resize(total, 0xFF);
        nal
    };
    for total in [65_535, 65_536] {
        let ok = total <= 65_535;
        let with_sps = [padded(sps(&s), total), pps(&p), pic.clone()];
        let with_pps = [sps(&s), padded(pps(&p), total), pic.clone()];
        for units_in in [with_sps, with_pps] {
            let result = read(&annexb(&units_in));
            if ok {
                assert_eq!(result.unwrap().len(), 1);
            } else {
                assert_eq!(result, Err(Error::Limit("parameter set size")));
            }
        }
    }
}

#[test]
fn several_slices_and_access_unit_boundaries() {
    let s = SpsSpec::default();
    let p = PpsSpec::default();
    let first = picture(0, None, true, 2);
    let second = SliceSpec {
        first_mb: 6,
        ..first.clone()
    };
    let next = picture(1, Some(2), true, 0);
    let next_second = SliceSpec {
        first_mb: 6,
        ..next.clone()
    };
    let sei = vec![0x06, 0x05, 0x01, 0xAA, 0x80];
    let filler = vec![0x0C, 0xFF, 0xFF, 0x80];
    let end_of_seq = vec![0x0A];
    let units_in = vec![
        sei.clone(),
        sps(&s),
        pps(&p),
        slice(&first, &s, &p),
        slice(&second, &s, &p),
        filler.clone(),
        // Repeated parameter sets start the next unit and are dropped.
        sps(&s),
        pps(&p),
        sei.clone(),
        slice(&next, &s, &p),
        slice(&next_second, &s, &p),
        end_of_seq.clone(),
    ];
    let out = read(&annexb(&units_in)).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(
        units(&out[0].data),
        vec![
            sei.clone(),
            slice(&first, &s, &p),
            slice(&second, &s, &p),
            filler
        ]
    );
    assert_eq!(
        units(&out[1].data),
        vec![
            sei,
            slice(&next, &s, &p),
            slice(&next_second, &s, &p),
            end_of_seq
        ]
    );

    // A later slice that disagrees with the first.
    let wrong = SliceSpec {
        poc_lsb: 4,
        ..next_second.clone()
    };
    let bad = vec![
        sps(&s),
        pps(&p),
        slice(&first, &s, &p),
        slice(&next, &s, &p),
        slice(&wrong, &s, &p),
    ];
    assert!(matches!(read(&annexb(&bad)), Err(Invalid(_))));
    // A picture without a slice at macroblock 0.
    let bad = vec![sps(&s), pps(&p), slice(&second, &s, &p)];
    assert!(matches!(read(&annexb(&bad)), Err(Error::Unsupported(_))));
    // Units after the last picture that start an access unit.
    let bad = vec![sps(&s), pps(&p), slice(&first, &s, &p), vec![0x09, 0xF0]];
    assert!(matches!(read(&annexb(&bad)), Err(Invalid(_))));
}

#[test]
fn parameter_sets_that_change() {
    let s = SpsSpec::default();
    let p = PpsSpec::default();
    let pic = slice(&SliceSpec::default(), &s, &p);
    let other_sps = sps(&SpsSpec {
        log2_lsb: 8,
        ..s.clone()
    });
    let other_pps = pps(&PpsSpec { id: 1, ..p.clone() });
    for bad in [
        vec![
            sps(&s),
            pps(&p),
            pic.clone(),
            other_sps.clone(),
            pic.clone(),
        ],
        vec![sps(&s), pps(&p), pic.clone(), other_pps.clone()],
        vec![sps(&s), other_sps.clone(), pps(&p), pic.clone()],
        vec![
            sps(&s),
            pps(&p),
            pic.clone(),
            sps(&SpsSpec { id: 1, ..s.clone() }),
        ],
    ] {
        assert_eq!(read(&annexb(&bad)), Err(Error::ParameterSetsChanged));
    }
    // A slice of a picture parameter set for another sequence parameter set.
    let s1 = SpsSpec { id: 1, ..s.clone() };
    let p1 = PpsSpec {
        id: 1,
        sps_id: 1,
        ..p.clone()
    };
    let two = vec![
        sps(&s),
        sps(&s1),
        pps(&p),
        pps(&p1),
        pic.clone(),
        slice(
            &SliceSpec {
                pps_id: 1,
                idr_pic_id: 1,
                ..SliceSpec::default()
            },
            &s1,
            &p1,
        ),
    ];
    assert_eq!(read(&annexb(&two)), Err(Error::ParameterSetsChanged));
    // Both are in avcC while only the first is used.
    let mut reader = AnnexBReader::new(Limits::default());
    reader.push(&annexb(&two[..5])).unwrap();
    reader.finish().unwrap();
    assert_eq!(reader.config().unwrap().avcc[5], 0xE2);
    // Missing parameter sets.
    assert!(matches!(
        read(&annexb(&[pps(&p), pic.clone()])),
        Err(Invalid(_))
    ));
    assert!(matches!(read(&annexb(&[sps(&s), pic])), Err(Invalid(_))));
}

#[test]
fn unsupported_streams() {
    let s = SpsSpec {
        frame_mbs_only: false,
        ..SpsSpec::default()
    };
    let p = PpsSpec::default();
    // Frame pictures of an interlaced stream are read (as MBAFF frames).
    let frames = [picture(0, None, true, 2), picture(1, Some(2), true, 0)];
    let out = read(&annexb(&stream_of(&s, &p, &frames))).unwrap();
    assert_eq!(order(&out), [0, 1]);
    let field = SliceSpec {
        field: Some(false),
        ..SliceSpec::default()
    };
    assert_eq!(
        read(&annexb(&stream_of(&s, &p, &[field]))),
        Err(Error::Unsupported("field pictures"))
    );
    let s = SpsSpec::default();
    for kind in [2u8, 14, 15, 19, 20] {
        let units_in = vec![sps(&s), pps(&p), vec![kind, 0x80]];
        assert!(
            matches!(read(&annexb(&units_in)), Err(Error::Unsupported(_))),
            "{kind}"
        );
    }
}

#[test]
fn byte_stream_errors() {
    let s = SpsSpec::default();
    let p = PpsSpec::default();
    let good = annexb(&stream_of(&s, &p, &[SliceSpec::default()]));
    let mut garbage = vec![7];
    garbage.extend_from_slice(&good);
    assert_eq!(
        read(&garbage),
        Err(Invalid("bytes before the first start code"))
    );
    let mut three_zeros = good.clone();
    three_zeros.extend_from_slice(&[0x0C, 0xFF, 0, 0, 0, 0xFF]);
    assert_eq!(
        read(&three_zeros),
        Err(Invalid("three zero bytes inside a NAL unit"))
    );
    let mut two = good.clone();
    two.extend_from_slice(&[0x0C, 0xFF, 0, 0, 2]);
    assert!(matches!(read(&two), Err(Invalid(_))));
    let mut empty = good.clone();
    empty.extend_from_slice(&[0, 0, 1, 0, 0, 1, 0x0C, 0xFF]);
    assert_eq!(read(&empty), Err(Invalid("an empty NAL unit")));
    let mut forbidden = good.clone();
    forbidden.extend_from_slice(&[0, 0, 1, 0x8C]);
    assert_eq!(read(&forbidden), Err(Invalid("forbidden_zero_bit is set")));
    // Truncated parameter set.
    let bad = annexb(&[vec![0x67, 100]]);
    assert_eq!(read(&bad), Err(TRUNCATED));
    // Nothing at all, or only zeros: no pictures, no error.
    assert_eq!(read(&[]), Ok(vec![]));
    assert_eq!(read(&[0, 0, 0]), Ok(vec![]));
    // After an error, every call fails.
    let mut reader = AnnexBReader::new(Limits::default());
    assert!(reader.push(&garbage).is_err());
    assert_eq!(reader.push(&good), Err(Error::Closed));
    assert_eq!(reader.finish(), Err(Error::Closed));
    let mut reader = AnnexBReader::new(Limits::default());
    reader.finish().unwrap();
    assert_eq!(reader.push(&good), Err(Error::Closed));
}

#[test]
fn emulation_prevention_is_kept_in_samples() {
    // A 16-bit lsb of 0 and frame_num 0 give zero bytes in the header.
    let s = SpsSpec {
        log2_lsb: 16,
        log2_frame_num: 16,
        ..SpsSpec::default()
    };
    let p = PpsSpec::default();
    // frame_num 0 and a small lsb: 31 zero bits in a row.
    let pics = [
        picture(0, None, true, 2),
        picture(0, Some(2), true, 0),
        picture(0, Some(1), false, 1),
    ];
    let nals = stream_of(&s, &p, &pics);
    assert!(nals[7].windows(3).any(|w| w == [0, 0, 3]));
    let out = read(&annexb(&nals)).unwrap();
    assert_eq!(order(&out), [0, 2, 1]);
    assert_eq!(units(&out[2].data)[1], nals[7]);
}

#[test]
fn limits() {
    let s = SpsSpec::default();
    let p = PpsSpec::default();
    let pics: Vec<SliceSpec> = (0..4)
        .map(|n| picture(n, Some(2 * n), true, if n == 0 { 2 } else { 0 }))
        .map(|mut pic| {
            pic.idr = pic.frame_num == 0;
            pic
        })
        .collect();
    let stream = annexb(&stream_of(&s, &p, &pics));
    let ok = read(&stream).unwrap();
    let biggest_nal = stream_of(&s, &p, &pics).iter().map(Vec::len).max().unwrap();
    let biggest_sample = ok.iter().map(|p| p.data.len()).max().unwrap();
    let total: usize = ok.iter().map(|p| p.data.len()).sum();
    let cases = [
        (
            Limits {
                max_nal_unit_bytes: biggest_nal - 1,
                ..Limits::default()
            },
            "NAL unit size",
        ),
        (
            Limits {
                max_access_unit_bytes: biggest_sample - 1,
                ..Limits::default()
            },
            "access unit size",
        ),
        (
            Limits {
                max_nal_units_per_access_unit: 1,
                ..Limits::default()
            },
            "NAL units in an access unit",
        ),
        (
            Limits {
                max_period_pictures: 3,
                ..Limits::default()
            },
            "pictures in one period",
        ),
        (
            Limits {
                max_period_bytes: total as u64 - 1,
                ..Limits::default()
            },
            "bytes in one period",
        ),
        (
            Limits {
                max_pictures: 3,
                ..Limits::default()
            },
            "more pictures than frames sent",
        ),
    ];
    for (limits, what) in cases {
        for chunk in [1, 7, usize::MAX] {
            assert_eq!(read_with(limits, &stream, chunk), Err(Error::Limit(what)));
        }
    }
    let exact = Limits {
        max_nal_unit_bytes: biggest_nal,
        max_access_unit_bytes: biggest_sample,
        max_nal_units_per_access_unit: 4,
        max_period_pictures: 4,
        max_period_bytes: total as u64,
        max_pictures: 4,
    };
    for chunk in [1, 7, usize::MAX] {
        assert_eq!(read_with(exact, &stream, chunk).unwrap(), ok);
    }
    // The limit can be raised as frames are sent.
    let mut reader = AnnexBReader::new(Limits {
        max_pictures: 0,
        ..Limits::default()
    });
    reader.set_max_pictures(4);
    reader.push(&stream).unwrap();
    reader.finish().unwrap();
    assert_eq!(reader.pictures(), 4);
}

#[test]
fn trailing_zeros_do_not_grow_a_unit() {
    let s = SpsSpec::default();
    let p = PpsSpec::default();
    let mut stream = annexb(&stream_of(&s, &p, &[SliceSpec::default()]));
    stream.extend(std::iter::repeat_n(0, 10_000));
    let limits = Limits {
        max_nal_unit_bytes: 64,
        ..Limits::default()
    };
    assert_eq!(read_with(limits, &stream, 3).unwrap().len(), 1);
}
