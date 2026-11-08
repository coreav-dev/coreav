#![doc = "Decoder-only scalar HEVC reference with separate pixel checks."]

use coreav_hevc::{DecodeLimits, Error, PictureDecoder, Result};
use serde_json::json;
use std::io::{Read, Write};

mod common;
#[path = "common/decoded.rs"]
mod decoded;

#[cfg(hevc_thread_diagnostics)]
fn dependencies(writer: &mut impl Write, bytes: &[u8]) -> Result<()> {
    struct Picture {
        index: u64,
        poc: i32,
        reconstruction_required: bool,
        wpp: bool,
        info: coreav_hevc::PictureInfo,
        dpb_capacity: u32,
        highest_sps_layer: u32,
        slice_types: [bool; 3],
        rps: [Option<(u64, i32, bool)>; 16],
        active: [Option<(u64, i32, bool)>; 16],
    }
    let mut parser = coreav_hevc::HeaderParser::new(coreav_hevc::Limits::default());
    parser.diagnostic_trace_outputs()?;
    let mut pictures: Vec<Picture> = Vec::new();
    pictures
        .try_reserve_exact(32)
        .map_err(|_| Error::Limit("dependency_records"))?;
    common::read_nals(
        bytes,
        coreav_hevc::Limits::default().max_nal_bytes,
        |_, nal| {
            if let coreav_hevc::NalInfo::Slice {
                header,
                picture,
                order,
            } = parser.read_nal(nal, |_| Ok(()))?
            {
                if header.first {
                    if pictures.len() == 32 {
                        return Err(Error::Limit("dependency_picture_count"));
                    }
                    let order = order.ok_or(Error::Internal("dependency_picture_order"))?;
                    let (dpb_capacity, highest_sps_layer) = parser.diagnostic_sps_envelope()?;
                    pictures.push(Picture {
                        index: pictures.len() as u64,
                        poc: order.poc,
                        reconstruction_required: order.reconstruction_required,
                        wpp: picture.wpp,
                        info: picture,
                        dpb_capacity,
                        highest_sps_layer,
                        slice_types: [false; 3],
                        rps: [None; 16],
                        active: [None; 16],
                    });
                }
                let current = pictures
                    .last_mut()
                    .ok_or(Error::InvalidData("dependency_first_slice"))?;
                if let Some(kind) = header.slice_type {
                    *current
                        .slice_types
                        .get_mut(kind as usize)
                        .ok_or(Error::InvalidData("dependency_slice_type"))? = true;
                }
                if current.reconstruction_required {
                    let snapshot = parser.diagnostic_dependencies()?;
                    if !header.first && current.rps != snapshot.rps {
                        return Err(Error::InvalidData("dependency_rps_changed"));
                    }
                    current.rps = snapshot.rps;
                    for (entry, value) in current.active.iter_mut().zip(snapshot.active) {
                        if value.is_some() {
                            *entry = value;
                        }
                    }
                }
            }
            Ok(())
        },
    )?;
    let summary = parser.finish(|_| Ok(()))?;
    let output_events: Vec<_> = parser
        .diagnostic_outputs()?
        .into_iter()
        .flatten()
        .map(|output| {
            json!({"picture_index":output.event.picture_index,"poc":output.event.poc,
            "before_picture":output.before_picture})
        })
        .collect();
    let required = pictures
        .iter()
        .filter(|picture| picture.reconstruction_required)
        .count();
    let rows: Vec<_> = pictures.into_iter().map(|picture| json!({
        "index":picture.index,"poc":picture.poc,"reconstruction_required":picture.reconstruction_required,
        "wpp":picture.wpp,"envelope":{"coded_width":picture.info.width,
            "coded_height":picture.info.height,"width":picture.info.visible_width,
            "height":picture.info.visible_height,"bit_depth":picture.info.bit_depth,
            "ctb_log2":picture.info.ctb_log2,"dpb_capacity":picture.dpb_capacity,
            "highest_sps_layer":picture.highest_sps_layer},
        "slice_types":picture.slice_types.into_iter().enumerate()
            .filter_map(|(kind, present)| present.then_some(kind)).collect::<Vec<_>>(),
        "rps":picture.rps.into_iter().flatten().collect::<Vec<_>>(),
        "active":picture.active.into_iter().flatten().collect::<Vec<_>>()
    })).collect();
    serde_json::to_writer(
        &mut *writer,
        &json!({"mode":"header_dependencies","header_only":true,
            "complete":true,"coded_pictures":summary.pictures,"reconstruction_required":required,
            "output_pictures":summary.output_pictures,"nals":summary.nals,"pictures":rows,
            "output_events":output_events
        }),
    )
    .map_err(|_| Error::Limit("reference_output"))?;
    writer
        .write_all(b"\n")
        .map_err(|_| Error::Limit("reference_output"))
}

