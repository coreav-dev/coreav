//! The tags, dispositions, extradata sizes and chapters `ffprobe` prints
//! for an ISO base media file, from the boxes `coreav_isobmff::metadata`
//! reads.
//!
//! Every rule here was derived black-box from the pinned `ffprobe` 7.0.2 on
//! crafted files (no FFmpeg source was read):
//!
//! - Format tags are one dictionary, filled in file order: each `ftyp`
//!   before the movie box (`major_brand`, `minor_version`,
//!   `compatible_brands`), the movie header's `creation_time`, then the
//!   metadata of every track box and of the movie box as the walk meets
//!   them (`udta` atoms, `ilst` items, `mdta` keys), then `timecode`.
//!   Setting a key that is already there removes it, moves the last entry
//!   into its place, and appends the new value ([`Dict`]): the order
//!   `ffprobe` prints after an overwrite.
//! - A QuickTime user data text atom (`©nam` and the others of [`atom`])
//!   holds a 16-bit size and a 16-bit language before its text when the
//!   size fits; else the whole payload is the text (`FIRM` is always
//!   whole). Text with a Macintosh language code (below 0x400, or 0x7fff)
//!   is Mac OS Roman. The tag is set under its name, then under
//!   `name-lang` when bytes follow the header and the language maps to a
//!   code other than `und`. A 3GPP `loci` atom is printed as ISO 6709.
//! - An item list entry's `data` box is text for type indicators 0, 1, 3
//!   and 4, a big-endian integer for 21 (signed) and 22 (unsigned) of 1 to
//!   4 bytes (other lengths print 0), a 32-bit float printed `%f` for 23,
//!   and left out otherwise; the keys of [`atom`] that hold numbers read
//!   their bytes whatever the type. Text stops at its first NUL.
//! - `creation_time` is the stored time less 1904's offset to 1970 (a
//!   version 0 time below the offset is taken as 1970-based), printed
//!   `%Y-%m-%dT%H:%M:%S.000000Z`; 0, or a time whose microseconds overflow
//!   64 bits, is left out.
//! - The default disposition is the track header's enabled flag; a `kind`
//!   box (`urn:mpeg:dash:role:2011`) adds `comment`, `dub`, `forced`,
//!   `hearing_impaired` and `captions`, or `visual_impaired` and
//!   `descriptions`.
//! - A timecode track's first sample, a frame number, is printed with the
//!   rate its sample description gives; streams whose `tmcd` reference
//!   names it repeat it, and the format has it when no `tmcd` reference
//!   names its position plus one.
//! - Chapters are the samples of the QuickTime chapter track, when a `chap`
//!   reference names one with samples (time base 1/its timescale, each
//!   ending where its sample ends), else the Nero `chpl` list (time base
//!   1/10,000,000, each ending where the next starts, the last at the
//!   format's duration).
//!
//! Left out because they cannot be reproduced faithfully yet: items in
//! UTF-16, a `loci` atom in an item list, the extradata `ffprobe` reports
//! for MPEG-2 video in an `mp4v` entry without a decoder specific info (it
//! seems to come from the first packet), and DNxHD's (`AVdn`), whose source
//! was not established.

use super::model::{Value, s, time};
use super::{Chapter, Entries};
use crate::error::Failure;
use coreav_isobmff::metadata::{
    self, Item, ItemData, ItemValue, ListKey, ListPlace, Metadata, SampleEntry, TimecodeEntry,
    Timestamp, TrackMetadata,
};
use coreav_isobmff::{Error, FourCc, Limits, Movie, Source};
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;

/// What `ffprobe` prints of a file's metadata.
#[derive(Clone, Debug, Default)]
pub struct IsoMeta {
    /// `format` tags in `ffprobe`'s order.
    pub format_tags: Vec<(String, String)>,
    /// Per track (index = `Track::index`): the stream tags in `ffprobe`'s
    /// order.
    pub stream_tags: Vec<Vec<(String, String)>>,
    /// Per track: the 18 disposition flags in `ffprobe`'s order.
    pub dispositions: Vec<Entries>,
    /// Per track: `extradata_size` as `ffprobe` prints it, `None` where it
    /// prints no `extradata_size`.
    pub extradata_sizes: Vec<Option<usize>>,
    /// The chapters `ffprobe` lists (`-show_chapters`).
    pub chapters: Vec<Chapter>,
    /// Per track: what a protected sample entry wraps.
    pub protected: Vec<Option<Protected>>,
    /// Per track: a timecode entry's `timescale` and `frameDuration`, the
    /// rate `ffprobe` prints as its stream's `avg_frame_rate`, unreduced.
    pub timecode_rates: Vec<Option<(u32, u32)>>,
    /// Per track: for the integer and floating-point PCM entries of
    /// ISO/IEC 23003-5 (`ipcm`, `fpcm`), the channel layout `ffprobe`
    /// names from their `chnl` box, `Some(None)` when it names none.
    pub pcm_layouts: Vec<Option<Option<&'static str>>>,
    /// Per track: the field order a QuickTime `fiel` box of the sample
    /// entry states.
    pub field_orders: Vec<Option<&'static str>>,
    /// The movie box holds protection system headers (`pssh`), which
    /// `ffprobe` gives each protected stream as its "Encryption
    /// initialization data".
    pub protection_headers: bool,
    /// The cover pictures of the movie's item list (`covr`), which
    /// `ffprobe` lists as attached-picture streams after the tracks.
    pub covers: Vec<Cover>,
    /// The video tracks of the chapter list ([`chapter_list`]), by
    /// `track_ID`: chapter pictures, whose first picture `ffprobe` lists
    /// again as an attached picture before every other packet.
    pub chapter_pictures: Vec<u32>,
    /// The same as `stream_tags`, `dispositions` and `extradata_sizes`, for
    /// every track box of the movie, dropped streams included (index =
    /// `Track::stream_index`, `DroppedStream::stream_index`).
    pub all_streams: Vec<StreamMeta>,
}

/// One stream's metadata.
#[derive(Clone, Debug, Default)]
pub struct StreamMeta {
    /// The stream tags in `ffprobe`'s order.
    pub tags: Vec<(String, String)>,
    /// See [`IsoMeta::field_orders`].
    pub field_order: Option<&'static str>,
    /// See [`IsoMeta::pcm_layouts`].
    pub pcm_layout: Option<Option<&'static str>>,
    /// The 18 disposition flags in `ffprobe`'s order.
    pub disposition: Entries,
    /// `extradata_size`, `None` where `ffprobe` prints none.
    pub extradata_size: Option<usize>,
    /// For a protected sample entry (`encv`, `enca`, ISO/IEC 14496-12
    /// 8.12): the original format its `sinf/frma` names, and the first
    /// codec configuration box of the entry.
    pub protected: Option<Protected>,
    /// A timecode entry's `timescale` and `frameDuration`.
    pub timecode_rate: Option<(u32, u32)>,
}

/// A cover picture: its bytes as read and where they are in the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cover {
    /// The picture.
    pub data: Vec<u8>,
    /// Where it starts in the file.
    pub offset: u64,
}

/// Whether an item list entry is a cover picture: `covr`, or the
/// QuickTime metadata key `com.apple.quicktime.artwork`.
fn is_cover(key: &metadata::ListKey) -> bool {
    match key {
        metadata::ListKey::Atom(kind) => kind.0 == *b"covr",
        metadata::ListKey::Key { name, .. } => name.as_slice() == b"com.apple.quicktime.artwork",
        _ => false,
    }
}

