use super::DecoderEligibilityKind;
use coreav_audio::Codec;
use serde_json::Value;
use std::collections::HashSet;

const RECORD: &str = include_str!("../../../../../docs/compat/audio-decoder-eligibility.json");
const ORACLE: &str = include_str!("../../../../../scripts/oracle-ffmpeg.env");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Requirement {
    Mpeg1Layer3,
    AacLc,
    SupportedMp4Timeline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Admission {
    pub codec: Codec,
    pub kind: DecoderEligibilityKind,
    pub requirements: Vec<Requirement>,
}

fn keys(value: &Value, expected: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
    })
}

pub(super) fn admission(codec: Codec, container: &str) -> Option<Admission> {
    parse(RECORD, codec, container)
}

fn parse(text: &str, codec: Codec, container: &str) -> Option<Admission> {
    let root: Value = serde_json::from_str(text).ok()?;
    if !keys(&root, &["schema", "oracle_version", "decoders"])
        || root.get("schema")?.as_str()? != "coreav.audio-decoder-eligibility/1"
        || Some(root.get("oracle_version")?.as_str()?)
            != ORACLE
                .lines()
                .find_map(|line| line.strip_prefix("VERSION="))
    {
        return None;
    }
    let mut names = HashSet::new();
    let mut admitted = None;
    for row in root.get("decoders")?.as_array()? {
        if !keys(
            row,
            &[
                "codec",
                "kind",
                "decoder_eligible",
                "presentations",
                "evidence",
                "limitations",
            ],
        ) {
            return None;
        }
        let name = row.get("codec")?.as_str()?;
        if !matches!(name, "flac" | "mp3" | "opus" | "aac") || !names.insert(name) {
            return None;
        }
        let kind = match row.get("kind")?.as_str()? {
            "exact" => DecoderEligibilityKind::Exact,
            "conformance" => DecoderEligibilityKind::Conformance,
            _ => return None,
        };
        let eligible = row.get("decoder_eligible")?.as_bool()?;
        let evidence = row.get("evidence")?.as_str()?;
        if !evidence.starts_with("docs/")
            || evidence.contains("..")
            || !row
                .get("limitations")?
                .as_array()?
                .iter()
                .all(|item| item.as_str().is_some_and(|text| !text.is_empty()))
        {
            return None;
        }
        let mut containers = HashSet::new();
        for presentation in row.get("presentations")?.as_array()? {
            if !keys(presentation, &["container", "eligible", "requirements"]) {
                return None;
            }
            let family = presentation.get("container")?.as_str()?;
            if !matches!(family, "mp4" | "matroska") || !containers.insert(family) {
                return None;
            }
            let presented = presentation.get("eligible")?.as_bool()?;
            let mut requirements = Vec::new();
            for predicate in presentation.get("requirements")?.as_array()? {
                let requirement = match predicate.as_str()? {
                    "mpeg1_layer3" => Requirement::Mpeg1Layer3,
                    "aac_lc" => Requirement::AacLc,
                    "supported_mp4_timeline" => Requirement::SupportedMp4Timeline,
                    _ => return None,
                };
                if requirements.contains(&requirement) {
                    return None;
                }
                if requirement == Requirement::SupportedMp4Timeline && family != "mp4" {
                    return None;
                }
                requirements.push(requirement);
            }
            if name == codec.name() && family == container && eligible && presented {
                admitted = Some(Admission {
                    codec,
                    kind,
                    requirements,
                });
            }
        }
    }
    admitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record() -> Value {
        json!({"schema": "coreav.audio-decoder-eligibility/1", "oracle_version": "9.0.2",
            "decoders": [{"codec": "mp3", "kind": "conformance", "decoder_eligible": true,
                "presentations": [{"container": "mp4", "eligible": true,
                    "requirements": ["mpeg1_layer3", "supported_mp4_timeline"]}],
                "evidence": "docs/design/audio-decode.md#mp3-bars", "limitations": ["Not byte-exact"]}]})
    }

    fn accepted(value: &Value) -> Option<Admission> {
        parse(&value.to_string(), Codec::Mp3, "mp4")
    }

    #[test]
    fn registered_decoder_contract_does_not_relax_conversion() {
        let result = accepted(&record());
        assert!(result.is_some(), "valid decoder record refused");
        let Some(admission) = result else {
            return;
        };
        assert_eq!(admission.kind, DecoderEligibilityKind::Conformance);
        assert_eq!(
            admission.requirements,
            [Requirement::Mpeg1Layer3, Requirement::SupportedMp4Timeline]
        );
        assert!(parse(RECORD, Codec::Opus, "mp4").is_none());
    }

    #[test]
    fn empty_missing_unknown_and_disabled_entries_deny() {
        for text in [
            "{}",
            "not json",
            r#"{"schema":"coreav.audio-decoder-eligibility/1","oracle_version":"9.0.2","decoders":[]}"#,
        ] {
            assert!(parse(text, Codec::Mp3, "mp4").is_none());
        }
        for path in ["decoder_eligible", "evidence", "kind"] {
            let mut value = record();
            if let Some(row) = value
                .get_mut("decoders")
                .and_then(Value::as_array_mut)
                .and_then(|rows| rows.first_mut())
                .and_then(Value::as_object_mut)
            {
                row.remove(path);
            }
            assert!(accepted(&value).is_none());
        }
        assert!(parse(&record().to_string(), Codec::Mp3, "matroska").is_none());
    }

    #[test]
    fn unknown_requirements_and_duplicate_rows_deny() {
        let mut value = record();
        if let Some(rows) = value.get_mut("decoders").and_then(Value::as_array_mut) {
            if let Some(row) = rows.first().cloned() {
                rows.push(row);
            }
        }
        assert!(accepted(&value).is_none());
        let mut value = record();
        if let Some(row) = value
            .get_mut("decoders")
            .and_then(Value::as_array_mut)
            .and_then(|rows| rows.first_mut())
            .and_then(|row| row.get_mut("presentations"))
            .and_then(Value::as_array_mut)
            .and_then(|rows| rows.first_mut())
            .and_then(|row| row.get_mut("requirements"))
            .and_then(Value::as_array_mut)
        {
            row.push(Value::String("trust_decoder".to_owned()));
        }
        assert!(accepted(&value).is_none());
    }

    #[test]
    fn disabled_and_unknown_fields_never_enable_a_decoder() {
        let original = record().to_string();
        for (old, new) in [
            ("\"decoder_eligible\":true", "\"decoder_eligible\":false"),
            ("\"eligible\":true", "\"eligible\":false"),
            ("\"conformance\"", "\"approximate\""),
            ("\"9.0.2\"", "\"0.0.0\""),
            ("\"mpeg1_layer3\"", "\"unknown_requirement\""),
            ("\"codec\":\"mp3\"", "\"codec\":\"mp3\",\"allow_all\":true"),
        ] {
            assert!(parse(&original.replace(old, new), Codec::Mp3, "mp4").is_none());
        }
    }
}
