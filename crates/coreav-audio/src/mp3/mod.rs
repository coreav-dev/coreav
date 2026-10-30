//! MPEG audio Layer III decoding (MP3), written from ISO/IEC 11172-3,
//! ISO/IEC 13818-3 (the lower sampling frequencies) and Fraunhofer IIS's
//! "MPEG-Layer3 Bitstream Syntax and Decoding" (the tables and MPEG 2.5),
//! as recorded in `docs/design/audio-decode.md` ("MP3").
//!
//! [`Mp3Decoder`] takes one frame per packet, header included, and writes
//! its 1,152 (MPEG-1) or 576 (MPEG-2 and 2.5) samples per channel as
//! floating point. The bit reservoir keeps the last 511 bytes of main data,
//! all that `main_data_begin` can reach. The steps are the standard's:
//! side information, scalefactors, Huffman decoding, requantization,
//! reordering of short blocks, MS and intensity stereo, alias reduction,
//! IMDCT with its windows and overlap, frequency inversion, and the
//! polyphase synthesis filterbank.

mod dct32;
pub(crate) mod frames;
mod huffman;
mod imdct;
mod lanes;
mod layer3;
mod synth;
mod tables;
mod trig;

use crate::limits::check_channels;
use crate::{Error, Limits, Pcm};
use layer3::{Channel, Granule};
use synth::Synthesis;

/// The MPEG audio version of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    /// ISO/IEC 11172-3: 32, 44.1 and 48 kHz.
    Mpeg1,
    /// ISO/IEC 13818-3 lower sampling frequencies: 16, 22.05 and 24 kHz.
    Mpeg2,
    /// The non-ISO extension to 8, 11.025 and 12 kHz.
    Mpeg25,
}

/// The channel mode of a frame (ISO/IEC 11172-3 2.4.2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Two channels.
    Stereo,
    /// Two channels with MS and/or intensity stereo.
    JointStereo,
    /// Two independent channels.
    DualChannel,
    /// One channel.
    Mono,
}

/// A Layer III frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// The version.
    pub version: Version,
    /// Whether a CRC follows the header.
    pub protected: bool,
    /// Bit rate in bits per second; 0 for free format.
    pub bit_rate: u32,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// The padding bit.
    pub padding: bool,
    /// The channel mode.
    pub mode: Mode,
    /// Intensity stereo on (joint stereo only).
    pub intensity: bool,
    /// MS stereo on (joint stereo only).
    pub ms: bool,
}

impl Header {
    /// Reads the 4-byte header at the start of `data`.
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let bytes = data.get(..4).ok_or(Error::Truncated)?;
        let word = u32::from_be_bytes(<[u8; 4]>::try_from(bytes).map_err(|_| Error::Truncated)?);
        if word >> 21 != 0x7FF {
            return Err(Error::Invalid("no MPEG audio sync word"));
        }
        let version = match (word >> 19) & 3 {
            3 => Version::Mpeg1,
            2 => Version::Mpeg2,
            0 => Version::Mpeg25,
            _ => return Err(Error::Invalid("reserved MPEG audio version")),
        };
        match (word >> 17) & 3 {
            1 => {}
            0 => return Err(Error::Invalid("reserved MPEG audio layer")),
            _ => return Err(Error::Unsupported("MPEG audio Layer I or II")),
        }
        let protected = (word >> 16) & 1 == 0;
        let rate_index = ((word >> 10) & 3) as usize;
        let rates: [u32; 3] = match version {
            Version::Mpeg1 => [44_100, 48_000, 32_000],
            Version::Mpeg2 => [22_050, 24_000, 16_000],
            Version::Mpeg25 => [11_025, 12_000, 8_000],
        };
        let sample_rate = *rates
            .get(rate_index)
            .ok_or(Error::Invalid("reserved sample rate"))?;
        let bit_index = ((word >> 12) & 0xF) as usize;
        let kbps: [u32; 15] = match version {
            Version::Mpeg1 => [
                0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
            ],
            _ => [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
        };
        let bit_rate = kbps
            .get(bit_index)
            .ok_or(Error::Invalid("forbidden bit rate"))?
            * 1000;
        let padding = (word >> 9) & 1 == 1;
        let mode = match (word >> 6) & 3 {
            0 => Mode::Stereo,
            1 => Mode::JointStereo,
            2 => Mode::DualChannel,
            _ => Mode::Mono,
        };
        let extension = (word >> 4) & 3;
        let joint = mode == Mode::JointStereo;
        Ok(Self {
            version,
            protected,
            bit_rate,
            sample_rate,
            padding,
            mode,
            intensity: joint && extension & 1 == 1,
            ms: joint && extension & 2 == 2,
        })
    }

    /// Channels.
    pub fn channels(&self) -> usize {
        if self.mode == Mode::Mono { 1 } else { 2 }
    }

