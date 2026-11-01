//! Shared ffprobe field derivations and number formats.

use coreav_isobmff::{Codec, Color, FourCc, Movie, Track};

/// The size `ffprobe` reports for a 3GPP timed text stream: the track
/// header's width and height (signed 16.16 fixed point, ISO/IEC 14496-12
/// 8.3.2), whole pixels, when both whole parts are positive (measured on
/// crafted files; the `tx3g` entry's default text box plays no part).
/// Known limit: `ffprobe` also reports nothing for sizes it rejects as a
/// picture size, measured between 16,000 and 16,384 pixels a side; those
/// are reported here. A track that a `chap` track reference names, from
/// any track, gets no size: `ffprobe` lists it as a `bin_data` data stream
/// (`KD-CHAPTER-TEXT`). `tools/isobmff-adapter` maps the
/// oracle's fields the same way.
pub(crate) fn text_size(movie: &Movie, track: &Track) -> Option<(u32, u32)> {
    let whole = |fixed: u32| i32::from_be_bytes(fixed.to_be_bytes()) >> 16;
    let (width, height) = (
        whole(track.presentation_size.0),
        whole(track.presentation_size.1),
    );
    (track.description.codec == Codec::TimedText
        && !movie.chapter_references.contains(&track.track_id)
        && width > 0
        && height > 0)
        .then(|| (width.unsigned_abs(), height.unsigned_abs()))
}

/// The colour description `ffprobe` reports, from the two sources probe
/// keeps apart (`docs/design/probe-color.md`): the sample entry's `colr` box
/// (`color`) and the parameter set of the codec configuration, raw
/// (`parameter_set_color`). For H.264 the parameter set's colour description
/// when its video usability information carries one ("unspecified"
/// included) and its range when it signals one, else the box's (only for a
/// track with frames in the file); for HEVC and AV1 always the parameter set, with HEVC's range limited when the
/// parameter set does not signal it (ITU-T H.265 E.3.1); for other codecs
/// the box. `tools/isobmff-adapter` maps the oracle's fields the same way.
pub(crate) fn reported_color(track: &Track) -> Color {
    let d = &track.description;
    // ffprobe applies an H.264 SPS's colour only to a frame it decodes: a
    // track without samples in the file (an initialization segment) keeps
    // the box's.
    let frames = !track.external_data && (track.sample_count > 0 || track.fragment_samples > 0);
    precedence(d.codec, d.color, d.parameter_set_color, frames)
}

/// [`reported_color`] from its inputs: the codec, the box's colour, the
/// parameter set's (raw), and whether the track has frames in the file.
fn precedence(codec: Codec, boxed: Color, set: Option<Color>, frames: bool) -> Color {
    let reads_set = match codec {
        Codec::H264 => frames,
        Codec::Hevc | Codec::Av1 => true,
        _ => false,
    };
    let pick = |own: Option<u16>, other: Option<u16>| own.filter(|&code| code != 2).or(other);
    let color = match set.filter(|_| reads_set) {
        None => boxed,
        Some(set) => Color {
            primaries: pick(boxed.primaries, set.primaries),
            transfer: pick(boxed.transfer, set.transfer),
            matrix: pick(boxed.matrix, set.matrix),
            full_range: boxed.full_range.or(set.full_range),
        },
    };
    let limited = match codec {
        Codec::Hevc => set.is_some(),
        Codec::ProRes => true,
        _ => false,
    };
    Color {
        full_range: color.full_range.or(limited.then_some(false)),
        ..color
    }
}

/// The format bit rate: file size in bits over the duration, truncated.
pub(crate) fn format_bit_rate(size: u64, micros: i64) -> Option<u64> {
    let micros = u128::try_from(micros).ok().filter(|us| *us > 0)?;
    let bits = u128::from(size).checked_mul(8_000_000)?;
    u64::try_from(bits / micros).ok()
}

/// A four-character code as `ffprobe` prints `codec_tag_string`: printable
/// bytes as characters, others as `[N]`.
pub(crate) fn tag_string(fourcc: FourCc) -> String {
    let mut out = String::new();
    for byte in fourcc.0 {
        if byte.is_ascii_graphic() || byte == b' ' {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("[{byte}]"));
        }
    }
    out
}

pub(crate) fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

/// `N:D` in lowest terms; `None` when either side is zero.
pub(crate) fn ratio(a: u64, b: u64) -> Option<String> {
    if a == 0 || b == 0 {
        return None;
    }
    let g = gcd(a, b);
    Some(format!("{}:{}", a / g, b / g))
}

/// The largest term of a display aspect ratio `ffprobe` prints.
const MAX_RATIO_TERM: u64 = 1 << 20;

