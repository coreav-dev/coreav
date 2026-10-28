//! Names `ffprobe` prints for things CoreAV identifies: codec long names,
//! channel layouts, sample formats. Taken from the pinned `ffprobe`'s own
//! listings (`ffprobe -codecs`, `ffprobe -layouts`), black box.

/// `codec_long_name` for a `codec_name`.
pub fn long_name(codec: &str) -> Option<&'static str> {
    Some(match codec {
        "av1" => "Alliance for Open Media AV1",
        "h264" => "H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10",
        "hevc" => "H.265 / HEVC (High Efficiency Video Coding)",
        "vvc" => "H.266 / VVC (Versatile Video Coding)",
        "mjpeg" => "Motion JPEG",
        "mpeg1video" => "MPEG-1 video",
        "mpeg2video" => "MPEG-2 video",
        "mpeg4" => "MPEG-4 part 2",
        "png" => "PNG (Portable Network Graphics) image",
        "prores" => "Apple ProRes (iCodec Pro)",
        "theora" => "Theora",
        "vp8" => "On2 VP8",
        "vp9" => "Google VP9",
        "aac" => "AAC (Advanced Audio Coding)",
        "aac_latm" => "AAC LATM (Advanced Audio Coding LATM syntax)",
        "ac3" => "ATSC A/52A (AC-3)",
        "alac" => "ALAC (Apple Lossless Audio Codec)",
        "dts" => "DCA (DTS Coherent Acoustics)",
        "eac3" => "ATSC A/52B (AC-3, E-AC-3)",
        "flac" => "FLAC (Free Lossless Audio Codec)",
        "mp1" => "MP1 (MPEG audio layer 1)",
        "mp2" => "MP2 (MPEG audio layer 2)",
        "mp3" => "MP3 (MPEG audio layer 3)",
        "opus" => "Opus (Opus Interactive Audio Codec)",
        "truehd" => "TrueHD",
        "vorbis" => "Vorbis",
        "pcm_alaw" => "PCM A-law / G.711 A-law",
        "pcm_bluray" => "PCM signed 16|20|24-bit big-endian for Blu-ray media",
        "pcm_dvd" => "PCM signed 20|24-bit big-endian",
        "pcm_f32be" => "PCM 32-bit floating point big-endian",
        "pcm_f32le" => "PCM 32-bit floating point little-endian",
        "pcm_f64be" => "PCM 64-bit floating point big-endian",
        "pcm_f64le" => "PCM 64-bit floating point little-endian",
        "pcm_mulaw" => "PCM mu-law / G.711 mu-law",
        "pcm_s16be" => "PCM signed 16-bit big-endian",
        "pcm_s16le" => "PCM signed 16-bit little-endian",
        "pcm_s24be" => "PCM signed 24-bit big-endian",
        "pcm_s24le" => "PCM signed 24-bit little-endian",
        "pcm_s32be" => "PCM signed 32-bit big-endian",
        "pcm_s32le" => "PCM signed 32-bit little-endian",
        "pcm_s64be" => "PCM signed 64-bit big-endian",
        "pcm_s64le" => "PCM signed 64-bit little-endian",
        "pcm_s8" => "PCM signed 8-bit",
        "pcm_u16be" => "PCM unsigned 16-bit big-endian",
        "pcm_u16le" => "PCM unsigned 16-bit little-endian",
        "pcm_u24be" => "PCM unsigned 24-bit big-endian",
        "pcm_u24le" => "PCM unsigned 24-bit little-endian",
        "pcm_u32be" => "PCM unsigned 32-bit big-endian",
        "pcm_u32le" => "PCM unsigned 32-bit little-endian",
        "pcm_u8" => "PCM unsigned 8-bit",
        "bin_data" => "binary data",
        "epg" => "Electronic Program Guide",
        "otf" => "OpenType font",
        "ttf" => "TrueType font",
        "scte_35" => "SCTE 35 Message Queue",
        "timed_id3" => "timed ID3 metadata",
        "arib_caption" => "ARIB STD-B24 caption",
        "ass" => "ASS (Advanced SSA) subtitle",
        "ssa" => "SSA (SubStation Alpha) subtitle",
        "dvb_subtitle" => "DVB subtitles",
        "dvb_teletext" => "DVB teletext",
        "eia_608" => "EIA-608 closed captions",
        "hdmv_pgs_subtitle" => "HDMV Presentation Graphic Stream subtitles",
        "mov_text" => "MOV text",
        "subrip" => "SubRip subtitle",
        "text" => "raw UTF-8 text",
        "webvtt" => "WebVTT subtitle",
        "8bps" => "QuickTime 8BPS video",
        "aic" => "Apple Intermediate Codec",
        "avrp" => "Avid 1:1 10-bit RGB Packer",
        "avui" => "Avid Meridien Uncompressed",
        "cfhd" => "GoPro CineForm HD",
        "cinepak" => "Cinepak",
        "dnxhd" => "VC3/DNxHD",
        "dvvideo" => "DV (Digital Video)",
        "gif" => "CompuServe GIF (Graphics Interchange Format)",
        "h261" => "H.261",
        "h263" => "H.263 / H.263-1996, H.263+ / H.263-1998 / H.263 version 2",
        "hap" => "Vidvox Hap",
        "jpeg2000" => "JPEG 2000",
        "mjpegb" => "Apple MJPEG-B",
        "qdraw" => "Apple QuickDraw",
        "qtrle" => "QuickTime Animation (RLE) video",
        "r210" => "Uncompressed RGB 10-bit",
        "rawvideo" => "raw video",
        "rpza" => "QuickTime video (RPZA)",
        "smc" => "QuickTime Graphics (SMC)",
        "svq1" => "Sorenson Vector Quantizer 1 / Sorenson Video 1 / SVQ1",
        "svq3" => "Sorenson Vector Quantizer 3 / Sorenson Video 3 / SVQ3",
        "tiff" => "TIFF image",
        "v210" => "Uncompressed 4:2:2 10-bit",
        "v308" => "Uncompressed packed 4:4:4",
        "v408" => "Uncompressed packed QT 4:4:4:4",
        "v410" => "Uncompressed 4:4:4 10-bit",
        "vc1" => "SMPTE VC-1",
        "y41p" => "Uncompressed YUV 4:1:1 12-bit",
        "yuv4" => "Uncompressed packed 4:2:0",
        "ac4" => "AC-4",
        "adpcm_ima_qt" => "ADPCM IMA QuickTime",
        "amr_nb" => "AMR-NB (Adaptive Multi-Rate NarrowBand)",
        "amr_wb" => "AMR-WB (Adaptive Multi-Rate WideBand)",
        "evrc" => "EVRC (Enhanced Variable Rate Codec)",
        "gsm" => "GSM",
        "ilbc" => "iLBC (Internet Low Bitrate Codec)",
        "mace3" => "MACE (Macintosh Audio Compression/Expansion) 3:1",
        "mace6" => "MACE (Macintosh Audio Compression/Expansion) 6:1",
        "qcelp" => "QCELP / PureVoice",
        "qdm2" => "QDesign Music Codec 2",
        "qdmc" => "QDesign Music",
        "ttml" => "Timed Text Markup Language",
        _ => return None,
    })
}

