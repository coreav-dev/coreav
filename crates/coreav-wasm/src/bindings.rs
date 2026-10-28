//! The `wasm-bindgen` exports and the JavaScript source and sink they read
//! and write through. Built for `wasm32` only.
//!
//! The JavaScript side is trusted to
//! implement the two interfaces below; what it returns is still checked:
//! every read must return exactly the bytes asked for, and the size must be
//! an integer that a JavaScript number holds exactly.

// `suspending` and `jspi` are marked experimental in `wasm-bindgen` 0.2.129
// and warn through a deprecated item that the macros generate at module
// level, so an allowance on the items does not reach it; this module holds
// only the bindings. The version is pinned in Cargo.toml.
#![allow(deprecated)]

use crate::ops;
use coreav_isobmff::{BufferedSource, Error, Source};
use std::io::{self, BufWriter, ErrorKind, Write};
use wasm_bindgen::prelude::*;

/// Largest offset or size a JavaScript number holds exactly (2^53).
const MAX_EXACT: u64 = 1 << 53;

#[wasm_bindgen]
extern "C" {
    /// A byte source of known size (`engine.js`).
    pub type ByteSource;

    /// Copies the bytes at `offset` into `buf`, whose length is the count,
    /// if the source holds them now; returns the count copied, or 0. An
    /// in-memory source always holds them; a remote source holds what it
    /// has received and not dropped, and otherwise the module calls
    /// [`read`](ByteSource::read). Never throws (`engine.js` catches), so
    /// the call needs no exception wrapper.
    #[wasm_bindgen(method, js_name = readInto)]
    fn read_into(this: &ByteSource, offset: f64, buf: &mut [u8]) -> u32;

    /// Remote sources: the `length` bytes at `offset`. The module suspends
    /// until the promise settles.
    #[wasm_bindgen(method, suspending, catch)]
    fn read(this: &ByteSource, offset: f64, length: u32) -> Result<Vec<u8>, JsValue>;

    /// Remote sources: advice that reads of about `length` bytes from
    /// `offset` follow ([`Source::will_read`]). Never throws (`engine.js`
    /// catches).
    #[wasm_bindgen(method)]
    fn plan(this: &ByteSource, offset: f64, length: f64);

    /// Remote sources: advice that the reads that follow touch only these
    /// regions, as offset and length pairs ([`Source::will_read_regions`]).
    /// Never throws (`engine.js` catches).
    #[wasm_bindgen(method, js_name = planRegions)]
    fn plan_regions(this: &ByteSource, regions: &[f64]);

    /// An output sink (`engine.js`).
    pub type ByteSink;

    /// Takes a copy of `chunk` before it returns; the module suspends until
    /// the promise settles (backpressure).
    #[wasm_bindgen(method, suspending, catch)]
    fn write(this: &ByteSink, chunk: &[u8]) -> Result<(), JsValue>;
}

/// A [`Source`] over a [`ByteSource`].
struct Js<'a> {
    source: &'a ByteSource,
    size: u64,
    in_memory: bool,
}

impl Source for Js<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.size)
    }

    fn will_read(&mut self, offset: u64, len: u64) {
        if self.in_memory || offset >= self.size {
            return;
        }
        // Both below 2^53 once clamped to the size: exact as numbers.
        #[allow(clippy::cast_precision_loss)]
        let (at, length) = (offset as f64, len.min(self.size - offset) as f64);
        self.source.plan(at, length);
    }

    fn wants_regions(&mut self) -> bool {
        !self.in_memory
    }

    fn will_read_regions(&mut self, regions: &[(u64, u64)]) {
        if self.in_memory {
            return;
        }
        let mut flat = Vec::new();
        if flat.try_reserve(regions.len().saturating_mul(2)).is_err() {
            return;
        }
        for &(offset, len) in regions {
            if offset >= self.size {
                continue;
            }
            // Both below 2^53 once clamped to the size: exact as numbers.
            #[allow(clippy::cast_precision_loss)]
            flat.extend([offset as f64, len.min(self.size - offset) as f64]);
        }
        self.source.plan_regions(&flat);
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        offset
            .checked_add(buf.len() as u64)
            .filter(|end| *end <= self.size)
            .ok_or(Error::Truncated { offset })?;
        if buf.is_empty() {
            return Ok(());
        }
        let length = u32::try_from(buf.len()).map_err(|_| Error::Truncated { offset })?;
        // Exact: offsets are below the size, which is at most 2^53.
        #[allow(clippy::cast_precision_loss)]
        let at = offset as f64;
        // A failed or short read is an I/O error; the package keeps the
        // JavaScript error and attaches it as the cause.
        let failed = |_| Error::Io(ErrorKind::Other);
        // The bytes may be here already: no suspension, no allocation.
        let copied = self.source.read_into(at, buf);
        if copied == length {
            return Ok(());
        }
        if self.in_memory {
            return Err(Error::Io(ErrorKind::Other));
        }
        let data = self.source.read(at, length).map_err(failed)?;
        let bytes = data
            .get(..buf.len())
            .filter(|_| data.len() == buf.len())
            .ok_or(Error::Io(ErrorKind::UnexpectedEof))?;
        buf.copy_from_slice(bytes);
        Ok(())
    }
}

