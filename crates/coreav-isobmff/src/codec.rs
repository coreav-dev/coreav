//! Sample descriptions: codec identification, configuration and the allowlist.

use crate::bitstream;
use crate::bytes::{Bits, Bytes};
use crate::error::Warning;
use crate::size;
use crate::{Error, FourCc};

/// A codec, identified from a sample description.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    /// ITU-T H.264 (`avc1`, `avc3`).
    H264,
    /// ITU-T H.265 (`hvc1`, `hev1`).
    Hevc,
    /// AV1 (`av01`).
    Av1,
    /// VP9 (`vp09`).
    Vp9,
    /// Apple ProRes (SMPTE RDD 36).
    ProRes,
    /// MPEG-4 AAC (`mp4a`, object types 0x40, 0x66, 0x67, 0x68).
    Aac,
    /// MPEG-1/2 Layer III (`mp4a`, object types 0x69, 0x6B; QuickTime's
    /// `.mp3`).
    Mp3,
    /// Opus (`Opus`).
    Opus,
    /// FLAC (`fLaC`, configured by `dfLa`; the FLAC-in-ISOBMFF mapping of
    /// RFC 9639 10.3).
    Flac,
    /// Apple Lossless (`alac`, configured by its `alac` box holding the
    /// magic cookie).
    Alac,
    /// AC-3 (`ac-3`, configured by `dac3`; ETSI TS 102 366 Annex F).
    Ac3,
    /// Enhanced AC-3 (`ec-3`, configured by `dec3`; ETSI TS 102 366
    /// Annex F).
    Eac3,
    /// Uncompressed linear PCM.
    Pcm,
    /// 3GPP timed text (`tx3g`).
    TimedText,
    /// QuickTime text (`text`), the sample entry of QuickTime chapter
    /// tracks. Not on the default allowlist: its samples are read only by
    /// callers that ask for it, such as chapter extraction.
    QuickTimeText,
    /// QuickTime timecode (`tmcd`).
    Timecode,
    /// Anything else; never on the default allowlist.
    Other,
}

/// The set of codecs a caller accepts, and what happens to a stream outside
/// it.
///
/// By default a stream whose codec is not allowed is dropped: it is left out
/// of [`crate::Movie::tracks`], its samples are never exposed, and it is
/// reported in [`crate::Movie::dropped`]. In strict mode the whole file is
/// rejected with [`crate::Error::CodecNotAllowed`] instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowlist {
    codecs: Vec<Codec>,
    strict: bool,
    /// Every stream is kept, unidentified codecs included.
    listing: bool,
}

impl Allowlist {
    /// An allowlist of exactly these codecs, dropping other streams.
    pub fn new(codecs: &[Codec]) -> Self {
        Self {
            codecs: codecs
                .iter()
                .copied()
                .filter(|c| *c != Codec::Other)
                .collect(),
            strict: false,
            listing: false,
        }
    }

    /// Keeps every stream, those with a codec this crate does not identify
    /// ([`Codec::Other`]) included, for a caller that only lists streams and
    /// packets (`coreav ffprobe`). Their samples are listed like any
    /// others; nothing here reads their coded data.
    pub fn listing() -> Self {
        Self {
            codecs: Vec::new(),
            strict: false,
            listing: true,
        }
    }

    /// The same allowlist in strict mode: a stream with a codec outside it
    /// rejects the whole file.
    #[must_use]
    pub fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Whether a disallowed stream rejects the whole file.
    pub fn is_strict(&self) -> bool {
        self.strict
    }

    /// Whether a codec is allowed.
    pub fn allows(&self, codec: Codec) -> bool {
        self.listing || self.codecs.contains(&codec)
    }

    /// The allowed codecs.
    pub fn codecs(&self) -> &[Codec] {
        &self.codecs
    }
}

impl Default for Allowlist {
    fn default() -> Self {
        Self::new(&[
            Codec::H264,
            Codec::Hevc,
            Codec::Av1,
            Codec::Vp9,
            Codec::ProRes,
            Codec::Aac,
            Codec::Mp3,
            Codec::Opus,
            Codec::Flac,
            Codec::Alac,
            Codec::Ac3,
            Codec::Eac3,
            Codec::Pcm,
            Codec::TimedText,
            Codec::Timecode,
        ])
    }
}

/// Layout of linear PCM samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmFormat {
    /// IEEE floating point instead of integer.
    pub float: bool,
    /// Bits per sample and channel.
    pub bits: u16,
    /// Little-endian byte order.
    pub little_endian: bool,
    /// Signed integers (ignored for floating point).
    pub signed: bool,
}

/// Color description as ITU-T H.273 code points, plus the range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color {
    /// `colour_primaries`, when signalled.
    pub primaries: Option<u16>,
    /// `transfer_characteristics`, when signalled.
    pub transfer: Option<u16>,
    /// `matrix_coefficients`, when signalled.
    pub matrix: Option<u16>,
    /// Full range (`true`) or limited range (`false`), when signalled.
    pub full_range: Option<bool>,
}

/// What the first sample description of a track says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleDescription {
    /// Sample entry type.
    pub fourcc: FourCc,
    /// Identified codec.
    pub codec: Codec,
    /// Coded width in pixels (visual entries).
    pub width: Option<u16>,
    /// Coded height in pixels (visual entries).
    pub height: Option<u16>,
    /// Bits a pixel (visual entries' `depth`, ISO/IEC 14496-12 12.1.3;
    /// QuickTime adds 32 for a grey palette).
    pub depth: Option<u16>,
    /// Pixel aspect ratio from `pasp`.
    pub pixel_aspect: Option<(u32, u32)>,
    /// Color description.
    pub color: Color,
    /// Channel count (audio entries).
    pub channels: Option<u32>,
    /// Sample rate in Hz (audio entries); fractional rates are truncated.
    pub sample_rate: Option<u32>,
    /// The sampling frequency the codec configuration declares: for AAC
    /// the `AudioSpecificConfig`'s explicitly signalled SBR extension
    /// frequency, else its core frequency (ISO/IEC 14496-3 1.6.2.1, table
    /// 1.18); for FLAC the `STREAMINFO` block of `dfLa` (RFC 9639 8.2);
    /// for ALAC the magic cookie's `sampleRate`. `None` for other codecs
    /// and when it cannot be read. The sample entry's 16.16 `sample_rate`
    /// is kept as stored: writers copy it, and it cannot hold a rate of
    /// 65,536 Hz or more.
    pub configured_sample_rate: Option<u32>,
    /// The bits per sample the codec configuration declares: FLAC's
    /// `STREAMINFO` in `dfLa`, the ALAC magic cookie's `bitDepth`;
    /// `None` for other codecs and when it cannot be read.
    pub configured_bits: Option<u16>,
    /// AAC whose configuration is mono with an LC core, under a stereo
    /// sample entry, without an explicit signal that rules parametric
    /// stereo out (`sbrPresentFlag` 0, or `psPresentFlag` 0):
    /// parametric stereo may be signalled only in the bitstream (implicit
    /// signalling, ISO/IEC 14496-3 1.6.5), which probe does not decode.
    /// `channels` keeps the sample entry's 2 then; a decoder reports 1 if
    /// the bitstream carries no parametric stereo.
    pub implicit_ps_possible: bool,
    /// PCM layout, for [`Codec::Pcm`].
    pub pcm: Option<PcmFormat>,
    /// Bytes of one audio frame of uncompressed audio, from the sound
    /// sample entry: `bytesPerFrame` of a QuickTime version 1 entry, else
    /// the channel count times the bytes of one sample (QuickTime File
    /// Format, sound sample descriptions).
    pub(crate) frame_bytes: Option<u32>,
    /// `samplesPerPacket` and `bytesPerFrame` of a QuickTime version 1
    /// sound sample description of a compressed format (QuickTime File
    /// Format, sound sample descriptions): how many of the track's samples,
    /// one per audio sample, make one compressed frame, and that frame's
    /// bytes over all channels. `None` for other entries, for the
    /// uncompressed codes, or when either field is 0.
    pub sound_packet: Option<(u32, u32)>,
    /// Profile and level code points from the codec configuration, `None`
    /// when the codec carries none or the record is malformed.
    pub profile: Option<Profile>,
    /// Bit rate declared by the container (`btrt`, else `esds`), as stored.
    pub declared_bit_rate: Option<DeclaredBitRate>,
    /// The first codec configuration box of the entry, as stored.
    pub codec_config: Option<CodecConfig>,
    /// The Dolby Vision decoder configuration record of a visual entry
    /// (`dvcC`, `dvvC` or `dvwC`), when it has one and it can be read; the
    /// first readable one if a malformed entry has several (the
    /// specification allows one).
    pub dolby_vision: Option<DolbyVision>,
    /// The ambient viewing environment of a visual entry (`amve`):
    /// illuminance in 0.0001 lux, then the light's chromaticity x and y in
    /// 0.00002 steps.
    pub ambient: Option<(u32, u16, u16)>,
    /// The first `pasp`, `clap`, `colr` and `btrt` boxes of a visual entry,
    /// each as stored, in the entry's order (ISO/IEC 14496-12). The clip
    /// writer copies them.
    pub entry_boxes: Vec<CodecConfig>,
    /// Types of the other child boxes of a visual entry, each listed once,
    /// at most [`MAX_OTHER_BOXES`]: what a copy of the entry leaves out.
    pub other_boxes: Vec<FourCc>,
    /// The bound, in each dimension, of the picture sizes the parameter
    /// sets of the codec configuration define: every sequence parameter set
    /// of an `avcC` or `hvcC` record, cropped, or the largest frame size of
    /// the sequence header in an `av1C` record. `None` when the record holds
    /// no parameter set or one of them cannot be read. Unlike `width` and
    /// `height`, which the sample entry declares, this is what a decoder
    /// configured from the record produces; parameter sets carried in the
    /// samples (`avc3`, `hev1`, AV1 sync samples) are not included.
    pub parameter_set_size: Option<(u32, u32)>,
    /// The colour description the parameter set of the codec configuration
    /// signals, as stored (`docs/design/probe-color.md`): the video
    /// usability information of the first sequence parameter set of the
    /// first `avcC` or `hvcC` (ITU-T H.264 E.1.1, H.265 E.2.1), or the colour
    /// configuration of the sequence header of the first `av1C` (AV1
    /// 5.5.2). Code points are raw, "unspecified" (2) and reserved values
    /// included; a field the parameter set does not signal is `None` (no
    /// colour description, no video signal type). `None` as a whole when
    /// the entry has no such record or its parameter set cannot be read.
    /// `color` stays the `colr` box's.
    pub parameter_set_color: Option<Color>,
    /// The first record of the codec's type (`avcC`, `hvcC`, `av1C`) could
    /// not be read: a version other than 1, a structure past its end, or a
    /// parameter set that does not parse. No size bounds the track then.
    pub(crate) parameter_record_unreadable: bool,
}