/// `ffprobe`'s codec name for an ISO base media sample entry type CoreAV
/// does not identify, with the MPEG-4 `objectTypeIndication` of its `esds`
/// when it has one (ISO/IEC 14496-1 7.2.6.6.2).
pub fn iso_codec(fourcc: &[u8; 4], object_type: Option<u8>) -> Option<&'static str> {
    if let Some(oti) = object_type {
        match oti {
            0x20 => return Some("mpeg4"),
            0x60..=0x65 => return Some("mpeg2video"),
            0x6a => return Some("mpeg1video"),
            0x6c => return Some("mjpeg"),
            0x40 | 0x66..=0x68 => return Some("aac"),
            0x69 | 0x6b => return Some("mp3"),
            0xa5 => return Some("ac3"),
            0xa6 => return Some("eac3"),
            _ => {}
        }
    }
    Some(match fourcc {
        b"mp4v" => "mpeg4",
        b"jpeg" | b"mjpa" | b"AVDJ" | b"dmb1" => "mjpeg",
        b"mjpb" => "mjpegb",
        b"png " => "png",
        b"tiff" => "tiff",
        b"gif " => "gif",
        b"SVQ1" | b"svq1" | b"svqi" => "svq1",
        b"SVQ3" => "svq3",
        b"rle " => "qtrle",
        b"raw " | b"yuv2" | b"2vuy" | b"yuvs" | b"L555" | b"L565" | b"b16g" | b"b48r" | b"b64a"
        | b"BGRA" => "rawvideo",
        b"v210" => "v210",
        b"v308" => "v308",
        b"v408" => "v408",
        b"v410" => "v410",
        b"r210" => "r210",
        b"dvc " | b"dvcp" | b"dvpp" | b"dv5n" | b"dv5p" | b"dvh5" | b"dvh6" | b"dvhq" | b"dvhp"
        | b"AVdv" | b"AVd1" => "dvvideo",
        b"h263" | b"s263" | b"H263" => "h263",
        b"cvid" => "cinepak",
        b"smc " => "smc",
        b"rpza" => "rpza",
        b"icod" => "aic",
        b"8BPS" => "8bps",
        b"qdrw" => "qdraw",
        b"CFHD" => "cfhd",
        b"AVdn" | b"AVdh" => "dnxhd",
        b"Hap1" | b"Hap5" | b"HapY" | b"HapM" | b"HapA" | b"Hap7" => "hap",
        b"mjp2" => "jpeg2000",
        b"vc-1" => "vc1",
        b"vp08" => "vp8",
        b"mp2v" | b"hdv1" | b"hdv2" | b"hdv3" | b"hdv5" | b"hdv6" | b"hdv7" | b"hdv8" | b"hdv9"
        | b"xdv1" | b"xdv2" | b"xdv3" | b"xdv4" | b"xdv5" | b"xdv6" | b"xdv7" | b"xdv8"
        | b"xdv9" | b"xdva" | b"xdvb" | b"xdvc" | b"xdvd" | b"xdve" | b"xdvf" | b"xd5a"
        | b"xd5b" | b"xd5c" | b"xd5d" | b"xd5e" | b"xd5f" | b"xd59" | b"xd54" | b"xd55"
        | b"xdhd" | b"xdh2" | b"AVmp" | b"mx3p" | b"mx4p" | b"mx5p" | b"mx3n" | b"mx4n"
        | b"mx5n" | b"m2v1" => "mpeg2video",
        b"mp1v" | b"m1v " | b"m1v1" => "mpeg1video",
        b"dvh1" | b"dvhe" => "hevc",
        b"dvav" | b"dva1" => "h264",
        b"ac-3" | b"sac3" => "ac3",
        b"ec-3" => "eac3",
        b"ac-4" => "ac4",
        b"alac" => "alac",
        b"samr" => "amr_nb",
        b"sawb" => "amr_wb",
        b"sevc" => "evrc",
        b"sqcp" | b"Qclp" | b"Qclq" => "qcelp",
        b"ulaw" => "pcm_mulaw",
        b"alaw" => "pcm_alaw",
        b"ima4" => "adpcm_ima_qt",
        b"QDM2" => "qdm2",
        b"QDMC" => "qdmc",
        b"MAC3" => "mace3",
        b"MAC6" => "mace6",
        b"agsm" => "gsm",
        b"ilbc" => "ilbc",
        b".mp3" | b"ms\x00\x55" => "mp3",
        b"dtsc" | b"dtsh" | b"dtsl" | b"dtse" => "dts",
        b"mlpa" => "mlp",
        b"fLaC" => "flac",
        b"c608" => "eia_608",
        b"gpmd" => "bin_data",
        b"stpp" => "ttml",
        _ => return None,
    })
}

