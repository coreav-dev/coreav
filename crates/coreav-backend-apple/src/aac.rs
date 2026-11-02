//! AAC-LC output metadata, from ISO/IEC 14496-3 AudioSpecificConfig and Apple's
//! AudioStreamPacketDescription. No decoder or codec implementation is linked.

use std::ops::Range;

/// Sample frames per channel in the supported 1024-frame AAC-LC configuration.
pub const FRAMES_PER_PACKET: u32 = 1024;

const SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// A refusal to accept encoder output metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The configuration is not the supported two-byte AAC-LC configuration.
    Configuration,
    /// The encoder changed the requested rate or channel configuration.
    FormatMismatch,
    /// A packet has an invalid extent, frame count, or ordering.
    PacketDescription,
    /// Output exceeds a caller-supplied bound.
    Limit,
    /// Rust-owned output storage could not be reserved.
    Allocation,
    /// Declared priming and padding do not account for the submitted samples.
    DrainCount,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Configuration => "unsupported AAC output configuration",
            Self::FormatMismatch => "AAC output does not match the requested format",
            Self::PacketDescription => "invalid AAC output packet description",
            Self::Limit => "AAC output exceeds its configured limit",
            Self::Allocation => "cannot reserve AAC output storage",
            Self::DrainCount => "AAC drain counts do not match the submitted samples",
        })
    }
}

impl std::error::Error for Error {}

/// A checked AAC-LC AudioSpecificConfig, with mono or stereo channel order.
///
/// Only indexed sample rates and 1024-sample frames are supported. Program
/// configurations, explicit frequencies, core-coder dependencies, and extensions
/// are refused instead of being ignored or interpreted as AAC-LC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    bytes: [u8; 2],
    sample_rate: u32,
    channels: u16,
}

impl Config {
    /// Constructs the supported configuration for an unchanged PCM format.
    pub fn for_pcm(sample_rate: u32, channels: u16) -> Result<Self, Error> {
        if !matches!(channels, 1 | 2) {
            return Err(Error::Configuration);
        }
        let index = SAMPLE_RATES
            .iter()
            .position(|&rate| rate == sample_rate)
            .and_then(|index| u16::try_from(index).ok())
            .ok_or(Error::Configuration)?;
        let bytes = ((2 << 11) | (index << 7) | (channels << 3)).to_be_bytes();
        Self::parse(&bytes, sample_rate, channels)
    }

    /// Reads an AAC elementary-stream descriptor cookie or a supported raw ASC.
    ///
    /// The only accepted optional ASC suffix explicitly signals absent SBR.
    /// Unknown descriptors and suffixes are refused, not searched or skipped.
    pub fn from_cookie(data: &[u8], sample_rate: u32, channels: u16) -> Result<Self, Error> {
        let asc = match data {
            [_, _] | [_, _, 0x56, 0xe5, 0] => data,
            _ => {
                let (tag, es, rest) = descriptor(data)?;
                if tag != 3 || !rest.is_empty() {
                    return Err(Error::Configuration);
                }
                let [_, _, 0, children @ ..] = es else {
                    return Err(Error::Configuration);
                };
                let (tag, decoder, sl) = descriptor(children)?;
                if tag != 4 {
                    return Err(Error::Configuration);
                }
                let (sl_tag, sl_data, rest) = descriptor(sl)?;
                if sl_tag != 6 || sl_data != [2] || !rest.is_empty() {
                    return Err(Error::Configuration);
                }
                let [0x40, 0x14 | 0x15, ..] = decoder else {
                    return Err(Error::Configuration);
                };
                let (tag, asc, rest) = descriptor(decoder.get(13..).ok_or(Error::Configuration)?)?;
                if tag != 5 || !rest.is_empty() {
                    return Err(Error::Configuration);
                }
                asc
            }
        };
        let [first, second] = asc else {
            if let [first, second, 0x56, 0xe5, 0] = asc {
                return Self::parse(&[*first, *second], sample_rate, channels);
            }
            return Err(Error::Configuration);
        };
        Self::parse(&[*first, *second], sample_rate, channels)
    }

