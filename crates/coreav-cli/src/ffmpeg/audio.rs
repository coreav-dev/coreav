use super::args::{AudioEncoder, Command, Encoding, MapMetadata, Muxer};
use super::{Job, Refusal};
use crate::ffprobe::model::Value;
use crate::ffprobe::{Report, Stream, StreamKind};

pub(super) fn plan(
    command: &Command,
    report: &Report,
    output: &[(usize, &Stream)],
    encoded: &[(usize, AudioEncoder)],
    trims: bool,
) -> Result<Job, Refusal> {
    let ([(index, stream)], [(chosen, encoder)]) = (output, encoded) else {
        return Err(Refusal::not_native(match encoded.first().map(|(_, e)| e) {
            Some(AudioEncoder::Libopus) => {
                "encoding audio with libopus without encoding the video with libx264"
            }
            Some(AudioEncoder::Libmp3lame) => {
                "encoding other than one selected audio stream with libmp3lame"
            }
            _ => "encoding audio with aac without encoding the video with libx264",
        }));
    };
    if index != chosen || stream.kind != StreamKind::Audio {
        return Err(Refusal::not_native(
            "encoding a non-audio stream with an audio encoder",
        ));
    }
    if trims {
        return Err(Refusal::not_native("seeking or trimming while encoding"));
    }
    if stream
        .entries
        .iter()
        .any(|(key, value)| *key == "start_pts" && matches!(value, Value::Int(n) if *n > 0))
    {
        return Err(Refusal::not_native(
            "an audio-only encode with a delayed source stream",
        ));
    }
    if !matches!(
        (command.muxer, encoder),
        (Muxer::Mp4, AudioEncoder::Aac | AudioEncoder::Libopus)
            | (Muxer::Ipod, AudioEncoder::Aac)
            | (Muxer::Mp3, AudioEncoder::Libmp3lame)
    ) {
        return Err(Refusal::not_native(
            "the selected audio encoder in this output container",
        ));
    }
    let mut options = command.encoding.clone();
    options.audio_bitrate = None;
    if *encoder == AudioEncoder::Libmp3lame {
        options.audio_quality = None;
    }
    if options != Encoding::default() {
        return Err(Refusal::not_native(
            "video encoder options on an audio-only encode",
        ));
    }
    if !report.chapters.is_empty() {
        return Err(Refusal::not_native("chapters on an audio-only encode"));
    }
    if matches!(command.map_metadata, MapMetadata::Stream(_)) {
        return Err(Refusal::not_native(
            "metadata from a selected stream on an audio-only encode",
        ));
    }
    if command.muxer != Muxer::Mp3
        && stream
            .disposition
            .iter()
            .any(|(key, value)| *key != "default" && !matches!(value, Value::Int(0)))
    {
        return Err(Refusal::not_native(
            "source dispositions on an audio-only encode",
        ));
    }
    if command.muxer != Muxer::Mp3
        && command.map_metadata == MapMetadata::Default
        && (report.format_tags.iter().any(|(key, _)| {
            !matches!(
                key.as_str(),
                "major_brand" | "minor_version" | "compatible_brands" | "encoder" | "ENCODER"
            )
        }) || stream.tags.iter().any(|(key, value)| match key.as_str() {
            "language" => value != "und",
            "handler_name" => value != "SoundHandler",
            "vendor_id" | "encoder" | "ENCODER" | "DURATION" => false,
            _ => true,
        }))
    {
        return Err(Refusal::not_native(
            "source metadata on an audio-only encode",
        ));
    }
    super::audio_source(*encoder, stream)?;
    Ok(Job::AudioEncode {
        stream: *index,
        encoder: *encoder,
    })
}

