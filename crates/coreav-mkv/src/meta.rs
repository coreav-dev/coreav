//! Metadata read in memory: `Tags` (RFC 9559 5.1.8) and `Chapters` (RFC
//! 9559 5.1.7). Each `Tag` and each `EditionEntry` is read on its own, so
//! one oversized element is skipped without the rest.

use crate::Error;
use crate::ebml::{self, ElementHeader};
use crate::ids;
use crate::reader::{Children, Counter};

/// How deep `SimpleTag` and `ChapterAtom` elements may nest; deeper ones
/// are left out.
pub const MAX_NESTING: usize = 8;

/// One `Tag`: what it applies to and its simple tags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tag {
    /// `TargetTypeValue`, when stored (default 50).
    pub target_type_value: Option<u64>,
    /// `TargetType`, when stored.
    pub target_type: Option<String>,
    /// `TagTrackUID` values; a 0 means every track.
    pub track_uids: Vec<u64>,
    /// `TagEditionUID` values.
    pub edition_uids: Vec<u64>,
    /// `TagChapterUID` values.
    pub chapter_uids: Vec<u64>,
    /// `TagAttachmentUID` values.
    pub attachment_uids: Vec<u64>,
    /// The `SimpleTag` children, in stored order.
    pub simple_tags: Vec<SimpleTag>,
}

impl Tag {
    /// The tag applies to the whole segment: it names no track, edition,
    /// chapter or attachment (a UID of 0 names none, RFC 9559 5.1.8.1.1).
    pub fn global(&self) -> bool {
        [
            &self.track_uids,
            &self.edition_uids,
            &self.chapter_uids,
            &self.attachment_uids,
        ]
        .iter()
        .all(|uids| uids.iter().all(|uid| *uid == 0))
    }
}

/// One `SimpleTag`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SimpleTag {
    /// `TagName`.
    pub name: String,
    /// `TagLanguage` as stored (default `und`).
    pub language: Option<String>,
    /// `TagLanguageBCP47`, when stored.
    pub language_bcp47: Option<String>,
    /// `TagDefault` (default 1).
    pub default: bool,
    /// `TagDefault` is stored (whatever its value).
    pub default_stored: bool,
    /// `TagString`, when stored.
    pub string: Option<String>,
    /// Size of `TagBinary`, when stored.
    pub binary_size: Option<u64>,
    /// Nested `SimpleTag` elements.
    pub children: Vec<SimpleTag>,
}

/// One `EditionEntry`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edition {
    /// `EditionUID`.
    pub uid: Option<u64>,
    /// `EditionFlagHidden`.
    pub hidden: bool,
    /// `EditionFlagDefault`.
    pub default: bool,
    /// `EditionFlagOrdered`.
    pub ordered: bool,
    /// The top-level `ChapterAtom` elements.
    pub chapters: Vec<Chapter>,
}

/// One `ChapterAtom`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chapter {
    /// `ChapterUID`.
    pub uid: Option<u64>,
    /// `ChapterStringUID`.
    pub string_uid: Option<String>,
    /// `ChapterTimeStart` in nanoseconds, when stored.
    pub start: Option<u64>,
    /// `ChapterTimeEnd` in nanoseconds, when stored.
    pub end: Option<u64>,
    /// `ChapterFlagHidden`.
    pub hidden: bool,
    /// `ChapterFlagEnabled` (default 1).
    pub enabled: bool,
    /// `ChapterDisplay` elements: `ChapString` and its `ChapLanguage`
    /// values (`eng` when none is stored).
    pub displays: Vec<(String, Vec<String>)>,
    /// Nested `ChapterAtom` elements.
    pub children: Vec<Chapter>,
}

/// What reading metadata retained, in bytes: its strings, and a fixed
/// charge for each element kept.
pub(crate) struct Retained(pub(crate) u64);

/// What one `SimpleTag` or `ChapterAtom` kept costs besides its strings.
const ELEMENT_CHARGE: u64 = 128;

impl Retained {
    fn string(&mut self, data: &[u8]) -> String {
        self.0 = self.0.saturating_add(data.len() as u64);
        ebml::string(data)
    }

    fn element(&mut self) {
        self.0 = self.0.saturating_add(ELEMENT_CHARGE);
    }
}

