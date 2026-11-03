//! Matroska and WebM output from source packets and encoded replacements.

mod config;
mod source;

use crate::{Replacements, replace};
use coreav_isobmff::{Limits, Rational, Source};
use coreav_mkv::write::{Media, Options, Packet, Track};
use coreav_mkv::{DocType, Error};
use std::collections::{BTreeSet, VecDeque};
use std::io::Write;

/// Source selection and output profile.
#[derive(Clone, Debug)]
pub struct OutputOptions {
    /// Matroska input rather than ISO base media.
    pub matroska_source: bool,
    /// Matroska or WebM output.
    pub doc_type: DocType,
    /// Source stream positions in output order, or all readable tracks.
    pub streams: Option<Vec<usize>>,
}

/// One written track.
#[derive(Clone, Debug)]
pub struct OutputTrack {
    /// Source stream position.
    pub stream_index: usize,
    /// Output codec.
    pub codec: coreav_mkv::Codec,
    /// Whether its packets were encoded rather than copied.
    pub replaced: bool,
}

/// Output size and tracks in output order.
#[derive(Clone, Debug)]
pub struct OutputSummary {
    /// Complete file size.
    pub bytes_written: u64,
    /// Written tracks.
    pub tracks: Vec<OutputTrack>,
}

#[derive(Clone, Copy)]
enum Data {
    Source,
    Video,
    Audio,
}

struct Stored {
    packet: Packet,
    dts_ns: i64,
    offset: u64,
    data: Data,
}

struct InputTrack {
    index: usize,
    header: Track,
    packets: Vec<Stored>,
    time_base: Rational,
    offset: Rational,
    prefix: Vec<u8>,
    replaced: bool,
}

struct MetadataBudget {
    used: u64,
    limit: u64,
}

impl MetadataBudget {
    fn charge(&mut self, bytes: u64) -> Result<(), Error> {
        self.used = self
            .used
            .checked_add(bytes)
            .filter(|used| *used <= self.limit)
            .ok_or(Error::LimitExceeded {
                limit: coreav_mkv::LimitKind::RetainedBytes,
                offset: 0,
            })?;
        Ok(())
    }

    fn track(
        &mut self,
        private: usize,
        language: usize,
        name: usize,
        prefix: usize,
    ) -> Result<(), Error> {
        for bytes in [private, language, name, prefix, 256] {
            self.charge(bytes as u64)?;
        }
        Ok(())
    }
}

