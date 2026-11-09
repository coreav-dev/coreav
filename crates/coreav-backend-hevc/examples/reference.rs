#![doc = "Ordinary native HEVC packet reference and admission evidence."]

use coreav_backend_hevc::{NativeHevc, NativeOptions, test_support};
use coreav_decode::{BackendKind, Codec, Decoder, Limits, Packet, Policy, StreamConfig};
use coreav_frame::Pixels;
use md5::{Digest, Md5};
use serde_json::json;
use std::io::Write;

#[path = "common/access_units.rs"]
mod access_units;
#[allow(dead_code)]
#[path = "../../coreav-hevc/examples/common/mod.rs"]
mod common;

#[path = "../../coreav-hevc/examples/common/counters.rs"]
mod counters;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn drain(
    decoder: &mut dyn Decoder,
    writer: &mut impl Write,
    check: bool,
    pictures: &mut u64,
    bytes: &mut u64,
    hash: &mut Md5,
) -> Result<()> {
    while let Some(frame) = decoder.receive_frame()? {
        *pictures += 1;
        if !check {
            std::hint::black_box(frame);
            continue;
        }
        let Pixels::Cpu(picture) = &frame.pixels else {
            return Err("native reference returned non-CPU pixels".into());
        };
        let mut picture_hash = Md5::new();
        let mut picture_bytes = 0u64;
        for plane in picture.planes() {
            picture_hash.update(plane.data());
            hash.update(plane.data());
            picture_bytes += plane.data().len() as u64;
        }
        *bytes += picture_bytes;
        let color = frame.color;
        let chroma_location = color.chroma_location.map(|value| match value {
            coreav_frame::ChromaLocation::Left => 0,
            coreav_frame::ChromaLocation::Center => 1,
            coreav_frame::ChromaLocation::TopLeft => 2,
            coreav_frame::ChromaLocation::Top => 3,
            coreav_frame::ChromaLocation::BottomLeft => 4,
            coreav_frame::ChromaLocation::Bottom => 5,
        });
        serde_json::to_writer(
            &mut *writer,
            &json!({
                "type":"picture", "timestamp":frame.pts, "width":picture.width(),
                "height":picture.height(), "bit_depth":picture.format().depth(),
                "packing":if picture.format().depth()==8 {"yuv420p"} else {"yuv420p10le"},
                "color":{"primaries":color.primaries,"transfer":color.transfer,"matrix":color.matrix,
                    "full_range":color.full_range,"chroma_location":chroma_location}, "output_bytes":picture_bytes,
                "output_md5":format!("{:x}", picture_hash.finalize())
            }),
        )?;
        writer.write_all(b"\n")?;
    }
    Ok(())
}

fn run(writer: &mut impl Write) -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = args.next().ok_or("missing native reference source")?;
    let mode = args.next().ok_or("missing native reference mode")?;
    let requested: usize = args
        .next()
        .map(|s| -> Result<usize> {
            Ok(s.into_string()
                .map_err(|_| "invalid native threads")?
                .parse()?)
        })
        .transpose()?
        .unwrap_or(1);
    let memory = args
        .next()
        .map(|s| {
            s.into_string()
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("invalid native memory")
        })
        .transpose()?;
    if args.next().is_some() || requested > 16 || mode != "check" && mode != "decode" {
        return Err("invalid native reference arguments".into());
    }
    reference(
        std::fs::File::open(path)?,
        mode == "check",
        requested,
        memory,
        writer,
    )
}

