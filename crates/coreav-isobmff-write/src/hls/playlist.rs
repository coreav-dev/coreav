//! Master and media playlists (RFC 8216), and the names of the resources
//! they refer to.

use super::Rendition;
use super::index::{HlsIndex, IndexTrack};
use core::fmt::Write;
use core::str::FromStr;
use coreav_isobmff::Rational;

/// `EXT-X-VERSION`: 7, for `EXT-X-MAP` in media playlists without
/// `EXT-X-I-FRAMES-ONLY` (RFC 8216 7, version 6) and the `SERVICE`
/// attribute values of version 7, which a reader may assume.
const VERSION: u32 = 7;
/// Audio group of the master playlist.
const AUDIO_GROUP: &str = "audio";

/// A resource of a packaged source, by its relative name: `master.m3u8`,
/// `video.m3u8` and `audio.m3u8`, `video-init.mp4` and `audio-init.mp4`,
/// and media segments `video-N.m4s` and `audio-N.m4s` counted from 0. The
/// playlists refer to each other and to the segments by these names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resource {
    /// The master playlist.
    Master,
    /// A rendition's media playlist.
    Playlist(Rendition),
    /// A rendition's init segment.
    Init(Rendition),
    /// A rendition's media segment.
    Segment(Rendition, u32),
}

impl Resource {
    /// The MIME type to serve the resource with.
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Master | Self::Playlist(_) => "application/vnd.apple.mpegurl",
            Self::Init(Rendition::Audio) | Self::Segment(Rendition::Audio, _) => "audio/mp4",
            Self::Init(Rendition::Video) | Self::Segment(Rendition::Video, _) => "video/mp4",
        }
    }
}

impl core::fmt::Display for Resource {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Master => f.write_str("master.m3u8"),
            Self::Playlist(r) => write!(f, "{}.m3u8", r.name()),
            Self::Init(r) => write!(f, "{}-init.mp4", r.name()),
            Self::Segment(r, n) => write!(f, "{}-{n}.m4s", r.name()),
        }
    }
}

impl FromStr for Resource {
    type Err = ();

    /// Parses a name as [`Display`](core::fmt::Display) writes it; segment
    /// numbers are decimal without leading zeros.
    fn from_str(name: &str) -> Result<Self, ()> {
        if name == "master.m3u8" {
            return Ok(Self::Master);
        }
        let (rendition, rest) = if let Some(rest) = name.strip_prefix("video") {
            (Rendition::Video, rest)
        } else if let Some(rest) = name.strip_prefix("audio") {
            (Rendition::Audio, rest)
        } else {
            return Err(());
        };
        match rest {
            ".m3u8" => Ok(Self::Playlist(rendition)),
            "-init.mp4" => Ok(Self::Init(rendition)),
            _ => {
                let digits = rest
                    .strip_prefix('-')
                    .and_then(|r| r.strip_suffix(".m4s"))
                    .ok_or(())?;
                let canonical = digits == "0" || !digits.starts_with('0');
                if digits.is_empty() || !canonical || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(());
                }
                Ok(Self::Segment(rendition, digits.parse().map_err(|_| ())?))
            }
        }
    }
}

impl HlsIndex {
    /// The master playlist: one variant stream, the video with the audio
    /// as an alternative rendition, or the audio alone.
    pub fn master_playlist(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "#EXTM3U\n#EXT-X-VERSION:{VERSION}");
        // RFC 8216 4.3.5.1: every segment decodes without the ones before.
        if self.tracks.iter().all(|t| t.independent) {
            out.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
        }
        let video = self.track(Rendition::Video);
        let audio = self.track(Rendition::Audio);
        if let (Some(_), Some(a)) = (video, audio) {
            // The ISO 639-2 code names the rendition. It is not a valid
            // RFC 5646 tag where a two-letter code exists, so LANGUAGE is
            // left out (RFC 8216 4.3.4.1 makes it optional).
            let name = core::str::from_utf8(&a.language).unwrap_or("und");
            let _ = write!(
                out,
                "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"{AUDIO_GROUP}\",NAME=\"{name}\",DEFAULT=YES,AUTOSELECT=YES,"
            );
            if a.channels > 0 {
                let _ = write!(out, "CHANNELS=\"{}\",", a.channels);
            }
            let _ = writeln!(out, "URI=\"{}\"", Resource::Playlist(Rendition::Audio));
        }
        let peak = self
            .tracks
            .iter()
            .fold(0u64, |sum, t| sum.saturating_add(t.peak_bit_rate));
        let average = self
            .tracks
            .iter()
            .fold(0u64, |sum, t| sum.saturating_add(t.average_bit_rate));
        let codecs: Vec<&str> = self.tracks.iter().map(|t| t.codecs.as_str()).collect();
        let _ = write!(
            out,
            "#EXT-X-STREAM-INF:BANDWIDTH={peak},AVERAGE-BANDWIDTH={average},CODECS=\"{}\"",
            codecs.join(",")
        );
        if let Some(v) = video {
            let (width, height) = resolution(v);
            let _ = write!(out, ",RESOLUTION={width}x{height}");
            if let Some(rate) = frame_rate(v) {
                let _ = write!(out, ",FRAME-RATE={rate}");
            }
            if audio.is_some() {
                let _ = write!(out, ",AUDIO=\"{AUDIO_GROUP}\"");
            }
        }
        let main = if video.is_some() {
            Rendition::Video
        } else {
            Rendition::Audio
        };
        let _ = writeln!(out, "\n{}", Resource::Playlist(main));
        out
    }

    /// The media playlist of `rendition`, `None` when the index does not
    /// have it.
    pub fn media_playlist(&self, rendition: Rendition) -> Option<String> {
        let track = self.track(rendition)?;
        let durations = extinf_micros(track)?;
        let target = target_of(&durations);
        let mut out = String::new();
        let _ = writeln!(
            out,
            "#EXTM3U\n#EXT-X-VERSION:{VERSION}\n#EXT-X-TARGETDURATION:{target}\n\
             #EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD",
        );
        if track.independent {
            out.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
        }
        let _ = writeln!(out, "#EXT-X-MAP:URI=\"{}\"", Resource::Init(rendition));
        for (n, duration) in durations.iter().enumerate() {
            let number = u32::try_from(n).ok()?;
            let _ = writeln!(
                out,
                "#EXTINF:{}.{:06},\n{}",
                duration / 1_000_000,
                duration % 1_000_000,
                Resource::Segment(rendition, number)
            );
        }
        out.push_str("#EXT-X-ENDLIST\n");
        Some(out)
    }
}

