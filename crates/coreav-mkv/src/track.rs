//! Tracks: `TrackEntry` (RFC 9559 5.1.4), codec identification by `CodecID`
//! (the Matroska codec mappings) and the codec allowlist.

use crate::Error;
use crate::ebml::{self, ElementHeader};
use crate::error::{Warning, warn};
use crate::ids;
use crate::reader::{Children, Counter};
use coreav_isobmff::{Color, FourCc, Profile};

/// A codec, identified from a track's `CodecID`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    /// `V_MPEG4/ISO/AVC`.
    H264,
    /// `V_MPEGH/ISO/HEVC`.
    Hevc,
    /// `V_AV1`.
    Av1,
    /// `V_VP8`.
    Vp8,
    /// `V_VP9`.
    Vp9,
    /// `V_PRORES`.
    ProRes,
    /// `V_MPEG1`.
    Mpeg1Video,
    /// `V_MPEG2`.
    Mpeg2Video,
    /// `V_MPEG4/ISO/SP`, `V_MPEG4/ISO/ASP`, `V_MPEG4/ISO/AP`.
    Mpeg4,
    /// `V_THEORA`.
    Theora,
    /// `V_MJPEG`.
    Mjpeg,
    /// `V_FFV1`.
    Ffv1,
    /// `A_AAC` and the legacy `A_AAC/MPEG2/...` and `A_AAC/MPEG4/...` IDs.
    Aac,
    /// `A_MPEG/L3`.
    Mp3,
    /// `A_MPEG/L2`.
    Mp2,
    /// `A_OPUS`.
    Opus,
    /// `A_VORBIS`.
    Vorbis,
    /// `A_FLAC`.
    Flac,
    /// `A_AC3`, `A_AC3/BSID9`, `A_AC3/BSID10`.
    Ac3,
    /// `A_EAC3`.
    Eac3,
    /// `A_DTS`, `A_DTS/EXPRESS`, `A_DTS/LOSSLESS`.
    Dts,
    /// `A_TRUEHD`.
    TrueHd,
    /// `A_ALAC`.
    Alac,
    /// `A_PCM/INT/LIT`, `A_PCM/INT/BIG`, `A_PCM/FLOAT/IEEE`.
    Pcm,
    /// `S_TEXT/UTF8` and `S_TEXT/ASCII` (SubRip text).
    SubRip,
    /// `S_TEXT/SSA`, `S_TEXT/ASS`.
    Ass,
    /// `S_TEXT/WEBVTT` and the WebM `D_WEBVTT/...` IDs.
    WebVtt,
    /// `S_HDMV/PGS`.
    Pgs,
    /// `S_VOBSUB`.
    VobSub,
    /// `S_DVBSUB`.
    DvbSub,
    /// Anything else; on no allowlist but [`Allowlist::any`].
    Other,
}

impl Codec {
    /// The codec a `CodecID` names.
    pub fn from_codec_id(id: &str) -> Self {
        match id {
            "V_MPEG4/ISO/AVC" => Self::H264,
            "V_MPEGH/ISO/HEVC" => Self::Hevc,
            "V_AV1" => Self::Av1,
            "V_VP8" => Self::Vp8,
            "V_VP9" => Self::Vp9,
            "V_PRORES" => Self::ProRes,
            "V_MPEG1" => Self::Mpeg1Video,
            "V_MPEG2" => Self::Mpeg2Video,
            "V_MPEG4/ISO/SP" | "V_MPEG4/ISO/ASP" | "V_MPEG4/ISO/AP" => Self::Mpeg4,
            "V_THEORA" => Self::Theora,
            "V_MJPEG" => Self::Mjpeg,
            "V_FFV1" => Self::Ffv1,
            "A_MPEG/L3" => Self::Mp3,
            "A_MPEG/L2" => Self::Mp2,
            "A_OPUS" => Self::Opus,
            "A_VORBIS" => Self::Vorbis,
            "A_FLAC" => Self::Flac,
            "A_AC3" | "A_AC3/BSID9" | "A_AC3/BSID10" => Self::Ac3,
            "A_EAC3" => Self::Eac3,
            "A_DTS" | "A_DTS/EXPRESS" | "A_DTS/LOSSLESS" => Self::Dts,
            "A_TRUEHD" => Self::TrueHd,
            "A_ALAC" => Self::Alac,
            "A_PCM/INT/LIT" | "A_PCM/INT/BIG" | "A_PCM/FLOAT/IEEE" => Self::Pcm,
            "S_TEXT/UTF8" | "S_TEXT/ASCII" => Self::SubRip,
            "S_TEXT/SSA" | "S_TEXT/ASS" => Self::Ass,
            "S_TEXT/WEBVTT" => Self::WebVtt,
            "S_HDMV/PGS" => Self::Pgs,
            "S_VOBSUB" => Self::VobSub,
            "S_DVBSUB" => Self::DvbSub,
            _ if id == "A_AAC" || id.starts_with("A_AAC/") => Self::Aac,
            _ if id.starts_with("D_WEBVTT/") => Self::WebVtt,
            _ => Self::Other,
        }
    }

    /// The codec a Video for Windows `biCompression` code names, for the
    /// codecs CoreAV names.
    pub fn from_vfw_fourcc(fourcc: [u8; 4]) -> Self {
        match &fourcc.map(|b| b.to_ascii_uppercase()) {
            b"DIVX" | b"DX50" | b"XVID" | b"FMP4" | b"MP4V" => Self::Mpeg4,
            b"H264" | b"X264" | b"AVC1" => Self::H264,
            b"MJPG" => Self::Mjpeg,
            b"VP80" => Self::Vp8,
            b"VP90" => Self::Vp9,
            b"AV01" => Self::Av1,
            _ => Self::Other,
        }
    }

