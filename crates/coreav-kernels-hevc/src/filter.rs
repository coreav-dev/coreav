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

pub(super) fn thresholds(
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

pub(super) fn tc(index: i32) -> i32 {
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

pub(super) fn curvature(line: [u16; 8]) -> [i32; 2] {
    let [_, p2, p1, p0, q0, q1, q2, _] = line.map(i32::from);
    [(p2 - 2 * p1 + p0).abs(), (q2 - 2 * q1 + q0).abs()]
}

pub(super) fn strong(line: [u16; 8], beta: i32, tc: i32) -> bool {
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
    #[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
    let avx2 = (depth.bits() == 8)
        .then(crate::simd::Avx2::detect)
        .flatten();
    #[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
    if let Some(avx2) = avx2 {
        return avx2.deblock_luma(edge, qp, offsets, strength, protected);
    }
    deblock_luma_scalar(depth, edge, qp, offsets, strength, protected)
}

#[doc(hidden)]
pub fn deblock_luma_scalar(
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
    #[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
    let avx2 = (depth.bits() == 8)
        .then(crate::simd::Avx2::detect)
        .flatten();
    #[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
    if let Some(avx2) = avx2 {
        return avx2.deblock_chroma(edge, qp, tc_offset, protected);
    }
    deblock_chroma_scalar(depth, edge, qp, tc_offset, protected)
}

#[doc(hidden)]
pub fn deblock_chroma_scalar(
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

/// Validated band offsets shared by a complete SAO block.
#[derive(Clone, Copy, Debug)]
pub struct SaoBand {
    depth: BitDepth,
    offsets: [i16; 4],
    position: u8,
}

impl SaoBand {
    /// Validates the four offsets and the starting band position.
    pub fn new(depth: BitDepth, offsets: [i16; 4], position: u8) -> Result<Self, Error> {
        sao_sample(depth, 0, offsets, SaoClass::Band(position))?;
        Ok(Self {
            depth,
            offsets,
            position,
        })
    }

    /// Filters an in-place row and returns its changed-sample count.
    /// Every sample is validated before any output is written.
    pub fn apply(self, samples: &mut [u16]) -> Result<u64, Error> {
        if samples.iter().any(|v| *v > self.depth.max_sample()) {
            return Err(Error::Sample);
        }
        let (lanes, changed) = (0, 0);
        #[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
        let (lanes, changed) = if self.depth.bits() == 8 {
            crate::simd::Avx2::detect().map_or((lanes, changed), |avx2| {
                avx2.sao_band(samples, self.offsets, self.position)
            })
        } else {
            (lanes, changed)
        };
        let mut changed = changed;
        for sample in samples.get_mut(lanes..).ok_or(Error::Buffer)? {
            let original = *sample;
            let index =
                ((original >> (self.depth.bits() - 5)) + 32 - u16::from(self.position)) & 31;
            if let Some(&offset) = self.offsets.get(usize::from(index)) {
                *sample = self.depth.clip(i64::from(original) + i64::from(offset));
            }
            changed += u64::from(original != *sample);
        }
        Ok(changed)
    }
}

/// Validated edge offsets shared by a complete SAO block.
#[derive(Clone, Copy, Debug)]
pub struct SaoEdge {
    depth: BitDepth,
    offsets: [i16; 4],
}

impl SaoEdge {
    /// Validates the four offsets.
    pub fn new(depth: BitDepth, offsets: [i16; 4]) -> Result<Self, Error> {
        sao_sample(depth, 0, offsets, SaoClass::Edge([0; 2]))?;
        Ok(Self { depth, offsets })
    }

    /// Filters equal-length source and neighbour rows into a disjoint output.
    /// All lengths and samples are validated before any output is written.
    pub fn apply(
        self,
        source: &[u16],
        neighbours: [&[u16]; 2],
        output: &mut [u16],
    ) -> Result<u64, Error> {
        if source.len() != output.len() || neighbours.iter().any(|row| row.len() != source.len()) {
            return Err(Error::Buffer);
        }
        if source
            .iter()
            .chain(neighbours.into_iter().flatten())
            .any(|v| *v > self.depth.max_sample())
        {
            return Err(Error::Sample);
        }
        let (lanes, changed) = (0, 0);
        #[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
        let (lanes, changed) = if self.depth.bits() == 8 {
            if let Some(avx2) = crate::simd::Avx2::detect() {
                avx2.sao_edge(source, neighbours, output, self.offsets)?
            } else {
                (lanes, changed)
            }
        } else {
            (lanes, changed)
        };
        let mut changed = changed;
        let source = source.get(lanes..).ok_or(Error::Buffer)?;
        let neighbours = [
            neighbours[0].get(lanes..).ok_or(Error::Buffer)?,
            neighbours[1].get(lanes..).ok_or(Error::Buffer)?,
        ];
        let output = output.get_mut(lanes..).ok_or(Error::Buffer)?;
        for (((&sample, &a), &b), destination) in source
            .iter()
            .zip(neighbours[0])
            .zip(neighbours[1])
            .zip(output)
        {
            let sign = (i32::from(sample) - i32::from(a)).signum()
                + (i32::from(sample) - i32::from(b)).signum();
            let offset = match sign {
                -2 => self.offsets[0],
                -1 => self.offsets[1],
                0 => 0,
                1 => self.offsets[2],
                _ => self.offsets[3],
            };
            *destination = self.depth.clip(i64::from(sample) + i64::from(offset));
            changed += u64::from(sample != *destination);
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deblock_matches_scalar_across_parameters_and_protection() -> Result<(), Error> {
        let patterns: [[u16; 8]; 6] = [
            [100, 100, 100, 100, 104, 104, 104, 104],
            [100, 100, 100, 100, 140, 140, 140, 140],
            [255, 255, 255, 254, 255, 255, 255, 255],
            [0, 0, 0, 1, 0, 0, 0, 0],
            [0, 255, 0, 255, 0, 255, 0, 255],
            [12, 10, 14, 14, 21, 19, 18, 20],
        ];
        for bits in [8, 10] {
            let depth = BitDepth::new(bits)?;
            for qp in -25i32..=64 {
                for beta in -7i32..=7 {
                    for tc in -7i32..=7 {
                        let pattern = patterns[(qp + beta + tc).rem_euclid(6) as usize];
                        let edge = core::array::from_fn(|row| {
                            core::array::from_fn(|column| {
                                let value = pattern[column]
                                    .saturating_add(row as u16 * if column >= 4 { 2 } else { 1 })
                                    .min(255);
                                value << (bits - 8)
                            })
                        });
                        let chroma = edge.map(|line| [line[2], line[3], line[4], line[5]]);
                        for protection in 0..4 {
                            let protected = [protection & 1 != 0, protection & 2 != 0];
                            for strength in [
                                BoundaryStrength::Zero,
                                BoundaryStrength::One,
                                BoundaryStrength::Two,
                            ] {
                                assert_eq!(
                                    deblock_luma_with_strength(
                                        depth,
                                        edge,
                                        qp,
                                        [beta, tc],
                                        strength,
                                        protected
                                    ),
                                    deblock_luma_scalar(
                                        depth,
                                        edge,
                                        qp,
                                        [beta, tc],
                                        strength,
                                        protected
                                    ),
                                    "{bits} {qp} {beta} {tc} {strength:?} {protected:?}"
                                );
                            }
                            assert_eq!(
                                deblock_chroma(depth, chroma, qp, tc, protected),
                                deblock_chroma_scalar(depth, chroma, qp, tc, protected),
                                "{bits} {qp} {tc} {protected:?}"
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn deblock_matches_scalar_for_every_sample_word_and_error_order() -> Result<(), Error> {
        for bits in [8, 10] {
            let depth = BitDepth::new(bits)?;
            for sample in 0..=u16::MAX {
                let mut edge = [[0; 8]; 4];
                edge[usize::from(sample) % 4][usize::from(sample / 4) % 8] = sample;
                let mut chroma = [[0; 4]; 4];
                chroma[usize::from(sample) % 4][usize::from(sample / 4) % 4] = sample;
                for qp in [i32::MIN, 26, i32::MAX] {
                    assert_eq!(
                        deblock_luma_with_strength(
                            depth,
                            edge,
                            qp,
                            [0; 2],
                            BoundaryStrength::Two,
                            [false; 2]
                        ),
                        deblock_luma_scalar(
                            depth,
                            edge,
                            qp,
                            [0; 2],
                            BoundaryStrength::Two,
                            [false; 2]
                        )
                    );
                    assert_eq!(
                        deblock_chroma(depth, chroma, qp, 0, [false; 2]),
                        deblock_chroma_scalar(depth, chroma, qp, 0, [false; 2])
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn sao_rows_cover_bands_offsets_tails_and_clipping() -> Result<(), Error> {
        for bits in [8, 10] {
            let depth = BitDepth::new(bits)?;
            let maximum = (1i16 << (bits - 5)) - 1;
            for position in 0..32 {
                for offset in -maximum..=maximum {
                    let offsets = [offset, -offset, maximum, -maximum];
                    let band = SaoBand::new(depth, offsets, position)?;
                    let input: std::vec::Vec<_> = (0..=depth.max_sample()).collect();
                    let mut output = input.clone();
                    let expected: std::vec::Vec<_> = input
                        .iter()
                        .map(|v| sao_sample(depth, *v, offsets, SaoClass::Band(position)))
                        .collect::<Result<_, _>>()?;
                    let changed = band.apply(&mut output)?;
                    assert_eq!(output, expected);
                    assert_eq!(
                        changed,
                        input.iter().zip(&expected).filter(|(a, b)| a != b).count() as u64
                    );
                }
            }
            for len in 0..65 {
                let mut output = [0u16; 67];
                for start in 0..3 {
                    let row = output.get_mut(start..start + len).ok_or(Error::Buffer)?;
                    assert_eq!(SaoBand::new(depth, [1; 4], 0)?.apply(row)?, len as u64);
                    row.fill(0);
                }
            }
            for offset in -maximum..=maximum {
                let offsets = [offset; 4];
                let edge = SaoEdge::new(depth, offsets)?;
                for sample in 0..=depth.max_sample() {
                    let source = [sample; 19];
                    for a in [
                        sample.saturating_sub(1),
                        sample,
                        (sample + 1).min(depth.max_sample()),
                    ] {
                        for b in [
                            sample.saturating_sub(1),
                            sample,
                            (sample + 1).min(depth.max_sample()),
                        ] {
                            let expected =
                                sao_sample(depth, sample, offsets, SaoClass::Edge([a, b]))?;
                            let mut output = [0; 19];
                            assert_eq!(
                                edge.apply(&source, [&[a; 19], &[b; 19]], &mut output)?,
                                u64::from(sample != expected) * 19
                            );
                            assert_eq!(output, [expected; 19]);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn sao_edge_exhausts_eight_bit_sample_triples() -> Result<(), Error> {
        let depth = BitDepth::new(8)?;
        let offsets = [7, -7, 5, -5];
        let edge = SaoEdge::new(depth, offsets)?;
        let b: std::vec::Vec<_> = (0..256).collect();
        for sample in 0..256 {
            for a in 0..256 {
                let mut output = [0; 256];
                edge.apply(&[sample; 256], [&[a; 256], &b], &mut output)?;
                for (&b, &value) in b.iter().zip(&output) {
                    assert_eq!(
                        value,
                        sao_sample(depth, sample, offsets, SaoClass::Edge([a, b]))?
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn sao_row_refusals_never_write_output() -> Result<(), Error> {
        for bits in [8, 10] {
            let depth = BitDepth::new(bits)?;
            assert!(SaoBand::new(depth, [0; 4], 32).is_err());
            let invalid = 1 << (bits - 5);
            assert!(SaoBand::new(depth, [invalid; 4], 0).is_err());
            assert!(SaoEdge::new(depth, [-invalid; 4]).is_err());
            let band = SaoBand::new(depth, [1; 4], 0)?;
            let mut samples = [0; 33];
            *samples.last_mut().ok_or(Error::Buffer)? = depth.max_sample() + 1;
            let original = samples;
            assert_eq!(band.apply(&mut samples), Err(Error::Sample));
            assert_eq!(samples, original);
            let edge = SaoEdge::new(depth, [1; 4])?;
            let mut output = [123; 33];
            for neighbours in [[&samples[..], &[0; 33][..]], [&[0; 33][..], &samples[..]]] {
                assert_eq!(
                    edge.apply(&[0; 33], neighbours, &mut output),
                    Err(Error::Sample)
                );
                assert_eq!(output, [123; 33]);
            }
            assert_eq!(
                edge.apply(&samples, [&[0; 33]; 2], &mut output),
                Err(Error::Sample)
            );
            assert_eq!(
                edge.apply(&[0; 33], [&[0; 32], &[0; 33]], &mut output),
                Err(Error::Buffer)
            );
            assert_eq!(output, [123; 33]);
        }
        Ok(())
    }

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