/// Most child box types listed in [`SampleDescription::other_boxes`].
pub const MAX_OTHER_BOXES: usize = 16;

/// Profile and level as the codec configuration stores them.
/// Names are an output concern; these are the specification's code points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// `avcC` (ISO/IEC 14496-15 5.3.3.1; ITU-T H.264 7.3.2.1.1).
    H264 {
        /// `AVCProfileIndication` (`profile_idc`).
        profile_idc: u8,
        /// `profile_compatibility`: `constraint_set0_flag` is the most
        /// significant bit.
        constraint_flags: u8,
        /// `AVCLevelIndication` (`level_idc`).
        level_idc: u8,
    },
    /// `hvcC` (ISO/IEC 14496-15 8.3.3.1; ITU-T H.265 7.3.3).
    Hevc {
        /// `general_profile_space`.
        profile_space: u8,
        /// `general_tier_flag`.
        high_tier: bool,
        /// `general_profile_idc`.
        profile_idc: u8,
        /// `general_profile_compatibility_flags`; flag `j` is bit `31 - j`.
        compatibility_flags: u32,
        /// `general_constraint_indicator_flags`, the low 48 bits.
        constraint_flags: u64,
        /// `general_level_idc` (30 times the level number).
        level_idc: u8,
    },
    /// `av1C` (AV1 ISOBMFF binding 2.3.3).
    Av1 {
        /// `seq_profile`.
        profile: u8,
        /// `seq_level_idx_0`.
        level_idx: u8,
        /// `seq_tier_0`.
        high_tier: bool,
        /// `high_bitdepth`.
        high_bitdepth: bool,
        /// `twelve_bit`.
        twelve_bit: bool,
        /// `mono_chrome`.
        monochrome: bool,
    },
    /// `vpcC` (VP codec ISOBMFF binding).
    Vp9 {
        /// `profile`.
        profile: u8,
        /// `level`.
        level: u8,
        /// `bitDepth`.
        bit_depth: u8,
    },
    /// `AudioSpecificConfig` in `esds` (ISO/IEC 14496-3 1.6.2.1).
    Aac {
        /// The core `audioObjectType` (2 for AAC LC), after explicit
        /// hierarchical SBR or parametric-stereo signalling.
        object_type: u8,
        /// SBR is signalled explicitly in the configuration. `false` means
        /// not signalled there; implicit signalling is only in the bitstream.
        sbr: bool,
        /// Parametric stereo is signalled explicitly in the configuration.
        ps: bool,
    },
    /// ProRes, selected by the sample entry type (SMPTE RDD 36); no level.
    ProRes(ProResProfile),
}

/// ProRes profiles by sample entry type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProResProfile {
    /// `apco`.
    Proxy,
    /// `apcs`.
    Lt,
    /// `apcn`.
    Standard,
    /// `apch`.
    Hq,
    /// `ap4h`.
    P4444,
    /// `ap4x`.
    P4444Xq,
}

/// A bit rate declared by the container, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeclaredBitRate {
    /// Where it comes from: `btrt` (ISO/IEC 14496-12 8.5.2.2) or `esds`
    /// (`DecoderConfigDescriptor`, ISO/IEC 14496-1 7.2.6.6).
    pub source: FourCc,
    /// `avgBitrate` in bits per second (0 when the writer did not know it).
    pub average: u32,
    /// `maxBitrate` in bits per second.
    pub maximum: u32,
    /// `bufferSizeDB` in bytes.
    pub buffer_size: u32,
}

/// A codec configuration box as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecConfig {
    /// Box type: `avcC`, `hvcC`, `av1C`, `vpcC`, `esds`, `dOps`, `dfLa`,
    /// `alac`, `dac3`, `dec3` or `QDCA`.
    pub kind: FourCc,
    /// Box payload.
    pub data: Vec<u8>,
}

/// Sample entry layout chosen by the track handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryLayout {
    Visual,
    Audio,
    Other,
}

const VISUAL_FIXED: usize = 70;