    /// CoreAV's name for the codec, as `--allow` takes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
            Self::Vp8 => "vp8",
            Self::Vp9 => "vp9",
            Self::ProRes => "prores",
            Self::Mpeg1Video => "mpeg1video",
            Self::Mpeg2Video => "mpeg2video",
            Self::Mpeg4 => "mpeg4",
            Self::Theora => "theora",
            Self::Mjpeg => "mjpeg",
            Self::Ffv1 => "ffv1",
            Self::Aac => "aac",
            Self::Mp3 => "mp3",
            Self::Mp2 => "mp2",
            Self::Opus => "opus",
            Self::Vorbis => "vorbis",
            Self::Flac => "flac",
            Self::Ac3 => "ac3",
            Self::Eac3 => "eac3",
            Self::Dts => "dts",
            Self::TrueHd => "truehd",
            Self::Alac => "alac",
            Self::Pcm => "pcm",
            Self::SubRip => "subrip",
            Self::Ass => "ass",
            Self::WebVtt => "webvtt",
            Self::Pgs => "pgs",
            Self::VobSub => "vobsub",
            Self::DvbSub => "dvbsub",
            Self::Other => "other",
        }
    }

    /// Every named codec, `Other` excluded.
    pub const ALL: [Self; 30] = [
        Self::H264,
        Self::Hevc,
        Self::Av1,
        Self::Vp8,
        Self::Vp9,
        Self::ProRes,
        Self::Mpeg1Video,
        Self::Mpeg2Video,
        Self::Mpeg4,
        Self::Theora,
        Self::Mjpeg,
        Self::Ffv1,
        Self::Aac,
        Self::Mp3,
        Self::Mp2,
        Self::Opus,
        Self::Vorbis,
        Self::Flac,
        Self::Ac3,
        Self::Eac3,
        Self::Dts,
        Self::TrueHd,
        Self::Alac,
        Self::Pcm,
        Self::SubRip,
        Self::Ass,
        Self::WebVtt,
        Self::Pgs,
        Self::VobSub,
        Self::DvbSub,
    ];

    /// Whether decode order equals presentation order for every stream of
    /// this codec, so a frame's decode time is its presentation time: audio
    /// and subtitle codecs, and video codecs that store one shown frame per
    /// block in presentation order (VP8, VP9, AV1 temporal units, and the
    /// intra-only codecs).
    pub fn decodes_in_presentation_order(self) -> bool {
        !matches!(
            self,
            Self::H264
                | Self::Hevc
                | Self::Mpeg1Video
                | Self::Mpeg2Video
                | Self::Mpeg4
                | Self::Theora
                | Self::Other
        )
    }
}

/// The set of codecs a caller accepts, and what happens to a track outside
/// it: by default the track is dropped (left out of
/// [`crate::Segment::tracks`], its blocks never exposed, and reported in
/// [`crate::Segment::dropped`]); in strict mode the whole file is rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowlist {
    codecs: Vec<Codec>,
    strict: bool,
}

impl Allowlist {
    /// An allowlist of exactly these codecs, dropping other tracks. `Other`
    /// is never allowed here (only [`Allowlist::any`] lets it through).
    pub fn new(codecs: &[Codec]) -> Self {
        Self {
            codecs: codecs
                .iter()
                .copied()
                .filter(|c| *c != Codec::Other)
                .collect(),
            strict: false,
        }
    }

    /// Every named codec (all of [`Codec::ALL`]).
    pub fn all() -> Self {
        Self::new(&Codec::ALL)
    }

    /// Every track, `Other` included: for a caller that only describes the
    /// file and its packets (`coreav ffprobe`), never for one that hands
    /// frames to a decoder.
    pub fn any() -> Self {
        let mut codecs = Codec::ALL.to_vec();
        codecs.push(Codec::Other);
        Self {
            codecs,
            strict: false,
        }
    }

    /// The same allowlist in strict mode.
    #[must_use]
    pub fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Whether a disallowed track rejects the whole file.
    pub fn is_strict(&self) -> bool {
        self.strict
    }

    /// Whether a codec is allowed.
    pub fn allows(&self, codec: Codec) -> bool {
        self.codecs.contains(&codec)
    }

    /// The allowed codecs.
    pub fn codecs(&self) -> &[Codec] {
        &self.codecs
    }
}

impl Default for Allowlist {
    /// The codecs in CoreAV's scope that Matroska carries, the audio CoreAV
    /// copies into MP4 (FLAC, AC-3, E-AC-3, Apple Lossless), WebM's own
    /// codecs (VP8, Vorbis) and the text subtitle formats
    /// (`docs/design/mkv.md`).
    fn default() -> Self {
        Self::new(&[
            Codec::H264,
            Codec::Hevc,
            Codec::Av1,
            Codec::Vp8,
            Codec::Vp9,
            Codec::ProRes,
            Codec::Aac,
            Codec::Mp3,
            Codec::Opus,
            Codec::Vorbis,
            Codec::Flac,
            Codec::Ac3,
            Codec::Eac3,
            Codec::Alac,
            Codec::Pcm,
            Codec::SubRip,
            Codec::Ass,
            Codec::WebVtt,
        ])
    }
}

/// What a track carries, from `TrackType` (RFC 9559 Table 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    /// 1.
    Video,
    /// 2.
    Audio,
    /// 17.
    Subtitle,
    /// Any other value: complex, logo, buttons, control, metadata.
    Data,
}

