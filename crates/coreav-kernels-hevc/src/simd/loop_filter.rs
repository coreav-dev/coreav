use super::Avx2;
use crate::Error;
use core::arch::x86_64::*;

#[test]
fn runtime_detection_reaches_the_vector_filters() {
    if std::is_x86_feature_detected!("avx2") {
        assert!(Avx2::detect().is_some());
    }
}

impl Avx2 {
    pub(crate) fn sao_band(
        self,
        samples: &mut [u16],
        offsets: [i16; 4],
        position: u8,
    ) -> (usize, u64) {
        let lanes = samples.len() / 16 * 16;
        let changed = unsafe { band(samples, offsets, position, lanes) };
        (lanes, changed)
    }

    pub(crate) fn sao_edge(
        self,
        source: &[u16],
        neighbours: [&[u16]; 2],
        output: &mut [u16],
        offsets: [i16; 4],
    ) -> Result<(usize, u64), Error> {
        if source.len() != output.len() || neighbours.iter().any(|row| row.len() != source.len()) {
            return Err(Error::Buffer);
        }
        let lanes = source.len() / 16 * 16;
        let changed = unsafe { edge(source, neighbours, output, offsets, lanes) };
        Ok((lanes, changed))
    }
}

#[target_feature(enable = "avx2")]
unsafe fn band(samples: &mut [u16], offsets: [i16; 4], position: u8, lanes: usize) -> u64 {
    let mut changed = 0;
    for x in (0..lanes).step_by(16) {
        let original = unsafe { _mm256_loadu_si256(samples.as_ptr().add(x).cast()) };
        let index = _mm256_and_si256(
            _mm256_sub_epi16(
                _mm256_srli_epi16::<3>(original),
                _mm256_set1_epi16(i16::from(position)),
            ),
            _mm256_set1_epi16(31),
        );
        let mut offset = _mm256_setzero_si256();
        for (i, value) in offsets.into_iter().enumerate() {
            offset = _mm256_or_si256(
                offset,
                _mm256_and_si256(
                    _mm256_cmpeq_epi16(index, _mm256_set1_epi16(i as i16)),
                    _mm256_set1_epi16(value),
                ),
            );
        }
        let value = _mm256_min_epi16(
            _mm256_max_epi16(_mm256_add_epi16(original, offset), _mm256_setzero_si256()),
            _mm256_set1_epi16(255),
        );
        changed += u64::from(
            (32 - (_mm256_movemask_epi8(_mm256_cmpeq_epi16(original, value)) as u32).count_ones())
                / 2,
        );
        unsafe { _mm256_storeu_si256(samples.as_mut_ptr().add(x).cast(), value) };
    }
    changed
}

#[target_feature(enable = "avx2")]
unsafe fn edge(
    source: &[u16],
    neighbours: [&[u16]; 2],
    output: &mut [u16],
    offsets: [i16; 4],
    lanes: usize,
) -> u64 {
    let mut changed = 0;
    for x in (0..lanes).step_by(16) {
        let original = unsafe { _mm256_loadu_si256(source.as_ptr().add(x).cast()) };
        let mut sign = _mm256_setzero_si256();
        for row in neighbours {
            let neighbour = unsafe { _mm256_loadu_si256(row.as_ptr().add(x).cast()) };
            sign = _mm256_add_epi16(
                sign,
                _mm256_sub_epi16(
                    _mm256_cmpgt_epi16(neighbour, original),
                    _mm256_cmpgt_epi16(original, neighbour),
                ),
            );
        }
        let mut offset = _mm256_setzero_si256();
        for (class, value) in [-2, -1, 1, 2].into_iter().zip(offsets) {
            offset = _mm256_or_si256(
                offset,
                _mm256_and_si256(
                    _mm256_cmpeq_epi16(sign, _mm256_set1_epi16(class)),
                    _mm256_set1_epi16(value),
                ),
            );
        }
        let value = _mm256_min_epi16(
            _mm256_max_epi16(_mm256_add_epi16(original, offset), _mm256_setzero_si256()),
            _mm256_set1_epi16(255),
        );
        changed += u64::from(
            (32 - (_mm256_movemask_epi8(_mm256_cmpeq_epi16(original, value)) as u32).count_ones())
                / 2,
        );
        unsafe { _mm256_storeu_si256(output.as_mut_ptr().add(x).cast(), value) };
    }
    changed
}