/// The covers of the movie's item lists, in file order, that `ffprobe`
/// lists: those typed JPEG (13) or PNG (14) (found black box: the same picture typed 0 or 1 is not listed; one typed
/// 27, BMP, is listed as BMP, which is not described here).
fn covers(meta: &metadata::Metadata) -> Vec<Cover> {
    meta.items
        .iter()
        .filter(|item| item.track.is_none())
        .filter_map(|item| match &item.value {
            metadata::ItemValue::ListItem {
                key,
                data: Some(data),
                ..
            } if !data.value.is_empty()
                && is_cover(key)
                && matches!(data.type_indicator, 13 | 14) =>
            {
                Some(Cover {
                    data: data.value.clone(),
                    offset: data.offset,
                })
            }
            _ => None,
        })
        .collect()
}

/// What a protected sample entry wraps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Protected {
    /// `frma`'s `data_format`.
    pub original: [u8; 4],
    /// The type and payload of the entry's codec configuration box.
    pub config: Option<([u8; 4], Vec<u8>)>,
}

/// The original format of a protected sample entry: `frma` inside `sinf`.
fn protected(entry: &metadata::SampleEntry) -> Option<Protected> {
    if !matches!(&entry.kind.0, b"encv" | b"enca" | b"encs" | b"enct") {
        return None;
    }
    let sinf = entry.children.iter().find(|c| c.kind.0 == *b"sinf")?;
    // The boxes of `sinf`: size, type, payload.
    let mut at = 0usize;
    let mut original = None;
    while let (Some(size), Some(kind)) = (
        sinf.payload.get(at..at + 4),
        sinf.payload.get(at + 4..at + 8),
    ) {
        let size = usize::try_from(u32::from_be_bytes(size.try_into().ok()?)).ok()?;
        if size < 8 {
            break;
        }
        if kind == b"frma" {
            original = sinf
                .payload
                .get(at + 8..at + 12)
                .and_then(|b| <[u8; 4]>::try_from(b).ok());
            break;
        }
        at = at.checked_add(size)?;
    }
    let config = entry
        .children
        .iter()
        .find(|c| {
            matches!(
                &c.kind.0,
                b"avcC" | b"hvcC" | b"av1C" | b"vpcC" | b"esds" | b"dOps" | b"dfLa"
            )
        })
        .map(|c| (c.kind.0, c.payload.clone()));
    Some(Protected {
        original: original?,
        config,
    })
}

/// Reads the metadata of `movie` from `source`, the file it was probed
/// from.
pub fn read<S: Source>(movie: &Movie, source: &mut S) -> Result<IsoMeta, Failure> {
    let limits = Limits::default();
    let meta = metadata::read(&mut *source, &limits).map_err(Failure::library)?;
    let quicktime = match meta.file_types.first() {
        Some(f) => f.major_brand == FourCc(*b"qt  "),
        None => meta
            .tracks
            .iter()
            .any(|t| t.handler_component == Some(FourCc(*b"mhlr"))),
    };
    let timecodes = timecodes(&meta, &mut *source)?;
    // The timecode of each `track_ID`: the first track with that ID.
    let mut by_id: HashMap<u32, &String> = HashMap::new();
    for (track, timecode) in meta.tracks.iter().zip(&timecodes) {
        if let (Some(id), Some(timecode)) = (track.track_id, timecode) {
            by_id.entry(id).or_insert(timecode);
        }
    }
    let chapter_list = chapter_list(&meta);
    let chapter_pictures: Vec<u32> = movie
        .tracks
        .iter()
        .filter(|t| {
            t.kind == coreav_isobmff::TrackKind::Video && chapter_list.contains(&t.track_id)
        })
        .map(|t| t.track_id)
        .collect();
    let mut all_streams = Vec::with_capacity(meta.tracks.len());
    for (index, track) in meta.tracks.iter().enumerate() {
        let kept = movie.tracks.iter().find(|t| t.stream_index == index);
        let handler_name = match kept {
            Some(t) => t.handler_name.clone(),
            None => handler_name(&track.handler_name, quicktime),
        };
        let own = timecodes.get(index).and_then(Option::as_ref);
        let timecode = own_or_referenced_timecode(track, own, &by_id);
        all_streams.push(StreamMeta {
            tags: stream_tags(track, handler_name, timecode),
            disposition: disposition(
                track,
                kept.is_some_and(|t| chapter_pictures.contains(&t.track_id)),
            ),
            extradata_size: track.sample_entry.as_ref().and_then(extradata_size),
            protected: track.sample_entry.as_ref().and_then(protected),
            pcm_layout: track.sample_entry.as_ref().and_then(pcm_layout),
            field_order: track.sample_entry.as_ref().and_then(field_order),
            timecode_rate: track
                .sample_entry
                .as_ref()
                .and_then(|e| e.timecode.as_ref())
                .map(|t| (t.timescale, t.frame_duration))
                .filter(|(_, d)| *d > 0),
        });
    }
    let mut out = IsoMeta {
        covers: covers(&meta),
        format_tags: format_tags(&meta, &timecodes),
        chapters: chapters(movie, &chapter_list, &meta, &mut *source)?,
        chapter_pictures,
        protection_headers: meta.protection_headers > 0,
        ..IsoMeta::default()
    };
    for track in &movie.tracks {
        let stream = all_streams
            .get(track.stream_index)
            .cloned()
            .unwrap_or_default();
        out.stream_tags.push(stream.tags);
        out.dispositions.push(if stream.disposition.is_empty() {
            super::iso::disposition(true)
        } else {
            stream.disposition
        });
        out.extradata_sizes.push(stream.extradata_size);
        out.protected.push(stream.protected);
        out.timecode_rates.push(stream.timecode_rate);
        out.pcm_layouts.push(stream.pcm_layout);
        out.field_orders.push(stream.field_order);
    }
    out.all_streams = all_streams;
    Ok(out)
}

/// A dictionary with `ffprobe`'s order: setting a key that is there
/// removes it, moves the last entry into its place and appends the new
/// value.
#[derive(Default)]
struct Dict {
    entries: Vec<(String, String)>,
    /// Each key's position in `entries`.
    positions: HashMap<String, usize>,
}

impl Dict {
    fn set(&mut self, key: &str, value: String) {
        if let Some(i) = self.positions.remove(key) {
            if i < self.entries.len() {
                self.entries.swap_remove(i);
            }
            if let Some((moved, _)) = self.entries.get(i) {
                self.positions.insert(moved.clone(), i);
            }
        }
        self.positions.insert(key.to_owned(), self.entries.len());
        self.entries.push((key.to_owned(), value));
    }
}

/// How an atom's value is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parse {
    /// Text (or a number, by the `data` box's type).
    Text,
    /// The first byte.
    Byte,
    /// The fourth byte.
    FourthByte,
    /// Track or disc number: 16-bit number at 2 and total at 4.
    Index,
    /// The text of the whole payload, never with a size and language.
    Raw,
    /// A genre number: the second byte, 1 for the first of [`GENRES`].
    Genre,
}

