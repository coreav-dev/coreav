//! What sanitization removes (`docs/design/sanitize.md`): a bounded walk of
//! the `udta` and `meta` boxes left out, listing each box and item with its
//! path, size and, from its type or key name, what it describes, and picking
//! out the two items written again: track kinds and gapless playback
//! information. Written from ISO/IEC 14496-12 (`udta`, `meta`, `hdlr`,
//! `kind`), the QuickTime File Format (user data types, metadata `keys` and
//! `ilst`, `----` items), Apple's iTunes metadata conventions (`iTunSMPB`)
//! and 3GPP TS 26.244 (`loci`). Box headers, plus the payloads of `keys`,
//! `kind` and `----` boxes (bounded by `max_box_bytes` and the sizes below);
//! every box counts against `max_total_boxes`.

use crate::mux::RawBox;
use crate::scan::{Path, Skipped};
use crate::{ClearedField, Keep, MetadataKind, moov::overflow};
use coreav_isobmff::{
    BoxHeader, Error, FourCc, LimitKind, Limits, MAX_HEADER_LEN, Parsed, Source, parse_box_header,
};

/// Longest key name reported.
const MAX_NAME: usize = 128;

/// Most keys named per `keys` box.
const MAX_KEYS: usize = 256;

/// Largest `kind` payload read: version and flags, then a scheme and a
/// value from [`KINDS`].
const MAX_KIND: u64 = 64;

/// Most `kind` boxes kept per track.
const MAX_KINDS: usize = 16;

/// The track kinds kept (ISO/IEC 14496-12 8.10.4): the role scheme of
/// ISO/IEC 23009-1 (DASH) and the HTML track kinds, each value from its
/// vocabulary. A `kind` box is written again from this table, so no other
/// text reaches the output.
const KINDS: [(&str, &[&str]); 2] = [
    (
        "urn:mpeg:dash:role:2011",
        &[
            "caption",
            "subtitle",
            "main",
            "alternate",
            "supplementary",
            "commentary",
            "dub",
            "description",
            "sign",
            "metadata",
            "enhanced-audio-intelligibility",
            "emergency",
            "forced-subtitle",
            "easyreader",
            "karaoke",
        ],
    ),
    (
        "about:html-kind",
        &[
            "alternative",
            "captions",
            "descriptions",
            "main",
            "main-desc",
            "sign",
            "subtitles",
            "translation",
            "commentary",
        ],
    ),
];

/// Longest metadata key or item name the summary repeats.
const MAX_REPORTED_NAME: usize = 64;

/// Largest `----` item read for its gapless information.
const MAX_FREEFORM: u64 = 1024;

/// One removed box or item.
pub(crate) struct Removed {
    pub(crate) skipped: Skipped,
    pub(crate) kind: Option<MetadataKind>,
    pub(crate) name: Option<String>,
}

/// An item that sanitization writes again, rebuilt from what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Rewritten {
    /// A track's kind (ISO/IEC 14496-12 8.10.4): its role, such as a forced
    /// subtitle track: a scheme and a value from [`KINDS`].
    Kind {
        scheme: &'static str,
        value: &'static str,
    },
    /// Gapless playback information (the `iTunSMPB` item): samples of
    /// encoder delay at the start, of padding at the end, and the number of
    /// samples between them.
    Gapless {
        delay: u32,
        padding: u32,
        length: u64,
    },
}

/// Walks `tags` (`udta` and `meta` boxes under `parent`), calling `found`
/// with the box itself and each item inside, in file order. Items that
/// `keep` writes again are returned instead; their parents are still
/// listed (the output's are new boxes).
pub(crate) fn describe<S: Source>(
    source: &mut S,
    limits: &Limits,
    keep: Keep,
    boxes: &mut u64,
    parent: Path,
    tags: &[BoxHeader],
    found: &mut dyn FnMut(Removed),
) -> Result<Vec<Rewritten>, Error> {
    let mut rewritten = Vec::new();
    let mut walk = Walk {
        source,
        limits,
        keep,
        boxes,
        rewritten: &mut rewritten,
    };
    for tag in tags {
        found(Removed {
            skipped: Skipped {
                parent,
                header: *tag,
            },
            kind: None,
            name: None,
        });
        let path = parent.with(tag.kind);
        match &tag.kind.0 {
            b"udta" => walk.udta(tag, path, found)?,
            b"meta" => walk.meta(tag, path, found)?,
            _ => {}
        }
    }
    Ok(rewritten)
}

struct Walk<'a, S> {
    source: &'a mut S,
    limits: &'a Limits,
    keep: Keep,
    boxes: &'a mut u64,
    rewritten: &'a mut Vec<Rewritten>,
}

