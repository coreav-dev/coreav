use std::io;
use std::path::Path;

use coreav_avi::{Packet, Track};
use coreav_isobmff::{Allowlist, Budget, BufferedSource, FileSource, Limits, Rational, Source};

#[cfg(not(feature = "transcode"))]
use super::args::Muxer;
use super::args::{AudioEncoder, AviAudio, Codec, Command, Encoding, MapMetadata};
use super::{Job, Refusal, Stop};
use crate::error::Failure;
use crate::ffprobe::{Report, StreamKind};
use crate::output::{self, WriteFailure};
#[cfg(not(feature = "transcode"))]
use crate::strict_safe::StrictSafe;

const AUDIO_BYTES: usize = 64 << 20;
const MAX_PACKETS: usize = 1_000_000;

#[cfg(not(feature = "transcode"))]
pub(super) fn strict(command: &Command, strict: StrictSafe) -> Result<(), Refusal> {
    if command.muxer == Muxer::Avi {
        let explicit = command
            .codecs
            .iter()
            .any(|(_, c)| *c == Codec::Audio(AudioEncoder::Libmp3lame));
        if explicit {
            strict
                .mp3()
                .map_err(|_| Refusal::not_native(
                    "MP3 encoding in strict-safe mode (its encoder is the ffmpeg program); use -c:a copy or PCM",
                ))?;
        }
    }
    Ok(())
}

pub(super) fn job(
    command: &Command,
    report: &Report,
    selected: &[usize],
    concat: bool,
) -> Result<Job, Refusal> {
    if selected.len() > coreav_avi::Limits::default().max_tracks {
        return Err(Refusal::not_native("more than sixteen AVI output streams"));
    }
    if super::format_name(report) != Some("mov,mp4,m4a,3gp,3g2,mj2") {
        return Err(Refusal::not_native(
            "AVI output from an input other than MP4 or MOV",
        ));
    }
    if concat
        || command.input_seek.is_some()
        || command.input_duration.is_some()
        || command.input_until.is_some()
        || command.seek.is_some()
        || command.duration.is_some()
        || command.until.is_some()
    {
        return Err(Refusal::not_native("trimming or concatenating AVI output"));
    }
    if command.map_metadata != MapMetadata::Default {
        return Err(Refusal::not_native(
            "explicit metadata mapping in AVI output",
        ));
    }
    if (Encoding {
        audio_bitrate: None,
        ..command.encoding.clone()
    }) != Encoding::default()
    {
        return Err(Refusal::not_native(
            "video encoder or timing options in AVI output",
        ));
    }
    let output: Vec<_> = selected
        .iter()
        .filter_map(|i| report.streams.get(*i).map(|s| (*i, s)))
        .collect();
    let codecs = super::codecs(command, &output);
    let mut jobs = Vec::new();
    for ((i, stream), codec) in output.iter().zip(codecs) {
        if report.covers.iter().any(|(cover, _)| cover == i) {
            return Err(Refusal::not_native("cover pictures in AVI output"));
        }
        let codec = match (stream.kind, codec) {
            (StreamKind::Video, Some(Codec::Copy)) => Codec::Copy,
            (StreamKind::Video, None) => {
                return Err(Refusal::not_native(
                    "AVI's default MPEG-4 Part 2 video encoder",
                ));
            }
            (StreamKind::Audio, Some(c @ (Codec::Copy | Codec::AviAudio(_)))) => c,
            (StreamKind::Audio, Some(Codec::Audio(AudioEncoder::Libmp3lame))) => {
                Codec::AviAudio(AviAudio::Mp3)
            }
            (StreamKind::Audio, None) => Codec::AviAudio(AviAudio::Mp3),
            _ => return Err(Refusal::not_native("this AVI stream kind or encoder")),
        };
        jobs.push((*i, codec));
    }
    if command.encoding.audio_bitrate.is_some()
        && !jobs
            .iter()
            .any(|(_, c)| *c == Codec::AviAudio(AviAudio::Mp3))
    {
        return Err(Refusal::not_native(
            "an AVI audio bitrate without MP3 encoding",
        ));
    }
    if jobs
        .iter()
        .any(|(_, c)| *c == Codec::AviAudio(AviAudio::Mp3))
        && !cfg!(feature = "transcode")
    {
        return Err(Refusal::not_native(
            "MP3 encoding in a build without the transcode feature",
        ));
    }
    Ok(Job::Avi(jobs))
}

enum Location {
    Source(u64),
    Memory { buffer: usize, offset: usize },
}