/// The genre names `ffprobe` gives `gnre` numbers 1 to 191.
const GENRES: &str = "Blues\nClassic Rock\nCountry\nDance\nDisco\nFunk\nGrunge\nHip-Hop\nJazz\nMetal\nNew Age\nOldies\nOther\nPop\nR&B\nRap\nReggae\nRock\nTechno\nIndustrial\nAlternative\nSka\nDeath Metal\nPranks\nSoundtrack\nEuro-Techno\nAmbient\nTrip-Hop\nVocal\nJazz+Funk\nFusion\nTrance\nClassical\nInstrumental\nAcid\nHouse\nGame\nSound Clip\nGospel\nNoise\nAlternRock\nBass\nSoul\nPunk\nSpace\nMeditative\nInstrumental Pop\nInstrumental Rock\nEthnic\nGothic\nDarkwave\nTechno-Industrial\nElectronic\nPop-Folk\nEurodance\nDream\nSouthern Rock\nComedy\nCult\nGangsta\nTop 40\nChristian Rap\nPop/Funk\nJungle\nNative American\nCabaret\nNew Wave\nPsychedelic\nRave\nShowtunes\nTrailer\nLo-Fi\nTribal\nAcid Punk\nAcid Jazz\nPolka\nRetro\nMusical\nRock & Roll\nHard Rock\nFolk\nFolk-Rock\nNational Folk\nSwing\nFast Fusion\nBebop\nLatin\nRevival\nCeltic\nBluegrass\nAvantgarde\nGothic Rock\nProgressive Rock\nPsychedelic Rock\nSymphonic Rock\nSlow Rock\nBig Band\nChorus\nEasy Listening\nAcoustic\nHumour\nSpeech\nChanson\nOpera\nChamber Music\nSonata\nSymphony\nBooty Bass\nPrimus\nPorn Groove\nSatire\nSlow Jam\nClub\nTango\nSamba\nFolklore\nBallad\nPower Ballad\nRhythmic Soul\nFreestyle\nDuet\nPunk Rock\nDrum Solo\nA Cappella\nEuro-House\nDance Hall\nGoa\nDrum & Bass\nClub-House\nHardcore Techno\nTerror\nIndie\nBritPop\nNegerpunk\nPolsk Punk\nBeat\nChristian Gangsta Rap\nHeavy Metal\nBlack Metal\nCrossover\nContemporary Christian\nChristian Rock\nMerengue\nSalsa\nThrash Metal\nAnime\nJpop\nSynthpop\nAbstract\nArt Rock\nBaroque\nBhangra\nBig Beat\nBreakbeat\nChillout\nDowntempo\nDub\nEBM\nEclectic\nElectro\nElectroclash\nEmo\nExperimental\nGarage\nGlobal\nIDM\nIllbient\nIndustro-Goth\nJam Band\nKrautrock\nLeftfield\nLounge\nMath Rock\nNew Romantic\nNu-Breakz\nPost-Punk\nPost-Rock\nPsytrance\nShoegaze\nSpace Rock\nTrop Rock\nWorld Music\nNeoclassical\nAudiobook\nAudio Theatre\nNeue Deutsche Welle\nPodcast\nIndie Rock\nG-Funk\nDubstep\nGarage Rock";

/// The atoms `ffprobe` names, in item lists and as user data atoms: the
/// key it prints and how it reads the value. A `match` rather than a
/// table, which would cost every command two relocations an entry at
/// start-up.
fn atom(kind: FourCc) -> Option<(&'static str, Parse)> {
    Some(match &kind.0 {
        b"\xa9nam" => ("title", Parse::Text),
        b"\xa9ART" => ("artist", Parse::Text),
        b"aART" => ("album_artist", Parse::Text),
        b"\xa9alb" => ("album", Parse::Text),
        b"\xa9cmt" => ("comment", Parse::Text),
        b"\xa9day" => ("date", Parse::Text),
        b"\xa9gen" => ("genre", Parse::Text),
        b"\xa9too" => ("encoder", Parse::Text),
        b"\xa9wrt" => ("composer", Parse::Text),
        b"cprt" => ("copyright", Parse::Text),
        b"desc" => ("description", Parse::Text),
        b"ldes" => ("synopsis", Parse::Text),
        b"tvsh" => ("show", Parse::Text),
        b"tven" => ("episode_id", Parse::Text),
        b"tvnn" => ("network", Parse::Text),
        b"\xa9lyr" => ("lyrics", Parse::Text),
        b"\xa9grp" => ("grouping", Parse::Text),
        b"sonm" => ("sort_name", Parse::Text),
        b"soar" => ("sort_artist", Parse::Text),
        b"soaa" => ("sort_album_artist", Parse::Text),
        b"soal" => ("sort_album", Parse::Text),
        b"soco" => ("sort_composer", Parse::Text),
        b"sosn" => ("sort_show", Parse::Text),
        b"keyw" => ("keywords", Parse::Text),
        b"catg" => ("category", Parse::Text),
        b"\xa9enc" => ("encoder", Parse::Text),
        b"\xa9st3" => ("subtitle", Parse::Text),
        b"\xa9xyz" => ("location", Parse::Text),
        b"\xa9swr" => ("encoder", Parse::Text),
        b"\xa9mak" => ("make", Parse::Text),
        b"\xa9mod" => ("model", Parse::Text),
        b"\xa9inf" => ("comment", Parse::Text),
        b"\xa9req" => ("playback_requirements", Parse::Text),
        b"\xa9aut" => ("artist", Parse::Text),
        b"\xa9cpy" => ("copyright", Parse::Text),
        b"\xa9prd" => ("producer", Parse::Text),
        b"\xa9dir" => ("director", Parse::Text),
        b"\xa9ed1" => ("edit_date", Parse::Text),
        b"\xa9fmt" => ("original_format", Parse::Text),
        b"\xa9PRD" => ("producer", Parse::Text),
        b"\xa9src" => ("original_source", Parse::Text),
        b"\xa9url" => ("URL", Parse::Text),
        b"\xa9wrn" => ("warning", Parse::Text),
        b"\xa9dis" => ("disclaimer", Parse::Text),
        b"\xa9hst" => ("host_computer", Parse::Text),
        b"\xa9com" => ("composer", Parse::Text),
        b"\xa9ope" => ("original_artist", Parse::Text),
        b"\xa9trk" => ("track", Parse::Text),
        b"\xa9chp" => ("chapter", Parse::Text),
        b"apID" => ("account_id", Parse::Text),
        b"purd" => ("purchase_date", Parse::Text),
        b"manu" => ("make", Parse::Text),
        b"modl" => ("model", Parse::Text),
        b"FIRM" => ("firmware", Parse::Raw),
        b"egid" => ("episode_uid", Parse::Byte),
        b"akID" => ("account_type", Parse::Byte),
        b"rtng" => ("rating", Parse::Byte),
        b"stik" => ("media_type", Parse::Byte),
        b"pcst" => ("podcast", Parse::Byte),
        b"pgap" => ("gapless_playback", Parse::Byte),
        b"hdvd" => ("hd_video", Parse::Byte),
        b"cpil" => ("compilation", Parse::Byte),
        b"tvsn" => ("season_number", Parse::FourthByte),
        b"tves" => ("episode_sort", Parse::FourthByte),
        b"trkn" => ("track", Parse::Index),
        b"disk" => ("disc", Parse::Index),
        b"gnre" => ("genre", Parse::Genre),
        _ => return None,
    })
}

/// The text before the first NUL.
fn c_str(bytes: &[u8]) -> &[u8] {
    bytes.split(|b| *b == 0).next().unwrap_or_default()
}

fn utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(c_str(bytes)).into_owned()
}

/// Mac OS Roman bytes as text (`ffprobe`'s table above 0x7f).
fn mac_roman(bytes: &[u8]) -> String {
    c_str(bytes)
        .iter()
        .map(|&b| match b.checked_sub(0x80) {
            Some(i) => MAC_ROMAN.get(usize::from(i)).copied().unwrap_or('\u{fffd}'),
            None => char::from(b),
        })
        .collect()
}

