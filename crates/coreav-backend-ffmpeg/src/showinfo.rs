//! Reading FFmpeg's `showinfo` lines and raw frame layouts. FFmpeg is
//! untrusted: every field is checked, and a line or a value that does not
//! parse is an error, never a guess.

use coreav_frame::{Chroma, ChromaLocation, PixelFormat};

/// What one `showinfo` line says about a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    /// Frame number (`n`).
    pub n: u64,
    /// `pts`, in the input's time base.
    pub pts: i64,
    /// Pixel format.
    pub format: RawFormat,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Chroma location (`cl`), when stated.
    pub chroma_location: Option<ChromaLocation>,
}

/// A raw video pixel format FFmpeg writes: the frame model's format, plus
/// what the frame model drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawFormat {
    /// The format of the colour planes.
    pub format: PixelFormat,
    /// An alpha plane follows the colour planes, at full size and the same
    /// depth.
    pub alpha: bool,
    /// The `yuvj` formats: full range.
    pub full_range: bool,
}

impl RawFormat {
    /// The format FFmpeg names `name`, for the formats the frame model
    /// takes.
    pub fn from_name(name: &str) -> Option<Self> {
        let (yuvj, rest) = if let Some(rest) = name.strip_prefix("yuvj") {
            (true, rest)
        } else if let Some(rest) = name.strip_prefix("yuva") {
            (false, rest)
        } else if let Some(rest) = name.strip_prefix("yuv") {
            (false, rest)
        } else {
            (false, name)
        };
        let alpha = name.starts_with("yuva") || name.starts_with("gbrap");
        let plain = |format| {
            Some(Self {
                format,
                alpha,
                full_range: yuvj,
            })
        };
        if name.starts_with("yuv") {
            let (chroma, depth_part) = if let Some(d) = rest.strip_prefix("420p") {
                (Chroma::Cs420, d)
            } else if let Some(d) = rest.strip_prefix("422p") {
                (Chroma::Cs422, d)
            } else {
                (Chroma::Cs444, rest.strip_prefix("444p")?)
            };
            if yuvj && !depth_part.is_empty() {
                return None;
            }
            let depth = depth_le(depth_part)?;
            return plain(PixelFormat::Yuv { chroma, depth });
        }
        if let Some(d) = name.strip_prefix("gray") {
            let depth = depth_le(d)?;
            return plain(PixelFormat::Yuv {
                chroma: Chroma::Mono,
                depth,
            });
        }
        if let Some(d) = name
            .strip_prefix("gbrap")
            .or_else(|| name.strip_prefix("gbrp"))
        {
            let depth = depth_le(d)?;
            return plain(PixelFormat::Gbr { depth });
        }
        match name {
            "nv12" => plain(PixelFormat::SemiPlanar { depth: 8 }),
            "p010le" => plain(PixelFormat::SemiPlanar { depth: 10 }),
            "p016le" => plain(PixelFormat::SemiPlanar { depth: 16 }),
            _ => None,
        }
    }

    /// Bytes of each plane FFmpeg writes for a `width` × `height` frame,
    /// rows packed, in order; the alpha plane last when there is one.
    pub fn plane_bytes(&self, width: u32, height: u32) -> Option<Vec<usize>> {
        let mut out = Vec::new();
        for plane in 0..self.format.plane_count() {
            let row = self.format.row_bytes(plane, width, height)?;
            let (_, rows) = self.format.plane_size(plane, width, height)?;
            out.push(row.checked_mul(usize::try_from(rows).ok()?)?);
        }
        if self.alpha {
            let luma = out.first().copied()?;
            out.push(luma);
        }
        Some(out)
    }
}

/// `""` is 8 bits; `"10le"` 10 bits, little-endian samples.
fn depth_le(s: &str) -> Option<u8> {
    if s.is_empty() {
        return Some(8);
    }
    let digits = s.strip_suffix("le")?;
    let depth: u8 = digits.parse().ok()?;
    (9..=16).contains(&depth).then_some(depth)
}

/// The frame line of a `showinfo` filter (`[Parsed_showinfo_0 @ 0x...]
/// n: 0 pts: 0 ... fmt:yuv420p cl:left ... s:640x360 ...`); `None` for its
/// other lines and FFmpeg's other messages. `Some(Err)` for a frame line
/// that does not parse.
pub fn parse_line(line: &str) -> Option<Result<FrameInfo, String>> {
    let body = line.strip_prefix("[Parsed_showinfo_0 @ ")?;
    let (_, rest) = body.split_once("] ")?;
    let rest = rest.trim_start();
    if !rest.starts_with("n:") {
        return None;
    }
    Some(parse_fields(rest))
}