/// Parses the payload of the first entry of an `stsd` box. `entry` holds the
/// whole entry, header included; `offset` is its absolute position.
pub(crate) fn parse_sample_entry(
    entry: &[u8],
    offset: u64,
    layout: EntryLayout,
    warnings: &mut Vec<Warning>,
) -> Result<SampleDescription, Error> {
    let mut bytes = Bytes::new(entry, offset);
    bytes.skip(4)?;
    let fourcc = bytes.fourcc()?;
    bytes.skip(8)?;
    let mut desc = SampleDescription {
        fourcc,
        codec: codec_from_fourcc(fourcc),
        width: None,
        height: None,
        depth: None,
        pixel_aspect: None,
        color: Color::default(),
        channels: None,
        sample_rate: None,
        configured_sample_rate: None,
        configured_bits: None,
        implicit_ps_possible: false,
        pcm: None,
        frame_bytes: None,
        sound_packet: None,
        profile: None,
        declared_bit_rate: None,
        codec_config: None,
        dolby_vision: None,
        ambient: None,
        entry_boxes: Vec::new(),
        other_boxes: Vec::new(),
        parameter_set_size: None,
        parameter_set_color: None,
        parameter_record_unreadable: false,
    };
    let children = match layout {
        EntryLayout::Visual => {
            bytes.skip(16)?;
            desc.width = Some(bytes.u16()?);
            desc.height = Some(bytes.u16()?);
            bytes.skip(4 + 4 + 4 + 2 + 32)?;
            desc.depth = Some(bytes.u16()?);
            bytes.skip(VISUAL_FIXED - 20 - 48)?;
            bytes
        }
        EntryLayout::Audio => audio_fields(&mut bytes, &mut desc)?,
        EntryLayout::Other => bytes,
    };
    let mut codec_range = None;
    let mut record_seen = false;
    let mut esds = None;
    let mut little_endian = None;
    let mut profile = None;
    let mut btrt = None;
    let mut lossless = None;
    visit_children(
        children.rest(),
        children.offset(),
        2,
        &mut |kind, payload| {
            if desc.codec_config.is_none() && CONFIG_BOXES.contains(&&kind.0) {
                desc.codec_config = Some(CodecConfig {
                    kind,
                    data: payload.to_vec(),
                });
            }
            // The size comes from the first record of the codec's own type,
            // whatever other configuration boxes come before it.
            if !record_seen
                && matches!(
                    (&kind.0, desc.codec),
                    (b"avcC", Codec::H264) | (b"hvcC", Codec::Hevc) | (b"av1C", Codec::Av1)
                )
            {
                record_seen = true;
                match size::configured(&kind.0, payload) {
                    Ok(size) => desc.parameter_set_size = size,
                    Err(()) => desc.parameter_record_unreadable = true,
                }
                desc.parameter_set_color = signalled_color(&kind.0, payload);
            }
            if layout == EntryLayout::Visual {
                if ENTRY_BOXES.contains(&&kind.0) {
                    if !desc.entry_boxes.iter().any(|b| b.kind == kind) {
                        desc.entry_boxes.push(CodecConfig {
                            kind,
                            data: payload.to_vec(),
                        });
                    }
                } else if !CONFIG_BOXES.contains(&&kind.0)
                    && desc.other_boxes.len() < MAX_OTHER_BOXES
                    && !desc.other_boxes.contains(&kind)
                {
                    desc.other_boxes.push(kind);
                }
            }
            let wanted = match (&kind.0, desc.codec) {
                (b"avcC", Codec::H264) => Some(avc_profile(payload)),
                (b"hvcC", Codec::Hevc) => Some(hevc_profile(payload)),
                (b"av1C", Codec::Av1) => Some(av1_profile(payload)),
                (b"vpcC", Codec::Vp9) => Some((vp9_profile(payload), true)),
                _ => None,
            };
            if let Some((parsed, consistent)) = wanted {
                if profile.is_none() {
                    if parsed.is_none() || !consistent {
                        crate::error::warn(warnings, Warning::MalformedCodecConfig(kind));
                    }
                    profile = Some(parsed);
                }
            }
            match &kind.0 {
                b"btrt" => {
                    let mut p = Bytes::new(payload, 0);
                    if let (Ok(buffer_size), Ok(maximum), Ok(average)) = (p.u32(), p.u32(), p.u32())
                    {
                        btrt.get_or_insert(DeclaredBitRate {
                            source: kind,
                            average,
                            maximum,
                            buffer_size,
                        });
                    }
                }
                b"pasp" => {
                    let mut p = Bytes::new(payload, 0);
                    if let (Ok(h), Ok(v)) = (p.u32(), p.u32()) {
                        desc.pixel_aspect = Some((h, v));
                    }
                }
                b"colr" => parse_colr(payload, &mut desc.color),
                b"dvcC" | b"dvvC" | b"dvwC" if desc.dolby_vision.is_none() => {
                    desc.dolby_vision = dolby_vision(payload);
                }
                b"amve" if desc.ambient.is_none() => {
                    let mut p = Bytes::new(payload, 0);
                    if let (Ok(lux), Ok(x), Ok(y)) = (p.u32(), p.u16(), p.u16()) {
                        desc.ambient = Some((lux, x, y));
                    }
                }
                b"hvcC" => {
                    codec_range = bitstream::hevc_full_range(payload);
                    if codec_range.is_none() {
                        crate::error::warn(warnings, Warning::MalformedCodecConfig(kind));
                    }
                }
                b"av1C" => {
                    codec_range = bitstream::av1_full_range(payload);
                    if codec_range.is_none() {
                        crate::error::warn(warnings, Warning::MalformedCodecConfig(kind));
                    }
                }
                b"vpcC" => {
                    // VP9 ISOBMFF binding: full box, then profile, level and
                    // bitDepth(4) chromaSubsampling(3) videoFullRangeFlag(1).
                    codec_range = payload.get(6).map(|b| b & 1 == 1);
                }
                b"esds" if esds.is_none() => {
                    esds = Some(parse_esds(payload));
                }
                b"dfLa" if desc.codec == Codec::Flac && lossless.is_none() => {
                    lossless = Some((kind, lossless_config(kind, payload)));
                }
                b"alac" if fourcc.0 == *b"alac" && lossless.is_none() => {
                    lossless = Some((kind, lossless_config(kind, payload)));
                }
                b"enda" => little_endian = payload.get(..2).map(|v| v != [0, 0]),
                b"pcmC" => {
                    // ISO/IEC 23003-5: full box, format_flags, PCM_sample_size.
                    if let (Some(flags), Some(size)) = (payload.get(4), payload.get(5)) {
                        if let Some(pcm) = desc.pcm.as_mut() {
                            pcm.little_endian = flags & 1 == 1;
                            pcm.bits = u16::from(*size);
                        }
                    }
                }
                _ => {}
            }
        },
    )?;
    if desc.color.full_range.is_none() {
        desc.color.full_range = codec_range;
    }
    match lossless {
        Some((_, Some(config))) => {
            desc.configured_sample_rate = Some(config.sample_rate);
            desc.configured_bits = Some(config.bits);
        }
        Some((kind, None)) => crate::error::warn(warnings, Warning::MalformedCodecConfig(kind)),
        None if desc.codec == Codec::Flac => {
            crate::error::warn(warnings, Warning::MalformedCodecConfig(FourCc(*b"dfLa")));
        }
        None if fourcc.0 == *b"alac" => {
            crate::error::warn(warnings, Warning::MalformedCodecConfig(FourCc(*b"alac")));
        }
        None => {}
    }
    let esds = esds.flatten();
    if fourcc == FourCc(*b"mp4a") {
        desc.codec = match esds.as_ref().map(|e| e.object_type) {
            Some(0x40 | 0x66 | 0x67 | 0x68) => Codec::Aac,
            Some(0x69 | 0x6b) => Codec::Mp3,
            _ => Codec::Other,
        };
    }
    desc.profile = match desc.codec {
        Codec::Aac => {
            let parsed = esds.as_ref().and_then(|e| e.aac);
            if parsed.is_none() {
                crate::error::warn(warnings, Warning::MalformedCodecConfig(FourCc(*b"esds")));
            }
            // The sample entry's channel count is definitive only for codecs
            // that do not record their own output configuration (ISO/IEC
            // 14496-12, 12.2.3); AAC records it in `channelConfiguration`.
            // Parametric stereo makes a mono core stereo (ISO/IEC 14496-3,
            // 8.6.4): explicitly signalled, it counts as 2. It can also be
            // signalled only in the bitstream of an LC core (implicit
            // signalling), which probe does not decode; a mono
            // configuration under a stereo sample entry keeps the entry's
            // 2 then.
            // A file that signals explicitly that there is no SBR, or SBR
            // without parametric stereo, cannot carry parametric stereo, so
            // its configuration is trusted.
            let layout = esds.as_ref().and_then(|e| e.layout);
            desc.configured_sample_rate = layout.and_then(|l| l.rate);
            let configured = layout.and_then(|l| l.channels);
            let no_ps = layout.is_some_and(|l| l.no_ps);
            let (core, ps) = match parsed {
                Some(Profile::Aac {
                    object_type, ps, ..
                }) => (object_type, ps),
                _ => (0, false),
            };
            // Only for the general audio object types (ISO/IEC 14496-3,
            // table 1.1: AAC main, LC, SSR, LTP, scalable, TwinVQ, and their
            // error-resilient forms), whose configuration names the output
            // channels; others (for example AAC-ELD with MPEG Surround)
            // keep the sample entry's count.
            let general = matches!(core, 1..=4 | 6 | 7 | 17 | 19..=23);
            if let Some(channels) = configured.filter(|_| general) {
                let implicit_ps = core == 2 && desc.channels == Some(2) && !no_ps;
                desc.implicit_ps_possible = channels == 1 && !ps && implicit_ps;
                desc.channels = Some(if channels == 1 && (ps || implicit_ps) {
                    2
                } else {
                    channels
                });
            }
            parsed
        }
        Codec::ProRes => prores_profile(fourcc).map(Profile::ProRes),
        _ => profile.flatten(),
    };
    // AC-3 and E-AC-3 record their output channels in `dac3` and `dec3`
    // too, so those win over the sample entry's count.
    if let Some(channels) = desc
        .codec_config
        .as_ref()
        .and_then(|c| ac3_channels(desc.codec, c))
    {
        desc.channels = Some(channels);
    }
    desc.declared_bit_rate = btrt.or_else(|| {
        let (buffer_size, maximum, average) = esds.as_ref()?.rates?;
        Some(DeclaredBitRate {
            source: FourCc(*b"esds"),
            average,
            maximum,
            buffer_size,
        })
    });
    if let (Some(pcm), Some(le)) = (desc.pcm.as_mut(), little_endian) {
        if pcm.bits > 8 {
            pcm.little_endian = le;
        }
    }
    Ok(desc)
}

/// What a FLAC `dfLa` or Apple Lossless `alac` configuration box declares
/// of the decoded audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LosslessConfig {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Bits per sample.
    pub bits: u16,
    /// Channels.
    pub channels: u16,
    /// The most samples a frame holds: FLAC's maximum block size, ALAC's
    /// `frameLength`.
    pub frame_length: u32,
}

/// The configuration a `dfLa` or `alac` box payload declares; `None` for
/// another box, and for a payload that is missing a field or states a value
/// the format does not allow (an invalid FLAC block size, an unsupported
/// ALAC cookie version).
pub fn lossless_config(kind: FourCc, payload: &[u8]) -> Option<LosslessConfig> {
    match &kind.0 {
        b"dfLa" => flac_streaminfo(payload),
        b"alac" => alac_cookie(payload),
        _ => None,
    }
}

/// The sample rate, bits per sample, channels and maximum block size of the
/// `STREAMINFO` block a `dfLa` box carries. The box (the FLAC-in-ISOBMFF mapping) is a full box of
/// version 0 and flags 0 whose metadata blocks start with `STREAMINFO`:
/// a block header of type 0 and length 34 (RFC 9639 8.1), then 16 bits
/// each of minimum and maximum block size, 24 each of minimum and maximum
/// frame size, 20 of sample rate, 3 of channels less one and 5 of bits per
/// sample less one (RFC 9639 8.2). `None` for any other layout, and for
/// what RFC 9639 does not allow: a block size under 16, a minimum above
/// the maximum, a rate of 0, or fewer than 4 bits per sample.
fn flac_streaminfo(payload: &[u8]) -> Option<LosslessConfig> {
    let mut p = Bytes::new(payload, 0);
    if p.u32().ok()? != 0 || p.u8().ok()? & 0x7F != 0 || p.u24().ok()? != 34 {
        return None;
    }
    let (min_block, max_block) = (p.u16().ok()?, p.u16().ok()?);
    if min_block < 16 || min_block > max_block {
        return None;
    }
    let mut bits = Bits::new(p.take(30).ok()?);
    bits.skip(48)?;
    let rate = bits.bits(20)?;
    let channels = u16::try_from(bits.bits(3)?).ok()? + 1;
    let depth = u16::try_from(bits.bits(5)?).ok()? + 1;
    (rate > 0 && depth >= 4).then_some(LosslessConfig {
        sample_rate: rate,
        bits: depth,
        channels,
        frame_length: u32::from(max_block),
    })
}

