//! Bounded receipts from the actual native packet decode boundary.

use coreav_backend_hevc::NativeHevc;
use coreav_decode::{Backend, BackendKind, Codec, Decoder, Limits, Packet, Policy, StreamConfig};
use coreav_frame::{Frame, Pixels};
use coreav_hevc::NalHeader;
use coreav_isobmff::{FileSource, Rational};
use coreav_thumbnail::ThumbnailOptions;
use md5::{Digest, Md5};
use serde_json::{Value, json};
use std::io::Write;

#[path = "common/access_units.rs"]
mod access_units;
#[allow(dead_code)]
#[path = "../../coreav-hevc/examples/common/mod.rs"]
mod common;
#[path = "boundary/random_access.rs"]
mod random_access;
#[path = "boundary/thumbnails.rs"]
mod thumbnails;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Output<'a, W> {
    writer: &'a mut W,
    pictures: u64,
    bytes: u64,
    hash: Md5,
    inherited_color: coreav_frame::ColorInfo,
    wanted: Vec<(usize, i64)>,
    baselines: Vec<(usize, Frame)>,
    c5: Option<Value>,
    full_records: Option<Vec<Value>>,
}

fn color_record(color: coreav_frame::ColorInfo) -> Value {
    use coreav_frame::ChromaLocation;
    json!({"primaries":color.primaries,"transfer":color.transfer,"matrix":color.matrix,
        "full_range":color.full_range,"chroma_location":color.chroma_location.map(|value| match value {
            ChromaLocation::Left=>0, ChromaLocation::Center=>1, ChromaLocation::TopLeft=>2,
            ChromaLocation::Top=>3, ChromaLocation::BottomLeft=>4, ChromaLocation::Bottom=>5 })})
}

fn frame_record(frame: &Frame) -> Result<(Value, u64)> {
    let Pixels::Cpu(picture) = &frame.pixels else {
        return Err("boundary output is not CPU pixels".into());
    };
    let mut hash = Md5::new();
    let mut bytes = 0u64;
    for plane in picture.planes() {
        hash.update(plane.data());
        bytes = bytes
            .checked_add(plane.data().len() as u64)
            .ok_or("output bytes overflow")?;
    }
    Ok((
        json!({"type":"picture", "timestamp":frame.pts,
        "width":picture.width(), "height":picture.height(), "bit_depth":picture.format().depth(),
        "packing":if picture.format().depth()==8 {"yuv420p"} else {"yuv420p10le"},
        "output_bytes":bytes, "output_md5":format!("{:x}",hash.finalize()),
        "color":color_record(frame.color)}),
        bytes,
    ))
}

impl<W: Write> Output<'_, W> {
    fn record(&mut self, value: &Value) -> Result<()> {
        serde_json::to_writer(&mut *self.writer, value)?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }

    fn drain(&mut self, decoder: &mut dyn Decoder) -> Result<()> {
        while let Some(frame) = decoder.receive_frame()? {
            let (record, bytes) = frame_record(&frame)?;
            let Pixels::Cpu(picture) = &frame.pixels else {
                return Err("boundary output is not CPU pixels".into());
            };
            for plane in picture.planes() {
                self.hash.update(plane.data());
            }
            self.pictures = self
                .pictures
                .checked_add(1)
                .ok_or("output count overflow")?;
            self.bytes = self
                .bytes
                .checked_add(bytes)
                .ok_or("output bytes overflow")?;
            self.record(&record)?;
            if let Some(records) = &mut self.full_records {
                records.push(record);
            }
            if let Some((index, _)) = self.wanted.iter().find(|(_, pts)| frame.pts == Some(*pts)) {
                self.baselines.push((*index, frame));
            }
        }
        Ok(())
    }

    fn finish(&mut self, decoder: &mut dyn Decoder, input_bytes: u64, packets: u64) -> Result<()> {
        decoder.send_eof()?;
        self.drain(decoder)?;
        decoder.send_eof()?;
        if decoder.receive_frame()?.is_some() {
            return Err("duplicate EOF output".into());
        }
        if decoder.info().backend != BackendKind::Native || decoder.info().fell_back() {
            return Err("boundary did not select native HEVC".into());
        }
        self.record(&json!({"type":"summary", "complete":true, "boundary":true,
            "backend":decoder.info().backend.name(), "input_bytes":input_bytes,"sent_packets":packets,
            "inherited_color":color_record(self.inherited_color),
            "c5":self.c5,
            "output_pictures":self.pictures,"output_bytes":self.bytes,
            "output_md5":format!("{:x}",self.hash.clone().finalize())}))
    }
}

fn send(
    packet: &[u8],
    pts: Option<i64>,
    decoder: &mut dyn Decoder,
    output: &mut Output<'_, impl Write>,
) -> Result<()> {
    decoder.send_packet(&Packet {
        data: packet,
        pts,
        dts: None,
        keyframe: false,
        discard: false,
    })?;
    output.drain(decoder)
}

