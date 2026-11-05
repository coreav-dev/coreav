use super::{Pick, Spans, table_limit};
use crate::hls::nal::{MAX_UNITS, Units};
use crate::m4a::unsupported;
use crate::moov::{UNIT_RATE, overflow};
use crate::rational::{add, int, round, scale, sub, to_i64};
use crate::{ConcatFile, ExtractError};
use coreav_isobmff::{
    Codec, Error, FourCc, LimitKind, Limits, Movie, Packet, Packets, Profile, Rational, Source,
    Track, TrackKind,
};
use std::io::Write;

const FRAME: i64 = 1024;
const RATE: u32 = 48_000;
const REASON: &str = "a concat input with a positive AAC edit offset";

pub(super) enum Sample {
    Idr(u32),
    Inter,
    Unsupported,
}

pub(super) struct Video {
    qualified: bool,
    pub(super) position: Option<u32>,
}

impl Video {
    pub(super) fn new(qualified: bool) -> Self {
        Self {
            qualified,
            position: None,
        }
    }

    pub(super) fn observe(&mut self, count: u64, sample: Sample, sync: bool) {
        match (count, sample) {
            (1, Sample::Idr(position)) if sync => self.position = Some(position),
            (_, Sample::Inter) if count > 1 && !sync => {}
            _ => self.qualified = false,
        }
    }
}

pub(super) struct InBand<'a> {
    sps: &'a [u8],
    pps: &'a [u8],
    units: Units,
    extra: u32,
}

impl<'a> InBand<'a> {
    pub(super) fn of(track: &'a Track) -> Result<Option<Self>, Error> {
        let Some(data) = Self::configuration(track) else {
            return Ok(None);
        };
        let sets = coreav_isobmff::avc_parameter_sets_with_planning(data, None)
            .map_err(|_| table_limit())?;
        Ok(sets.and_then(|sets| Self::with_sets(track, data, &sets)))
    }

    fn configuration(track: &Track) -> Option<&[u8]> {
        if track.description.fourcc != FourCc(*b"avc1") {
            return None;
        }
        let config = track.description.codec_config.as_ref()?;
        if config.kind != FourCc(*b"avcC")
            || config.data.get(4)? & 3 != 3
            || config.data.get(5)? & 31 != 1
        {
            return None;
        }
        let length = config
            .data
            .get(6..8)?
            .iter()
            .fold(0usize, |n, b| (n << 8) | usize::from(*b));
        let count = 8usize.checked_add(length)?;
        (config.data.get(count)? == &1).then_some(config.data.as_slice())
    }

    fn with_sets(track: &'a Track, data: &'a [u8], sets: &[&'a [u8]]) -> Option<Self> {
        let [sps, pps] = sets else {
            return None;
        };
        if sps.first()? & 31 != 7 || pps.first()? & 31 != 8 {
            return None;
        }
        let base = 11usize.checked_add(sps.len())?.checked_add(pps.len())?;
        match data.get(base..)? {
            [] => {}
            [chroma, luma, depth, 0]
                if chroma & 0xfc == 0xfc && luma & 0xf8 == 0xf8 && depth & 0xf8 == 0xf8 => {}
            _ => return None,
        }
        Some(Self {
            sps,
            pps,
            units: Units::of(track)?,
            extra: u32::try_from(sps.len().checked_add(pps.len())?.checked_add(8)?).ok()?,
        })
    }

    pub(super) fn grown(&self, size: u32, limits: &Limits) -> Result<u32, Error> {
        let size = u64::from(size)
            .checked_add(u64::from(self.extra))
            .ok_or_else(overflow)?;
        if size > limits.max_packet_bytes {
            return Err(Error::LimitExceeded {
                limit: LimitKind::PacketBytes,
                offset: 0,
            });
        }
        u32::try_from(size).map_err(|_| Error::LimitExceeded {
            limit: LimitKind::PacketBytes,
            offset: 0,
        })
    }

    pub(super) fn sample<S: Source>(
        &self,
        packets: &mut Packets<'_, S>,
        packet: &Packet,
    ) -> Result<Sample, Error> {
        packets.charge(u64::from(MAX_UNITS))?;
        let end = packet
            .offset
            .checked_add(u64::from(packet.size))
            .ok_or_else(overflow)?;
        let mut at = packet.offset;
        let mut picture = None;
        for _ in 0..MAX_UNITS {
            if at == end {
                break;
            }
            let Some((next, kind)) = self.units.avc_header(packets.source_mut(), at, end)? else {
                return Ok(Sample::Unsupported);
            };
            match kind {
                6 if picture.is_none() => {}
                1 | 5 => {
                    if picture.is_some_and(|(_, previous)| previous != kind) {
                        return Ok(Sample::Unsupported);
                    }
                    if picture.is_none() {
                        picture = Some((at, kind));
                    }
                }
                _ => return Ok(Sample::Unsupported),
            }
            at = next;
        }
        if at != end {
            return Ok(Sample::Unsupported);
        }
        Ok(match picture {
            Some((at, 5)) => Sample::Idr(
                u32::try_from(at.checked_sub(packet.offset).ok_or_else(overflow)?)
                    .map_err(|_| overflow())?,
            ),
            Some((_, 1)) => Sample::Inter,
            _ => Sample::Unsupported,
        })
    }

    pub(super) fn check<S: Source>(
        &self,
        packets: &mut Packets<'_, S>,
        packet: &Packet,
        first: bool,
        position: u32,
    ) -> Result<(), ExtractError> {
        match (first, self.sample(packets, packet)?) {
            (true, Sample::Idr(current)) if current == position => Ok(()),
            (false, Sample::Inter) => Ok(()),
            _ => Err(ExtractError::SourceChanged),
        }
    }

    pub(super) fn write<W: Write>(&self, out: &mut W) -> Result<(), std::io::ErrorKind> {
        for unit in [self.sps, self.pps] {
            let size = u32::try_from(unit.len()).map_err(|_| std::io::ErrorKind::InvalidData)?;
            out.write_all(&size.to_be_bytes()).map_err(|e| e.kind())?;
            out.write_all(unit).map_err(|e| e.kind())?;
        }
        Ok(())
    }
}

pub(super) struct Samples {
    qualified: bool,
    last: u32,
}

impl Samples {
    pub(super) fn new() -> Self {
        Self {
            qualified: true,
            last: 0,
        }
    }

