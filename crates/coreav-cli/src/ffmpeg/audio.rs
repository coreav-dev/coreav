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
    if !matches!(command.muxer, Muxer::Mp4 | Muxer::Ipod) {
        return Err(Refusal::not_native("an audio encode outside MP4 or M4A"));
    }
    if command.muxer == Muxer::Ipod && *encoder != AudioEncoder::Aac {
        return Err(Refusal::not_native(
            "an audio encode to M4A with a codec other than AAC",
        ));
    }
    let mut options = command.encoding.clone();
    options.audio_bitrate = None;
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
    if stream
        .disposition
        .iter()
        .any(|(key, value)| *key != "default" && !matches!(value, Value::Int(0)))
    {
        return Err(Refusal::not_native(
            "source dispositions on an audio-only encode",
        ));
    }
    if command.map_metadata == MapMetadata::Default
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
    let encode = match encoder {
        AudioEncoder::Libopus => Encode::Opus(&opus),
        AudioEncoder::Aac => {
            let program = find_encoder(EncoderChoice::Ffmpeg, None, None).ok_or_else(|| {
                Refusal::NotNative(super::NotNative::property(
                    "no ffmpeg program for AAC audio",
                    "no ffmpeg program for AAC audio: install FFmpeg with its built-in aac encoder",
                ))
            })?;
            Encode::Aac(&aac, program.path().to_path_buf())
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
