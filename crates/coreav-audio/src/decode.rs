//! Decoding one audio track of an MP4/MOV or Matroska/WebM file to
//! interleaved PCM (`docs/design/audio-decode.md`, "Pipeline").
//!
//! [`decode`] detects the container, picks the track, reads its packets
//! through the existing demuxers, decodes them, applies the container's
//! trimming (an MP4 edit list's first media time, Matroska's `CodecDelay`
//! and `DiscardPadding`) and hands interleaved little-endian samples in the
//! asked [`SampleFormat`] to a [`Sink`], such as [`crate::WavSink`].

use crate::conversion::Converter;
use crate::stream::{Declared, Piece, Stream};
use crate::{Error, LimitKind, Limits, Warning};
use coreav_isobmff::{self as mp4, Source};
use coreav_mkv as mkv;
use std::collections::VecDeque;
use std::thread::Scope;

/// A codec this crate decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    /// FLAC (RFC 9639).
    Flac,
    /// MPEG audio Layer III (ISO/IEC 11172-3, 13818-3).
    Mp3,
    /// Opus (RFC 6716, RFC 7845), decoded to 48 kHz.
    Opus,
    /// AAC LC (ISO/IEC 14496-3, 13818-7), with the opt-in feature `aac`.
    #[cfg(feature = "aac")]
    Aac,
}

impl Codec {
    /// The codec's name, as `coreav probe` and `ffprobe` print it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Opus => "opus",
            #[cfg(feature = "aac")]
            Self::Aac => "aac",
        }
    }
}

/// The sample format of the output, interleaved and little-endian.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SampleFormat {
    /// Signed 16-bit integers, the default of `ffmpeg`'s WAV output.
    S16,
    /// Signed 24-bit integers in three bytes.
    S24,
    /// Signed 32-bit integers.
    S32,
    /// 32-bit IEEE floating point, full scale ±1.0.
    F32,
}

impl SampleFormat {
    /// Bits per sample.
    pub fn bits(self) -> u16 {
        match self {
            Self::S16 => 16,
            Self::S24 => 24,
            Self::S32 | Self::F32 => 32,
        }
    }

    /// Bytes per sample.
    pub fn bytes(self) -> usize {
        usize::from(self.bits() / 8)
    }

    /// The name `--format` takes.
    pub fn name(self) -> &'static str {
        match self {
            Self::S16 => "s16",
            Self::S24 => "s24",
            Self::S32 => "s32",
            Self::F32 => "f32",
        }
    }

    /// The format a name means.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "s16" => Self::S16,
            "s24" => Self::S24,
            "s32" => Self::S32,
            "f32" => Self::F32,
            _ => return None,
        })
    }
}

/// What to decode, and into what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// The stream to decode, by its position among all tracks of the file
    /// (`ffprobe`'s `index`); `None` for the first audio stream with a codec
    /// this crate decodes.
    pub stream: Option<usize>,
    /// The output sample format.
    pub format: SampleFormat,
    /// Threads that decode packets in parallel (FLAC, and MP3 with each
    /// job primed by the frames before it): 1 decodes on the caller's
    /// thread, 0 uses one per CPU, at most 16. Default 1. Opus always
    /// decodes on the caller's thread: its frames carry the decoder's state
    /// forward, and no priming reproduces it exactly.
    pub threads: usize,
    /// The output's sample rate in Hz, 1,000 to 768,000; `None` keeps the
    /// stream's. Another rate is resampled
    /// (`docs/design/audio-decode.md`, "Resampling and downmixing").
    pub sample_rate: Option<u32>,
    /// The output's channels; `None` keeps the stream's. Other than the
    /// stream's, 1 or 2: downmixed (or mono spread to stereo) by ITU-R
    /// BS.775.
    pub channels: Option<u16>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            stream: None,
            format: SampleFormat::S16,
            threads: 1,
            sample_rate: None,
            channels: None,
        }
    }
}

/// The format of the output, known before its first sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputFormat {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channels.
    pub channels: u16,
    /// Speaker positions, as WAV's `dwChannelMask` (0 when unknown).
    pub channel_mask: u32,
    /// Sample format.
    pub format: SampleFormat,
}

/// The decoded stream as its codec configuration describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamFormat {
    /// The codec.
    pub codec: Codec,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channels.
    pub channels: u16,
    /// Bits per sample of a lossless codec.
    pub bits_per_sample: Option<u32>,
}

