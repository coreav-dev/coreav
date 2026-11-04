//! Atomic formatting of the default-info input diagnostic.

use super::model::Value;
use super::{Entries, Report, SideData, Stream, StreamKind, SummaryAspect, SummaryEvidence};
use crate::error::Failure;

/// Builds a complete default-info input summary without emitting output.
pub fn render(report: &Report, input_index: usize, name: &str) -> Result<Vec<u8>, Failure> {
    if name.chars().any(|ch| ch.is_ascii_control()) {
        return unsupported("input name control characters");
    }
    let (start, duration) = match report.summary {
        SummaryEvidence::Complete { start, duration } => (start, duration),
        SummaryEvidence::Unverified => return unsupported("unverified input summary"),
        SummaryEvidence::Unsupported(reason) => return unsupported(reason),
    };
    let format = required(&report.format, "format_name")?;
    if !matches!(
        format,
        "mov,mp4,m4a,3gp,3g2,mj2" | "matroska,webm" | "mpegts" | "wav" | "flac"
    ) {
        return unsupported("input summary format");
    }
    let mut out = format!("Input #{input_index}, {format}, from '{name}':\n");
    metadata(&mut out, &report.format_tags, 2)?;
    out.push_str("  Duration: ");
    if let Some(us) = duration {
        if us < 0 {
            return unsupported("negative input duration");
        }
        let centiseconds = (i128::from(us) + 5_000) / 10_000;
        out.push_str(&format!(
            "{:02}:{:02}:{:02}.{:02}",
            centiseconds / 360_000,
            centiseconds / 6_000 % 60,
            centiseconds / 100 % 60,
            centiseconds % 100
        ));
    } else {
        out.push_str("N/A");
    }
    if let Some(us) = start {
        let magnitude = us.unsigned_abs();
        out.push_str(&format!(
            ", start: {}{}.{:06}",
            if us < 0 { "-" } else { "" },
            magnitude / 1_000_000,
            magnitude % 1_000_000
        ));
    }
    out.push_str(", bitrate: ");
    if let Some(rate) = integer(&report.format, "bit_rate")? {
        if rate <= 0 {
            out.push_str("N/A");
        } else {
            out.push_str(&format!("{} kb/s", rate / 1000));
        }
    } else {
        out.push_str("N/A");
    }
    out.push('\n');
    if !report.chapters.is_empty() {
        out.push_str("  Chapters:\n");
        for (index, chapter) in report.chapters.iter().enumerate() {
            let start =
                seconds(&chapter.entries, "start_time")?.ok_or_else(|| missing("chapter start"))?;
            let end =
                seconds(&chapter.entries, "end_time")?.ok_or_else(|| missing("chapter end"))?;
            out.push_str(&format!(
                "    Chapter #{input_index}:{index}: start {start:.6}, end {end:.6}\n"
            ));
            metadata(&mut out, &chapter.tags, 6)?;
        }
    }
    let mut members = vec![false; report.streams.len()];
    let mut memberships = 0usize;
    for program in &report.programs {
        let id =
            integer(&program.entries, "program_id")?.ok_or_else(|| missing("program identity"))?;
        out.push_str(&format!("  Program {id} \n"));
        metadata(&mut out, &program.tags, 4)?;
        memberships = memberships.saturating_add(program.streams.len());
        for &index in &program.streams {
            let stream = report
                .streams
                .get(index)
                .ok_or_else(|| missing("program membership"))?;
            let member = members
                .get_mut(index)
                .ok_or_else(|| missing("program membership"))?;
            *member = true;
            render_stream(
                &mut out,
                stream,
                input_index,
                &report.summary_aspects,
                format,
            )?;
        }
    }
    if !report.programs.is_empty() && memberships < report.streams.len() {
        out.push_str("  No Program\n");
    }
    for (stream, member) in report.streams.iter().zip(members) {
        if !member {
            render_stream(
                &mut out,
                stream,
                input_index,
                &report.summary_aspects,
                format,
            )?;
        }
    }
    Ok(out.into_bytes())
}

/// Builds qualified decoder warnings that follow an ffprobe input summary.
pub fn warnings(report: &Report) -> Result<Vec<u8>, Failure> {
    match report.summary {
        SummaryEvidence::Complete { .. } => {}
        SummaryEvidence::Unverified => return unsupported("unverified input warnings"),
        SummaryEvidence::Unsupported(reason) => return unsupported(reason),
    }
    let mut out = String::new();
    for stream in &report.streams {
        let codec = required(&stream.entries, "codec_name")?;
        if super::names::long_name(codec).is_none() {
            return unsupported("unknown input warning codec");
        }
        let id = match codec {
            "bin_data" => Some(98314),
            "ttf" => Some(98304),
            "otf" => Some(98310),
            _ if matches!(stream.kind, StreamKind::Data | StreamKind::Attachment)
                || codec == "ttml" =>
            {
                return unsupported("unqualified decoder warning");
            }
            _ => None,
        };
        if let Some(id) = id {
            let index = integer(&stream.entries, "index")?
                .filter(|index| *index >= 0)
                .ok_or_else(|| missing("input warning stream identity"))?;
            out.push_str(&format!(
                "Unsupported codec with id {id} for input stream {index}\n"
            ));
        }
    }
    Ok(out.into_bytes())
}

