//! The text sample format shared by 3GPP timed text (3GPP TS 26.245 5.17,
//! `TextSample`) and QuickTime text media (QuickTime File Format, "Text
//! sample data"): a 16-bit big-endian text length, the text, then boxes
//! (atoms) that modify it, up to the end of the sample.

use coreav_isobmff::FourCc;

/// Modifier boxes counted by type in [`TextSample::modifiers`] and in the
/// summaries: those of 3GPP TS 26.245 5.17.1. Any other type, including
/// the QuickTime text extensions, counts as [`OTHER_MODIFIER`].
pub const MODIFIERS: [&[u8; 4]; 10] = [
    b"styl", b"hlit", b"hclr", b"krok", b"dlay", b"href", b"tbox", b"blnk", b"twrp", b"disp",
];

/// Index in [`ModifierCounts`] of a box whose type is not in
/// [`MODIFIERS`].
pub const OTHER_MODIFIER: usize = MODIFIERS.len();

/// How many modifier boxes of each type were seen: one slot per entry of
/// [`MODIFIERS`], then one for every other type.
pub type ModifierCounts = [u64; MODIFIERS.len() + 1];

/// Why a sample could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleError {
    /// Shorter than its 16-bit text length.
    Truncated,
}

/// One text sample.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSample {
    /// The text, decoded: UTF-16 when it starts with a byte order mark (and
    /// with that mark removed), else UTF-8, up to its first NUL character.
    /// Sequences that are not valid are replaced by U+FFFD.
    pub text: String,
    /// Whether any sequence was replaced while decoding.
    pub invalid_text: bool,
    /// Modifier boxes after the text, counted by type.
    pub modifiers: ModifierCounts,
    /// Bytes after the text that do not form whole boxes: ignored.
    pub trailing_bytes: bool,
}

/// Reads one sample. Never fails on bad text or modifier boxes; only a
/// sample shorter than its declared text is an error. An empty input is an
/// empty sample, as QuickTime and 3GPP writers use for gaps.
pub fn parse_sample(data: &[u8]) -> Result<TextSample, SampleError> {
    let mut sample = TextSample {
        text: String::new(),
        invalid_text: false,
        modifiers: [0; MODIFIERS.len() + 1],
        trailing_bytes: false,
    };
    let Some((length, rest)) = data.split_first_chunk::<2>() else {
        if !data.is_empty() {
            return Err(SampleError::Truncated);
        }
        return Ok(sample);
    };
    let length = usize::from(u16::from_be_bytes(*length));
    let (text, mut boxes) = rest
        .split_at_checked(length)
        .ok_or(SampleError::Truncated)?;
    (sample.text, sample.invalid_text) = decode(text);
    while !boxes.is_empty() {
        let Some((size, kind)) = boxes
            .split_first_chunk::<4>()
            .and_then(|(size, rest)| Some((u32::from_be_bytes(*size), rest.first_chunk::<4>()?)))
        else {
            sample.trailing_bytes = true;
            break;
        };
        // A box at least as large as its header and inside the sample;
        // 64-bit sizes and "to the end" (0) are not used in samples.
        let Some(rest) = usize::try_from(size)
            .ok()
            .filter(|size| *size >= 8)
            .and_then(|size| boxes.get(size..))
        else {
            sample.trailing_bytes = true;
            break;
        };
        let slot = MODIFIERS
            .iter()
            .position(|known| *known == kind)
            .unwrap_or(OTHER_MODIFIER);
        if let Some(count) = sample.modifiers.get_mut(slot) {
            *count = count.saturating_add(1);
        }
        boxes = rest;
    }
    Ok(sample)
}

/// The type of the modifier counted at `slot`, `None` for
/// [`OTHER_MODIFIER`].
pub fn modifier_type(slot: usize) -> Option<FourCc> {
    MODIFIERS.get(slot).map(|kind| FourCc(**kind))
}

