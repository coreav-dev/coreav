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
        let expected_packed: Vec<u8> = (0..frames)
            .flat_map(|i| (0..channels).flat_map(move |c| bits[(i + c) % frames].to_le_bytes()))
            .collect();
        let mut packed = Vec::new();
        interleave(&pcm, 0, frames, SampleFormat::F32, &mut packed, 0);
        assert_eq!(packed, expected_packed);
        for c in 0..channels {
            let mut plane = Vec::new();
            crate::mix::deinterleave(&expected_packed, channels, c, &mut plane);
            assert_eq!(
                plane.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                (0..frames)
                    .map(|i| bits[(i + c) % frames])
                    .collect::<Vec<_>>()
            );
        }
    }
}
