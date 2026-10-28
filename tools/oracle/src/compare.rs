//! Schema validation and deterministic comparisons for normalized oracle files.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// A normalized comparison level.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level {
    /// Stream metadata.
    Streams,
    /// Packet metadata.
    Packets,
    /// Decoded frame metadata.
    Frames,
}

impl Level {
    /// Return the level name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Streams => "streams",
            Self::Packets => "packets",
            Self::Frames => "frames",
        }
    }

    /// Return the required schema string.
    pub fn schema(self) -> &'static str {
        match self {
            Self::Streams => "coreav-oracle/streams/1",
            Self::Packets => "coreav-oracle/packets/1",
            Self::Frames => "coreav-oracle/frames/1",
        }
    }
}

/// A field-level difference.
#[derive(Clone, Debug)]
pub struct Difference {
    /// JSON-pointer-ish path.
    pub path: String,
    /// Reference value.
    pub reference: Value,
    /// Candidate value.
    pub candidate: Value,
    /// Whether this difference is informational only.
    pub informational: bool,
}

/// Per-file comparison results.
#[derive(Clone, Debug, Default)]
pub struct FileComparison {
    /// File name.
    pub file: String,
    /// Reference-side failure, excluded from rates.
    pub reference_failure: bool,
    /// Gating differences.
    pub gating_differences: Vec<Difference>,
    /// Informational differences.
    pub informational_differences: Vec<Difference>,
    /// Gating failure outside the file/unit agreement percentages.
    pub file_level_failure: bool,
    /// Number of agreeing packets.
    pub packets_agree: u64,
    /// Number of disagreeing packets.
    pub packets_disagree: u64,
    /// Number of agreeing frames.
    pub frames_agree: u64,
    /// Number of disagreeing frames.
    pub frames_disagree: u64,
}

impl FileComparison {
    /// Whether this valid-reference file meets its gating criteria.
    pub fn agrees(&self) -> bool {
        self.gating_differences.is_empty()
    }
}

/// Metrics and detailed file results for one level.
#[derive(Clone, Debug)]
pub struct LevelComparison {
    /// Level being compared.
    pub level: Level,
    /// Number of usable reference files.
    pub files: u64,
    /// Reference failures excluded from the denominator.
    pub reference_failures: u64,
    /// Agreeing files for streams, or units for packets/frames.
    pub agree: u64,
    /// Disagreeing files for streams, or units for packets/frames.
    pub disagree: u64,
    /// Total unit count.
    pub total: u64,
    /// Required integer percentage.
    pub required_percent: u64,
    /// Minimum number of files, if any.
    pub minimum_files: u64,
    /// Minimum number of units, if any.
    pub minimum_units: u64,
    /// Whether this level passed.
    pub passed: bool,
    /// Per-file comparisons in reference order.
    pub details: Vec<FileComparison>,
    /// Streams level only: the profile and level rate and the bit-rate
    /// summary.
    pub metadata: Option<Metadata>,
}

/// Streams of a document keyed by their `index`.
fn streams_by_index(document: Option<&Value>) -> std::collections::BTreeMap<i64, &Value> {
    document
        .map(stream_list)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| Some((s.get("index")?.as_i64()?, s)))
        .collect()
}

/// The profile and level rate and the informational bit-rate comparison.
#[derive(Clone, Debug, Default)]
pub struct Metadata {
    /// Usable files with a stream where either side reports a profile or level.
    pub profile_level_files: u64,
    /// Of those, files where every such stream agrees on both.
    pub profile_level_agree: u64,
    /// Files that disagree, with their differences, for the report.
    pub profile_level_failures: Vec<Value>,
    /// Streams with a profile or level that only one side lists (for
    /// example a cover image the reference shows as a stream, or a stream
    /// the candidate drops because its codec is not allowed);
    /// the stream oracle's stream
    /// count gates them, so they are counted here but not compared.
    pub unpaired_streams: u64,
    /// Required integer percentage.
    pub required_percent: u64,
    /// Per-stream bit rates where both sides report one:
    /// (file, stream, reference, candidate).
    pub bit_rates: Vec<(String, usize, i64, i64)>,
}

impl Metadata {
    /// Whether the profile and level rate meets its threshold.
    pub fn passed(&self) -> bool {
        self.profile_level_agree.saturating_mul(100)
            >= self
                .required_percent
                .saturating_mul(self.profile_level_files)
    }

    /// Pairs streams by `index` (the position among all tracks) and compares
    /// the streams both sides list; see [`Metadata::unpaired_streams`].
    fn add(&mut self, file: &str, reference: &Value, candidate: Option<&Value>) {
        let reference_streams = streams_by_index(Some(reference));
        let candidate_streams = streams_by_index(candidate);
        let carried = |stream: Option<&&Value>| {
            stream.is_some_and(|s| {
                ["profile", "level"]
                    .iter()
                    .any(|key| s.get(key).is_some_and(|v| !v.is_null()))
            })
        };
        for (index, stream) in reference_streams.iter().chain(candidate_streams.iter()) {
            let paired =
                reference_streams.contains_key(index) && candidate_streams.contains_key(index);
            if !paired && carried(Some(stream)) {
                self.unpaired_streams += 1;
            }
        }
        let indexes = reference_streams
            .keys()
            .filter(|index| candidate_streams.contains_key(index))
            .copied()
            .collect::<Vec<_>>();
        let mut carries = false;
        let mut differences = Vec::new();
        for index in indexes {
            let left = reference_streams.get(&index);
            let right = candidate_streams.get(&index);
            let rate =
                |s: Option<&&Value>| s.and_then(|s| s.get("bit_rate")).and_then(Value::as_i64);
            if let (Some(a), Some(b)) = (rate(left), rate(right)) {
                self.bit_rates
                    .push((file.to_owned(), usize::try_from(index).unwrap_or(0), a, b));
            }
            if !carried(left) && !carried(right) {
                continue;
            }
            carries = true;
            for key in ["profile", "level"] {
                let a = left
                    .and_then(|s| s.get(key))
                    .cloned()
                    .unwrap_or(Value::Null);
                let b = right
                    .and_then(|s| s.get(key))
                    .cloned()
                    .unwrap_or(Value::Null);
                if a != b {
                    differences.push(json!({
                        "path": format!("streams/{index}/{key}"),
                        "reference": a,
                        "candidate": b,
                    }));
                }
            }
        }
        if !carries {
            return;
        }
        self.profile_level_files += 1;
        if differences.is_empty() {
            self.profile_level_agree += 1;
        } else {
            self.profile_level_failures
                .push(json!({"file": file, "differences": differences}));
        }
    }

    /// The report section.
    pub fn report(&self) -> Value {
        let mut relative = self
            .bit_rates
            .iter()
            .filter(|(_, _, reference, _)| *reference > 0)
            .map(|(_, _, reference, candidate)| {
                (*candidate as f64 - *reference as f64).abs() / *reference as f64
            })
            .collect::<Vec<_>>();
        relative.sort_by(f64::total_cmp);
        let quantile = |q: f64| -> Value {
            if relative.is_empty() {
                return Value::Null;
            }
            let index = ((relative.len() - 1) as f64 * q).round() as usize;
            json!(relative.get(index).copied())
        };
        let within = |bound: f64| relative.iter().filter(|value| **value <= bound).count();
        json!({
            "profile_level": {
                "files": self.profile_level_files,
                "agree": self.profile_level_agree,
                "agreement_percent": percentage(self.profile_level_agree, self.profile_level_files),
                "required_percent": self.required_percent,
                "unpaired_streams": self.unpaired_streams,
                "passed": self.passed(),
                "failures": self.profile_level_failures,
            },
            "bit_rate": {
                "gating": false,
                "streams": self.bit_rates.len(),
                "exact": self.bit_rates.iter().filter(|(_, _, a, b)| a == b).count(),
                "within_0_1_percent": within(0.001),
                "within_1_percent": within(0.01),
                "relative_difference_median": quantile(0.5),
                "relative_difference_p95": quantile(0.95),
                "relative_difference_max": quantile(1.0),
                "streams_detail": self.bit_rates.iter().map(|(file, stream, reference, candidate)| {
                    json!({"file": file, "stream": stream, "reference": reference, "candidate": candidate})
                }).collect::<Vec<_>>(),
            },
        })
    }
}

