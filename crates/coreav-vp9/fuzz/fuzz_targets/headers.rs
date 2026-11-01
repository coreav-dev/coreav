//! Arbitrary bytes as a stream of packets through the header reader: each
//! packet is split into its frames and every frame's uncompressed header
//! read, with the reference slots carried from packet to packet. The result
//! is headers or errors, never a panic or an unbounded allocation.
//!
//! Each packet is a 2-byte little-endian length and that many bytes.
#![no_main]

use coreav_vp9::{Decoder, Header, Limits, split_superframe};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut decoder = Decoder::new(Limits::for_size(512, 512));
    let mut rest = data;
    while let Some((len, after)) = rest.split_first_chunk::<2>() {
        let len = usize::from(u16::from_le_bytes(*len)).min(after.len());
        let (packet, after) = after.split_at(len);
        rest = after;
        if let Ok(frames) = split_superframe(packet) {
            assert!(frames.iter().map(|f| f.len()).sum::<usize>() <= packet.len());
        }
        let Ok(headers) = decoder.read_headers(packet) else {
            continue;
        };
        for header in headers {
            if let Header::Frame(h) = header {
                assert!(h.width <= 512 && h.height <= 512);
                assert!(
                    h.uncompressed_size + usize::from(h.compressed_header_size) <= packet.len()
                );
            }
        }
    }
});
