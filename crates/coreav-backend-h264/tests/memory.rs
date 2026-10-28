//! The memory budget bounds what the native backend allocates:
//! a stream decoded under the tightest budget that keeps a thread count
//! allocates no more than that budget, on every thread together, at one
//! thread and at eight (`docs/design/h264.md`, "Limits").
//!
//! The counting allocator counts every thread here (`measure_process`),
//! so this binary holds this one test. Needs `ffmpeg` on `PATH` (with
//! libx264) for the fixture, so it is ignored by default; CI runs it in
//! the fixtures workflow.
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use coreav_backend_h264::{NativeH264, NativeOptions};
use coreav_counting_alloc::{CountingAllocator, measure_process, report};
use coreav_decode::{Backend, Codec, CodecConfig, ConfigKind, Limits, Packet, StreamConfig};
use coreav_isobmff::{Allowlist, Budget, FileSource, Source, probe};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

fn ffmpeg() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path)
        .map(|d| d.join("ffmpeg"))
        .find(|p| p.is_file())
        .expect("ffmpeg on PATH")
}

fn run_ffmpeg(args: &[&str]) {
    let status = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success());
}

/// `size` with B pictures, four references and a keyframe every 24
/// pictures.
fn fixture(dir: &Path, size: &str) -> PathBuf {
    let out = dir.join(format!("{size}.mp4"));
    let input = format!("testsrc2=size={size}:rate=25");
    run_ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &input,
        "-t",
        "2",
        "-c:v",
        "libx264",
        "-bf",
        "3",
        "-refs",
        "4",
        "-g",
        "24",
        "-pix_fmt",
        "yuv420p",
        out.to_str().expect("path"),
    ]);
    out
}

/// A stream made of parts of these sizes in turn, a second each, with
/// their parameter sets in band and no B pictures.
fn joined(dir: &Path, name: &str, sizes: &[&str]) -> PathBuf {
    let mut list = String::new();
    for (i, size) in sizes.iter().enumerate() {
        let part = dir.join(format!("{name}-{i}.mp4"));
        let input = format!("testsrc2=size={size}:rate=25");
        run_ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            &input,
            "-t",
            "1",
            "-c:v",
            "libx264",
            "-bf",
            "0",
            "-refs",
            "4",
            "-x264-params",
            "repeat-headers=1",
            "-pix_fmt",
            "yuv420p",
            part.to_str().expect("path"),
        ]);
        list.push_str(&format!("file '{}'\n", part.display()));
    }
    let listing = dir.join(format!("{name}.txt"));
    std::fs::write(&listing, list).expect("list");
    let out = dir.join(format!("{name}.mp4"));
    run_ffmpeg(&[
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        listing.to_str().expect("path"),
        "-c",
        "copy",
        out.to_str().expect("path"),
    ]);
    out
}

/// 1080p whose parameter sets crop it to 32x40: what it shows says
/// nothing of what it costs.
fn cropped(dir: &Path) -> PathBuf {
    let out = dir.join("cropped.mp4");
    run_ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=1920x1080:rate=25",
        "-t",
        "1",
        "-c:v",
        "libx264",
        "-bsf:v",
        "h264_metadata=crop_right=1888:crop_bottom=1048",
        "-pix_fmt",
        "yuv420p",
        out.to_str().expect("path"),
    ]);
    out
}

/// An access unit: its bytes, timestamps and whether it is a keyframe.
type Au = (Vec<u8>, i64, i64, bool);

/// The stream's configuration and its access units.
fn read(file: &Path) -> (StreamConfig, Vec<Au>) {
    let mut source = FileSource::open(file).expect("open");
    let movie = probe(&mut source, &Default::default(), &Allowlist::default()).expect("probe");
    let track = movie.tracks.first().expect("track");
    let stream = StreamConfig {
        config: track
            .description
            .codec_config
            .as_ref()
            .map(|c| CodecConfig {
                kind: ConfigKind::Avcc,
                data: c.data.clone(),
            }),
        width: track.description.width.map(u32::from),
        height: track.description.height.map(u32::from),
        ..StreamConfig::new(Codec::H264)
    };
    let mut budget = Budget::new(&Default::default());
    let packets: Vec<_> = movie
        .packets(0, &mut source, &Default::default(), &mut budget)
        .expect("packets")
        .collect::<Result<_, _>>()
        .expect("packet list");
    let aus = packets
        .iter()
        .map(|p| {
            let mut data = vec![0u8; p.size as usize];
            source.read_exact_at(p.offset, &mut data).expect("read");
            (data, p.cts, p.dts, p.keyframe)
        })
        .collect();
    (stream, aus)
}