fn reference(
    reader: impl std::io::Read,
    check: bool,
    requested: usize,
    memory: Option<u64>,
    writer: &mut impl Write,
) -> Result<()> {
    let host = std::thread::available_parallelism().map_or(1, usize::from);
    let threads = if requested == 0 {
        host.min(16)
    } else {
        requested
    };
    let limits = Limits {
        max_memory_bytes: memory.unwrap_or(Limits::default().max_memory_bytes),
        max_decoded_bytes: 64 << 30,
        timeout: std::time::Duration::from_secs(600),
        ..Limits::default()
    };
    test_support::begin();
    let backend = NativeHevc::with_options(NativeOptions {
        threads: Some(threads),
    });
    let mut opened = coreav_decode::open(
        &StreamConfig::new(Codec::Hevc),
        &Policy::default(),
        &limits,
        &[&backend],
    )?;
    let (mut pictures, mut output_bytes, mut hash) = (0, 0, Md5::new());
    let input = access_units::units(reader, limits.max_packet_bytes, |data, pts, _, _| {
        opened.decoder.send_packet(&Packet {
            data,
            pts,
            dts: None,
            keyframe: false,
            discard: false,
        })?;
        drain(
            &mut *opened.decoder,
            writer,
            check,
            &mut pictures,
            &mut output_bytes,
            &mut hash,
        )
    });
    let (input_bytes, packets) = match input {
        Ok(receipt) => receipt,
        Err(error) => {
            let error = match error.downcast::<coreav_decode::Error>() {
                Ok(error) => *error,
                Err(error) => coreav_decode::Error::new(
                    coreav_decode::ErrorKind::InvalidData,
                    error.to_string(),
                ),
            };
            return Err(opened.decoder.settle_native_hevc_refusal(error).into());
        }
    };
    opened.decoder.send_eof()?;
    drain(
        &mut *opened.decoder,
        writer,
        check,
        &mut pictures,
        &mut output_bytes,
        &mut hash,
    )?;
    if opened.decoder.info().backend != BackendKind::Native || opened.decoder.info().fell_back() {
        return Err("native reference changed backend".into());
    }
    let report = test_support::take().ok_or("native report is missing")?;
    if report.overflow {
        return Err("native admission report overflow".into());
    }
    let summary = report.summary.ok_or("native complete summary is missing")?;
    let mut semantic = summary;
    semantic.frame_parallel_pictures = 0;
    semantic.parallel_pictures = 0;
    semantic.max_workers = 0;
    semantic.memory_bytes = 0;
    semantic.reconstruction.memory_bytes = 0;
    let semantic = json!({
        "headers":counters::header_stats(semantic.headers),
        "decoded_pictures":semantic.decoded_pictures,"skipped_pictures":semantic.skipped_pictures,
        "skipped_picture_hashes":semantic.skipped_picture_hashes,"skipped_reserved_hashes":semantic.skipped_reserved_hashes,
        "stats":common::reconstruction_stats(semantic.reconstruction),"inter":counters::inter_stats(semantic.inter),
    });
    let mut admissions = Vec::new();
    for admission in report.admissions {
        if admission
            .lag
            .checked_mul(admission.frame_bytes)
            .and_then(|n| n.checked_add(admission.reserved_bytes))
            .is_none_or(|n| n > admission.limit_bytes)
        {
            return Err("native admission certificate exceeds its limit".into());
        }
        admissions.push(json!({
            "sps_id":admission.sps_id, "sps_sha256":admission.sps_hash,
            "ceiling":admission.ceiling, "workers":admission.workers, "lag":admission.lag,
            "frame_bytes":admission.frame_bytes, "reserved_bytes":admission.reserved_bytes,
            "limit_bytes":admission.limit_bytes
        }));
    }
    serde_json::to_writer(
        &mut *writer,
        &json!({
            "type":"summary", "complete":true, "boundary":true, "backend":"native",
            "input_bytes":input_bytes, "sent_packets":packets, "pictures":pictures,"output_pictures":pictures,
            "inherited_color":{"primaries":null,"transfer":null,"matrix":null,"full_range":null,"chroma_location":null},
            "output_bytes":if check { Some(output_bytes) } else { None },
            "output_md5":if check { Some(format!("{:x}", hash.finalize())) } else { None },
            "semantic":semantic, "decoded_pictures":summary.decoded_pictures,
            "bins":summary.reconstruction.bins, "requested_threads":requested, "host_threads":host,
            "thread_limit":threads.min(host), "max_workers":summary.max_workers,"max_memory_bytes":limits.max_memory_bytes,
            "memory_bytes":summary.memory_bytes,
            "parallel_pictures":summary.parallel_pictures, "frame_parallel_pictures":summary.frame_parallel_pictures,
            "row_parallel_pictures":report.row_parallel_pictures,
            "admissions":admissions
        }),
    )?;
    writer.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
#[path = "../../coreav-hevc/test-support/pcm.rs"]
mod pcm;
#[cfg(test)]
#[path = "../../coreav-hevc/test-support/writer.rs"]
mod writer;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_reference_checks_generated_depths_at_every_worker_boundary() -> Result<()> {
        for depth in [8, 10] {
            let fixture = pcm::pcm_fixture(depth, depth, false)?;
            let mut input = Vec::new();
            for nal in fixture.sets.iter().chain([&fixture.nal, &fixture.nal]) {
                input.extend_from_slice(&[0, 0, 0, 1]);
                input.extend_from_slice(nal);
            }
            let mut pixels = Vec::new();
            for _ in 0..2 {
                for plane in &fixture.planes {
                    for sample in plane {
                        pixels.extend_from_slice(
                            sample
                                .to_le_bytes()
                                .get(..if depth == 10 { 2 } else { 1 })
                                .ok_or("generated sample depth")?,
                        );
                    }
                }
            }
            let expected = format!("{:x}", Md5::digest(&pixels));
            let mut semantic = None;
            for threads in [1, 2, 4, 8, 0] {
                for check in [false, true] {
                    let mut output = Vec::new();
                    reference(input.as_slice(), check, threads, None, &mut output)?;
                    let records: Vec<serde_json::Value> = std::str::from_utf8(&output)?
                        .lines()
                        .map(serde_json::from_str)
                        .collect::<std::result::Result<_, _>>()?;
                    let summary = records.last().ok_or("missing summary")?;
                    assert_eq!(summary["type"], "summary");
                    assert_eq!(summary["complete"], true);
                    assert_eq!(summary["boundary"], true);
                    assert_eq!(summary["pictures"], 2);
                    assert_eq!(summary["decoded_pictures"], 2);
                    assert_eq!(summary["sent_packets"], 2);
                    let workers = summary["max_workers"]
                        .as_u64()
                        .ok_or("missing worker count")?;
                    let ceiling = summary["thread_limit"]
                        .as_u64()
                        .ok_or("missing worker ceiling")?;
                    assert!((1..=ceiling).contains(&workers));
                    let accounted = summary["memory_bytes"]
                        .as_u64()
                        .ok_or("missing memory count")?;
                    assert!(
                        accounted > 0
                            && accounted
                                <= summary["max_memory_bytes"]
                                    .as_u64()
                                    .ok_or("missing memory limit")?
                    );
                    assert_eq!(summary["semantic"]["headers"]["pictures"], 2);
                    assert_eq!(summary["semantic"]["stats"]["ctus"], 2);
                    assert_eq!(summary["semantic"]["stats"]["bins"], summary["bins"]);
                    assert_eq!(
                        *semantic.get_or_insert_with(|| summary["semantic"].clone()),
                        summary["semantic"]
                    );
                    assert_eq!(records.len(), if check { 3 } else { 1 });
                    if check {
                        assert_eq!(summary["output_md5"], expected);
                        assert_eq!(summary["output_bytes"], pixels.len());
                        assert_eq!(
                            records.first().ok_or("missing first picture")?["timestamp"],
                            0
                        );
                        assert_eq!(
                            records.get(1).ok_or("missing second picture")?["timestamp"],
                            1
                        );
                    } else {
                        assert!(summary["output_md5"].is_null());
                    }
                    let admitted = summary["admissions"]
                        .as_array()
                        .ok_or("missing admissions")?;
                    let effective = if threads == 0 { 16 } else { threads }
                        .min(std::thread::available_parallelism().map_or(1, usize::from));
                    assert_eq!(admitted.is_empty(), effective == 1);
                }
            }
        }
        Ok(())
    }
}

fn main() -> std::process::ExitCode {
    let mut writer = std::io::BufWriter::new(std::io::stdout().lock());
    match run(&mut writer) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let mut stderr = std::io::stderr().lock();
            let _ = serde_json::to_writer(
                &mut stderr,
                &json!({"type":"refusal","reason":error.to_string()}),
            );
            let _ = stderr.write_all(b"\n");
            std::process::ExitCode::FAILURE
        }
    }
}
