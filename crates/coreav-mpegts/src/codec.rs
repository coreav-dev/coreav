//! Codecs from stream types (ISO/IEC 13818-1 table 2-34) and descriptors:
//! the registration descriptor (2.6.8, with the format identifiers the
//! SMPTE registration authority lists), the ATSC stream types (A/53 and
//! A/52), and the DVB descriptors of private streams (ETSI EN 300 468
//! 6.2 and annex D).

use crate::section::Descriptor;

/// A codec a stream carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    /// ITU-T H.264 (stream type 0x1B, or 0x20 for an MVC sub-bitstream).
    H264,
    /// ITU-T H.265 (0x24, or registration `HEVC`).
    Hevc,
    /// ITU-T H.266 (0x33).
    Vvc,
    /// MPEG-1 video (0x01).
    Mpeg1Video,
    /// MPEG-2 video (0x02).
    Mpeg2Video,
    /// MPEG-4 part 2 visual (0x10).
    Mpeg4,
    /// AAC in ADTS (0x0F).
    Aac,
    /// AAC in LATM/LOAS (0x11).
    AacLatm,
    /// MPEG audio (0x03, 0x04); the layer is in the frames.
    MpegAudio,
    /// AC-3 (0x81, or a DVB AC-3 descriptor or registration `AC-3`).
    Ac3,
    /// E-AC-3 (0x87, or a DVB enhanced AC-3 descriptor or `EAC3`).
    Eac3,
    /// DTS (0x82, 0x85, 0x86, or a DVB DTS descriptor).
    Dts,
    /// Blu-ray LPCM (0x80 with registration `HDMV`).
    BlurayPcm,
    /// Opus (registration `Opus`, ETSI TS 102 366 annex).
    Opus,
    /// SCTE-35 splice information (0x86 without `HDMV`).
    Scte35,
    /// ID3 timed metadata (0x15 with registration `ID3 `).
    Id3,
    /// DVB subtitles (a subtitling descriptor).
    DvbSubtitle,
    /// DVB teletext (a teletext descriptor).
    DvbTeletext,
    /// Blu-ray presentation graphics (0x90).
    PgsSubtitle,
    /// ARIB closed captions (ARIB STD-B24), a private stream (0x06) whose
    /// data component descriptor (STD-B10 part 2 6.2.20) names the
    /// caption component.
    AribCaption,
    /// The DVB Event Information Table (ETSI EN 300 468 5.2.4), on PID
    /// 0x12. It is a table, not an elementary stream, but `ffprobe` lists
    /// it as a data stream and so does this reader, so that the two agree
    /// on what a broadcast capture holds.
    Epg,
    /// Anything else.
    Other,
}

/// What kind of media a stream holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    /// Pictures.
    Video,
    /// Sound.
    Audio,
    /// Subtitles.
    Subtitle,
    /// Data and metadata.
    Data,
}

impl Codec {
    /// Every codec this crate names, in the order the enumeration lists
    /// them; `Other` is not one of them, as it can never be allowed.
    pub const ALL: [Self; 21] = [
        Self::H264,
        Self::Hevc,
        Self::Vvc,
        Self::Mpeg1Video,
        Self::Mpeg2Video,
        Self::Mpeg4,
        Self::Aac,
        Self::AacLatm,
        Self::MpegAudio,
        Self::Ac3,
        Self::Eac3,
        Self::Dts,
        Self::BlurayPcm,
        Self::Opus,
        Self::Scte35,
        Self::Id3,
        Self::DvbSubtitle,
        Self::DvbTeletext,
        Self::PgsSubtitle,
        Self::AribCaption,
        Self::Epg,
    ];

