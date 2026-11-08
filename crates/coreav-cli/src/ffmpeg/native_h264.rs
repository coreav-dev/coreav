//! Explicit native encoding preference with unchanged external fallback arguments.

use coreav_transcode::native_h264::{NativeH264, RateMode};
use std::time::Instant;

use super::args::{Codec, Command, Muxer, Spec};
use super::{Refusal, Stop};
use crate::ffprobe::{Report, args::Kind2, model::Value};
use crate::strict_safe::StrictSafe;

#[derive(Clone, Copy)]
/// An encode attempt within the original opt-in command's time budget.
pub struct EncodeAttempt {
    /// Native encoding, or the existing encoder on retry.
    pub native: Option<NativeH264>,
    /// The same deadline for both attempts.
    pub deadline: Instant,
}

fn candidate(args: &[String]) -> Option<(Command, NativeH264)> {
    let (command, qp) = super::args::native_h264(args).ok()?;
    let encoding = &command.encoding;
    if !command.more_inputs.is_empty()
        || !matches!(command.muxer, Muxer::Mp4 | Muxer::Matroska)
        || !command.codecs.iter().any(|(spec, codec)| {
            *codec == Codec::Libx264 && matches!(spec, Spec::Kind(Kind2::Video, None | Some(0)))
        })
        || encoding.bitrate.is_some()
        || encoding.preset.is_some()
        || encoding.filter.is_some()
        || encoding.size.is_some()
        || encoding
            .pixel_format
            .as_deref()
            .is_some_and(|s| s != "yuv420p")
    {
        return None;
    }
    let quality = |text: &str| text.parse::<u8>().ok().filter(|q| (10..=51).contains(q));
    let rate = match (encoding.crf.as_deref(), qp.as_deref()) {
        (None, None) => RateMode::Crf(23),
        (Some(text), None) => RateMode::Crf(quality(text)?),
        (None, Some(text)) => RateMode::FixedQp(quality(text)?),
        (Some(_), Some(_)) => return None,
    };
    Some((command, NativeH264 { rate }))
}

pub(super) fn source(report: &Report, index: usize) -> bool {
    let Some(stream) = report.streams.get(index) else {
        return false;
    };
    let field = |name| {
        stream
            .entries
            .iter()
            .find_map(|(k, v)| (*k == name).then_some(v))
    };
    let dimension = |name, max| matches!(field(name), Some(Value::Int(n)) if (16..=max).contains(n) && n % 2 == 0);
    matches!(field("field_order"), Some(Value::Str(s)) if s == "progressive")
        && matches!(field("pix_fmt"), Some(Value::Str(s)) if s == "yuv420p")
        && dimension("width", 1920)
        && dimension("height", 1080)
        && stream.r_frame_rate().is_some()
}

