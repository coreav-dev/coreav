use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ops::Range;
use std::path::Path;
use std::process::{Command, Output};

struct Track {
    handler: [u8; 4],
    media_clock: (u32, u64),
    avcc: Option<Vec<u8>>,
    bitrate: Option<u32>,
}

fn integer(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0, |value, byte| (value << 8) | u64::from(*byte))
}

fn boxes(bytes: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut remaining = bytes;
    let mut result = Vec::new();
    while !remaining.is_empty() {
        assert!(remaining.len() >= 8, "truncated box header");
        let (size, header) = match integer(&remaining[..4]) {
            0 => (remaining.len(), 8),
            1 => {
                assert!(remaining.len() >= 16);
                (usize::try_from(integer(&remaining[8..16])).unwrap(), 16)
            }
            size => (usize::try_from(size).unwrap(), 8),
        };
        assert!(
            size >= header && size <= remaining.len(),
            "invalid box extent"
        );
        result.push((
            remaining[4..8].try_into().unwrap(),
            &remaining[header..size],
        ));
        remaining = &remaining[size..];
    }
    result
}

fn child<'a>(bytes: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
    let found: Vec<_> = boxes(bytes)
        .into_iter()
        .filter(|(tag, _)| tag == kind)
        .collect();
    assert_eq!(found.len(), 1, "expected one {kind:?}");
    found[0].1
}

fn movie(bytes: &[u8]) -> (Range<usize>, Vec<Track>) {
    let media = child(bytes, b"mdat");
    let start = media.as_ptr() as usize - bytes.as_ptr() as usize;
    let tracks = boxes(child(bytes, b"moov"))
        .into_iter()
        .filter(|(tag, _)| tag == b"trak")
        .map(|(_, trak)| {
            let mdia = child(trak, b"mdia");
            let mdhd = child(mdia, b"mdhd");
            let media_clock = match mdhd[0] {
                0 => (integer(&mdhd[12..16]) as u32, integer(&mdhd[16..20])),
                1 => (integer(&mdhd[20..24]) as u32, integer(&mdhd[24..32])),
                version => panic!("unexpected mdhd version {version}"),
            };
            let handler = child(mdia, b"hdlr")[8..12].try_into().unwrap();
            let stsd = child(child(child(mdia, b"minf"), b"stbl"), b"stsd");
            assert_eq!(integer(&stsd[4..8]), 1);
            let entries = boxes(&stsd[8..]);
            assert_eq!(entries.len(), 1);
            let (tag, entry) = entries[0];
            let (avcc, bitrate) = if handler == *b"vide" {
                assert_eq!(tag, *b"avc1", "configuration-only AVC must not use avc3");
                let children = &entry[78..];
                (
                    Some(child(children, b"avcC").to_vec()),
                    boxes(children)
                        .into_iter()
                        .find(|(tag, _)| tag == b"btrt")
                        .map(|(_, payload)| integer(&payload[8..12]) as u32),
                )
            } else {
                assert_eq!(handler, *b"soun");
                (None, None)
            };
            Track {
                handler,
                media_clock,
                avcc,
                bitrate,
            }
        })
        .collect();
    (start..start + media.len(), tracks)
}

fn parameter_sets(avcc: &[u8]) -> (usize, Vec<&[u8]>) {
    assert_eq!(avcc[0], 1);
    assert_eq!(avcc[4] & 0xfc, 0xfc);
    assert_eq!(avcc[5] & 0xe0, 0xe0);
    let width = usize::from(avcc[4] & 3) + 1;
    assert!(matches!(width, 1 | 2 | 4));
    let mut at = 6;
    let mut count = usize::from(avcc[5] & 31);
    let mut sets = Vec::new();
    for kind in [7, 8] {
        assert!(count > 0);
        for _ in 0..count {
            let length = usize::try_from(integer(&avcc[at..at + 2])).unwrap();
            at += 2;
            let nal = &avcc[at..at + length];
            assert!(!nal.is_empty());
            assert_eq!(nal[0] & 31, kind);
            sets.push(nal);
            at += length;
        }
        if kind == 7 {
            count = usize::from(avcc[at]);
            at += 1;
        }
    }
    (width, sets)
}