    /// Granules: 2 for MPEG-1, 1 otherwise.
    pub fn granules(&self) -> usize {
        if self.version == Version::Mpeg1 { 2 } else { 1 }
    }

    /// Samples per channel: 1,152 for MPEG-1, 576 otherwise.
    pub fn samples(&self) -> usize {
        self.granules() * 576
    }

    /// Bytes of the frame from its header, `None` for free format.
    pub fn frame_bytes(&self) -> Option<usize> {
        if self.bit_rate == 0 {
            return None;
        }
        let per: u64 = if self.version == Version::Mpeg1 {
            144
        } else {
            72
        };
        let bytes = per * u64::from(self.bit_rate) / u64::from(self.sample_rate);
        usize::try_from(bytes)
            .ok()
            .map(|b| b + usize::from(self.padding))
    }

    /// Bytes of side information (ISO/IEC 11172-3 2.4.1.7, 13818-3 2.4.1.7).
    fn side_bytes(&self) -> usize {
        match (self.version == Version::Mpeg1, self.channels()) {
            (true, 1) => 17,
            (true, _) => 32,
            (false, 1) => 9,
            (false, _) => 17,
        }
    }
}

/// What [`Mp3Decoder::decode`] read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The frame's header.
    pub header: Header,
    /// Bytes of the frame: from its header when the header gives them
    /// (else all of `data`).
    pub bytes: usize,
    /// Whether the frame's main data could be read: false when
    /// `main_data_begin` reaches before the data the decoder has seen (the
    /// first frames after a cut), in which case the granules whose data
    /// starts there are silent.
    pub complete: bool,
    /// Whether the CRC matched, when the frame has one.
    pub crc_ok: bool,
}

/// Bytes of main data kept for the next frames: all that a 9-bit
/// `main_data_begin` reaches.
pub(crate) const RESERVOIR: usize = 511;

/// A Layer III decoder for one stream.
#[derive(Clone)]
pub struct Mp3Decoder {
    /// The output's channel count.
    channels: Option<usize>,
    /// [`Limits::max_channels`], for a stream whose channels are its
    /// first frame's.
    max_channels: u32,
    /// Frame samples before they are mapped to another channel count.
    scratch: Vec<f32>,
    sample_rate: Option<u32>,
    reservoir: Vec<u8>,
    /// The bytes at the end of `reservoir` after the last granule's data:
    /// all a later frame's main data may start in.
    unused: usize,
    main: Vec<u8>,
    state: [Channel; 2],
    synthesis: [Synthesis; 2],
    granule: Granule,
}

impl Mp3Decoder {
    /// A decoder. MP3 has no codec configuration. The output has
    /// `channels` channels (1 or 2) when the container states them, else
    /// those of the first frame; a frame with another count (the mode may
    /// change from frame to frame) is mapped to it: one channel is copied
    /// to both, two are averaged into one. The sample rate of the first
    /// frame must not change. The output's channels are checked against
    /// `limits.max_channels`: here when they are given, else at the first
    /// frame.
    pub fn new(channels: Option<u16>, limits: &Limits) -> Result<Self, Error> {
        let channels = match channels {
            None => None,
            Some(n @ 1..=2) => {
                check_channels(usize::from(n), limits.max_channels)?;
                Some(usize::from(n))
            }
            Some(_) => return Err(Error::Invalid("MP3 has one or two channels")),
        };
        Ok(Self {
            channels,
            max_channels: limits.max_channels,
            scratch: Vec::new(),
            sample_rate: None,
            reservoir: Vec::with_capacity(RESERVOIR + 2048),
            unused: 0,
            main: Vec::with_capacity(RESERVOIR + 2048),
            state: [Channel::default(), Channel::default()],
            synthesis: [Synthesis::default(), Synthesis::default()],
            granule: Granule::default(),
        })
    }

    /// Expects every frame at `rate` Hz, as if a frame at that rate had
    /// been decoded.
    pub(crate) fn expect_sample_rate(&mut self, rate: u32) {
        self.sample_rate = Some(rate);
    }

    /// Forgets the frames decoded so far (the bit reservoir and the filter
    /// banks), keeping the stream's format: the state of a decoder that
    /// starts in the middle of the stream.
    pub(crate) fn reset(&mut self) {
        self.reservoir.clear();
        self.unused = 0;
        self.state = [Channel::default(), Channel::default()];
        self.synthesis = [Synthesis::default(), Synthesis::default()];
    }