/// Mac OS Roman's characters 0x80 to 0xff, as `ffprobe` converts them.
const MAC_ROMAN: [char; 128] = [
    '\u{00c4}', '\u{00c5}', '\u{00c7}', '\u{00c9}', '\u{00d1}', '\u{00d6}', '\u{00dc}', '\u{00e1}',
    '\u{00e0}', '\u{00e2}', '\u{00e4}', '\u{00e3}', '\u{00e5}', '\u{00e7}', '\u{00e9}', '\u{00e8}',
    '\u{00ea}', '\u{00eb}', '\u{00ed}', '\u{00ec}', '\u{00ee}', '\u{00ef}', '\u{00f1}', '\u{00f3}',
    '\u{00f2}', '\u{00f4}', '\u{00f6}', '\u{00f5}', '\u{00fa}', '\u{00f9}', '\u{00fb}', '\u{00fc}',
    '\u{2020}', '\u{00b0}', '\u{00a2}', '\u{00a3}', '\u{00a7}', '\u{2022}', '\u{00b6}', '\u{00df}',
    '\u{00ae}', '\u{00a9}', '\u{2122}', '\u{00b4}', '\u{00a8}', '\u{2260}', '\u{00c6}', '\u{00d8}',
    '\u{221e}', '\u{00b1}', '\u{2264}', '\u{2265}', '\u{00a5}', '\u{00b5}', '\u{2202}', '\u{2211}',
    '\u{220f}', '\u{03c0}', '\u{222b}', '\u{00aa}', '\u{00ba}', '\u{03a9}', '\u{00e6}', '\u{00f8}',
    '\u{00bf}', '\u{00a1}', '\u{00ac}', '\u{221a}', '\u{0192}', '\u{2248}', '\u{2206}', '\u{00ab}',
    '\u{00bb}', '\u{2026}', '\u{00a0}', '\u{00c0}', '\u{00c3}', '\u{00d5}', '\u{0152}', '\u{0153}',
    '\u{2013}', '\u{2014}', '\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}', '\u{00f7}', '\u{25ca}',
    '\u{00ff}', '\u{0178}', '\u{2044}', '\u{20ac}', '\u{2039}', '\u{203a}', '\u{fb01}', '\u{fb02}',
    '\u{2021}', '\u{00b7}', '\u{201a}', '\u{201e}', '\u{2030}', '\u{00c2}', '\u{00ca}', '\u{00c1}',
    '\u{00cb}', '\u{00c8}', '\u{00cd}', '\u{00ce}', '\u{00cf}', '\u{00cc}', '\u{00d3}', '\u{00d4}',
    '\u{f8ff}', '\u{00d2}', '\u{00da}', '\u{00db}', '\u{00d9}', '\u{0131}', '\u{02c6}', '\u{02dc}',
    '\u{00af}', '\u{02d8}', '\u{02d9}', '\u{02da}', '\u{00b8}', '\u{02dd}', '\u{02db}', '\u{02c7}',
];

/// The ISO 639-2 codes `ffprobe` gives Macintosh language codes 0 to 94,
/// three letters each; `---` where it gives none.
const MAC_LANGUAGES: &str = "engfrageritadutsvespadanpornorhebjpnarafingreicemltturhr chiurdhinthakorlitpolhunestlavsmifo perruschi---irialbroncesslkslvyidsr macbulukrbeluzbkazazeazearmgeomolkirtgktukmon---puskurkassndtibnepsanmarbenasmgujpa orimalkantamtelsinburkhmlaovieindtglmaymayamhtirormsomswakinrunnyamlgepo";

/// The codes for Macintosh language codes 128 to 138.
const MAC_LANGUAGES_128: &str = "welbaqcatlatquegrnaymtatuigdzojav";

/// The `index`-th three-letter code of a table packed as one string (one
/// string rather than an array of them: an array of strings costs every
/// command a relocation per entry at start-up); `---` marks a code with no
/// name, read as empty.
fn slot(table: &'static str, index: usize) -> Option<&'static str> {
    let code = table.get(index.checked_mul(3)?..index.checked_mul(3)?.checked_add(3)?)?;
    Some(if code == "---" { "" } else { code })
}

/// A language code as `ffprobe` names it: a Macintosh code by its table,
/// a packed ISO 639-2/T code by its letters; `None` for 0x7fff and
/// unmapped Macintosh codes.
fn language(code: u16) -> Option<String> {
    if code == 0x7fff {
        return None;
    }
    if code < 0x400 {
        let name = match code.checked_sub(128) {
            Some(i) => slot(MAC_LANGUAGES_128, usize::from(i)),
            None => slot(MAC_LANGUAGES, usize::from(code)),
        };
        return name.filter(|n| !n.is_empty()).map(|n| (*n).to_owned());
    }
    Some(
        [10u16, 5, 0]
            .iter()
            .map(|shift| char::from((((code >> shift) & 0x1f) as u8).saturating_add(0x60)))
            .collect(),
    )
}

/// Whether text in a language code is Mac OS Roman.
fn mac_text(code: u16) -> bool {
    code < 0x400 || code == 0x7fff
}

/// `creation_time` as `ffprobe` prints it.
fn creation_time(stamp: Option<Timestamp>) -> Option<String> {
    const MAC_TO_UNIX: i64 = 2_082_844_800;
    let stamp = stamp?;
    if stamp.seconds == 0 {
        return None;
    }
    let unix = if stamp.version == 1 {
        i64::try_from(stamp.seconds)
            .ok()?
            .checked_sub(MAC_TO_UNIX)?
    } else {
        let v = i64::try_from(stamp.seconds).ok()?;
        if v >= MAC_TO_UNIX { v - MAC_TO_UNIX } else { v }
    };
    unix.checked_mul(1_000_000)?;
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (year, month, day) = civil(days);
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000000Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    ))
}

/// The proleptic Gregorian date of a day count from 1970-01-01.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The value of an item list entry's `data` box.
fn list_value(parse: Parse, data: &ItemData) -> Option<String> {
    let v = &data.value;
    match parse {
        Parse::Text | Parse::Raw => typed_value(data),
        _ => numbers(parse, v),
    }
}

/// What the keys that hold numbers read from their bytes.
fn numbers(parse: Parse, v: &[u8]) -> Option<String> {
    let byte = |i: usize| v.get(i).copied().unwrap_or(0);
    match parse {
        Parse::Byte => Some(byte(0).to_string()),
        Parse::FourthByte => Some(if v.len() >= 4 { byte(3) } else { 0 }.to_string()),
        Parse::Index => {
            let half = |i: usize| match v.get(i..i + 2) {
                Some(&[a, b]) => u16::from_be_bytes([a, b]),
                _ => 0,
            };
            let (number, total) = (half(2), half(4));
            Some(if total > 0 {
                format!("{number}/{total}")
            } else {
                number.to_string()
            })
        }
        Parse::Genre => {
            let n = usize::from(*v.get(1)?);
            GENRES.split('\n').nth(n.checked_sub(1)?).map(str::to_owned)
        }
        Parse::Text | Parse::Raw => None,
    }
}

/// A `data` box's value by its type indicator.
fn typed_value(data: &ItemData) -> Option<String> {
    let v = &data.value;
    match data.type_indicator {
        0 | 1 | 3 | 4 => Some(utf8(v)),
        21 => Some(match v.len() {
            1..=4 => {
                let mut n: i64 = if v.first().is_some_and(|b| b & 0x80 != 0) {
                    -1
                } else {
                    0
                };
                for b in v {
                    n = (n << 8) | i64::from(*b);
                }
                n.to_string()
            }
            _ => "0".to_owned(),
        }),
        22 => Some(match v.len() {
            1..=4 => v
                .iter()
                .fold(0u64, |n, b| (n << 8) | u64::from(*b))
                .to_string(),
            _ => "0".to_owned(),
        }),
        23 => {
            let bytes: [u8; 4] = v.get(..4)?.try_into().ok()?;
            Some(format!("{:.6}", f64::from(f32::from_be_bytes(bytes))))
        }
        _ => None,
    }
}

/// The text of a QuickTime user data atom and its language: with a 16-bit
/// size and a 16-bit language before the text when the size fits, else the
/// whole payload without a language. A header with nothing after it gives
/// empty text with no language.
fn atom_text(payload: &[u8]) -> (Option<u16>, &[u8]) {
    let size = match payload.get(..2) {
        Some(&[a, b]) => usize::from(u16::from_be_bytes([a, b])),
        _ => 0,
    };
    match (payload.get(4..4 + size), payload.get(2..4)) {
        (Some(text), Some(&[a, b])) => {
            let lang = (payload.len() > 4).then(|| u16::from_be_bytes([a, b]));
            (lang, text)
        }
        _ => (None, payload),
    }
}