impl<S: Source> Walk<'_, S> {
    fn header(&mut self, at: u64, end: u64) -> Result<Option<BoxHeader>, Error> {
        let room = end.saturating_sub(at);
        if room < 8 {
            return Ok(None);
        }
        let mut buf = [0u8; MAX_HEADER_LEN];
        let n = usize::try_from(room.min(MAX_HEADER_LEN as u64)).unwrap_or(MAX_HEADER_LEN);
        let slice = buf.get_mut(..n).ok_or_else(overflow)?;
        self.source.read_exact_at(at, slice)?;
        // A list that does not parse ends here: the remaining bytes are part
        // of the removed parent, which is reported.
        let header = match parse_box_header(slice, at, room) {
            Ok(Parsed::Done(header)) => header,
            Ok(Parsed::NeedMore(_)) | Err(Error::InvalidBox { .. } | Error::Truncated { .. }) => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        self.count(at)?;
        Ok(Some(header))
    }

    /// Counts one box (or key) at `at` against `max_total_boxes`.
    fn count(&mut self, at: u64) -> Result<(), Error> {
        *self.boxes = self.boxes.saturating_add(1);
        if *self.boxes > u64::from(self.limits.max_total_boxes) {
            return Err(Error::LimitExceeded {
                limit: LimitKind::TotalBoxes,
                offset: at,
            });
        }
        Ok(())
    }

    /// The children of `parent` from `start`.
    fn children(
        &mut self,
        parent: &BoxHeader,
        start: u64,
        visit: &mut dyn FnMut(&mut Self, BoxHeader) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let end = parent.end();
        let mut at = start;
        while let Some(header) = self.header(at, end)? {
            visit(self, header)?;
            at = header.end();
        }
        Ok(())
    }

    /// User data: each child an item; a `meta` child walked as one.
    fn udta(
        &mut self,
        udta: &BoxHeader,
        path: Path,
        found: &mut dyn FnMut(Removed),
    ) -> Result<(), Error> {
        self.children(udta, udta.payload_offset(), &mut |walk, child| {
            // Kinds describe tracks; elsewhere they are removed.
            let in_track = path.kinds().contains(&FourCc(*b"trak"));
            if child.kind.0 == *b"kind" && walk.keep.kinds && in_track {
                let kinds = walk
                    .rewritten
                    .iter()
                    .filter(|r| matches!(r, Rewritten::Kind { .. }))
                    .count();
                if kinds < MAX_KINDS {
                    if let Some(kind) = walk.kind(&child)? {
                        walk.rewritten.push(kind);
                        return Ok(());
                    }
                }
            }
            found(Removed {
                skipped: Skipped {
                    parent: path,
                    header: child,
                },
                kind: Some(kind_of_type(child.kind)),
                name: None,
            });
            if child.kind.0 == *b"meta" {
                walk.meta(&child, path.with(child.kind), found)?;
            }
            Ok(())
        })
    }

    /// A metadata box: ISO/IEC 14496-12's full box, or QuickTime's without
    /// version and flags (its first child, `hdlr`, then starts at once).
    /// The `keys` names are reported; `ilst` items by type, or for keyed
    /// items by the key they refer to.
    fn meta(
        &mut self,
        meta: &BoxHeader,
        path: Path,
        found: &mut dyn FnMut(Removed),
    ) -> Result<(), Error> {
        let mut probe = [0u8; 12];
        let n = usize::try_from(meta.payload_len().min(12)).unwrap_or(0);
        let head = probe.get_mut(..n).ok_or_else(overflow)?;
        self.source.read_exact_at(meta.payload_offset(), head)?;
        let quicktime = head.get(4..8) == Some(b"hdlr");
        let start = meta
            .payload_offset()
            .saturating_add(if quicktime { 0 } else { 4 });
        let mut keys: Vec<String> = Vec::new();
        self.children(meta, start, &mut |walk, child| {
            match &child.kind.0 {
                b"keys" => {
                    keys = walk.keys(&child)?;
                    for key in &keys {
                        found(Removed {
                            skipped: Skipped {
                                parent: path,
                                header: child,
                            },
                            kind: Some(kind_of_key(key)),
                            name: reported(key),
                        });
                    }
                }
                b"ilst" => {
                    let ilst = path.with(child.kind);
                    walk.children(&child, child.payload_offset(), &mut |walk, item| {
                        // A keyed item's type is the 1-based index of its key;
                        // a free-form item (`----`) is named by its `name`.
                        let mut name = usize::try_from(u32::from_be_bytes(item.kind.0))
                            .ok()
                            .and_then(|i| i.checked_sub(1))
                            .and_then(|i| keys.get(i))
                            .cloned();
                        let mut kind = name
                            .as_deref()
                            .map_or_else(|| kind_of_type(item.kind), kind_of_key);
                        if item.kind.0 == *b"----" {
                            let freeform = walk.freeform(&item)?;
                            let gapless = walk
                                .rewritten
                                .iter()
                                .any(|r| matches!(r, Rewritten::Gapless { .. }));
                            if walk.keep.gapless && !gapless {
                                if let Some(Freeform {
                                    gapless: Some(g), ..
                                }) = &freeform
                                {
                                    walk.rewritten.push(g.clone());
                                    return Ok(());
                                }
                            }
                            if let Some(Freeform { name: n, .. }) = freeform {
                                kind = kind_of_key(&n);
                                name = Some(n);
                            }
                        }
                        found(Removed {
                            skipped: Skipped {
                                parent: ilst,
                                header: item,
                            },
                            kind: Some(kind),
                            name: name.as_deref().and_then(reported),
                        });
                        Ok(())
                    })?;
                }
                _ => found(Removed {
                    skipped: Skipped {
                        parent: path,
                        header: child,
                    },
                    kind: None,
                    name: None,
                }),
            }
            Ok(())
        })
    }

    /// The key names of a QuickTime `keys` box: version and flags, an entry
    /// count, then per key its size (header included), namespace and name.
    fn keys(&mut self, header: &BoxHeader) -> Result<Vec<String>, Error> {
        let len = header.payload_len();
        if len > self.limits.max_box_bytes {
            return Ok(Vec::new());
        }
        let mut payload = Vec::new();
        let size = usize::try_from(len).map_err(|_| overflow())?;
        if payload.try_reserve_exact(size).is_err() {
            return Ok(Vec::new());
        }
        payload.resize(size, 0);
        self.source
            .read_exact_at(header.payload_offset(), &mut payload)?;
        let mut names = Vec::new();
        let mut at = 8usize;
        while names.len() < MAX_KEYS {
            let Some(entry) = payload
                .get(at..at.saturating_add(4))
                .and_then(|b| <[u8; 4]>::try_from(b).ok())
            else {
                break;
            };
            let entry = usize::try_from(u32::from_be_bytes(entry)).unwrap_or(0);
            let Some(name) = entry.checked_sub(8).and_then(|n| {
                payload.get(at.saturating_add(8)..at.saturating_add(8).saturating_add(n))
            }) else {
                break;
            };
            // Each key counts as a box.
            self.count(header.offset)?;
            names.push(printable(name));
            at = at.saturating_add(entry);
        }
        Ok(names)
    }

    /// The kind a `kind` box names, when it is version 0 with flags 0 and
    /// holds exactly a scheme and a value of [`KINDS`], each NUL-terminated;
    /// anything else is removed.
    fn kind(&mut self, header: &BoxHeader) -> Result<Option<Rewritten>, Error> {
        let Some(payload) = self.payload(header, MAX_KIND)? else {
            return Ok(None);
        };
        if payload.get(..4) != Some(&[0; 4][..]) {
            return Ok(None);
        }
        let mut strings = payload.get(4..).unwrap_or_default().split(|b| *b == 0);
        let (Some(scheme), Some(value), Some(b""), None) = (
            strings.next(),
            strings.next(),
            strings.next(),
            strings.next(),
        ) else {
            return Ok(None);
        };
        Ok(KINDS.iter().find_map(|(s, values)| {
            let value = values.iter().find(|v| v.as_bytes() == value)?;
            (s.as_bytes() == scheme).then_some(Rewritten::Kind { scheme: s, value })
        }))
    }

    /// The name of a free-form item (`----`: `mean`, `name`, `data`
    /// children), and its gapless information when it is `iTunSMPB` in the
    /// `com.apple.iTunes` namespace with a UTF-8 value of hexadecimal fields
    /// (unused, delay, padding, length, ...).
    fn freeform(&mut self, header: &BoxHeader) -> Result<Option<Freeform>, Error> {
        let Some(payload) = self.payload(header, MAX_FREEFORM)? else {
            return Ok(None);
        };
        let (mut mean, mut name, mut data) = (None, None, None);
        let mut at = 0usize;
        while let Some(size) = payload
            .get(at..at.saturating_add(4))
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(u32::from_be_bytes)
            .and_then(|n| usize::try_from(n).ok())
        {
            let (Some(kind), Some(body)) = (
                payload.get(at.saturating_add(4)..at.saturating_add(8)),
                size.checked_sub(8).and_then(|n| {
                    payload.get(at.saturating_add(8)..at.saturating_add(8).saturating_add(n))
                }),
            ) else {
                break;
            };
            match kind {
                b"mean" => mean = body.get(4..),
                b"name" => name = body.get(4..),
                b"data" => data = Some(body),
                _ => {}
            }
            at = at.saturating_add(size);
        }
        let Some(name) = name else {
            return Ok(None);
        };
        let gapless = (mean == Some(b"com.apple.iTunes".as_slice()) && name == b"iTunSMPB")
            .then(|| data.and_then(gapless))
            .flatten();
        Ok(Some(Freeform {
            name: printable(name),
            gapless,
        }))
    }

    /// The payload of `header` when it is at most `max` bytes.
    fn payload(&mut self, header: &BoxHeader, max: u64) -> Result<Option<Vec<u8>>, Error> {
        let len = header.payload_len();
        if len > max.min(self.limits.max_box_bytes) {
            return Ok(None);
        }
        let mut payload = vec![0; usize::try_from(len).map_err(|_| overflow())?];
        self.source
            .read_exact_at(header.payload_offset(), &mut payload)?;
        Ok(Some(payload))
    }
}

