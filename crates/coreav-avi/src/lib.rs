//! Packet-based AVI 1.0 output. See `docs/design/avi-write.md`.

use std::io::{self, Write};

/// The largest file this writer emits. Larger files need OpenDML.
pub const MAX_FILE_BYTES: u64 = 1 << 30;

/// An audio or video stream's format and fixed clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Track {
    /// H.264 with an AVC configuration record and length-prefixed samples.
    H264 {
        /// Coded width.
        width: u16,
        /// Coded height.
        height: u16,
        /// Numerator of seconds per frame.
        scale: u32,
        /// Denominator of seconds per frame.
        rate: u32,
        /// AVCDecoderConfigurationRecord.
        config: Vec<u8>,
    },
    /// Mono or stereo unsigned 8-bit or signed little-endian 16-bit PCM.
    Pcm {
        /// Samples per second.
        rate: u32,
        /// Channels in each sample frame.
        channels: u16,
        /// Bits per channel, 8 or 16.
        bits: u16,
    },
    /// MPEG audio Layer III, one complete frame in each packet.
    Mp3 {
        /// Samples per second.
        rate: u32,
        /// One or two channels.
        channels: u16,
        /// Samples per compressed frame, 576 or 1152.
        frame_samples: u32,
        /// Average compressed bytes per second.
        bytes_per_second: u32,
    },
}

/// One complete chunk. Times use its track's clock, starting at zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    /// Position among the output tracks.
    pub track: usize,
    /// Decode and presentation time. Reordering is not supported.
    pub time: u64,
    /// PCM sample frames, or one compressed audio/video frame.
    pub duration: u32,
    /// Payload bytes, excluding chunk headers and padding.
    pub size: u32,
    /// Whether this chunk is a random access point.
    pub keyframe: bool,
}

/// A layout or output error.
#[derive(Debug)]
pub enum Error {
    /// Metadata cannot be represented by this writer.
    Unsupported(&'static str),
    /// A bounded layout exceeded a resource limit.
    Limit(&'static str),
    /// Writing failed.
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(reason) | Self::Limit(reason) => f.write_str(reason),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Bounds on retained layout metadata and packet work.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Output streams, at most 100.
    pub max_tracks: usize,
    /// Media chunks.
    pub max_packets: usize,
    /// Bytes in one chunk.
    pub max_packet_bytes: u32,
    /// Bytes in stream configuration records.
    pub max_metadata_bytes: usize,
    /// Bytes retained for the index.
    pub max_index_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_tracks: 16,
            max_packets: 1_000_000,
            max_packet_bytes: 16 << 20,
            max_metadata_bytes: 1 << 20,
            max_index_bytes: 16 << 20,
        }
    }
}

/// A checked front-to-back layout. Media bytes remain with the caller.
#[derive(Debug)]
pub struct Plan<'a> {
    packets: &'a [Packet],
    head: Vec<u8>,
    index: Vec<u8>,
    bytes: u64,
}

fn invalid(reason: &'static str) -> Error {
    Error::Unsupported(reason)
}

fn narrow(n: u64) -> Result<u32, Error> {
    u32::try_from(n).map_err(|_| Error::Limit("AVI field exceeds 32 bits"))
}

fn word(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_le_bytes());
}

fn dword(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}

fn chunk(id: &[u8; 4], data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(data.len() + 9);
    out.extend_from_slice(id);
    dword(&mut out, narrow(data.len() as u64)?);
    out.extend_from_slice(data);
    if data.len() % 2 != 0 {
        out.push(0);
    }
    Ok(out)
}

fn list(kind: &[u8; 4], data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut payload = kind.to_vec();
    payload.extend_from_slice(data);
    chunk(b"LIST", &payload)
}

impl Track {
    fn clock(&self) -> (u32, u32) {
        match self {
            Self::H264 { scale, rate, .. } => (*scale, *rate),
            Self::Pcm { rate, .. } => (1, *rate),
            Self::Mp3 {
                rate,
                frame_samples,
                ..
            } => (*frame_samples, *rate),
        }
    }

    fn sample_size(&self) -> Result<u32, Error> {
        match self {
            Self::Pcm { channels, bits, .. } => {
                let bytes = u32::from(*channels) * u32::from(*bits / 8);
                if bytes == 0 || bytes > u32::from(u16::MAX) {
                    return Err(invalid("AVI PCM block alignment is invalid"));
                }
                Ok(bytes)
            }
            _ => Ok(0),
        }
    }