/// Where the output goes.
pub trait Sink {
    /// Called once, before the first samples.
    fn start(&mut self, format: &OutputFormat) -> Result<(), Error>;
    /// Interleaved little-endian samples of the output format: whole sample
    /// frames (one sample of every channel).
    fn write(&mut self, samples: &[u8]) -> Result<(), Error>;
}

/// What a request decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    /// The stream's position among all tracks of the file.
    pub stream_index: usize,
    /// The stream as decoded.
    pub stream: StreamFormat,
    /// The output.
    pub output: OutputFormat,
    /// Packets read.
    pub packets: u64,
    /// Samples written per channel.
    pub samples: u64,
    /// Where the presented media sits in the output: the silence written
    /// for empty edits (`docs/design/audio-decode.md`, "Edit lists").
    pub timeline: Timeline,
    /// Recoverable problems.
    pub warnings: Vec<Warning>,
}

/// The output's presentation timeline: sample n of the output is the
/// sound at presentation time n / rate, and empty edits are silence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timeline {
    /// Samples per channel before the first presented media sample: the
    /// silence of the leading empty edits, 0 without one (all the samples
    /// when no media sample was written).
    pub start: u64,
    /// Every span of silence an empty edit wrote, in output order.
    pub silence: Vec<SilenceSpan>,
}

/// A span of silence an empty edit wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SilenceSpan {
    /// Its first sample per channel in the output.
    pub at: u64,
    /// Its length in samples per channel.
    pub samples: u64,
}

/// Decodes the stream `options` names from `source` into `sink`.
pub fn decode<S: Source>(
    source: S,
    options: &Options,
    limits: &Limits,
    sink: &mut dyn Sink,
) -> Result<Summary, Error> {
    let mut source = source;
    let len = source.len()?;
    let mut prefix = [0u8; 4];
    if len >= 4 {
        source.read_exact_at(0, &mut prefix)?;
    }
    // The decoding threads, if any, live inside this scope, which ends
    // only once every one of them has.
    std::thread::scope(|scope| {
        if mkv::sniff(&prefix) {
            decode_matroska(source, options, limits, sink, scope)
        } else {
            match decode_mp4(source, options, limits, sink, scope) {
                Err(Error::Isobmff(mp4::Error::NotIsobmff)) => Err(Error::UnknownContainer),
                other => other,
            }
        }
    })
}

/// An I/O error of the source, as opposed to a malformed file.
pub(crate) fn is_io(error: &Error) -> bool {
    matches!(
        error,
        Error::Isobmff(mp4::Error::Io(_)) | Error::Matroska(mkv::Error::Io(_))
    )
}

/// The codec this crate decodes for an MP4 codec.
fn mp4_codec(codec: mp4::Codec) -> Option<Codec> {
    match codec {
        mp4::Codec::Flac => Some(Codec::Flac),
        mp4::Codec::Mp3 => Some(Codec::Mp3),
        mp4::Codec::Opus => Some(Codec::Opus),
        #[cfg(feature = "aac")]
        mp4::Codec::Aac => Some(Codec::Aac),
        _ => None,
    }
}

/// The codec this crate decodes for a Matroska codec.
fn mkv_codec(codec: mkv::Codec) -> Option<Codec> {
    match codec {
        mkv::Codec::Flac => Some(Codec::Flac),
        mkv::Codec::Mp3 => Some(Codec::Mp3),
        mkv::Codec::Opus => Some(Codec::Opus),
        #[cfg(feature = "aac")]
        mkv::Codec::Aac => Some(Codec::Aac),
        _ => None,
    }
}

fn not_decoded(name: &str) -> Error {
    Error::CodecNotDecoded(name.to_owned())
}

