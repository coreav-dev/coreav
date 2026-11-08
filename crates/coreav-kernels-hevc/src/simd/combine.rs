use super::Avx2;
use crate::{Error, Rect};
use core::arch::x86_64::*;

impl Avx2 {
    pub(crate) fn validate_combine(
        self,
        first: Rect<'_, i32>,
        second: Option<Rect<'_, i32>>,
        minimum: i32,
        maximum: i32,
    ) -> Result<(), Error> {
        unsafe { validate_predictors(first, second, minimum, maximum) }
    }

    #[cfg(test)]
    pub(crate) fn combine_samples_in_range(
        self,
        samples: &[i32],
        minimum: i32,
        maximum: i32,
    ) -> bool {
        unsafe { samples_in_range(samples, minimum, maximum) }
    }
}

#[target_feature(enable = "avx2")]
unsafe fn validate_predictors(
    first: Rect<'_, i32>,
    second: Option<Rect<'_, i32>>,
    minimum: i32,
    maximum: i32,
) -> Result<(), Error> {
    for input in [Some(first), second].into_iter().flatten() {
        for row in input.data.chunks(input.stride).take(input.height) {
            let samples = row.get(..input.width).ok_or(Error::Buffer)?;
            if !unsafe { samples_in_range(samples, minimum, maximum) } {
                return Err(Error::Sample);
            }
        }
    }
    Ok(())
}

#[inline]
#[target_feature(enable = "avx2")]
unsafe fn samples_in_range(samples: &[i32], minimum: i32, maximum: i32) -> bool {
    let minimum_vector = _mm256_set1_epi32(minimum);
    let maximum_vector = _mm256_set1_epi32(maximum);
    let mut invalid = _mm256_setzero_si256();
    let mut groups = samples.chunks_exact(8);
    for group in groups.by_ref() {
        let values = unsafe { _mm256_loadu_si256(group.as_ptr().cast()) };
        invalid = _mm256_or_si256(
            invalid,
            _mm256_or_si256(
                _mm256_cmpgt_epi32(minimum_vector, values),
                _mm256_cmpgt_epi32(values, maximum_vector),
            ),
        );
    }
    _mm256_movemask_epi8(invalid) == 0
        && groups
            .remainder()
            .iter()
            .all(|value| (minimum..=maximum).contains(value))
}

#[test]
fn range_validation_checks_every_lane_without_reading_padding() {
    let Some(vector) = Avx2::detect() else {
        assert!(!std::is_x86_feature_detected!("avx2"));
        return;
    };
    for (minimum, maximum) in [(-16830, 33150), (-16880, 33247)] {
        for width in 0..=64 {
            for offset in 0..8 {
                let mut storage = std::vec![i32::MAX; offset + width];
                let row = &mut storage[offset..];
                for (index, value) in row.iter_mut().enumerate() {
                    *value = if index % 2 == 0 { minimum } else { maximum };
                }
                assert!(vector.combine_samples_in_range(row, minimum, maximum));
                for lane in 0..width {
                    let saved = row[lane];
                    for value in [minimum - 1, maximum + 1, i32::MIN, i32::MAX] {
                        row[lane] = value;
                        assert!(!vector.combine_samples_in_range(row, minimum, maximum));
                    }
                    row[lane] = saved;
                }
            }
        }
    }
}
