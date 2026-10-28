//! What a hostile FFmpeg writes on its standard error, through the line
//! splitter and the `showinfo` parser, and pixel format names with
//! arbitrary sizes through the raw frame layout. Every line parses or is
//! refused, never panics; long lines are cut.
#![no_main]

use coreav_backend_ffmpeg::showinfo::{RawFormat, parse_line};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (infos, kept) = coreav_backend_ffmpeg::parse_messages_for_test(data);
    assert!(kept.len() <= 12);
    let _ = infos;
    let text = String::from_utf8_lossy(data);
    for line in text.lines().take(256) {
        let _ = parse_line(line);
        let mut words = line.split_whitespace();
        if let (Some(name), Some(w), Some(h)) = (words.next(), words.next(), words.next()) {
            if let (Some(format), Ok(w), Ok(h)) = (RawFormat::from_name(name), w.parse(), h.parse()) {
                let _ = format.plane_bytes(w, h);
            }
        }
    }
});