/// A free-form item's name, and its gapless information.
struct Freeform {
    name: String,
    gapless: Option<Rewritten>,
}

/// Gapless information from an `iTunSMPB` value (`data` payload: type 1,
/// UTF-8, then a locale): space-separated hexadecimal fields, the second
/// to fourth being delay, padding and length.
fn gapless(data: &[u8]) -> Option<Rewritten> {
    if data.get(..4) != Some(&[0, 0, 0, 1]) {
        return None;
    }
    let text = std::str::from_utf8(data.get(8..)?).ok()?;
    let mut fields = text.split_ascii_whitespace();
    fields.next()?;
    let delay = u32::from_str_radix(fields.next()?, 16).ok()?;
    let padding = u32::from_str_radix(fields.next()?, 16).ok()?;
    let length = u64::from_str_radix(fields.next()?, 16).ok()?;
    Some(Rewritten::Gapless {
        delay,
        padding,
        length,
    })
}

/// The `udta` box that carries `items`, or `None` when there are none:
/// the kinds as stored, then the gapless information as a `meta` box (ISO
/// full box with an `mdir` handler) whose `ilst` holds one `----` item,
/// written in the canonical form (the other fields zero).
pub(crate) fn rewrite(items: &[Rewritten]) -> Result<Option<RawBox>, Error> {
    if items.is_empty() {
        return Ok(None);
    }
    let mut udta = Vec::new();
    for item in items {
        match item {
            Rewritten::Kind { scheme, value } => {
                let payload = [&[0; 4], scheme.as_bytes(), b"\0", value.as_bytes(), b"\0"].concat();
                udta.extend(boxed(b"kind", &payload)?);
            }
            Rewritten::Gapless {
                delay,
                padding,
                length,
            } => {
                let value = format!(
                    " 00000000 {delay:08X} {padding:08X} {length:016X}{}",
                    " 00000000".repeat(8)
                );
                let mut data = vec![0, 0, 0, 1, 0, 0, 0, 0];
                data.extend(value.as_bytes());
                let mut item = boxed(b"mean", b"\0\0\0\0com.apple.iTunes")?;
                item.extend(boxed(b"name", b"\0\0\0\0iTunSMPB")?);
                item.extend(boxed(b"data", &data)?);
                let ilst = boxed(b"ilst", &boxed(b"----", &item)?)?;
                let mut meta = vec![0; 4];
                meta.extend(boxed(
                    b"hdlr",
                    b"\0\0\0\0\0\0\0\0mdirappl\0\0\0\0\0\0\0\0\0",
                )?);
                meta.extend(ilst);
                udta.extend(boxed(b"meta", &meta)?);
            }
        }
    }
    Ok(Some(RawBox {
        kind: FourCc(*b"udta"),
        payload: udta,
    }))
}