fn decode_mp4<'scope, 'env, S: Source>(
    mut source: S,
    options: &Options,
    limits: &Limits,
    sink: &mut dyn Sink,
    scope: &'scope Scope<'scope, 'env>,
) -> Result<Summary, Error> {
    // Every audio codec the demuxer names, so that a stream this crate does
    // not decode is reported by name rather than as missing.
    let allowlist = mp4::Allowlist::new(&[
        mp4::Codec::Aac,
        mp4::Codec::Mp3,
        mp4::Codec::Opus,
        mp4::Codec::Flac,
        mp4::Codec::Pcm,
    ]);
    let movie = mp4::probe(&mut source, &limits.isobmff, &allowlist)?;
    let track = match options.stream {
        Some(index) => {
            if let Some(track) = movie.tracks.iter().find(|t| t.stream_index == index) {
                if track.kind != mp4::TrackKind::Audio {
                    return Err(Error::NoSuchStream);
                }
                if mp4_codec(track.description.codec).is_none() {
                    return Err(not_decoded(
                        &format!("{:?}", track.description.codec).to_lowercase(),
                    ));
                }
                track
            } else if let Some(dropped) = movie.dropped.iter().find(|d| d.stream_index == index) {
                return Err(not_decoded(&dropped.fourcc.to_string()));
            } else {
                return Err(Error::NoSuchStream);
            }
        }
        None => {
            let audio: Vec<_> = movie
                .tracks
                .iter()
                .filter(|t| t.kind == mp4::TrackKind::Audio)
                .collect();
            match audio
                .iter()
                .copied()
                .find(|t| mp4_codec(t.description.codec).is_some())
            {
                Some(track) => track,
                None => {
                    return Err(match audio.first() {
                        Some(t) => {
                            not_decoded(&format!("{:?}", t.description.codec).to_lowercase())
                        }
                        None => match movie
                            .dropped
                            .iter()
                            .find(|d| d.kind == mp4::TrackKind::Audio)
                        {
                            Some(d) => not_decoded(&d.fourcc.to_string()),
                            None => Error::NoSuchStream,
                        },
                    });
                }
            }
        }
    };
    let codec = mp4_codec(track.description.codec).ok_or(Error::NoSuchStream)?;
    let config = track
        .description
        .codec_config
        .as_ref()
        .map(|c| c.data.as_slice())
        .unwrap_or_default();
    // AAC's configuration is the `DecoderSpecificInfo` inside `esds`;
    // MP4 has no fallback without it.
    #[cfg(feature = "aac")]
    let config = if codec == Codec::Aac {
        match mp4::esds_decoder_specific_info(config) {
            Some((0x66, _)) => return Err(not_decoded("AAC Main")),
            Some((0x68, _)) => return Err(not_decoded("AAC SSR")),
            Some((_, Some(specific))) if !specific.is_empty() => specific,
            _ => return Err(Error::Invalid("an AAC track without a DecoderSpecificInfo")),
        }
    } else {
        config
    };
    let timescale = u64::from(track.timescale.max(1));
    let mut budget = mp4::Budget::new(&limits.isobmff);
    let mut packets = movie.packets(track.index, &mut source, &limits.isobmff, &mut budget)?;
    let mut data = Vec::new();
    // The first packet is read before the stream is set up: an MP3
    // stream's format is its first frame's.
    let first = next_mp4(&mut packets, &mut data, limits)?;
    let mut stream = Stream::new(
        codec,
        config,
        first.as_ref().map(|_| data.as_slice()),
        Declared {
            sample_rate: track.description.sample_rate,
            channels: track.description.channels,
        },
        options,
        limits,
        scope,
    )?;
    let mut converter = None;
    let sink: &mut dyn Sink = match stream.plan.take() {
        Some(plan) => converter.insert(Converter::new(plan, sink)),
        None => sink,
    };
    // The edit list's presentation timeline (ISO/IEC 14496-12 8.6.6;
    // docs/design/audio-decode.md, "Edit lists"): empty edits as silence,
    // media edits as their windows, sample-exact.
    let rate = u64::from(stream.out.format.sample_rate);
    let first_dts = first.as_ref().map_or(0, |p| p.dts);
    match track.edit_list {
        // Without an edit list to present, the codec's priming samples are
        // still dropped: Opus's pre-skip, which the Opus-in-ISOBMFF mapping
        // leaves to the edit list.
        mp4::EditList::None => {
            stream.out.plan = VecDeque::from([Piece::Media {
                from: stream.pre_skip,
                to: u64::MAX,
            }]);
        }
        mp4::EditList::Simple { .. } | mp4::EditList::Multiple => {
            let edits = track
                .edits
                .iter()
                .map(|e| (e.segment_duration, e.media_time));
            stream.out.plan = plan(
                edits,
                first_dts,
                rate,
                timescale,
                u64::from(movie.timescale),
            );
            // Opus's priming samples are never presented: a window that
            // starts inside the pre-skip starts after it, as for a file
            // whose edit list starts at media time 0 and leaves the
            // pre-skip to `dOps`.
            let pre_skip = stream.pre_skip;
            for piece in &mut stream.out.plan {
                if let Piece::Media { from, to } = piece {
                    *from = (*from).max(pre_skip);
                    *to = (*to).max(*from);
                }
            }
            // Past the last media edit nothing is presented: packets that
            // start at or after its end are not decoded, by their
            // timestamps, nor frames cut from packets, by their own
            // positions, so where decoding stops is known before it starts
            // and never depends on the threads.
            // A last window that runs to the media's end (its end
            // saturated) sets no stop.
            stream.end = stream
                .out
                .plan
                .iter()
                .rev()
                .find_map(|piece| match piece {
                    Piece::Media { to, .. } => Some(*to),
                    _ => None,
                })
                .filter(|to| *to != u64::MAX);
        }
        mp4::EditList::Unsupported => {
            stream.out.warnings.push(Warning::EditListIgnored);
            stream.out.plan = VecDeque::from([Piece::Media {
                from: stream.pre_skip,
                to: u64::MAX,
            }]);
        }
    }
    stream.sink_started(sink)?;
    let mut next = first;
    while let Some(packet) = next {
        // The packet's start in samples from the first packet, compared
        // exactly: (dts - first) × rate ≥ end × timescale.
        let since = u128::try_from(i128::from(packet.dts) - i128::from(first_dts)).unwrap_or(0);
        if !stream.reframed()
            && stream.end.is_some_and(|end| {
                since.saturating_mul(u128::from(rate))
                    >= u128::from(end).saturating_mul(u128::from(timescale))
            })
        {
            break;
        }
        let duration = scale(u64::from(packet.duration), rate, timescale);
        stream.packet(&data, Some(duration), 0, sink)?;
        next = match next_mp4(&mut packets, &mut data, limits) {
            Ok(next) => next,
            Err(error) => {
                stream.demux_error(error)?;
                None
            }
        };
    }
    let summary = stream.finish(track.stream_index, sink)?;
    match converter {
        Some(converter) => converter.finish(summary),
        None => Ok(summary),
    }
}

