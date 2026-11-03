use super::{
    Data, Error, InputTrack, Media, MetadataBudget, OutputOptions, Packet, Rational, Source,
    Stored, Track, cadence, charge_next, config, iso, ns, ns_u, overflow, unsupported,
};
use coreav_isobmff::{Allowlist, EditList, Limits, TrackKind};

pub(super) fn isobmff<S: Source>(
    source: &mut S,
    options: &OutputOptions,
    limits: &Limits,
    mkv_limits: &coreav_mkv::Limits,
    replacements: &[(usize, Track)],
    total: &mut usize,
    metadata: &mut MetadataBudget,
) -> Result<Vec<InputTrack>, Error> {
    let probe_limits = Limits {
        max_retained_bytes: limits
            .max_retained_bytes
            .min(metadata.limit - metadata.used),
        ..*limits
    };
    let movie =
        coreav_isobmff::probe(&mut *source, &probe_limits, &Allowlist::listing()).map_err(iso)?;
    for t in &movie.tracks {
        metadata.track(
            t.description
                .codec_config
                .as_ref()
                .map_or(0, |c| c.data.len()),
            3,
            t.handler_name.as_ref().map_or(0, |s| s.len()),
            0,
        )?;
    }
    let all;
    let selected = if let Some(selected) = &options.streams {
        selected.as_slice()
    } else {
        all = movie
            .tracks
            .iter()
            .map(|t| t.stream_index)
            .collect::<Vec<_>>();
        &all
    };
    if selected.len() as u64 > u64::from(mkv_limits.max_tracks) {
        return Err(unsupported("too many Matroska output tracks"));
    }
    let mut tracks = Vec::new();
    let mut budget = coreav_isobmff::Budget::new(limits);
    for &index in selected {
        let t = movie
            .tracks
            .iter()
            .find(|t| t.stream_index == index)
            .ok_or_else(|| unsupported("a selected ISO track is not readable"))?;
        let replacement = replacements
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, h)| h);
        if !matches!(t.edit_list, EditList::None | EditList::Simple { .. }) {
            return Err(unsupported("Matroska output needs a simple edit list"));
        }
        if t.kind == TrackKind::Video
            && (t.rotation != Some(0) || t.matrix != coreav_isobmff::IDENTITY_MATRIX)
        {
            return Err(unsupported(
                "a rotated or transformed ISO track in Matroska output",
            ));
        }
        let d = &t.description;
        if replacement.is_none()
            && (!t.tables_valid
                || t.external_data
                || t.descriptions != 1
                || d.sound_packet.is_some())
        {
            return Err(unsupported(
                "Matroska copy needs one internal frame-based sample description",
            ));
        }
        let mut header = if let Some(header) = replacement {
            if matches!(header.media, Media::Video { .. }) != (t.kind == TrackKind::Video)
                || !matches!(t.kind, TrackKind::Video | TrackKind::Audio)
            {
                return Err(unsupported(
                    "a replacement does not match its source stream kind",
                ));
            }
            metadata.track(
                header.private.len(),
                3,
                header.name.as_ref().map_or(0, |s| s.len()),
                0,
            )?;
            header.clone()
        } else {
            metadata.track(
                d.codec_config
                    .as_ref()
                    .map_or(0, |c| c.data.len())
                    .checked_add(4)
                    .ok_or_else(overflow)?,
                3,
                0,
                0,
            )?;
            let mut header = iso_mapping(d)?;
            if let Media::Video { colour, .. } = &mut header.media {
                *colour = d.color;
            }
            header
        };
        header.language = t
            .language
            .and_then(|l| String::from_utf8(l.to_vec()).ok())
            .unwrap_or_else(|| "und".into());
        let media = match t.edit_list {
            EditList::Simple { media_time, .. } => media_time,
            _ => 0,
        };
        let mut trim =
            u64::try_from(media).map_err(|_| unsupported("a negative media edit time"))?;
        if replacement.is_none() && d.codec == coreav_isobmff::Codec::Opus {
            let skip = header
                .private
                .get(10..12)
                .and_then(|v| <[u8; 2]>::try_from(v).ok())
                .map(u16::from_le_bytes)
                .ok_or_else(|| unsupported("invalid OpusHead"))?;
            let ticks = u64::from(skip)
                .checked_mul(u64::from(t.timescale))
                .ok_or_else(overflow)?;
            if ticks % 48_000 != 0 {
                return Err(unsupported("Opus pre-skip is not a whole media tick"));
            }
            trim = trim.max(ticks / 48_000);
        }
        if replacement.is_none() && t.kind == TrackKind::Audio {
            config::set_delay(&mut header, trim, t.timescale)?;
        }
        let offset = t
            .presentation_offset(movie.timescale)
            .and_then(|v| v.checked_add(Rational::from_int(-i128::from(trim))))
            .ok_or_else(overflow)?;
        let end = t
            .presentation(movie.timescale)
            .map(|(_, end)| {
                ns(end.num, end.den).and_then(|end| {
                    end.checked_sub(
                        ns_u(trim.saturating_sub(media as u64), u64::from(t.timescale))? as i64,
                    )
                    .ok_or_else(overflow)
                })
            })
            .transpose()?;
        let mut track = InputTrack {
            index,
            header,
            packets: Vec::new(),
            time_base: Rational::new(1, u64::from(t.timescale)).ok_or_else(overflow)?,
            offset,
            prefix: Vec::new(),
            replaced: replacement.is_some(),
        };
        if replacement.is_some() {
            tracks.push(track);
            continue;
        }
        let mut packets = movie
            .packets(t.index, &mut *source, limits, &mut budget)
            .map_err(iso)?
            .peekable();
        while let Some(packet) = packets.next() {
            let p = packet.map_err(iso)?;
            let pts = track.presentation_ns(p.cts)?;
            let dts = track.presentation_ns(p.dts)?;
            let sample_duration = ns_u(u64::from(p.duration), u64::from(t.timescale))?;
            let encoded_duration = match (d.aac_frame_length, d.configured_sample_rate) {
                (Some(samples), Some(rate)) if d.codec == coreav_isobmff::Codec::Aac => {
                    ns_u(u64::from(samples), u64::from(rate))?
                }
                _ => sample_duration,
            };
            let mut duration = if d.codec == coreav_isobmff::Codec::Aac && packets.peek().is_some()
            {
                encoded_duration
            } else {
                sample_duration
            };
            if let Some(end) = end
                && pts < end
            {
                duration = duration.min(end.abs_diff(pts));
            }
            if sample_duration == 0 || duration == 0 {
                return Err(unsupported("an ISO sample without a duration"));
            }
            charge_next(total, mkv_limits)?;
            track.packets.push(Stored {
                packet: Packet {
                    track: 0,
                    pts_ns: super::output_pts(pts, track.header.codec_delay_ns)?,
                    duration_ns: Some(if t.kind == TrackKind::Video {
                        duration
                    } else {
                        encoded_duration
                    }),
                    size: u64::from(p.size),
                    keyframe: p.keyframe,
                    invisible: t.kind == TrackKind::Video && !p.presented,
                    discard_padding_ns: (t.kind == TrackKind::Audio)
                        .then_some(encoded_duration)
                        .and_then(|encoded| encoded.checked_sub(duration).filter(|p| *p != 0))
                        .map(|p| i64::try_from(p).map_err(|_| overflow()))
                        .transpose()?,
                },
                dts_ns: dts,
                offset: p.offset,
                data: Data::Source,
            });
        }
        if d.codec == coreav_isobmff::Codec::Opus {
            for stored in &mut track.packets {
                let mut bytes = [0u8; 2];
                let size = usize::try_from(stored.packet.size.min(2)).map_err(|_| overflow())?;
                source
                    .read_exact_at(stored.offset, bytes.get_mut(..size).ok_or_else(overflow)?)
                    .map_err(iso)?;
                let duration = config::opus_duration(bytes.get(..size).ok_or_else(overflow)?)?;
                let extra = duration
                    .checked_sub(stored.packet.duration_ns.ok_or_else(overflow)?)
                    .ok_or_else(|| {
                        unsupported("an Opus sample duration exceeds its encoded frame")
                    })?;
                let padding = stored
                    .packet
                    .discard_padding_ns
                    .unwrap_or(0)
                    .checked_add(i64::try_from(extra).map_err(|_| overflow())?)
                    .ok_or_else(overflow)?;
                stored.packet.duration_ns = Some(duration);
                stored.packet.discard_padding_ns = (padding != 0).then_some(padding);
            }
        }
        while track.packets.last().is_some_and(|s| {
            if t.kind == TrackKind::Video {
                s.packet.invisible
            } else {
                end.is_some_and(|end| s.dts_ns >= end)
            }
        }) {
            track.packets.pop();
        }
        cadence(&mut track);
        tracks.push(track);
    }
    Ok(tracks)
}