    pub(super) fn observe(&mut self, count: u64, duration: u32) {
        if count > 1 {
            self.qualified &= self.last == FRAME as u32;
        }
        self.qualified &= duration > 0 && duration <= FRAME as u32;
        self.last = duration;
    }
}

pub(super) struct Run {
    candidate: i64,
    floor: i64,
    samples: u64,
    last_delta: u32,
    source_tail: u32,
}

impl Run {
    pub(super) fn check(&self, count: u64, duration: u32) -> Result<(), ExtractError> {
        let expected = if count == self.samples {
            self.source_tail
        } else {
            FRAME as u32
        };
        if count > self.samples || duration != expected {
            return Err(ExtractError::SourceChanged);
        }
        Ok(())
    }

    fn dts(&self, index: u64) -> Result<i64, Error> {
        let index = i64::try_from(index).map_err(|_| overflow())?;
        let candidate = index
            .checked_mul(FRAME)
            .and_then(|n| self.candidate.checked_add(n))
            .ok_or_else(overflow)?;
        let floor = self.floor.checked_add(index).ok_or_else(overflow)?;
        Ok(candidate.max(floor))
    }

    pub(super) fn delta(&self, index: u64) -> Result<u32, Error> {
        if index.checked_add(1).ok_or_else(overflow)? == self.samples {
            return Ok(self.last_delta);
        }
        let duration = self
            .dts(index.checked_add(1).ok_or_else(overflow)?)?
            .checked_sub(self.dts(index)?)
            .ok_or_else(overflow)?;
        u32::try_from(duration).map_err(|_| unsupported("a concat packet delta above 32 bits"))
    }
}

pub(super) struct Clock<'a> {
    pub(super) offsets: Vec<Rational>,
    pub(super) audio: Vec<Run>,
    pub(super) charged: u64,
    pub(super) in_band: InBand<'a>,
}

pub(super) fn storage(count: usize, limits: &Limits) -> Result<u64, Error> {
    let bytes = std::mem::size_of::<ConcatFile>()
        + std::mem::size_of::<Rational>()
        + std::mem::size_of::<Run>()
        + std::mem::size_of::<Option<Samples>>()
        + std::mem::size_of::<Option<Video>>();
    u64::try_from(count)
        .ok()
        .and_then(|n| n.checked_mul(bytes as u64))
        .and_then(|n| n.checked_add((4 * std::mem::size_of::<&[u8]>()) as u64))
        .filter(|n| *n <= limits.max_table_bytes)
        .ok_or_else(table_limit)
}

