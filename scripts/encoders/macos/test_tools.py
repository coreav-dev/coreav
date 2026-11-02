"""Synthetic records and fake commands only; never the registered roster."""

import ast
import copy
import math
import os
from pathlib import Path
import select
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import evidence
import judging
import process
import registration
import smoke
import timing


def h(value):
    return evidence.sha(str(value).encode())


def observation():
    return {"swap_bytes": 0, "thermal_warning": False, "throttled": False,
            "competing_encoder": False, "background_load": 0.5,
            "caller_settings": {"coreav_owner_threads": 1, "ffmpeg_input_decoder_threads": 1,
                                "environment": {"OMP_NUM_THREADS": "1", "VECLIB_MAXIMUM_THREADS": "1"}},
            "encoder_threads": "framework_managed_uncontrolled", "memory_limit": "none", "affinity": "unsupported",
            "hardware_sha256": h("hardware")}


def cache_fact(source, started=1, ended=2):
    size = registration.raw_size(source)
    pages = (size + 16383) // 16384
    return {"observer_sha256": h("cache-observer"), "raw_sha256": source["raw_sha256"], "bytes": size,
            "page_size": 16384, "file_identity_sha256": h(source["id"]),
            "stdout": f"Files: 1\nDirectories: 0\nResident Pages: {pages}/{pages} {pages * 16384}/{pages * 16384} 100%\n",
            "started_ns": started, "ended_ns": ended}


def item(index=0, height=240):
    frames = 240
    return {"id": h(index), "class": next(bound for bound, _ in registration.CLASSES if height <= bound),
            "source_sha256": h(index), "raw_file": f"raw/{index}.yuv", "raw_sha256": h(f"raw{index}"),
            "stream": 0, "width": 64, "height": height, "frames": frames,
            "rate": {"num": 30, "den": 1}, "colour": dict(registration.COLOUR),
            "source_frame_sha256": [h(f"frame{n}") for n in range(frames)],
            "audio_start_ticks": None, "source_start_ticks": 0,
            "source_tick": {"num": 1, "den": 90000}}


def registered():
    inputs = [item(f"{height}-{n}", height) for height in (240, 480, 900, 1200) for n in range(10)]
    inputs.sort(key=lambda row: (row["class"], h("coreav-encoders-v1:" + row["source_sha256"])))
    fixture = item("fixture")
    fixture = {k: fixture[k] for k in ("id", "class", "width", "height", "frames", "rate", "colour", "raw_file",
                                       "raw_sha256", "source_frame_sha256", "source_tick")}
    fixture["id"] = "fixture"
    result = {"schema": "coreav.videotoolbox-registration/1", "candidate": "a" * 40,
              "tooling": "b" * 40, "plan_sha256": h("plan"), "tools": {"tool.py": h("tool")},
              "host": {**registration.HOST, "os_build": "24G-test", "allocation_sha256": h("allocation"),
                       "hardware_sha256": h("hardware")},
              "executables": {side: {"file": f"bin/{side}", "sha256": h(side),
                                     "configuration_sha256": h(f"config{side}")}
                              for side in ("coreav", "ffmpeg")},
              "baseline_build": {"file": "build.json", "sha256": h("build"), "review_commit": "c" * 40},
              "coreav_builds": {host: {"file": f"{host}-build.json", "sha256": h(f"{host}-build"),
                                       "review_commit": "e" * 40} for host in ("macos", "linux")},
              "eligibility": {"file": "eligible.json", "sha256": h("eligible"), "cutoff_commit": "d" * 40},
              "settings": dict(registration.SETTINGS),
              "environment": {"OMP_NUM_THREADS": "1", "VECLIB_MAXIMUM_THREADS": "1"},
              "cache_observer": {"file": "bin/cache-observer", "sha256": h("cache-observer"),
                                 "kind": "vmtouch-verbose/1", "page_size": 16384, "arguments": ["-v", "-m", "32G"]},
              "cells": ["hardware"], "inputs": inputs, "fixtures": {"fixture": fixture},
              "metric_tools": {name: h(name) for name in
                               ("ffmpeg", "ffprobe", "coreav_decoder", "metrics", "model", "evidence_producer")},
              "session_id": "synthetic",
              "command_controls": {cell: ["-profile:v", "high", "-bf", "0", "-allow_sw",
                                          "1" if cell == "software" else "0", "-require_sw",
                                          "1" if cell == "software" else "0"]
                                   for cell in ("hardware", "software")}}
    result["commands"] = {cell: registration.command_templates(result, cell) for cell in ("hardware", "software")}
    return result