/// Opens and decodes every access unit, discarding each where `discard`
/// says, and drops each frame as it comes: the frames returned, or the
/// first error.
fn decode(
    stream: &StreamConfig,
    aus: &[Au],
    threads: usize,
    limits: &Limits,
    discard: bool,
) -> Result<usize, coreav_decode::Error> {
    let backend = NativeH264::with_options(NativeOptions {
        threads: Some(threads),
        ..NativeOptions::default()
    });
    let mut decoder = backend.open(stream, limits)?;
    let mut frames = 0;
    for (data, pts, dts, keyframe) in aus {
        decoder.send_packet(&Packet {
            data,
            pts: Some(*pts),
            dts: Some(*dts),
            keyframe: *keyframe,
            discard,
        })?;
        while decoder.receive_frame()?.is_some() {
            frames += 1;
        }
    }
    decoder.send_eof()?;
    while decoder.receive_frame()?.is_some() {
        frames += 1;
    }
    Ok(frames)
}

/// The access units with what a stream may carry beyond its pictures:
/// 200 picture parameter sets of 64 KiB of padding in the first and the
/// stream's sequence parameter set with 1 MB of padding in the second,
/// which the decoder reads only the start of, and 1 MB of
/// `cabac_zero_words` at the end of every slice, which it reads whole.
fn padded(stream: &StreamConfig, aus: &[Au]) -> Vec<Au> {
    // The first sequence parameter set of the avcC: its count in the low
    // five bits of byte 5, then a 16-bit length.
    let avcc = &stream.config.as_ref().expect("avcC").data;
    assert!(avcc[5] & 31 >= 1);
    let length = usize::from(u16::from_be_bytes([avcc[6], avcc[7]]));
    let sps = &avcc[8..8 + length];
    let mut out = aus.to_vec();
    for (i, au) in out.iter_mut().enumerate() {
        let mut data = Vec::new();
        if i == 1 {
            let mut nal = sps.to_vec();
            nal.extend(std::iter::repeat_n(0x55, 1 << 20));
            data.extend((nal.len() as u32).to_be_bytes());
            data.extend(nal);
        }
        if i == 0 {
            for id in 1..=200u32 {
                // pic_parameter_set_id as ue(v) in its own bytes: a 0 bit
                // count, a 1 and the value's bits, padded with 1s.
                let v = id + 1;
                let len = 32 - v.leading_zeros();
                let mut bits = vec![0u8; (len - 1) as usize];
                bits.extend((0..len).rev().map(|b| ((v >> b) & 1) as u8));
                bits.resize(bits.len().div_ceil(8) * 8, 1);
                let mut nal = vec![0x68];
                nal.extend(
                    bits.chunks(8)
                        .map(|c| c.iter().fold(0, |a, &b| (a << 1) | b)),
                );
                nal.extend(std::iter::repeat_n(0x55, 64 << 10));
                nal.push(0x80);
                data.extend((nal.len() as u32).to_be_bytes());
                data.extend(nal);
            }
        }
        let mut at = 0;
        while at + 4 <= au.0.len() {
            let len = u32::from_be_bytes(au.0[at..at + 4].try_into().unwrap()) as usize;
            let mut nal = au.0[at + 4..at + 4 + len].to_vec();
            if matches!(nal[0] & 31, 1 | 5) {
                for _ in 0..(1 << 20) / 3 {
                    nal.extend([0, 0, 3]);
                }
            }
            data.extend((nal.len() as u32).to_be_bytes());
            data.extend(nal);
            at += 4 + len;
        }
        au.0 = data;
    }
    out
}

/// The fewest references the backend opens the stream with: a budget
/// written for sixteen would not be tight.
fn references(stream: &StreamConfig) -> u32 {
    (1..=16)
        .find(|&n| {
            let limits = Limits {
                max_reference_frames: n,
                ..Limits::default()
            };
            NativeH264::new().open(stream, &limits).is_ok()
        })
        .expect("a reference count the backend opens")
}

/// Decodes under `limits` with `threads` asked for and checks that every
/// thread together allocated no more than the budget: the frames, or the
/// error the decoder stopped with.
fn bounded(
    name: &str,
    stream: &StreamConfig,
    aus: &[Au],
    threads: usize,
    limits: &Limits,
    discard: bool,
) -> Result<usize, coreav_decode::Error> {
    let (result, counters) = measure_process(|| decode(stream, aus, threads, limits, discard));
    report(&format!("{name}-{threads}-threads"), &counters);
    println!(
        "{name}, {threads} threads, {} references: peak {} of a budget of {} ({:.3}), {}",
        limits.max_reference_frames,
        counters.peak_bytes,
        limits.max_memory_bytes,
        counters.peak_bytes as f64 / limits.max_memory_bytes as f64,
        match &result {
            Ok(frames) => format!("{frames} frames"),
            Err(e) => format!("{e}"),
        }
    );
    // Nothing allocated before was freed inside, which would have offset
    // the peak.
    assert_eq!(
        counters.trough_bytes, 0,
        "{name}, {threads} threads: {counters}"
    );
    assert!(
        counters.peak_bytes <= limits.max_memory_bytes,
        "{name}, {threads} threads: {counters} against a budget of {}",
        limits.max_memory_bytes
    );
    result
}