pub(super) fn matroska<S: Source>(
    source: &mut S,
    options: &OutputOptions,
    limits: &coreav_mkv::Limits,
    replacements: &[(usize, Track)],
    total: &mut usize,
    metadata: &mut MetadataBudget,
) -> Result<Vec<InputTrack>, Error> {
    let probe_limits = coreav_mkv::Limits {
        max_retained_bytes: metadata.limit - metadata.used,
        ..*limits
    };
    let segment = coreav_mkv::probe(&mut *source, &probe_limits, &coreav_mkv::Allowlist::any())?;
    for t in &segment.tracks {
        metadata.track(
            t.codec_private.as_ref().map_or(0, |p| p.len()),
            t.language.len() + t.language_iso639.as_ref().map_or(0, |s| s.len()) + t.codec_id.len(),
            t.name.as_ref().map_or(0, |s| s.len()),
            match &t.encoding {
                coreav_mkv::Encoding::HeaderStripping(p) => p.len(),
                _ => 0,
            },
        )?;
    }
    let all;
    let selected = if let Some(selected) = &options.streams {
        selected.as_slice()
    } else {
        all = segment
            .tracks
            .iter()
            .map(|t| t.stream_index)
            .collect::<Vec<_>>();
        &all
    };
    if selected.len() as u64 > u64::from(limits.max_tracks) {
        return Err(unsupported("too many Matroska output tracks"));
    }
    let mut tracks = Vec::new();
    for &index in selected {
        let t = segment
            .tracks
            .iter()
            .find(|t| t.stream_index == index)
            .ok_or_else(|| unsupported("a selected Matroska track is not readable"))?;
        let replacement = replacements
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, h)| h);
        if !t.timestamp_scale_one || replacement.is_none() && !t.packets_available() {
            return Err(unsupported(
                "a Matroska track with unsupported timing or encoding",
            ));
        }
        let media = match (&t.video, &t.audio) {
            (Some(v), _) => {
                if v.rotation() != Some(0) || v.crop != [0; 4] || v.alpha_mode != 0 {
                    return Err(unsupported("a transformed or alpha Matroska video track"));
                }
                Media::Video {
                    width: u32::try_from(v.pixel_width.unwrap_or(0)).map_err(|_| overflow())?,
                    height: u32::try_from(v.pixel_height.unwrap_or(0)).map_err(|_| overflow())?,
                    colour: t.colour,
                    interlaced: u8::try_from(v.interlaced).map_err(|_| overflow())?,
                    field_order: u8::try_from(v.field_order).map_err(|_| overflow())?,
                    display: v
                        .display_width
                        .zip(v.display_height)
                        .map(|(w, h)| {
                            Ok::<_, Error>((
                                u32::try_from(w).map_err(|_| overflow())?,
                                u32::try_from(h).map_err(|_| overflow())?,
                            ))
                        })
                        .transpose()?,
                }
            }
            (_, Some(a)) => {
                matroska_audio_rate(a, replacement.is_none())?;
                Media::Audio {
                    sample_rate: a.sampling_frequency as u32,
                    channels: u32::try_from(a.channels).map_err(|_| overflow())?,
                    bit_depth: a
                        .bit_depth
                        .map(u32::try_from)
                        .transpose()
                        .map_err(|_| overflow())?,
                }
            }
            _ => return Err(unsupported("a Matroska track other than video or audio")),
        };
        let prefix_size = match &t.encoding {
            coreav_mkv::Encoding::HeaderStripping(p) if replacement.is_none() => p.len(),
            _ => 0,
        };
        metadata.track(
            replacement.map_or_else(
                || t.codec_private.as_ref().map_or(0, |p| p.len()),
                |h| h.private.len(),
            ),
            t.language_iso639.as_ref().map_or(3, |s| s.len()),
            t.name.as_ref().map_or(0, |s| s.len()),
            prefix_size,
        )?;
        let prefix = match &t.encoding {
            _ if replacement.is_some() => Vec::new(),
            coreav_mkv::Encoding::None => Vec::new(),
            coreav_mkv::Encoding::HeaderStripping(p) => p.clone(),
            _ => {
                return Err(unsupported(
                    "compressed or encrypted Matroska output packets",
                ));
            }
        };
        let mut header = if let Some(header) = replacement {
            if matches!(header.media, Media::Video { .. }) != matches!(media, Media::Video { .. }) {
                return Err(unsupported(
                    "a replacement does not match its source stream kind",
                ));
            }
            header.clone()
        } else {
            Track {
                codec: config::codec_id(&t.codec_id)?,
                private: t.codec_private.clone().unwrap_or_default(),
                media,
                default_duration_ns: t.default_duration,
                codec_delay_ns: t.codec_delay,
                seek_pre_roll_ns: t.seek_pre_roll,
                language: String::new(),
                name: None,
                default: t.default,
            }
        };
        header.language = t.language_iso639.clone().unwrap_or_else(|| "und".into());
        header.name = t.name.clone();
        header.default = t.default;
        tracks.push(InputTrack {
            index,
            header,
            packets: Vec::new(),
            time_base: Rational::new(i128::from(segment.timestamp_scale), 1_000_000_000)
                .ok_or_else(overflow)?,
            offset: Rational::from_int(0),
            prefix,
            replaced: replacement.is_some(),
        });
    }
    for packet in segment.packets(&mut *source, limits) {
        let p = packet?;
        let original = segment.tracks.get(p.track).ok_or_else(overflow)?;
        for t in tracks
            .iter_mut()
            .filter(|t| t.index == original.stream_index && !t.replaced)
        {
            if p.block_additions.is_some() {
                return Err(unsupported("Matroska block additions in copied output"));
            }
            if p.frames_in_block > 1 && t.header.default_duration_ns.is_none() {
                return Err(unsupported("Matroska laces without per-frame duration"));
            }
            let pts = i64::try_from(
                p.pts_ns(&segment)
                    .ok_or_else(|| unsupported("an unknown Matroska packet timestamp"))?,
            )
            .map_err(|_| overflow())?;
            let padding = p.discard_padding.filter(|v| {
                if *v > 0 {
                    p.lace + 1 == p.frames_in_block
                } else {
                    p.lace == 0
                }
            });
            let duration = p
                .block_duration
                .map(|d| {
                    d.checked_mul(segment.timestamp_scale)
                        .and_then(|n| {
                            if p.frames_in_block == 1 {
                                Some(n)
                            } else {
                                t.header.default_duration_ns.and_then(|step| {
                                    if p.lace + 1 == p.frames_in_block {
                                        n.checked_sub(
                                            step.checked_mul(u64::from(p.frames_in_block - 1))?,
                                        )
                                    } else {
                                        Some(step)
                                    }
                                })
                            }
                        })
                        .and_then(|n| n.checked_add(padding.unwrap_or(0).max(0) as u64))
                        .ok_or_else(overflow)
                })
                .transpose()?
                .or(t.header.default_duration_ns);
            charge_next(total, limits)?;
            t.packets.push(Stored {
                packet: Packet {
                    track: 0,
                    pts_ns: pts,
                    duration_ns: duration,
                    size: p.size(),
                    keyframe: p.keyframe,
                    invisible: p.invisible,
                    discard_padding_ns: padding,
                },
                dts_ns: pts,
                offset: p.offset,
                data: Data::Source,
            });
        }
    }
    Ok(tracks)
}

