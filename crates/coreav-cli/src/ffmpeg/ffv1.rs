use std::path::Path;

use super::args::{Codec, Command, Encoding, FrameOptions, Muxer};
use super::{Job, Refusal};
use crate::ffprobe::model::Value;
use crate::ffprobe::{Report, Stream, StreamKind};

pub(super) fn plan(
    command: &Command,
    report: &Report,
    output: &[(usize, &Stream)],
    codecs: &[Option<Codec>],
    concat: bool,
    trims: bool,
) -> Result<Job, Refusal> {
    if command.muxer != Muxer::Matroska
        || !Path::new(&command.output)
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("mkv"))
    {
        return Err(Refusal::not_native(
            "FFV1 output requires a .mkv Matroska destination",
        ));
    }
    let [(video, stream)] = output else {
        return Err(Refusal::not_native(
            "FFV1 output requires exactly one video stream; exclude other streams",
        ));
    };
    if stream.kind != StreamKind::Video || stream.attached_pic || codecs != [Some(Codec::Ffv1)] {
        return Err(Refusal::not_native(
            "FFV1 output requires one non-attached video stream",
        ));
    }
    if concat || trims || command.frames != FrameOptions::default() {
        return Err(Refusal::not_native(
            "FFV1 with concatenation, trimming or frame selection",
        ));
    }
    if (Encoding {
        pixel_format: None,
        passthrough: false,
        ..command.encoding.clone()
    }) != Encoding::default()
    {
        return Err(Refusal::not_native(
            "FFV1 with rate, quality, preset, threading or transformation options",
        ));
    }
    if !matches!(
        command.encoding.pixel_format.as_deref(),
        None | Some("yuv420p" | "yuv422p" | "yuv444p")
    ) {
        return Err(Refusal::not_native(
            "FFV1 supports only unconverted 8-bit planar YUV 420, 422 or 444",
        ));
    }
    if !stream.entries.iter().any(|(key, value)| {
        *key == "field_order" && matches!(value, Value::Str(s) if s == "progressive")
    }) {
        return Err(Refusal::not_native(
            "FFV1 requires known progressive source video",
        ));
    }
    if !report.chapters.is_empty()
        || !stream.side_data.is_empty()
        || report.format_tags.iter().any(|(key, _)| {
            ![
                "major_brand",
                "minor_version",
                "compatible_brands",
                "encoder",
            ]
            .iter()
            .any(|allowed| key.eq_ignore_ascii_case(allowed))
        })
        || stream.tags.iter().any(|(key, value)| match key.as_str() {
            "language" => value != "und",
            "handler_name" => !["VideoHandler", "Core Media Video"].contains(&value.as_str()),
            "vendor_id" | "encoder" | "ENCODER" | "DURATION" | "duration" => false,
            _ => true,
        })
        || stream
            .disposition
            .iter()
            .any(|(key, value)| *key != "default" && matches!(value, Value::Int(n) if *n != 0))
    {
        return Err(Refusal::not_native(
            "FFV1 source metadata or dispositions the output cannot preserve",
        ));
    }
    Ok(Job::Ffv1 { video: *video })
}