fn render_stream(
    out: &mut String,
    stream: &Stream,
    input: usize,
    aspects: &[SummaryAspect],
    format: &str,
) -> Result<(), Failure> {
    let entries = &stream.entries;
    let index = integer(entries, "index")?.ok_or_else(|| missing("stream identity"))?;
    if index < 0 {
        return unsupported("negative stream identity");
    }
    out.push_str(&format!("  Stream #{input}:{index}"));
    if let Some(id) = text(entries, "id") {
        if !id.starts_with("0x") || u64::from_str_radix(id.trim_start_matches("0x"), 16).is_err() {
            return unsupported("stream identity");
        }
        out.push_str(&format!("[{id}]"));
    }
    if let Some((_, language)) = stream
        .tags
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("language"))
    {
        if language.chars().any(|ch| ch.is_ascii_control()) {
            return unsupported("stream language");
        }
        out.push_str(&format!("({language})"));
    }
    let kind = match stream.kind {
        StreamKind::Video => "Video",
        StreamKind::Audio => "Audio",
        StreamKind::Subtitle => "Subtitle",
        StreamKind::Data => "Data",
        StreamKind::Attachment => "Attachment",
    };
    let codec = required(entries, "codec_name")?;
    if super::names::long_name(codec).is_none() {
        return unsupported("unknown summary codec");
    }
    out.push_str(&format!(": {kind}: {codec}"));
    let decoder = match codec {
        "av1" => Some("libdav1d"),
        "mp3" => Some("mp3float"),
        "mp2" if format == "mpegts" => Some("mp3float"),
        "dts" => Some("dca"),
        "amr_nb" => Some("amrnb"),
        "amr_wb" => Some("amrwb"),
        "ass" => Some("ssa"),
        "subrip" => Some("srt"),
        "dvb_subtitle" => Some("dvbsub"),
        "dvb_teletext" => Some("libzvbi_teletextdec"),
        "eia_608" => Some("cc_dec"),
        "hdmv_pgs_subtitle" => Some("pgssub"),
        _ => None,
    };
    if let Some(decoder) = decoder {
        out.push_str(&format!(" ({decoder})"));
    }
    if let Some(profile) = text(entries, "profile").filter(|_| codec != "vp8") {
        out.push_str(&format!(" ({profile})"));
    } else if matches!(
        codec,
        "h264"
            | "hevc"
            | "vvc"
            | "av1"
            | "vp9"
            | "mpeg2video"
            | "mpeg4"
            | "aac"
            | "aac_latm"
            | "dts"
            | "prores"
    ) {
        return unsupported("unknown codec profile");
    }
    let tag = required(entries, "codec_tag")?;
    let tag_value = tag
        .strip_prefix("0x")
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .ok_or_else(|| missing("codec tag"))?;
    if tag_value != 0 {
        let tag_name = required(entries, "codec_tag_string")?;
        out.push_str(&format!(" ({tag_name} / 0x{tag_value:04X})"));
    }
    let mut stream_aspect = None;
    match stream.kind {
        StreamKind::Video => {
            let aspect = usize::try_from(index)
                .ok()
                .and_then(|index| aspects.get(index))
                .copied()
                .unwrap_or_default();
            stream_aspect = video(out, entries, aspect)?;
        }
        StreamKind::Audio => audio(out, entries)?,
        StreamKind::Subtitle => {
            let width = integer(entries, "width")?.unwrap_or(0);
            let height = integer(entries, "height")?.unwrap_or(0);
            if width > 0 && height > 0 {
                out.push_str(&format!(", {width}x{height}"));
            }
        }
        StreamKind::Data | StreamKind::Attachment => {}
    }
    if let Some(rate) = integer(entries, "bit_rate")?.filter(|rate| *rate > 0) {
        out.push_str(&format!(", {} kb/s", rate / 1000));
    }
    if let Some(aspect) = stream_aspect {
        out.push_str(&aspect);
    }
    if stream.kind == StreamKind::Video {
        for (key, label, reciprocal) in [
            ("avg_frame_rate", "fps", false),
            ("r_frame_rate", "tbr", false),
            ("time_base", "tbn", true),
        ] {
            if let Some(rate) = ratio(entries, key)? {
                let value = if reciprocal { 1.0 / rate } else { rate };
                out.push_str(&format!(", {} {label}", rate_text(value)?));
            }
        }
    }
    if let Some(start) = seconds(entries, "start_time")?.filter(|start| *start != 0.0) {
        out.push_str(&format!(", start {start:.6}"));
    }
    let service_disposition = if matches!(codec, "ac3" | "eac3") {
        let service = stream
            .side_data
            .iter()
            .find(|side| side.typ == "audio_service_type");
        match service
            .map(|side| integer(&side.entries, "service_type"))
            .transpose()?
            .flatten()
        {
            Some(1) => Some("clean_effects"),
            Some(2) => Some("visual_impaired"),
            Some(3) => Some("hearing_impaired"),
            Some(5) => Some("comment"),
            Some(8) => Some("karaoke"),
            _ => None,
        }
    } else {
        None
    };
    if let Some(flag) = service_disposition {
        let mut effective = stream.disposition.clone();
        for (key, value) in &mut effective {
            if *key == "default" {
                *value = Value::Int(0);
            }
        }
        effective.retain(|(key, _)| *key != flag);
        effective.push((flag, Value::Int(1)));
        dispositions(out, &effective)?;
    } else {
        dispositions(out, &stream.disposition)?;
    }
    out.push('\n');
    metadata(out, &stream.tags, 4)?;
    if !stream.side_data.is_empty() {
        out.push_str("    Side data:\n");
        for side in &stream.side_data {
            side_data(out, side)?;
        }
    }
    Ok(())
}

fn video(
    out: &mut String,
    entries: &Entries,
    aspect: SummaryAspect,
) -> Result<Option<String>, Failure> {
    let codec = match aspect {
        SummaryAspect::Complete { codec } => codec,
        SummaryAspect::Unverified => return unsupported("video aspect ratio provenance"),
    };
    for field in ["closed_captions", "film_grain"] {
        if integer(entries, field)?.is_some_and(|value| value != 0) {
            return unsupported("video auxiliary description");
        }
    }
    out.push_str(&format!(", {}", required(entries, "pix_fmt")?));
    let mut color = Vec::new();
    if let Some(range) = text(entries, "color_range") {
        color.push(range.to_owned());
    }
    let space = text(entries, "color_space");
    let primaries = text(entries, "color_primaries");
    let transfer = text(entries, "color_transfer");
    if space.is_some() || primaries.is_some() || transfer.is_some() {
        let (space, primaries, transfer) = (
            space.unwrap_or("unknown"),
            primaries.unwrap_or("unknown"),
            transfer.unwrap_or("unknown"),
        );
        color.push(if space == primaries && space == transfer {
            space.to_owned()
        } else {
            format!("{space}/{primaries}/{transfer}")
        });
    }
    if let Some(order) = text(entries, "field_order") {
        color.push(
            match order {
                "progressive" => "progressive",
                "tt" => "top first",
                "bb" => "bottom first",
                "tb" => "top coded first (swapped)",
                "bt" => "bottom coded first (swapped)",
                _ => return unsupported("video field order"),
            }
            .to_owned(),
        );
    }
    if !color.is_empty() {
        out.push_str(&format!("({})", color.join(", ")));
    }
    let width = integer(entries, "width")?
        .filter(|value| *value > 0)
        .ok_or_else(|| missing("video width"))?;
    let height = integer(entries, "height")?
        .filter(|value| *value > 0)
        .ok_or_else(|| missing("video height"))?;
    out.push_str(&format!(", {width}x{height}"));
    if let Some((num, den)) = codec {
        if num == 0 || den == 0 {
            return unsupported("codec sample aspect ratio");
        }
        let common = crate::compat::gcd(u64::from(num), u64::from(den));
        let (dar_num, dar_den) = crate::compat::bounded_ratio(
            u64::from(u32::try_from(width).map_err(|_| missing("video width"))?) * u64::from(num),
            u64::from(u32::try_from(height).map_err(|_| missing("video height"))?) * u64::from(den),
        )
        .ok_or_else(|| missing("codec display aspect ratio"))?;
        out.push_str(&format!(
            " [SAR {}:{} DAR {dar_num}:{dar_den}]",
            u64::from(num) / common,
            u64::from(den) / common,
        ));
    }
    let Some(sar) = text(entries, "sample_aspect_ratio").filter(|sar| *sar != "0:1") else {
        return Ok(None);
    };
    let (num, den) = sar
        .split_once(':')
        .ok_or_else(|| missing("stream sample aspect ratio"))?;
    let num: u32 = num
        .parse()
        .map_err(|_| missing("stream sample aspect ratio"))?;
    let den: u32 = den
        .parse()
        .map_err(|_| missing("stream sample aspect ratio"))?;
    if num == 0 || den == 0 {
        return unsupported("stream sample aspect ratio");
    }
    if codec
        .is_some_and(|(cn, cd)| u64::from(num) * u64::from(cd) == u64::from(cn) * u64::from(den))
    {
        return Ok(None);
    }
    let dar = required(entries, "display_aspect_ratio")?;
    Ok(Some(format!(", SAR {sar} DAR {dar}")))
}

fn audio(out: &mut String, entries: &Entries) -> Result<(), Failure> {
    let sample_rate = integer(entries, "sample_rate")?
        .filter(|rate| *rate > 0)
        .ok_or_else(|| missing("audio sample rate"))?;
    out.push_str(&format!(", {sample_rate} Hz, "));
    if let Some(layout) = text(entries, "channel_layout") {
        out.push_str(layout);
    } else {
        let channels = integer(entries, "channels")?
            .filter(|channels| *channels > 0)
            .ok_or_else(|| missing("audio channels"))?;
        out.push_str(&format!("{channels} channels"));
    }
    let sample_format = required(entries, "sample_fmt")?;
    let bits = match sample_format {
        "u8" | "u8p" => 8,
        "s16" | "s16p" => 16,
        "s32" | "s32p" | "flt" | "fltp" => 32,
        "s64" | "s64p" | "dbl" | "dblp" => 64,
        _ => return unsupported("audio sample format"),
    };
    out.push_str(&format!(", {sample_format}"));
    if let Some(raw) =
        integer(entries, "bits_per_raw_sample")?.filter(|raw| *raw > 0 && *raw != bits)
    {
        if raw > bits {
            return unsupported("audio sample depth");
        }
        out.push_str(&format!(" ({raw} bit)"));
    }
    Ok(())
}