/// The `Video` element of a track (RFC 9559 5.1.4.1.28).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Video {
    /// `PixelWidth`.
    pub pixel_width: Option<u64>,
    /// `PixelHeight`.
    pub pixel_height: Option<u64>,
    /// `DisplayWidth`, as stored.
    pub display_width: Option<u64>,
    /// `DisplayHeight`, as stored.
    pub display_height: Option<u64>,
    /// `DisplayUnit` (0 pixels, 3 display aspect ratio).
    pub display_unit: u64,
    /// `PixelCropTop`, `PixelCropBottom`, `PixelCropLeft`, `PixelCropRight`.
    pub crop: [u64; 4],
    /// `FlagInterlaced` (0 undetermined, 1 interlaced, 2 progressive).
    pub interlaced: u64,
    /// `StereoMode`.
    pub stereo_mode: u64,
    /// `AlphaMode`.
    pub alpha_mode: u64,
    /// `Colour\BitsPerChannel`, when present and not 0.
    pub bits_per_channel: Option<u64>,
    /// `FieldOrder` (default 2, undetermined), meaningful when
    /// `FlagInterlaced` is 1.
    pub field_order: u64,
    /// `Colour\ChromaSitingHorz` and `Colour\ChromaSitingVert`, when
    /// present (0 unspecified, 1 left or top collocated, 2 half).
    pub chroma_siting: (Option<u64>, Option<u64>),
    /// `StereoMode` was present.
    pub stereo_mode_present: bool,
    /// `Colour\MaxCLL` and `Colour\MaxFALL` in cd/m², when either is
    /// stored (the other is 0).
    pub light_level: Option<(u64, u64)>,
    /// `Colour\MasteringMetadata`: the red, green and blue primaries and
    /// the white point as CIE 1931 x, y pairs, then the maximum and minimum
    /// luminance in cd/m²; each `None` when not stored.
    pub mastering: Option<[Option<f64>; 10]>,
    /// `FrameRate` (deprecated, RFC 9559 5.1.4.1.28.40) in frames per
    /// second, when stored, finite and positive.
    pub frame_rate: Option<f64>,
    /// `Projection\ProjectionType`.
    pub projection_type: u64,
    /// A `Projection` element was present.
    pub projection_present: bool,
    /// `ProjectionPoseYaw`, `ProjectionPosePitch`, `ProjectionPoseRoll` in
    /// degrees; roll is counter-clockwise (RFC 9559 15.2).
    pub pose: [f64; 3],
}

impl Video {
    /// The clockwise rotation for presentation (0, 90, 180 or 270): 0 when
    /// the pose is all zero, else the roll when the projection is a pure
    /// quarter-turn roll of a rectangular image; `None` otherwise.
    pub fn rotation(&self) -> Option<u16> {
        let [yaw, pitch, roll] = self.pose;
        if yaw == 0.0 && pitch == 0.0 && roll == 0.0 {
            return Some(0);
        }
        if self.projection_type != 0 || yaw != 0.0 || pitch != 0.0 || !roll.is_finite() {
            return None;
        }
        if roll.fract() != 0.0 || !(-180.0..=180.0).contains(&roll) {
            return None;
        }
        // In range and integral, so the conversion is exact.
        #[allow(clippy::cast_possible_truncation)]
        let ccw = (roll as i32).rem_euclid(360);
        if ccw % 90 != 0 {
            return None;
        }
        u16::try_from((360 - ccw) % 360).ok()
    }

    /// The display aspect ratio the track declares: `DisplayWidth` by
    /// `DisplayHeight` in pixels or as a ratio (`DisplayUnit` 0 or 3).
    pub fn display_aspect(&self) -> Option<(u64, u64)> {
        if self.display_unit != 0 && self.display_unit != 3 {
            return None;
        }
        Some((self.display_width?, self.display_height?)).filter(|(w, h)| *w > 0 && *h > 0)
    }
}

/// The `Audio` element of a track (RFC 9559 5.1.4.1.29).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Audio {
    /// `SamplingFrequency` in Hz (default 8000).
    pub sampling_frequency: f64,
    /// `OutputSamplingFrequency` in Hz, when present.
    pub output_sampling_frequency: Option<f64>,
    /// `Channels` (default 1).
    pub channels: u64,
    /// `BitDepth`, when present.
    pub bit_depth: Option<u64>,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            sampling_frequency: 8000.0,
            output_sampling_frequency: None,
            channels: 1,
            bit_depth: None,
        }
    }
}

impl Audio {
    /// The sampling frequency as an integer number of Hz, when it is one.
    pub fn sample_rate(&self) -> Option<u32> {
        whole_hz(self.sampling_frequency)
    }

    /// The output sampling frequency (`OutputSamplingFrequency`, "used for
    /// SBR techniques", RFC 9559 5.1.4.1.29.2), else the sampling
    /// frequency, as an integer number of Hz when it is one.
    pub fn output_sample_rate(&self) -> Option<u32> {
        match self.output_sampling_frequency {
            Some(rate) => whole_hz(rate),
            None => self.sample_rate(),
        }
    }
}

fn whole_hz(rate: f64) -> Option<u32> {
    if rate.fract() != 0.0 || !(1.0..=f64::from(u32::MAX)).contains(&rate) {
        return None;
    }
    // In range and integral, so the conversion is exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(rate as u32)
}

