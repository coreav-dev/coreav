//! Bounded JSONL inspection for the registered HEVC conformance gate.

use coreav_hevc::{AnnexB, Error, HeaderParser, Limits, NalHeader, NalInfo, OutputEvent};
use serde_json::{Value, json};
use std::io::{Read, Write};

const MAX_STREAM_BYTES: usize = 256 * 1024 * 1024;

fn row(output: &mut impl Write, value: Value) -> coreav_hevc::Result<()> {
    serde_json::to_writer(&mut *output, &value).map_err(|_| Error::Limit("inspection_output"))?;
    output
        .write_all(b"\n")
        .map_err(|_| Error::Limit("inspection_output"))
}

fn emit(output: &mut impl Write, event: OutputEvent) -> coreav_hevc::Result<()> {
    row(
        output,
        json!({"kind":"output", "picture_index":event.picture_index,
        "poc":event.poc,"width":event.width,"height":event.height,"bit_depth":event.bit_depth}),
    )
}

fn inspect(output: &mut impl Write) -> coreav_hevc::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or(Error::InvalidData("one_input_file_required"))?;
    if args.next().is_some() {
        return Err(Error::InvalidData("one_input_file_required"));
    }
    let mut file = std::fs::File::open(path).map_err(|_| Error::InvalidData("input_open"))?;
    let mut data = Vec::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| Error::InvalidData("input_read"))?;
        if count == 0 {
            break;
        }
        if data.len().saturating_add(count) > MAX_STREAM_BYTES {
            return Err(Error::Limit("inspection_stream_bytes"));
        }
        data.try_reserve(count)
            .map_err(|_| Error::Limit("inspection_allocation"))?;
        data.extend_from_slice(
            buffer
                .get(..count)
                .ok_or(Error::InvalidData("input_read_size"))?,
        );
    }
    let mut parser = HeaderParser::new(Limits::default());
    let mut picture_index = None;
    let mut pictures = 0u64;
    let mut tools = json!({"tiles":false,"wpp":false,"dependent_segments":false,
        "sao":false,"deblocking":false,"unfiltered_intra":false,"filtered_intra":false,
        "depth8":false,"depth10":false});
    for (nals, framed) in AnnexB::new(&data).enumerate() {
        let bytes = framed?;
        let header = NalHeader::parse(bytes)?;
        let offset = (bytes.as_ptr() as usize)
            .checked_sub(data.as_ptr() as usize)
            .ok_or(Error::InvalidData("nal_offset"))?;
        let info = parser.read_nal(bytes, |event| emit(output, event))?;
        let mut value = json!({"kind":"nal","index":nals,"offset":offset,"size":bytes.len(),
            "nal_type":header.unit_type,"layer_id":header.layer_id,"temporal_id":header.temporal_id});
        match info {
            NalInfo::Slice {
                header: h,
                picture: p,
                order,
            } => {
                if h.first {
                    picture_index = Some(pictures);
                    pictures += 1;
                }
                let additions = json!({"first_slice_segment_in_pic_flag":h.first,
                    "dependent_slice_segment_flag":h.dependent,"picture_index":picture_index,
                    "poc":order.map(|x| x.poc),"poc_lsb":h.poc_lsb,"slice_type":h.slice_type,
                    "no_rasl_output":order.map(|x|x.no_rasl_output),
                    "reconstruction_required":order.map(|x|x.reconstruction_required),
                    "pic_output_flag":h.output,"slice_qp_y":h.slice_qp_y,
                    "deblock_offsets":h.deblock_offsets,"loop_filter_slices":h.loop_filter_slices,
                    "chroma_qp_offsets":h.chroma_qp_offsets,"cabac_init_flag":h.cabac_init_flag,
                    "coded_width":p.width,"coded_height":p.height,
                    "width":p.visible_width,"height":p.visible_height,"bit_depth":p.bit_depth,
                    "profile_idc":p.profile_idc,"ctb_log2":p.ctb_log2,
                    "conformance_window":p.conformance_window,"tiles":p.tiles,"wpp":p.wpp,
                    "pps_id":h.pps_id,"address":h.address,"rbsp_header_bytes":h.rbsp_header_bytes,
                    "entry_points":h.entry_points,"sao":h.sao,"deblocking_disabled":h.deblock_disabled});
                if let (Some(value), Some(additions)) =
                    (value.as_object_mut(), additions.as_object())
                {
                    value.extend(
                        additions
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone())),
                    );
                }
                for (key, present) in [
                    ("tiles", p.tiles),
                    ("wpp", p.wpp),
                    ("dependent_segments", h.dependent),
                    (
                        "sao",
                        h.sao.is_some_and(|flags| flags.into_iter().any(|v| v)),
                    ),
                    ("deblocking", h.deblock_disabled == Some(false)),
                    (
                        "unfiltered_intra",
                        h.slice_type == Some(2)
                            && h.sao == Some([false; 2])
                            && h.deblock_disabled == Some(true),
                    ),
                    (
                        "filtered_intra",
                        h.slice_type == Some(2)
                            && (h.sao.is_some_and(|flags| flags.into_iter().any(|v| v))
                                || h.deblock_disabled == Some(false)),
                    ),
                    ("depth8", p.bit_depth == 8),
                    ("depth10", p.bit_depth == 10),
                ] {
                    if present {
                        if let Some(value) = tools.get_mut(key) {
                            *value = json!(true);
                        }
                    }
                }
            }
            NalInfo::Sei(info) => {
                if let Some(value) = value.as_object_mut() {
                    value.insert(
                        "picture_index".into(),
                        json!(if header.unit_type == 40 {
                            picture_index
                        } else {
                            None
                        }),
                    );
                    value.insert("sei_messages".into(), json!(info.messages));
                    value.insert("picture_hash".into(), json!(info.picture_hash.is_some()));
                    value.insert(
                        "picture_hash_reference".into(),
                        info.picture_hash.map_or(Value::Null, |hash| match hash {
                            coreav_hevc::PictureHash::Md5(components) => {
                                json!({"algorithm":"md5","components":components})
                            }
                            coreav_hevc::PictureHash::Crc(components) => {
                                json!({"algorithm":"crc","components":components})
                            }
                            coreav_hevc::PictureHash::Checksum(components) => {
                                json!({"algorithm":"checksum","components":components})
                            }
                        }),
                    );
                    value.insert("reserved_hashes".into(), json!(info.reserved_hashes));
                }
            }
            _ => {}
        }
        row(output, value)?;
    }
    let summary = parser.finish(|event| emit(output, event))?;
    row(
        output,
        json!({"kind":"summary","complete":true,"bytes":data.len(),"nals":summary.nals,
        "pictures":summary.pictures,"output_pictures":summary.output_pictures,
        "output_flag_pictures":summary.output_flag_pictures,"independent_slices":summary.independent_slices,
        "dependent_slices":summary.dependent_slices,"picture_hashes":summary.picture_hashes,
        "reserved_hashes":summary.reserved_hashes,"tools":tools,
        "warnings":{"profile_bit_depth":summary.warnings.profile_bit_depth,
            "invalid_sample_aspect_ratio":summary.warnings.invalid_sample_aspect_ratio,
            "temporal_nesting_nal_type":summary.warnings.temporal_nesting_nal_type}}),
    )
}

fn main() -> std::process::ExitCode {
    let mut output = std::io::BufWriter::new(std::io::stdout().lock());
    match inspect(&mut output).and_then(|()| {
        output
            .flush()
            .map_err(|_| Error::Limit("inspection_output"))
    }) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let (category, reason) = match error {
                Error::InvalidData(reason) => ("invalid_data", reason),
                Error::Unsupported(reason) => ("unsupported", reason),
                Error::Limit(reason) => ("limit", reason),
                Error::Internal(reason) => ("backend", reason),
            };
            let _ = row(
                &mut output,
                json!({"kind":"refusal","category":category,"reason":reason}),
            );
            std::process::ExitCode::FAILURE
        }
    }
}