fn dispositions(out: &mut String, entries: &Entries) -> Result<(), Failure> {
    const FLAGS: [(&str, &str); 19] = [
        ("default", "default"),
        ("dub", "dub"),
        ("original", "original"),
        ("comment", "comment"),
        ("lyrics", "lyrics"),
        ("karaoke", "karaoke"),
        ("forced", "forced"),
        ("hearing_impaired", "hearing impaired"),
        ("visual_impaired", "visual impaired"),
        ("clean_effects", "clean effects"),
        ("attached_pic", "attached pic"),
        ("timed_thumbnails", "timed thumbnails"),
        ("non_diegetic", "non-diegetic"),
        ("captions", "captions"),
        ("descriptions", "descriptions"),
        ("metadata", "metadata"),
        ("dependent", "dependent"),
        ("still_image", "still image"),
        ("multilayer", "multilayer"),
    ];
    for (key, _) in entries {
        if !FLAGS.iter().any(|(flag, _)| flag == key) {
            return unsupported("stream disposition");
        }
    }
    for (key, label) in FLAGS {
        match integer(entries, key)? {
            Some(1) => out.push_str(&format!(" ({label})")),
            Some(0) | None => {}
            _ => return unsupported("stream disposition value"),
        }
    }
    Ok(())
}

fn metadata(out: &mut String, tags: &[(String, String)], indent: usize) -> Result<(), Failure> {
    if let [(key, _)] = tags
        && key.eq_ignore_ascii_case("language")
    {
        return Ok(());
    }
    let mut heading = false;
    for (key, value) in tags {
        if key == "language" {
            continue;
        }
        if key.chars().any(|ch| ch.is_ascii_control()) || value.contains('\0') {
            return unsupported("metadata control characters");
        }
        if !heading {
            out.push_str(&format!("{:indent$}Metadata:\n", ""));
            heading = true;
        }
        let prefix = format!("{:width$}", "", width = indent + 2);
        for (line, text) in value.split('\n').enumerate() {
            let key = if line == 0 { key.as_str() } else { "" };
            let text = if text.len() > 255 {
                if text.chars().any(|ch| ch.is_ascii_control()) {
                    return unsupported("long metadata with control characters");
                }
                text.get(..255)
                    .ok_or_else(|| missing("metadata truncation splits UTF-8"))?
            } else {
                text
            };
            let text: String = text
                .chars()
                .filter_map(|ch| match ch {
                    '\u{8}' | '\u{b}' | '\u{c}' => None,
                    '\r' => Some(' '),
                    '\t' => Some(ch),
                    '\u{1}'..='\u{1f}' => Some('?'),
                    _ => Some(ch),
                })
                .collect();
            out.push_str(&format!(
                "{prefix}{key}{}: {text}\n",
                " ".repeat(16usize.saturating_sub(key.len()))
            ));
        }
    }
    Ok(())
}

fn side_data(out: &mut String, side: &SideData) -> Result<(), Failure> {
    let entries = &side.entries;
    match side.typ {
        "display_matrix" => {
            side_keys(entries, &["side_data_type", "displaymatrix", "rotation"])?;
            let matrix = required(entries, "displaymatrix")?;
            let mut values = Vec::new();
            for row in matrix.lines().filter(|row| !row.is_empty()) {
                let (_, row) = row
                    .split_once(':')
                    .ok_or_else(|| missing("display matrix"))?;
                for value in row.split_whitespace() {
                    let value: i32 = value.parse().map_err(|_| missing("display matrix"))?;
                    values.push(value);
                }
            }
            let matrix: &[i32; 9] = values
                .as_slice()
                .try_into()
                .map_err(|_| missing("display matrix dimensions"))?;
            let angle = match *matrix {
                [a, 0, _, 0, d, ..] if a != 0 && d != 0 => {
                    if a > 0 {
                        "-0.00"
                    } else {
                        "-180.00"
                    }
                }
                [0, b, _, c, 0, ..] if b != 0 && c != 0 => {
                    if b > 0 {
                        "-90.00"
                    } else {
                        "90.00"
                    }
                }
                _ => return unsupported("non-axis display matrix"),
            };
            out.push_str(&format!(
                "      Display Matrix: rotation of {angle} degrees\n"
            ));
        }
        "cpb_properties" => {
            side_keys(
                entries,
                &[
                    "side_data_type",
                    "max_bitrate",
                    "min_bitrate",
                    "avg_bitrate",
                    "buffer_size",
                    "vbv_delay",
                ],
            )?;
            let max = integer(entries, "max_bitrate")?.ok_or_else(|| missing("CPB maximum"))?;
            let min = integer(entries, "min_bitrate")?.ok_or_else(|| missing("CPB minimum"))?;
            let avg = integer(entries, "avg_bitrate")?.ok_or_else(|| missing("CPB average"))?;
            let buffer = integer(entries, "buffer_size")?.ok_or_else(|| missing("CPB buffer"))?;
            let delay = match integer(entries, "vbv_delay")? {
                Some(-1) => "N/A".to_owned(),
                Some(value) if value >= 0 => value.to_string(),
                _ => return unsupported("CPB delay"),
            };
            out.push_str(&format!("      CPB properties: bitrate max/min/avg: {max}/{min}/{avg} buffer size: {buffer} vbv_delay: {delay}\n"));
        }
        "stereo_3d" => {
            side_keys(
                entries,
                &[
                    "side_data_type",
                    "type",
                    "inverted",
                    "view",
                    "primary_eye",
                    "baseline",
                    "horizontal_disparity_adjustment",
                    "horizontal_field_of_view",
                ],
            )?;
            if integer(entries, "baseline")? != Some(0)
                || text(entries, "horizontal_disparity_adjustment") != Some("0/1")
                || text(entries, "horizontal_field_of_view") != Some("0/1")
            {
                return unsupported("stereo viewing geometry");
            }
            let typ = required(entries, "type")?;
            let view = required(entries, "view")?;
            let eye = required(entries, "primary_eye")?;
            if view != "packed"
                || eye != "none"
                || !matches!(
                    typ,
                    "2D" | "side by side"
                        | "top and bottom"
                        | "frame alternate"
                        | "checkerboard"
                        | "side by side (quincunx subsampling)"
                        | "interleaved lines"
                        | "interleaved columns"
                )
            {
                return unsupported("stereo type");
            }
            let inverted = match integer(entries, "inverted")? {
                Some(0) => "",
                Some(1) => " (inverted)",
                _ => return unsupported("stereo inversion"),
            };
            out.push_str(&format!(
                "      Stereo 3D: {typ}, view: {view}, primary eye: {eye}{inverted}\n"
            ));
        }
        "audio_service_type" => {
            side_keys(entries, &["side_data_type", "service_type"])?;
            let service = match integer(entries, "service_type")? {
                Some(0) => "main",
                Some(1) => "effects",
                Some(2) => "visually impaired",
                Some(3) => "hearing impaired",
                Some(4) => "dialogue",
                Some(5) => "commentary",
                Some(6) => "emergency",
                Some(7) => "voice over",
                Some(8) => "karaoke",
                _ => return unsupported("audio service type"),
            };
            out.push_str(&format!("      Audio Service Type: {service}\n"));
        }
        "content_light_level_metadata" => {
            side_keys(entries, &["side_data_type", "max_content", "max_average"])?;
            let content =
                integer(entries, "max_content")?.ok_or_else(|| missing("content light level"))?;
            let average =
                integer(entries, "max_average")?.ok_or_else(|| missing("average light level"))?;
            out.push_str(&format!(
                "      Content light level metadata: MaxCLL={content}, MaxFALL={average}\n"
            ));
        }
        "mastering_display_metadata" => {
            side_keys(
                entries,
                &[
                    "side_data_type",
                    "red_x",
                    "red_y",
                    "green_x",
                    "green_y",
                    "blue_x",
                    "blue_y",
                    "white_point_x",
                    "white_point_y",
                    "min_luminance",
                    "max_luminance",
                ],
            )?;
            let red_x = fraction(entries, "red_x")?;
            let red_y = fraction(entries, "red_y")?;
            let green_x = fraction(entries, "green_x")?;
            let green_y = fraction(entries, "green_y")?;
            let blue_x = fraction(entries, "blue_x")?;
            let blue_y = fraction(entries, "blue_y")?;
            let white_x = fraction(entries, "white_point_x")?;
            let white_y = fraction(entries, "white_point_y")?;
            let min = fraction(entries, "min_luminance")?;
            let max = fraction(entries, "max_luminance")?;
            out.push_str(&format!("      Mastering display metadata: has_primaries:1 has_luminance:1 r({red_x:.4},{red_y:.4}) g({green_x:.4},{green_y:.4}) b({blue_x:.4} {blue_y:.4}) wp({white_x:.4}, {white_y:.4}) min_luminance={min:.6}, max_luminance={max:.6}\n"));
        }
        "spherical_mapping" => {
            side_keys(
                entries,
                &["side_data_type", "projection", "yaw", "pitch", "roll"],
            )?;
            let projection = required(entries, "projection")?;
            if projection != "equirectangular"
                || ["yaw", "pitch", "roll"]
                    .into_iter()
                    .any(|key| !matches!(integer(entries, key), Ok(Some(0))))
            {
                return unsupported("spherical projection geometry");
            }
            out.push_str("      Spherical Mapping: equirectangular \n");
        }
        "ambient_viewing_environment" => {
            side_keys(
                entries,
                &[
                    "side_data_type",
                    "ambient_illuminance",
                    "ambient_light_x",
                    "ambient_light_y",
                ],
            )?;
            let illuminance = fraction(entries, "ambient_illuminance")?;
            let x = fraction(entries, "ambient_light_x")?;
            let y = fraction(entries, "ambient_light_y")?;
            out.push_str(&format!("      Ambient viewing environment: ambient_illuminance={illuminance:.6}, ambient_light_x={x:.6}, ambient_light_y={y:.6}\n"));
        }
        "dovi_configuration_record" => {
            side_keys(
                entries,
                &[
                    "side_data_type",
                    "dv_version_major",
                    "dv_version_minor",
                    "dv_profile",
                    "dv_level",
                    "rpu_present_flag",
                    "el_present_flag",
                    "bl_present_flag",
                    "dv_bl_signal_compatibility_id",
                    "dv_md_compression",
                ],
            )?;
            let major = integer(entries, "dv_version_major")?
                .ok_or_else(|| missing("Dolby Vision version"))?;
            let minor = integer(entries, "dv_version_minor")?
                .ok_or_else(|| missing("Dolby Vision version"))?;
            let profile =
                integer(entries, "dv_profile")?.ok_or_else(|| missing("Dolby Vision profile"))?;
            let level =
                integer(entries, "dv_level")?.ok_or_else(|| missing("Dolby Vision level"))?;
            let rpu =
                integer(entries, "rpu_present_flag")?.ok_or_else(|| missing("Dolby Vision RPU"))?;
            let el = integer(entries, "el_present_flag")?
                .ok_or_else(|| missing("Dolby Vision enhancement layer"))?;
            let bl = integer(entries, "bl_present_flag")?
                .ok_or_else(|| missing("Dolby Vision base layer"))?;
            let compatibility = integer(entries, "dv_bl_signal_compatibility_id")?
                .ok_or_else(|| missing("Dolby Vision compatibility"))?;
            if text(entries, "dv_md_compression") != Some("none") {
                return unsupported("Dolby Vision metadata compression");
            }
            out.push_str(&format!("      DOVI configuration record: version: {major}.{minor}, profile: {profile}, level: {level}, rpu flag: {rpu}, el flag: {el}, bl flag: {bl}, compatibility id: {compatibility}, compression: 0\n"));
        }
        _ => return unsupported(&format!("side data family {}", side.typ)),
    }
    Ok(())
}

