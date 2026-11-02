#![no_main]

use coreav_backend_apple::aac::{BatchLimits, Config, PacketBatch, PacketDescription, Trim};
use libfuzzer_sys::fuzz_target;

#[derive(arbitrary::Arbitrary, Debug)]
struct Description {
    start: i64,
    size: u32,
    frames: u32,
}

#[derive(arbitrary::Arbitrary, Debug)]
struct Input<'a> {
    config: &'a [u8],
    data: &'a [u8],
    descriptions: Vec<Description>,
    packets: u64,
    submitted: u64,
    leading: u32,
    trailing: u32,
    trim_limit: u32,
}

fuzz_target!(|input: Input<'_>| {
    for (rate, channels) in [(44_100, 1), (44_100, 2), (48_000, 1), (48_000, 2)] {
        if let Ok(config) = Config::from_cookie(input.config, rate, channels) {
            assert_eq!(config.sample_rate(), rate);
            assert_eq!(config.channels(), channels);
            assert_eq!(Config::parse(config.as_bytes(), rate, channels), Ok(config));
        }
        if let Ok(config) = Config::parse(input.config, rate, channels) {
            assert_eq!(config.sample_rate(), rate);
            assert_eq!(config.channels(), channels);
            assert_eq!(config.as_bytes().as_slice(), input.config);
        }
    }
    let descriptions: Vec<_> = input
        .descriptions
        .iter()
        .take(65)
        .map(|description| PacketDescription {
            start_offset: description.start,
            byte_size: description.size,
            variable_frames: description.frames,
        })
        .collect();
    let limits = BatchLimits {
        max_buffer_bytes: 16_384,
        max_packet_bytes: 4096,
        max_packets: 64,
    };
    if let Ok(batch) = PacketBatch::copy_from(input.data, &descriptions, limits) {
        assert_eq!(batch.packets().len(), descriptions.len());
        assert_eq!(batch.nominal_frames(), descriptions.len() as u64 * 1024);
        assert!(batch.packets().map(<[u8]>::len).sum::<usize>() <= input.data.len());
        assert!(
            batch
                .packets()
                .all(|packet| !packet.is_empty() && packet.len() <= 4096)
        );
    }
    if let Ok(trim) = Trim::validate(
        input.packets,
        input.submitted,
        input.leading,
        input.trailing,
        input.trim_limit,
    ) {
        assert_eq!(
            u128::from(input.packets) * 1024,
            u128::from(input.submitted)
                + u128::from(trim.leading_frames())
                + u128::from(trim.trailing_frames())
        );
    }
});
