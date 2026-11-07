#![doc = "Decoder-only scalar HEVC reference with separate pixel checks."]

use coreav_hevc::{DecodeLimits, Error, PictureDecoder, Result};
use serde_json::json;
use std::io::{Read, Write};

mod common;
#[path = "common/decoded.rs"]
mod decoded;

fn run(writer: &mut impl Write) -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or(Error::InvalidData("reference_arguments"))?;
    let mode = args
        .next()
        .ok_or(Error::InvalidData("reference_arguments"))?;
    let threads = args
        .next()
        .map(|value| {
            value
                .into_string()
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
        })
        .unwrap_or(Some(1))
        .filter(|value| *value <= 16)
        .ok_or(Error::InvalidData("reference_arguments"))?;
    let threads = if threads == 0 {
        std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(16)
    } else {
        threads
    };
    if args.next().is_some() || (mode != "check" && mode != "decode") {
        return Err(Error::InvalidData("reference_arguments"));
    }
    let source = std::fs::File::open(path).map_err(|_| Error::InvalidData("input_open"))?;
    let mut bytes = Vec::new();
    source
        .take(64 << 20)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Limit("reference_input"))?;
    if bytes.len() >= 64 << 20 {
        return Err(Error::Limit("reference_input"));
    }
    let limits = DecodeLimits {
        max_memory_bytes: usize::try_from(coreav_decode::Limits::default().max_memory_bytes)
            .map_err(|_| Error::Limit("reference_memory"))?,
        ..DecodeLimits::default()
    };
    let mut decoder = PictureDecoder::new(limits);
    #[cfg(not(hevc_serial_baseline))]
    {
        decoder = decoder.with_threads(threads);
    }
    #[cfg(hevc_serial_baseline)]
    {
        std::hint::black_box(threads);
    }
    let mut output = decoded::Output::new(writer);
    let mut pictures = 0u64;
    let mut callback = |picture: coreav_hevc::DecodedPicture<'_>| {
        if mode == "check" {
            output.picture(picture)?;
        } else {
            std::hint::black_box(picture);
        }
        pictures += 1;
        Ok(())
    };
    let receipt = common::read_nals(bytes.as_slice(), limits.headers.max_nal_bytes, |_, nal| {
        decoder.read_nal(nal, None, &mut callback)
    })?;
    let summary = decoder.finish(&mut callback)?;
    if mode == "check" {
        output.finish(summary, receipt, limits)
    } else {
        let mut record = json!({"pictures":pictures,"thread_limit":threads,"max_memory_bytes":limits.max_memory_bytes,
            "decoded_pictures":summary.decoded_pictures,"bins":summary.reconstruction.bins});
        #[cfg(not(hevc_serial_baseline))]
        {
            record["parallel_pictures"] = json!(summary.parallel_pictures);
            record["max_workers"] = json!(summary.max_workers);
        }
        serde_json::to_writer(&mut *writer, &record)
            .map_err(|_| Error::Limit("reference_output"))?;
        writer
            .write_all(b"\n")
            .map_err(|_| Error::Limit("reference_output"))
    }
}

fn main() -> std::process::ExitCode {
    let mut writer = std::io::BufWriter::new(std::io::stdout().lock());
    match run(&mut writer)
        .and_then(|()| writer.flush().map_err(|_| Error::Limit("reference_output")))
    {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:?}");
            std::process::ExitCode::FAILURE
        }
    }
}
