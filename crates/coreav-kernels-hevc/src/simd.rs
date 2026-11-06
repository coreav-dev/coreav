use crate::{Error, inter};
use core::arch::x86_64::*;
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub(super) struct Avx2(());

impl Avx2 {
    pub(super) fn detect() -> Option<Self> {
        static CAPABILITY: OnceLock<Option<Avx2>> = OnceLock::new();
        *CAPABILITY.get_or_init(|| std::is_x86_feature_detected!("avx2").then_some(Self(())))
    }

    pub(super) fn horizontal(
        self,
        source: &[u16],
        filter: &[i32],
        shift: u8,
        output: &mut [i32],
    ) -> Result<(), Error> {
        if !matches!(filter.len(), 4 | 8) || shift > 2 {
            return Err(Error::Buffer);
        }
        let extent = output
            .len()
            .checked_add(filter.len() - 1)
            .ok_or(Error::Buffer)?;
        if source.len() < extent {
            return Err(Error::Buffer);
        }
        let lanes = output.len() / 8 * 8;
        unsafe { horizontal_avx2(source, filter, shift, output, lanes) };
        inter::horizontal_row(
            source.get(lanes..).ok_or(Error::Buffer)?,
            filter,
            shift,
            output.get_mut(lanes..).ok_or(Error::Buffer)?,
        )
    }

    pub(super) fn vertical(
        self,
        rows: &[&[i32]],
        filter: &[i32],
        output: &mut [i32],
    ) -> Result<(), Error> {
        if !matches!(filter.len(), 4 | 8)
            || rows.len() < filter.len()
            || rows
                .iter()
                .take(filter.len())
                .any(|row| row.len() < output.len())
        {
            return Err(Error::Buffer);
        }
        let lanes = output.len() / 8 * 8;
        unsafe { vertical_avx2(rows, filter, output, lanes) };
        let mut tails: [&[i32]; 8] = [&[]; 8];
        for (tail, row) in tails.iter_mut().zip(rows).take(filter.len()) {
            *tail = row.get(lanes..).ok_or(Error::Buffer)?;
        }
        inter::vertical_row(
            &tails,
            filter,
            output.get_mut(lanes..).ok_or(Error::Buffer)?,
        )
    }
}

#[target_feature(enable = "avx2")]
unsafe fn horizontal_avx2(
    source: &[u16],
    filter: &[i32],
    shift: u8,
    output: &mut [i32],
    lanes: usize,
) {
    let count = _mm_cvtsi32_si128(i32::from(shift));
    for x in (0..lanes).step_by(8) {
        let mut sum = _mm256_setzero_si256();
        for (tap, &coefficient) in filter.iter().enumerate() {
            let input = unsafe { _mm_loadu_si128(source.as_ptr().add(x + tap).cast()) };
            let samples = _mm256_cvtepu16_epi32(input);
            sum = _mm256_add_epi32(
                sum,
                _mm256_mullo_epi32(samples, _mm256_set1_epi32(coefficient)),
            );
        }
        unsafe {
            _mm256_storeu_si256(
                output.as_mut_ptr().add(x).cast(),
                _mm256_sra_epi32(sum, count),
            )
        };
    }
}

#[target_feature(enable = "avx2")]
unsafe fn vertical_avx2(rows: &[&[i32]], filter: &[i32], output: &mut [i32], lanes: usize) {
    for x in (0..lanes).step_by(8) {
        let mut sum = _mm256_setzero_si256();
        for (row, &coefficient) in rows.iter().zip(filter) {
            let samples = unsafe { _mm256_loadu_si256(row.as_ptr().add(x).cast()) };
            sum = _mm256_add_epi32(
                sum,
                _mm256_mullo_epi32(samples, _mm256_set1_epi32(coefficient)),
            );
        }
        unsafe {
            _mm256_storeu_si256(
                output.as_mut_ptr().add(x).cast(),
                _mm256_srai_epi32::<6>(sum),
            )
        };
    }
}
