//! Evaluating a policy over a `coreav.probe/1` document (ISO base media)
//! or a `coreav.probe-matroska/1` document (Matroska and WebM; the rule
//! mapping is in `docs/design/policy-matroska.md`).

use crate::exact::Frac;
use crate::policy::{
    Action, Format, KINDS, KindRules, Policy, Seconds, SizeSource, Unidentified, pointer,
};
use crate::verdict::{Code, Reason, Verdict};
use core::cmp::Ordering;
use serde_json::{Value, json};

/// The probe schema of ISO base media files.
pub const PROBE_SCHEMA: &str = "coreav.probe/1";

/// The probe schema of Matroska and WebM files.
pub const MATROSKA_PROBE_SCHEMA: &str = "coreav.probe-matroska/1";

/// Which probe document a check reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Iso,
    Matroska,
}

/// Checks a `coreav.probe/1` or `coreav.probe-matroska/1` document (the
/// output of `coreav probe`, or of the library's serializer) against a
/// policy.
///
/// A pure function of its two arguments: the same policy and document give
/// the same verdict, reasons in the same order. It never fails: a value a
/// rule needs that the document leaves `null` or lacks gives a
/// [`Code::Unknown`] reason, so an unverifiable rule rejects the file.
pub fn evaluate(policy: &Policy, probe: &Value) -> Verdict {
    let mut check = Check {
        reasons: Vec::new(),
    };
    let shape = match probe.get("schema").and_then(Value::as_str) {
        Some(PROBE_SCHEMA) => Some(Shape::Iso),
        Some(MATROSKA_PROBE_SCHEMA) => Some(Shape::Matroska),
        _ => None,
    };
    if let Some(shape) = shape {
        check.document(policy, probe, shape);
    } else {
        check.push(Reason {
            code: Code::Unknown,
            rule: None,
            path: Some("/schema".to_owned()),
            stream_index: None,
            value: probe.get("schema").cloned().unwrap_or(Value::Null),
            exact: None,
            limit: json!([PROBE_SCHEMA, MATROSKA_PROBE_SCHEMA]),
            message: format!(
                "the document is not a {PROBE_SCHEMA} or {MATROSKA_PROBE_SCHEMA} probe output"
            ),
            error: None,
        });
    }
    Verdict {
        policy: policy.name.clone(),
        reasons: check.reasons,
    }
}

/// A value read from the document: what the reason prints and, for
/// durations and bit rates, the exact fraction compared.
struct Read {
    value: Value,
    exact: Option<Frac>,
}

impl Read {
    fn count(value: u64) -> Self {
        Self {
            value: json!(value),
            exact: None,
        }
    }

    fn compared(&self) -> Option<Frac> {
        self.exact.or_else(|| self.value.as_u64().map(Frac::int))
    }
}

/// Where a reason points, and how its message names the subject.
struct At<'a> {
    rule: String,
    path: Option<String>,
    stream_index: Option<u64>,
    subject: &'a str,
    what: &'a str,
    unit: &'a str,
}

struct Check {
    reasons: Vec<Reason>,
}

impl Check {
    fn push(&mut self, reason: Reason) {
        self.reasons.push(reason);
    }

    fn reason(
        &mut self,
        at: &At<'_>,
        code: Code,
        read: Option<&Read>,
        limit: Value,
        message: String,
    ) {
        self.push(Reason {
            code,
            rule: Some(at.rule.clone()),
            path: at.path.clone(),
            stream_index: at.stream_index,
            value: read.map_or(Value::Null, |r| r.value.clone()),
            exact: read.and_then(|r| r.exact).and_then(Frac::reduced),
            limit,
            message,
            error: None,
        });
    }

    fn unknown(&mut self, at: &At<'_>, limit: Value) {
        let message = format!("{}: {} is unknown", at.subject, at.what);
        self.reason(at, Code::Unknown, None, limit, message);
    }