    /// CoreAV's name for the codec, as `coreav probe` prints it and
    /// `--allow` takes it (`Other` never).
    pub fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Vvc => "vvc",
            Self::Mpeg1Video => "mpeg1video",
            Self::Mpeg2Video => "mpeg2video",
            Self::Mpeg4 => "mpeg4",
            Self::Aac => "aac",
            Self::AacLatm => "aac_latm",
            Self::MpegAudio => "mpeg_audio",
            Self::Ac3 => "ac3",
            Self::Eac3 => "eac3",
            Self::Dts => "dts",
            Self::BlurayPcm => "bluray_pcm",
            Self::Opus => "opus",
            Self::Scte35 => "scte35",
            Self::Id3 => "id3",
            Self::DvbSubtitle => "dvb_subtitle",
            Self::DvbTeletext => "dvb_teletext",
            Self::PgsSubtitle => "pgs",
            Self::AribCaption => "arib_caption",
            Self::Epg => "epg",
            Self::Other => "other",
        }
    }

    /// The kind of media the codec carries.
    pub fn kind(self) -> StreamKind {
        match self {
            Self::H264 | Self::Hevc | Self::Vvc | Self::Mpeg1Video | Self::Mpeg2Video => {
                StreamKind::Video
            }
            Self::Mpeg4 => StreamKind::Video,
            Self::Aac
            | Self::AacLatm
            | Self::MpegAudio
            | Self::Ac3
            | Self::Eac3
            | Self::Dts
            | Self::BlurayPcm
            | Self::Opus => StreamKind::Audio,
            Self::DvbSubtitle | Self::DvbTeletext | Self::PgsSubtitle | Self::AribCaption => {
                StreamKind::Subtitle
            }
            Self::Scte35 | Self::Id3 | Self::Epg | Self::Other => StreamKind::Data,
        }
    }

    /// The codecs whose PES payloads are split into one packet per frame.
    pub(crate) fn frames(self) -> Option<crate::frames::Syntax> {
        match self {
            Self::Aac => Some(crate::frames::Syntax::Adts),
            Self::MpegAudio => Some(crate::frames::Syntax::MpegAudio),
            Self::Ac3 | Self::Eac3 => Some(crate::frames::Syntax::Ac3),
            Self::AacLatm => Some(crate::frames::Syntax::Latm),
            Self::Opus => Some(crate::frames::Syntax::Opus),
            _ => None,
        }
    }
}

/// DVB descriptor tags (EN 300 468 table 12).
const DVB_TELETEXT: u8 = 0x56;
const DVB_SUBTITLING: u8 = 0x59;
const DVB_AC3: u8 = 0x6A;
const DVB_ENHANCED_AC3: u8 = 0x7A;
const DVB_DTS: u8 = 0x7B;
/// `registration_descriptor` (13818-1 2.6.8).
const REGISTRATION: u8 = 0x05;
/// ARIB descriptors (STD-B10 part 2): the stream identifier, whose
/// `component_tag` says what a private stream carries (table 6-5: 0x30 to
/// 0x37 captions, 0x87 a one-segment caption), and the data component
/// descriptor, whose `data_component_id` names the caption system of
/// STD-B24: 0x0008 for a broadcast service and 0x0012 for the
/// one-segment service of the same standard, which a broadcast capture
/// carries beside it.
const STREAM_IDENTIFIER: u8 = 0x52;
const DATA_COMPONENT: u8 = 0xFD;
const ARIB_CAPTION_COMPONENTS: [u16; 2] = [0x0008, 0x0012];

/// The ARIB caption profile a private stream's descriptors give it, as
/// `ffprobe` names it: `Profile A` for a caption component (STD-B24 part
/// 1, the broadcasting profile), `Profile C` for the one-segment
/// component. `None` when the descriptors do not make it a caption
/// stream.
pub fn arib_caption_profile(descriptors: &[Descriptor]) -> Option<&'static str> {
    let names = descriptors
        .iter()
        .filter(|d| d.tag == DATA_COMPONENT)
        .filter_map(|d| match *d.data.as_slice() {
            [high, low, ..] => Some(u16::from_be_bytes([high, low])),
            _ => None,
        })
        .any(|id| ARIB_CAPTION_COMPONENTS.contains(&id));
    if !names {
        return None;
    }
    let tag = descriptors
        .iter()
        .find(|d| d.tag == STREAM_IDENTIFIER)
        .and_then(|d| d.data.first())
        .copied()?;
    match tag {
        0x30..=0x37 => Some("Profile A"),
        0x87 => Some("Profile C"),
        _ => None,
    }
}
/// `metadata_pointer_descriptor` and `metadata_descriptor` (13818-1 2.6.58
/// and 2.6.60), which name a metadata stream's format. HLS signals ID3
/// timed metadata with them rather than with a registration descriptor
/// (RFC 8216 3.4).
const METADATA_POINTER: u8 = 0x25;
const METADATA: u8 = 0x26;