/// The sample rate, bit depth, channels and frame length of an ALAC magic
/// cookie: the `alac` box
/// of an `alac` sample entry is a full box of version 0 and flags 0 that
/// holds an `ALACSpecificConfig` (Apple Lossless Audio Codec, magic cookie
/// description): `frameLength` (32 bits), `compatibleVersion` (8),
/// `bitDepth` (8), `pb`, `mb`, `kb`, `numChannels` (8 each), `maxRun`
/// (16), `maxFrameBytes` (32), `avgBitRate` (32), `sampleRate` (32).
/// `None` for a shorter box, another version of the box or of the cookie
/// (`compatibleVersion` other than 0), a bit depth of 0 or above 32, or a
/// rate of 0.
fn alac_cookie(payload: &[u8]) -> Option<LosslessConfig> {
    let mut p = Bytes::new(payload, 0);
    if p.u32().ok()? != 0 {
        return None;
    }
    let frame_length = p.u32().ok()?;
    if p.u8().ok()? != 0 {
        return None;
    }
    let depth = p.u8().ok()?;
    p.skip(3).ok()?;
    let channels = p.u8().ok()?;
    p.skip(10).ok()?;
    let rate = p.u32().ok()?;
    ((1..=32).contains(&depth) && rate > 0).then_some(LosslessConfig {
        sample_rate: rate,
        bits: u16::from(depth),
        channels: u16::from(channels),
        frame_length,
    })
}

/// A Dolby Vision decoder configuration record (`DOVIDecoderConfigurationRecord`
/// in Dolby's "Dolby Vision Streams Within the ISO Base Media File Format",
/// carried in `dvcC`, `dvvC` or `dvwC`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DolbyVision {
    /// `dv_version_major`.
    pub version_major: u8,
    /// `dv_version_minor`.
    pub version_minor: u8,
    /// `dv_profile`.
    pub profile: u8,
    /// `dv_level`.
    pub level: u8,
    /// `rpu_present_flag`.
    pub rpu_present: bool,
    /// `el_present_flag`.
    pub el_present: bool,
    /// `bl_present_flag`.
    pub bl_present: bool,
    /// `dv_bl_signal_compatibility_id`.
    pub bl_signal_compatibility_id: u8,
}

/// Reads a `DOVIDecoderConfigurationRecord`: two version octets, then 7
/// bits of profile, 6 of level, the three presence flags and 4 bits of base
/// layer compatibility; `None` when it is cut short.
fn dolby_vision(payload: &[u8]) -> Option<DolbyVision> {
    let mut bits = Bits::new(payload);
    let mut field = |width| bits.bits(width).and_then(|v| u8::try_from(v).ok());
    Some(DolbyVision {
        version_major: field(8)?,
        version_minor: field(8)?,
        profile: field(7)?,
        level: field(6)?,
        rpu_present: field(1)? == 1,
        el_present: field(1)? == 1,
        bl_present: field(1)? == 1,
        bl_signal_compatibility_id: field(4)?,
    })
}

/// Configuration boxes whose payload is kept as `codec_config`.
const CONFIG_BOXES: [&[u8; 4]; 11] = [
    b"avcC", b"hvcC", b"av1C", b"vpcC", b"esds", b"dOps", b"dfLa", b"alac", b"dac3", b"dec3",
    b"QDCA",
];

/// Visual sample entry children kept as stored, for the clip writer to copy.
const ENTRY_BOXES: [&[u8; 4]; 4] = [b"pasp", b"clap", b"colr", b"btrt"];

/// What a codec configuration record says, read exactly as in a sample
/// description. Other containers carry the same records: a Matroska
/// `CodecPrivate` holds an `avcC`, `hvcC` or `av1C` payload, or an
/// `AudioSpecificConfig` (the Matroska codec mappings).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConfigRecord {
    /// Profile and level code points.
    pub profile: Option<Profile>,
    /// The video signal type of the parameter set the record carries (the
    /// first sequence parameter set, or the AV1 sequence header): the
    /// color range, and the ITU-T H.273 code points except "unspecified"
    /// and reserved values. Unknown fields are `None`.
    pub color: Color,
    /// The record carries a parameter set and it was read.
    pub parameter_set: bool,
    /// The bound of the picture sizes its parameter sets define, as
    /// [`SampleDescription::parameter_set_size`].
    pub parameter_set_size: Option<(u32, u32)>,
    /// The picture size the record's first sequence parameter set
    /// describes: its coded size less the whole conformance crop. This is
    /// the size to report for a stream whose container states none (a
    /// transport stream), where `parameter_set_size`, a bound on what a
    /// decoder may put out, keeps a crop larger than padding.
    pub parameter_set_display_size: Option<(u32, u32)>,
    /// The record could not be read, or its header disagrees with its
    /// parameter set (the parameter set wins).
    pub malformed: bool,
}

/// Reads an `avcC`, `hvcC` or `av1C` payload (`kind` names which);
/// `None` for any other kind.
pub fn describe_config_record(kind: FourCc, record: &[u8]) -> Option<ConfigRecord> {
    let ((profile, consistent), signal) = match &kind.0 {
        b"avcC" => (avc_profile(record), bitstream::avc_signal(record)),
        b"hvcC" => (hevc_profile(record), bitstream::hevc_signal(record)),
        b"av1C" => (av1_profile(record), bitstream::av1_signal(record)),
        _ => return None,
    };
    let known = |value: u8, codes: &[u8]| codes.contains(&value).then_some(u16::from(value));
    let mut color = Color::default();
    if let Some(signal) = signal {
        color.full_range = signal.full_range;
        if let Some((primaries, transfer, matrix)) = signal.colour {
            color.primaries = known(primaries, &[1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 22]);
            color.transfer = known(
                transfer,
                &[1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18],
            );
            color.matrix = known(matrix, &[0, 1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]);
        }
    }
    // A record without a parameter set is valid (an `av1C` without
    // configuration OBUs, an `avc3`-style `avcC`); one whose parameter set
    // cannot be read is not.
    let carried = bitstream::has_parameter_set(&kind.0, record);
    let unreadable = signal.is_none() && carried;
    Some(ConfigRecord {
        profile,
        color,
        parameter_set: carried && signal.is_some(),
        parameter_set_size: crate::size::configured_size(&kind.0, record),
        parameter_set_display_size: crate::size::configured_display_size(&kind.0, record),
        malformed: profile.is_none() || !consistent || unreadable,
    })
}

/// Whether the first sequence parameter set of an `hvcC` payload has a
/// conformance window that leaves no picture, which ITU-T H.265 7.4.3.2.1
/// does not allow: such a stream is not valid HEVC. `false` when the
/// payload holds no sequence parameter set or it cannot be read.
pub fn hevc_window_empty(hvcc: &[u8]) -> bool {
    crate::size::hvcc_window_empty(hvcc)
}

/// `max_num_ref_frames` of the first sequence parameter set of an `avcC`
/// payload (ITU-T H.264 7.4.2.1.1): the most reference frames the
/// decoding process keeps. `None` when it cannot be read.
pub fn avc_max_num_ref_frames(avcc: &[u8]) -> Option<u32> {
    bitstream::avc_max_num_ref_frames(avcc)
}

/// `num_units_in_tick` and `time_scale` of the first sequence parameter
/// set of an `avcC` payload (ITU-T H.264 E.2.1), the clock the stream
/// states for itself: a field lasts `num_units_in_tick / time_scale`
/// seconds and a frame twice that. `None` when the set states no timing
/// information or cannot be read.
pub fn avc_timing(avcc: &[u8]) -> Option<(u32, u32)> {
    bitstream::avc_timing(avcc)
}

/// The AAC profile of an `AudioSpecificConfig` (ISO/IEC 14496-3 1.6.2.1),
/// read as from an `esds` descriptor; `None` when it cannot be read.
pub fn describe_audio_specific_config(config: &[u8]) -> Option<Profile> {
    aac_profile(config)
}

// Profile readers return the profile and whether the record header agrees
// with the parameter set it carries. The parameter set (the first sequence
// parameter set, or the AV1 sequence header OBU) is what a decoder uses, so
// its values win; the header is the fallback.

/// `avcC` (ISO/IEC 14496-15 5.3.3.1): version, profile, compatibility, level.
fn avc_profile(record: &[u8]) -> (Option<Profile>, bool) {
    let [1, profile_idc, constraint_flags, level_idc, ..] = *record else {
        return (None, true);
    };
    let header = (profile_idc, constraint_flags, level_idc);
    let (profile_idc, constraint_flags, level_idc) =
        bitstream::avc_sps_profile(record).unwrap_or(header);
    let profile = Profile::H264 {
        profile_idc,
        constraint_flags,
        level_idc,
    };
    (
        Some(profile),
        header == (profile_idc, constraint_flags, level_idc),
    )
}

/// `hvcC` (ISO/IEC 14496-15 8.3.3.1): the general profile, tier and level.
fn hevc_profile(record: &[u8]) -> (Option<Profile>, bool) {
    let header = || -> Option<bitstream::HevcGeneral> {
        let mut b = Bytes::new(record, 0);
        if b.u8().ok()? != 1 {
            return None;
        }
        let first = b.u8().ok()?;
        let compatibility = b.u32().ok()?;
        let high = u64::from(b.u16().ok()?);
        let low = u64::from(b.u32().ok()?);
        let level = b.u8().ok()?;
        Some((
            first >> 6,
            first & 0x20 != 0,
            first & 0x1f,
            compatibility,
            (high << 32) | low,
            level,
        ))
    };
    let Some(header) = header() else {
        return (None, true);
    };
    let general = bitstream::hevc_sps_profile(record).unwrap_or(header);
    let (profile_space, high_tier, profile_idc, compatibility_flags, constraint_flags, level_idc) =
        general;
    let profile = Profile::Hevc {
        profile_space,
        high_tier,
        profile_idc,
        compatibility_flags,
        constraint_flags,
        level_idc,
    };
    (Some(profile), header == general)
}