    /// `value` must be at most `limit`.
    fn maximum(&mut self, at: &At<'_>, read: Option<Read>, limit: Frac, limit_json: Value) {
        let Some(read) = read else {
            return self.unknown(at, limit_json);
        };
        let Some(value) = read.compared() else {
            return self.unknown(at, limit_json);
        };
        if value.cmp_exact(limit) == Ordering::Greater {
            let message = format!(
                "{}: {} {}{} is above the maximum {}{}",
                at.subject, at.what, read.value, at.unit, limit_json, at.unit
            );
            self.reason(at, Code::AboveMaximum, Some(&read), limit_json, message);
        }
    }

    fn maximum_int(&mut self, at: &At<'_>, read: Option<Read>, limit: Option<u64>) {
        if let Some(limit) = limit {
            self.maximum(at, read, Frac::int(limit), json!(limit));
        }
    }

    fn minimum_int(&mut self, at: &At<'_>, value: u64, limit: Option<u64>) {
        if let Some(limit) = limit.filter(|l| value < *l) {
            let read = Read::count(value);
            let message = format!(
                "{}: {} {} is below the minimum {}",
                at.subject, at.what, value, limit
            );
            self.reason(at, Code::BelowMinimum, Some(&read), json!(limit), message);
        }
    }

    fn not_allowed(&mut self, at: &At<'_>, value: Value, limit: Value, message: String) {
        let read = Read { value, exact: None };
        self.reason(at, Code::NotAllowed, Some(&read), limit, message);
    }

    fn document(&mut self, policy: &Policy, doc: &Value, shape: Shape) {
        let streams = doc
            .get("streams")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let dropped = doc
            .get("dropped_streams")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let size = doc.pointer("/file/size").and_then(Value::as_u64);
        let duration = seconds(doc.get("duration"));
        // A Matroska file without `Duration` (a live recording): the
        // longest stream, when every stream's duration is known.
        let file_duration = match (shape, &duration) {
            (Shape::Matroska, None) if doc.get("duration") == Some(&Value::Null) => {
                longest_stream(streams)
            }
            _ => duration.as_ref().map(|d| Read {
                value: d.value.clone(),
                exact: d.exact,
            }),
        };

        self.maximum_int(
            &At {
                rule: "/file/max_size".to_owned(),
                path: Some("/file/size".to_owned()),
                stream_index: None,
                subject: "file",
                what: "size",
                unit: " bytes",
            },
            size.map(Read::count),
            policy.file.max_size,
        );
        self.maximum_int(
            &At {
                rule: "/file/max_bit_rate".to_owned(),
                path: None,
                stream_index: None,
                subject: "file",
                what: "overall bit rate",
                unit: " bits/s",
            },
            size.zip(file_duration.as_ref())
                .and_then(|(size, duration)| overall_bit_rate(size, duration)),
            policy.file.max_bit_rate,
        );
        self.container(policy, doc, shape);
        if let Some(max) = &policy.duration.max_seconds {
            // A missing Matroska `Duration` does not count when every
            // stream's duration is known.
            let file = match (shape, duration) {
                (Shape::Matroska, None)
                    if doc.get("duration") == Some(&Value::Null)
                        && longest_stream(streams).is_some() =>
                {
                    None
                }
                (_, duration) => Some(duration),
            };
            self.duration(streams, file, max);
        }
        self.maximum_int(
            &At {
                rule: "/streams/max_count".to_owned(),
                path: None,
                stream_index: None,
                subject: "file",
                what: "stream count",
                unit: "",
            },
            Some(Read::count(
                u64::try_from(streams.len().saturating_add(dropped.len())).unwrap_or(u64::MAX),
            )),
            policy.streams.max_count,
        );
        for kind in KINDS {
            let Some(rules) = policy.streams.kind(kind) else {
                continue;
            };
            let count = streams
                .iter()
                .filter(|s| policy_kind(s).0 == Some(kind))
                .count();
            let count = u64::try_from(count).unwrap_or(u64::MAX);
            let what = format!("{kind} stream count");
            let at = |key: &str| At {
                rule: pointer(&pointer("/streams", kind), key),
                path: None,
                stream_index: None,
                subject: "file",
                what: &what,
                unit: "",
            };
            self.minimum_int(&at("min_count"), count, rules.min_count);
            self.maximum_int(&at("max_count"), Some(Read::count(count)), rules.max_count);
        }
        for (i, stream) in streams.iter().enumerate() {
            self.stream(policy, stream, &format!("/streams/{i}"), shape);
        }
        let allowed: &[String] = match &policy.streams.unidentified {
            Unidentified::Ignore => return,
            Unidentified::Reject => &[],
            Unidentified::Allow(fourccs) => fourccs,
        };
        if shape == Shape::Matroska {
            self.encodings(streams);
        }
        let key = match shape {
            Shape::Iso => "fourcc",
            Shape::Matroska => "codec_id",
        };
        for (j, stream) in dropped.iter().enumerate() {
            let fourcc = stream.get(key).cloned().unwrap_or(Value::Null);
            if fourcc
                .as_str()
                .is_some_and(|f| allowed.iter().any(|a| a == f))
            {
                continue;
            }
            let stream_index = stream.get("stream_index").and_then(Value::as_u64);
            let subject = stream_label(stream_index, None);
            let message = format!(
                "{subject}: codec {fourcc} is not one CoreAV identifies, and the policy does not \
                 list it in streams.unidentified"
            );
            self.not_allowed(
                &At {
                    rule: "/streams/unidentified".to_owned(),
                    path: Some(format!("/dropped_streams/{j}/{key}")),
                    stream_index,
                    subject: &subject,
                    what: "codec",
                    unit: "",
                },
                fourcc,
                json!(allowed),
                message,
            );
        }
    }