fn decode_matroska<'scope, 'env, S: Source>(
    mut source: S,
    options: &Options,
    limits: &Limits,
    sink: &mut dyn Sink,
    scope: &'scope Scope<'scope, 'env>,
) -> Result<Summary, Error> {
    let segment = mkv::probe(&mut source, &limits.matroska, &mkv::Allowlist::all())?;
    let track = match options.stream {
        Some(index) => {
            if let Some(track) = segment.tracks.iter().find(|t| t.stream_index == index) {
                if track.kind != mkv::TrackKind::Audio {
                    return Err(Error::NoSuchStream);
                }
                if mkv_codec(track.codec).is_none() {
                    return Err(not_decoded(codec_label(track)));
                }
                track
            } else if let Some(dropped) = segment.dropped.iter().find(|d| d.stream_index == index) {
                return Err(not_decoded(&dropped.codec_id));
            } else {
                return Err(Error::NoSuchStream);
            }
        }
        None => {
            let audio: Vec<_> = segment
                .tracks
                .iter()
                .filter(|t| t.kind == mkv::TrackKind::Audio)
                .collect();
            match audio.iter().copied().find(|t| mkv_codec(t.codec).is_some()) {
                Some(track) => track,
                None => {
                    return Err(match audio.first() {
                        Some(t) => not_decoded(codec_label(t)),
                        None => match segment
                            .dropped
                            .iter()
                            .find(|d| d.kind == mkv::TrackKind::Audio)
                        {
                            Some(d) => not_decoded(&d.codec_id),
                            None => Error::NoSuchStream,
                        },
                    });
                }
            }
        }
    };
    let codec = mkv_codec(track.codec).ok_or(Error::NoSuchStream)?;
    if !track.packets_available() {
        return Err(Error::Unsupported(
            "the track's frames cannot be read (content encoding)",
        ));
    }
    let prefix: &[u8] = match &track.encoding {
        mkv::Encoding::None => &[],
        mkv::Encoding::HeaderStripping(bytes) => bytes,
        mkv::Encoding::Encrypted => return Err(Error::Unsupported("encrypted frames")),
        mkv::Encoding::Zlib => return Err(Error::Unsupported("zlib-compressed frames")),
        mkv::Encoding::Unsupported => {
            return Err(Error::Unsupported("the track's content encoding"));
        }
    };
    // A legacy AAC codec ID without `CodecPrivate` names its object type
    // and SBR (the configuration then comes from the rate and channels).
    #[cfg(feature = "aac")]
    if codec == Codec::Aac && track.codec_private.is_none() {
        let refused = match track.profile {
            Some(mkv::Profile::Aac { sbr: true, .. }) => Some("HE-AAC (SBR)"),
            Some(mkv::Profile::Aac { object_type: 1, .. }) => Some("AAC Main"),
            Some(mkv::Profile::Aac { object_type: 3, .. }) => Some("AAC SSR"),
            Some(mkv::Profile::Aac { object_type: 4, .. }) => Some("AAC LTP"),
            _ => None,
        };
        if let Some(name) = refused {
            return Err(not_decoded(name));
        }
    }
    let audio = track.audio.as_ref();
    let ns_per_tick = segment.timestamp_scale.max(1);
    let mut data = Vec::new();
    let mut packets = segment.packets(&mut source, &limits.matroska);
    // The first packet is read before the stream is set up: an MP3
    // stream's format is its first frame's.
    let first = next_mkv(&mut packets, track.index, prefix, &mut data, limits)?;
    let mut stream = Stream::new(
        codec,
        track.codec_private.as_deref().unwrap_or_default(),
        first.as_ref().map(|_| data.as_slice()),
        Declared {
            sample_rate: audio.and_then(mkv::Audio::sample_rate),
            channels: audio.and_then(|a| u32::try_from(a.channels).ok()),
        },
        options,
        limits,
        scope,
    )?;
    let mut converter = None;
    let sink: &mut dyn Sink = match stream.plan.take() {
        Some(plan) => converter.insert(Converter::new(plan, sink)),
        None => sink,
    };
    let rate = u64::from(stream.out.format.sample_rate);
    // `CodecDelay` (RFC 9559 5.1.4.1.24): samples to drop from the start;
    // without one, the codec's own priming samples (Opus's pre-skip).
    let delay = match track.codec_delay {
        0 => stream.pre_skip,
        ns => scale(ns, rate, 1_000_000_000),
    };
    stream.out.plan = VecDeque::from([Piece::Media {
        from: delay,
        to: u64::MAX,
    }]);
    stream.sink_started(sink)?;
    let mut next = first;
    while let Some(packet) = next {
        let duration = packet
            .block_duration
            .map(|ticks| scale(ticks.saturating_mul(ns_per_tick), rate, 1_000_000_000))
            .or_else(|| {
                track
                    .default_duration
                    .map(|ns| scale(ns, rate, 1_000_000_000))
            });
        // `DiscardPadding` (RFC 9559 5.1.3.5.4): samples to drop from the
        // end of this frame.
        let discard = packet
            .discard_padding
            .and_then(|ns| u64::try_from(ns).ok())
            .map_or(0, |ns| scale(ns, rate, 1_000_000_000));
        stream.packet(&data, duration, discard, sink)?;
        next = match next_mkv(&mut packets, track.index, prefix, &mut data, limits) {
            Ok(next) => next,
            Err(error) => {
                stream.demux_error(error)?;
                None
            }
        };
    }
    let summary = stream.finish(track.stream_index, sink)?;
    match converter {
        Some(converter) => converter.finish(summary),
        None => Ok(summary),
    }
}

