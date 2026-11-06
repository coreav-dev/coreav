//! Container-packet test adapter; not a registered decode backend.

use coreav_decode::{Codec, ConfigKind};
use coreav_hevc::{DecodeLimits, Error, LengthPrefixed, NalHeader, PictureDecoder, Result};
use coreav_isobmff::{EditList, FileSource, Rational, Source};
use coreav_thumbnail::{Container, StreamPacket, ThumbnailOptions, VideoStream};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Write;

mod common;
#[path = "common/decoded.rs"]
mod decoded;

const PACKET_LIMIT: u64 = 64 << 20;
const PACKET_COUNT_LIMIT: usize = 1_000_000;

fn packet_nal(nal: &[u8], max: usize) -> Result<(&[u8], usize)> {
    if nal.len() > max {
        return Err(Error::Limit("max_nal_bytes"));
    }
    let payload = nal
        .get(2..)
        .ok_or(Error::InvalidData("truncated_nal_header"))?;
    let end = 2 + payload
        .iter()
        .rposition(|&byte| byte != 0)
        .map_or(0, |i| i + 1);
    let coded = nal
        .get(..end)
        .ok_or(Error::InvalidData("packet_nal_bounds"))?;
    Ok((coded, nal.len() - end))
}

fn integer<T: std::str::FromStr>(value: Option<std::ffi::OsString>) -> Result<T> {
    value
        .and_then(|v| v.into_string().ok())
        .and_then(|v| v.parse().ok())
        .ok_or(Error::InvalidData("packet_arguments"))
}

struct PacketClock {
    pts: i64,
    presented: bool,
}

fn edit_snap(packets: &[StreamPacket], media_time: i64) -> Result<i64> {
    let start = media_time.max(0);
    let snap = packets
        .iter()
        .filter_map(|p| (p.pts >= start).then_some(p.pts))
        .min()
        .or_else(|| {
            packets.iter().find_map(|p| {
                let end = i128::from(p.pts) + i128::from(p.duration?);
                (end > i128::from(start)).then_some(p.pts)
            })
        })
        .ok_or(Error::Unsupported("packet_edit_snap"))?;
    snap.checked_sub(start)
        .ok_or(Error::Limit("packet_timestamp"))
}