fn annex_b(path: std::ffi::OsString, maximum: u64, writer: &mut impl Write) -> Result<()> {
    let limits = Limits {
        max_memory_bytes: maximum,
        max_decoded_bytes: 64 << 30,
        timeout: std::time::Duration::from_secs(240),
        ..Limits::default()
    };
    let native = NativeHevc::new();
    let mut opened = coreav_decode::open(
        &StreamConfig::new(Codec::Hevc),
        &Policy::default(),
        &limits,
        &[&native],
    )?;
    let mut output = Output {
        writer,
        pictures: 0,
        bytes: 0,
        hash: Md5::new(),
        inherited_color: coreav_frame::ColorInfo::default(),
        wanted: Vec::new(),
        baselines: Vec::new(),
        c5: None,
        full_records: Some(Vec::new()),
    };
    let (bytes, packets) = random_access::units(
        std::fs::File::open(&path)?,
        limits.max_packet_bytes,
        |data, pts, _, _| send(data, pts, &mut *opened.decoder, &mut output),
    )?;
    opened.decoder.send_eof()?;
    output.drain(&mut *opened.decoder)?;
    let full = output
        .full_records
        .take()
        .ok_or("C1 full-decode receipts are absent")?;
    output.c5 = Some(random_access::check(&path, &limits, &full)?);
    output.finish(&mut *opened.decoder, bytes, packets)
}

fn packet_stream(
    path: std::ffi::OsString,
    maximum: u64,
    receipt_path: std::ffi::OsString,
    writer: &mut impl Write,
) -> Result<()> {
    let metadata = std::fs::metadata(&receipt_path)?;
    if metadata.len() > 128 << 20 {
        return Err("packet receipt is too large".into());
    }
    let records: Vec<Value> =
        std::io::BufRead::lines(std::io::BufReader::new(std::fs::File::open(receipt_path)?))
            .map(|line| -> Result<Value> { Ok(serde_json::from_str(&line?)?) })
            .collect::<Result<_>>()?;
    let raw_stream = records
        .first()
        .filter(|row| row.get("type") == Some(&json!("stream")))
        .ok_or("missing source-bound packet stream")?;
    let original: Vec<&Value> = records
        .iter()
        .filter(|row| row.get("type") == Some(&json!("packet")))
        .collect();
    let sent = raw_stream
        .get("sent_packets")
        .and_then(Value::as_u64)
        .ok_or("missing packet count")?;
    let index = raw_stream
        .get("stream_index")
        .and_then(Value::as_u64)
        .ok_or("missing stream index")?;
    let mut options = ThumbnailOptions::at(Rational::from_int(0));
    options.stream = Some(usize::try_from(index)?);
    let mut source = FileSource::open(&path)?;
    let stream = coreav_thumbnail::video_stream(&mut source, &options)?;
    if stream.stream.codec != Codec::Hevc
        || stream.packets.len() != original.len()
        || stream.stream_index != usize::try_from(index)?
        || sent == 0
        || sent > original.len() as u64
    {
        return Err("source-bound packet listing differs".into());
    }
    let limits = Limits {
        max_memory_bytes: maximum,
        max_decoded_bytes: 64 << 30,
        timeout: std::time::Duration::from_secs(3540),
        ..Limits::default()
    };
    let mut decoder = NativeHevc::new().open(&stream.stream, &limits)?;
    let wanted: Vec<(usize, i64)> = stream
        .packets
        .iter()
        .zip(&original)
        .enumerate()
        .take(usize::try_from(sent)?)
        .filter(|(_, (packet, clock))| {
            packet.keyframe
                && packet.presented
                && packet.time.is_some()
                && clock.get("pts").and_then(Value::as_i64).is_some_and(|pts| {
                    records
                        .iter()
                        .filter(|row| {
                            row.get("type") == Some(&json!("picture"))
                                && row.get("timestamp").and_then(Value::as_i64) == Some(pts)
                        })
                        .count()
                        == 1
                })
        })
        .take(3)
        .filter_map(|(index, (_, clock))| {
            clock
                .get("pts")
                .and_then(Value::as_i64)
                .map(|pts| (index, pts))
        })
        .collect();
    let mut output = Output {
        writer,
        pictures: 0,
        bytes: 0,
        hash: Md5::new(),
        inherited_color: stream.stream.color,
        wanted,
        baselines: Vec::new(),
        c5: None,
        full_records: None,
    };
    let mut input_bytes = 0u64;
    for (ordinal, (packet, clock)) in stream.packets.iter().zip(&original).enumerate() {
        if clock.get("index").and_then(Value::as_u64) != Some(ordinal as u64)
            || clock.get("offset").and_then(Value::as_u64) != Some(packet.offset)
            || clock.get("size").and_then(Value::as_u64) != Some(packet.size)
            || clock.get("media_pts").and_then(Value::as_i64) != Some(packet.pts)
            || clock.get("dts").and_then(Value::as_i64) != packet.dts
            || clock.get("keyframe").and_then(Value::as_bool) != Some(packet.keyframe)
        {
            return Err("source-bound packet identity differs".into());
        }
        if ordinal as u64 >= sent {
            continue;
        }
        let bytes = stream.read_packet(&mut source, ordinal, 64 << 20)?;
        let expected = records
            .iter()
            .find(|row| {
                row.get("type") == Some(&json!("payload"))
                    && row.get("index").and_then(Value::as_u64) == Some(ordinal as u64)
            })
            .ok_or("missing original packet payload receipt")?;
        let digest = format!("{:x}", sha2::Sha256::digest(&bytes));
        if expected.get("sha256").and_then(Value::as_str) != Some(digest.as_str())
            || expected.get("bytes").and_then(Value::as_u64) != Some(bytes.len() as u64)
        {
            return Err("original packet payload differs".into());
        }
        let pts = clock
            .get("pts")
            .and_then(Value::as_i64)
            .ok_or("missing caller clock")?;
        decoder.send_packet(&Packet {
            data: &bytes,
            pts: Some(pts),
            dts: packet.dts,
            keyframe: packet.keyframe,
            discard: false,
        })?;
        output.drain(&mut *decoder)?;
        input_bytes = input_bytes
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= common::INPUT_LIMIT)
            .ok_or("packet input size overflow")?;
    }
    decoder.send_eof()?;
    output.drain(&mut *decoder)?;
    if output.baselines.len() != output.wanted.len() {
        return Err("C5 full-decode targets are missing".into());
    }
    output.c5 = Some(thumbnails::check(
        &path,
        &mut source,
        &stream,
        &limits,
        std::mem::take(&mut output.baselines),
        usize::try_from(sent)?,
    )?);
    output.finish(&mut *decoder, input_bytes, sent)
}

