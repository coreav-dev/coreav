#![no_main]

use coreav_kernels_hevc::{BitDepth, SaoBand, SaoClass, SaoEdge, sao_sample};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Some((&flags, data)) = data.split_first() else {
        return;
    };
    let depth = BitDepth::new(if flags & 1 == 0 { 8 } else { 10 }).unwrap();
    let maximum = (1i16 << (depth.bits() - 5)) - 1;
    let Some(parameters) = data.get(..5) else {
        return;
    };
    let offsets: [i16; 4] =
        core::array::from_fn(|i| i16::from(parameters[i] as i8) % (maximum + 1));
    let position = parameters[4] & 31;
    let mut rows = [Vec::new(), Vec::new(), Vec::new()];
    for (i, sample) in data[5..].chunks_exact(2).take(387).enumerate() {
        rows[i % 3].push(u16::from_le_bytes([sample[0], sample[1]]) & depth.max_sample());
    }
    let len = rows.iter().map(Vec::len).min().unwrap();
    for row in &mut rows {
        row.truncate(len);
    }
    let mut output = rows[0].clone();
    let mut expected = output.clone();
    for (i, sample) in expected.iter_mut().enumerate() {
        let class = if flags & 2 == 0 {
            SaoClass::Band(position)
        } else {
            SaoClass::Edge([rows[1][i], rows[2][i]])
        };
        *sample = sao_sample(depth, *sample, offsets, class).unwrap();
    }
    let changed = if flags & 2 == 0 {
        SaoBand::new(depth, offsets, position)
            .unwrap()
            .apply(&mut output)
            .unwrap()
    } else {
        SaoEdge::new(depth, offsets)
            .unwrap()
            .apply(&rows[0], [&rows[1], &rows[2]], &mut output)
            .unwrap()
    };
    assert_eq!(output, expected);
    assert_eq!(
        changed,
        rows[0]
            .iter()
            .zip(&expected)
            .filter(|(a, b)| a != b)
            .count() as u64
    );
});
