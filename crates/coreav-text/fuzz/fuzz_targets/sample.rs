//! Arbitrary bytes as one text sample (docs/design/text.md): parsing never
//! panics, the text holds no NUL, the modifier counts add up to at most one
//! box per 8 bytes, and the cue text built from it is escaped and has no
//! empty line.
#![no_main]

use coreav_text::{cue_text, parse_sample};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(sample) = parse_sample(data) else {
        return;
    };
    assert!(!sample.text.contains('\0'));
    let boxes: u64 = sample.modifiers.iter().sum();
    assert!(boxes <= data.len() as u64 / 8);
    if let Some(cue) = cue_text(&sample.text) {
        assert!(!cue.is_empty());
        assert!(!cue.contains('<') && !cue.contains('>') && !cue.contains('\r'));
        assert!(cue.split('\n').all(|line| !line.trim().is_empty()));
        let rest = cue.replace("&amp;", "").replace("&lt;", "").replace("&gt;", "");
        assert!(!rest.contains('&'));
    }
});
