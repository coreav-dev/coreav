//! Decoded video output, with image naming independent of the packet encoder.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use coreav_decode::{Frame, Packet};
use coreav_frame::{
    Chroma, ChromaLocation, ChromaUpsampling, ColorInfo, Filter, Picture, PixelFormat,
};
use coreav_isobmff::{BufferedSource, FileSource, Rational};
use coreav_thumbnail::{ThumbnailOptions, VideoStream};

use super::args::{Command, Muxer, VideoEncoder};
use super::{Refusal, Stop, check_output, clip_range};
use crate::error::Failure;
use crate::ffprobe::{Report, model::Value};
use crate::output::{self, WriteFailure};
use crate::split::Pattern;
use crate::strict_safe::StrictSafe;

fn unsupported(why: &'static str) -> Stop {
    Refusal::not_native(why).into()
}

fn image_error(error: coreav_thumbnail::Error) -> Stop {
    Stop::Failed(crate::thumbnail::failure(error))
}

fn source_error(failure: WriteFailure) -> Stop {
    let why = match &failure {
        WriteFailure::Rejected(Failure::Unsupported(why) | Failure::StrictSafe(why)) => {
            Some(why.clone())
        }
        WriteFailure::Rejected(Failure::VideoDecode(e))
            if matches!(
                e.kind,
                coreav_decode::ErrorKind::Unsupported | coreav_decode::ErrorKind::CodecNotAllowed
            ) =>
        {
            Some(e.to_string())
        }
        WriteFailure::Rejected(Failure::Library(coreav_isobmff::Error::Unsupported { reason }))
        | WriteFailure::Rejected(Failure::Matroska(coreav_mkv::Error::Unsupported { reason })) => {
            Some((*reason).to_owned())
        }
        _ => None,
    };
    match why {
        Some(why) => Refusal::NotNative(super::NotNative::property(
            "a source this frame output cannot decode",
            format!("not supported by coreav ffmpeg: {why}"),
        ))
        .into(),
        None => Stop::Failed(failure),
    }
}

fn frame_error(error: coreav_frame::FrameError) -> Stop {
    match error {
        coreav_frame::FrameError::Unsupported(why) => unsupported(why),
        error => decode_error(error.into()),
    }
}

fn decode_error(error: coreav_decode::Error) -> Stop {
    Stop::Failed(WriteFailure::Rejected(Failure::VideoDecode(error)))
}

fn checked<T>(value: Option<T>) -> Result<T, Stop> {
    value.ok_or({
        Stop::Failed(WriteFailure::Rejected(Failure::Invalid(
            "frame-output arithmetic outside its range",
        )))
    })
}

fn multiply(a: Rational, b: Rational) -> Result<Rational, Stop> {
    checked(Rational::new(
        checked(a.num.checked_mul(b.num))?,
        checked(a.den.checked_mul(b.den))?,
    ))
}

fn times_rate(video: &VideoStream) -> Result<Option<Rational>, Stop> {
    let pts: BTreeSet<_> = video
        .packets
        .iter()
        .filter(|p| p.presented)
        .map(|p| p.pts)
        .collect();
    let mut step = None;
    for (a, b) in pts.iter().zip(pts.iter().skip(1)) {
        let delta = checked(b.checked_sub(*a))?;
        if step.is_some_and(|s| s != delta) {
            return Ok(None);
        }
        step = Some(delta);
    }
    let ticks = step.or_else(|| {
        video
            .packets
            .iter()
            .find(|p| p.presented)
            .and_then(|p| p.duration)
            .and_then(|d| i64::try_from(d).ok())
    });
    let Some(ticks) = ticks.filter(|t| *t > 0) else {
        return Ok(None);
    };
    let duration = multiply(Rational::from_int(i128::from(ticks)), video.time_base)?;
    Ok(Rational::new(
        i128::from(duration.den),
        u64::try_from(duration.num).unwrap_or(0),
    ))
}

