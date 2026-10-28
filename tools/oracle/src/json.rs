//! Test-only JSON comparator for the black-box oracles; never
//! invokes media tools.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::Path,
    process::ExitCode,
};

const STREAM_GATE: &[&str] = &[
    "index",
    "kind",
    "codec_fourcc",
    "codec_name",
    "width",
    "height",
    "rotation",
    "time_base",
    "duration",
    "color_primaries",
    "color_transfer",
    "color_matrix",
    "color_range",
];
const STREAM_INFO: &[&str] = &[
    "sample_rate",
    "channels",
    "language",
    "aspect_ratio",
    "handler_name",
];
const PACKET_GATE: &[&str] = &[
    "stream_index",
    "position",
    "ordinal",
    "pts",
    "dts",
    "media_pts",
    "media_dts",
    "pts_offset",
    "dts_offset",
    "size",
    "keyframe",
];
const PACKET_INFO: &[&str] = &["duration", "discard"];
const FRAME_GATE: &[&str] = &[
    "stream_index",
    "ordinal",
    "time_base",
    "pts",
    "dts",
    "duration",
    "size",
    "md5",
];

#[derive(Default)]
struct Report {
    diffs: Vec<Value>,
    files: usize,
    stream_eligible: usize,
    stream_agree: usize,
    packet_agree: usize,
    packet_disagree: usize,
    frame_agree: usize,
    frame_disagree: usize,
    probe_failures: usize,
    frame_failures: usize,
    fatal: bool,
}

impl Report {
    fn diff(&mut self, file: &str, pointer: &str, expected: &Value, actual: &Value, gating: bool) {
        self.diffs.push(json!({"file": file, "pointer": pointer, "expected": expected, "actual": actual, "gating": gating}));
    }
    fn fields(
        &mut self,
        file: &str,
        pointer: &str,
        expected: &Value,
        actual: &Value,
        fields: &[&str],
        gating: bool,
    ) -> bool {
        let mut agrees = true;
        for field in fields {
            if expected.get(field) != actual.get(field) {
                self.diff(
                    file,
                    &format!("{pointer}/{field}"),
                    &expected.get(field).cloned().unwrap_or(Value::Null),
                    &actual.get(field).cloned().unwrap_or(Value::Null),
                    gating,
                );
                agrees = false;
            }
        }
        agrees
    }
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{key} must be an array"))
}

fn required(value: &Value, fields: &[&str]) -> Result<(), String> {
    for field in fields {
        if value.get(field).is_none() {
            return Err(format!("missing required field {field}"));
        }
    }
    Ok(())
}

fn integer(value: &Value, field: &str, nullable: bool) -> Result<(), String> {
    let item = value.get(field).ok_or_else(|| format!("missing {field}"))?;
    if item.as_i64().is_some() || nullable && item.is_null() {
        Ok(())
    } else {
        Err(format!(
            "{field} must be an integer{}",
            if nullable { " or null" } else { "" }
        ))
    }
}

fn rational(value: &Value) -> bool {
    if value.is_null() {
        return true;
    }
    value.as_array().is_some_and(|items| {
        let Some(numerator) = items.first().and_then(Value::as_i64) else {
            return false;
        };
        let Some(denominator) = items.get(1).and_then(Value::as_i64) else {
            return false;
        };
        if items.len() != 2 || denominator <= 0 {
            return false;
        }
        let mut a = numerator.unsigned_abs();
        let mut b = denominator.unsigned_abs();
        while b != 0 {
            let remainder = a % b;
            a = b;
            b = remainder;
        }
        a == 1
    })
}

