"""Replay retained evidence; never infer acceptance from elapsed times alone."""

from fractions import Fraction
import math
import json
import random
import statistics

from evidence import digest, integer, keys, loads, number, require, sha
from registration import CLASSES, COLOUR, SCOPE, caller_settings, clock, coverage, covered_class, roster_coverage
import cache_residency as cache


def summary(ratios):
    require(type(ratios) is list and ratios, "ratios")
    for ratio in ratios:
        number(ratio, 1e-12)
    randomizer = random.Random(0)
    estimates = sorted(statistics.median(randomizer.choices(ratios, k=len(ratios)))
                       for _ in range(10000))
    median = statistics.median(ratios)
    low, high = estimates[249], estimates[9749]
    return {"inputs": len(ratios), "median": median, "ci95": [low, high],
            "share_faster": sum(r < 1 for r in ratios) / len(ratios),
            "bar_pass": median <= 1, "faster": median <= 1 and high < 1}


def thread_controls(value, registration):
    require(value == caller_settings(registration) and type(value["coreav_owner_threads"]) is int and
            type(value["ffmpeg_input_decoder_threads"]) is int, "caller_settings")


def gate(value, registration, phase, baseline_files):
    keys(value, "schema session_id phase registration_sha256 host tools settings_sha256 "
         "aa null observations started_ns ended_ns covered_classes uncovered_classes")
    require(coverage(value) == roster_coverage(registration), "class_coverage")
    require(value["schema"] == "coreav.videotoolbox-host-gate/3" and
            value["session_id"] == registration["session_id"] and value["phase"] == phase,
            "gate_identity")
    digest(value["registration_sha256"])
    require(value["host"] == registration["host"] and
            value["tools"] == registration["executables"], "gate_binding")
    digest(value["settings_sha256"])
    integer(value["started_ns"], 1, 2**63 - 1)
    integer(value["ended_ns"], value["started_ns"], 2**63 - 1)
    keys(value["observations"], "swap_bytes thermal_warning throttled competing_encoder "
         "background_load caller_settings encoder_threads memory_limit affinity hardware_sha256")
    observed = value["observations"]
    thread_controls(observed["caller_settings"], registration)
    require(observed["hardware_sha256"] == registration["host"]["hardware_sha256"], "host_mismatch")
    require(type(observed["swap_bytes"]) is int and observed["swap_bytes"] == 0 and
            all(observed[k] is False for k in
                ("thermal_warning", "throttled", "competing_encoder")), "host_fault")
    number(observed["background_load"], 0, 8)
    require(observed["caller_settings"] == caller_settings(registration) and
            observed["encoder_threads"] == "framework_managed_uncontrolled" and observed["memory_limit"] == "none" and
            observed["affinity"] == "unsupported", "host_limits")
    medians = []
    for arm in ("aa", "null"):
        require(type(value[arm]) is list and len(value[arm]) == 6, "gate_samples")
        ratios = []
        left, right = [], []
        for index, pair in enumerate(value[arm]):
            keys(pair, "a_ns b_ns a_sha256 b_sha256 a_cache_before a_cache_after b_cache_before b_cache_after "
                 "a_started_ns a_ended_ns b_started_ns b_ended_ns a_baseline b_baseline")
            integer(pair["a_ns"], 1, 3600 * 10**9)
            integer(pair["b_ns"], 1, 3600 * 10**9)
            digest(pair["a_sha256"])
            digest(pair["b_sha256"])
            require(pair["a_sha256"] == pair["b_sha256"] ==
                    registration["executables"]["ffmpeg"]["sha256"], "gate_tools")
            if index:
                left.append(pair["a_ns"])
                right.append(pair["b_ns"])
                ratios.append(pair["a_ns"] / pair["b_ns"])
            for side in ("a", "b"):
                integer(pair[side + "_started_ns"], 1, 2**63 - 1)
                integer(pair[side + "_ended_ns"], pair[side + "_started_ns"], 2**63 - 1)
                cache.pair(pair[side + "_cache_before"], pair[side + "_cache_after"], pair[side + "_started_ns"],
                           pair[side + "_ended_ns"], registration, registration["inputs"][0])
                descriptor = pair[side + "_baseline"]
                require(descriptor["directory"] == f"gate-{phase}.artifacts/{arm}-{index}-{side}", "gate_artifact_identity")
                gate_baseline(baseline_files(descriptor), registration, registration["inputs"][0], pair[side + "_ended_ns"])
        medians.append((statistics.median(left), statistics.median(right)))
        if arm == "aa":
            require(abs(medians[-1][0] / medians[-1][1] - 1) <= 0.03, "aa_gate")
        else:
            low, high = summary(ratios)["ci95"]
            require(low <= 1 <= high, "null_gate")
    return statistics.median(medians[0])