/// How a track's frames are stored (`ContentEncodings`, RFC 9559
/// 5.1.4.1.31).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// Frames are stored as they are.
    None,
    /// Header stripping (`ContentCompAlgo` 3): every frame starts with these
    /// bytes, which are not stored.
    HeaderStripping(Vec<u8>),
    /// Frames are encrypted (`ContentEncodingType` 1); packets are listed
    /// as stored, and their bytes are opaque.
    Encrypted,
    /// Frames are compressed with zlib (`ContentCompAlgo` 0): packets are
    /// listed with their stored byte ranges and their inflated sizes
    /// ([`crate::Packet::inflated_size`]); their stored bytes are not the
    /// frames a decoder reads.
    Zlib,
    /// Compression other than header stripping and zlib, a chain of
    /// encodings, or an encoding of the codec private data: the track has no
    /// packets.
    Unsupported,
}

impl Encoding {
    /// The stored bytes of the track's frames are not the frames a decoder
    /// reads (encrypted or zlib-compressed), so nothing is parsed from them.
    pub fn opaque(&self) -> bool {
        matches!(self, Self::Encrypted | Self::Zlib)
    }
}

/// One track of the segment.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// Position in [`crate::Segment::tracks`].
    pub index: usize,
    /// Position among all `TrackEntry` elements, dropped ones included.
    pub stream_index: usize,
    /// `TrackNumber`, which blocks refer to.
    pub number: u64,
    /// `TrackUID`.
    pub uid: Option<u64>,
    /// `TrackType`, as stored.
    pub track_type: u64,
    /// What the track carries.
    pub kind: TrackKind,
    /// `CodecID`.
    pub codec_id: String,
    /// The codec it names.
    pub codec: Codec,
    /// `CodecPrivate`.
    pub codec_private: Option<Vec<u8>>,
    /// The `biCompression` code of a `V_MS/VFW/FOURCC` track's
    /// `BITMAPINFOHEADER`, which names its codec.
    pub fourcc: Option<[u8; 4]>,
    /// Width and height of an image subtitle track, from the `size:` line of
    /// a VobSub `CodecPrivate` (the Matroska codec mappings, `S_VOBSUB`).
    pub image_size: Option<(u32, u32)>,
    /// Profile and level from the codec private data; for VP9 without one,
    /// the profile and bit depth of the first frame header (level 0).
    pub profile: Option<Profile>,
    /// The profile the first frame header gives: the VP9 `Profile` or the
    /// VP8 `version` (VP8 and VP9 tracks only).
    pub frame_profile: Option<u8>,
    /// The chroma subsampling the first VP9 key frame header gives:
    /// horizontal and vertical, 1 when halved (4:2:0 is `(1, 1)`).
    pub chroma_subsampling: Option<(u8, u8)>,
    /// Color code points (ITU-T H.273) from `Colour`; values `Colour` does
    /// not give come from the codec private data (HEVC, AV1) or the first
    /// frame header (VP9).
    pub color: Color,
    /// The code points of the `Colour` element alone, before anything
    /// from the codec private data or the first frame fills them.
    pub colour: Color,
    /// `Name`.
    pub name: Option<String>,
    /// `LanguageBCP47`, else `Language` (default `eng`).
    pub language: String,
    /// `Language` (ISO 639-2) as stored, when the track has one, even when
    /// `LanguageBCP47` overrides it.
    pub language_iso639: Option<String>,
    /// `FlagEnabled`.
    pub enabled: bool,
    /// `FlagDefault`.
    pub default: bool,
    /// `FlagForced`.
    pub forced: bool,
    /// `FlagHearingImpaired`.
    pub hearing_impaired: bool,
    /// `FlagVisualImpaired`.
    pub visual_impaired: bool,
    /// `FlagTextDescriptions`.
    pub text_descriptions: bool,
    /// `FlagOriginal`.
    pub original: bool,
    /// `FlagCommentary`.
    pub commentary: bool,
    /// `FlagLacing`.
    pub lacing: bool,
    /// `DefaultDuration` in nanoseconds.
    pub default_duration: Option<u64>,
    /// `CodecDelay` in nanoseconds.
    pub codec_delay: u64,
    /// `SeekPreRoll` in nanoseconds.
    pub seek_pre_roll: u64,
    /// `TrackTimestampScale` is 1.0, the only value this crate applies.
    pub timestamp_scale_one: bool,
    /// How frames are stored.
    pub encoding: Encoding,
    /// `Video`, for video tracks.
    pub video: Option<Video>,
    /// `Audio`, for audio tracks.
    pub audio: Option<Audio>,
    /// Recoverable problems in this track.
    pub warnings: Vec<Warning>,
}

impl Track {
    /// Fills the colors `Colour` left unknown from a codec configuration
    /// record (`Colour` wins), and warns when the record is malformed.
    pub(crate) fn fill_from_record(&mut self, described: &coreav_isobmff::ConfigRecord) {
        let (color, from) = (&mut self.color, described.color);
        color.primaries = color.primaries.or(from.primaries);
        color.transfer = color.transfer.or(from.transfer);
        color.matrix = color.matrix.or(from.matrix);
        color.full_range = color.full_range.or(from.full_range);
        if described.malformed {
            warn(&mut self.warnings, Warning::MalformedCodecPrivate);
        }
    }

    /// Whether the track's packets can be listed.
    pub fn packets_available(&self) -> bool {
        self.encoding != Encoding::Unsupported
    }
}

/// A track left out because its codec is not allowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedTrack {
    /// Position among all `TrackEntry` elements.
    pub stream_index: usize,
    /// `TrackNumber`.
    pub number: u64,
    /// `CodecID`.
    pub codec_id: String,
    /// The codec it names.
    pub codec: Codec,
    /// From `TrackType`.
    pub kind: TrackKind,
}

