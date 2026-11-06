use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Oracle {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Oracle {
    pub fn run(&self, program: &Path, args: &[&str], dir: &Path) -> Vec<u8> {
        let mut command = Command::new(program);
        command.args(["-v", "error"]);
        for arg in args {
            if *arg == "-i" {
                command.args(
                    include_str!("../../../../scripts/oracle-audio-args.txt").split_whitespace(),
                );
            }
            command.arg(arg);
        }
        let output = command.current_dir(dir).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn probe(&self, path: &Path, raw: bool, frames: bool) -> Value {
        let mut args = vec!["-i", path.to_str().unwrap()];
        if raw {
            args.splice(0..0, ["-ignore_editlist", "1"]);
        }
        args.extend(if frames {
            vec!["-show_frames"]
        } else {
            vec![
                "-show_streams",
                "-show_format",
                "-show_packets",
                "-show_data",
                "-show_data_hash",
                "sha256",
            ]
        });
        args.extend(["-of", "json"]);
        serde_json::from_slice(&self.run(&self.ffprobe, &args, path.parent().unwrap())).unwrap()
    }

    fn decode(&self, path: &Path, raw: bool, video: bool) -> Vec<u8> {
        let mut args = vec!["-nostdin", "-xerror", "-noautorotate"];
        if raw {
            args.extend(["-ignore_editlist", "1"]);
        }
        args.extend([
            "-i",
            path.to_str().unwrap(),
            "-map",
            if video { "0:v:0" } else { "0:a:0" },
        ]);
        if video {
            args.extend([
                "-noautoscale",
                "-c:v",
                "rawvideo",
                "-pix_fmt",
                "yuv420p",
                "-fps_mode",
                "passthrough",
                "-f",
                "rawvideo",
            ]);
        } else {
            args.extend(["-c:a", "pcm_f64le", "-f", "f64le"]);
        }
        args.push("pipe:1");
        self.run(&self.ffmpeg, &args, path.parent().unwrap())
    }
}

fn uint(bytes: &[u8]) -> u64 {
    assert!(bytes.len() <= 8);
    bytes
        .iter()
        .fold(0, |value, byte| value << 8 | u64::from(*byte))
}

fn signed(bytes: &[u8]) -> i64 {
    match bytes.len() {
        4 => i64::from(i32::from_be_bytes(bytes.try_into().unwrap())),
        8 => i64::from_be_bytes(bytes.try_into().unwrap()),
        _ => panic!("invalid signed field width"),
    }
}

fn boxes(bytes: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut at = 0;
    let mut result = Vec::new();
    while at < bytes.len() {
        assert!(bytes.len() - at >= 8);
        let (size, header) = match uint(&bytes[at..at + 4]) {
            0 => panic!("generated boxes must have explicit extents"),
            1 => (usize::try_from(uint(&bytes[at + 8..at + 16])).unwrap(), 16),
            size => (usize::try_from(size).unwrap(), 8),
        };
        let end = at.checked_add(size).unwrap();
        assert!(size >= header && end <= bytes.len());
        result.push((
            bytes[at + 4..at + 8].try_into().unwrap(),
            &bytes[at + header..end],
        ));
        at = end;
    }
    result
}

fn optional<'a>(bytes: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let matches: Vec<_> = boxes(bytes)
        .into_iter()
        .filter(|(tag, _)| tag == kind)
        .collect();
    assert!(matches.len() <= 1, "duplicate box {kind:?}");
    matches.first().map(|(_, payload)| *payload)
}

fn child<'a>(bytes: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
    optional(bytes, kind).unwrap_or_else(|| panic!("missing box {kind:?}"))
}

