//! CoreAV's side of the packet-listing speed bar of `docs/design/mpegts.md`
//! ("Speed bars, round 2"): `ts-speed packets FILE`.
//!
//! Times, inside the process, the work the bar names: opening the file and
//! the probe, then opening it again and listing every access unit of every
//! kept stream, with the library's default limits and allowlist. Prints one
//! JSON line `{"ok", "ns", "count", "bytes"}` (`"error"` in place of the
//! counts when the listing fails), as `lav_ts.c` does for libavformat; the
//! exit status is 0 then too, and 2 on a usage error.

use coreav_mpegts::{Allowlist, BufferedSource, Error, FileSource, Limits, probe};
use serde_json::json;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some("packets"), Some(path), None) =
        (args.get(1).map(String::as_str), args.get(2), args.get(3))
    else {
        eprintln!("usage: ts-speed packets FILE");
        return ExitCode::from(2);
    };
    let started = Instant::now();
    let listed = list(path);
    let ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let line = match listed {
        Ok((count, bytes)) => json!({"ok": true, "ns": ns, "count": count, "bytes": bytes}),
        Err(error) => json!({"ok": false, "ns": ns, "error": error.to_string()}),
    };
    println!("{line}");
    ExitCode::SUCCESS
}

/// The access units listed and their bytes.
fn list(path: &str) -> Result<(u64, u64), Error> {
    let limits = Limits::default();
    let source = BufferedSource::new(FileSource::open(path)?);
    let stream = probe(source, &limits, &Allowlist::default())?;
    let mut count = 0u64;
    let mut bytes = 0u64;
    for packet in stream.packets(BufferedSource::new(FileSource::open(path)?), &limits) {
        count += 1;
        bytes += u64::try_from(packet?.data.len()).unwrap_or(u64::MAX);
    }
    Ok((count, bytes))
}
