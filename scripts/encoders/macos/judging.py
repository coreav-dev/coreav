"""Replay retained evidence; never infer acceptance from elapsed times alone."""

from fractions import Fraction
import math
import random
import statistics

from evidence import digest, integer, keys, number, require
from registration import CLASSES, COLOUR, caller_settings, clock
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


def gate(value, registration, phase):
    keys(value, "schema session_id phase registration_sha256 host tools settings_sha256 "
         "aa null observations started_ns ended_ns")
    require(value["schema"] == "coreav.videotoolbox-host-gate/1" and
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
        require(type(value[arm]) is list and len(value[arm]) >= 5, "gate_samples")
        ratios = []
        left, right = [], []
        for pair in value[arm]:
            keys(pair, "a_ns b_ns a_sha256 b_sha256 a_cache_before a_cache_after b_cache_before b_cache_after "
                 "a_started_ns a_ended_ns b_started_ns b_ended_ns")
            left.append(integer(pair["a_ns"], 1, 3600 * 10**9))
            right.append(integer(pair["b_ns"], 1, 3600 * 10**9))
            digest(pair["a_sha256"])
            digest(pair["b_sha256"])
            require(pair["a_sha256"] == pair["b_sha256"] ==
                    registration["executables"]["ffmpeg"]["sha256"], "gate_tools")
            ratios.append(pair["a_ns"] / pair["b_ns"])
            for side in ("a", "b"):
                integer(pair[side + "_started_ns"], 1, 2**63 - 1)
                integer(pair[side + "_ended_ns"], pair[side + "_started_ns"], 2**63 - 1)
                cache.pair(pair[side + "_cache_before"], pair[side + "_cache_after"], pair[side + "_started_ns"],
                           pair[side + "_ended_ns"], registration, registration["inputs"][0])
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


def conformance(value, item, cell, side, output_hash, registration):
    keys(value, "schema input_id cell side output_sha256 raw_sha256 producer_sha256 decoders "
         "width height pixel_format colour profile reordering keyframe_max_frames "
         "keyframe_max_seconds encoder_id hardware frames audio_offset_error "
         "audio_applicability parameter_sets_agree sample_description_applicability "
         "payload_bytes container_bytes metrics retained stream_clock")
    require(type(value["retained"]) is dict, "retained_coverage")
    require(value["schema"] == "coreav.videotoolbox-output-evidence/1" and
            value["input_id"] == item["id"] and value["cell"] == cell and
            value["side"] == side and value["output_sha256"] == output_hash and
            value["raw_sha256"] == item["raw_sha256"] and
            value["producer_sha256"] == registration["metric_tools"]["evidence_producer"],
            "evidence_binding")
    require(type(value["encoder_id"]) is str and 0 < len(value["encoder_id"]) <= 512 and
            value["hardware"] == cell, "encoder_identity")
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
    keys(value["stream_clock"], "time_base advertised_rate")
    stream_tick = clock(value["stream_clock"]["time_base"])
    require(clock(value["stream_clock"]["advertised_rate"]) == clock(item["rate"]), "stream_clock")
    for index, frame in enumerate(value["frames"]):
        keys(frame, "id source_sha256 decoded_sha256 pts duration sync")
        require(type(frame["id"]) is int and frame["id"] == index and
                frame["source_sha256"] == item["source_frame_sha256"][index] and
                frame["decoded_sha256"] == value["decoders"]["ffmpeg"]["frame_sha256"][index],
                "frame_identity")
        expected = Fraction(index, 1) / clock(item["rate"])
        observed_duration = rational(frame["duration"], True)
        require(abs(rational(frame["pts"]) - expected) <= clock(item["source_tick"]) / 2 and
                abs(observed_duration - 1 / clock(item["rate"])) <= clock(item["source_tick"]) / 2 and
                (observed_duration / stream_tick).denominator == 1, "clock")
        require(type(frame["sync"]) is bool and (index != 0 or frame["sync"]), "keyframe")
    sync = [index for index, frame in enumerate(value["frames"]) if frame["sync"]]
    require(all(b - a <= maximum for a, b in zip(sync, sync[1:] + [item["frames"]])), "keyframe")
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


def quality(deltas, fixtures):
    require(type(deltas) is dict and set(deltas) == {height for height, _ in CLASSES} and
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
             "cache_before cache_after")
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
        for field in ("packet_clock_sha256", "observations_sha256"):
            require(record[field] is None if record["side"] == "coreav" else bool(digest(record[field])), "baseline_evidence")
        if record["iteration"]:
            results.setdefault((record["cell"], record["input_id"], record["side"]), []).append(record["elapsed_ns"])
    summaries = {}
    for cell in registration["cells"]:
        per_input = [{"input_id": item["id"], "class": item["class"],
                      "ratio": statistics.median(results[cell, item["id"], "coreav"]) /
                      statistics.median(results[cell, item["id"], "ffmpeg"])}
                     for item in registration["inputs"]]
        summaries[cell] = {"inputs": per_input,
                           "overall": summary([row["ratio"] for row in per_input]),
                           "classes": {str(height): summary([row["ratio"] for row in per_input
                                                             if row["class"] == height])
                                       for height, _ in CLASSES}}
    return summaries