struct Chunk {
    packet: Packet,
    location: Location,
}

fn refused(reason: &'static str) -> Stop {
    Refusal::not_native(reason).into()
}

fn format_error(error: coreav_avi::Error) -> Stop {
    match error {
        coreav_avi::Error::Unsupported(reason) | coreav_avi::Error::Limit(reason) => {
            refused(reason)
        }
        coreav_avi::Error::Io(error) => WriteFailure::output(&error).into(),
    }
}

fn push(chunks: &mut Vec<Chunk>, packet: Packet, location: Location) -> Result<(), Stop> {
    if chunks.len() >= MAX_PACKETS || packet.size > 16 << 20 || packet.time > u64::from(u32::MAX) {
        return Err(refused("AVI packet, timing or index budget exceeded"));
    }
    chunks.push(Chunk { packet, location });
    Ok(())
}

fn same_time(a: Rational, b: Rational) -> bool {
    match (
        a.num.checked_mul(i128::from(b.den)),
        b.num.checked_mul(i128::from(a.den)),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn source_track(
    source: &mut BufferedSource<FileSource>,
    movie: &coreav_isobmff::Movie,
    stream: usize,
    output: usize,
    chunks: &mut Vec<Chunk>,
    budget: &mut Budget,
) -> Result<Track, Stop> {
    let track = movie
        .tracks
        .iter()
        .find(|t| t.stream_index == stream)
        .ok_or_else(|| refused("AVI source stream is unavailable"))?;
    if track.descriptions != 1
        || movie.has_fragments()
        || track.matrix != coreav_isobmff::IDENTITY_MATRIX
        || track.timescale == 0
    {
        return Err(refused(
            "AVI source fragments, changing descriptions or rotation",
        ));
    }
    let description = &track.description;
    if !matches!(
        track.edit_list,
        coreav_isobmff::EditList::None
            | coreav_isobmff::EditList::Simple {
                empty: 0,
                media_time: 0,
                ..
            }
    ) || description
        .pixel_aspect
        .is_some_and(|(n, d)| n != d || d == 0)
    {
        return Err(refused("AVI source edits or nonsquare pixels"));
    }
    let mut clock = None;
    let mut units = 0u64;
    let mut compressed_bytes = 0u64;
    let mut mp3_format = None;
    let pcm = description.pcm.filter(|p| {
        !p.float
            && matches!(p.bits, 8 | 16)
            && ((p.bits == 8 && !p.signed) || (p.bits != 8 && p.signed && p.little_endian))
    });
    let is_h264 = description.codec == coreav_isobmff::Codec::H264;
    let is_mp3 = description.codec == coreav_isobmff::Codec::Mp3;
    if !is_h264 && !is_mp3 && pcm.is_none() {
        return Err(refused(
            "AVI stream copy supports H.264, integer PCM and MP3 only",
        ));
    }
    let rate = description.sample_rate.unwrap_or(0);
    let channels = description
        .channels
        .and_then(|n| u16::try_from(n).ok())
        .unwrap_or(0);
    let mut packets = movie
        .packets(track.index, &mut *source, &Limits::default(), budget)
        .map_err(WriteFailure::from)?;
    while let Some(packet) = packets.next() {
        let packet = packet.map_err(WriteFailure::from)?;
        let (duration, scale, denominator) = if is_h264 {
            if packet.cts != packet.dts || !packet.presented {
                return Err(refused("AVI reordered or non-presented video packets"));
            }
            let step = *clock.get_or_insert(packet.duration);
            if step == 0 || packet.duration != step {
                return Err(refused("AVI variable-rate video"));
            }
            (1, step, track.timescale)
        } else if is_mp3 {
            let mut bytes = [0u8; 4];
            if packet.size < 4 {
                return Err(refused("AVI MP3 packet is shorter than its header"));
            }
            packets
                .source_mut()
                .read_exact_at(packet.offset, &mut bytes)
                .map_err(WriteFailure::from)?;
            let header = coreav_audio::mp3::Header::parse(&bytes)
                .map_err(|_| refused("AVI MP3 packet has an invalid header"))?;
            let samples = header.samples() as u32;
            let format = (samples, header.sample_rate, header.channels() as u16);
            if header.frame_bytes() != Some(packet.size as usize)
                || mp3_format.is_some_and(|f| f != format)
            {
                return Err(refused(
                    "AVI MP3 needs complete frames with a fixed sample rate and channels",
                ));
            }
            mp3_format = Some(format);
            (1, samples, header.sample_rate)
        } else {
            let pcm = pcm.ok_or_else(|| refused("AVI PCM layout"))?;
            let align = u32::from(channels) * u32::from(pcm.bits / 8);
            if align == 0 || packet.size % align != 0 || rate == 0 {
                return Err(refused("AVI PCM packet alignment"));
            }
            (packet.size / align, 1, rate)
        };
        let expected = Rational::new(
            i128::from(units) * i128::from(scale),
            u64::from(denominator),
        )
        .ok_or_else(|| refused("AVI source clock"))?;
        let duration_time = Rational::new(
            i128::from(duration) * i128::from(scale),
            u64::from(denominator),
        )
        .ok_or_else(|| refused("AVI source duration"))?;
        let source_duration =
            Rational::new(i128::from(packet.duration), u64::from(track.timescale))
                .ok_or_else(|| refused("AVI source duration"))?;
        let seconds = |time: Option<Rational>| {
            time.and_then(|t| Rational::new(t.num, t.den.checked_mul(u64::from(track.timescale))?))
        };
        if !packet.presented
            || !seconds(packet.pts).is_some_and(|t| same_time(t, expected))
            || !seconds(packet.presentation_dts).is_some_and(|t| same_time(t, expected))
            || !same_time(duration_time, source_duration)
        {
            return Err(refused(
                "AVI source edits, reordering, nonzero starts or timestamp gaps",
            ));
        }
        push(
            chunks,
            Packet {
                track: output,
                time: units,
                duration,
                size: packet.size,
                keyframe: packet.keyframe,
            },
            Location::Source(packet.offset),
        )?;
        units += u64::from(duration);
        compressed_bytes += u64::from(packet.size);
    }
    let mut result = if is_h264 {
        let config = description
            .codec_config
            .as_ref()
            .filter(|c| c.kind.0 == *b"avcC")
            .ok_or_else(|| refused("AVI H.264 without an AVC configuration record"))?
            .data
            .clone();
        Track::H264 {
            width: description.width.unwrap_or(0),
            height: description.height.unwrap_or(0),
            scale: clock.unwrap_or(0),
            rate: track.timescale,
            config,
        }
    } else if let Some((frame_samples, sample_rate, channels)) = mp3_format {
        let bytes_per_second = u32::try_from(
            compressed_bytes * u64::from(sample_rate) / (units.max(1) * u64::from(frame_samples)),
        )
        .map_err(|_| refused("AVI MP3 byte rate"))?;
        Track::Mp3 {
            rate: sample_rate,
            channels,
            frame_samples,
            bytes_per_second,
        }
    } else {
        Track::Pcm {
            rate,
            channels,
            bits: pcm.ok_or_else(|| refused("AVI PCM layout"))?.bits,
        }
    };
    let (scale, rate) = match &result {
        Track::H264 { scale, rate, .. } => (*scale, *rate),
        Track::Pcm { rate, .. } => (1, *rate),
        Track::Mp3 {
            frame_samples,
            rate,
            ..
        } => (*frame_samples, *rate),
    };
    let end = Rational::new(i128::from(units) * i128::from(scale), u64::from(rate))
        .ok_or_else(|| refused("AVI source duration"))?;
    if !track
        .presentation(movie.timescale)
        .is_some_and(|(start, stop)| start.num == 0 && same_time(stop, end))
    {
        return Err(refused(
            "AVI source has a partial final packet or an unknown presentation window",
        ));
    }
    if let Track::H264 { scale, rate, .. } = &mut result {
        let ticks = u64::from(*scale) * 600;
        if ticks % u64::from(*rate) != 0 {
            return Err(refused(
                "AVI H.264 cadence does not fit the 600 Hz copy clock",
            ));
        }
        let slots = u32::try_from(ticks / u64::from(*rate))
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| refused("AVI H.264 clock slots are out of range"))?;
        for chunk in chunks.iter_mut().filter(|c| c.packet.track == output) {
            chunk.packet.time = chunk
                .packet
                .time
                .checked_mul(u64::from(slots))
                .ok_or_else(|| refused("AVI H.264 duration overflows"))?;
            if chunk.packet.time > u64::from(u32::MAX) {
                return Err(refused("AVI H.264 duration exceeds 32 bits"));
            }
            chunk.packet.duration = slots;
        }
        *scale = 1;
        *rate = 600;
    }
    Ok(result)
}

struct Samples {
    data: Vec<u8>,
    limit: usize,
}

impl coreav_audio::Sink for Samples {
    fn start(&mut self, format: &coreav_audio::OutputFormat) -> Result<(), coreav_audio::Error> {
        if !matches!(format.channels, 1 | 2) {
            return Err(coreav_audio::Error::Unsupported(
                "AVI encoding needs mono or stereo audio",
            ));
        }
        Ok(())
    }

    fn write(&mut self, samples: &[u8]) -> Result<(), coreav_audio::Error> {
        if self.data.len().saturating_add(samples.len()) > self.limit {
            return Err(coreav_audio::Error::Unsupported(
                "AVI decoded audio exceeds the 64 MiB buffer limit",
            ));
        }
        self.data.extend_from_slice(samples);
        Ok(())
    }
}

fn encoded_audio(
    command: &Command,
    stream: usize,
    output: usize,
    encoder: AviAudio,
    chunks: &mut Vec<Chunk>,
    buffers: &mut Vec<Vec<u8>>,
) -> Result<Track, Stop> {
    let options = coreav_audio::Options {
        stream: Some(stream),
        threads: command.input_threads.unwrap_or(1),
        format: if encoder == AviAudio::Mp3 {
            coreav_audio::SampleFormat::F32
        } else {
            coreav_audio::SampleFormat::S16
        },
        ..Default::default()
    };
    let source = BufferedSource::new(FileSource::open(&command.input).map_err(WriteFailure::from)?);
    let kept = buffers
        .iter()
        .try_fold(0usize, |sum, bytes| sum.checked_add(bytes.len()))
        .ok_or_else(|| refused("AVI audio buffers exceed their limit"))?;
    let mut samples = Samples {
        data: Vec::new(),
        limit: AUDIO_BYTES.saturating_sub(kept),
    };
    let limits = coreav_audio::Limits {
        max_decode_errors: 0,
        ..Default::default()
    };
    let summary = coreav_audio::decode(source, &options, &limits, &mut samples)
        .map_err(WriteFailure::from)?;
    if !summary.warnings.is_empty() || summary.samples == 0 {
        return Err(refused("AVI audio decoding warnings or an empty stream"));
    }
    let rate = summary.output.sample_rate;
    let channels = summary.output.channels;
    let buffer = buffers.len();
    if encoder == AviAudio::Mp3 {
        #[cfg(feature = "transcode")]
        {
            let bitrate = command
                .encoding
                .audio_bitrate
                .as_deref()
                .map(|text| {
                    let (number, factor) = if let Some(n) = text.strip_suffix(['k', 'K']) {
                        (n, 1000u32)
                    } else {
                        (text, 1)
                    };
                    number
                        .parse::<u32>()
                        .ok()
                        .and_then(|n| n.checked_mul(factor))
                        .filter(|b| (8000..=320000).contains(b))
                        .ok_or_else(|| {
                            refused("AVI MP3 bitrate must be 8k to 320k bits per second")
                        })
                })
                .transpose()?;
            let program = coreav_worker::find_ffmpeg()
                .ok_or_else(|| refused("AVI MP3 encoding needs the operator's ffmpeg program"))?;
            let launcher = coreav_worker::ExecLauncher::current_exe()
                .map_err(|e| WriteFailure::Rejected(Failure::io(e)))?;
            let settings = coreav_transcode::mp3::Mp3Audio {
                stream: Some(stream),
                mode: coreav_transcode::mp3::Mode::Constant(bitrate),
            };
            let encode = coreav_transcode::audio::Encode::Mp3(&settings, program);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            let encoded = coreav_transcode::audio::encode_pcm(
                &samples.data,
                &summary,
                &encode,
                Some(&launcher),
                deadline,
                Path::new(&command.output),
                coreav_transcode::mp3::most_bytes(summary.samples.saturating_add(1152)),
            )
            .map_err(crate::transcode::failure)?;
            let encoded = encoded
                .mp3_packets(deadline, 16 << 20)
                .map_err(crate::transcode::failure)?;
            if kept.saturating_add(encoded.data.len()) > AUDIO_BYTES {
                return Err(refused("AVI encoded audio buffers exceed 64 MiB"));
            }
            for (n, range) in encoded.frames.iter().enumerate() {
                push(
                    chunks,
                    Packet {
                        track: output,
                        time: n as u64,
                        duration: 1,
                        size: range.len() as u32,
                        keyframe: true,
                    },
                    Location::Memory {
                        buffer,
                        offset: range.start,
                    },
                )?;
            }
            buffers.push(encoded.data);
            return Ok(Track::Mp3 {
                rate,
                channels,
                frame_samples: encoded.frame_samples,
                bytes_per_second: encoded.bytes_per_second,
            });
        }
        #[cfg(not(feature = "transcode"))]
        return Err(refused("AVI MP3 encoding needs the transcode feature"));
    }
    let bits = if encoder == AviAudio::PcmU8 { 8 } else { 16 };
    if bits == 8 {
        samples.data = samples
            .data
            .chunks_exact(2)
            .map(|b| {
                let mut bytes = [0; 2];
                bytes.copy_from_slice(b);
                ((i32::from(i16::from_le_bytes(bytes)) + 32768) >> 8) as u8
            })
            .collect();
    }
    let align = usize::from(channels) * usize::from(bits / 8);
    let chunk_bytes = 1024 * align;
    for (n, bytes) in samples.data.chunks(chunk_bytes).enumerate() {
        push(
            chunks,
            Packet {
                track: output,
                time: (n * 1024) as u64,
                duration: (bytes.len() / align) as u32,
                size: bytes.len() as u32,
                keyframe: true,
            },
            Location::Memory {
                buffer,
                offset: n * chunk_bytes,
            },
        )?;
    }
    buffers.push(samples.data);
    Ok(Track::Pcm {
        rate,
        channels,
        bits,
    })
}

pub(super) fn write(command: &Command, jobs: &[(usize, Codec)]) -> Result<serde_json::Value, Stop> {
    let path = Path::new(&command.output);
    let force = super::check_output(command, path)?;
    let mut source =
        BufferedSource::new(FileSource::open(&command.input).map_err(WriteFailure::from)?);
    let limits = Limits::default();
    let movie = coreav_isobmff::probe(&mut source, &limits, &Allowlist::listing())
        .map_err(WriteFailure::from)?;
    let mut tracks = Vec::new();
    let mut chunks = Vec::new();
    let mut buffers = Vec::new();
    let mut budget = Budget::new(&limits);
    for (output, (stream, codec)) in jobs.iter().enumerate() {
        let track = match codec {
            Codec::Copy => source_track(
                &mut source,
                &movie,
                *stream,
                output,
                &mut chunks,
                &mut budget,
            )?,
            Codec::AviAudio(encoder) => encoded_audio(
                command,
                *stream,
                output,
                *encoder,
                &mut chunks,
                &mut buffers,
            )?,
            _ => return Err(refused("this AVI encoder")),
        };
        tracks.push(track);
    }
    let clock = |index: usize| match tracks.get(index) {
        Some(Track::H264 { scale, rate, .. }) => (*scale, *rate),
        Some(Track::Pcm { rate, .. }) => (1, *rate),
        Some(Track::Mp3 {
            frame_samples,
            rate,
            ..
        }) => (*frame_samples, *rate),
        None => (1, 1),
    };
    chunks.sort_by(|a, b| {
        let (sa, ra) = clock(a.packet.track);
        let (sb, rb) = clock(b.packet.track);
        (i128::from(a.packet.time) * i128::from(sa) * i128::from(rb))
            .cmp(&(i128::from(b.packet.time) * i128::from(sb) * i128::from(ra)))
    });
    let packets: Vec<_> = chunks.iter().map(|c| c.packet).collect();
    let plan = coreav_avi::plan(&tracks, &packets, &coreav_avi::Limits::default())
        .map_err(format_error)?;
    let written = output::to_file(path, force, |out| {
        let mut buffer = Vec::new();
        plan.write(out, |i, out| {
            let chunk = chunks
                .get(i)
                .ok_or_else(|| io::Error::other("AVI chunk missing"))?;
            let size = chunk.packet.size as usize;
            match chunk.location {
                Location::Source(offset) => {
                    buffer.resize(size, 0);
                    source
                        .read_exact_at(offset, &mut buffer)
                        .map_err(|e| io::Error::other(e.to_string()))?;
                    out.write_all(&buffer)
                }
                Location::Memory { buffer, offset } => {
                    let bytes = buffers
                        .get(buffer)
                        .and_then(|b| b.get(offset..offset + size))
                        .ok_or_else(|| io::Error::other("AVI buffered packet missing"))?;
                    out.write_all(bytes)
                }
            }
        })
        .map_err(|e| match e {
            coreav_avi::Error::Io(e) => WriteFailure::output(&e),
            _ => WriteFailure::Rejected(Failure::Unsupported(e.to_string())),
        })
    })?;
    Ok(serde_json::json!({ "bytes": written }))
}