/// The `metadata_format_identifier` of a metadata descriptor, when it has
/// one. Both descriptors start with `metadata_application_format` (16
/// bits, followed by a 32-bit identifier when it is 0xFFFF) and
/// `metadata_format` (8 bits, followed by a 32-bit identifier when it is
/// 0xFF), so one reader serves both (13818-1 2.6.58, 2.6.60).
fn metadata_format(data: &[u8]) -> Option<[u8; 4]> {
    let application = u16::from_be_bytes([*data.first()?, *data.get(1)?]);
    let at = if application == 0xFFFF { 6 } else { 2 };
    if *data.get(at)? != 0xFF {
        return None;
    }
    let bytes = data.get(at.checked_add(1)?..at.checked_add(5)?)?;
    <[u8; 4]>::try_from(bytes).ok()
}

/// The `metadata_format_identifier`s the descriptors of a loop name.
fn metadata_formats(descriptors: &[Descriptor]) -> impl Iterator<Item = [u8; 4]> + '_ {
    descriptors
        .iter()
        .filter(|d| d.tag == METADATA || d.tag == METADATA_POINTER)
        .filter_map(|d| metadata_format(&d.data))
}

/// The four-byte identifier a stream's own descriptors give its format:
/// the registration descriptor's `format_identifier` (13818-1 2.6.8), else
/// the `metadata_format_identifier` of its metadata descriptor (2.6.60),
/// which is how HLS names ID3 timed metadata. A pointer descriptor
/// (2.6.58) is not one: it names the format of the stream it points at,
/// not of the stream whose loop it sits in.
pub(crate) fn format_identifier(stream: &[Descriptor]) -> Option<[u8; 4]> {
    registrations(stream).next().or_else(|| {
        stream
            .iter()
            .filter(|d| d.tag == METADATA)
            .find_map(|d| metadata_format(&d.data))
    })
}

/// The `format_identifier`s of a descriptor loop's registration
/// descriptors.
fn registrations(descriptors: &[Descriptor]) -> impl Iterator<Item = [u8; 4]> + '_ {
    descriptors
        .iter()
        .filter(|d| d.tag == REGISTRATION)
        .filter_map(|d| d.data.get(..4).and_then(|f| <[u8; 4]>::try_from(f).ok()))
}

/// The codec of a stream from its type, its descriptors and its program's.
pub(crate) fn codec(stream_type: u8, stream: &[Descriptor], program: &[Descriptor]) -> Codec {
    let registered = |id: &[u8; 4]| {
        registrations(stream)
            .chain(registrations(program))
            .any(|f| &f == id)
    };
    let has = |tag: u8| stream.iter().any(|d| d.tag == tag);
    // A metadata stream's own format descriptor.
    let metadata = |id: &[u8; 4]| metadata_formats(stream).any(|f| &f == id);
    let hdmv = registered(b"HDMV");
    if stream_type == 0x06 && arib_caption_profile(stream).is_some() {
        return Codec::AribCaption;
    }
    match stream_type {
        0x01 => Codec::Mpeg1Video,
        0x02 => Codec::Mpeg2Video,
        0x03 | 0x04 => Codec::MpegAudio,
        0x0F => Codec::Aac,
        0x10 => Codec::Mpeg4,
        0x11 => Codec::AacLatm,
        0x1B => Codec::H264,
        // The MVC video sub-bitstream of a Blu-ray 3D capture (table
        // 2-34, ITU-T H.264 annex H): H.264 all the same, though its
        // subset sequence parameter sets give no picture size.
        0x20 => Codec::H264,
        0x24 => Codec::Hevc,
        0x33 => Codec::Vvc,
        0x15 if registrations(stream).any(|f| &f == b"ID3 ") || metadata(b"ID3 ") => Codec::Id3,
        0x80 if hdmv => Codec::BlurayPcm,
        0x81 => Codec::Ac3,
        0x82 | 0x85 | 0x86 if hdmv => Codec::Dts,
        0x86 => Codec::Scte35,
        0x87 => Codec::Eac3,
        0x90 if hdmv => Codec::PgsSubtitle,
        0x06 => {
            if has(DVB_AC3) || registrations(stream).any(|f| &f == b"AC-3") {
                Codec::Ac3
            } else if has(DVB_ENHANCED_AC3) || registrations(stream).any(|f| &f == b"EAC3") {
                Codec::Eac3
            } else if has(DVB_DTS) {
                Codec::Dts
            } else if registrations(stream).any(|f| &f == b"Opus") {
                Codec::Opus
            } else if registrations(stream).any(|f| &f == b"HEVC") {
                Codec::Hevc
            } else if has(DVB_SUBTITLING) {
                Codec::DvbSubtitle
            } else if has(DVB_TELETEXT) {
                Codec::DvbTeletext
            } else {
                Codec::Other
            }
        }
        _ => Codec::Other,
    }
}

