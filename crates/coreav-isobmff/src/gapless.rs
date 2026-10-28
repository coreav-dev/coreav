//! The iTunes gapless item `iTunSMPB`.
//!
//! Some AAC files give their encoder delay and padding only in this
//! metadata item, not in an edit list. It is read on request, never by
//! probe, and only for timing: no other metadata is parsed or exposed
//! (other tags are deferred). The item sits in the movie's `udta`/`meta`
//! (ISO/IEC 14496-12 8.10.1 and 8.11.1, or the QuickTime layout without
//! version and flags) in an `ilst` list as a `----` item whose `mean` is
//! `com.apple.iTunes` and whose `name` is `iTunSMPB`; its `data` holds text
//! fields in hexadecimal. This layout is an Apple iTunes convention, not part
//! of the cited specifications, so a caller uses the values only after
//! checking them against the sample table.
//!
//! Reading is bounded, and malformed data never fails the caller: any
//! malformed box, an oversized payload or more than [`MAX_TAG_BOXES`] boxes
//! gives `None`. A source that fails to read ([`Error::Io`]) is an error,
//! not an absent item, so that a transient read error cannot change the
//! output.

use crate::header::{BoxHeader, read_header};
use crate::{Error, FourCc, Source};

/// Box headers read at most while looking for the item.
pub const MAX_TAG_BOXES: u32 = 1024;
/// Largest `mean`, `name` or `data` payload read.
const MAX_ITEM_PAYLOAD: u64 = 256;
const MEAN: &[u8] = b"com.apple.iTunes";
const NAME: &[u8] = b"iTunSMPB";

/// Values of an `iTunSMPB` item, in samples at the decoded sample rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItunesGapless {
    /// Leading samples that are encoder delay (priming), not content.
    pub encoder_delay: u32,
    /// Trailing samples that fill the last frame.
    pub padding: u32,
    /// Samples of content between the delay and the padding.
    pub original_samples: u64,
}