/// Parses one `TrackEntry` held in memory. `index` and `stream_index` are
/// filled by the caller.
pub(crate) fn parse_track_entry(
    data: &[u8],
    base: u64,
    entry: &ElementHeader,
    max_size_len: u8,
    counter: &Counter,
) -> Result<Track, Error> {
    let mut track = Track {
        index: 0,
        stream_index: 0,
        number: 0,
        uid: None,
        track_type: 0,
        kind: TrackKind::Data,
        codec_id: String::new(),
        codec: Codec::Other,
        codec_private: None,
        fourcc: None,
        image_size: None,
        profile: None,
        frame_profile: None,
        chroma_subsampling: None,
        color: Color::default(),
        colour: Color::default(),
        name: None,
        language: "eng".to_owned(),
        language_iso639: None,
        enabled: true,
        default: true,
        forced: false,
        hearing_impaired: false,
        visual_impaired: false,
        text_descriptions: false,
        original: false,
        commentary: false,
        lacing: true,
        default_duration: None,
        codec_delay: 0,
        seek_pre_roll: 0,
        timestamp_scale_one: true,
        encoding: Encoding::None,
        video: None,
        audio: None,
        warnings: Vec::new(),
    };
    let mut bcp47 = None;
    for child in Children::new(data, base, max_size_len, counter) {
        let (h, d) = child?;
        match h.id {
            ids::TRACK_NUMBER => track.number = ebml::uint(d, &h)?,
            ids::TRACK_UID => track.uid = Some(ebml::uint(d, &h)?),
            ids::TRACK_TYPE => track.track_type = ebml::uint(d, &h)?,
            ids::FLAG_ENABLED => track.enabled = ebml::uint(d, &h)? != 0,
            ids::FLAG_DEFAULT => track.default = ebml::uint(d, &h)? != 0,
            ids::FLAG_FORCED => track.forced = ebml::uint(d, &h)? != 0,
            // The flags read for listings only: a malformed one is ignored,
            // as it was before they were read.
            ids::FLAG_HEARING_IMPAIRED => track.hearing_impaired = flag(d, &h),
            ids::FLAG_VISUAL_IMPAIRED => track.visual_impaired = flag(d, &h),
            ids::FLAG_TEXT_DESCRIPTIONS => track.text_descriptions = flag(d, &h),
            ids::FLAG_ORIGINAL => track.original = flag(d, &h),
            ids::FLAG_COMMENTARY => track.commentary = flag(d, &h),
            ids::FLAG_LACING => track.lacing = ebml::uint(d, &h)? != 0,
            ids::DEFAULT_DURATION => {
                track.default_duration = Some(ebml::uint(d, &h)?).filter(|v| *v > 0);
            }
            ids::TRACK_TIMESTAMP_SCALE => {
                let scale = ebml::float(d, &h)?;
                // An empty element means the default, 1.0 (RFC 8794 6.1).
                track.timestamp_scale_one = d.is_empty() || scale == 1.0;
            }
            ids::NAME => track.name = Some(ebml::string(d)),
            ids::LANGUAGE => {
                track.language = ebml::string(d);
                track.language_iso639 = Some(track.language.clone());
            }
            ids::LANGUAGE_BCP47 => bcp47 = Some(ebml::string(d)),
            ids::CODEC_ID => track.codec_id = ebml::string(d),
            ids::CODEC_PRIVATE => track.codec_private = Some(d.to_vec()),
            ids::CODEC_DELAY => track.codec_delay = ebml::uint(d, &h)?,
            ids::SEEK_PRE_ROLL => track.seek_pre_roll = ebml::uint(d, &h)?,
            ids::VIDEO => {
                let (video, color) = parse_video(d, h.data_offset(), max_size_len, counter)?;
                track.video = Some(video);
                track.color = color;
                track.colour = color;
            }
            ids::AUDIO => {
                track.audio = Some(parse_audio(d, h.data_offset(), max_size_len, counter)?)
            }
            ids::CONTENT_ENCODINGS => {
                track.encoding = parse_encodings(d, h.data_offset(), max_size_len, counter)?;
            }
            _ => {}
        }
    }
    if track.number == 0 {
        return Err(Error::InvalidElement {
            offset: entry.offset,
            id: entry.id,
            reason: "TrackEntry without a TrackNumber",
        });
    }
    if let Some(tag) = bcp47 {
        track.language = tag;
    }
    if track.language.is_empty() {
        // An empty element takes the default value (RFC 8794 6.1).
        track.language = "eng".to_owned();
    }
    track.kind = match track.track_type {
        1 => TrackKind::Video,
        2 => TrackKind::Audio,
        17 => TrackKind::Subtitle,
        _ => TrackKind::Data,
    };
    if track.kind != TrackKind::Video {
        track.video = None;
        track.color = Color::default();
        track.colour = Color::default();
    }
    if track.kind != TrackKind::Audio {
        track.audio = None;
    } else if track.audio.is_none() {
        track.audio = Some(Audio::default());
    }
    if !track.timestamp_scale_one {
        warn(&mut track.warnings, Warning::UnsupportedTrackTimestampScale);
    }
    match track.encoding {
        Encoding::Unsupported => warn(&mut track.warnings, Warning::UnsupportedEncoding),
        Encoding::Encrypted => warn(&mut track.warnings, Warning::Encrypted),
        Encoding::Zlib => warn(&mut track.warnings, Warning::ZlibCompressed),
        _ => {}
    }
    track.codec = Codec::from_codec_id(&track.codec_id);
    if track.codec_id == "V_MS/VFW/FOURCC" {
        // BITMAPINFOHEADER: biCompression at byte 16.
        track.fourcc = track
            .codec_private
            .as_deref()
            .and_then(|p| p.get(16..20))
            .and_then(|f| <[u8; 4]>::try_from(f).ok());
        track.codec = track.fourcc.map_or(Codec::Other, Codec::from_vfw_fourcc);
    }
    if track.codec == Codec::VobSub {
        track.image_size = track.codec_private.as_deref().and_then(vobsub_size);
    }
    describe_private(&mut track);
    Ok(track)
}