fn strip(bytes: &[u8], avcc: &[u8], native: bool) -> (Vec<u8>, Vec<u8>) {
    let (width, sets) = parameter_sets(avcc);
    let mut result = Vec::new();
    let mut removed = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let length = usize::try_from(integer(&bytes[at..at + width])).unwrap();
        let end = at.checked_add(width).unwrap().checked_add(length).unwrap();
        let nal = &bytes[at + width..end];
        assert!(!nal.is_empty());
        let kind = nal[0] & 31;
        if matches!(kind, 7 | 8) {
            assert!(!native, "native avc1 sample contains SPS/PPS");
            assert!(
                sets.contains(&nal),
                "oracle parameter set differs from avcC"
            );
            removed.push(kind);
        } else {
            result.extend_from_slice(&bytes[at..end]);
        }
        at = end;
    }
    (result, removed)
}

fn normalized(stderr: &[u8]) -> String {
    let text = String::from_utf8(stderr.to_vec()).unwrap();
    let mut result = String::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("@ 0x") {
        result.push_str(&rest[..at]);
        result.push('@');
        rest = &rest[at + 4..];
        let digits = rest.bytes().take_while(u8::is_ascii_hexdigit).count();
        assert!(digits > 0);
        rest = &rest[digits..];
    }
    result.push_str(rest);
    result
}

fn probe(program: &Path, path: &Path) -> (Value, String) {
    let output = Command::new(program)
        .args(["-v", "error"])
        .args(include_str!("../../../../scripts/oracle-audio-args.txt").split_whitespace())
        .arg("-i")
        .arg(path)
        .args([
            "-show_format",
            "-show_streams",
            "-show_packets",
            "-show_data",
            "-show_data_hash",
            "sha256",
            "-of",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        serde_json::from_slice(&output.stdout).unwrap(),
        normalized(&output.stderr),
    )
}

fn packet_bytes<'a>(packet: &Value, bytes: &'a [u8]) -> &'a [u8] {
    let at: usize = packet["pos"].as_str().unwrap().parse().unwrap();
    let size: usize = packet["size"].as_str().unwrap().parse().unwrap();
    let payload = &bytes[at..at.checked_add(size).unwrap()];
    assert_eq!(dump_bytes(&packet["data"]), payload);
    assert_eq!(
        packet["data_hash"],
        format!("SHA256:{:x}", Sha256::digest(payload))
    );
    payload
}

fn dump_bytes(value: &Value) -> Vec<u8> {
    let mut result = Vec::new();
    for line in value
        .as_str()
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
    {
        let (offset, body) = line.split_once(": ").unwrap();
        assert_eq!(usize::from_str_radix(offset, 16).unwrap(), result.len());
        let hex = body.split_once("  ").unwrap().0.replace(' ', "");
        assert_eq!(hex.len() % 2, 0);
        result.extend(
            (0..hex.len())
                .step_by(2)
                .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap()),
        );
    }
    result
}

fn bitrate(size: usize, clock: (u32, u64)) -> String {
    assert!(clock.0 > 0 && clock.1 > 0);
    (size as u128 * 8 * u128::from(clock.0) / u128::from(clock.1)).to_string()
}

fn formats(
    actual: &Value,
    expected: &Value,
    candidate: &Path,
    reference: &Path,
    sizes: (usize, usize),
) {
    assert_eq!(actual["tags"]["encoder"], "Lavf63.1.102");
    assert_eq!(expected["tags"]["encoder"], actual["tags"]["encoder"]);
    for (value, path, bytes) in [(actual, candidate, sizes.0), (expected, reference, sizes.1)] {
        assert_eq!(value["filename"], path.to_str().unwrap());
        assert_eq!(value["size"], bytes.to_string());
        let (seconds, fraction) = value["duration"].as_str().unwrap().split_once('.').unwrap();
        assert_eq!(fraction.len(), 6);
        let micros =
            seconds.parse::<u128>().unwrap() * 1_000_000 + fraction.parse::<u128>().unwrap();
        assert_eq!(
            value["bit_rate"],
            ((bytes as u128 * 8_000_000) / micros).to_string()
        );
    }
    eprintln!(
        "format size native={} oracle={}; bitrate native={} oracle={}",
        sizes.0, sizes.1, actual["bit_rate"], expected["bit_rate"]
    );
    let mut adjusted = expected.clone();
    for field in ["filename", "size", "bit_rate"] {
        adjusted[field] = actual[field].clone();
    }
    assert_eq!(
        *actual, adjusted,
        "complete format metadata after explicit physical-size and path differences"
    );
}

