#![no_main]

use coreav_backend_apple::h264::{AccessUnit, Config, Dimensions, Limits, Sync};
use libfuzzer_sys::fuzz_target;

#[derive(arbitrary::Arbitrary, Debug)]
struct Input<'a> {
    sps: &'a [u8],
    pps: &'a [u8],
    data: &'a [u8],
    width: u32,
    height: u32,
    byte_limit: u16,
    nal_limit: u8,
}

fn exercise(config: &Config, data: &[u8], limits: Limits) {
    assert_eq!(
        Config::from_parameter_sets(config.sps(), config.pps(), config.dimensions(), limits)
            .map(|rebuilt| rebuilt.as_avcc().to_vec()),
        Ok(config.as_avcc().to_vec())
    );
    for sync in [Sync::Idr, Sync::NonIdr] {
        if let Ok(unit) = AccessUnit::copy_from(data, config, sync, limits) {
            assert_eq!(unit.sync(), sync);
            assert_eq!(unit.as_bytes(), data);
            assert!(unit.as_bytes().len() <= limits.max_access_unit_bytes);
            assert!(unit.slices() > 0 && unit.slices() <= unit.nals());
            assert!(unit.nals() <= limits.max_nals);
            let other = if sync == Sync::Idr {
                Sync::NonIdr
            } else {
                Sync::Idr
            };
            assert!(AccessUnit::copy_from(data, config, other, limits).is_err());
        }
    }
}

fuzz_target!(|input: Input<'_>| {
    let limits = Limits {
        max_configuration_bytes: 4096,
        max_width: 4096,
        max_height: 4096,
        max_access_unit_bytes: usize::from(input.byte_limit).min(16_384),
        max_nals: usize::from(input.nal_limit).min(64),
    };
    if let Ok(dimensions) = Dimensions::new(input.width, input.height) {
        if let Ok(config) = Config::from_parameter_sets(input.sps, input.pps, dimensions, limits) {
            assert_eq!(config.dimensions(), dimensions);
            assert!(config.as_avcc().len() <= limits.max_configuration_bytes);
            exercise(&config, input.data, limits);
        }
    }
    let sps = [0x67, 0x64, 0x00, 0x1f, 0xac, 0xec, 0x96, 0x40];
    let pps = [0x68, 0xee, 0x3c, 0x80];
    if let Ok(dimensions) = Dimensions::new(32, 32) {
        let config = Config::from_parameter_sets(&sps, &pps, dimensions, limits);
        assert!(config.is_ok());
        if let Ok(config) = config {
            exercise(&config, input.data, limits);
        }
    }
});