    /// The longest of the file's duration and every stream's must be at
    /// most the limit: one reason, at the longest value (the first of equal
    /// ones), or at the first unknown one. Streams count because a movie
    /// header can understate how long the media lasts.
    fn duration(&mut self, streams: &[Value], file: Option<Option<Read>>, max: &Seconds) {
        let limit = Value::Number(max.number.clone());
        // (subject, path, stream_index, value) of the file, then each stream.
        let mut candidates = Vec::new();
        if let Some(file) = file {
            candidates.push(("file".to_owned(), "/duration".to_owned(), None, file));
        }
        for (i, stream) in streams.iter().enumerate() {
            let stream_index = stream.get("stream_index").and_then(Value::as_u64);
            let kind = policy_kind(stream).0;
            candidates.push((
                stream_label(stream_index, kind),
                format!("/streams/{i}/duration"),
                stream_index,
                seconds(stream.get("duration")),
            ));
        }
        let mut longest: Option<(usize, Frac)> = None;
        let mut unknown = None;
        for (i, (_, _, _, read)) in candidates.iter().enumerate() {
            match read.as_ref().and_then(|r| r.exact) {
                None => {
                    unknown = Some(i);
                    break;
                }
                Some(exact) => {
                    if longest.is_none_or(|(_, best)| exact.cmp_exact(best) == Ordering::Greater) {
                        longest = Some((i, exact));
                    }
                }
            }
        }
        let Some(chosen) = unknown.or(longest.map(|(i, _)| i)) else {
            return;
        };
        let Some((subject, path, stream_index, read)) = candidates.into_iter().nth(chosen) else {
            return;
        };
        let at = At {
            rule: "/duration/max_seconds".to_owned(),
            path: Some(path),
            stream_index,
            subject: &subject,
            what: "duration",
            unit: " s",
        };
        self.maximum(&at, read, max.exact, limit);
    }

