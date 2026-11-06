use crate::{BitDepth, Error};

/// Normative boundary filtering strength under H.265 clause 8.7.2.4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryStrength {
    /// No filtering.
    Zero,
    /// A residual or reference/motion discontinuity between inter blocks.
    One,
    /// At least one side uses intra prediction.
    Two,
}

impl BoundaryStrength {
    /// Returns the normative value in 0..=2.
    pub const fn value(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::One => 1,
            Self::Two => 2,
        }
    }
}

fn thresholds(
    depth: BitDepth,
    qp: i32,
    offsets: [i32; 2],
    strength: BoundaryStrength,
) -> Result<(i32, i32), Error> {
    if !(-6 * (i32::from(depth.bits()) - 8)..=51).contains(&qp)
        || offsets.into_iter().any(|v| !(-6..=6).contains(&v))
    {
        return Err(Error::Qp);
    }
    let index = (qp + 2 * offsets[0]).clamp(0, 51);
    let beta = match index {
        0..=15 => 0,
        16..=28 => index - 10,
        _ => 2 * index - 38,
    };
    let scale = 1 << (depth.bits() - 8);
    Ok((
        beta * scale,
        tc(qp + 2 * (i32::from(strength.value()) - 1) + 2 * offsets[1]) * scale,
    ))
}

fn tc(index: i32) -> i32 {
    match index.clamp(0, 53) {
        0..=17 => 0,
        18..=26 => 1,
        27..=30 => 2,
        31..=34 => 3,
        35..=37 => 4,
        38..=39 => 5,
        40..=41 => 6,
        42 => 7,
        43 => 8,
        44 => 9,
        45 => 10,
        46 => 11,
        47 => 13,
        48 => 14,
        49 => 16,
        50 => 18,
        51 => 20,
        52 => 22,
        _ => 24,
    }
}

fn curvature(line: [u16; 8]) -> [i32; 2] {
    let [_, p2, p1, p0, q0, q1, q2, _] = line.map(i32::from);
    [(p2 - 2 * p1 + p0).abs(), (q2 - 2 * q1 + q0).abs()]
}

fn strong(line: [u16; 8], beta: i32, tc: i32) -> bool {
    let [p3, _, _, p0, q0, _, _, q3] = line.map(i32::from);
    let [dp, dq] = curvature(line);
    2 * (dp + dq) < beta >> 2
        && (p3 - p0).abs() + (q0 - q3).abs() < beta >> 3
        && (p0 - q0).abs() < (5 * tc + 1) >> 1
}

/// Filters four intra luma edge lines, each ordered p3,p2,p1,p0,q0,q1,q2,q3.
/// `qp` is the rounded mean of the two luma QPs; protected sides retain their input.
pub fn deblock_luma(
    depth: BitDepth,
    edge: [[u16; 8]; 4],
    qp: i32,
    offsets: [i32; 2],
    protected: [bool; 2],
) -> Result<[[u16; 8]; 4], Error> {
    deblock_luma_with_strength(depth, edge, qp, offsets, BoundaryStrength::Two, protected)
}

