//! Mutation-based checks that verify gating and informational comparison policy.

use std::path::Path;

use serde_json::{Value, json};

use crate::compare::{self, FileComparison, Level};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Expected {
    Gating,
    Informational,
    MustPass,
}

#[derive(Clone, Copy)]
struct Perturbation {
    name: &'static str,
    level: Level,
    expected: Expected,
    mutate: fn(&mut Value) -> bool,
}

/// Run every specified mutation against each applicable reference file.
pub fn run_selfcheck(reference_dir: &Path, candidate_dir: Option<&Path>) -> Result<bool, String> {
    let levels = [Level::Streams, Level::Packets, Level::Frames];
    if let Some(candidate_dir) = candidate_dir {
        let comparisons =
            compare::compare_directories(reference_dir, candidate_dir, &levels, "fixtures")?;
        for comparison in &comparisons {
            compare::print_summary(std::slice::from_ref(comparison), 20);
        }
        if comparisons.iter().any(|comparison| !comparison.passed) {
            return Ok(false);
        }
    }

    let perturbations = perturbations();
    let mut overall_passed = true;
    println!("selfcheck perturbation | files tested | detected | result");
    for perturbation in perturbations {
        let paths = compare::level_files(reference_dir, perturbation.level)?;
        let mut tested = 0_u64;
        let mut detected = 0_u64;
        for path in paths {
            let reference = compare::read_document(&path)?;
            compare::validate_document(perturbation.level, &reference)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if !reference_status_ok(perturbation.level, &reference) {
                continue;
            }
            let mut candidate = reference.clone();
            if !(perturbation.mutate)(&mut candidate) {
                continue;
            }
            tested += 1;
            let comparison =
                compare::compare_file(perturbation.level, &reference, Some(&candidate))?;
            if expected_detected(perturbation.expected, &comparison) {
                detected += 1;
            }
        }
        let passed = tested > 0 && tested == detected;
        if !passed {
            overall_passed = false;
        }
        println!(
            "{} | {} | {} | {}",
            perturbation.name,
            tested,
            detected,
            if passed { "PASS" } else { "FAIL" }
        );
    }
    Ok(overall_passed)
}

fn expected_detected(expected: Expected, comparison: &FileComparison) -> bool {
    match expected {
        Expected::Gating => !comparison.gating_differences.is_empty(),
        Expected::Informational => {
            comparison.gating_differences.is_empty()
                && !comparison.informational_differences.is_empty()
        }
        Expected::MustPass => {
            comparison.gating_differences.is_empty()
                && comparison.informational_differences.is_empty()
        }
    }
}

