//! Arbitrary bytes as each kind of codec configuration record, read by
//! `StreamConfig::traits` (the `avcC` sequence parameter set included).
//! Every record gives traits or none, never a panic.
#![no_main]

use coreav_decode::{Codec, CodecConfig, ConfigKind, StreamConfig};
use libfuzzer_sys::fuzz_target;

const KINDS: [ConfigKind; 7] = [
    ConfigKind::Avcc,
    ConfigKind::Hvcc,
    ConfigKind::Av1c,
    ConfigKind::Vpcc,
    ConfigKind::ProResFourcc,
    ConfigKind::Mpeg4Dsi,
    ConfigKind::MatroskaPrivate,
];

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let kind = KINDS[usize::from(selector) % KINDS.len()];
    let codec = Codec::ALL[usize::from(selector >> 4) % Codec::ALL.len()];
    let stream = StreamConfig {
        config: Some(CodecConfig {
            kind,
            data: rest.to_vec(),
        }),
        ..StreamConfig::new(codec)
    };
    let traits = stream.traits();
    if let Some(depth) = traits.bit_depth {
        assert!((8..=16).contains(&depth));
    }
});