fn parse_fields(rest: &str) -> Result<FrameInfo, String> {
    // Fields are `key:value`; `n:` and `pts:` may be followed by padding.
    let mut n = None;
    let mut pts = None;
    let mut format = None;
    let mut size = None;
    let mut chroma_location = None;
    let mut tokens = rest.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        let Some((key, value)) = token.split_once(':') else {
            continue;
        };
        let value = if value.is_empty() {
            tokens.next().unwrap_or("")
        } else {
            value
        };
        match key {
            "n" => n = value.parse::<u64>().ok(),
            "pts" => pts = value.parse::<i64>().ok(),
            "fmt" => format = Some(value),
            "s" => size = value.split_once('x'),
            "cl" => {
                chroma_location = match value {
                    "left" => Some(ChromaLocation::Left),
                    "center" => Some(ChromaLocation::Center),
                    "topleft" => Some(ChromaLocation::TopLeft),
                    "top" => Some(ChromaLocation::Top),
                    "bottomleft" => Some(ChromaLocation::BottomLeft),
                    "bottom" => Some(ChromaLocation::Bottom),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    let n = n.ok_or("showinfo line without a frame number")?;
    let pts = pts.ok_or("showinfo line without a pts")?;
    let name = format.ok_or("showinfo line without a pixel format")?;
    let format = RawFormat::from_name(name)
        .ok_or_else(|| format!("unsupported pixel format {}", truncate(name, 32)))?;
    let (w, h) = size.ok_or("showinfo line without a size")?;
    let width = w.parse::<u32>().map_err(|_| "bad width")?;
    let height = h.parse::<u32>().map_err(|_| "bad height")?;
    Ok(FrameInfo {
        n,
        pts,
        format,
        width,
        height,
        chroma_location,
    })
}

/// At most `max` bytes of `s`, cut at a character boundary.
pub fn truncate(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_lines() {
        let line = "[Parsed_showinfo_0 @ 0x7fea48002640] n:   0 pts:      0 pts_time:0       duration:     40 duration_time:0.04    fmt:yuv420p cl:left sar:1/1 s:320x240 i:P iskey:1 type:I ";
        let info = parse_line(line).expect("frame line").expect("parses");
        assert_eq!(info.n, 0);
        assert_eq!(info.pts, 0);
        assert_eq!((info.width, info.height), (320, 240));
        assert_eq!(info.format.format, PixelFormat::YUV420P);
        assert_eq!(info.chroma_location, Some(ChromaLocation::Left));
        let ten = "[Parsed_showinfo_0 @ 0x7fc650002640] n:  12 pts: 1099511627776 pts_time:1099.51 duration: 40 fmt:yuv420p10le cl:unspecified sar:1/1 s:320x240 i:P iskey:0 type:B";
        let info = parse_line(ten).expect("line").expect("parses");
        assert_eq!(info.pts, 1 << 40);
        assert_eq!(
            info.format.format,
            PixelFormat::Yuv {
                chroma: Chroma::Cs420,
                depth: 10
            }
        );
        assert_eq!(info.chroma_location, None);
        assert!(parse_line("[Parsed_showinfo_0 @ 0x1] color_range:tv color_space:bt709").is_none());
        assert!(parse_line("frame=    1 fps=0.0").is_none());
        let odd = "[Parsed_showinfo_0 @ 0x1] n: 1 pts: 5 fmt:bayer_rggb8 s:2x2";
        assert!(matches!(parse_line(odd), Some(Err(_))));
        let bad = "[Parsed_showinfo_0 @ 0x1] n: 1 pts: x fmt:yuv420p s:2x2";
        assert!(matches!(parse_line(bad), Some(Err(_))));
    }

    #[test]
    fn formats_and_layouts() {
        let j = RawFormat::from_name("yuvj422p").expect("yuvj422p");
        assert!(j.full_range);
        let a = RawFormat::from_name("yuva444p12le").expect("yuva444p12le");
        assert!(a.alpha);
        assert_eq!(a.plane_bytes(4, 2), Some(vec![16, 16, 16, 16]));
        assert_eq!(
            RawFormat::from_name("gbrp10le").map(|f| f.format),
            Some(PixelFormat::Gbr { depth: 10 })
        );
        assert_eq!(
            RawFormat::from_name("gray").map(|f| f.format),
            Some(PixelFormat::Yuv {
                chroma: Chroma::Mono,
                depth: 8
            })
        );
        assert!(RawFormat::from_name("yuv440p").is_none());
        assert!(RawFormat::from_name("yuv420p10be").is_none());
        assert!(RawFormat::from_name("yuvj420p10le").is_none());
        let p = RawFormat::from_name("yuv420p").expect("yuv420p");
        assert_eq!(p.plane_bytes(5, 3), Some(vec![15, 6, 6]));
        assert_eq!(truncate("héllo", 2), "h");
    }
}
