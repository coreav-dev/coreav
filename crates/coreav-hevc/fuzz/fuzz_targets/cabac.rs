#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if data.len() <= 4096 {
        assert!(coreav_hevc::cabac_fast_enabled());
        assert!(coreav_hevc::cabac_engines_match_reference(data));
    }
});
