//! `coreav ffmpeg` command lines that encode the video with `libx264`,
//! run through `coreav transcode`'s pipeline
//! (`docs/design/transcode.md`, "Through `coreav ffmpeg`"): the encoder
//! options read as `ffmpeg` reads them, and the refusals where `ffmpeg`'s
//! defaults would give another output than the pipeline's.

use std::path::{Path, PathBuf};

use coreav_frame::Rect;
use coreav_isobmff::{BufferedSource, FileSource, Rational};
use coreav_thumbnail::ThumbnailOptions;
use coreav_transcode::PixelFormat;
use coreav_transcode::aac;
use coreav_transcode::encoder::{MAX_BITRATE, MAX_THREADS, PRESETS};
use coreav_worker::opus::{MAX_BITRATE as MAX_OPUS_BITRATE, MIN_BITRATE};

use super::args::{AudioEncoder, Codec, Command, Encoding};
use super::{NotNative, Refusal, Stop, TranscodeJob};
use crate::error::Failure;
use crate::fallback::Reason;
use crate::output::{self, WriteFailure};
use crate::strict_safe::{Encoder, StrictSafe};
use crate::transcode::{self, Request};

/// Strict-safe mode's refusal (`crate::strict_safe`), as a command that is
/// not native: refused with exit status 4, and never handed to FFmpeg.
fn refused_strict(message: String) -> Refusal {
    Refusal::NotNative(NotNative {
        reason: Reason::Property("strict-safe mode"),
        message,
    })
}

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
    u32::try_from(bits(text)? / 1_000).ok()
}

/// `ffmpeg`'s number syntax ([`kbps`]) in bits a second, the fraction of a
/// bit dropped.
pub(super) fn bits(text: &str) -> Option<u128> {
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
    Some(digits.checked_mul(scale)? / ten)
}

/// Checks explicitly named encoders before reading the input.
pub fn preflight(command: &Command, strict_safe: StrictSafe) -> Result<(), Refusal> {
    if command.muxer == super::args::Muxer::Mp3 {
        authorize_audio(AudioEncoder::Libmp3lame, strict_safe)?;
    }
    for (_, codec) in &command.codecs {
        if let Codec::Audio(encoder) = codec {
            authorize_audio(*encoder, strict_safe)?;
        }
    }
    if command.codecs.iter().any(|(_, c)| *c == Codec::Ffv1)
        || !command.codecs.iter().any(|(_, c)| *c == Codec::Libx264)
    {
        return Ok(());
    }
    strict_safe
        .encoder("auto", None)
        .map(|_| ())
        .map_err(refused_strict)
}

fn check_aac(strict_safe: StrictSafe) -> Result<(), Refusal> {
    strict_safe
        .aac("use -c:a copy or libopus")
        .map_err(|message| Refusal::NotNative(NotNative {
            reason: Reason::Property(
                "encoding audio with aac in strict-safe mode (its encoder is the ffmpeg program)",
            ),
            message,
        }))
}

pub(super) fn authorize_audio(
    encoder: AudioEncoder,
    strict_safe: StrictSafe,
) -> Result<(), Refusal> {
    match encoder {
        AudioEncoder::Aac => check_aac(strict_safe),
        AudioEncoder::Libmp3lame => strict_safe.mp3().map_err(refused_strict),
        AudioEncoder::Libopus => strict_safe.sandbox().map_err(refused_strict),
    }
}

/// Checks the selected encoders, including the muxer's defaults, before decoding.
pub fn encoder(job: &TranscodeJob, strict_safe: StrictSafe) -> Result<Encoder, Refusal> {
    if let Some(audio) = job.audio_encoder {
        authorize_audio(audio, strict_safe)?;
    }
    strict_safe.encoder("auto", None).map_err(refused_strict)
}

