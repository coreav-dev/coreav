#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use bits::Bits;

fn settings(width: usize, height: usize, qp: u8) -> Settings {
    Settings {
        width,
        height,
        qp,
        deblocking: Deblocking::Disabled,
        gop: Gop::AllIntra,
        colour: Colour {
            full_range: false,
            primaries: 1,
            transfer: 1,
            matrix: 1,
            chroma_location: 0,
        },
    }
}

#[derive(Default)]
struct Reader {
    bytes: Vec<u8>,
    at: usize,
}
impl Reader {
    fn bit(&mut self) -> u32 {
        let b = (self.bytes[self.at / 8] >> (7 - self.at % 8)) & 1;
        self.at += 1;
        u32::from(b)
    }
    fn u(&mut self, n: u32) -> u32 {
        (0..n).fold(0, |v, _| v * 2 + self.bit())
    }
    fn ue(&mut self) -> u32 {
        let mut n = 0;
        while self.bit() == 0 {
            n += 1;
            assert!(n < 32);
        }
        (1 << n) - 1 + self.u(n)
    }
    fn vlc(&mut self, codes: impl IntoIterator<Item = (usize, u32)>) -> usize {
        let codes: Vec<_> = codes.into_iter().filter(|(_, v)| *v & 31 != 0).collect();
        let mut v = 0;
        for n in 1..=16 {
            v = v * 2 + self.bit();
            if let Some((i, _)) = codes.iter().find(|(_, c)| *c & 31 == n && *c >> 5 == v) {
                return *i;
            }
        }
        unreachable!("invalid VLC")
    }
}

fn reader(w: Bits) -> Reader {
    let nal = w.nal(0x65).unwrap();
    nal_reader(&nal)
}