/// Parses one `Tag` held in memory, whose data starts at `base`.
pub(crate) fn parse_tag(
    data: &[u8],
    base: u64,
    max_size_len: u8,
    counter: &Counter,
    retained: &mut Retained,
) -> Result<Tag, Error> {
    let mut tag = Tag::default();
    for child in Children::new(data, base, max_size_len, counter) {
        let (h, d) = child?;
        match h.id {
            ids::TARGETS => {
                for child in Children::new(d, h.data_offset(), max_size_len, counter) {
                    let (h, d) = child?;
                    match h.id {
                        ids::TARGET_TYPE_VALUE => {
                            tag.target_type_value = Some(ebml::uint(d, &h)?);
                        }
                        ids::TARGET_TYPE => tag.target_type = Some(retained.string(d)),
                        ids::TAG_TRACK_UID => tag.track_uids.push(ebml::uint(d, &h)?),
                        ids::TAG_EDITION_UID => tag.edition_uids.push(ebml::uint(d, &h)?),
                        ids::TAG_CHAPTER_UID => tag.chapter_uids.push(ebml::uint(d, &h)?),
                        ids::TAG_ATTACHMENT_UID => {
                            tag.attachment_uids.push(ebml::uint(d, &h)?);
                        }
                        _ => {}
                    }
                }
            }
            ids::SIMPLE_TAG => {
                if let Some(simple) = parse_simple_tag(d, &h, max_size_len, counter, retained, 1)? {
                    tag.simple_tags.push(simple);
                }
            }
            _ => {}
        }
    }
    Ok(tag)
}

fn parse_simple_tag(
    data: &[u8],
    header: &ElementHeader,
    max_size_len: u8,
    counter: &Counter,
    retained: &mut Retained,
    depth: usize,
) -> Result<Option<SimpleTag>, Error> {
    if depth > MAX_NESTING {
        return Ok(None);
    }
    retained.element();
    let mut tag = SimpleTag {
        default: true,
        ..SimpleTag::default()
    };
    for child in Children::new(data, header.data_offset(), max_size_len, counter) {
        let (h, d) = child?;
        match h.id {
            ids::TAG_NAME => tag.name = retained.string(d),
            ids::TAG_LANGUAGE => tag.language = Some(retained.string(d)),
            ids::TAG_LANGUAGE_BCP47 => tag.language_bcp47 = Some(retained.string(d)),
            ids::TAG_DEFAULT => {
                tag.default = ebml::uint(d, &h)? != 0;
                tag.default_stored = true;
            }
            ids::TAG_STRING => tag.string = Some(retained.string(d)),
            ids::TAG_BINARY => tag.binary_size = h.size,
            ids::SIMPLE_TAG => {
                if let Some(nested) =
                    parse_simple_tag(d, &h, max_size_len, counter, retained, depth + 1)?
                {
                    tag.children.push(nested);
                }
            }
            _ => {}
        }
    }
    Ok(Some(tag))
}

/// Parses one `EditionEntry` held in memory, whose data starts at `base`.
pub(crate) fn parse_edition(
    data: &[u8],
    base: u64,
    max_size_len: u8,
    counter: &Counter,
    retained: &mut Retained,
) -> Result<Edition, Error> {
    let mut edition = Edition::default();
    for child in Children::new(data, base, max_size_len, counter) {
        let (h, d) = child?;
        match h.id {
            ids::EDITION_UID => edition.uid = Some(ebml::uint(d, &h)?),
            ids::EDITION_FLAG_HIDDEN => edition.hidden = ebml::uint(d, &h)? != 0,
            ids::EDITION_FLAG_DEFAULT => edition.default = ebml::uint(d, &h)? != 0,
            ids::EDITION_FLAG_ORDERED => edition.ordered = ebml::uint(d, &h)? != 0,
            ids::CHAPTER_ATOM => {
                if let Some(chapter) = parse_chapter(d, &h, max_size_len, counter, retained, 1)? {
                    edition.chapters.push(chapter);
                }
            }
            _ => {}
        }
    }
    Ok(edition)
}

