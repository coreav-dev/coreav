//! Decodes an Annex B byte stream and writes the pictures to standard
//! output as cropped planar 4:2:0 frames, the layout FFmpeg calls
//! `rawvideo` with `-pix_fmt yuv420p`.
//!
//! ```text
//! cargo run -p coreav-h264 --release --example decode -- stream.264 > out.yuv
//! cargo run -p coreav-h264 --release --example decode -- --frames 10 stream.264 | ...
//! ```
//!
//! `--kernels scalar` forces the reference kernels instead of the fastest
//! ones this CPU supports, and `--kernels avx2` the vector kernels without
//! their AVX-512 tier, as a CPU with AVX2 alone runs them; `--keyframes`
//! decodes only the pictures that need no other picture
//! ([`Mode::KeyframesOnly`]), as a thumbnail does;
//! `--discard` decodes without writing anything, which is what a
//! measurement of decoding alone times (`scripts/h264_speed.py`);
//! `--threads N` decodes N pictures at once ([`Decoder::with_threads`]),
//! which gives the same pictures as one thread. `--band-height N` cuts
//! pictures into bands of N macroblock rows as threaded decoding does,
//! whatever the thread count: a check that the bands change nothing.
//!
//! It is also the shape of a caller: open a stream, hand the decoder NAL
//! units as they arrive, take the pictures it finishes.
#![forbid(unsafe_code)]

use std::io::{BufWriter, Read, Write};

use coreav_h264::{AnnexB, Decoder, Limits, Mode, Picture};
use coreav_kernels_h264::Auto;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("decode: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut path = None;
    let mut limit = usize::MAX;
    let mut kernels = Auto::detect();
    let mut mode = Mode::EveryPicture;
    let mut discard = false;
    let mut threads = 1usize;
    let mut band_height = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => {
                limit = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--frames needs a number")?;
            }
            "--kernels" => {
                kernels = match args.next().as_deref() {
                    Some("scalar") => Auto::scalar(),
                    Some("auto") | None => Auto::detect(),
                    Some("avx2") => Auto::detect().without_avx512(),
                    Some(other) => return Err(format!("unknown kernels: {other}")),
                };
            }
            "--threads" => {
                threads = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--threads needs a number")?;
            }
            "--band-height" => {
                band_height = Some(
                    args.next()
                        .and_then(|n| n.parse().ok())
                        .ok_or("--band-height needs a number")?,
                );
            }
            "--keyframes" => mode = Mode::KeyframesOnly,
            "--discard" => discard = true,
            // An unknown option used to become the input path, so a
            // harness could pass a flag this example does not have and
            // see no error — `-o FILE` was read as two input paths, the
            // last of which won.
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(format!("unknown option: {other}"));
            }
            other => path = Some(other.to_owned()),
        }
    }
    let path = path
        .ok_or("usage: decode [--frames N] [--threads N] [--keyframes] [--discard] STREAM.264")?;
    let mut file = std::fs::File::open(&path).map_err(|e| format!("{path}: {e}"))?;

    let mut out = BufWriter::with_capacity(1 << 20, raw_stdout()?);
    let mut decoder = Decoder::with_kernels(kernels, Limits::default())
        .with_mode(mode)
        .with_threads(threads);
    if let Some(rows) = band_height {
        decoder = decoder.with_band_height(rows);
    }
    let mut splitter = AnnexB::new(&Limits::default());
    let mut written = 0usize;
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut chunk).map_err(|e| format!("{path}: {e}"))?;
        if read == 0 {
            break;
        }
        let part = chunk.get(..read).unwrap_or_default();
        let mut failed = None;
        let mut io = None;
        splitter
            .push(part, |nal| {
                // `--frames` stops the decoding, not only the writing: a
                // chunk holds many pictures, and a measurement of the
                // first N must decode N and no more.
                if written >= limit {
                    return Ok(());
                }
                if let Err(e) = decoder.decode_nal(nal) {
                    // One bad NAL unit does not end the stream: the next
                    // picture may still decode.
                    failed = Some(e);
                }
                match write_pictures(&mut out, &decoder.take_pictures(), limit - written, discard) {
                    Ok(n) => written += n,
                    Err(e) => io = Some(e),
                }
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        if let Some(e) = io {
            return Err(e);
        }
        if let Some(e) = failed {
            eprintln!("decode: {e}");
        }
        if written >= limit {
            return out.flush().map_err(|e| e.to_string());
        }
    }
    let mut failed = None;
    splitter
        .finish(|nal| {
            if let Err(e) = decoder.decode_nal(nal) {
                failed = Some(e);
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    if let Some(e) = failed {
        eprintln!("decode: {e}");
    }
    written += write_pictures(&mut out, &decoder.take_pictures(), limit - written, discard)?;
    if let Err(e) = decoder.flush() {
        // Carries on: the pictures are the ones one thread gives.
        eprintln!("decode: {e}");
    }
    write_pictures(&mut out, &decoder.take_pictures(), limit - written, discard)?;
    for warning in decoder.warnings() {
        eprintln!("decode: {warning:?}");
    }
    out.flush().map_err(|e| e.to_string())
}

/// Standard output without its line buffering. `std::io::Stdout` is
/// line-buffered even into a pipe or a file, and it scans every write for
/// the last newline byte. Sample data almost never holds one (luma is 16 or
/// more in most video), so each 1 MiB flush was scanned to its start: 2 to
/// 15% of the instructions of a decode that writes its frames (callgrind,
/// eight training streams). On Unix the frames go to a handle on the same
/// file descriptor instead.
#[cfg(unix)]
fn raw_stdout() -> Result<impl Write, String> {
    use std::os::fd::AsFd;
    let fd = std::io::stdout()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|e| format!("stdout: {e}"))?;
    Ok(std::fs::File::from(fd))
}

/// Standard output, line-buffered, where no descriptor can be taken.
#[cfg(not(unix))]
fn raw_stdout() -> Result<impl Write, String> {
    Ok(std::io::stdout().lock())
}

/// Writes at most `limit` pictures, and returns how many it wrote. With
/// `discard` it writes nothing and only counts them, so that a
/// measurement times the decoding and not the writing.
fn write_pictures(
    out: &mut impl Write,
    pictures: &[std::sync::Arc<Picture>],
    limit: usize,
    discard: bool,
) -> Result<usize, String> {
    let mut written = 0;
    for picture in pictures.iter().take(limit) {
        if discard {
            written += 1;
            continue;
        }
        let (cb, cr) = picture.chroma();
        for rows in [picture.luma().rows(), cb.rows(), cr.rows()] {
            for row in rows {
                out.write_all(row).map_err(|e| e.to_string())?;
            }
        }
        written += 1;
    }
    Ok(written)
}