/// A box of `kind` around `payload`.
fn boxed(kind: &[u8; 4], payload: &[u8]) -> Result<Vec<u8>, Error> {
    let size = u32::try_from(payload.len().saturating_add(8)).map_err(|_| overflow())?;
    let mut out = Vec::with_capacity(payload.len().saturating_add(8));
    out.extend(size.to_be_bytes());
    out.extend(kind);
    out.extend(payload);
    Ok(out)
}

/// `name` as the summary repeats it: only a name that reads as an
/// identifier (letters, digits, `.`, `_`, `-`, at most
/// [`MAX_REPORTED_NAME`] bytes), such as `com.apple.quicktime.make`; a name
/// that could carry a value itself is not repeated.
fn reported(name: &str) -> Option<String> {
    let identifier = !name.is_empty()
        && name.len() <= MAX_REPORTED_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    identifier.then(|| name.to_string())
}

/// Printable ASCII, other bytes as `?`, at most [`MAX_NAME`] bytes.
fn printable(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(MAX_NAME)
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                char::from(*b)
            } else {
                '?'
            }
        })
        .collect()
}

/// What a user data or item type describes (QuickTime File Format user data
/// types, 3GPP TS 26.244).
fn kind_of_type(kind: FourCc) -> MetadataKind {
    match &kind.0 {
        b"\xa9xyz" | b"loci" => MetadataKind::Location,
        b"\xa9mak" | b"\xa9mod" | b"\xa9swr" | b"\xa9too" | b"\xa9enc" => MetadataKind::Device,
        b"\xa9day" => MetadataKind::Date,
        _ => MetadataKind::Other,
    }
}