    fn container(&mut self, policy: &Policy, doc: &Value, shape: Shape) {
        if shape == Shape::Matroska {
            return self.matroska_container(policy, doc);
        }
        if let Some(formats) = &policy.container.formats {
            let names: Vec<&str> = formats.iter().map(|f| f.name()).collect();
            let at = At {
                rule: "/container/formats".to_owned(),
                path: Some("/container/major_brand".to_owned()),
                stream_index: None,
                subject: "file",
                what: "container format",
                unit: "",
            };
            let brand = match doc.pointer("/container/major_brand") {
                Some(Value::Null) => Some(None),
                Some(Value::String(brand)) => Some(Some(brand.as_str())),
                _ => None,
            };
            match brand {
                None => self.unknown(&at, json!(names)),
                Some(brand) => {
                    let format = Format::of_major_brand(brand);
                    if !formats.contains(&format) {
                        let described = brand.map_or_else(
                            || "no ftyp box".to_owned(),
                            |b| format!("major brand {}", json!(b)),
                        );
                        let message = format!(
                            "file: container format \"{}\" ({described}) is not allowed; \
                             allowed: {}",
                            format.name(),
                            names.join(", ")
                        );
                        self.not_allowed(&at, json!(format.name()), json!(names), message);
                    }
                }
            }
        }
        if !policy.container.fragmented {
            let at = At {
                rule: "/container/fragmented".to_owned(),
                path: Some("/container/fragmented".to_owned()),
                stream_index: None,
                subject: "file",
                what: "fragmentation",
                unit: "",
            };
            match doc
                .pointer("/container/fragmented")
                .and_then(Value::as_bool)
            {
                None => self.unknown(&at, json!(false)),
                Some(true) => self.not_allowed(
                    &at,
                    json!(true),
                    json!(false),
                    "file: fragmented files are not allowed".to_owned(),
                ),
                Some(false) => {}
            }
        }
    }

    /// `container.formats` from the `DocType`; `container.fragmented` from
    /// `live`, a Segment of unknown size (written while recording).
    fn matroska_container(&mut self, policy: &Policy, doc: &Value) {
        if let Some(formats) = &policy.container.formats {
            let names: Vec<&str> = formats.iter().map(|f| f.name()).collect();
            let at = At {
                rule: "/container/formats".to_owned(),
                path: Some("/container/doc_type".to_owned()),
                stream_index: None,
                subject: "file",
                what: "container format",
                unit: "",
            };
            match doc
                .pointer("/container/doc_type")
                .and_then(Value::as_str)
                .and_then(Format::of_doc_type)
            {
                None => self.unknown(&at, json!(names)),
                Some(format) if formats.contains(&format) => {}
                Some(format) => {
                    let message = format!(
                        "file: container format \"{}\" (DocType \"{}\") is not allowed; \
                         allowed: {}",
                        format.name(),
                        format.name(),
                        names.join(", ")
                    );
                    self.not_allowed(&at, json!(format.name()), json!(names), message);
                }
            }
        }
        if !policy.container.fragmented {
            let at = At {
                rule: "/container/fragmented".to_owned(),
                path: Some("/container/live".to_owned()),
                stream_index: None,
                subject: "file",
                what: "live recording",
                unit: "",
            };
            match doc.pointer("/container/live").and_then(Value::as_bool) {
                None => self.unknown(&at, json!(false)),
                Some(true) => self.not_allowed(
                    &at,
                    json!(true),
                    json!(false),
                    "file: live recordings (a Segment of unknown size) are not allowed, as \
                     fragmented files"
                        .to_owned(),
                ),
                Some(false) => {}
            }
        }
    }

    /// Streams whose frames are encrypted or encoded in a way the probe
    /// cannot undo: what they carry is unknown, so `streams.unidentified`
    /// rejects them unless it is "ignore", whatever its list names.
    fn encodings(&mut self, streams: &[Value]) {
        for (i, stream) in streams.iter().enumerate() {
            let encoding = stream.get("encoding").cloned().unwrap_or(Value::Null);
            if matches!(encoding.as_str(), Some("none" | "header_stripping")) {
                continue;
            }
            let stream_index = stream.get("stream_index").and_then(Value::as_u64);
            let subject = stream_label(stream_index, policy_kind(stream).0);
            let message = format!(
                "{subject}: frames with encoding {encoding} cannot be read, so what they carry \
                 is not identified"
            );
            let at = At {
                rule: "/streams/unidentified".to_owned(),
                path: Some(format!("/streams/{i}/encoding")),
                stream_index,
                subject: &subject,
                what: "encoding",
                unit: "",
            };
            self.not_allowed(&at, encoding, json!(["none", "header_stripping"]), message);
        }
    }