/// The codec of a private stream (type 0x06) that no descriptor names,
/// from its first PES packet: an ADTS, AC-3 or E-AC-3 frame header, or an
/// MPEG audio frame header in an audio stream (`stream_id` 0xC0 to 0xDF).
pub(crate) fn sniff(stream_id: u8, payload: &[u8]) -> Codec {
    use crate::frames::{Syntax, header};
    let audio_id = (0xC0..=0xDF).contains(&stream_id);
    if let Some(frame) = header(Syntax::Ac3, payload) {
        return if frame.detail == 1 {
            Codec::Eac3
        } else {
            Codec::Ac3
        };
    }
    // An ADTS header is a valid MPEG audio header too (layer bits 00 are
    // what tells them apart), so ADTS is tried first.
    if header(Syntax::Adts, payload).is_some() {
        return Codec::Aac;
    }
    if audio_id && header(Syntax::MpegAudio, payload).is_some() {
        return Codec::MpegAudio;
    }
    Codec::Other
}

/// Which codecs a caller accepts, and what happens to a stream outside it:
/// by default the stream is listed as dropped
/// ([`crate::TransportStream::dropped`]) and never read; in strict mode the
/// whole file is rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowlist {
    codecs: Vec<Codec>,
    strict: bool,
}

impl Allowlist {
    /// Accepts exactly `codecs` (`Other` never).
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

    /// The same allowlist in strict mode.
    #[must_use]
    pub fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Whether a stream outside the allowlist rejects the whole file.
    pub fn is_strict(&self) -> bool {
        self.strict
    }

    /// Every codec this crate names (all of [`Codec::ALL`]).
    pub fn all() -> Self {
        Self::new(&Codec::ALL)
    }

    /// Whether `codec` is accepted.
    pub fn allows(&self, codec: Codec) -> bool {
        self.codecs.contains(&codec)
    }

    /// Whether a stream the tables name as `codec` is listed and read.
    /// The allowlist decides, except that a codec this crate cannot name
    /// is always read, and that the stream type tells MPEG-1 from MPEG-2
    /// video unreliably (`ffmpeg` writes MPEG-1 video as type 0x02) and
    /// the sequence header decides, so a stream of either type is read
    /// when either is allowed and judged again on the codec its header
    /// gives it.
    pub(crate) fn reads(&self, codec: Codec) -> bool {
        match codec {
            // A codec this crate cannot name is always read: the tables
            // declare the stream, so a reader that left it out would
            // report a file with fewer streams than it has, and nothing of
            // its payload is parsed anyway — its access units are the PES
            // packets as stored. `--allow` cannot name it, so dropping it
            // could never be asked for either.
            Codec::Other => true,
            Codec::Mpeg1Video | Codec::Mpeg2Video => {
                self.allows(Codec::Mpeg1Video) || self.allows(Codec::Mpeg2Video)
            }
            other => self.allows(other),
        }
    }
}

