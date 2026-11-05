#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "../../../../test-support/oracle.rs"]
mod test_oracle;

use super::*;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn format(format: SampleFormat) -> (&'static str, &'static str) {
    match format {
        SampleFormat::S16 => ("s16le", "s16"),
        SampleFormat::S24 => ("s24le", "s32"),
        SampleFormat::S32 => ("s32le", "s32"),
        SampleFormat::F32 => ("f32le", "flt"),
    }
}

fn reference(
    ffmpeg: &Path,
    input: &[u8],
    source: SampleFormat,
    target: SampleFormat,
    channels: usize,
    planar: bool,
) -> Vec<u8> {
    let (input_format, _) = format(source);
    let (output_format, sample_format) = format(target);
    let layout = format!(
        "aformat=sample_fmts={sample_format}{}",
        if planar { "p" } else { "" }
    );
    let mut child = Command::new(ffmpeg)
        .args(["-v", "error", "-nostdin"])
        .args(include_str!("../../../../scripts/oracle-audio-args.txt").split_whitespace())
        .args(["-f", input_format, "-ar", "48000", "-ac"])
        .arg(channels.to_string())
        .args([
            "-i",
            "pipe:0",
            "-af",
            &layout,
            "-dither_method",
            "none",
            "-c:a",
        ])
        .arg(format!("pcm_{output_format}"))
        .args(["-f", output_format, "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn sample_conversion_is_exact_against_the_pinned_oracle() {
    let ffmpeg = test_oracle::require().ffmpeg().to_path_buf();
    let formats = [
        SampleFormat::S16,
        SampleFormat::S24,
        SampleFormat::S32,
        SampleFormat::F32,
    ];
    let mut differences = Vec::new();
    for source in formats {
        for channels in [1, 2, 3] {
            let mut pcm = Pcm::new();
            let frames = 65;
            let mut input = Vec::new();
            if source == SampleFormat::F32 {
                let samples = pcm.float_buffer(channels, frames);
                let values = [
                    -1.0f32,
                    1.0,
                    -0.0,
                    0.0,
                    -0.5,
                    0.5,
                    0.5 / 32_768.0,
                    1.5 / 32_768.0,
                    -1.5 / 32_768.0,
                    0.5 / 8_388_608.0,
                    1.5 / 8_388_608.0,
                    -1.5 / 8_388_608.0,
                    f32::MIN_POSITIVE,
                    -f32::MIN_POSITIVE,
                    f32::from_bits(1),
                    -f32::from_bits(1),
                    f32::from_bits(0.5f32.to_bits() + 1),
                    f32::from_bits(0.5f32.to_bits() - 1),
                ];
                for (c, plane) in samples.chunks_exact_mut(frames).enumerate() {
                    for (i, value) in plane.iter_mut().enumerate() {
                        *value = values[(i + c * 7) % values.len()];
                    }
                }
                for i in 0..frames {
                    for c in 0..channels {
                        input.extend_from_slice(&samples[c * frames + i].to_le_bytes());
                    }
                }
            } else {
                let bits = u32::from(source.bits());
                let samples = pcm.int_buffer(channels, frames, bits);
                for (c, plane) in samples.chunks_exact_mut(frames).enumerate() {
                    for (i, value) in plane.iter_mut().enumerate() {
                        let v = match (i + c * 7) % 8 {
                            0 => i32::MIN,
                            1 => i32::MAX,
                            2 => 0,
                            3 => -1,
                            4 => 1,
                            5 => i32::MIN + 255,
                            6 => i32::MAX - 255,
                            _ => (i as i32).wrapping_mul(2_654_435_761u32 as i32),
                        };
                        *value = v >> (32 - bits);
                    }
                }
                for i in 0..frames {
                    for c in 0..channels {
                        input.extend_from_slice(
                            &samples[c * frames + i].to_le_bytes()[..source.bytes()],
                        );
                    }
                }
            }
            for target in [SampleFormat::S16, SampleFormat::S32, SampleFormat::F32] {
                let mut actual = Vec::new();
                interleave(&pcm, 0, frames, target, &mut actual, 0);
                for planar in [false, true] {
                    let expected = reference(&ffmpeg, &input, source, target, channels, planar);
                    let first = actual.iter().zip(&expected).position(|(a, b)| a != b);
                    if actual != expected {
                        differences.push(format!(
                            "{source:?} -> {target:?}, {channels} channels, planar={planar}, lengths {}/{}, first different byte {first:?}", actual.len(), expected.len()));
                    }
                }
                let mut chunks = vec![0xA5; 7];
                interleave(&pcm, 0, 17, target, &mut chunks, 7);
                interleave(
                    &pcm,
                    17,
                    frames - 17,
                    target,
                    &mut chunks,
                    7 + 17 * channels * target.bytes(),
                );
                assert_eq!(&chunks[..7], &[0xA5; 7]);
                assert_eq!(&chunks[7..], actual);
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    assert_eq!(
        reference(
            &ffmpeg,
            &(3.0f32 / 16_777_216.0).to_le_bytes(),
            SampleFormat::F32,
            SampleFormat::S24,
            1,
            false,
        ),
        [1, 0, 0],
        "the documented PCM24 reference value changed"
    );
}
