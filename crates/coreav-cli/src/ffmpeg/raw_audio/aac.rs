use super::{DecodeError, Decoded, Pass, Plan};
use crate::ffprobe::Packet;
use crate::ffprobe::model::Value;
use coreav_aac::{Config, Decoder};
use coreav_audio::raw::RawF32Stream;
use coreav_audio::{Error, Limits, Sink};
use coreav_isobmff::{self as mp4, FileSource, Source};
use std::path::Path;

const MAX_PACKETS: u64 = 16_384;
const MAX_BYTES: u64 = 64 << 20;
const MAX_PACKET_BYTES: u64 = 16 << 20;

fn refused() -> Error {
    Error::Unsupported(
        "AAC decode tolerance requires clean mono/stereo LC packets, separable PNS tools and a registered MP4 timeline",
    )
}

struct Content {
    pns: bool,
    separable: bool,
    sequences: [u8; 2],
}

impl Content {
    fn new(config: &Config, pns: bool) -> Self {
        Self {
            pns,
            separable: config.frame_length() == 1024
                && matches!(config.channel_configuration(), 1 | 2),
            sequences: [0; 2],
        }
    }

    fn packet(
        &mut self,
        decoder: &Decoder,
        decoded: coreav_aac::Frame,
        size: usize,
    ) -> Result<(), Error> {
        self.pns |= decoded.noise_bands > 0;
        self.separable &= decoded.pns_tns_bands == 0
            && decoded.pns_stereo_bands == 0
            && decoded.coupling_channels == 0
            && size <= 8191 - 7;
        for (previous, next) in self.sequences.iter_mut().zip(decoder.window_sequences()) {
            self.separable &= matches!((*previous, next), (0 | 3, 0 | 1) | (1, 2) | (2, 2 | 3));
            *previous = next;
        }
        if self.pns && !self.separable {
            return Err(refused());
        }
        Ok(())
    }
}

fn clean(
    decoder: &mut Decoder,
    packet: &[u8],
    output: &mut [f32],
    frame: usize,
) -> Result<coreav_aac::Frame, Error> {
    let decoded = decoder.decode(packet, output).map_err(|_| refused())?;
    if decoded.samples != frame
        || decoded.intensity_all_ms_bands != 0
        || output.iter().any(|sample| !sample.is_finite())
    {
        return Err(refused());
    }
    Ok(decoded)
}

fn gapless(
    movie: &mp4::Movie,
    source: &mut FileSource,
    limits: &mp4::Limits,
) -> Result<Option<mp4::ItunesGapless>, Error> {
    use mp4::metadata::{ItemValue, ListKey, ListPlace};
    let metadata = mp4::metadata::read(&mut *source, limits)?;
    if !metadata.complete || metadata.compressed {
        return Err(refused());
    }
    let parsed = movie.itunes_gapless(&mut *source)?;
    let mut found = false;
    for item in &metadata.items {
        let ItemValue::ListItem {
            list,
            key: ListKey::Free { mean, name },
            data,
        } = &item.value
        else {
            continue;
        };
        if name.as_deref() != Some(b"iTunSMPB") {
            continue;
        }
        if found
            || item.track.is_some()
            || mean.as_deref() != Some(b"com.apple.iTunes")
            || *list
                != (ListPlace::Meta {
                    handler: Some(mp4::FourCc(*b"mdir")),
                })
            || data.len() != 1
        {
            return Err(refused());
        }
        let data = data.first().ok_or_else(refused)?;
        if data.type_indicator != 1 || data.locale != 0 {
            return Err(refused());
        }
        let text = std::str::from_utf8(&data.value).map_err(|_| refused())?;
        let mut fields = text.split_ascii_whitespace();
        let mut values = [0u64; 12];
        for value in &mut values {
            *value = u64::from_str_radix(fields.next().ok_or_else(refused)?, 16)
                .map_err(|_| refused())?;
        }
        let value = parsed.ok_or_else(refused)?;
        if fields.next().is_some()
            || values.first() != Some(&0)
            || values.get(1) != Some(&u64::from(value.encoder_delay))
            || values.get(2) != Some(&u64::from(value.padding))
            || values.get(3) != Some(&value.original_samples)
        {
            return Err(refused());
        }
        found = true;
    }
    if parsed.is_some() != found {
        return Err(refused());
    }
    Ok(parsed)
}