    /// Checks the encoder's configuration against the requested PCM format.
    ///
    /// `data` is the AudioSpecificConfig itself, not an AudioToolbox magic cookie
    /// or an MPEG-4 descriptor containing it.
    pub fn parse(data: &[u8], sample_rate: u32, channels: u16) -> Result<Self, Error> {
        let &[first, second] = data else {
            return Err(Error::Configuration);
        };
        let word = u16::from_be_bytes([first, second]);
        if word >> 11 != 2 || word & 7 != 0 {
            return Err(Error::Configuration);
        }
        let rate = SAMPLE_RATES
            .get(usize::from((word >> 7) & 15))
            .copied()
            .ok_or(Error::Configuration)?;
        let channel_configuration = (word >> 3) & 15;
        if !matches!(channel_configuration, 1 | 2) {
            return Err(Error::Configuration);
        }
        if rate != sample_rate || channel_configuration != channels {
            return Err(Error::FormatMismatch);
        }
        Ok(Self {
            bytes: [first, second],
            sample_rate: rate,
            channels: channel_configuration,
        })
    }

    /// Returns the checked AudioSpecificConfig for a container sample entry.
    pub fn as_bytes(&self) -> &[u8; 2] {
        &self.bytes
    }

    /// Returns the sample rate in sample frames per second.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns one for mono or two for left-right stereo.
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

fn descriptor(data: &[u8]) -> Result<(u8, &[u8], &[u8]), Error> {
    let (&tag, mut data) = data.split_first().ok_or(Error::Configuration)?;
    let mut size = 0_usize;
    for _ in 0..4 {
        let (&byte, rest) = data.split_first().ok_or(Error::Configuration)?;
        data = rest;
        size = size
            .checked_mul(128)
            .and_then(|n| n.checked_add(usize::from(byte & 127)))
            .ok_or(Error::Configuration)?;
        if byte & 128 == 0 {
            return Ok((
                tag,
                data.get(..size).ok_or(Error::Configuration)?,
                data.get(size..).ok_or(Error::Configuration)?,
            ));
        }
    }
    Err(Error::Configuration)
}

/// Untrusted values copied from one AudioStreamPacketDescription.
///
/// This is not an FFI type and must not be cast to an Apple structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketDescription {
    /// Signed byte offset from the start of the returned buffer.
    pub start_offset: i64,
    /// Byte extent of this packet.
    pub byte_size: u32,
    /// Zero for the format's fixed frame count, or an explicit frame count.
    pub variable_frames: u32,
}

/// Bounds checked before allocating or copying an output batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchLimits {
    /// Maximum length of the returned buffer, including any gaps.
    pub max_buffer_bytes: usize,
    /// Maximum byte length of any single packet.
    pub max_packet_bytes: usize,
    /// Maximum number of packet descriptions in the batch.
    pub max_packets: usize,
}

/// Rust-owned packet storage with checked, nonoverlapping packet extents.
///
/// This checks packetization, not compressed AAC syntax. A successful batch is
/// not evidence of a successful encode or drain. The caller must bound the
/// number of batches and total output separately.
#[derive(Debug)]
pub struct PacketBatch {
    packets: Vec<Vec<u8>>,
    nominal_frames: u64,
}

