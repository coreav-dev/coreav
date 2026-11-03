//! Sample-preserving FFV1 output through the existing Matroska writer.

use std::fmt;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

use coreav_decode::{Backend, Frame, Packet};
use coreav_frame::{ColorInfo, PixelFormat, Pixels};
use coreav_isobmff::{Color, Rational, Source};
use coreav_mkv::write::{CodecId, Media, Options as WriteOptions, Packet as OutputPacket, Track};
use coreav_thumbnail::{StreamPacket, ThumbnailOptions};

/// Source admission and independent resource limits for one encode.
#[derive(Clone, Debug)]
pub struct Options {
    /// Demuxer, selected stream, decoder policy and decoder limits.
    pub source: ThumbnailOptions,
    /// Whether the caller's source-header preflight established progressive video.
    pub progressive: bool,
    /// A required decoded format, without conversion.
    pub pixel_format: Option<PixelFormat>,
    /// Limits for sample coding and its returned packet.
    pub encoder: coreav_ffv1::Limits,
    /// Maximum size of the temporary encoded-packet file.
    pub max_spool_bytes: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            source: ThumbnailOptions::at(Rational::from_int(0)),
            progressive: false,
            pixel_format: None,
            encoder: coreav_ffv1::Limits::default(),
            max_spool_bytes: crate::pipeline::MAX_SPOOL_BYTES,
        }
    }
}