#[cfg(hevc_thread_diagnostics)]
fn row_dependencies(writer: &mut impl Write, bytes: &[u8]) -> Result<()> {
    let mut header_bytes = Vec::new();
    dependencies(&mut header_bytes, bytes)?;
    let header: serde_json::Value =
        serde_json::from_slice(&header_bytes).map_err(|_| Error::Internal("row_trace_header"))?;
    coreav_hevc::thread_diagnostics::begin_rows()?;
    let limits = DecodeLimits {
        max_memory_bytes: usize::try_from(coreav_decode::Limits::default().max_memory_bytes)
            .map_err(|_| Error::Limit("reference_memory"))?,
        threads: 1,
        ..DecodeLimits::default()
    };
    let mut decoder = PictureDecoder::new(limits);
    let mut output_pictures = 0u64;
    let mut emit = |_: coreav_hevc::DecodedPicture<'_>| {
        output_pictures = output_pictures
            .checked_add(1)
            .ok_or(Error::Limit("row_trace_outputs"))?;
        Ok(())
    };
    common::read_nals(bytes, limits.headers.max_nal_bytes, |_, nal| {
        decoder.read_nal(nal, None, &mut emit)
    })?;
    let summary = decoder.finish(&mut emit)?;
    drop(decoder);
    if header
        .get("output_pictures")
        .and_then(serde_json::Value::as_u64)
        != Some(output_pictures)
    {
        return Err(Error::Internal("row_trace_output_count"));
    }
    let pictures: Vec<_> = coreav_hevc::thread_diagnostics::take_rows(summary.max_workers)?
        .into_iter()
        .map(|picture| {
            let rows: Vec<_> = picture
                .rows
                .into_iter()
                .enumerate()
                .map(|(row, reads)| {
                    let references: Vec<_> = reads
                        .references
                        .into_iter()
                        .flatten()
                        .map(|reference| {
                            json!([
                                reference.index,
                                reference.poc,
                                reference.sample_row,
                                reference.motion_row
                            ])
                        })
                        .collect();
                    json!({"row":row,"references":references,"sample_calls":reads.sample_calls,
                "motion_calls":reads.motion_calls})
                })
                .collect();
            json!({"index":picture.index,"poc":picture.poc,"width":picture.info.width,
            "height":picture.info.height,"ctb_log2":picture.info.ctb_log2,
            "tiles":picture.info.tiles,"wpp":picture.info.wpp,"rows":rows})
        })
        .collect();
    serde_json::to_writer(&mut *writer, &json!({"mode":"row_dependencies","complete":true,
        "header_only":false,"requested_threads":1,"thread_limit":1,"max_workers":summary.max_workers,
            "decoded_pictures":summary.decoded_pictures,"output_pictures":output_pictures,
            "header_dependencies":header,"pictures":pictures
    })).map_err(|_| Error::Limit("reference_output"))?;
    writer
        .write_all(b"\n")
        .map_err(|_| Error::Limit("reference_output"))
}