struct Prepared {
    config: Config,
    end: Option<i64>,
    complete: Option<(u64, u64)>,
}

fn setup(command: &super::Command, plan: &Plan, limits: &mp4::Limits) -> Result<Prepared, Error> {
    let mut source = FileSource::open(Path::new(&command.input))?;
    let movie = mp4::probe(
        &mut source,
        limits,
        &mp4::Allowlist::new(&[mp4::Codec::Aac]),
    )?;
    let track = movie
        .tracks
        .iter()
        .find(|track| Some(track.stream_index) == plan.options.stream)
        .ok_or_else(refused)?;
    if movie.has_fragments()
        || track.timescale != plan.input_rate
        || track.sample_count == 0
        || u64::from(track.sample_count) > MAX_PACKETS
        || track.descriptions != 1
        || track.ctts_version().is_some()
    {
        return Err(refused());
    }
    let config = track
        .description
        .codec_config
        .as_ref()
        .filter(|config| config.kind.0 == *b"esds")
        .and_then(|config| mp4::esds_decoder_specific_info(&config.data))
        .and_then(|(_, config)| config)
        .ok_or_else(refused)?;
    let config =
        Config::parse(config, &coreav_aac::Limits { max_channels: 2 }).map_err(|_| refused())?;
    let gapless = gapless(&movie, &mut source, limits)?;
    let media = track.sample_duration;
    let end = match track.edit_list {
        mp4::EditList::None => gapless
            .filter(|g| g.original_samples > 0)
            .and_then(|g| u64::from(g.encoder_delay).checked_add(g.original_samples))
            .map(|end| end.min(media)),
        mp4::EditList::Simple {
            media_time,
            segment,
            ..
        } if media_time >= 0 && segment > 0 && movie.timescale > 0 => None,
        _ => return Err(refused()),
    };
    let end = end.map(i64::try_from).transpose().map_err(|_| refused())?;
    let mut budget = mp4::Budget::new(limits);
    let mut packets = movie.packets(track.index, &mut source, limits, &mut budget)?;
    let mut decoder = Decoder::new(&config);
    let mut data = Vec::new();
    let mut output = [0f32; 2048];
    let mut bytes = 0u64;
    let mut count = 0u64;
    let mut first = None;
    let mut content = Content::new(&config, false);
    while let Some(packet) = packets.next() {
        let packet = packet?;
        first.get_or_insert(packet.offset);
        count = count.checked_add(1).ok_or_else(refused)?;
        bytes = bytes
            .checked_add(u64::from(packet.size))
            .ok_or_else(refused)?;
        if count > MAX_PACKETS || bytes > MAX_BYTES || u64::from(packet.size) > MAX_PACKET_BYTES {
            return Err(refused());
        }
        let size = usize::try_from(packet.size).map_err(|_| refused())?;
        data.try_reserve(size.saturating_sub(data.len()))
            .map_err(|_| refused())?;
        data.resize(size, 0);
        packets
            .source_mut()
            .read_exact_at(packet.offset, &mut data)?;
        let decoded = clean(
            &mut decoder,
            &data,
            output
                .get_mut(..config.frame_length() * config.channels())
                .ok_or_else(refused)?,
            config.frame_length(),
        )?;
        content.packet(&decoder, decoded, data.len())?;
    }
    if count != u64::from(track.sample_count) {
        return Err(refused());
    }
    let complete = if content.pns {
        Some((count, first.ok_or_else(refused)?))
    } else {
        None
    };
    Ok(Prepared {
        config,
        end,
        complete,
    })
}

struct State<'a> {
    decoder: Decoder,
    output: RawF32Stream<'a>,
    channels: usize,
    frame: usize,
    end: i64,
    skip: u64,
    packets: u64,
    bytes: u64,
    content: Content,
    complete: Option<(u64, u64)>,
}

