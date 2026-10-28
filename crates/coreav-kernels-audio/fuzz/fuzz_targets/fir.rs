//! `Fir::run` on arbitrary windows, taps and values: the vector kernel and
//! the portable one agree bit for bit, or both refuse the windows.
#![no_main]

use coreav_kernels_audio::Fir;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((head, rest)) = data.split_first_chunk::<4>() else {
        return;
    };
    let taps = usize::from(head[0] % 16) * 8;
    let windows_len = usize::from(head[1] % 64);
    let input_len = usize::from(u16::from_le_bytes([head[2], head[3]]) % 2048);
    let mut bytes = rest.iter().copied().cycle();
    let mut next = || bytes.next().unwrap_or(0);
    // Values from the bytes, NaN and infinities included.
    let mut value = || f32::from_le_bytes([next(), next(), next(), next()]);
    let input: Vec<f32> = (0..input_len).map(|_| value()).collect();
    let table: Vec<f32> = (0..input_len / 2 + 8).map(|_| value()).collect();
    // Mostly inside the slices, sometimes just past them.
    let mut index = |len: usize| usize::from(u16::from_le_bytes([next(), next()])) % (len + 2);
    let windows: Vec<(usize, usize)> = (0..windows_len)
        .map(|_| {
            let start = index(input_len.saturating_sub(taps));
            (start, index(table.len().saturating_sub(taps)))
        })
        .collect();
    let mut a = vec![0.0f32; windows.len()];
    let mut b = vec![0.0f32; windows.len()];
    let ra = Fir::portable().run(&input, &table, taps, &windows, &mut a);
    let rb = Fir::detect().run(&input, &table, taps, &windows, &mut b);
    assert_eq!(ra, rb);
    for (x, y) in a.iter().zip(&b) {
        assert!(x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()));
    }
});