def rational(value, positive=False):
    keys(value, "value scale")
    numerator = integer(value["value"], 1 if positive else -(2**53), 2**53)
    return Fraction(numerator, integer(value["scale"], 1, 2**31 - 1))


def packet_facts(value, item, payload_bytes):
    covered_class(item)
    require(type(value) is list and len(value) == item["frames"], "packet_clock_count")
    total, pts = 0, []
    for index, row in enumerate(value):
        keys(row, "pts_value dts_value time_base bytes")
        tick = clock(row["time_base"])
        position = integer(row["pts_value"], -(2**53), 2**53) * tick
        dts = integer(row["dts_value"], -(2**53), 2**53) * tick
        require(abs(position - Fraction(index, 1) / clock(item["rate"])) <= clock(item["source_tick"]) / 2 and
                position == dts and (not pts or (position > pts[-1] and
                abs(position - pts[-1] - 1 / clock(item["rate"])) <= clock(item["source_tick"]) / 2)), "clock")
        total += integer(row["bytes"], 1, 1024**3)
        pts.append(position)
    require(total == payload_bytes, "packet_payload_size")
    return pts


def gate_baseline(value, registration, item, encoder_ended_ns):
    from observations import BASELINE_FILES, identity_record, trace, validate_requested
    from producer import packet_ledger
    keys(value, "files specification output_sha256 payload_bytes packet_text")
    require(type(value["files"]) is dict and set(value["files"]) == set(BASELINE_FILES), "identity_binding")
    files = value["files"]
    require(all(type(data) is str and len(data.encode()) <= BASELINE_FILES[name]
                for name, data in files.items()), "identity_binding")
    query, report = loads(files["identity-query.json"]), loads(files["identity.json"])
    observed, _ = trace(files["observations.txt"].encode(), value["specification"])
    digest(value["output_sha256"])
    identity_record(query, report, sha(files["observations.txt"].encode()), registration, "hardware",
                    value["output_sha256"], encoder_ended_ns)
    require(observed["encoder_id"] == report["encoder_id"] and files["identity-error"] == "", "identity_binding")
    for name, field in (("identity-out", "stdout_sha256"), ("identity-error", "stderr_sha256"),
                        ("identity.json", "report_sha256")):
        require(sha(files[name].encode()) == query["process"][field], "identity_binding")
    validate_requested(loads(files["requested.json"]), item, registration, "hardware")
    integer(value["payload_bytes"], 1, 1024**3)
    require(type(value["packet_text"]) is str and len(value["packet_text"].encode()) <= 16 * 1024**2,
            "packet_clock_bounds")
    packet_ledger(value["packet_text"].encode(), item, value["payload_bytes"])


