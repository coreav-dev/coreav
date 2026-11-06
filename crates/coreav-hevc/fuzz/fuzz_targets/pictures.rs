#![no_main]

use coreav_hevc::{AnnexB, DecodeLimits, Limits, PictureDecoder};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    let limits = DecodeLimits {
        headers: Limits {
            max_nal_bytes: 65536,
            max_width: 256,
            max_height: 256,
            max_ctus: 256,
            max_pictures: 16,
        },
        max_memory_bytes: 2 * 1024 * 1024,
        max_stream_bins: 32768,
        max_picture_bins: 16384,
    };
    let mut decoder = PictureDecoder::new(limits);
    let mut outputs = 0u64;
    for framed in AnnexB::new(data) {
        let Ok(nal) = framed else { break };
        if decoder
            .read_nal(nal, None, |_| {
                outputs += 1;
                Ok(())
            })
            .is_err()
        {
            break;
        }
    }
    let result = decoder.finish(|_| {
        outputs += 1;
        Ok(())
    });
    if let Ok(summary) = result {
        assert!(summary.memory_bytes <= limits.max_memory_bytes);
        assert!(summary.reconstruction.bins <= limits.max_stream_bins);
        assert_eq!(summary.headers.output_pictures, outputs);
        assert_eq!(
            summary.decoded_pictures + summary.skipped_pictures,
            summary.headers.pictures
        );
        assert_eq!(
            summary.reconstruction.picture_hashes + summary.skipped_picture_hashes,
            summary.headers.picture_hashes
        );
        assert_eq!(
            summary.reconstruction.reserved_hashes + summary.skipped_reserved_hashes,
            summary.headers.reserved_hashes
        );
    }
    let _ = decoder.read_nal(&[], None, |_| Ok(()));
    let _ = decoder.finish(|_| Ok(()));
});