fn validate(command: &Command, video: &VideoStream) -> Result<Option<Rational>, Stop> {
    if command.frames.update && command.muxer != Muxer::Image2 {
        return Err(unsupported("-update on an output other than image2"));
    }
    if command.output != "-"
        && command.muxer == Muxer::Image2
        && command.output.contains('%')
        && Pattern::parse(&command.output).is_none()
        && !command.frames.update
    {
        return Err(unsupported(
            "an image name other than one %d or %0Nd pattern",
        ));
    }
    let rate = times_rate(video)?;
    if command.frames.rate.is_some() || command.muxer == Muxer::Yuv4mpeg {
        let source = rate.ok_or_else(|| {
            unsupported("frame-rate selection or YUV4MPEG for variable or unknown frame timing")
        })?;
        if let Some(wanted) = command.frames.rate {
            if checked(source.checked_lt(wanted))? {
                return Err(unsupported(
                    "an output frame rate that requires duplicated frames",
                ));
            }
        }
    }
    Ok(command.frames.rate.or(rate))
}

pub(super) fn write(
    command: &Command,
    stream: usize,
    encoder: VideoEncoder,
    report: &Report,
    strict_safe: StrictSafe,
    capture: Option<&Path>,
) -> Result<serde_json::Value, Stop> {
    #[cfg(not(feature = "input-capture"))]
    if capture.is_some() {
        return Err(unsupported("image capture is not enabled in this build"));
    }
    let mut source = BufferedSource::new(
        FileSource::open(Path::new(&command.input)).map_err(WriteFailure::from)?,
    );
    let mut options = ThumbnailOptions::at(Rational::from_int(0));
    options.stream = Some(stream);
    let video = coreav_thumbnail::video_stream(&mut source, &options)
        .map_err(|e| source_error(crate::thumbnail::failure(e)))?;
    if !video.warnings.is_empty() {
        return Err(unsupported("a video stream with ambiguous packet timing"));
    }
    let rate = validate(command, &video)?;
    if command.muxer == Muxer::Yuv4mpeg
        && report.streams.get(stream).is_none_or(|s| {
            !s.entries.iter().any(|(key, value)| {
                *key == "field_order" && matches!(value, Value::Str(s) if s == "progressive")
            })
        })
    {
        return Err(unsupported("YUV4MPEG without known progressive frames"));
    }
    let source_duration = times_rate(&video)?
        .and_then(|r| Rational::new(i128::from(r.den), u64::try_from(r.num).ok()?));
    let times: BTreeMap<_, _> = video
        .packets
        .iter()
        .filter(|p| p.presented)
        .map(|p| {
            p.time
                .map(|time| (p.pts, time))
                .ok_or_else(|| unsupported("a video without known presentation times"))
        })
        .collect::<Result<_, _>>()?;
    if command.frames.rate.is_some()
        && times
            .first_key_value()
            .is_some_and(|(_, time)| time.num != 0)
    {
        return Err(unsupported(
            "frame-rate selection with a delayed video start",
        ));
    }
    let (start, end) = checked(clip_range(command, Rational::from_int(0)))?;
    let this = std::env::current_exe().ok();
    let threads = command
        .encoding
        .threads
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let backends = crate::thumbnail::backends(this.as_deref(), threads, false, strict_safe);
    let opened = coreav_decode::open(
        &video.stream,
        &options.policy,
        &options.decode_limits,
        &backends.list(),
    )
    .map_err(|e| source_error(backends.explain(WriteFailure::Rejected(Failure::VideoDecode(e)))))?;
    let mut decoder = opened.decoder;
    let mut run = Run {
        command,
        stream: &video.stream,
        encoder,
        pixel_aspect: video.pixel_aspect,
        quarter_turns: video.quarter_turns,
        times,
        start,
        end,
        rate,
        source_duration,
        count: 0,
        last_time: None,
        shape: None,
        input_shape: None,
        continuous: None,
        #[cfg(feature = "input-capture")]
        capture: capture.map(super::image_capture::Recorder::new),
    };
    let mut pending = PacketQueue {
        sizes: VecDeque::new(),
        bytes: 0,
        budget: options.decode_limits.max_pending_bytes / 4,
    };
    let mut progress = Instant::now();
    for (index, packet) in video.packets.iter().enumerate() {
        if run.complete() {
            break;
        }
        while pending.full() && !run.complete() {
            match decoder.receive_frame().map_err(decode_error)? {
                Some(frame) => {
                    run.take(frame)?;
                    pending.retire();
                    progress = Instant::now();
                }
                None if progress.elapsed() >= Duration::from_secs(5) => {
                    pending.clear();
                    progress = Instant::now();
                }
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        if run.complete() {
            break;
        }
        let data = video
            .read_packet(&mut source, index, options.decode_limits.max_packet_bytes)
            .map_err(image_error)?;
        decoder
            .send_packet(&Packet {
                data: &data,
                pts: Some(packet.pts),
                dts: packet.dts,
                keyframe: packet.keyframe,
                discard: !packet.presented,
            })
            .map_err(decode_error)?;
        pending.sent(data.len());
        while let Some(frame) = decoder.receive_frame().map_err(decode_error)? {
            pending.retire();
            progress = Instant::now();
            run.take(frame)?;
            if run.complete() {
                break;
            }
        }
    }
    if !run.complete() {
        decoder.send_eof().map_err(decode_error)?;
        while let Some(frame) = decoder.receive_frame().map_err(decode_error)? {
            run.take(frame)?;
            if run.complete() {
                break;
            }
        }
    }
    drop(decoder);
    if command.muxer != Muxer::Image2 {
        if command.muxer == Muxer::Yuv4mpeg && run.count == 0 {
            return Err(unsupported("YUV4MPEG output without a selected frame"));
        }
        let output = match run.continuous.take() {
            Some(output) => output,
            None => Continuous::open(command)?,
        };
        output.finish()?;
    }
    let summary = serde_json::json!({"frames": run.count});
    #[cfg(feature = "input-capture")]
    if let Some(capture) = run.capture.take() {
        capture.finish()?;
        return Ok(serde_json::json!({"frames": run.count, "image_capture_stream": stream}));
    }
    Ok(summary)
}

struct Run<'a> {
    command: &'a Command,
    stream: &'a coreav_decode::StreamConfig,
    encoder: VideoEncoder,
    pixel_aspect: Option<(u64, u64)>,
    quarter_turns: u8,
    times: BTreeMap<i64, Rational>,
    start: Rational,
    end: Option<Rational>,
    rate: Option<Rational>,
    source_duration: Option<Rational>,
    count: u64,
    last_time: Option<Rational>,
    shape: Option<(u32, u32, PixelFormat)>,
    input_shape: Option<(u32, u32, PixelFormat)>,
    continuous: Option<Continuous>,
    #[cfg(feature = "input-capture")]
    capture: Option<super::image_capture::Recorder>,
}

impl Run<'_> {
    fn complete(&self) -> bool {
        self.command
            .frames
            .limit
            .is_some_and(|limit| self.count >= limit)
            || self
                .end
                .zip(self.last_time)
                .is_some_and(|(end, last)| last.checked_lt(end) == Some(false))
    }

    fn take(&mut self, frame: Frame) -> Result<(), Stop> {
        match self.take_frame(frame) {
            Err(Stop::Refused(Refusal::NotNative(reason))) if self.count > 0 => Err(decode_error(
                coreav_decode::Error::new(coreav_decode::ErrorKind::InvalidData, reason.message),
            )),
            result => result,
        }
    }

    fn take_frame(&mut self, mut frame: Frame) -> Result<(), Stop> {
        if self.complete() {
            return Ok(());
        }
        let pts = frame.pts.ok_or_else(|| {
            decode_error(coreav_decode::Error::new(
                coreav_decode::ErrorKind::InvalidData,
                "a decoded frame without a presentation timestamp",
            ))
        })?;
        let Some(&time) = self.times.get(&pts) else {
            return Ok(());
        };
        self.last_time = Some(time);
        if checked(time.checked_lt(self.start))?
            || self
                .end
                .is_some_and(|end| time.checked_lt(end) != Some(true))
        {
            return Ok(());
        }
        if let Some(rate) = self.command.frames.rate {
            let negative = checked(Rational::new(
                checked(self.start.num.checked_neg())?,
                self.start.den,
            ))?;
            let relative = checked(time.checked_add(negative))?;
            let position = multiply(relative, rate)?;
            let duration = self
                .source_duration
                .ok_or_else(|| unsupported("unknown frame rate"))?;
            let end = checked(position.checked_add(multiply(duration, rate)?))?;
            let delta = checked(end.checked_add(Rational::from_int(-i128::from(self.count))))?;
            if checked(delta.checked_lt(checked(Rational::new(-11, 10))?))? {
                return Ok(());
            }
        }
        let input_shape = (frame.width(), frame.height(), frame.format());
        if self.input_shape.is_some_and(|old| old != input_shape) {
            return Err(decode_error(coreav_decode::Error::new(
                coreav_decode::ErrorKind::InvalidData,
                "video dimensions or pixel format changed",
            )));
        }
        self.input_shape = Some(input_shape);
        frame.color =
            coreav_thumbnail::frame_color(self.stream.codec, frame.color, self.stream.color);
        let mut picture = frame.to_cpu().map_err(frame_error)?.into_owned();
        let turns = if self.command.noautorotate {
            0
        } else {
            self.quarter_turns
        };
        if turns != 0 && self.command.muxer == Muxer::Yuv4mpeg {
            return Err(unsupported(
                "YUV4MPEG autorotation without chroma resiting (use -noautorotate)",
            ));
        }
        let convert_first = self.encoder != VideoEncoder::Rawvideo
            && turns % 2 != 0
            && matches!(
                picture.format(),
                PixelFormat::Yuv {
                    chroma: Chroma::Cs422,
                    ..
                }
            );
        if turns != 0 && !convert_first {
            picture = picture.rotate(turns).map_err(frame_error)?;
        }
        let mut picture = match self.encoder {
            VideoEncoder::Png
                if matches!(
                    picture.format(),
                    PixelFormat::Rgb24
                        | PixelFormat::Rgba
                        | PixelFormat::Yuv {
                            chroma: Chroma::Mono,
                            depth: 8 | 16
                        }
                ) =>
            {
                picture
            }
            VideoEncoder::Png | VideoEncoder::Bmp => {
                if picture.format().depth() > 8 {
                    return Err(unsupported("high-bit-depth colour image conversion"));
                }
                picture
                    .resize_to_rgb(
                        picture.width(),
                        picture.height(),
                        Filter::Bicubic,
                        &frame.color,
                        ChromaUpsampling::FfmpegCompatible,
                    )
                    .map_err(frame_error)?
            }
            VideoEncoder::Rawvideo => picture,
        };
        if convert_first {
            picture = picture.rotate(turns).map_err(frame_error)?;
        }
        let name = match self.encoder {
            VideoEncoder::Bmp => "bgr24".to_owned(),
            VideoEncoder::Png
                if picture.format()
                    == (PixelFormat::Yuv {
                        chroma: Chroma::Mono,
                        depth: 16,
                    }) =>
            {
                "gray16be".to_owned()
            }
            _ => pixel_name(picture.format()),
        };
        if self
            .command
            .encoding
            .pixel_format
            .as_ref()
            .is_some_and(|s| *s != name)
        {
            return Err(unsupported("a pixel format that requires conversion"));
        }
        let shape = (picture.width(), picture.height(), picture.format());
        if self.shape.is_some_and(|old| old != shape) {
            return Err(Stop::Failed(WriteFailure::Rejected(Failure::Invalid(
                "video dimensions or pixel format changed",
            ))));
        }
        let header = if self.shape.is_none() && self.command.muxer == Muxer::Yuv4mpeg {
            Some(self.y4m(&picture, &frame.color)?)
        } else {
            None
        };
        self.shape = Some(shape);
        #[cfg(feature = "input-capture")]
        if let Some(capture) = &mut self.capture {
            capture.picture(&picture, pts, time, pixel_name(input_shape.2), &frame.color)?;
        }
        let bytes = match self.encoder {
            VideoEncoder::Png => coreav_thumbnail::encode_png_with_aspect(&picture, self.aspect())
                .map_err(image_error)?,
            VideoEncoder::Bmp => coreav_thumbnail::encode_bmp(&picture).map_err(image_error)?,
            VideoEncoder::Rawvideo => {
                let mut bytes = Vec::new();
                picture.write_packed(&mut bytes).map_err(frame_error)?;
                bytes
            }
        };
        if self.command.muxer == Muxer::Image2 {
            let path = if self.command.frames.update {
                PathBuf::from(&self.command.output)
            } else if let Some(pattern) = Pattern::parse(&self.command.output) {
                PathBuf::from(
                    pattern.name(
                        usize::try_from(self.count + 1)
                            .map_err(|_| unsupported("too many image files"))?,
                    ),
                )
            } else if self.count == 0 {
                PathBuf::from(&self.command.output)
            } else {
                return Err(Refusal::Usage(
                    "An image sequence needs a %d pattern, -update 1 or -frames:v 1.".into(),
                )
                .into());
            };
            let force = if self.command.frames.update && self.count != 0 {
                if let Some(why) = output::refuse(Path::new(&self.command.input), &path, true) {
                    return Err(Refusal::Usage(why).into());
                }
                true
            } else {
                check_output(self.command, &path)?
            };
            output::to_file(&path, force, |out| {
                out.write_all(&bytes).map_err(|e| WriteFailure::output(&e))
            })?;
            #[cfg(feature = "input-capture")]
            if let Some(capture) = &mut self.capture {
                capture.output(&path)?;
            }
        } else {
            if self.continuous.is_none() {
                self.continuous = Some(Continuous::open(self.command)?);
            }
            if let Some(output) = &mut self.continuous {
                if let Some(header) = header {
                    output.write(header.as_bytes())?;
                }
                if self.command.muxer == Muxer::Yuv4mpeg {
                    output.write(b"FRAME\n")?;
                }
                output.write(&bytes)?;
            }
        }
        self.count += 1;
        Ok(())
    }

    fn y4m(&self, picture: &Picture, color: &ColorInfo) -> Result<String, Stop> {
        let chroma = match picture.format() {
            PixelFormat::Yuv {
                chroma: Chroma::Cs420,
                depth: 8,
            } => match color.chroma_location {
                None | Some(ChromaLocation::Left) => "420mpeg2",
                Some(ChromaLocation::Center) => "420jpeg",
                _ => return Err(unsupported("YUV4MPEG with this chroma location")),
            },
            PixelFormat::Yuv {
                chroma: Chroma::Cs422,
                depth: 8,
            } => "422",
            PixelFormat::Yuv {
                chroma: Chroma::Cs444,
                depth: 8,
            } => "444",
            PixelFormat::Yuv {
                chroma: Chroma::Mono,
                depth: 8,
            } => "mono",
            _ => return Err(unsupported("a pixel format outside 8-bit planar YUV4MPEG")),
        };
        let rate = self
            .rate
            .ok_or_else(|| unsupported("YUV4MPEG without a known frame rate"))?;
        let (a, b) = self.aspect().unwrap_or((0, 0));
        let range = match color.full_range {
            Some(true) => " XCOLORRANGE=FULL",
            Some(false) => " XCOLORRANGE=LIMITED",
            None => "",
        };
        Ok(format!(
            "YUV4MPEG2 W{} H{} F{}:{} Ip A{a}:{b} C{chroma}{range}\n",
            picture.width(),
            picture.height(),
            rate.num,
            rate.den
        ))
    }

    fn aspect(&self) -> Option<(u64, u64)> {
        self.pixel_aspect.map(|(a, b)| {
            if !self.command.noautorotate && self.quarter_turns % 2 != 0 {
                (b, a)
            } else {
                (a, b)
            }
        })
    }
}