fn side_keys(entries: &Entries, allowed: &[&str]) -> Result<(), Failure> {
    if entries.iter().any(|(key, _)| !allowed.contains(key)) {
        return unsupported("unrepresented side data fields");
    }
    Ok(())
}

fn fraction(entries: &Entries, key: &str) -> Result<f64, Failure> {
    let value = required(entries, key)?;
    let (num, den) = value.split_once('/').ok_or_else(|| missing(key))?;
    let num: u32 = num.parse().map_err(|_| missing(key))?;
    let den: u32 = den.parse().map_err(|_| missing(key))?;
    if den == 0 {
        return unsupported(key);
    }
    Ok(f64::from(num) / f64::from(den))
}

fn text<'a>(entries: &'a Entries, key: &str) -> Option<&'a str> {
    entries
        .iter()
        .find(|(name, _)| *name == key)
        .and_then(|(_, value)| match value {
            Value::Str(text) if text != "unknown" && text != "N/A" => Some(text.as_str()),
            _ => None,
        })
}

fn required<'a>(entries: &'a Entries, key: &str) -> Result<&'a str, Failure> {
    text(entries, key)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| missing(key))
}

fn integer(entries: &Entries, key: &str) -> Result<Option<i64>, Failure> {
    match entries
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value)
    {
        None | Some(Value::Na | Value::Unset(_)) => Ok(None),
        Some(Value::Int(value) | Value::Unit(value, _)) => Ok(Some(*value)),
        Some(Value::Str(value)) => value.parse().map(Some).map_err(|_| missing(key)),
        _ => unsupported(key),
    }
}

fn seconds(entries: &Entries, key: &str) -> Result<Option<f64>, Failure> {
    let value = match entries
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value)
    {
        None | Some(Value::Na | Value::Unset(_)) => return Ok(None),
        Some(Value::Time(value)) => *value,
        Some(Value::Str(value)) => value.parse().map_err(|_| missing(key))?,
        _ => return unsupported(key),
    };
    if !value.is_finite() {
        return unsupported(key);
    }
    Ok(Some(value))
}

fn ratio(entries: &Entries, key: &str) -> Result<Option<f64>, Failure> {
    let Some(value) = text(entries, key) else {
        return Ok(None);
    };
    let (num, den) = value.split_once('/').ok_or_else(|| missing(key))?;
    let num: u32 = num.parse().map_err(|_| missing(key))?;
    let den: u32 = den.parse().map_err(|_| missing(key))?;
    if num == 0 || den == 0 {
        return Ok(None);
    }
    Ok(Some(f64::from(num) / f64::from(den)))
}

fn rate_text(value: f64) -> Result<String, Failure> {
    if !value.is_finite() || value <= 0.0 || value > f64::from(u32::MAX) {
        return unsupported("video rate");
    }
    let hundredths = ((value * 100.0) as f32).round_ties_even() as u64;
    if hundredths == 0 {
        return Ok(format!("{value:.4}"));
    }
    if hundredths % 100_000 == 0 {
        return Ok(format!("{}k", hundredths / 100_000));
    }
    if hundredths % 100 == 0 {
        return Ok((hundredths / 100).to_string());
    }
    Ok(format!("{value:.2}"))
}

fn missing(field: &str) -> Failure {
    Failure::Unsupported(format!("input summary cannot represent {field}"))
}

