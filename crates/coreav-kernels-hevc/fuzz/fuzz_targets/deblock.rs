#![no_main]

use coreav_kernels_hevc::{
    BitDepth, BoundaryStrength, deblock_chroma, deblock_chroma_scalar, deblock_luma_scalar,
    deblock_luma_with_strength,
};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Some(parameters) = data.get(..6) else {
        return;
    };
    let flags = parameters[0];
    let depth = BitDepth::new(if flags & 1 == 0 { 8 } else { 10 }).unwrap();
    let qp = i32::from(parameters[1] as i8);
    let offsets = [
        i32::from(parameters[2] as i8) % 8,
        i32::from(parameters[3] as i8) % 8,
    ];
    let strength = match parameters[4] % 3 {
        0 => BoundaryStrength::Zero,
        1 => BoundaryStrength::One,
        _ => BoundaryStrength::Two,
    };
    let protected = [parameters[5] & 1 != 0, parameters[5] & 2 != 0];
    let mut edge = [[0; 8]; 4];
    for (sample, bytes) in edge.iter_mut().flatten().zip(data[6..].chunks_exact(2)) {
        let value = u16::from_le_bytes([bytes[0], bytes[1]]);
        *sample = if flags & 2 == 0 {
            value & depth.max_sample()
        } else {
            value
        };
    }
    assert_eq!(
        deblock_luma_with_strength(depth, edge, qp, offsets, strength, protected),
        deblock_luma_scalar(depth, edge, qp, offsets, strength, protected)
    );
    let chroma = edge.map(|row| [row[2], row[3], row[4], row[5]]);
    assert_eq!(
        deblock_chroma(depth, chroma, qp, offsets[1], protected),
        deblock_chroma_scalar(depth, chroma, qp, offsets[1], protected)
    );
});