fn validate(value: &Value, file: &str) -> Result<(), String> {
    required(
        value,
        &[
            "schema_version",
            "fixture",
            "probe_success",
            "packet_success",
            "frames_success",
            "format_duration",
            "streams",
            "packets",
            "frames",
            "warnings",
        ],
    )?;
    if value.get("schema_version") != Some(&json!(1))
        || value.get("fixture").and_then(Value::as_str) != Some(file)
    {
        return Err("wrong schema_version or fixture".into());
    }
    for key in ["probe_success", "packet_success"] {
        if value.get(key).and_then(Value::as_bool).is_none() {
            return Err(format!("{key} must be boolean"));
        }
    }
    if !value
        .get("frames_success")
        .is_some_and(|v| v.is_boolean() || v.is_null())
        || !value.get("format_duration").is_some_and(rational)
    {
        return Err("invalid frames_success or format_duration".into());
    }
    let mut streams = BTreeSet::new();
    for stream in array(value, "streams")? {
        required(stream, STREAM_GATE)?;
        required(stream, STREAM_INFO)?;
        for key in [
            "index",
            "width",
            "height",
            "rotation",
            "sample_rate",
            "channels",
        ] {
            integer(stream, key, key != "index")?;
        }
        let index = stream
            .get("index")
            .and_then(Value::as_i64)
            .ok_or("missing stream index")?;
        if index < 0 || !streams.insert(index) {
            return Err("negative or duplicate stream index".into());
        }
        for key in ["time_base", "duration"] {
            if !stream.get(key).is_some_and(rational) {
                return Err(format!("invalid {key}"));
            }
        }
        for key in [
            "kind",
            "codec_fourcc",
            "codec_name",
            "color_primaries",
            "color_transfer",
            "color_matrix",
            "color_range",
            "language",
            "aspect_ratio",
            "handler_name",
        ] {
            if !stream
                .get(key)
                .is_some_and(|v| v.is_string() || v.is_null())
            {
                return Err(format!("invalid {key}"));
            }
        }
    }
    for (key, fields) in [("packets", PACKET_GATE), ("frames", FRAME_GATE)] {
        let mut ordinals = BTreeSet::new();
        for item in array(value, key)? {
            required(item, fields)?;
            integer(item, "stream_index", false)?;
            integer(item, "ordinal", false)?;
            let stream = item
                .get("stream_index")
                .and_then(Value::as_i64)
                .ok_or("invalid stream index")?;
            let ordinal = item
                .get("ordinal")
                .and_then(Value::as_i64)
                .ok_or("invalid ordinal")?;
            if !streams.contains(&stream) || ordinal < 0 || !ordinals.insert((stream, ordinal)) {
                return Err(format!("invalid {key} stream or ordinal"));
            }
            for field in ["pts", "dts", "size", "duration"] {
                integer(item, field, true)?;
            }
            if key == "packets" {
                required(item, PACKET_INFO)?;
                for field in [
                    "position",
                    "media_pts",
                    "media_dts",
                    "pts_offset",
                    "dts_offset",
                ] {
                    integer(item, field, true)?;
                }
                for field in ["keyframe", "discard"] {
                    if item.get(field).and_then(Value::as_bool).is_none() {
                        return Err(format!("{field} must be boolean"));
                    }
                }
            } else if !item.get("time_base").is_some_and(rational)
                || !item
                    .get("md5")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err("invalid frame time_base or md5".into());
            }
        }
    }
    if array(value, "warnings")?.iter().any(|v| !v.is_string()) {
        return Err("warnings must be strings".into());
    }
    Ok(())
}

type Groups<'a> = BTreeMap<(i64, Option<i64>), Vec<(usize, &'a Value)>>;

fn grouped<'a>(values: &'a [Value], kind: &str) -> Groups<'a> {
    let mut groups = BTreeMap::new();
    for (index, value) in values.iter().enumerate() {
        let stream = value
            .get(if kind == "streams" {
                "index"
            } else {
                "stream_index"
            })
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        let position = value
            .get(if kind == "packets" {
                "position"
            } else {
                "ordinal"
            })
            .and_then(Value::as_i64);
        groups
            .entry((stream, position))
            .or_insert_with(Vec::new)
            .push((index, value));
    }
    groups
}