impl State<'_> {
    fn packet(&mut self, packet: Packet, last: bool) -> Result<(), Error> {
        if self.packets == 0
            && self
                .complete
                .is_some_and(|(_, first)| packet.pos != Some(first))
        {
            return Err(refused());
        }
        self.packets = self.packets.checked_add(1).ok_or_else(refused)?;
        self.bytes = self.bytes.checked_add(packet.size).ok_or_else(refused)?;
        if self.packets > MAX_PACKETS
            || self.bytes > MAX_BYTES
            || packet.size > MAX_PACKET_BYTES
            || packet.data.len() as u64 != packet.size
            || packet.corrupt
            || packet.pts.is_none()
            || packet.pts != packet.dts
            || packet.duration.is_none_or(|duration| duration <= 0)
        {
            return Err(refused());
        }
        for data in &packet.side_data {
            if data.typ != "skip_samples" {
                return Err(refused());
            }
            for (key, value) in &data.entries {
                if *key == "skip_samples" {
                    let Value::Int(skip) = value else {
                        return Err(refused());
                    };
                    if *skip < 0 || *skip > 0 && self.packets != 1 {
                        return Err(refused());
                    }
                    if self.packets == 1 {
                        self.skip = u64::try_from(*skip).map_err(|_| refused())?;
                    }
                }
            }
        }
        let mut samples = [0f32; 2048];
        let decoded = clean(
            &mut self.decoder,
            &packet.data,
            samples
                .get_mut(..self.frame * self.channels)
                .ok_or_else(refused)?,
            self.frame,
        )?;
        self.content
            .packet(&self.decoder, decoded, packet.data.len())?;
        let skip = self.skip.min(self.frame as u64) as usize;
        self.skip -= skip as u64;
        if packet.discard && skip != self.frame {
            return Err(refused());
        }
        let padding = if last {
            let pts = packet.pts.ok_or_else(refused)?;
            if pts < self.end {
                let covered = packet.duration.ok_or_else(refused)?.max(self.frame as i64);
                covered.saturating_sub(self.end.saturating_sub(pts)).max(0) as u64
            } else {
                0
            }
        } else {
            0
        };
        let kept =
            (self.frame - skip).saturating_sub(usize::try_from(padding).unwrap_or(usize::MAX));
        let mut packed = [0u8; 8192];
        let mut at = 0;
        for index in skip..skip + kept {
            for channel in 0..self.channels {
                let sample = samples
                    .get(channel * self.frame + index)
                    .ok_or_else(refused)?;
                packed
                    .get_mut(at..at + 4)
                    .ok_or_else(refused)?
                    .copy_from_slice(&sample.to_le_bytes());
                at += 4;
            }
        }
        self.output.write(packed.get(..at).ok_or_else(refused)?)
    }
}

