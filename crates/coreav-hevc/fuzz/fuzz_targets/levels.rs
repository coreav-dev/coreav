#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if data.len() <= 4099 {
        assert!(coreav_hevc::coefficient_levels_match_reference(data));
    }
});
