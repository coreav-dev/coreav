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
        max_memory_bytes: 1 << 30,
        ..DecodeLimits::default()
    };
    let mut decoder = PictureDecoder::new(limits);
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
        serde_json::to_writer(
            &mut *writer,
            &json!({"pictures":pictures,
            "decoded_pictures":summary.decoded_pictures,"bins":summary.reconstruction.bins}),
        )
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