pub(super) fn decode(
    command: &super::Command,
    plan: &Plan,
    sink: &mut dyn Sink,
) -> Result<Pass, DecodeError> {
    if plan.input_rate != plan.output_rate || plan.channels.input != plan.channels.output {
        return Err(DecodeError::Audio(refused()));
    }
    let limits = mp4::Limits {
        max_packet_bytes: MAX_PACKET_BYTES,
        ..mp4::Limits::default()
    };
    let Prepared {
        config,
        end,
        complete,
    } = setup(command, plan, &limits).map_err(DecodeError::Audio)?;
    let mut source = FileSource::open(Path::new(&command.input))
        .map_err(Error::from)
        .map_err(DecodeError::Audio)?;
    let size = source
        .len()
        .map_err(Error::from)
        .map_err(DecodeError::Audio)?;
    let mut probed =
        crate::ffprobe::probe_limited(&mut source, size, &command.input, None, &limits)
            .map_err(DecodeError::Wave)?;
    let index = plan
        .options
        .stream
        .ok_or_else(|| DecodeError::Audio(refused()))?;
    let end = end
        .or_else(|| {
            probed
                .report
                .streams
                .get(index)?
                .entries
                .iter()
                .find_map(|(key, value)| match (*key, value) {
                    ("duration_ts", Value::Int(duration)) if *duration > 0 => Some(*duration),
                    _ => None,
                })
        })
        .ok_or_else(|| DecodeError::Audio(refused()))?;
    if !matches!(config.channel_configuration(), 1 | 2)
        || config.channels() != usize::from(plan.channels.input)
        || config.sample_rate() != plan.input_rate
    {
        return Err(DecodeError::Audio(refused()));
    }
    let output = RawF32Stream::packing(
        plan.input_rate,
        plan.channels.input,
        plan.output_format,
        &Limits::default(),
        sink,
    )
    .map_err(DecodeError::Audio)?;
    let mut state = State {
        decoder: Decoder::new(&config),
        output,
        channels: config.channels(),
        frame: config.frame_length(),
        end,
        skip: 0,
        packets: 0,
        bytes: 0,
        content: Content::new(&config, complete.is_some()),
        complete,
    };
    let mut payloads = vec![false; probed.report.streams.len()];
    *payloads
        .get_mut(index)
        .ok_or_else(|| DecodeError::Audio(refused()))? = true;
    let mut pending = None;
    let mut failure = None;
    probed
        .packets
        .visit(&payloads, &mut |packet| {
            if packet.stream == index {
                if let Some(previous) = pending.take() {
                    if let Err(error) = state.packet(previous, false) {
                        failure = Some(error);
                        return Ok(false);
                    }
                }
                pending = Some(packet);
            }
            Ok(true)
        })
        .map_err(DecodeError::Wave)?;
    if let Some(error) = failure {
        return Err(DecodeError::Audio(error));
    }
    if let Some(packet) = pending {
        state.packet(packet, true).map_err(DecodeError::Audio)?;
    }
    if state.packets == 0
        || state.skip != 0
        || complete.is_some_and(|(count, _)| state.packets != count)
    {
        return Err(DecodeError::Audio(refused()));
    }
    let decoded = Decoded::Aac {
        packets: state.packets,
        bytes: state.bytes,
    };
    Ok(Pass {
        decoded,
        converted: Some(state.output.finish().map_err(DecodeError::Audio)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(noise: bool) -> coreav_aac::Frame {
        coreav_aac::Frame {
            samples: 1024,
            noise_bands: u32::from(noise),
            intensity_all_ms_bands: 0,
            pns_tns_bands: 0,
            pns_stereo_bands: 0,
            coupling_channels: 0,
        }
    }

    #[test]
    fn pns_rejects_tools_even_when_the_conflict_precedes_the_first_noise_band() {
        let Ok(config) = Config::parse(&[0x12, 0x08], &coreav_aac::Limits { max_channels: 2 })
        else {
            unreachable!();
        };
        let decoder = Decoder::new(&config);
        let mut good = Content::new(&config, false);
        assert!(good.packet(&decoder, frame(true), 100).is_ok());
        for conflict in [0, 1, 2] {
            let mut content = Content::new(&config, false);
            let mut decoded = frame(false);
            match conflict {
                0 => decoded.pns_tns_bands = 1,
                1 => decoded.pns_stereo_bands = 1,
                _ => decoded.coupling_channels = 1,
            }
            assert!(content.packet(&decoder, decoded, 100).is_ok());
            assert!(content.packet(&decoder, frame(true), 100).is_err());
            let mut content = Content::new(&config, true);
            assert!(content.packet(&decoder, decoded, 100).is_err());
        }
    }

    #[test]
    fn pns_requires_a_legal_window_history_and_adts_transport_extent() {
        let Ok(config) = Config::parse(&[0x12, 0x08], &coreav_aac::Limits { max_channels: 2 })
        else {
            unreachable!();
        };
        let mut decoder = Decoder::new(&config);
        let mut content = Content::new(&config, false);
        assert!(content.packet(&decoder, frame(false), 8191).is_ok());
        assert!(content.packet(&decoder, frame(true), 100).is_err());
        let mut content = Content::new(&config, true);
        assert!(content.packet(&decoder, frame(true), 8184).is_ok());
        assert!(content.packet(&decoder, frame(true), 8185).is_err());
        let mut samples = [0.0; 1024];
        assert!(
            decoder
                .decode(&[0x00, 0xc8, 0xc0, 0x07], &mut samples)
                .is_ok()
        );
        let mut content = Content::new(&config, false);
        assert!(content.packet(&decoder, frame(false), 100).is_ok());
        assert!(content.packet(&decoder, frame(true), 100).is_err());
    }
}