/// `av1C` (AV1 ISOBMFF binding 2.3.3): marker and version, then the
/// sequence profile, level and tier.
fn av1_profile(record: &[u8]) -> (Option<Profile>, bool) {
    let [0x81, first, second, ..] = *record else {
        return (None, true);
    };
    let header = (first >> 5, first & 0x1f, second & 0x80 != 0);
    let (profile, level_idx, high_tier) = bitstream::av1_sequence_profile(record).unwrap_or(header);
    let value = Profile::Av1 {
        profile,
        level_idx,
        high_tier,
        high_bitdepth: second & 0x40 != 0,
        twelve_bit: second & 0x20 != 0,
        monochrome: second & 0x10 != 0,
    };
    (Some(value), header == (profile, level_idx, high_tier))
}

/// `vpcC` (VP codec ISOBMFF binding): a full box, then profile, level and
/// bitDepth(4). Versions 0 and 1 share these positions.
fn vp9_profile(record: &[u8]) -> Option<Profile> {
    let [version, _, _, _, profile, level, depth, ..] = *record else {
        return None;
    };
    if version > 1 {
        return None;
    }
    Some(Profile::Vp9 {
        profile,
        level,
        bit_depth: depth >> 4,
    })
}

fn prores_profile(fourcc: FourCc) -> Option<ProResProfile> {
    Some(match &fourcc.0 {
        b"apco" => ProResProfile::Proxy,
        b"apcs" => ProResProfile::Lt,
        b"apcn" => ProResProfile::Standard,
        b"apch" => ProResProfile::Hq,
        b"ap4h" => ProResProfile::P4444,
        b"ap4x" => ProResProfile::P4444Xq,
        _ => return None,
    })
}

/// `GetAudioObjectType()` (ISO/IEC 14496-3 1.6.2.1).
fn audio_object_type(bits: &mut Bits<'_>) -> Option<u8> {
    let value = bits.bits(5)?;
    let value = if value == 31 {
        32 + bits.bits(6)?
    } else {
        value
    };
    u8::try_from(value).ok()
}

/// `samplingFrequencyIndex`, with the 24-bit frequency after the escape
/// (ISO/IEC 14496-3 1.6.2.1, table 1.18); `None` inside the frequency when
/// the index is reserved.
fn sampling_frequency(bits: &mut Bits<'_>) -> Option<Option<u32>> {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let index = bits.bits(4)?;
    if index == 0xf {
        return Some(Some(bits.bits(24)?).filter(|r| *r > 0));
    }
    Some(
        usize::try_from(index)
            .ok()
            .and_then(|i| RATES.get(i))
            .copied(),
    )
}

/// The channels a `channelConfiguration` defines (ISO/IEC 14496-3, table
/// 1.19); `None` for 0 (defined by a program config element) and reserved
/// values.
fn configured_channels(configuration: u8) -> Option<u32> {
    match configuration {
        1..=6 => Some(u32::from(configuration)),
        7 | 12 | 14 => Some(8),
        11 => Some(7),
        13 => Some(24),
        _ => None,
    }
}

/// The channels an AC-3 `dac3` or E-AC-3 `dec3` box declares (ETSI TS
/// 102 366 F.4 and F.6): those its `acmod` (table 4.3) and `lfeon` code,
/// for E-AC-3 those of the first independent substream. Channels that
/// dependent substreams add (`chan_loc`) are not counted.
fn ac3_channels(codec: Codec, config: &CodecConfig) -> Option<u32> {
    let mut bits = Bits::new(&config.data);
    match (codec, &config.kind.0) {
        (Codec::Ac3, b"dac3") => bits.skip(2 + 5 + 3)?,
        (Codec::Eac3, b"dec3") => bits.skip(13 + 3 + 2 + 5 + 1 + 1 + 3)?,
        _ => return None,
    }
    let full = match bits.bits(3)? {
        1 => 1,
        0 | 2 => 2,
        3 | 4 => 3,
        5 | 6 => 4,
        _ => 5,
    };
    Some(full + bits.bits(1)?)
}

/// The output channels of an `AudioSpecificConfig` (ISO/IEC 14496-3
/// 1.6.2.1): its `channelConfiguration`, or its program config element when
/// that is 0; and whether the backward compatible extension after a plain
/// `GASpecificConfig` (object types 1 to 4) sets `sbrPresentFlag` to 0.
fn aac_layout(config: &[u8]) -> Option<AacLayout> {
    let mut bits = Bits::new(config);
    let object_type = audio_object_type(&mut bits)?;
    let core_rate = sampling_frequency(&mut bits)?;
    let configuration = u8::try_from(bits.bits(4)?).ok()?;
    let mut layout = AacLayout {
        channels: configured_channels(configuration),
        no_ps: false,
        rate: core_rate,
    };
    // Explicit hierarchical signalling (5: SBR, 29: SBR and parametric
    // stereo): the extension sampling frequency is the output's.
    if matches!(object_type, 5 | 29) {
        layout.rate = sampling_frequency(&mut bits).flatten().or(core_rate);
        return Some(layout);
    }
    // GASpecificConfig (4.4.1) only for the general audio object types.
    if !matches!(object_type, 1..=4 | 6 | 7 | 17 | 19..=23) {
        return Some(layout);
    }
    // frameLengthFlag, dependsOnCoreCoder (+ 14-bit delay), extensionFlag.
    if bits.skip(1).is_none() {
        return Some(layout);
    }
    match bits.flag() {
        Some(true) if bits.skip(14).is_some() => {}
        Some(false) => {}
        _ => return Some(layout),
    }
    let Some(extension) = bits.flag() else {
        return Some(layout);
    };
    if configuration == 0 {
        layout.channels = program_config_channels(&mut bits);
        return Some(layout);
    }
    if extension || !(1..=4).contains(&object_type) {
        return Some(layout);
    }
    let mut ext = bits;
    if ext.clone().bits(16).is_some()
        && ext.bits(11) == Some(0x2b7)
        && audio_object_type(&mut ext) == Some(5)
    {
        // sbrPresentFlag 0 rules out SBR and so parametric stereo; with
        // SBR, the extension sampling frequency, then 0x548 and
        // psPresentFlag 0 rule it out explicitly (ISO/IEC 14496-3, 1.6.2.1).
        layout.no_ps = match ext.flag() {
            Some(false) => true,
            Some(true) => {
                // extensionSamplingFrequencyIndex, and the 24-bit
                // frequency after an escape: the output's.
                let frequency = sampling_frequency(&mut ext);
                if let Some(Some(rate)) = frequency {
                    layout.rate = Some(rate);
                }
                frequency.is_some()
                    && ext.clone().bits(12).is_some()
                    && ext.bits(11) == Some(0x548)
                    && ext.flag() == Some(false)
            }
            None => false,
        };
    }
    Some(layout)
}

/// The channels of a `program_config_element()` (ISO/IEC 14496-3, 4.4.1.1,
/// table 4.2): one per single channel element, two per channel pair
/// element, one per LFE; `None` when it is cut short or has none.
fn program_config_channels(bits: &mut Bits<'_>) -> Option<u32> {
    // element_instance_tag, object_type, sampling_frequency_index.
    bits.skip(4 + 2 + 4)?;
    let front = bits.bits(4)?;
    let side = bits.bits(4)?;
    let back = bits.bits(4)?;
    let lfe = bits.bits(2)?;
    // num_assoc_data_elements, num_valid_cc_elements.
    bits.skip(3 + 4)?;
    // Mono and stereo mixdown element numbers, matrix mixdown index and
    // pseudo surround flag, each after its present flag.
    for width in [4, 4, 3] {
        if bits.flag()? {
            bits.skip(width)?;
        }
    }
    let mut channels = lfe;
    for _ in 0..front + side + back {
        let pair = bits.flag()?;
        bits.skip(4)?;
        channels += if pair { 2 } else { 1 };
    }
    // The LFE element tags.
    bits.skip(4 * lfe)?;
    Some(channels).filter(|c| *c > 0)
}

/// The object type and the explicit SBR and parametric-stereo signalling of
/// an `AudioSpecificConfig` (ISO/IEC 14496-3 1.6.2.1, 4.4.1). The backward
/// compatible extension is read only after a plain `GASpecificConfig`
/// (object types 1 to 4 with a non-zero `channelConfiguration`).
fn aac_profile(config: &[u8]) -> Option<Profile> {
    let mut bits = Bits::new(config);
    let mut object_type = audio_object_type(&mut bits)?;
    sampling_frequency(&mut bits)?;
    let channels = bits.bits(4)?;
    let (mut sbr, mut ps) = (false, false);
    if object_type == 5 || object_type == 29 {
        sbr = true;
        ps = object_type == 29;
        sampling_frequency(&mut bits)?;
        object_type = audio_object_type(&mut bits)?;
        if object_type == 22 {
            bits.skip(4)?;
        }
    }
    let profile = |object_type, sbr, ps| Profile::Aac {
        object_type,
        sbr,
        ps,
    };
    if sbr || channels == 0 || !(1..=4).contains(&object_type) {
        return Some(profile(object_type, sbr, ps));
    }
    // GASpecificConfig: frameLengthFlag, dependsOnCoreCoder (+ 14-bit
    // delay), extensionFlag; no extension fields for object types 1 to 4.
    bits.skip(1)?;
    let core = bits.flag()?;
    if core {
        bits.skip(14)?;
    }
    bits.skip(1)?;
    let mut ext = bits.clone();
    if ext.clone().bits(16).is_some() && ext.bits(11)? == 0x2b7 {
        if let Some(5) = audio_object_type(&mut ext) {
            sbr = ext.flag().unwrap_or(false);
            if sbr
                && sampling_frequency(&mut ext).is_some()
                && ext.clone().bits(12).is_some()
                && ext.bits(11) == Some(0x548)
            {
                ps = ext.flag().unwrap_or(false);
            }
        }
    }
    Some(profile(object_type, sbr, ps))
}