/// What a QuickTime metadata key describes, from its name
/// (`com.apple.quicktime.location.ISO6709`, `...make`, `...model`,
/// `...software`, `...creationdate`; keys without a namespace prefix such
/// as `creation_time` and `encoder` too).
fn kind_of_key(key: &str) -> MetadataKind {
    let key = key.to_ascii_lowercase();
    let last = key.rsplit('.').next().unwrap_or(&key);
    if key.contains("location") {
        MetadataKind::Location
    } else if matches!(last, "make" | "model" | "software" | "encoder") {
        MetadataKind::Device
    } else if key.contains("date") || last.starts_with("creation") {
        MetadataKind::Date
    } else {
        MetadataKind::Other
    }
}

/// What sanitization writes for an allowlisted sample entry child.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EntryBox {
    /// This payload, and what was left out of the source's, if anything.
    Write(Vec<u8>, Option<ClearedField>),
    /// Nothing: the box is removed and reported.
    Remove,
    /// The stream cannot be sanitized: its codec configuration does not
    /// parse.
    Unsupported(&'static str),
}

/// Payload sizes of the allowlisted boxes with a fixed layout: `pasp`,
/// `btrt`, `clap` (ISO/IEC 14496-12 12.1.4 to 12.1.6), `clli`, `mdcv`
/// (ISO/IEC 23001-8 boxes of the HEVC and AV1 bindings), the VP binding's
/// `SmDm` and `CoLL` (full boxes), the Dolby Vision configurations (24
/// bytes), QuickTime's `fiel` and the stereo video box `st3d` (a full box
/// and one byte).
const FIXED: [(&[u8; 4], usize); 12] = [
    (b"pasp", 8),
    (b"btrt", 12),
    (b"clap", 32),
    (b"clli", 4),
    (b"mdcv", 24),
    (b"SmDm", 28),
    (b"CoLL", 8),
    (b"dvcC", 24),
    (b"dvvC", 24),
    (b"dvwC", 24),
    (b"fiel", 2),
    (b"st3d", 5),
];

/// An allowlisted sample entry child as sanitization writes it: only the
/// fields its format defines, so no text rides along. Parameter sets of
/// H.264 and HEVC configurations (other NAL units, such as SEI with the
/// encoder's name and settings, left out); coded colour parameters of
/// `colr` (an ICC profile can name the device); a spherical video box
/// without its metadata source; fixed-layout boxes cut to their size. Other
/// boxes (`av1C`, `vpcC`, `esds`, `dOps`, `chnl`) as stored.
pub(crate) fn entry_box(kind: FourCc, payload: Vec<u8>) -> EntryBox {
    let extras = |out: Vec<u8>, changed: bool| {
        EntryBox::Write(out, changed.then_some(ClearedField::SampleEntryExtras))
    };
    match &kind.0 {
        b"hvcC" => hevc_parameter_sets(&payload).map_or(
            EntryBox::Unsupported("the HEVC configuration does not parse"),
            |(out, changed)| extras(out, changed),
        ),
        b"avcC" => avc_parameter_sets(&payload).map_or(
            EntryBox::Unsupported("the H.264 configuration does not parse"),
            |(out, changed)| extras(out, changed),
        ),
        b"sv3d" => match spherical(&payload) {
            Some((out, Some(field))) => EntryBox::Write(out, Some(field)),
            Some((out, None)) => EntryBox::Write(out, None),
            None => EntryBox::Remove,
        },
        b"colr" => {
            let size = match payload.get(..4) {
                Some(b"nclx") => 11,
                Some(b"nclc") => 10,
                _ => return EntryBox::Remove,
            };
            cut(payload, size).map_or(EntryBox::Remove, |(out, changed)| extras(out, changed))
        }
        other => match FIXED.iter().find(|(k, _)| *k == other) {
            Some((_, size)) => {
                cut(payload, *size).map_or(EntryBox::Remove, |(out, changed)| extras(out, changed))
            }
            None => EntryBox::Write(payload, None),
        },
    }
}

/// `payload` cut to `size` bytes and whether anything was cut, or `None`
/// when it is shorter.
fn cut(mut payload: Vec<u8>, size: usize) -> Option<(Vec<u8>, bool)> {
    if payload.len() < size {
        return None;
    }
    let changed = payload.len() > size;
    payload.truncate(size);
    Some((payload, changed))
}

/// A length-prefixed NAL unit at `at`: its bytes and the offset after it.
fn nal_unit(config: &[u8], at: usize) -> Option<(&[u8], usize)> {
    let len = u16::from_be_bytes(config.get(at..at.checked_add(2)?)?.try_into().ok()?);
    let start = at.checked_add(2)?;
    let end = start.checked_add(usize::from(len))?;
    Some((config.get(start..end)?, end))
}

/// A length-prefixed NAL unit as written.
fn push_nal_unit(out: &mut Vec<u8>, nalu: &[u8]) -> Option<()> {
    out.extend(u16::try_from(nalu.len()).ok()?.to_be_bytes());
    out.extend_from_slice(nalu);
    Some(())
}

