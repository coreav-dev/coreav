use crate::*;
use std::{vec, vec::Vec};

#[test]
fn combination_rows_match_literal_full_domain_and_exact_extents() {
    let mut seed = 0x943a_1807_u32;
    let deltas = [-128, -127, -1, 0, 1, 126, 127];
    let offsets = [-128, -1, 0, 1, 127];
    for (bits, minimum, maximum) in [(8, -16830, 33150), (10, -16880, 33247)] {
        let depth = BitDepth::new(bits).unwrap();
        for width in 1..=64 {
            let height = 3;
            let first_stride = width + 3;
            let second_stride = width + 5;
            let output_stride = width + 1;
            let mut first = vec![i32::MIN; 1 + first_stride * (height - 1) + width];
            let mut second = vec![i32::MAX; 1 + second_stride * (height - 1) + width];
            for y in 0..height {
                for x in 0..width {
                    for (data, stride) in [(&mut first, first_stride), (&mut second, second_stride)]
                    {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        data[1 + y * stride + x] = match x % 4 {
                            0 => minimum,
                            1 => maximum,
                            2 => 0,
                            _ => minimum + (seed % (maximum - minimum + 1) as u32) as i32,
                        };
                    }
                }
            }
            let first = Rect::new(&first[1..], first_stride, width, height).unwrap();
            let second = Rect::new(&second[1..], second_stride, width, height).unwrap();
            for denom in 0..=7 {
                for case in 0..49 {
                    let weight = PredictionWeight::new(
                        denom,
                        (1 << denom) + deltas[case % 7],
                        offsets[case % 5],
                    )
                    .unwrap();
                    let other_weight = PredictionWeight::new(
                        denom,
                        (1 << denom) + deltas[case / 7],
                        offsets[(case / 5) % 5],
                    )
                    .unwrap();
                    for bi in [false, true] {
                        let mut actual = vec![999; 1 + output_stride * (height - 1) + width];
                        let mut expected = actual.clone();
                        let other = bi.then_some((other_weight, second));
                        let result = if bi {
                            weight_bi(
                                depth,
                                weight,
                                other_weight,
                                first,
                                second,
                                RectMut::new(&mut actual[1..], output_stride, width, height)
                                    .unwrap(),
                            )
                        } else {
                            weight_uni(
                                depth,
                                weight,
                                first,
                                RectMut::new(&mut actual[1..], output_stride, width, height)
                                    .unwrap(),
                            )
                        };
                        let reference = inter::combine_reference(
                            depth,
                            weight,
                            first,
                            other,
                            RectMut::new(&mut expected[1..], output_stride, width, height).unwrap(),
                        );
                        assert_eq!(result, reference, "{bits}/{width}/{denom}/{case}/{bi}");
                        assert_eq!(actual, expected, "{bits}/{width}/{denom}/{case}/{bi}");
                    }
                }
            }
        }
    }
}