/// Each segment's EXTINF in microseconds: the difference of its start and
/// end, both rounded to the microsecond, so that the playlist's timeline
/// stays within half a microsecond of the segments' however many there are.
fn extinf_micros(track: &IndexTrack) -> Option<Vec<i128>> {
    let micros = |ticks: u64| -> Option<i128> {
        Some(
            Rational::new(
                i128::from(ticks).checked_mul(1_000_000)?,
                u64::from(track.timescale),
            )?
            .round(),
        )
    };
    let mut bounds: Vec<i128> = track
        .segments()
        .iter()
        .map(|s| micros(s.start))
        .collect::<Option<_>>()?;
    bounds.push(micros(track.end)?);
    Some(
        bounds
            .windows(2)
            .map(|w| match w {
                [a, b] => (b - a).max(0),
                _ => 0,
            })
            .collect(),
    )
}

/// `EXT-X-TARGETDURATION`: every EXTINF rounded to the nearest integer is
/// at most it (RFC 8216 4.3.3.1), and it is at least 1.
fn target_of(durations: &[i128]) -> i128 {
    durations
        .iter()
        .map(|d| (d + 500_000) / 1_000_000)
        .max()
        .unwrap_or(1)
        .max(1)
}

/// The target duration of a rendition's media playlist, in seconds.
pub(crate) fn target_duration(track: &IndexTrack) -> Option<i128> {
    extinf_micros(track).map(|d| target_of(&d))
}

/// The display size (pixel aspect ratio applied), swapped when the matrix
/// turns the picture by 90 or 270 degrees.
fn resolution(track: &IndexTrack) -> (u32, u32) {
    let (width, height) = (u32::from(track.width), u32::from(track.height));
    // (a, b, c, d) of the matrix: a quarter turn has a = d = 0.
    match track.matrix {
        [0, b, _, c, 0, ..] if b != 0 && c != 0 => (height, width),
        _ => (width, height),
    }
}

/// The maximum frame rate (RFC 8216 4.3.4.2): one over the shortest
/// presented sample duration, three decimals; none when the index has no
/// plausible one (the attribute is optional).
fn frame_rate(track: &IndexTrack) -> Option<String> {
    let rate = Rational::new(
        i128::from(track.timescale),
        u64::from(track.min_sample_duration).max(1),
    )?;
    (track.min_sample_duration > 0).then(|| decimal(rate, 3))
}

/// `value` rounded to `places` decimals, halves away from zero; `value` is
/// at least 0.
fn decimal(value: Rational, places: u32) -> String {
    let scale = 10i128.pow(places);
    let scaled = Rational::new(value.num.saturating_mul(scale), value.den)
        .map_or(0, Rational::round)
        .max(0);
    let whole = scaled / scale;
    let fraction = scaled % scale;
    format!("{whole}.{fraction:0width$}", width = places as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for resource in [
            Resource::Master,
            Resource::Playlist(Rendition::Video),
            Resource::Playlist(Rendition::Audio),
            Resource::Init(Rendition::Audio),
            Resource::Segment(Rendition::Video, 0),
            Resource::Segment(Rendition::Audio, 4_294_967_295),
        ] {
            assert_eq!(resource.to_string().parse::<Resource>(), Ok(resource));
        }
        for bad in [
            "",
            "video",
            "video-.m4s",
            "video-01.m4s",
            "video-+1.m4s",
            "video-4294967296.m4s",
            "subtitles.m3u8",
            "video-1.mp4",
            "../video.m3u8",
        ] {
            assert!(bad.parse::<Resource>().is_err(), "{bad}");
        }
    }

    #[test]
    fn decimals_round() {
        let q = |n, d| Rational::new(n, d).unwrap();
        assert_eq!(decimal(q(1001, 1000), 6), "1.001000");
        assert_eq!(decimal(q(2, 3), 6), "0.666667");
        assert_eq!(decimal(q(30_000, 1001), 3), "29.970");
        assert_eq!(decimal(q(0, 1), 3), "0.000");
    }
}
