//! PSI sections: reassembly across packet payloads (pointer fields,
//! several sections per payload, sections across payloads), the CRC-32,
//! and the PAT and PMT with their descriptor loops.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = coreav_mpegts::fuzzing::sections(data);
});