fn parse_chapter(
    data: &[u8],
    header: &ElementHeader,
    max_size_len: u8,
    counter: &Counter,
    retained: &mut Retained,
    depth: usize,
) -> Result<Option<Chapter>, Error> {
    if depth > MAX_NESTING {
        return Ok(None);
    }
    retained.element();
    let mut chapter = Chapter {
        enabled: true,
        ..Chapter::default()
    };
    for child in Children::new(data, header.data_offset(), max_size_len, counter) {
        let (h, d) = child?;
        match h.id {
            ids::CHAPTER_UID => chapter.uid = Some(ebml::uint(d, &h)?),
            ids::CHAPTER_STRING_UID => chapter.string_uid = Some(retained.string(d)),
            ids::CHAPTER_TIME_START => chapter.start = Some(ebml::uint(d, &h)?),
            ids::CHAPTER_TIME_END => chapter.end = Some(ebml::uint(d, &h)?),
            ids::CHAPTER_FLAG_HIDDEN => chapter.hidden = ebml::uint(d, &h)? != 0,
            ids::CHAPTER_FLAG_ENABLED => chapter.enabled = ebml::uint(d, &h)? != 0,
            ids::CHAPTER_DISPLAY => {
                let (mut string, mut languages) = (String::new(), Vec::new());
                for child in Children::new(d, h.data_offset(), max_size_len, counter) {
                    let (h, d) = child?;
                    match h.id {
                        ids::CHAP_STRING => string = retained.string(d),
                        ids::CHAP_LANGUAGE => languages.push(retained.string(d)),
                        _ => {}
                    }
                }
                if languages.is_empty() {
                    languages.push("eng".to_owned());
                }
                chapter.displays.push((string, languages));
            }
            ids::CHAPTER_ATOM => {
                if let Some(nested) =
                    parse_chapter(d, &h, max_size_len, counter, retained, depth + 1)?
                {
                    chapter.children.push(nested);
                }
            }
            _ => {}
        }
    }
    Ok(Some(chapter))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_with_targets_and_nested_simple_tags() {
        // Tag { Targets { TagTrackUID 5 }, SimpleTag { TagName "A",
        // TagString "b", SimpleTag { TagName "C" } } }
        let data = [
            0x63, 0xC0, 0x84, 0x63, 0xC5, 0x81, 0x05, // Targets
            0x67, 0xC8, 0x8F, // SimpleTag, 15 bytes
            0x45, 0xA3, 0x81, b'A', // TagName
            0x44, 0x87, 0x81, b'b', // TagString
            0x67, 0xC8, 0x84, 0x45, 0xA3, 0x81, b'C', // nested
        ];
        let counter = Counter::new(100);
        let mut retained = Retained(0);
        let tag = parse_tag(&data, 0, 8, &counter, &mut retained).unwrap();
        assert_eq!(tag.track_uids, vec![5]);
        assert!(!tag.global());
        let simple = &tag.simple_tags[0];
        assert_eq!(simple.name, "A");
        assert_eq!(simple.string.as_deref(), Some("b"));
        assert!(simple.default);
        assert!(!simple.default_stored);
        assert_eq!(simple.children[0].name, "C");
        assert_eq!(retained.0, 3 + 2 * ELEMENT_CHARGE);
    }

    #[test]
    fn an_edition_with_a_chapter() {
        // EditionEntry { ChapterAtom { ChapterUID 7, ChapterTimeStart 1000,
        // ChapterDisplay { ChapString "x" } } }
        let data = [
            0xB6, 0x8D, // ChapterAtom, 13 bytes
            0x73, 0xC4, 0x81, 0x07, // ChapterUID
            0x91, 0x82, 0x03, 0xE8, // ChapterTimeStart
            0x80, 0x83, 0x85, 0x81, b'x', // ChapterDisplay
        ];
        let counter = Counter::new(100);
        let mut retained = Retained(0);
        let edition = parse_edition(&data, 0, 8, &counter, &mut retained).unwrap();
        let chapter = &edition.chapters[0];
        assert_eq!(chapter.uid, Some(7));
        assert_eq!(chapter.start, Some(1000));
        assert_eq!(chapter.end, None);
        assert!(chapter.enabled);
        assert_eq!(
            chapter.displays,
            vec![("x".to_owned(), vec!["eng".to_owned()])]
        );
    }
}
