//! `--scale W:H` (`docs/design/transcode.md`, "Size"): the forms of
//! FFmpeg's `scale` filter that CoreAV takes, and the output size they give
//! for an input size.
//!
//! Each side is a size in pixels (1 to [`MAX_SIZE`]), `-1` (keep the
//! aspect ratio of the input's pixel grid, rounded to the nearest whole
//! pixel, halves up) or `-2` (the same, rounded to the nearest even number,
//! halves up). Both sides negative keep the input's size, rounding
//! included. The ratio is the input's width to its height, not its display
//! aspect ratio: the sample aspect ratio is changed instead, so that the
//! display aspect ratio is kept. All of this is FFmpeg's `scale` filter as
//! measured as a black box (the pinned FFmpeg's `scale=W:H` on sources of
//! several sizes; the cases are this module's tests); a size of 0 that the
//! rounding gives is refused, as FFmpeg refuses it. The parser has a fuzz
//! target (`fuzz/fuzz_targets/scale.rs`, bar B7).

use crate::encoder::MAX_SIZE;

/// One side of a [`Scale`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// This many pixels.
    Pixels(u32),
    /// `-1`: from the other side and the input's aspect ratio.
    Keep,
    /// `-2`: as [`Side::Keep`], rounded to an even number.
    KeepEven,
}

/// The output size `--scale` asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale {
    /// The width.
    pub width: Side,
    /// The height.
    pub height: Side,
}

/// Reads `W:H`: each side digits (1 to [`MAX_SIZE`], no sign), `-1` or
/// `-2`; `None` for anything else.
pub fn parse_scale(text: &str) -> Option<Scale> {
    let (width, height) = text.split_once(':')?;
    Some(Scale {
        width: side(width)?,
        height: side(height)?,
    })
}

fn side(text: &str) -> Option<Side> {
    match text {
        "-1" => Some(Side::Keep),
        "-2" => Some(Side::KeepEven),
        _ if !text.is_empty() && text.len() <= 5 && text.bytes().all(|b| b.is_ascii_digit()) => {
            let value: u32 = text.parse().ok()?;
            (1..=MAX_SIZE)
                .contains(&value)
                .then_some(Side::Pixels(value))
        }
        _ => None,
    }
}

impl Scale {
    /// The output size for an input of `width` × `height`; `None` when a
    /// side rounds to 0 or exceeds [`MAX_SIZE`], or the input has a side
    /// of 0.
    pub fn resolve(&self, width: u32, height: u32) -> Option<(u32, u32)> {
        if width == 0 || height == 0 {
            return None;
        }
        let (w, h) = match (self.width, self.height) {
            (Side::Pixels(w), Side::Pixels(h)) => (w, h),
            (Side::Pixels(w), other) => (w, derived(other, w, height, width)?),
            (other, Side::Pixels(h)) => (derived(other, h, width, height)?, h),
            // Both from the input: its own size.
            _ => (width, height),
        };
        let fits = |v: u32| (1..=MAX_SIZE).contains(&v);
        (fits(w) && fits(h)).then_some((w, h))
    }
}