fn clock(bytes: &[u8]) -> (u32, u64) {
    match bytes[0] {
        0 => (uint(&bytes[12..16]) as u32, uint(&bytes[16..20])),
        1 => (uint(&bytes[20..24]) as u32, uint(&bytes[24..32])),
        version => panic!("unsupported clock version {version}"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Edit {
    duration: i64,
    media: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Sample {
    dts: i64,
    cts: i64,
    duration: i64,
    sync: bool,
    pos: usize,
    data: Vec<u8>,
}

#[derive(Clone, Debug)]
struct Track {
    video: bool,
    scale: u32,
    duration: u64,
    edits: Vec<Edit>,
    samples: Vec<Sample>,
    config: Vec<u8>,
    btrt: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
struct Movie {
    scale: u32,
    duration: u64,
    tracks: Vec<Track>,
}

fn table(bytes: &[u8], signed_offsets: bool) -> Vec<i64> {
    assert_eq!(bytes.len(), 8 + uint(&bytes[4..8]) as usize * 8);
    let mut values = Vec::new();
    for row in bytes[8..].chunks_exact(8) {
        let value = if signed_offsets {
            signed(&row[4..8])
        } else {
            uint(&row[4..8]) as i64
        };
        assert!(uint(&row[..4]) < 100_000);
        values.extend(std::iter::repeat_n(value, uint(&row[..4]) as usize));
    }
    values
}

fn descriptor(bytes: &[u8], tag: u8) -> &[u8] {
    assert_eq!(bytes[0], tag);
    let mut length = 0usize;
    let mut at = 1;
    loop {
        assert!(at <= 4);
        let byte = bytes[at];
        at += 1;
        length = length.checked_mul(128).unwrap() + usize::from(byte & 127);
        if byte & 128 == 0 {
            break;
        }
    }
    &bytes[at..at.checked_add(length).unwrap()]
}

fn asc(esds: &[u8]) -> Vec<u8> {
    let es = descriptor(&esds[4..], 3);
    assert_eq!(es[2], 0);
    let decoder = descriptor(&es[3..], 4);
    assert_eq!(decoder[0], 0x40);
    descriptor(&decoder[13..], 5).to_vec()
}

fn aac_frame(config: &[u8]) -> usize {
    let mut bit = 0;
    let mut read = |count: usize| {
        let mut value = 0;
        for _ in 0..count {
            value = value << 1 | usize::from((config[bit / 8] >> (7 - bit % 8)) & 1);
            bit += 1;
        }
        value
    };
    assert_eq!(
        read(5),
        2,
        "configuration-declared AAC-LC, not explicit SBR/PS"
    );
    if read(4) == 15 {
        read(24);
    }
    assert!(matches!(read(4), 1 | 2));
    assert_eq!(read(1), 0);
    1024
}

fn parse(bytes: &[u8]) -> Movie {
    let moov = child(bytes, b"moov");
    let (scale, duration) = clock(child(moov, b"mvhd"));
    let tracks = boxes(moov)
        .into_iter()
        .filter(|(tag, _)| tag == b"trak")
        .map(|(_, trak)| {
            let mdia = child(trak, b"mdia");
            let (scale, duration) = clock(child(mdia, b"mdhd"));
            let handler = &child(mdia, b"hdlr")[8..12];
            assert!(handler == b"vide" || handler == b"soun");
            let video = handler == b"vide";
            let edits = optional(trak, b"edts").map_or_else(Vec::new, |edts| {
                let elst = child(edts, b"elst");
                let width = match elst[0] {
                    0 => 4,
                    1 => 8,
                    _ => panic!("invalid elst version"),
                };
                assert_eq!(elst.len(), 8 + uint(&elst[4..8]) as usize * (width * 2 + 4));
                elst[8..]
                    .chunks_exact(width * 2 + 4)
                    .map(|row| {
                        assert_eq!(&row[width * 2..], &[0, 1, 0, 0]);
                        Edit {
                            duration: uint(&row[..width]) as i64,
                            media: signed(&row[width..width * 2]),
                        }
                    })
                    .collect()
            });
            let stbl = child(child(mdia, b"minf"), b"stbl");
            let durations = table(child(stbl, b"stts"), false);
            let offsets = optional(stbl, b"ctts").map_or_else(
                || vec![0; durations.len()],
                |ctts| table(ctts, ctts[0] == 1),
            );
            assert_eq!(durations.len(), offsets.len());
            assert_eq!(
                durations.iter().sum::<i64>(),
                duration as i64,
                "mdhd stores the complete decode duration"
            );
            let stsz = child(stbl, b"stsz");
            let count = uint(&stsz[8..12]) as usize;
            assert_eq!(count, durations.len());
            let fixed = uint(&stsz[4..8]) as usize;
            let sizes: Vec<_> = if fixed == 0 {
                stsz[12..]
                    .chunks_exact(4)
                    .map(|row| uint(row) as usize)
                    .collect()
            } else {
                vec![fixed; count]
            };
            assert_eq!(sizes.len(), count);
            let (chunks, width) =
                optional(stbl, b"stco").map_or_else(|| (child(stbl, b"co64"), 8), |stco| (stco, 4));
            assert_eq!(chunks.len(), 8 + uint(&chunks[4..8]) as usize * width);
            let chunks: Vec<_> = chunks[8..]
                .chunks_exact(width)
                .map(|row| uint(row) as usize)
                .collect();
            let stsc = child(stbl, b"stsc");
            assert_eq!(stsc.len(), 8 + uint(&stsc[4..8]) as usize * 12);
            let rows: Vec<_> = stsc[8..]
                .chunks_exact(12)
                .map(|row| {
                    assert_eq!(uint(&row[8..12]), 1, "one sample description only");
                    (uint(&row[..4]) as usize, uint(&row[4..8]) as usize)
                })
                .collect();
            let sync: Vec<_> = optional(stbl, b"stss").map_or_else(
                || (1..=count).collect(),
                |stss| {
                    stss[8..]
                        .chunks_exact(4)
                        .map(|row| uint(row) as usize)
                        .collect()
                },
            );
            let mut samples = Vec::new();
            let mut dts = 0;
            for (index, chunk) in chunks.into_iter().enumerate() {
                let per_chunk = rows
                    .iter()
                    .rev()
                    .find(|(first, _)| *first <= index + 1)
                    .unwrap()
                    .1;
                let mut pos = chunk;
                for _ in 0..per_chunk {
                    let i = samples.len();
                    let end = pos.checked_add(sizes[i]).unwrap();
                    samples.push(Sample {
                        dts,
                        cts: offsets[i],
                        duration: durations[i],
                        sync: sync.contains(&(i + 1)),
                        pos,
                        data: bytes[pos..end].to_vec(),
                    });
                    pos = end;
                    dts += durations[i];
                }
            }
            assert_eq!(samples.len(), count);
            let stsd = child(stbl, b"stsd");
            assert_eq!(uint(&stsd[4..8]), 1);
            let entries = boxes(&stsd[8..]);
            assert_eq!(entries.len(), 1);
            let (tag, entry) = entries[0];
            let children = if video {
                assert_eq!(tag, *b"avc1");
                &entry[78..]
            } else {
                assert_eq!(tag, *b"mp4a");
                &entry[28..]
            };
            let config = if video {
                child(children, b"avcC").to_vec()
            } else {
                asc(child(children, b"esds"))
            };
            if !video {
                assert_eq!(aac_frame(&config), 1024);
            }
            Track {
                video,
                scale,
                duration,
                edits,
                samples,
                config,
                btrt: optional(children, b"btrt").map(<[u8]>::to_vec),
            }
        })
        .collect();
    Movie {
        scale,
        duration,
        tracks,
    }
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn exact(value: i64, from: u32, to: u32) -> i64 {
    let product = i128::from(value) * i128::from(to);
    assert_eq!(
        product % i128::from(from),
        0,
        "presentation boundary must be exactly representable"
    );
    i64::try_from(product / i128::from(from)).unwrap()
}

fn nearest(value: i64, from: u32, to: u32) -> i64 {
    assert!(value >= 0);
    i64::try_from((i128::from(value) * i128::from(to) + i128::from(from / 2)) / i128::from(from))
        .unwrap()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Window {
    start: i64,
    duration: i64,
    media: i64,
    input: usize,
}

fn source_window(movie: &Movie, track: &Track, common: u32) -> Window {
    if track.edits.is_empty() {
        let first = track.samples.iter().map(|s| s.dts + s.cts).min().unwrap();
        let end = track
            .samples
            .iter()
            .map(|s| s.dts + s.cts + s.duration)
            .max()
            .unwrap();
        return Window {
            start: exact(first, track.scale, common),
            duration: exact(end - first, track.scale, common),
            media: first,
            input: 0,
        };
    }
    let mut start = 0;
    let mut windows = Vec::new();
    for edit in &track.edits {
        if edit.media >= 0 {
            windows.push(Window {
                start: exact(start, movie.scale, common),
                duration: exact(edit.duration, movie.scale, common),
                media: edit.media,
                input: 0,
            });
        }
        start += edit.duration;
    }
    assert_eq!(windows.len(), 1, "one simple source presentation edit");
    windows.remove(0)
}

#[derive(Clone, Debug)]
struct Expected {
    scale: u32,
    offsets: Vec<i64>,
    samples: Vec<Vec<Sample>>,
    edits: Vec<Vec<Edit>>,
    windows: Vec<Vec<Window>>,
    media_bases: Vec<Vec<i64>>,
}

fn edits(windows: &[Window]) -> Vec<Edit> {
    let mut edits: Vec<Edit> = Vec::new();
    let mut at = 0;
    for window in windows {
        assert!(window.start >= at);
        if window.start > at {
            edits.push(Edit {
                duration: window.start - at,
                media: -1,
            });
        }
        edits.push(Edit {
            duration: window.duration,
            media: window.media,
        });
        at = window.start + window.duration;
    }
    edits
}

fn expected(inputs: &[Movie], video_only: bool) -> Expected {
    assert!(!inputs.is_empty());
    let video_scale = inputs[0].tracks[0].scale;
    let audio_scale = if video_only {
        video_scale
    } else {
        inputs[0].tracks[1].scale
    };
    let common = u32::try_from(
        i64::from(video_scale) / gcd(i64::from(video_scale), i64::from(audio_scale))
            * i64::from(audio_scale),
    )
    .unwrap();
    let mut micros = 0;
    let offsets: Vec<_> = inputs
        .iter()
        .map(|input| {
            let offset = nearest(micros, 1_000_000, video_scale);
            let span_scale = u32::try_from(
                i64::from(video_scale)
                    / gcd(i64::from(video_scale), i64::from(input.tracks[1].scale))
                    * i64::from(input.tracks[1].scale),
            )
            .unwrap();
            let spans: Vec<_> = input
                .tracks
                .iter()
                .map(|track| source_window(input, track, span_scale))
                .collect();
            let start = spans.iter().map(|window| window.start).min().unwrap();
            let end = spans
                .iter()
                .map(|window| window.start + window.duration)
                .max()
                .unwrap();
            micros += nearest(end, span_scale, 1_000_000) - nearest(start, span_scale, 1_000_000);
            exact(offset, video_scale, common)
        })
        .collect();
    let count = if video_only { 1 } else { 2 };
    let mut samples = Vec::new();
    let mut all_windows = Vec::new();
    let mut all_edits = Vec::new();
    let mut all_bases = Vec::new();
    for stream in 0..count {
        let mut windows: Vec<_> = inputs
            .iter()
            .enumerate()
            .map(|(i, input)| {
                let track = &input.tracks[stream];
                assert_eq!(track.scale, inputs[0].tracks[stream].scale);
                assert_eq!(track.config, inputs[0].tracks[stream].config);
                let mut window = source_window(input, track, common);
                window.start += offsets[i];
                window.input = i;
                if !track.video {
                    assert!(window.media >= aac_frame(&track.config) as i64);
                }
                window
            })
            .collect();
        let mut data = Vec::new();
        let mut base = 0;
        let mut bases = Vec::new();
        for i in 0..inputs.len() {
            let track = &inputs[i].tracks[stream];
            bases.push(base);
            let extension = if stream == 0 && i + 1 < inputs.len() {
                let gap = windows[i + 1].start - windows[i].start - windows[i].duration;
                assert!(gap >= 0);
                windows[i].duration += gap;
                exact(gap, common, track.scale)
            } else {
                0
            };
            let mut own = track.samples.clone();
            for sample in &mut own {
                sample.dts += base;
            }
            own.last_mut().unwrap().duration += extension;
            windows[i].media += base;
            base += track.duration as i64 + extension;
            data.extend(own);
        }
        let mut composed = edits(&windows);
        let scale = inputs[0].tracks[stream].scale;
        let mut merged: Vec<Edit> = Vec::new();
        for edit in composed.drain(..) {
            if let Some(previous) = merged.last_mut()
                && previous.media >= 0
                && edit.media >= 0
                && previous.media + exact(previous.duration, common, scale) == edit.media
            {
                previous.duration += edit.duration;
                continue;
            }
            merged.push(edit);
        }
        samples.push(data);
        all_windows.push(windows);
        all_edits.push(merged);
        all_bases.push(bases);
    }
    Expected {
        scale: common,
        offsets,
        samples,
        edits: all_edits,
        windows: all_windows,
        media_bases: all_bases,
    }
}

fn decimal(value: i64, scale: u32) -> String {
    assert!(value >= 0);
    let micros = nearest(value, scale, 1_000_000);
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

fn hash(bytes: &[u8]) -> String {
    format!("SHA256:{:x}", Sha256::digest(bytes))
}

fn assert_nals(sample: &Sample, config: &[u8]) {
    assert_eq!(config[0], 1);
    let width = usize::from(config[4] & 3) + 1;
    assert!(matches!(width, 1 | 2 | 4));
    let mut at = 0;
    while at < sample.data.len() {
        let count = uint(&sample.data[at..at + width]) as usize;
        at += width;
        let nal = &sample.data[at..at + count];
        assert!(!nal.is_empty());
        assert!(
            !matches!(nal[0] & 31, 7 | 8),
            "configuration-only avc1 excludes in-band SPS/PPS"
        );
        at += count;
    }
}

fn packet_views(document: &Value, movie: &Movie, bytes: &[u8]) {
    let packets = document["packets"].as_array().unwrap();
    let mut indices = vec![0; movie.tracks.len()];
    for packet in packets {
        let stream = packet["stream_index"].as_u64().unwrap() as usize;
        let sample = &movie.tracks[stream].samples[indices[stream]];
        assert_eq!(packet["pts"], sample.dts + sample.cts);
        assert_eq!(packet["dts"], sample.dts);
        assert_eq!(packet["duration"], sample.duration);
        assert_eq!(packet["size"], sample.data.len().to_string());
        assert_eq!(packet["pos"], sample.pos.to_string());
        assert_eq!(packet["data_hash"], hash(&sample.data));
        assert_eq!(packet["flags"], if sample.sync { "K__" } else { "___" });
        assert_eq!(
            &bytes[sample.pos..sample.pos + sample.data.len()],
            sample.data
        );
        if let Some(side_data) = packet.get("side_data_list") {
            for side in side_data.as_array().unwrap() {
                assert_eq!(side["side_data_type"], "Skip Samples");
                assert_eq!(
                    side["skip_samples"], 0,
                    "raw media origin must not skip priming"
                );
                assert_eq!(side["discard_padding"], 1024 - sample.duration);
                assert_eq!(side["skip_reason"], 0);
                assert_eq!(side["discard_reason"], 0);
                assert_eq!(indices[stream] + 1, movie.tracks[stream].samples.len());
            }
        }
        indices[stream] += 1;
    }
    assert_eq!(
        indices,
        movie
            .tracks
            .iter()
            .map(|track| track.samples.len())
            .collect::<Vec<_>>()
    );
    let ordering: String = packets
        .iter()
        .map(|packet| {
            char::from_digit(packet["stream_index"].as_u64().unwrap() as u32, 10).unwrap()
        })
        .collect();
    eprintln!("complete raw global stream-index order={ordering}");
}

fn complete_packets(
    output: &Value,
    source_documents: &[Value],
    inputs: &[Movie],
    wanted: &Expected,
    actual: &Movie,
) {
    for stream in 0..actual.tracks.len() {
        let actual_packets: Vec<_> = output["packets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|packet| packet["stream_index"] == stream)
            .collect();
        let mut at = 0;
        for (input, document) in source_documents.iter().enumerate() {
            let packets: Vec<_> = document["packets"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|packet| packet["stream_index"] == stream)
                .collect();
            assert_eq!(packets.len(), inputs[input].tracks[stream].samples.len());
            for packet in packets {
                let sample = &wanted.samples[stream][at];
                let stored = &actual.tracks[stream].samples[at];
                let mut expected = packet.clone();
                expected["pts"] = json!(sample.dts + sample.cts);
                expected["pts_time"] = json!(decimal(
                    sample.dts + sample.cts,
                    actual.tracks[stream].scale
                ));
                expected["dts"] = json!(sample.dts);
                expected["dts_time"] = json!(decimal(sample.dts, actual.tracks[stream].scale));
                expected["duration"] = json!(sample.duration);
                expected["duration_time"] =
                    json!(decimal(sample.duration, actual.tracks[stream].scale));
                expected["pos"] = json!(stored.pos.to_string());
                if !actual.tracks[stream].video
                    && at + 1 < actual_packets.len()
                    && sample.duration < 1024
                {
                    let padding = expected
                        .as_object_mut()
                        .unwrap()
                        .remove("side_data_list")
                        .unwrap();
                    assert_eq!(
                        padding,
                        json!([{"side_data_type":"Skip Samples", "skip_samples":0, "discard_padding":1024-sample.duration, "skip_reason":0, "discard_reason":0}])
                    );
                }
                assert_eq!(
                    *actual_packets[at], expected,
                    "stream {stream} sample {at} complete raw packet JSON"
                );
                at += 1;
            }
        }
        assert_eq!(at, actual_packets.len());
    }
}

fn structure(actual: &Movie, inputs: &[Movie], wanted: &Expected) {
    assert_eq!(actual.scale, wanted.scale);
    assert_eq!(actual.tracks.len(), wanted.samples.len());
    let mut end = 0;
    for (index, track) in actual.tracks.iter().enumerate() {
        let first = &inputs[0].tracks[index];
        assert_eq!(track.scale, first.scale);
        assert_eq!(track.video, first.video);
        assert_eq!(track.config, first.config);
        if track.video {
            assert_eq!(
                track.btrt, first.btrt,
                "declared source video btrt is copied, not replaced by a host constant"
            );
        } else {
            assert_eq!(
                track.btrt, None,
                "the existing reconstructed mp4a layout has esds and no audio btrt"
            );
        }
        assert_eq!(
            track.edits, wanted.edits[index],
            "independently derived priming windows and empty gaps"
        );
        assert_eq!(
            track.duration as i64,
            wanted.samples[index]
                .iter()
                .map(|sample| sample.duration)
                .sum::<i64>()
        );
        assert_eq!(track.samples.len(), wanted.samples[index].len());
        for (number, (sample, expected)) in
            track.samples.iter().zip(&wanted.samples[index]).enumerate()
        {
            assert_eq!(
                sample.data, expected.data,
                "stream {index} stored packet {number} complete payload"
            );
            assert_eq!(
                (sample.dts, sample.cts, sample.duration, sample.sync),
                (expected.dts, expected.cts, expected.duration, expected.sync),
                "stream {index} stored packet {number} clocks and flags"
            );
            if track.video {
                assert_nals(sample, &track.config);
            }
        }
        end = end.max(track.edits.iter().map(|edit| edit.duration).sum::<i64>());
    }
    assert_eq!(actual.duration as i64, end);
}

fn metadata(output: &Value, source: &Value, movie: &Movie, candidate: &Path, bytes: &[u8]) {
    let mut streams = source["streams"].clone();
    streams.as_array_mut().unwrap().truncate(movie.tracks.len());
    for (index, track) in movie.tracks.iter().enumerate() {
        let start = track
            .samples
            .iter()
            .map(|sample| sample.dts + sample.cts)
            .min()
            .unwrap();
        let stream = &mut streams[index];
        stream["start_pts"] = json!(start);
        stream["start_time"] = json!(decimal(start, track.scale));
        stream["duration_ts"] = json!(track.duration);
        stream["duration"] = json!(decimal(track.duration as i64, track.scale));
        stream["nb_frames"] = json!(track.samples.len().to_string());
        stream["nb_read_packets"] = json!(track.samples.len().to_string());
        let size: usize = track.samples.iter().map(|sample| sample.data.len()).sum();
        stream["bit_rate"] = json!(
            (size as u128 * 8 * u128::from(track.scale) / u128::from(track.duration)).to_string()
        );
        if track.video {
            let numerator = track.samples.len() as i64 * i64::from(track.scale);
            let divisor = gcd(numerator, track.duration as i64);
            stream["avg_frame_rate"] = json!(format!(
                "{}/{}",
                numerator / divisor,
                track.duration as i64 / divisor
            ));
        }
        eprintln!(
            "stream {index} calculated raw metadata start={start} duration={} frames={} bitrate={}",
            track.duration,
            track.samples.len(),
            stream["bit_rate"]
        );
    }
    assert_eq!(
        output["streams"], streams,
        "complete raw stream JSON after closed, sample-derived clock/count/bitrate changes"
    );
    let mut format = source["format"].clone();
    format["filename"] = json!(candidate.to_str().unwrap());
    format["size"] = json!(bytes.len().to_string());
    format["nb_streams"] = json!(movie.tracks.len());
    let common = movie.scale;
    let start = movie
        .tracks
        .iter()
        .map(|track| {
            let pts = track
                .samples
                .iter()
                .map(|sample| sample.dts + sample.cts)
                .min()
                .unwrap();
            nearest(pts, track.scale, common)
        })
        .min()
        .unwrap();
    let end = movie
        .tracks
        .iter()
        .map(|track| {
            let pts = track
                .samples
                .iter()
                .map(|sample| sample.dts + sample.cts + sample.duration)
                .max()
                .unwrap();
            nearest(pts, track.scale, common)
        })
        .max()
        .unwrap();
    format["start_time"] = json!(decimal(start, common));
    format["duration"] = json!(decimal(end - start, common));
    let micros = nearest(end - start, common, 1_000_000);
    format["bit_rate"] = json!((bytes.len() as u128 * 8_000_000 / micros as u128).to_string());
    assert_eq!(
        output["format"], format,
        "complete raw format JSON after calculated size/path/clock differences"
    );
}

fn frames(document: &Value, video: bool) -> Vec<&Value> {
    document["frames"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|frame| frame["media_type"] == if video { "video" } else { "audio" })
        .collect()
}

fn retain(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn audio_presentation(
    oracle: &Oracle,
    candidate: &Path,
    movie: &Movie,
    wanted: &Expected,
    presented_counts: &[usize],
    evidence: &Path,
) {
    let track = &movie.tracks[1];
    let raw = oracle.decode(candidate, true, false);
    let frame_document = oracle.probe(candidate, true, true);
    retain(&evidence.join("raw-frames.json"), &frame_document);
    let frames = frames(&frame_document, false);
    assert_eq!(frames.len(), track.samples.len());
    let channels = frames[0]["channels"].as_u64().unwrap() as usize;
    let mut offsets = Vec::new();
    let mut samples = 0usize;
    for (index, (frame, packet)) in frames.iter().zip(&track.samples).enumerate() {
        assert_eq!(frame["best_effort_timestamp"], packet.dts);
        let count = if index + 1 == frames.len() {
            packet.duration.min(1024) as usize
        } else {
            1024
        };
        assert_eq!(frame["nb_samples"], count);
        offsets.push(samples);
        samples += count;
    }
    assert_eq!(frames[0]["best_effort_timestamp"], 0);
    assert_eq!(
        frames[0]["nb_samples"], 1024,
        "continuous raw origin has no implicit priming skip"
    );
    assert_eq!(raw.len(), samples * channels * 8);
    assert_eq!(presented_counts.len(), wanted.windows[1].len());
    let mut presented = Vec::new();
    let mut cursor = 0;
    for (window, count) in wanted.windows[1].iter().zip(presented_counts) {
        assert_eq!(
            exact(window.duration, wanted.scale, track.scale) as usize,
            *count
        );
        let gap = exact(window.start - cursor, wanted.scale, track.scale) as usize;
        presented.resize(presented.len() + gap * channels * 8, 0);
        let end = window.media + *count as i64;
        let before = presented.len();
        let mut ranges = Vec::new();
        for (index, sample) in track.samples.iter().enumerate() {
            let first = window.media.max(sample.dts);
            let last = end.min(sample.dts + sample.duration);
            if first >= last {
                continue;
            }
            let start = offsets[index] + (first - sample.dts) as usize;
            let finish = start + (last - first) as usize;
            assert!(
                finish <= offsets[index] + frames[index]["nb_samples"].as_u64().unwrap() as usize
            );
            ranges.push((index, start, finish));
            presented.extend_from_slice(&raw[start * channels * 8..finish * channels * 8]);
        }
        assert_eq!(presented.len() - before, count * channels * 8);
        assert_eq!(
            window.media - wanted.media_bases[1][window.input],
            1024,
            "every source excludes its own complete priming frame"
        );
        retain(
            &evidence.join(format!("audio-window-{}.json", window.input)),
            &json!({"start":window.start,"duration":window.duration,"media":window.media,"count":count,"gap":gap,"raw_frame_ranges":&ranges}),
        );
        eprintln!(
            "input {} audio window start={} media={} count={} gap={} raw-first={:?} raw-last={:?}",
            window.input,
            window.start,
            window.media,
            count,
            gap,
            ranges.first(),
            ranges.last()
        );
        cursor = window.start + window.duration;
    }
    assert_eq!(
        presented.len(),
        exact(cursor, wanted.scale, track.scale) as usize * channels * 8
    );
    eprintln!(
        "continuous raw audio samples={samples} hash={}; independently edited presentation samples={} hash={}",
        hash(&raw),
        presented.len() / (channels * 8),
        hash(&presented)
    );
    let default = oracle.decode(candidate, false, false);
    let first_difference = default.iter().zip(&presented).position(|(a, b)| a != b);
    eprintln!(
        "default edit-honoring audio reader samples={} hash={} versus derived presentation samples={} first-difference={first_difference:?}",
        default.len() / (channels * 8),
        hash(&default),
        presented.len() / (channels * 8)
    );
}

fn video_presentation(
    oracle: &Oracle,
    candidate: &Path,
    input_paths: &[PathBuf],
    inputs: &[Movie],
    wanted: &Expected,
    evidence: &Path,
) {
    let actual = oracle.decode(candidate, true, true);
    let actual_document = oracle.probe(candidate, true, true);
    retain(&evidence.join("raw-video-frames.json"), &actual_document);
    let actual_frames = frames(&actual_document, true);
    let mut expected_pixels = Vec::new();
    let mut expected_pts = Vec::new();
    let mut expected_presentation = Vec::new();
    let mut actual_presentation = Vec::new();
    let mut frame_index = 0;
    for (index, path) in input_paths.iter().enumerate() {
        expected_pixels.extend(oracle.decode(path, true, true));
        let document = oracle.probe(path, true, true);
        retain(
            &evidence.join(format!("input-{index}.raw-frames.json")),
            &document,
        );
        let input_frames = frames(&document, true);
        assert_eq!(input_frames.len(), inputs[index].tracks[0].samples.len());
        for frame in input_frames {
            let pts = frame["best_effort_timestamp"].as_i64().unwrap();
            expected_pts.push(pts + wanted.media_bases[0][index]);
            let raw_pts = actual_frames[frame_index]["best_effort_timestamp"]
                .as_i64()
                .unwrap();
            let window = &wanted.windows[0][index];
            actual_presentation.push(
                window.start
                    + exact(
                        raw_pts - window.media,
                        inputs[index].tracks[0].scale,
                        wanted.scale,
                    ),
            );
            frame_index += 1;
        }
        let presented = oracle.probe(path, false, true);
        retain(
            &evidence.join(format!("input-{index}.default-frames.json")),
            &presented,
        );
        for frame in frames(&presented, true) {
            let pts = frame["best_effort_timestamp"].as_i64().unwrap();
            let placed =
                exact(pts, inputs[index].tracks[0].scale, wanted.scale) + wanted.offsets[index];
            let window = &wanted.windows[0][index];
            assert!(placed >= window.start && placed < window.start + window.duration);
            expected_presentation.push(placed);
        }
    }
    assert_eq!(actual.len(), expected_pixels.len());
    assert_eq!(
        actual
            .iter()
            .zip(&expected_pixels)
            .position(|(a, b)| a != b),
        None,
        "complete video pixels with independent IDR-reset input decodes"
    );
    assert_eq!(
        actual_frames
            .iter()
            .map(|frame| frame["best_effort_timestamp"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        expected_pts
    );
    assert_eq!(
        actual_presentation, expected_presentation,
        "every video frame presents at the input's own timestamp plus independently rounded placement"
    );
}

pub fn verify(
    oracle: &Oracle,
    input_paths: &[PathBuf],
    candidate: &Path,
    video_only: bool,
    presented_audio_counts: &[usize],
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::var_os("COREAV_CONCAT_PRESENTATION_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| candidate.parent().unwrap().join("evidence"));
    let evidence = root.join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&evidence).unwrap();
    std::fs::copy(candidate, evidence.join("output.mp4")).unwrap();
    for (index, path) in input_paths.iter().enumerate() {
        std::fs::copy(path, evidence.join(format!("input-{index}.mp4"))).unwrap();
    }
    let inputs: Vec<_> = input_paths
        .iter()
        .map(|path| parse(&std::fs::read(path).unwrap()))
        .collect();
    let wanted = expected(&inputs, video_only);
    let bytes = std::fs::read(candidate).unwrap();
    let actual = parse(&bytes);
    structure(&actual, &inputs, &wanted);
    let media = child(&bytes, b"mdat");
    let mut position = media.as_ptr() as usize - bytes.as_ptr() as usize;
    let end = position + media.len();
    let mut physical: Vec<_> = actual
        .tracks
        .iter()
        .enumerate()
        .flat_map(|(stream, track)| {
            track
                .samples
                .iter()
                .enumerate()
                .map(move |(index, sample)| (sample.pos, stream, index, sample.data.len()))
        })
        .collect();
    physical.sort();
    for (pos, _, _, size) in &physical {
        assert_eq!(
            *pos, position,
            "complete stored samples tile the output mdat with no hidden bytes or omitted packet"
        );
        position += size;
    }
    assert_eq!(position, end);
    retain(&evidence.join("physical-order.json"), &json!(&physical));
    eprintln!(
        "physical packets={} media-end={end} evidence={}",
        physical.len(),
        evidence.display()
    );
    let output = oracle.probe(candidate, true, false);
    retain(&evidence.join("output.raw.json"), &output);
    packet_views(&output, &actual, &bytes);
    let mut sources = Vec::new();
    for (index, path) in input_paths.iter().enumerate() {
        let document = oracle.probe(path, true, false);
        retain(&evidence.join(format!("input-{index}.raw.json")), &document);
        packet_views(&document, &inputs[index], &std::fs::read(path).unwrap());
        sources.push(document);
    }
    complete_packets(&output, &sources, &inputs, &wanted, &actual);
    metadata(&output, &sources[0], &actual, candidate, &bytes);
    video_presentation(oracle, candidate, input_paths, &inputs, &wanted, &evidence);
    if !video_only {
        audio_presentation(
            oracle,
            candidate,
            &actual,
            &wanted,
            presented_audio_counts,
            &evidence,
        );
    }
    let default = oracle.probe(candidate, false, false);
    retain(&evidence.join("output.default.json"), &default);
    eprintln!(
        "default reader start={} duration={} packets={}",
        default["format"]["start_time"],
        default["format"]["duration"],
        default["packets"].as_array().unwrap().len()
    );
    eprintln!(
        "independent placement offsets={:?} movie-timescale={} output windows={:?}",
        wanted.offsets, wanted.scale, wanted.windows
    );
}

#[cfg(test)]
fn synthetic() -> Movie {
    let sample = |dts| Sample {
        dts,
        cts: 0,
        duration: 1024,
        sync: true,
        pos: 0,
        data: vec![0, 0, 0, 1, 0x65],
    };
    let video = Track {
        video: true,
        scale: 48_000,
        duration: 2048,
        edits: vec![Edit {
            duration: 2048,
            media: 0,
        }],
        samples: vec![sample(0), sample(1024)],
        config: vec![1, 0, 0, 0, 255],
        btrt: None,
    };
    let audio = Track {
        video: false,
        scale: 48_000,
        duration: 3072,
        edits: vec![Edit {
            duration: 2048,
            media: 1024,
        }],
        samples: vec![sample(0), sample(1024), sample(2048)],
        config: vec![0x11, 0x88],
        btrt: None,
    };
    Movie {
        scale: 48_000,
        duration: 2048,
        tracks: vec![video, audio],
    }
}

#[test]
fn payload_priming_window_gap_and_clock_mutations_fail() {
    let source = synthetic();
    let inputs = vec![source.clone(), source];
    let wanted = expected(&inputs, false);
    let actual = Movie {
        scale: wanted.scale,
        duration: 4096,
        tracks: (0..2)
            .map(|index| {
                let mut track = inputs[0].tracks[index].clone();
                track.edits = wanted.edits[index].clone();
                track.samples = wanted.samples[index].clone();
                track.duration = track
                    .samples
                    .iter()
                    .map(|sample| sample.duration as u64)
                    .sum();
                track
            })
            .collect(),
    };
    structure(&actual, &inputs, &wanted);
    for change in 0..8 {
        let mut broken = actual.clone();
        match change {
            0 => broken.tracks[0].samples[0].data[4] ^= 1,
            1 => broken.tracks[1].edits[0].media += 1,
            2 => broken.tracks[1].edits[0].duration += 1,
            3 => broken.tracks[1].edits.insert(
                1,
                Edit {
                    duration: 1,
                    media: -1,
                },
            ),
            4 => broken.tracks[0].samples[0].cts += 1,
            5 => broken.tracks[0].samples[1].duration += 1,
            6 => broken.tracks[0].btrt = Some(vec![1]),
            7 => broken.tracks[1].btrt = Some(vec![1]),
            _ => unreachable!(),
        }
        assert!(std::panic::catch_unwind(|| structure(&broken, &inputs, &wanted)).is_err());
    }
}

#[test]
fn unknown_stream_and_format_metadata_mutations_fail() {
    let mut movie = synthetic();
    movie.tracks.truncate(1);
    let source = json!({"streams":[{}],"format":{}});
    let output = json!({"streams":[{"start_pts":0,"start_time":"0.000000","duration_ts":2048,"duration":"0.042667","nb_frames":"2","nb_read_packets":"2","bit_rate":"1875","avg_frame_rate":"375/8"}],
        "format":{"filename":"output.mp4","size":"10","nb_streams":1,"start_time":"0.000000","duration":"0.042667","bit_rate":"1874"}});
    metadata(&output, &source, &movie, Path::new("output.mp4"), &[0; 10]);
    for field in ["streams", "format"] {
        let mut broken = output.clone();
        if field == "streams" {
            broken[field][0]["hidden"] = json!(1);
        } else {
            broken[field]["hidden"] = json!(1);
        }
        assert!(
            std::panic::catch_unwind(|| metadata(
                &broken,
                &source,
                &movie,
                Path::new("output.mp4"),
                &[0; 10]
            ))
            .is_err()
        );
    }
}

#[test]
fn raw_format_duration_is_the_span_after_a_nonzero_start() {
    let mut movie = synthetic();
    movie.tracks.truncate(1);
    for sample in &mut movie.tracks[0].samples {
        sample.cts = 1024;
    }
    let source = json!({"streams":[{}],"format":{}});
    let output = json!({"streams":[{"start_pts":1024,"start_time":"0.021333","duration_ts":2048,"duration":"0.042667","nb_frames":"2","nb_read_packets":"2","bit_rate":"1875","avg_frame_rate":"375/8"}],
        "format":{"filename":"output.mp4","size":"10","nb_streams":1,"start_time":"0.021333","duration":"0.042667","bit_rate":"1874"}});
    metadata(&output, &source, &movie, Path::new("output.mp4"), &[0; 10]);
    let mut absolute_end = output;
    absolute_end["format"]["duration"] = json!("0.064000");
    absolute_end["format"]["bit_rate"] = json!("1250");
    assert!(
        std::panic::catch_unwind(|| metadata(
            &absolute_end,
            &source,
            &movie,
            Path::new("output.mp4"),
            &[0; 10]
        ))
        .is_err()
    );
}