/// Looks for the item under the movie-level `udta` and `meta` boxes
/// `roots`.
pub(crate) fn read<S: Source>(
    source: &mut S,
    roots: &[BoxHeader],
) -> Result<Option<ItunesGapless>, Error> {
    let mut budget = MAX_TAG_BOXES;
    for root in roots {
        let found = match &root.kind.0 {
            b"udta" => {
                let mut found = None;
                for meta in children(source, root.payload_offset(), root.end(), &mut budget)? {
                    if meta.kind == FourCc(*b"meta") {
                        found = in_meta(source, &meta, &mut budget)?;
                        if found.is_some() {
                            break;
                        }
                    }
                }
                found
            }
            b"meta" => in_meta(source, root, &mut budget)?,
            _ => None,
        };
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

/// `Ok(None)` for an error that malformed data causes, the error itself
/// for a source that failed to read.
pub(crate) fn absent<T>(error: Error) -> Result<Option<T>, Error> {
    match error {
        Error::Io(_) => Err(error),
        _ => Ok(None),
    }
}

fn in_meta<S: Source>(
    source: &mut S,
    meta: &BoxHeader,
    budget: &mut u32,
) -> Result<Option<ItunesGapless>, Error> {
    // ISO `meta` is a full box; the QuickTime one starts with its handler.
    let mut probe = [0u8; 8];
    if let Err(error) = source.read_exact_at(meta.payload_offset(), &mut probe) {
        return absent(error);
    }
    let start = if probe.get(4..8) == Some(b"hdlr".as_slice()) {
        meta.payload_offset()
    } else {
        let Some(start) = meta.payload_offset().checked_add(4) else {
            return Ok(None);
        };
        start
    };
    let Some(ilst) = children(source, start, meta.end(), budget)?
        .into_iter()
        .find(|c| c.kind == FourCc(*b"ilst"))
    else {
        return Ok(None);
    };
    for item in children(source, ilst.payload_offset(), ilst.end(), budget)? {
        if item.kind != FourCc(*b"----") {
            continue;
        }
        let (mut mean, mut name, mut data) = (None, None, None);
        for part in children(source, item.payload_offset(), item.end(), budget)? {
            let slot = match &part.kind.0 {
                b"mean" => &mut mean,
                b"name" => &mut name,
                b"data" => &mut data,
                _ => continue,
            };
            *slot = small_payload(source, &part)?;
        }
        // `mean` and `name` are full boxes; `data` has a type indicator and a
        // locale before the value.
        let is_smpb = mean.as_deref().and_then(|m| m.get(4..)) == Some(MEAN)
            && name.as_deref().and_then(|n| n.get(4..)) == Some(NAME);
        if is_smpb {
            return Ok(data.as_deref().and_then(|d| d.get(8..)).and_then(parse));
        }
    }
    Ok(None)
}

/// Child box headers between `start` and `end`, stopping at the first
/// malformed one or when the budget is spent; a failed read is an error.
fn children<S: Source>(
    source: &mut S,
    start: u64,
    end: u64,
    budget: &mut u32,
) -> Result<Vec<BoxHeader>, Error> {
    let mut out = Vec::new();
    let mut at = start;
    while end.saturating_sub(at) >= 8 && *budget > 0 {
        *budget = budget.saturating_sub(1);
        let header = match read_header(source, at, end) {
            Ok(header) => header,
            Err(error @ Error::Io(_)) => return Err(error),
            Err(_) => break,
        };
        at = header.end();
        out.push(header);
    }
    Ok(out)
}

fn small_payload<S: Source>(source: &mut S, header: &BoxHeader) -> Result<Option<Vec<u8>>, Error> {
    if header.payload_len() > MAX_ITEM_PAYLOAD {
        return Ok(None);
    }
    let Ok(len) = usize::try_from(header.payload_len()) else {
        return Ok(None);
    };
    let mut payload = vec![0u8; len];
    if let Err(error) = source.read_exact_at(header.payload_offset(), &mut payload) {
        return absent(error);
    }
    Ok(Some(payload))
}

/// Parses the item's text: fields of hexadecimal digits separated by spaces,
/// the second the encoder delay, the third the padding and the fourth the
/// content length.
pub(crate) fn parse(text: &[u8]) -> Option<ItunesGapless> {
    let text = core::str::from_utf8(text).ok()?.trim_end_matches('\0');
    let mut fields = text.split_ascii_whitespace();
    let _reserved = fields.next()?;
    let mut hex = || {
        let field = fields.next()?;
        if field.is_empty() || field.len() > 16 || !field.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u64::from_str_radix(field, 16).ok()
    };
    let encoder_delay = u32::try_from(hex()?).ok()?;
    let padding = u32::try_from(hex()?).ok()?;
    let original_samples = hex()?;
    Some(ItunesGapless {
        encoder_delay,
        padding,
        original_samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SliceSource;

    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(payload.len() + 8)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn item(mean: &[u8], name: &[u8], value: &[u8]) -> Vec<u8> {
        let full = |body: &[u8]| [&[0u8; 4][..], body].concat();
        let data = [&[0, 0, 0, 1, 0, 0, 0, 0][..], value].concat();
        let parts = [
            boxed(b"mean", &full(mean)),
            boxed(b"name", &full(name)),
            boxed(b"data", &data),
        ]
        .concat();
        boxed(b"----", &parts)
    }

    const TEXT: &[u8] = b" 00000000 00000840 000003DC 00000000004D8BE4 00000000 00000000";

    fn udta(meta_full_box: bool, items: &[u8]) -> Vec<u8> {
        let hdlr = boxed(b"hdlr", &[0; 25]);
        let ilst = boxed(b"ilst", items);
        let body = if meta_full_box {
            [&[0u8; 4][..], &hdlr, &ilst].concat()
        } else {
            [hdlr, ilst].concat()
        };
        boxed(b"udta", &boxed(b"meta", &body))
    }

    fn read_all(data: &[u8]) -> Option<ItunesGapless> {
        let mut source = SliceSource::new(data);
        let root = read_header(&mut source, 0, data.len() as u64).unwrap();
        // Slices fail only past their end, which is malformed data.
        read(&mut source, &[root]).unwrap()
    }

    const EXPECTED: ItunesGapless = ItunesGapless {
        encoder_delay: 0x840,
        padding: 0x3dc,
        original_samples: 0x4d_8be4,
    };

    #[test]
    fn reads_the_item_in_both_meta_layouts() {
        let other = item(b"com.apple.iTunes", b"iTunNORM", b" 1 2 3 4");
        let smpb = item(MEAN, NAME, TEXT);
        for full in [true, false] {
            let data = udta(full, &[other.clone(), smpb.clone()].concat());
            assert_eq!(read_all(&data), Some(EXPECTED));
        }
    }

    #[test]
    fn other_items_and_malformed_boxes_give_none() {
        assert_eq!(
            read_all(&udta(true, &item(b"com.example", NAME, TEXT))),
            None
        );
        assert_eq!(read_all(&udta(true, &item(MEAN, b"iTunNORM", TEXT))), None);
        let mut data = udta(true, &item(MEAN, NAME, TEXT));
        // A child that runs past its parent.
        let at = data.len() - 20;
        data[at..at + 4].copy_from_slice(&1000u32.to_be_bytes());
        assert_eq!(read_all(&data), None);
        // Every truncation is safe.
        let whole = udta(true, &item(MEAN, NAME, TEXT));
        for len in 8..whole.len() {
            let mut source = SliceSource::new(&whole[..len]);
            if let Ok(root) = read_header(&mut source, 0, whole.len() as u64) {
                assert_eq!(read(&mut source, &[root]), Ok(None));
            }
        }
    }

    #[test]
    fn text_is_parsed_strictly() {
        assert_eq!(parse(TEXT), Some(EXPECTED));
        assert_eq!(parse(b" 0 840 3dc 4d8be4\0\0"), Some(EXPECTED));
        assert_eq!(parse(b" 0 840 3dc"), None);
        assert_eq!(parse(b" 0 +840 3dc 4d8be4"), None);
        assert_eq!(parse(b" 0 840 3dc 4d8be4g"), None);
        assert_eq!(parse(b" 0 100000000 0 1"), None);
        assert_eq!(parse(b" 0 840 3dc 10000000000000000"), None);
        assert_eq!(parse(&[0xff, 0xfe]), None);
    }
}