    fn stream(&mut self, policy: &Policy, stream: &Value, base: &str, shape: Shape) {
        let stream_index = stream.get("stream_index").and_then(Value::as_u64);
        let (kind, kind_key) = policy_kind(stream);
        let subject = stream_label(stream_index, kind);
        let at = |rule: String, key: Option<&str>, what: &'static str, unit: &'static str| At {
            rule,
            path: key.map(|k| format!("{base}/{k}")),
            stream_index,
            subject: &subject,
            what,
            unit,
        };

        let rules = match kind {
            None => {
                self.unknown(
                    &at("/streams".to_owned(), Some(kind_key), "kind", ""),
                    json!(policy.streams.allowed_kinds()),
                );
                None
            }
            Some(kind) => {
                let rules = policy.streams.kind(kind);
                if rules.is_none() {
                    let allowed = policy.streams.allowed_kinds();
                    let message = format!(
                        "{subject}: {kind} streams are not allowed; allowed: {}",
                        if allowed.is_empty() {
                            "none".to_owned()
                        } else {
                            allowed.join(", ")
                        }
                    );
                    self.not_allowed(
                        &at(pointer("/streams", kind), Some(kind_key), "kind", ""),
                        json!(kind),
                        json!(allowed),
                        message,
                    );
                }
                rules.map(|rules| (kind, rules))
            }
        };
        if let Some((kind, rules)) = rules {
            let rule = |key: &str| pointer(&pointer("/streams", kind), key);
            self.codec(
                rules,
                stream,
                &at(rule("codecs"), Some("codec"), "codec", ""),
            );
            self.maximum_int(
                &at(
                    rule("max_bit_rate"),
                    Some("bit_rate"),
                    "bit rate",
                    " bits/s",
                ),
                bit_rate(stream.get("bit_rate")),
                rules.max_bit_rate,
            );
            let count = |key: &str| stream.pointer(key).and_then(Value::as_u64);
            if let (Some(video), "video") = (&policy.streams.video, kind) {
                // The larger of the declared size and the verified one
                // (docs/design/frame-sizes.md, decision 4).
                let verified = match video.size_source {
                    SizeSource::Scan => Some("scanned_size"),
                    SizeSource::Configuration => Some("parameter_set_size"),
                    SizeSource::Declared => None,
                };
                let side = |dim: &str| -> (Option<u64>, String) {
                    let declared = count(&format!("/video/{dim}"));
                    let declared_path = format!("video/{dim}");
                    match (declared, verified) {
                        (None, _) => (None, declared_path),
                        (Some(d), None) => (Some(d), declared_path),
                        (Some(d), Some(key)) => {
                            let path = format!("video/{key}/{dim}");
                            match count(&format!("/{path}")) {
                                None => (None, path),
                                Some(v) if v > d => (Some(v), path),
                                Some(_) => (Some(d), declared_path),
                            }
                        }
                    }
                };
                let (width, width_path) = side("width");
                let (height, height_path) = side("height");
                self.maximum_int(
                    &at(rule("max_width"), Some(&width_path), "width", " pixels"),
                    width.map(Read::count),
                    video.max_width,
                );
                self.maximum_int(
                    &at(rule("max_height"), Some(&height_path), "height", " pixels"),
                    height.map(Read::count),
                    video.max_height,
                );
                // The larger picture of the two, by area.
                let area =
                    |w: Option<u64>, h: Option<u64>| w.zip(h).and_then(|(w, h)| w.checked_mul(h));
                let declared = area(count("/video/width"), count("/video/height"));
                let pixels = match verified {
                    None => declared,
                    Some(key) => area(
                        count(&format!("/video/{key}/width")),
                        count(&format!("/video/{key}/height")),
                    )
                    .zip(declared)
                    .map(|(v, d)| v.max(d)),
                };
                self.maximum_int(
                    &at(rule("max_pixels"), None, "width × height", " pixels"),
                    pixels.map(Read::count),
                    video.max_pixels,
                );
                if let Some(rotations) = &video.rotations {
                    self.rotation(
                        stream,
                        shape,
                        rotations,
                        &at(rule("rotations"), Some("video/rotation"), "rotation", ""),
                    );
                }
            }
            if let (Some(audio), "audio") = (&policy.streams.audio, kind) {
                self.maximum_int(
                    &at(
                        rule("max_channels"),
                        Some("audio/channels"),
                        "channel count",
                        "",
                    ),
                    count("/audio/channels").map(Read::count),
                    audio.max_channels,
                );
                self.maximum_int(
                    &at(
                        rule("max_sample_rate"),
                        Some("audio/sample_rate"),
                        "sample rate",
                        " Hz",
                    ),
                    count("/audio/sample_rate").map(Read::count),
                    audio.max_sample_rate,
                );
            }
        }
        if policy.streams.invalid_tables == Action::Reject {
            let (key, what, problem) = match shape {
                Shape::Iso => (
                    "tables_valid",
                    "sample table validity",
                    "the sample tables are inconsistent (no packets)",
                ),
                Shape::Matroska => (
                    "frames_listed",
                    "frame listing",
                    "the frames cannot all be listed (an encoding the probe does not undo, or a \
                     block structure the scan cannot account for)",
                ),
            };
            let at = at("/streams/invalid_tables".to_owned(), Some(key), what, "");
            match stream.get(key).and_then(Value::as_bool) {
                None => self.unknown(&at, json!(true)),
                Some(false) => {
                    let message = format!("{subject}: {problem}");
                    self.not_allowed(&at, json!(false), json!(true), message);
                }
                Some(true) => {}
            }
        }
        // Matroska has no external data references and one CodecPrivate
        // per track: the two rules do not apply.
        if shape == Shape::Matroska {
            return;
        }
        if policy.streams.external_data == Action::Reject {
            let at = at(
                "/streams/external_data".to_owned(),
                Some("external_data"),
                "data location",
                "",
            );
            match stream.get("external_data").and_then(Value::as_bool) {
                None => self.unknown(&at, json!(false)),
                Some(true) => {
                    let message = format!("{subject}: the data is in another file");
                    self.not_allowed(&at, json!(true), json!(false), message);
                }
                Some(false) => {}
            }
        }
        if policy.streams.multiple_descriptions == Action::Reject {
            let at = at(
                "/streams/multiple_descriptions".to_owned(),
                Some("sample_descriptions"),
                "sample description count",
                "",
            );
            match stream.get("sample_descriptions").and_then(Value::as_u64) {
                None => self.unknown(&at, json!(1)),
                Some(1) => {}
                Some(count) => {
                    let message = format!(
                        "{subject}: {count} sample descriptions; only the first is described, \
                         so the codec and size rules cannot check the others"
                    );
                    self.not_allowed(&at, json!(count), json!(1), message);
                }
            }
        }
    }

