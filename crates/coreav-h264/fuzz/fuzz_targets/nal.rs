//! Arbitrary bytes through the NAL unit framings: the byte-stream
//! splitter in chunks of every size, the whole-buffer iterator, the
//! length-prefixed iterator and the removal of emulation prevention
//! bytes. The two splitters must agree, and nothing may panic or grow
//! without bound.
#![no_main]

use coreav_h264::{AnnexB, Limits, NalHeader, annex_b, length_prefixed, rbsp};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&first, rest)) = data.split_first() else {
        return;
    };
    // The whole-buffer iterator is the reference for the splitter.
    let whole: Vec<Vec<u8>> = annex_b(rest).map(<[u8]>::to_vec).collect();

    // The same bytes through the chunked splitter, in chunks of 1 to 64.
    let chunk = usize::from(first % 64) + 1;
    let limits = Limits::default();
    let mut splitter = AnnexB::new(&limits);
    let mut split = Vec::new();
    let mut failed = false;
    for part in rest.chunks(chunk) {
        if splitter
            .push(part, |nal| {
                split.push(nal.to_vec());
                Ok(())
            })
            .is_err()
        {
            failed = true;
            break;
        }
    }
    if !failed {
        let _ = splitter.finish(|nal| {
            split.push(nal.to_vec());
            Ok(())
        });
        assert_eq!(split, whole, "the two splitters disagree");
    }

    for unit in &whole {
        // A header is read, or refused; either way the unit is bounded.
        if let Ok(header) = NalHeader::parse(unit) {
            let mut payload = Vec::new();
            rbsp(unit.get(1..).unwrap_or_default(), &mut payload);
            assert!(payload.len() <= unit.len());
            let _ = header.is_slice();
        }
    }

    // The length-prefixed framing over the same bytes.
    for size in 1..=4 {
        let mut total = 0usize;
        for unit in length_prefixed(rest, size) {
            match unit {
                Ok(unit) => total += unit.len(),
                Err(_) => break,
            }
        }
        assert!(total <= rest.len());
    }
});