fn pixel_name(format: PixelFormat) -> String {
    match format {
        PixelFormat::Rgb24 => "rgb24".into(),
        PixelFormat::Rgba => "rgba".into(),
        PixelFormat::Yuv {
            chroma: Chroma::Mono,
            depth: 8,
        } => "gray".into(),
        PixelFormat::Yuv {
            chroma: Chroma::Mono,
            depth,
        } => format!("gray{depth}le"),
        PixelFormat::Yuv { chroma, depth } => {
            let base = match chroma {
                Chroma::Cs420 => "yuv420p",
                Chroma::Cs422 => "yuv422p",
                Chroma::Cs444 => "yuv444p",
                Chroma::Mono => "gray",
            };
            if depth == 8 {
                base.into()
            } else {
                format!("{base}{depth}le")
            }
        }
        PixelFormat::Gbr { depth: 8 } => "gbrp".into(),
        PixelFormat::Gbr { depth } => format!("gbrp{depth}le"),
        PixelFormat::SemiPlanar { depth: 8 } => "nv12".into(),
        PixelFormat::SemiPlanar { depth } => format!("p0{depth}le"),
    }
}

struct Continuous {
    writer: Option<Sink>,
    temporary: PathBuf,
    path: PathBuf,
    force: bool,
}

enum Sink {
    File(BufWriter<File>),
    Stdout(BufWriter<Box<dyn Write>>),
}

