#!/usr/bin/env python3
"""Prepared raw I420 encoder timing and separate evidence replay.

No source acquisition, baseline build, host provisioning or acceptance evidence
generation occurs here. Reports remain nonpassing until complete retained
conformance, quality, bitrate, fixture, availability and host-gate evidence binds
every captured output. Integrated transcode and container acceptance never apply.
"""

import argparse
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import sys
import tempfile
import time

from evidence import Invalid, digest, file_hash, keys, load, read
from evidence import require, sha, verify, write
from judging import conformance, deltas as metric_deltas, gate, quality, timings
from process import run
from registration import CLASSES, ROOT, SCOPE, caller_settings, command_templates, coverage, covered_class, git, hardware_fingerprint, input_hashes, preflight, raw_size, roster_coverage, source_registration
import cache_residency as cache
from observations import BASELINE_FILES, capture_identity, identity, validate_requested


def settings_hash(registration):
    return sha(json.dumps({k: registration[k] for k in
                           ("settings", "environment", "commands", "metric_tools")},
                          sort_keys=True, separators=(",", ":")).encode())


def warm(path, item):
    remaining = raw_size(item)
    with path.open("rb") as source:
        while remaining:
            block = source.read(min(remaining, 1024 * 1024))
            require(block, "input_hash")
            remaining -= len(block)
        require(not source.read(1), "input_hash")


def bound_gate(path, registration, registration_hash, phase):
    value = load(path)
    median = gate(value, registration, phase)
    require(value["registration_sha256"] == registration_hash and
            value["settings_sha256"] == settings_hash(registration), "gate_binding")
    return value, median


def ram_volume(directory):
    directory = Path(directory).resolve(strict=True)
    require(sys.platform == "darwin", "ram_platform")
    with tempfile.TemporaryDirectory() as temporary:
        run(["/usr/bin/hdiutil", "info", "-plist"], temporary, 10)
        try:
            info = plistlib.loads(read(temporary, "stdout", 4 * 1024**2))
        except (ValueError, plistlib.InvalidFileException):
            raise Invalid("ram_volume") from None
        mounts = [Path(entity["mount-point"]).resolve()
                  for image in info.get("images", [])
                  if str(image.get("image-path", "")).startswith("ram://")
                  for entity in image.get("system-entities", []) if "mount-point" in entity]
        require(any(directory == mount or directory.is_relative_to(mount) for mount in mounts), "ram_volume")


