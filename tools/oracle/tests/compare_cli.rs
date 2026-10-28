//! Regression tests for file-level gates and the comparison exit status.

#![cfg(test)]

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};

fn units(level: &str, count: usize) -> Value {
    let (schema, status, records) = match level {
        "packets" => (
            "coreav-oracle/packets/1",
            "probe_ok",
            json!({"packets": (0..count).map(|pos| json!({
                "pos":pos,"pts":0,"dts":0,"size":5,"keyframe":true,
                "discard":false,"duration":1
            })).collect::<Vec<_>>() }),
        ),
        "frames" => (
            "coreav-oracle/frames/1",
            "decode_ok",
            json!({"frames": (0..count).map(|_| json!({
                "pts":0,"dts":0,"duration":1,"size":5,
                "md5":"0123456789abcdef0123456789abcdef"
            })).collect::<Vec<_>>() }),
        ),
        _ => unreachable!("test level"),
    };
    let mut stream = records;
    stream["index"] = json!(0);
    stream["time_base"] = json!([1, 10]);
    json!({"schema":schema,"file":"a.mp4",status:true,"streams":[stream]})
}

fn assert_file_gate(level: &str, scenario: &str, documents: &[(Value, Value)], path: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/oracle")
        .join(format!(
            "file-gate-{level}-{scenario}-{}",
            std::process::id()
        ));
    let reference_dir = root.join("reference");
    let candidate_dir = root.join("candidate");
    std::fs::create_dir_all(&reference_dir).expect("reference directory");
    std::fs::create_dir_all(&candidate_dir).expect("candidate directory");
    for (index, (reference, candidate)) in documents.iter().enumerate() {
        let filename = format!("{index}.{level}.json");
        std::fs::write(
            reference_dir.join(&filename),
            serde_json::to_vec(reference).expect("reference JSON"),
        )
        .expect("write reference");
        std::fs::write(
            candidate_dir.join(filename),
            serde_json::to_vec(candidate).expect("candidate JSON"),
        )
        .expect("write candidate");
    }
    for profile in ["fixtures", "corpus"] {
        let report_path = root.join("report.json");
        let output = Command::new(env!("CARGO_BIN_EXE_coreav-oracle"))
            .arg("compare")
            .arg("--reference")
            .arg(&reference_dir)
            .arg("--candidate")
            .arg(&candidate_dir)
            .args(["--levels", level, "--profile", profile, "--report"])
            .arg(&report_path)
            .output()
            .expect("run comparator");
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let report: Value =
            serde_json::from_slice(&std::fs::read(&report_path).expect("read report"))
                .expect("report JSON");
        assert_eq!(report["passed"], false);
        let result = &report["levels"][0];
        assert_eq!(result["passed"], false);
        assert_eq!(result["agreement_percent"], 100.0);
        assert_eq!(result["disagree"], 0);
        assert!(
            result["details"]
                .as_array()
                .expect("details")
                .iter()
                .any(|file| {
                    file["gating_differences"]
                        .as_array()
                        .expect("differences")
                        .iter()
                        .any(|difference| difference["path"] == path)
                })
        );
    }
    std::fs::remove_dir_all(root).expect("remove test directory");
}

#[test]
fn extra_empty_packet_stream_fails_despite_full_packet_agreement() {
    let reference = units("packets", 1);
    let mut candidate = reference.clone();
    candidate["streams"]
        .as_array_mut()
        .expect("streams")
        .push(json!({"index":1,"time_base":[1,10],"packets":[]}));
    assert_file_gate(
        "packets",
        "extra-stream",
        &[(reference, candidate)],
        "streams/count",
    );
}

#[test]
fn failed_candidate_probe_on_packetless_file_fails_despite_full_packet_agreement() {
    let valid = units("packets", 1);
    let empty = units("packets", 0);
    let mut failed = empty.clone();
    failed["probe_ok"] = json!(false);
    assert_file_gate(
        "packets",
        "probe-failure",
        &[(valid.clone(), valid), (empty, failed)],
        "probe_ok",
    );
}

#[test]
fn extra_empty_frame_stream_fails_despite_full_frame_agreement() {
    let reference = units("frames", 1_000);
    let mut candidate = reference.clone();
    candidate["streams"]
        .as_array_mut()
        .expect("streams")
        .push(json!({"index":1,"time_base":[1,10],"frames":[]}));
    assert_file_gate(
        "frames",
        "extra-stream",
        &[(reference, candidate)],
        "streams/1/frames",
    );
}

#[test]
fn failed_candidate_decode_on_frameless_file_fails_despite_full_frame_agreement() {
    let valid = units("frames", 1_000);
    let empty = units("frames", 0);
    let mut failed = empty.clone();
    failed["decode_ok"] = json!(false);
    assert_file_gate(
        "frames",
        "decode-failure",
        &[(valid.clone(), valid), (empty, failed)],
        "decode_ok",
    );
}