    fn format(&self, largest: u32) -> Result<Vec<u8>, Error> {
        let (scale, rate) = self.clock();
        if scale == 0 || rate == 0 {
            return Err(invalid("AVI stream clock must be positive"));
        }
        let mut out = Vec::new();
        match self {
            Self::H264 {
                width,
                height,
                config,
                ..
            } => {
                if *width == 0
                    || *height == 0
                    || *width > 32767
                    || *height > 32767
                    || config.len() < 7
                    || config.first() != Some(&1)
                {
                    return Err(invalid(
                        "AVI H.264 needs valid dimensions and an AVC configuration record",
                    ));
                }
                dword(&mut out, narrow(40 + config.len() as u64)?);
                dword(&mut out, u32::from(*width));
                dword(&mut out, u32::from(*height));
                word(&mut out, 1);
                word(&mut out, 24);
                out.extend_from_slice(b"H264");
                dword(&mut out, u32::from(*width) * u32::from(*height) * 3);
                out.extend_from_slice(&[0; 16]);
                out.extend_from_slice(config);
            }
            Self::Pcm {
                rate,
                channels,
                bits,
            } => {
                if !matches!(channels, 1 | 2) || !matches!(bits, 8 | 16) {
                    return Err(invalid(
                        "AVI PCM supports mono or stereo 8-bit unsigned or 16-bit signed audio",
                    ));
                }
                let align = self.sample_size()?;
                word(&mut out, 1);
                word(&mut out, *channels);
                dword(&mut out, *rate);
                dword(&mut out, narrow(u64::from(*rate) * u64::from(align))?);
                word(
                    &mut out,
                    u16::try_from(align).map_err(|_| invalid("AVI PCM alignment"))?,
                );
                word(&mut out, *bits);
                word(&mut out, 0);
            }
            Self::Mp3 {
                rate,
                channels,
                frame_samples,
                bytes_per_second,
            } => {
                if !matches!(channels, 1 | 2)
                    || !matches!(frame_samples, 576 | 1152)
                    || *bytes_per_second == 0
                {
                    return Err(invalid("AVI MP3 format is invalid"));
                }
                word(&mut out, 0x55);
                word(&mut out, *channels);
                dword(&mut out, *rate);
                dword(&mut out, *bytes_per_second);
                word(
                    &mut out,
                    u16::try_from(*frame_samples).map_err(|_| invalid("AVI MP3 frame size"))?,
                );
                word(&mut out, 0);
                word(&mut out, 12);
                word(&mut out, 1);
                dword(&mut out, 0);
                word(
                    &mut out,
                    u16::try_from(largest).map_err(|_| invalid("AVI MP3 block exceeds 16 bits"))?,
                );
                word(&mut out, 1);
                word(&mut out, 0);
            }
        }
        Ok(out)
    }
}

fn chunk_id(track: usize, video: bool) -> Result<[u8; 4], Error> {
    if track >= 100 {
        return Err(invalid("AVI stream numbers exceed two decimal digits"));
    }
    Ok([
        b'0' + (track / 10) as u8,
        b'0' + (track % 10) as u8,
        if video { b'd' } else { b'w' },
        if video { b'c' } else { b'b' },
    ])
}