/// The request for the pipeline from the command's encoder options, its
/// `encoder` and the audio streams it copies, or the one it encodes with
/// libopus or aac (`audio_encoder`).
pub fn request(
    command: &Command,
    job: &TranscodeJob,
    encoder: Encoder,
    strict_safe: StrictSafe,
) -> Result<Request, Refusal> {
    let TranscodeJob {
        audio,
        audio_encoder,
        left_out_covers,
        ..
    } = job;
    let Encoding {
        crf,
        bitrate,
        preset,
        threads,
        filter,
        size,
        pixel_format,
        passthrough: _,
        audio_bitrate,
        audio_quality,
        video_quality: _,
    } = &command.encoding;
    if audio_quality.is_some() {
        return Err(Refusal::not_native(
            "audio quality options beside video encoding",
        ));
    }
    let (opus, aac) = match (audio_encoder, audio.as_slice()) {
        (None, _) => (None, None),
        (Some(encoder), [stream]) => {
            let stream = Some(*stream);
            let (range, why) = match encoder {
                AudioEncoder::Libopus => (
                    MIN_BITRATE..=MAX_OPUS_BITRATE,
                    "the Opus bit rate must be from 6k to 510k bits a second",
                ),
                AudioEncoder::Aac => (
                    aac::MIN_BITRATE..=aac::MAX_BITRATE,
                    "the AAC bit rate must be from 16k to 512k bits a second",
                ),
                AudioEncoder::Libmp3lame => {
                    return Err(Refusal::not_native("MP3 encoding beside video"));
                }
            };
            let bitrate = match audio_bitrate {
                None => None,
                Some(text) => match bits(text).and_then(|b| u32::try_from(b).ok()) {
                    Some(b) if range.contains(&b) => Some(b),
                    _ => return Err(refused("-b:a", text, why)),
                },
            };
            match encoder {
                AudioEncoder::Libopus => (Some(transcode::OpusRequest { stream, bitrate }), None),
                AudioEncoder::Aac => (None, Some(transcode::AacRequest { stream, bitrate })),
                AudioEncoder::Libmp3lame => {
                    return Err(Refusal::not_native("MP3 encoding beside video"));
                }
            }
        }
        (Some(encoder), _) => {
            return Err(Refusal::not_native(match encoder {
                AudioEncoder::Libopus => "encoding other than one audio stream with libopus",
                AudioEncoder::Aac => "encoding other than one audio stream with aac",
                AudioEncoder::Libmp3lame => "MP3 encoding beside video",
            }));
        }
    };
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
        Some(p) => {
            return Err(refused(
                "-preset",
                p,
                "a preset outside the native preset names",
            ));
        }
    };
    let threads = match threads.as_deref() {
        None | Some("auto" | "0") => None,
        Some(text) => match text.parse::<u32>() {
            Ok(n) if n <= MAX_THREADS => Some(n),
            _ => return Err(refused("-threads", text, "at most 128 encoder threads")),
        },
    };
    let mut crop = None;
    let scale = match (filter, size) {
        (Some(_), Some(s)) => {
            return Err(refused("-s", s, "a size beside -vf is not native"));
        }
        (Some(f), None) => {
            if let Some(rect) = f.strip_prefix("crop=") {
                crop = Some(literal_crop(rect).ok_or_else(|| {
                    refused(
                        "-vf",
                        f,
                        "of the crops only crop=W:H:X:Y in even pixels is native",
                    )
                })?);
                None
            } else {
                let parsed = f
                    .strip_prefix("scale=")
                    .and_then(|wh| transcode::scale(wh).ok());
                Some(parsed.ok_or_else(|| {
                    refused(
                        "-vf",
                        f,
                        "of the filters only one scale=W:H or crop=W:H:X:Y is native",
                    )
                })?)
            }
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
            let parsed =
                parsed.ok_or_else(|| refused("-s", s, "the size must be WxH in pixels"))?;
            if command.muxer == super::args::Muxer::Mp4
                && !command
                    .codecs
                    .iter()
                    .any(|(_, codec)| *codec == Codec::Libx264)
            {
                None
            } else {
                Some(parsed)
            }
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
        matroska_output: command.muxer.matroska(),
        crf,
        bitrate,
        preset,
        scale,
        crop,
        no_audio: audio.is_empty(),
        audio_streams: Some(audio.clone()),
        left_out_covers: left_out_covers.clone(),
        opus,
        aac,
        aac_mp4_tail: true,
        copied_audio_bit_rate: audio_encoder
            .is_none()
            .then(|| {
                audio_bitrate
                    .as_deref()
                    .and_then(bits)
                    .and_then(|b| u32::try_from(b).ok())
            })
            .flatten(),
        #[cfg(feature = "input-capture")]
        input_capture: None,
        pixel_format,
        one_size: true,
        encoder,
        x264: None,
        ffmpeg: None,
        threads,
        timeout: super::ENCODE_LIMIT.as_secs(),
        strict_safe,
    })
}