/// A failed encode, without external-encoder discovery or fallback.
#[derive(Debug)]
pub enum Error {
    /// A source or requested transformation outside the supported subset.
    Unsupported(&'static str),
    /// A decoded frame violates an encode that has already started.
    InvalidFrame(&'static str),
    /// An index, spool, dimension, timestamp or allocation bound was exceeded.
    Limit,
    /// The command deadline elapsed.
    Timeout,
    /// Source demuxing failed.
    Source(coreav_thumbnail::Error),
    /// The selected decoder failed.
    Decode(coreav_decode::Error),
    /// Sample coding failed.
    Encode(coreav_ffv1::Error),
    /// Matroska layout or output failed.
    Matroska(coreav_mkv::Error),
    /// Creating or reopening the bounded spool failed.
    Storage(crate::pipeline::Error),
    /// Reading or writing the spool or destination failed.
    Io(std::io::ErrorKind),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(why) | Self::InvalidFrame(why) => write!(f, "FFV1: {why}"),
            Self::Limit => f.write_str("FFV1 resource or arithmetic limit exceeded"),
            Self::Timeout => f.write_str("FFV1 command deadline exceeded"),
            Self::Source(e) => write!(f, "{e}"),
            Self::Decode(e) => write!(f, "{e}"),
            Self::Encode(e) => write!(f, "{e}"),
            Self::Matroska(e) => write!(f, "{e}"),
            Self::Storage(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "FFV1 I/O: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// The failure class used by native command adapters.
    pub fn kind(&self) -> coreav_decode::ErrorKind {
        use coreav_decode::ErrorKind;
        match self {
            Self::Unsupported(_) => ErrorKind::Unsupported,
            Self::InvalidFrame(_) => ErrorKind::InvalidData,
            Self::Limit => ErrorKind::Limit,
            Self::Timeout => ErrorKind::Timeout,
            Self::Encode(coreav_ffv1::Error::Unsupported(_)) => ErrorKind::Unsupported,
            Self::Encode(coreav_ffv1::Error::Limit(_) | coreav_ffv1::Error::Allocation) => {
                ErrorKind::Limit
            }
            Self::Encode(coreav_ffv1::Error::Timeout) => ErrorKind::Timeout,
            Self::Encode(coreav_ffv1::Error::InvalidFrame) => ErrorKind::InvalidData,
            Self::Decode(e) => e.kind,
            Self::Storage(_) | Self::Io(_) | Self::Matroska(coreav_mkv::Error::Io(_)) => {
                ErrorKind::Io
            }
            Self::Source(_) | Self::Matroska(_) => ErrorKind::InvalidData,
        }
    }
}

fn encode_error(error: coreav_ffv1::Error) -> Error {
    match error {
        coreav_ffv1::Error::Unsupported(why) => Error::Unsupported(why),
        error => Error::Encode(error),
    }
}

/// Complete frame and container counts after successful output.
#[derive(Clone, Copy, Debug)]
pub struct Summary {
    /// Number of encoded and presented frames.
    pub frames: u64,
    /// Complete Matroska file size.
    pub bytes_written: u64,
}

fn checked<T>(value: Option<T>) -> Result<T, Error> {
    value.ok_or(Error::Limit)
}

fn deadline(at: Instant) -> Result<(), Error> {
    if Instant::now() >= at {
        return Err(Error::Timeout);
    }
    Ok(())
}

fn io(error: std::io::Error) -> Error {
    Error::Io(error.kind())
}

fn ns(value: Rational) -> Result<i64, Error> {
    let n = checked(value.num.checked_mul(1_000_000_000))?;
    let d = i128::from(value.den);
    if d == 0 {
        return Err(Error::Limit);
    }
    let q = n.div_euclid(d);
    let up = checked(n.rem_euclid(d).checked_mul(2))? >= d;
    i64::try_from(checked(q.checked_add(i128::from(up)))?).map_err(|_| Error::Limit)
}

fn tick(ns: i64) -> Result<i64, Error> {
    let n = i128::from(ns) + 500_000;
    i64::try_from(n.div_euclid(1_000_000)).map_err(|_| Error::Limit)
}

#[derive(Clone, Copy)]
struct Expected {
    pts: i64,
    time_ns: i64,
    duration_ns: Option<u64>,
}

fn reserve<T>(count: usize) -> Result<Vec<T>, Error> {
    let mut values = Vec::new();
    values.try_reserve_exact(count).map_err(|_| Error::Limit)?;
    Ok(values)
}

fn expected(
    packets: &[StreamPacket],
    time_base: Rational,
    budget: u64,
) -> Result<Vec<Expected>, Error> {
    let count = packets.iter().filter(|p| p.presented).count();
    if count == 0 {
        return Err(Error::Unsupported("no presented video frames"));
    }
    let estimate = checked(count.checked_mul(
        std::mem::size_of::<Expected>() + std::mem::size_of::<OutputPacket>() + 8 + 512,
    ))?;
    if estimate as u64 > budget {
        return Err(Error::Limit);
    }
    let mut values: Vec<Expected> = reserve(count)?;
    for packet in packets.iter().filter(|p| p.presented) {
        let time_ns = ns(packet.time.ok_or(Error::Unsupported(
            "source presentation timestamps are unknown",
        ))?)?;
        let duration_ns = packet
            .duration
            .filter(|d| *d != 0)
            .map(|d| {
                let duration = checked(Rational::new(
                    checked(time_base.num.checked_mul(i128::from(d)))?,
                    time_base.den,
                ))?;
                u64::try_from(ns(duration)?).map_err(|_| Error::Limit)
            })
            .transpose()?;
        if duration_ns == Some(0) {
            return Err(Error::Unsupported("source frame duration rounds to zero"));
        }
        values.push(Expected {
            pts: packet.pts,
            time_ns,
            duration_ns,
        });
    }
    values.sort_unstable_by_key(|p| p.pts);
    for pair in values.windows(2) {
        let a = pair.first().ok_or(Error::Limit)?;
        let b = pair.get(1).ok_or(Error::Limit)?;
        if a.pts >= b.pts || tick(a.time_ns)? >= tick(b.time_ns)? {
            return Err(Error::Unsupported(
                "duplicate, decreasing or unrepresentable presentation timestamps",
            ));
        }
    }
    for index in 0..values.len().saturating_sub(1) {
        let next = values.get(index + 1).ok_or(Error::Limit)?.time_ns;
        let value = values.get_mut(index).ok_or(Error::Limit)?;
        if value.duration_ns.is_none() {
            value.duration_ns = Some(
                u64::try_from(checked(next.checked_sub(value.time_ns))?)
                    .map_err(|_| Error::Limit)?,
            );
        }
    }
    Ok(values)
}

fn colour(stream: ColorInfo, frame: ColorInfo) -> Result<ColorInfo, &'static str> {
    if [
        (stream.primaries, frame.primaries),
        (stream.transfer, frame.transfer),
        (stream.matrix, frame.matrix),
    ]
    .iter()
    .any(|(a, b)| a.zip(*b).is_some_and(|(a, b)| a != b))
        || stream
            .full_range
            .zip(frame.full_range)
            .is_some_and(|(a, b)| a != b)
        || stream
            .chroma_location
            .zip(frame.chroma_location)
            .is_some_and(|(a, b)| a != b)
    {
        return Err("conflicting stream and frame colour");
    }
    Ok(stream.or(frame))
}

fn display(
    width: u32,
    height: u32,
    aspect: Option<(u32, u32)>,
) -> Result<Option<(u32, u32)>, Error> {
    let Some((mut n, mut d)) = aspect else {
        return Ok(None);
    };
    if n == 0 || d == 0 {
        return Err(Error::Unsupported("zero sample aspect ratio"));
    }
    let (mut a, mut b) = (n, d);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    n /= a;
    d /= a;
    if n == d {
        return Ok(None);
    }
    Ok(Some((
        checked(width.checked_mul(n))?,
        checked(height.checked_mul(d))?,
    )))
}

struct Run<'a> {
    options: &'a Options,
    expected: &'a [Expected],
    packets: Vec<OutputPacket>,
    offsets: Vec<u64>,
    encoder: Option<coreav_ffv1::Encoder>,
    track: Option<Track>,
    shape: Option<(u32, u32, PixelFormat, ColorInfo)>,
    aspect: Option<(u32, u32)>,
    stream_colour: ColorInfo,
    spool: std::fs::File,
    bytes: u64,
    deadline: Instant,
}

impl Run<'_> {
    fn refused(&self, why: &'static str) -> Error {
        if self.encoder.is_some() {
            Error::InvalidFrame(why)
        } else {
            Error::Unsupported(why)
        }
    }

