"""Separate logged identity, same-host queries and unobserved requests."""

import os
from pathlib import Path
import re
import time

from evidence import digest, file_hash, integer, keys, load, read, require, sha, write
from process import run
from registration import cadence_hint, covered_class, host_observation

LIMIT = 16 * 1024**2
BASELINE_FILES = {name: LIMIT for name in ("observations.txt", "identity.json", "identity-query.json",
                                         "identity-out", "identity-error", "requested.json")}


def specification(value):
    keys(value, "schema encoder_id refusal_fields")
    require(value["schema"] == "coreav.videotoolbox-observability/2" and
            value["encoder_id"] == {"prefix": "Init the encoder: ", "suffix": ""},
            "ffmpeg_observability_missing")
    if value["refusal_fields"] is None:
        return value
    keys(value["refusal_fields"], "status submitted")
    prefixes = {"Init the encoder: "}
    for rule in value["refusal_fields"].values():
        keys(rule, "prefix suffix")
        require(type(rule["prefix"]) is str and 1 <= len(rule["prefix"]) <= 256 and
                type(rule["suffix"]) is str and len(rule["suffix"]) <= 128 and
                all(32 <= ord(c) < 127 and c not in "/\\" for c in rule["prefix"] + rule["suffix"]) and
                rule["prefix"] not in prefixes, "ffmpeg_observability_missing")
        prefixes.add(rule["prefix"])
    return value


def trace(data, spec, failed=False):
    specification(spec)
    require(len(data) <= LIMIT, "trace_bounds")
    try:
        lines = data.decode("utf-8").splitlines()
    except UnicodeError:
        require(False, "ffmpeg_observability_missing")
    identities, refusal = [], {}
    for raw in lines:
        require(len(raw) <= 8192, "trace_bounds")
        match = re.fullmatch(r"\[h264_videotoolbox @ (?:0x)?[0-9a-fA-F]+\]\s*(.*)", raw)
        if match is None:
            continue
        line = match[1]
        if line.startswith("Init the encoder: "):
            value = line.removeprefix("Init the encoder: ")
            require(re.fullmatch(r"[A-Za-z0-9_.-]{1,512}", value) is not None, "encoder_identity")
            identities.append(value)
        for field, rule in (spec["refusal_fields"] or {}).items():
            if line.startswith(rule["prefix"]) and line.endswith(rule["suffix"]):
                end = len(line) - len(rule["suffix"]) if rule["suffix"] else len(line)
                require(field not in refusal, "ffmpeg_observation_changed")
                refusal[field] = line[len(rule["prefix"]):end]
    if failed:
        require(not identities and set(refusal) == {"status", "submitted"} and
                refusal["status"] in ("no_encoder", "temporarily_unavailable") and
                refusal["submitted"] == "0", "ffmpeg_observability_missing")
        return {"status": refusal["status"], "submitted": 0}, data
    require(len(identities) == 1 and not refusal, "ffmpeg_observability_missing")
    return {"encoder_id": identities[0]}, data


def requested(arguments, item):
    covered_class(item)
    require(type(arguments) is list and all(type(arg) is str for arg in arguments), "baseline_requests")
    controls = {}
    for field, options in (("realtime", ("-realtime",)),
                           ("rate_control", ("-b:v", "-q:v", "-constant_bit_rate", "-maxrate", "-bufsize"))):
        controls[field] = {option: arguments[arguments.index(option) + 1] if option in arguments else None
                           for option in options}
    return {"schema": "coreav.videotoolbox-requested-settings/1", "observation": "unobserved",
            "arguments": arguments, **controls,
            "nominal_frame_rate_hint": {"value": cadence_hint(item),
                                        "provenance": "derived_nearest_whole_fps_ties_up",
                                        "observation": "unobserved"}}


def validate_requested(value, item, reg, cell):
    keys(value, "schema observation arguments realtime rate_control nominal_frame_rate_hint")
    arguments = value["arguments"]
    require(type(arguments) is list and all(type(arg) is str for arg in arguments), "baseline_requests")
    from timing import commands
    require(arguments and arguments[0].endswith("/" + reg["executables"]["ffmpeg"]["file"]), "baseline_requests")
    root = arguments[0][:-len(reg["executables"]["ffmpeg"]["file"])]
    require("-stats_enc_post" in arguments and arguments.index("-stats_enc_post") + 1 < len(arguments),
            "baseline_requests")
    expected = commands(reg, {"raw_file": "unused", **item}, cell, Path(root), Path(arguments[-1]),
                        Path(arguments[arguments.index("-stats_enc_post") + 1]))["ffmpeg"]
    if item["id"] == "preflight":
        expected[expected.index("-i") + 1] = str(Path(arguments[-1]).parent / "source.yuv")
        expected[expected.index("-b:v") + 1] = "2000000"
    require(arguments == expected and value == requested(arguments, item), "baseline_requests")