def commands(registration, item, cell, root, output, native_report):
    covered_class(item)
    rate = item["rate"]
    values = {name: str(Path(root) / record["file"])
              for name, record in registration["executables"].items()}
    values.update(raw=str(Path(root) / item["raw_file"]), width=str(item["width"]),
                  height=str(item["height"]), geometry=f'{item["width"]}x{item["height"]}',
                  rate_num=str(rate["num"]), rate_den=str(rate["den"]),
                  rate=f'{rate["num"]}/{rate["den"]}', frames=str(item["frames"]),
                  bitrate=str(dict(CLASSES)[item["class"]]),
                  keyframe_frames=str(2 * rate["num"] // rate["den"]),
                  output=str(output), report=str(native_report))
    templates = command_templates(registration, cell)
    require(templates == registration["commands"][cell], "command_changed")
    return {side: [values.get(argument[1:-1], argument) if argument.startswith("{") else argument
                   for argument in command] for side, command in templates.items()}


def availability(value, registration):
    keys(value, "schema session_id registration_sha256 cells covered_classes uncovered_classes")
    require(coverage(value) == roster_coverage(registration), "class_coverage")
    require(value["schema"] == "coreav.videotoolbox-availability/2" and
            value["session_id"] == registration["session_id"], "availability_binding")
    digest(value["registration_sha256"])
    keys(value["cells"], "hardware software")
    for cell, sides in value["cells"].items():
        keys(sides, "coreav ffmpeg")
        statuses = []
        for side, record in sides.items():
            keys(record, "status submitted reason smoke_sha256 binary_sha256 hardware "
                 "encoder_id decoded_frames pattern_identity drain_complete")
            require(record["binary_sha256"] == registration["executables"][side]["sha256"], "availability_tool")
            digest(record["smoke_sha256"])
            statuses.append(record["status"])
            if record["status"] == "unavailable":
                require(cell == "software" and type(record["submitted"]) is int and
                        record["submitted"] == 0 and record["reason"] in
                        ("no_encoder", "temporarily_unavailable") and
                        record["decoded_frames"] == 0 and record["pattern_identity"] is False and
                        record["drain_complete"] is False, "availability")
            else:
                require(record["status"] == "encoded" and type(record["submitted"]) is int and
                        record["submitted"] == 4 and record["reason"] is None and
                        record["hardware"] == cell and type(record["encoder_id"]) is str and
                        0 < len(record["encoder_id"]) <= 512 and
                        type(record["decoded_frames"]) is int and record["decoded_frames"] == 4 and
                        record["pattern_identity"] is True and record["drain_complete"] is True,
                        "availability")
        require(statuses[0] == statuses[1], "availability_mismatch")
        require((cell in registration["cells"]) == (statuses[0] == "encoded"), "cell_coverage")


def availability_file(path, registration, root=ROOT):
    value = load(path)
    availability(value, registration)
    for cell, sides in value["cells"].items():
        for side, record in sides.items():
            retained = Path(path).parent / f"preflight.{cell}.{side}.json"
            require(file_hash(retained, 16 * 1024**2) == record["smoke_sha256"], "availability_hash")
            proof = load(retained)
            keys(proof, "schema registration_sha256 producer_sha256 settings_sha256 result files covered_classes uncovered_classes")
            require(coverage(proof) == coverage(registration), "class_coverage")
            require(proof["schema"] == "coreav.videotoolbox-preflight/2" and
                    proof["registration_sha256"] == value["registration_sha256"] and
                    proof["producer_sha256"] == registration["metric_tools"]["evidence_producer"] and
                    proof["settings_sha256"] == settings_hash(registration) and
                    proof["result"] == {k: v for k, v in record.items() if k != "smoke_sha256"},
                    "availability_evidence")
            from producer import verify_preflight
            verify_preflight(proof, Path(path).parent / f"preflight.{cell}.{side}.artifacts", cell, side,
                             registration, load(Path(root) / registration["baseline_build"]["file"])["observability"])
    return value


def host_state(registration):
    require(sys.platform == "darwin", "host_platform")
    hardware = hardware_fingerprint()
    require(hardware == registration["host"]["hardware_sha256"], "host_mismatch")
    with tempfile.TemporaryDirectory() as temporary:
        run(["/usr/sbin/sysctl", "-n", "vm.swapusage"], temporary, 10,
            stdout="swap", stderr="swap-error")
        swap = read(temporary, "swap", 1024).decode()
        match = re.search(r"\bused\s*=\s*([0-9]+(?:\.[0-9]+)?)M\b", swap)
        require(match is not None and float(match[1]) == 0, "host_fault")
        run(["/usr/bin/pmset", "-g", "therm"], temporary, 10,
            stdout="thermal", stderr="thermal-error")
        thermal = read(temporary, "thermal", 8192).decode()
        require("No thermal warning level has been recorded" in thermal and
                "No performance warning level has been recorded" in thermal, "host_fault")
        limits = re.findall(r"CPU_(?:Speed|Scheduler)_Limit\s*=\s*([0-9]+)", thermal)
        require(all(int(limit) == 100 for limit in limits), "host_fault")
        run(["/bin/ps", "-axo", "comm="], temporary, 10,
            stdout="processes", stderr="processes-error")
        processes = read(temporary, "processes", 1024**2).decode().splitlines()
        binaries = {Path(record["file"]).name for record in registration["executables"].values()}
        require(not any(Path(line.strip()).name in binaries for line in processes), "host_fault")
    return {"swap_bytes": 0, "thermal_warning": False, "throttled": False,
            "competing_encoder": False, "background_load": os.getloadavg()[0],
            "caller_settings": caller_settings(registration), "encoder_threads": "framework_managed_uncontrolled",
            "memory_limit": "none", "affinity": "unsupported", "hardware_sha256": hardware}


def capture_gate(args):
    result = {"schema": "coreav.videotoolbox-host-gate-failure/1", "status": "failed",
              "phase": args.phase, "diagnostic": "gate_incomplete"}
    write(args.report, result)
    try:
        reg = preflight(args.registration, args.root)
        cache.preflight(reg, args.root)
        reg_hash = file_hash(args.registration)
        available = availability_file(args.availability, reg, args.root)
        require(available["registration_sha256"] == reg_hash, "availability_binding")
        ram_volume(args.ram_output)
        input_hashes(args.root, reg)
        source = reg["inputs"][0]
        environment = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "LANG": "C", "LC_ALL": "C", **reg["environment"]}
        result = {"schema": "coreav.videotoolbox-host-gate/2", **roster_coverage(reg), "session_id": reg["session_id"],
                  "phase": args.phase, "registration_sha256": reg_hash, "host": reg["host"],
                  "tools": reg["executables"], "settings_sha256": settings_hash(reg),
                  "aa": [], "null": [], "observations": host_state(reg),
                  "started_ns": time.time_ns(), "ended_ns": None}
        with tempfile.TemporaryDirectory(dir=args.ram_output) as temporary:
            temporary = Path(temporary)
            for arm in ("aa", "null"):
                for iteration in range(6):
                    pair = {"a_sha256": reg["executables"]["ffmpeg"]["sha256"],
                            "b_sha256": reg["executables"]["ffmpeg"]["sha256"]}
                    result["pending"] = pair
                    for label in (("a", "b") if iteration % 2 == 0 else ("b", "a")):
                        host_state(reg)
                        warm(args.root / source["raw_file"], source)
                        before = cache.observe(reg, args.root, args.root / source["raw_file"], source)
                        pair[label + "_cache_before"] = before
                        write(args.report, result)
                        cache.validate(before, reg, source)
                        directory = temporary / f"{arm}-{iteration}-{label}"
                        directory.mkdir()
                        output = directory / "video.h264"
                        command = commands(reg, source, "hardware", args.root, output, directory / "unused")["ffmpeg"]
                        pair[label + "_started_ns"] = time.time_ns()
                        pair[label + "_ns"] = run(command, directory, 300, env=environment)
                        pair[label + "_ended_ns"] = time.time_ns()
                        after = cache.observe(reg, args.root, args.root / source["raw_file"], source)
                        pair[label + "_cache_after"] = after
                        write(args.report, result)
                        cache.validate(after, reg, source)
                        require(output.stat().st_size > 0, "gate_output")
                        file_hash(output, 1024**3)
                        from producer import packet_ledger
                        spec = load(args.root / reg["baseline_build"]["file"])["observability"]
                        try:
                            capture_identity(directory, reg, args.root, spec, pair[label + "_ended_ns"], command, source, "hardware")
                        finally:
                            pair[label + "_baseline"] = {
                                "files": {name: read(directory, name, limit).decode("utf-8")
                                          for name, limit in BASELINE_FILES.items() if (directory / name).exists()},
                                "specification": spec, "output_sha256": file_hash(output, 1024**3),
                                "payload_bytes": output.stat().st_size, "packet_ledger": None,
                                "packet_text": read(directory, "unused", 16 * 1024**2).decode("ascii")}
                        pair[label + "_baseline"]["packet_ledger"] = packet_ledger(
                            read(directory, "unused", 16 * 1024**2), source, output.stat().st_size)
                        host_state(reg)
                        shutil.rmtree(directory)
                    result[arm].append(pair)
                    write(args.report, result)
                    del result["pending"]
        result["observations"] = host_state(reg)
        result["ended_ns"] = time.time_ns()
        require(file_hash(args.registration) == reg_hash, "registration_changed")
        input_hashes(args.root, reg)
        preflight(args.registration, args.root)
        gate(result, reg, args.phase)
    except BaseException:
        result = {"schema": "coreav.videotoolbox-host-gate-failure/1", "status": "failed",
                  "phase": args.phase, "diagnostic": "gate_failed", "retained": result}
        raise
    finally:
        write(args.report, result)


