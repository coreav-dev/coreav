//! Converts ffprobe and framemd5 output into the stable oracle schema.

use serde_json::{Map, Value, json};

/// Normalize a ffprobe streams document.
pub fn streams(file: &str, exit_code: i32, input: &str) -> Value {
    let parsed = parse_probe(input, exit_code);
    let Some(root) = parsed.as_ref() else {
        return empty_probe("streams", file);
    };
    let Some(input_streams) = root.get("streams").and_then(Value::as_array) else {
        return empty_probe("streams", file);
    };
    let streams = input_streams
        .iter()
        .map(normalize_stream)
        .collect::<Vec<_>>();
    let format_duration_us = root
        .get("format")
        .and_then(|format| format.get("duration"))
        .and_then(decimal_seconds_to_microseconds);
    json!({
        "schema": "coreav-oracle/streams/1",
        "file": file,
        "probe_ok": true,
        "format_duration_us": format_duration_us,
        "streams": streams
    })
}

/// Normalize a ffprobe packets document.
pub fn packets(file: &str, exit_code: i32, input: &str) -> Value {
    let parsed = parse_probe(input, exit_code);
    let Some(root) = parsed.as_ref() else {
        return empty_probe("packets", file);
    };
    let Some(input_streams) = root.get("streams").and_then(Value::as_array) else {
        return empty_probe("packets", file);
    };
    let input_packets = root
        .get("packets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let streams = input_streams
        .iter()
        .map(|stream| {
            let stream_index = integer(stream.get("index"));
            let packets = input_packets
                .iter()
                .filter(|packet| integer(packet.get("stream_index")) == stream_index)
                .map(normalize_packet)
                .collect::<Vec<_>>();
            json!({
                "index": integer(stream.get("index")),
                "time_base": parse_time_base(stream.get("time_base")),
                "packets": packets
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema": "coreav-oracle/packets/1",
        "file": file,
        "probe_ok": true,
        "streams": streams
    })
}

/// Normalize FFmpeg framemd5 text. Each stream-map entry maps output stream order
/// to its input stream index.
pub fn frames(file: &str, exit_code: i32, stream_map: &[i64], input: &str) -> Value {
    if exit_code != 0 {
        return empty_probe("frames", file);
    }
    let mut time_bases = Map::new();
    let mut frames_by_output: Vec<Vec<Value>> = stream_map.iter().map(|_| Vec::new()).collect();
    for line in input.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#tb ") {
            if let Some((output, value)) = rest.split_once(':') {
                if let (Ok(output), Some(time_base)) =
                    (output.trim().parse::<usize>(), parse_tb_text(value.trim()))
                {
                    time_bases.insert(output.to_string(), json!(time_base));
                }
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let columns = trimmed.split(',').map(str::trim).collect::<Vec<_>>();
        if columns.len() < 6 {
            continue;
        }
        let Ok(output) = columns.first().unwrap_or(&"").parse::<usize>() else {
            continue;
        };
        let Some(stream_frames) = frames_by_output.get_mut(output) else {
            continue;
        };
        let frame = json!({
            "dts": parse_i64(columns.get(1).copied()),
            "pts": parse_i64(columns.get(2).copied()),
            "duration": parse_i64(columns.get(3).copied()),
            "size": parse_i64(columns.get(4).copied()),
            "md5": columns.get(5).copied().filter(|value| !value.is_empty())
        });
        stream_frames.push(frame);
    }
    let streams = stream_map
        .iter()
        .enumerate()
        .map(|(output, index)| {
            json!({
                "index": index,
                "time_base": time_bases.get(&output.to_string()).cloned().unwrap_or(Value::Null),
                "frames": frames_by_output.get(output).cloned().unwrap_or_default()
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema": "coreav-oracle/frames/1",
        "file": file,
        "decode_ok": true,
        "streams": streams
    })
}

fn parse_probe(input: &str, exit_code: i32) -> Option<Value> {
    if exit_code != 0 {
        return None;
    }
    let parsed = serde_json::from_str::<Value>(input).ok()?;
    parsed.as_object()?;
    Some(parsed)
}

fn empty_probe(level: &str, file: &str) -> Value {
    match level {
        "streams" => json!({
            "schema": "coreav-oracle/streams/1",
            "file": file,
            "probe_ok": false,
            "format_duration_us": null,
            "streams": []
        }),
        "packets" => json!({
            "schema": "coreav-oracle/packets/1",
            "file": file,
            "probe_ok": false,
            "streams": []
        }),
        _ => json!({
            "schema": "coreav-oracle/frames/1",
            "file": file,
            "decode_ok": false,
            "streams": []
        }),
    }
}

fn normalize_stream(stream: &Value) -> Value {
    let kind = string(stream.get("codec_type"));
    let is_video = kind.as_deref() == Some("video");
    let tag = string(stream.get("codec_tag_string"));
    let fourcc = if stream.get("codec_tag").and_then(Value::as_str) == Some("0x0000") {
        None
    } else {
        tag.filter(|value| !value.is_empty() && value != "0x0000")
    };
    let rotation = display_rotation(stream, is_video);
    let time_base = parse_time_base(stream.get("time_base"));
    let timescale = time_base
        .as_ref()
        .filter(|values| values.first() == Some(&1))
        .and_then(|values| values.get(1).copied());
    let tags = stream.get("tags");
    json!({
        "index": integer(stream.get("index")),
        "kind": kind,
        "codec_fourcc": fourcc,
        "codec_name": string(stream.get("codec_name")),
        "width": integer(stream.get("width")),
        "height": integer(stream.get("height")),
        "rotation": rotation,
        "time_base": time_base,
        "timescale": timescale,
        "duration_ts": integer(stream.get("duration_ts")),
        "color_primaries": color(stream.get("color_primaries")),
        "color_transfer": color(stream.get("color_transfer")),
        "color_matrix": color(stream.get("color_space")),
        "color_range": color(stream.get("color_range")),
        "sample_rate": integer(stream.get("sample_rate")),
        "channels": integer(stream.get("channels")),
        "language": string(tags.and_then(|value| value.get("language"))),
        "sample_aspect_ratio": parse_ratio(stream.get("sample_aspect_ratio")),
        "display_aspect_ratio": parse_ratio(stream.get("display_aspect_ratio")),
        "handler_name": string(tags.and_then(|value| value.get("handler_name"))),
        "profile": string(stream.get("profile")).filter(|value| value != "unknown"),
        "level": integer(stream.get("level")).filter(|value| *value != -99),
        "bit_rate": integer(stream.get("bit_rate")),
        "creation_time": string(tags.and_then(|value| value.get("creation_time")))
    })
}

fn normalize_packet(packet: &Value) -> Value {
    let flags = string(packet.get("flags")).unwrap_or_default();
    json!({
        "pos": integer(packet.get("pos")),
        "pts": integer(packet.get("pts")),
        "dts": integer(packet.get("dts")),
        "size": integer(packet.get("size")),
        "keyframe": flags.contains('K'),
        "discard": flags.contains('D'),
        "duration": integer(packet.get("duration"))
    })
}

fn display_rotation(stream: &Value, is_video: bool) -> Option<i64> {
    if !is_video {
        return None;
    }
    let entries = stream.get("side_data_list").and_then(Value::as_array);
    let Some(entries) = entries else {
        return Some(0);
    };
    for entry in entries {
        if string(entry.get("side_data_type")).as_deref() == Some("Display Matrix") {
            let raw = entry.get("rotation")?;
            let rotation = raw.as_i64().or_else(|| {
                raw.as_f64()
                    .filter(|value| value.fract() == 0.0)
                    .map(|value| value as i64)
            });
            return rotation.map(|value| value.rem_euclid(360));
        }
    }
    Some(0)
}

fn parse_time_base(value: Option<&Value>) -> Option<Vec<i64>> {
    let text = value?.as_str()?;
    let (numerator, denominator) = text.split_once('/')?;
    let numerator = numerator.trim().parse::<i64>().ok()?;
    let denominator = denominator.trim().parse::<i64>().ok()?;
    if numerator <= 0 || denominator <= 0 {
        return None;
    }
    Some(vec![numerator, denominator])
}

fn parse_tb_text(text: &str) -> Option<Vec<i64>> {
    parse_time_base(Some(&Value::String(text.to_owned())))
}

fn parse_ratio(value: Option<&Value>) -> Option<Vec<i64>> {
    let text = value?.as_str()?;
    let (numerator, denominator) = text.split_once(':')?;
    let numerator = numerator.trim().parse::<i64>().ok()?;
    let denominator = denominator.trim().parse::<i64>().ok()?;
    if numerator <= 0 || denominator <= 0 {
        return None;
    }
    Some(vec![numerator, denominator])
}

fn decimal_seconds_to_microseconds(value: &Value) -> Option<i64> {
    let text = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_number().map(|number| number.to_string()))?;
    let text = text.trim();
    let (negative, unsigned) = match text.chars().next() {
        Some('-') => (true, text.get(1..)?),
        Some('+') => (false, text.get(1..)?),
        _ => (false, text),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.chars().all(|character| character.is_ascii_digit())
        || !fraction.chars().all(|character| character.is_ascii_digit())
    {
        return None;
    }
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<i128>().ok()?
    };
    let base = whole.checked_mul(1_000_000)?;
    let mut micros_fraction = 0_i128;
    for (position, character) in fraction.chars().take(6).enumerate() {
        let digit = character.to_digit(10)? as i128;
        let position = u32::try_from(position).ok()?;
        let place = 10_i128.checked_pow(5_u32.checked_sub(position)?)?;
        micros_fraction = micros_fraction.checked_add(digit.checked_mul(place)?)?;
    }
    let round_digit = fraction
        .chars()
        .nth(6)
        .and_then(|character| character.to_digit(10));
    let rounded_fraction =
        micros_fraction.checked_add(if round_digit.unwrap_or(0) >= 5 { 1 } else { 0 })?;
    let absolute = base.checked_add(rounded_fraction)?;
    let signed = if negative {
        absolute.checked_neg()?
    } else {
        absolute
    };
    i64::try_from(signed).ok()
}

fn integer(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| parse_i64(Some(text))))
}

fn parse_i64(value: Option<&str>) -> Option<i64> {
    let text = value?.trim();
    if text.is_empty() || text == "N/A" {
        None
    } else {
        text.parse::<i64>().ok()
    }
}

fn string(value: Option<&Value>) -> Option<String> {
    value?.as_str().map(str::to_owned)
}

fn color(value: Option<&Value>) -> Option<String> {
    string(value).filter(|value| value != "unknown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_normalize_documented_fields() {
        let source = json!({
            "format": {"duration": "2.000000"},
            "streams": [{
                "index": 0, "codec_type": "video", "codec_tag": "0x0000",
                "codec_tag_string": "[0][0][0][0]", "codec_name": "h264",
                "width": 320, "height": 240, "time_base": "1/12800",
                "duration_ts": "25600", "color_primaries": "unknown",
                "side_data_list": [{"side_data_type": "Display Matrix", "rotation": -90}],
                "sample_rate": "44100", "channels": 2, "tags": {"language": "eng"},
                "sample_aspect_ratio": "1:1", "display_aspect_ratio": "4:3"
            }]
        });
        let normalized = streams("sample.mp4", 0, &source.to_string());
        assert_eq!(normalized["format_duration_us"], 2_000_000);
        assert_eq!(normalized["streams"][0]["rotation"], 270);
        assert_eq!(normalized["streams"][0]["codec_fourcc"], Value::Null);
        assert_eq!(normalized["streams"][0]["color_primaries"], Value::Null);
        assert_eq!(normalized["streams"][0]["timescale"], 12_800);
    }

    #[test]
    fn stream_without_display_matrix_has_zero_rotation() {
        let normalized = streams("v.mp4", 0, r#"{"streams":[{"codec_type":"video"}]}"#);
        assert_eq!(normalized["streams"][0]["rotation"], 0);
    }

    #[test]
    fn packet_strings_and_flags_are_normalized() {
        let normalized = packets(
            "x.mp4",
            0,
            r#"{"streams":[{"index":0,"time_base":"1/10"}],"packets":[{"stream_index":0,"pos":"N/A","pts":"1","dts":null,"size":"4","flags":"KD_"}]}"#,
        );
        assert_eq!(normalized["streams"][0]["packets"][0]["pos"], Value::Null);
        assert!(
            normalized["streams"][0]["packets"][0]["keyframe"]
                .as_bool()
                .unwrap_or(false)
        );
        assert!(
            normalized["streams"][0]["packets"][0]["discard"]
                .as_bool()
                .unwrap_or(false)
        );
    }

    #[test]
    fn packets_are_grouped_by_input_stream_and_preserve_order() {
        let normalized = packets(
            "multi.mp4",
            0,
            r#"{"streams":[{"index":0,"time_base":"1/10"},{"index":1,"time_base":"1/20"}],"packets":[{"stream_index":1,"pos":"2","pts":2,"dts":1,"size":"4","flags":"_","duration":1},{"stream_index":0,"pos":"1","pts":3,"dts":2,"size":"5","flags":"K","duration":1},{"stream_index":1,"pos":"3","pts":4,"dts":3,"size":"6","flags":"_","duration":1}]}"#,
        );
        assert_eq!(
            normalized["streams"][0]["packets"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(normalized["streams"][1]["packets"][0]["pos"], 2);
        assert_eq!(normalized["streams"][1]["packets"][1]["pos"], 3);
    }

    #[test]
    fn failed_or_unparsable_probe_yields_failure_document() {
        let failed = streams("broken.mp4", 1, r#"{"streams":[]}"#);
        assert!(!failed["probe_ok"].as_bool().unwrap_or(true));
        assert_eq!(failed["streams"], json!([]));
        let malformed = packets("broken.mp4", 0, "not json");
        assert!(!malformed["probe_ok"].as_bool().unwrap_or(true));
        assert_eq!(malformed["streams"], json!([]));
    }

    #[test]
    fn duration_rounds_half_away_from_zero() {
        assert_eq!(
            decimal_seconds_to_microseconds(&json!("1.0000005")),
            Some(1_000_001)
        );
        assert_eq!(
            decimal_seconds_to_microseconds(&json!("-1.0000005")),
            Some(-1_000_001)
        );
    }

    #[test]
    fn framemd5_maps_streams_and_rows() {
        let normalized = frames(
            "a.mp4",
            0,
            &[2],
            "#tb 0: 1/25\n#stream#, dts, pts, duration, size, hash\n0, 0, 0, 1, 4, abcdef\n",
        );
        assert_eq!(normalized["streams"][0]["index"], 2);
        assert_eq!(normalized["streams"][0]["time_base"], json!([1, 25]));
        assert_eq!(normalized["streams"][0]["frames"][0]["md5"], "abcdef");
    }
}
