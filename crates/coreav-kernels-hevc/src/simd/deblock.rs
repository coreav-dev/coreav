use super::Avx2;
use crate::{BitDepth, BoundaryStrength, Error, filter};
use core::arch::x86_64::*;

#[test]
fn detected_vector_deblocking_matches_scalar() -> Result<(), Error> {
    let Some(avx2) = Avx2::detect() else {
        assert!(!std::is_x86_feature_detected!("avx2"));
        return Ok(());
    };
    let depth = BitDepth::new(8)?;
    let edge = [
        [100, 100, 100, 100, 104, 104, 104, 104],
        [103, 103, 103, 103, 108, 108, 108, 108],
        [98, 98, 98, 98, 107, 107, 107, 107],
        [105, 105, 105, 105, 113, 113, 113, 113],
    ];
    assert_eq!(
        avx2.deblock_luma(edge, 32, [0; 2], BoundaryStrength::Two, [false; 2]),
        filter::deblock_luma_scalar(depth, edge, 32, [0; 2], BoundaryStrength::Two, [false; 2])
    );
    let chroma = edge.map(|line| [line[2], line[3], line[4], line[5]]);
    assert_eq!(
        avx2.deblock_chroma(chroma, 32, 0, [false; 2]),
        filter::deblock_chroma_scalar(depth, chroma, 32, 0, [false; 2])
    );
    Ok(())
}

impl Avx2 {
    pub(crate) fn deblock_luma(
        self,
        edge: [[u16; 8]; 4],
        qp: i32,
        offsets: [i32; 2],
        strength: BoundaryStrength,
        protected: [bool; 2],
    ) -> Result<[[u16; 8]; 4], Error> {
        unsafe { luma(edge, qp, offsets, strength, protected) }
    }

    pub(crate) fn deblock_chroma(
        self,
        edge: [[u16; 4]; 4],
        qp: i32,
        tc_offset: i32,
        protected: [bool; 2],
    ) -> Result<[[u16; 4]; 4], Error> {
        unsafe { chroma(edge, qp, tc_offset, protected) }
    }
}

