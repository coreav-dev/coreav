//! A stream of arbitrary bytes read as the worker's messages by the parent
//! (and as the parent's by the worker), under the size limits of
//! `protocol::read_message`. Every message decodes or fails, never panics;
//! a length over the limit is refused before memory is reserved for it.
#![no_main]

use coreav_worker::protocol::{Incoming, ParentMessage, WorkerMessage, read_message};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut input = data;
    for _ in 0..64 {
        match read_message(&mut input, |kind| WorkerMessage::max_len(kind, 1 << 16)) {
            Ok(Incoming::Message(kind, payload)) => {
                let _ = WorkerMessage::decode(kind, &payload);
                let _ = ParentMessage::decode(kind & 0x7F, &payload);
            }
            Ok(Incoming::End) | Err(_) => break,
        }
    }
});