fn perturbations() -> Vec<Perturbation> {
    vec![
        gating("streams.probe_ok=false", Level::Streams, set_probe_false),
        gating("streams.drop_last_stream", Level::Streams, drop_last_stream),
        gating("streams.kind", Level::Streams, |value| {
            set_stream_string(value, "kind", "perturbed")
        }),
        gating("streams.codec_fourcc", Level::Streams, |value| {
            set_stream_string(value, "codec_fourcc", "zzzz")
        }),
        gating("streams.codec_name", Level::Streams, |value| {
            set_stream_string(value, "codec_name", "perturbed")
        }),
        gating("streams.width+1", Level::Streams, |value| {
            increment_stream_integer(value, "width")
        }),
        gating("streams.height+1", Level::Streams, |value| {
            increment_stream_integer(value, "height")
        }),
        gating("streams.rotation+90", Level::Streams, perturb_rotation),
        gating("streams.timescale+1", Level::Streams, |value| {
            increment_stream_integer(value, "timescale")
        }),
        gating("streams.duration_ts+1", Level::Streams, |value| {
            increment_stream_integer(value, "duration_ts")
        }),
        gating(
            "streams.format_duration_us+1",
            Level::Streams,
            increment_format_duration,
        ),
        gating("streams.color_primaries=null", Level::Streams, |value| {
            clear_non_null_stream_field(value, "color_primaries")
        }),
        gating("streams.color_transfer=null", Level::Streams, |value| {
            clear_non_null_stream_field(value, "color_transfer")
        }),
        gating("streams.color_matrix=null", Level::Streams, |value| {
            clear_non_null_stream_field(value, "color_matrix")
        }),
        gating("streams.color_range=null", Level::Streams, |value| {
            clear_non_null_stream_field(value, "color_range")
        }),
        informational("streams.sample_rate+1", Level::Streams, |value| {
            increment_stream_integer(value, "sample_rate")
        }),
        informational("streams.channels+1", Level::Streams, |value| {
            increment_stream_integer(value, "channels")
        }),
        informational("streams.language=zzz", Level::Streams, perturb_language),
        informational("streams.sample_aspect_ratio", Level::Streams, |value| {
            set_stream_value(value, "sample_aspect_ratio", json!([99, 101]))
        }),
        informational("streams.display_aspect_ratio", Level::Streams, |value| {
            set_stream_value(value, "display_aspect_ratio", json!([99, 101]))
        }),
        informational("streams.handler_name", Level::Streams, |value| {
            set_stream_value(value, "handler_name", json!("perturbed"))
        }),
        gating("streams.profile=null", Level::Streams, |value| {
            clear_non_null_stream_field(value, "profile")
        }),
        gating("streams.level=null", Level::Streams, |value| {
            clear_non_null_stream_field(value, "level")
        }),
        informational("streams.bit_rate+1", Level::Streams, |value| {
            increment_stream_integer(value, "bit_rate")
        }),
        gating("packets.probe_ok=false", Level::Packets, set_probe_false),
        gating("packets.drop_last_stream", Level::Packets, drop_last_stream),
        gating("packets.pts+1", Level::Packets, |value| {
            increment_packet_field(value, "pts")
        }),
        gating("packets.dts+1", Level::Packets, |value| {
            increment_packet_field(value, "dts")
        }),
        gating("packets.size+1", Level::Packets, |value| {
            increment_packet_field(value, "size")
        }),
        gating(
            "packets.keyframe-toggle",
            Level::Packets,
            toggle_packet_keyframe,
        ),
        gating("packets.remove_one", Level::Packets, remove_packet),
        gating("packets.add_extra", Level::Packets, add_packet),
        informational("packets.duration+1", Level::Packets, |value| {
            increment_packet_field(value, "duration")
        }),
        informational(
            "packets.discard-toggle",
            Level::Packets,
            toggle_packet_discard,
        ),
        Perturbation {
            name: "packets.time_base_rescale",
            level: Level::Packets,
            expected: Expected::MustPass,
            mutate: rescale_packets,
        },
        gating("frames.decode_ok=false", Level::Frames, set_decode_false),
        gating("frames.md5-change", Level::Frames, change_frame_md5),
        gating("frames.size+1", Level::Frames, |value| {
            increment_frame_field(value, "size")
        }),
        gating("frames.pts+1", Level::Frames, |value| {
            increment_frame_field(value, "pts")
        }),
        gating("frames.remove_one", Level::Frames, remove_frame),
        gating("frames.add_one", Level::Frames, add_frame),
        informational("frames.dts+1", Level::Frames, |value| {
            increment_frame_field(value, "dts")
        }),
        informational("frames.duration+1", Level::Frames, |value| {
            increment_frame_field(value, "duration")
        }),
        Perturbation {
            name: "frames.time_base_rescale",
            level: Level::Frames,
            expected: Expected::MustPass,
            mutate: rescale_frames,
        },
    ]
}

fn gating(name: &'static str, level: Level, mutate: fn(&mut Value) -> bool) -> Perturbation {
    Perturbation {
        name,
        level,
        expected: Expected::Gating,
        mutate,
    }
}

fn informational(name: &'static str, level: Level, mutate: fn(&mut Value) -> bool) -> Perturbation {
    Perturbation {
        name,
        level,
        expected: Expected::Informational,
        mutate,
    }
}

fn reference_status_ok(level: Level, value: &Value) -> bool {
    let key = if level == Level::Frames {
        "decode_ok"
    } else {
        "probe_ok"
    };
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn set_probe_false(value: &mut Value) -> bool {
    set_top_value(value, "probe_ok", json!(false))
}

fn set_decode_false(value: &mut Value) -> bool {
    set_top_value(value, "decode_ok", json!(false))
}

fn drop_last_stream(value: &mut Value) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    streams.pop().is_some()
}

fn set_stream_string(value: &mut Value, field: &str, replacement: &str) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams {
        if stream
            .get(field)
            .is_some_and(|field_value| !field_value.is_null())
        {
            return set_object_value(stream, field, json!(replacement));
        }
    }
    false
}

fn set_stream_value(value: &mut Value, field: &str, replacement: Value) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    let Some(stream) = streams.first_mut() else {
        return false;
    };
    set_object_value(stream, field, replacement)
}

fn increment_stream_integer(value: &mut Value, field: &str) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams {
        let Some(current) = stream.get(field).and_then(Value::as_i64) else {
            continue;
        };
        let Some(next) = current.checked_add(1) else {
            continue;
        };
        return set_object_value(stream, field, json!(next));
    }
    false
}

fn clear_non_null_stream_field(value: &mut Value, field: &str) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams {
        if stream.get(field).is_some_and(|value| !value.is_null()) {
            return set_object_value(stream, field, Value::Null);
        }
    }
    false
}

fn perturb_rotation(value: &mut Value) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams.iter_mut() {
        if let Some(rotation) = stream.get("rotation").and_then(Value::as_i64) {
            return set_object_value(
                stream,
                "rotation",
                json!((rotation.rem_euclid(360) + 90).rem_euclid(360)),
            );
        }
    }
    if let Some(stream) = streams.first_mut() {
        return set_object_value(stream, "rotation", json!(90));
    }
    false
}