/// An HEVC configuration (ISO/IEC 14496-15 8.3.3.1) with only its VPS, SPS
/// and PPS (NAL unit types 32 to 34, each NAL unit's own header matching
/// its array's type): SEI arrays carry encoder names and settings. Returns
/// the record and whether anything was left out, or `None` when it does not
/// parse.
pub(crate) fn hevc_parameter_sets(config: &[u8]) -> Option<(Vec<u8>, bool)> {
    let fixed = config.get(..22)?;
    let count = *config.get(22)?;
    let mut kept = Vec::new();
    let mut arrays = 0u8;
    let mut changed = false;
    let mut at = 23usize;
    for _ in 0..count {
        let head = *config.get(at)?;
        let nal_type = head & 0x3f;
        let nalus = u16::from_be_bytes(
            config
                .get(at.checked_add(1)?..at.checked_add(3)?)?
                .try_into()
                .ok()?,
        );
        at = at.checked_add(3)?;
        let mut array = Vec::new();
        let mut written = 0u16;
        for _ in 0..nalus {
            let (nalu, next) = nal_unit(config, at)?;
            at = next;
            let own = nalu.first().map(|b| (b >> 1) & 0x3f);
            if (32..=34).contains(&nal_type) && own == Some(nal_type) {
                push_nal_unit(&mut array, nalu)?;
                written = written.checked_add(1)?;
            } else {
                changed = true;
            }
        }
        if written > 0 {
            kept.push(head);
            kept.extend(written.to_be_bytes());
            kept.extend(array);
            arrays = arrays.checked_add(1)?;
        }
    }
    changed |= at != config.len();
    let mut out = Vec::with_capacity(kept.len().saturating_add(23));
    out.extend_from_slice(fixed);
    out.push(arrays);
    out.extend(kept);
    Some((out, changed))
}

/// An H.264 configuration (ISO/IEC 14496-15 5.3.3.1) with only its sequence
/// and picture parameter sets (NAL unit types 7 and 8) and, for the
/// profiles that have it, the extension fields with sequence parameter set
/// extensions (type 13); nothing after them. `None` when it does not parse.
pub(crate) fn avc_parameter_sets(config: &[u8]) -> Option<(Vec<u8>, bool)> {
    let mut out = config.get(..5)?.to_vec();
    let mut changed = false;
    let mut at = 5usize;
    for (mask, nal_type) in [(0x1f, 7u8), (0xff, 8)] {
        let head = *config.get(at)?;
        let count = head & mask;
        at = at.checked_add(1)?;
        let mut kept = Vec::new();
        let mut written = 0u8;
        for _ in 0..count {
            let (nalu, next) = nal_unit(config, at)?;
            at = next;
            if nalu.first().map(|b| b & 0x1f) == Some(nal_type) {
                push_nal_unit(&mut kept, nalu)?;
                written = written.checked_add(1)?;
            } else {
                changed = true;
            }
        }
        out.push((head & !mask) | written);
        out.extend(kept);
    }
    // High profiles may carry chroma format, bit depths and extensions.
    let profile = *config.get(1)?;
    if matches!(profile, 100 | 110 | 122 | 144) && config.len() >= at.checked_add(4)? {
        let fields = config.get(at..at.checked_add(4)?)?;
        at = at.checked_add(4)?;
        let count = *fields.get(3)?;
        let mut kept = Vec::new();
        let mut written = 0u8;
        for _ in 0..count {
            let (nalu, next) = nal_unit(config, at)?;
            at = next;
            if nalu.first().map(|b| b & 0x1f) == Some(13) {
                push_nal_unit(&mut kept, nalu)?;
                written = written.checked_add(1)?;
            } else {
                changed = true;
            }
        }
        out.extend_from_slice(fields.get(..3)?);
        out.push(written);
        out.extend(kept);
    }
    changed |= at != config.len();
    Some((out, changed))
}

/// A spherical video box (`sv3d`) as written: its header (`svhd`) with an
/// empty metadata source (the tool or camera that wrote it), and its
/// projection (`proj`) with only the projection header (`prhd`) and an
/// equirectangular, cubemap or mesh projection (`equi`, `cbmp`, `mshp`);
/// other children left out. Returns the payload and what was cleared, or
/// `None` when the children do not parse.
pub(crate) fn spherical(payload: &[u8]) -> Option<(Vec<u8>, Option<ClearedField>)> {
    let mut out = Vec::with_capacity(payload.len());
    let mut cleared = None;
    for (kind, body) in children(payload)? {
        match kind {
            b"svhd" => {
                if body.get(4..).is_some_and(|s| s.iter().any(|b| *b != 0)) {
                    cleared = Some(ClearedField::SphericalSource);
                }
                out.extend(boxed(b"svhd", &[0, 0, 0, 0, 0]).ok()?);
            }
            b"proj" => {
                let mut proj = Vec::new();
                for (inner, data) in children(body)? {
                    if matches!(inner, b"prhd" | b"equi" | b"cbmp" | b"mshp") {
                        proj.extend(boxed(inner, data).ok()?);
                    } else {
                        cleared = cleared.or(Some(ClearedField::SampleEntryExtras));
                    }
                }
                out.extend(boxed(b"proj", &proj).ok()?);
            }
            _ => cleared = cleared.or(Some(ClearedField::SampleEntryExtras)),
        }
    }
    Some((out, cleared))
}

