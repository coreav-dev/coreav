use coreav_h264_encoder::{Colour, Deblocking, Encoder, Gop, Limits, Picture, Plane, Settings};
use std::{env, fs, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = env::args().skip(1).collect();
    let (w, h, qp, n): (usize, usize, u8, usize) = (a[0].parse()?, a[1].parse()?, a[2].parse()?, a[3].parse()?);
    let raw = fs::read(&a[4])?;
    let size = w * h * 3 / 2;
    let settings = Settings {
        width: w,
        height: h,
        qp,
        deblocking: Deblocking::Enabled,
        gop: Gop::Fixed { idr_interval: a[7].parse()? },
        colour: Colour { full_range: false, primaries: 1, transfer: 1, matrix: 1, chroma_location: 0 },
    };
    let mut encoder = Encoder::new(settings, Limits { frames: n as u64, ..Limits::default() })?;
    let mut stream = Vec::new();
    let mut recon = Vec::new();
    let start = Instant::now();
    for f in 0..n {
        let frame = &raw[f * size..(f + 1) * size];
        let (y, uv) = frame.split_at(w * h);
        let (cb, cr) = uv.split_at(w * h / 4);
        let au = encoder.encode(Picture {
            y: Plane::new(y, w, w, h)?,
            cb: Plane::new(cb, w / 2, w / 2, h / 2)?,
            cr: Plane::new(cr, w / 2, w / 2, h / 2)?,
        })?;
        for nal in au.nals {
            stream.extend_from_slice(&[0, 0, 0, 1]);
            stream.extend_from_slice(&nal);
        }
        recon.extend_from_slice(&au.reconstruction);
    }
    let seconds = start.elapsed().as_secs_f64();
    fs::write(&a[5], &stream)?;
    fs::write(&a[6], &recon)?;
    println!("{seconds}");
    Ok(())
}