fn compare_items(
    report: &mut Report,
    file: &str,
    expected: &Value,
    actual: &Value,
    kind: &str,
    gate: &[&str],
    info: &[&str],
) -> Result<(usize, usize), String> {
    let left = grouped(array(expected, kind)?, kind);
    let right = grouped(array(actual, kind)?, kind);
    let keys: BTreeSet<_> = left.keys().chain(right.keys()).copied().collect();
    let mut agree = 0;
    let mut disagree = 0;
    for key in keys {
        let a = left.get(&key).map(Vec::as_slice).unwrap_or(&[]);
        let b = right.get(&key).map(Vec::as_slice).unwrap_or(&[]);
        for occurrence in 0..a.len().max(b.len()) {
            let index = a
                .get(occurrence)
                .or_else(|| b.get(occurrence))
                .map(|(index, _)| *index)
                .unwrap_or(0);
            let pointer = format!("/{kind}/{index}");
            match (a.get(occurrence), b.get(occurrence)) {
                (Some((_, x)), Some((_, y))) => {
                    if report.fields(file, &pointer, x, y, gate, true) {
                        agree += 1;
                    } else {
                        disagree += 1;
                    }
                    report.fields(file, &pointer, x, y, info, false);
                }
                (x, y) => {
                    disagree += 1;
                    report.diff(
                        file,
                        &pointer,
                        x.map(|(_, value)| *value).unwrap_or(&Value::Null),
                        y.map(|(_, value)| *value).unwrap_or(&Value::Null),
                        true,
                    );
                }
            }
        }
    }
    Ok((agree, disagree))
}

fn compare_file(
    report: &mut Report,
    file: &str,
    expected: &Value,
    actual: &Value,
) -> Result<(), String> {
    validate(expected, file)?;
    validate(actual, file)?;
    let status_agrees = report.fields(
        file,
        "",
        expected,
        actual,
        &["probe_success", "packet_success", "frames_success"],
        true,
    );
    if !status_agrees {
        report.fatal = true;
    }
    if expected.get("probe_success").and_then(Value::as_bool) != Some(true) {
        report.probe_failures += 1;
        return Ok(());
    }
    report.stream_eligible += 1;
    let duration_agrees = report.fields(file, "", expected, actual, &["format_duration"], true);
    let (_, stream_bad) = compare_items(
        report,
        file,
        expected,
        actual,
        "streams",
        STREAM_GATE,
        STREAM_INFO,
    )?;
    if duration_agrees
        && stream_bad == 0
        && actual.get("probe_success").and_then(Value::as_bool) == Some(true)
    {
        report.stream_agree += 1;
    }
    let (good, bad) = compare_items(
        report,
        file,
        expected,
        actual,
        "packets",
        PACKET_GATE,
        PACKET_INFO,
    )?;
    if array(expected, "streams")?.len() != array(actual, "streams")?.len() {
        report.packet_disagree += good + bad;
        report.diff(
            file,
            "/streams",
            &json!(array(expected, "streams")?.len()),
            &json!(array(actual, "streams")?.len()),
            true,
        );
    } else {
        report.packet_agree += good;
        report.packet_disagree += bad;
    }
    // A failed packet oracle for an otherwise probed file cannot establish
    // agreement. Malformed files with failed stream probes were excluded above.
    if expected.get("packet_success").and_then(Value::as_bool) != Some(true) {
        report.fatal = true;
    }
    match expected.get("frames_success").and_then(Value::as_bool) {
        Some(true) => {
            let (good, bad) =
                compare_items(report, file, expected, actual, "frames", FRAME_GATE, &[])?;
            report.frame_agree += good;
            report.frame_disagree += bad;
        }
        Some(false) => {
            report.frame_failures += 1;
        }
        None => {}
    }
    Ok(())
}