/// The child boxes (type, payload) of a payload, or `None` when they do
/// not fill it exactly.
fn children(payload: &[u8]) -> Option<Vec<(&[u8; 4], &[u8])>> {
    let mut found = Vec::new();
    let mut at = 0usize;
    while at < payload.len() {
        let size = usize::try_from(u32::from_be_bytes(
            payload.get(at..at.checked_add(4)?)?.try_into().ok()?,
        ))
        .ok()?;
        let end = at.checked_add(size)?;
        let kind: &[u8; 4] = payload
            .get(at.checked_add(4)?..at.checked_add(8)?)?
            .try_into()
            .ok()?;
        found.push((kind, payload.get(at.checked_add(8)?..end)?));
        at = end;
    }
    Some(found)
}

/// Visual sample entry children that sanitization keeps: the codec
/// configurations of the cited bindings and boxes that describe how to
/// present the coded picture (ISO/IEC 14496-12 12.1.4 to 12.1.6: `pasp`,
/// `clap`, `colr`, `btrt`; mastering display and content light level
/// (`mdcv`, `clli`, and the VP binding's `SmDm`, `CoLL`); Dolby Vision
/// configuration (`dvcC`, `dvvC`, `dvwC`); QuickTime field order (`fiel`);
/// stereo and spherical video (`st3d`, `sv3d`).
pub(crate) const VISUAL_KEPT: [&[u8; 4]; 18] = [
    b"avcC", b"hvcC", b"av1C", b"vpcC", b"pasp", b"clap", b"colr", b"btrt", b"mdcv", b"clli",
    b"SmDm", b"CoLL", b"dvcC", b"dvvC", b"dvwC", b"fiel", b"st3d", b"sv3d",
];

/// Audio sample entry children that sanitization keeps besides the codec
/// configuration: `btrt` and the ISO channel layout `chnl`.
pub(crate) const AUDIO_KEPT: [&[u8; 4]; 2] = [b"btrt", b"chnl"];

#[cfg(test)]
mod tests {
    use super::*;

    /// An `hvcC` with the 22 fixed bytes and `arrays` of (type, NAL units).
    fn hvcc(arrays: &[(u8, &[&[u8]])], trailing: &[u8]) -> Vec<u8> {
        let mut out = vec![1; 22];
        out.push(u8::try_from(arrays.len()).unwrap_or(0));
        for (nal_type, nalus) in arrays {
            out.push(0x80 | nal_type);
            out.extend(u16::try_from(nalus.len()).unwrap_or(0).to_be_bytes());
            for nalu in *nalus {
                out.extend(u16::try_from(nalu.len()).unwrap_or(0).to_be_bytes());
                out.extend(*nalu);
            }
        }
        out.extend(trailing);
        out
    }

    #[test]
    fn hevc_configurations_keep_their_parameter_sets() {
        // NAL unit headers carry the type in bits 1 to 6.
        let sets: [(u8, &[&[u8]]); 3] = [
            (32, &[b"\x40vps"]),
            (33, &[b"\x42sps"]),
            (34, &[b"\x44pps"]),
        ];
        let clean = hvcc(&sets, b"");
        assert_eq!(hevc_parameter_sets(&clean), Some((clean.clone(), false)));
        let sei: &[&[u8]] = &[b"\x4ex265 (build 199) options"];
        let noisy = hvcc(&[sets[0], sets[1], (39, sei), sets[2], (40, sei)], b"tail");
        assert_eq!(hevc_parameter_sets(&noisy), Some((clean.clone(), true)));
        // An SEI NAL unit in a parameter set array is left out too.
        let hidden: &[&[u8]] = &[b"\x42sps", b"\x4eSECRET"];
        let mislabelled = hvcc(&[sets[0], (33, hidden), sets[2]], b"");
        assert_eq!(hevc_parameter_sets(&mislabelled), Some((clean, true)));
        // A NAL unit past the end does not parse.
        let mut short = hvcc(&sets, b"");
        short.truncate(short.len() - 1);
        assert_eq!(hevc_parameter_sets(&short), None);
        assert_eq!(hevc_parameter_sets(&[1; 10]), None);
    }