def native(mode="hardware", unavailable=False):
    return {"schema": "coreav.videotoolbox-smoke/1", "status": "unavailable" if unavailable else "encoded",
            "requested_mode": mode, "unavailable_reason": "no_encoder" if unavailable else None,
            "width": 64, "height": 64, "fps_num": 30, "fps_den": 1, "bitrate": 2000000,
            "submitted": 0 if unavailable else 4, "encoder_id": None if unavailable else "test.encoder",
            "hardware": "unknown" if unavailable else mode,
            "frames": [] if unavailable else [
                {"id": n, "pts_value": n, "pts_scale": 30, "duration_value": 1,
                 "duration_scale": 30, "sync": n == 0} for n in range(4)],
            "elementary_file": None if unavailable else "video.h264",
            "source_file": None if unavailable else "source.yuv"}


def smoke_directory(path, mode="hardware", unavailable=False):
    record = native(mode, unavailable)
    evidence.write(path / "manifest.json", record)
    if not unavailable:
        (path / "video.h264").write_bytes(b"fake-encoded-record")
        (path / "source.yuv").write_bytes(smoke.source())
    names = smoke.manifest(record, mode)
    evidence.write(path / "capture.json", {
        "schema": "coreav.videotoolbox-capture/1", "requested_mode": mode,
        "exporter_sha256": h("exporter"), "runner_image": "synthetic-ci",
        "os_build": "synthetic-build", "machine": "arm64",
        "files": evidence.bind(path, names, smoke.LIMIT)})