fn unsupported<T>(field: &str) -> Result<T, Failure> {
    Err(missing(field))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::ffprobe::{Chapter, Program};

    fn report(format: &str) -> Report {
        Report {
            summary: SummaryEvidence::Complete {
                start: Some(0),
                duration: Some(400_000),
            },
            summary_aspects: vec![SummaryAspect::Complete {
                codec: Some((1, 1)),
            }],
            format: vec![
                ("format_name", Value::Str(format.to_owned())),
                ("bit_rate", Value::Int(241_980)),
            ],
            ..Report::default()
        }
    }

    fn video() -> Stream {
        Stream {
            kind: StreamKind::Video,
            attached_pic: false,
            entries: vec![
                ("index", Value::Int(0)),
                ("codec_name", Value::Str("h264".to_owned())),
                ("profile", Value::Str("High".to_owned())),
                ("codec_tag", Value::Str("0x31637661".to_owned())),
                ("codec_tag_string", Value::Str("avc1".to_owned())),
                ("id", Value::Str("0x1".to_owned())),
                ("pix_fmt", Value::Str("yuv420p".to_owned())),
                ("field_order", Value::Str("progressive".to_owned())),
                ("width", Value::Int(128)),
                ("height", Value::Int(96)),
                ("sample_aspect_ratio", Value::Str("1:1".to_owned())),
                ("display_aspect_ratio", Value::Str("4:3".to_owned())),
                ("bit_rate", Value::Int(130_540)),
                ("avg_frame_rate", Value::Str("25/1".to_owned())),
                ("r_frame_rate", Value::Str("25/1".to_owned())),
                ("time_base", Value::Str("1/12800".to_owned())),
                ("start_time", Value::Time(0.0)),
            ],
            disposition: vec![("default", Value::Int(1))],
            tags: vec![
                ("language".to_owned(), "und".to_owned()),
                ("handler_name".to_owned(), "VideoHandler".to_owned()),
                ("encoder".to_owned(), "Lavc63.1.102 libx264".to_owned()),
            ],
            side_data: Vec::new(),
        }
    }

    fn audio() -> Stream {
        Stream {
            kind: StreamKind::Audio,
            attached_pic: false,
            entries: vec![
                ("index", Value::Int(1)),
                ("codec_name", Value::Str("aac".to_owned())),
                ("profile", Value::Str("LC".to_owned())),
                ("codec_tag", Value::Str("0x6134706d".to_owned())),
                ("codec_tag_string", Value::Str("mp4a".to_owned())),
                ("id", Value::Str("0x2".to_owned())),
                ("sample_rate", Value::Str("48000".to_owned())),
                ("channel_layout", Value::Str("mono".to_owned())),
                ("channels", Value::Int(1)),
                ("sample_fmt", Value::Str("fltp".to_owned())),
                ("bit_rate", Value::Str("72018".to_owned())),
                ("start_time", Value::Time(0.0)),
            ],
            disposition: vec![("default", Value::Int(1))],
            tags: vec![
                ("language".to_owned(), "und".to_owned()),
                ("handler_name".to_owned(), "SoundHandler".to_owned()),
            ],
            side_data: Vec::new(),
        }
    }

    fn set(entries: &mut Entries, key: &'static str, value: Value) {
        entries.retain(|(name, _)| *name != key);
        entries.push((key, value));
    }

    #[test]
    fn mp4_summary_matches_the_pinned_default_info_output() {
        let mut report = report("mov,mp4,m4a,3gp,3g2,mj2");
        report.streams = vec![video(), audio()];
        report.format_tags = [
            ("major_brand", "isom"),
            ("minor_version", "512"),
            ("compatible_brands", "isomiso2avc1mp41"),
            ("title", "summary title"),
            ("encoder", "Lavf63.1.102"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
        let actual = render(&report, 0, "av.mp4")
            .map_err(|failure| failure.to_json())
            .unwrap();
        assert_eq!(
            String::from_utf8(actual).unwrap(),
            concat!(
                "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'av.mp4':\n",
                "  Metadata:\n",
                "    major_brand     : isom\n",
                "    minor_version   : 512\n",
                "    compatible_brands: isomiso2avc1mp41\n",
                "    title           : summary title\n",
                "    encoder         : Lavf63.1.102\n",
                "  Duration: 00:00:00.40, start: 0.000000, bitrate: 241 kb/s\n",
                "  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 128x96 [SAR 1:1 DAR 4:3], 130 kb/s, 25 fps, 25 tbr, 12800 tbn (default)\n",
                "    Metadata:\n",
                "      handler_name    : VideoHandler\n",
                "      encoder         : Lavc63.1.102 libx264\n",
                "  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, mono, fltp, 72 kb/s (default)\n",
                "    Metadata:\n",
                "      handler_name    : SoundHandler\n",
            )
        );
    }

    #[test]
    fn programs_repeat_members_and_leave_unassigned_streams_last() {
        let mut report = report("mpegts");
        let mut video = video();
        video.tags.clear();
        video.disposition.clear();
        set(
            &mut video.entries,
            "codec_tag",
            Value::Str("0x001b".to_owned()),
        );
        set(
            &mut video.entries,
            "codec_tag_string",
            Value::Str("[27][0][0][0]".to_owned()),
        );
        set(&mut video.entries, "id", Value::Str("0x100".to_owned()));
        set(
            &mut video.entries,
            "time_base",
            Value::Str("1/90000".to_owned()),
        );
        set(&mut video.entries, "bit_rate", Value::Na);
        set(&mut video.entries, "start_time", Value::Time(1.48));
        report.streams = vec![video, audio()];
        report.programs = [7, 8]
            .into_iter()
            .map(|id| Program {
                entries: vec![("program_id", Value::Int(id))],
                tags: Vec::new(),
                streams: vec![0],
            })
            .collect();
        let text = String::from_utf8(
            render(&report, 2, "av.ts")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        let line = "  Stream #2:0[0x100]: Video: h264 (High) ([27][0][0][0] / 0x001B), yuv420p(progressive), 128x96 [SAR 1:1 DAR 4:3], 25 fps, 25 tbr, 90k tbn, start 1.480000\n";
        assert_eq!(text.matches(line).count(), 2);
        assert!(text.contains(&format!(
            "  Program 7 \n{line}  Program 8 \n{line}  Stream #2:1"
        )));
    }

    #[test]
    fn programs_mark_unassigned_streams_without_deduplicating_memberships() {
        let mut report = report("mpegts");
        report.streams = vec![video(), audio()];
        report.programs.push(Program {
            entries: vec![("program_id", Value::Int(7))],
            tags: Vec::new(),
            streams: vec![0],
        });
        let text = String::from_utf8(
            render(&report, 0, "unassigned.ts")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert!(text.contains("  No Program\n  Stream #0:1"), "{text}");
        report.programs.first_mut().unwrap().streams.push(0);
        let text = String::from_utf8(
            render(&report, 0, "duplicate.ts")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert!(!text.contains("No Program"), "{text}");
        assert_eq!(text.matches("  Stream #0:0").count(), 2);
    }

    #[test]
    fn mixed_case_language_is_retained_beside_other_metadata() {
        let mut text = String::new();
        metadata(
            &mut text,
            &[
                ("LANGUAGE".to_owned(), "fra".to_owned()),
                ("title".to_owned(), "control".to_owned()),
            ],
            2,
        )
        .map_err(|failure| failure.to_json())
        .unwrap();
        assert_eq!(
            text,
            "  Metadata:\n    LANGUAGE        : fra\n    title           : control\n"
        );
    }

    #[test]
    fn metadata_preserves_order_multiline_values_and_chapter_indentation() {
        let mut report = report("matroska,webm");
        report.format_tags = vec![
            ("language".to_owned(), "eng".to_owned()),
            ("title".to_owned(), "a\tb\r\nc\rd\n".to_owned()),
            ("second".to_owned(), "two".to_owned()),
        ];
        report.chapters.push(Chapter {
            entries: vec![
                ("id", Value::Int(99)),
                ("start_time", Value::Time(0.0)),
                ("end_time", Value::Time(0.1)),
            ],
            tags: vec![("title".to_owned(), "Chapter A".to_owned())],
        });
        let text = String::from_utf8(
            render(&report, 3, "chapters.mkv")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            text,
            concat!(
                "Input #3, matroska,webm, from 'chapters.mkv':\n",
                "  Metadata:\n",
                "    title           : a\tb \n",
                "                    : c d\n",
                "                    : \n",
                "    second          : two\n",
                "  Duration: 00:00:00.40, start: 0.000000, bitrate: 241 kb/s\n",
                "  Chapters:\n",
                "    Chapter #3:0: start 0.000000, end 0.100000\n",
                "      Metadata:\n",
                "        title           : Chapter A\n",
            )
        );
    }

    #[test]
    fn duration_rounds_integer_microseconds_and_keeps_negative_starts() {
        let mut report = report("flac");
        report.summary = SummaryEvidence::Complete {
            start: Some(-1),
            duration: Some(3_599_995_000),
        };
        assert!(
            String::from_utf8(
                render(&report, 0, "audio.flac")
                    .map_err(|failure| failure.to_json())
                    .unwrap()
            )
            .unwrap()
            .contains("Duration: 01:00:00.00, start: -0.000001, bitrate: 241 kb/s\n")
        );
        report.summary = SummaryEvidence::Complete {
            start: None,
            duration: None,
        };
        set(&mut report.format, "bit_rate", Value::Na);
        assert!(
            String::from_utf8(
                render(&report, 0, "unknown.flac")
                    .map_err(|failure| failure.to_json())
                    .unwrap()
            )
            .unwrap()
            .ends_with("  Duration: N/A, bitrate: N/A\n")
        );
    }

    #[test]
    fn wav_audio_uses_minimum_four_uppercase_tag_digits_and_raw_depth() {
        let mut report = report("wav");
        let mut audio = audio();
        audio
            .entries
            .retain(|(key, _)| !matches!(*key, "profile" | "channel_layout" | "id" | "start_time"));
        audio.tags.clear();
        audio.disposition.clear();
        set(&mut audio.entries, "index", Value::Int(0));
        set(
            &mut audio.entries,
            "codec_name",
            Value::Str("pcm_s24le".to_owned()),
        );
        set(
            &mut audio.entries,
            "codec_tag",
            Value::Str("0x0001".to_owned()),
        );
        set(
            &mut audio.entries,
            "codec_tag_string",
            Value::Str("[1][0][0][0]".to_owned()),
        );
        set(
            &mut audio.entries,
            "sample_fmt",
            Value::Str("s32".to_owned()),
        );
        set(&mut audio.entries, "bits_per_raw_sample", Value::Int(24));
        set(&mut audio.entries, "bit_rate", Value::Int(1_152_000));
        report.streams.push(audio);
        assert!(String::from_utf8(render(&report, 0, "audio.wav").map_err(|failure| failure.to_json()).unwrap()).unwrap().contains("  Stream #0:0: Audio: pcm_s24le ([1][0][0][0] / 0x0001), 48000 Hz, 1 channels, s32 (24 bit), 1152 kb/s\n"));
    }

    #[test]
    fn supported_side_data_follows_metadata_in_native_order() {
        let mut report = report("mov,mp4,m4a,3gp,3g2,mj2");
        let mut video = video();
        video.side_data = vec![
            SideData {
                typ: "display_matrix",
                entries: vec![("rotation", Value::Int(90)), ("displaymatrix", Value::Str("\n00000000: 0 -65536 0\n00000001: 65536 0 0\n00000002: 0 0 1073741824\n".to_owned()))],
            },
            SideData {
                typ: "cpb_properties",
                entries: vec![
                    ("max_bitrate", Value::Int(0)),
                    ("min_bitrate", Value::Int(0)),
                    ("avg_bitrate", Value::Int(0)),
                    ("buffer_size", Value::Int(49152)),
                    ("vbv_delay", Value::Int(-1)),
                ],
            },
        ];
        report.streams.push(video);
        assert!(String::from_utf8(render(&report, 0, "side.mp4").map_err(|failure| failure.to_json()).unwrap()).unwrap().ends_with(concat!(
            "      encoder         : Lavc63.1.102 libx264\n",
            "    Side data:\n",
            "      Display Matrix: rotation of 90.00 degrees\n",
            "      CPB properties: bitrate max/min/avg: 0/0/0 buffer size: 49152 vbv_delay: N/A\n",
        )));
    }

    #[test]
    fn unknown_late_descriptions_and_side_data_return_no_summary() {
        let mut report = report("matroska,webm");
        report.streams = vec![video(), audio()];
        report.streams.last_mut().unwrap().side_data.push(SideData {
            typ: "unrepresented",
            entries: Vec::new(),
        });
        assert!(matches!(
            render(&report, 0, "late.mkv"),
            Err(Failure::Unsupported(_))
        ));
        report.streams.last_mut().unwrap().side_data.clear();
        set(
            &mut report.streams.last_mut().unwrap().entries,
            "codec_name",
            Value::Unset("unknown"),
        );
        assert!(matches!(
            render(&report, 0, "codec.mkv"),
            Err(Failure::Unsupported(_))
        ));
        report.summary = SummaryEvidence::Unverified;
        assert!(matches!(
            render(&report, 0, "unverified.mkv"),
            Err(Failure::Unsupported(_))
        ));
        report.summary = SummaryEvidence::Unsupported("missing source fact");
        assert!(matches!(
            render(&report, 0, "unsupported.mkv"),
            Err(Failure::Unsupported(_))
        ));
    }

    #[test]
    fn rates_keep_hundredths_and_kilo_units_at_rounding_boundaries() {
        for (rate, expected) in [
            (30.3, "30.30"),
            (27.5, "27.50"),
            (0.2, "0.20"),
            (3_000_000.0, "3000k"),
            (30.005, "30"),
            (30.006, "30.01"),
            (30.049, "30.05"),
            (29.995, "30"),
            (1.0 / 199.0, "0.01"),
            (1.0 / 200.0, "0.0050"),
            (1.0 / 201.0, "0.0050"),
            (1.0 / 256.0, "0.0039"),
        ] {
            assert_eq!(rate_text(rate).map_err(|f| f.to_json()).unwrap(), expected);
        }
        for rate in [
            0.0,
            -1.0,
            f64::NAN,
            f64::INFINITY,
            f64::from(u32::MAX) + 1.0,
        ] {
            assert!(rate_text(rate).is_err());
        }
    }

    #[test]
    fn metadata_truncates_each_line_but_refuses_split_utf8_or_controls() {
        let mut out = String::new();
        metadata(
            &mut out,
            &[(
                "title".to_owned(),
                format!("{}\n{}", "a".repeat(300), "b".repeat(300)),
            )],
            2,
        )
        .map_err(|f| f.to_json())
        .unwrap();
        assert_eq!(
            out,
            format!(
                "  Metadata:\n    title           : {}\n                    : {}\n",
                "a".repeat(255),
                "b".repeat(255)
            )
        );
        for value in [
            format!("{}é", "a".repeat(254)),
            format!("{}\r{}", "a".repeat(254), "z".repeat(10)),
        ] {
            assert!(metadata(&mut String::new(), &[("title".to_owned(), value)], 2).is_err());
        }
    }

    #[test]
    fn codec_and_stream_aspects_surround_the_stream_bitrate() {
        let mut report = report("mov,mp4,m4a,3gp,3g2,mj2");
        let mut stream = video();
        set(
            &mut stream.entries,
            "sample_aspect_ratio",
            Value::Str("4:3".to_owned()),
        );
        set(
            &mut stream.entries,
            "display_aspect_ratio",
            Value::Str("16:9".to_owned()),
        );
        report.streams.push(stream);
        let out = String::from_utf8(
            render(&report, 0, "aspects.mp4")
                .map_err(|f| f.to_json())
                .unwrap(),
        )
        .unwrap();
        assert!(
            out.contains("128x96 [SAR 1:1 DAR 4:3], 130 kb/s, SAR 4:3 DAR 16:9, 25 fps"),
            "{out}"
        );
    }

    #[test]
    fn vp8_omits_its_version_profile_and_language_suffix_ignores_key_case() {
        let mut report = report("matroska,webm");
        let mut stream = video();
        set(
            &mut stream.entries,
            "codec_name",
            Value::Str("vp8".to_owned()),
        );
        set(&mut stream.entries, "profile", Value::Str("0".to_owned()));
        stream.tags = vec![("LANGUAGE".to_owned(), "fra".to_owned())];
        report.streams.push(stream);
        let out = String::from_utf8(
            render(&report, 0, "vp8.webm")
                .map_err(|f| f.to_json())
                .unwrap(),
        )
        .unwrap();
        assert!(
            out.contains("Stream #0:0[0x1](fra): Video: vp8 (avc1"),
            "{out}"
        );
        assert!(!out.contains("vp8 (0)"), "{out}");
        assert!(!out.contains("Metadata:"), "{out}");
    }

    #[test]
    fn video_color_and_fractional_rates_use_default_info_spelling() {
        let mut report = report("mov,mp4,m4a,3gp,3g2,mj2");
        let mut video = video();
        set(
            &mut video.entries,
            "color_range",
            Value::Str("tv".to_owned()),
        );
        set(
            &mut video.entries,
            "color_space",
            Value::Str("bt709".to_owned()),
        );
        set(
            &mut video.entries,
            "avg_frame_rate",
            Value::Str("24000/1001".to_owned()),
        );
        set(
            &mut video.entries,
            "r_frame_rate",
            Value::Str("24000/1001".to_owned()),
        );
        set(
            &mut video.entries,
            "time_base",
            Value::Str("1/24000".to_owned()),
        );
        report.streams.push(video);
        let text = String::from_utf8(
            render(&report, 0, "colors.mp4")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert!(text.contains("yuv420p(tv, bt709/unknown/unknown, progressive)"));
        assert!(text.contains(", 23.98 fps, 23.98 tbr, 24k tbn (default)\n"));
    }

    #[test]
    fn decoded_video_aspect_is_distinct_from_parameter_aspect() {
        let mut report = report("matroska,webm");
        let mut video = video();
        video.tags.clear();
        video.disposition.clear();
        video.entries.retain(|(key, _)| *key != "id");
        for (key, value) in [
            ("codec_name", "vp9"),
            ("profile", "Profile 0"),
            ("codec_tag", "0x0000"),
            ("time_base", "1/1000"),
        ] {
            set(&mut video.entries, key, Value::Str(value.to_owned()));
        }
        set(&mut video.entries, "bit_rate", Value::Na);
        report.streams.push(video);
        report.summary_aspects = vec![SummaryAspect::Complete { codec: None }];
        let text = String::from_utf8(
            render(&report, 0, "vp9.webm")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            text,
            concat!(
                "Input #0, matroska,webm, from 'vp9.webm':\n",
                "  Duration: 00:00:00.40, start: 0.000000, bitrate: 241 kb/s\n",
                "  Stream #0:0: Video: vp9 (Profile 0), yuv420p(progressive), 128x96, SAR 1:1 DAR 4:3, 25 fps, 25 tbr, 1k tbn\n",
            )
        );
        report.summary_aspects.clear();
        assert!(matches!(
            render(&report, 0, "vp9.webm"),
            Err(Failure::Unsupported(_))
        ));
    }

    #[test]
    fn hdr_and_projection_side_data_keep_the_pinned_punctuation() {
        let mut report = report("matroska,webm");
        let mut video = video();
        video.side_data = vec![
            SideData {
                typ: "content_light_level_metadata",
                entries: vec![
                    ("max_content", Value::Int(1000)),
                    ("max_average", Value::Int(400)),
                ],
            },
            SideData {
                typ: "mastering_display_metadata",
                entries: [
                    ("red_x", "16/25"),
                    ("red_y", "33/100"),
                    ("green_x", "3/10"),
                    ("green_y", "3/5"),
                    ("blue_x", "3/20"),
                    ("blue_y", "3/50"),
                    ("white_point_x", "3127/10000"),
                    ("white_point_y", "329/1000"),
                    ("min_luminance", "1/200"),
                    ("max_luminance", "1000/1"),
                ]
                .into_iter()
                .map(|(key, value)| (key, Value::Str(value.to_owned())))
                .collect(),
            },
            SideData {
                typ: "spherical_mapping",
                entries: vec![
                    ("projection", Value::Str("equirectangular".to_owned())),
                    ("yaw", Value::Int(0)),
                    ("pitch", Value::Int(0)),
                    ("roll", Value::Int(0)),
                ],
            },
        ];
        report.streams.push(video);
        let text = String::from_utf8(
            render(&report, 0, "hdr.mkv")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert!(text.ends_with(concat!(
            "    Side data:\n",
            "      Content light level metadata: MaxCLL=1000, MaxFALL=400\n",
            "      Mastering display metadata: has_primaries:1 has_luminance:1 r(0.6400,0.3300) g(0.3000,0.6000) b(0.1500 0.0600) wp(0.3127, 0.3290) min_luminance=0.005000, max_luminance=1000.000000\n",
            "      Spherical Mapping: equirectangular \n",
        )));
    }

    #[test]
    fn fractional_rotation_uses_the_matrix_instead_of_the_integer_hint() {
        let mut report = report("mov,mp4,m4a,3gp,3g2,mj2");
        let mut video = video();
        video.side_data.push(SideData { typ: "display_matrix", entries: vec![("displaymatrix", Value::Str("\n00000000: 63982 -14184 0\n00000001: 14184 63982 0\n00000002: 0 0 1073741824\n".to_owned())), ("rotation", Value::Int(12))] });
        report.streams.push(video);
        assert!(matches!(
            render(&report, 0, "fractional.mp4"),
            Err(Failure::Unsupported(_))
        ));
    }

    #[test]
    fn display_matrix_summaries_admit_only_nondegenerate_axes() {
        for (linear, expected) in [
            ([65536, 0, 0, 65536], Some("-0.00")),
            ([0, -65536, 65536, 0], Some("90.00")),
            ([-65536, 0, 0, -65536], Some("-180.00")),
            ([0, 65536, -65536, 0], Some("-90.00")),
            ([32768, 0, 0, 131072], Some("-0.00")),
            ([63982, -14184, 14184, 63982], None),
            ([63983, -14184, 14184, 63982], None),
            ([127964, -28368, 28368, 127964], None),
            ([65536, 0, 1, 65536], None),
            ([1, 65536, -65536, 0], None),
            ([0, 0, 0, 0], None),
            ([65536, 0, 0, 0], None),
            ([0, 65536, 0, 0], None),
        ] {
            let [a, b, c, d] = linear;
            let side = SideData {
                typ: "display_matrix",
                entries: vec![(
                    "displaymatrix",
                    Value::Str(format!(
                        "\n00000000: {a} {b} 0\n00000001: {c} {d} 0\n00000002: 0 0 1073741824\n"
                    )),
                )],
            };
            let mut text = String::new();
            let result = side_data(&mut text, &side);
            if let Some(angle) = expected {
                assert!(result.is_ok(), "{linear:?}");
                assert_eq!(
                    text,
                    format!("      Display Matrix: rotation of {angle} degrees\n")
                );
            } else {
                assert!(matches!(result, Err(Failure::Unsupported(_))), "{linear:?}");
                assert!(text.is_empty(), "{linear:?}");
            }
        }
    }

    #[test]
    fn metadata_key_padding_counts_utf8_bytes() {
        let mut report = report("matroska,webm");
        report.format_tags = vec![("café".to_owned(), "unicode".to_owned())];
        let text = String::from_utf8(
            render(&report, 0, "unicode.mkv")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert!(text.contains("  Metadata:\n    café           : unicode\n"));
    }

    #[test]
    fn metadata_value_controls_follow_default_info_logging() {
        let mut report = report("matroska,webm");
        report.format_tags = vec![(
            "title".to_owned(),
            "a\u{1}b\u{7f}c\u{b}\u{c}\u{1b}z\u{85}".to_owned(),
        )];
        let text = String::from_utf8(
            render(&report, 0, "controls.mkv")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert!(text.contains("  Metadata:\n    title           : a?b\u{7f}c?z\u{85}\n"));
    }

    #[test]
    fn native_decoder_names_precede_codec_profiles() {
        let mut report = report("matroska,webm");
        let mut video = video();
        video.tags.clear();
        video.disposition.clear();
        video.entries.retain(|(key, _)| *key != "id");
        set(
            &mut video.entries,
            "codec_name",
            Value::Str("av1".to_owned()),
        );
        set(&mut video.entries, "profile", Value::Str("Main".to_owned()));
        set(
            &mut video.entries,
            "codec_tag",
            Value::Str("0x0000".to_owned()),
        );
        report.streams.push(video);
        let text = String::from_utf8(
            render(&report, 0, "av1.mkv")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert!(text.contains("  Stream #0:0: Video: av1 (libdav1d) (Main), yuv420p(progressive), 128x96 [SAR 1:1 DAR 4:3], 130 kb/s, 25 fps, 25 tbr, 12800 tbn\n"));
    }

    #[test]
    fn ambient_stereo_and_service_side_data_use_complete_lines() {
        let mut report = report("matroska,webm");
        let mut video = video();
        video.side_data = vec![
            SideData {
                typ: "ambient_viewing_environment",
                entries: [
                    ("ambient_illuminance", "123456/10000"),
                    ("ambient_light_x", "15635/50000"),
                    ("ambient_light_y", "16450/50000"),
                ]
                .into_iter()
                .map(|(key, value)| (key, Value::Str(value.to_owned())))
                .collect(),
            },
            SideData {
                typ: "stereo_3d",
                entries: vec![
                    ("type", Value::Str("side by side".to_owned())),
                    ("view", Value::Str("packed".to_owned())),
                    ("primary_eye", Value::Str("none".to_owned())),
                    ("inverted", Value::Int(1)),
                    ("baseline", Value::Int(0)),
                    (
                        "horizontal_disparity_adjustment",
                        Value::Str("0/1".to_owned()),
                    ),
                    ("horizontal_field_of_view", Value::Str("0/1".to_owned())),
                ],
            },
            SideData {
                typ: "audio_service_type",
                entries: vec![("service_type", Value::Int(8))],
            },
        ];
        report.streams.push(video);
        let text = String::from_utf8(
            render(&report, 0, "sides.mkv")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert!(text.ends_with(concat!(
            "    Side data:\n",
            "      Ambient viewing environment: ambient_illuminance=12.345600, ambient_light_x=0.312700, ambient_light_y=0.329000\n",
            "      Stereo 3D: side by side, view: packed, primary eye: none (inverted)\n",
            "      Audio Service Type: karaoke\n",
        )));
    }

    #[test]
    fn dolby_vision_requires_retained_compression_information() {
        let mut report = report("mov,mp4,m4a,3gp,3g2,mj2");
        let mut video = video();
        let entries = [
            ("dv_version_major", 1),
            ("dv_version_minor", 0),
            ("dv_profile", 9),
            ("dv_level", 4),
            ("rpu_present_flag", 1),
            ("el_present_flag", 0),
            ("bl_present_flag", 1),
            ("dv_bl_signal_compatibility_id", 0),
        ]
        .into_iter()
        .map(|(key, value)| (key, Value::Int(value)))
        .collect();
        video.side_data.push(SideData {
            typ: "dovi_configuration_record",
            entries,
        });
        report.streams.push(video);
        assert!(matches!(
            render(&report, 0, "dovi.mp4"),
            Err(Failure::Unsupported(_))
        ));
        report
            .streams
            .first_mut()
            .unwrap()
            .side_data
            .first_mut()
            .unwrap()
            .entries
            .push(("dv_md_compression", Value::Str("none".to_owned())));
        let text = String::from_utf8(
            render(&report, 0, "dovi.mp4")
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert!(text.ends_with("    Side data:\n      DOVI configuration record: version: 1.0, profile: 9, level: 4, rpu flag: 1, el flag: 0, bl flag: 1, compatibility id: 0, compression: 0\n"));
    }

    #[test]
    fn ac3_service_types_replace_the_default_disposition_when_required() {
        for (service, disposition) in [
            (0, "default"),
            (1, "clean effects"),
            (2, "visual impaired"),
            (3, "hearing impaired"),
            (4, "default"),
            (5, "comment"),
            (6, "default"),
            (7, "default"),
            (8, "karaoke"),
        ] {
            let mut report = report("mov,mp4,m4a,3gp,3g2,mj2");
            let mut audio = audio();
            set(
                &mut audio.entries,
                "codec_name",
                Value::Str("ac3".to_owned()),
            );
            set(&mut audio.entries, "profile", Value::Unset("unknown"));
            set(
                &mut audio.entries,
                "codec_tag",
                Value::Str("0x332d6361".to_owned()),
            );
            set(
                &mut audio.entries,
                "codec_tag_string",
                Value::Str("ac-3".to_owned()),
            );
            audio.side_data.push(SideData {
                typ: "audio_service_type",
                entries: vec![("service_type", Value::Int(service))],
            });
            report.streams.push(audio);
            let text = String::from_utf8(
                render(&report, 0, "service.mp4")
                    .map_err(|failure| failure.to_json())
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(text.lines().find(|line| line.starts_with("  Stream")), Some(format!("  Stream #0:1[0x2](und): Audio: ac3 (ac-3 / 0x332D6361), 48000 Hz, mono, fltp, 72 kb/s ({disposition})").as_str()));
        }
    }

    #[test]
    fn decoder_warnings_use_qualified_ids_and_original_stream_indexes() {
        let mut report = report("matroska,webm");
        for (index, codec, kind) in [
            (0, "bin_data", StreamKind::Data),
            (1, "ttf", StreamKind::Attachment),
            (2, "otf", StreamKind::Attachment),
        ] {
            let mut stream = audio();
            stream.kind = kind;
            set(&mut stream.entries, "index", Value::Int(index));
            set(
                &mut stream.entries,
                "codec_name",
                Value::Str(codec.to_owned()),
            );
            report.streams.push(stream);
        }
        let text = String::from_utf8(
            warnings(&report)
                .map_err(|failure| failure.to_json())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            text,
            concat!(
                "Unsupported codec with id 98314 for input stream 0\n",
                "Unsupported codec with id 98304 for input stream 1\n",
                "Unsupported codec with id 98310 for input stream 2\n",
            )
        );
        report.streams = vec![audio(), video()];
        assert!(
            warnings(&report)
                .map_err(|failure| failure.to_json())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unqualified_decoder_warnings_never_return_partial_output() {
        let mut report = report("matroska,webm");
        let mut stream = audio();
        stream.kind = StreamKind::Attachment;
        set(
            &mut stream.entries,
            "codec_name",
            Value::Str("ttf".to_owned()),
        );
        report.streams.push(stream);
        let mut unknown = audio();
        set(&mut unknown.entries, "codec_name", Value::Unset("unknown"));
        report.streams.push(unknown);
        assert!(matches!(warnings(&report), Err(Failure::Unsupported(_))));
        report.summary = SummaryEvidence::Unverified;
        assert!(matches!(warnings(&report), Err(Failure::Unsupported(_))));
    }
}
