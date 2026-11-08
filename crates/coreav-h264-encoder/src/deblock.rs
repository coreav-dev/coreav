#![forbid(unsafe_code)]

use crate::motion::MotionMb;
use crate::{Error, Result};
use coreav_kernels_h264::{EdgeMut, EdgeParams, Kernels, Reach, Scalar};

const ALPHA: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 5, 6, 7, 8, 9, 10, 12, 13, 15, 17, 20,
    22, 25, 28, 32, 36, 40, 45, 50, 56, 63, 71, 80, 90, 101, 113, 127, 144, 162, 182, 203, 226,
    255, 255,
];
const BETA: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 6, 6, 7, 7, 8, 8,
    9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 15, 16, 16, 17, 17, 18, 18,
];
const TC0_3: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 3,
    3, 3, 4, 4, 4, 5, 6, 6, 7, 8, 9, 10, 11, 13, 14, 16, 18, 20, 23, 25,
];
const TC0_1: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 6, 6, 7, 8, 9, 10, 11, 13,
];
const TC0_2: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2,
    2, 2, 2, 3, 3, 3, 4, 4, 5, 5, 6, 7, 8, 8, 10, 11, 12, 13, 15, 17,
];

#[derive(Clone, Copy, Default)]
pub(crate) struct Record {
    pub(crate) motion: MotionMb,
    pub(crate) nonzero: u16,
}

fn strength(p: &Record, q: &Record, pi: usize, qi: usize, external: bool) -> Result<u8> {
    if p.motion.intra || q.motion.intra {
        return Ok(if external { 4 } else { 3 });
    }
    if (p.nonzero & (1 << pi)) != 0 || (q.nonzero & (1 << qi)) != 0 {
        return Ok(2);
    }
    let p = p.motion.vectors.get(pi).ok_or(Error::Internal)?;
    let q = q.motion.vectors.get(qi).ok_or(Error::Internal)?;
    Ok(u8::from((p.x - q.x).abs() >= 4 || (p.y - q.y).abs() >= 4))
}

#[cfg(test)]
pub(crate) fn intra(
    data: &mut [u8],
    width: usize,
    height: usize,
    qp: u8,
    chroma: bool,
) -> Result<()> {
    picture(
        data,
        width,
        height,
        qp,
        chroma,
        |_, _| Ok(Record::default()),
    )
}

#[cfg(test)]
pub(crate) fn picture(
    data: &mut [u8],
    width: usize,
    height: usize,
    qp: u8,
    chroma: bool,
    record: impl Fn(usize, usize) -> Result<Record>,
) -> Result<()> {
    let n = if chroma { 8 } else { 16 };
    rows(data, width, qp, chroma, 0..height / n, record)
}

#[cfg(test)]
fn row(
    data: &mut [u8],
    width: usize,
    qp: u8,
    chroma: bool,
    my: usize,
    record: impl Fn(usize, usize) -> Result<Record>,
) -> Result<()> {
    rows(data, width, qp, chroma, my..my + 1, record)
}