/// Filters four luma edge lines with the supplied normative boundary strength.
/// QP, offsets and samples are validated even when strength is zero.
pub fn deblock_luma_with_strength(
    depth: BitDepth,
    edge: [[u16; 8]; 4],
    qp: i32,
    offsets: [i32; 2],
    strength: BoundaryStrength,
    protected: [bool; 2],
) -> Result<[[u16; 8]; 4], Error> {
    if edge.iter().flatten().any(|v| *v > depth.max_sample()) {
        return Err(Error::Sample);
    }
    let (beta, tc) = thresholds(depth, qp, offsets, strength)?;
    if strength == BoundaryStrength::Zero {
        return Ok(edge);
    }
    let first = *edge.first().ok_or(Error::Buffer)?;
    let last = *edge.last().ok_or(Error::Buffer)?;
    let [dp0, dq0] = curvature(first);
    let [dp3, dq3] = curvature(last);
    let dp = dp0 + dp3;
    let dq = dq0 + dq3;
    if dp + dq >= beta {
        return Ok(edge);
    }
    let strong = strong(first, beta, tc) && strong(last, beta, tc);
    let secondary = (beta + (beta >> 1)) >> 3;
    let mut result = edge;
    for (destination, original) in result.iter_mut().zip(edge) {
        let [p3, p2, p1, p0, q0, q1, q2, q3] = original.map(i32::from);
        let values = if strong {
            [
                p3,
                ((2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3).clamp(p2 - 2 * tc, p2 + 2 * tc),
                ((p2 + p1 + p0 + q0 + 2) >> 2).clamp(p1 - 2 * tc, p1 + 2 * tc),
                ((p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3).clamp(p0 - 2 * tc, p0 + 2 * tc),
                ((p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3).clamp(q0 - 2 * tc, q0 + 2 * tc),
                ((p0 + q0 + q1 + q2 + 2) >> 2).clamp(q1 - 2 * tc, q1 + 2 * tc),
                ((p0 + q0 + q1 + 3 * q2 + 2 * q3 + 4) >> 3).clamp(q2 - 2 * tc, q2 + 2 * tc),
                q3,
            ]
        } else {
            let delta = (9 * (q0 - p0) - 3 * (q1 - p1) + 8) >> 4;
            if delta.abs() >= 10 * tc {
                continue;
            }
            let delta = delta.clamp(-tc, tc);
            let half = tc >> 1;
            [
                p3,
                p2,
                p1 + if dp < secondary {
                    ((((p2 + p0 + 1) >> 1) - p1 + delta) >> 1).clamp(-half, half)
                } else {
                    0
                },
                p0 + delta,
                q0 - delta,
                q1 + if dq < secondary {
                    ((((q2 + q0 + 1) >> 1) - q1 - delta) >> 1).clamp(-half, half)
                } else {
                    0
                },
                q2,
                q3,
            ]
        };
        for (index, (sample, value)) in destination.iter_mut().zip(values).enumerate() {
            if !protected
                .get(usize::from(index >= 4))
                .copied()
                .ok_or(Error::Buffer)?
            {
                *sample = depth.clip(i64::from(value));
            }
        }
    }
    Ok(result)
}

/// Filters four chroma edge lines ordered p1,p0,q0,q1.
/// `qp` includes only the averaged luma QP and picture-level chroma offset.
pub fn deblock_chroma(
    depth: BitDepth,
    edge: [[u16; 4]; 4],
    qp: i32,
    tc_offset: i32,
    protected: [bool; 2],
) -> Result<[[u16; 4]; 4], Error> {
    if !(-24..=63).contains(&qp) || !(-6..=6).contains(&tc_offset) {
        return Err(Error::Qp);
    }
    if edge.iter().flatten().any(|v| *v > depth.max_sample()) {
        return Err(Error::Sample);
    }
    let qp = crate::chroma_qp_index(qp)?;
    let tc = tc(qp + 2 + 2 * tc_offset) << (depth.bits() - 8);
    let [protect_p, protect_q] = protected;
    Ok(edge.map(|line| {
        let [p1, p0, q0, q1] = line.map(i32::from);
        let delta = ((4 * (q0 - p0) + p1 - q1 + 4) >> 3).clamp(-tc, tc);
        [
            p1 as u16,
            if protect_p {
                p0 as u16
            } else {
                depth.clip(i64::from(p0 + delta))
            },
            if protect_q {
                q0 as u16
            } else {
                depth.clip(i64::from(q0 - delta))
            },
            q1 as u16,
        ]
    }))
}

/// SAO classification inputs gathered from the unmodified deblocked component.
#[derive(Clone, Copy, Debug)]
pub enum SaoClass {
    /// Starting band position in 0..32.
    Band(u8),
    /// The two available neighbours for an edge orientation.
    Edge([u16; 2]),
}

/// Applies four decoded SAO offsets to one unprotected sample.
pub fn sao_sample(
    depth: BitDepth,
    sample: u16,
    offsets: [i16; 4],
    class: SaoClass,
) -> Result<u16, Error> {
    let maximum = (1i16 << (depth.bits() - 5)) - 1;
    if sample > depth.max_sample() || offsets.into_iter().any(|v| v < -maximum || v > maximum) {
        return Err(Error::Sample);
    }
    let index = match class {
        SaoClass::Band(position) => {
            if position >= 32 {
                return Err(Error::Sample);
            }
            let index = ((sample >> (depth.bits() - 5)) + 32 - u16::from(position)) & 31;
            if index >= 4 {
                return Ok(sample);
            }
            index as usize
        }
        SaoClass::Edge([a, b]) => {
            if a > depth.max_sample() || b > depth.max_sample() {
                return Err(Error::Sample);
            }
            let sign = (i32::from(sample) - i32::from(a)).signum()
                + (i32::from(sample) - i32::from(b)).signum();
            match sign {
                -2 => 0,
                -1 => 1,
                0 => return Ok(sample),
                1 => 2,
                _ => 3,
            }
        }
    };
    Ok(depth.clip(i64::from(sample) + i64::from(*offsets.get(index).ok_or(Error::Buffer)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inter_strength_uses_its_own_tc_index_and_zero_never_filters() -> Result<(), Error> {
        for bits in [8, 10] {
            let depth = BitDepth::new(bits)?;
            let scale = 1u16 << (bits - 8);
            let edge = [[100, 100, 100, 100, 140, 140, 140, 140].map(|v| v * scale); 4];
            for (qp, offsets) in [(26, [0, 0]), (38, [0, -6])] {
                assert_eq!(
                    deblock_luma_with_strength(
                        depth,
                        edge,
                        qp,
                        offsets,
                        BoundaryStrength::Zero,
                        [false; 2]
                    )?,
                    edge
                );
                assert_eq!(
                    deblock_luma_with_strength(
                        depth,
                        edge,
                        qp,
                        offsets,
                        BoundaryStrength::One,
                        [false; 2]
                    )?,
                    edge
                );
                assert_eq!(
                    deblock_luma_with_strength(
                        depth,
                        edge,
                        qp,
                        offsets,
                        BoundaryStrength::Two,
                        [false; 2]
                    )?,
                    [[100, 100, 101, 102, 138, 139, 140, 140].map(|v| v * scale); 4]
                );
            }
            assert_eq!(
                deblock_luma_with_strength(
                    depth,
                    [[depth.max_sample() + 1; 8]; 4],
                    26,
                    [0; 2],
                    BoundaryStrength::Zero,
                    [false; 2]
                ),
                Err(Error::Sample)
            );
        }
        Ok(())
    }

    #[test]
    fn strength_dependent_thresholds_follow_equations_8_348_through_8_351() -> Result<(), Error> {
        for (qp, beta, one, two) in [
            (15, 0, 0, 0),
            (16, 6, 0, 1),
            (26, 16, 1, 2),
            (30, 22, 2, 3),
            (38, 38, 5, 6),
            (51, 64, 20, 24),
        ] {
            for bits in [8, 10] {
                let depth = BitDepth::new(bits)?;
                let scale = 1 << (bits - 8);
                assert_eq!(
                    thresholds(depth, qp, [0; 2], BoundaryStrength::One)?,
                    (beta * scale, one * scale)
                );
                assert_eq!(
                    thresholds(depth, qp, [0; 2], BoundaryStrength::Two)?,
                    (beta * scale, two * scale)
                );
            }
        }
        Ok(())
    }

    #[test]
    fn equation_derived_flat_strong_and_normal_edges() -> Result<(), Error> {
        let depth = BitDepth::new(8)?;
        assert_eq!(
            deblock_luma(
                depth,
                [[100, 100, 100, 100, 104, 104, 104, 104]; 4],
                32,
                [0; 2],
                [false; 2]
            )?,
            [[100, 101, 101, 102, 103, 103, 104, 104]; 4]
        );
        assert_eq!(
            deblock_luma(
                depth,
                [[100, 100, 100, 100, 140, 140, 140, 140]; 4],
                32,
                [0; 2],
                [false; 2]
            )?,
            [[100, 100, 101, 103, 137, 139, 140, 140]; 4]
        );
        assert_eq!(
            deblock_chroma(depth, [[100, 100, 140, 140]; 4], 32, 0, [false; 2])?,
            [[100, 103, 137, 140]; 4]
        );
        assert_eq!(
            deblock_luma(
                BitDepth::new(10)?,
                [[400, 400, 400, 400, 416, 416, 416, 416]; 4],
                32,
                [0; 2],
                [false; 2]
            )?,
            [[400, 402, 404, 406, 410, 412, 414, 416]; 4]
        );
        Ok(())
    }

    #[test]
    fn depths_protection_and_zero_thresholds() -> Result<(), Error> {
        for bits in [8, 10] {
            let depth = BitDepth::new(bits)?;
            let edge = [[100, 100, 100, 100, 104, 104, 104, 104]; 4];
            assert_eq!(deblock_luma(depth, edge, 0, [0; 2], [false; 2])?, edge);
            assert_eq!(deblock_luma(depth, edge, 32, [0; 2], [true; 2])?, edge);
            let result = deblock_luma(depth, edge, 32, [0; 2], [true, false])?;
            for line in result {
                let [p3, p2, p1, p0, _, _, _, _] = line;
                assert_eq!([p3, p2, p1, p0], [100; 4]);
            }
            assert_eq!(
                sao_sample(depth, depth.max_sample(), [1; 4], SaoClass::Band(31))?,
                depth.max_sample()
            );
            assert_eq!(sao_sample(depth, 0, [-1; 4], SaoClass::Band(0))?, 0);
            assert_eq!(
                sao_sample(depth, 4, [3, 1, -1, -3], SaoClass::Edge([5, 5]))?,
                7
            );
            assert_eq!(
                sao_sample(depth, 4, [3, 1, -1, -3], SaoClass::Edge([3, 5]))?,
                4
            );
        }
        Ok(())
    }
}
