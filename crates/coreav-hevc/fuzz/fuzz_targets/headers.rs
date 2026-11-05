#![no_main]

use coreav_hevc::{AnnexB, HeaderParser, LengthPrefixed, Limits};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_nal_bytes: 65536,
        max_width: 512,
        max_height: 512,
        max_ctus: 1024,
        max_pictures: 1024,
    };
    for width in 1..=4 {
        let mut parser = HeaderParser::new(limits);
        if let Ok(nals) = LengthPrefixed::new(data, width) {
            for framed in nals {
                let Ok(nal) = framed else { break };
                if parser.read_nal(nal, |_| Ok(())).is_err() {
                    break;
                }
            }
        }
        let _ = parser.finish(|_| Ok(()));
    }
    let mut parser = HeaderParser::new(limits);
    for framed in AnnexB::new(data) {
        let Ok(nal) = framed else { break };
        if parser.read_nal(nal, |_| Ok(())).is_err() {
            break;
        }
    }
    let _ = parser.finish(|_| Ok(()));
    let _ = HeaderParser::new(limits).read_nal(data, |_| Ok(()));
});