fn clocks(stream: &VideoStream, source: &mut FileSource) -> Result<(Vec<PacketClock>, Value)> {
    let mut clocks = Vec::new();
    clocks
        .try_reserve_exact(stream.packets.len())
        .map_err(|_| Error::Limit("packet_clock_memory"))?;
    let mapping = match stream.container {
        Container::Matroska => {
            clocks.extend(stream.packets.iter().map(|p| PacketClock {
                pts: p.pts,
                presented: p.presented,
            }));
            json!({"kind":"segment_ticks"})
        }
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
            let snap = match track.edit_list {
                EditList::Simple { media_time, .. } if track.sample_count != 0 => {
                    edit_snap(&stream.packets, media_time)?
                }
                _ => 0,
            };
            let window = match track.edit_list {
                EditList::Simple {
                    media_time,
                    segment,
                    ..
                } if track.sample_count != 0 => {
                    let start = media_time.max(0);
                    let end = if segment == 0 {
                        None
                    } else {
                        let denominator = u128::from(movie.timescale.max(1));
                        let ticks = (u128::from(segment) * u128::from(track.timescale)
                            + denominator / 2)
                            / denominator;
                        Some(
                            i64::try_from(ticks)
                                .ok()
                                .and_then(|n| start.checked_add(n))
                                .ok_or(Error::Limit("packet_timestamp"))?,
                        )
                    };
                    Some((start, end))
                }
                _ => None,
            };
            let packets = movie
                .packets(track.index, &mut *source, &limits, &mut budget)
                .map_err(|_| Error::InvalidData("packet_clock_listing"))?;
            for (index, packet) in packets.enumerate() {
                let mut packet = packet.map_err(|_| Error::InvalidData("packet_clock_listing"))?;
                if track.ctts_signed() == Some(false)
                    && packet.cts.saturating_sub(packet.dts) >= 1 << 31
                {
                    packet.cts = packet
                        .cts
                        .checked_sub(1 << 32)
                        .ok_or(Error::Limit("packet_timestamp"))?;
                    packet.pts = packet
                        .pts
                        .and_then(|t| t.checked_add(Rational::from_int(-(1i128 << 32))));
                    packet.presented = true;
                }
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
                    .and_then(|t| t.checked_add(Rational::from_int(-i128::from(snap))))
                    .ok_or(Error::Unsupported("packet_edit_mapping"))?;
                let time =
                    i64::try_from(time.round()).map_err(|_| Error::Limit("packet_timestamp"))?;
                let presented = window.map_or(packet.presented, |(start, end)| {
                    packet.cts >= start && end.is_none_or(|end| packet.cts < end)
                });
                clocks.push(PacketClock {
                    pts: time,
                    presented,
                });
            }
            let offset = track
                .presentation_offset(movie.timescale)
                .ok_or(Error::Unsupported("packet_edit_mapping"))?;
            json!({"kind":"video_edit_start_snap", "snap_shift":snap,
                "edit_window":window,
                "movie_timescale":movie.timescale,"track_timescale":track.timescale,
                "sample_count":track.sample_count,"ctts_version":track.ctts_version(),
                "ctts_signed":track.ctts_signed(),
                "presentation_offset":{"num":offset.num.to_string(),"den":offset.den},
                "edits":track.edits.iter().map(|e|json!({"segment_duration":e.segment_duration,
                    "media_time":e.media_time,"media_rate":e.media_rate})).collect::<Vec<_>>()})
        }
    };
    if clocks.len() != stream.packets.len() {
        return Err(Error::InvalidData("packet_clock_count"));
    }
    Ok((clocks, mapping))
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
    let stream =
        coreav_thumbnail::video_stream(&mut source, &options).map_err(|error| match error {
            coreav_thumbnail::Error::Isobmff(coreav_isobmff::Error::Truncated { .. }) => {
                Error::InvalidData("packet_container_truncated")
            }
            coreav_thumbnail::Error::Isobmff(coreav_isobmff::Error::InvalidBox { .. }) => {
                Error::InvalidData("packet_container_structure")
            }
            coreav_thumbnail::Error::Unsupported(_) => {
                Error::Unsupported("packet_container_unsupported")
            }
            _ => Error::InvalidData("packet_container"),
        })?;
    if stream.stream.codec != Codec::Hevc
        || stream.packets.is_empty()
        || stream.packets.len() > PACKET_COUNT_LIMIT
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
    let (clocks, mapping) = clocks(&stream, &mut source)?;
    let sent = if packet_limit == 0 {
        stream.packets.len()
    } else {
        stream.packets.len().min(packet_limit)
    };
    let cutoff = clocks
        .get(sent..)
        .and_then(|times| times.iter().map(|c| c.pts).min());
    decoded::json_line(
        writer,
        &json!({"type":"stream", "stream_index":stream.stream_index,
        "container":match stream.container {Container::Mp4=>"mp4",Container::Matroska=>"matroska"},
        "time_base":{"num":stream.time_base.num.to_string(),"den":stream.time_base.den},
        "clock":"demux_presentation_ticks_nearest_away", "length_size":length_size,
        "mapping":mapping,
        "container_warnings":stream.warnings,
        "packet_count":stream.packets.len(),"sent_packets":sent,"cutoff":cutoff,
        "max_packet_bytes":PACKET_LIMIT,"max_packets":PACKET_COUNT_LIMIT,
        "presentation":stream.presentation.map(|w|json!({"start":w.start,"end":w.end}))}),
    )?;
    for (index, (packet, clock)) in stream.packets.iter().zip(&clocks).enumerate() {
        decoded::json_line(
            writer,
            &json!({"type":"packet","index":index,
            "offset":packet.offset,"size":packet.size,"media_pts":packet.pts,"pts":clock.pts,
            "dts":packet.dts,"duration":packet.duration,"presented":clock.presented,
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
    for (index, clock) in clocks.iter().take(sent).enumerate() {
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
        for (nal_index, nal) in LengthPrefixed::new(&packet, length_size)?.enumerate() {
            let nal = nal?;
            let (nal, padding) = packet_nal(nal, limits.headers.max_nal_bytes)?;
            if padding != 0 {
                output.record(&json!({"type":"nal_padding", "index":index,
                    "nal_index":nal_index, "coded_bytes":nal.len(), "trailing_zero_bytes":padding}))?;
            }
            if NalHeader::parse(nal)?.unit_type < 32
                && nal.get(2).is_some_and(|byte| byte & 128 != 0)
            {
                first_slices += 1;
                if first_slices > 1 {
                    return Err(Error::InvalidData("packet_multiple_pictures"));
                }
            }
            decoder.read_nal(nal, Some(clock.pts), |picture| output.picture(picture))?;
        }
    }
    receipt.bytes = input_bytes;
    let summary = decoder.finish(|picture| output.picture(picture))?;
    output.finish(summary, receipt, limits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(pts: i64, duration: u64) -> StreamPacket {
        StreamPacket {
            offset: 0,
            size: 1,
            pts,
            time: None,
            dts: Some(0),
            duration: Some(duration),
            keyframe: true,
            presented: true,
        }
    }

    #[test]
    fn edit_start_snaps_to_the_first_composition_start_not_an_overlapping_sample() {
        let packets = [packet(1507, 2997), packet(4504, 9), packet(3006, 1489)];
        assert_eq!(edit_snap(&packets, 1508), Ok(1498));
        assert_eq!(edit_snap(&packets, 1507), Ok(0));
        assert_eq!(edit_snap(&[packet(100, 100)], 150), Ok(-50));
        assert_eq!(
            edit_snap(&[packet(100, 100)], 200),
            Err(Error::Unsupported("packet_edit_snap"))
        );
    }

    #[test]
    fn packet_padding_matches_annex_b_without_removing_cabac_zero_words() {
        let padded = [2, 1, 0x80, 0, 0];
        assert_eq!(packet_nal(&padded, 5), Ok((&padded[..3], 2)));
        let zero_word = [2, 1, 0x80, 0, 0, 3];
        assert_eq!(packet_nal(&zero_word, 6), Ok((&zero_word[..], 0)));
        assert_eq!(packet_nal(&padded, 4), Err(Error::Limit("max_nal_bytes")));
        assert_eq!(
            packet_nal(&[2], 4),
            Err(Error::InvalidData("truncated_nal_header"))
        );
        let stored = LengthPrefixed::new(&[0, 5, 2, 1, 0x80, 0, 0], 2)
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(stored, padded);
        let annex = coreav_hevc::AnnexB::new(&[0, 0, 1, 2, 1, 0x80, 0, 0])
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(packet_nal(stored, 5).unwrap().0, annex);
    }
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