/// Validates clocks, lengths and budgets before a byte of output is written.
/// Packets remain in caller order, with contiguous times on each track.
pub fn plan<'a>(
    tracks: &[Track],
    packets: &'a [Packet],
    limits: &Limits,
) -> Result<Plan<'a>, Error> {
    if tracks.is_empty() || packets.is_empty() {
        return Err(invalid("AVI needs streams and packets"));
    }
    if tracks.len() > limits.max_tracks.min(100)
        || packets.len() > limits.max_packets
        || packets
            .len()
            .checked_mul(16)
            .is_none_or(|n| n > limits.max_index_bytes)
    {
        return Err(Error::Limit("AVI stream, packet or index budget exceeded"));
    }
    let metadata = tracks
        .iter()
        .try_fold(0usize, |sum, track| {
            sum.checked_add(match track {
                Track::H264 { config, .. } => config.len(),
                _ => 0,
            })
        })
        .ok_or(Error::Limit("AVI metadata size overflows"))?;
    if metadata > limits.max_metadata_bytes {
        return Err(Error::Limit("AVI metadata budget exceeded"));
    }
    for track in tracks {
        track.format(1)?;
    }
    let mut lengths = vec![0u64; tracks.len()];
    let mut largest = vec![0u32; tracks.len()];
    let mut index = Vec::with_capacity(packets.len() * 16);
    let mut movi_size = 4u64;
    for packet in packets {
        let track = tracks
            .get(packet.track)
            .ok_or_else(|| invalid("AVI packet stream is out of range"))?;
        let length = lengths
            .get_mut(packet.track)
            .ok_or_else(|| invalid("AVI packet stream is out of range"))?;
        if packet.time != *length || packet.duration == 0 || packet.size == 0 {
            return Err(invalid(
                "AVI needs nonempty packets on a contiguous zero-based timeline",
            ));
        }
        let align = track.sample_size()?;
        if (align == 0 && packet.duration != 1)
            || (align != 0
                && u64::from(packet.duration) * u64::from(align) != u64::from(packet.size))
        {
            return Err(invalid(
                "AVI packet size or duration does not match its stream format",
            ));
        }
        if packet.size > limits.max_packet_bytes {
            return Err(Error::Limit("AVI packet budget exceeded"));
        }
        *length = length
            .checked_add(u64::from(packet.duration))
            .ok_or(Error::Limit("AVI duration overflows"))?;
        if let Some(max) = largest.get_mut(packet.track) {
            *max = (*max).max(packet.size);
        }
        index.extend_from_slice(&chunk_id(
            packet.track,
            matches!(track, Track::H264 { .. }),
        )?);
        dword(&mut index, if packet.keyframe { 0x10 } else { 0 });
        dword(&mut index, narrow(movi_size)?);
        dword(&mut index, packet.size);
        movi_size = movi_size
            .checked_add(8 + u64::from(packet.size) + u64::from(packet.size % 2))
            .ok_or(Error::Limit("AVI size overflows"))?;
        if movi_size >= MAX_FILE_BYTES {
            return Err(Error::Limit(
                "AVI output at or above 1 GiB needs unsupported OpenDML",
            ));
        }
    }
    if lengths.contains(&0) {
        return Err(invalid("AVI cannot write an empty stream"));
    }
    let video = tracks
        .iter()
        .enumerate()
        .find_map(|(i, track)| match track {
            Track::H264 {
                width,
                height,
                scale,
                rate,
                ..
            } => Some((i, *width, *height, *scale, *rate)),
            _ => None,
        });
    let mut main = Vec::new();
    let (width, height, microseconds, frames) = match video {
        Some((i, w, h, scale, rate)) => (
            w,
            h,
            narrow((u64::from(scale) * 1_000_000 + u64::from(rate) / 2) / u64::from(rate))?,
            narrow(*lengths.get(i).ok_or_else(|| invalid("AVI video stream"))?)?,
        ),
        None => (0, 0, 0, 0),
    };
    for n in [
        microseconds,
        0,
        0,
        0x10,
        frames,
        0,
        narrow(tracks.len() as u64)?,
        largest.iter().copied().max().unwrap_or(0),
        u32::from(width),
        u32::from(height),
        0,
        0,
        0,
        0,
    ] {
        dword(&mut main, n);
    }
    let mut hdrl = chunk(b"avih", &main)?;
    for (i, track) in tracks.iter().enumerate() {
        let (scale, rate) = track.clock();
        let mut header = Vec::new();
        let is_video = matches!(track, Track::H264 { .. });
        header.extend_from_slice(if is_video { b"vids" } else { b"auds" });
        header.extend_from_slice(if is_video { b"H264" } else { &[0; 4] });
        for n in [
            0,
            0,
            0,
            scale,
            rate,
            0,
            narrow(*lengths.get(i).ok_or_else(|| invalid("AVI stream length"))?)?,
            *largest.get(i).ok_or_else(|| invalid("AVI buffer size"))?,
            u32::MAX,
            track.sample_size()?,
        ] {
            dword(&mut header, n);
        }
        for n in match track {
            Track::H264 { width, height, .. } => [0, 0, *width, *height],
            _ => [0; 4],
        } {
            word(&mut header, n);
        }
        let mut stream = chunk(b"strh", &header)?;
        stream.extend(chunk(
            b"strf",
            &track.format(*largest.get(i).ok_or_else(|| invalid("AVI buffer size"))?)?,
        )?);
        hdrl.extend(list(b"strl", &stream)?);
    }
    let hdrl = list(b"hdrl", &hdrl)?;
    let index = chunk(b"idx1", &index)?;
    let bytes = 12 + hdrl.len() as u64 + 8 + movi_size + index.len() as u64;
    if bytes >= MAX_FILE_BYTES {
        return Err(Error::Limit(
            "AVI output at or above 1 GiB needs unsupported OpenDML",
        ));
    }
    let mut head = b"RIFF".to_vec();
    dword(&mut head, narrow(bytes - 8)?);
    head.extend_from_slice(b"AVI ");
    head.extend(hdrl);
    head.extend_from_slice(b"LIST");
    dword(&mut head, narrow(movi_size)?);
    head.extend_from_slice(b"movi");
    Ok(Plan {
        packets,
        head,
        index,
        bytes,
    })
}

impl Plan<'_> {
    /// Total output bytes, including padding and the index.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Copies each packet exactly once, checking the callback's byte count.
    pub fn write(
        &self,
        mut out: impl Write,
        mut copy: impl FnMut(usize, &mut dyn Write) -> io::Result<()>,
    ) -> Result<u64, Error> {
        out.write_all(&self.head)?;
        for (i, packet) in self.packets.iter().enumerate() {
            let entry = self
                .index
                .get(8 + i * 16..12 + i * 16)
                .ok_or_else(|| invalid("AVI index entry is missing"))?;
            out.write_all(entry)?;
            out.write_all(&packet.size.to_le_bytes())?;
            let mut bounded = Payload {
                out: &mut out,
                left: u64::from(packet.size),
            };
            copy(i, &mut bounded)?;
            if bounded.left != 0 {
                return Err(invalid(
                    "AVI payload callback wrote fewer bytes than planned",
                ));
            }
            if packet.size % 2 != 0 {
                out.write_all(&[0])?;
            }
        }
        out.write_all(&self.index)?;
        Ok(self.bytes)
    }
}

struct Payload<'a> {
    out: &'a mut dyn Write,
    left: u64,
}

impl Write for Payload<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.left {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "AVI payload exceeds planned size",
            ));
        }
        let written = self.out.write(bytes)?;
        self.left -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}