struct PacketQueue {
    sizes: VecDeque<usize>,
    bytes: u64,
    budget: u64,
}

impl PacketQueue {
    fn full(&self) -> bool {
        self.sizes.len() >= 64 || self.bytes >= self.budget
    }

    fn sent(&mut self, bytes: usize) {
        self.sizes.push_back(bytes);
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    }

    fn retire(&mut self) {
        if let Some(bytes) = self.sizes.pop_front() {
            self.bytes = self
                .bytes
                .saturating_sub(u64::try_from(bytes).unwrap_or(u64::MAX));
        }
    }

    fn clear(&mut self) {
        self.sizes.clear();
        self.bytes = 0;
    }
}

impl Continuous {
    fn open(command: &Command) -> Result<Self, Stop> {
        let path = PathBuf::from(&command.output);
        if command.output == "-" {
            return Ok(Self {
                writer: Some(Sink::Stdout(output::media_stdout())),
                temporary: PathBuf::new(),
                path,
                force: false,
            });
        }
        let force = check_output(command, &path)?;
        let temporary = output::temporary_path(&path);
        let file = File::options()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| WriteFailure::output(&e))?;
        Ok(Self {
            writer: Some(Sink::File(BufWriter::new(file))),
            temporary,
            path,
            force,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Stop> {
        match &mut self.writer {
            Some(Sink::File(file)) => file
                .write_all(bytes)
                .map_err(|e| Stop::Failed(WriteFailure::output(&e))),
            Some(Sink::Stdout(out)) => out
                .write_all(bytes)
                .map_err(|e| Stop::Failed(WriteFailure::Rejected(Failure::io(e)))),
            None => Err(Stop::Failed(WriteFailure::Rejected(Failure::Invalid(
                "output already closed",
            )))),
        }
    }

    fn finish(mut self) -> Result<(), Stop> {
        match self.writer.take() {
            Some(Sink::File(mut file)) => {
                file.flush().map_err(|e| WriteFailure::output(&e))?;
                drop(file);
                output::place(&self.temporary, &self.path, self.force)
                    .map_err(|e| WriteFailure::output(&e))?;
            }
            Some(Sink::Stdout(mut out)) => out
                .flush()
                .map_err(|e| WriteFailure::Rejected(Failure::io(e)))?,
            None => {}
        }
        Ok(())
    }
}

impl Drop for Continuous {
    fn drop(&mut self) {
        self.writer.take();
        if !self.temporary.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn geometry_changes_are_checked_only_on_selected_frames() {
        let command = crate::ffmpeg::args::parse(
            &"-i input.mp4 -r 1 -f rawvideo -"
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let original_shape = (2, 2, PixelFormat::Rgb24);
        let stream = coreav_decode::StreamConfig::new(coreav_decode::Codec::H264);
        let mut run = Run {
            command: &command,
            stream: &stream,
            encoder: VideoEncoder::Rawvideo,
            pixel_aspect: None,
            quarter_turns: 0,
            times: BTreeMap::from([(0, Rational::from_int(1)), (1, Rational::from_int(2))]),
            start: Rational::from_int(0),
            end: None,
            rate: command.frames.rate,
            source_duration: Rational::new(1, 5),
            count: 3,
            last_time: None,
            shape: Some(original_shape),
            input_shape: Some(original_shape),
            continuous: None,
            #[cfg(feature = "input-capture")]
            capture: None,
        };
        let mut frame = Frame {
            pts: Some(0),
            color: ColorInfo::default(),
            pixels: Picture::zeroed(2, 1, PixelFormat::Rgb24).unwrap().into(),
        };
        assert!(run.take(frame.clone()).is_ok());
        assert_eq!(run.count, 3);
        assert_eq!(run.input_shape, Some(original_shape));
        assert!(run.continuous.is_none());
        frame.pts = Some(1);
        assert!(
            matches!(run.take(frame), Err(Stop::Failed(WriteFailure::Rejected(Failure::VideoDecode(error)))) if error.kind == coreav_decode::ErrorKind::InvalidData)
        );
        assert_eq!(run.count, 3);
    }

    #[test]
    fn packet_pacing_bounds_bytes_before_the_packet_count() {
        let mut queue = PacketQueue {
            sizes: VecDeque::new(),
            bytes: 0,
            budget: 256 << 20,
        };
        queue.sent(120 << 20);
        queue.sent(120 << 20);
        assert!(!queue.full());
        queue.sent(120 << 20);
        assert!(queue.full());
        queue.retire();
        assert!(!queue.full());
        queue.clear();
        assert_eq!(queue.bytes, 0);
        assert!(queue.sizes.is_empty());
        for _ in 0..64 {
            queue.sent(1);
        }
        assert!(queue.full());
    }
}
