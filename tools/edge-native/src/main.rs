//! The native build of `coreav-wasm`'s operations, the reference the
//! WebAssembly build is compared with.
//!
//! ```text
//! edge-native run OPERATION FILE [OPTIONS] [OUTPUT]
//! edge-native run-indexed OPERATION INDEX FILE [OPTIONS] [OUTPUT]
//! edge-native bench OPERATION FILE [OPTIONS] [ROUNDS] [MIN_MS]
//! ```
//!
//! `run` reads FILE through a buffered file source, as `coreav` does, writes
//! the output of a streaming operation to OUTPUT (discarded when absent),
//! and prints the document on stdout (exit 0) or the failure's JSON, the
//! text the WebAssembly module rejects with (exit 1). `run-indexed` does the
//! same for an operation that reads an index (`hlsGet`), with the index
//! read from the file INDEX. `bench` loads FILE into
//! memory and prints one JSON line: a discarded warm-up batch, then ROUNDS
//! (default 5) batches of calls lasting at least MIN_MS (default 50) ms each,
//! as milliseconds per call. The output is delivered as the WebAssembly
//! build delivers it: through a buffer of `ops::WRITE_CHUNK` bytes, each
//! chunk copied into a new buffer that the consumer drops.

use coreav_isobmff::{BufferedSource, FileSource, SliceSource, Source};
use coreav_wasm::ops::{self, Failure, Operation};
use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    let result = match (arg(0), arg(1), arg(2)) {
        (Some("run"), Some(operation), Some(file)) => run(operation, file, arg(3), arg(4)),
        (Some("run-indexed"), Some(operation), Some(index)) => match arg(3) {
            Some(file) => run_indexed(operation, index, file, arg(4), arg(5)),
            None => Err("usage: edge-native run-indexed OPERATION INDEX FILE ...".to_owned()),
        },
        (Some("bench"), Some(operation), Some(file)) => {
            bench(operation, file, arg(3), arg(4), arg(5))
        }
        _ => Err("usage: edge-native run|run-indexed|bench OPERATION ...".to_owned()),
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            let _ = writeln!(io::stderr(), "error: {message}");
            ExitCode::from(2)
        }
    }
}

fn operation(name: &str) -> Result<Operation, String> {
    Operation::from_name(name).ok_or_else(|| format!("unknown operation {name}"))
}

/// One call, writing to `out` when the operation writes an output.
fn call<S: Source, W: Write>(
    operation: Operation,
    name: &str,
    source: S,
    options: &str,
    out: W,
) -> Result<String, Failure> {
    if operation.writes_output() {
        ops::run_stream(name, source, options, out)
    } else {
        ops::run_document(name, source, options)
    }
}

fn run(
    name: &str,
    file: &str,
    options: Option<&str>,
    output: Option<&str>,
) -> Result<ExitCode, String> {
    let operation = operation(name)?;
    let source = BufferedSource::new(FileSource::open(file).map_err(|e| e.to_string())?);
    let options = options.unwrap_or("{}");
    let result = match output {
        Some(path) => {
            let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
            call(operation, name, source, options, io::BufWriter::new(file))
        }
        None => call(operation, name, source, options, io::sink()),
    };
    print_result(result)
}

fn run_indexed(
    name: &str,
    index: &str,
    file: &str,
    options: Option<&str>,
    output: Option<&str>,
) -> Result<ExitCode, String> {
    let index = std::fs::read(index).map_err(|e| e.to_string())?;
    let source = BufferedSource::new(FileSource::open(file).map_err(|e| e.to_string())?);
    let options = options.unwrap_or("{}");
    let result = match output {
        Some(path) => {
            let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
            ops::run_indexed(name, &index, source, options, io::BufWriter::new(file))
        }
        None => ops::run_indexed(name, &index, source, options, io::sink()),
    };
    print_result(result)
}

/// Prints the document (exit 0) or the failure's JSON (exit 1).
fn print_result(result: Result<String, Failure>) -> Result<ExitCode, String> {
    let mut stdout = io::stdout().lock();
    match result {
        Ok(document) => {
            writeln!(stdout, "{document}").map_err(|e| e.to_string())?;
            Ok(ExitCode::SUCCESS)
        }
        Err(failure) => {
            writeln!(stdout, "{}", failure.to_json()).map_err(|e| e.to_string())?;
            Ok(ExitCode::from(1))
        }
    }
}

/// Hands each chunk to a consumer as a new buffer, as the package does with
/// the module's output (a copy into a fresh `Uint8Array` per chunk, which
/// the stream's reader takes and drops); counts bytes.
struct Delivered(u64);

impl Write for Delivered {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let chunk = std::hint::black_box(buf.to_vec());
        self.0 = self.0.saturating_add(chunk.len() as u64);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn bench(
    name: &str,
    file: &str,
    options: Option<&str>,
    rounds: Option<&str>,
    min_ms: Option<&str>,
) -> Result<ExitCode, String> {
    let operation = operation(name)?;
    let data = std::fs::read(file).map_err(|e| e.to_string())?;
    let options = options.unwrap_or("{}");
    let rounds: usize = rounds.unwrap_or("5").parse().map_err(|_| "ROUNDS")?;
    let min_ms: f64 = min_ms.unwrap_or("50").parse().map_err(|_| "MIN_MS")?;
    let once = || {
        let mut sink = Delivered(0);
        let out = io::BufWriter::with_capacity(ops::WRITE_CHUNK, &mut sink);
        let ok = call(operation, name, SliceSource::new(&data), options, out).is_ok();
        (ok, sink.0)
    };
    let (ok, bytes) = once();
    let batch = || {
        let start = Instant::now();
        let mut calls = 0u64;
        loop {
            std::hint::black_box(once());
            calls += 1;
            let elapsed = start.elapsed().as_secs_f64() * 1e3;
            if elapsed >= min_ms {
                #[allow(clippy::cast_precision_loss)]
                return (elapsed / calls as f64, calls);
            }
        }
    };
    batch();
    let mut per_call = Vec::with_capacity(rounds);
    let mut calls = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let (ms, n) = batch();
        per_call.push(format!("{ms:.6}"));
        calls.push(n.to_string());
    }
    println!(
        "{{\"ok\":{ok},\"output_bytes\":{bytes},\"ms_per_call\":[{}],\"calls\":[{}]}}",
        per_call.join(","),
        calls.join(",")
    );
    Ok(ExitCode::SUCCESS)
}