def conformance(value, item, cell, side, output_hash, registration):
    from observations import encoder_identifier
    roster_coverage(registration)
    covered_class(item)
    keys(value, "schema input_id cell side output_sha256 raw_sha256 producer_sha256 decoders "
         "width height pixel_format colour profile reordering keyframe_max_frames "
         "keyframe_max_seconds encoder_id hardware frames audio_offset_error "
         "audio_applicability parameter_sets_agree sample_description_applicability "
         "payload_bytes container_bytes metrics retained stream_clock level baseline requested_settings packet_ledger packet_text class scope")
    require(type(value["retained"]) is dict, "retained_coverage")
    require(value["schema"] == "coreav.videotoolbox-output-evidence/2" and
            value["scope"] == SCOPE and type(value["class"]) is int and value["class"] == item["class"] and
            value["input_id"] == item["id"] and value["cell"] == cell and
            value["side"] == side and value["output_sha256"] == output_hash and
            value["raw_sha256"] == item["raw_sha256"] and
            value["producer_sha256"] == registration["metric_tools"]["evidence_producer"],
            "evidence_binding")
    encoder_identifier(value["encoder_id"])
    require(value["hardware"] == cell, "encoder_identity")
    integer(value["level"], 1, 255)
    if side == "ffmpeg":
        from observations import identity_record, validate_requested
        from producer import packet_ledger
        keys(value["baseline"], "query identity")
        retained = value["retained"]
        require({"observations.txt", "identity-query.json", "identity.json", "identity-out", "identity-error",
                 "requested.json", "packets.stats"} <= set(retained), "identity_binding")
        query = value["baseline"]["query"]
        report = identity_record(query, value["baseline"]["identity"], retained["observations.txt"]["sha256"],
                                 registration, cell, output_hash)
        require(report["encoder_id"] == value["encoder_id"] and report["hardware"] == value["hardware"],
                "identity_binding")
        for name, field in (("identity-out", "stdout_sha256"), ("identity-error", "stderr_sha256"),
                            ("identity.json", "report_sha256")):
            require(query["process"][field] == retained[name]["sha256"], "identity_binding")
        validate_requested(value["requested_settings"], item, registration, cell)
        for filename, record in (("identity-query.json", query), ("requested.json", value["requested_settings"])):
            require(sha((json.dumps(record, sort_keys=True, indent=2, allow_nan=False) + "\n").encode()) ==
                    retained[filename]["sha256"], "identity_binding")
        require(type(value["packet_text"]) is str and len(value["packet_text"].encode()) <= 16 * 1024**2 and
                sha(value["packet_text"].encode()) == retained["packets.stats"]["sha256"], "packet_clock")
        require(packet_ledger(value["packet_text"].encode(), item, value["payload_bytes"]) == value["packet_ledger"],
                "packet_clock")
        packet_pts = packet_facts(value["packet_ledger"], item, value["payload_bytes"])
    else:
        require(side == "coreav" and value["baseline"] is None and value["requested_settings"] is None and
                value["packet_ledger"] is None and value["packet_text"] is None, "evidence_binding")
    require(type(value["width"]) is int and value["width"] == item["width"] and
            type(value["height"]) is int and value["height"] == item["height"] and
            value["pixel_format"] == "yuv420p" and value["colour"] == item["colour"] == COLOUR and
            value["profile"] == "high" and value["reordering"] is False and
            value["parameter_sets_agree"] is True and
            value["sample_description_applicability"] == "not_applicable_elementary", "conformance")
    maximum = int(2 * clock(item["rate"]))
    integer(value["keyframe_max_frames"], 1, maximum)
    number(value["keyframe_max_seconds"], 0, 2)
    require(value["audio_applicability"] == "not_applicable_raw_driver" and
            value["audio_offset_error"] is None, "audio_applicability")
    keys(value["decoders"], "ffmpeg coreav")
    for decoder, record in value["decoders"].items():
        keys(record, "sha256 status frame_sha256 bytes")
        expected_tool = registration["metric_tools"]["ffmpeg" if decoder == "ffmpeg" else "coreav_decoder"]
        require(record["sha256"] == expected_tool and record["status"] == "complete", "decoder")
        require(type(record["frame_sha256"]) is list and
                len(record["frame_sha256"]) == item["frames"], "decoder_frames")
        for expected in record["frame_sha256"]:
            digest(expected)
        require(type(record["bytes"]) is int and record["bytes"] ==
                item["width"] * item["height"] * 3 // 2 * item["frames"], "decoder_bytes")
    require(value["decoders"]["ffmpeg"]["frame_sha256"] ==
            value["decoders"]["coreav"]["frame_sha256"], "decoder_disagreement")
    require(type(value["frames"]) is list and len(value["frames"]) == item["frames"], "frame_coverage")
    keys(value["stream_clock"], "source_rate provenance duration_provenance demux_time_base_diagnostic advertised_rate_diagnostic")
    require(value["stream_clock"]["source_rate"] == item["rate"] and
            clock(value["stream_clock"]["source_rate"]) == clock(item["rate"]) and
            value["stream_clock"]["provenance"] == ("native_report" if side == "coreav" else "packet_ledger") and
            value["stream_clock"]["duration_provenance"] == "original_source_rational", "stream_clock")
    clock(value["stream_clock"]["demux_time_base_diagnostic"])
    advertised = value["stream_clock"]["advertised_rate_diagnostic"]
    keys(advertised, "num den")
    integer(advertised["num"], 0, 2**31 - 1)
    integer(advertised["den"], 1, 2**31 - 1)
    for index, frame in enumerate(value["frames"]):
        keys(frame, "id source_sha256 decoded_sha256 pts duration sync")
        require(type(frame["id"]) is int and frame["id"] == index and
                frame["source_sha256"] == item["source_frame_sha256"][index] and
                frame["decoded_sha256"] == value["decoders"]["ffmpeg"]["frame_sha256"][index],
                "frame_identity")
        expected = Fraction(index, 1) / clock(item["rate"])
        observed_duration = rational(frame["duration"], True)
        require(abs(rational(frame["pts"]) - expected) <= clock(item["source_tick"]) / 2 and
                abs(observed_duration - 1 / clock(item["rate"])) <= clock(item["source_tick"]) / 2,
                "clock")
        if side == "ffmpeg":
            require(rational(frame["pts"]) == packet_pts[index], "clock")
        require(type(frame["sync"]) is bool and (index != 0 or frame["sync"]), "keyframe")
    sync = [index for index, frame in enumerate(value["frames"]) if frame["sync"]]
    gaps = [b - a for a, b in zip(sync, sync[1:] + [item["frames"]])]
    end = rational(value["frames"][-1]["pts"]) + rational(value["frames"][-1]["duration"], True)
    seconds = [(rational(value["frames"][b]["pts"]) if b < item["frames"] else end) -
               rational(value["frames"][a]["pts"]) for a, b in zip(sync, sync[1:] + [item["frames"]])]
    require(all(gap <= maximum for gap in gaps) and value["keyframe_max_frames"] == max(gaps) and
            all(0 < gap <= 2 for gap in seconds) and value["keyframe_max_seconds"] == float(max(seconds)), "keyframe")
    integer(value["payload_bytes"], 1, 1024**3)
    require(value["container_bytes"] is None, "container_applicability")
    duration = Fraction(item["frames"], 1) / clock(item["rate"])
    target = dict(CLASSES)[item["class"]]
    if duration >= 8:
        require(Fraction(9, 10) <= Fraction(value["payload_bytes"] * 8, 1) / duration / target <=
                Fraction(11, 10), "bitrate")
    metrics = value["metrics"]
    keys(metrics, "tool_sha256 model_sha256 model resolution alignment psnr_planes ssim_component frames")
    require(metrics["tool_sha256"] == registration["metric_tools"]["metrics"] and
            metrics["model_sha256"] == registration["metric_tools"]["model"] and
            metrics["model"] == "vmaf_v0.6.1" and metrics["resolution"] == "native" and
            metrics["alignment"] == "fixed_presentation" and metrics["psnr_planes"] == "all" and
            metrics["ssim_component"] == "All", "metric_settings")
    require(type(metrics["frames"]) is list and len(metrics["frames"]) == item["frames"], "metric_frames")
    for index, frame in enumerate(metrics["frames"]):
        keys(frame, "id source_sha256 decoded_sha256 vmaf psnr ssim psnr_status squared_error samples")
        require(type(frame["id"]) is int and frame["id"] == index and
                frame["source_sha256"] == item["source_frame_sha256"][index] and
                frame["decoded_sha256"] == value["frames"][index]["decoded_sha256"], "metric_identity")
        number(frame["vmaf"], 0, 100)
        samples = item["width"] * item["height"] * 3 // 2
        require(type(frame["samples"]) is int and frame["samples"] == samples, "metric_samples")
        squared = integer(frame["squared_error"], 0, 255**2 * samples)
        if squared:
            number(frame["psnr"], 0, 1000)
            require(frame["psnr_status"] == "finite" and abs(frame["psnr"] -
                    10 * math.log10(255**2 * samples / squared)) <= 1e-9, "psnr_aggregate")
        else:
            require(frame["psnr"] is None and frame["psnr_status"] == "perfect", "psnr_aggregate")
        number(frame["ssim"], -1, 1)
    squared = sum(frame["squared_error"] for frame in metrics["frames"])
    samples = sum(frame["samples"] for frame in metrics["frames"])
    return {"vmaf": statistics.mean(frame["vmaf"] for frame in metrics["frames"]),
            "ssim": statistics.mean(frame["ssim"] for frame in metrics["frames"]),
            "psnr": 10 * math.log10(255**2 * samples / squared) if squared else None}


def deltas(coreav, baseline):
    result = {name: coreav[name] - baseline[name] for name in ("vmaf", "ssim")}
    a, b = coreav["psnr"], baseline["psnr"]
    result["psnr"] = (0 if a is None and b is None else math.inf if a is None else
                      -math.inf if b is None else a - b)
    return result


def quality(deltas, fixtures, registration):
    declaration = roster_coverage(registration)
    require(type(deltas) is dict and set(deltas) == set(declaration["covered_classes"]) and
            all(type(height) is int for height in deltas) and
            all(type(rows) is list and len(rows) == 10 for rows in deltas.values()) and fixtures,
            "quality_coverage")
    floors = {"vmaf": -2, "psnr": -0.5, "ssim": -0.005}
    passes = lambda row: all(row[k] >= limit for k, limit in floors.items())
    require(all(passes(row) for row in fixtures), "fixture_quality")
    for rows in deltas.values():
        require(sum(passes(row) for row in rows) * 100 >= 98 * len(rows), "input_quality")
        for name, floor in (("vmaf", -1), ("psnr", -0.10), ("ssim", -0.001)):
            require(statistics.median(row[name] for row in rows) >= floor, "median_quality")


def timings(records, registration):
    declaration = roster_coverage(registration)
    require(type(records) is list, "timings")
    expected = [(cell, item["id"], iteration, side)
                for cell in registration["cells"] for item in registration["inputs"]
                for iteration in range(6)
                for side in (("coreav", "ffmpeg") if iteration % 2 == 0 else ("ffmpeg", "coreav"))]
    require(len(records) == len(expected), "timing_coverage")
    results = {}
    for record, identity in zip(records, expected):
        keys(record, "cell input_id iteration side elapsed_ns status output_sha256 output_bytes "
             "native_report_sha256 started_ns ended_ns host_before host_after packet_clock_sha256 observations_sha256 "
             "identity_query_sha256 requested_sha256 cache_before cache_after")
        require((record["cell"], record["input_id"], record["iteration"], record["side"]) == identity,
                "timing_order")
        require(type(record["iteration"]) is int and record["status"] == "success", "timing_status")
        for observation in (record["host_before"], record["host_after"]):
            keys(observation, "swap_bytes thermal_warning throttled competing_encoder background_load "
                 "caller_settings encoder_threads memory_limit affinity hardware_sha256")
            require(observation["hardware_sha256"] == registration["host"]["hardware_sha256"], "host_mismatch")
            thread_controls(observation["caller_settings"], registration)
            require(type(observation["swap_bytes"]) is int and observation["swap_bytes"] == 0 and
                    all(observation[k] is False for k in
                        ("thermal_warning", "throttled", "competing_encoder")) and
                    observation["caller_settings"] == caller_settings(registration) and
                    observation["encoder_threads"] == "framework_managed_uncontrolled" and
                    observation["memory_limit"] == "none" and
                    observation["affinity"] == "unsupported", "host_fault")
            number(observation["background_load"], 0, 8)
        integer(record["elapsed_ns"], 1, 3600 * 10**9)
        integer(record["started_ns"], 1, 2**63 - 1)
        require(type(record["ended_ns"]) is int and
                record["ended_ns"] - record["started_ns"] >= record["elapsed_ns"], "timing_clock")
        item = next(item for item in registration["inputs"] if item["id"] == record["input_id"])
        cache.pair(record["cache_before"], record["cache_after"], record["started_ns"], record["ended_ns"], registration, item)
        digest(record["output_sha256"])
        integer(record["output_bytes"], 1, 1024**3)
        require(record["native_report_sha256"] is None if record["side"] == "ffmpeg" else
                bool(digest(record["native_report_sha256"])), "native_report")
        for field in ("packet_clock_sha256", "observations_sha256", "identity_query_sha256", "requested_sha256"):
            require(record[field] is None if record["side"] == "coreav" else bool(digest(record[field])), "baseline_evidence")
        if record["iteration"]:
            results.setdefault((record["cell"], record["input_id"], record["side"]), []).append(record["elapsed_ns"])
    summaries = {}
    for cell in registration["cells"]:
        per_input = [{"input_id": item["id"], "class": item["class"],
                      "ratio": statistics.median(results[cell, item["id"], "coreav"]) /
                      statistics.median(results[cell, item["id"], "ffmpeg"])}
                     for item in registration["inputs"]]
        summaries[cell] = {**declaration, "scope": SCOPE, "inputs": per_input,
                           "overall": summary([row["ratio"] for row in per_input]),
                           "classes": {str(height): summary([row["ratio"] for row in per_input
                                                             if row["class"] == height])
                                       for height in declaration["covered_classes"]}}
    return summaries