fn codec_from_fourcc(fourcc: FourCc) -> Codec {
    match &fourcc.0 {
        b"avc1" | b"avc3" => Codec::H264,
        b"hvc1" | b"hev1" => Codec::Hevc,
        b"av01" => Codec::Av1,
        b"vp09" => Codec::Vp9,
        b"apco" | b"apcs" | b"apcn" | b"apch" | b"ap4h" | b"ap4x" => Codec::ProRes,
        b"Opus" => Codec::Opus,
        b"fLaC" => Codec::Flac,
        b"alac" => Codec::Alac,
        b"ac-3" => Codec::Ac3,
        b"ec-3" => Codec::Eac3,
        // QuickTime's MPEG-1 Layer 3 sound sample description.
        b".mp3" => Codec::Mp3,
        b"sowt" | b"twos" | b"in24" | b"in32" | b"fl32" | b"fl64" | b"lpcm" | b"ipcm" | b"fpcm"
        | b"raw " => Codec::Pcm,
        b"tx3g" => Codec::TimedText,
        b"text" => Codec::QuickTimeText,
        b"tmcd" => Codec::Timecode,
        // `mp4a` is refined from its `esds` object type.
        _ => Codec::Other,
    }
}

/// Reads the sound sample entry fields (ISO/IEC 14496-12 12.2.3 and the
/// QuickTime version 1 and 2 layouts) and returns the remaining bytes.
fn audio_fields<'a>(
    bytes: &mut Bytes<'a>,
    desc: &mut SampleDescription,
) -> Result<Bytes<'a>, Error> {
    let version = bytes.u16()?;
    bytes.skip(6)?;
    let channels = bytes.u16()?;
    let sample_size = bytes.u16()?;
    bytes.skip(4)?;
    let rate = bytes.u32()? >> 16;
    desc.channels = Some(u32::from(channels));
    desc.sample_rate = Some(rate);
    let pcm = |bits: u16, float: bool, little_endian: bool, signed: bool| PcmFormat {
        float,
        bits,
        little_endian,
        signed,
    };
    desc.pcm = match &desc.fourcc.0 {
        b"sowt" => Some(pcm(
            if sample_size == 8 { 8 } else { 16 },
            false,
            true,
            true,
        )),
        b"twos" => Some(pcm(
            if sample_size == 8 { 8 } else { 16 },
            false,
            false,
            true,
        )),
        b"in24" => Some(pcm(24, false, false, true)),
        b"in32" => Some(pcm(32, false, false, true)),
        b"fl32" => Some(pcm(32, true, false, true)),
        b"fl64" => Some(pcm(64, true, false, true)),
        b"raw " => Some(pcm(8, false, false, false)),
        b"ipcm" => Some(pcm(sample_size, false, false, true)),
        b"fpcm" => Some(pcm(sample_size, true, false, true)),
        b"lpcm" => Some(pcm(sample_size, false, false, true)),
        _ => None,
    };
    // The QuickTime codes of uncompressed audio (not the ISO `ipcm`, `fpcm`
    // or the version 2 `lpcm`, whose sample tables give real sizes): a
    // frame is the channels times the width each code fixes, or that a
    // version 1 entry stores as `bytesPerFrame`.
    let quicktime = matches!(
        &desc.fourcc.0,
        b"sowt" | b"twos" | b"in24" | b"in32" | b"fl32" | b"fl64" | b"raw "
    );
    desc.frame_bytes = desc
        .pcm
        .filter(|_| quicktime)
        .and_then(|pcm| u32::from(channels).checked_mul(u32::from(pcm.bits).div_ceil(8)));
    match version {
        1 => {
            // samplesPerPacket, bytesPerPacket, bytesPerFrame, bytesPerSample.
            let per_packet = bytes.u32()?;
            bytes.skip(4)?;
            let per_frame = bytes.u32()?;
            bytes.skip(4)?;
            if per_frame > 0 && quicktime {
                desc.frame_bytes = Some(per_frame);
            }
            if per_frame > 0 && per_packet > 0 && desc.pcm.is_none() {
                desc.sound_packet = Some((per_packet, per_frame));
            }
        }
        2 => {
            bytes.skip(4)?;
            let rate = f64::from_bits(bytes.u64()?);
            let channels = bytes.u32()?;
            bytes.skip(4)?;
            let bits = bytes.u32()?;
            let flags = bytes.u32()?;
            bytes.skip(8)?;
            desc.channels = Some(channels);
            desc.frame_bytes = None;
            desc.sample_rate = (rate.is_finite() && (0.0..=f64::from(u32::MAX)).contains(&rate))
                .then_some(rate as u32);
            if desc.fourcc == FourCc(*b"lpcm") {
                desc.pcm = Some(PcmFormat {
                    float: flags & 1 != 0,
                    bits: u16::try_from(bits).unwrap_or(0),
                    little_endian: flags & 2 == 0,
                    signed: flags & 4 != 0,
                });
            }
        }
        _ => {}
    }
    Ok(bytes.clone())
}

/// Calls `visit` for every child box inside `data`, descending into QuickTime
/// `wave` boxes up to `depth` levels. A malformed child list ends the walk
/// without an error; codec configuration is optional metadata.
fn visit_children(
    data: &[u8],
    offset: u64,
    depth: u32,
    visit: &mut dyn FnMut(FourCc, &[u8]),
) -> Result<(), Error> {
    let mut bytes = Bytes::new(data, offset);
    while bytes.remaining() >= 8 {
        let size = bytes.u32()?;
        let kind = bytes.fourcc()?;
        let Some(len) = (size as usize).checked_sub(8) else {
            // A terminator (size 0) or a malformed size ends the list.
            return Ok(());
        };
        let Ok(payload) = bytes.take(len) else {
            return Ok(());
        };
        if kind == FourCc(*b"wave") && depth > 0 {
            visit_children(payload, 0, depth - 1, visit)?;
        } else {
            visit(kind, payload);
        }
    }
    Ok(())
}

/// The colour description the parameter set of an `avcC`, `hvcC` or `av1C`
/// record (`kind` names which) signals, raw, as
/// [`SampleDescription::parameter_set_color`]: "unspecified" (2) included,
/// `None` for a field it does not signal; `None` as a whole for another
/// kind or a parameter set that cannot be read. For a Matroska
/// `CodecPrivate`, which stores the same records.
pub fn parameter_set_color(kind: FourCc, record: &[u8]) -> Option<Color> {
    signalled_color(&kind.0, record)
}

/// [`parameter_set_color`] by the record's type.
fn signalled_color(kind: &[u8; 4], record: &[u8]) -> Option<Color> {
    let signal = match kind {
        b"avcC" => bitstream::avc_signal(record),
        b"hvcC" => bitstream::hevc_signal(record),
        b"av1C" => bitstream::av1_signal(record),
        _ => None,
    }?;
    let (primaries, transfer, matrix) = signal.colour.map_or((None, None, None), |(p, t, m)| {
        (Some(u16::from(p)), Some(u16::from(t)), Some(u16::from(m)))
    });
    Some(Color {
        primaries,
        transfer,
        matrix,
        full_range: signal.full_range,
    })
}

fn parse_colr(payload: &[u8], color: &mut Color) {
    let mut p = Bytes::new(payload, 0);
    let Ok(kind) = p.fourcc() else { return };
    if kind != FourCc(*b"nclx") && kind != FourCc(*b"nclc") {
        return;
    }
    let (Ok(primaries), Ok(transfer), Ok(matrix)) = (p.u16(), p.u16(), p.u16()) else {
        return;
    };
    color.primaries = Some(primaries);
    color.transfer = Some(transfer);
    color.matrix = Some(matrix);
    if kind == FourCc(*b"nclx") {
        if let Ok(flag) = p.u8() {
            color.full_range = Some(flag & 0x80 != 0);
        }
    }
}

/// The `DecoderConfigDescriptor` of an `esds` box (ISO/IEC 14496-1 7.2.6.5
/// and 7.2.6.6).
struct Esds {
    object_type: u8,
    /// `bufferSizeDB`, `maxBitrate` and `avgBitrate`, only when the whole
    /// descriptor fits inside the `ES_Descriptor` and the box.
    rates: Option<(u32, u32, u32)>,
    /// The `AudioSpecificConfig` read from the `DecoderSpecificInfo`.
    aac: Option<Profile>,
    /// Its output channels and SBR signalling.
    layout: Option<AacLayout>,
}

/// What an `AudioSpecificConfig` says about its output channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AacLayout {
    /// The channels of its `channelConfiguration`, or of its program config
    /// element when that is 0 (ISO/IEC 14496-3, 4.4.1.1); `None` when
    /// neither can be read.
    channels: Option<u32>,
    /// The backward compatible extension rules out parametric stereo
    /// explicitly: no SBR (`sbrPresentFlag` 0), or SBR without parametric
    /// stereo (`psPresentFlag` 0).
    no_ps: bool,
    /// The output sampling frequency it declares: the extension frequency
    /// when SBR is signalled explicitly (hierarchically, or in the backward
    /// compatible extension), else the core frequency (ISO/IEC 14496-3
    /// 1.6.2.1, 1.6.5); `None` for a reserved index. Implicitly signalled
    /// SBR is in the bitstream only, which probe does not decode.
    rate: Option<u32>,
}

/// The `objectTypeIndication` and `DecoderSpecificInfo` of an `esds`
/// box's payload (ISO/IEC 14496-1 7.2.6.5 and 7.2.6.6): for AAC, the
/// `AudioSpecificConfig` a decoder needs. The information is `None` when
/// the descriptor is truncated or lacks it.
pub fn esds_decoder_specific_info(esds: &[u8]) -> Option<(u8, Option<&[u8]>)> {
    esds_parts(esds).map(|(object_type, _, specific)| (object_type, specific))
}

