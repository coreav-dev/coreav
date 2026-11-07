use coreav_h264_encoder::{Colour, Deblocking, Encoder, Gop, Limits, Picture, Plane, Settings};
use std::{env, fs, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = env::args().skip(1).collect();
    let (w, h, n): (usize, usize, usize) = (a[0].parse()?, a[1].parse()?, a[3].parse()?);
    let qp: u8 = a[2].parse()?;
    let raw = fs::read(&a[4])?;
    let size = w * h * 3 / 2;
    if raw.len() != size * n { return Err("incomplete input".into()); }
    let settings = Settings {
        width: w, height: h, qp,
        deblocking: Deblocking::Enabled,
        gop: Gop::Fixed { idr_interval: 32 },
        colour: Colour { full_range: false, primaries: 1, transfer: 1, matrix: 1, chroma_location: 0 },
    };
    let mut encoder = Encoder::with_threads(settings, Limits { frames: n as u64, ..Limits::default() }, 1)?;
    let mut stream = Vec::new();
    let mut recon = Vec::new();
    let mut seconds = 0.;
    let mut nal_bits = 0usize;
    for frame in raw.chunks_exact(size) {
        let (y, uv) = frame.split_at(w * h);
        let (cb, cr) = uv.split_at(w * h / 4);
        let picture = Picture {
            y: Plane::new(y, w, w, h)?,
            cb: Plane::new(cb, w / 2, w / 2, h / 2)?,
            cr: Plane::new(cr, w / 2, w / 2, h / 2)?,
        };
        let start = Instant::now();
        let au = encoder.encode(picture)?;
        seconds += start.elapsed().as_secs_f64();
        for nal in au.nals {
            nal_bits += 8 * nal.len();
            stream.extend_from_slice(&[0, 0, 0, 1]);
            stream.extend_from_slice(&nal);
        }
        recon.extend_from_slice(&au.reconstruction);
    }
    fs::write(&a[5], &stream)?;
    fs::write(&a[6], &recon)?;
    println!("{{\"seconds\":{seconds},\"nal_bits\":{nal_bits}}}");
    Ok(())
}