impl PacketBatch {
    /// Checks all descriptions before copying any framework bytes.
    ///
    /// `data` and `descriptions` contain only the returned written bytes and
    /// packet descriptions, not the unused capacity of the output buffers.
    ///
    /// Descriptions must be in buffer order. Gaps are permitted by Apple's
    /// packet description contract. Empty packets, overlaps, negative offsets,
    /// and packets outside `data` are refused. An empty batch has no bytes.
    pub fn copy_from(
        data: &[u8],
        descriptions: &[PacketDescription],
        limits: BatchLimits,
    ) -> Result<Self, Error> {
        if data.len() > limits.max_buffer_bytes || descriptions.len() > limits.max_packets {
            return Err(Error::Limit);
        }
        if descriptions.is_empty() && !data.is_empty() {
            return Err(Error::PacketDescription);
        }
        let mut previous_end = 0;
        for description in descriptions {
            let range = packet_range(description, data.len(), limits.max_packet_bytes)?;
            if range.start < previous_end {
                return Err(Error::PacketDescription);
            }
            previous_end = range.end;
        }
        let nominal_frames = u64::try_from(descriptions.len())
            .ok()
            .and_then(|count| count.checked_mul(u64::from(FRAMES_PER_PACKET)))
            .ok_or(Error::DrainCount)?;
        let mut packets = Vec::new();
        packets
            .try_reserve_exact(descriptions.len())
            .map_err(|_| Error::Allocation)?;
        for description in descriptions {
            let range = packet_range(description, data.len(), limits.max_packet_bytes)?;
            let packet = data.get(range).ok_or(Error::PacketDescription)?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(packet.len())
                .map_err(|_| Error::Allocation)?;
            bytes.extend_from_slice(packet);
            packets.push(bytes);
        }
        Ok(Self {
            packets,
            nominal_frames,
        })
    }

    /// Iterates over packet bytes without exposing gaps in the buffer.
    pub fn packets(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.packets.iter().map(Vec::as_slice)
    }

    /// Returns the nominal sample frames per channel, without decoding or trimming.
    pub fn nominal_frames(&self) -> u64 {
        self.nominal_frames
    }
}

fn packet_range(
    description: &PacketDescription,
    buffer_len: usize,
    max_packet_bytes: usize,
) -> Result<Range<usize>, Error> {
    if !matches!(description.variable_frames, 0 | FRAMES_PER_PACKET) || description.byte_size == 0 {
        return Err(Error::PacketDescription);
    }
    let start = usize::try_from(description.start_offset).map_err(|_| Error::PacketDescription)?;
    let size = usize::try_from(description.byte_size).map_err(|_| Error::PacketDescription)?;
    if size > max_packet_bytes {
        return Err(Error::Limit);
    }
    let end = start.checked_add(size).ok_or(Error::PacketDescription)?;
    if end > buffer_len {
        return Err(Error::PacketDescription);
    }
    Ok(start..end)
}

/// Checked leading and trailing trim in sample frames per channel.
///
/// These counts describe the encoded stream, not the converter's input lookahead
/// requirements. Independent decoding must still verify the declared trim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trim {
    leading_frames: u32,
    trailing_frames: u32,
}

impl Trim {
    /// Requires the declared trim to account exactly for the submitted PCM.
    ///
    /// `packet_count` is the cumulative count of checked packets after drain.
    /// Both trim counts must fit `max_trim_frames`, supplied by the caller.
    pub fn validate(
        packet_count: u64,
        submitted_frames: u64,
        leading_frames: u32,
        trailing_frames: u32,
        max_trim_frames: u32,
    ) -> Result<Self, Error> {
        if leading_frames > max_trim_frames || trailing_frames > max_trim_frames {
            return Err(Error::Limit);
        }
        let valid_frames = packet_count
            .checked_mul(u64::from(FRAMES_PER_PACKET))
            .and_then(|frames| frames.checked_sub(u64::from(leading_frames)))
            .and_then(|frames| frames.checked_sub(u64::from(trailing_frames)))
            .ok_or(Error::DrainCount)?;
        if valid_frames != submitted_frames {
            return Err(Error::DrainCount);
        }
        Ok(Self {
            leading_frames,
            trailing_frames,
        })
    }

    /// Returns the number of leading decoded sample frames to discard.
    pub fn leading_frames(&self) -> u32 {
        self.leading_frames
    }

    /// Returns the number of trailing decoded sample frames to discard.
    pub fn trailing_frames(&self) -> u32 {
        self.trailing_frames
    }
}