fn read(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn rate(good: usize, total: usize) -> Value {
    if total == 0 {
        Value::Null
    } else {
        json!(100.0 * good as f64 / total as f64)
    }
}

fn passes(report: &Report, real: bool, reference_files: usize) -> bool {
    let packets = report.packet_agree + report.packet_disagree;
    let frames = report.frame_agree + report.frame_disagree;
    !report.fatal
        && report.stream_eligible > 0
        && packets > 0
        && (!real || reference_files >= 100)
        && report.stream_agree * 100 >= report.stream_eligible * if real { 98 } else { 100 }
        && report.packet_agree * 100 >= packets * if real { 99 } else { 100 }
        && report.frame_disagree == 0
        && frames >= 1000
}

fn run() -> Result<bool, String> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: coreav-oracle-json REFERENCE_DIR CANDIDATE_DIR fixtures|real (JSON report on stdout)".into());
    }
    let reference = Path::new(args.first().ok_or("missing reference")?);
    let candidate = Path::new(args.get(1).ok_or("missing candidate")?);
    let profile = args.get(2).ok_or("missing profile")?;
    if profile != "fixtures" && profile != "real" {
        return Err("profile must be fixtures or real".into());
    }
    let names = |dir: &Path| -> Result<BTreeSet<String>, String> {
        let mut names = BTreeSet::new();
        for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|e| e == "json") {
                names.insert(
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .ok_or("invalid file name")?
                        .to_owned(),
                );
            }
        }
        Ok(names)
    };
    let left = names(reference)?;
    let right = names(candidate)?;
    if left.is_empty() {
        return Err("empty reference directory".into());
    }
    let mut report = Report::default();
    for name in left.union(&right) {
        report.files += 1;
        if !left.contains(name) || !right.contains(name) {
            report.fatal = true;
            report.diff(
                name,
                "",
                &json!(left.contains(name)),
                &json!(right.contains(name)),
                true,
            );
            continue;
        }
        let result = read(&reference.join(name)).and_then(|expected| {
            read(&candidate.join(name)).and_then(|actual| {
                compare_file(
                    &mut report,
                    name.trim_end_matches(".json"),
                    &expected,
                    &actual,
                )
            })
        });
        if let Err(error) = result {
            report.fatal = true;
            report.diff(
                name,
                "/schema",
                &json!("valid normalized JSON"),
                &json!(error),
                true,
            );
        }
    }
    let packet_total = report.packet_agree + report.packet_disagree;
    let frame_total = report.frame_agree + report.frame_disagree;
    let real = profile == "real";
    let passed = passes(&report, real, left.len());
    println!(
        "{}",
        json!({"schema_version": 1, "profile": profile, "passed": passed, "files": report.files,
        "streams": {"agreeing_files": report.stream_agree, "eligible_files": report.stream_eligible, "ffprobe_failures": report.probe_failures, "agreement_percent": rate(report.stream_agree, report.stream_eligible)},
        "packets": {"agreeing": report.packet_agree, "disagreeing": report.packet_disagree, "agreement_percent": rate(report.packet_agree, packet_total)},
        "frames": {"agreeing": report.frame_agree, "disagreeing": report.frame_disagree, "reference_decode_failures": report.frame_failures, "agreement_percent": rate(report.frame_agree, frame_total), "minimum_required": 1000},
        "diffs": report.diffs})
    );
    Ok(passed)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{}", json!({"error": error}));
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({"schema_version": 1, "fixture": "sample.mp4", "probe_success": true,
            "packet_success": true, "frames_success": true, "format_duration": [1, 1], "warnings": [],
            "streams": [{"index": 0, "kind": "video", "codec_fourcc": "avc1", "codec_name": "h264", "width": 320, "height": 240, "rotation": 0, "time_base": [1, 25], "duration": [1, 1], "color_primaries": null, "color_transfer": null, "color_matrix": null, "color_range": null, "sample_rate": null, "channels": null, "language": null, "aspect_ratio": "1:1", "handler_name": "VideoHandler"}],
            "packets": [{"stream_index": 0, "position": 48, "ordinal": 0, "pts": 0, "dts": -2, "media_pts": 2, "media_dts": 0, "pts_offset": -2, "dts_offset": -2, "size": 100, "keyframe": true, "duration": 1, "discard": false}],
            "frames": [{"stream_index": 0, "ordinal": 0, "time_base": [1, 25], "pts": 0, "dts": 0, "duration": 1, "size": 115200, "md5": "0123456789abcdef0123456789abcdef"}]})
    }

    #[test]
    fn informational_differences_do_not_reduce_agreement() {
        let reference = sample();
        let mut candidate = reference.clone();
        candidate["streams"][0]["language"] = json!("eng");
        candidate["packets"][0]["discard"] = json!(true);
        let mut report = Report::default();
        compare_file(&mut report, "sample.mp4", &reference, &candidate).unwrap();
        assert_eq!(
            (report.stream_agree, report.packet_agree, report.frame_agree),
            (1, 1, 1)
        );
        assert_eq!(report.diffs.len(), 2);
        assert!(report.diffs.iter().all(|d| d["gating"] == false));
    }

    #[test]
    fn packet_timestamps_and_edit_offsets_are_gating() {
        for field in [
            "pts",
            "dts",
            "media_pts",
            "media_dts",
            "pts_offset",
            "dts_offset",
            "size",
        ] {
            let reference = sample();
            let mut candidate = reference.clone();
            candidate["packets"][0][field] = json!(999);
            let mut report = Report::default();
            compare_file(&mut report, "sample.mp4", &reference, &candidate).unwrap();
            assert_eq!(report.packet_disagree, 1, "{field}");
            assert_eq!(report.stream_agree, 1);
        }
    }

    #[test]
    fn missing_and_extra_packets_enter_the_denominator() {
        let reference = sample();
        let mut candidate = reference.clone();
        candidate["packets"] = json!([]);
        let mut report = Report::default();
        compare_file(&mut report, "sample.mp4", &reference, &candidate).unwrap();
        assert_eq!((report.packet_agree, report.packet_disagree), (0, 1));
        let mut report = Report::default();
        compare_file(&mut report, "sample.mp4", &candidate, &reference).unwrap();
        assert_eq!((report.packet_agree, report.packet_disagree), (0, 1));
    }

    #[test]
    fn packets_pair_by_position_even_if_json_array_is_reordered() {
        let mut reference = sample();
        let mut packet = reference["packets"][0].clone();
        packet["position"] = json!(148);
        packet["ordinal"] = json!(1);
        reference["packets"].as_array_mut().unwrap().push(packet);
        let mut candidate = reference.clone();
        candidate["packets"].as_array_mut().unwrap().reverse();
        let mut report = Report::default();
        compare_file(&mut report, "sample.mp4", &reference, &candidate).unwrap();
        assert_eq!(report.packet_agree, 2);
        assert_eq!(report.packet_disagree, 0);
        candidate["packets"][0]["ordinal"] = json!(0);
        assert!(validate(&candidate, "sample.mp4").is_err());
    }

    #[test]
    fn failed_reference_probes_are_excluded_but_status_mismatch_fails() {
        let mut reference = sample();
        reference["probe_success"] = json!(false);
        let candidate = reference.clone();
        let mut report = Report::default();
        compare_file(&mut report, "sample.mp4", &reference, &candidate).unwrap();
        assert_eq!(report.stream_eligible, 0);
        assert_eq!(report.probe_failures, 1);
        assert!(!report.fatal);
        let mut candidate = candidate;
        candidate["probe_success"] = json!(true);
        compare_file(&mut report, "sample.mp4", &reference, &candidate).unwrap();
        assert!(report.fatal);
    }

    #[test]
    fn schema_rejects_missing_keys_and_unknown_tracks() {
        let mut candidate = sample();
        candidate.as_object_mut().unwrap().remove("format_duration");
        assert!(validate(&candidate, "sample.mp4").is_err());
        let mut candidate = sample();
        candidate["packets"][0]["stream_index"] = json!(4);
        assert!(validate(&candidate, "sample.mp4").is_err());
        let mut candidate = sample();
        candidate["streams"][0]["time_base"] = json!([1, 0]);
        assert!(validate(&candidate, "sample.mp4").is_err());
    }
}

#[cfg(test)]
mod threshold_tests {
    use super::*;

    #[test]
    fn thresholds_and_minimum_counts_are_enforced() {
        let mut report = Report {
            stream_eligible: 100,
            stream_agree: 98,
            packet_agree: 99,
            packet_disagree: 1,
            frame_agree: 1000,
            ..Report::default()
        };
        assert!(passes(&report, true, 100));
        assert!(!passes(&report, true, 99));
        assert!(!passes(&report, false, 100));
        report.stream_agree = 97;
        assert!(!passes(&report, true, 100));
        report.stream_agree = 98;
        report.packet_agree = 98;
        report.packet_disagree = 2;
        assert!(!passes(&report, true, 100));
        report.stream_agree = 100;
        report.packet_agree = 100;
        report.packet_disagree = 0;
        assert!(passes(&report, false, 100));
        report.frame_agree = 999;
        assert!(!passes(&report, false, 100));
        report.frame_agree = 1000;
        report.frame_disagree = 1;
        assert!(!passes(&report, true, 100));
    }
}