    fn decode_error(&self, mut error: coreav_decode::Error) -> Error {
        if self.encoder.is_some()
            && matches!(
                error.kind,
                coreav_decode::ErrorKind::Unsupported | coreav_decode::ErrorKind::CodecNotAllowed
            )
        {
            error.kind = coreav_decode::ErrorKind::InvalidData;
        }
        Error::Decode(error)
    }

    fn take(&mut self, frame: Frame) -> Result<(), Error> {
        deadline(self.deadline)?;
        let pts = frame
            .pts
            .ok_or_else(|| self.refused("decoded frame has no timestamp"))?;
        let Some(wanted) = self.expected.get(self.packets.len()) else {
            return Err(self.refused("decoder returned extra presented frames"));
        };
        if pts != wanted.pts {
            return Err(self.refused("decoder did not preserve complete presentation order"));
        }
        if self
            .options
            .pixel_format
            .is_some_and(|f| f != frame.format())
        {
            return Err(self.refused("requested pixel format requires conversion"));
        }
        let colour = colour(self.stream_colour, frame.color).map_err(|why| self.refused(why))?;
        if colour.matrix == Some(0) {
            return Err(self.refused("RGB colour representation is not supported"));
        }
        let shape = (frame.width(), frame.height(), frame.format(), frame.color);
        if self.shape.is_some_and(|s| s != shape) {
            return Err(self.refused("video geometry, format or colour changes"));
        }
        let Pixels::Cpu(picture) = frame.pixels else {
            return Err(self.refused("FFV1 requires CPU-owned planar pixels"));
        };
        if self.encoder.is_none() {
            let encoder = coreav_ffv1::Encoder::new(
                coreav_ffv1::Settings {
                    width: shape.0,
                    height: shape.1,
                    format: shape.2,
                    sample_aspect: self.aspect,
                },
                self.options.encoder,
            )
            .map_err(encode_error)?;
            if encoder.configuration().len() as u64
                > self.options.source.matroska_limits.max_element_bytes
                || encoder.configuration().len() as u64 + 259
                    > self.options.source.matroska_limits.max_retained_bytes
            {
                return Err(Error::Limit);
            }
            let mut private = reserve(encoder.configuration().len())?;
            private.extend_from_slice(encoder.configuration());
            self.track = Some(Track {
                codec: CodecId::Ffv1,
                private,
                media: Media::Video {
                    width: shape.0,
                    height: shape.1,
                    display: display(shape.0, shape.1, self.aspect)?,
                    colour: Color {
                        primaries: colour.primaries.map(u16::from),
                        transfer: colour.transfer.map(u16::from),
                        matrix: colour.matrix.map(u16::from),
                        full_range: colour.full_range,
                    },
                    interlaced: 2,
                    field_order: 0,
                },
                default_duration_ns: None,
                codec_delay_ns: 0,
                seek_pre_roll_ns: 0,
                language: "und".to_owned(),
                name: None,
                default: true,
            });
            self.encoder = Some(encoder);
            self.shape = Some(shape);
        }
        let data = self
            .encoder
            .as_mut()
            .ok_or(Error::Limit)?
            .encode(&picture, self.deadline)
            .map_err(encode_error)?;
        let end = checked(self.bytes.checked_add(data.len() as u64))?;
        if end > self.options.max_spool_bytes
            || data.len() as u64 > self.options.source.matroska_limits.max_packet_bytes
        {
            return Err(Error::Limit);
        }
        self.spool.write_all(&data).map_err(io)?;
        self.offsets.push(self.bytes);
        self.bytes = end;
        self.packets.push(OutputPacket {
            track: 0,
            pts_ns: wanted.time_ns,
            duration_ns: wanted.duration_ns,
            size: data.len() as u64,
            keyframe: true,
            invisible: false,
            discard_padding_ns: None,
        });
        deadline(self.deadline)
    }
}

