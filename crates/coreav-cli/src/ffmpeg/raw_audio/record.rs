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
    OpusMapping0,
    SupportedMp4Timeline,
    AacSeparablePnsOrNoNoise,
    FfmpegAacMp4Timeline,
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
        let tolerance = row.get("kind")?.as_str()? == "decode_tolerance";
        let mut fields = vec![
            "codec",
            "kind",
            "decoder_eligible",
            "presentations",
            "evidence",
            "limitations",
        ];
        if tolerance {
            fields.push("tolerance");
        }
        if !keys(row, &fields) {
            return None;
        }
        if tolerance
            && (row.get("codec")?.as_str()? != "aac"
                || row.get("tolerance")?
                    != &serde_json::json!({
                        "f32_absolute": 0.0000152587890625, "s16_lsb": 1,
                        "sample_count": "exact", "priming": "exact", "timeline": "exact", "feature": "aac",
                        "pns": {"energy_db": 0.1, "sample_domain": "non_pns_projection", "packing": "per_decoder_f32"}
                    }))
        {
            return None;
        }
        let name = row.get("codec")?.as_str()?;
        if !matches!(name, "flac" | "mp3" | "opus" | "aac") || !names.insert(name) {
            return None;
        }
        let kind = match row.get("kind")?.as_str()? {
            "exact" => DecoderEligibilityKind::Exact,
            "conformance" => DecoderEligibilityKind::Conformance,
            "decode_tolerance" => DecoderEligibilityKind::DecodeTolerance,
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
                    "opus_mapping0" => Requirement::OpusMapping0,
                    "supported_mp4_timeline" => Requirement::SupportedMp4Timeline,
                    "aac_separable_pns_or_no_noise" => Requirement::AacSeparablePnsOrNoNoise,
                    "ffmpeg_aac_mp4_timeline" => Requirement::FfmpegAacMp4Timeline,
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
            if name == "aac"
                && eligible
                && presented
                && (!tolerance
                    || family != "mp4"
                    || requirements
                        != [
                            Requirement::AacLc,
                            Requirement::AacSeparablePnsOrNoNoise,
                            Requirement::FfmpegAacMp4Timeline,
                        ])
            {
                return None;
            }
            if name == codec.name() && family == container && eligible && presented {
                if name == "aac" && !cfg!(feature = "aac") {
                    return None;
                }
                if name == "opus"
                    && (family != "mp4"
                        || !requirements.contains(&Requirement::OpusMapping0)
                        || !requirements.contains(&Requirement::SupportedMp4Timeline))
                {
                    return None;
                }
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
        assert_eq!(
            parse(RECORD, Codec::Opus, "mp4").map(|row| row.kind),
            Some(DecoderEligibilityKind::Conformance)
        );
    }

    #[test]
    fn checked_in_record_admits_only_the_qualified_mp3_presentation() {
        let mp3 = admission(Codec::Mp3, "mp4");
        assert_eq!(
            mp3,
            Some(Admission {
                codec: Codec::Mp3,
                kind: DecoderEligibilityKind::Conformance,
                requirements: vec![Requirement::Mpeg1Layer3, Requirement::SupportedMp4Timeline],
            })
        );
        for codec in [Codec::Mp3, Codec::Opus] {
            assert!(admission(codec, "matroska").is_none());
        }
        assert_eq!(
            admission(Codec::Opus, "mp4"),
            Some(Admission {
                codec: Codec::Opus,
                kind: DecoderEligibilityKind::Conformance,
                requirements: vec![Requirement::OpusMapping0, Requirement::SupportedMp4Timeline],
            })
        );
        #[cfg(feature = "aac")]
        {
            assert_eq!(
                admission(Codec::Aac, "mp4").map(|row| row.kind),
                Some(DecoderEligibilityKind::DecodeTolerance)
            );
            assert!(admission(Codec::Aac, "matroska").is_none());
        }
    }

    #[test]
    #[allow(clippy::unwrap_used, clippy::indexing_slicing)]
    fn tolerance_contract_and_predicates_cannot_be_relaxed() {
        let record: Value = serde_json::from_str(RECORD).unwrap();
        let index = record["decoders"]
            .as_array()
            .unwrap()
            .iter()
            .position(|row| row["codec"] == "aac")
            .unwrap();
        for (field, value) in [
            ("f32_absolute", json!(0.0001)),
            ("s16_lsb", json!(2)),
            ("s16_lsb", json!(true)),
            ("s16_lsb", json!(1.0)),
            ("sample_count", json!("approximate")),
            ("priming", json!("approximate")),
            ("timeline", json!("approximate")),
            ("feature", json!("default")),
            ("unknown", json!(true)),
        ] {
            let mut changed = record.clone();
            changed["decoders"][index]["tolerance"][field] = value;
            assert!(parse(&changed.to_string(), Codec::Mp3, "mp4").is_none());
        }
        for requirements in [
            json!([]),
            json!(["aac_lc"]),
            json!(["aac_lc", "supported_mp4_timeline"]),
            json!(["aac_lc", "ffmpeg_aac_mp4_timeline"]),
            json!(["aac_separable_pns_or_no_noise", "ffmpeg_aac_mp4_timeline"]),
            json!(["aac_lc", "aac_separable_pns_or_no_noise"]),
            json!([
                "aac_lc",
                "aac_no_noise_or_all_ms_intensity",
                "ffmpeg_aac_mp4_timeline"
            ]),
            json!([
                "aac_lc",
                "aac_separable_pns_or_no_noise",
                "ffmpeg_aac_mp4_timeline",
                "supported_mp4_timeline"
            ]),
        ] {
            let mut changed = record.clone();
            changed["decoders"][index]["presentations"][0]["requirements"] = requirements;
            assert!(parse(&changed.to_string(), Codec::Mp3, "mp4").is_none());
            #[cfg(feature = "aac")]
            assert!(parse(&changed.to_string(), Codec::Aac, "mp4").is_none());
        }
    }

    #[test]
    #[allow(clippy::unwrap_used, clippy::indexing_slicing)]
    fn pns_contract_rejects_missing_changed_and_unknown_fields() {
        let record: Value = serde_json::from_str(RECORD).unwrap();
        let index = record["decoders"]
            .as_array()
            .unwrap()
            .iter()
            .position(|row| row["codec"] == "aac")
            .unwrap();
        assert_eq!(
            record["decoders"][index]["tolerance"]["pns"],
            json!({"energy_db": 0.1, "sample_domain": "non_pns_projection", "packing": "per_decoder_f32"})
        );
        for (field, value) in [
            ("energy_db", json!(0.2)),
            ("energy_db", json!(0.05)),
            ("energy_db", json!("0.1")),
            ("sample_domain", json!("complete_pcm")),
            ("packing", json!("cross_decoder")),
            ("unknown", json!(true)),
        ] {
            let mut changed = record.clone();
            changed["decoders"][index]["tolerance"]["pns"][field] = value;
            assert!(parse(&changed.to_string(), Codec::Mp3, "mp4").is_none());
        }
        for field in [
            "f32_absolute",
            "s16_lsb",
            "sample_count",
            "priming",
            "timeline",
            "feature",
            "pns",
        ] {
            let mut changed = record.clone();
            changed["decoders"][index]["tolerance"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(parse(&changed.to_string(), Codec::Mp3, "mp4").is_none());
        }
        for field in ["energy_db", "sample_domain", "packing"] {
            let mut changed = record.clone();
            changed["decoders"][index]["tolerance"]["pns"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(parse(&changed.to_string(), Codec::Mp3, "mp4").is_none());
        }
        for value in [Value::Null, json!([]), json!({}), json!(true)] {
            let mut changed = record.clone();
            changed["decoders"][index]["tolerance"]["pns"] = value;
            assert!(parse(&changed.to_string(), Codec::Mp3, "mp4").is_none());
        }
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

    #[test]
    fn opus_predicate_keeps_mp3_and_cannot_bypass_its_scope() {
        let mut value: Value = serde_json::from_str(RECORD).unwrap_or_default();
        assert!(accepted(&value).is_some());
        assert!(parse(&value.to_string(), Codec::Opus, "mp4").is_some());
        let Some(opus) = value
            .get_mut("decoders")
            .and_then(Value::as_array_mut)
            .and_then(|rows| {
                rows.iter_mut()
                    .find(|row| row.get("codec").and_then(Value::as_str) == Some("opus"))
            })
            .and_then(|row| row.get_mut("presentations"))
            .and_then(Value::as_array_mut)
            .and_then(|rows| rows.first_mut())
        else {
            return;
        };
        opus["requirements"] = json!(["supported_mp4_timeline"]);
        assert!(parse(&value.to_string(), Codec::Opus, "mp4").is_none());
        let mut unknown = RECORD.replace("opus_mapping0", "future_opus");
        assert!(parse(&unknown, Codec::Mp3, "mp4").is_none());
        unknown = RECORD.replace("\"opus_mapping0\", ", "");
        assert!(parse(&unknown, Codec::Opus, "mp4").is_none());
    }
}