/// Compare normalized files at a single level.
///
/// A missing candidate is treated as a candidate-side failure.
pub fn compare_file(
    level: Level,
    reference: &Value,
    candidate: Option<&Value>,
) -> Result<FileComparison, String> {
    validate_document(level, reference)?;
    if let Some(candidate) = candidate {
        validate_document(level, candidate)?;
    }
    let file = reference
        .get("file")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>")
        .to_owned();
    let reference_ok = status_ok(level, reference);
    let mut result = FileComparison {
        file,
        reference_failure: !reference_ok,
        ..FileComparison::default()
    };
    if !reference_ok {
        return Ok(result);
    }
    let Some(candidate) = candidate else {
        result.file_level_failure = true;
        add_difference(&mut result, "file", reference.get("file"), None, false);
        add_all_unit_disagreements(level, reference, None, &mut result);
        return Ok(result);
    };
    compare_exact(
        &mut result,
        "file",
        reference.get("file"),
        candidate.get("file"),
        false,
    );
    result.file_level_failure = !result.agrees();
    if !status_ok(level, candidate) {
        result.file_level_failure = true;
        add_difference(
            &mut result,
            status_field(level),
            reference.get(status_field(level)),
            candidate.get(status_field(level)),
            false,
        );
        add_all_unit_disagreements(level, reference, Some(candidate), &mut result);
        return Ok(result);
    }

    match level {
        Level::Streams => compare_streams(reference, candidate, &mut result),
        Level::Packets => compare_packets(reference, candidate, &mut result),
        Level::Frames => compare_frames(reference, candidate, &mut result),
    }
    Ok(result)
}

