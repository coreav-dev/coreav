use coreav_h264_encoder::{Colour, Deblocking, Encoder, Gop, Limits, Picture, Plane, Settings};
use std::{env, fs, time::Instant};

#[inline(never)]
fn measured_encode(encoder: &mut Encoder, picture: Picture<'_>) -> coreav_h264_encoder::Result<coreav_h264_encoder::AccessUnit> {
    std::hint::black_box(encoder.encode(picture))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    let [w, h, qp, frames, input, stream_path, recon_path, interval] = args.as_slice() else {
        return Err("invalid arguments".into());
    };
    let (w, h, n): (usize, usize, usize) = (w.parse()?, h.parse()?, frames.parse()?);
    let raw = fs::read(input)?;
    let size = w * h * 3 / 2;
    if raw.len() != size * n { return Err("incomplete input".into()); }
    let settings = Settings {
        frame_rate: Default::default(), sample_aspect_ratio: Default::default(),
        width: w, height: h,
        rate_mode: match qp.strip_prefix("crf:") {
            Some(value) => coreav_h264_encoder::RateMode::Crf(value.parse()?),
            None => coreav_h264_encoder::RateMode::FixedQp(qp.parse()?),
        },
        deblocking: if env::var("DEBLOCK").as_deref() == Ok("0") { Deblocking::Disabled } else { Deblocking::Enabled },
        gop: Gop::Fixed { idr_interval: interval.parse()? },
        colour: Colour { full_range: false, primaries: 1, transfer: 1, matrix: 1, chroma_location: 0 },
    };
    let repetitions = env::var("REPETITIONS").ok().map(|v| v.parse::<usize>()).transpose()?.unwrap_or(1);
    let mut stream = Vec::new();
    let mut recon = Vec::new();
    let mut seconds = 0.0;
    for _ in 0..repetitions {
    let threads = env::var("THREADS").unwrap_or_else(|_| "1".into()).parse()?;
    let mut encoder = Encoder::with_threads(settings, Limits { frames: n as u64, ..Limits::default() }, threads)?;
    stream.clear();
    recon.clear();
    for frame in raw.chunks_exact(size) {
        let (y, uv) = frame.split_at(w * h);
        let (cb, cr) = uv.split_at(w * h / 4);
        let start = Instant::now();
        let au = measured_encode(&mut encoder, Picture {
            y: Plane::new(y, w, w, h)?, cb: Plane::new(cb, w / 2, w / 2, h / 2)?,
            cr: Plane::new(cr, w / 2, w / 2, h / 2)?,
        })?;
        seconds += start.elapsed().as_secs_f64();
        for nal in au.nals {
            stream.extend_from_slice(&[0, 0, 0, 1]);
            stream.extend_from_slice(&nal);
        }
        recon.extend_from_slice(&au.reconstruction);
    }
    }
    fs::write(stream_path, &stream)?;
    fs::write(recon_path, &recon)?;
    println!("{seconds}");
    Ok(())
}
