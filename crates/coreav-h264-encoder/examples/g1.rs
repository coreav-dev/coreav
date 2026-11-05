//! Test-only raw-picture driver and independent CoreAV reconstruction check.
#![forbid(unsafe_code)]

use coreav_h264_encoder::{Colour, Deblocking, Encoder, Limits, Picture, Plane, Settings};
use std::{
    env,
    fs::File,
    io::{Read, Write},
};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    let (args, deblocking) = if args.len() == 14 {
        let enabled = args
            .last()
            .ok_or("missing deblocking choice")?
            .parse::<bool>()?;
        (
            args.get(..13).ok_or("missing picture arguments")?,
            if enabled {
                Deblocking::Enabled
            } else {
                Deblocking::Disabled
            },
        )
    } else {
        (args.as_slice(), Deblocking::Disabled)
    };
    let [
        width,
        height,
        qp,
        frames,
        input,
        output,
        reconstruction,
        decoded,
        range,
        primaries,
        transfer,
        matrix,
        location,
    ] = args
    else {
        return Err("usage: g1 WIDTH HEIGHT QP FRAMES INPUT OUTPUT RECON DECODED FULL_RANGE PRIMARIES TRANSFER MATRIX CHROMA_LOCATION [DEBLOCKING]".into());
    };
    let width = width.parse::<usize>()?;
    let height = height.parse::<usize>()?;
    let frames = frames.parse::<u64>()?;
    let settings = Settings {
        width,
        height,
        qp: qp.parse()?,
        deblocking,
        colour: Colour {
            full_range: range.parse()?,
            primaries: primaries.parse()?,
            transfer: transfer.parse()?,
            matrix: matrix.parse()?,
            chroma_location: location.parse()?,
        },
    };
    let mut encoder = Encoder::new(
        settings,
        Limits {
            frames,
            ..Limits::default()
        },
    )?;
    let mut decoder =
        coreav_h264::Decoder::with_kernels(coreav_h264::Scalar, coreav_h264::Limits::default())
            .with_mode(coreav_h264::Mode::EveryPicture);
    let mut input = File::open(input)?;
    let mut output = File::create(output)?;
    let mut recon = File::create(reconstruction)?;
    let mut decoded = File::create(decoded)?;
    let ysize = width * height;
    let mut raw = vec![0; ysize * 3 / 2];
    let mut pending = std::collections::VecDeque::new();
    let mut count = 0;
    for _ in 0..frames {
        input.read_exact(&mut raw)?;
        let (y, uv) = raw.split_at(ysize);
        let (cb, cr) = uv.split_at(ysize / 4);
        let au = encoder.encode(Picture {
            y: Plane::new(y, width, width, height)?,
            cb: Plane::new(cb, width / 2, width / 2, height / 2)?,
            cr: Plane::new(cr, width / 2, width / 2, height / 2)?,
        })?;
        recon.write_all(&au.reconstruction)?;
        pending.push_back(au.reconstruction);
        for nal in au.nals {
            output.write_all(&[0, 0, 0, 1])?;
            output.write_all(&nal)?;
            decoder.decode_nal(&nal)?;
        }
        decoder.end_access_unit()?;
        for picture in decoder.take_pictures() {
            let expected = pending.pop_front().ok_or("decoder emitted extra picture")?;
            let (cb, cr) = picture.chroma();
            let actual: Vec<u8> = picture
                .luma()
                .rows()
                .chain(cb.rows())
                .chain(cr.rows())
                .flatten()
                .copied()
                .collect();
            if picture.width() != width || picture.height() != height || actual != expected {
                return Err("CoreAV reconstruction mismatch".into());
            }
            decoded.write_all(&actual)?;
            count += 1;
        }
    }
    let mut extra = [0];
    if input.read(&mut extra)? != 0 {
        return Err("extra input bytes".into());
    }
    decoder.flush()?;
    for picture in decoder.take_pictures() {
        let expected = pending.pop_front().ok_or("decoder emitted extra picture")?;
        let (cb, cr) = picture.chroma();
        let actual: Vec<u8> = picture
            .luma()
            .rows()
            .chain(cb.rows())
            .chain(cr.rows())
            .flatten()
            .copied()
            .collect();
        if picture.width() != width || picture.height() != height || actual != expected {
            return Err("CoreAV reconstruction mismatch".into());
        }
        decoded.write_all(&actual)?;
        count += 1;
    }
    if count != frames
        || !pending.is_empty()
        || !decoder.warnings().is_empty()
        || decoder.warnings_dropped() != 0
    {
        return Err("CoreAV frame count mismatch".into());
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}