/// The limits of `stream`'s own reference count, with the budget the
/// backend asks for pictures of `width` by `height` with `threads`.
fn budget(stream: &StreamConfig, aus: &[Au], width: u32, height: u32, threads: usize) -> Limits {
    // The largest packet, and so NAL unit, the stream has: what the
    // decoder holds of slice data is counted at the most a packet may
    // carry.
    let largest = aus.iter().map(|au| au.0.len()).max().unwrap_or(1) as u64;
    let mut limits = Limits {
        max_reference_frames: references(stream),
        max_packet_bytes: largest,
        ..Limits::default()
    };
    limits.max_memory_bytes = NativeH264::memory_needed(width, height, threads, &limits);
    limits
}

#[test]
#[ignore = "needs ffmpeg"]
fn decoding_allocates_no_more_than_the_memory_budget() {
    let dir = std::env::temp_dir().join(format!("coreav-native-memory-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");

    // Under the tightest budget for the declared size and the thread
    // count, every frame. 720p is cut into bands of two macroblock rows,
    // sd into bands of one, whose aprons cost the most; at 48x48 the
    // fixed costs are most of what it needs, and padded it is mostly
    // slice data. A stream that changes size leaves pictures of the other
    // size for reuse.
    let mut streams: Vec<(String, StreamConfig, Vec<Au>)> = ["1280x720", "720x480", "48x48"]
        .iter()
        .map(|size| {
            let (stream, aus) = read(&fixture(&dir, size));
            (size.to_string(), stream, aus)
        })
        .collect();
    let small = streams
        .last()
        .map(|(_, stream, aus)| (stream.clone(), padded(stream, aus)));
    if let Some((stream, aus)) = small {
        streams.push(("48x48-padded".into(), stream, aus));
    }
    let sizes = ["1280x720", "1280x704", "1280x720", "1280x704"];
    let (stream, aus) = read(&joined(&dir, "changing", &sizes));
    streams.push(("changing".into(), stream, aus));
    for (name, stream, aus) in &streams {
        let (width, height) = (stream.width.expect("width"), stream.height.expect("height"));
        for threads in [1, 8] {
            let limits = budget(stream, aus, width, height, threads);
            let frames = bounded(name, stream, aus, threads, &limits, false).expect("decodes");
            assert_eq!(frames, aus.len(), "{name}");
            // Discarded pictures are decoded all the same.
            let frames = bounded(name, stream, aus, threads, &limits, true).expect("decodes");
            assert_eq!(frames, 0, "{name}");
        }
    }

    // A packet that carries every picture of the stream: the decoder lets
    // go of each picture it releases as it comes, rather than holding them
    // to the end of the packet. (The packet's frame is its first picture;
    // the others have no packet of their own to be the frame of.)
    let (name, stream, aus) = &streams[0];
    let (width, height) = (stream.width.expect("width"), stream.height.expect("height"));
    let mut whole = aus[0].clone();
    for au in &aus[1..] {
        whole.0.extend(&au.0);
    }
    let whole = vec![whole];
    for threads in [1, 8] {
        let limits = budget(stream, &whole, width, height, threads);
        let name = format!("{name}-in-one-packet");
        let frames = bounded(&name, stream, &whole, threads, &limits, false).expect("decodes");
        assert_eq!(frames, 1);
    }

    // A stream that grows: the budget for its larger size at one thread
    // decodes it whatever the threads asked for, with or without
    // frames; the budget for its smaller size refuses it.
    let (stream, aus) = read(&joined(&dir, "growing", &["320x240", "1280x720"]));
    let roomy = budget(&stream, &aus, 1280, 720, 1);
    for threads in [1, 8, 16] {
        for discard in [false, true] {
            bounded("growing", &stream, &aus, threads, &roomy, discard).expect("decodes");
        }
    }
    let small = budget(&stream, &aus, 320, 240, 1);
    let error = bounded("growing", &stream, &aus, 1, &small, false).expect_err("refused");
    assert_eq!(error.kind, coreav_decode::ErrorKind::Limit, "{error}");

    // What the parameter sets crop away is decoded all the same: the
    // budget for what the stream shows refuses it.
    let (mut stream, aus) = read(&cropped(&dir));
    stream.width = None;
    stream.height = None;
    for threads in [1, 8] {
        let limits = budget(&stream, &aus, 32, 40, 1);
        let error =
            bounded("cropped", &stream, &aus, threads, &limits, false).expect_err("refused");
        assert_eq!(error.kind, coreav_decode::ErrorKind::Limit, "{error}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