fn run(writer: &mut impl Write) -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or(Error::InvalidData("reference_arguments"))?;
    let mode = args
        .next()
        .ok_or(Error::InvalidData("reference_arguments"))?;
    let requested_threads = args
        .next()
        .map(|value| {
            value
                .into_string()
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
        })
        .unwrap_or(Some(1))
        .filter(|value| *value <= 16)
        .ok_or(Error::InvalidData("reference_arguments"))?;
    let host_threads = std::thread::available_parallelism().map_or(1, usize::from);
    let threads = if requested_threads == 0 {
        host_threads.min(16)
    } else {
        requested_threads
    };
    let header_only = cfg!(hevc_thread_diagnostics) && mode == "dependencies";
    let row_reads = cfg!(hevc_thread_diagnostics) && mode == "rows";
    if args.next().is_some() || (mode != "check" && mode != "decode" && !header_only && !row_reads)
    {
        return Err(Error::InvalidData("reference_arguments"));
    }
    if row_reads && requested_threads != 1 {
        return Err(Error::InvalidData("row_trace_threads"));
    }
    let source = std::fs::File::open(path).map_err(|_| Error::InvalidData("input_open"))?;
    let mut bytes = Vec::new();
    source
        .take(64 << 20)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Limit("reference_input"))?;
    if bytes.len() >= 64 << 20 {
        return Err(Error::Limit("reference_input"));
    }
    #[cfg(hevc_thread_diagnostics)]
    if header_only {
        return dependencies(writer, &bytes);
    }
    #[cfg(hevc_thread_diagnostics)]
    if row_reads {
        return row_dependencies(writer, &bytes);
    }
    let limits = DecodeLimits {
        max_memory_bytes: usize::try_from(coreav_decode::Limits::default().max_memory_bytes)
            .map_err(|_| Error::Limit("reference_memory"))?,
        ..DecodeLimits::default()
    };
    let mut decoder = PictureDecoder::new(limits).with_threads(threads);
    let mut output = decoded::Output::new(writer);
    let mut pictures = 0u64;
    let mut callback = |picture: coreav_hevc::DecodedPicture<'_>| {
        if mode == "check" {
            output.picture(picture)?;
        } else {
            std::hint::black_box(picture);
        }
        pictures += 1;
        Ok(())
    };
    let receipt = common::read_nals(bytes.as_slice(), limits.headers.max_nal_bytes, |_, nal| {
        #[cfg(hevc_synchronous_baseline)]
        {
            decoder.read_nal(nal, None, &mut callback)
        }
        #[cfg(not(hevc_synchronous_baseline))]
        {
            decoder.read_nal_scheduled(nal, None, None, |picture, _| callback(picture))
        }
    });
    let receipt = match receipt {
        Ok(receipt) => receipt,
        Err(error) => {
            #[cfg(not(hevc_synchronous_baseline))]
            decoder.synchronize_pending()?;
            return Err(error);
        }
    };
    let summary = decoder.finish(&mut callback)?;
    if mode == "check" {
        output.finish(summary, receipt, limits)
    } else {
        let record = json!({"pictures":pictures,"requested_threads":requested_threads,
            "host_threads":host_threads,"thread_limit":threads,"max_memory_bytes":limits.max_memory_bytes,
            "decoded_pictures":summary.decoded_pictures,"bins":summary.reconstruction.bins});
        #[cfg(not(hevc_synchronous_baseline))]
        let record = {
            let mut record = record;
            let fields = record
                .as_object_mut()
                .ok_or(Error::Internal("reference_record"))?;
            fields.insert("thread_limit".to_owned(), json!(threads.min(host_threads)));
            fields.insert(
                "parallel_pictures".to_owned(),
                json!(summary.parallel_pictures),
            );
            fields.insert(
                "frame_parallel_pictures".to_owned(),
                json!(summary.frame_parallel_pictures),
            );
            fields.insert("max_workers".to_owned(), json!(summary.max_workers));
            record
        };
        #[cfg(hevc_thread_diagnostics)]
        let record = {
            let mut record = record;
            record
                .as_object_mut()
                .ok_or(Error::Internal("reference_record"))?
                .insert(
                    "thread_diagnostics".to_owned(),
                    json!(coreav_hevc::thread_diagnostics::take().as_slice()),
                );
            record
        };
        serde_json::to_writer(&mut *writer, &record)
            .map_err(|_| Error::Limit("reference_output"))?;
        writer
            .write_all(b"\n")
            .map_err(|_| Error::Limit("reference_output"))
    }
}

fn main() -> std::process::ExitCode {
    let mut writer = std::io::BufWriter::new(std::io::stdout().lock());
    match run(&mut writer)
        .and_then(|()| writer.flush().map_err(|_| Error::Limit("reference_output")))
    {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:?}");
            std::process::ExitCode::FAILURE
        }
    }
}