fn nal_reader(nal: &[u8]) -> Reader {
    let mut bytes = Vec::new();
    let mut zeros = 0;
    for &b in nal.iter().skip(1) {
        if zeros == 2 && b == 3 {
            zeros = 0;
            continue;
        }
        bytes.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    Reader { bytes, at: 0 }
}

fn read_residual(r: &mut Reader, max: usize, nc: i32) -> Vec<i32> {
    let column = match nc {
        -1 => 4,
        0..=1 => 0,
        2..=3 => 1,
        4..=7 => 2,
        _ => 3,
    };
    let token = r.vlc(
        vlc::TOKENS
            .iter()
            .enumerate()
            .map(|(i, row)| (i, row[column])),
    );
    let total = token / 4;
    let trailing = token % 4;
    let mut levels = Vec::new();
    for _ in 0..trailing {
        levels.push(if r.bit() == 0 { 1 } else { -1 });
    }
    let mut suffix = u32::from(total > 10 && trailing < 3);
    for i in trailing..total {
        let mut prefix = 0;
        while r.bit() == 0 {
            prefix += 1;
            assert!(prefix <= 15);
        }
        let size = if prefix == 14 && suffix == 0 {
            4
        } else if prefix >= 15 {
            prefix - 3
        } else {
            suffix
        };
        let mut code = (prefix << suffix) + r.u(size);
        if prefix >= 15 && suffix == 0 {
            code += 15;
        }
        if i == trailing && trailing < 3 {
            code += 2;
        }
        let level = if code % 2 == 0 {
            (code as i32 + 2) / 2
        } else {
            -(code as i32 + 1) / 2
        };
        levels.push(level);
        suffix = suffix.max(1);
        if suffix < 6 && level.unsigned_abs() > 3 << (suffix - 1) {
            suffix += 1;
        }
    }
    let mut zeros = if total == 0 || total == max {
        0
    } else {
        let table = if max == 4 {
            &vlc::CHROMA_ZEROS[..]
        } else {
            &vlc::ZEROS[..]
        };
        r.vlc(table[total - 1].iter().copied().enumerate())
    };
    let mut runs = Vec::new();
    for _ in 1..total {
        let run = if zeros == 0 {
            0
        } else {
            r.vlc(vlc::RUNS[zeros.min(7) - 1].iter().copied().enumerate())
        };
        zeros -= run;
        runs.push(run);
    }
    if total > 0 {
        runs.push(zeros);
    }
    let mut result = vec![0; max];
    let mut p = 0;
    for (level, run) in levels.into_iter().zip(runs).rev() {
        p += run;
        result[p] = level;
        p += 1;
    }
    result
}

#[test]
fn every_finite_vlc_mapping_has_a_unique_prefix_and_roundtrips() {
    let mut tables: Vec<Vec<u32>> = (0..5)
        .map(|c| vlc::TOKENS.iter().map(|r| r[c]).collect())
        .collect();
    tables.extend(vlc::ZEROS.iter().map(|r| r.to_vec()));
    tables.extend(vlc::CHROMA_ZEROS.iter().map(|r| r.to_vec()));
    tables.extend(vlc::RUNS.iter().map(|r| r.to_vec()));
    assert_eq!(tables[0].iter().filter(|&&v| v != 0).count(), 62);
    assert_eq!(tables[4].iter().filter(|&&v| v != 0).count(), 14);
    for (i, row) in vlc::ZEROS.iter().enumerate() {
        assert_eq!(row.iter().filter(|&&v| v != 0).count(), 16 - i);
    }
    for (i, row) in vlc::RUNS.iter().enumerate() {
        assert_eq!(
            row.iter().filter(|&&v| v != 0).count(),
            if i == 6 { 15 } else { i + 2 }
        );
    }
    for table in tables {
        for (i, &code) in table.iter().enumerate().filter(|(_, v)| **v != 0) {
            let mut w = Bits::new(100);
            w.code(code).unwrap();
            let mut r = reader(w);
            assert_eq!(r.vlc(table.iter().copied().enumerate()), i);
            assert_eq!(r.at, code as usize & 31);
            for (j, &other) in table.iter().enumerate().filter(|(_, v)| **v != 0) {
                if i == j || code & 31 > other & 31 {
                    continue;
                }
                assert_ne!(code >> 5, (other >> 5) >> ((other & 31) - (code & 31)));
            }
        }
    }
    let mut cbp = vlc::INTRA_CBP.to_vec();
    cbp.sort();
    assert_eq!(cbp, (0..48).collect::<Vec<_>>());
    assert_eq!(&vlc::INTRA_CBP[..4], &[47, 31, 15, 0]);
}

#[test]
fn residuals_roundtrip_sparse_dense_signed_and_suffix_boundaries() {
    for max in [4, 15, 16] {
        for nc in if max == 4 {
            vec![-1]
        } else {
            vec![0, 1, 2, 3, 4, 7, 8, 16]
        } {
            for level in [
                -2048, -100, -25, -13, -7, -4, -3, -2, -1, 0, 1, 2, 3, 4, 7, 13, 25, 100, 2048,
            ] {
                for position in 0..max {
                    let mut levels = vec![0; max];
                    levels[position] = level;
                    for dense in [false, true] {
                        if dense {
                            for (p, v) in levels.iter_mut().enumerate() {
                                *v = if p % 3 == 0 {
                                    1
                                } else if p % 3 == 1 {
                                    -1
                                } else {
                                    level
                                };
                            }
                        }
                        let mut w = Bits::new(1000);
                        let total = cavlc::residual(&mut w, &levels, nc).unwrap();
                        let mut counter = Bits::count();
                        assert_eq!(cavlc::residual(&mut counter, &levels, nc).unwrap(), total);
                        assert_eq!(counter.len, w.len);
                        let bits = w.len;
                        let mut r = reader(w);
                        assert_eq!(read_residual(&mut r, max, nc), levels, "max={max} nc={nc}");
                        assert_eq!(r.at, bits);
                    }
                }
            }
        }
    }
}

#[test]
fn unrepresentable_levels_fail_without_wrapping() {
    let mut w = Bits::count();
    assert_eq!(
        cavlc::residual(&mut w, &[i32::MIN, 0, 0, 0], -1),
        Err(Error::Coefficient)
    );
}

#[test]
fn bit_writer_has_checked_fields_and_emulation_prevention() {
    let mut w = Bits::new(100);
    for x in [0, 1, 2, 3, 4, 31, 32, u32::MAX] {
        w.ue(x).unwrap();
    }
    assert!(w.u(3, 8).is_err());
    assert!(w.u(65, 0).is_err());
    let mut w = Bits::new(100);
    w.u(24, 1).unwrap();
    w.u(24, 2).unwrap();
    w.u(24, 3).unwrap();
    w.u(24, 0).unwrap();
    let nal = w.nal(0x65).unwrap();
    assert!(
        nal.windows(3)
            .all(|v| v != [0, 0, 0] && v != [0, 0, 1] && v != [0, 0, 2])
    );
    assert!(nal.windows(3).any(|v| v == [0, 0, 3]));
    let mut w = Bits::new(1);
    assert_eq!(w.u(9, 0), Err(Error::Limit));
}

#[test]
fn forward_basis_and_dc_paths_have_explicit_scales() {
    for p in 0..16 {
        let mut impulse = [0; 16];
        impulse[p] = 1;
        let f = transform::forward(&impulse, &[0; 16], 0, 4).unwrap();
        assert_eq!(f.dc(), 1);
        assert_eq!(f.coefficients().iter().map(|v| v.abs()).max(), Some(4));
    }
    for qp in 10..=51 {
        for (sign, source, prediction) in [(-1, [0; 16], [255; 16]), (1, [255; 16], [0; 16])] {
            let f = transform::forward(&source, &prediction, 0, 4).unwrap();
            assert_eq!(f.dc(), sign * 4080);
            assert!(f.coefficients().iter().skip(1).all(|&v| v == 0));
            let quant = transform::Quant::of(qp).unwrap();
            let q = quant.levels(&f, false);
            assert!(q.iter().skip(1).all(|&v| v == 0));
            let mut block = prediction.to_vec();
            quant.reconstruct(&q, None, &mut block, 0, 4).unwrap();
            assert!(block.iter().all(|&v| v == block[0]));
            let (levels, dc) = transform::luma_dc([sign * 16; 16], qp).unwrap();
            assert!(levels.iter().skip(1).all(|&v| v == 0));
            assert!(dc.iter().all(|&v| v == dc[0]));
            let (levels, dc) =
                transform::chroma_dc([sign * 16; 4], transform::chroma_qp(qp)).unwrap();
            assert!(levels.iter().skip(1).all(|&v| v == 0));
            assert!(dc.iter().all(|&v| v == dc[0]));
        }
    }
    for (qp, expected) in [(29, 29), (30, 29), (31, 30), (34, 32), (39, 35), (51, 39)] {
        assert_eq!(transform::chroma_qp(qp), expected);
    }
}

#[test]
fn checked_views_and_sequence_bounds_reject_before_allocation() {
    assert!(Plane::new(&[], usize::MAX, 16, usize::MAX).is_err());
    assert!(Plane::new(&[0; 16], 3, 4, 4).is_err());
    assert!(Plane::new(&[0; 15], 4, 4, 4).is_err());
    assert!(Plane::new(&[0; 19], 5, 4, 4).is_ok());
    for (w, h, q) in [
        (15, 16, 26),
        (16, 17, 26),
        (1922, 16, 26),
        (16, 1082, 26),
        (16, 16, 9),
        (16, 16, 52),
    ] {
        assert!(Encoder::new(settings(w, h, q), Limits::default()).is_err());
    }
    assert!(Encoder::new(settings(1920, 1080, 26), Limits::default()).is_ok());
    assert!(
        Encoder::new(
            settings(16, 16, 26),
            Limits {
                coded_pixels: 255,
                ..Limits::default()
            }
        )
        .is_err()
    );
}

#[test]
fn late_failure_preserves_the_sequence_and_next_idr_identity() {
    let data = [128; 256];
    let chroma = [128; 64];
    let picture = Picture {
        y: Plane::new(&data, 16, 16, 16).unwrap(),
        cb: Plane::new(&chroma, 8, 8, 8).unwrap(),
        cr: Plane::new(&chroma, 8, 8, 8).unwrap(),
    };
    let mut reference = Encoder::new(settings(16, 16, 26), Limits::default()).unwrap();
    let first = reference.encode(picture).unwrap();
    let mut encoder = Encoder::new(
        settings(16, 16, 26),
        Limits {
            total_bytes: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(encoder.encode(picture).unwrap_err(), Error::Limit);
    encoder.limits.total_bytes = 1_000_000;
    assert_eq!(encoder.encode(picture).unwrap().nals, first.nals);
    assert_eq!(
        encoder.encode(picture).unwrap().nals,
        reference.encode(picture).unwrap().nals
    );
    encoder.limits.frames = 2;
    assert_eq!(encoder.encode(picture).unwrap_err(), Error::Limit);
}

#[test]
fn unspecified_primaries_cannot_signal_derived_colour_matrices() {
    for matrix in [12, 13] {
        let mut s = settings(16, 16, 26);
        s.colour.primaries = 2;
        s.colour.matrix = matrix;
        assert!(matches!(
            Encoder::new(s, Limits::default()),
            Err(Error::Settings)
        ));
        s.colour.primaries = 1;
        assert!(Encoder::new(s, Limits::default()).is_ok());
    }
}

fn dense_bytes() -> [u8; 384] {
    let mut state = 17u32;
    let mut bytes = [0u8; 384];
    for value in &mut bytes {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *value = state as u8;
    }
    bytes
}

#[test]
fn a_small_textured_picture_uses_the_level_ceiling_not_a_raw_size_fraction() {
    let bytes = dense_bytes();
    let picture = Picture {
        y: Plane::new(&bytes[..256], 16, 16, 16).unwrap(),
        cb: Plane::new(&bytes[256..320], 8, 8, 8).unwrap(),
        cr: Plane::new(&bytes[320..], 8, 8, 8).unwrap(),
    };
    let mut encoder = Encoder::new(settings(16, 16, 16), Limits::default()).unwrap();
    let au = encoder.encode(picture).unwrap();
    assert!(au.nals.iter().map(Vec::len).sum::<usize>() > 192);
    let mut limited = Encoder::new(
        settings(16, 16, 16),
        Limits {
            access_unit_bytes: 192,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(limited.encode(picture).unwrap_err(), Error::Limit);
}

#[test]
fn dense_low_qp_content_is_refused_without_advancing_sequence_state() {
    let bytes = dense_bytes();
    let dense = Picture {
        y: Plane::new(&bytes[..256], 16, 16, 16).unwrap(),
        cb: Plane::new(&bytes[256..320], 8, 8, 8).unwrap(),
        cr: Plane::new(&bytes[320..], 8, 8, 8).unwrap(),
    };
    let flat = [128; 384];
    let picture = Picture {
        y: Plane::new(&flat[..256], 16, 16, 16).unwrap(),
        cb: Plane::new(&flat[256..320], 8, 8, 8).unwrap(),
        cr: Plane::new(&flat[320..], 8, 8, 8).unwrap(),
    };
    let mut encoder = Encoder::new(settings(16, 16, 10), Limits::default()).unwrap();
    let mut reference = Encoder::new(settings(16, 16, 10), Limits::default()).unwrap();
    assert_eq!(encoder.encode(dense).unwrap_err(), Error::MacroblockLimit);
    let first = encoder.encode(picture).unwrap();
    let expected = reference.encode(picture).unwrap();
    assert_eq!(first.nals, expected.nals);
    assert_eq!(first.reconstruction, expected.reconstruction);
    let expected = reference.encode(picture).unwrap();
    encoder.limits.frames = 2;
    encoder.limits.total_bytes = first
        .nals
        .iter()
        .chain(&expected.nals)
        .map(Vec::len)
        .sum::<usize>() as u64;
    assert_eq!(encoder.encode(dense).unwrap_err(), Error::MacroblockLimit);
    let second = encoder.encode(picture).unwrap();
    assert_eq!(second.nals, expected.nals);
    assert_eq!(second.reconstruction, expected.reconstruction);
    assert_eq!(encoder.encode(picture).unwrap_err(), Error::Limit);
}

#[test]
fn enabled_filter_reconstructs_the_complete_picture_before_cropping() {
    let (width, height) = (18, 22);
    let y: Vec<_> = (0..width * height)
        .map(|p| 16 + ((p % width * 3 + p / width * 5) % 220) as u8)
        .collect();
    let cb: Vec<_> = (0..width * height / 4)
        .map(|p| 32 + (p % 97) as u8)
        .collect();
    let cr: Vec<_> = (0..width * height / 4)
        .map(|p| 180 - (p % 83) as u8)
        .collect();
    let picture = Picture {
        y: Plane::new(&y, width, width, height).unwrap(),
        cb: Plane::new(&cb, width / 2, width / 2, height / 2).unwrap(),
        cr: Plane::new(&cr, width / 2, width / 2, height / 2).unwrap(),
    };
    let mut s = settings(width, height, 28);
    let original = Encoder::new(s, Limits::default())
        .unwrap()
        .encode(picture)
        .unwrap();
    s.deblocking = Deblocking::Enabled;
    let mut encoder = Encoder::new(s, Limits::default()).unwrap();
    let mut decoder =
        coreav_h264::Decoder::with_kernels(coreav_h264::Scalar, coreav_h264::Limits::default())
            .with_mode(coreav_h264::Mode::EveryPicture);
    let mut expected = Vec::new();
    for _ in 0..2 {
        let au = encoder.encode(picture).unwrap();
        assert_ne!(au.reconstruction, original.reconstruction);
        expected.push(au.reconstruction);
        for nal in &au.nals {
            decoder.decode_nal(nal).unwrap();
        }
        decoder.end_access_unit().unwrap();
    }
    decoder.flush().unwrap();
    let pictures = decoder.take_pictures();
    assert_eq!(pictures.len(), expected.len());
    for (picture, expected) in pictures.iter().zip(expected) {
        let (cb, cr) = picture.chroma();
        let actual: Vec<_> = picture
            .luma()
            .rows()
            .chain(cb.rows())
            .chain(cr.rows())
            .flatten()
            .copied()
            .collect();
        assert_eq!(actual, expected);
    }
    assert!(decoder.warnings().is_empty());
    assert_eq!(decoder.warnings_dropped(), 0);
}

#[test]
fn fixed_gop_reconstructs_every_cropped_picture_and_rejects_zero_interval() {
    let (width, height) = (18, 18);
    let mut s = settings(width, height, 34);
    s.gop = Gop::Fixed { idr_interval: 0 };
    assert!(matches!(
        Encoder::new(s, Limits::default()),
        Err(Error::Settings)
    ));
    s.gop = Gop::Fixed { idr_interval: 2 };
    s.deblocking = Deblocking::Enabled;
    let mut encoder = Encoder::new(s, Limits::default()).unwrap();
    let mut decoder =
        coreav_h264::Decoder::with_kernels(coreav_h264::Scalar, coreav_h264::Limits::default())
            .with_mode(coreav_h264::Mode::EveryPicture);
    let mut expected = std::collections::VecDeque::new();
    let mut count = 0;
    for f in 0..3 {
        let y: Vec<_> = (0..width * height)
            .map(|p| 32 + ((p % width + f) * 3 + p / width * 5) as u8)
            .collect();
        let cb: Vec<_> = (0..width * height / 4)
            .map(|p| 64 + ((p % 9 + f) * 7 + p / 9 * 3) as u8)
            .collect();
        let cr: Vec<_> = (0..width * height / 4)
            .map(|p| 180 - ((p % 9 + f) * 3 + p / 9 * 5) as u8)
            .collect();
        let au = encoder
            .encode(Picture {
                y: Plane::new(&y, width, width, height).unwrap(),
                cb: Plane::new(&cb, width / 2, width / 2, height / 2).unwrap(),
                cr: Plane::new(&cr, width / 2, width / 2, height / 2).unwrap(),
            })
            .unwrap();
        assert_eq!(au.nals[2][0], if f == 1 { 0x61 } else { 0x65 });
        expected.push_back(au.reconstruction);
        for nal in &au.nals {
            decoder.decode_nal(nal).unwrap();
        }
        decoder.end_access_unit().unwrap();
        for picture in decoder.take_pictures() {
            let (cb, cr) = picture.chroma();
            let samples: Vec<_> = picture
                .luma()
                .rows()
                .chain(cb.rows())
                .chain(cr.rows())
                .flatten()
                .copied()
                .collect();
            assert_eq!(samples, expected.pop_front().unwrap());
            count += 1;
        }
    }
    decoder.flush().unwrap();
    for picture in decoder.take_pictures() {
        let (cb, cr) = picture.chroma();
        let samples: Vec<_> = picture
            .luma()
            .rows()
            .chain(cb.rows())
            .chain(cr.rows())
            .flatten()
            .copied()
            .collect();
        assert_eq!(samples, expected.pop_front().unwrap());
        count += 1;
    }
    assert_eq!(count, 3);
    assert!(expected.is_empty());
    assert!(decoder.warnings().is_empty());
    assert_eq!(decoder.warnings_dropped(), 0);
}

#[test]
fn p_failure_retains_reference_identity_frame_number_and_carried_count() {
    let mut s = settings(16, 16, 34);
    s.gop = Gop::Fixed { idr_interval: 3 };
    let mut encoder = Encoder::new(s, Limits::default()).unwrap();
    let y = [104; 256];
    let cb = [92; 64];
    let cr = [156; 64];
    let picture = Picture {
        y: Plane::new(&y, 16, 16, 16).unwrap(),
        cb: Plane::new(&cb, 8, 8, 8).unwrap(),
        cr: Plane::new(&cr, 8, 8, 8).unwrap(),
    };
    let mut reference = Encoder::new(s, Limits::default()).unwrap();
    assert_eq!(
        encoder.encode(picture).unwrap().nals,
        reference.encode(picture).unwrap().nals
    );
    encoder.previous_mv_count = 16;
    reference.previous_mv_count = 16;
    encoder.limits.total_bytes = 1;
    assert_eq!(encoder.encode(picture).unwrap_err(), Error::Limit);
    assert_eq!(encoder.previous_mv_count, 16);
    encoder.limits.total_bytes = Limits::default().total_bytes;
    let retried = encoder.encode(picture).unwrap();
    let fresh = reference.encode(picture).unwrap();
    assert_eq!(retried.nals, fresh.nals);
    assert_eq!(retried.reconstruction, fresh.reconstruction);
    assert_eq!(encoder.previous_mv_count, 0);
    assert_eq!(
        encoder.encode(picture).unwrap().nals,
        reference.encode(picture).unwrap().nals
    );
    assert_eq!(
        encoder.encode(picture).unwrap().nals,
        reference.encode(picture).unwrap().nals
    );
    assert!(api::zeros::<u8>(usize::MAX).is_err());
}

#[test]
fn complete_skip_sequence_wraps_frame_numbers_then_resets_at_idr() {
    let mut s = settings(16, 16, 34);
    s.gop = Gop::Fixed { idr_interval: 17 };
    let mut encoder = Encoder::new(s, Limits::default()).unwrap();
    let mut decoder =
        coreav_h264::Decoder::with_kernels(coreav_h264::Scalar, coreav_h264::Limits::default())
            .with_mode(coreav_h264::Mode::EveryPicture);
    let y = [128; 256];
    let c = [128; 64];
    let picture = Picture {
        y: Plane::new(&y, 16, 16, 16).unwrap(),
        cb: Plane::new(&c, 8, 8, 8).unwrap(),
        cr: Plane::new(&c, 8, 8, 8).unwrap(),
    };
    let mut expected = std::collections::VecDeque::new();
    let mut parameters = None;
    let mut count = 0;
    for f in 0..19 {
        let au = encoder.encode(picture).unwrap();
        let idr = f % 17 == 0;
        let mut reader = nal_reader(&au.nals[2]);
        assert_eq!(reader.ue(), 0);
        assert_eq!(reader.ue(), if idr { 7 } else { 5 });
        assert_eq!(reader.ue(), 0);
        assert_eq!(reader.u(4), if idr { 0 } else { f % 17 % 16 });
        if idr {
            assert_eq!(reader.ue(), f / 17 % 2);
            assert_eq!(reader.u(2), 0);
        } else {
            assert_eq!(reader.u(3), 0);
        }
        assert_eq!(reader.ue(), 15);
        assert_eq!(reader.ue(), 1);
        if !idr {
            assert_eq!(reader.ue(), 1);
            assert_eq!(reader.bit(), 1);
            while reader.at % 8 != 0 {
                assert_eq!(reader.bit(), 0);
            }
            assert_eq!(reader.at / 8, reader.bytes.len());
            assert_eq!(encoder.previous_mv_count, 1);
        } else {
            assert_eq!(encoder.previous_mv_count, 0);
        }
        let pair = [au.nals[0].clone(), au.nals[1].clone()];
        if let Some(previous) = &parameters {
            assert_eq!(&pair, previous);
        } else {
            parameters = Some(pair);
        }
        assert_eq!(au.reconstruction, [128; 384]);
        expected.push_back(au.reconstruction);
        for nal in &au.nals {
            decoder.decode_nal(nal).unwrap();
        }
        decoder.end_access_unit().unwrap();
        for picture in decoder.take_pictures() {
            let (cb, cr) = picture.chroma();
            let actual: Vec<_> = picture
                .luma()
                .rows()
                .chain(cb.rows())
                .chain(cr.rows())
                .flatten()
                .copied()
                .collect();
            assert_eq!(actual, expected.pop_front().unwrap());
            count += 1;
        }
    }
    decoder.flush().unwrap();
    for picture in decoder.take_pictures() {
        let (cb, cr) = picture.chroma();
        let actual: Vec<_> = picture
            .luma()
            .rows()
            .chain(cb.rows())
            .chain(cr.rows())
            .flatten()
            .copied()
            .collect();
        assert_eq!(actual, expected.pop_front().unwrap());
        count += 1;
    }
    assert_eq!(count, 19);
    assert!(expected.is_empty());
    assert!(decoder.warnings().is_empty());
    assert_eq!(decoder.warnings_dropped(), 0);
}

#[test]
fn disabled_all_intra_bytes_preserve_the_original_synthetic_stream() {
    let expected = concat!(
        "000000016742c033da7a6a020203e0000003002000000790800000000168ce3e800000000165888422e4d9315bfc1a80",
        "00807a9544b64f52a894aa12c44f57b9f5caa96d7286002143e18001449075f1d7c75f1d7c1cdc7371cdc73780000000",
        "016742c033da7a6a020203e0000003002000000790800000000168ce3e800000000165888208bb36ccd2ff06a000201a",
        "a55094bc9ea5512954252f27aacb85eaa4f5b2aa50c004286c300028920ebe3af8ebe3af839b8e6e39b8e6f000000001",
        "6742c033da7a6a020203e0000003002000000790800000000168ce3e800000000165888422e419013137f06a0002012a",
        "5512e7870aa129544b9e1c7973639e1cb3c381800850d8800051183af8ebe3af8ebe0e6e39b8e6e39bc0",
    );
    for gop in [Gop::AllIntra, Gop::Fixed { idr_interval: 1 }] {
        let mut s = settings(16, 16, 28);
        s.gop = gop;
        let mut encoder = Encoder::new(s, Limits::default()).unwrap();
        let mut output = Vec::new();
        for f in 0..3 {
            let y: Vec<_> = (0..256)
                .map(|p| (16 + ((p % 16 + f) * 7 + p / 16 * 5) % 220) as u8)
                .collect();
            let cb: Vec<_> = (0..64)
                .map(|p| (32 + (p % 8 * 9 + p / 8 * 7 + f) % 180) as u8)
                .collect();
            let cr: Vec<_> = (0..64)
                .map(|p| (180 - (p % 8 * 3 + p / 8 * 11 + f) % 140) as u8)
                .collect();
            let au = encoder
                .encode(Picture {
                    y: Plane::new(&y, 16, 16, 16).unwrap(),
                    cb: Plane::new(&cb, 8, 8, 8).unwrap(),
                    cr: Plane::new(&cr, 8, 8, 8).unwrap(),
                })
                .unwrap();
            assert!(
                encoder.reference.is_none(),
                "{gop:?} retains no coded reference"
            );
            for nal in au.nals {
                output.extend([0, 0, 0, 1]);
                output.extend(nal);
            }
        }
        let actual: String = output.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(actual, expected);
    }
}

fn xorshift(seed: u32) -> Vec<u8> {
    let mut x = seed;
    (0..384)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn encode_16x16(encoder: &mut Encoder, raw: &[u8]) -> Result<String> {
    let (y, uv) = raw.split_at(256);
    let (cb, cr) = uv.split_at(64);
    let au = encoder.encode(Picture {
        y: Plane::new(y, 16, 16, 16).unwrap(),
        cb: Plane::new(cb, 8, 8, 8).unwrap(),
        cr: Plane::new(cr, 8, 8, 8).unwrap(),
    })?;
    Ok(au
        .nals
        .iter()
        .flat_map(|nal| [0, 0, 0, 1].iter().chain(nal))
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[test]
fn idr_shortlist_falls_back_to_every_candidate_when_none_is_admitted() {
    for (seed, qp) in [(5, 16), (18, 16), (20, 16), (10, 15)] {
        let mut encoder = Encoder::new(settings(16, 16, qp), Limits::default()).unwrap();
        assert!(
            encode_16x16(&mut encoder, &xorshift(seed)).is_ok(),
            "{seed} {qp}"
        );
    }
}

#[test]
fn p_intra_fallback_keeps_every_candidate() {
    let mut s = settings(16, 16, 16);
    s.gop = Gop::Fixed { idr_interval: 32 };
    let mut encoder = Encoder::new(s, Limits::default()).unwrap();
    encode_16x16(&mut encoder, &[128; 384]).unwrap();
    let p = encode_16x16(&mut encoder, &xorshift(1)).unwrap();
    let expected = concat!(
        "000000016742c033da7a6a020203e0000003002000000790800000000168ce3e8000000001619a202aa6bd5dd909fb32",
        "f00040b8e021809fcc17ac9e9c9f9abde0301035508134d56b9f84d51b53fde000080d8039050b6843463b4922b994d1",
        "e00097a00803872583f89e74f381ef83017560025e0c99b736f19d274a35f0000401403a408438307ace3e2f8dc2287c",
        "0001004053201793da56c56aa815475f810e4879dd8335e50726fb368875e0000805060ebcc268aeed078f2c9d41e781",
        "0e02d84fbb438c8364084d871d7c010107989eb6174be48d626894afc20404d031a74b0b0dc8f99d57da780102800d63",
        "f25dbdf4267f9aa7887c0ca00049060339f912dd26ef173e7804002a848ca4ff166509e1d6498a3c00030400528dac30",
        "4b2925c6be4a9780800040460200202000101a041cc8005800080480e0bb6c2536f2c8c7728a1ff70801840285776b42",
        "c8b0a5d20d7800838638da06ecf03680dbd4ed3880004010000404802c1c4be65530f60e2a40000e040a00020084910b",
        "c4838438d34c5668000200a2800b198396b9dd5a8bb2d9ee000c7004b61454f9d6d263466649f4080080d39fd5a670a1",
        "f6d1075a",
    );
    assert_eq!(p, expected);
}

#[test]
fn thread_counts_preserve_streams_reconstruction_and_sequence_rollback() {
    for (width, height) in [(16, 80), (18, 34), (32, 48), (80, 66), (96, 96)] {
        for qp in [22, 37] {
            for deblocking in [Deblocking::Disabled, Deblocking::Enabled] {
                let mut settings = settings(width, height, qp);
                settings.deblocking = deblocking;
                settings.gop = Gop::Fixed { idr_interval: 3 };
                let mut encoders: Vec<_> = [1, 2, 3, 8]
                    .into_iter()
                    .map(|threads| {
                        Encoder::with_threads(settings, Limits::default(), threads).unwrap()
                    })
                    .collect();
                let mut control = Encoder::with_threads(settings, Limits::default(), 1).unwrap();
                assert!(Encoder::with_threads(settings, Limits::default(), 0).is_err());
                for frame in 0..5 {
                    let mut seed = 0x81f259a3u32 + frame;
                    let raw: Vec<_> = (0..width * height * 3 / 2)
                        .map(|p| {
                            seed ^= seed << 13;
                            seed ^= seed >> 17;
                            seed ^= seed << 5;
                            (80 + (p + frame as usize * 3) % 64 + (seed as usize % 16)) as u8
                        })
                        .collect();
                    let (y, uv) = raw.split_at(width * height);
                    let (cb, cr) = uv.split_at(width * height / 4);
                    let picture = Picture {
                        y: Plane::new(y, width, width, height).unwrap(),
                        cb: Plane::new(cb, width / 2, width / 2, height / 2).unwrap(),
                        cr: Plane::new(cr, width / 2, width / 2, height / 2).unwrap(),
                    };
                    let outputs: Vec<_> = encoders
                        .iter_mut()
                        .map(|encoder| encoder.encode(picture).unwrap())
                        .collect();
                    let expected = control.encode(picture).unwrap();
                    assert_eq!(outputs[0].nals, expected.nals);
                    assert_eq!(outputs[0].reconstruction, expected.reconstruction);
                    for other in &outputs[1..] {
                        assert_eq!(outputs[0].nals, other.nals);
                        assert_eq!(outputs[0].reconstruction, other.reconstruction);
                    }
                    for encoder in &mut encoders {
                        let previous = encoder.previous_mv_count;
                        let old = encoder.limits.access_unit_bytes;
                        encoder.limits.access_unit_bytes = 1;
                        assert!(matches!(encoder.encode(picture), Err(Error::Limit)));
                        assert_eq!(encoder.previous_mv_count, previous);
                        encoder.limits.access_unit_bytes = old;
                        let old = encoder.limits.total_bytes;
                        encoder.limits.total_bytes = 0;
                        assert!(matches!(encoder.encode(picture), Err(Error::Limit)));
                        assert_eq!(encoder.previous_mv_count, previous);
                        encoder.limits.total_bytes = old;
                    }
                }
            }
        }
    }
}

#[test]
fn eight_worker_textured_multirow_p_pictures_match_independent_decoding() {
    let (width, height) = (320, 128);
    let mut settings = settings(width, height, 27);
    settings.gop = Gop::Fixed { idr_interval: 32 };
    settings.deblocking = Deblocking::Enabled;
    let mut encoder = Encoder::with_threads(settings, Limits::default(), 8).unwrap();
    let mut decoder =
        coreav_h264::Decoder::with_kernels(coreav_h264::Scalar, coreav_h264::Limits::default())
            .with_mode(coreav_h264::Mode::EveryPicture);
    let mut expected = Vec::new();
    let chroma = vec![128; width * height / 4];
    for frame in 0..4 {
        let y: Vec<_> = (0..width * height)
            .map(|p| {
                if frame == 0 {
                    128
                } else {
                    let (x, y) = (p % width + frame * 3, p / width);
                    (16 + (x * 13 + y * 7 + x / 7 * (y / 3) * 11 + frame * 19) % 224) as u8
                }
            })
            .collect();
        let output = encoder
            .encode(Picture {
                y: Plane::new(&y, width, width, height).unwrap(),
                cb: Plane::new(&chroma, width / 2, width / 2, height / 2).unwrap(),
                cr: Plane::new(&chroma, width / 2, width / 2, height / 2).unwrap(),
            })
            .unwrap();
        let mut syntax = nal_reader(&output.nals[2]);
        assert_eq!(syntax.ue(), 0);
        assert_eq!(syntax.ue(), if frame == 0 { 7 } else { 5 });
        expected.push(output.reconstruction);
        for nal in &output.nals {
            decoder.decode_nal(nal).unwrap();
        }
        decoder.end_access_unit().unwrap();
    }
    decoder.flush().unwrap();
    let decoded = decoder.take_pictures();
    assert_eq!(decoded.len(), expected.len());
    for (frame, reference) in decoded.iter().zip(expected) {
        let (cb, cr) = frame.chroma();
        let actual: Vec<_> = frame
            .luma()
            .rows()
            .chain(cb.rows())
            .chain(cr.rows())
            .flatten()
            .copied()
            .collect();
        assert_eq!(actual, reference);
    }
    assert!(decoder.warnings().is_empty());
    assert_eq!(decoder.warnings_dropped(), 0);
}