/// Profile and color range from the codec private data, which holds the
/// same records as an ISO base media sample entry for these codecs (the
/// Matroska codec mappings: `AVCDecoderConfigurationRecord`,
/// `HEVCDecoderConfigurationRecord`, `AV1CodecConfigurationRecord`,
/// `AudioSpecificConfig`, and the WebM VP9 codec feature metadata).
fn describe_private(track: &mut Track) {
    let Some(private) = track.codec_private.as_deref() else {
        track.profile = legacy_aac_profile(&track.codec_id);
        return;
    };
    let record = match track.codec {
        Codec::H264 => FourCc(*b"avcC"),
        Codec::Hevc => FourCc(*b"hvcC"),
        Codec::Av1 => FourCc(*b"av1C"),
        Codec::Aac => {
            track.profile = coreav_isobmff::describe_audio_specific_config(private);
            if track.profile.is_none() {
                warn(&mut track.warnings, Warning::MalformedCodecPrivate);
            }
            return;
        }
        Codec::Vp9 => {
            match vp9_features(private) {
                Some(profile) => track.profile = profile,
                None => warn(&mut track.warnings, Warning::MalformedCodecPrivate),
            }
            return;
        }
        _ => return,
    };
    // An AV1 CodecPrivate may be empty only when the bitstream carries the
    // sequence header; there is nothing to describe then.
    if private.is_empty() {
        return;
    }
    if let Some(described) = coreav_isobmff::describe_config_record(record, private) {
        track.profile = described.profile;
        track.fill_from_record(&described);
    }
}

/// The AAC profile a legacy codec ID names (the Matroska codec mappings,
/// `A_AAC/MPEG2/...` and `A_AAC/MPEG4/...`); those tracks have no
/// `CodecPrivate`.
fn legacy_aac_profile(codec_id: &str) -> Option<Profile> {
    let rest = codec_id
        .strip_prefix("A_AAC/MPEG2/")
        .or_else(|| codec_id.strip_prefix("A_AAC/MPEG4/"))?;
    let (object_type, sbr) = match rest {
        "MAIN" => (1, false),
        "LC" => (2, false),
        "LC/SBR" => (2, true),
        "SSR" => (3, false),
        "LTP" => (4, false),
        _ => return None,
    };
    Some(Profile::Aac {
        object_type,
        sbr,
        ps: false,
    })
}

/// The `size: WxH` line of a VobSub `CodecPrivate`, which holds lines of
/// the `.idx` file.
fn vobsub_size(private: &[u8]) -> Option<(u32, u32)> {
    let text = core::str::from_utf8(private).ok()?;
    let line = text.lines().find_map(|l| l.trim().strip_prefix("size:"))?;
    let (w, h) = line.trim().split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?)).filter(|(w, h)| *w > 0 && *h > 0)
}

/// The VP9 codec feature metadata of a WebM `CodecPrivate` (WebM container
/// guidelines): a list of (ID, length, value) features, 1 profile, 2 level,
/// 3 bit depth. `Some(None)` when it names no profile; `None` when it is
/// malformed.
fn vp9_features(data: &[u8]) -> Option<Option<Profile>> {
    let (mut profile, mut level, mut depth) = (None, None, None);
    let mut rest = data;
    while let [id, len, tail @ ..] = rest {
        let value = tail.get(..usize::from(*len))?;
        let byte = (*len == 1).then(|| value.first().copied()).flatten();
        match id {
            1 => profile = byte,
            2 => level = byte,
            3 => depth = byte,
            _ => {}
        }
        rest = tail.get(usize::from(*len)..)?;
    }
    if !rest.is_empty() {
        return None;
    }
    Some(profile.map(|profile| Profile::Vp9 {
        profile,
        level: level.unwrap_or(0),
        bit_depth: depth.unwrap_or(0),
    }))
}

fn parse_video(
    data: &[u8],
    base: u64,
    max_size_len: u8,
    counter: &Counter,
) -> Result<(Video, Color), Error> {
    let mut video = Video {
        field_order: 2,
        ..Video::default()
    };
    let mut color = Color::default();
    for child in Children::new(data, base, max_size_len, counter) {
        let (h, d) = child?;
        match h.id {
            ids::PIXEL_WIDTH => video.pixel_width = Some(ebml::uint(d, &h)?),
            ids::PIXEL_HEIGHT => video.pixel_height = Some(ebml::uint(d, &h)?),
            ids::DISPLAY_WIDTH => video.display_width = Some(ebml::uint(d, &h)?),
            ids::DISPLAY_HEIGHT => video.display_height = Some(ebml::uint(d, &h)?),
            ids::DISPLAY_UNIT => video.display_unit = ebml::uint(d, &h)?,
            ids::PIXEL_CROP_TOP => video.crop[0] = ebml::uint(d, &h)?,
            ids::PIXEL_CROP_BOTTOM => video.crop[1] = ebml::uint(d, &h)?,
            ids::PIXEL_CROP_LEFT => video.crop[2] = ebml::uint(d, &h)?,
            ids::PIXEL_CROP_RIGHT => video.crop[3] = ebml::uint(d, &h)?,
            ids::FLAG_INTERLACED => video.interlaced = ebml::uint(d, &h)?,
            // The elements below are read for listings only: a malformed
            // one is ignored, as it was before they were read.
            ids::FIELD_ORDER => video.field_order = ebml::uint(d, &h).unwrap_or(2),
            ids::FRAME_RATE => {
                video.frame_rate = ebml::float(d, &h)
                    .ok()
                    .filter(|r| r.is_finite() && *r > 0.0);
            }
            ids::STEREO_MODE => {
                video.stereo_mode = ebml::uint(d, &h)?;
                video.stereo_mode_present = true;
            }
            ids::ALPHA_MODE => video.alpha_mode = ebml::uint(d, &h)?,
            ids::COLOUR => parse_colour(
                d,
                h.data_offset(),
                max_size_len,
                counter,
                &mut video,
                &mut color,
            )?,
            ids::PROJECTION => {
                video.projection_present = true;
                parse_projection(d, h.data_offset(), max_size_len, counter, &mut video)?
            }
            _ => {}
        }
    }
    Ok((video, color))
}

