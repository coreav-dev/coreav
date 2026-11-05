#![no_main]

mod intra_parameters;

use coreav_hevc::{AnnexB, IntraLimits, IntraPictureDecoder, Limits};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 { return; }
    let limits = IntraLimits {
        headers: Limits { max_nal_bytes: 65536, max_width: 256, max_height: 256,
            max_ctus: 256, max_pictures: 16 },
        max_memory_bytes: 2 * 1024 * 1024,
        max_bins: 16384,
    };
    let mut decoder = IntraPictureDecoder::new(limits, 0);
    for framed in AnnexB::new(data) {
        let Ok(nal) = framed else { break };
        if decoder.read_nal(nal, |_| Ok(())).is_err() { break; }
    }
    let _ = decoder.finish(|_| Ok(()));
    for parameters in [intra_parameters::EIGHT, intra_parameters::TEN] {
        let mut decoder = IntraPictureDecoder::new(limits, 0);
        for parameter in parameters { let _ = decoder.read_nal(parameter, |_| Ok(())); }
        let mut nal = vec![38, 1, 0xaf];
        let mut zeros = 0;
        for &byte in data.iter().take(4096) {
            if zeros == 2 && byte <= 3 { nal.push(3); zeros = 0; }
            nal.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        let _ = decoder.read_nal(&nal, |_| Ok(()));
        let _ = decoder.finish(|_| Ok(()));
    }
});
