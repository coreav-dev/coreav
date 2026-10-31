//! A hostile Opus encoder process's output through the parent's reader
//! (`docs/design/transcode.md`, "Opus audio"): arbitrary bytes, split into
//! pieces at arbitrary points. Every packet handed on is within the
//! length limit and codes one 20 ms frame, and the result is the packets or
//! an error, never a panic.
#![no_main]

use coreav_worker::opus::{FRAME_SAMPLES, MAX_PACKET_BYTES, PacketReader, ReadError, packet_samples};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&step, data)) = data.split_first() else {
        return;
    };
    let mut reader = PacketReader::new();
    let mut handed = 0u64;
    for piece in data.chunks(usize::from(step).max(1)) {
        let pushed = reader.push::<ReadError>(piece, &mut |packet| {
            assert!(!packet.is_empty() && packet.len() <= MAX_PACKET_BYTES);
            assert_eq!(packet_samples(packet), Ok(FRAME_SAMPLES));
            handed += 1;
            Ok(())
        });
        if pushed.is_err() {
            return;
        }
    }
    assert_eq!(reader.packets(), handed);
    let _ = reader.finish();
    let _ = packet_samples(data);
});