    fn codec(&mut self, rules: &KindRules, stream: &Value, at: &At<'_>) {
        let Some(codecs) = &rules.codecs else {
            return;
        };
        match stream.get("codec").and_then(Value::as_str) {
            None => self.unknown(at, json!(codecs)),
            Some(codec) if codecs.contains(&codec) => {}
            Some(codec) => {
                let message = format!(
                    "{}: codec \"{codec}\" is not allowed; allowed: {}",
                    at.subject,
                    if codecs.is_empty() {
                        "none".to_owned()
                    } else {
                        codecs.join(", ")
                    }
                );
                self.not_allowed(at, json!(codec), json!(codecs), message);
            }
        }
    }

    fn rotation(&mut self, stream: &Value, shape: Shape, rotations: &[u64], at: &At<'_>) {
        match stream.pointer("/video/rotation") {
            Some(Value::Null) => {
                let what = match shape {
                    Shape::Iso => "the display matrix",
                    Shape::Matroska => "the projection pose",
                };
                let message = format!("{}: {what} is not a quarter-turn rotation", at.subject);
                self.not_allowed(at, Value::Null, json!(rotations), message);
            }
            Some(value) => match value.as_u64() {
                Some(rotation) if rotations.contains(&rotation) => {}
                Some(rotation) => {
                    let message = format!(
                        "{}: rotation {rotation} degrees is not allowed; allowed: {}",
                        at.subject,
                        rotations
                            .iter()
                            .map(u64::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    self.not_allowed(at, json!(rotation), json!(rotations), message);
                }
                None => self.unknown(at, json!(rotations)),
            },
            None => self.unknown(at, json!(rotations)),
        }
    }
}

/// The kind a stream's rules come from, and the key of the probe value it
/// was read from: the family of its codec (a video codec gets the video
/// rules whatever its handler says), else the probe's `kind`, which comes
/// from the handler.
fn policy_kind(stream: &Value) -> (Option<&str>, &'static str) {
    let family = match stream.get("codec").and_then(Value::as_str) {
        Some(
            "h264" | "hevc" | "av1" | "vp8" | "vp9" | "prores" | "mpeg1video" | "mpeg2video"
            | "mpeg4" | "theora" | "mjpeg" | "ffv1",
        ) => Some("video"),
        Some(
            "aac" | "mp3" | "mp2" | "opus" | "vorbis" | "flac" | "ac3" | "eac3" | "dts" | "truehd"
            | "alac" | "pcm",
        ) => Some("audio"),
        Some("timed_text" | "subrip" | "ass" | "webvtt" | "pgs" | "vobsub" | "dvbsub") => {
            Some("subtitle")
        }
        Some("timecode") => Some("timecode"),
        _ => None,
    };
    match family {
        Some(kind) => (Some(kind), "codec"),
        None => (stream.get("kind").and_then(Value::as_str), "kind"),
    }
}

/// How messages name a stream: `stream 1 (audio)`.
fn stream_label(stream_index: Option<u64>, kind: Option<&str>) -> String {
    let index = stream_index.map_or_else(|| "?".to_owned(), |i| i.to_string());
    match kind {
        Some(kind) => format!("stream {index} ({kind})"),
        None => format!("stream {index}"),
    }
}

/// A probe time (`ticks`, `timescale`, `seconds`) in exact seconds; the
/// printed value is its `seconds`.
fn seconds(time: Option<&Value>) -> Option<Read> {
    let time = time?;
    let ticks = time.get("ticks")?;
    let num = u128::from(ticks.get("num")?.as_u64()?);
    let den = u128::from(ticks.get("den")?.as_u64()?);
    let timescale = u128::from(time.get("timescale")?.as_u64()?);
    let exact = Frac::new(num, den.checked_mul(timescale)?)?;
    let value = match time.get("seconds") {
        Some(Value::Number(n)) => Value::Number(n.clone()),
        _ => json!(exact.to_f64()),
    };
    Some(Read {
        value,
        exact: Some(exact),
    })
}

/// A probe bit rate (`exact`, `bits_per_second`); the printed value is its
/// `bits_per_second`.
fn bit_rate(rate: Option<&Value>) -> Option<Read> {
    let rate = rate?;
    let exact = rate.get("exact")?;
    let num = u128::from(exact.get("num")?.as_u64()?);
    let exact = Frac::new(num, u128::from(exact.get("den")?.as_u64()?))?;
    let value = match rate.get("bits_per_second") {
        Some(Value::Number(n)) => Value::Number(n.clone()),
        _ => json!(exact.to_f64()),
    };
    Some(Read {
        value,
        exact: Some(exact),
    })
}

/// The longest stream duration, when there is a stream and every stream's
/// duration is known.
fn longest_stream(streams: &[Value]) -> Option<Read> {
    let mut longest: Option<Read> = None;
    for stream in streams {
        let read = seconds(stream.get("duration"))?;
        let exact = read.exact?;
        if longest
            .as_ref()
            .and_then(|l| l.exact)
            .is_none_or(|best| exact.cmp_exact(best) == Ordering::Greater)
        {
            longest = Some(read);
        }
    }
    longest
}

/// File size × 8 over the duration in seconds; unknown for a zero duration.
fn overall_bit_rate(size: u64, duration: &Read) -> Option<Read> {
    let duration = duration.exact?;
    let num = u128::from(size).checked_mul(8)?.checked_mul(duration.den)?;
    let exact = Frac::new(num, duration.num)?;
    Some(Read {
        value: json!(exact.to_f64()),
        exact: Some(exact),
    })
}