#[cfg(feature = "transcode")]
pub(super) fn write(
    command: &Command,
    report: &Report,
    stream: usize,
    strict_safe: crate::strict_safe::StrictSafe,
) -> Result<serde_json::Value, super::Stop> {
    use crate::error::Failure;
    use crate::output::{self, WriteFailure};
    use coreav_frame::{Chroma, PixelFormat};
    use coreav_isobmff::{BufferedSource, FileSource};
    use coreav_transcode::ffv1::{self, Error};

    let input = Path::new(&command.input);
    let container = match super::format_name(report) {
        Some("mov,mp4,m4a,3gp,3g2,mj2") => coreav_thumbnail::Container::Mp4,
        _ => coreav_thumbnail::Container::Matroska,
    };
    super::encode::check_container(command, container, stream, &[], input)?;
    let path = Path::new(&command.output);
    let force = super::check_output(command, path)?;
    let this = std::env::current_exe().ok();
    let backends = crate::thumbnail::backends(this.as_deref(), 0, false, strict_safe);
    let mut options = job_options(stream);
    {
        let source = BufferedSource::new(FileSource::open(input).map_err(WriteFailure::from)?);
        let video = coreav_thumbnail::video_stream(source, &options.source)
            .map_err(crate::thumbnail::failure)?;
        if let Some(coreav_decode::CodecConfig {
            kind: coreav_decode::ConfigKind::Avcc,
            data,
        }) = &video.stream.config
            && crate::ffprobe::video::avc(data, false)
                .is_some_and(|info| info.coded_size.is_some() && info.h264_corner.is_none())
        {
            return Err(Refusal::not_native(
                "FFV1 from an H.264 picture cropped at its left or top edge",
            )
            .into());
        }
    }
    options.pixel_format = match command.encoding.pixel_format.as_deref() {
        Some("yuv420p") => Some(PixelFormat::YUV420P),
        Some("yuv422p") => Some(PixelFormat::Yuv {
            chroma: Chroma::Cs422,
            depth: 8,
        }),
        Some("yuv444p") => Some(PixelFormat::Yuv {
            chroma: Chroma::Cs444,
            depth: 8,
        }),
        _ => None,
    };
    let result = output::to_file(path, force, |out| {
        let source = BufferedSource::new(FileSource::open(input).map_err(WriteFailure::from)?);
        ffv1::transcode(source, &options, &backends.list(), path, out)
            .map(|s| serde_json::json!({"frames": s.frames, "bytes_written": s.bytes_written}))
            .map_err(|error| {
                let kind = error.kind();
                let failure = match error {
                    Error::Unsupported(why) => Failure::Unsupported(format!("FFV1: {why}")),
                    Error::Decode(e) => Failure::VideoDecode(e),
                    Error::Source(e) => return crate::thumbnail::failure(e),
                    Error::Matroska(e) => Failure::Matroska(e),
                    Error::Io(kind) => Failure::Io(kind),
                    e => Failure::Encode(coreav_decode::Error::new(kind, e.to_string())),
                };
                backends.explain(WriteFailure::Rejected(failure))
            })
    });
    result.map_err(|failure| super::encode::failed(failure, None))
}

#[cfg(feature = "transcode")]
fn job_options(stream: usize) -> coreav_transcode::ffv1::Options {
    let mut options = coreav_transcode::ffv1::Options {
        progressive: true,
        ..Default::default()
    };
    options.source.stream = Some(stream);
    options.source.decode_limits.timeout = super::ENCODE_LIMIT;
    options
}

#[cfg(test)]
mod tests {
    use super::super::args;
    use super::*;

    fn report() -> Report {
        Report {
            format: vec![("format_name", Value::Str("mov,mp4,m4a,3gp,3g2,mj2".into()))],
            streams: vec![Stream {
                kind: StreamKind::Video,
                attached_pic: false,
                entries: vec![("field_order", Value::Str("progressive".into()))],
                disposition: Vec::new(),
                tags: Vec::new(),
                side_data: Vec::new(),
            }],
            ..Report::default()
        }
    }

    #[test]
    fn resolved_codec_and_native_subset_own_dispatch() {
        let report = report();
        let line = |extra: &str| {
            args::parse(
                &format!("-i in.mp4 -c:v ffv1 {extra} out.mkv")
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            )
        };
        assert!(matches!(
            super::super::plan(&line("").unwrap(), &report, None),
            Ok(Job::Ffv1 { video: 0 })
        ));
        for extra in [
            "-crf 0",
            "-b:v 1M",
            "-preset fast",
            "-threads 1",
            "-vf scale=2:2",
            "-s 2x2",
            "-pix_fmt rgb24",
            "-frames:v 1",
            "-r 25",
            "-ss 0",
        ] {
            assert!(
                line(extra)
                    .and_then(|command| super::super::plan(&command, &report, None))
                    .is_err(),
                "{extra}"
            );
        }
        assert!(!matches!(
            super::super::plan(&line("-c:v copy").unwrap(), &report, None),
            Ok(Job::Ffv1 { .. })
        ));
        let command = line("-pix_fmt yuv422p -fps_mode passthrough").unwrap();
        assert!(matches!(
            super::super::plan(&command, &report, None),
            Ok(Job::Ffv1 { .. })
        ));
    }

    #[cfg(feature = "transcode")]
    #[test]
    fn the_encode_has_the_same_command_limit_as_other_encodes() {
        let options = job_options(3);
        assert!(options.progressive);
        assert_eq!(options.source.stream, Some(3));
        assert_eq!(
            options.source.decode_limits.timeout,
            super::super::ENCODE_LIMIT
        );
        assert!(super::super::ENCODE_LIMIT > coreav_decode::Limits::default().timeout);
    }
}
