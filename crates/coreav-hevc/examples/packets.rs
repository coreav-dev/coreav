//! Container-packet test adapter; not a registered decode backend.

use coreav_decode::{Codec, ConfigKind};
use coreav_hevc::{DecodeLimits, Error, LengthPrefixed, NalHeader, PictureDecoder, Result};
use coreav_isobmff::{FileSource, Rational, Source};
use coreav_thumbnail::{Container, ThumbnailOptions, VideoStream};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Write;

mod common;
#[path = "common/decoded.rs"]
mod decoded;

const PACKET_LIMIT: u64 = 64 << 20;
const PACKET_COUNT_LIMIT: usize = 1_000_000;

fn integer<T: std::str::FromStr>(value: Option<std::ffi::OsString>) -> Result<T> {
    value
        .and_then(|v| v.into_string().ok())
        .and_then(|v| v.parse().ok())
        .ok_or(Error::InvalidData("packet_arguments"))
}

fn clocks(stream: &VideoStream, source: &mut FileSource) -> Result<Vec<i64>> {
    let mut clocks = Vec::new();
    clocks
        .try_reserve_exact(stream.packets.len())
        .map_err(|_| Error::Limit("packet_clock_memory"))?;
    match stream.container {
        Container::Matroska => clocks.extend(stream.packets.iter().map(|p| p.pts)),
        Container::Mp4 => {
            let limits = coreav_isobmff::Limits::default();
            let mut movie =
                coreav_isobmff::probe(&mut *source, &limits, &coreav_isobmff::Allowlist::default())
                    .map_err(|_| Error::InvalidData("packet_clock_container"))?;
            if movie.needs_fragment_scan() {
                movie
                    .scan_fragments(&mut *source, &limits)
                    .map_err(|_| Error::InvalidData("packet_clock_fragments"))?;
            }
            let track = movie
                .tracks
                .iter()
                .find(|t| t.stream_index == stream.stream_index)
                .ok_or(Error::InvalidData("packet_clock_track"))?;
            if stream.time_base
                != Rational::new(1, u64::from(track.timescale))
                    .ok_or(Error::InvalidData("packet_time_base"))?
            {
                return Err(Error::InvalidData("packet_time_base"));
            }
            let mut budget = coreav_isobmff::Budget::new(&limits);
            let packets = movie
                .packets(track.index, &mut *source, &limits, &mut budget)
                .map_err(|_| Error::InvalidData("packet_clock_listing"))?;
            for (index, packet) in packets.enumerate() {
                let packet = packet.map_err(|_| Error::InvalidData("packet_clock_listing"))?;
                let original = stream
                    .packets
                    .get(index)
                    .ok_or(Error::InvalidData("packet_clock_count"))?;
                if (
                    packet.offset,
                    u64::from(packet.size),
                    packet.cts,
                    packet.presented,
                ) != (
                    original.offset,
                    original.size,
                    original.pts,
                    original.presented,
                ) {
                    return Err(Error::InvalidData("packet_clock_identity"));
                }
                let time = packet
                    .pts
                    .ok_or(Error::Unsupported("packet_edit_mapping"))?;
                let time =
                    i64::try_from(time.round()).map_err(|_| Error::Limit("packet_timestamp"))?;
                clocks.push(time);
            }
        }
    }
    if clocks.len() != stream.packets.len() {
        return Err(Error::InvalidData("packet_clock_count"));
    }
    Ok(clocks)
}