/// Plans selected tracks and replacements, then streams their payloads to
/// the output. Media stays in its original source or encoder spool.
pub fn write_output<S: Source, R: Source, W: Write>(
    mut source: S,
    options: &OutputOptions,
    limits: &Limits,
    mkv_limits: &coreav_mkv::Limits,
    mut replacements: Replacements<'_, R>,
    out: W,
) -> Result<OutputSummary, Error> {
    if options
        .streams
        .as_ref()
        .is_some_and(|streams| streams.len() as u64 > u64::from(mkv_limits.max_tracks))
    {
        return Err(unsupported("too many Matroska output tracks"));
    }
    let mut metadata = MetadataBudget {
        used: 0,
        limit: mkv_limits.max_retained_bytes,
    };
    if let Some(v) = &mut replacements.video {
        replace::validate(v, limits).map_err(iso)?;
    }
    if let Some(a) = &mut replacements.audio {
        replace::validate_audio(a, limits).map_err(iso)?;
    }
    let mut replacement_headers = Vec::new();
    if let Some(v) = &replacements.video {
        metadata.track(v.sample_entry.len(), 3, 0, 0)?;
        replacement_headers.push((v.stream_index, config::entry(v.sample_entry, false)?));
    }
    if let Some(a) = &replacements.audio {
        if replacement_headers
            .iter()
            .any(|(i, _)| *i == a.stream_index)
        {
            return Err(unsupported("one stream cannot have two replacements"));
        }
        metadata.track(a.sample_entry.len(), 3, 0, 0)?;
        replacement_headers.push((a.stream_index, config::entry(a.sample_entry, true)?));
    }
    let mut total = 0usize;
    let mut tracks = if options.matroska_source {
        source::matroska(
            &mut source,
            options,
            mkv_limits,
            &replacement_headers,
            &mut total,
            &mut metadata,
        )?
    } else {
        source::isobmff(
            &mut source,
            options,
            limits,
            mkv_limits,
            &replacement_headers,
            &mut total,
            &mut metadata,
        )?
    };
    if replacement_headers
        .iter()
        .any(|(index, _)| !tracks.iter().any(|t| t.index == *index))
    {
        return Err(unsupported(
            "a replacement names an unselected source stream",
        ));
    }
    for t in &mut tracks {
        if let Some(v) = &replacements.video
            && v.stream_index == t.index
        {
            let mut offset = 0u64;
            for (sample, duration) in replace::durations(v.samples, v.last_duration) {
                let pts = t.presentation_ns(sample.presentation_time)?;
                charge_next(&mut total, mkv_limits)?;
                t.packets.push(Stored {
                    packet: Packet {
                        track: 0,
                        pts_ns: output_pts(pts, t.header.codec_delay_ns)?,
                        duration_ns: Some(media_ns(i128::from(duration), t.time_base)? as u64),
                        size: u64::from(sample.size),
                        keyframe: sample.sync,
                        invisible: false,
                        discard_padding_ns: None,
                    },
                    dts_ns: t.presentation_ns(sample.decode_time)?,
                    offset,
                    data: Data::Video,
                });
                offset = offset
                    .checked_add(u64::from(sample.size))
                    .ok_or_else(overflow)?;
            }
            cadence(t);
            t.prefix.clear();
            t.replaced = true;
        }
        if let Some(a) = &replacements.audio
            && a.stream_index == t.index
        {
            config::set_delay(&mut t.header, u64::from(a.priming), a.timescale)?;
            let end = u64::from(a.priming)
                .checked_add(a.presented)
                .ok_or_else(overflow)?;
            let mut time = 0u64;
            let mut offset = 0u64;
            for sample in a.samples {
                if time >= end {
                    break;
                }
                let packet_end = time
                    .checked_add(u64::from(sample.duration))
                    .ok_or_else(overflow)?;
                let pts = ns(
                    i128::from(time) + i128::from(a.start) - i128::from(a.priming),
                    u64::from(a.timescale),
                )?;
                let padding = packet_end
                    .saturating_sub(end)
                    .min(u64::from(sample.duration));
                charge_next(&mut total, mkv_limits)?;
                t.packets.push(Stored {
                    packet: Packet {
                        track: 0,
                        pts_ns: output_pts(pts, t.header.codec_delay_ns)?,
                        duration_ns: Some(ns_u(
                            u64::from(sample.duration),
                            u64::from(a.timescale),
                        )?),
                        size: u64::from(sample.size),
                        keyframe: true,
                        invisible: false,
                        discard_padding_ns: (padding != 0)
                            .then(|| ns(i128::from(padding), u64::from(a.timescale)))
                            .transpose()?,
                    },
                    dts_ns: pts,
                    offset,
                    data: Data::Audio,
                });
                time = packet_end;
                offset = offset
                    .checked_add(u64::from(sample.size))
                    .ok_or_else(overflow)?;
            }
            t.prefix.clear();
            t.replaced = true;
        }
    }
    let mut queues = Vec::new();
    for (index, t) in tracks.iter_mut().enumerate() {
        let mut queue = VecDeque::new();
        for mut packet in t.packets.drain(..) {
            packet.packet.track = index;
            queue.push_back(packet);
        }
        queues.push(queue);
    }
    let key = |s: &Stored| -> Result<i128, Error> {
        Ok(if options.doc_type == DocType::Webm {
            i128::from(s.packet.pts_ns)
                + i128::from(
                    tracks
                        .get(s.packet.track)
                        .ok_or_else(overflow)?
                        .header
                        .codec_delay_ns,
                )
        } else {
            i128::from(s.dts_ns)
        })
    };
    let mut heads = BTreeSet::new();
    for (index, queue) in queues.iter().enumerate() {
        if let Some(s) = queue.front() {
            heads.insert((key(s)?, index));
        }
    }
    let mut stored = Vec::with_capacity(total);
    while let Some((_, index)) = heads.pop_first() {
        let queue = queues.get_mut(index).ok_or_else(overflow)?;
        stored.push(queue.pop_front().ok_or_else(overflow)?);
        if let Some(s) = queue.front() {
            heads.insert((key(s)?, index));
        }
    }
    let summaries = tracks
        .iter()
        .map(|t| OutputTrack {
            stream_index: t.index,
            codec: coreav_mkv::Codec::from_codec_id(t.header.codec.as_str()),
            replaced: t.replaced,
        })
        .collect();
    let (headers, prefixes): (Vec<_>, Vec<_>) =
        tracks.into_iter().map(|t| (t.header, t.prefix)).unzip();
    let packets: Vec<_> = stored.iter().map(|s| s.packet).collect();
    let plan = coreav_mkv::write::plan(
        &headers,
        &packets,
        Options {
            doc_type: options.doc_type,
            ..Options::default()
        },
        mkv_limits,
    )?;
    let mut buffer = [0u8; 65536];
    let bytes_written = plan.write(out, |index, out| {
        let s = stored.get(index).ok_or_else(overflow)?;
        let prefix = prefixes.get(s.packet.track).ok_or_else(overflow)?;
        out.write_all(prefix).map_err(|e| Error::Io(e.kind()))?;
        let mut left = s
            .packet
            .size
            .checked_sub(prefix.len() as u64)
            .ok_or_else(overflow)?;
        let mut offset = s.offset;
        while left > 0 {
            let size = usize::try_from(left.min(buffer.len() as u64)).map_err(|_| overflow())?;
            let bytes = buffer.get_mut(..size).ok_or_else(overflow)?;
            match s.data {
                Data::Source => source.read_exact_at(offset, bytes).map_err(iso)?,
                Data::Video => replacements
                    .video
                    .as_mut()
                    .ok_or_else(overflow)?
                    .data
                    .read_exact_at(offset, bytes)
                    .map_err(iso)?,
                Data::Audio => replacements
                    .audio
                    .as_mut()
                    .ok_or_else(overflow)?
                    .data
                    .read_exact_at(offset, bytes)
                    .map_err(iso)?,
            }
            out.write_all(bytes).map_err(|e| Error::Io(e.kind()))?;
            left -= size as u64;
            offset = offset.checked_add(size as u64).ok_or_else(overflow)?;
        }
        Ok(())
    })?;
    Ok(OutputSummary {
        bytes_written,
        tracks: summaries,
    })
}