fn packet_record(native: &Value, oracle: &Value, pos: usize, payload: &[u8], video: bool) {
    let mut adjusted = oracle.clone();
    adjusted["pos"] = json!(pos.to_string());
    if video {
        adjusted["size"] = json!(payload.len().to_string());
        adjusted["data"] = native["data"].clone();
        adjusted["data_hash"] = json!(format!("SHA256:{:x}", Sha256::digest(payload)));
    }
    assert_eq!(*native, adjusted, "complete aligned packet record");
}

fn streams(actual: &Value, expected: &Value, tracks: &[Track], sizes: &[(usize, usize)]) {
    let mut adjusted = expected.clone();
    for (index, track) in tracks.iter().enumerate() {
        if track.avcc.is_some() {
            assert_eq!(actual[index]["codec_tag_string"], "avc1");
            assert_eq!(actual[index]["codec_tag"], "0x31637661");
            let native = bitrate(sizes[index].0, track.media_clock);
            let oracle = bitrate(sizes[index].1, track.media_clock);
            assert_eq!(actual[index]["bit_rate"], native);
            assert_eq!(expected[index]["bit_rate"], oracle);
            assert_ne!(
                native, oracle,
                "the measured SPS/PPS payload difference remains visible"
            );
            eprintln!("stream {index} bit_rate native={native} oracle={oracle}");
            adjusted[index]["bit_rate"] = json!(native);
        }
        if expected[index]["codec_name"] == "alac" {
            let native = dump_bytes(&actual[index]["extradata"]);
            let oracle = dump_bytes(&expected[index]["extradata"]);
            assert_eq!(oracle.len(), 36);
            let channels = expected[index]["channels"].as_u64().unwrap();
            assert!(matches!(channels, 1 | 2));
            assert_eq!(
                integer(&oracle[24..28]),
                channels * 8192 + 4,
                "source maximum ALAC frame bytes"
            );
            assert_eq!(
                integer(&oracle[28..32]),
                channels * 768_000,
                "oracle retains the source average bitrate"
            );
            let mut combined = oracle.clone();
            combined[28..32].fill(0);
            assert_eq!(
                native, combined,
                "joined ALAC average bitrate is explicitly unknown; all other configuration bytes are preserved"
            );
            for (stream, bytes) in [(&actual[index], &native), (&expected[index], &oracle)] {
                assert_eq!(
                    stream["extradata_hash"],
                    format!("SHA256:{:x}", Sha256::digest(bytes))
                );
            }
            eprintln!("stream {index} ALAC extradata native={native:02x?} oracle={oracle:02x?}");
            adjusted[index]["extradata"] = actual[index]["extradata"].clone();
            adjusted[index]["extradata_hash"] = actual[index]["extradata_hash"].clone();
        }
        if expected[index]["codec_name"] == "flac" {
            let native = dump_bytes(&actual[index]["extradata"]);
            let oracle = dump_bytes(&expected[index]["extradata"]);
            assert_eq!(oracle.len(), 34);
            assert_eq!(integer(&oracle[..2]), 1024);
            assert_eq!(integer(&oracle[2..4]), 1024);
            assert_eq!(integer(&oracle[4..7]), 269);
            assert_eq!(integer(&oracle[7..10]), 383);
            assert_eq!(integer(&oracle[10..18]) & ((1 << 36) - 1), 96_000);
            assert_ne!(&oracle[18..], &[0; 16]);
            let mut combined = oracle.clone();
            combined[..2].copy_from_slice(&768u16.to_be_bytes());
            let packed = (integer(&oracle[10..18]) & !((1 << 36) - 1)) | 192_000;
            combined[10..18].copy_from_slice(&packed.to_be_bytes());
            combined[18..].fill(0);
            assert_eq!(
                native, combined,
                "joined FLAC STREAMINFO has combined bounds and total, with unknown MD5"
            );
            for (stream, bytes) in [(&actual[index], &native), (&expected[index], &oracle)] {
                assert_eq!(
                    stream["extradata_hash"],
                    format!("SHA256:{:x}", Sha256::digest(bytes))
                );
            }
            eprintln!("stream {index} FLAC extradata native={native:02x?} oracle={oracle:02x?}");
            adjusted[index]["extradata"] = actual[index]["extradata"].clone();
            adjusted[index]["extradata_hash"] = actual[index]["extradata_hash"].clone();
        }
    }
    assert_eq!(
        *actual, adjusted,
        "complete stream JSON, including all codec configuration and tags"
    );
}