def identity_record(value, report, trace_hash, reg, cell, output_hash, encoder_ended_ns=None):
    keys(value, "schema encoder_id binary_sha256 host trace_sha256 output_sha256 encoder_ended_ns process")
    keys(report, "schema encoder_id hardware")
    keys(value["host"], " ".join(reg["host"]))
    require(report["schema"] == "coreav.videotoolbox-encoder-identity/1" and
            type(report["encoder_id"]) is str and report["encoder_id"] == value["encoder_id"] and
            report["hardware"] in ("hardware", "software") and report["hardware"] == cell,
            "encoder_identity")
    require(value["schema"] == "coreav.videotoolbox-identity-query/1" and
            value["binary_sha256"] == reg["executables"]["coreav"]["sha256"] and
            value["host"] == reg["host"] and
            all(type(value["host"][key]) is type(expected) for key, expected in reg["host"].items()) and
            value["trace_sha256"] == trace_hash and
            value["output_sha256"] == output_hash, "identity_binding")
    integer(value["encoder_ended_ns"], 1, 2**63 - 1)
    if encoder_ended_ns is not None:
        require(value["encoder_ended_ns"] == encoder_ended_ns, "identity_clock")
    process = value["process"]
    keys(process, "arguments status exit_code started_ns ended_ns elapsed_ns stdout_sha256 stderr_sha256 report_sha256")
    arguments = process["arguments"]
    require(type(arguments) is list and len(arguments) == 6 and
            type(arguments[0]) is str and arguments[0].endswith("/" + reg["executables"]["coreav"]["file"]) and
            arguments[1:5] == ["identify", "--encoder-id", report["encoder_id"], "--report"] and
            type(arguments[5]) is str and arguments[5].endswith("/identity.json") and
            process["status"] == "success" and type(process["exit_code"]) is int and
            process["exit_code"] == 0, "identity_process")
    integer(process["started_ns"], value["encoder_ended_ns"], 2**63 - 1)
    integer(process["ended_ns"], process["started_ns"], 2**63 - 1)
    integer(process["elapsed_ns"], 1, 10 * 10**9)
    require(process["ended_ns"] - process["started_ns"] >= process["elapsed_ns"], "identity_clock")
    for field in ("stdout_sha256", "stderr_sha256", "report_sha256"):
        digest(process[field])
    return report


def identity(directory, reg, cell, spec, output_hash, encoder_ended_ns=None):
    value = load(Path(directory) / "identity-query.json", LIMIT)
    report = load(Path(directory) / "identity.json", LIMIT)
    data = read(directory, "observations.txt", LIMIT)
    observed, _ = trace(data, spec)
    identity_record(value, report, sha(data), reg, cell, output_hash, encoder_ended_ns)
    require(value["encoder_id"] == observed["encoder_id"], "identity_binding")
    for filename, field in (("identity-out", "stdout_sha256"), ("identity-error", "stderr_sha256"),
                            ("identity.json", "report_sha256")):
        require(file_hash(Path(directory) / filename, LIMIT) == value["process"][field], "identity_binding")
    require(not read(directory, "identity-error", LIMIT), "identity_process")
    return report


def capture_identity(directory, reg, root, spec, encoder_ended_ns, command, item, cell):
    covered_class(item)
    directory = Path(directory)
    data = read(directory, "stderr", LIMIT)
    observed, _ = trace(data, spec)
    (directory / "observations.txt").write_bytes(data)
    binary = Path(root) / reg["executables"]["coreav"]["file"]
    require(file_hash(binary) == reg["executables"]["coreav"]["sha256"], "binary_changed")
    host = host_observation()
    require(all(type(host[k]) is type(reg["host"][k]) and host[k] == reg["host"][k] for k in host) and
            os.environ.get("COREAV_MAC_ALLOCATION_SHA256") == reg["host"]["allocation_sha256"], "host_mismatch")
    write(directory / "requested.json", requested(command, item))
    validate_requested(load(directory / "requested.json"), item, reg, cell)
    command = [str(binary), "identify", "--encoder-id", observed["encoder_id"],
               "--report", str(directory / "identity.json")]
    process = {"arguments": command, "status": "failed", "exit_code": None,
               "started_ns": None, "ended_ns": None, "elapsed_ns": None,
               "stdout_sha256": None, "stderr_sha256": None, "report_sha256": None}
    value = {"schema": "coreav.videotoolbox-identity-query/1", "encoder_id": observed["encoder_id"],
             "binary_sha256": reg["executables"]["coreav"]["sha256"], "host": dict(reg["host"]),
             "trace_sha256": sha(data), "output_sha256": file_hash(directory / "video.h264", 1024**3),
             "encoder_ended_ns": encoder_ended_ns, "process": process}
    try:
        process["started_ns"] = time.time_ns()
        process["elapsed_ns"] = run(command, directory, 10, stdout="identity-out", stderr="identity-error")
        process["ended_ns"] = time.time_ns()
        process.update(status="success", exit_code=0,
                       stdout_sha256=file_hash(directory / "identity-out", LIMIT),
                       stderr_sha256=file_hash(directory / "identity-error", LIMIT),
                       report_sha256=file_hash(directory / "identity.json", LIMIT))
    finally:
        if process["ended_ns"] is None:
            process["ended_ns"] = time.time_ns()
        write(directory / "identity-query.json", value)
    return identity(directory, reg, cell, spec,
                    value["output_sha256"], encoder_ended_ns)