/// Validate a normalized document, returning an actionable schema error.
pub fn validate_document(level: Level, document: &Value) -> Result<(), String> {
    let object = document
        .as_object()
        .ok_or_else(|| "document must be a JSON object".to_owned())?;
    require_value(object, "schema")?;
    if object.get("schema").and_then(Value::as_str) != Some(level.schema()) {
        return Err(format!("wrong schema; expected {}", level.schema()));
    }
    require_value(object, "file")?;
    if !object.get("file").is_some_and(Value::is_string) {
        return Err("key `file` must be a string".to_owned());
    }
    require_value(object, status_field(level))?;
    if !object
        .get(status_field(level))
        .is_some_and(Value::is_boolean)
    {
        return Err(format!("key `{}` must be a boolean", status_field(level)));
    }
    if !status_ok(level, document) {
        return Ok(());
    }
    if level == Level::Streams {
        require_value(object, "format_duration_us")?;
    }
    let streams = object
        .get("streams")
        .and_then(Value::as_array)
        .ok_or_else(|| "key `streams` must be an array when the probe succeeded".to_owned())?;
    for (position, stream) in streams.iter().enumerate() {
        let stream = stream
            .as_object()
            .ok_or_else(|| format!("streams/{position} must be an object"))?;
        match level {
            Level::Streams => {
                for key in [
                    "index",
                    "kind",
                    "codec_fourcc",
                    "codec_name",
                    "width",
                    "height",
                    "rotation",
                    "time_base",
                    "timescale",
                    "duration_ts",
                    "color_primaries",
                    "color_transfer",
                    "color_matrix",
                    "color_range",
                    "sample_rate",
                    "channels",
                    "language",
                    "sample_aspect_ratio",
                    "display_aspect_ratio",
                    "handler_name",
                ] {
                    require_value(stream, key)
                        .map_err(|error| format!("streams/{position}/{error}"))?;
                }
            }
            Level::Packets => {
                require_value(stream, "index")?;
                require_value(stream, "time_base")?;
                let packets = stream
                    .get("packets")
                    .and_then(Value::as_array)
                    .ok_or_else(|| format!("streams/{position}/packets must be an array"))?;
                for (packet_position, packet) in packets.iter().enumerate() {
                    let packet = packet.as_object().ok_or_else(|| {
                        format!("streams/{position}/packets/{packet_position} must be an object")
                    })?;
                    for key in [
                        "pos", "pts", "dts", "size", "keyframe", "discard", "duration",
                    ] {
                        require_value(packet, key).map_err(|error| {
                            format!("streams/{position}/packets/{packet_position}/{error}")
                        })?;
                    }
                    if let Some(run) = packet.get("run") {
                        let valid = run.as_u64().is_some_and(|n| n >= 2)
                            && packet.get("pos").and_then(Value::as_u64).is_some()
                            && packet.get("size").and_then(Value::as_u64).is_some()
                            && packet.get("duration").and_then(Value::as_i64).is_some();
                        if !valid {
                            return Err(format!(
                                "streams/{position}/packets/{packet_position}: a run needs an integer `run` of at least 2, and integer `pos`, `size` and `duration`"
                            ));
                        }
                    }
                }
            }
            Level::Frames => {
                require_value(stream, "index")?;
                require_value(stream, "time_base")?;
                let frames = stream
                    .get("frames")
                    .and_then(Value::as_array)
                    .ok_or_else(|| format!("streams/{position}/frames must be an array"))?;
                for (frame_position, frame) in frames.iter().enumerate() {
                    let frame = frame.as_object().ok_or_else(|| {
                        format!("streams/{position}/frames/{frame_position} must be an object")
                    })?;
                    for key in ["pts", "dts", "duration", "size", "md5"] {
                        require_value(frame, key).map_err(|error| {
                            format!("streams/{position}/frames/{frame_position}/{error}")
                        })?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Compare all requested reference files and calculate the selected profile threshold.
pub fn compare_directories(
    reference_dir: &Path,
    candidate_dir: &Path,
    levels: &[Level],
    profile: &str,
) -> Result<Vec<LevelComparison>, String> {
    if profile != "fixtures" && profile != "corpus" {
        return Err(format!("unknown profile `{profile}`"));
    }
    let mut results = Vec::new();
    for level in levels {
        let reference_files = level_files(reference_dir, *level)?;
        if reference_files.is_empty() {
            return Err(format!(
                "reference directory has no {} files",
                level.as_str()
            ));
        }
        let reference_names = reference_files
            .iter()
            .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        for candidate_file in level_files(candidate_dir, *level)? {
            if let Some(name) = candidate_file.file_name().and_then(|name| name.to_str()) {
                if !reference_names.contains(name) {
                    eprintln!("warning: ignoring candidate file without reference: {name}");
                }
            }
        }
        let mut details = Vec::new();
        let mut metadata = (*level == Level::Streams).then(|| Metadata {
            required_percent: if profile == "corpus" { 98 } else { 100 },
            ..Metadata::default()
        });
        let mut valid_files = 0_u64;
        let mut reference_failures = 0_u64;
        let mut agreed_files = 0_u64;
        let mut agree_units = 0_u64;
        let mut disagree_units = 0_u64;
        for reference_path in reference_files {
            let reference = read_document(&reference_path)?;
            validate_document(*level, &reference)
                .map_err(|error| format!("{}: {error}", reference_path.display()))?;
            let name = reference_path
                .file_name()
                .ok_or_else(|| format!("invalid filename: {}", reference_path.display()))?;
            let candidate_path = candidate_dir.join(name);
            let candidate = if candidate_path.is_file() {
                let document = read_document(&candidate_path)?;
                validate_document(*level, &document)
                    .map_err(|error| format!("{}: {error}", candidate_path.display()))?;
                Some(document)
            } else {
                None
            };
            let comparison = compare_file(*level, &reference, candidate.as_ref())
                .map_err(|error| format!("{}: {error}", reference_path.display()))?;
            if comparison.reference_failure {
                reference_failures += 1;
            } else {
                // Files both tools probe; a probe
                // failure is gated by the stream oracle's `probe_ok`.
                if let (Some(metadata), Some(document)) = (
                    metadata.as_mut(),
                    candidate
                        .as_ref()
                        .filter(|document| status_ok(*level, document)),
                ) {
                    metadata.add(&comparison.file, &reference, Some(document));
                }
                valid_files += 1;
                if comparison.agrees() {
                    agreed_files += 1;
                }
                match level {
                    Level::Streams => {}
                    Level::Packets => {
                        agree_units += comparison.packets_agree;
                        disagree_units += comparison.packets_disagree;
                    }
                    Level::Frames => {
                        agree_units += comparison.frames_agree;
                        disagree_units += comparison.frames_disagree;
                    }
                }
            }
            details.push(comparison);
        }
        let (agree, disagree, total, required_percent, minimum_files, minimum_units) = match level {
            Level::Streams => (
                agreed_files,
                valid_files.saturating_sub(agreed_files),
                valid_files,
                if profile == "corpus" { 98 } else { 100 },
                if profile == "corpus" { 100 } else { 0 },
                0,
            ),
            Level::Packets | Level::Frames => (
                agree_units,
                disagree_units,
                agree_units.saturating_add(disagree_units),
                if *level == Level::Packets && profile == "corpus" {
                    99
                } else {
                    100
                },
                0,
                if *level == Level::Frames { 1_000 } else { 0 },
            ),
        };
        let passed = !details.iter().any(|file| file.file_level_failure)
            && metadata.as_ref().is_none_or(Metadata::passed)
            && threshold_met(
                agree,
                total,
                required_percent,
                valid_files,
                minimum_files,
                minimum_units,
            );
        results.push(LevelComparison {
            level: *level,
            files: valid_files,
            reference_failures,
            agree,
            disagree,
            total,
            required_percent,
            minimum_files,
            minimum_units,
            passed,
            details,
            metadata,
        });
    }
    Ok(results)
}

/// Build the machine-readable report document.
pub fn report_json(results: &[LevelComparison], profile: &str) -> Value {
    let levels = results
        .iter()
        .map(|result| {
            json!({
                "level": result.level.as_str(),
                "files": result.files,
                "reference_failures": result.reference_failures,
                "agree": result.agree,
                "disagree": result.disagree,
                "total": result.total,
                "agreement_percent": percentage(result.agree, result.total),
                "required_percent": result.required_percent,
                "minimum_files": result.minimum_files,
                "minimum_units": result.minimum_units,
                "passed": result.passed,
                "metadata": result.metadata.as_ref().map(Metadata::report),
                "details": result.details.iter().map(file_report).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema": "coreav-oracle/report/1",
        "profile": profile,
        "passed": results.iter().all(|result| result.passed),
        "levels": levels
    })
}

/// Print a concise human-readable comparison summary.
pub fn print_summary(results: &[LevelComparison], max_diffs: usize) {
    for result in results {
        let threshold = if result.minimum_files > 0 {
            format!(
                "{}% and at least {} files",
                result.required_percent, result.minimum_files
            )
        } else if result.minimum_units > 0 {
            format!(
                "{}% and at least {} compared frames",
                result.required_percent, result.minimum_units
            )
        } else {
            format!("{}%", result.required_percent)
        };
        println!(
            "{}: {}/{} ({:.2}%), threshold {} — {} ({} reference failures)",
            result.level.as_str(),
            result.agree,
            result.total,
            percentage(result.agree, result.total),
            threshold,
            if result.passed { "PASS" } else { "FAIL" },
            result.reference_failures
        );
        if let Some(metadata) = &result.metadata {
            println!(
                "  profile and level: {}/{} files ({:.2}%), threshold {}% — {}; bit rate (informational): {} streams compared",
                metadata.profile_level_agree,
                metadata.profile_level_files,
                percentage(metadata.profile_level_agree, metadata.profile_level_files),
                metadata.required_percent,
                if metadata.passed() { "PASS" } else { "FAIL" },
                metadata.bit_rates.len()
            );
        }
        for file in &result.details {
            if file.reference_failure || file.agrees() {
                continue;
            }
            println!("  {}: FAIL", file.file);
            for difference in file
                .gating_differences
                .iter()
                .chain(file.informational_differences.iter())
                .take(max_diffs)
            {
                let label = if difference.informational {
                    "info "
                } else {
                    ""
                };
                println!(
                    "    {label}{}: ref={} cand={}",
                    difference.path,
                    compact(&difference.reference),
                    compact(&difference.candidate)
                );
            }
        }
    }
}

fn compare_streams(reference: &Value, candidate: &Value, result: &mut FileComparison) {
    let reference_streams = stream_list(reference);
    let candidate_streams = stream_list(candidate);
    compare_exact(
        result,
        "probe_ok",
        reference.get("probe_ok"),
        candidate.get("probe_ok"),
        false,
    );
    compare_exact(
        result,
        "format_duration_us",
        reference.get("format_duration_us"),
        candidate.get("format_duration_us"),
        false,
    );
    if reference_streams.len() != candidate_streams.len() {
        result.file_level_failure = true;
        add_difference(
            result,
            "streams/count",
            Some(&json!(reference_streams.len())),
            Some(&json!(candidate_streams.len())),
            false,
        );
    }
    const GATING: &[&str] = &[
        "kind",
        "codec_fourcc",
        "codec_name",
        "width",
        "height",
        "rotation",
        "timescale",
        "duration_ts",
        "color_primaries",
        "color_transfer",
        "color_matrix",
        "color_range",
        // The profile and level.
        "profile",
        "level",
    ];
    const INFORMATIONAL: &[&str] = &[
        "sample_rate",
        "channels",
        "language",
        "sample_aspect_ratio",
        "display_aspect_ratio",
        "handler_name",
        "bit_rate",
        "creation_time",
    ];
    let pairs = paired_streams(reference_streams, candidate_streams);
    report_unpaired(result, reference_streams, candidate_streams, &pairs);
    for (position, (reference_stream, candidate_stream)) in
        reference_streams.iter().zip(pairs).enumerate()
    {
        let Some(candidate_stream) = candidate_stream else {
            continue;
        };
        for field in GATING {
            compare_exact(
                result,
                &format!("streams/{position}/{field}"),
                reference_stream.get(field),
                candidate_stream.get(field),
                false,
            );
        }
        for field in INFORMATIONAL {
            compare_exact(
                result,
                &format!("streams/{position}/{field}"),
                reference_stream.get(field),
                candidate_stream.get(field),
                true,
            );
        }
    }
}

fn compare_packets(reference: &Value, candidate: &Value, result: &mut FileComparison) {
    let reference_streams = stream_list(reference);
    let candidate_streams = stream_list(candidate);
    compare_exact(
        result,
        "probe_ok",
        reference.get("probe_ok"),
        candidate.get("probe_ok"),
        false,
    );
    if reference_streams.len() != candidate_streams.len() {
        result.file_level_failure = true;
        add_difference(
            result,
            "streams/count",
            Some(&json!(reference_streams.len())),
            Some(&json!(candidate_streams.len())),
            false,
        );
    }
    let pairs = paired_streams(reference_streams, candidate_streams);
    report_unpaired(result, reference_streams, candidate_streams, &pairs);
    for (stream_position, (reference_stream, candidate_stream)) in reference_streams
        .iter()
        .zip(pairs.iter().copied())
        .enumerate()
    {
        let reference_packets = packet_list(reference_stream);
        let Some(candidate_stream) = candidate_stream else {
            result.packets_disagree += count(reference_packets.len());
            continue;
        };
        let candidate_packets = packet_list(candidate_stream);
        let tbs = (
            reference_stream.get("time_base"),
            candidate_stream.get("time_base"),
        );
        if candidate_packets.iter().any(|p| p.get("run").is_some()) {
            if runs_are_disjoint(candidate_packets) {
                compare_runs(
                    result,
                    stream_position,
                    reference_packets,
                    candidate_packets,
                    tbs,
                );
            } else {
                // Overlapping runs, or entries without a position: the
                // packets one by one, as listed (rare; memory grows with
                // the stream only here).
                let expanded = expand_runs(candidate_packets);
                compare_listed(result, stream_position, reference_packets, &expanded, tbs);
            }
            continue;
        }
        compare_listed(
            result,
            stream_position,
            reference_packets,
            candidate_packets,
            tbs,
        );
    }
    // Candidate streams no reference stream is paired with.
    for (stream_position, candidate_stream) in
        candidate_streams.iter().enumerate().filter(|(_, stream)| {
            !pairs
                .iter()
                .any(|paired| paired.is_some_and(|p| std::ptr::eq(p, *stream)))
        })
    {
        let packets = packet_list(candidate_stream);
        let total: u64 = packets.iter().map(run_length).sum();
        result.packets_disagree += total;
        add_difference(
            result,
            &format!("streams/{stream_position}/packets"),
            None,
            Some(&json!(total)),
            false,
        );
    }
}

fn compare_frames(reference: &Value, candidate: &Value, result: &mut FileComparison) {
    let reference_streams = stream_list(reference);
    let candidate_streams = stream_list(candidate);
    compare_exact(
        result,
        "decode_ok",
        reference.get("decode_ok"),
        candidate.get("decode_ok"),
        false,
    );
    for (reference_position, reference_stream) in reference_streams.iter().enumerate() {
        let reference_index = reference_stream.get("index");
        let paired_stream = candidate_streams
            .iter()
            .find(|stream| stream.get("index") == reference_index);
        let Some(candidate_stream) = paired_stream else {
            result.file_level_failure = true;
            let frames = frame_list(reference_stream);
            result.frames_disagree += count(frames.len());
            add_difference(
                result,
                &format!("streams/{reference_position}/index"),
                reference_index,
                None,
                false,
            );
            continue;
        };
        let reference_frames = frame_list(reference_stream);
        let candidate_frames = frame_list(candidate_stream);
        let reference_tb = reference_stream.get("time_base");
        let candidate_tb = candidate_stream.get("time_base");
        let paired_count = reference_frames.len().min(candidate_frames.len());
        for position in 0..paired_count {
            let Some(reference_frame) = reference_frames.get(position) else {
                continue;
            };
            let Some(candidate_frame) = candidate_frames.get(position) else {
                continue;
            };
            let path = format!("streams/{}/frames/{position}", index_text(reference_index));
            let before = result.gating_differences.len();
            compare_timestamp(
                result,
                &format!("{path}/pts"),
                reference_frame.get("pts"),
                candidate_frame.get("pts"),
                reference_tb,
                candidate_tb,
                false,
            );
            for field in ["size", "md5"] {
                compare_exact(
                    result,
                    &format!("{path}/{field}"),
                    reference_frame.get(field),
                    candidate_frame.get(field),
                    false,
                );
            }
            for field in ["dts", "duration"] {
                compare_timestamp(
                    result,
                    &format!("{path}/{field}"),
                    reference_frame.get(field),
                    candidate_frame.get(field),
                    reference_tb,
                    candidate_tb,
                    true,
                );
            }
            if result.gating_differences.len() == before {
                result.frames_agree += 1;
            } else {
                result.frames_disagree += 1;
            }
        }
        for position in paired_count..reference_frames.len() {
            result.frames_disagree += 1;
            add_difference(
                result,
                &format!("streams/{}/frames/{position}", index_text(reference_index)),
                reference_frames.get(position),
                None,
                false,
            );
        }
        for position in paired_count..candidate_frames.len() {
            result.frames_disagree += 1;
            add_difference(
                result,
                &format!("streams/{}/frames/extra", index_text(reference_index)),
                None,
                candidate_frames.get(position),
                false,
            );
        }
    }
    for candidate_stream in candidate_streams {
        if !reference_streams
            .iter()
            .any(|stream| stream.get("index") == candidate_stream.get("index"))
        {
            result.file_level_failure = true;
            let frames = frame_list(candidate_stream);
            result.frames_disagree += count(frames.len());
            add_difference(
                result,
                &format!(
                    "streams/{}/frames",
                    index_text(candidate_stream.get("index"))
                ),
                None,
                Some(&json!(frames.len())),
                false,
            );
        }
    }
}

fn add_all_unit_disagreements(
    level: Level,
    reference: &Value,
    candidate: Option<&Value>,
    result: &mut FileComparison,
) {
    match level {
        Level::Streams => {}
        Level::Packets => {
            result.packets_disagree = failed_packet_disagreements(reference, candidate);
        }
        Level::Frames => {
            result.frames_disagree = failed_frame_disagreements(reference, candidate);
        }
    }
}

fn failed_packet_disagreements(reference: &Value, candidate: Option<&Value>) -> u64 {
    let reference_streams = stream_list(reference);
    let candidate_streams = candidate.map(stream_list).unwrap_or_default();
    let mut disagree = 0_u64;
    for (stream_position, reference_stream) in reference_streams.iter().enumerate() {
        let packets = packet_list(reference_stream);
        let Some(candidate_stream) = candidate_streams.get(stream_position) else {
            disagree = disagree.saturating_add(count(packets.len()));
            continue;
        };
        let candidates = packet_list(candidate_stream);
        let mut candidate_positions = packet_positions(candidates);
        for packet in packets {
            candidate_positions
                .get_mut(&packet.get("pos"))
                .and_then(VecDeque::pop_front);
            disagree = disagree.saturating_add(1);
        }
        disagree =
            disagree.saturating_add(count(candidate_positions.values().map(VecDeque::len).sum()));
    }
    for stream in candidate_streams.iter().skip(reference_streams.len()) {
        disagree = disagree.saturating_add(count(packet_list(stream).len()));
    }
    disagree
}

fn failed_frame_disagreements(reference: &Value, candidate: Option<&Value>) -> u64 {
    let reference_streams = stream_list(reference);
    let candidate_streams = candidate.map(stream_list).unwrap_or_default();
    let mut disagree = 0_u64;
    for reference_stream in reference_streams {
        let frames = frame_list(reference_stream);
        let candidate_stream = candidate_streams
            .iter()
            .find(|stream| stream.get("index") == reference_stream.get("index"));
        match candidate_stream {
            Some(candidate_stream) => {
                disagree = disagree
                    .saturating_add(count(frames.len().max(frame_list(candidate_stream).len())));
            }
            None => disagree = disagree.saturating_add(count(frames.len())),
        }
    }
    for candidate_stream in candidate_streams {
        if !reference_streams
            .iter()
            .any(|stream| stream.get("index") == candidate_stream.get("index"))
        {
            disagree = disagree.saturating_add(count(frame_list(candidate_stream).len()));
        }
    }
    disagree
}

fn compare_exact(
    result: &mut FileComparison,
    path: &str,
    reference: Option<&Value>,
    candidate: Option<&Value>,
    informational: bool,
) {
    let left = reference.cloned().unwrap_or(Value::Null);
    let right = candidate.cloned().unwrap_or(Value::Null);
    if left != right {
        add_difference(result, path, Some(&left), Some(&right), informational);
    }
}

fn compare_timestamp(
    result: &mut FileComparison,
    path: &str,
    reference: Option<&Value>,
    candidate: Option<&Value>,
    reference_tb: Option<&Value>,
    candidate_tb: Option<&Value>,
    informational: bool,
) {
    let agrees = match (reference, candidate) {
        (Some(Value::Null) | None, Some(Value::Null) | None) => true,
        (Some(left), Some(right)) => {
            let left = left.as_i64();
            let right = right.as_i64();
            let reference_time_base = parse_time_base_value(reference_tb);
            let candidate_time_base = parse_time_base_value(candidate_tb);
            match (left, right, reference_time_base, candidate_time_base) {
                (Some(left), Some(right), Some((num_a, den_a)), Some((num_b, den_b))) => {
                    rational_equal(left, num_a, den_a, right, num_b, den_b)
                }
                _ => false,
            }
        }
        _ => false,
    };
    if !agrees {
        add_difference(result, path, reference, candidate, informational);
    }
}

fn rational_equal(left: i64, num_a: i64, den_a: i64, right: i64, num_b: i64, den_b: i64) -> bool {
    let Some(left_value) = i128::from(left)
        .checked_mul(i128::from(num_a))
        .and_then(|value| value.checked_mul(i128::from(den_b)))
    else {
        return false;
    };
    let Some(right_value) = i128::from(right)
        .checked_mul(i128::from(num_b))
        .and_then(|value| value.checked_mul(i128::from(den_a)))
    else {
        return false;
    };
    left_value == right_value
}

fn parse_time_base_value(value: Option<&Value>) -> Option<(i64, i64)> {
    let values = value?.as_array()?;
    let numerator = values.first()?.as_i64()?;
    let denominator = values.get(1)?.as_i64()?;
    if numerator <= 0 || denominator <= 0 {
        return None;
    }
    Some((numerator, denominator))
}

fn add_difference(
    result: &mut FileComparison,
    path: &str,
    reference: Option<&Value>,
    candidate: Option<&Value>,
    informational: bool,
) {
    let difference = Difference {
        path: path.to_owned(),
        reference: reference.cloned().unwrap_or(Value::Null),
        candidate: candidate.cloned().unwrap_or(Value::Null),
        informational,
    };
    if informational {
        result.informational_differences.push(difference);
    } else {
        result.gating_differences.push(difference);
    }
}

fn status_field(level: Level) -> &'static str {
    if level == Level::Frames {
        "decode_ok"
    } else {
        "probe_ok"
    }
}

fn status_ok(level: Level, document: &Value) -> bool {
    document
        .get(status_field(level))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The candidate stream paired with each reference stream: the one with the
/// same `index` (the position among all tracks) when every stream on both
/// sides has one, as the frames level pairs them, so a stream the candidate
/// leaves out does not shift the others; else the one at the same position.
fn paired_streams<'a>(reference: &[Value], candidate: &'a [Value]) -> Vec<Option<&'a Value>> {
    let indexed = reference
        .iter()
        .chain(candidate)
        .all(|stream| stream.get("index").and_then(Value::as_i64).is_some());
    reference
        .iter()
        .enumerate()
        .map(|(position, stream)| {
            if indexed {
                candidate
                    .iter()
                    .find(|c| c.get("index") == stream.get("index"))
            } else {
                candidate.get(position)
            }
        })
        .collect()
}

/// When the stream counts agree but some stream has no partner (different
/// index sets), records the difference: with equal counts `streams/count`
/// does not, and an unpaired stream would otherwise go uncompared.
fn report_unpaired(
    result: &mut FileComparison,
    reference: &[Value],
    candidate: &[Value],
    pairs: &[Option<&Value>],
) {
    if reference.len() != candidate.len() {
        return;
    }
    let unpaired = pairs.iter().any(Option::is_none)
        || candidate.iter().any(|stream| {
            !pairs
                .iter()
                .any(|p| p.is_some_and(|p| std::ptr::eq(p, stream)))
        });
    if unpaired {
        result.file_level_failure = true;
        let indexes = |streams: &[Value]| {
            json!(
                streams
                    .iter()
                    .map(|s| s.get("index").cloned())
                    .collect::<Vec<_>>()
            )
        };
        add_difference(
            result,
            "streams/indexes",
            Some(&indexes(reference)),
            Some(&indexes(candidate)),
            false,
        );
    }
}

fn stream_list(document: &Value) -> &[Value] {
    document
        .get("streams")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// Pairs the packets of one stream as listed (SPEC.md, packets level).
fn compare_listed(
    result: &mut FileComparison,
    stream_position: usize,
    reference_packets: &[Value],
    candidate_packets: &[Value],
    (reference_tb, candidate_tb): (Option<&Value>, Option<&Value>),
) {
    let mut candidate_positions = packet_positions(candidate_packets);
    let mut paired = vec![false; candidate_packets.len()];
    for (packet_position, reference_packet) in reference_packets.iter().enumerate() {
        let pos = reference_packet.get("pos");
        let candidate_position = candidate_positions
            .get_mut(&pos)
            .and_then(VecDeque::pop_front);
        let Some(candidate_position) = candidate_position else {
            result.packets_disagree += 1;
            add_difference(
                result,
                &format!("streams/{stream_position}/packets/{packet_position}"),
                Some(reference_packet),
                None,
                false,
            );
            continue;
        };
        if let Some(is_paired) = paired.get_mut(candidate_position) {
            *is_paired = true;
        }
        let Some(candidate_packet) = candidate_packets.get(candidate_position) else {
            continue;
        };
        let path = format!("streams/{stream_position}/packets/{packet_position}");
        compare_packet_fields(
            result,
            &path,
            reference_packet,
            candidate_packet,
            (reference_tb, candidate_tb),
        );
    }
    for (candidate_position, packet) in candidate_packets.iter().enumerate() {
        if !paired.get(candidate_position).copied().unwrap_or(true) {
            result.packets_disagree += 1;
            add_difference(
                result,
                &format!("streams/{stream_position}/packets/extra"),
                None,
                Some(packet),
                false,
            );
        }
    }
}

/// The packets runs stand for, one entry each (SPEC.md, "Runs").
fn expand_runs(packets: &[Value]) -> Vec<Value> {
    let mut expanded = Vec::new();
    for packet in packets {
        let length = run_length(packet);
        if packet.get("run").is_some() {
            expanded.extend((0..length).map(|index| run_packet(packet, index)));
        } else {
            expanded.push(packet.clone());
        }
    }
    expanded
}

/// Whether every entry has an integer position and size and no two entries
/// cover the same bytes, so that arithmetic pairing equals pairing the
/// expanded packets by position.
fn runs_are_disjoint(packets: &[Value]) -> bool {
    let mut spans = Vec::with_capacity(packets.len());
    for packet in packets {
        let (Some(pos), Some(size)) = (
            packet.get("pos").and_then(Value::as_u64),
            packet.get("size").and_then(Value::as_u64),
        ) else {
            return false;
        };
        let Some(end) = size
            .checked_mul(run_length(packet))
            .and_then(|bytes| pos.checked_add(bytes))
        else {
            return false;
        };
        spans.push((pos, end.max(pos.saturating_add(1))));
    }
    spans.sort_unstable();
    spans.windows(2).all(|pair| match pair {
        [(_, end), (next, _)] => end <= next,
        _ => true,
    })
}

/// Compares one packet's fields; counts it as agreeing or disagreeing.
fn compare_packet_fields(
    result: &mut FileComparison,
    path: &str,
    reference_packet: &Value,
    candidate_packet: &Value,
    (reference_tb, candidate_tb): (Option<&Value>, Option<&Value>),
) {
    let before = result.gating_differences.len();
    for field in ["pts", "dts"] {
        compare_timestamp(
            result,
            &format!("{path}/{field}"),
            reference_packet.get(field),
            candidate_packet.get(field),
            reference_tb,
            candidate_tb,
            false,
        );
    }
    for field in ["size", "keyframe"] {
        compare_exact(
            result,
            &format!("{path}/{field}"),
            reference_packet.get(field),
            candidate_packet.get(field),
            false,
        );
    }
    compare_exact(
        result,
        &format!("{path}/discard"),
        reference_packet.get("discard"),
        candidate_packet.get("discard"),
        true,
    );
    compare_timestamp(
        result,
        &format!("{path}/duration"),
        reference_packet.get("duration"),
        candidate_packet.get("duration"),
        reference_tb,
        candidate_tb,
        true,
    );
    if result.gating_differences.len() == before {
        result.packets_agree += 1;
    } else {
        result.packets_disagree += 1;
    }
}

/// The number of packets an entry stands for: `run` for a run, else 1.
fn run_length(packet: &Value) -> u64 {
    packet.get("run").and_then(Value::as_u64).unwrap_or(1)
}

/// One packet of a run: its `index`th, the first's fields advanced by
/// `index` sizes in position and `index` durations in every timestamp
/// (SPEC.md, "Runs").
fn run_packet(run: &Value, index: u64) -> Value {
    let mut packet = run.clone();
    let Some(object) = packet.as_object_mut() else {
        return packet;
    };
    object.remove("run");
    let size = run.get("size").and_then(Value::as_u64).unwrap_or(0);
    let duration = run.get("duration").and_then(Value::as_i64).unwrap_or(0);
    let step = i64::try_from(index).unwrap_or(i64::MAX);
    if let Some(pos) = run.get("pos").and_then(Value::as_u64) {
        object.insert(
            "pos".to_owned(),
            json!(pos.saturating_add(size.saturating_mul(index))),
        );
    }
    for field in ["pts", "dts", "media_dts", "media_cts"] {
        if let Some(value) = run.get(field).and_then(Value::as_i64) {
            object.insert(
                field.to_owned(),
                json!(value.saturating_add(duration.saturating_mul(step))),
            );
        }
    }
    packet
}

/// A candidate stream whose packets come in runs (constant-size samples at
/// consecutive positions, SPEC.md "Runs"): each reference packet pairs with
/// the run sample at its position, found by arithmetic, so no run is
/// expanded; the samples no reference packet pairs with count as
/// disagreeing, reported once per run (`packets/extra_run`).
fn compare_runs(
    result: &mut FileComparison,
    stream_position: usize,
    reference_packets: &[Value],
    candidate_packets: &[Value],
    (reference_tb, candidate_tb): (Option<&Value>, Option<&Value>),
) {
    // (first position, size, length, entry) sorted by position.
    let mut runs: Vec<(u64, u64, u64, &Value)> = candidate_packets
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("pos")?.as_u64()?,
                entry.get("size")?.as_u64()?,
                run_length(entry),
                entry,
            ))
        })
        .collect();
    runs.sort_by_key(|run| run.0);
    let mut paired: Vec<std::collections::BTreeSet<u64>> = vec![Default::default(); runs.len()];
    for (packet_position, reference_packet) in reference_packets.iter().enumerate() {
        let path = format!("streams/{stream_position}/packets/{packet_position}");
        let found = reference_packet
            .get("pos")
            .and_then(Value::as_u64)
            .and_then(|pos| {
                let at = runs.partition_point(|run| run.0 <= pos).checked_sub(1)?;
                let (start, size, length, _) = *runs.get(at)?;
                let offset = pos - start;
                let index = offset.checked_div(size)?;
                (offset.checked_rem(size)? == 0 && index < length).then_some((at, index))
            })
            .filter(|(at, index)| paired.get(*at).is_some_and(|set| !set.contains(index)));
        let Some((at, index)) = found else {
            result.packets_disagree += 1;
            add_difference(result, &path, Some(reference_packet), None, false);
            continue;
        };
        if let Some(set) = paired.get_mut(at) {
            set.insert(index);
        }
        let Some((_, _, _, run)) = runs.get(at) else {
            continue;
        };
        let candidate_packet = run_packet(run, index);
        compare_packet_fields(
            result,
            &path,
            reference_packet,
            &candidate_packet,
            (reference_tb, candidate_tb),
        );
    }
    for ((start, _, length, run), set) in runs.iter().zip(&paired) {
        let unpaired = length.saturating_sub(count(set.len()));
        if unpaired > 0 {
            result.packets_disagree += unpaired;
            let mut summary = (*run).clone();
            if let Some(object) = summary.as_object_mut() {
                object.insert("run".to_owned(), json!(length));
                object.insert("unpaired".to_owned(), json!(unpaired));
                object.insert("pos".to_owned(), json!(start));
            }
            add_difference(
                result,
                &format!("streams/{stream_position}/packets/extra_run"),
                None,
                Some(&summary),
                false,
            );
        }
    }
}

fn packet_list(stream: &Value) -> &[Value] {
    stream
        .get("packets")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn frame_list(stream: &Value) -> &[Value] {
    stream
        .get("frames")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn packet_positions(packets: &[Value]) -> HashMap<Option<&Value>, VecDeque<usize>> {
    let mut positions = HashMap::<_, VecDeque<_>>::new();
    for (index, packet) in packets.iter().enumerate() {
        positions
            .entry(packet.get("pos"))
            .or_default()
            .push_back(index);
    }
    positions
}

fn require_value(object: &Map<String, Value>, key: &str) -> Result<(), String> {
    if object.contains_key(key) {
        Ok(())
    } else {
        Err(format!("missing required key `{key}`"))
    }
}

pub(crate) fn read_document(path: &Path) -> Result<Value, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("unable to read {}: {error}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("invalid JSON in {}: {error}", path.display()))
}

pub(crate) fn level_files(directory: &Path, level: Level) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("unable to read directory {}: {error}", directory.display()))?;
    let suffix = format!(".{}.json", level.as_str());
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("directory entry error: {error}"))?;
        if entry
            .file_type()
            .map_err(|error| format!("file type error: {error}"))?
            .is_file()
            && entry.file_name().to_string_lossy().ends_with(&suffix)
        {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

fn percentage(agree: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        agree as f64 * 100.0 / total as f64
    }
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn threshold_met(
    agree: u64,
    total: u64,
    required_percent: u64,
    files: u64,
    minimum_files: u64,
    minimum_units: u64,
) -> bool {
    files >= minimum_files
        && total >= minimum_units
        && total > 0
        && u128::from(agree) * 100 >= u128::from(required_percent) * u128::from(total)
}

fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}

fn index_text(value: Option<&Value>) -> String {
    value.map(compact).unwrap_or_else(|| "null".to_owned())
}

fn file_report(file: &FileComparison) -> Value {
    json!({
        "file": file.file,
        "reference_failure": file.reference_failure,
        "agrees": file.agrees(),
        "packets_agree": file.packets_agree,
        "packets_disagree": file.packets_disagree,
        "frames_agree": file.frames_agree,
        "frames_disagree": file.frames_disagree,
        "gating_differences": file.gating_differences.iter().map(difference_report).collect::<Vec<_>>(),
        "informational_differences": file.informational_differences.iter().map(difference_report).collect::<Vec<_>>()
    })
}

fn difference_report(difference: &Difference) -> Value {
    json!({
        "path": difference.path,
        "reference": difference.reference,
        "candidate": difference.candidate,
        "informational": difference.informational
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream_fixture(width: i64, probe_ok: bool) -> Value {
        json!({
            "schema":"coreav-oracle/streams/1","file":"a.mp4","probe_ok":probe_ok,
            "format_duration_us":1,
            "streams":[{
                "index":0,"kind":"video","codec_fourcc":"avc1","codec_name":"h264",
                "width":width,"height":10,"rotation":0,"time_base":[1,10],"timescale":10,
                "duration_ts":10,"color_primaries":null,"color_transfer":null,"color_matrix":null,
                "color_range":null,"sample_rate":null,"channels":null,"language":null,
                "sample_aspect_ratio":null,"display_aspect_ratio":null,"handler_name":null
            }]
        })
    }

    fn packets_fixture(positions: &[Value], tb: Value) -> Value {
        json!({
            "schema":"coreav-oracle/packets/1","file":"a.mp4","probe_ok":true,
            "streams":[{
                "index":0,"time_base":tb,
                "packets":positions.iter().map(|pos| json!({
                    "pos":pos,"pts":0,"dts":0,"size":5,"keyframe":true,
                    "discard":false,"duration":1
                })).collect::<Vec<_>>()
            }]
        })
    }

    #[test]
    fn packet_pairing_uses_position_and_duplicate_order_including_null() {
        let left = packets_fixture(&[json!(null), json!(7), json!(7)], json!([1, 10]));
        let right = packets_fixture(&[json!(7), json!(null), json!(7)], json!([1, 10]));
        let result = compare_file(Level::Packets, &left, Some(&right)).expect("comparison");
        assert_eq!(result.packets_agree, 3);
        assert_eq!(result.packets_disagree, 0);
    }

    #[test]
    fn packet_pairing_consumes_duplicate_positions_in_order_and_counts_extras() {
        let mut left = packets_fixture(&[json!(7), json!(null), json!(7)], json!([1, 10]));
        let mut right = packets_fixture(
            &[json!(null), json!(7), json!(7), json!(7), json!(null)],
            json!([1, 10]),
        );
        left["streams"][0]["packets"][2]["size"] = json!(9);
        right["streams"][0]["packets"][2]["size"] = json!(9);
        let result = compare_file(Level::Packets, &left, Some(&right)).expect("comparison");
        assert_eq!(result.packets_agree, 3);
        assert_eq!(result.packets_disagree, 2);
        assert_eq!(failed_packet_disagreements(&left, Some(&right)), 5);

        right["streams"][0]["packets"][1]["size"] = json!(9);
        right["streams"][0]["packets"][2]["size"] = json!(5);
        let swapped = compare_file(Level::Packets, &left, Some(&right)).expect("comparison");
        assert_eq!(swapped.packets_agree, 1);
        assert_eq!(swapped.packets_disagree, 4);
    }

    #[test]
    fn packet_pairing_handles_many_distinct_positions_in_reverse_order() {
        let positions = (0..10_000).map(|pos| json!(pos)).collect::<Vec<_>>();
        let left = packets_fixture(&positions, json!([1, 10]));
        let reversed = positions.into_iter().rev().collect::<Vec<_>>();
        let right = packets_fixture(&reversed, json!([1, 10]));
        let result = compare_file(Level::Packets, &left, Some(&right)).expect("comparison");
        assert_eq!(result.packets_agree, 10_000);
        assert_eq!(result.packets_disagree, 0);
    }

    #[test]
    fn packet_field_paths_use_reference_array_indices() {
        let left = packets_fixture(&[json!(7), json!(null), json!(9), json!(7)], json!([1, 10]));
        let mut right =
            packets_fixture(&[json!(9), json!(7), json!(7), json!(null)], json!([1, 10]));
        right["streams"][0]["packets"][0]["pts"] = json!(1);
        right["streams"][0]["packets"][2]["size"] = json!(6);
        right["streams"][0]["packets"][3]["discard"] = json!(true);
        let result = compare_file(Level::Packets, &left, Some(&right)).expect("comparison");
        let paths = result
            .gating_differences
            .iter()
            .map(|difference| difference.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            ["streams/0/packets/2/pts", "streams/0/packets/3/size"]
        );
        assert_eq!(
            result.informational_differences[0].path,
            "streams/0/packets/1/discard"
        );
        assert_eq!(result.packets_agree, 2);
        assert_eq!(result.packets_disagree, 2);
    }

    #[test]
    fn timestamps_compare_as_rational_values() {
        let mut left = packets_fixture(&[json!(1)], json!([1, 10]));
        if let Some(packet) = left
            .get_mut("streams")
            .and_then(Value::as_array_mut)
            .and_then(|streams| streams.first_mut())
            .and_then(|stream| stream.get_mut("packets"))
            .and_then(Value::as_array_mut)
            .and_then(|packets| packets.first_mut())
        {
            packet["pts"] = json!(2);
            packet["dts"] = json!(-2);
        }
        let mut right = packets_fixture(&[json!(1)], json!([1, 20]));
        if let Some(packet) = right
            .get_mut("streams")
            .and_then(Value::as_array_mut)
            .and_then(|streams| streams.first_mut())
            .and_then(|stream| stream.get_mut("packets"))
            .and_then(Value::as_array_mut)
            .and_then(|packets| packets.first_mut())
        {
            packet["pts"] = json!(4);
            packet["dts"] = json!(-4);
            packet["duration"] = json!(2);
        }
        let equivalent = compare_file(Level::Packets, &left, Some(&right)).expect("comparison");
        assert!(equivalent.agrees());
        assert!(equivalent.informational_differences.is_empty());

        if let Some(packet) = right
            .get_mut("streams")
            .and_then(Value::as_array_mut)
            .and_then(|streams| streams.first_mut())
            .and_then(|stream| stream.get_mut("packets"))
            .and_then(Value::as_array_mut)
            .and_then(|packets| packets.first_mut())
        {
            packet["pts"] = json!(1);
        }
        let different = compare_file(Level::Packets, &left, Some(&right)).expect("comparison");
        assert!(!different.agrees());
    }

    #[test]
    fn stream_comparison_separates_info_and_gating_fields() {
        let reference = stream_fixture(10, true);
        let mut candidate = reference.clone();
        candidate["streams"][0]["sample_rate"] = json!(48000);
        let informational =
            compare_file(Level::Streams, &reference, Some(&candidate)).expect("comparison");
        assert!(informational.agrees());
        assert_eq!(informational.informational_differences.len(), 1);
        candidate["streams"][0]["width"] = json!(11);
        let gating =
            compare_file(Level::Streams, &reference, Some(&candidate)).expect("comparison");
        assert!(!gating.agrees());
    }

    #[test]
    fn streams_pair_by_index_when_the_candidate_leaves_one_out() {
        // The reference lists streams 0 and 1; the candidate dropped 0.
        let mut reference = stream_fixture(10, true);
        let mut second = reference["streams"][0].clone();
        second["index"] = json!(1);
        second["kind"] = json!("audio");
        second["codec_name"] = json!("aac");
        reference["streams"] = json!([reference["streams"][0].clone(), second.clone()]);
        let mut candidate = reference.clone();
        candidate["streams"] = json!([second.clone()]);
        let result =
            compare_file(Level::Streams, &reference, Some(&candidate)).expect("comparison");
        let paths: Vec<_> = result
            .gating_differences
            .iter()
            .map(|d| d.path.clone())
            .collect();
        // Only the count: stream 1 is compared with stream 1, not with 0.
        assert_eq!(paths, ["streams/count"]);
        let mut packets = packets_fixture(&[json!(1), json!(2)], json!([1, 10]));
        let mut audio = packets["streams"][0].clone();
        audio["index"] = json!(1);
        audio["packets"] = json!([{"pos":3,"pts":0,"dts":0,"size":5,"keyframe":true,"discard":false,"duration":1}]);
        packets["streams"] = json!([packets["streams"][0].clone(), audio.clone()]);
        let mut kept = packets.clone();
        kept["streams"] = json!([audio]);
        let result = compare_file(Level::Packets, &packets, Some(&kept)).expect("comparison");
        assert_eq!((result.packets_agree, result.packets_disagree), (1, 2));
        let paths: Vec<_> = result
            .gating_differences
            .iter()
            .map(|d| d.path.clone())
            .collect();
        assert_eq!(paths, ["streams/count"]);
    }

    #[test]
    fn streams_with_equal_counts_but_other_indexes_disagree() {
        let reference = stream_fixture(10, true);
        let mut candidate = reference.clone();
        candidate["streams"][0]["index"] = json!(1);
        let result =
            compare_file(Level::Streams, &reference, Some(&candidate)).expect("comparison");
        assert!(!result.agrees());
        let paths: Vec<_> = result
            .gating_differences
            .iter()
            .map(|d| d.path.clone())
            .collect();
        assert_eq!(paths, ["streams/indexes"]);
    }

    #[test]
    fn packets_of_streams_with_other_indexes_disagree() {
        // Equal stream counts, other indexes: at the packets level too the
        // index set differs (`report_unpaired`), and every packet on both
        // sides is unpaired, never compared across streams. The reference
        // stream's packets are counted; the candidate's unpaired stream is
        // listed at its position with its packet count.
        let reference = packets_fixture(&[json!(1), json!(2), json!(3)], json!([1, 10]));
        let mut candidate = reference.clone();
        candidate["streams"][0]["index"] = json!(1);
        let result =
            compare_file(Level::Packets, &reference, Some(&candidate)).expect("comparison");
        assert!(!result.agrees());
        assert_eq!((result.packets_agree, result.packets_disagree), (0, 6));
        let paths: Vec<_> = result
            .gating_differences
            .iter()
            .map(|d| d.path.clone())
            .collect();
        assert_eq!(paths, ["streams/indexes", "streams/0/packets"]);
        let unpaired = &result.gating_differences[1];
        assert_eq!(
            (&unpaired.reference, &unpaired.candidate),
            (&Value::Null, &json!(3))
        );
    }

    #[test]
    fn runs_pair_by_position_without_expanding() {
        // The reference groups four 2-byte samples per packet; the candidate
        // lists all eight samples as one run.
        let mut reference = packets_fixture(&[json!(100), json!(108)], json!([1, 10]));
        for (i, packet) in reference["streams"][0]["packets"]
            .as_array_mut()
            .expect("packets")
            .iter_mut()
            .enumerate()
        {
            packet["size"] = json!(8);
            packet["pts"] = json!(i * 4);
            packet["dts"] = json!(i * 4);
            packet["duration"] = json!(4);
        }
        let mut candidate = packets_fixture(&[json!(100)], json!([1, 10]));
        candidate["streams"][0]["packets"][0]["size"] = json!(2);
        candidate["streams"][0]["packets"][0]["run"] = json!(8);
        let result =
            compare_file(Level::Packets, &reference, Some(&candidate)).expect("comparison");
        // Two reference packets pair with samples 0 and 4 (sizes differ);
        // the six other samples are reported once, as the run's unpaired.
        assert_eq!((result.packets_agree, result.packets_disagree), (0, 8));
        let runs: Vec<_> = result
            .gating_differences
            .iter()
            .filter(|d| d.path.ends_with("/extra_run"))
            .collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].candidate.get("unpaired"), Some(&json!(6)));
        // The sample at 108 is the run's fifth: pts 4.
        let paired: Vec<_> = result
            .gating_differences
            .iter()
            .filter(|d| d.path == "streams/0/packets/1/size")
            .collect();
        assert_eq!(paired.len(), 1);
        assert!(
            !result
                .gating_differences
                .iter()
                .any(|d| d.path.starts_with("streams/0/packets/1/pts"))
        );
    }

    /// The same comparison with the candidate's runs expanded.
    fn expanded(candidate: &Value) -> Value {
        let mut out = candidate.clone();
        let packets = expand_runs(packet_list(&candidate["streams"][0]));
        out["streams"][0]["packets"] = json!(packets);
        out
    }

    #[test]
    fn overlapping_runs_and_runs_without_positions_compare_as_expanded() {
        let reference = packets_fixture(&[json!(100), json!(100), json!(104)], json!([1, 10]));
        let run = |pos: Value, run: u64| json!({"pos":pos,"pts":0,"dts":0,"size":2,"keyframe":true,"discard":false,"duration":1,"run":run});
        let single = |pos: Value| json!({"pos":pos,"pts":0,"dts":0,"size":5,"keyframe":true,"discard":false,"duration":1});
        for packets in [
            json!([run(json!(100), 4), run(json!(100), 4)]),
            json!([run(json!(100), 10), single(json!(102))]),
            json!([run(json!(100), 4), single(Value::Null)]),
        ] {
            let mut candidate = packets_fixture(&[json!(1)], json!([1, 10]));
            candidate["streams"][0]["packets"] = packets;
            let runs =
                compare_file(Level::Packets, &reference, Some(&candidate)).expect("comparison");
            let plain = compare_file(Level::Packets, &reference, Some(&expanded(&candidate)))
                .expect("comparison");
            assert_eq!(
                (runs.packets_agree, runs.packets_disagree),
                (plain.packets_agree, plain.packets_disagree)
            );
        }
    }

    #[test]
    fn runs_must_be_well_formed() {
        let mut candidate = packets_fixture(&[json!(1)], json!([1, 10]));
        for run in [json!(1), json!(0), json!("x"), Value::Null] {
            candidate["streams"][0]["packets"][0]["run"] = run;
            assert!(validate_document(Level::Packets, &candidate).is_err());
        }
        candidate["streams"][0]["packets"][0]["run"] = json!(2);
        assert!(validate_document(Level::Packets, &candidate).is_ok());
    }

    #[test]
    fn reference_failures_are_excluded() {
        let reference = stream_fixture(10, false);
        let result = compare_file(Level::Streams, &reference, None).expect("comparison");
        assert!(result.reference_failure);
        assert!(result.agrees());
    }

    #[test]
    fn reference_failures_do_not_enter_stream_rate_denominator() {
        let root = std::env::temp_dir().join(format!(
            "coreav-oracle-reference-failure-{}",
            std::process::id()
        ));
        let reference_dir = root.join("reference");
        let candidate_dir = root.join("candidate");
        std::fs::create_dir_all(&reference_dir).expect("reference directory");
        std::fs::create_dir_all(&candidate_dir).expect("candidate directory");
        let valid = stream_fixture(10, true);
        let failed = json!({
            "schema":"coreav-oracle/streams/1","file":"failed.mp4","probe_ok":false
        });
        std::fs::write(
            reference_dir.join("valid.mp4.streams.json"),
            serde_json::to_vec(&valid).expect("valid document"),
        )
        .expect("write reference");
        std::fs::write(
            reference_dir.join("failed.mp4.streams.json"),
            serde_json::to_vec(&failed).expect("failure document"),
        )
        .expect("write failed reference");
        std::fs::write(
            candidate_dir.join("valid.mp4.streams.json"),
            serde_json::to_vec(&valid).expect("candidate document"),
        )
        .expect("write candidate");

        let results = compare_directories(
            &reference_dir,
            &candidate_dir,
            &[Level::Streams],
            "fixtures",
        )
        .expect("directory comparison");
        let result = results.first().expect("level result");
        assert_eq!(result.files, 1);
        assert_eq!(result.reference_failures, 1);
        assert_eq!(result.agree, 1);
        assert_eq!(result.total, 1);
        assert!(result.passed);
        std::fs::remove_dir_all(root).expect("remove temporary directory");
    }

    #[test]
    fn integer_threshold_math_profiles_and_minima() {
        assert!(threshold_met(1, 1, 100, 1, 0, 0));
        assert!(!threshold_met(99, 100, 100, 100, 0, 0));
        assert!(threshold_met(98, 100, 98, 100, 100, 0));
        assert!(!threshold_met(97, 100, 98, 100, 100, 0));
        assert!(!threshold_met(99, 100, 98, 99, 100, 0));
        assert!(threshold_met(99, 100, 99, 1, 0, 0));
        assert!(threshold_met(1_000, 1_000, 100, 10, 0, 1_000));
        assert!(!threshold_met(999, 999, 100, 10, 0, 1_000));
    }
}