/// Sets the tags of one metadata item.
fn item_tags(dict: &mut Dict, item: &Item) {
    match &item.value {
        ItemValue::UserData { kind, payload } => {
            if kind.0 == *b"loci" {
                if let Some((lang, value)) = location(payload) {
                    if let Some(suffix) = language(lang).filter(|l| l != "und") {
                        dict.set(&format!("location-{suffix}"), value.clone());
                    }
                    dict.set("location", value);
                }
                return;
            }
            let Some((name, parse)) = atom(*kind) else {
                return;
            };
            if parse == Parse::Raw {
                dict.set(name, utf8(payload));
                return;
            }
            let (lang, text) = atom_text(payload);
            if parse != Parse::Text {
                if let Some(value) = numbers(parse, text) {
                    dict.set(name, value);
                }
                return;
            }
            let value = match lang {
                Some(code) if !mac_text(code) => utf8(text),
                Some(_) => mac_roman(text),
                None => utf8(text),
            };
            dict.set(name, value.clone());
            if let Some(suffix) = lang.and_then(language).filter(|l| l != "und") {
                dict.set(&format!("{name}-{suffix}"), value);
            }
        }
        ItemValue::ListItem { list, key, data } => {
            if *list == (ListPlace::Meta { handler: None }) {
                return;
            }
            let Some(data) = data else {
                return;
            };
            match key {
                ListKey::Atom(kind) => {
                    if let Some((name, parse)) = atom(*kind) {
                        if let Some(value) = list_value(parse, data) {
                            dict.set(name, value);
                        }
                    }
                }
                ListKey::Key { name, .. } => {
                    let name = utf8(name);
                    if item.track.is_some() || name == "com.apple.quicktime.artwork" {
                        return;
                    }
                    if let Some(value) = typed_value(data) {
                        dict.set(&name, value);
                    }
                }
                ListKey::Free { mean, name } => {
                    let (Some(_), Some(name)) = (mean, name) else {
                        return;
                    };
                    if !data.value.is_empty() {
                        dict.set(&utf8(name), utf8(&data.value));
                    }
                }
            }
        }
    }
}

/// A 3GPP location box (`loci`, 3GPP TS 26.244 8.10): version and flags,
/// language, a NUL-terminated name, a role byte, then longitude, latitude
/// and altitude in 16.16 fixed point. Printed as ISO 6709 latitude and
/// longitude, the altitude when not 0, and the name after a slash.
fn location(payload: &[u8]) -> Option<(u16, String)> {
    let lang = match payload.get(4..6)? {
        &[a, b] => u16::from_be_bytes([a, b]),
        _ => return None,
    };
    let rest = payload.get(6..)?;
    let name = c_str(rest);
    let at = name.len() + 2;
    let fixed = |i: usize| -> Option<f64> {
        let b: [u8; 4] = rest.get(at + i * 4..at + i * 4 + 4)?.try_into().ok()?;
        Some(f64::from(i32::from_be_bytes(b)) / 65536.0)
    };
    let (longitude, latitude, altitude) = (fixed(0)?, fixed(1)?, fixed(2)?);
    let mut value = format!("{latitude:+08.4}{longitude:+09.4}");
    if altitude != 0.0 {
        value.push_str(&format!("{altitude:+.6}"));
    }
    value.push('/');
    value.push_str(&String::from_utf8_lossy(name));
    Some((lang, value))
}

fn format_tags(meta: &Metadata, timecodes: &[Option<String>]) -> Vec<(String, String)> {
    let mut dict = Dict::default();
    for file_type in &meta.file_types {
        dict.set("major_brand", utf8(&file_type.major_brand.0));
        dict.set("minor_version", file_type.minor_version.to_string());
        dict.set("compatible_brands", utf8(&file_type.compatible_brands));
    }
    if let Some(time) = creation_time(meta.movie_created) {
        dict.set("creation_time", time);
    }
    for item in &meta.items {
        item_tags(&mut dict, item);
    }
    if timecodes.iter().any(Option::is_some) {
        let named: HashSet<u32> = meta
            .tracks
            .iter()
            .flat_map(|t| t.references.iter())
            .filter(|r| r.kind == FourCc(*b"tmcd"))
            .flat_map(|r| r.track_ids.iter().copied())
            .collect();
        for (index, timecode) in timecodes.iter().enumerate() {
            let position = u32::try_from(index).ok().and_then(|i| i.checked_add(1));
            if let (Some(timecode), Some(position)) = (timecode, position) {
                if !named.contains(&position) {
                    dict.set("timecode", timecode.clone());
                }
            }
        }
    }
    dict.entries
}

/// A handler name: QuickTime stores a counted string (a length byte equal
/// to the rest of the field), ISO/IEC 14496-12 a null-terminated one.
fn handler_name(raw: &[u8], quicktime: bool) -> Option<String> {
    let text = match raw.split_first() {
        Some((&len, tail)) if quicktime && usize::from(len) == tail.len() && len > 0 => tail,
        _ => c_str(raw),
    };
    (!text.is_empty()).then(|| String::from_utf8_lossy(text).into_owned())
}

/// A four-character code as `ffprobe` prints a vendor: printable bytes as
/// they are, others as `[n]`.
fn vendor(code: [u8; 4]) -> String {
    let mut out = String::new();
    for b in code {
        if (0x20..0x7f).contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("[{b}]"));
        }
    }
    out
}

/// A visual entry's compressor name: a counted string of at most 31
/// bytes, in Mac OS Roman.
fn compressor(field: &[u8; 32]) -> Option<String> {
    let [len, rest @ ..] = field;
    let len = usize::from(*len).min(31);
    let text = mac_roman(rest.get(..len).unwrap_or_default());
    (!text.is_empty()).then_some(text)
}

fn stream_tags(
    track: &TrackMetadata,
    handler: Option<String>,
    timecode: Option<String>,
) -> Vec<(String, String)> {
    let mut tags = Vec::new();
    let mut push = |key: &str, value: String| tags.push((key.to_owned(), value));
    if let Some(time) = creation_time(track.media_created) {
        push("creation_time", time);
    }
    if let Some(lang) = track.language.and_then(language) {
        push("language", lang);
    }
    if let Some(name) = handler {
        push("handler_name", name);
    }
    if let Some(entry) = &track.sample_entry {
        if let Some(code) = entry.vendor {
            push("vendor_id", vendor(code));
        }
        if let Some(name) = entry.compressor_name.as_ref().and_then(compressor) {
            push("encoder", name);
        }
        if let Some(name) = entry
            .timecode
            .and(entry.children.iter().find(|c| c.kind == FourCc(*b"name")))
        {
            let (lang, text) = atom_text(&name.payload);
            let value = match lang {
                Some(code) if !mac_text(code) => utf8(text),
                _ => mac_roman(text),
            };
            push("reel_name", value);
        }
    }
    if let Some(timecode) = timecode {
        push("timecode", timecode);
    }
    tags
}

/// The scheme of the `kind` values `ffprobe` reads.
const DASH_ROLE: &[u8] = b"urn:mpeg:dash:role:2011";

/// A video track a `chap` reference names holds chapter pictures, which
/// `ffprobe` marks as an attached picture and timed thumbnails.
fn disposition(track: &TrackMetadata, chapter_pictures: bool) -> Entries {
    let mut entries = super::iso::disposition(track.flags.is_some_and(|f| f & 1 != 0));
    if chapter_pictures {
        for (key, v) in &mut entries {
            if *key == "attached_pic" || *key == "timed_thumbnails" {
                *v = Value::Int(1);
            }
        }
    }
    for (scheme, value) in &track.kinds {
        if scheme.as_slice() != DASH_ROLE {
            continue;
        }
        let flags: &[&str] = match value.as_slice() {
            b"commentary" => &["comment"],
            b"dub" => &["dub"],
            b"forced-subtitle" => &["forced"],
            b"caption" => &["hearing_impaired", "captions"],
            b"description" => &["visual_impaired", "descriptions"],
            _ => &[],
        };
        for (key, v) in &mut entries {
            if flags.contains(key) {
                *v = Value::Int(1);
            }
        }
    }
    entries
}