/// `Colour` (RFC 9559 5.1.4.1.28.16): the ITU-T H.273 code points, with
/// "unspecified" (2) and reserved values left unknown, and `Range` 1
/// (broadcast) or 2 (full).
fn parse_colour(
    data: &[u8],
    base: u64,
    max_size_len: u8,
    counter: &Counter,
    video: &mut Video,
    color: &mut Color,
) -> Result<(), Error> {
    let code = |value: u64, known: &[u64]| {
        known
            .contains(&value)
            .then(|| u16::try_from(value).ok())
            .flatten()
    };
    for child in Children::new(data, base, max_size_len, counter) {
        let (h, d) = child?;
        match h.id {
            ids::MATRIX_COEFFICIENTS => {
                color.matrix = code(
                    ebml::uint(d, &h)?,
                    &[0, 1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14],
                );
            }
            ids::TRANSFER_CHARACTERISTICS => {
                color.transfer = code(
                    ebml::uint(d, &h)?,
                    &[1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18],
                );
            }
            ids::PRIMARIES => {
                color.primaries = code(ebml::uint(d, &h)?, &[1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 22]);
            }
            ids::RANGE => {
                color.full_range = match ebml::uint(d, &h)? {
                    1 => Some(false),
                    2 => Some(true),
                    _ => None,
                };
            }
            // The elements below are read for listings only: a malformed
            // one is ignored, as it was before they were read.
            ids::CHROMA_SITING_HORZ => video.chroma_siting.0 = ebml::uint(d, &h).ok(),
            ids::CHROMA_SITING_VERT => video.chroma_siting.1 = ebml::uint(d, &h).ok(),
            ids::MAX_CLL => {
                if let Ok(v) = ebml::uint(d, &h) {
                    video.light_level = Some((v, video.light_level.map_or(0, |l| l.1)));
                }
            }
            ids::MAX_FALL => {
                if let Ok(v) = ebml::uint(d, &h) {
                    video.light_level = Some((video.light_level.map_or(0, |l| l.0), v));
                }
            }
            ids::MASTERING_METADATA => {
                let mut values = [None; 10];
                // Its own small budget: the elements of the file's counter
                // are not spent on what was skipped before it was read.
                let own = Counter::new(MASTERING_ELEMENTS);
                for child in Children::new(d, h.data_offset(), max_size_len, &own) {
                    let Ok((h, d)) = child else { break };
                    // PrimaryRChromaticityX (0x55D1) to LuminanceMin (0x55DA).
                    if let Some(slot) =
                        h.id.checked_sub(0x55D1)
                            .and_then(|i| values.get_mut(usize::try_from(i).ok()?))
                    {
                        *slot = ebml::float(d, &h).ok();
                    }
                }
                video.mastering = Some(values);
            }
            ids::BITS_PER_CHANNEL => {
                video.bits_per_channel = Some(ebml::uint(d, &h)?).filter(|v| *v > 0);
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_projection(
    data: &[u8],
    base: u64,
    max_size_len: u8,
    counter: &Counter,
    video: &mut Video,
) -> Result<(), Error> {
    for child in Children::new(data, base, max_size_len, counter) {
        let (h, d) = child?;
        match h.id {
            ids::PROJECTION_TYPE => video.projection_type = ebml::uint(d, &h)?,
            ids::PROJECTION_POSE_YAW => video.pose[0] = ebml::float(d, &h)?,
            ids::PROJECTION_POSE_PITCH => video.pose[1] = ebml::float(d, &h)?,
            ids::PROJECTION_POSE_ROLL => video.pose[2] = ebml::float(d, &h)?,
            _ => {}
        }
    }
    Ok(())
}

fn parse_audio(
    data: &[u8],
    base: u64,
    max_size_len: u8,
    counter: &Counter,
) -> Result<Audio, Error> {
    let mut audio = Audio::default();
    for child in Children::new(data, base, max_size_len, counter) {
        let (h, d) = child?;
        match h.id {
            ids::SAMPLING_FREQUENCY => {
                // An empty element takes the default (RFC 8794 6.1).
                if !d.is_empty() {
                    audio.sampling_frequency = ebml::float(d, &h)?;
                }
            }
            ids::OUTPUT_SAMPLING_FREQUENCY => {
                audio.output_sampling_frequency = Some(ebml::float(d, &h)?);
            }
            ids::CHANNELS => {
                if !d.is_empty() {
                    audio.channels = ebml::uint(d, &h)?;
                }
            }
            ids::BIT_DEPTH => audio.bit_depth = Some(ebml::uint(d, &h)?).filter(|v| *v > 0),
            _ => {}
        }
    }
    Ok(audio)
}

/// `ContentEncodings`: header stripping of frames is undone and zlib
/// compression of frames is sized; any other encoding makes the track's
/// packets unavailable.
fn parse_encodings(
    data: &[u8],
    base: u64,
    max_size_len: u8,
    counter: &Counter,
) -> Result<Encoding, Error> {
    let mut result = Encoding::None;
    for child in Children::new(data, base, max_size_len, counter) {
        let (h, d) = child?;
        if h.id != ids::CONTENT_ENCODING {
            continue;
        }
        if result != Encoding::None {
            // A chain of encodings is not undone.
            return Ok(Encoding::Unsupported);
        }
        let (mut scope, mut kind) = (1u64, 0u64);
        let (mut algo, mut settings, mut compression) = (0u64, Vec::new(), false);
        for child in Children::new(d, h.data_offset(), max_size_len, counter) {
            let (h, d) = child?;
            match h.id {
                ids::CONTENT_ENCODING_SCOPE => scope = ebml::uint(d, &h)?,
                ids::CONTENT_ENCODING_TYPE => kind = ebml::uint(d, &h)?,
                ids::CONTENT_COMPRESSION => {
                    compression = true;
                    for child in Children::new(d, h.data_offset(), max_size_len, counter) {
                        let (h, d) = child?;
                        match h.id {
                            ids::CONTENT_COMP_ALGO => algo = ebml::uint(d, &h)?,
                            ids::CONTENT_COMP_SETTINGS => settings = d.to_vec(),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        result = match (kind, scope) {
            (0, 1) if compression && algo == 3 => Encoding::HeaderStripping(settings),
            (0, 1) if compression && algo == 0 => Encoding::Zlib,
            (1, 1) => Encoding::Encrypted,
            _ => Encoding::Unsupported,
        };
    }
    Ok(result)
}

/// The most elements read of a `MasteringMetadata` (it defines ten).
const MASTERING_ELEMENTS: u32 = 64;

/// A flag element read for listings only: unset when malformed.
fn flag(data: &[u8], header: &ElementHeader) -> bool {
    ebml::uint(data, header).is_ok_and(|v| v != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_ids() {
        assert_eq!(Codec::from_codec_id("V_VP9"), Codec::Vp9);
        assert_eq!(Codec::from_codec_id("A_AAC/MPEG4/LC/SBR"), Codec::Aac);
        assert_eq!(Codec::from_codec_id("D_WEBVTT/SUBTITLES"), Codec::WebVtt);
        assert_eq!(Codec::from_codec_id("V_MS/VFW/FOURCC"), Codec::Other);
        for codec in Codec::ALL {
            assert_ne!(codec.name(), "other");
        }
        assert!(!Allowlist::new(&[Codec::Other]).allows(Codec::Other));
        assert!(Allowlist::any().allows(Codec::Other));
    }

    #[test]
    fn rotation_from_roll() {
        let mut video = Video::default();
        assert_eq!(video.rotation(), Some(0));
        video.pose[2] = 90.0;
        assert_eq!(video.rotation(), Some(270));
        video.pose[2] = -90.0;
        assert_eq!(video.rotation(), Some(90));
        video.pose[2] = 180.0;
        assert_eq!(video.rotation(), Some(180));
        video.pose[2] = 45.0;
        assert_eq!(video.rotation(), None);
        video.pose = [180.0, 0.0, 180.0];
        assert_eq!(video.rotation(), None);
        video.pose = [0.0; 3];
        video.projection_type = 1;
        assert_eq!(video.rotation(), Some(0));
        video.pose[2] = 90.0;
        assert_eq!(video.rotation(), None);
    }

    #[test]
    fn legacy_aac_ids() {
        let he = Profile::Aac {
            object_type: 2,
            sbr: true,
            ps: false,
        };
        assert_eq!(legacy_aac_profile("A_AAC/MPEG4/LC/SBR"), Some(he));
        assert_eq!(legacy_aac_profile("A_AAC/MPEG2/LC/SBR"), Some(he));
        assert!(matches!(
            legacy_aac_profile("A_AAC/MPEG2/MAIN"),
            Some(Profile::Aac { object_type: 1, .. })
        ));
        assert_eq!(legacy_aac_profile("A_AAC"), None);
    }

    #[test]
    fn vfw_and_vobsub() {
        assert_eq!(Codec::from_vfw_fourcc(*b"DX50"), Codec::Mpeg4);
        assert_eq!(Codec::from_vfw_fourcc(*b"xvid"), Codec::Mpeg4);
        assert_eq!(Codec::from_vfw_fourcc(*b"WMV3"), Codec::Other);
        assert_eq!(
            vobsub_size(b"# VobSub\nsize: 720x480\npalette: 0"),
            Some((720, 480))
        );
        assert_eq!(vobsub_size(b"size: 0x480"), None);
        assert_eq!(vobsub_size(&[0xff, 0xfe]), None);
    }

    #[test]
    fn vp9_feature_metadata() {
        assert_eq!(
            vp9_features(&[1, 1, 2, 2, 1, 31, 3, 1, 10]),
            Some(Some(Profile::Vp9 {
                profile: 2,
                level: 31,
                bit_depth: 10
            }))
        );
        assert_eq!(vp9_features(&[]), Some(None));
        assert_eq!(vp9_features(&[1, 4, 0]), None);
    }
}
