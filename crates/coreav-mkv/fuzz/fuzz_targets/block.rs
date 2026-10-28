//! The sans-IO block header and lacing parser on its own: every frame it
//! returns lies inside the block.
#![no_main]

use coreav_mkv::{Parsed, parse_block};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&size, bytes)) = data.split_first() else {
        return;
    };
    let block_size = u64::from(size) * 64 + bytes.len() as u64 / 2;
    if let Ok(Parsed::Done(header)) = parse_block(bytes, block_size, 0) {
        let mut end = 0;
        for (offset, size) in header.frames {
            assert!(offset >= end);
            end = offset + size;
            assert!(end <= block_size);
        }
    }
});
