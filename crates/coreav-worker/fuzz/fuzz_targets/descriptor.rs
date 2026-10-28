//! A frame message from a hostile worker through the parent's checks and
//! plane copies (`docs/design/decode.md`, "Descriptors"): arbitrary bytes
//! as the payload of a `Frame` message, against shared memory of an
//! arbitrary slot layout, possibly shorter than the layout claims. Every result is a frame within
//! the limits or an error, never a panic or a read outside the memory.
#![no_main]

use coreav_decode::Limits;
use coreav_worker::descriptor::{SlotMemory, Slots, accept};
use coreav_worker::shm::ParentMemory;
use coreav_worker::protocol::WorkerMessage;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let [count, s0, s1, m0, m1, rest @ ..] = data else {
        return;
    };
    let size = u64::from(u16::from_le_bytes([*s0, *s1]));
    let len = usize::from(u16::from_le_bytes([*m0, *m1]));
    let plain = vec![0x5Au8; len];
    // The high bit of `count` picks a real sealed memory file, so that the
    // parent's pointer copy out of its mapping is fuzzed too.
    let shared = (*count & 0x80 != 0 && len > 0)
        .then(|| ParentMemory::create(len as u64).ok())
        .flatten();
    let memory: &dyn SlotMemory = match &shared {
        Some(m) => m,
        None => &plain,
    };
    let count = *count & 0x7F;
    let slots = (count > 0).then_some(Slots {
        memory,
        count: u32::from(count),
        size,
    });
    let limits = Limits {
        max_width: 4096,
        max_height: 4096,
        max_pixels: 1 << 22,
        max_frame_bytes: 1 << 22,
        ..Limits::default()
    };
    if let Ok(WorkerMessage::Frame(descriptor, inline)) = WorkerMessage::decode(0x82, rest) {
        for seq in [descriptor.seq, 0] {
            if let Ok(accepted) = accept(&descriptor, &inline, seq, slots, &limits) {
                assert!(accepted.bytes <= limits.max_frame_bytes);
                assert!(accepted.frame.width() <= limits.max_width);
            }
        }
    }
});
