//! The Opus identification header and packet layer on arbitrary bytes: the
//! first two bytes set how many of the next bytes are the configuration
//! (`OpusHead` or a `dOps` body); the rest is one sample, read as a single
//! packet, as a self-delimited one, and as the head's streams.
#![no_main]

use coreav_opus::{Head, Packet, for_each_stream};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&[a, b], rest)) = data.split_first_chunk::<2>() else {
        return;
    };
    let split = usize::from(u16::from_le_bytes([a, b]));
    let Some((config, sample)) = rest.split_at_checked(split) else {
        return;
    };
    if let Ok(packet) = Packet::parse(sample) {
        assert!(packet.samples() <= 5760);
        assert!(packet.frames().iter().all(|f| f.len() <= 1275));
    }
    if let Ok((_, used)) = Packet::parse_self_delimited(sample) {
        assert!(used <= sample.len());
    }
    if let Ok(head) = Head::from_config(config) {
        let layout = head.layout();
        for channel in 0..usize::from(head.channels()) {
            assert!(layout.opus_channel(channel) < usize::from(head.channels()));
            assert!(head.source(channel).is_some());
        }
        let _ = for_each_stream(sample, usize::from(head.streams()), |_, _| Ok(()));
    }
});
