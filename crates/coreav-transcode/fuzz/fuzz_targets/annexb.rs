//! Arbitrary bytes as an encoder's H.264 byte stream, pushed in arbitrary
//! chunks under small or default limits: every call returns or fails with
//! an error, never panics. The pictures read must carry well-formed
//! length-prefixed samples and presentation indices that are a permutation,
//! and their sample times must compute. Arbitrary presentation times and
//! indices go through `sample_times` as well.
#![no_main]

use coreav_transcode::annexb::{AnnexBReader, Limits};
use coreav_transcode::timing::sample_times;
use libfuzzer_sys::fuzz_target;

fn limits(selector: u8) -> Limits {
    if selector & 1 == 0 {
        return Limits::default();
    }
    Limits {
        max_nal_unit_bytes: 16 + usize::from(selector) * 4,
        max_access_unit_bytes: 32 + usize::from(selector) * 8,
        max_nal_units_per_access_unit: 1 + usize::from(selector >> 4),
        max_period_pictures: 1 + usize::from(selector >> 3),
        max_period_bytes: 64 + u64::from(selector) * 16,
        max_pictures: u64::from(selector >> 2),
    }
}

fn arbitrary_times(data: &[u8]) {
    let [last, rest @ ..] = data else {
        return;
    };
    let mut presented = Vec::new();
    let mut order = Vec::new();
    for chunk in rest.chunks_exact(9) {
        let mut time = [0u8; 8];
        time.copy_from_slice(chunk.get(..8).unwrap_or(&[0; 8]));
        presented.push(i64::from_le_bytes(time));
        order.push(u64::from(chunk.get(8).copied().unwrap_or(0)));
    }
    let _ = sample_times(&presented, &order, u64::from(*last));
    // Sorted, so the permutation and overflow checks are reached more often.
    presented.sort_unstable();
    presented.dedup();
    order.truncate(presented.len());
    let _ = sample_times(&presented, &order, u64::from(*last));
}

fuzz_target!(|data: &[u8]| run(data));

fn run(data: &[u8]) {
    let [selector, seed, stream @ ..] = data else {
        return;
    };
    if selector & 0x80 != 0 && selector & 0x40 != 0 {
        arbitrary_times(stream);
        return;
    }
    let mut reader = AnnexBReader::new(limits(*selector));
    let mut pictures = Vec::new();
    let mut rest = stream;
    let mut state = u32::from(*seed) | 1;
    let mut failed = false;
    while !rest.is_empty() {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let size = 1 + (state >> 16) as usize % 64;
        let (chunk, tail) = rest.split_at(size.min(rest.len()));
        if reader.push(chunk).is_err() {
            failed = true;
            break;
        }
        pictures.extend(std::iter::from_fn(|| reader.pop()));
        rest = tail;
    }
    if failed || reader.finish().is_err() {
        assert!(reader.push(&[0, 0, 1]).is_err());
        return;
    }
    pictures.extend(std::iter::from_fn(|| reader.pop()));
    if !pictures.is_empty() {
        let config = reader.config().map(|c| c.avcc.len());
        assert!(config.is_some_and(|n| n >= 7));
    }
    let mut seen = vec![false; pictures.len()];
    for picture in &pictures {
        let mut sample = picture.data.as_slice();
        while let [a, b, c, d, tail @ ..] = sample {
            let n = u32::from_be_bytes([*a, *b, *c, *d]) as usize;
            assert!(n > 0 && n <= tail.len());
            sample = tail.get(n..).unwrap_or_default();
        }
        assert!(sample.is_empty());
        let slot = seen.get_mut(picture.presentation as usize);
        let slot = slot.expect("a presentation index within the pictures");
        assert!(!*slot);
        *slot = true;
    }
    let presented: Vec<i64> = (0..pictures.len() as i64).map(|i| i * 3).collect();
    let order: Vec<u64> = pictures.iter().map(|p| p.presentation).collect();
    let times = sample_times(&presented, &order, 3).expect("sample times of a permutation");
    assert!(times.windows(2).all(|w| w[0].dts < w[1].dts));
}