#[target_feature(enable = "avx2")]
unsafe fn luma(
    edge: [[u16; 8]; 4],
    qp: i32,
    offsets: [i32; 2],
    strength: BoundaryStrength,
    protected: [bool; 2],
) -> Result<[[u16; 8]; 4], Error> {
    let rows = unsafe {
        [
            _mm_loadu_si128(edge[0].as_ptr().cast()),
            _mm_loadu_si128(edge[1].as_ptr().cast()),
            _mm_loadu_si128(edge[2].as_ptr().cast()),
            _mm_loadu_si128(edge[3].as_ptr().cast()),
        ]
    };
    let samples = _mm_or_si128(
        _mm_or_si128(rows[0], rows[1]),
        _mm_or_si128(rows[2], rows[3]),
    );
    if _mm_testz_si128(samples, _mm_set1_epi16(-256)) == 0 {
        return Err(Error::Sample);
    }
    let (beta, tc) = filter::thresholds(BitDepth::new(8)?, qp, offsets, strength)?;
    if strength == BoundaryStrength::Zero || tc == 0 || protected == [true; 2] {
        return Ok(edge);
    }
    let [dp0, dq0] = filter::curvature(edge[0]);
    let [dp3, dq3] = filter::curvature(edge[3]);
    let (dp, dq) = (dp0 + dp3, dq0 + dq3);
    if dp + dq >= beta {
        return Ok(edge);
    }
    let use_strong = filter::strong(edge[0], beta, tc) && filter::strong(edge[3], beta, tc);
    let a = _mm_unpacklo_epi16(rows[0], rows[1]);
    let b = _mm_unpacklo_epi16(rows[2], rows[3]);
    let c = _mm_unpackhi_epi16(rows[0], rows[1]);
    let d = _mm_unpackhi_epi16(rows[2], rows[3]);
    let p32 = _mm_unpacklo_epi32(a, b);
    let p10 = _mm_unpackhi_epi32(a, b);
    let q01 = _mm_unpacklo_epi32(c, d);
    let q23 = _mm_unpackhi_epi32(c, d);
    let far = _mm_unpacklo_epi64(p32, _mm_srli_si128::<8>(q23));
    let outer = _mm_unpacklo_epi64(_mm_srli_si128::<8>(p32), q23);
    let inner = _mm_unpacklo_epi64(p10, _mm_srli_si128::<8>(q01));
    let center = _mm_unpacklo_epi64(_mm_srli_si128::<8>(p10), q01);
    let across = _mm_shuffle_epi32::<0x4e>(center);
    let across_inner = _mm_shuffle_epi32::<0x4e>(inner);
    let zero = _mm_setzero_si128();
    let (outer_value, inner_value, center_value) = if use_strong {
        let radius = _mm_set1_epi16((2 * tc) as i16);
        let mut sum = _mm_add_epi16(
            _mm_slli_epi16::<1>(far),
            _mm_add_epi16(_mm_slli_epi16::<1>(outer), outer),
        );
        sum = _mm_add_epi16(sum, inner);
        sum = _mm_add_epi16(sum, center);
        sum = _mm_add_epi16(sum, across);
        let outer_value = _mm_srli_epi16::<3>(_mm_add_epi16(sum, _mm_set1_epi16(4)));
        let outer_value = _mm_min_epi16(
            _mm_max_epi16(outer_value, _mm_sub_epi16(outer, radius)),
            _mm_add_epi16(outer, radius),
        );
        let sum = _mm_add_epi16(_mm_add_epi16(outer, inner), _mm_add_epi16(center, across));
        let inner_value = _mm_srli_epi16::<2>(_mm_add_epi16(sum, _mm_set1_epi16(2)));
        let inner_value = _mm_min_epi16(
            _mm_max_epi16(inner_value, _mm_sub_epi16(inner, radius)),
            _mm_add_epi16(inner, radius),
        );
        let mut sum = _mm_add_epi16(outer, _mm_slli_epi16::<1>(inner));
        sum = _mm_add_epi16(sum, _mm_slli_epi16::<1>(center));
        sum = _mm_add_epi16(sum, _mm_slli_epi16::<1>(across));
        sum = _mm_add_epi16(sum, across_inner);
        let center_value = _mm_srli_epi16::<3>(_mm_add_epi16(sum, _mm_set1_epi16(4)));
        let center_value = _mm_min_epi16(
            _mm_max_epi16(center_value, _mm_sub_epi16(center, radius)),
            _mm_add_epi16(center, radius),
        );
        (outer_value, inner_value, center_value)
    } else {
        let primary = _mm_sub_epi16(across, center);
        let neighbour = _mm_sub_epi16(across_inner, inner);
        let numerator = _mm_sub_epi16(
            _mm_add_epi16(_mm_slli_epi16::<3>(primary), primary),
            _mm_add_epi16(_mm_slli_epi16::<1>(neighbour), neighbour),
        );
        let delta = _mm_srai_epi16::<4>(_mm_add_epi16(numerator, _mm_set1_epi16(8)));
        let eligible = _mm_cmpgt_epi16(_mm_set1_epi16((10 * tc) as i16), _mm_abs_epi16(delta));
        let eligible = _mm_unpacklo_epi64(eligible, eligible);
        let clipped = _mm_min_epi16(
            _mm_max_epi16(delta, _mm_set1_epi16(-tc as i16)),
            _mm_set1_epi16(tc as i16),
        );
        let delta = _mm_unpacklo_epi64(clipped, _mm_sub_epi16(zero, clipped));
        let delta = _mm_and_si128(delta, eligible);
        let secondary = (beta + (beta >> 1)) >> 3;
        let sides = _mm_set_epi64x(
            if dq < secondary { -1 } else { 0 },
            if dp < secondary { -1 } else { 0 },
        );
        let average = _mm_srli_epi16::<1>(_mm_add_epi16(
            _mm_add_epi16(outer, center),
            _mm_set1_epi16(1),
        ));
        let correction = _mm_srai_epi16::<1>(_mm_add_epi16(_mm_sub_epi16(average, inner), delta));
        let half = (tc >> 1) as i16;
        let correction = _mm_min_epi16(
            _mm_max_epi16(correction, _mm_set1_epi16(-half)),
            _mm_set1_epi16(half),
        );
        let correction = _mm_and_si128(correction, _mm_and_si128(sides, eligible));
        (
            outer,
            _mm_add_epi16(inner, correction),
            _mm_add_epi16(center, delta),
        )
    };
    let protect = _mm_set_epi64x(
        if protected[1] { -1 } else { 0 },
        if protected[0] { -1 } else { 0 },
    );
    let maximum = _mm_set1_epi16(255);
    let outer_value = _mm_min_epi16(_mm_max_epi16(outer_value, zero), maximum);
    let inner_value = _mm_min_epi16(_mm_max_epi16(inner_value, zero), maximum);
    let center_value = _mm_min_epi16(_mm_max_epi16(center_value, zero), maximum);
    let values = [
        _mm_blendv_epi8(outer_value, outer, protect),
        _mm_blendv_epi8(inner_value, inner, protect),
        _mm_blendv_epi8(center_value, center, protect),
    ];
    let p32 = _mm_unpacklo_epi64(far, values[0]);
    let p10 = _mm_unpacklo_epi64(values[1], values[2]);
    let q01 = _mm_unpackhi_epi64(values[2], values[1]);
    let q23 = _mm_unpackhi_epi64(values[0], far);
    let a = _mm_unpacklo_epi16(p32, p10);
    let b = _mm_unpackhi_epi16(p32, p10);
    let c = _mm_unpacklo_epi16(q01, q23);
    let d = _mm_unpackhi_epi16(q01, q23);
    let p01 = _mm_unpacklo_epi16(a, b);
    let p23 = _mm_unpackhi_epi16(a, b);
    let q01 = _mm_unpacklo_epi16(c, d);
    let q23 = _mm_unpackhi_epi16(c, d);
    let mut output = edge;
    unsafe {
        _mm_storeu_si128(output[0].as_mut_ptr().cast(), _mm_unpacklo_epi64(p01, q01));
        _mm_storeu_si128(output[1].as_mut_ptr().cast(), _mm_unpackhi_epi64(p01, q01));
        _mm_storeu_si128(output[2].as_mut_ptr().cast(), _mm_unpacklo_epi64(p23, q23));
        _mm_storeu_si128(output[3].as_mut_ptr().cast(), _mm_unpackhi_epi64(p23, q23));
    }
    Ok(output)
}