fn parse_esds(payload: &[u8]) -> Option<Esds> {
    let (object_type, rates, specific) = esds_parts(payload)?;
    Some(Esds {
        object_type,
        rates,
        aac: specific.and_then(aac_profile),
        layout: specific.and_then(aac_layout),
    })
}

type EsdsParts<'a> = (u8, Option<(u32, u32, u32)>, Option<&'a [u8]>);

/// The object type, rates and decoder-specific information of an `esds`
/// payload.
fn esds_parts(payload: &[u8]) -> Option<EsdsParts<'_>> {
    let mut at = 4usize;
    let (tag, len) = descriptor(payload, &mut at)?;
    if tag != 3 {
        return None;
    }
    let end = at.checked_add(len)?.min(payload.len());
    let flags = *payload.get(at.checked_add(2)?)?;
    at = at.checked_add(3)?;
    if flags & 0x80 != 0 {
        at = at.checked_add(2)?;
    }
    if flags & 0x40 != 0 {
        let url_len = usize::from(*payload.get(at)?);
        at = at.checked_add(1 + url_len)?;
    }
    if flags & 0x20 != 0 {
        at = at.checked_add(2)?;
    }
    while at < end {
        let (tag, len) = descriptor(payload, &mut at)?;
        let body_end = at.checked_add(len)?;
        if tag == 4 {
            // The decoder configuration must lie inside its `ES_Descriptor`;
            // a truncated one still identifies the codec, but its rates and
            // decoder-specific information are not reported.
            let complete = body_end <= end;
            let body = payload.get(at..body_end.min(end))?;
            let mut b = Bytes::new(body, 0);
            let object_type = b.u8().ok()?;
            let rates = match (b.u8(), b.u24(), b.u32(), b.u32()) {
                (Ok(_), Ok(buffer_size), Ok(maximum), Ok(average)) if complete => {
                    Some((buffer_size, maximum, average))
                }
                _ => None,
            };
            let mut inner = 13usize;
            let mut specific = None;
            while rates.is_some() && inner < body.len() {
                let Some((tag, len)) = descriptor(body, &mut inner) else {
                    break;
                };
                let Some(stop) = inner.checked_add(len) else {
                    break;
                };
                if tag == 5 {
                    specific = body.get(inner..stop);
                    break;
                }
                inner = stop;
            }
            return Some((object_type, rates, specific));
        }
        at = body_end;
    }
    None
}

