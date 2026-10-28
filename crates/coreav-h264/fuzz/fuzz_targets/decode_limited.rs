//! The same bytes through the decoder, under the limits and in the mode
//! the first byte picks, so that the paths that refuse a stream are
//! reached: a picture larger than `max_mbs`, a stream that needs more
//! references than `max_dpb_frames`, more slices than
//! `max_slices_per_picture` — in the keyframe-only mode the slices of a
//! picture it skips count too — and a NAL unit larger than
//! `max_nal_bytes`.
#![no_main]

use coreav_h264::{AnnexB, Decoder, Limits, Mode};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&choice, rest)) = data.split_first() else {
        return;
    };
    let limits = Limits {
        max_mbs: 1 + usize::from(choice) * 17,
        max_dpb_frames: 1 + usize::from(choice % 17),
        max_slices_per_picture: 1 + usize::from(choice % 8),
        max_nal_bytes: 16 + usize::from(choice) * 64,
        max_threads: 1 + usize::from(choice % 3),
        // From 64 KiB, which refuses all but the smallest pictures, to
        // 8 GiB: the memory check refuses or lowers the threads.
        max_memory_bytes: 1 << (16 + u32::from(choice % 18)),
    };
    let mode = if choice % 2 == 0 {
        Mode::EveryPicture
    } else {
        Mode::KeyframesOnly
    };
    // Asks for more threads than the limit allows, which bounds them.
    let mut decoder = Decoder::new(limits).with_mode(mode).with_threads(4);
    let mut splitter = AnnexB::new(&limits);
    let mut units = 0usize;
    let fed = splitter.push(rest, |nal| {
        units += 1;
        // A unit the splitter hands out is inside the limit it was built
        // with.
        assert!(nal.len() <= limits.max_nal_bytes);
        let _ = decoder.decode_nal(nal);
        Ok(())
    });
    if fed.is_ok() {
        let _ = splitter.finish(|nal| {
            let _ = decoder.decode_nal(nal);
            Ok(())
        });
    }
    let _ = decoder.flush();
    for picture in decoder.take_pictures() {
        // No picture may exceed the limit it was decoded under.
        let macroblocks = picture.width().div_ceil(16) * picture.height().div_ceil(16);
        assert!(macroblocks <= limits.max_mbs);
    }
});