#[cfg(feature = "transcode")]
pub(super) fn write(
    command: &Command,
    report: &Report,
    stream: usize,
    encoder: AudioEncoder,
    strict_safe: crate::strict_safe::StrictSafe,
) -> Result<serde_json::Value, super::Stop> {
    use crate::output::{self, WriteFailure};
    use coreav_isobmff::{BufferedSource, FileSource, FourCc};
    use coreav_isobmff_write::mux::FileType;
    use coreav_transcode::audio::{self, Encode};
    use coreav_transcode::encoder::{EncoderChoice, find_encoder};
    use coreav_worker::{EXEC_ARG, ExecLauncher, WorkerCommand};
    use std::path::Path;
    use std::time::{Duration, Instant};

    strict_safe.sandbox().map_err(|message| {
        Refusal::NotNative(super::NotNative::property(
            "audio encoding in strict-safe mode",
            message,
        ))
    })?;
    let this = std::env::current_exe()
        .map_err(|_| Refusal::not_native("encoding without the path of the running program"))?;
    let bitrate = command
        .encoding
        .audio_bitrate
        .as_deref()
        .map(|text| {
            super::encode::bits(text)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| Refusal::Usage(format!("Invalid audio bit rate '{text}'.")))
        })
        .transpose()?;
    let opus = coreav_transcode::OpusAudio {
        stream: Some(stream),
        bitrate,
        program: this.clone(),
    };
    let aac = coreav_transcode::AacAudio {
        stream: Some(stream),
        bitrate,
    };
    let mp3 = coreav_transcode::mp3::Mp3Audio {
        stream: Some(stream),
        mode: match command.encoding.audio_quality.as_deref() {
            Some(text) => coreav_transcode::mp3::Mode::Quality(
                text.parse::<f32>()
                    .ok()
                    .filter(|q| q.is_finite() && (0.0..=9.0).contains(q))
                    .ok_or_else(|| {
                        Refusal::Usage(format!("Invalid MP3 quality '{text}': use 0 to 9."))
                    })?,
            ),
            None => coreav_transcode::mp3::Mode::Constant(bitrate),
        },
    };
    let encode = match encoder {
        AudioEncoder::Libopus => Encode::Opus(&opus),
        external => {
            let (words, message) = if external == AudioEncoder::Aac {
                (
                    "no ffmpeg program for AAC audio",
                    "no ffmpeg program for AAC audio: install FFmpeg with its built-in aac encoder",
                )
            } else {
                (
                    "no ffmpeg program for MP3 audio",
                    "no ffmpeg program for MP3 audio: install FFmpeg with libmp3lame",
                )
            };
            let program = find_encoder(EncoderChoice::Ffmpeg, None, None)
                .ok_or_else(|| Refusal::NotNative(super::NotNative::property(words, message)))?;
            if external == AudioEncoder::Aac {
                Encode::Aac(&aac, program.path().to_path_buf())
            } else {
                Encode::Mp3(&mp3, program.path().to_path_buf())
            }
        }
    };
    let input = Path::new(&command.input);
    let path = Path::new(&command.output);
    let force = super::check_output(command, path)?;
    let launcher = ExecLauncher {
        command: WorkerCommand {
            program: this,
            args: strict_safe.entry(EXEC_ARG),
        },
    };
    let deadline = Instant::now() + Duration::from_secs(3600);
    output::to_file(path, force, |out| {
        let source = BufferedSource::new(FileSource::open(input).map_err(WriteFailure::from)?);
        let limits = coreav_audio::Limits::default();
        let encoded = audio::encode(
            source,
            &encode,
            &limits,
            Some(&launcher),
            deadline,
            path,
            coreav_transcode::pipeline::MAX_SPOOL_BYTES,
        )
        .map_err(crate::transcode::failure)?;
        if command.muxer == Muxer::Mp3 {
            let tags = if command.map_metadata == MapMetadata::Strip {
                &[][..]
            } else {
                report.format_tags.as_slice()
            };
            encoded
                .write_mp3(tags, deadline, out)
                .map_err(crate::transcode::failure)?;
            return Ok(serde_json::Value::Null);
        }
        let file_type = if command.muxer == Muxer::Ipod {
            FileType {
                major: FourCc(*b"M4A "),
                minor: 512,
                compatible: vec![FourCc(*b"M4A "), FourCc(*b"isom"), FourCc(*b"iso2")],
            }
        } else {
            FileType {
                major: FourCc(*b"isom"),
                minor: 512,
                compatible: vec![FourCc(*b"isom"), FourCc(*b"iso2"), FourCc(*b"mp41")],
            }
        };
        encoded
            .write_mp4(file_type, &limits.isobmff, deadline, out)
            .map_err(crate::transcode::failure)?;
        Ok(serde_json::Value::Null)
    })
    .map_err(|failure| super::encode::failed(failure, Some(encoder)))
}
