use crate::{Error, Result, bits::Bits};

const INTRA: [u8; 64] = [
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 16, 17, 16, 17, 18, 17, 18, 18, 17, 18, 21, 19, 20,
    21, 20, 19, 21, 24, 22, 22, 24, 24, 22, 22, 24, 25, 25, 27, 30, 27, 25, 25, 29, 31, 35, 35, 31,
    29, 36, 41, 44, 41, 36, 47, 54, 54, 47, 65, 70, 65, 88, 88, 115,
];
const INTER: [u8; 64] = [
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 17, 17, 17, 17, 18, 18, 18, 18, 18, 18, 20, 20, 20,
    20, 20, 20, 20, 24, 24, 24, 24, 24, 24, 24, 24, 25, 25, 25, 25, 25, 25, 25, 28, 28, 28, 28, 28,
    28, 33, 33, 33, 33, 33, 41, 41, 41, 41, 54, 54, 54, 71, 71, 91,
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScalingLists {
    values: [[[u8; 64]; 6]; 4],
    dc: [[u8; 6]; 2],
}

impl ScalingLists {
    pub(crate) fn flat() -> Self {
        Self {
            values: [[[16; 64]; 6]; 4],
            dc: [[16; 6]; 2],
        }
    }

    pub(crate) fn normative() -> Self {
        let mut result = Self::flat();
        for size in result.values.iter_mut().skip(1) {
            for (matrix, values) in size.iter_mut().enumerate() {
                *values = if matrix < 3 { INTRA } else { INTER };
            }
        }
        result
    }

    pub(crate) fn parse(bits: &mut Bits<'_>) -> Result<Self> {
        let mut result = Self::normative();
        for size in 0..4 {
            let step = if size == 3 { 3 } else { 1 };
            for matrix in (0..6).step_by(step) {
                if !bits.flag()? {
                    let delta = bits.ue((matrix / step) as u32)? as usize;
                    if delta != 0 {
                        let reference = matrix - delta * step;
                        let row = result
                            .values
                            .get_mut(size)
                            .ok_or(Error::InvalidData("scaling_list_size"))?;
                        let values = *row
                            .get(reference)
                            .ok_or(Error::InvalidData("scaling_list_matrix"))?;
                        *row.get_mut(matrix)
                            .ok_or(Error::InvalidData("scaling_list_matrix"))? = values;
                        if size > 1 {
                            let row = result
                                .dc
                                .get_mut(size - 2)
                                .ok_or(Error::InvalidData("scaling_list_size"))?;
                            let dc = *row
                                .get(reference)
                                .ok_or(Error::InvalidData("scaling_list_matrix"))?;
                            *row.get_mut(matrix)
                                .ok_or(Error::InvalidData("scaling_list_matrix"))? = dc;
                        }
                    }
                    continue;
                }
                let mut next = 8;
                if size > 1 {
                    next = bits.se(-7, 247)? + 8;
                    *result
                        .dc
                        .get_mut(size - 2)
                        .and_then(|row| row.get_mut(matrix))
                        .ok_or(Error::InvalidData("scaling_list_matrix"))? = next as u8;
                }
                let row = result
                    .values
                    .get_mut(size)
                    .and_then(|row| row.get_mut(matrix))
                    .ok_or(Error::InvalidData("scaling_list_matrix"))?;
                for coefficient in row.iter_mut().take(if size == 0 { 16 } else { 64 }) {
                    next = (next + bits.se(-128, 127)? + 256) % 256;
                    if next == 0 {
                        return Err(Error::InvalidData("scaling_list_zero_coefficient"));
                    }
                    *coefficient = next as u8;
                }
            }
        }
        Ok(result)
    }

    pub(crate) fn factors(
        &self,
        log2_size: u32,
        component: usize,
        output: &mut [u8],
    ) -> Result<()> {
        if !(2..=5).contains(&log2_size) || component > 2 || log2_size == 5 && component != 0 {
            return Err(Error::InvalidData("scaling_factor_block"));
        }
        let n = 1usize << log2_size;
        let output = output
            .get_mut(..n * n)
            .ok_or(Error::InvalidData("scaling_factor_output"))?;
        let size = log2_size as usize - 2;
        let source_n = n.min(8);
        let scale = n / source_n;
        let values = self
            .values
            .get(size)
            .and_then(|row| row.get(component))
            .ok_or(Error::InvalidData("scaling_list_matrix"))?;
        let mut index = 0;
        for diagonal in 0..2 * source_n - 1 {
            for x in 0..=diagonal.min(source_n - 1) {
                let y = diagonal - x;
                if y >= source_n {
                    continue;
                }
                let value = *values
                    .get(index)
                    .ok_or(Error::InvalidData("scaling_list_scan"))?;
                index += 1;
                for j in 0..scale {
                    for k in 0..scale {
                        *output
                            .get_mut((y * scale + j) * n + x * scale + k)
                            .ok_or(Error::InvalidData("scaling_factor_output"))? = value;
                    }
                }
            }
        }
        if size > 1 {
            let dc = *self
                .dc
                .get(size - 2)
                .and_then(|row| row.get(component))
                .ok_or(Error::InvalidData("scaling_list_matrix"))?;
            *output
                .first_mut()
                .ok_or(Error::InvalidData("scaling_factor_output"))? = dc;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::tests::Writer;

    #[test]
    fn disabled_lists_are_flat_at_every_transform_size() -> Result<()> {
        let lists = ScalingLists::flat();
        let mut factors = [0; 1024];
        for log2 in 2..=5 {
            lists.factors(log2, 0, &mut factors)?;
            assert!(
                factors
                    .get(..1 << (2 * log2))
                    .is_some_and(|f| f.iter().all(|v| *v == 16))
            );
        }
        Ok(())
    }

    #[test]
    fn default_diagonal_scan_and_large_matrix_dc_follow_table_7_6() -> Result<()> {
        let lists = ScalingLists::normative();
        let mut factors = [0; 1024];
        lists.factors(3, 0, &mut factors)?;
        assert_eq!(factors.get(7), Some(&24));
        assert_eq!(factors.get(56), Some(&24));
        assert_eq!(factors.get(63), Some(&115));
        lists.factors(4, 0, &mut factors)?;
        assert_eq!(factors.first(), Some(&16));
        assert_eq!(factors.get(255), Some(&115));
        lists.factors(5, 0, &mut factors)?;
        assert_eq!(factors.get(1023), Some(&115));
        assert!(lists.factors(5, 1, &mut factors).is_err());
        let short = factors
            .get_mut(..63)
            .ok_or(Error::InvalidData("test_buffer"))?;
        assert!(lists.factors(3, 0, short).is_err());
        Ok(())
    }

    #[test]
    fn predicted_lists_copy_coefficients_and_separate_dc() -> Result<()> {
        let mut writer = Writer::default();
        for size in 0..4 {
            for matrix in (0..6).step_by(if size == 3 { 3 } else { 1 }) {
                let explicit = size == 2 && matrix == 0;
                writer.flag(explicit);
                if explicit {
                    writer.se(5);
                    for _ in 0..64 {
                        writer.se(1);
                    }
                } else {
                    writer.ue(u32::from(
                        size == 2 && matrix == 1 || size == 3 && matrix == 3,
                    ));
                }
            }
        }
        let bytes = writer.bytes();
        let lists = ScalingLists::parse(&mut Bits::new(&bytes))?;
        let mut factors = [0; 256];
        lists.factors(4, 1, &mut factors)?;
        assert_eq!(factors.first(), Some(&13));
        assert_eq!(factors.get(1), Some(&14));
        assert_eq!(factors.get(255), Some(&77));
        assert_eq!(lists.values.get(3).and_then(|row| row.get(3)), Some(&INTRA));
        Ok(())
    }

    #[test]
    fn explicit_zero_is_not_a_valid_scaling_factor() {
        let mut writer = Writer::default();
        writer.flag(true);
        writer.se(-8);
        assert!(matches!(
            ScalingLists::parse(&mut Bits::new(&writer.bytes())),
            Err(Error::InvalidData("scaling_list_zero_coefficient"))
        ));
    }
}