pub(super) fn positive_aac(movie: &Movie) -> bool {
    movie.tracks.iter().any(|track| {
        std::iter::once(&track.description)
            .chain(&track.later_descriptions)
            .any(|description| description.codec == Codec::Aac)
            && track.edits.iter().any(|edit| edit.media_time > 0)
    })
}

fn aac(track: &Track) -> bool {
    let description = &track.description;
    let config = description
        .codec_config
        .as_ref()
        .and_then(|c| coreav_isobmff::esds_decoder_specific_info(&c.data))
        .and_then(|(_, data)| data);
    description.codec == Codec::Aac
        && description.sample_rate == Some(RATE)
        && description.configured_sample_rate == Some(RATE)
        && matches!(description.channels, Some(1 | 2))
        && description.aac_frame_length == Some(FRAME as u32)
        && matches!(
            description.profile,
            Some(Profile::Aac {
                object_type: 2,
                sbr: false,
                ps: false
            })
        )
        && config.is_some_and(coreav_isobmff::aac_has_explicit_no_sbr)
        && track.timescale == RATE
        && track.descriptions == 1
        && track.sample_count > 1
        && matches!(track.edits.as_slice(), [edit] if edit.media_time == FRAME && edit.media_rate == UNIT_RATE)
}

fn ticks(value: Rational, timescale: u32) -> Result<i64, Error> {
    let ticks = scale(value, timescale, 1)?;
    let remainder = ticks.num.unsigned_abs() % u128::from(ticks.den);
    if remainder.checked_mul(2).ok_or_else(overflow)? == u128::from(ticks.den) {
        return Err(unsupported("a concat offset on an unqualified half tick"));
    }
    to_i64(round(ticks))
}