/// Reads the next packet of the track into `data`.
fn next_mp4<S: Source>(
    packets: &mut mp4::Packets<'_, S>,
    data: &mut Vec<u8>,
    limits: &Limits,
) -> Result<Option<mp4::Packet>, Error> {
    let Some(packet) = packets.next() else {
        return Ok(None);
    };
    let packet = packet?;
    if u64::from(packet.size) > limits.max_packet_bytes {
        return Err(Error::LimitExceeded(LimitKind::PacketBytes));
    }
    data.clear();
    data.resize(packet.size as usize, 0);
    packets.source_mut().read_exact_at(packet.offset, data)?;
    Ok(Some(packet))
}

/// Reads the next packet of track `track` into `data`, the stripped header
/// bytes `prefix` first.
fn next_mkv<S: Source>(
    packets: &mut mkv::Packets<'_, S>,
    track: usize,
    prefix: &[u8],
    data: &mut Vec<u8>,
    limits: &Limits,
) -> Result<Option<mkv::Packet>, Error> {
    loop {
        let Some(packet) = packets.next() else {
            return Ok(None);
        };
        let packet = packet?;
        if packet.track != track {
            continue;
        }
        if packet.size() > limits.max_packet_bytes {
            return Err(Error::LimitExceeded(LimitKind::PacketBytes));
        }
        data.clear();
        data.extend_from_slice(prefix);
        let start = data.len();
        data.resize(start.saturating_add(packet.stored_size as usize), 0);
        let stored = data.get_mut(start..).unwrap_or_default();
        packets.source_mut().read_exact_at(packet.offset, stored)?;
        return Ok(Some(packet));
    }
}