/// The header `write_output` builds for a copied ISO track. One mapping,
/// shared with the encode preflight.
pub(super) fn iso_mapping(d: &coreav_isobmff::SampleDescription) -> Result<Track, Error> {
    config::description(
        config::iso_codec(d.codec)?,
        d.width
            .zip(d.height)
            .map(|(w, h)| (u32::from(w), u32::from(h))),
        d.pixel_aspect,
        d.configured_sample_rate.or(d.sample_rate),
        d.channels,
        d.configured_bits.map(u32::from),
        d.codec_config
            .as_ref()
            .map_or(&[][..], |c| c.data.as_slice()),
    )
}

/// The sampling-frequency refusal `write_output` raises. A replaced track
/// skips the preservation check: the encoder's clock replaces the source.
pub(super) fn matroska_audio_rate(audio: &coreav_mkv::Audio, copied: bool) -> Result<(), Error> {
    if copied
        && (audio.sampling_frequency.fract() != 0.0
            || audio
                .output_sampling_frequency
                .is_some_and(|rate| rate != audio.sampling_frequency))
    {
        return Err(unsupported(
            "Matroska output cannot preserve this sampling frequency",
        ));
    }
    if !audio.sampling_frequency.is_finite()
        || audio.sampling_frequency < 1.0
        || audio.sampling_frequency > f64::from(u32::MAX)
    {
        return Err(unsupported("an invalid Matroska sampling frequency"));
    }
    Ok(())
}