/// A [`Write`] over a [`ByteSink`].
struct Sink<'a>(&'a ByteSink);

impl Write for Sink<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.0
            .write(buf)
            .map_err(|_| io::Error::from(ErrorKind::BrokenPipe))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn size(value: f64) -> Result<u64, String> {
    // `as` saturates; the round trip rejects fractions, NaN and infinities.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let size = value as u64;
    #[allow(clippy::cast_precision_loss)]
    let exact = size as f64 == value;
    if exact && size <= MAX_EXACT {
        Ok(size)
    } else {
        Err(ops::Failure::Usage("size must be an integer from 0 to 2^53".to_owned()).to_json())
    }
}

/// Every source is read through a [`BufferedSource`], as `coreav` reads
/// files: small reads are served from one window of at most 1 MiB and
/// reads of 64 KiB or more go to the source directly. A remote source thus
/// sees few, mostly forward reads (the package keeps recent bytes itself),
/// and an in-memory source few calls across the boundary.
fn with_source<T>(
    source: &ByteSource,
    size: u64,
    in_memory: bool,
    run: impl FnOnce(&mut dyn Source) -> T,
) -> T {
    let js = Js {
        source,
        size,
        in_memory,
    };
    let mut buffered = BufferedSource::new(js);
    run(&mut buffered)
}

/// Runs an operation that returns a document (`probe`) over an in-memory
/// source, synchronously: nothing can suspend, so the call skips JSPI.
/// Returns the document's JSON text; throws the JSON text of a usage error
/// (`{"usage": ...}`) or of a `coreav.error/1` object.
#[wasm_bindgen(js_name = runDocumentInMemory)]
pub fn run_document_in_memory(
    operation: &str,
    source: &ByteSource,
    size_value: f64,
    options: &str,
) -> Result<String, String> {
    let size = size(size_value)?;
    with_source(source, size, true, |source| {
        ops::run_document(operation, source, options)
    })
    .map_err(|failure| failure.to_json())
}

/// Runs an operation that returns a document (`probe`). Resolves to the
/// document's JSON text; rejects as [`run_document_in_memory`] throws.
#[wasm_bindgen(jspi, js_name = runDocument)]
pub fn run_document(
    operation: &str,
    source: &ByteSource,
    size_value: f64,
    in_memory: bool,
    options: &str,
) -> Result<String, String> {
    let size = size(size_value)?;
    with_source(source, size, in_memory, |source| {
        ops::run_document(operation, source, options)
    })
    .map_err(|failure| failure.to_json())
}

/// Runs an operation that writes an output to `sink` in chunks of at most
/// [`ops::WRITE_CHUNK`] bytes and resolves to its summary's JSON text after the last chunk;
/// rejects as [`run_document`] does.
#[wasm_bindgen(jspi, js_name = runStream)]
pub fn run_stream(
    operation: &str,
    source: &ByteSource,
    size_value: f64,
    in_memory: bool,
    sink: &ByteSink,
    options: &str,
) -> Result<String, String> {
    let size = size(size_value)?;
    let mut out = BufWriter::with_capacity(ops::WRITE_CHUNK, Sink(sink));
    let result = with_source(source, size, in_memory, |source| {
        ops::run_stream(operation, source, options, &mut out)
    });
    if result.is_err() {
        // Dropping a `BufWriter` writes out what it holds: a failure before
        // the first chunk would then reach the sink as output, and the
        // package would answer with a broken body instead of the error.
        let _unwritten = out.into_parts();
    }
    result.map_err(|failure| failure.to_json())
}

/// Runs an operation that reads an index besides the source (`hlsGet`) and
/// writes an output to `sink`, as [`run_stream`] does. `index` is copied
/// into the module's memory for the call; it is read as untrusted input.
#[allow(clippy::too_many_arguments)]
#[wasm_bindgen(jspi, js_name = runIndexedStream)]
pub fn run_indexed_stream(
    operation: &str,
    index: &[u8],
    source: &ByteSource,
    size_value: f64,
    in_memory: bool,
    sink: &ByteSink,
    options: &str,
) -> Result<String, String> {
    let size = size(size_value)?;
    let mut out = BufWriter::with_capacity(ops::WRITE_CHUNK, Sink(sink));
    let result = with_source(source, size, in_memory, |source| {
        ops::run_indexed(operation, index, source, options, &mut out)
    });
    if result.is_err() {
        // As in `run_stream`: no output after a failure.
        let _unwritten = out.into_parts();
    }
    result.map_err(|failure| failure.to_json())
}

/// The crate version, for the package's diagnostics.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