/// The channel layout `ffprobe` names for a channel count when nothing
/// more specific is known.
pub fn default_layout(channels: u32) -> Option<&'static str> {
    Some(match channels {
        1 => "mono",
        2 => "stereo",
        3 => "2.1",
        4 => "4.0",
        5 => "5.0(side)",
        6 => "5.1(side)",
        7 => "6.1",
        8 => "7.1",
        _ => return None,
    })
}

/// The layout of an AAC `channelConfiguration` (ISO/IEC 14496-3 1.6.3.4),
/// as `ffprobe` names it.
pub fn aac_layout(config: u8) -> Option<&'static str> {
    Some(match config {
        1 => "mono",
        2 => "stereo",
        3 => "3.0",
        4 => "4.0",
        5 => "5.0",
        6 => "5.1",
        7 => "7.1",
        _ => return None,
    })
}

/// The sample format the decoder of `codec` puts out, which `ffprobe`
/// prints as `sample_fmt`.
pub fn sample_fmt(codec: &str, bits: Option<u16>) -> Option<&'static str> {
    Some(match codec {
        "aac" | "aac_latm" | "mp3" | "mp2" | "mp1" | "opus" | "vorbis" | "ac3" | "eac3" | "dts" => {
            "fltp"
        }
        "flac" | "alac" => match bits {
            Some(b) if b > 16 => "s32",
            _ => "s16",
        },
        "pcm_u8" => "u8",
        "pcm_s8" => "u8",
        "pcm_s16le" | "pcm_s16be" | "pcm_u16le" | "pcm_u16be" | "pcm_bluray" | "pcm_dvd" => "s16",
        "pcm_s24le" | "pcm_s24be" | "pcm_s32le" | "pcm_s32be" | "pcm_u24le" | "pcm_u24be"
        | "pcm_u32le" | "pcm_u32be" => "s32",
        "pcm_f32le" | "pcm_f32be" => "flt",
        "pcm_f64le" | "pcm_f64be" => "dbl",
        "pcm_s64le" | "pcm_s64be" => "s64",
        "pcm_alaw" | "pcm_mulaw" => "s16",
        "adpcm_ima_qt" => "s16p",
        _ => return None,
    })
}
