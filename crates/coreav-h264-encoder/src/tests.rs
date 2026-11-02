#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use bits::Bits;

fn settings(width: usize, height: usize, qp: u8) -> Settings {
    Settings {
        width,
        height,
        qp,
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
        let f = transform::forward(&impulse).unwrap();
        assert_eq!(f[0], 1);
        assert_eq!(f.iter().map(|v| v.abs()).max(), Some(4));
    }
    for qp in 10..=51 {
        for sign in [-1, 1] {
            let f = transform::forward(&[sign * 255; 16]).unwrap();
            assert_eq!(f[0], i64::from(sign) * 4080);
            assert!(f.iter().skip(1).all(|&v| v == 0));
            let q = transform::quantize(&f, qp, false).unwrap();
            let d = transform::scaled(&q, qp, None).unwrap();
            assert!(d.iter().skip(1).all(|&v| v == 0));
            let (levels, dc) = transform::luma_dc([i64::from(sign) * 16; 16], qp).unwrap();
            assert!(levels.iter().skip(1).all(|&v| v == 0));
            assert!(dc.iter().all(|&v| v == dc[0]));
            let (levels, dc) =
                transform::chroma_dc([i64::from(sign) * 16; 4], transform::chroma_qp(qp)).unwrap();
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