def execute(args):
    report = {"schema": "coreav.videotoolbox-timing/2", "status": "aborted",
              "scope": SCOPE, "covered_classes": None, "uncovered_classes": None,
              "diagnostic": "preflight", "records": [],
              "registration_sha256": None, "before_gate_sha256": None,
              "availability_sha256": None, "started_ns": time.time_ns(), "ended_ns": None}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    write(args.report, report)
    try:
        registration = preflight(args.registration, args.root)
        report.update(coverage(registration))
        cache.preflight(registration, args.root)
        registration_hash = file_hash(args.registration)
        report["registration_sha256"] = registration_hash
        before, _ = bound_gate(args.before_gate, registration, registration_hash, "before")
        require(before["ended_ns"] <= report["started_ns"] and
                report["started_ns"] - before["ended_ns"] <= 3600 * 10**9, "gate_clock")
        available = availability_file(args.availability, registration, args.root)
        require(available["registration_sha256"] == registration_hash, "availability_binding")
        report["before_gate_sha256"] = file_hash(args.before_gate)
        report["availability_sha256"] = file_hash(args.availability)
        ram_volume(args.ram_output)
        require(not args.artifacts.exists() and not args.artifacts.is_symlink(), "artifacts_exist")
        args.artifacts.mkdir(parents=False)
        input_hashes(args.root, registration)
        environment = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "LANG": "C", "LC_ALL": "C",
                       **registration["environment"]}
        with tempfile.TemporaryDirectory(dir=args.ram_output) as working:
            working = Path(working)
            deadline = time.monotonic() + 6 * 3600
            for cell in registration["cells"]:
                for item in registration["inputs"]:
                    for iteration in range(6):
                        order = ("coreav", "ffmpeg") if iteration % 2 == 0 else ("ffmpeg", "coreav")
                        for side in order:
                            require(time.monotonic() < deadline, "timeout")
                            host_before = host_state(registration)
                            call = f"call-{len(report['records']):04d}"
                            directory = working / call
                            directory.mkdir()
                            output, native = directory / "video.h264", directory / "native.json"
                            command = commands(registration, item, cell, args.root, output, native)[side]
                            record = {"cell": cell, "input_id": item["id"], "iteration": iteration,
                                      "side": side, "elapsed_ns": None, "status": "aborted",
                                      "output_sha256": None, "output_bytes": None,
                                      "native_report_sha256": None, "started_ns": None,
                                      "ended_ns": None, "host_before": host_before, "host_after": None,
                                      "packet_clock_sha256": None, "observations_sha256": None,
                                      "identity_query_sha256": None, "requested_sha256": None,
                                      "cache_before": None, "cache_after": None}
                            report["records"].append(record)
                            write(args.report, report)
                            try:
                                warm(args.root / item["raw_file"], item)
                                record["cache_before"] = cache.observe(registration, args.root,
                                                                       args.root / item["raw_file"], item)
                                cache.validate(record["cache_before"], registration, item)
                                write(args.report, report)
                                record["started_ns"] = time.time_ns()
                                record["elapsed_ns"] = run(command, directory,
                                                           min(300, deadline - time.monotonic()),
                                                           env=environment)
                                record["ended_ns"] = time.time_ns()
                                record["cache_after"] = cache.observe(registration, args.root,
                                                                      args.root / item["raw_file"], item)
                                cache.validate(record["cache_after"], registration, item)
                                record["host_after"] = host_state(registration)
                                record["output_sha256"] = file_hash(output, 1024**3)
                                record["output_bytes"] = output.stat().st_size
                                require(0 < record["output_bytes"] <= 1024**3, "output_bounds")
                                if side == "coreav":
                                    record["native_report_sha256"] = sha(read(directory, "native.json", 16 * 1024**2))
                                else:
                                    from producer import packet_ledger
                                    spec = load(args.root / registration["baseline_build"]["file"])["observability"]
                                    capture_identity(directory, registration, args.root, spec, record["ended_ns"], command, item, cell)
                                    packet_ledger(read(directory, "native.json", 16 * 1024**2), item, record["output_bytes"])
                                    record["packet_clock_sha256"] = sha(read(directory, "native.json", 4 * 1024**2))
                                    record["observations_sha256"] = file_hash(directory / "observations.txt", 16 * 1024**2)
                                    record["identity_query_sha256"] = file_hash(directory / "identity-query.json", 16 * 1024**2)
                                    record["requested_sha256"] = file_hash(directory / "requested.json", 16 * 1024**2)
                                retained = args.artifacts / call
                                retained.mkdir()
                                shutil.copyfile(output, retained / "video.h264")
                                require(file_hash(retained / "video.h264") == record["output_sha256"], "copy_hash")
                                if side == "coreav":
                                    shutil.copyfile(native, retained / "native.json")
                                    require(file_hash(retained / "native.json") == record["native_report_sha256"], "copy_hash")
                                else:
                                    shutil.copyfile(native, retained / "packets.stats")
                                    for name in BASELINE_FILES:
                                        shutil.copyfile(directory / name, retained / name)
                                    require(file_hash(retained / "packets.stats") == record["packet_clock_sha256"] and
                                            file_hash(retained / "observations.txt") == record["observations_sha256"] and
                                            file_hash(retained / "identity-query.json") == record["identity_query_sha256"] and
                                            file_hash(retained / "requested.json") == record["requested_sha256"], "copy_hash")
                                record["status"] = "success"
                            except Invalid as error:
                                record["status"] = (error.code if error.code in
                                                    ("timeout", "child_failure", "process_infrastructure")
                                                    else "aborted")
                                raise
                            finally:
                                if record["started_ns"] is not None and record["ended_ns"] is None:
                                    record["ended_ns"] = time.time_ns()
                                if side == "ffmpeg" and record["status"] != "success" and (directory / "identity-query.json").exists():
                                    retained = args.artifacts / call
                                    retained.mkdir(exist_ok=True)
                                    failure_files = {"video.h264": 1024**3, "native.json": 16 * 1024**2, **BASELINE_FILES}
                                    record["validation_failure_files"] = {}
                                    for name, limit in failure_files.items():
                                        if (directory / name).exists():
                                            expected_hash = file_hash(directory / name, limit)
                                            shutil.copyfile(directory / name, retained / name)
                                            require(file_hash(retained / name, limit) == expected_hash, "copy_hash")
                                            record["validation_failure_files"][name] = expected_hash
                                write(args.report, report)
                            shutil.rmtree(directory)
        require(file_hash(args.registration) == registration_hash, "registration_changed")
        input_hashes(args.root, registration)
        preflight(args.registration, args.root)
        report["status"] = "awaiting_evidence"
        report["diagnostic"] = "evidence_required"
        timings(report["records"], registration)
    except BaseException as error:
        report["status"] = "aborted"
        report["diagnostic"] = error.code if isinstance(error, Invalid) and error.code in {
            "timeout", "child_crash", "child_failure", "process_infrastructure", "host_fault", "aa_gate",
            "null_gate", "dirty_checkout", "judged_event", "host_mismatch", "input_hash",
            "availability_mismatch", "tool_changed", "baseline_review", "ram_volume",
            "ffmpeg_observability_missing", "ffmpeg_observation_changed",
            "cache_observer_missing", "cache_observer_changed", "cache_observer_unsupported",
            "cache_observer_output", "cache_not_resident"} else "validation_failed"
        raise
    finally:
        report["ended_ns"] = time.time_ns()
        write(args.report, report)