/// `num / den` as the closest fraction whose terms are both at most
/// [`MAX_RATIO_TERM`], which is how `ffprobe` prints a display aspect
/// ratio that does not reduce that far (1045314:784271 for a 914x686
/// picture of samples 2740:2739, exactly 1252180:939477): the best of
/// the convergents and intermediate fractions of its continued fraction
/// within the bound. `None` when either term is 0.
pub(crate) fn bounded_ratio(num: u64, den: u64) -> Option<(u64, u64)> {
    if num == 0 || den == 0 {
        return None;
    }
    let g = gcd(num, den);
    let (num, den) = (num / g, den / g);
    if num <= MAX_RATIO_TERM && den <= MAX_RATIO_TERM {
        return Some((num, den));
    }
    // Convergents p/q of num/den, and between two of them the
    // intermediate fractions, kept while both terms stay in bounds.
    let (mut p0, mut q0, mut p1, mut q1) = (0u64, 1u64, 1u64, 0u64);
    let (mut a, mut b) = (num, den);
    let mut best: Option<(u64, u64)> = None;
    // |p/q - num/den| times q * den, exactly.
    let off = |(p, q): (u64, u64)| {
        (u128::from(p) * u128::from(den)).abs_diff(u128::from(num) * u128::from(q))
    };
    // Whether x lies closer to num/den than y does.
    let closer = |x: (u64, u64), y: (u64, u64)| off(x) * u128::from(y.1) < off(y) * u128::from(x.1);
    while b != 0 {
        let q = a / b;
        for t in 1..=q {
            let p = t.checked_mul(p1).and_then(|v| v.checked_add(p0));
            let k = t.checked_mul(q1).and_then(|v| v.checked_add(q0));
            let (Some(p), Some(k)) = (p, k) else {
                return best;
            };
            if p > MAX_RATIO_TERM || k > MAX_RATIO_TERM {
                return best;
            }
            if best.is_none_or(|b| closer((p, k), b)) {
                best = Some((p, k));
            }
        }
        (p0, p1) = (p1, q * p1 + p0);
        (q0, q1) = (q1, q * q1 + q0);
        (a, b) = (b, a % b);
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_aspect_ratios_within_twenty_bits() {
        // 914x686 at 2740:2739 is 1252180:939477, which `ffprobe` prints
        // as the closest fraction of terms up to 2^20.
        assert_eq!(
            bounded_ratio(914 * 2740, 686 * 2739),
            Some((1_045_314, 784_271))
        );
        assert_eq!(bounded_ratio(1920, 1080), Some((16, 9)));
        assert_eq!(bounded_ratio(0, 9), None);
    }

    #[test]
    fn number_formats() {
        assert_eq!(format_bit_rate(92_353, 2_000_000), Some(369_412));
        assert_eq!(format_bit_rate(1, 0), None);
        assert_eq!(ratio(1280, 960).as_deref(), Some("4:3"));
        assert_eq!(tag_string(FourCc(*b"avc1")), "avc1");
        assert_eq!(tag_string(FourCc([0, b'a', b'b', b'c'])), "[0]abc");
    }
}

#[cfg(test)]
mod colour_tests {
    use super::precedence;
    use coreav_isobmff::{Codec, Color};

    fn c(p: Option<u16>, t: Option<u16>, m: Option<u16>, r: Option<bool>) -> Color {
        Color {
            primaries: p,
            transfer: t,
            matrix: m,
            full_range: r,
        }
    }

    #[test]
    fn ffprobe_precedence() {
        let boxed = c(Some(1), Some(1), Some(1), Some(true));
        let sps = c(Some(6), Some(6), Some(6), Some(false));
        let range_only = c(None, None, None, Some(false));
        let silent = Color::default();
        let unspecified = c(Some(2), Some(2), Some(2), None);
        // H.264 with frames, HEVC and AV1: each code of the box unless it is
        // 2 (unspecified), else the parameter set's; the range likewise.
        assert_eq!(precedence(Codec::H264, boxed, Some(sps), true), boxed);
        assert_eq!(precedence(Codec::H264, unspecified, Some(sps), true), sps);
        assert_eq!(precedence(Codec::H264, silent, Some(sps), true), sps);
        assert_eq!(precedence(Codec::Hevc, boxed, Some(sps), true), boxed);
        assert_eq!(
            precedence(Codec::Av1, silent, Some(range_only), true),
            range_only
        );
        assert_eq!(precedence(Codec::Av1, boxed, Some(sps), true), boxed);
        // H.264 without frames, or without a parameter set: the box's.
        assert_eq!(
            precedence(Codec::H264, unspecified, Some(sps), false),
            unspecified
        );
        assert_eq!(precedence(Codec::H264, boxed, None, true), boxed);
        // HEVC with a parameter set, and ProRes: an unsignalled range is
        // limited.
        assert_eq!(
            precedence(Codec::Hevc, silent, Some(silent), true),
            range_only
        );
        assert_eq!(precedence(Codec::Hevc, silent, None, true), silent);
        assert_eq!(precedence(Codec::ProRes, silent, None, true), range_only);
        assert_eq!(precedence(Codec::ProRes, boxed, None, true), boxed);
        // Other codecs: the box's.
        assert_eq!(precedence(Codec::Vp9, silent, Some(sps), true), silent);
    }
}