fn literal_crop(text: &str) -> Option<Rect> {
    let mut fields = text.split(':');
    let mut next = || {
        let field = fields.next()?;
        if field.is_empty() || field.len() > 5 || !field.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let value = field.parse::<u32>().ok()?;
        (value % 2 == 0).then_some(value)
    };
    let (width, height, x, y) = (next()?, next()?, next()?, next()?);
    if fields.next().is_some()
        || !(2..=coreav_transcode::encoder::MAX_SIZE).contains(&width)
        || !(2..=coreav_transcode::encoder::MAX_SIZE).contains(&height)
    {
        return None;
    }
    Some(Rect {
        x,
        y,
        width,
        height,
    })
}

const NOT_COPIED: &str = "copying audio the MP4 writer does not take (AAC, MP3 or Opus as MP4 \
     carries them)";

/// The track matrix of an MP4 track that leaves the frames as they are.
const IDENTITY: [i32; 9] = [0x1_0000, 0, 0, 0, 0x1_0000, 0, 0, 0, 0x4000_0000];

/// The refusals read from the `container` of the input, whose `video` is
/// encoded. Copied `audio` the MP4 writer does not take is refused before
/// the encode, not after it: each copied stream must be AAC or MP3 in an
/// `mp4a` sample entry or Opus in an `Opus` one from MP4/MOV, AAC or Opus
/// from Matroska (`coreav-isobmff-write`'s `remux` and `matroska`); the
/// other audio the writer copies (FLAC, ALAC, AC-3, E-AC-3) is not taken
/// next to an encoded video yet. Matroska output skips that MP4 list: the
/// adapter's mapping check in [`check_source`] refuses what the writer
/// cannot map, with the writer's error, before the encoder starts. The
/// display transform is read whole: without
/// `-noautorotate` `ffmpeg` applies any transform other than the identity
/// to the frames, and with it `ffmpeg` writes the source's matrix, which
/// the transcode writes the same only when the matrix has no translation or
/// projection. A Matroska projection or rotation is refused.
pub(super) fn check_container(
    command: &Command,
    container: coreav_thumbnail::Container,
    video: usize,
    audio: &[usize],
    input: &Path,
) -> Result<(), Stop> {
    let file = FileSource::open(input).map_err(WriteFailure::from)?;
    let refused = |why: &'static str| Err(Refusal::not_native(why).into());
    match container {
        coreav_thumbnail::Container::Mp4 => {
            let Ok(movie) = coreav_isobmff::probe(
                BufferedSource::new(file),
                &coreav_isobmff::Limits::default(),
                &coreav_isobmff::Allowlist::default(),
            ) else {
                return refused("a source whose track matrix could not be read");
            };
            let copied = command.muxer.matroska().is_some()
                || audio.iter().all(|i| {
                    movie.tracks.iter().any(|t| {
                        t.stream_index == *i
                            && matches!(
                                (t.description.codec, &t.description.fourcc.0),
                                (
                                    coreav_isobmff::Codec::Aac | coreav_isobmff::Codec::Mp3,
                                    b"mp4a"
                                ) | (coreav_isobmff::Codec::Opus, b"Opus")
                            )
                    })
                });
            if !copied {
                return refused(NOT_COPIED);
            }
            let Some(track) = movie.tracks.iter().find(|t| t.stream_index == video) else {
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
            let copied = command.muxer.matroska().is_some()
                || audio.iter().all(|i| {
                    segment.tracks.iter().any(|t| {
                        t.stream_index == *i
                            && matches!(t.codec, coreav_mkv::Codec::Aac | coreav_mkv::Codec::Opus)
                    })
                });
            if !copied {
                return refused(NOT_COPIED);
            }
            let turned = segment
                .tracks
                .iter()
                .find(|t| t.stream_index == video)
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
/// encode; the copied audio the MP4 writer would fail on after the encode;
/// and, for Matroska output, copied audio the writer cannot map.
fn check_source(
    command: &Command,
    video: usize,
    audio: &[usize],
    reordered: bool,
    input: &Path,
) -> Result<(), Stop> {
    // The Matroska writer refuses an unmappable copied audio description
    // only while writing, which is after the video encoder has run. Ask it
    // first, and keep that refusal: the same error, before any encoder.
    if command.muxer.matroska().is_some() {
        let file = FileSource::open(input).map_err(WriteFailure::from)?;
        if let Err(error) =
            coreav_isobmff_write::mkv_output::copied_audio(&mut BufferedSource::new(file), audio)
        {
            return Err(Stop::from(WriteFailure::Rejected(Failure::Matroska(error))));
        }
    }
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
    check_container(command, stream.container, video, audio, input)?;
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

/// Encodes `video` and copies `audio` into the command's MP4 output, or
/// encodes the one audio stream with libopus or aac (`audio_encoder`).
pub fn write(
    command: &Command,
    job: &TranscodeJob,
    encoder: Encoder,
    strict_safe: StrictSafe,
) -> Result<serde_json::Value, Stop> {
    write_inner(command, job, encoder, strict_safe, None)
}

pub(super) fn write_inner(
    command: &Command,
    job: &TranscodeJob,
    encoder: Encoder,
    strict_safe: StrictSafe,
    capture: Option<&Path>,
) -> Result<serde_json::Value, Stop> {
    let request = request(command, job, encoder, strict_safe)?;
    #[cfg(feature = "input-capture")]
    let request = {
        let mut request = request;
        request.input_capture =
            capture.map(|path| coreav_transcode::capture::Target(path.to_owned()));
        request
    };
    #[cfg(not(feature = "input-capture"))]
    let _ = capture;
    let input = Path::new(&command.input);
    let copied = if job.audio_encoder.is_some() {
        &[][..]
    } else {
        &job.audio[..]
    };
    check_source(command, job.video, copied, job.reordered, input)?;
    let this = std::env::current_exe()
        .map_err(|_| Refusal::not_native("encoding without the path of the running program"))?;
    let path = PathBuf::from(&command.output);
    let force = super::check_output(command, &path)?;
    output::to_file(&path, force, |out| {
        let file = FileSource::open(input).map_err(WriteFailure::from)?;
        transcode::make(&request, Some(&this), BufferedSource::new(file), &path, out)
    })
    .map_err(|failure| failed(failure, job.audio_encoder))
}

pub(super) fn failed(failure: WriteFailure, audio_encoder: Option<AudioEncoder>) -> Stop {
    match failure {
        WriteFailure::Rejected(Failure::StrictSafe(message)) => {
            Stop::Refused(refused_strict(message))
        }
        WriteFailure::Rejected(Failure::Unsupported(what)) => {
            Stop::Refused(Refusal::NotNative(NotNative {
                reason: Reason::Property("a source the transcode pipeline does not take"),
                message: format!("not supported by coreav ffmpeg: {what}"),
            }))
        }
        WriteFailure::Rejected(Failure::Decode(object))
            if object.get("kind").and_then(serde_json::Value::as_str)
                == Some("unsupported_feature") =>
        {
            let message = object
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Stop::Refused(Refusal::NotNative(NotNative {
                reason: Reason::Property(match audio_encoder {
                    Some(AudioEncoder::Aac) => "an audio stream the AAC encode does not take",
                    Some(AudioEncoder::Libmp3lame) => {
                        "an audio stream the MP3 encode does not take"
                    }
                    _ => "an audio stream the Opus encode does not take",
                }),
                message: format!("not supported by coreav ffmpeg: the audio stream: {message}"),
            }))
        }
        WriteFailure::Rejected(Failure::VideoDecode(e))
            if matches!(
                e.kind,
                coreav_decode::ErrorKind::Unsupported | coreav_decode::ErrorKind::CodecNotAllowed
            ) =>
        {
            Stop::Refused(Refusal::NotNative(NotNative {
                reason: Reason::Property("a video stream no decoder of this build takes"),
                message: format!("not supported by coreav ffmpeg: decoding: {e}"),
            }))
        }
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use crate::test_isobmff::{aac_entry, audio_entry, av_movie_with};

    fn geometry(options: &[&str]) -> Result<Request, Refusal> {
        let mut line = vec!["-i", "in.mp4", "-an", "-c:v", "libx264"];
        line.extend_from_slice(options);
        line.push("out.mp4");
        let line: Vec<_> = line.into_iter().map(String::from).collect();
        let command = super::super::args::parse(&line).unwrap();
        let mut video = job(AudioEncoder::Aac);
        video.audio.clear();
        video.audio_encoder = None;
        request(
            &command,
            &video,
            StrictSafe::new(false).encoder("auto", None).unwrap(),
            StrictSafe::new(false),
        )
    }

    #[test]
    fn literal_crop_and_existing_scale_requests() {
        let crop = geometry(&["-vf", "crop=32:24:0:0"]).unwrap();
        assert_eq!(
            crop.crop,
            Some(Rect {
                x: 0,
                y: 0,
                width: 32,
                height: 24
            })
        );
        assert_eq!(crop.scale, None);
        for options in [vec!["-vf", "scale=32:24"], vec!["-s", "32x24"]] {
            let request = geometry(&options).unwrap();
            assert_eq!(request.crop, None);
            assert_eq!(request.scale, Some(transcode::scale("32:24").unwrap()));
        }
        for option in ["-vf", "-filter:v"] {
            for first in ["scale=16:16", "crop=16:16:0:0"] {
                let scaled = geometry(&[option, first, option, "scale=32:24"]).unwrap();
                assert_eq!(scaled.crop, None);
                assert_eq!(scaled.scale, Some(transcode::scale("32:24").unwrap()));
                let cropped = geometry(&[option, first, option, "crop=32:24:16:8"]).unwrap();
                assert_eq!(cropped.scale, None);
                assert_eq!(cropped.crop, literal_crop("32:24:16:8"));
            }
        }
    }

    fn crop_refused(value: &str) {
        match geometry(&["-vf", value]) {
            Err(Refusal::NotNative(refusal)) => {
                assert_eq!(
                    refusal.reason,
                    Reason::Option("-vf".into(), Some(value.into()))
                );
                assert_eq!(
                    refusal.message,
                    format!(
                        "'-vf {value}' is not supported: of the crops only crop=W:H:X:Y in even pixels is native"
                    )
                );
            }
            other => panic!("{value}: {other:?}"),
        }
    }

    #[test]
    fn crop_expressions_are_refused() {
        for value in [
            "crop=iw/2:24:0:0",
            "crop=32:ih/2:0:0",
            "crop=32:24:(iw-ow)/2:0",
            "crop=w=32:h=24:x=0:y=0",
        ] {
            crop_refused(value);
        }
    }

    #[test]
    fn crop_filter_chains_are_refused() {
        for value in [
            "crop=32:24:0:0,scale=16:16",
            "crop=32:24:0:0,hflip",
            "crop=32:24:0:0;crop=16:16:0:0",
        ] {
            crop_refused(value);
        }
        assert!(geometry(&["-vf", "scale=32:24,crop=16:16:0:0"]).is_err());
    }

    #[test]
    fn crop_beside_size_is_refused() {
        match geometry(&["-vf", "crop=32:24:0:0", "-s", "16x16"]) {
            Err(Refusal::NotNative(refusal)) => assert_eq!(
                refusal.message,
                "'-s 16x16' is not supported: a size beside -vf is not native"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn odd_crop_dimensions_are_refused() {
        for value in ["crop=31:24:0:0", "crop=32:23:0:0"] {
            crop_refused(value);
        }
    }

    #[test]
    fn odd_crop_offsets_are_refused() {
        for value in ["crop=32:24:1:0", "crop=32:24:0:1"] {
            crop_refused(value);
        }
    }

    #[test]
    fn crop_fields_are_literal_and_bounded() {
        for value in [
            "crop=32:24",
            "crop=32:24:0:0:0",
            "crop=0:24:0:0",
            "crop=32:0:0:0",
            "crop=-2:24:0:0",
            "crop=+32:24:0:0",
            "crop=32.0:24:0:0",
            "crop=16386:24:0:0",
            "crop=32:24:4294967294:0",
            "crop=32:24: 0:0",
            "crop=３２:24:0:0",
        ] {
            crop_refused(value);
        }
        assert!(literal_crop("16384:16384:99998:99998").is_some());
    }

    /// Copied MP4 audio other than AAC, MP3 or Opus is refused before the
    /// encode.
    #[test]
    fn copied_audio_the_mp4_writer_does_not_take_is_refused() {
        let dir = std::env::temp_dir().join(format!("coreav-encode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let args = ["-i", "in.mp4", "-c:v", "libx264", "-c:a", "copy", "out.mp4"].map(String::from);
        let command = super::super::args::parse(&args).unwrap();
        for (name, entry, taken) in [
            ("aac", aac_entry(), true),
            ("flac", audio_entry(b"fLaC"), false),
            ("alac", audio_entry(b"alac"), false),
            ("ac3", audio_entry(b"ac-3"), false),
            ("pcm", audio_entry(b"sowt"), false),
        ] {
            let path = dir.join(format!("{name}.mp4"));
            std::fs::write(&path, av_movie_with(entry, false)).unwrap();
            let checked =
                check_container(&command, coreav_thumbnail::Container::Mp4, 0, &[1], &path);
            match checked {
                Ok(()) => assert!(taken, "{name}: taken"),
                Err(Stop::Refused(Refusal::NotNative(refusal))) => {
                    assert!(!taken, "{name}: {}", refusal.message);
                    assert!(
                        refusal.message.contains(NOT_COPIED),
                        "{name}: {}",
                        refusal.message
                    );
                }
                Err(_) => panic!("{name}: neither taken nor refused"),
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Copied ISO PCM into Matroska is the writer's mapping refusal, raised
    /// from the preflight rather than after an encode.
    #[test]
    fn unmapped_copied_audio_is_refused_before_matroska_encode() {
        let dir = std::env::temp_dir().join(format!("coreav-encode-mkv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pcm.mp4");
        std::fs::write(&path, av_movie_with(audio_entry(b"sowt"), false)).unwrap();
        let args = ["-i", "in.mp4", "-c:v", "libx264", "-c:a", "copy", "out.mkv"].map(String::from);
        let command = super::super::args::parse(&args).unwrap();
        let checked = check_source(&command, 0, &[1], false, &path);
        match checked {
            Err(Stop::Refused(Refusal::NotNative(refusal))) => {
                assert!(
                    refusal
                        .message
                        .contains("an ISO codec without a Matroska mapping"),
                    "{}",
                    refusal.message
                );
            }
            Ok(()) => panic!("expected a refusal before encode"),
            Err(_) => panic!("expected the writer's mapping refusal"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn job(audio_encoder: AudioEncoder) -> TranscodeJob {
        TranscodeJob {
            video: 0,
            audio: vec![1],
            audio_encoder: Some(audio_encoder),
            reordered: false,
            left_out_covers: Vec::new(),
        }
    }

    /// `-b:a` with `-c:a aac`: the AAC request's bit rate, 16k to 512k
    /// bits a second; the encoder's default without it.
    #[test]
    fn aac_lines_request_aac_at_their_bit_rate() {
        let encoder = StrictSafe::new(false).encoder("auto", None).unwrap();
        let request = |rate: Option<&str>, audio_encoder| {
            let mut line = vec!["-i", "in.mkv", "-c:v", "libx264", "-c:a", "aac"];
            line.extend(rate.iter().flat_map(|r| ["-b:a", *r]));
            line.push("out.mp4");
            let line: Vec<String> = line.into_iter().map(String::from).collect();
            let command = super::super::args::parse(&line).unwrap();
            super::request(
                &command,
                &job(audio_encoder),
                encoder,
                StrictSafe::new(false),
            )
        };
        for (rate, bits) in [
            (None, None),
            (Some("96k"), Some(96_000)),
            (Some("16k"), Some(16_000)),
            (Some("512k"), Some(512_000)),
            (Some("128000"), Some(128_000)),
        ] {
            let made = request(rate, AudioEncoder::Aac).unwrap();
            assert_eq!(
                made.aac,
                Some(transcode::AacRequest {
                    stream: Some(1),
                    bitrate: bits
                }),
                "{rate:?}"
            );
            assert_eq!(made.opus, None, "{rate:?}");
        }
        for rate in ["8k", "15999", "513k", "1M", "fast"] {
            match request(Some(rate), AudioEncoder::Aac) {
                Err(Refusal::NotNative(refusal)) => {
                    assert_eq!(
                        refusal.reason,
                        Reason::Option("-b:a".to_owned(), Some(rate.to_owned()))
                    );
                    assert!(refusal.message.contains("16k to 512k"), "{rate}");
                }
                other => panic!("{rate}: {other:?}"),
            }
        }
        // libopus keeps its own range.
        assert!(request(Some("4k"), AudioEncoder::Libopus).is_err());
        assert_eq!(
            request(Some("6k"), AudioEncoder::Libopus).unwrap().opus,
            Some(transcode::OpusRequest {
                stream: Some(1),
                bitrate: Some(6_000)
            })
        );
    }

    /// Strict-safe mode refuses every `-c:a aac` line before the input is
    /// read: its encoder is the `ffmpeg` program.
    #[test]
    fn strict_safe_mode_refuses_aac_lines() {
        for line in [
            "-i in.mkv -c:v libx264 -c:a aac out.mp4",
            "-i in.mkv -c:v copy -acodec aac out.mp4",
        ] {
            let line: Vec<String> = line.split(' ').map(String::from).collect();
            let command = super::super::args::parse(&line).unwrap();
            match preflight(&command, StrictSafe::new(true)) {
                Err(Refusal::NotNative(refusal)) => {
                    assert_eq!(
                        refusal.reason,
                        Reason::Property(
                            "encoding audio with aac in strict-safe mode (its encoder is the ffmpeg program)"
                        )
                    );
                    assert!(
                        refusal.message.contains("use -c:a copy or libopus"),
                        "{}",
                        refusal.message
                    );
                }
                other => panic!("{line:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn strict_safe_mode_checks_resolved_aac_before_selecting_video_encoder() {
        let refusal = encoder(&job(AudioEncoder::Aac), StrictSafe::new(true)).unwrap_err();
        assert!(
            matches!(refusal, Refusal::NotNative(refusal) if refusal.reason == Reason::Property(
                "encoding audio with aac in strict-safe mode (its encoder is the ffmpeg program)"
            ))
        );
    }

    #[test]
    fn all_audio_encoders_follow_the_strict_safe_policy() {
        for encoder in [
            AudioEncoder::Aac,
            AudioEncoder::Libmp3lame,
            AudioEncoder::Libopus,
        ] {
            assert!(authorize_audio(encoder, StrictSafe::new(false)).is_ok());
            assert_eq!(
                authorize_audio(encoder, StrictSafe::new(true)).is_ok(),
                encoder == AudioEncoder::Libopus && coreav_worker::enforces_memory_limit(),
                "{encoder:?}"
            );
        }
    }

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