fn run(writer: &mut impl Write) -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let mode = arguments.next().ok_or("missing mode")?;
    let source = arguments.next().ok_or("missing source")?;
    let memory = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse().ok())
        .ok_or("invalid memory limit")?;
    if mode == "refusal" {
        let index = arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .and_then(|value| value.parse().ok())
            .ok_or("invalid stream index")?;
        if arguments.next().is_some() {
            return Err("extra argument".into());
        }
        profile_refusal(source, memory, index, writer)
    } else if mode == "packets" {
        let receipt = arguments.next().ok_or("missing packet receipt")?;
        if arguments.next().is_some() {
            return Err("extra argument".into());
        }
        packet_stream(source, memory, receipt, writer)
    } else if mode == "annex-b" {
        if arguments.next().is_some() {
            return Err("extra argument".into());
        }
        annex_b(source, memory, writer)
    } else {
        Err("unknown boundary mode".into())
    }
}

fn profile_refusal(
    path: std::ffi::OsString,
    maximum: u64,
    index: usize,
    writer: &mut impl Write,
) -> Result<()> {
    let mut source = FileSource::open(path)?;
    let mut options = ThumbnailOptions::at(Rational::from_int(0));
    options.stream = Some(index);
    let stream = coreav_thumbnail::video_stream(&mut source, &options)?;
    let limits = Limits {
        max_memory_bytes: maximum,
        ..Limits::default()
    };
    let mut attempted = NativeHevc::new().open(&stream.stream, &limits);
    if let Ok(decoder) = &mut attempted {
        for (index, packet) in stream.packets.iter().take(120).enumerate() {
            let bytes = stream.read_packet(&mut source, index, 64 << 20)?;
            let result = decoder.send_packet(&Packet {
                data: &bytes,
                pts: Some(packet.pts),
                dts: packet.dts,
                keyframe: packet.keyframe,
                discard: false,
            });
            if let Err(error) = result {
                attempted = Err(error);
                break;
            }
            while decoder.receive_frame()?.is_some() {}
        }
    }
    match attempted {
        Err(error) if error.kind == coreav_decode::ErrorKind::Unsupported => {
            serde_json::to_writer(
                &mut *writer,
                &json!({"type":"admission", "complete":true,
                "boundary":true, "backend":"native", "expected_refusal":true,
                "category":"unsupported", "reason":error.message}),
            )?;
            writer.write_all(b"\n")?;
            Ok(())
        }
        _ => Err("boundary did not reproduce the expected profile refusal".into()),
    }
}

fn main() -> std::process::ExitCode {
    let mut writer = std::io::BufWriter::new(std::io::stdout().lock());
    match run(&mut writer) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let _ = serde_json::to_writer(
                std::io::stderr().lock(),
                &json!({"kind":"refusal", "reason":error.to_string()}),
            );
            std::process::ExitCode::FAILURE
        }
    }
}