pub(crate) fn rows(
    data: &mut [u8],
    width: usize,
    qp: u8,
    chroma: bool,
    rows: std::ops::Range<usize>,
    record: impl Fn(usize, usize) -> Result<Record>,
) -> Result<()> {
    let index = usize::from(qp);
    let mut tc0 = [0; 8];
    *tc0.get_mut(3).ok_or(Error::Internal)? = *TC0_3.get(index).ok_or(Error::Internal)?;
    *tc0.get_mut(1).ok_or(Error::Internal)? = *TC0_1.get(index).ok_or(Error::Internal)?;
    *tc0.get_mut(2).ok_or(Error::Internal)? = *TC0_2.get(index).ok_or(Error::Internal)?;
    let mut params = EdgeParams {
        alpha: *ALPHA.get(index).ok_or(Error::Internal)?,
        beta: *BETA.get(index).ok_or(Error::Internal)?,
        tc0,
        bs: [0; 4],
    };
    let (n, reach) = if chroma {
        (8, Reach::Chroma)
    } else {
        (16, Reach::Luma)
    };
    for my in rows {
        let y = my * n;
        for x in (0..width).step_by(n) {
            let q = record(x / n, y / n)?;
            let left = if x != 0 {
                Some(record(x / n - 1, y / n)?)
            } else {
                None
            };
            let top = if y != 0 {
                Some(record(x / n, y / n - 1)?)
            } else {
                None
            };
            for vertical in [true, false] {
                for offset in (0..n).step_by(4) {
                    let p = match (offset, vertical) {
                        (0, true) => left.as_ref(),
                        (0, false) => top.as_ref(),
                        _ => Some(&q),
                    };
                    let Some(p) = p else {
                        continue;
                    };
                    let edge = offset * 4 / n;
                    for (segment, bs) in params.bs.iter_mut().enumerate() {
                        let (qi, pi) = if vertical {
                            (
                                segment * 4 + edge,
                                segment * 4 + if edge == 0 { 3 } else { edge - 1 },
                            )
                        } else {
                            (
                                edge * 4 + segment,
                                (if edge == 0 { 3 } else { edge - 1 }) * 4 + segment,
                            )
                        };
                        *bs = strength(p, &q, pi, qi, offset == 0)?;
                    }
                    if params.bs == [0; 4] {
                        continue;
                    }
                    let (q0, across, along) = if vertical {
                        (y * width + x + offset, 1, width)
                    } else {
                        ((y + offset) * width + x, width, 1)
                    };
                    let edge = EdgeMut::new(data, q0, across, along, n, reach)
                        .map_err(|_| Error::Internal)?;
                    Scalar.filter_edge(edge, &params);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oriented(row: &[u8], side: usize, vertical: bool) -> Vec<u8> {
        (0..side * side)
            .map(|p| row[if vertical { p % side } else { p / side }])
            .collect()
    }

    #[test]
    fn incremental_rows_match_whole_picture_with_mixed_edges() {
        for chroma in [false, true] {
            let n = if chroma { 8 } else { 16 };
            for (columns, rows) in [(1, 1), (1, 5), (5, 1), (5, 4)] {
                let width = columns * n;
                let height = rows * n;
                let raw: Vec<_> = (0..width * height)
                    .map(|p| 100 + ((p % width / 4 + p / width / 4) % 5) as u8 * 2)
                    .collect();
                let record = |mx, my| {
                    let mut record = Record::default();
                    if (mx + my) % 3 != 0 {
                        record.motion.intra = false;
                        record.nonzero = if mx % 2 == 0 { 0x55aa } else { 0 };
                        for (p, mv) in record.motion.vectors.iter_mut().enumerate() {
                            mv.x = (p % 4) as i32 * 4;
                            mv.y = (p / 4) as i32 * 4;
                        }
                    }
                    Ok(record)
                };
                for qp in [10, 22, 28, 40, 51] {
                    let mut expected = raw.clone();
                    picture(&mut expected, width, height, qp, chroma, record).unwrap();
                    let mut incremental = vec![0; raw.len()];
                    for my in 0..rows {
                        let start = my * n * width;
                        let end = start + n * width;
                        incremental[start..end].copy_from_slice(&raw[start..end]);
                        row(&mut incremental, width, qp, chroma, my, |mx, y| {
                            assert!(y <= my);
                            record(mx, y)
                        })
                        .unwrap();
                        assert!(incremental[end..].iter().all(|&p| p == 0));
                    }
                    assert_eq!(incremental, expected);
                }
            }
        }
    }

    #[test]
    fn strong_macroblock_edges_have_hand_derived_samples_at_every_qp() {
        for chroma in [false, true] {
            let n = if chroma { 8 } else { 16 };
            let side = n * 2;
            let input: Vec<_> = (0..side).map(|p| if p < n { 100 } else { 102 }).collect();
            for qp in 10..=51 {
                let mut expected = input.clone();
                if qp >= 16 {
                    expected[n - 1] = 101;
                    if !chroma {
                        expected[n - 2] = 101;
                        expected[n] = 101;
                    }
                }
                for vertical in [true, false] {
                    let mut data = oriented(&input, side, vertical);
                    intra(&mut data, side, side, qp, chroma).unwrap();
                    assert_eq!(
                        data,
                        oriented(&expected, side, vertical),
                        "qp={qp} chroma={chroma} vertical={vertical}"
                    );
                }
            }
        }
    }

    #[test]
    fn normal_internal_edges_apply_the_specified_clipping_and_order() {
        let mut input = [108; 16];
        input[..4].fill(100);
        let mut expected = input;
        expected[2..7].copy_from_slice(&[102, 103, 105, 106, 107]);
        for vertical in [true, false] {
            let mut data = oriented(&input, 16, vertical);
            intra(&mut data, 16, 16, 28, false).unwrap();
            assert_eq!(data, oriented(&expected, 16, vertical));
        }
    }

    #[test]
    fn threshold_equality_does_not_filter_and_picture_boundaries_are_untouched() {
        for chroma in [false, true] {
            let n = if chroma { 8 } else { 16 };
            let side = n * 2;
            for (qp, &alpha) in ALPHA.iter().enumerate().skip(10) {
                let row: Vec<_> = (0..side).map(|p| if p < n { 0 } else { alpha }).collect();
                for vertical in [true, false] {
                    let mut data = oriented(&row, side, vertical);
                    let before = data.clone();
                    intra(&mut data, side, side, qp as u8, chroma).unwrap();
                    assert_eq!(data, before, "alpha equality qp={qp}");
                }
            }
            let row: Vec<_> = (0..n)
                .map(|p| {
                    if p < 3 {
                        93
                    } else if p == 3 {
                        100
                    } else {
                        102
                    }
                })
                .collect();
            for vertical in [true, false] {
                let mut data = oriented(&row, n, vertical);
                let before = data.clone();
                intra(&mut data, n, n, 28, chroma).unwrap();
                assert_eq!(data, before, "beta equality chroma={chroma}");
            }
        }
    }

    #[test]
    fn inter_strengths_follow_intra_then_coefficients_then_component_differences() {
        let mut p = Record {
            motion: MotionMb {
                intra: false,
                ..MotionMb::default()
            },
            nonzero: 0,
        };
        let mut q = p;
        assert_eq!(strength(&p, &q, 3, 0, true).unwrap(), 0);
        for delta in [-4, 4] {
            q.motion.vectors[0].x = delta;
            assert_eq!(strength(&p, &q, 3, 0, true).unwrap(), 1);
            q.motion.vectors[0].x = 0;
            q.motion.vectors[0].y = delta;
            assert_eq!(strength(&p, &q, 3, 0, true).unwrap(), 1);
            q.motion.vectors[0].y = 0;
        }
        q.motion.vectors[0].x = 3;
        q.motion.vectors[0].y = -3;
        assert_eq!(strength(&p, &q, 3, 0, true).unwrap(), 0);
        p.nonzero = 1 << 3;
        assert_eq!(strength(&p, &q, 3, 0, true).unwrap(), 2);
        p.nonzero = 0;
        q.nonzero = 1;
        assert_eq!(strength(&p, &q, 3, 0, true).unwrap(), 2);
        p.motion.intra = true;
        assert_eq!(strength(&p, &q, 3, 0, true).unwrap(), 4);
        assert_eq!(strength(&p, &q, 3, 0, false).unwrap(), 3);
        assert_eq!(TC0_1[33], 2);
        assert_eq!(TC0_2[33], 2);
        assert_eq!(TC0_3[33], 3);
        assert_eq!([TC0_1[51], TC0_2[51], TC0_3[51]], [13, 17, 25]);
    }
}
