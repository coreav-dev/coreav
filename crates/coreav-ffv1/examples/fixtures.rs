//! Generates deterministic native FFV1 Matroska fixtures and packed references.

use coreav_ffv1::{Encoder, Limits, Settings};
use coreav_frame::{Chroma, Picture, PixelFormat, Plane};
use coreav_mkv::write::{CodecId, Media, Options, Packet, Track, plan};
use serde_json::{Value, json};
use std::error::Error;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum Pattern {
    Zero,
    Maximum,
    Bytes,
    Horizontal,
    Vertical,
    Alternating,
    Random,
    Seams,
}

struct Case {
    label: &'static str,
    width: u32,
    height: u32,
    padding: usize,
    patterns: &'static [Pattern],
    variable: bool,
    aspect: (u32, u32),
    colour: Option<bool>,
    unknown_final_duration: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = args.next().ok_or("expected one output directory")?;
    if args.next().is_some() {
        return Err("expected one output directory".into());
    }
    let directory = Path::new(&directory);
    fs::create_dir_all(directory)?;
    let mut cases = Vec::new();
    for (chroma, name) in [
        (Chroma::Cs420, "yuv420p"),
        (Chroma::Cs422, "yuv422p"),
        (Chroma::Cs444, "yuv444p"),
    ] {
        for case in matrix() {
            cases.push(generate(directory, chroma, name, &case)?);
        }
    }
    let manifest = json!({"cases": cases});
    let mut output = File::create(directory.join("manifest.json"))?;
    serde_json::to_writer_pretty(&mut output, &manifest)?;
    output.write_all(b"\n")?;
    Ok(())
}

fn matrix() -> Vec<Case> {
    let mut cases = vec![
        Case {
            label: "tiny",
            width: 1,
            height: 1,
            padding: 0,
            patterns: &[Pattern::Zero, Pattern::Maximum, Pattern::Zero],
            variable: false,
            aspect: (1, 1),
            colour: None,
            unknown_final_duration: false,
        },
        Case {
            label: "odd",
            width: 17,
            height: 19,
            padding: 0,
            patterns: &[Pattern::Horizontal, Pattern::Vertical, Pattern::Alternating],
            variable: true,
            aspect: (1, 1),
            colour: Some(false),
            unknown_final_duration: false,
        },
        Case {
            label: "padded",
            width: 67,
            height: 49,
            padding: 23,
            patterns: &[Pattern::Random, Pattern::Bytes, Pattern::Random],
            variable: false,
            aspect: (4, 3),
            colour: Some(false),
            unknown_final_duration: false,
        },
        Case {
            label: "all_bytes",
            width: 256,
            height: 8,
            padding: 7,
            patterns: &[Pattern::Bytes],
            variable: false,
            aspect: (1, 1),
            colour: None,
            unknown_final_duration: false,
        },
        Case {
            label: "narrow",
            width: 1,
            height: 257,
            padding: 3,
            patterns: &[Pattern::Vertical],
            variable: false,
            aspect: (1, 1),
            colour: Some(false),
            unknown_final_duration: false,
        },
        Case {
            label: "short",
            width: 257,
            height: 1,
            padding: 0,
            patterns: &[Pattern::Horizontal],
            variable: false,
            aspect: (1, 1),
            colour: None,
            unknown_final_duration: true,
        },
        Case {
            label: "threshold",
            width: 352,
            height: 288,
            padding: 0,
            patterns: &[Pattern::Seams],
            variable: false,
            aspect: (1, 1),
            colour: Some(false),
            unknown_final_duration: false,
        },
        Case {
            label: "four_slices",
            width: 640,
            height: 480,
            padding: 11,
            patterns: &[Pattern::Seams, Pattern::Random],
            variable: true,
            aspect: (1, 1),
            colour: Some(false),
            unknown_final_duration: false,
        },
        Case {
            label: "odd_chroma_width",
            width: 854,
            height: 480,
            padding: 7,
            patterns: &[Pattern::Seams, Pattern::Random],
            variable: false,
            aspect: (1, 1),
            colour: None,
            unknown_final_duration: false,
        },
        Case {
            label: "odd_large_dimensions",
            width: 855,
            height: 483,
            padding: 7,
            patterns: &[Pattern::Seams, Pattern::Random],
            variable: false,
            aspect: (1, 1),
            colour: None,
            unknown_final_duration: false,
        },
        Case {
            label: "hd_seams",
            width: 1920,
            height: 1080,
            padding: 0,
            patterns: &[Pattern::Seams],
            variable: false,
            aspect: (1, 1),
            colour: Some(false),
            unknown_final_duration: false,
        },
    ];
    cases.push(Case {
        label: "extrema",
        width: 31,
        height: 37,
        padding: 5,
        patterns: &[Pattern::Zero, Pattern::Maximum, Pattern::Alternating],
        variable: false,
        aspect: (1, 1),
        colour: None,
        unknown_final_duration: false,
    });
    cases.push(Case {
        label: "full_range",
        width: 16,
        height: 16,
        padding: 3,
        patterns: &[Pattern::Zero, Pattern::Maximum, Pattern::Bytes],
        variable: false,
        aspect: (1, 1),
        colour: Some(true),
        unknown_final_duration: false,
    });
    cases
}

