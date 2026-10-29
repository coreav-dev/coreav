//! `coreav ffmpeg` command lines that encode the video with `libx264`,
//! run through `coreav transcode`'s pipeline
//! (`docs/design/transcode.md`, "Through `coreav ffmpeg`"): the encoder
//! options read as `ffmpeg` reads them, and the refusals where `ffmpeg`'s
//! defaults would give another output than the pipeline's.

use std::path::{Path, PathBuf};

use coreav_isobmff::{BufferedSource, FileSource, Rational};
use coreav_thumbnail::ThumbnailOptions;
use coreav_transcode::PixelFormat;
use coreav_transcode::encoder::{MAX_BITRATE, MAX_THREADS, PRESETS};

use super::args::{Command, Encoding};
use super::{NotNative, Refusal, Stop};
use crate::error::Failure;
use crate::fallback::Reason;
use crate::output::{self, WriteFailure};
use crate::transcode::{self, Request};

fn refused(option: &str, value: &str, why: &str) -> Refusal {
    Refusal::NotNative(NotNative {
        reason: Reason::Option(option.to_owned(), Some(value.to_owned())),
        message: format!("'{option} {value}' is not supported: {why}"),
    })
}

/// `-b:v`: `ffmpeg`'s number syntax (digits, a fraction, an SI suffix `k`,
/// `M` or `G`) in bits a second, as the kbit/s libx264 receives: divided by
/// 1,000, the remainder dropped.
fn kbps(text: &str) -> Option<u32> {
    let (number, scale) = match text.char_indices().last()? {
        (i, 'k' | 'K') => (text.get(..i)?, 1_000u128),
        (i, 'M') => (text.get(..i)?, 1_000_000),
        (i, 'G') => (text.get(..i)?, 1_000_000_000),
        _ => (text, 1),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let digits: u128 = format!("{whole}{fraction}").parse().ok()?;
    let ten = 10u128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let bits = digits.checked_mul(scale)? / ten;
    u32::try_from(bits / 1_000).ok()
}

/// The request for the pipeline from the command's encoder options and the
/// audio streams it copies.
pub fn request(command: &Command, audio: &[usize]) -> Result<Request, Refusal> {
    let Encoding {
        crf,
        bitrate,
        preset,
        threads,
        filter,
        size,
        pixel_format,
        passthrough: _,
    } = &command.encoding;
    if let (Some(_), Some(rate)) = (crf, bitrate) {
        return Err(refused(
            "-b:v",
            rate,
            "a bit rate beside -crf is not native",
        ));
    }
    let crf = match crf {
        None => None,
        Some(text) => match text.parse::<f32>() {
            Ok(q) if (0.0..=51.0).contains(&q) => Some(q),
            _ => return Err(refused("-crf", text, "CRF is a number from 0 to 51")),
        },
    };
    let bitrate = match bitrate {
        None => None,
        Some(text) => match kbps(text) {
            Some(k) if (1..=MAX_BITRATE).contains(&k) => Some(k),
            _ => {
                return Err(refused(
                    "-b:v",
                    text,
                    "the bit rate must be from 1k to 2000M bits a second",
                ));
            }
        },
    };
    let preset = match preset {
        None => "medium".to_owned(),
        Some(p) if PRESETS.contains(&p.as_str()) => p.clone(),
        Some(p) => return Err(Refusal::Usage(format!("Invalid preset '{p}' for libx264."))),
    };
    let threads = match threads.as_deref() {
        None | Some("auto" | "0") => None,
        Some(text) => match text.parse::<u32>() {
            Ok(n) if n <= MAX_THREADS => Some(n),
            _ => return Err(refused("-threads", text, "at most 128 encoder threads")),
        },
    };
    let scale =
        match (filter, size) {
            (Some(_), Some(s)) => {
                return Err(refused("-s", s, "a size beside -vf is not native"));
            }
            (Some(f), None) => {
                let parsed = f
                    .strip_prefix("scale=")
                    .and_then(|wh| transcode::scale(wh).ok());
                Some(parsed.ok_or_else(|| {
                    refused("-vf", f, "of the filters only one scale=W:H is native")
                })?)
            }
            (None, Some(s)) => {
                let parsed = s
                    .split_once('x')
                    .filter(|(w, h)| {
                        [w, h]
                            .iter()
                            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                    })
                    .and_then(|(w, h)| transcode::scale(&format!("{w}:{h}")).ok());
                Some(parsed.ok_or_else(|| refused("-s", s, "the size must be WxH in pixels"))?)
            }
            (None, None) => None,
        };
    let pixel_format = match pixel_format.as_deref() {
        None => None,
        Some("yuv420p") => Some(PixelFormat::YUV420P),
        Some(other) => {
            return Err(refused(
                "-pix_fmt",
                other,
                "the frames are not converted; of the pixel formats only yuv420p is native",
            ));
        }
    };
    Ok(Request {
        crf,
        bitrate,
        preset,
        scale,
        no_audio: audio.is_empty(),
        audio_streams: Some(audio.to_vec()),
        pixel_format,
        one_size: true,
        encoder: "auto".to_owned(),
        x264: None,
        ffmpeg: None,
        threads,
        timeout: 3600,
    })
}

const NOT_COPIED: &str = "copying audio the MP4 writer does not take (AAC, MP3 or Opus as MP4 \
     carries them)";

/// The track matrix of an MP4 track that leaves the frames as they are.
const IDENTITY: [i32; 9] = [0x1_0000, 0, 0, 0, 0x1_0000, 0, 0, 0, 0x4000_0000];

/// The refusals read from the container. Copied `audio` the MP4 writer
/// does not take is refused before the encode, not after it: AAC or MP3 in
/// an `mp4a` sample entry and Opus in an `Opus` one from MP4/MOV, AAC
/// and Opus from Matroska (`coreav-isobmff-write`'s `remux` and
/// `matroska`). The display transform is read whole: without
/// `-noautorotate` `ffmpeg` applies any transform other than the identity
/// to the frames, and with it `ffmpeg` writes the source's matrix, which
/// the transcode writes the same only when the matrix has no translation or
/// projection. A Matroska projection or rotation is refused.
fn check_container(
    command: &Command,
    stream: &coreav_thumbnail::VideoStream,
    audio: &[usize],
    input: &Path,
) -> Result<(), Stop> {
    let file = FileSource::open(input).map_err(WriteFailure::from)?;
    let refused = |why: &'static str| Err(Refusal::not_native(why).into());
    match stream.container {
        coreav_thumbnail::Container::Mp4 => {
            let Ok(movie) = coreav_isobmff::probe(
                BufferedSource::new(file),
                &coreav_isobmff::Limits::default(),
                &coreav_isobmff::Allowlist::default(),
            ) else {
                return refused("a source whose track matrix could not be read");
            };
            let copied = movie
                .tracks
                .iter()
                .filter(|t| audio.contains(&t.stream_index))
                .all(|t| {
                    matches!(
                        (t.description.codec, &t.description.fourcc.0),
                        (
                            coreav_isobmff::Codec::Aac | coreav_isobmff::Codec::Mp3,
                            b"mp4a"
                        ) | (coreav_isobmff::Codec::Opus, b"Opus")
                    )
                });
            if !copied {
                return refused(NOT_COPIED);
            }
            let Some(track) = movie
                .tracks
                .iter()
                .find(|t| t.stream_index == stream.stream_index)
            else {
                return refused("a source whose track matrix could not be read");
            };
            let m = track.matrix;
            if m == IDENTITY {
                return Ok(());
            }
            if !command.noautorotate {
                return refused(
                    "a track matrix other than the identity, without -noautorotate \
                     (ffmpeg would apply it to the frames)",
                );
            }
            if [m[2], m[5], m[6], m[7]] != [0; 4] || m[8] != 0x4000_0000 {
                return refused(
                    "a track matrix with a translation or a projection, which the transcode does \
                     not write as ffmpeg does",
                );
            }
            Ok(())
        }
        coreav_thumbnail::Container::Matroska => {
            let Ok(segment) = coreav_mkv::probe(
                BufferedSource::new(file),
                &coreav_mkv::Limits::default(),
                &coreav_mkv::Allowlist::any(),
            ) else {
                return refused("a source whose projection could not be read");
            };
            let copied = segment
                .tracks
                .iter()
                .filter(|t| audio.contains(&t.stream_index))
                .all(|t| matches!(t.codec, coreav_mkv::Codec::Aac | coreav_mkv::Codec::Opus));
            if !copied {
                return refused(NOT_COPIED);
            }
            let turned = segment
                .tracks
                .iter()
                .find(|t| t.stream_index == stream.stream_index)
                .and_then(|t| t.video.as_ref())
                .is_none_or(|v| v.projection_type != 0 || v.pose != [0.0; 3]);
            if turned {
                return refused("a Matroska projection or rotation");
            }
            Ok(())
        }
    }
}

/// Whether `pts`, sorted, are evenly spaced: one interval above zero
/// between every two. A repeated time is not: `ffmpeg` cannot keep both
/// frames at a constant rate, where the pipeline moves the second one on.
fn evenly_spaced(mut pts: Vec<i64>) -> bool {
    pts.sort_unstable();
    let mut gaps = pts.windows(2).map(|w| match w {
        [a, b] => b.checked_sub(*a),
        _ => None,
    });
    let Some(first) = gaps.next() else {
        return true;
    };
    first.is_some_and(|g| g > 0) && gaps.all(|g| g == first)
}

/// The refusals where `ffmpeg`'s defaults would change the output: a
/// rotated source without `-noautorotate`, an H.264 picture cropped at its
/// left or top edge (the pinned `ffmpeg` decodes a left crop that is not
/// aligned uncropped, `docs/design/transcode.md`, B9), a source not evenly
/// spaced or whose reordered frames have no composition offsets, without
/// `-fps_mode passthrough`, and a video stream the pipeline would not
/// encode; and the copied audio the MP4 writer would fail on after the
/// encode.
fn check_source(
    command: &Command,
    video: usize,
    audio: &[usize],
    reordered: bool,
    input: &Path,
) -> Result<(), Stop> {
    let file = FileSource::open(input).map_err(WriteFailure::from)?;
    let Ok(stream) = coreav_thumbnail::video_stream(
        BufferedSource::new(file),
        &ThumbnailOptions::at(Rational::from_int(0)),
    ) else {
        // The pipeline reads the stream the same way, and reports its error.
        return Ok(());
    };
    if stream.stream_index != video {
        return Err(Refusal::not_native(
            "encoding another video stream than the one with the most pixels",
        )
        .into());
    }
    check_container(command, &stream, audio, input)?;
    let corner_kept = match &stream.stream.config {
        Some(coreav_decode::CodecConfig {
            kind: coreav_decode::ConfigKind::Avcc,
            data,
        }) => crate::ffprobe::video::avc(data, false)
            .is_none_or(|info| info.coded_size.is_none() || info.h264_corner.is_some()),
        _ => true,
    };
    if !corner_kept {
        return Err(Refusal::not_native(
            "an H.264 picture cropped at its left or top edge \
             (ffmpeg's default decode does not apply every such crop)",
        )
        .into());
    }
    if reordered
        && !command.encoding.passthrough
        && stream.packets.iter().all(|p| p.dts == Some(p.pts))
    {
        return Err(Refusal::not_native(
            "reordered frames without composition offsets, without -fps_mode passthrough \
             (ffmpeg would take other times for them)",
        )
        .into());
    }
    let presented = stream
        .packets
        .iter()
        .filter(|p| p.presented)
        .map(|p| p.pts)
        .collect();
    if !command.encoding.passthrough && !evenly_spaced(presented) {
        return Err(Refusal::not_native(
            "a source whose frames are not evenly spaced, without -fps_mode passthrough \
             (ffmpeg would duplicate or drop frames)",
        )
        .into());
    }
    Ok(())
}

/// Encodes `video` and copies `audio` into the command's MP4 output.
pub fn write(
    command: &Command,
    video: usize,
    audio: &[usize],
    reordered: bool,
) -> Result<serde_json::Value, Stop> {
    let request = request(command, audio)?;
    let input = Path::new(&command.input);
    check_source(command, video, audio, reordered, input)?;
    let this = std::env::current_exe()
        .map_err(|_| Refusal::not_native("encoding without the path of the running program"))?;
    let path = PathBuf::from(&command.output);
    let force = super::check_output(command, &path)?;
    output::to_file(&path, force, |out| {
        let file = FileSource::open(input).map_err(WriteFailure::from)?;
        transcode::make(&request, Some(&this), BufferedSource::new(file), &path, out)
    })
    .map_err(|failure| match failure {
        WriteFailure::Rejected(Failure::Unsupported(what)) => {
            Stop::Refused(Refusal::NotNative(NotNative {
                reason: Reason::Property("a source the transcode pipeline does not take"),
                message: format!("not supported by coreav ffmpeg: {what}"),
            }))
        }
        other => other.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_rates_are_read_as_libx264_receives_them() {
        for (text, k) in [
            ("1M", Some(1_000)),
            ("1500k", Some(1_500)),
            ("1.5M", Some(1_500)),
            ("1234567", Some(1_234)),
            ("1001", Some(1)),
            ("999", Some(0)),
            ("2.5k", Some(2)),
            ("", None),
            ("k", None),
            ("1e6", None),
            ("-1M", None),
        ] {
            assert_eq!(kbps(text), k, "{text}");
        }
    }

    #[test]
    fn spacing_is_judged_on_sorted_times() {
        assert!(evenly_spaced(vec![0, 2, 1, 3]));
        assert!(evenly_spaced(vec![5]));
        assert!(!evenly_spaced(vec![0, 1, 3]));
    }

    #[test]
    fn repeated_times_are_not_evenly_spaced() {
        assert!(!evenly_spaced(vec![0, 0, 40, 80]));
        assert!(!evenly_spaced(vec![0, 40, 40, 80]));
        assert!(!evenly_spaced(vec![7, 7]));
    }
}