def replay(args):
    result = {"schema": "coreav.videotoolbox-speed-report/2", "status": "failed",
              "scope": SCOPE, "covered_classes": None, "uncovered_classes": None,
              "integrated_acceptance": "not_applicable",
              "container_acceptance": "not_applicable", "diagnostic": "evidence_invalid",
              "baseline_framework_settings": {"observation": "unobserved",
                                               "requested_only": ["realtime", "nominal_frame_rate_hint", "rate_control"],
                                               "literal_arguments": "retained_per_output"}}
    try:
        from producer import trusted_analysis_event, verify_campaign, verify_fixture_capture, verify_output
        trusted_analysis_event()
        registration = source_registration(args.registration, args.root)
        result.update(roster_coverage(registration))
        if os.environ.get("GITHUB_ACTIONS"):
            require(os.environ.get("GITHUB_SHA") == git(args.root, "rev-parse", "HEAD").decode().strip(), "judged_event")
        require(file_hash(Path(__file__).with_name("producer.py")) ==
                registration["metric_tools"]["evidence_producer"], "producer_changed")
        observability = load(args.root / registration["baseline_build"]["file"])["observability"]
        reg_hash = file_hash(args.registration)
        captured = load(args.capture, 16 * 1024**2)
        keys(captured, "schema status scope diagnostic records registration_sha256 before_gate_sha256 "
             "availability_sha256 started_ns ended_ns covered_classes uncovered_classes")
        require(coverage(captured) == coverage(registration), "class_coverage")
        require(captured["schema"] == "coreav.videotoolbox-timing/2" and
                captured["status"] == "awaiting_evidence" and
                captured["registration_sha256"] == reg_hash and
                captured["scope"] == SCOPE, "capture_status")
        summaries = timings(captured["records"], registration)
        expected_artifacts = {f"call-{index:04d}" for index in range(len(captured["records"]))}
        expected_artifacts |= {f"fixture.{cell}.{fixture}.{side}" for cell in registration["cells"]
                               for fixture in registration["fixtures"] for side in ("coreav", "ffmpeg")}
        require(not args.artifacts.is_symlink() and
                {p.name for p in args.artifacts.iterdir()} == expected_artifacts, "artifact_coverage")
        evidence = load(args.evidence / "evidence.json", 16 * 1024**2)
        keys(evidence, "schema registration_sha256 capture_sha256 files outputs fixtures covered_classes uncovered_classes scope")
        require(coverage(evidence) == coverage(registration) and evidence["scope"] == SCOPE, "class_coverage")
        require(evidence["schema"] == "coreav.videotoolbox-evidence/2" and
                evidence["registration_sha256"] == reg_hash and
                evidence["capture_sha256"] == file_hash(args.capture), "evidence_binding")
        require(type(evidence["outputs"]) is dict and type(evidence["fixtures"]) is dict,
                "evidence_shape")
        preflights = {f"preflight.{cell}.{side}.json" for cell in ("hardware", "software")
                      for side in ("coreav", "ffmpeg")}
        names = {"before.json", "after.json", "availability.json", "fixtures.capture.json", "campaign.json", "producer-status.json"} | preflights
        names.update(evidence["outputs"].values())
        names.update(evidence["fixtures"].values())
        require(len(names) == 10 + len(evidence["outputs"]) + len(evidence["fixtures"]), "evidence_duplicate")
        preflight_artifacts = {f"preflight.{cell}.{side}.artifacts" for cell in ("hardware", "software")
                               for side in ("coreav", "ffmpeg")}
        require({p.name for p in args.evidence.iterdir()} == names | {"evidence.json", ".campaign.lock"} | preflight_artifacts,
                "evidence_coverage")
        require(read(args.evidence, ".campaign.lock", 0) == b"", "campaign_lock")
        verify(args.evidence, evidence["files"], names, 16 * 1024**2)
        verify_campaign(args.evidence, args.registration, args.capture, registration)
        before, a = bound_gate(args.evidence / "before.json", registration, reg_hash, "before")
        after, b = bound_gate(args.evidence / "after.json", registration, reg_hash, "after")
        require(file_hash(args.evidence / "before.json") == captured["before_gate_sha256"] and
                file_hash(args.evidence / "availability.json") == captured["availability_sha256"], "capture_binding")
        require(abs(a / b - 1) <= 0.03, "aa_session")
        require(before["ended_ns"] <= captured["started_ns"] < captured["ended_ns"] <=
                after["started_ns"], "gate_clock")
        available = availability_file(args.evidence / "availability.json", registration, args.root)
        require(available["registration_sha256"] == reg_hash, "availability_binding")
        fixtures_capture = verify_fixture_capture(args.evidence / "fixtures.capture.json", registration,
                                                  file_hash(args.capture), args.artifacts)
        require(fixtures_capture["registration_sha256"] == reg_hash, "fixture_capture")
        expected = {f"call-{index:04d}" for index in range(len(captured["records"]))}
        require(set(evidence["outputs"]) == expected, "output_coverage")
        require(set(evidence["fixtures"]) == {f"{cell}.{fixture}" for cell in registration["cells"]
                                                for fixture in registration["fixtures"]}, "fixture_coverage")
        by_id = {item["id"]: item for item in registration["inputs"]}
        output_values = {}
        for index, record in enumerate(captured["records"]):
            call = f"call-{index:04d}"
            output = args.artifacts / call / "video.h264"
            require(file_hash(output, 1024**3, root=args.artifacts) == record["output_sha256"] and
                    output.stat().st_size == record["output_bytes"], "output_hash")
            if record["side"] == "coreav":
                require(file_hash(args.artifacts / call / "native.json", 16 * 1024**2, root=args.artifacts) == record["native_report_sha256"],
                        "native_report_hash")
            else:
                require(file_hash(args.artifacts / call / "packets.stats", 16 * 1024**2, root=args.artifacts) == record["packet_clock_sha256"] and
                        file_hash(args.artifacts / call / "observations.txt", 16 * 1024**2, root=args.artifacts) == record["observations_sha256"] and
                        file_hash(args.artifacts / call / "identity-query.json", 16 * 1024**2, root=args.artifacts) == record["identity_query_sha256"] and
                        file_hash(args.artifacts / call / "requested.json", 16 * 1024**2, root=args.artifacts) == record["requested_sha256"],
                        "baseline_evidence")
                identity(args.artifacts / call, registration, record["cell"], observability,
                         record["output_sha256"], record["ended_ns"])
            proof = load(args.evidence / evidence["outputs"][call], 16 * 1024**2)
            item = by_id[record["input_id"]]
            verify_output(proof, args.artifacts / call, args.root / item["raw_file"], item, record["cell"],
                          record["side"], registration, observability, args)
            metrics = conformance(proof, by_id[record["input_id"]], record["cell"], record["side"],
                                  record["output_sha256"], registration)
            require(proof["payload_bytes"] == record["output_bytes"], "payload_size")
            output_values[record["cell"], record["input_id"], record["iteration"], record["side"]] = (metrics, proof["payload_bytes"])
        for cell in registration["cells"]:
            deltas = {iteration: {height: [] for height in registration["covered_classes"]} for iteration in range(6)}
            for item in registration["inputs"]:
                for iteration in range(6):
                    coreav, ca_size = output_values[cell, item["id"], iteration, "coreav"]
                    ffmpeg, ff_size = output_values[cell, item["id"], iteration, "ffmpeg"]
                    require(0.95 <= ca_size / ff_size <= 1.05, "payload_ratio")
                    delta = metric_deltas(coreav, ffmpeg)
                    deltas[iteration][item["class"]].append(delta)
            fixture_deltas = []
            for fixture in registration["fixtures"]:
                proof = load(args.evidence / evidence["fixtures"][f"{cell}.{fixture}"])
                keys(proof, "schema fixture cell producer_sha256 registration_sha256 coreav ffmpeg")
                require(proof["schema"] == "coreav.videotoolbox-fixture-quality/1" and
                        proof["fixture"] == fixture and proof["cell"] == cell and
                        proof["registration_sha256"] == reg_hash and proof["producer_sha256"] ==
                        registration["metric_tools"]["evidence_producer"], "fixture_binding")
                fixture_metrics = {}
                for side in ("coreav", "ffmpeg"):
                    output_hash = digest(proof[side]["output_sha256"])
                    fixture_output = args.artifacts / f"fixture.{cell}.{fixture}.{side}" / "video.h264"
                    require(file_hash(fixture_output, 1024**3, root=args.artifacts) == output_hash and fixture_output.stat().st_size ==
                            proof[side]["payload_bytes"], "fixture_output_hash")
                    item = registration["fixtures"][fixture]
                    verify_output(proof[side], fixture_output.parent, args.root / item["raw_file"], item,
                                  cell, side, registration, observability, args)
                    fixture_metrics[side] = conformance(
                        proof[side], registration["fixtures"][fixture], cell, side, output_hash, registration)
                require(0.95 <= proof["coreav"]["payload_bytes"] / proof["ffmpeg"]["payload_bytes"] <= 1.05,
                        "fixture_payload_ratio")
                fixture_deltas.append(metric_deltas(fixture_metrics["coreav"], fixture_metrics["ffmpeg"]))
            for values in deltas.values():
                quality(values, fixture_deltas, registration)
        passed = all(cell["overall"]["bar_pass"] and all(c["bar_pass"] for c in cell["classes"].values())
                     for cell in summaries.values())
        result.update(status="pass" if passed else "bar_miss", diagnostic="complete",
                      registration_sha256=reg_hash, capture_sha256=file_hash(args.capture),
                      evidence_sha256=file_hash(args.evidence / "evidence.json"), cells=summaries,
                      caller_settings=caller_settings(registration), encoder_threads="framework_managed_uncontrolled")
    finally:
        write(args.report, result)
    return result["status"] == "pass"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands_parser = parser.add_subparsers(dest="command", required=True)
    capture_parser = commands_parser.add_parser("run")
    capture_parser.add_argument("--root", type=Path, default=ROOT)
    capture_parser.add_argument("--before-gate", required=True, type=Path)
    capture_parser.add_argument("--availability", required=True, type=Path)
    capture_parser.add_argument("--ram-output", required=True, type=Path)
    replay_parser = commands_parser.add_parser("report")
    replay_parser.add_argument("--root", type=Path, default=ROOT)
    replay_parser.add_argument("--capture", required=True, type=Path)
    replay_parser.add_argument("--evidence", required=True, type=Path)
    gate_parser = commands_parser.add_parser("gate", help="Five baseline/self pairs for each A/A and null gate")
    gate_parser.add_argument("--root", type=Path, default=ROOT)
    gate_parser.add_argument("--phase", choices=("before", "after"), required=True)
    gate_parser.add_argument("--availability", required=True, type=Path)
    gate_parser.add_argument("--ram-output", required=True, type=Path)
    for sub in (capture_parser, replay_parser, gate_parser):
        sub.add_argument("--registration", required=True, type=Path)
        sub.add_argument("--report", required=True, type=Path)
    for sub in (capture_parser, replay_parser):
        sub.add_argument("--artifacts", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == "run":
            execute(args)
            return 0
        if args.command == "gate":
            capture_gate(args)
            return 0
        return 0 if replay(args) else 1
    except (Invalid, OSError, ValueError, UnicodeError, KeyError, TypeError):
        print("videotoolbox-timing: failed", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
