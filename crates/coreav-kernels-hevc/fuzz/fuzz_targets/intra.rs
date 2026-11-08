#![no_main]
use coreav_kernels_hevc::{BitDepth, BlockMut, BlockSize, Component, IntraMode, IntraPrediction};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let n = 4_usize << (data[0] & 3);
    let depth = BitDepth::new(if data[0] & 4 == 0 { 8 } else { 10 }).unwrap();
    let config = IntraPrediction {
        depth,
        component: if data[0] & 8 == 0 {
            Component::Luma
        } else {
            Component::Chroma
        },
        mode: IntraMode::new(data[1] % 35).unwrap(),
        strong_smoothing: data[0] & 16 != 0,
    };
    let count = if data[0] & 32 == 0 {
        4 * n + 1
    } else {
        usize::from(data[2]) % (4 * n + 1)
    };
    let references: Vec<_> = (0..count)
        .map(|index| {
            let position = 4 + (3 * index) % (data.len() - 4);
            let value =
                u16::from_le_bytes([data[position], data.get(position + 1).copied().unwrap_or(0)]);
            if data[position] & 3 == 0 {
                None
            } else {
                Some(if data[0] & 64 == 0 {
                    value & depth.max_sample()
                } else {
                    value
                })
            }
        })
        .collect();
    let stride = n + usize::from(data[2] % 4);
    let offset = usize::from(data[3] % 7);
    let extent = stride * (n - 1) + n;
    let mut actual = vec![u16::MAX; offset + extent + 5];
    let mut expected = actual.clone();
    let result = coreav_kernels_hevc::predict_intra(
        config,
        &references,
        BlockMut::new(
            &mut actual[offset..offset + extent],
            stride,
            BlockSize::new(n).unwrap(),
        )
        .unwrap(),
    );
    let reference = coreav_kernels_hevc::predict_intra_reference(
        config,
        &references,
        BlockMut::new(
            &mut expected[offset..offset + extent],
            stride,
            BlockSize::new(n).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(result, reference);
    assert_eq!(actual, expected);
    if result.is_err() {
        assert!(actual.iter().all(|&value| value == u16::MAX));
    }
});