/// Decodes text bytes (TS 26.245 5.1): UTF-16 after a byte order mark,
/// else UTF-8. TS 26.245 needs only the big-endian mark; Apple's subtitle
/// sample data also allows the little-endian one, so both are read. A UTF-8
/// byte order mark is dropped. The text ends at its first NUL character:
/// TS 26.245 needs no terminator, but some writers store a NUL-terminated
/// string in a fixed-size field followed by stale bytes. Returns the text
/// and whether anything was replaced.
fn decode(bytes: &[u8]) -> (String, bool) {
    let utf16 = match bytes {
        [0xFE, 0xFF, rest @ ..] => Some((rest, u16::from_be_bytes as fn([u8; 2]) -> u16)),
        [0xFF, 0xFE, rest @ ..] => Some((rest, u16::from_le_bytes as fn([u8; 2]) -> u16)),
        _ => None,
    };
    if let Some((units, unit)) = utf16 {
        let mut units: Vec<u16> = units
            .chunks_exact(2)
            .filter_map(|pair| pair.first_chunk::<2>().map(|pair| unit(*pair)))
            .collect();
        let mut invalid = false;
        // An odd final byte is half a code unit: replaced, like any other
        // invalid sequence, unless the text ended at a NUL before it.
        let mut half_unit = units.len().saturating_mul(2) != bytes.len().saturating_sub(2);
        if let Some(end) = units.iter().position(|u| *u == 0) {
            units.truncate(end);
            half_unit = false;
        }
        let mut text: String = char::decode_utf16(units)
            .map(|c| {
                c.unwrap_or_else(|_| {
                    invalid = true;
                    char::REPLACEMENT_CHARACTER
                })
            })
            .collect();
        if half_unit {
            text.push(char::REPLACEMENT_CHARACTER);
            invalid = true;
        }
        return (text, invalid);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let bytes = bytes
        .iter()
        .position(|b| *b == 0)
        .and_then(|end| bytes.get(..end))
        .unwrap_or(bytes);
    match core::str::from_utf8(bytes) {
        Ok(text) => (text.to_owned(), false),
        Err(_) => (String::from_utf8_lossy(bytes).into_owned(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(kind: &[u8; 4]) -> usize {
        MODIFIERS.iter().position(|k| *k == kind).unwrap()
    }

    #[test]
    fn utf8_text_with_a_style_box() {
        let mut data = vec![0, 5];
        data.extend_from_slice(b"Hello");
        data.extend_from_slice(&[0, 0, 0, 10]);
        data.extend_from_slice(b"styl");
        data.extend_from_slice(&[0, 0]);
        let sample = parse_sample(&data).unwrap();
        assert_eq!(sample.text, "Hello");
        assert!(!sample.invalid_text && !sample.trailing_bytes);
        assert_eq!(sample.modifiers[slot(b"styl")], 1);
    }

    #[test]
    fn utf16_after_a_byte_order_mark() {
        let data = [0, 6, 0xFE, 0xFF, 0, b'h', 0xD8, 0x3D];
        let sample = parse_sample(&data).unwrap();
        // A lone high surrogate is replaced.
        assert_eq!(sample.text, "h\u{FFFD}");
        assert!(sample.invalid_text);
    }

    #[test]
    fn text_ends_at_the_first_nul() {
        let sample = parse_sample(&[0, 6, b'a', b'b', 0, 0xFF, 0xFE, b'z']).unwrap();
        assert_eq!(sample.text, "ab");
        assert!(!sample.invalid_text);
        let sample = parse_sample(&[0, 8, 0xFE, 0xFF, 0, b'a', 0, 0, 0xD8, 0]).unwrap();
        assert_eq!(sample.text, "a");
        assert!(!sample.invalid_text);
    }

    #[test]
    fn a_half_utf16_unit_is_replaced() {
        let sample = parse_sample(&[0, 5, 0xFE, 0xFF, 0, b'h', 0xD8]).unwrap();
        assert_eq!(sample.text, "h\u{FFFD}");
        assert!(sample.invalid_text);
    }

    #[test]
    fn little_endian_utf16() {
        let data = [0, 6, 0xFF, 0xFE, b'h', 0, b'i', 0];
        let sample = parse_sample(&data).unwrap();
        assert_eq!(sample.text, "hi");
        assert!(!sample.invalid_text);
    }

    #[test]
    fn empty_and_truncated_samples() {
        assert_eq!(parse_sample(&[]).unwrap().text, "");
        assert_eq!(parse_sample(&[0, 0]).unwrap().text, "");
        assert_eq!(parse_sample(&[0]), Err(SampleError::Truncated));
        assert_eq!(parse_sample(&[0, 3, b'a']), Err(SampleError::Truncated));
    }

    #[test]
    fn bad_boxes_are_ignored() {
        let sample = parse_sample(&[0, 1, b'a', 0, 0, 0, 4, b'x', b'x', b'x', b'x']).unwrap();
        assert_eq!(sample.text, "a");
        assert!(sample.trailing_bytes);
        let sample = parse_sample(&[0, 1, 0xFF, 0, 0, 0, 8, b'z', b'z', b'z', b'z']).unwrap();
        assert!(sample.invalid_text);
        assert_eq!(sample.modifiers[OTHER_MODIFIER], 1);
    }
}
