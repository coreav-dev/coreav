use coreav_kernels_hevc::{BitDepth, Component, InterPrediction, MotionVector, RectMut, RefPlane};

pub fn compare(data: &[u8], vertical: bool) {
    if data.len() < 14 {
        return;
    }
    let depth = BitDepth::new(if data[0] & 1 == 0 { 8 } else { 10 }).unwrap();
    let component = if data[0] & 2 == 0 {
        Component::Luma
    } else {
        Component::Chroma
    };
    let limit = if component == Component::Luma { 64 } else { 32 };
    let phases = if component == Component::Luma { 4 } else { 8 };
    let width = 1 + usize::from(data[1]) % limit;
    let height = 1 + usize::from(data[2]) % limit;
    let source_width = 1 + usize::from(data[3]) % 79;
    let source_height = 1 + usize::from(data[4]) % 79;
    let source_stride = source_width + usize::from(data[5] % 9);
    let output_stride = width + usize::from(data[6] % 9);
    let mut source = vec![u16::MAX; source_stride * (source_height - 1) + source_width];
    for y in 0..source_height {
        for x in 0..source_width {
            let position = 14 + (2 * (y * source_width + x)) % (data.len() - 13);
            let low = data.get(position).copied().unwrap_or(0);
            let high = data.get(position + 1).copied().unwrap_or(0);
            let value = u16::from_le_bytes([low, high]);
            source[y * source_stride + x] = if data[0] & 4 == 0 {
                value & depth.max_sample()
            } else {
                value
            };
        }
    }
    let mx = i16::from_le_bytes([data[7], data[8]]);
    let my = i16::from_le_bytes([data[9], data[10]]);
    let config = InterPrediction {
        depth,
        component,
        x: if data[0] & 8 == 0 {
            usize::from(data[11])
        } else {
            i32::MAX as usize + 1
        },
        y: usize::from(data[12]),
        motion: MotionVector {
            x: if vertical {
                mx
            } else {
                (mx & !(phases - 1)) + 1 + i16::from(data[13]) % (phases - 1)
            },
            y: if vertical {
                (my & !(phases - 1)) + 1 + i16::from(data[13]) % (phases - 1)
            } else {
                my & !(phases - 1)
            },
        },
    };
    let reference = RefPlane::new(&source, source_stride, source_width, source_height).unwrap();
    let mut automatic = vec![i32::MAX; output_stride * (height - 1) + width];
    let mut scalar = automatic.clone();
    let actual = coreav_kernels_hevc::interpolate_inter(
        config,
        reference,
        RectMut::new(&mut automatic, output_stride, width, height).unwrap(),
    );
    let expected = coreav_kernels_hevc::interpolate_inter_scalar(
        config,
        reference,
        RectMut::new(&mut scalar, output_stride, width, height).unwrap(),
    );
    assert_eq!(actual, expected);
    assert_eq!(automatic, scalar);
    if actual.is_err() {
        assert!(automatic.iter().all(|&value| value == i32::MAX));
    }
}