def proof(reg, source, side="coreav"):
    count = source["frames"]
    decoded = [h(f"decoded-{n}") for n in range(count)]
    result = {
        "schema": "coreav.videotoolbox-output-evidence/1", "input_id": source["id"],
        "cell": "hardware", "side": side, "output_sha256": h("output"),
        "raw_sha256": source["raw_sha256"], "producer_sha256": reg["metric_tools"]["evidence_producer"],
        "width": source["width"], "height": source["height"], "pixel_format": "yuv420p",
        "colour": dict(registration.COLOUR), "profile": "high", "reordering": False,
        "keyframe_max_frames": 60, "keyframe_max_seconds": 2,
        "encoder_id": "synthetic.encoder", "hardware": "hardware",
        "frames": [{"id": n, "source_sha256": source["source_frame_sha256"][n],
                    "decoded_sha256": decoded[n], "pts": {"value": n, "scale": 30},
                    "duration": {"value": 1, "scale": 30}, "sync": n % 60 == 0}
                   for n in range(count)],
        "audio_offset_error": None, "audio_applicability": "not_applicable_raw_driver",
        "parameter_sets_agree": True, "sample_description_applicability": "not_applicable_elementary",
        "payload_bytes": dict(registration.CLASSES)[source["class"]], "container_bytes": None,
        "stream_clock": {"time_base": {"num": 1, "den": 30}, "advertised_rate": source["rate"]},
        "retained": {},
        "decoders": {name: {"sha256": reg["metric_tools"]["ffmpeg" if name == "ffmpeg" else "coreav_decoder"],
                            "status": "complete", "frame_sha256": list(decoded),
                            "bytes": source["width"] * source["height"] * 3 // 2 * count}
                     for name in ("ffmpeg", "coreav")},
        "metrics": {"tool_sha256": reg["metric_tools"]["metrics"], "model_sha256": reg["metric_tools"]["model"],
                    "model": "vmaf_v0.6.1", "resolution": "native", "alignment": "fixed_presentation",
                    "psnr_planes": "all", "ssim_component": "All",
                    "frames": [{"id": n, "source_sha256": source["source_frame_sha256"][n],
                                "decoded_sha256": decoded[n], "vmaf": 90,
                                "psnr": 10 * math.log10(255**2 / 4), "ssim": 0.99,
                                "psnr_status": "finite", "squared_error": source["width"] * source["height"] * 6,
                                "samples": source["width"] * source["height"] * 3 // 2}
                               for n in range(count)]}}
    return result


def host_gate(reg, phase="before"):
    pair = {"a_ns": 100, "b_ns": 100, "a_sha256": reg["executables"]["ffmpeg"]["sha256"],
            "b_sha256": reg["executables"]["ffmpeg"]["sha256"],
            "a_cache_before": cache_fact(reg["inputs"][0]), "a_cache_after": cache_fact(reg["inputs"][0], 120, 121),
            "b_cache_before": cache_fact(reg["inputs"][0]), "b_cache_after": cache_fact(reg["inputs"][0], 120, 121),
            "a_started_ns": 10, "a_ended_ns": 110, "b_started_ns": 10, "b_ended_ns": 110}
    return {"schema": "coreav.videotoolbox-host-gate/1", "session_id": reg["session_id"],
            "phase": phase, "registration_sha256": h("registration"), "host": reg["host"],
            "tools": reg["executables"], "settings_sha256": timing.settings_hash(reg),
            "aa": [dict(pair) for _ in range(5)], "null": [dict(pair) for _ in range(5)],
            "observations": observation(), "started_ns": 1, "ended_ns": 2}


def calls(reg):
    records = []
    for cell in reg["cells"]:
        for source in reg["inputs"]:
            for iteration in range(6):
                for side in (("coreav", "ffmpeg") if iteration % 2 == 0 else ("ffmpeg", "coreav")):
                    duration = 90 if side == "coreav" else 100
                    records.append({"cell": cell, "input_id": source["id"], "iteration": iteration,
                                    "side": side, "elapsed_ns": duration, "status": "success",
                                    "output_sha256": h("output"), "output_bytes": 1000,
                                    "native_report_sha256": h("native") if side == "coreav" else None,
                                    "packet_clock_sha256": h("packets") if side == "ffmpeg" else None,
                                    "observations_sha256": h("observations") if side == "ffmpeg" else None,
                                    "started_ns": 10, "ended_ns": 110,
                                    "cache_before": cache_fact(source), "cache_after": cache_fact(source, 120, 121),
                                    "host_before": observation(), "host_after": observation()})
    return records


class Records(unittest.TestCase):
    def test_duplicate_json_keys_and_nonfinite_values_fail(self):
        for data in ('{"a": 1, "a": 2}', '{"a": NaN}', '{"a": Infinity}', '[1e999]'):
            with self.subTest(data=data):
                if "1e999" in data:
                    with self.assertRaises(evidence.Invalid):
                        evidence.number(evidence.loads(data)[0])
                else:
                    with self.assertRaises(evidence.Invalid):
                        evidence.loads(data)

    def test_paths_symlinks_sizes_and_hash_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a").write_bytes(b"a")
            record = evidence.bind(root, ["a"], 1)
            evidence.verify(root, record, ["a"], 1)
            for names in ([], ["a", "b"]):
                with self.assertRaises(evidence.Invalid):
                    evidence.verify(root, record, names, 1)
            for path in ("../a", "/a", "a/../a", "a//a"):
                with self.assertRaises(evidence.Invalid):
                    evidence.read(root, path, 1)
            with self.assertRaises(evidence.Invalid):
                evidence.read(root, "a", 0)
            (root / "link").symlink_to(root / "a")
            with self.assertRaises((evidence.Invalid, OSError)):
                evidence.read(root, "link", 1)
            (root / "nested").symlink_to(root, target_is_directory=True)
            with self.assertRaises((evidence.Invalid, OSError)):
                evidence.read(root, "nested/a", 1)
            (root / "a").write_bytes(b"b")
            with self.assertRaises(evidence.Invalid):
                evidence.verify(root, record, ["a"], 1)


class Smoke(unittest.TestCase):
    def test_independent_source_contract_and_fixed_frame_order(self):
        expected = b"".join(bytes([level]) * 64 * 64 + b"\x80" * (64 * 64 // 2)
                            for level in (37, 91, 145, 199))
        self.assertEqual(smoke.source(), expected)
        self.assertEqual(len(expected), 24576)
        smoke.pixels(expected)
        for changed in (expected[:-1], expected[6144:] + expected[:6144],
                        bytes([46]) + expected[1:]):
            with self.assertRaises(evidence.Invalid):
                smoke.pixels(changed)
        tolerated = bytes([45]) * 4096 + expected[4096:]
        smoke.pixels(tolerated)

    def test_status_settings_clocks_and_identity_are_explicit(self):
        smoke.manifest(native(), "hardware")
        smoke.manifest(native("software", True), "software")
        mutations = [lambda r: r.update(hardware="unknown"),
                     lambda r: r.update(width=True), lambda r: r.update(fps_num=29),
                     lambda r: r.update(elementary_file="../video.h264"),
                     lambda r: r["frames"][1].update(id=0),
                     lambda r: r["frames"][1].update(pts_scale=29),
                     lambda r: r["frames"][1].update(duration_value=2),
                     lambda r: r["frames"].reverse(), lambda r: r["frames"].pop(),
                     lambda r: r.update(unavailable_reason="no_encoder"),
                     lambda r: r.update(extra=True)]
        for mutate in mutations:
            record = native()
            mutate(record)
            with self.subTest(record=record), self.assertRaises(evidence.Invalid):
                smoke.manifest(record, "hardware")
        for change in ({"submitted": 1}, {"frames": native()["frames"]},
                       {"unavailable_reason": "timeout"}, {"source_file": "source.yuv"}):
            record = native("software", True)
            record.update(change)
            with self.assertRaises(evidence.Invalid):
                smoke.manifest(record, "software")

    def test_transferred_artifacts_have_complete_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            smoke_directory(root)
            smoke.artifacts(root, "hardware", True)
            (root / "extra").write_bytes(b"unexpected")
            with self.assertRaises(evidence.Invalid):
                smoke.artifacts(root, "hardware", True)
            (root / "extra").unlink()
            (root / "source.yuv").write_bytes(b"a" * 24576)
            with self.assertRaises(evidence.Invalid):
                smoke.artifacts(root, "hardware", True)

    @mock.patch.object(smoke.sys, "platform", "linux")
    @mock.patch.object(smoke.platform, "machine", return_value="x86_64")
    def test_oracle_commands_are_explicit_and_errors_cannot_be_availability(self, machine):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            capture, oracle = root / "capture", root / "oracle"
            capture.mkdir()
            oracle.mkdir()
            smoke_directory(capture)
            for name in ("ffmpeg", "ffprobe"):
                (oracle / name).write_bytes(name.encode())
            decoder = root / "decoder"
            decoder.write_bytes(b"fake independent decoder")
            commands = []

            def fake(command, temporary, timeout, stdout="stdout", stderr="stderr", **kwargs):
                commands.append(command)
                temporary = Path(temporary)
                (temporary / stderr).write_bytes(b"")
                if str(command[0]).endswith("ffmpeg"):
                    (temporary / "decoded.yuv").write_bytes(smoke.source())
                elif str(command[0]).endswith("ffprobe"):
                    evidence.write(temporary / stdout, {
                        "streams": [{"codec_name": "h264", "width": 64, "height": 64,
                                     "pix_fmt": "yuv420p", "color_range": "tv", "color_space": "bt709",
                                     "color_transfer": "bt709", "color_primaries": "bt709"}],
                        "frames": [{"media_type": "video", "key_frame": int(n == 0),
                                    "width": 64, "height": 64, "pix_fmt": "yuv420p"} for n in range(4)]})
                elif str(command[0]).endswith("decoder"):
                    (temporary / "coreav.yuv").write_bytes(smoke.source())
                    evidence.write(temporary / "coreav.json", {"schema": "coreav.videotoolbox-decode/1",
                                   "width": 64, "height": 64, "frames": 4, "warnings": 0})
                return 1

            with mock.patch.object(smoke, "run", side_effect=fake):
                result = smoke.verify_capture(capture, oracle, "hardware", decoder, True)
            self.assertEqual(result["status"], "verified")
            self.assertTrue(any("install-oracle-ffmpeg.sh" in str(arg) for command in commands for arg in command))
            self.assertTrue(any("--verify-bin" in command for command in commands))
            decode = next(command for command in commands if str(command[0]).endswith("ffmpeg"))
            self.assertLess(decode.index("-c:v"), decode.index("-i"))
            self.assertEqual(decode[decode.index("-c:v") + 1], "h264")
            self.assertIn("-xerror", decode)
            self.assertNotIn("-c", decode)
            for error in (evidence.Invalid("timeout"), evidence.Invalid("child_failure"), OSError("secret-path")):
                with mock.patch.object(smoke, "run", side_effect=error), self.assertRaises(type(error)):
                    smoke.verify_capture(capture, oracle, "hardware", decoder)
            record = evidence.load(capture / "capture.json")
            record["files"]["video.h264"]["sha256"] = h("changed")
            evidence.write(capture / "capture.json", record)
            with mock.patch.object(smoke, "run") as execute, self.assertRaises(evidence.Invalid):
                smoke.verify_capture(capture, oracle, "hardware", decoder)
            execute.assert_not_called()


class Registration(unittest.TestCase):
    def test_first_ten_are_selected_from_the_complete_frozen_eligible_pool(self):
        reg = registered()
        candidates = [{"sha256": source["id"], "width": source["width"], "height": source["height"],
                       "frames": source["frames"], "rate": source["rate"], "pixel_format": "yuv420p",
                       "sdr": True, "partition": "held_out"} for source in reg["inputs"]]
        record = {"schema": "coreav.videotoolbox-eligibility/1", "candidates": candidates}
        registration.eligible_roster(record, reg)
        candidates[0]["partition"] = "training"
        with self.assertRaises(evidence.Invalid):
            registration.eligible_roster(record, reg)
        candidates[0]["partition"] = "held_out"
        candidates.append(copy.deepcopy(candidates[0]))
        with self.assertRaises(evidence.Invalid):
            registration.eligible_roster(record, reg)

    def test_complete_roster_and_fixed_settings(self):
        record = registered()
        registration.validate(record)
        mutations = [lambda r: r["inputs"].pop(), lambda r: r["inputs"].reverse(),
                     lambda r: r["inputs"].__setitem__(1, copy.deepcopy(r["inputs"][0])),
                     lambda r: r["inputs"][0].update(frames=239),
                     lambda r: r["inputs"][0].update(rate={"num": 1, "den": 1}),
                     lambda r: r["inputs"][0].update(raw_file="../raw.yuv"),
                     lambda r: r["inputs"][0]["source_frame_sha256"].pop(),
                     lambda r: r["host"].update(cpu="Apple M4"),
                     lambda r: r["host"].update(virtual=True),
                     lambda r: r["host"].update(os_version="other"),
                     lambda r: r["settings"].update(pairs=4),
                     lambda r: r["commands"]["hardware"]["coreav"].append("--different"),
                     lambda r: r["metric_tools"].pop("model"),
                     lambda r: r.update(candidate=""), lambda r: r.update(extra=True)]
        for mutate in mutations:
            changed = copy.deepcopy(record)
            mutate(changed)
            with self.assertRaises(evidence.Invalid):
                registration.validate(changed)

    def test_pr_and_wrong_platform_refuse_before_any_record_or_input_read(self):
        for event in ("pull_request", "pull_request_target", "merge_group"):
            with mock.patch.dict(os.environ, {"GITHUB_EVENT_NAME": event}, clear=True), \
                    mock.patch.object(registration, "load") as load, \
                    mock.patch.object(registration, "git") as git, self.assertRaises(evidence.Invalid):
                registration.preflight(Path("missing"))
            load.assert_not_called()
            git.assert_not_called()
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(registration.sys, "platform", "linux"), \
                mock.patch.object(registration, "load") as load, self.assertRaises(evidence.Invalid):
            registration.preflight(Path("missing"))
        load.assert_not_called()

    def test_commands_keep_rationals_source_geometry_and_distinct_modes(self):
        reg = registered()
        source = reg["inputs"][0]
        source["rate"] = {"num": 30000, "den": 1001}
        command = timing.commands(reg, source, "hardware", Path("/private"), Path("/ram/video"), Path("/ram/report"))
        self.assertEqual(command["coreav"][command["coreav"].index("--rate-num") + 1], "30000")
        self.assertEqual(command["coreav"][command["coreav"].index("--rate-den") + 1], "1001")
        self.assertEqual(command["ffmpeg"][command["ffmpeg"].index("-g") + 1], "59")
        self.assertNotIn("-vf", command["ffmpeg"])
        self.assertNotIn("-r", command["ffmpeg"])


class Judgments(unittest.TestCase):
    def test_conformance_is_complete_and_bound_to_frame_identity(self):
        reg = registered()
        source = reg["inputs"][0]
        good = proof(reg, source)
        judging.conformance(good, source, "hardware", "coreav", h("output"), reg)
        mutations = [lambda r: r.update(output_sha256=h("wrong")),
                     lambda r: r.update(raw_sha256=h("wrong")),
                     lambda r: r.update(hardware="unknown"),
                     lambda r: r.update(width=2), lambda r: r.update(reordering=True),
                     lambda r: r.update(audio_applicability="verified"),
                     lambda r: r.update(container_bytes=100),
                     lambda r: r.update(sample_description_applicability="verified"),
                     lambda r: r.update(payload_bytes=1),
                     lambda r: r["frames"][0].update(pts={"value": 1, "scale": 30}),
                     lambda r: r["frames"][1].update(duration={"value": 1, "scale": 29}),
                     lambda r: r["frames"][1].update(source_sha256=h("swapped")),
                     lambda r: r["frames"].reverse(), lambda r: r["frames"].pop(),
                     lambda r: r["decoders"]["ffmpeg"].update(status="timeout"),
                     lambda r: r["decoders"]["coreav"]["frame_sha256"].__setitem__(0, h("wrong")),
                     lambda r: r["metrics"].update(alignment="searched"),
                     lambda r: r["metrics"].update(psnr_planes="luma"),
                     lambda r: r["metrics"]["frames"][0].update(vmaf=float("nan")),
                     lambda r: r["metrics"]["frames"].pop(), lambda r: r.update(extra=True)]
        for mutate in mutations:
            changed = copy.deepcopy(good)
            mutate(changed)
            with self.assertRaises(evidence.Invalid):
                judging.conformance(changed, source, "hardware", "coreav", h("output"), reg)

    def test_quality_denominator_medians_and_fixture_floors(self):
        good = {"vmaf": 0, "psnr": 0, "ssim": 0}
        bad = {"vmaf": -2.01, "psnr": 0, "ssim": 0}
        grouped = {height: [good] * 10 for height, _ in registration.CLASSES}
        judging.quality(grouped, [good])
        for rows, fixtures in (({**grouped, 360: [good] * 9 + [bad]}, [good]), (grouped, [bad]),
                               ({**grouped, 360: [{"vmaf": -1.01, "psnr": 0, "ssim": 0}] * 10}, [good]),
                               ({}, [good]), (grouped, []),
                               ({**grouped, 360: [good] * 9}, [good])):
            with self.assertRaises(evidence.Invalid):
                judging.quality(rows, fixtures)

    def test_host_gates_and_faults(self):
        reg = registered()
        good = host_gate(reg)
        judging.gate(good, reg, "before")
        for mutate in (lambda r: r["aa"][0].update(a_ns=200),
                       lambda r: [p.update(a_ns=104) for p in r["aa"]],
                       lambda r: [p.update(a_ns=90) for p in r["null"]],
                       lambda r: r["observations"].update(swap_bytes=1),
                       lambda r: r["observations"].update(throttled=True),
                       lambda r: r["observations"].update(competing_encoder=True),
                       lambda r: r["host"].update(os_build="changed"),
                       lambda r: r.update(phase="after")):
            changed = copy.deepcopy(good)
            mutate(changed)
            if changed["aa"][0]["a_ns"] == 200 and changed["aa"][1]["a_ns"] == 100:
                judging.gate(changed, reg, "before")
            else:
                with self.assertRaises(evidence.Invalid):
                    judging.gate(changed, reg, "before")

    def test_timing_medians_classes_ci_and_no_hidden_failed_calls(self):
        reg = registered()
        records = calls(reg)
        result = judging.timings(records, reg)["hardware"]
        self.assertEqual(result["overall"]["median"], 0.9)
        self.assertEqual(result["overall"]["ci95"], [0.9, 0.9])
        self.assertEqual(result["overall"]["share_faster"], 1)
        self.assertTrue(result["overall"]["faster"])
        for change in (lambda r: r.pop(), lambda r: r.reverse(),
                       lambda r: r[0].update(status="timeout"),
                       lambda r: r[0].update(status="child_failure"),
                       lambda r: r[0].update(elapsed_ns=0),
                       lambda r: r[0].update(iteration=True),
                       lambda r: r[0].update(cache_before=None),
                       lambda r: r[0]["cache_after"].update(file_identity_sha256=h("changed-file")),
                       lambda r: r[0]["host_after"].update(encoder_threads=1),
                       lambda r: r[0]["host_after"].update(thermal_warning=True)):
            changed = copy.deepcopy(records)
            change(changed)
            with self.assertRaises(evidence.Invalid):
                judging.timings(changed, reg)
        for record in records:
            if record["side"] == "coreav" and record["input_id"] in {
                    source["id"] for source in reg["inputs"] if source["class"] == 360}:
                record.update(elapsed_ns=101, ended_ns=111)
        result = judging.timings(records, reg)["hardware"]
        self.assertTrue(result["overall"]["bar_pass"])
        self.assertFalse(result["classes"]["360"]["bar_pass"])
        self.assertFalse(judging.summary([1.0] * 10)["faster"])


class Retention(unittest.TestCase):
    def test_infrastructure_abort_after_a_success_retains_both_outcomes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reg = registered()
            reg["inputs"] = reg["inputs"][:1]
            source = root / reg["inputs"][0]["raw_file"]
            source.parent.mkdir()
            source.write_bytes(b"disjoint fake source; never a roster input")
            evidence.write(root / "registration.json", {})
            reg_hash = evidence.file_hash(root / "registration.json")
            evidence.write(root / "availability.json", {"registration_sha256": reg_hash})
            evidence.write(root / "before.json", {})
            args = SimpleNamespace(report=root / "report.json", root=root, registration=root / "registration.json",
                                   availability=root / "availability.json", before_gate=root / "before.json",
                                   artifacts=root / "artifacts", ram_output=root)
            count = 0

            def fake(command, location, timeout, **kwargs):
                nonlocal count
                count += 1
                if count == 2:
                    raise evidence.Invalid("process_infrastructure")
                Path(command[command.index("--output") + 1]).write_bytes(b"fake output")
                evidence.write(Path(command[command.index("--report") + 1]), {})
                return 1

            with mock.patch.object(timing, "preflight", return_value=reg), \
                    mock.patch.object(timing, "bound_gate", return_value=({"ended_ns": time.time_ns() - 1000000}, 1)), \
                    mock.patch.object(timing, "availability_file", return_value={"registration_sha256": reg_hash}), \
                    mock.patch.object(timing, "ram_volume"), \
                    mock.patch.object(timing, "input_hashes"), \
                    mock.patch.object(timing, "warm"), \
                    mock.patch.object(timing.cache, "preflight"), \
                    mock.patch.object(timing.cache, "observe", side_effect=lambda reg, root, path, item:
                                      cache_fact(item, time.time_ns(), time.time_ns())), \
                    mock.patch.object(timing, "host_state", return_value=observation()), \
                    mock.patch.object(timing, "run", side_effect=fake), self.assertRaises(evidence.Invalid):
                timing.execute(args)
            result = evidence.load(args.report)
            self.assertEqual(result["status"], "aborted")
            self.assertEqual([record["status"] for record in result["records"]],
                             ["success", "process_infrastructure"])
            self.assertTrue((args.artifacts / "call-0000" / "video.h264").exists())

    def test_infrastructure_abort_retains_nonpassing_report_without_exception_text(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args = SimpleNamespace(report=root / "report.json", root=root, registration=root / "registration.json")
            with mock.patch.object(timing, "preflight", side_effect=OSError("credential-marker /private/host")), \
                    self.assertRaises(OSError):
                timing.execute(args)
            text = args.report.read_text()
            self.assertNotIn("credential-marker", text)
            self.assertNotIn("/private/host", text)
            report = evidence.loads(text)
            self.assertEqual(report["status"], "aborted")
            self.assertIsNotNone(report["ended_ns"])

    @mock.patch.dict(os.environ, {}, clear=True)
    def test_missing_evidence_and_aborted_capture_cannot_become_acceptance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reg = registered()
            reg["metric_tools"]["evidence_producer"] = evidence.file_hash(Path(timing.__file__).with_name("producer.py"))
            evidence.write(root / "build.json", {"observability": {}})
            evidence.write(root / "registration.json", reg)
            captured = {"schema": "coreav.videotoolbox-timing/1", "status": "aborted",
                        "scope": "raw-driver-elementary-only", "diagnostic": "timeout", "records": calls(reg),
                        "registration_sha256": evidence.file_hash(root / "registration.json"),
                        "before_gate_sha256": h("before"), "availability_sha256": h("available"),
                        "started_ns": 3, "ended_ns": 4}
            evidence.write(root / "capture.json", captured)
            args = SimpleNamespace(root=root, registration=root / "registration.json", capture=root / "capture.json",
                                   evidence=root / "missing", artifacts=root / "artifacts", report=root / "report.json")
            with mock.patch.object(timing, "source_registration", return_value=reg), self.assertRaises(evidence.Invalid):
                timing.replay(args)
            self.assertEqual(evidence.load(args.report)["status"], "failed")
            captured["status"] = "awaiting_evidence"
            evidence.write(args.capture, captured)
            with mock.patch.object(timing, "source_registration", return_value=reg), self.assertRaises(OSError):
                timing.replay(args)
            self.assertEqual(evidence.load(args.report)["status"], "failed")


@unittest.skipUnless(sys.platform == "darwin", "real-child lifecycle checks belong on hosted macOS")
class KernelExit(unittest.TestCase):
    def test_exit_is_not_pipe_eof_and_does_not_sleep_poll(self):
        script = ("import os,time\n"
                  "if os.fork() == 0:\n"
                  " time.sleep(20)\n"
                  "else:\n"
                  " os._exit(0)\n")
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(process.time, "sleep", side_effect=AssertionError("polling")):
            before = time.monotonic()
            elapsed = process.run([sys.executable, "-c", script], directory, 10)
            self.assertLess(time.monotonic() - before, 10)
            self.assertGreater(elapsed, 0)

    def test_timeout_kills_sigterm_ignoring_group_and_reaps_child(self):
        with tempfile.TemporaryDirectory() as directory, select.kqueue() as queue:
            original = process.subprocess.Popen
            children = []
            ready_read, ready_write = os.pipe()
            script = ("import os,signal,time\n"
                      "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
                      "pid=os.fork()\n"
                      "if pid == 0:\n"
                      " os.close(1); os.close(2); time.sleep(60)\n"
                      "else:\n"
                      f" os.write({ready_write},str(pid).encode()); os.close({ready_write}); time.sleep(60)\n")

            def launch(*args, **kwargs):
                kwargs["pass_fds"] = (ready_write,)
                child = original(*args, **kwargs)
                children.append(child)
                self.assertTrue(select.select([ready_read], [], [], 10)[0])
                descendant = int(os.read(ready_read, 128))
                queue.control([select.kevent(descendant, filter=select.KQ_FILTER_PROC,
                                              flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                                              fflags=select.KQ_NOTE_EXIT)], 0, 0)
                return child

            try:
                with mock.patch.object(process.subprocess, "Popen", side_effect=launch), \
                        self.assertRaisesRegex(evidence.Invalid, "timeout"):
                    process.run([sys.executable, "-c", script], directory, 5)
                self.assertEqual(len(children), 1)
                self.assertIsNotNone(children[0].returncode)
                self.assertTrue(queue.control([], 1, 10))
                with self.assertRaises(ChildProcessError):
                    os.waitpid(children[0].pid, os.WNOHANG)
            finally:
                os.close(ready_read)
                os.close(ready_write)

    def test_already_exited_child_registration_race(self):
        with tempfile.TemporaryDirectory() as directory:
            for index in range(20):
                process.run([sys.executable, "-c", "pass"], directory, 10,
                            stdout=f"out{index}", stderr=f"err{index}")


class StaticContract(unittest.TestCase):
    def test_timer_has_no_polling_or_pipe_capture(self):
        tree = ast.parse(Path(process.__file__).read_text())
        calls = [node for node in ast.walk(tree) if isinstance(node, ast.Call)]
        self.assertFalse(any(isinstance(node.func, ast.Attribute) and node.func.attr in
                             ("sleep", "poll", "communicate") for node in calls))
        self.assertFalse(any(isinstance(node, ast.Attribute) and node.attr == "PIPE"
                             for node in ast.walk(tree)))


if __name__ == "__main__":
    unittest.main()
