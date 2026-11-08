#![no_main]
use coreav_kernels_hevc::{BitDepth, PredictionWeight, Rect, RectMut};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if data.len() < 24 {
        return;
    }
    let bits = if data[0] & 1 == 0 { 8 } else { 10 };
    let depth = BitDepth::new(bits).unwrap();
    let (minimum, maximum) = if bits == 8 {
        (-16830, 33150)
    } else {
        (-16880, 33247)
    };
    let width = usize::from(data[2] % 64) + 1;
    let height = usize::from(data[3] % 64) + 1;
    let first_stride = width + usize::from(data[4] & 7);
    let second_stride = width + usize::from(data[5] & 7);
    let output_stride = width + usize::from(data[6] & 7);
    let first_offset = usize::from(data[7] & 7);
    let second_offset = usize::from(data[8] & 7);
    let output_offset = usize::from(data[13] & 7);
    let mut first = vec![i32::MIN; first_offset + first_stride * (height - 1) + width];
    let mut second = vec![i32::MAX; second_offset + second_stride * (height - 1) + width];
    for (side, buffer, stride, offset) in [
        (0, &mut first, first_stride, first_offset),
        (1, &mut second, second_stride, second_offset),
    ] {
        for y in 0..height {
            for x in 0..width {
                let index = side * width * height + y * width + x;
                let position = 20 + (4 * index) % (data.len() - 20);
                let raw =
                    u32::from_le_bytes(core::array::from_fn(|i| data[(position + i) % data.len()]));
                buffer[offset + y * stride + x] =
                    minimum + (raw % (maximum - minimum + 1) as u32) as i32;
            }
        }
        if data[8] & 0xc0 == 0xc0 && side == usize::from(data[0] & 2 != 0) {
            let index = if data[14] & 1 == 0 {
                buffer.len() - 1
            } else {
                offset + usize::from(data[15]) % height * stride + usize::from(data[16]) % width
            };
            buffer[index] = match data[17] & 3 {
                0 => i32::MAX,
                1 => i32::MIN,
                2 => minimum - 1,
                _ => maximum + 1,
            };
        }
    }
    let denom = data[1] % 8;
    let second_denom = if data[13] & 0xc0 == 0x40 {
        (denom + 1) % 8
    } else {
        denom
    };
    let weight = PredictionWeight::new(
        denom,
        (1 << denom) + i16::from(data[9] as i8),
        i16::from(data[10] as i8),
    )
    .unwrap();
    let second_weight = PredictionWeight::new(
        second_denom,
        (1 << second_denom) + i16::from(data[11] as i8),
        i16::from(data[12] as i8),
    )
    .unwrap();
    let first = Rect::new(&first[first_offset..], first_stride, width, height).unwrap();
    let second = Rect::new(&second[second_offset..], second_stride, width, height).unwrap();
    let extent = output_stride * (height - 1) + width;
    let mut actual = vec![u16::MAX; output_offset + extent + 5];
    let mut expected = actual.clone();
    let result = if data[0] & 2 != 0 {
        coreav_kernels_hevc::weight_bi(
            depth,
            weight,
            second_weight,
            first,
            second,
            RectMut::new(
                &mut actual[output_offset..output_offset + extent],
                output_stride,
                width,
                height,
            )
            .unwrap(),
        )
    } else {
        coreav_kernels_hevc::weight_uni(
            depth,
            weight,
            first,
            RectMut::new(
                &mut actual[output_offset..output_offset + extent],
                output_stride,
                width,
                height,
            )
            .unwrap(),
        )
    };
    let reference = coreav_kernels_hevc::combine_reference(
        depth,
        weight,
        first,
        (data[0] & 2 != 0).then_some((second_weight, second)),
        RectMut::new(
            &mut expected[output_offset..output_offset + extent],
            output_stride,
            width,
            height,
        )
        .unwrap(),
    );
    assert_eq!(result, reference);
    assert_eq!(actual, expected);
    if result.is_err() {
        assert!(actual.iter().all(|&value| value == u16::MAX));
    }
});