impl Default for Allowlist {
    /// Every codec this crate names ([`Allowlist::all`]). A transport
    /// stream carries no duration of its own: the file's, and the list of
    /// what it holds, are derived from its streams, so leaving one out by
    /// default would change what the file is reported to be. A caller that
    /// wants fewer builds its own list.
    fn default() -> Self {
        Self::all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(tag: u8, data: &[u8]) -> Descriptor {
        Descriptor {
            tag,
            data: data.to_vec(),
        }
    }

    #[test]
    fn stream_types_and_descriptors() {
        assert_eq!(codec(0x1B, &[], &[]), Codec::H264);
        assert_eq!(codec(0x06, &[d(0x6A, &[0])], &[]), Codec::Ac3);
        assert_eq!(codec(0x06, &[d(0x05, b"EAC3")], &[]), Codec::Eac3);
        assert_eq!(codec(0x06, &[d(0x05, b"Opus")], &[]), Codec::Opus);
        assert_eq!(codec(0x06, &[d(0x59, &[])], &[]), Codec::DvbSubtitle);
        // ARIB captions: the data component descriptor names the caption
        // system of STD-B24 -- 0x0008 for a broadcast service and 0x0012
        // for the one-segment service beside it -- and the stream
        // identifier's component tag says which profile.
        let caption = |id: &[u8], tag: u8| {
            let stream = [d(0xFD, id), d(0x52, &[tag])];
            (codec(0x06, &stream, &[]), arib_caption_profile(&stream))
        };
        assert_eq!(
            caption(b"\x00\x08\x3d", 0x30),
            (Codec::AribCaption, Some("Profile A"))
        );
        assert_eq!(
            caption(b"\x00\x12\xad", 0x87),
            (Codec::AribCaption, Some("Profile C"))
        );
        // A superimpose component tag is not a caption, and neither is
        // another data component.
        assert_eq!(caption(b"\x00\x08\x3c", 0x38), (Codec::Other, None));
        assert_eq!(caption(b"\x00\x0c\x1f", 0x40), (Codec::Other, None));
        assert_eq!(codec(0x06, &[], &[]), Codec::Other);
        assert_eq!(codec(0x86, &[], &[]), Codec::Scte35);
        assert_eq!(codec(0x86, &[], &[d(0x05, b"HDMV")]), Codec::Dts);
        assert_eq!(codec(0x80, &[], &[]), Codec::Other);
        // HLS ID3 timed metadata: a metadata descriptor on the stream
        // (13818-1 2.6.60), not a registration descriptor.
        let meta = d(0x26, b"\xff\xffID3 \xffID3 \x00\x0f");
        assert_eq!(codec(0x15, std::slice::from_ref(&meta), &[]), Codec::Id3);
        // The program's pointer descriptor names the format of the stream
        // it points at, not of every stream of the program.
        let pointer = d(0x25, b"\xff\xffID3 \xffID3 \x00\x1f\x00\x01");
        assert_eq!(
            codec(0x15, &[], std::slice::from_ref(&pointer)),
            Codec::Other
        );
        // A metadata stream of another format is not ID3; a truncated
        // descriptor names nothing and does not panic.
        assert_eq!(
            codec(0x15, &[d(0x26, b"\x00\x10\xffKLVA\x00\x0f")], &[]),
            Codec::Other
        );
        assert_eq!(codec(0x15, &[d(0x26, b"\xff\xffID3")], &[]), Codec::Other);
        assert_eq!(
            format_identifier(std::slice::from_ref(&meta)),
            Some(*b"ID3 ")
        );
        assert_eq!(format_identifier(&[pointer]), None);
        assert_eq!(format_identifier(&[d(0x05, b"HEVC")]), Some(*b"HEVC"));
        assert!(!Allowlist::all().allows(Codec::Other));
        assert!(Allowlist::default().allows(Codec::Aac));
        assert_eq!(Allowlist::default(), Allowlist::all());
        // Every named codec has a distinct name, and `all` is `ALL`.
        let mut names: Vec<&str> = Codec::ALL.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Codec::ALL.len());
        assert!(!names.contains(&"other"));
        assert!(Codec::ALL.iter().all(|c| Allowlist::all().allows(*c)));
        // Either MPEG video type is read when either codec is allowed.
        // A codec the crate cannot name is read whatever the list says.
        assert!(Allowlist::new(&[]).reads(Codec::Other));
        assert!(!Allowlist::all().allows(Codec::Other));
        let mpeg1 = Allowlist::new(&[Codec::Mpeg1Video]);
        assert!(mpeg1.reads(Codec::Mpeg2Video) && mpeg1.reads(Codec::Mpeg1Video));
        assert!(!mpeg1.allows(Codec::Mpeg2Video));
        assert!(!mpeg1.reads(Codec::H264));
    }
}
