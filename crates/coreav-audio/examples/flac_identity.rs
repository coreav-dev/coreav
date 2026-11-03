//! Deterministic complete-file fixtures for FLAC byte-identity checks.

use coreav_audio::flac::encode::FlacSink;
use coreav_audio::{Limits, OutputFormat, SampleFormat, Sink};
use std::{env, error::Error, fs, path::PathBuf};

struct Case {
    bits: u16,
    channels: u16,
    frames: usize,
    kind: u8,
    rate: u32,
}

impl Case {
    fn name(&self) -> String {
        format!(
            "s{}-c{}-n{}-p{}.flac",
            self.bits, self.channels, self.frames, self.kind
        )
    }

    fn encode(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        let mask = match self.channels {
            1 => 0x4,
            2 => 0x3,
            3 => 0x7,
            4 => 0x33,
            5 => 0x37,
            6 => 0x3F,
            7 => 0x70F,
            8 => 0x63F,
            _ => return Err("unsupported fixture channel count".into()),
        };
        let format = OutputFormat {
            channels: self.channels,
            sample_rate: self.rate,
            format: if self.bits == 16 {
                SampleFormat::S16
            } else {
                SampleFormat::S24
            },
            channel_mask: mask,
        };
        let bound = 1i64 << (self.bits - 1);
        let mut random = 7u32;
        let (mut a, mut b) = (bound / 5, bound / 5);
        let mut pcm = Vec::new();
        for frame in 0..self.frames {
            let next = ((127 * b) >> 6) - a;
            a = b;
            b = next;
            for channel in 0..self.channels {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                let value = match self.kind {
                    0 => 0,
                    1 => {
                        if frame % 2 == 0 {
                            -bound
                        } else {
                            bound - 1
                        }
                    }
                    2 => {
                        ((frame as i64 * 17 + i64::from(channel) * 11) % (bound / 4) - bound / 8)
                            * 8
                    }
                    3 => {
                        if frame % 71 == 0 {
                            bound - 1
                        } else {
                            0
                        }
                    }
                    4 => i64::from(random >> (32 - self.bits)) - bound,
                    5 => {
                        let p = frame as i64 % 4093;
                        (p * p * 13 + p * 71 + i64::from(channel)) % (2 * bound - 1) - (bound - 1)
                    }
                    6 => {
                        ((frame as f64 * 0.13).sin() * (20000i64 << (self.bits - 16)) as f64) as i64
                            + i64::from(channel)
                    }
                    7 => {
                        if channel % 2 == 0 {
                            next
                        } else {
                            -next + frame as i64 % 3 - 1
                        }
                    }
                    _ => return Err("unsupported fixture signal".into()),
                };
                if !(-bound..bound).contains(&value) {
                    return Err("fixture sample exceeds stored width".into());
                }
                pcm.extend(
                    value
                        .to_le_bytes()
                        .into_iter()
                        .take(usize::from(self.bits / 8)),
                );
            }
        }
        let mut output = Vec::new();
        let mut sink = FlacSink::new(&mut output, Limits::default());
        sink.start(&format)?;
        let chunk = if self.kind % 2 == 0 {
            pcm.len().max(1)
        } else {
            17 * usize::from(self.channels) * format.format.bytes()
        };
        for part in pcm.chunks(chunk) {
            sink.write(part)?;
        }
        let header = sink.finish()?;
        output
            .get_mut(..header.len())
            .ok_or("missing fixture header")?
            .copy_from_slice(&header);
        Ok(output)
    }
}

fn cases() -> Vec<Case> {
    let mut result = Vec::new();
    for bits in [16, 24] {
        for channels in 1..=8 {
            for kind in 0..8 {
                for (index, frames) in [0, 1, 15, 16, 17, 4095, 4096, 4097, 8243]
                    .into_iter()
                    .enumerate()
                {
                    let rate = match (index + usize::from(channels) + usize::from(kind)) % 5 {
                        0 => 8000,
                        1 => 44100,
                        2 => 48000,
                        3 => 96000,
                        _ => 12345,
                    };
                    result.push(Case {
                        bits,
                        channels,
                        frames,
                        kind,
                        rate,
                    });
                }
            }
        }
    }
    result
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let destination = args
        .next()
        .ok_or("expected --list or a new output directory")?;
    if args.next().is_some() {
        return Err("unexpected fixture argument".into());
    }
    if destination == "--list" {
        for case in cases() {
            println!("{}", case.name());
        }
        return Ok(());
    }
    let destination = PathBuf::from(destination);
    fs::create_dir(&destination)?;
    for case in cases() {
        fs::write(destination.join(case.name()), case.encode()?)?;
    }
    Ok(())
}