fn decode(program: &Path, path: &Path, stream: usize, video: bool) -> Output {
    let mut command = Command::new(program);
    command
        .args(["-nostdin", "-v", "error", "-xerror", "-noautorotate"])
        .args(include_str!("../../../../scripts/oracle-audio-args.txt").split_whitespace())
        .arg("-i")
        .arg(path)
        .args(["-map", &format!("0:{stream}")]);
    if video {
        command.args([
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
        command.args(["-c:a", "pcm_f64le", "-f", "f64le"]);
    }
    command.arg("-").output().unwrap()
}

fn frame_timing(program: &Path, path: &Path, stream: usize) -> Value {
    let output = Command::new(program)
        .args(["-v", "error"])
        .args(include_str!("../../../../scripts/oracle-audio-args.txt").split_whitespace())
        .arg("-i").arg(path)
        .args(["-select_streams", &stream.to_string(), "-show_entries",
            "format=start_time,duration:stream=index,codec_name,time_base,start_pts,duration_ts,width,height,pix_fmt,sample_rate,channels:frame=stream_index,best_effort_timestamp,duration,nb_samples,width,height,pix_fmt", "-of", "json"])
        .output().unwrap();
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!document["frames"].as_array().unwrap().is_empty());
    document
}

pub fn compare(
    ffmpeg: &Path,
    ffprobe: &Path,
    candidate: &Path,
    reference: &Path,
    list: &Path,
    case: &str,
) {
    let (order, idrs, added_bytes) = match case {
        "01-unedited-av" => ("010011011011011010110110110110101101101101101011011011011010110110110110101101101101101011011011011010110110110101101101101101011011011011010110110110110101101101101101011011011011010110110110110101101101101011011011111010011011011011010110110110110101101101101101011011011011010110110110110101101101101101011011011011010110110110101101101101101011011011011010110110110110101101101101101011011011011010110110110110101101101101011011011111".to_owned(), vec![0,24,48,72,78,102,126,150], 304),
        "gap-short-2-video" => ("0".repeat(156), vec![0,24,48,72,78,102,126,150], 304),
        "gap-short-3-video" => ("0".repeat(234), vec![0,24,48,72,78,102,126,150,156,180,204,228], 456),
        "gap-fractional-2-video" => ("0".repeat(196), vec![0,24,48,72,96,98,122,146,170,194], 360),
        "gap-fractional-3-video" => ("0".repeat(294), vec![0,24,48,72,96,98,122,146,170,194,196,220,244,268,292], 540),
        "filter-av" | "filter-transport-like" => ("01110110110110110110110111011011011011011011".to_owned(), vec![0,7], 72),
        "filter-video" => ("0".repeat(14), vec![0,7], 72),
        "filter-alac" => ("010010010010010010010010".to_owned(), vec![0,8], 72),
        "audio-copy-flac" => ("000110110110110110110110101101101101101101101101011011011011011011011010110110110110110110110101101101101101101101101011011011011011011011010110110110110110110110110110101101101101101101101101011011011011011011011010110110110110110110110101101101101101101101101011011011011011011011011111".to_owned(), vec![0,25,50,75], 156),
        "audio-copy-alac" => ("0001001001001001001001001000100100100100100100100010010010010010010010010001010010010010010010010010001001001001001001001000100100100100100100100101".to_owned(), vec![0,25,50,75], 156),
        "guard-C02" => ("0".repeat(50), vec![0,25], 78),
        _ => panic!("unregistered concat case {case}"),
    };
    let native_bytes = std::fs::read(candidate).unwrap();
    let oracle_bytes = std::fs::read(reference).unwrap();
    let (native_mdat, native_tracks) = movie(&native_bytes);
    let (oracle_mdat, oracle_tracks) = movie(&oracle_bytes);
    let inputs = coreav_cli::concat::read(list, false).unwrap();
    let (_, source_tracks) = movie(&std::fs::read(&inputs[0]).unwrap());
    let source_video = source_tracks
        .iter()
        .find(|track| track.handler == *b"vide")
        .unwrap();
    assert_eq!(native_tracks.len(), oracle_tracks.len());
    for (native, oracle) in native_tracks.iter().zip(&oracle_tracks) {
        assert_eq!(native.handler, oracle.handler);
        assert_eq!(
            native.media_clock, oracle.media_clock,
            "stored mdhd timescale and duration"
        );
        assert_eq!(native.avcc, oracle.avcc, "complete stored avcC");
    }
    let (actual, native_stderr) = probe(ffprobe, candidate);
    let (expected, oracle_stderr) = probe(ffprobe, reference);
    formats(
        &actual["format"],
        &expected["format"],
        candidate,
        reference,
        (native_bytes.len(), oracle_bytes.len()),
    );
    let mut rest = expected.clone();
    for section in ["format", "streams", "packets"] {
        rest[section] = actual[section].clone();
    }
    assert_eq!(actual, rest, "all other exposed document sections");
    assert_eq!(native_stderr, oracle_stderr);
    assert!(native_stderr.is_empty(), "{native_stderr}");
    assert_eq!(
        actual["streams"].as_array().unwrap().len(),
        native_tracks.len()
    );
    assert_eq!(
        expected["streams"].as_array().unwrap().len(),
        oracle_tracks.len()
    );
    let actual_packets = actual["packets"].as_array().unwrap();
    let expected_packets = expected["packets"].as_array().unwrap();
    let ordering = |packets: &[Value]| -> String {
        packets
            .iter()
            .map(|packet| {
                char::from_digit(packet["stream_index"].as_u64().unwrap() as u32, 10).unwrap()
            })
            .collect()
    };
    eprintln!(
        "{case} native global stream-index order={}",
        ordering(actual_packets)
    );
    eprintln!(
        "{case} oracle global stream-index order={}",
        ordering(expected_packets)
    );
    assert_eq!(ordering(actual_packets), order);
    assert_eq!(ordering(expected_packets), order);
    let mut sizes = vec![(0, 0); native_tracks.len()];
    let mut sample_indices = vec![0; native_tracks.len()];
    let mut observed_idrs = Vec::new();
    let mut removed_before = 0;
    let mut native_at = native_mdat.start;
    let mut oracle_at = oracle_mdat.start;
    let mut positions = Vec::new();
    let mut flac_frames = Vec::new();
    for (native, oracle) in actual_packets.iter().zip(expected_packets) {
        let stream = native["stream_index"].as_u64().unwrap() as usize;
        let native_payload = packet_bytes(native, &native_bytes);
        let oracle_payload = packet_bytes(oracle, &oracle_bytes);
        let native_pos = native["pos"].as_str().unwrap().parse::<usize>().unwrap();
        let oracle_pos = oracle["pos"].as_str().unwrap().parse::<usize>().unwrap();
        assert_eq!(native_pos, native_at, "native samples tile mdat exactly");
        assert_eq!(oracle_pos, oracle_at, "oracle samples tile mdat exactly");
        assert_eq!(
            native_pos - native_mdat.start,
            oracle_pos - oracle_mdat.start - removed_before
        );
        positions.push((stream, sample_indices[stream], native_pos, oracle_pos));
        if let Some(avcc) = &native_tracks[stream].avcc {
            assert_eq!(strip(native_payload, avcc, true).0, native_payload);
            let (stripped, removed) = strip(oracle_payload, avcc, false);
            assert_eq!(
                native_payload, stripped,
                "stream {stream} sample {}",
                sample_indices[stream]
            );
            if !removed.is_empty() {
                assert_eq!(removed, [7, 8]);
                assert!(oracle["flags"].as_str().unwrap().contains('K'));
                observed_idrs.push(sample_indices[stream]);
            }
            removed_before += oracle_payload.len() - native_payload.len();
        } else {
            assert_eq!(native_payload, oracle_payload, "every audio packet byte");
            if case == "audio-copy-flac" {
                let frame = sample_indices[stream];
                assert_eq!(native_payload[0], 0xff);
                assert_eq!(native_payload[1] & 0xfe, 0xf8);
                assert_eq!(
                    native_payload[1] & 1,
                    0,
                    "known MP4 packet-copy deviation shared with the oracle: FLAC retains fixed-block strategy, not standalone-stream conformance"
                );
                assert_eq!(
                    usize::from(native_payload[4]),
                    frame % 94,
                    "known MP4 packet-copy deviation shared with the oracle: the frame number resets at input two"
                );
                let block = match native_payload[2] >> 4 {
                    10 => 1024,
                    7 => integer(&native_payload[5..7]) as usize + 1,
                    code => panic!("unexpected generated FLAC block-size code {code}"),
                };
                assert_eq!(block, if frame % 94 == 93 { 768 } else { 1024 });
                flac_frames.push((block, native_payload.len()));
            }
        }
        packet_record(
            native,
            oracle,
            native_pos,
            native_payload,
            native_tracks[stream].avcc.is_some(),
        );
        sample_indices[stream] += 1;
        sizes[stream].0 += native_payload.len();
        sizes[stream].1 += oracle_payload.len();
        native_at += native_payload.len();
        oracle_at += oracle_payload.len();
    }
    assert_eq!(observed_idrs, idrs);
    assert_eq!(removed_before, added_bytes);
    assert_eq!(native_at, native_mdat.end);
    assert_eq!(oracle_at, oracle_mdat.end);
    if case == "audio-copy-flac" {
        assert_eq!(flac_frames.len(), 188);
        assert_eq!(
            flac_frames.iter().map(|(block, _)| block).sum::<usize>(),
            192_000
        );
        assert_eq!(flac_frames.iter().map(|(block, _)| *block).min(), Some(768));
        assert_eq!(
            flac_frames.iter().map(|(block, _)| *block).max(),
            Some(1024)
        );
        assert_eq!(flac_frames.iter().map(|(_, bytes)| *bytes).min(), Some(269));
        assert_eq!(flac_frames.iter().map(|(_, bytes)| *bytes).max(), Some(383));
    }
    assert_ne!(
        native_mdat.start, oracle_mdat.start,
        "the asserted physical layout difference remains observable"
    );
    eprintln!(
        "{case} positions (stream, sample, native, oracle)={positions:?}; removed={removed_before}"
    );
    streams(
        &actual["streams"],
        &expected["streams"],
        &native_tracks,
        &sizes,
    );
    for (index, (native, oracle)) in native_tracks.iter().zip(&oracle_tracks).enumerate() {
        if native.avcc.is_some() {
            assert_eq!(
                native.bitrate, source_video.bitrate,
                "native btrt retains the selected source value, including zero for unknown or absence; the exposed bitrate is calculated above"
            );
            eprintln!(
                "{case} stored btrt average native={:?} oracle={:?}",
                native.bitrate, oracle.bitrate
            );
            assert_eq!(
                oracle.bitrate.unwrap().to_string(),
                bitrate(sizes[index].1, oracle.media_clock)
            );
        }
        let video = native.handler == *b"vide";
        assert_eq!(
            actual["streams"][index]["codec_type"],
            if video { "video" } else { "audio" }
        );
        let native = decode(ffmpeg, candidate, index, video);
        let oracle = decode(ffmpeg, reference, index, video);
        assert_eq!(native.status.code(), oracle.status.code());
        assert_eq!(normalized(&native.stderr), normalized(&oracle.stderr));
        assert_eq!(native.stdout.len(), oracle.stdout.len());
        assert_eq!(
            native
                .stdout
                .iter()
                .zip(&oracle.stdout)
                .position(|(a, b)| a != b),
            None,
            "stream {index} complete decoded byte offset"
        );
        if case == "filter-transport-like" && !video {
            assert_eq!(native.status.code(), Some(183));
            assert_eq!(native.stdout.len(), 122_880);
            assert!(normalized(&native.stderr).contains("Prediction is not allowed in AAC-LC"));
        } else {
            assert!(
                native.status.success(),
                "{}",
                String::from_utf8_lossy(&native.stderr)
            );
            assert!(native.stderr.is_empty());
            assert!(!native.stdout.is_empty());
            assert_eq!(
                frame_timing(ffprobe, candidate, index),
                frame_timing(ffprobe, reference, index),
                "stream {index} every decoded frame clock and complete format timing"
            );
        }
    }
}

#[test]
fn changed_parameter_sets_and_native_inband_copies_fail() {
    let avcc = [1, 100, 0, 10, 255, 225, 0, 2, 103, 1, 1, 0, 2, 104, 2];
    let sample = [0, 0, 0, 2, 103, 1, 0, 0, 0, 2, 104, 2, 0, 0, 0, 2, 101, 3];
    assert_eq!(
        strip(&sample, &avcc, false),
        (vec![0, 0, 0, 2, 101, 3], vec![7, 8])
    );
    let mut changed = sample;
    changed[5] ^= 1;
    assert!(std::panic::catch_unwind(|| strip(&changed, &avcc, false)).is_err());
    assert!(std::panic::catch_unwind(|| strip(&sample, &avcc, true)).is_err());
}

#[test]
fn hidden_stream_fields_and_unexplained_bitrates_fail() {
    let tracks = [Track {
        handler: *b"vide",
        media_clock: (1, 1),
        avcc: Some(vec![]),
        bitrate: Some(8),
    }];
    let native = json!([{"codec_tag_string":"avc1","codec_tag":"0x31637661","bit_rate":"8","tags":{"encoder":"source"}}]);
    let oracle = json!([{"codec_tag_string":"avc1","codec_tag":"0x31637661","bit_rate":"16","tags":{"encoder":"source"}}]);
    streams(&native, &oracle, &tracks, &[(1, 2)]);
    for (field, value) in [
        ("new_hidden_field", json!(true)),
        ("bit_rate", json!("7")),
        ("codec_tag_string", json!("avc3")),
    ] {
        let mut changed = native.clone();
        changed[0][field] = value;
        assert!(
            std::panic::catch_unwind(|| streams(&changed, &oracle, &tracks, &[(1, 2)])).is_err()
        );
    }
}

#[test]
fn hidden_packet_fields_and_changed_payload_hashes_fail() {
    let payload = [1, 2, 3];
    let packet = json!({"pos":"4", "size":"3", "data_hash":format!("SHA256:{:x}",Sha256::digest(payload)), "pts":0, "duration":1, "flags":"K_"});
    packet_record(&packet, &packet, 4, &payload, false);
    for (field, value) in [
        ("new_hidden_field", json!(true)),
        ("pts", json!(1)),
        ("duration", json!(2)),
        ("flags", json!("__")),
        ("data_hash", json!("SHA256:changed")),
    ] {
        let mut changed = packet.clone();
        changed[field] = value;
        assert!(
            std::panic::catch_unwind(|| packet_record(&changed, &packet, 4, &payload, false))
                .is_err()
        );
    }
}