fn codec_label(track: &mkv::Track) -> &str {
    if track.codec == mkv::Codec::Other {
        &track.codec_id
    } else {
        track.codec.name()
    }
}

/// The presentation timeline of an edit list, its entries' `(segment_duration,
/// media_time)` in `edits`, in samples at `rate`
/// from the first packet (decode time `first_dts`, media ticks at
/// `timescale`): entry k starts at the exact sum of the durations before it
/// (movie ticks at `movie_timescale`), each boundary rounded to the nearest
/// sample, halves up, and each span the difference of its boundaries. A
/// media edit of duration 0 (the simple model's open one) lasts to the end.
/// 128-bit arithmetic, saturating: no entry overflows it.
fn plan(
    edits: impl IntoIterator<Item = (u64, i64)>,
    first_dts: i64,
    rate: u64,
    timescale: u64,
    movie_timescale: u64,
) -> VecDeque<Piece> {
    let (rate, timescale, movie_timescale) = (
        u128::from(rate),
        u128::from(timescale.max(1)),
        u128::from(movie_timescale.max(1)),
    );
    let samples = |numerator: u128, denominator: u128| -> u64 {
        let rounded = numerator
            .saturating_mul(rate)
            .saturating_mul(2)
            .saturating_add(denominator)
            / denominator.saturating_mul(2);
        u64::try_from(rounded).unwrap_or(u64::MAX)
    };
    let mut pieces = VecDeque::new();
    let mut elapsed = 0u128;
    for (segment_duration, media_time) in edits {
        let begin = samples(elapsed, movie_timescale);
        elapsed = elapsed.saturating_add(u128::from(segment_duration));
        let length = samples(elapsed, movie_timescale).saturating_sub(begin);
        if media_time < 0 {
            pieces.push_back(Piece::Silence(length));
            continue;
        }
        // The window's start in samples from the first packet, rounded
        // halves up. A media time before the first packet (a fragment's
        // decode time above 0) starts the window before the media: it ends
        // where the edit ends, and the decoded samples begin at 0.
        let since = i128::from(media_time) - i128::from(first_dts);
        let (from, before) = match u128::try_from(since) {
            Ok(since) => (samples(since, timescale), 0),
            Err(_) => {
                let ahead = since.unsigned_abs();
                let rounded = ahead
                    .saturating_mul(rate)
                    .saturating_mul(2)
                    .saturating_add(timescale - 1)
                    / timescale.saturating_mul(2);
                (0, u64::try_from(rounded).unwrap_or(u64::MAX))
            }
        };
        let to = if segment_duration == 0 {
            u64::MAX
        } else {
            from.saturating_add(length).saturating_sub(before)
        };
        pieces.push_back(Piece::Media { from, to });
    }
    pieces
}

/// `value * num / den`, rounded to the nearest integer, saturating.
fn scale(value: u64, num: u64, den: u64) -> u64 {
    if den == 0 {
        return 0;
    }
    let product = u128::from(value) * u128::from(num);
    let rounded = (product + u128::from(den) / 2) / u128::from(den);
    u64::try_from(rounded).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(edits: &[(u64, i64)], first_dts: i64) -> Vec<Piece> {
        plan(edits.iter().copied(), first_dts, 1000, 1000, 1000).into()
    }

    #[test]
    fn a_window_ends_where_its_edit_ends() {
        assert_eq!(
            pieces(&[(300, 200)], 0),
            [Piece::Media { from: 200, to: 500 }]
        );
        // A media time 100 ticks before the first packet: the window's
        // first 100 samples are before the media.
        assert_eq!(
            pieces(&[(300, 200)], 300),
            [Piece::Media { from: 0, to: 200 }]
        );
        assert_eq!(pieces(&[(50, 200)], 300), [Piece::Media { from: 0, to: 0 }]);
        assert_eq!(
            pieces(&[(10, -1), (0, 5)], 0),
            [
                Piece::Silence(10),
                Piece::Media {
                    from: 5,
                    to: u64::MAX
                }
            ]
        );
    }
}