/// The writer's refusal when a copied audio stream cannot be mapped.
/// A stream that is not audio, or a file that cannot be probed, is `Ok`:
/// the writer still owns those failures. Reads descriptions only.
pub(super) fn copied_audio_mapping<S: Source>(
    source: &mut S,
    streams: &[usize],
) -> Result<(), Error> {
    if streams.is_empty() {
        return Ok(());
    }
    if matroska_prefix(source) {
        copied_matroska_audio(source, streams)
    } else {
        copied_iso_audio(source, streams)
    }
}

fn matroska_prefix<S: Source>(source: &mut S) -> bool {
    let Ok(size) = source.len() else {
        return false;
    };
    if size < 4 {
        return false;
    }
    let mut prefix = [0u8; 4];
    source.read_exact_at(0, &mut prefix).is_ok() && coreav_mkv::sniff(&prefix)
}

fn copied_iso_audio<S: Source>(source: &mut S, streams: &[usize]) -> Result<(), Error> {
    let movie =
        coreav_isobmff::probe(source, &Limits::default(), &Allowlist::listing()).map_err(iso)?;
    for &index in streams {
        let track = movie
            .tracks
            .iter()
            .find(|t| t.stream_index == index)
            .ok_or_else(|| unsupported("a selected ISO track is not readable"))?;
        if track.kind != TrackKind::Audio {
            continue;
        }
        let _ = iso_mapping(&track.description)?;
    }
    Ok(())
}

fn copied_matroska_audio<S: Source>(source: &mut S, streams: &[usize]) -> Result<(), Error> {
    let segment = coreav_mkv::probe(
        source,
        &coreav_mkv::Limits::default(),
        &coreav_mkv::Allowlist::any(),
    )?;
    for &index in streams {
        let track = segment
            .tracks
            .iter()
            .find(|t| t.stream_index == index)
            .ok_or_else(|| unsupported("a selected Matroska track is not readable"))?;
        let Some(audio) = &track.audio else {
            continue;
        };
        matroska_audio_rate(audio, true)?;
        let _ = config::codec_id(&track.codec_id)?;
    }
    Ok(())
}