fn increment_format_duration(value: &mut Value) -> bool {
    let Some(current) = value.get("format_duration_us").and_then(Value::as_i64) else {
        return false;
    };
    let Some(next) = current.checked_add(1) else {
        return false;
    };
    set_top_value(value, "format_duration_us", json!(next))
}

fn perturb_language(value: &mut Value) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    let Some(stream) = streams.first_mut() else {
        return false;
    };
    set_object_value(stream, "language", json!("zzz"))
}

fn increment_packet_field(value: &mut Value, field: &str) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams {
        let Some(packets) = stream.get_mut("packets").and_then(Value::as_array_mut) else {
            continue;
        };
        for packet in packets {
            let Some(current) = packet.get(field).and_then(Value::as_i64) else {
                continue;
            };
            let Some(next) = current.checked_add(1) else {
                continue;
            };
            return set_object_value(packet, field, json!(next));
        }
    }
    false
}

fn toggle_packet_keyframe(value: &mut Value) -> bool {
    toggle_first_boolean(value, "packets", "keyframe")
}

fn toggle_packet_discard(value: &mut Value) -> bool {
    toggle_first_boolean(value, "packets", "discard")
}

fn toggle_first_boolean(value: &mut Value, list: &str, field: &str) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams {
        let Some(items) = stream.get_mut(list).and_then(Value::as_array_mut) else {
            continue;
        };
        for item in items {
            let Some(current) = item.get(field).and_then(Value::as_bool) else {
                continue;
            };
            return set_object_value(item, field, json!(!current));
        }
    }
    false
}

fn remove_packet(value: &mut Value) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams {
        if let Some(packets) = stream.get_mut("packets").and_then(Value::as_array_mut) {
            if !packets.is_empty() {
                packets.remove(0);
                return true;
            }
        }
    }
    false
}

fn add_packet(value: &mut Value) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams {
        let Some(packets) = stream.get_mut("packets").and_then(Value::as_array_mut) else {
            continue;
        };
        let max_pos = packets
            .iter()
            .filter_map(|packet| packet.get("pos").and_then(Value::as_i64))
            .max()
            .unwrap_or(-1);
        let Some(position) = max_pos.checked_add(1) else {
            continue;
        };
        let packet = packets.last().cloned().unwrap_or_else(|| {
            json!({
                "pos": null, "pts": null, "dts": null, "size": 0,
                "keyframe": false, "discard": false, "duration": null
            })
        });
        let mut packet = packet;
        if !set_object_value(&mut packet, "pos", json!(position)) {
            continue;
        }
        packets.push(packet);
        return true;
    }
    false
}

fn rescale_packets(value: &mut Value) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    for stream in streams {
        let Some((numerator, denominator)) = time_base(stream.get("time_base")) else {
            continue;
        };
        let Some(new_denominator) = denominator.checked_mul(2) else {
            continue;
        };
        if let Some(packets) = stream.get_mut("packets").and_then(Value::as_array_mut) {
            for packet in packets {
                for field in ["pts", "dts", "duration"] {
                    if packet.get(field).and_then(Value::as_i64).is_some() {
                        let Some(current) = packet.get(field).and_then(Value::as_i64) else {
                            continue;
                        };
                        let Some(scaled) = current.checked_mul(2) else {
                            return false;
                        };
                        let _ = set_object_value(packet, field, json!(scaled));
                    }
                }
            }
        }
        if set_object_value(stream, "time_base", json!([numerator, new_denominator])) {
            changed = true;
        }
    }
    changed
}

fn increment_frame_field(value: &mut Value, field: &str) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams {
        let Some(frames) = stream.get_mut("frames").and_then(Value::as_array_mut) else {
            continue;
        };
        for frame in frames {
            let Some(current) = frame.get(field).and_then(Value::as_i64) else {
                continue;
            };
            let Some(next) = current.checked_add(1) else {
                continue;
            };
            return set_object_value(frame, field, json!(next));
        }
    }
    false
}

fn change_frame_md5(value: &mut Value) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams {
        let Some(frames) = stream.get_mut("frames").and_then(Value::as_array_mut) else {
            continue;
        };
        if let Some(frame) = frames.first_mut() {
            let replacement = if frame.get("md5").and_then(Value::as_str) == Some("perturbed") {
                "changed"
            } else {
                "perturbed"
            };
            return set_object_value(frame, "md5", json!(replacement));
        }
    }
    false
}

fn remove_frame(value: &mut Value) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams {
        if let Some(frames) = stream.get_mut("frames").and_then(Value::as_array_mut) {
            if !frames.is_empty() {
                frames.remove(0);
                return true;
            }
        }
    }
    false
}

