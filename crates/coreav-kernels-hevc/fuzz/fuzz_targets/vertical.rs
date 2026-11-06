#![no_main]
mod interpolation;
libfuzzer_sys::fuzz_target!(|data: &[u8]| interpolation::compare(data, true));
