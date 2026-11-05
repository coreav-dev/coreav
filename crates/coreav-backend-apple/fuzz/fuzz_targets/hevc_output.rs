#![no_main]

use coreav_backend_apple::{
    h264::{Dimensions, Limits},
    hevc::{AccessUnit, Config, Stream},
};
use libfuzzer_sys::fuzz_target;

#[derive(arbitrary::Arbitrary, Debug)]
struct Input<'a> {
    vps: &'a [u8],
    sps: &'a [u8],
    pps: &'a [u8],
    hvcc: &'a [u8],
    sample: &'a [u8],
    sync: bool,
    width: u16,
    height: u16,
}

fuzz_target!(|input: Input<'_>| {
    let Ok(dimensions) = Dimensions::new(u32::from(input.width), u32::from(input.height)) else {
        return;
    };
    let limits = Limits {
        max_configuration_bytes: 8192,
        max_access_unit_bytes: 16384,
        max_width: 4096,
        max_height: 4096,
        max_nals: 64,
    };
    let Ok(config) = Config::from_parameter_sets(
        input.vps, input.sps, input.pps, input.hvcc, dimensions, limits,
    ) else {
        return;
    };
    if let (Ok(mut stream), Ok(unit)) = (
        Stream::new(&config, Default::default()),
        AccessUnit::copy_from(input.sample, input.sync, limits),
    ) {
        let _ = stream.validate(&unit, &config);
        let _ = stream.finish(1);
    }
});