/// Decodes exactly the selected progressive presentation and writes one FFV1 track.
/// The caller establishes scan structure from source headers before setting
/// [`Options::progressive`]. Only CPU-owned frames are admitted.
pub fn transcode<S: Source, W: Write>(
    mut source: S,
    options: &Options,
    backends: &[&dyn Backend],
    output: &Path,
    mut out: W,
) -> Result<Summary, Error> {
    if !options.progressive {
        return Err(Error::Unsupported(
            "source scan structure is not known progressive",
        ));
    }
    let end = checked(Instant::now().checked_add(options.source.decode_limits.timeout))?;
    let video =
        coreav_thumbnail::video_stream(&mut source, &options.source).map_err(Error::Source)?;
    deadline(end)?;
    if !matches!(
        video.stream.codec,
        coreav_decode::Codec::H264
            | coreav_decode::Codec::Av1
            | coreav_decode::Codec::Vp8
            | coreav_decode::Codec::Vp9
    ) {
        return Err(Error::Unsupported(
            "source codec has no verified progressive decoder for FFV1",
        ));
    }
    if !video.warnings.is_empty() || video.quarter_turns != 0 {
        return Err(Error::Unsupported(
            "ambiguous timing or rotated source video",
        ));
    }
    if video.packets.iter().any(|p| !p.presented) {
        return Err(Error::Unsupported("source presentation omits video frames"));
    }
    let source_bytes = checked(
        video
            .packets
            .capacity()
            .checked_mul(std::mem::size_of::<StreamPacket>()),
    )?;
    let index_budget = checked(
        options
            .source
            .matroska_limits
            .max_index_bytes
            .checked_sub(source_bytes as u64),
    )?;
    let expected = expected(&video.packets, video.time_base, index_budget)?;
    if let Some(window) = video.presentation {
        let first = expected.first().ok_or(Error::Limit)?;
        let last = expected.last().ok_or(Error::Limit)?;
        let packet = video
            .packets
            .iter()
            .find(|p| p.pts == last.pts)
            .ok_or(Error::Limit)?;
        let complete_end = packet
            .duration
            .and_then(|d| i64::try_from(d).ok())
            .and_then(|d| last.pts.checked_add(d));
        if window.start != first.pts || window.end.is_some_and(|end| Some(end) != complete_end) {
            return Err(Error::Unsupported(
                "source presentation trims a partial frame",
            ));
        }
    }
    let aspect = video
        .pixel_aspect
        .map(|(n, d)| {
            Ok((
                u32::try_from(n).map_err(|_| Error::Limit)?,
                u32::try_from(d).map_err(|_| Error::Limit)?,
            ))
        })
        .transpose()?;
    let count = expected.len();
    let mut packets: Vec<OutputPacket> = reserve(count)?;
    let offsets: Vec<u64> = reserve(count)?;
    let retained = checked(
        checked(
            expected
                .capacity()
                .checked_mul(std::mem::size_of::<Expected>()),
        )?
        .checked_add(checked(
            packets
                .capacity()
                .checked_mul(std::mem::size_of::<OutputPacket>()),
        )?),
    )?;
    let retained = checked(retained.checked_add(checked(
        offsets.capacity().checked_mul(std::mem::size_of::<u64>()),
    )?))?;
    let retained = checked(retained.checked_add(source_bytes))?;
    let mut writer_limits = options.source.matroska_limits;
    writer_limits.max_index_bytes =
        checked(writer_limits.max_index_bytes.checked_sub(retained as u64))?;
    if checked((count as u64).checked_mul(512))? > writer_limits.max_index_bytes
        || count as u64 > writer_limits.max_block_work
    {
        return Err(Error::Limit);
    }
    let spool = crate::pipeline::Spool::create(output, "ffv1").map_err(Error::Storage)?;
    let mut decode_limits = options.source.decode_limits.clone();
    decode_limits.timeout = end.saturating_duration_since(Instant::now());
    let mut policy = options.source.policy.clone();
    policy.order.retain(|backend| {
        matches!(
            backend,
            coreav_decode::BackendKind::Native
                | coreav_decode::BackendKind::Dav1d
                | coreav_decode::BackendKind::Libvpx
        )
    });
    let opened = coreav_decode::open(&video.stream, &policy, &decode_limits, backends)
        .map_err(Error::Decode)?;
    let mut decoder = opened.decoder;
    let mut run = Run {
        options,
        expected: &expected,
        packets: std::mem::take(&mut packets),
        offsets,
        encoder: None,
        track: None,
        shape: None,
        aspect,
        stream_colour: video.stream.color,
        spool: spool.handle().map_err(Error::Storage)?,
        bytes: 0,
        deadline: end,
    };
    for (index, packet) in video.packets.iter().enumerate() {
        deadline(end)?;
        let data = video
            .read_packet(&mut source, index, decode_limits.max_packet_bytes)
            .map_err(Error::Source)?;
        decoder
            .send_packet(&Packet {
                data: &data,
                pts: Some(packet.pts),
                dts: packet.dts,
                keyframe: packet.keyframe,
                discard: false,
            })
            .map_err(|error| run.decode_error(error))?;
        while let Some(frame) = decoder
            .receive_frame()
            .map_err(|error| run.decode_error(error))?
        {
            run.take(frame)?;
        }
    }
    decoder
        .send_eof()
        .map_err(|error| run.decode_error(error))?;
    while let Some(frame) = decoder
        .receive_frame()
        .map_err(|error| run.decode_error(error))?
    {
        run.take(frame)?;
    }
    drop(decoder);
    if run.packets.len() != count {
        return Err(run.refused("decoder omitted presented frames"));
    }
    run.spool.flush().map_err(io)?;
    let track = run
        .track
        .take()
        .ok_or(Error::Unsupported("decoder returned no frames"))?;
    let tracks = [track];
    let plan = coreav_mkv::write::plan(
        &tracks,
        &run.packets,
        WriteOptions::default(),
        &writer_limits,
    )
    .map_err(Error::Matroska)?;
    let bytes_written = plan
        .write(&mut out, |index, destination| {
            let expired = || coreav_mkv::Error::Io(std::io::ErrorKind::TimedOut);
            let invalid = || coreav_mkv::Error::Overflow { offset: 0 };
            deadline(end).map_err(|_| expired())?;
            let offset = run.offsets.get(index).ok_or_else(invalid)?;
            let packet = run.packets.get(index).ok_or_else(invalid)?;
            run.spool
                .seek(SeekFrom::Start(*offset))
                .map_err(|e| coreav_mkv::Error::Io(e.kind()))?;
            let mut remaining = packet.size;
            let mut buffer = [0u8; 65_536];
            while remaining != 0 {
                deadline(end).map_err(|_| expired())?;
                let length =
                    usize::try_from(remaining.min(buffer.len() as u64)).map_err(|_| invalid())?;
                let bytes = buffer.get_mut(..length).ok_or_else(invalid)?;
                run.spool
                    .read_exact(bytes)
                    .map_err(|e| coreav_mkv::Error::Io(e.kind()))?;
                destination
                    .write_all(bytes)
                    .map_err(|e| coreav_mkv::Error::Io(e.kind()))?;
                remaining -= length as u64;
            }
            Ok(())
        })
        .map_err(Error::Matroska)?;
    out.flush().map_err(io)?;
    deadline(end)?;
    Ok(Summary {
        frames: count as u64,
        bytes_written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(pts: i64, duration: Option<u64>) -> StreamPacket {
        StreamPacket {
            offset: 0,
            size: 0,
            pts,
            time: Rational::new(i128::from(pts), 1000),
            dts: Some(pts),
            duration,
            keyframe: true,
            presented: true,
        }
    }

    #[test]
    fn timeline_keeps_nonuniform_times_and_does_not_invent_a_final_duration() {
        let packets = [packet(120, None), packet(0, None), packet(40, Some(80))];
        let per = std::mem::size_of::<Expected>() + std::mem::size_of::<OutputPacket>() + 8 + 512;
        let budget = (per * packets.len()) as u64;
        let values = expected(&packets, Rational::new(1, 1000).unwrap(), budget).unwrap();
        assert_eq!(
            values.iter().map(|v| v.pts).collect::<Vec<_>>(),
            [0, 40, 120]
        );
        assert_eq!(
            values.iter().map(|v| v.duration_ns).collect::<Vec<_>>(),
            [Some(40_000_000), Some(80_000_000), None]
        );
        assert!(matches!(
            expected(&packets, Rational::new(1, 1000).unwrap(), budget - 1),
            Err(Error::Limit)
        ));
        let duplicates = [packet(0, None), packet(0, None)];
        assert!(expected(&duplicates, Rational::new(1, 1000).unwrap(), budget).is_err());
        let mut collapsed = [packet(0, None), packet(1, None)];
        collapsed[1].time = Rational::new(1, 10_000);
        assert!(expected(&collapsed, Rational::new(1, 1000).unwrap(), budget).is_err());
    }

    #[test]
    fn packet_admission_refuses_missing_frames_shape_changes_and_spool_overflow() {
        let options = Options {
            progressive: true,
            ..Options::default()
        };
        let times = [
            Expected {
                pts: 0,
                time_ns: 0,
                duration_ns: Some(40_000_000),
            },
            Expected {
                pts: 40,
                time_ns: 40_000_000,
                duration_ns: None,
            },
        ];
        let path = std::env::temp_dir().join(format!("coreav-ffv1-adapter-{}", std::process::id()));
        let spool = crate::pipeline::Spool::create(&path, "test").unwrap();
        let mut run = Run {
            options: &options,
            expected: &times,
            packets: reserve(2).unwrap(),
            offsets: reserve(2).unwrap(),
            encoder: None,
            track: None,
            shape: None,
            aspect: Some((1, 1)),
            stream_colour: ColorInfo::default(),
            spool: spool.handle().unwrap(),
            bytes: 0,
            deadline: Instant::now() + std::time::Duration::from_secs(5),
        };
        let frame = |pts, width| Frame {
            pts: Some(pts),
            color: ColorInfo::default(),
            pixels: coreav_frame::Picture::zeroed(width, 4, PixelFormat::YUV420P)
                .unwrap()
                .into(),
        };
        assert!(run.take(frame(40, 4)).is_err());
        assert_eq!(run.bytes, 0);
        run.take(frame(0, 4)).unwrap();
        assert!(run.take(frame(0, 4)).is_err());
        assert!(run.take(frame(40, 8)).is_err());
        run.take(frame(40, 4)).unwrap();
        assert!(run.take(frame(80, 4)).is_err());
        assert_eq!(run.packets.len(), 2);
        drop(run);
        let options = Options {
            max_spool_bytes: 1,
            ..options
        };
        let mut run = Run {
            options: &options,
            expected: &times,
            packets: reserve(2).unwrap(),
            offsets: reserve(2).unwrap(),
            encoder: None,
            track: None,
            shape: None,
            aspect: Some((1, 1)),
            stream_colour: ColorInfo::default(),
            spool: spool.handle().unwrap(),
            bytes: 0,
            deadline: Instant::now() + std::time::Duration::from_secs(5),
        };
        assert!(matches!(run.take(frame(0, 4)), Err(Error::Limit)));
        assert_eq!(run.packets.len(), 0);
        assert_eq!(run.bytes, 0);
    }

    #[test]
    fn late_frame_changes_are_terminal_without_publishing_another_packet() {
        let options = Options {
            progressive: true,
            pixel_format: Some(PixelFormat::YUV420P),
            ..Options::default()
        };
        let times = [
            Expected {
                pts: 0,
                time_ns: 0,
                duration_ns: Some(40_000_000),
            },
            Expected {
                pts: 40,
                time_ns: 40_000_000,
                duration_ns: None,
            },
        ];
        let original = ColorInfo {
            primaries: Some(1),
            ..ColorInfo::default()
        };
        let changed = ColorInfo {
            primaries: Some(9),
            ..original
        };
        let yuv444 = PixelFormat::Yuv {
            chroma: coreav_frame::Chroma::Cs444,
            depth: 8,
        };
        for (index, (width, format, color)) in [
            (8, PixelFormat::YUV420P, original),
            (4, yuv444, original),
            (4, PixelFormat::YUV420P, changed),
            (4, PixelFormat::YUV420P, ColorInfo::default()),
        ]
        .into_iter()
        .enumerate()
        {
            let path = std::env::temp_dir()
                .join(format!("coreav-ffv1-late-{}-{index}", std::process::id()));
            let spool = crate::pipeline::Spool::create(&path, "test").unwrap();
            let mut run = Run {
                options: &options,
                expected: &times,
                packets: reserve(2).unwrap(),
                offsets: reserve(2).unwrap(),
                encoder: None,
                track: None,
                shape: None,
                aspect: Some((1, 1)),
                stream_colour: original,
                spool: spool.handle().unwrap(),
                bytes: 0,
                deadline: Instant::now() + std::time::Duration::from_secs(5),
            };
            let frame = |pts, width, format, color| Frame {
                pts: Some(pts),
                color,
                pixels: coreav_frame::Picture::zeroed(width, 4, format)
                    .unwrap()
                    .into(),
            };
            assert_eq!(
                run.take(frame(0, 4, yuv444, original)).unwrap_err().kind(),
                coreav_decode::ErrorKind::Unsupported
            );
            assert_eq!(
                run.decode_error(coreav_decode::Error::new(
                    coreav_decode::ErrorKind::Unsupported,
                    "unsupported source"
                ))
                .kind(),
                coreav_decode::ErrorKind::Unsupported
            );
            run.take(frame(0, 4, PixelFormat::YUV420P, original))
                .unwrap();
            for kind in [
                coreav_decode::ErrorKind::Unsupported,
                coreav_decode::ErrorKind::CodecNotAllowed,
            ] {
                assert_eq!(
                    run.decode_error(coreav_decode::Error::new(kind, "changed source"))
                        .kind(),
                    coreav_decode::ErrorKind::InvalidData
                );
            }
            assert_eq!(
                run.decode_error(coreav_decode::Error::new(
                    coreav_decode::ErrorKind::Timeout,
                    "expired source"
                ))
                .kind(),
                coreav_decode::ErrorKind::Timeout
            );
            let before = run.bytes;
            let error = run.take(frame(40, width, format, color)).unwrap_err();
            assert_eq!(error.kind(), coreav_decode::ErrorKind::InvalidData);
            assert_eq!(run.packets.len(), 1);
            assert_eq!(run.bytes, before);
        }
    }

    #[test]
    fn rounding_is_exact_for_both_signs_and_extremes() {
        for (n, expected) in [(1, 1), (-1, 0), (3, 2), (-3, -1)] {
            assert_eq!(
                ns(Rational::new(n, 2_000_000_000).unwrap()).unwrap(),
                expected
            );
        }
        assert!(ns(Rational::from_int(i128::MAX)).is_err());
        assert_eq!(tick(-500_000).unwrap(), 0);
        assert_eq!(tick(-500_001).unwrap(), -1);
    }

    #[test]
    fn display_preserves_the_exact_ratio_and_refuses_overflow() {
        assert_eq!(display(720, 480, Some((4, 3))).unwrap(), Some((2880, 1440)));
        assert_eq!(display(720, 480, Some((2, 2))).unwrap(), None);
        assert!(display(720, 480, Some((u32::MAX, 1))).is_err());
        assert!(display(720, 480, Some((0, 1))).is_err());
    }
}