pub(super) fn prepare<'a>(
    files: &[ConcatFile],
    movies: &[Movie],
    picks: &[Pick<'_>],
    spans: &[Spans<'_>],
    charged: u64,
    in_band: Option<InBand<'a>>,
) -> Result<Clock<'a>, Error> {
    if files.len() != movies.len() || files.first().is_none_or(|file| file.slot.num != 0) {
        return Err(unsupported(
            "a concat placement that does not name every input",
        ));
    }
    let mut offsets = Vec::new();
    let mut audio: Vec<Run> = Vec::new();
    offsets
        .try_reserve_exact(files.len())
        .map_err(|_| table_limit())?;
    audio
        .try_reserve_exact(files.len())
        .map_err(|_| table_limit())?;
    let origin = movies
        .first()
        .and_then(|movie| {
            movie
                .tracks
                .iter()
                .find(|track| track.kind == TrackKind::Audio)
        })
        .ok_or_else(|| unsupported(REASON))?;
    let media = origin
        .edits
        .first()
        .ok_or_else(|| unsupported(REASON))?
        .media_time;
    let exact_shift =
        Rational::new(i128::from(media), u64::from(origin.timescale)).ok_or_else(overflow)?;
    let shift =
        Rational::new(round(scale(exact_shift, 1_000_000, 1)?), 1_000_000).ok_or_else(overflow)?;
    let mut previous = None;
    for (((file, movie), pick), span) in files.iter().zip(movies).zip(picks).zip(spans) {
        let source_audio = movie
            .tracks
            .iter()
            .find(|track| track.kind == TrackKind::Audio)
            .ok_or_else(|| unsupported(REASON))?;
        let v = &span.video;
        let video_edit = pick.video.edits.first();
        if movie.fragmented
            || movie.tracks.len() != 2
            || !movie.dropped.is_empty()
            || file.start.num != 0
            || !aac(source_audio)
            || source_audio.index != 1
            || pick.video.index != 0
            || pick.video.description.codec != Codec::H264
            || pick.video.edits.len() != 1
            || video_edit
                .is_none_or(|edit| edit.media_rate != UNIT_RATE || edit.media_time != v.comp.0)
            || v.start.num != 0
            || v.concat
                .as_ref()
                .is_none_or(|v| !v.qualified || v.position.is_none())
            || v.end != Rational::from_int(i128::from(v.listed.duration))
        {
            return Err(unsupported(REASON));
        }
        let offset = add(sub(file.slot, file.start)?, shift)?;
        let exact = add(sub(file.slot, file.start)?, exact_shift)?;
        for timescale in
            std::iter::once(pick.video.timescale).chain(span.audio.as_ref().map(|_| RATE))
        {
            if ticks(offset, timescale)? != ticks(exact, timescale)? {
                return Err(unsupported(
                    "a concat offset with an unqualified priming rounding",
                ));
            }
        }
        offsets.push(offset);
        if let Some(a) = &span.audio {
            let listed = &a.listed;
            let sample_clock = a.concat.as_ref().ok_or_else(|| unsupported(REASON))?;
            let edit = source_audio
                .edits
                .first()
                .ok_or_else(|| unsupported(REASON))?;
            let endpoint = add(
                scale(
                    Rational::new(
                        i128::from(edit.segment_duration),
                        u64::from(movie.timescale),
                    )
                    .ok_or_else(overflow)?,
                    RATE,
                    1,
                )?,
                int(FRAME),
            )?;
            if !sample_clock.qualified
                || listed.first_dts != 0
                || listed.samples != u64::from(source_audio.sample_count)
                || endpoint != Rational::from_int(i128::from(listed.duration))
                || source_audio.mdhd_duration != listed.duration
            {
                return Err(unsupported(REASON));
            }
            let candidate = ticks(offset, RATE)?
                .checked_sub(FRAME)
                .ok_or_else(overflow)?;
            let floor = previous.map_or(Ok(candidate), |last: i64| {
                last.checked_add(1).ok_or_else(overflow)
            })?;
            let run = Run {
                candidate,
                floor,
                samples: listed.samples,
                last_delta: sample_clock.last,
                source_tail: sample_clock.last,
            };
            let last_index = listed.samples.checked_sub(1).ok_or_else(overflow)?;
            let candidate_last = candidate
                .checked_add(
                    i64::try_from(last_index)
                        .map_err(|_| overflow())?
                        .checked_mul(FRAME)
                        .ok_or_else(overflow)?,
                )
                .ok_or_else(overflow)?;
            if candidate_last
                < floor
                    .checked_add(i64::try_from(last_index).map_err(|_| overflow())?)
                    .ok_or_else(overflow)?
            {
                return Err(unsupported("a concat overlap consuming a whole input"));
            }
            if let Some(last) = audio.last_mut() {
                last.last_delta = u32::try_from(
                    run.dts(0)?
                        .checked_sub(previous.ok_or_else(overflow)?)
                        .ok_or_else(overflow)?,
                )
                .map_err(|_| unsupported("a concat packet delta above 32 bits"))?;
            }
            previous = Some(run.dts(last_index)?);
            audio.push(run);
        }
    }
    Ok(Clock {
        offsets,
        audio,
        charged,
        in_band: in_band.ok_or_else(|| unsupported("unqualified concat AVC configuration"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_deltas_keep_abutment_gaps_and_cascading_clamps() {
        for (candidate, floor, tail, dts, deltas) in [
            (0, 0, 1024, [0, 1024, 2048, 3072], [1024, 1024, 1024, 1024]),
            (
                3000,
                4001,
                100,
                [4001, 4024, 5048, 6072],
                [23, 1024, 1024, 100],
            ),
            (3000, 6001, 3000, [6001, 6002, 6003, 6072], [1, 1, 69, 3000]),
            (-1, 0, 5, [0, 1023, 2047, 3071], [1023, 1024, 1024, 5]),
        ] {
            let run = Run {
                candidate,
                floor,
                samples: 4,
                last_delta: tail,
                source_tail: 5,
            };
            for (index, (dts, delta)) in dts.into_iter().zip(deltas).enumerate() {
                assert_eq!(run.dts(index as u64).unwrap(), dts);
                assert_eq!(run.delta(index as u64).unwrap(), delta);
            }
            assert!(run.check(1, 1024).is_ok());
            assert!(run.check(4, 5).is_ok());
            assert!(matches!(
                run.check(4, 1024),
                Err(ExtractError::SourceChanged)
            ));
            assert!(matches!(
                run.check(5, 1024),
                Err(ExtractError::SourceChanged)
            ));
        }
    }

    #[test]
    fn clock_arithmetic_and_retained_storage_fail_closed() {
        let run = Run {
            candidate: i64::MAX,
            floor: 0,
            samples: 2,
            last_delta: 1,
            source_tail: 1,
        };
        assert!(run.dts(1).is_err());
        assert!(run.dts(u64::MAX).is_err());
        assert!(ticks(Rational::new(1, 2).unwrap(), 1).is_err());
        assert!(ticks(Rational::new(-1, 2).unwrap(), 1).is_err());
        let limits = Limits {
            max_table_bytes: 1,
            ..Limits::default()
        };
        assert!(matches!(
            storage(2, &limits),
            Err(Error::LimitExceeded {
                limit: LimitKind::TableBytes,
                ..
            })
        ));
        assert!(storage(usize::MAX, &Limits::default()).is_err());
    }
}