fn add_frame(value: &mut Value) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    for stream in streams {
        let Some(frames) = stream.get_mut("frames").and_then(Value::as_array_mut) else {
            continue;
        };
        let frame = frames.last().cloned().unwrap_or_else(|| {
            json!({
                "pts": 0, "dts": 0, "duration": 1, "size": 0, "md5": "extra"
            })
        });
        frames.push(frame);
        return true;
    }
    false
}

fn rescale_frames(value: &mut Value) -> bool {
    let Some(streams) = value.get_mut("streams").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    for stream in streams {
        let Some((numerator, denominator)) = time_base(stream.get("time_base")) else {
            continue;
        };
        let Some(new_denominator) = denominator.checked_mul(2) else {
            continue;
        };
        let Some(frames) = stream.get_mut("frames").and_then(Value::as_array_mut) else {
            continue;
        };
        for frame in frames {
            for field in ["pts", "dts", "duration"] {
                if let Some(current) = frame.get(field).and_then(Value::as_i64) {
                    let Some(scaled) = current.checked_mul(2) else {
                        return false;
                    };
                    let _ = set_object_value(frame, field, json!(scaled));
                }
            }
        }
        if set_object_value(stream, "time_base", json!([numerator, new_denominator])) {
            changed = true;
        }
    }
    changed
}

fn time_base(value: Option<&Value>) -> Option<(i64, i64)> {
    let values = value?.as_array()?;
    let numerator = values.first()?.as_i64()?;
    let denominator = values.get(1)?.as_i64()?;
    if numerator <= 0 || denominator <= 0 {
        return None;
    }
    Some((numerator, denominator))
}

fn set_top_value(value: &mut Value, key: &str, replacement: Value) -> bool {
    set_object_value(value, key, replacement)
}

fn set_object_value(value: &mut Value, key: &str, replacement: Value) -> bool {
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    let Some(current) = object.get_mut(key) else {
        return false;
    };
    *current = replacement;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet_document() -> Value {
        json!({
            "schema":"coreav-oracle/packets/1","file":"a.mp4","probe_ok":true,
            "streams":[{"index":0,"time_base":[1,10],"packets":[
                {"pos":1,"pts":2,"dts":1,"size":4,"keyframe":true,"discard":false,"duration":1}
            ]}]
        })
    }

    fn frame_document() -> Value {
        json!({
            "schema":"coreav-oracle/frames/1","file":"a.mp4","decode_ok":true,
            "streams":[{"index":0,"time_base":[1,10],"frames":[
                {"pts":2,"dts":1,"duration":1,"size":4,"md5":"abc"}
            ]}]
        })
    }

    #[test]
    fn rescaling_time_bases_preserves_rational_timestamps() {
        let original = packet_document();
        let mut rescaled = original.clone();
        assert!(rescale_packets(&mut rescaled));
        let result =
            compare::compare_file(Level::Packets, &original, Some(&rescaled)).expect("comparison");
        assert!(result.agrees());
        assert!(result.informational_differences.is_empty());

        let original = frame_document();
        let mut rescaled = original.clone();
        assert!(rescale_frames(&mut rescaled));
        let result =
            compare::compare_file(Level::Frames, &original, Some(&rescaled)).expect("comparison");
        assert!(result.agrees());
        assert!(result.informational_differences.is_empty());
    }

    #[test]
    fn all_synthetic_perturbations_have_expected_effects() {
        let streams = json!({
            "schema":"coreav-oracle/streams/1","file":"s.mp4","probe_ok":true,"format_duration_us":1,
            "streams":[{
                "index":0,"kind":"video","codec_fourcc":"a","codec_name":"v","width":10,"height":10,
                "rotation":0,"time_base":[1,10],"timescale":10,"duration_ts":10,
                "color_primaries":"bt709","color_transfer":"bt709","color_matrix":"bt709","color_range":"tv",
                "sample_rate":48000,"channels":2,"language":"eng","sample_aspect_ratio":[1,1],
                "display_aspect_ratio":[1,1],"handler_name":"handler",
                "profile":"High","level":31,"bit_rate":1000,"creation_time":null
            }]
        });
        let packets = packet_document();
        let frames = frame_document();
        for perturbation in perturbations() {
            let original = match perturbation.level {
                Level::Streams => &streams,
                Level::Packets => &packets,
                Level::Frames => &frames,
            };
            let mut candidate = original.clone();
            if !(perturbation.mutate)(&mut candidate) {
                panic!(
                    "synthetic perturbation did not apply: {}",
                    perturbation.name
                );
            }
            let comparison = compare::compare_file(perturbation.level, original, Some(&candidate))
                .expect("comparison");
            assert!(
                expected_detected(perturbation.expected, &comparison),
                "synthetic perturbation was not detected: {}",
                perturbation.name
            );
        }
    }
}
