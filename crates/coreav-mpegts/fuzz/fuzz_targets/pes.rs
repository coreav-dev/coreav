//! PES packets: the header (PTS, DTS, lengths), the audio frame syntaxes
//! of the payload (ADTS, MPEG audio, AC-3 and E-AC-3, LOAS, Opus), and
//! video access units (parameter sets, configuration records, keyframes,
//! the MPEG-2 sequence header).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = coreav_mpegts::fuzzing::pes(data);
});