/// `given × num / den`, rounded as `side` asks.
fn derived(side: Side, given: u32, num: u32, den: u32) -> Option<u32> {
    let (product, den) = (u64::from(given) * u64::from(num), u64::from(den));
    let value = match side {
        // round(p / d), halves up.
        Side::Keep => (2 * product + den) / (2 * den),
        // 2 × round(p / 2d), halves up.
        Side::KeepEven => 2 * ((product + den) / (2 * den)),
        Side::Pixels(v) => u64::from(v),
    };
    u32::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_forms() {
        let s = |w, h| {
            Some(Scale {
                width: w,
                height: h,
            })
        };
        assert_eq!(parse_scale("-2:360"), s(Side::KeepEven, Side::Pixels(360)));
        assert_eq!(parse_scale("640:-1"), s(Side::Pixels(640), Side::Keep));
        assert_eq!(
            parse_scale("16384:1"),
            s(Side::Pixels(16384), Side::Pixels(1))
        );
        assert_eq!(parse_scale("-1:-2"), s(Side::Keep, Side::KeepEven));
        for bad in [
            "",
            ":",
            "640",
            "640:",
            ":360",
            "0:360",
            "16385:2",
            "-3:360",
            "+640:360",
            "640x360",
            "640:360:1",
            " 640:360",
            "640 :360",
            "99999999999:1",
            "-0:1",
            "1.5:2",
            "--1:2",
        ] {
            assert_eq!(parse_scale(bad), None, "{bad:?}");
        }
    }

    /// FFmpeg's `scale` filter on sources of these sizes (pinned FFmpeg
    /// 7.0.2, `ffprobe` of `testsrc2=size=WxH,format=yuv444p,setsar=1,
    /// scale=SPEC`).
    #[test]
    fn sizes_as_ffmpeg_gives_them() {
        let cases = [
            ((320, 240), "-2:121", (162, 121)),
            ((320, 240), "-1:121", (161, 121)),
            ((320, 240), "-2:120", (160, 120)),
            ((320, 240), "101:-2", (101, 76)),
            ((320, 240), "-2:7", (10, 7)),
            ((320, 240), "-1:7", (9, 7)),
            ((1920, 1080), "-2:121", (216, 121)),
            ((1920, 1080), "-1:121", (215, 121)),
            ((1920, 1080), "-2:120", (214, 120)),
            ((1920, 1080), "101:-2", (101, 56)),
            ((1920, 1080), "101:-1", (101, 57)),
            ((1920, 1080), "-2:360", (640, 360)),
            ((720, 480), "-1:121", (182, 121)),
            ((720, 480), "-2:121", (182, 121)),
            ((720, 480), "101:-2", (101, 68)),
            ((720, 480), "101:-1", (101, 67)),
            ((720, 480), "-2:7", (10, 7)),
            ((720, 480), "-1:7", (11, 7)),
            ((720, 480), "-2:10", (16, 10)),
            ((720, 480), "-2:6", (10, 6)),
            ((1280, 544), "-2:121", (284, 121)),
            ((1280, 544), "-1:121", (285, 121)),
            ((1280, 544), "-2:360", (848, 360)),
            ((332, 500), "-2:7", (4, 7)),
            ((332, 500), "-1:7", (5, 7)),
            ((321, 241), "-2:-2", (321, 241)),
            ((321, 241), "-1:-1", (321, 241)),
            ((321, 241), "-2:241", (322, 241)),
            ((321, 241), "640:-2", (640, 480)),
            ((1000, 10), "-1:1", (100, 1)),
        ];
        for ((w, h), spec, want) in cases {
            let scale = parse_scale(spec).unwrap();
            assert_eq!(scale.resolve(w, h), Some(want), "{w}x{h} {spec}");
        }
        // A side that rounds to 0 is refused, as FFmpeg refuses it.
        assert_eq!(parse_scale("1:-1").unwrap().resolve(1000, 10), None);
        assert_eq!(parse_scale("16384:-1").unwrap().resolve(1, 16384), None);
        assert_eq!(parse_scale("-1:-1").unwrap().resolve(0, 10), None);
    }

    /// The fuzz target's properties over many sizes (`fuzz_targets/scale.rs`).
    #[test]
    fn resolved_sizes_are_in_range() {
        let specs = [
            "-1:-1", "-2:-2", "-1:-2", "-2:-1", "-2:1", "1:-2", "-1:16384", "16384:-2", "640:-1",
            "-2:360", "7:-1", "-1:7", "320:240",
        ];
        let mut state = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..20_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let w = (state & 0xffff) as u32;
            let h = ((state >> 16) & 0xffff) as u32;
            let spec = specs[(state >> 40) as usize % specs.len()];
            let scale = parse_scale(spec).unwrap();
            if let Some((rw, rh)) = scale.resolve(w, h) {
                assert!((1..=MAX_SIZE).contains(&rw) && (1..=MAX_SIZE).contains(&rh));
                let negative = |s: Side| s != Side::Keep && s != Side::KeepEven;
                if scale.width == Side::KeepEven && negative(scale.height) {
                    assert_eq!(rw % 2, 0, "{spec} {w}x{h}");
                }
                if scale.height == Side::KeepEven && negative(scale.width) {
                    assert_eq!(rh % 2, 0, "{spec} {w}x{h}");
                }
            }
        }
    }
}