    #[test]
    fn avc_configurations_keep_their_parameter_sets() {
        // Main profile: version, profile, compatibility, level, length size;
        // then the sequence and picture parameter sets.
        let clean = [
            &[1, 77, 0, 30, 0xff, 0xe1, 0, 4][..],
            b"\x67sps",
            &[1, 0, 4],
            b"\x68pps",
        ]
        .concat();
        assert_eq!(avc_parameter_sets(&clean), Some((clean.clone(), false)));
        let noisy = [
            &[1, 77, 0, 30, 0xff, 0xe2, 0, 4][..],
            b"\x67sps",
            &[0, 7],
            b"\x06SECRET",
            &[1, 0, 4],
            b"\x68pps",
            b"tail",
        ]
        .concat();
        assert_eq!(avc_parameter_sets(&noisy), Some((clean, true)));
        assert_eq!(avc_parameter_sets(&[1, 77, 0, 30, 0xff, 0xe1, 0, 9]), None);
    }

    #[test]
    fn allowlisted_boxes_keep_only_their_fields() {
        let write = |kind: &[u8; 4], payload: &[u8]| entry_box(FourCc(*kind), payload.to_vec());
        assert_eq!(
            write(b"pasp", &[0, 0, 0, 1, 0, 0, 0, 1]),
            EntryBox::Write(vec![0, 0, 0, 1, 0, 0, 0, 1], None)
        );
        assert_eq!(
            write(b"pasp", b"\0\0\0\x01\0\0\0\x01SECRET"),
            EntryBox::Write(
                vec![0, 0, 0, 1, 0, 0, 0, 1],
                Some(ClearedField::SampleEntryExtras)
            )
        );
        assert_eq!(write(b"pasp", &[0, 0, 0, 1]), EntryBox::Remove);
        assert_eq!(
            write(b"colr", b"nclx\0\x01\0\x01\0\x01\x80SECRET"),
            EntryBox::Write(
                b"nclx\0\x01\0\x01\0\x01\x80".to_vec(),
                Some(ClearedField::SampleEntryExtras)
            )
        );
        assert_eq!(write(b"colr", b"profICC: Maker"), EntryBox::Remove);
        assert_eq!(
            write(b"av1C", &[0x81, 0, 0, 0]),
            EntryBox::Write(vec![0x81, 0, 0, 0], None)
        );
        assert_eq!(
            write(b"hvcC", &[1; 10]),
            EntryBox::Unsupported("the HEVC configuration does not parse")
        );
    }

    #[test]
    fn spherical_headers_lose_their_source() {
        let proj =
            boxed(b"proj", &boxed(b"prhd", &[0; 16]).unwrap_or_default()).unwrap_or_default();
        let empty = [boxed(b"svhd", &[0; 5]).unwrap_or_default(), proj.clone()].concat();
        let named = [
            boxed(b"svhd", b"\0\0\0\0Camera\0").unwrap_or_default(),
            proj.clone(),
        ]
        .concat();
        assert_eq!(
            spherical(&named),
            Some((empty.clone(), Some(ClearedField::SphericalSource)))
        );
        assert_eq!(spherical(&empty), Some((empty.clone(), None)));
        // Unknown children, of `sv3d` or of `proj`, are left out.
        let extra = [
            boxed(b"svhd", &[0; 5]).unwrap_or_default(),
            boxed(
                b"proj",
                &[
                    boxed(b"prhd", &[0; 16]).unwrap_or_default(),
                    boxed(b"free", b"SECRET").unwrap_or_default(),
                ]
                .concat(),
            )
            .unwrap_or_default(),
            boxed(b"xtra", b"SECRET").unwrap_or_default(),
        ]
        .concat();
        assert_eq!(
            spherical(&extra),
            Some((empty, Some(ClearedField::SampleEntryExtras)))
        );
        assert_eq!(spherical(&[0, 0, 0, 4, 1]), None);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of_type(FourCc(*b"\xa9xyz")), MetadataKind::Location);
        assert_eq!(kind_of_type(FourCc(*b"\xa9mod")), MetadataKind::Device);
        assert_eq!(kind_of_type(FourCc(*b"\xa9day")), MetadataKind::Date);
        assert_eq!(kind_of_type(FourCc(*b"\xa9nam")), MetadataKind::Other);
        assert_eq!(
            kind_of_key("com.apple.quicktime.location.ISO6709"),
            MetadataKind::Location
        );
        assert_eq!(
            kind_of_key("com.apple.quicktime.model"),
            MetadataKind::Device
        );
        assert_eq!(
            kind_of_key("com.apple.quicktime.creationdate"),
            MetadataKind::Date
        );
        assert_eq!(
            kind_of_key("com.apple.quicktime.title"),
            MetadataKind::Other
        );
        assert_eq!(
            reported("com.apple.quicktime.make").as_deref(),
            Some("com.apple.quicktime.make")
        );
        assert_eq!(reported("lat=+37.7 owner=Alice"), None);
        assert_eq!(reported(&"k".repeat(65)), None);
        assert_eq!(printable(b"a\x00b\xff"), "a?b?");
        assert_eq!(printable(&[b'x'; 300]).len(), MAX_NAME);
    }
}
