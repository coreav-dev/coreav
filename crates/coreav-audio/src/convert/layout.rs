#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;

#[test]
fn packed_float_to_planes_preserves_every_sample_bit() {
    let bits = [
        0,
        0x8000_0000,
        1,
        0x8000_0001,
        0x3F80_0000,
        0xBF80_0000,
        0x7F80_0000,
        0xFF80_0000,
        0x7FC0_0001,
        0xFFC0_1234,
    ];
    for channels in [1, 2, 3] {
        let mut pcm = Pcm::new();
        let frames = bits.len();
        let samples = pcm.float_buffer(channels, frames);
        for (c, plane) in samples.chunks_exact_mut(frames).enumerate() {
            for (i, value) in plane.iter_mut().enumerate() {
                *value = f32::from_bits(*bits.iter().cycle().nth(i + c).unwrap());
            }
        }
        let mut packed = Vec::new();
        interleave(&pcm, 0, frames, SampleFormat::F32, &mut packed, 0);
        for c in 0..channels {
            let mut plane = Vec::new();
            crate::mix::deinterleave(&packed, channels, c, &mut plane);
            let Some(Plane::Float(expected)) = pcm.plane(c) else {
                panic!("missing float plane");
            };
            assert_eq!(
                plane.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
        }
    }
}
