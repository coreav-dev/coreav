//! Arbitrary options and input through the `ffprobe` operation, read
//! through a source that may fail at any read, as the WebAssembly bindings'
//! JavaScript source may. Nothing panics; a clean run gives the same result
//! every time; a run whose injected failure never fires gives exactly the
//! result of a clean run; a failure is a `coreav.error/1` object; a
//! listing is JSON with a `format` and `streams`. A read failure in an
//! optional part of the listing may leave that part out instead of failing
//! the run, so a fired failure is not required to fail it.
#![no_main]

use coreav_isobmff::{BufferedSource, Error, SliceSource, Source};
use coreav_wasm::ops::{Failure, run_document};
use libfuzzer_sys::fuzz_target;
use std::io;

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

const OPTIONS: [&str; 6] = [
    "{}",
    r#"{"filename": "movie.mp4"}"#,
    r#"{"filename": ""}"#,
    r#"{"limits": {"maxTracks": 1}}"#,
    r#"{"limits": {"maxTableBytes": 4096, "maxPacketBytes": 64}}"#,
    r#"{"filename": "-", "limits": {"maxSampleWork": 16, "maxTotalBoxes": 8}}"#,
];

/// Sample work and table bytes per run, as the other targets bound them, so
/// that one input stays well within the fuzzer's timeout under the
/// sanitizer (the library defaults allow seconds of work by design).
const FUZZ_LIMITS: [(&str, u64); 2] = [("maxSampleWork", 1 << 20), ("maxTableBytes", 1 << 20)];

/// The options with `limits` lowered to [`FUZZ_LIMITS`] (a lower value in
/// the options stays; a missing or null `limits` gets them). Options that
/// are not a JSON object, or whose `limits` is another non-object, are left
/// as they are: they fail as usage errors.
fn bounded(options: String) -> String {
    let text = if options.trim().is_empty() { "{}" } else { options.as_str() };
    let Ok(serde_json::Value::Object(mut object)) = serde_json::from_str(text) else {
        return options;
    };
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

fuzz_target!(|input: &[u8]| {
    let Some((&selector, rest)) = input.split_first() else {
        return;
    };
    let Some((&failure, rest)) = rest.split_first() else {
        return;
    };
    let (options, data) = match selector % 8 {
        6 | 7 => {
            let cut = rest.len().min(usize::from(failure));
            let (text, data) = rest.split_at(cut);
            (String::from_utf8_lossy(text).into_owned(), data)
        }
        index => (OPTIONS[usize::from(index)].to_owned(), rest),
    };
    let options = bounded(options);
    let fail_read = (failure & 0x80 == 0 && failure != 0).then(|| usize::from(failure & 0x3f));

    let clean = run_document("ffprobe", BufferedSource::new(SliceSource::new(data)), &options);
    assert_eq!(clean, run_document("ffprobe", BufferedSource::new(SliceSource::new(data)), &options));

    let mut flaky = Flaky { data: SliceSource::new(data), reads: 0, fail_at: fail_read, failed: false };
    let result = run_document("ffprobe", BufferedSource::new(&mut flaky), &options);

    if !flaky.failed {
        assert_eq!(result, clean);
    }
    for outcome in [&clean, &result] {
        match outcome {
            Ok(listing) => {
                let value: serde_json::Value = listing.parse().expect("a listing is JSON");
                assert!(value["format"].is_object(), "{listing}");
                assert!(value["streams"].is_array(), "{listing}");
            }
            Err(Failure::Error(error)) => assert_eq!(error["schema"], "coreav.error/1", "{error}"),
            Err(Failure::Usage(message)) => assert!(message.contains("options"), "{message}"),
        }
    }
});
