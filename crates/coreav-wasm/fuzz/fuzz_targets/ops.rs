//! Arbitrary options and input through the operations layer, read through
//! a source that may fail at any read and written to a sink that may fail
//! at any write, as the WebAssembly bindings' JavaScript source and sink
//! may. Nothing panics; a run whose injected failure never fires gives
//! exactly the result of a clean run; a run whose failure fires fails with
//! an `io` error object; a streaming summary counts the bytes written.
//! `hlsGet` reads an index built from the input, or the input itself as a
//! damaged index.
#![no_main]

use coreav_isobmff::{BufferedSource, Error, SliceSource, Source};
use coreav_wasm::ops::{Failure, run_document, run_indexed, run_stream};
use libfuzzer_sys::fuzz_target;
use std::io::{self, Write};

/// A source over bytes whose `fail_at`-th read fails.
struct Flaky<'a> {
    data: SliceSource<'a>,
    reads: usize,
    fail_at: Option<usize>,
    failed: bool,
}

impl Source for Flaky<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        self.data.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.reads += 1;
        if self.fail_at == Some(self.reads) {
            self.failed = true;
            return Err(Error::Io(io::ErrorKind::Other));
        }
        self.data.read_exact_at(offset, buf)
    }
}

/// A sink whose `fail_at`-th write fails.
struct Sink {
    bytes: Vec<u8>,
    writes: usize,
    fail_at: Option<usize>,
    failed: bool,
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        if self.fail_at == Some(self.writes) {
            self.failed = true;
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const OPTIONS: [&str; 8] = [
    "{}",
    r#"{"format": "ffprobe-compat", "scanFragments": true}"#,
    r#"{"start": "0", "end": "1"}"#,
    r#"{"start": "0.5", "audio": "none", "limits": {"maxTableBytes": 4096}}"#,
    r#"{"stream": 1, "strict": true}"#,
    r#"{"allow": ["h264", "aac"]}"#,
    r#"{"scanFrameSizes": true}"#,
    r#"{"policy": {"schema": "coreav.policy/1", "streams": {"video": {"max_width": 1920, "max_height": 1080}}}}"#,
];

/// The operations, and whether each takes `limits`; documents are probe,
/// check and chapters.
const OPERATIONS: [(&str, bool); 10] = [
    ("probe", true),
    ("extractAudio", true),
    ("clip", true),
    ("check", true),
    ("chapters", true),
    ("extractSubtitles", true),
    ("remux", true),
    ("sanitize", true),
    ("hlsIndex", true),
    ("hlsGet", true),
];
const DOCUMENTS: [&str; 3] = ["probe", "check", "chapters"];

/// Sample work and table bytes per run, as the other targets bound them, so
/// that one input stays well within the fuzzer's timeout under the
/// sanitizer (the library defaults allow seconds of work by design).
const FUZZ_LIMITS: [(&str, u64); 2] = [("maxSampleWork", 1 << 20), ("maxTableBytes", 1 << 20)];

/// The options with `limits` lowered to [`FUZZ_LIMITS`] (a lower value in
/// the options stays; a missing or null `limits` gets them). Options that
/// are not a JSON object, or whose `limits` is another non-object, are left
/// as they are: they fail as usage errors.
fn bounded(options: String, takes_limits: bool) -> String {
    if !takes_limits {
        return options;
    }
    // Blank options are `{}` to the operations layer (`ops::parse`), so
    // they get the bounds too.
    let text = if options.trim().is_empty() { "{}" } else { options.as_str() };
    let Ok(serde_json::Value::Object(mut object)) = serde_json::from_str(text) else {
        return options;
    };
    // A missing or null `limits` means no overrides.
    let limits = object.entry("limits").or_insert(serde_json::Value::Null);
    if limits.is_null() {
        *limits = serde_json::Value::Object(serde_json::Map::new());
    }
    let Some(limits) = limits.as_object_mut() else {
        return options;
    };
    for (key, cap) in FUZZ_LIMITS {
        let value = limits.get(key).and_then(serde_json::Value::as_u64).map_or(cap, |v| v.min(cap));
        limits.insert(key.to_owned(), value.into());
    }
    serde_json::Value::Object(object).to_string()
}

/// The resources `hlsGet` asks for.
const RESOURCES: [&str; 8] = [
    "master.m3u8",
    "video.m3u8",
    "audio.m3u8",
    "video-init.mp4",
    "audio-init.mp4",
    "video-0.m4s",
    "video-1.m4s",
    "audio-0.m4s",
];

fn run(
    name: &str,
    index: &[u8],
    source: impl Source,
    options: &str,
    sink: &mut Sink,
) -> Result<String, Failure> {
    let source = BufferedSource::new(source);
    match name {
        _ if DOCUMENTS.contains(&name) => run_document(name, source, options),
        "hlsGet" => run_indexed(name, index, source, options, &mut *sink),
        _ => run_stream(name, source, options, &mut *sink),
    }
}

/// The index of `data` at 1 s segments, or `data` itself when none can be
/// built (or when `raw`): a damaged index for the decoder.
fn index_of(data: &[u8], raw: bool) -> Vec<u8> {
    if !raw {
        let mut sink = Sink { bytes: Vec::new(), writes: 0, fail_at: None, failed: false };
        let options = bounded(r#"{"segmentDuration": "1"}"#.to_owned(), true);
        if run_stream("hlsIndex", BufferedSource::new(SliceSource::new(data)), &options, &mut sink).is_ok() {
            return sink.bytes;
        }
    }
    data.to_vec()
}

fuzz_target!(|input: &[u8]| {
    let Some((&selector, rest)) = input.split_first() else {
        return;
    };
    let Some((&failure, rest)) = rest.split_first() else {
        return;
    };
    let (name, takes_limits) = OPERATIONS[usize::from(selector % 10)];
    // Options: a fixed set, or arbitrary text taken from the input's head;
    // for `hlsGet`, a resource name.
    let (options, data) = match selector / 10 % 10 {
        _ if name == "hlsGet" => {
            let resource = RESOURCES[usize::from(selector / 10 % 8)];
            (format!(r#"{{"name": "{resource}"}}"#), rest)
        }
        8 | 9 => {
            let cut = rest.len().min(usize::from(failure));
            let (text, data) = rest.split_at(cut);
            (String::from_utf8_lossy(text).into_owned(), data)
        }
        index => (OPTIONS[usize::from(index)].to_owned(), rest),
    };
    let index = if name == "hlsGet" { index_of(data, selector >= 128) } else { Vec::new() };
    let options = bounded(options, takes_limits);
    let fail_read = (failure & 0x80 == 0 && failure != 0).then(|| usize::from(failure & 0x3f));
    let fail_write = (failure & 0xc0 == 0xc0).then(|| usize::from(failure & 0x3f) + 1);

    let mut clean_sink = Sink { bytes: Vec::new(), writes: 0, fail_at: None, failed: false };
    let clean = run(name, &index, SliceSource::new(data), &options, &mut clean_sink);

    let mut sink = Sink { bytes: Vec::new(), writes: 0, fail_at: fail_write, failed: false };
    let mut flaky = Flaky { data: SliceSource::new(data), reads: 0, fail_at: fail_read, failed: false };
    let result = run(name, &index, &mut flaky, &options, &mut sink);

    if flaky.failed || sink.failed {
        let Err(Failure::Error(error)) = &result else {
            panic!("an injected failure did not fail the run: {result:?}");
        };
        assert_eq!(error["kind"], "io", "{error}");
        assert_eq!(error["schema"], "coreav.error/1");
    } else {
        assert_eq!(result, clean);
        assert_eq!(sink.bytes, clean_sink.bytes);
    }
    if let Ok(summary) = &clean {
        let value: serde_json::Value = summary.parse().expect("a summary is JSON");
        match name {
            _ if DOCUMENTS.contains(&name) => {}
            "hlsIndex" => assert_eq!(value["index"]["bytes"], clean_sink.bytes.len()),
            "hlsGet" => assert_eq!(value["bytes"], clean_sink.bytes.len()),
            _ => assert_eq!(value["output"]["bytes"], clean_sink.bytes.len()),
        }
    }
});