fn cadence(t: &mut InputTrack) {
    if !matches!(t.header.media, Media::Video { .. }) {
        return;
    }
    let mut times: Vec<_> = t.packets.iter().map(|s| s.packet.pts_ns).collect();
    times.sort_unstable();
    let step = times.windows(2).next().and_then(|w| match w {
        [a, b] => b.checked_sub(*a),
        _ => None,
    });
    if let Some(step) = step.filter(|s| *s > 0)
        && times
            .windows(2)
            .all(|w| matches!(w,[a,b] if b.checked_sub(*a)==Some(step)))
    {
        t.header.default_duration_ns = Some(step as u64);
    }
}

fn ns(value: i128, scale: u64) -> Result<i64, Error> {
    if scale == 0 {
        return Err(unsupported("zero media timescale"));
    }
    i64::try_from(
        (value.checked_mul(1_000_000_000).ok_or_else(overflow)? + i128::from(scale / 2))
            .div_euclid(i128::from(scale)),
    )
    .map_err(|_| overflow())
}
fn ns_u(value: u64, scale: u64) -> Result<u64, Error> {
    u64::try_from(ns(i128::from(value), scale)?).map_err(|_| overflow())
}
fn media_ns(value: i128, base: Rational) -> Result<i64, Error> {
    ns(value.checked_mul(base.num).ok_or_else(overflow)?, base.den)
}
fn milliseconds(value: i64) -> Result<i64, Error> {
    let magnitude = i128::from(value).abs();
    let rounded = (magnitude + 500_000) / 1_000_000 * 1_000_000;
    i64::try_from(if value < 0 { -rounded } else { rounded }).map_err(|_| overflow())
}
fn output_pts(value: i64, delay: u64) -> Result<i64, Error> {
    let delay = i64::try_from(delay).map_err(|_| overflow())?;
    milliseconds(value)?
        .checked_add(milliseconds(delay)?)
        .and_then(|v| v.checked_sub(delay))
        .ok_or_else(overflow)
}
impl InputTrack {
    fn presentation_ns(&self, value: i64) -> Result<i64, Error> {
        let shifted = Rational::from_int(i128::from(value))
            .checked_add(self.offset)
            .ok_or_else(overflow)?;
        ns(
            shifted
                .num
                .checked_mul(self.time_base.num)
                .ok_or_else(overflow)?,
            shifted
                .den
                .checked_mul(self.time_base.den)
                .ok_or_else(overflow)?,
        )
    }
}
fn overflow() -> Error {
    Error::Overflow { offset: 0 }
}
fn unsupported(reason: &'static str) -> Error {
    Error::Unsupported { reason }
}
fn iso(error: coreav_isobmff::Error) -> Error {
    match error {
        coreav_isobmff::Error::Unsupported { reason } => unsupported(reason),
        other => other.into(),
    }
}
fn charge(count: usize, limits: &coreav_mkv::Limits) -> Result<(), Error> {
    if count as u64 > limits.max_block_work
        || (count as u64).saturating_mul(1024) > limits.max_index_bytes
    {
        Err(Error::LimitExceeded {
            limit: coreav_mkv::LimitKind::IndexBytes,
            offset: 0,
        })
    } else {
        Ok(())
    }
}
fn charge_next(total: &mut usize, limits: &coreav_mkv::Limits) -> Result<(), Error> {
    *total = total.checked_add(1).ok_or_else(overflow)?;
    charge(*total, limits)
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn retained_metadata_is_charged_at_the_boundary_before_copying() {
        let mut budget = MetadataBudget {
            used: 0,
            limit: 4096 + 3 + 4096 + 256,
        };
        assert!(budget.track(4096, 3, 4096, 0).is_ok());
        assert_eq!(budget.used, budget.limit);
        assert!(matches!(
            budget.charge(1),
            Err(Error::LimitExceeded {
                limit: coreav_mkv::LimitKind::RetainedBytes,
                ..
            })
        ));
        assert_eq!(budget.used, budget.limit);
    }
}
