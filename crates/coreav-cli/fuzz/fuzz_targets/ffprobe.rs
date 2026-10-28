//! `coreav ffprobe` on any bytes: container detection, the three builders
//! (the parameter sets, sample entries and tags they read included), the
//! packet listing and every writer, as a file the command was given.
#![no_main]

use coreav_cli::ffprobe::{self, args};
use coreav_isobmff::SliceSource;
use libfuzzer_sys::fuzz_target;

const WRITERS: [&str; 5] = ["default", "compact", "csv", "flat", "json=c=1"];

fuzz_target!(|data: &[u8]| {
    let writer = WRITERS
        .get(usize::from(data.first().copied().unwrap_or(0)) % WRITERS.len())
        .copied()
        .unwrap_or("json");
    let argv: Vec<String> = [
        "-show_format",
        "-show_streams",
        "-show_chapters",
        "-show_programs",
        "-show_packets",
        "-pretty",
        "-of",
        writer,
        "fuzz",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let Ok(options) = args::parse(&argv) else {
        return;
    };
    let mut source = SliceSource::new(data);
    let size = data.len() as u64;
    if let Ok(mut probed) = ffprobe::probe(&mut source, size, "fuzz", None) {
        let mut out = Vec::new();
        let _ = ffprobe::emit(&options, &mut probed, &mut out);
    }
});