/// The field order of a QuickTime `fiel` box (QuickTime File Format,
/// "Field handling"): one field is progressive whatever the detail, two
/// fields are named by their detail (1 top first, 6 bottom first, 9 and
/// 14 the same with the other field stored first), and anything else is
/// not named, as `ffprobe` reports entries crafted with each value.
fn field_order(entry: &SampleEntry) -> Option<&'static str> {
    let fiel = entry.children.iter().find(|c| c.kind.0 == *b"fiel")?;
    match fiel.payload.get(..2)? {
        [1, _] => Some("progressive"),
        [2, 1] => Some("tt"),
        [2, 6] => Some("bb"),
        [2, 9] => Some("tb"),
        [2, 14] => Some("bt"),
        _ => None,
    }
}

/// The channel layout of an `ipcm` or `fpcm` entry (ISO/IEC 23003-5),
/// from its `chnl` box (ISO/IEC 14496-12 12.2.4): none without the box
/// (`ffmpeg` writes none for mono), else the layout of its
/// `definedLayout` (a `ChannelConfiguration` of ISO/IEC 23091-3) or of
/// its explicit speaker positions. The table is what `ffprobe` names on
/// files `ffmpeg` wrote with each of its named layouts; anything else is
/// not named.
fn pcm_layout(entry: &SampleEntry) -> Option<Option<&'static str>> {
    if !matches!(&entry.kind.0, b"ipcm" | b"fpcm") {
        return None;
    }
    let Some(chnl) = entry.children.iter().find(|c| c.kind.0 == *b"chnl") else {
        return Some(None);
    };
    // Version 0 and a channel structure (`stream_structure` 1).
    let Some([0, _, _, _, 1, defined, positions @ ..]) = chnl.payload.get(..) else {
        return Some(None);
    };
    Some(match (defined, positions) {
        (2, _) => Some("stereo"),
        (3, _) => Some("3.0"),
        (4, _) => Some("4.0"),
        (5, _) => Some("5.0(side)"),
        (6, _) => Some("5.1(side)"),
        (7, _) => Some("7.1(wide-side)"),
        (9, _) => Some("3.0(back)"),
        (10, _) => Some("quad(side)"),
        (11, _) => Some("6.1"),
        (12, _) => Some("7.1"),
        (13, _) => Some("22.2"),
        (14, _) => Some("5.1.2"),
        (0, [0, 1, 3]) => Some("2.1"),
        (0, [0, 1, 8, 9]) => Some("quad"),
        (0, [0, 1, 2, 3]) => Some("3.1"),
        (0, [0, 1, 2, 8, 9]) => Some("5.0"),
        (0, [0, 1, 2, 3, 10]) => Some("4.1"),
        (0, [0, 1, 2, 3, 8, 9]) => Some("5.1"),
        (0, [0, 1, 2, 10, 13, 14]) => Some("6.0"),
        (0, [0, 1, 2, 3, 8, 9, 10]) => Some("6.1(back)"),
        (0, [0, 1, 2, 8, 9, 13, 14]) => Some("7.0"),
        (0, [0, 1, 2, 3, 8, 9, 6, 7]) => Some("7.1(wide)"),
        (0, [0, 1, 2, 8, 9, 10]) => Some("hexagonal"),
        (0, [0, 1, 2, 8, 9, 10, 13, 14]) => Some("octagonal"),
        (0, [0, 1, 2, 3, 8, 9, 13, 14, 17, 18, 20, 21]) => Some("7.1.4"),
        _ => None,
    })
}

/// The size of the extradata `ffprobe` gives the stream.
fn extradata_size(entry: &SampleEntry) -> Option<usize> {
    let child = |kind: &[u8; 4]| {
        entry
            .children
            .iter()
            .chain(entry.wave.iter())
            .find(|c| c.kind.0 == *kind)
    };
    match &entry.kind.0 {
        b"tmcd" | b"text" | b"tx3g" => {
            return usize::try_from(entry.declared_len.checked_sub(8)?).ok();
        }
        _ => {}
    }
    for kind in [b"avcC", b"hvcC", b"av1C"] {
        if let Some(c) = child(kind) {
            return Some(c.payload.len());
        }
    }
    if let Some(c) = child(b"esds") {
        return metadata::decoder_specific_info(&c.payload)
            .map(<[u8]>::len)
            .filter(|n| *n > 0);
    }
    if let Some(c) = child(b"dfLa") {
        // The first metadata block, STREAMINFO.
        let &[kind, a, b, c] = c.payload.get(4..8)? else {
            return None;
        };
        return (kind & 0x7f == 0)
            .then(|| usize::from(a) << 16 | usize::from(b) << 8 | usize::from(c));
    }
    for kind in [b"dOps", b"alac", b"SMI "] {
        if let Some(c) = child(kind) {
            return c.payload.len().checked_add(8);
        }
    }
    None
}

/// The timecode of every timecode track, by position.
fn timecodes<S: Source>(meta: &Metadata, source: &mut S) -> Result<Vec<Option<String>>, Failure> {
    let mut out = Vec::with_capacity(meta.tracks.len());
    for track in &meta.tracks {
        let entry = track.sample_entry.as_ref().and_then(|e| e.timecode);
        let value = match (entry, track.first_chunk_offset) {
            (Some(entry), Some(offset)) => {
                let mut sample = [0u8; 4];
                match source.read_exact_at(offset, &mut sample) {
                    Ok(()) => timecode(entry, i32::from_be_bytes(sample)),
                    Err(error @ Error::Io(_)) => return Err(Failure::library(error)),
                    Err(_) => None,
                }
            }
            _ => None,
        };
        out.push(value);
    }
    Ok(out)
}

/// The timecode a stream prints: its own, or that of the first track its
/// `tmcd` reference names.
fn own_or_referenced_timecode(
    track: &TrackMetadata,
    own: Option<&String>,
    by_id: &HashMap<u32, &String>,
) -> Option<String> {
    if let Some(own) = own {
        return Some(own.clone());
    }
    track
        .references
        .iter()
        .filter(|r| r.kind == FourCc(*b"tmcd"))
        .flat_map(|r| r.track_ids.iter())
        .find_map(|id| by_id.get(id).map(|t| (*t).clone()))
}

/// A timecode track's first frame number as `ffprobe` prints it.
fn timecode(entry: TimecodeEntry, value: i32) -> Option<String> {
    let (ts, fd) = (i64::from(entry.timescale), i64::from(entry.frame_duration));
    let frames = i64::from(entry.frames);
    if fd == 0 || frames == 0 {
        return None;
    }
    let fps = (ts + fd / 2) / fd;
    if fps <= 0 {
        return None;
    }
    let drop = entry.flags & 1 != 0;
    if drop && fps % 30 != 0 {
        return None;
    }
    let mut frame = i64::from(value);
    let truncated = ts / fd;
    let fractional = ts % fd != 0;
    if frames != fps && !(fractional && frames == truncated) {
        // The frame number counts `frames` per second: rescale it to `fps`,
        // rounding halves away from zero.
        let scaled = frame.checked_mul(fps)?;
        let half = frames / 2;
        frame = if scaled >= 0 {
            scaled.checked_add(half)? / frames
        } else {
            scaled.checked_sub(half)? / frames
        };
    }
    let negative = frame < 0;
    let mut frame = frame.checked_abs()?;
    if drop {
        // SMPTE ST 12-1 drop-frame numbering: 2 frame numbers per 30 frames
        // per second skipped each minute but every tenth.
        let dropped = fps / 30 * 2;
        let per_minute = fps.checked_mul(60)?.checked_sub(dropped)?;
        let per_ten = fps.checked_mul(600)?.checked_sub(dropped.checked_mul(9)?)?;
        let (tens, rest) = (frame / per_ten, frame % per_ten);
        frame = frame.checked_add(dropped.checked_mul(9)?.checked_mul(tens)?)?;
        if rest > dropped {
            frame = frame.checked_add(dropped.checked_mul((rest - dropped) / per_minute)?)?;
        }
    }
    let ff = frame % fps;
    let ss = frame / fps % 60;
    let mm = frame / fps.checked_mul(60)? % 60;
    let mut hh = frame / fps.checked_mul(3600)?;
    if entry.flags & 2 != 0 {
        hh %= 24;
    }
    let sign = if negative && entry.flags & 4 != 0 {
        "-"
    } else {
        ""
    };
    let sep = if drop { ';' } else { ':' };
    let width = if fps > 100 { 3 } else { 2 };
    Some(format!("{sign}{hh:02}:{mm:02}:{ss:02}{sep}{ff:0width$}"))
}