/// Tries the opt-in encoder without changing the command used on fallback.
pub fn run(
    args: &[String],
    strict_safe: StrictSafe,
    deadline: Instant,
) -> Result<Option<serde_json::Value>, Stop> {
    if strict_safe.on() {
        return Ok(None);
    }
    let Some((command, native)) = candidate(args) else {
        return Ok(None);
    };
    let attempt = EncodeAttempt {
        native: Some(native),
        deadline,
    };
    match super::run_command(command, strict_safe, None, None, None, Some(attempt)) {
        Ok(value) => Ok(Some(value)),
        Err(Stop::Refused(Refusal::NotNative(_))) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn expired_external_retry_keeps_destination_and_removes_staging() {
        let dir = std::env::temp_dir().join(format!("coreav-native-retry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.mp4");
        let output = dir.join("out.mp4");
        std::fs::write(
            &input,
            crate::test_isobmff::av_movie_with(crate::test_isobmff::aac_entry(), false),
        )
        .unwrap();
        std::fs::write(&output, b"keep destination").unwrap();
        let mut args = args("-y");
        for arg in &mut args {
            if arg == "in.mp4" {
                *arg = input.to_string_lossy().into_owned();
            } else if arg == "out.mp4" {
                *arg = output.to_string_lossy().into_owned();
            }
        }
        let result = super::super::run_attempt(
            &args,
            StrictSafe::new(false),
            EncodeAttempt {
                native: None,
                deadline: Instant::now(),
            },
        );
        match result {
            Err(Stop::Failed(failure)) => assert_eq!(failure.to_json()["limit"], "timeout"),
            _ => panic!("the expired external retry must fail with timeout"),
        }
        assert_eq!(std::fs::read(&output).unwrap(), b"keep destination");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_admission_requires_known_progressive_geometry_and_clock() {
        let mut report = Report::default();
        report.streams.push(crate::ffprobe::Stream {
            kind: crate::ffprobe::StreamKind::Video,
            attached_pic: false,
            entries: vec![
                ("width", Value::Int(1920)),
                ("height", Value::Int(1080)),
                ("field_order", Value::Str("progressive".into())),
                ("pix_fmt", Value::Str("yuv420p".into())),
                ("r_frame_rate", Value::Str("30/1".into())),
            ],
            disposition: Vec::new(),
            tags: Vec::new(),
            side_data: Vec::new(),
        });
        assert!(source(&report, 0));
        assert!(!source(&report, 1));
        for (name, value) in [
            ("width", Value::Int(1922)),
            ("height", Value::Int(1082)),
            ("width", Value::Int(14)),
            ("width", Value::Int(17)),
            ("field_order", Value::Str("tt".into())),
            ("field_order", Value::Na),
            ("pix_fmt", Value::Str("yuv420p10le".into())),
            ("pix_fmt", Value::Str("yuv444p".into())),
            ("r_frame_rate", Value::Str("0/0".into())),
        ] {
            let mut changed = report.clone();
            if let Some(stream) = changed.streams.first_mut() {
                for (key, field) in &mut stream.entries {
                    if *key == name {
                        *field = value.clone();
                    }
                }
            }
            assert!(!source(&changed, 0), "{name}: {value:?}");
        }
        for rate in ["25/1", "24000/1001", "30000/1001", "1000/33"] {
            let mut changed = report.clone();
            if let Some(stream) = changed.streams.first_mut() {
                for (name, field) in &mut stream.entries {
                    if *name == "r_frame_rate" {
                        *field = Value::Str(rate.into());
                    }
                }
            }
            assert!(source(&changed, 0), "{rate}");
        }
    }

    fn args(extra: &str) -> Vec<String> {
        format!("-i in.mp4 -an -c:v libx264 {extra} out.mp4")
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn integer_quality_modes_map_without_rounding() {
        for q in 10..=51 {
            assert_eq!(
                candidate(&args(&format!("-crf {q}"))).map(|(_, n)| n.rate),
                Some(RateMode::Crf(q))
            );
            assert_eq!(
                candidate(&args(&format!("-qp {q}"))).map(|(_, n)| n.rate),
                Some(RateMode::FixedQp(q))
            );
        }
        assert_eq!(
            candidate(&args("")).map(|(_, n)| n.rate),
            Some(RateMode::Crf(23))
        );
    }

    #[test]
    fn matroska_selection_keeps_arguments_and_excludes_webm() {
        for suffix in ["out.mp4", "out.mkv", "-f matroska out.bin"] {
            let args: Vec<String> = format!("-i in.mp4 -an -c:v libx264 -crf 23 {suffix}")
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            let before = args.clone();
            assert!(candidate(&args).is_some(), "{suffix}");
            assert_eq!(args, before);
        }
        let webm: Vec<String> = "-i in.mp4 -an -c:v libx264 out.webm"
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        assert!(candidate(&webm).is_none());
    }

    #[test]
    fn unsupported_controls_keep_the_existing_command() {
        for option in [
            "-profile:v high",
            "-profile:v baseline",
            "-bf 1",
            "-b:v 1M",
            "-maxrate 1M",
            "-bufsize 2M",
            "-x264-params cabac=1",
            "-x264opts bframes=1",
            "-tune film",
            "-g 12",
            "-level 4.1",
            "-r 25",
            "-preset medium",
            "-pix_fmt yuv420p10le",
            "-pix_fmt yuv444p",
            "-crf 9",
            "-crf 52",
            "-crf 23.5",
            "-qp 0",
            "-qp 52",
            "-qp 23.5",
            "-crf 23 -qp 24",
            "-vf scale=32:32",
            "-s 32x32",
            "-pass 1",
            "-b_strategy 1",
            "-refs 2",
            "-flags +ildct",
            "-force_key_frames 0",
        ] {
            let original = args(option);
            let before = original.clone();
            assert!(candidate(&original).is_none(), "{option}");
            assert_eq!(original, before);
        }
    }

    #[test]
    fn qp_tokens_belong_to_their_option_or_filename() {
        for line in [
            "-i in.mp4 -c:v -qp out.mp4",
            "-qp 23 -i in.mp4 -an -c:v libx264 out.mp4",
            "-i in.mp4 -an -c:v libx264 out.mp4 -qp 23",
        ] {
            let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
            assert!(candidate(&args).is_none(), "{line}");
        }
        let named: Vec<String> = "-i -qp -an -c:v libx264 out.mp4"
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        assert!(candidate(&named).is_some_and(
            |(command, native)| command.input == "-qp" && native.rate == RateMode::Crf(23)
        ));
        assert!(super::super::args::parse(&args("-qp 23")).is_err());
    }
}