fn run(writer: &mut impl Write) -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = args.next().ok_or(Error::InvalidData("packet_arguments"))?;
    let config = args.next().ok_or(Error::InvalidData("packet_arguments"))?;
    let stream_index: usize = integer(args.next())?;
    let packet_limit: usize = integer(args.next())?;
    let limits = DecodeLimits {
        max_memory_bytes: integer(args.next())?,
        max_stream_bins: integer(args.next())?,
        max_picture_bins: integer(args.next())?,
        ..DecodeLimits::default()
    };
    if args.next().is_some() || !matches!(packet_limit, 0 | 120) {
        return Err(Error::InvalidData("packet_arguments"));
    }
    let mut source = FileSource::open(path).map_err(|_| Error::InvalidData("input_open"))?;
    let mut options = ThumbnailOptions::at(Rational::from_int(0));
    options.stream = Some(stream_index);
    let stream = coreav_thumbnail::video_stream(&mut source, &options)
        .map_err(|_| Error::InvalidData("packet_container"))?;
    if stream.stream.codec != Codec::Hevc
        || stream.packets.is_empty()
        || stream.packets.len() > PACKET_COUNT_LIMIT
        || !stream.warnings.is_empty()
    {
        return Err(Error::InvalidData("packet_stream"));
    }
    let config_record = stream
        .stream
        .config
        .as_ref()
        .filter(|c| c.kind == ConfigKind::Hvcc)
        .ok_or(Error::Unsupported("packet_configuration"))?;
    let length_size = config_record
        .data
        .get(21)
        .map(|v| (v & 3) + 1)
        .ok_or(Error::InvalidData("packet_configuration"))?;
    let clocks = clocks(&stream, &mut source)?;
    let sent = if packet_limit == 0 {
        stream.packets.len()
    } else {
        stream.packets.len().min(packet_limit)
    };
    let cutoff = clocks
        .get(sent..)
        .and_then(|times| times.iter().copied().min());
    decoded::json_line(
        writer,
        &json!({"type":"stream", "stream_index":stream.stream_index,
        "container":match stream.container {Container::Mp4=>"mp4",Container::Matroska=>"matroska"},
        "time_base":{"num":stream.time_base.num.to_string(),"den":stream.time_base.den},
        "clock":"container_presentation_ticks_nearest_away", "length_size":length_size,
        "packet_count":stream.packets.len(),"sent_packets":sent,"cutoff":cutoff,
        "max_packet_bytes":PACKET_LIMIT,"max_packets":PACKET_COUNT_LIMIT,
        "presentation":stream.presentation.map(|w|json!({"start":w.start,"end":w.end}))}),
    )?;
    for (index, (packet, timestamp)) in stream.packets.iter().zip(&clocks).enumerate() {
        decoded::json_line(
            writer,
            &json!({"type":"packet","index":index,
            "offset":packet.offset,"size":packet.size,"media_pts":packet.pts,"pts":timestamp,
            "dts":packet.dts,"duration":packet.duration,"presented":packet.presented,
            "keyframe":packet.keyframe,"sent":index<sent}),
        )?;
    }
    let mut decoder = PictureDecoder::new(limits);
    let mut output = decoded::Output::new(writer);
    let mut parameters = 0u64;
    let mut configuration_done = false;
    let mut receipt = common::read_nals(
        std::fs::File::open(config).map_err(|_| Error::InvalidData("packet_configuration_open"))?,
        limits.headers.max_nal_bytes,
        |_, nal| {
            let kind = NalHeader::parse(nal)?.unit_type;
            configuration_done |= kind < 32;
            if !configuration_done && (32..=34).contains(&kind) {
                parameters += 1;
                decoder.read_nal(nal, None, |picture| output.picture(picture))?;
            }
            Ok(())
        },
    )?;
    if parameters == 0 {
        return Err(Error::InvalidData("packet_configuration_empty"));
    }
    let mut input_bytes = receipt.bytes;
    for (index, timestamp) in clocks.iter().copied().take(sent).enumerate() {
        let stored = stream
            .packets
            .get(index)
            .ok_or(Error::InvalidData("packet_index"))?;
        if stored.size > PACKET_LIMIT {
            return Err(Error::Limit("packet_bytes"));
        }
        let mut raw = Vec::new();
        source
            .read_append(
                stored.offset,
                usize::try_from(stored.size).map_err(|_| Error::Limit("packet_bytes"))?,
                &mut raw,
            )
            .map_err(|_| Error::InvalidData("packet_stored_payload"))?;
        let stored_sha256 = format!("{:x}", Sha256::digest(&raw));
        receipt.capacity = receipt.capacity.max(raw.capacity());
        drop(raw);
        let packet = stream
            .read_packet(&mut source, index, PACKET_LIMIT)
            .map_err(|_| Error::InvalidData("packet_payload"))?;
        input_bytes = input_bytes
            .checked_add(packet.len() as u64)
            .filter(|n| *n <= common::INPUT_LIMIT)
            .ok_or(Error::Limit("packet_input_bytes"))?;
        receipt.capacity = receipt.capacity.max(packet.capacity());
        let mut hash = Sha256::new();
        hash.update(&packet);
        output.record(
            &json!({"type":"payload","index":index,"stored_sha256":stored_sha256,
            "bytes":packet.len(),"sha256":format!("{:x}",hash.finalize())}),
        )?;
        let mut first_slices = 0u32;
        for nal in LengthPrefixed::new(&packet, length_size)? {
            let nal = nal?;
            if NalHeader::parse(nal)?.unit_type < 32
                && nal.get(2).is_some_and(|byte| byte & 128 != 0)
            {
                first_slices += 1;
                if first_slices > 1 {
                    return Err(Error::InvalidData("packet_multiple_pictures"));
                }
            }
            decoder.read_nal(nal, Some(timestamp), |picture| output.picture(picture))?;
        }
    }
    receipt.bytes = input_bytes;
    let summary = decoder.finish(|picture| output.picture(picture))?;
    output.finish(summary, receipt, limits)
}

fn main() -> std::process::ExitCode {
    let mut writer = std::io::BufWriter::new(std::io::stdout().lock());
    match run(&mut writer) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let (category, reason) = match error {
                Error::InvalidData(reason) => ("invalid_data", reason),
                Error::Unsupported(reason) => ("unsupported", reason),
                Error::Limit(reason) => ("limit", reason),
            };
            let mut stderr = std::io::stderr().lock();
            let _ = serde_json::to_writer(
                &mut stderr,
                &json!({"kind":"refusal","category":category,"reason":reason}),
            );
            let _ = stderr.write_all(b"\n");
            std::process::ExitCode::FAILURE
        }
    }
}