/// The chapter list as `ffprobe` reads it: the `chap` reference of the
/// last track box that has one, each track box's list replacing the one
/// before (found black box: with the audio naming a picture track and the
/// picture track a text track, the pictures are not chapter pictures; with
/// the second reference gone, they are).
fn chapter_list(meta: &Metadata) -> Vec<u32> {
    let chap = FourCc(*b"chap");
    meta.tracks
        .iter()
        .rev()
        .find_map(|t| {
            t.references
                .iter()
                .find(|r| r.kind == chap && !r.track_ids.is_empty())
        })
        .map(|r| r.track_ids.clone())
        .unwrap_or_default()
}

/// The chapters: those of the text tracks of the chapter list
/// ([`chapter_list`]) when they have any, else those of the Nero chapter
/// list.
fn chapters<S: Source>(
    movie: &Movie,
    list: &[u32],
    meta: &Metadata,
    source: &mut S,
) -> Result<Vec<Chapter>, Failure> {
    let limits = Limits::default();
    let text_limits = coreav_text::TextLimits::default();
    // Every referenced text track in turn, each chapter replacing the one
    // of the same number before it (found black box: of `chap` 2, 1 with
    // 15 and 9 chapters, the first 9 are track 1's and the rest track 2's).
    // Only the last time a track is named matters, as it replaces what
    // the earlier ones gave.
    let mut ids: Vec<u32> = Vec::new();
    for id in list.iter().rev() {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    ids.reverse();
    // The title bytes limit holds for the merged list too: a track whose
    // chapters would take it over, with what they replace and what they
    // keep, is left out, as a track over the limit on its own is.
    let mut listed: Vec<Chapter> = Vec::new();
    let mut title_sizes: Vec<u64> = Vec::new();
    let mut title_bytes = 0u64;
    let read =
        coreav_text::read_chapter_tracks(&mut *source, &ids, &limits, &text_limits, |result| {
            match result {
                Ok(list) => {
                    let replaced: u64 = title_sizes.iter().take(list.chapters.len()).sum();
                    let added: u64 = list.chapters.iter().map(|c| c.title.len() as u64).sum();
                    let merged = title_bytes.saturating_sub(replaced).saturating_add(added);
                    if merged > text_limits.max_title_bytes {
                        return ControlFlow::Continue(());
                    }
                    title_bytes = merged;
                    let den = i64::from(list.timescale.max(1));
                    for (n, c) in list.chapters.iter().enumerate() {
                        let start = crate::time::rounded(c.start).unwrap_or(0);
                        let end = crate::time::rounded(c.end).unwrap_or(start);
                        let one = chapter(n, den, start, end, &c.title);
                        let size = c.title.len() as u64;
                        match (listed.get_mut(n), title_sizes.get_mut(n)) {
                            (Some(slot), Some(slot_size)) => (*slot, *slot_size) = (one, size),
                            _ => {
                                listed.push(one);
                                title_sizes.push(size);
                            }
                        }
                    }
                    ControlFlow::Continue(())
                }
                Err(coreav_text::TextError::Source(error @ Error::Io(_))) => {
                    ControlFlow::Break(error)
                }
                Err(_) => ControlFlow::Continue(()),
            }
        });
    if let Ok(ControlFlow::Break(error))
    | Err(coreav_text::TextError::Source(error @ Error::Io(_))) = read
    {
        return Err(Failure::library(error));
    }
    if !listed.is_empty() {
        return Ok(listed);
    }
    let Some(list) = &meta.nero_chapters else {
        return Ok(Vec::new());
    };
    const DEN: i64 = 10_000_000;
    let duration = movie
        .ffprobe_format_duration_micros()
        .and_then(|us| i64::try_from(us).ok())
        .and_then(|us| us.checked_mul(10));
    let starts: Vec<i64> = list
        .iter()
        .map(|c| i64::try_from(c.start).unwrap_or(i64::MAX))
        .collect();
    Ok(list
        .iter()
        .enumerate()
        .map(|(id, c)| {
            let start = starts.get(id).copied().unwrap_or(0);
            let end = starts.get(id + 1).copied().or(duration).unwrap_or(start);
            chapter(id, DEN, start, end, &utf8(&c.title))
        })
        .collect())
}

fn chapter(id: usize, den: i64, start: i64, end: i64, title: &str) -> Chapter {
    Chapter {
        entries: vec![
            ("id", Value::Int(i64::try_from(id).unwrap_or(i64::MAX))),
            ("time_base", s(format!("1/{den}"))),
            ("start", Value::Int(start)),
            ("start_time", time(start, 1, den)),
            ("end", Value::Int(end)),
            ("end_time", time(end, 1, den)),
        ],
        tags: vec![("title".to_owned(), title.to_owned())],
    }
}

/// Expected values are `ffprobe` 7.0.2's output on crafted files.
#[cfg(test)]
mod tests {
    use super::*;

    fn tags(items: &[ItemValue]) -> Vec<(String, String)> {
        let mut dict = Dict::default();
        for value in items {
            let item = Item {
                track: None,
                value: value.clone(),
            };
            item_tags(&mut dict, &item);
        }
        dict.entries
    }

    fn atom_item(kind: &[u8; 4], payload: &[u8]) -> ItemValue {
        ItemValue::UserData {
            kind: FourCc(*kind),
            payload: payload.to_vec(),
        }
    }

    fn list_item(kind: &[u8; 4], type_indicator: u32, value: &[u8]) -> ItemValue {
        ItemValue::ListItem {
            list: ListPlace::Meta {
                handler: Some(FourCc(*b"mdir")),
            },
            key: ListKey::Atom(FourCc(*kind)),
            data: Some(ItemData {
                type_indicator,
                locale: 0,
                value: value.to_vec(),
                offset: 0,
            }),
        }
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn overwrites_move_the_last_entry() {
        let items = [
            atom_item(b"\xa9nam", b"\0\x01\0\0A"),
            atom_item(b"\xa9ART", b"\0\x01\0\0B"),
            atom_item(b"\xa9nam", b"\0\x01\0\0C"),
        ];
        assert_eq!(
            tags(&items),
            pairs(&[
                ("artist-eng", "B"),
                ("title", "C"),
                ("artist", "B"),
                ("title-eng", "C")
            ])
        );
    }

    #[test]
    fn user_data_text_atoms() {
        let one = |payload: &[u8]| tags(&[atom_item(b"\xa9nam", payload)]);
        assert_eq!(
            one(b"\0\x04\0\0Caf\x8e"),
            pairs(&[("title", "Café"), ("title-eng", "Café")])
        );
        assert_eq!(one(b"\0\x02\x7f\xffHi"), pairs(&[("title", "Hi")]));
        assert_eq!(one(b"\0\x02\x55\xc4Hi"), pairs(&[("title", "Hi")]));
        assert_eq!(
            one(b"\0\x02\x04\0Hi"),
            pairs(&[("title", "Hi"), ("title-a``", "Hi")])
        );
        assert_eq!(
            one(b"\0\x02\0\x17Hi"),
            pairs(&[("title", "Hi"), ("title-kor", "Hi")])
        );
        assert_eq!(one(b"HD5.02"), pairs(&[("title", "HD5.02")]));
        assert_eq!(one(b"\0\0\0\0"), pairs(&[("title", "")]));
        assert_eq!(
            one(b"\0\0\0\0Hello"),
            pairs(&[("title", ""), ("title-eng", "")])
        );
        assert_eq!(one(b"\0\x32\0\0Hi"), pairs(&[("title", "")]));
        assert_eq!(
            tags(&[atom_item(b"FIRM", b"HD5\0junk")]),
            pairs(&[("firmware", "HD5")])
        );
    }

    #[test]
    fn list_values_by_type() {
        let one = |kind: &[u8; 4], t: u32, v: &[u8]| {
            tags(&[list_item(kind, t, v)])
                .first()
                .map(|(_, v)| v.clone())
        };
        let nam = b"\xa9nam";
        assert_eq!(one(nam, 1, b"A\0B").as_deref(), Some("A"));
        assert_eq!(one(nam, 2, b"\0H").as_deref(), None);
        assert_eq!(one(nam, 21, b"\xff\xfe").as_deref(), Some("-2"));
        assert_eq!(one(nam, 21, b"\xff\x02\x03").as_deref(), Some("-65021"));
        assert_eq!(
            one(nam, 21, &[0, 0, 0, 1, 0, 0, 0, 2]).as_deref(),
            Some("0")
        );
        assert_eq!(
            one(nam, 22, b"\xff\xff\xff\xfe").as_deref(),
            Some("4294967294")
        );
        assert_eq!(
            one(nam, 23, &29.97f32.to_be_bytes()).as_deref(),
            Some("29.969999")
        );
        assert_eq!(one(nam, 24, &[0; 8]).as_deref(), None);
        assert_eq!(one(b"stik", 21, b"\0\0\0\x09").as_deref(), Some("0"));
        assert_eq!(
            one(b"tvsn", 21, b"\xff\xff\xff\xfb").as_deref(),
            Some("251")
        );
        assert_eq!(
            one(b"trkn", 0, b"\0\0\0\x03\0\x0a\0\0").as_deref(),
            Some("3/10")
        );
        assert_eq!(one(b"trkn", 0, b"\0\0\0\x03").as_deref(), Some("3"));
        assert_eq!(one(b"gnre", 0, b"\0\x12").as_deref(), Some("Rock"));
        assert_eq!(one(b"gnre", 0, b"\0\xc8").as_deref(), None);
    }

    #[test]
    fn creation_times() {
        let t = |version, seconds| creation_time(Some(Timestamp { version, seconds }));
        assert_eq!(t(0, 0), None);
        assert_eq!(t(0, 1).as_deref(), Some("1970-01-01T00:00:01.000000Z"));
        assert_eq!(
            t(0, 3_850_167_845).as_deref(),
            Some("2026-01-02T03:04:05.000000Z")
        );
        assert_eq!(
            t(0, 2_082_844_799).as_deref(),
            Some("2036-01-01T23:59:59.000000Z")
        );
        assert_eq!(t(1, 1).as_deref(), Some("1904-01-01T00:00:01.000000Z"));
        assert_eq!(
            t(1, 1 << 40).as_deref(),
            Some("36746-02-19T00:36:16.000000Z")
        );
        assert_eq!(
            t(1, 9_225_454_881_654).as_deref(),
            Some("294247-01-10T04:00:54.000000Z")
        );
        assert_eq!(t(1, 9_225_454_881_655), None);
        assert_eq!(t(1, u64::MAX), None);
    }

    #[test]
    fn languages_vendors_and_compressor_names() {
        assert_eq!(language(0).as_deref(), Some("eng"));
        assert_eq!(language(18).as_deref(), Some("hr "));
        assert_eq!(language(130).as_deref(), Some("cat"));
        assert_eq!(language(150), None);
        assert_eq!(language(0x7fff), None);
        assert_eq!(language(0x55c4).as_deref(), Some("und"));
        assert_eq!(vendor(*b"\x01ab\xff"), "[1]ab[255]");
        assert_eq!(vendor([0; 4]), "[0][0][0][0]");
        let mut name = [0u8; 32];
        name[..5].copy_from_slice(b"\x04Caf\x8e");
        assert_eq!(compressor(&name).as_deref(), Some("Café"));
        name[0] = 0x40;
        assert_eq!(compressor(&name).as_deref(), Some("Café"));
        name[0] = 0;
        assert_eq!(compressor(&name), None);
    }

    #[test]
    fn timecodes() {
        let tc = |flags, timescale, frame_duration, frames, value| {
            timecode(
                TimecodeEntry {
                    flags,
                    timescale,
                    frame_duration,
                    frames,
                },
                value,
            )
        };
        assert_eq!(tc(0, 25, 1, 25, 91_532).as_deref(), Some("01:01:01:07"));
        assert_eq!(tc(0, 25, 1, 30, 91_532).as_deref(), Some("00:50:51:02"));
        assert_eq!(tc(0, 25, 1, 0, 100), None);
        assert_eq!(tc(0, 25, 0, 25, 100), None);
        assert_eq!(
            tc(2, 60_000, 1001, 59, 3_000_000).as_deref(),
            Some("13:53:20:00")
        );
        assert_eq!(tc(0, 25, 1, 59, 3_000_000).as_deref(), Some("14:07:27:11"));
        assert_eq!(
            tc(0, 2500, 101, 24, 3_000_001).as_deref(),
            Some("33:20:00:01")
        );
        assert_eq!(
            tc(0, 25025, 1000, 24, 3_000_001).as_deref(),
            Some("34:43:20:01")
        );
        assert_eq!(
            tc(0, 120_000, 1001, 119, 3_000_001).as_deref(),
            Some("06:56:40:001")
        );
        assert_eq!(
            tc(1, 30_000, 1001, 30, 17_982).as_deref(),
            Some("00:10:00;00")
        );
        assert_eq!(
            tc(1, 30_000, 1001, 30, 10_799).as_deref(),
            Some("00:06:00;11")
        );
        assert_eq!(
            tc(1, 60_000, 1001, 60, 36_000).as_deref(),
            Some("00:10:00;36")
        );
        assert_eq!(tc(1, 25, 1, 25, 3000), None);
        assert_eq!(tc(2, 25, 1, 25, 2_250_010).as_deref(), Some("01:00:00:10"));
        assert_eq!(tc(0, 25, 1, 25, 2_250_010).as_deref(), Some("25:00:00:10"));
        assert_eq!(tc(4, 25, 1, 25, -10).as_deref(), Some("-00:00:00:10"));
        assert_eq!(tc(0, 25, 1, 25, -10).as_deref(), Some("00:00:00:10"));
        // Hostile rates overflow nothing.
        for flags in [0, 1, 7] {
            for frames in [1, 255] {
                for value in [i32::MIN, -1, i32::MAX] {
                    let _ = tc(flags, u32::MAX - 15, 1, frames, value);
                    let _ = tc(flags, u32::MAX, u32::MAX, frames, value);
                }
            }
        }
    }

    #[test]
    fn locations() {
        let mut loci = vec![0u8; 4];
        loci.extend_from_slice(&0x15c7u16.to_be_bytes());
        loci.extend_from_slice(b"Home\0\0");
        for v in [0x1_0000i32, 0x2_0000, 0x3_0000] {
            loci.extend_from_slice(&v.to_be_bytes());
        }
        loci.extend_from_slice(b"Earth\0\0");
        assert_eq!(
            tags(&[atom_item(b"loci", &loci)]),
            pairs(&[
                ("location-eng", "+02.0000+001.0000+3.000000/Home"),
                ("location", "+02.0000+001.0000+3.000000/Home")
            ])
        );
    }
}
