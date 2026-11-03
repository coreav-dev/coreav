use std::borrow::Cow;
use std::io::Write;

use coreav_frame::{ChromaUpsampling, Filter};
use coreav_isobmff::Source;
use coreav_thumbnail::{ImageFormat, ThumbnailOptions, thumbnail_bounded_with_budgets};
use serde::Deserialize;
use serde_json::Value;

use crate::ops::{Failure, WRITE_CHUNK};

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Format {
    #[default]
    #[serde(alias = "jpg")]
    Jpeg,
    Png,
    Webp,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Resampling {
    Bilinear,
    #[default]
    Bicubic,
    Area,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Chroma {
    #[default]
    Native,
    Full,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Options<'a> {
    #[serde(borrow)]
    at: Cow<'a, str>,
    stream: Option<usize>,
    width: Option<u32>,
    height: Option<u32>,
    #[serde(default)]
    format: Format,
    quality: Option<u8>,
    #[serde(default)]
    filter: Resampling,
    #[serde(default)]
    chroma: Chroma,
    limits: Option<Limits>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Limits {
    max_top_level_boxes: Option<u32>,
    max_depth: Option<u32>,
    max_total_boxes: Option<u32>,
    max_box_bytes: Option<u64>,
    max_retained_bytes: Option<u64>,
    max_table_bytes: Option<u64>,
    max_packet_bytes: Option<u64>,
    max_sample_work: Option<u64>,
    max_fragments: Option<u32>,
    max_tracks: Option<u32>,
    max_movie_bytes: Option<u64>,
    max_movie_ratio: Option<u32>,
    max_elements: Option<u32>,
    max_element_bytes: Option<u64>,
    max_index_bytes: Option<u64>,
    max_block_work: Option<u64>,
    max_width: Option<u32>,
    max_height: Option<u32>,
    max_pixels: Option<u64>,
    max_frame_bytes: Option<u64>,
    max_decoded_bytes: Option<u64>,
    max_pending_bytes: Option<u64>,
    max_memory_bytes: Option<u64>,
    max_planning_bytes: Option<usize>,
    max_output_bytes: Option<usize>,
}

fn resolve(options: Options<'_>) -> Result<(ThumbnailOptions, usize, usize), Failure> {
    let at = coreav_cli::clip::parse_time(&options.at)
        .ok_or_else(|| Failure::Usage("at must be nonnegative decimal seconds".into()))?;
    if options.width == Some(0) || options.height == Some(0) {
        return Err(Failure::Usage(
            "output dimensions must be positive integers".into(),
        ));
    }
    if options.width.is_some_and(|v| v > 1280)
        || options.height.is_some_and(|v| v > 1280)
        || options
            .width
            .zip(options.height)
            .is_some_and(|(w, h)| u64::from(w) * u64::from(h) > 921_600)
    {
        return Err(Failure::Error(serde_json::json!({
            "schema": "coreav.error/1",
            "kind": "limit",
            "limit": "thumbnail_output_dimensions",
            "offset": null,
            "message": "output dimensions exceed the thumbnail profile",
        })));
    }
    let quality = options.quality.unwrap_or(91);
    if !(1..=100).contains(&quality) {
        return Err(Failure::Usage("quality must be from 1 through 100".into()));
    }
    let mut resolved = ThumbnailOptions::at(at);
    resolved.stream = options.stream;
    resolved.width = options.width;
    resolved.height = options.height;
    resolved.quality = quality;
    resolved.format = match options.format {
        Format::Jpeg => ImageFormat::Jpeg,
        Format::Png => ImageFormat::Png,
        Format::Webp => ImageFormat::Webp,
    };
    resolved.filter = match options.filter {
        Resampling::Bilinear => Filter::Bilinear,
        Resampling::Bicubic => Filter::Bicubic,
        Resampling::Area => Filter::Area,
    };
    resolved.chroma = match options.chroma {
        Chroma::Native => ChromaUpsampling::FfmpegCompatible,
        Chroma::Full => ChromaUpsampling::Full,
    };
    resolved.isobmff_limits.max_retained_bytes = 4 << 20;
    resolved.isobmff_limits.max_table_bytes = 4 << 20;
    resolved.isobmff_limits.max_packet_bytes = 4 << 20;
    resolved.isobmff_limits.max_movie_bytes = 4 << 20;
    resolved.matroska_limits.max_retained_bytes = 4 << 20;
    resolved.matroska_limits.max_index_bytes = 4 << 20;
    resolved.matroska_limits.max_packet_bytes = 4 << 20;
    resolved.decode_limits.max_width = 1920;
    resolved.decode_limits.max_height = 1920;
    resolved.decode_limits.max_pixels = 2_088_960;
    resolved.decode_limits.max_frame_bytes = 4 << 20;
    resolved.decode_limits.max_decoded_bytes = 4 << 20;
    resolved.decode_limits.max_pending_bytes = 4 << 20;
    resolved.decode_limits.max_packet_bytes = 4 << 20;
    resolved.decode_limits.max_memory_bytes = 32 << 20;
    let mut planning = 4 << 20;
    let mut output = 4 << 20;
    if let Some(limits) = options.limits {
        macro_rules! lower {
            ($target:expr, $field:ident) => {
                if let Some(value) = limits.$field {
                    let target = &mut $target;
                    if value > *target {
                        return Err(Failure::Usage(
                            concat!(stringify!($field), " may only be lowered").into(),
                        ));
                    }
                    *target = value;
                }
            };
        }
        lower!(
            resolved.isobmff_limits.max_top_level_boxes,
            max_top_level_boxes
        );
        lower!(resolved.isobmff_limits.max_depth, max_depth);
        lower!(resolved.isobmff_limits.max_total_boxes, max_total_boxes);
        lower!(resolved.isobmff_limits.max_box_bytes, max_box_bytes);
        lower!(
            resolved.isobmff_limits.max_retained_bytes,
            max_retained_bytes
        );
        lower!(resolved.isobmff_limits.max_table_bytes, max_table_bytes);
        lower!(resolved.isobmff_limits.max_packet_bytes, max_packet_bytes);
        lower!(resolved.isobmff_limits.max_sample_work, max_sample_work);
        lower!(resolved.isobmff_limits.max_fragments, max_fragments);
        lower!(resolved.isobmff_limits.max_tracks, max_tracks);
        lower!(resolved.isobmff_limits.max_movie_bytes, max_movie_bytes);
        lower!(resolved.isobmff_limits.max_movie_ratio, max_movie_ratio);
        lower!(resolved.matroska_limits.max_elements, max_elements);
        lower!(
            resolved.matroska_limits.max_element_bytes,
            max_element_bytes
        );
        lower!(
            resolved.matroska_limits.max_retained_bytes,
            max_retained_bytes
        );
        lower!(resolved.matroska_limits.max_index_bytes, max_index_bytes);
        lower!(resolved.matroska_limits.max_packet_bytes, max_packet_bytes);
        lower!(resolved.matroska_limits.max_block_work, max_block_work);
        lower!(resolved.matroska_limits.max_tracks, max_tracks);
        lower!(resolved.decode_limits.max_width, max_width);
        lower!(resolved.decode_limits.max_height, max_height);
        lower!(resolved.decode_limits.max_pixels, max_pixels);
        lower!(resolved.decode_limits.max_frame_bytes, max_frame_bytes);
        lower!(resolved.decode_limits.max_decoded_bytes, max_decoded_bytes);
        lower!(resolved.decode_limits.max_pending_bytes, max_pending_bytes);
        lower!(resolved.decode_limits.max_packet_bytes, max_packet_bytes);
        lower!(resolved.decode_limits.max_memory_bytes, max_memory_bytes);
        lower!(planning, max_planning_bytes);
        lower!(output, max_output_bytes);
    }
    Ok((resolved, planning, output))
}

pub(crate) fn run<S: Source, W: Write>(
    source: S,
    options: &str,
    mut out: W,
) -> Result<Value, Failure> {
    let args: Options<'_> = serde_json::from_str(options)
        .map_err(|e| Failure::Usage(format!("invalid thumbnail options: {e}")))?;
    let (options, planning, output) = resolve(args)?;
    let made = thumbnail_bounded_with_budgets(source, &options, planning, output)
        .map_err(|e| Failure::Error(coreav_cli::thumbnail::failure(e).to_json()))?;
    let document = coreav_cli::thumbnail::document(&made, options.at);
    for chunk in made.image.chunks(WRITE_CHUNK) {
        out.write_all(chunk)
            .map_err(|e| Failure::Error(coreav_cli::output::WriteFailure::output(&e).to_json()))?;
    }
    out.flush()
        .map_err(|e| Failure::Error(coreav_cli::output::WriteFailure::output(&e).to_json()))?;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_isobmff::{Error, SliceSource};

    struct Unreadable;
    impl Source for Unreadable {
        fn len(&mut self) -> Result<u64, Error> {
            panic!("invalid options read the source")
        }
        fn read_exact_at(&mut self, _: u64, _: &mut [u8]) -> Result<(), Error> {
            panic!("invalid options read the source")
        }
    }

    #[test]
    fn invalid_options_are_refused_before_reading() {
        for options in [
            "{}",
            r#"{"at":1}"#,
            r#"{"at":"1s"}"#,
            r#"{"at":"-1"}"#,
            r#"{"at":"0","exact":true}"#,
            r#"{"at":"0","format":"bmp"}"#,
            r#"{"at":"0","filter":"lanczos"}"#,
            r#"{"at":"0","chroma":"wrong"}"#,
            r#"{"at":"0","width":0}"#,
            r#"{"at":"0","quality":0}"#,
            r#"{"at":"0","quality":101}"#,
            r#"{"at":"0","limits":{"maxPlanningBytes":4194305}}"#,
            r#"{"at":"0","limits":{"maxOutputBytes":4194305}}"#,
            r#"{"at":"0","limits":{"maxMemoryBytes":33554433}}"#,
            r#"{"at":"0","limits":{"unknown":0}}"#,
        ] {
            assert!(
                matches!(run(Unreadable, options, Vec::new()), Err(Failure::Usage(_))),
                "{options}"
            );
        }
    }

    #[test]
    fn excessive_output_dimensions_are_limits_before_reading() {
        for options in [
            r#"{"at":"0","height":1281}"#,
            r#"{"at":"0","width":1280,"height":721}"#,
        ] {
            let Err(Failure::Error(document)) = run(Unreadable, options, Vec::new()) else {
                panic!("excessive dimensions must be a limit");
            };
            assert_eq!(document["kind"], "limit");
        }
    }

    #[test]
    fn valid_options_reach_the_media_parser() {
        for options in [
            r#"{"at":"0"}"#,
            r#"{"at":"\u0030"}"#,
            r#"{"at":"1.5","height":120,"format":"png","filter":"area","chroma":"full"}"#,
            r#"{"at":"0","limits":{"maxPlanningBytes":0,"maxOutputBytes":0}}"#,
        ] {
            assert!(matches!(
                run(SliceSource::new(&[]), options, Vec::new()),
                Err(Failure::Error(_))
            ));
        }
    }
}