/// Reads a descriptor tag and its expandable size (ISO/IEC 14496-1 8.3.3).
fn descriptor(data: &[u8], at: &mut usize) -> Option<(u8, usize)> {
    let tag = *data.get(*at)?;
    *at = at.checked_add(1)?;
    let mut len = 0usize;
    for _ in 0..4 {
        let byte = *data.get(*at)?;
        *at = at.checked_add(1)?;
        len = (len << 7) | usize::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some((tag, len));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record's fields at their bit positions; cut short, none.
    #[test]
    fn dolby_vision_record() {
        // Version 1.0, profile 8, level 4, RPU and base layer present, base
        // layer compatibility 4.
        let record = [1, 0, 0x10, 0x25, 0x40, 0, 0, 0];
        assert_eq!(
            dolby_vision(&record),
            Some(DolbyVision {
                version_major: 1,
                version_minor: 0,
                profile: 8,
                level: 4,
                rpu_present: true,
                el_present: false,
                bl_present: true,
                bl_signal_compatibility_id: 4,
            })
        );
        assert_eq!(dolby_vision(&record[..4]), None);
    }

    /// Packs `(value, width)` fields most significant bit first.
    fn pack(fields: &[(u32, u32)]) -> Vec<u8> {
        let mut bits: Vec<bool> = Vec::new();
        for &(value, width) in fields {
            for i in (0..width).rev() {
                bits.push(value >> i & 1 == 1);
            }
        }
        bits.chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |b, (i, bit)| b | (u8::from(*bit) << (7 - i)))
            })
            .collect()
    }

    fn aac(config: &[u8]) -> Option<(u8, bool, bool)> {
        match aac_profile(config)? {
            Profile::Aac {
                object_type,
                sbr,
                ps,
            } => Some((object_type, sbr, ps)),
            _ => None,
        }
    }

    #[test]
    fn aac_object_types_and_explicit_signalling() {
        // LC, 44.1 kHz, stereo: 0x1210.
        assert_eq!(aac(&[0x12, 0x10]), Some((2, false, false)));
        // Explicit hierarchical: object type 5, then 24 kHz core, then LC.
        let he = pack(&[(5, 5), (6, 4), (2, 4), (3, 4), (2, 5)]);
        assert_eq!(aac(&he), Some((2, true, false)));
        let he2 = pack(&[(29, 5), (6, 4), (1, 4), (3, 4), (2, 5)]);
        assert_eq!(aac(&he2), Some((2, true, true)));
        // Escape: 31 then 6 bits (32 + 7 = 39, ELD).
        let eld = pack(&[(31, 5), (7, 6), (3, 4), (2, 4)]);
        assert_eq!(aac(&eld), Some((39, false, false)));
        // Backward compatible: LC GASpecificConfig, then 0x2B7, object type
        // 5, sbrPresentFlag, extension frequency, 0x548 and psPresentFlag.
        let compatible = pack(&[
            (2, 5),
            (6, 4),
            (1, 4),
            (0, 3),
            (0x2b7, 11),
            (5, 5),
            (1, 1),
            (3, 4),
            (0x548, 11),
            (1, 1),
        ]);
        assert_eq!(aac(&compatible), Some((2, true, true)));
        // An explicit frequency (index 15 plus 24 bits) is skipped.
        let explicit_rate = pack(&[(2, 5), (15, 4), (44_100, 24), (2, 4), (0, 3)]);
        assert_eq!(aac(&explicit_rate), Some((2, false, false)));
        // channelConfiguration 0 (program config element): no extension read.
        assert_eq!(
            aac(&pack(&[(2, 5), (4, 4), (0, 4)])),
            Some((2, false, false))
        );
        assert_eq!(aac(&[0x12]), None);
        assert_eq!(aac(&[]), None);
    }

    #[test]
    fn aac_channel_configuration() {
        let channels = |config: &[u8]| aac_layout(config).and_then(|l| l.channels);
        // LC, 44.1 kHz: channelConfiguration 2, then 6 (5.1) and 7 (7.1).
        assert_eq!(channels(&[0x12, 0x10]), Some(2));
        let surround = pack(&[(2, 5), (3, 4), (6, 4), (0, 3)]);
        assert_eq!(channels(&surround), Some(6));
        let escaped = pack(&[(31, 5), (7, 6), (15, 4), (48_000, 24), (7, 4)]);
        assert_eq!(channels(&escaped), Some(8));
        assert_eq!(aac_layout(&[0x12]), None);
        assert_eq!(configured_channels(6), Some(6));
        assert_eq!(configured_channels(7), Some(8));
        assert_eq!(configured_channels(11), Some(7));
        assert_eq!(configured_channels(13), Some(24));
        for none in [0, 8, 9, 10, 15] {
            assert_eq!(configured_channels(none), None);
        }
    }

    #[test]
    fn aac_program_config_element_channels() {
        // LC, 48 kHz, channelConfiguration 0, then GASpecificConfig and a
        // program config element: 2 front elements (one single, one pair),
        // 0 side, 1 back pair, 1 LFE: 1 + 2 + 2 + 1 = 6 channels.
        let pce = pack(&[
            (2, 5),
            (3, 4),
            (0, 4),
            (0, 3),
            (0, 4),
            (1, 2),
            (3, 4),
            (2, 4),
            (0, 4),
            (1, 4),
            (1, 2),
            (0, 3),
            (0, 4),
            (0, 1),
            (0, 1),
            (0, 1),
            (0, 1),
            (0, 4),
            (1, 1),
            (1, 4),
            (1, 1),
            (2, 4),
            (0, 4),
        ]);
        assert_eq!(aac_layout(&pce).and_then(|l| l.channels), Some(6));
        // Cut short inside the element: unknown.
        assert_eq!(aac_layout(&pce[..4]).and_then(|l| l.channels), None);
    }

    /// The output frequency: the core's, from the table or after the
    /// escape; the extension's with explicit SBR, hierarchical or in the
    /// backward compatible extension; none for a reserved index.
    #[test]
    fn aac_output_rates() {
        let rate = |fields: &[(u32, u32)]| aac_layout(&pack(fields)).and_then(|l| l.rate);
        // LC, index 3 (48 kHz), stereo.
        assert_eq!(rate(&[(2, 5), (3, 4), (2, 4), (0, 3)]), Some(48000));
        // The escape: 24 bits of frequency.
        assert_eq!(
            rate(&[(2, 5), (15, 4), (37800, 24), (2, 4), (0, 3)]),
            Some(37800)
        );
        // Reserved index 13.
        assert_eq!(rate(&[(2, 5), (13, 4), (2, 4), (0, 3)]), None);
        // Hierarchical SBR (object type 5): core 24 kHz, extension 48 kHz.
        assert_eq!(rate(&[(5, 5), (6, 4), (2, 4), (3, 4), (2, 5)]), Some(48000));
        // LC 24 kHz with the backward compatible extension, SBR present at
        // 48 kHz.
        assert_eq!(
            rate(&[
                (2, 5),
                (6, 4),
                (2, 4),
                (0, 3),
                (0x2b7, 11),
                (5, 5),
                (1, 1),
                (3, 4)
            ]),
            Some(48000)
        );
        // The same with SBR absent: the core's.
        assert_eq!(
            rate(&[(2, 5), (6, 4), (2, 4), (0, 3), (0x2b7, 11), (5, 5), (0, 1)]),
            Some(24000)
        );
    }

    #[test]
    fn aac_explicit_no_sbr() {
        // LC mono, then the backward compatible extension with
        // sbrPresentFlag 0: no parametric stereo possible.
        let no_sbr = pack(&[(2, 5), (4, 4), (1, 4), (0, 3), (0x2b7, 11), (5, 5), (0, 1)]);
        assert_eq!(
            aac_layout(&no_sbr),
            Some(AacLayout {
                channels: Some(1),
                no_ps: true,
                rate: Some(44100),
            })
        );
        // Without the extension, or with SBR present: not known.
        assert!(
            !aac_layout(&pack(&[(2, 5), (4, 4), (1, 4), (0, 3)]))
                .unwrap()
                .no_ps
        );
        let sbr = pack(&[
            (2, 5),
            (4, 4),
            (1, 4),
            (0, 3),
            (0x2b7, 11),
            (5, 5),
            (1, 1),
            (3, 4),
        ]);
        assert!(!aac_layout(&sbr).unwrap().no_ps);
        // SBR, then 0x548 with psPresentFlag 0: no parametric stereo; with
        // psPresentFlag 1 it is explicit, not ruled out.
        let with_ps = |flag| {
            pack(&[
                (2, 5),
                (4, 4),
                (1, 4),
                (0, 3),
                (0x2b7, 11),
                (5, 5),
                (1, 1),
                (3, 4),
                (0x548, 11),
                (flag, 1),
            ])
        };
        assert!(aac_layout(&with_ps(0)).unwrap().no_ps);
        assert!(!aac_layout(&with_ps(1)).unwrap().no_ps);
        // An escaped extension frequency (index 15, 24 bits) is skipped.
        let escaped = pack(&[
            (2, 5),
            (4, 4),
            (1, 4),
            (0, 3),
            (0x2b7, 11),
            (5, 5),
            (1, 1),
            (15, 4),
            (48_000, 24),
            (0x548, 11),
            (0, 1),
        ]);
        assert!(aac_layout(&escaped).unwrap().no_ps);
    }

    #[test]
    fn every_prefix_of_every_record_is_safe() {
        let records: [&[u8]; 6] = [
            &[
                1, 0x42, 0xc0, 0x1e, 0xff, 0xe1, 0, 4, 0x67, 0x42, 0xc0, 0x1e,
            ],
            &[
                1, 0x21, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 0x5d, 0xf0, 0, 0xfc,
            ],
            &[0x81, 0x05, 0x0c, 0, 0x0a, 0x0b, 0, 0, 0, 0x2d, 0x4c],
            &[1, 0, 0, 0, 0, 10, 0x82, 1, 1, 1, 0, 0],
            &[0x13, 0x10, 0x56, 0xe5, 0x98],
            &[
                0, 0, 0, 0, 3, 25, 0, 0, 0, 4, 17, 0x40, 0x15, 0, 1, 0, 0, 1, 0xf4, 0, 0, 1, 0xf4,
                0, 5, 2, 0x12, 0x10,
            ],
        ];
        for record in records {
            for end in 0..=record.len() {
                let prefix = &record[..end];
                let _ = avc_profile(prefix);
                let _ = hevc_profile(prefix);
                let _ = av1_profile(prefix);
                let _ = vp9_profile(prefix);
                let _ = aac_profile(prefix);
                let _ = aac_layout(prefix);
                let _ = parse_esds(prefix);
            }
        }
    }

    #[test]
    fn esds_decoder_config_fields() {
        let payload = [
            0, 0, 0, 0, 3, 25, 0, 0, 0, 4, 17, 0x40, 0x15, 0, 1, 0, 0, 1, 0xf4, 0, 0, 1, 0xf3, 0,
            5, 2, 0x12, 0x10,
        ];
        let esds = parse_esds(&payload).unwrap();
        assert_eq!(esds.object_type, 0x40);
        assert_eq!(esds.rates, Some((256, 128_000, 127_744)));
        assert_eq!(
            esds.aac,
            Some(Profile::Aac {
                object_type: 2,
                sbr: false,
                ps: false
            })
        );
    }

    #[test]
    fn incomplete_decoder_config_reports_no_rates() {
        // Only objectTypeIndication: the codec is identified, no rates.
        let short = [0, 0, 0, 0, 3, 6, 0, 0, 0, 4, 1, 0x40];
        let esds = parse_esds(&short).unwrap();
        assert_eq!((esds.object_type, esds.rates, esds.aac), (0x40, None, None));
        // The decoder configuration claims 17 bytes but its ES_Descriptor
        // ends after 8: bounded by the parent, so no rates either.
        let overrun = [
            0, 0, 0, 0, 3, 8, 0, 0, 0, 4, 17, 0x40, 0x15, 0, 1, 0, 0, 1, 0xf4, 0, 0, 1, 0xf3, 0, 5,
            2, 0x12, 0x10,
        ];
        let esds = parse_esds(&overrun).unwrap();
        assert_eq!((esds.object_type, esds.rates, esds.aac), (0x40, None, None));
    }

    #[test]
    fn parameter_sets_win_over_record_headers() {
        // avcC says Baseline level 4.1 without constraint flags; its SPS
        // says constraint_set0/1 and level 3.0 (ITU-T H.264 7.3.2.1.1).
        let avcc = [1, 66, 0, 41, 0xff, 0xe1, 0, 4, 0x67, 66, 0xc0, 30];
        assert_eq!(
            avc_profile(&avcc),
            (
                Some(Profile::H264 {
                    profile_idc: 66,
                    constraint_flags: 0xc0,
                    level_idc: 30
                }),
                false
            )
        );
        let consistent = [1, 66, 0xc0, 30, 0xff, 0xe1, 0, 4, 0x67, 66, 0xc0, 30];
        assert!(avc_profile(&consistent).1);
        // Header only (no SPS).
        assert_eq!(
            avc_profile(&[1, 100, 0, 30, 0xff, 0xe0, 0]),
            (
                Some(Profile::H264 {
                    profile_idc: 100,
                    constraint_flags: 0,
                    level_idc: 30
                }),
                true
            )
        );
        assert_eq!(avc_profile(&[0, 100, 0, 30]), (None, true));
        // hvcC header says Main level 3.0 (90); its SPS says level 3.1 (93).
        let mut hvcc = vec![1, 0x01, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 90];
        hvcc.extend_from_slice(&[0; 9]);
        hvcc.push(1);
        hvcc.extend_from_slice(&[33, 0, 1, 0, 15, 0x42, 0x01, 0x01, 0x01, 0x60, 0, 0, 0]);
        hvcc.extend_from_slice(&[0x90, 0, 0, 0, 0, 0, 93]);
        let (profile, consistent) = hevc_profile(&hvcc);
        assert!(!consistent);
        assert_eq!(
            profile,
            Some(Profile::Hevc {
                profile_space: 0,
                high_tier: false,
                profile_idc: 1,
                compatibility_flags: 0x6000_0000,
                constraint_flags: 0x9000_0000_0000,
                level_idc: 93
            })
        );
        hvcc.truncate(22);
        hvcc.push(0);
        assert_eq!(
            hevc_profile(&hvcc)
                .0
                .map(|p| matches!(p, Profile::Hevc { level_idc: 90, .. })),
            Some(true)
        );
        // av1C says level 0; a reduced still-picture sequence header says
        // profile 0, level 5.
        let sequence = pack(&[(0, 3), (1, 1), (1, 1), (5, 5), (0, 6)]);
        let mut av1c = vec![0x81, 0x00, 0x0c, 0, 0x0a, sequence.len() as u8];
        av1c.extend_from_slice(&sequence);
        let (profile, consistent) = av1_profile(&av1c);
        assert!(!consistent);
        assert!(matches!(
            profile,
            Some(Profile::Av1 {
                profile: 0,
                level_idx: 5,
                high_tier: false,
                ..
            })
        ));
    }

    #[test]
    fn prores_profiles_follow_the_entry_type() {
        assert_eq!(prores_profile(FourCc(*b"apco")), Some(ProResProfile::Proxy));
        assert_eq!(
            prores_profile(FourCc(*b"ap4x")),
            Some(ProResProfile::P4444Xq)
        );
        assert_eq!(prores_profile(FourCc(*b"avc1")), None);
    }

    #[test]
    fn records_without_parameter_sets_are_not_malformed() {
        // An av1C header without configuration OBUs.
        let av1 = describe_config_record(FourCc(*b"av1C"), &[0x81, 0x09, 0x0c, 0x00]).unwrap();
        assert!(!av1.malformed && !av1.parameter_set);
        assert!(matches!(
            av1.profile,
            Some(Profile::Av1 { level_idx: 9, .. })
        ));
        // An avcC with no sequence parameter set (avc3 style).
        let avc =
            describe_config_record(FourCc(*b"avcC"), &[1, 100, 0, 40, 0xFF, 0xE0, 0]).unwrap();
        assert!(!avc.malformed && !avc.parameter_set);
        // One that claims a parameter set it does not hold is malformed.
        let broken =
            describe_config_record(FourCc(*b"avcC"), &[1, 100, 0, 40, 0xFF, 0xE1, 0, 9]).unwrap();
        assert!(broken.malformed);
        assert!(describe_config_record(FourCc(*b"vpcC"), &[]).is_none());
    }
}