fn generate(
    directory: &Path,
    chroma: Chroma,
    pix_fmt: &str,
    case: &Case,
) -> Result<Value, Box<dyn Error>> {
    let name = format!("{pix_fmt}_{}", case.label);
    let file_name = format!("{name}.mkv");
    let raw_name = format!("{name}.raw");
    let settings = Settings {
        width: case.width,
        height: case.height,
        format: PixelFormat::Yuv { chroma, depth: 8 },
        sample_aspect: Some(case.aspect),
    };
    let mut encoder = Encoder::new(settings, Limits::default())?;
    let mut raw = File::create(directory.join(&raw_name))?;
    let mut encoded = Vec::new();
    let mut packets = Vec::new();
    let mut timestamps = Vec::new();
    let mut durations = Vec::new();
    let mut pts_ns = 0i64;
    for (frame, &pattern) in case.patterns.iter().enumerate() {
        let picture = pixels(settings, case.padding, pattern, frame)?;
        for plane in 0..3 {
            for row in picture.rows(plane) {
                raw.write_all(row)?;
            }
        }
        let packet = encoder.encode(&picture, Instant::now() + Duration::from_secs(120))?;
        let duration = if case.variable && frame % 2 != 0 {
            80_000_000
        } else {
            40_000_000
        };
        let duration_ns = if case.unknown_final_duration && frame + 1 == case.patterns.len() {
            None
        } else {
            Some(duration)
        };
        timestamps.push(pts_ns);
        durations.push(duration_ns);
        packets.push(Packet {
            track: 0,
            pts_ns,
            duration_ns,
            size: packet.len() as u64,
            keyframe: true,
            invisible: false,
            discard_padding_ns: None,
        });
        encoded.push(packet);
        pts_ns += duration as i64;
    }
    let mut media = Media::Video {
        width: settings.width,
        height: settings.height,
        display: Some((
            settings.width * case.aspect.0,
            settings.height * case.aspect.1,
        )),
        colour: Default::default(),
        interlaced: 2,
        field_order: 0,
    };
    if let Some(full_range) = case.colour {
        if let Media::Video { colour, .. } = &mut media {
            colour.primaries = Some(1);
            colour.transfer = Some(1);
            colour.matrix = Some(1);
            colour.full_range = Some(full_range);
        }
    }
    let track = Track {
        codec: CodecId::Ffv1,
        private: encoder.configuration().to_vec(),
        media,
        default_duration_ns: if case.variable || case.unknown_final_duration {
            None
        } else {
            Some(40_000_000)
        },
        codec_delay_ns: 0,
        seek_pre_roll_ns: 0,
        language: "und".to_owned(),
        name: None,
        default: true,
    };
    let tracks = [track];
    let layout = plan(
        &tracks,
        &packets,
        Options::default(),
        &coreav_mkv::Limits::default(),
    )?;
    let mut file = File::create(directory.join(&file_name))?;
    layout.write(&mut file, |index, output| {
        let bytes = encoded
            .get(index)
            .ok_or(coreav_mkv::Error::Io(std::io::ErrorKind::InvalidData))?;
        output
            .write_all(bytes)
            .map_err(|e| coreav_mkv::Error::Io(e.kind()))
    })?;
    Ok(json!({
        "name": name, "file": file_name, "raw": raw_name,
        "width": settings.width, "height": settings.height, "pix_fmt": pix_fmt,
        "frames": case.patterns.len(), "pts_ns": timestamps, "duration_ns": durations,
        "sample_aspect_ratio": format!("{}:{}", case.aspect.0, case.aspect.1),
        "time_base": "1/1000",
        "color": {
            "color_range": case.colour.map(|full| if full { "pc" } else { "tv" }),
            "color_space": case.colour.map(|_| "bt709"),
            "color_transfer": case.colour.map(|_| "bt709"),
            "color_primaries": case.colour.map(|_| "bt709")
        }
    }))
}

fn pixels(
    settings: Settings,
    padding: usize,
    pattern: Pattern,
    frame: usize,
) -> Result<Picture, Box<dyn Error>> {
    let mut planes = Vec::new();
    let mut random = 0x6a09_e667u32;
    for p in 0..3 {
        let (width, height) = settings
            .format
            .plane_size(p, settings.width, settings.height)
            .ok_or("missing plane geometry")?;
        let stride = width as usize + padding;
        let mut bytes = vec![0xa5; stride * height as usize];
        for (y, row) in bytes.chunks_exact_mut(stride).enumerate() {
            for (x, value) in row.iter_mut().take(width as usize).enumerate() {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                *value = match pattern {
                    Pattern::Zero => 0,
                    Pattern::Maximum => 255,
                    Pattern::Bytes => (y * width as usize + x + p * 41) as u8,
                    Pattern::Horizontal => x as u8,
                    Pattern::Vertical => y as u8,
                    Pattern::Alternating => {
                        if (x + y + p + frame) % 2 == 0 {
                            0
                        } else {
                            255
                        }
                    }
                    Pattern::Random => (random >> 16) as u8,
                    Pattern::Seams => {
                        let quadrant = usize::from(x >= width as usize / 2)
                            + 2 * usize::from(y >= height as usize / 2);
                        (quadrant * 61 + p * 17 + (x + 3 * y) % 13) as u8
                    }
                };
            }
        }
        planes.push(Plane::new(bytes, stride));
    }
    Ok(Picture::new(
        settings.width,
        settings.height,
        settings.format,
        planes,
    )?)
}