#[test]
fn automatic_and_scalar_match_all_widths_phases_and_tails() {
    let mut seed = 0x18a5_92c3_u32;
    for bits in [8, 10] {
        for component in [Component::Luma, Component::Chroma] {
            let phases = if component == Component::Luma { 4 } else { 8 };
            let limit = if component == Component::Luma { 64 } else { 32 };
            for width in 1..=limit {
                let height = 9;
                let mut source = vec![u16::MAX; 83 * 19 + 79];
                for y in 0..20 {
                    for x in 0..79 {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        source[y * 83 + x] = (seed & ((1 << bits) - 1)) as u16;
                    }
                }
                for py in 0..phases {
                    for px in 0..phases {
                        let motion = if width % 2 == 0 { -phases * 4 } else { 0 };
                        let request =
                            config(bits, component, 1, 1, motion + px, motion + py).unwrap();
                        let stride = width + 3;
                        let length = stride * (height - 1) + width;
                        let mut automatic = vec![i32::MAX; length];
                        let mut scalar = automatic.clone();
                        let reference = RefPlane::new(&source, 83, 79, 20).unwrap();
                        interpolate_inter(
                            request,
                            reference,
                            RectMut::new(&mut automatic, stride, width, height).unwrap(),
                        )
                        .unwrap();
                        inter::interpolate_inter_scalar(
                            request,
                            reference,
                            RectMut::new(&mut scalar, stride, width, height).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(automatic, scalar, "{bits}/{component:?}/{width}/{px}/{py}");
                        for y in 0..height {
                            for x in 0..width {
                                assert_eq!(
                                    automatic[y * stride + x],
                                    equation_sample(request, (&source, 83, 79, 20), (x, y))
                                        .unwrap()
                                );
                            }
                            if y + 1 < height {
                                assert!(
                                    automatic[y * stride + width..(y + 1) * stride]
                                        .iter()
                                        .all(|&v| v == i32::MAX)
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn inter_scratch_fits_existing_reconstruction_envelope() {
    assert!(core::mem::size_of::<inter::InterScratch>() <= SCRATCH_BYTES);
    assert_eq!(SCRATCH_BYTES, 9_224);
}

#[test]
fn interpolation_support_at_exact_row_extents_matches_equations_before_writes() {
    for bits in [8, 10] {
        for component in [Component::Luma, Component::Chroma] {
            let (taps, first, limit) = if component == Component::Luma {
                (8, 3, 64)
            } else {
                (4, 1, 32)
            };
            for width in [1, 7, 8, 9, 31, 32, 63, 64] {
                if width > limit {
                    continue;
                }
                let height = 2;
                let reference_width = width + taps - 1;
                let reference_height = height + taps - 1;
                let source_stride = reference_width + 3;
                let source_length = source_stride * (reference_height - 1) + reference_width;
                let mut source = vec![u16::MAX; 1 + source_length];
                for y in 0..reference_height {
                    for x in 0..reference_width {
                        source[1 + y * source_stride + x] =
                            ((47 * x + 91 * y + 13 * x * y) % (1 << bits)) as u16;
                    }
                }
                let stride = width + 3;
                let length = stride * (height - 1) + width;
                for origin in [first - 1, first, first + 1] {
                    let request = config(bits, component, origin, first, 1, 1).unwrap();
                    let mut output = vec![i32::MAX; 1 + length];
                    let mut expected = output.clone();
                    for y in 0..height {
                        for x in 0..width {
                            expected[1 + y * stride + x] = equation_sample(
                                request,
                                (
                                    &source[1..],
                                    source_stride,
                                    reference_width,
                                    reference_height,
                                ),
                                (x, y),
                            )
                            .unwrap();
                        }
                    }
                    interpolate_inter(
                        request,
                        RefPlane::new(
                            &source[1..],
                            source_stride,
                            reference_width,
                            reference_height,
                        )
                        .unwrap(),
                        RectMut::new(&mut output[1..], stride, width, height).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(output, expected, "{bits}/{component:?}/{width}/{origin}");
                }
                source[source_length] = 1 << bits;
                let mut output = vec![i32::MAX; 1 + length];
                assert_eq!(
                    interpolate_inter(
                        config(bits, component, first, first, 1, 1).unwrap(),
                        RefPlane::new(
                            &source[1..],
                            source_stride,
                            reference_width,
                            reference_height,
                        )
                        .unwrap(),
                        RectMut::new(&mut output[1..], stride, width, height).unwrap(),
                    ),
                    Err(Error::Sample)
                );
                assert_eq!(output, vec![i32::MAX; 1 + length]);
            }
        }
    }
}

#[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
#[test]
fn avx2_capability_matches_runtime_detection() {
    assert_eq!(
        crate::simd::Avx2::detect().is_some(),
        std::is_x86_feature_detected!("avx2")
    );
}

fn equation_luma(a: [i64; 8], phase: i16) -> Result<i64, Error> {
    let [m3, m2, m1, z, p1, p2, p3, p4] = a;
    Ok(match phase {
        0 => 64 * z,
        1 => -m3 + 4 * m2 - 10 * m1 + 58 * z + 17 * p1 - 5 * p2 + p3,
        2 => -m3 + 4 * m2 - 11 * m1 + 40 * z + 40 * p1 - 11 * p2 + 4 * p3 - p4,
        3 => m2 - 5 * m1 + 17 * z + 58 * p1 - 10 * p2 + 4 * p3 - p4,
        _ => return Err(Error::Buffer),
    })
}

fn equation_chroma(a: [i64; 4], phase: i16) -> Result<i64, Error> {
    let [m1, z, p1, p2] = a;
    Ok(match phase {
        0 => 64 * z,
        1 => -2 * m1 + 58 * z + 10 * p1 - 2 * p2,
        2 => -4 * m1 + 54 * z + 16 * p1 - 2 * p2,
        3 => -6 * m1 + 46 * z + 28 * p1 - 4 * p2,
        4 => -4 * m1 + 36 * z + 36 * p1 - 4 * p2,
        5 => -4 * m1 + 28 * z + 46 * p1 - 6 * p2,
        6 => -2 * m1 + 16 * z + 54 * p1 - 4 * p2,
        7 => -2 * m1 + 10 * z + 58 * p1 - 2 * p2,
        _ => return Err(Error::Buffer),
    })
}

fn equation_sample(
    config: InterPrediction,
    plane: (&[u16], usize, usize, usize),
    local: (usize, usize),
) -> Result<i32, Error> {
    let (source, stride, width, height) = plane;
    let units = if config.component == Component::Luma {
        4
    } else {
        8
    };
    let px = config.motion.x.rem_euclid(units);
    let py = config.motion.y.rem_euclid(units);
    let x = config.x as i64 + i64::from(config.motion.x.div_euclid(units)) + local.0 as i64;
    let y = config.y as i64 + i64::from(config.motion.y.div_euclid(units)) + local.1 as i64;
    let read = |dx: i64, dy: i64| {
        source
            .get(
                (y + dy).clamp(0, height as i64 - 1) as usize * stride
                    + (x + dx).clamp(0, width as i64 - 1) as usize,
            )
            .copied()
            .map(i64::from)
            .ok_or(Error::Buffer)
    };
    let horizontal = |dy: i64| {
        let sum = if config.component == Component::Luma {
            let mut row = [0; 8];
            for (i, value) in row.iter_mut().enumerate() {
                *value = read(i as i64 - 3, dy)?;
            }
            equation_luma(row, px)?
        } else {
            let mut row = [0; 4];
            for (i, value) in row.iter_mut().enumerate() {
                *value = read(i as i64 - 1, dy)?;
            }
            equation_chroma(row, px)?
        };
        Ok::<i64, Error>(sum.div_euclid(1 << (config.depth.bits() - 8)))
    };
    let value = if config.component == Component::Luma {
        let mut column = [0; 8];
        for (j, value) in column.iter_mut().enumerate() {
            *value = horizontal(j as i64 - 3)?;
        }
        equation_luma(column, py)?
    } else {
        let mut column = [0; 4];
        for (j, value) in column.iter_mut().enumerate() {
            *value = horizontal(j as i64 - 1)?;
        }
        equation_chroma(column, py)?
    };
    Ok(value.div_euclid(64) as i32)
}

fn config(
    bits: u8,
    component: Component,
    x: usize,
    y: usize,
    mx: i16,
    my: i16,
) -> Result<InterPrediction, Error> {
    Ok(InterPrediction {
        depth: BitDepth::new(bits)?,
        component,
        x,
        y,
        motion: MotionVector { x: mx, y: my },
    })
}

fn single(config: InterPrediction, samples: &[u16], size: usize) -> Result<i32, Error> {
    let mut result = [i32::MAX];
    interpolate_inter(
        config,
        RefPlane::new(samples, size, size, size)?,
        RectMut::new(&mut result, 1, 1, 1)?,
    )?;
    Ok(result[0])
}

#[test]
fn rectangular_views_check_dimensions_strides_extent_and_overflow() {
    for (width, height) in [(0, 4), (4, 0), (65, 4), (4, 65), (usize::MAX, 1)] {
        let mut data = [0; 64];
        assert!(matches!(
            Rect::new(&data, 8, width, height),
            Err(Error::Rectangle)
        ));
        assert!(matches!(
            RectMut::new(&mut data, 8, width, height),
            Err(Error::Rectangle)
        ));
    }
    for (stride, width, height) in [(3, 4, 2), (8, 4, 10), (usize::MAX, 1, 2)] {
        let mut data = [0; 64];
        assert!(matches!(
            Rect::new(&data, stride, width, height),
            Err(Error::Buffer)
        ));
        assert!(matches!(
            RectMut::new(&mut data, stride, width, height),
            Err(Error::Buffer)
        ));
        assert!(matches!(
            RefPlane::new(&data, stride, width, height),
            Err(Error::Buffer)
        ));
    }
    for (width, height) in [(0, 1), (1, 0), (i32::MAX as usize + 1, 1), (1, usize::MAX)] {
        assert!(matches!(
            RefPlane::new(&[0], 1, width, height),
            Err(Error::Buffer)
        ));
    }
    let mut samples = [0; 19];
    let input = Rect::new(&samples, 8, 3, 3).unwrap();
    assert_eq!((input.width(), input.height()), (3, 3));
    let output = RectMut::new(&mut samples, 8, 3, 3).unwrap();
    assert_eq!((output.width(), output.height()), (3, 3));
    assert!(Rect::new(&[0; 18], 8, 3, 3).is_err());
    assert!(RefPlane::new(&[0; 65], 65, 65, 1).is_ok());
}

#[test]
fn luma_every_phase_and_tap_matches_pinned_equations() {
    for bits in [8, 10] {
        let peak = (1 << bits) - 1;
        for y_phase in 0..4 {
            for x_phase in 0..4 {
                let request = config(bits, Component::Luma, 3, 3, x_phase, y_phase).unwrap();
                for impulse in 0..64 {
                    let mut source = [0; 64];
                    source[impulse] = peak;
                    assert_eq!(
                        single(request, &source, 8).unwrap(),
                        equation_sample(request, (&source, 8, 8, 8), (0, 0)).unwrap(),
                        "{bits}/{x_phase}/{y_phase}/{impulse}"
                    );
                }
            }
        }
    }
}

#[test]
fn chroma_every_phase_and_tap_matches_pinned_equations() {
    for bits in [8, 10] {
        let peak = (1 << bits) - 1;
        for y_phase in 0..8 {
            for x_phase in 0..8 {
                let request = config(bits, Component::Chroma, 1, 1, x_phase, y_phase).unwrap();
                for impulse in 0..16 {
                    let mut source = [0; 16];
                    source[impulse] = peak;
                    assert_eq!(
                        single(request, &source, 4).unwrap(),
                        equation_sample(request, (&source, 4, 4, 4), (0, 0)).unwrap(),
                        "{bits}/{x_phase}/{y_phase}/{impulse}"
                    );
                }
            }
        }
    }
}

#[test]
fn rectangular_interpolation_preserves_strides_and_fractional_precision() {
    for bits in [8, 10] {
        for component in [Component::Luma, Component::Chroma] {
            let phases = if component == Component::Luma { 4 } else { 8 };
            let mut source = [u16::MAX; 17 * 11];
            for y in 0..11 {
                for x in 0..13 {
                    source[y * 17 + x] = ((47 * x + 91 * y + 13 * x * y) % (1 << bits)) as u16;
                }
            }
            for py in 0..phases {
                for px in 0..phases {
                    let request = config(bits, component, 7, 4, px, py).unwrap();
                    let mut result = [i32::MAX; 30];
                    interpolate_inter(
                        request,
                        RefPlane::new(&source, 17, 13, 11).unwrap(),
                        RectMut::new(&mut result, 6, 3, 5).unwrap(),
                    )
                    .unwrap();
                    for y in 0..5 {
                        for x in 0..6 {
                            let expected = if x < 3 {
                                equation_sample(request, (&source, 17, 13, 11), (x, y)).unwrap()
                            } else {
                                i32::MAX
                            };
                            assert_eq!(
                                result[y * 6 + x],
                                expected,
                                "{bits}/{component:?}/{px}/{py}/{x}/{y}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn signed_motion_floor_and_picture_edge_replication_match_equations() {
    for bits in [8, 10] {
        let peak = (1 << bits) - 1;
        let source = [0, peak, 1, peak / 2, 17, peak - 1];
        for component in [Component::Luma, Component::Chroma] {
            for (x, y) in [(0, 0), (2, 1)] {
                for mx in [
                    i16::MIN,
                    -19,
                    -9,
                    -8,
                    -7,
                    -6,
                    -5,
                    -4,
                    -3,
                    -2,
                    -1,
                    0,
                    1,
                    2,
                    3,
                    4,
                    5,
                    6,
                    7,
                    19,
                    i16::MAX,
                ] {
                    for my in [
                        i16::MIN,
                        -9,
                        -8,
                        -7,
                        -6,
                        -5,
                        -4,
                        -3,
                        -2,
                        -1,
                        0,
                        1,
                        2,
                        3,
                        4,
                        5,
                        6,
                        7,
                        9,
                        i16::MAX,
                    ] {
                        let request = config(bits, component, x, y, mx, my).unwrap();
                        let mut result = [i32::MAX; 6];
                        interpolate_inter(
                            request,
                            RefPlane::new(&source, 3, 3, 2).unwrap(),
                            RectMut::new(&mut result, 3, 3, 2).unwrap(),
                        )
                        .unwrap();
                        for dy in 0..2 {
                            for dx in 0..3 {
                                assert_eq!(
                                    result[dy * 3 + dx],
                                    equation_sample(request, (&source, 3, 3, 2), (dx, dy)).unwrap(),
                                    "{bits}/{component:?}/{mx}/{my}/{dx}/{dy}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn all_rectangular_pu_dimensions_and_constant_depth_extremes_are_exact() {
    for bits in [8, 10] {
        for component in [Component::Luma, Component::Chroma] {
            let limit = if component == Component::Luma { 64 } else { 32 };
            for width in [1, 2, 4, 6, 8, 12, 16, 24, 32, 48, 64]
                .into_iter()
                .filter(|&v| v <= limit)
            {
                for height in [1, 2, 4, 6, 8, 12, 16, 24, 32, 48, 64]
                    .into_iter()
                    .filter(|&v| v <= limit)
                {
                    for value in [0, (1 << bits) - 1] {
                        let source = [value];
                        let mut result = vec![i32::MAX; width * height];
                        interpolate_inter(
                            config(bits, component, 0, 0, -1, 7).unwrap(),
                            RefPlane::new(&source, 1, 1, 1).unwrap(),
                            RectMut::new(&mut result, width, width, height).unwrap(),
                        )
                        .unwrap();
                        assert!(result.iter().all(|&v| v == i32::from(value) << (14 - bits)));
                    }
                }
            }
        }
    }
}

#[test]
fn half_phase_extrema_require_signed_i32_without_intermediate_clipping() {
    let positive = [false, true, false, true, true, false, true, false];
    for (bits, lower, upper) in [(8, -16830, 33150), (10, -16880, 33247)] {
        for (same_sign, expected) in [(true, upper), (false, lower)] {
            let mut source = [0; 64];
            for y in 0..8 {
                for x in 0..8 {
                    if (positive[x] == positive[y]) == same_sign {
                        source[y * 8 + x] = (1 << bits) - 1;
                    }
                }
            }
            let value = single(
                config(bits, Component::Luma, 3, 3, 2, 2).unwrap(),
                &source,
                8,
            )
            .unwrap();
            assert_eq!(value, expected);
            let mut output = [555];
            predict_uni(
                BitDepth::new(bits).unwrap(),
                Rect::new(&[value], 1, 1, 1).unwrap(),
                RectMut::new(&mut output, 1, 1, 1).unwrap(),
            )
            .unwrap();
            assert_eq!(output[0], if same_sign { (1 << bits) - 1 } else { 0 });
        }
    }
}

#[test]
fn interpolation_invalid_origin_support_or_size_never_writes() {
    for bits in [8, 10] {
        let mut source = [0; 64];
        source[63] = 1 << bits;
        let mut output = [777; 4];
        assert_eq!(
            interpolate_inter(
                config(bits, Component::Luma, 3, 3, 2, 2).unwrap(),
                RefPlane::new(&source, 8, 8, 8).unwrap(),
                RectMut::new(&mut output, 2, 2, 2).unwrap()
            ),
            Err(Error::Sample)
        );
        assert_eq!(output, [777; 4]);
        for (x, y) in [(usize::MAX, 0), (0, usize::MAX), (i32::MAX as usize + 1, 0)] {
            assert_eq!(
                interpolate_inter(
                    config(bits, Component::Luma, x, y, 0, 0).unwrap(),
                    RefPlane::new(&source, 8, 8, 8).unwrap(),
                    RectMut::new(&mut output, 2, 2, 2).unwrap()
                ),
                Err(Error::Buffer)
            );
            assert_eq!(output, [777; 4]);
        }
        source[63] = 0;
        interpolate_inter(
            config(
                bits,
                Component::Luma,
                i32::MAX as usize,
                i32::MAX as usize,
                i16::MAX,
                i16::MAX,
            )
            .unwrap(),
            RefPlane::new(&source, 8, 8, 8).unwrap(),
            RectMut::new(&mut output, 2, 2, 2).unwrap(),
        )
        .unwrap();
        assert_eq!(output, [0; 4]);
        let mut large = [777; 64];
        for (width, height) in [(33, 1), (1, 33)] {
            assert_eq!(
                interpolate_inter(
                    config(bits, Component::Chroma, 0, 0, 0, 0).unwrap(),
                    RefPlane::new(&[0], 1, 1, 1).unwrap(),
                    RectMut::new(&mut large, width, width, height).unwrap()
                ),
                Err(Error::Rectangle)
            );
            assert_eq!(large, [777; 64]);
        }
    }
}

#[test]
fn reference_padding_and_unread_samples_do_not_fail_interpolation() {
    let source = [12, 13, u16::MAX, 14, 15, u16::MAX];
    let mut output = [i32::MAX; 4];
    interpolate_inter(
        config(8, Component::Luma, 0, 0, 0, 0).unwrap(),
        RefPlane::new(&source, 3, 2, 2).unwrap(),
        RectMut::new(&mut output, 2, 2, 2).unwrap(),
    )
    .unwrap();
    assert_eq!(output, [768, 832, 896, 960]);
    assert_eq!(
        single(
            config(8, Component::Luma, 0, 0, 0, 0).unwrap(),
            &[12, u16::MAX, u16::MAX, u16::MAX],
            2
        )
        .unwrap(),
        768
    );
}

#[test]
fn weight_constructors_enforce_normative_effective_domains() {
    for denom in 0..=7 {
        let base = 1 << denom;
        for delta in [-128, 0, 127] {
            for offset in [-128, 0, 127] {
                let weight = PredictionWeight::new(denom, base + delta, offset).unwrap();
                assert_eq!(
                    (weight.denom(), weight.weight(), weight.offset()),
                    (denom, base + delta, offset)
                );
            }
        }
        for delta in [-129, 128] {
            assert_eq!(
                PredictionWeight::new(denom, base + delta, 0),
                Err(Error::Weight)
            );
        }
        for offset in [-129, 128] {
            assert_eq!(
                PredictionWeight::new(denom, base, offset),
                Err(Error::Weight)
            );
        }
    }
    for denom in [8, 31, 63, 255] {
        assert_eq!(PredictionWeight::new(denom, 1, 0), Err(Error::Weight));
    }
    assert_eq!(PredictionWeight::new(0, i16::MIN, 0), Err(Error::Weight));
    assert_eq!(PredictionWeight::new(7, i16::MAX, 0), Err(Error::Weight));
}

#[test]
fn default_uni_and_bi_round_and_clip_literal_internal_predictors() {
    for bits in [8, 10] {
        let depth = BitDepth::new(bits).unwrap();
        let scale = 1 << (14 - bits);
        let peak = i32::from(depth.max_sample());
        let first = [
            -1,
            0,
            scale / 2 - 1,
            scale / 2,
            3 * scale / 2,
            peak * scale,
            peak * scale + scale / 2,
        ];
        let second = [0, scale, scale / 2, scale / 2, scale / 2, 0, peak * scale];
        let mut uni = [999; 7];
        let mut bi = [999; 7];
        predict_uni(
            depth,
            Rect::new(&first, 7, 7, 1).unwrap(),
            RectMut::new(&mut uni, 7, 7, 1).unwrap(),
        )
        .unwrap();
        predict_bi(
            depth,
            Rect::new(&first, 7, 7, 1).unwrap(),
            Rect::new(&second, 7, 7, 1).unwrap(),
            RectMut::new(&mut bi, 7, 7, 1).unwrap(),
        )
        .unwrap();
        assert_eq!(uni, [0, 0, 0, 1, 2, peak as u16, peak as u16]);
        assert_eq!(bi, [0, 1, 0, 1, 1, ((peak + 1) / 2) as u16, peak as u16]);
    }
}

#[test]
fn explicit_uni_and_bi_match_equations_for_all_denominators_and_extreme_weights() {
    for (bits, minimum, maximum) in [(8, -16830, 33150), (10, -16880, 33247)] {
        let depth = BitDepth::new(bits).unwrap();
        let first = [minimum, -65, -1, 0, 1, 63, 64, 161, maximum];
        let second: Vec<i32> = first.iter().rev().copied().collect();
        for denom in 0..=7 {
            for delta in [-128, -1, 0, 1, 127] {
                for o0 in [-128, -1, 0, 1, 127] {
                    for (delta1, o1) in [(-128, -128), (-128, 127), (0, -1), (127, 0), (127, 127)] {
                        let w0 = (1 << denom) + delta;
                        let w1 = (1 << denom) + delta1;
                        let a = PredictionWeight::new(denom, w0, o0).unwrap();
                        let b = PredictionWeight::new(denom, w1, o1).unwrap();
                        let mut uni = [999; 9];
                        let mut bi = [999; 9];
                        weight_uni(
                            depth,
                            a,
                            Rect::new(&first, 3, 3, 3).unwrap(),
                            RectMut::new(&mut uni, 3, 3, 3).unwrap(),
                        )
                        .unwrap();
                        weight_bi(
                            depth,
                            a,
                            b,
                            Rect::new(&first, 3, 3, 3).unwrap(),
                            Rect::new(&second, 3, 3, 3).unwrap(),
                            RectMut::new(&mut bi, 3, 3, 3).unwrap(),
                        )
                        .unwrap();
                        let divisor = 1_i64 << (denom + 14 - bits);
                        let offset_scale = 1_i64 << (bits - 8);
                        for i in 0..9 {
                            let weighted = i64::from(first[i]) * i64::from(w0);
                            let expected_uni = (weighted + divisor / 2).div_euclid(divisor)
                                + i64::from(o0) * offset_scale;
                            let expected_bi = (weighted
                                + i64::from(second[i]) * i64::from(w1)
                                + (i64::from(o0 + o1) * offset_scale + 1) * divisor)
                                .div_euclid(2 * divisor);
                            assert_eq!(
                                uni[i],
                                expected_uni.clamp(0, i64::from(depth.max_sample())) as u16
                            );
                            assert_eq!(
                                bi[i],
                                expected_bi.clamp(0, i64::from(depth.max_sample())) as u16
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn explicit_bi_keeps_odd_scaled_offsets_until_the_final_shift() {
    for (bits, expected) in [(8, 2), (10, 6)] {
        let predictor = [65];
        let mut output = [999];
        weight_bi(
            BitDepth::new(bits).unwrap(),
            PredictionWeight::new(0, 1, 1).unwrap(),
            PredictionWeight::new(0, 1, 0).unwrap(),
            Rect::new(&predictor, 1, 1, 1).unwrap(),
            Rect::new(&predictor, 1, 1, 1).unwrap(),
            RectMut::new(&mut output, 1, 1, 1).unwrap(),
        )
        .unwrap();
        assert_eq!(output[0], expected);
    }
}

#[test]
fn combination_checks_all_domains_and_shapes_before_writing() {
    for (bits, minimum, maximum) in [(8, -16830, 33150), (10, -16880, 33247)] {
        let depth = BitDepth::new(bits).unwrap();
        let valid = [0; 6];
        for invalid in [i32::MIN, minimum - 1, maximum + 1, i32::MAX] {
            let bad = [0, 0, 0, 0, 0, invalid];
            let mut output = [777; 6];
            assert_eq!(
                predict_uni(
                    depth,
                    Rect::new(&bad, 3, 3, 2).unwrap(),
                    RectMut::new(&mut output, 3, 3, 2).unwrap()
                ),
                Err(Error::Sample)
            );
            assert_eq!(output, [777; 6]);
            for (first, second) in [(&valid, &bad), (&bad, &valid)] {
                assert_eq!(
                    predict_bi(
                        depth,
                        Rect::new(first, 3, 3, 2).unwrap(),
                        Rect::new(second, 3, 3, 2).unwrap(),
                        RectMut::new(&mut output, 3, 3, 2).unwrap()
                    ),
                    Err(Error::Sample)
                );
                assert_eq!(output, [777; 6]);
            }
        }
        let mut output = [777; 6];
        assert_eq!(
            predict_uni(
                depth,
                Rect::new(&valid, 2, 2, 3).unwrap(),
                RectMut::new(&mut output, 3, 3, 2).unwrap()
            ),
            Err(Error::Rectangle)
        );
        assert_eq!(output, [777; 6]);
        assert_eq!(
            predict_bi(
                depth,
                Rect::new(&valid, 3, 3, 2).unwrap(),
                Rect::new(&valid, 2, 2, 3).unwrap(),
                RectMut::new(&mut output, 3, 3, 2).unwrap()
            ),
            Err(Error::Rectangle)
        );
        assert_eq!(output, [777; 6]);
        assert_eq!(
            weight_bi(
                depth,
                PredictionWeight::new(0, 1, 0).unwrap(),
                PredictionWeight::new(1, 2, 0).unwrap(),
                Rect::new(&valid, 3, 3, 2).unwrap(),
                Rect::new(&valid, 3, 3, 2).unwrap(),
                RectMut::new(&mut output, 3, 3, 2).unwrap()
            ),
            Err(Error::Weight)
        );
        assert_eq!(output, [777; 6]);
    }
}

#[test]
fn combination_ignores_input_padding_and_preserves_independent_output_stride() {
    let first = [640, 1280, i32::MIN, i32::MAX, 1920, 2560];
    let second = [0, 1280, i32::MAX, 2560, 3840];
    let mut output = [999; 5];
    predict_bi(
        BitDepth::new(8).unwrap(),
        Rect::new(&first, 4, 2, 2).unwrap(),
        Rect::new(&second, 3, 2, 2).unwrap(),
        RectMut::new(&mut output, 3, 2, 2).unwrap(),
    )
    .unwrap();
    assert_eq!(output, [5, 20, 999, 35, 50]);
}