    /// Decodes the frame at the start of `data` into `out`, as floating
    /// point.
    pub fn decode(&mut self, data: &[u8], out: &mut Pcm) -> Result<Frame, Error> {
        let header = Header::parse(data)?;
        let channels = header.channels();
        if self.sample_rate.is_some_and(|r| r != header.sample_rate) {
            return Err(Error::Unsupported(
                "an MP3 frame with another sample rate than the stream's",
            ));
        }
        let output_channels = match self.channels {
            Some(n) => n,
            None => {
                check_channels(channels, self.max_channels)?;
                *self.channels.insert(channels)
            }
        };
        self.sample_rate = Some(header.sample_rate);
        let frame_bytes = header
            .frame_bytes()
            .map_or(data.len(), |n| n.min(data.len()));
        let frame = data.get(..frame_bytes).ok_or(Error::Truncated)?;
        let crc_bytes = if header.protected { 2 } else { 0 };
        let side_start = 4 + crc_bytes;
        let side_end = side_start + header.side_bytes();
        let side = frame.get(side_start..side_end).ok_or(Error::Truncated)?;
        let crc_ok = !header.protected || {
            let stored = frame.get(4..6).map_or(0, |b| {
                u16::from_be_bytes([
                    b.first().copied().unwrap_or(0),
                    b.get(1).copied().unwrap_or(0),
                ])
            });
            let mut crc = crc16_mpeg(0xFFFF, frame.get(2..4).unwrap_or_default());
            crc = crc16_mpeg(crc, side);
            crc == stored
        };
        let info = layer3::SideInfo::parse(side, &header)?;
        let frame_main = frame.get(side_end..).unwrap_or_default();
        // The main data of this frame starts `main_data_begin` bytes back in
        // the reservoir. Bytes the decoder has not seen (the first frames
        // of a cut stream), and bytes that hold earlier granules' data (a
        // stream cut and joined, whose frame reaches back past what the
        // frames before it left), are zeros, and the granules whose data
        // starts in them are silent.
        let begin = usize::from(info.main_data_begin);
        // The bytes of the reservoir the frame may use: the last `available`;
        // the `missing` before them are replaced by zeros, in place, so the
        // granules after them keep their offsets.
        let available = begin.min(self.reservoir.len()).min(self.unused);
        let missing = begin - available;
        let complete = missing == 0;
        let samples = header.samples();
        let planes = out.float_buffer(channels, samples);
        self.main.clear();
        self.main.resize(missing, 0);
        let tail = self.reservoir.len().saturating_sub(available);
        self.main
            .extend_from_slice(self.reservoir.get(tail..).unwrap_or_default());
        self.main.extend_from_slice(frame_main);
        layer3::decode_frame(
            &header,
            &info,
            &self.main,
            missing * 8,
            &mut self.state,
            &mut self.synthesis,
            &mut self.granule,
            planes,
            samples,
        )?;
        if channels != output_channels {
            // Map the frame's channels to the stream's.
            self.scratch.clear();
            self.scratch.extend_from_slice(planes);
            let target = out.float_buffer(output_channels, samples);
            let (first, second) = self.scratch.split_at(samples.min(self.scratch.len()));
            if channels == 1 {
                for plane in target.chunks_exact_mut(samples.max(1)) {
                    for (t, &v) in plane.iter_mut().zip(first) {
                        *t = v;
                    }
                }
            } else {
                for ((t, &a), &b) in target.iter_mut().zip(first).zip(second) {
                    *t = 0.5 * (a + b);
                }
            }
        }
        // What the granules left of the main data, then the last RESERVOIR
        // bytes of main data seen.
        let used = info
            .granules
            .iter()
            .take(header.granules())
            .flat_map(|granule| granule.iter().take(channels))
            .map(|gi| gi.part2_3_length as usize)
            .sum::<usize>()
            .div_ceil(8);
        self.unused = self.main.len().saturating_sub(used);
        self.reservoir.extend_from_slice(frame_main);
        if self.reservoir.len() > RESERVOIR {
            let drop = self.reservoir.len() - RESERVOIR;
            self.reservoir.drain(..drop);
        }
        Ok(Frame {
            header,
            bytes: frame_bytes,
            complete,
            crc_ok,
        })
    }
}

/// The main data bytes of the frame at the start of `packet`: what it adds
/// to the bit reservoir. 0 when it has no valid header.
pub(crate) fn main_bytes(packet: &[u8]) -> usize {
    let Ok(header) = Header::parse(packet) else {
        return 0;
    };
    let frame = header
        .frame_bytes()
        .map_or(packet.len(), |n| n.min(packet.len()));
    let crc = if header.protected { 2 } else { 0 };
    frame.saturating_sub(4 + crc + header.side_bytes())
}

/// The CRC-16 of MPEG audio (ISO/IEC 11172-3 2.4.3.1): polynomial
/// x^16 + x^15 + x^2 + 1, most significant bit first, from `crc`.
fn crc16_mpeg(mut crc: u16, data: &[u8]) -> u16 {
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}