#[target_feature(enable = "avx2")]
unsafe fn chroma(
    edge: [[u16; 4]; 4],
    qp: i32,
    tc_offset: i32,
    protected: [bool; 2],
) -> Result<[[u16; 4]; 4], Error> {
    if !(-24..=63).contains(&qp) || !(-6..=6).contains(&tc_offset) {
        return Err(Error::Qp);
    }
    let rows = unsafe {
        [
            _mm_loadl_epi64(edge[0].as_ptr().cast()),
            _mm_loadl_epi64(edge[1].as_ptr().cast()),
            _mm_loadl_epi64(edge[2].as_ptr().cast()),
            _mm_loadl_epi64(edge[3].as_ptr().cast()),
        ]
    };
    let samples = _mm_or_si128(
        _mm_or_si128(rows[0], rows[1]),
        _mm_or_si128(rows[2], rows[3]),
    );
    if _mm_testz_si128(samples, _mm_set1_epi16(-256)) == 0 {
        return Err(Error::Sample);
    }
    let qp = crate::chroma_qp_index(qp)?;
    let tc = filter::tc(qp + 2 + 2 * tc_offset) as i16;
    if tc == 0 || protected == [true; 2] {
        return Ok(edge);
    }
    let a = _mm_unpacklo_epi16(rows[0], rows[1]);
    let b = _mm_unpacklo_epi16(rows[2], rows[3]);
    let p10 = _mm_unpacklo_epi32(a, b);
    let q01 = _mm_unpackhi_epi32(a, b);
    let far = _mm_unpacklo_epi64(p10, _mm_srli_si128::<8>(q01));
    let center = _mm_unpacklo_epi64(_mm_srli_si128::<8>(p10), q01);
    let across = _mm_shuffle_epi32::<0x4e>(center);
    let across_far = _mm_shuffle_epi32::<0x4e>(far);
    let numerator = _mm_add_epi16(
        _mm_slli_epi16::<2>(_mm_sub_epi16(across, center)),
        _mm_sub_epi16(far, across_far),
    );
    let delta = _mm_srai_epi16::<3>(_mm_add_epi16(numerator, _mm_set1_epi16(4)));
    let delta = _mm_min_epi16(
        _mm_max_epi16(delta, _mm_set1_epi16(-tc)),
        _mm_set1_epi16(tc),
    );
    let delta = _mm_unpacklo_epi64(delta, _mm_sub_epi16(_mm_setzero_si128(), delta));
    let protect = _mm_set_epi64x(
        if protected[1] { -1 } else { 0 },
        if protected[0] { -1 } else { 0 },
    );
    let value = _mm_add_epi16(center, _mm_andnot_si128(protect, delta));
    let value = _mm_min_epi16(
        _mm_max_epi16(value, _mm_setzero_si128()),
        _mm_set1_epi16(255),
    );
    let p10 = _mm_unpacklo_epi64(far, value);
    let q01 = _mm_unpackhi_epi64(value, far);
    let a = _mm_unpacklo_epi16(p10, q01);
    let b = _mm_unpackhi_epi16(p10, q01);
    let first = _mm_unpacklo_epi16(a, b);
    let last = _mm_unpackhi_epi16(a, b);
    let mut output = edge;
    unsafe {
        _mm_storel_epi64(output[0].as_mut_ptr().cast(), first);
        _mm_storel_epi64(output[1].as_mut_ptr().cast(), _mm_srli_si128::<8>(first));
        _mm_storel_epi64(output[2].as_mut_ptr().cast(), last);
        _mm_storel_epi64(output[3].as_mut_ptr().cast(), _mm_srli_si128::<8>(last));
    }
    Ok(output)
}
