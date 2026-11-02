"""Disjoint fake-command records; no native encoder, oracle or judged input."""

import copy
import math
import os
from pathlib import Path
import plistlib
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import evidence
import judging
import observations
import cache_residency as cache
import producer
import registration
import smoke
import timing
from test_tools import h, native, observation, proof, registered

SPS = b"\x67\x64\x00\x1f\x80"
PPS = b"\x68\x80"
PARAMETERS = b"\0\0\0\1" + SPS + b"\0\0\0\1" + PPS
VIDEO = PARAMETERS + b"".join(b"\0\0\0\1" + bytes([0x65 if n == 0 else 0x41, 0x80]) for n in range(4))
AVCC = list(bytes([1, 100, 0, 31, 255, 225]) + len(SPS).to_bytes(2, "big") + SPS + b"\1" + len(PPS).to_bytes(2, "big") + PPS)


def spec():
    return {"schema": "coreav.videotoolbox-observability/1",
            "fields": {name: {"prefix": f"observed-{name}=", "suffix": ""} for name in observations.FIELDS}}


def observed(item, cell="hardware", bitrate=1000000):
    return {"encoder_id": "actual.fake.encoder", "hardware": cell, "profile": "high", "reordering": False,
            "realtime": False, "bitrate": bitrate, "width": item["width"], "height": item["height"],
            "fps_num": item["rate"]["num"], "fps_den": item["rate"]["den"],
            "keyframe_frames": 2 * item["rate"]["num"] // item["rate"]["den"], "keyframe_seconds": 2,
            "pixel_format": "yuv420p", "range": "limited", "primaries": "bt709", "transfer": "bt709",
            "matrix": "bt709", "submitted": item["frames"], "status": "encoded"}


def trace_bytes(values):
    return ("\n".join(f"observed-{name}={str(value).lower() if type(value) is bool else value}"
                      for name, value in values.items()) + "\n").encode()


def probe_document(item):
    extradata = "\n" + "\n".join(f"{at:08x}: {PARAMETERS[at:at + 16].hex()}  text"
                                   for at in range(0, len(PARAMETERS), 16)) + "\n"
    return {"streams": [{"codec_name": "h264", "profile": "High", "width": item["width"], "height": item["height"],
                         "pix_fmt": "yuv420p", "color_range": "tv", "color_space": "bt709", "color_transfer": "bt709",
                         "color_primaries": "bt709", "time_base": "1/30", "r_frame_rate": "30/1", "extradata": extradata,
                         "extradata_size": len(PARAMETERS)}],
            "frames": [{"media_type": "video", "key_frame": int(n == 0), "width": item["width"],
                        "height": item["height"], "pix_fmt": "yuv420p", "pict_type": "I" if n == 0 else "P",
                        "duration": 1} for n in range(item["frames"])]}


def native_report(item):
    frames = native()["frames"]
    return {"schema": "coreav.videotoolbox-encode/1", "status": "encoded", "requested_mode": "hardware",
            "width": item["width"], "height": item["height"], "fps_num": 30, "fps_den": 1,
            "bitrate": 1000000, "submitted": item["frames"], "access_unit_bytes": len(VIDEO) - len(PARAMETERS),
            "encoder_id": "actual.fake.encoder", "hardware": "hardware", "frames": frames, "avcc": AVCC,
            "settings": {"profile": "high", "reordering": False, "realtime": False, "keyframe_seconds": 2,
                         "keyframe_frames": 60, "pixel_format": "i420", "native_pixel_format": "420v",
                         "color_range": "limited", "primaries": "bt709", "transfer": "bt709", "matrix": "bt709"},
            "drain_complete": True}


class FakeCommands:
    def __init__(self, item, raw, decoded=None):
        self.item, self.raw = item, raw
        self.decoded = bytes(b + 1 for b in raw) if decoded is None else decoded
        self.commands = []
        self.fail = None
        self.drop_observation = None
        self.unavailable_software = False

    def __call__(self, command, directory, timeout, stdout="stdout", stderr="stderr", **kwargs):
        self.commands.append(command)
        directory = Path(directory)
        (directory / stdout).write_bytes(b"")
        (directory / stderr).write_bytes(b"")
        if self.fail:
            raise evidence.Invalid(self.fail)
        executable = Path(command[0]).name
        if executable == "ffprobe":
            evidence.write(directory / stdout, probe_document(self.item))
        elif executable == "decoder" and command[1] == "decode":
            Path(command[command.index("--output") + 1]).write_bytes(self.decoded)
            evidence.write(Path(command[command.index("--report") + 1]),
                           {"schema": "coreav.videotoolbox-decode/1", "width": self.item["width"],
                            "height": self.item["height"], "frames": self.item["frames"], "warnings": 0})
        elif executable == "decoder" and command[1] == "compare":
            size = self.item["width"] * self.item["height"] * 3 // 2
            evidence.write(Path(command[command.index("--report") + 1]), {
                "schema": "coreav.videotoolbox-error/1", "width": self.item["width"], "height": self.item["height"],
                "frames": [{"id": n, "samples": size,
                            "squared_error": sum((a - b) ** 2 for a, b in
                                zip(self.raw[n * size:(n + 1) * size], self.decoded[n * size:(n + 1) * size]))}
                           for n in range(self.item["frames"])]})
        elif executable == "ffmpeg" and "-filter_complex" in command:
            self.metric_files(directory)
        elif executable == "ffmpeg" and "-c:v" in command and command[command.index("-c:v") + 1] == "h264":
            Path(command[-1]).write_bytes(self.decoded)
        elif executable == "ffmpeg":
            cell = "software" if command[command.index("-require_sw") + 1] == "1" else "hardware"
            if self.unavailable_software and cell == "software":
                (directory / stderr).write_bytes(trace_bytes({"status": "no_encoder", "submitted": 0}))
                raise evidence.Invalid("child_failure")
            Path(command[-1]).write_bytes(VIDEO)
            Path(command[command.index("-stats_enc_post") + 1]).write_bytes(b"".join(
                f"{n} 1/30 {n} 6\n".encode() for n in range(self.item["frames"])))
            values = observed(self.item, cell, int(command[command.index("-b:v") + 1]))
            if self.drop_observation:
                values.pop(self.drop_observation)
            (directory / stderr).write_bytes(trace_bytes(values) + b"input /secret/home/file.yuv\n")
        else:
            raise AssertionError("unexpected fake command")
        return 1

    def metric_files(self, directory):
        size = self.item["width"] * self.item["height"] * 3 // 2
        psnr = ["psnr_log_version:2 fields:n,mse_avg,mse_y,mse_u,mse_v,psnr_avg,psnr_y,psnr_u,psnr_v"]
        ssim = []
        for n in range(self.item["frames"]):
            error = sum((a - b) ** 2 for a, b in zip(self.raw[n * size:(n + 1) * size],
                                                     self.decoded[n * size:(n + 1) * size]))
            mse = error / size
            score = f"{10 * math.log10(255**2 / mse):.2f}" if error else "inf"
            psnr.append(f"n:{n + 1} mse_avg:{mse:.2f} mse_y:{mse:.2f} mse_u:{mse:.2f} mse_v:{mse:.2f} "
                        f"psnr_avg:{score} psnr_y:{score} psnr_u:{score} psnr_v:{score}")
            ssim.append(f"n:{n + 1} Y:0.990000 U:0.990000 V:0.990000 All:0.990000 (20.000000)")
        (directory / "psnr.log").write_text("\n".join(psnr) + "\n")
        (directory / "ssim.log").write_text("\n".join(ssim) + "\n")
        evidence.write(directory / "vmaf.json", {"version": "fake", "frames": [
            {"frameNum": n, "metrics": {"vmaf": 95.0}} for n in range(self.item["frames"])]})


class Observability(unittest.TestCase):
    def test_nondividing_demux_clock_retains_observed_duration(self):
        for rate, ticks in ((144, 8333), (225, 5333)):
            item = producer.unit_item()
            item["rate"] = {"num": rate, "den": 1}
            item["source_tick"] = {"num": 1, "den": 90000}
            value = probe_document(item)
            value["streams"][0].update(time_base="1/1200000", r_frame_rate=f"{rate}/1")
            for frame in value["frames"]:
                frame["duration"] = ticks
            _, _, observed_clock = producer.probe_facts(value, item)
            self.assertEqual(observed_clock[2][0].numerator, ticks // math.gcd(ticks, 1200000))
            value["frames"][0]["duration"] = ticks + 8
            with self.assertRaises(evidence.Invalid):
                producer.probe_facts(value, item)
            value["frames"][0]["duration"] = ticks
            value["streams"][0]["r_frame_rate"] = "30/1"
            with self.assertRaises(evidence.Invalid):
                producer.probe_facts(value, item)

    def test_only_explicit_identity_and_controls_are_accepted(self):
        item = producer.unit_item()
        good = observed(item)
        parsed, retained = observations.trace(trace_bytes(good) + b"input /secret/path\n", spec())
        observations.effective(parsed, item, "hardware", 1000000)
        self.assertNotIn(b"/secret", retained)
        for field in ("hardware", "encoder_id", "reordering", "bitrate", "submitted", "status"):
            changed = dict(good)
            changed.pop(field)
            with self.assertRaises(evidence.Invalid):
                observations.trace(trace_bytes(changed), spec())
        good["hardware"] = "software"
        parsed, _ = observations.trace(trace_bytes(good), spec())
        with self.assertRaises(evidence.Invalid):
            observations.effective(parsed, item, "hardware", 1000000)
        with self.assertRaises(evidence.Invalid):
            observations.trace(b"-require_sw 0\n", spec())
        with self.assertRaises(evidence.Invalid):
            observations.trace(b"observed-status=no_encoder\n", spec())

    def test_exact_rational_clocks_and_complete_packet_coverage(self):
        item = producer.unit_item()
        good = b"".join(f"{n} 1/30 {n} 6\n".encode() for n in range(4))
        producer.packet_clock(good, item)
        producer.packet_clock(good.replace(b"1/30", b"1/29"), item)
        for changed in (good.replace(b"1/30", b"1/15"), good.replace(b"1 1/30 1", b"0 1/30 1"),
                        good.splitlines(keepends=True)[0], good + good[:13], good.replace(b"6", b"0")):
            with self.assertRaises(evidence.Invalid):
                producer.packet_clock(changed, item)


class RealEvidencePaths(unittest.TestCase):
    def test_resume_rebuilds_only_an_owned_incomplete_analysis_unit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source.yuv"
            source.write_bytes(smoke.source())
            model = root / "model.json"
            model.write_bytes(b'{"fake_model":true}')
            evidence.write(root / "registration.json", {})
            evidence.write(root / "timing.json", {})
            item, reg = producer.unit_item(), registered()
            reg["metric_tools"]["model"] = evidence.file_hash(model)
            args = SimpleNamespace(oracle_bin=root, coreav_decoder=root / "decoder", model=model, resume=False,
                                   registration=root / "registration.json", capture=root / "timing.json")
            directory = root / "capture"
            directory.mkdir()
            (directory / "video.h264").write_bytes(VIDEO)
            evidence.write(directory / "native.json", native_report(item))
            with mock.patch.object(producer, "run", side_effect=evidence.Invalid("process_infrastructure")), \
                    self.assertRaises(evidence.Invalid):
                producer.analyse(directory, source, item, "hardware", "coreav", reg, args, spec())
            self.assertEqual(evidence.load(directory / "analysis-owner.json")["status"], "incomplete")
            (directory / "analysis-owner.json.tmp").write_bytes(b'{"partial":')
            (directory / "analysis" / "unrelated.txt").write_bytes(b"keep")
            args.resume = True
            with mock.patch.object(producer, "run") as command, self.assertRaises(evidence.Invalid):
                producer.analyse(directory, source, item, "hardware", "coreav", reg, args, spec())
            command.assert_not_called()
            self.assertEqual((directory / "analysis" / "unrelated.txt").read_bytes(), b"keep")
            (directory / "analysis" / "unrelated.txt").unlink()
            fake = FakeCommands(item, smoke.source())
            with mock.patch.object(producer, "run", side_effect=fake):
                result = producer.analyse(directory, source, item, "hardware", "coreav", reg, args, spec())
            self.assertTrue((directory / "failure.json").exists())
            self.assertFalse((directory / "analysis-owner.json.tmp").exists())
            producer.verify_output(result, directory, source, item, "hardware", "coreav", reg, spec(), args)
            self.assertEqual(len(evidence.load(directory / "analysis-owner.json")["history"]), 2)
            other = root / "failed"
            other.mkdir()
            (other / "video.h264").write_bytes(VIDEO)
            evidence.write(other / "native.json", native_report(item))
            with mock.patch.object(producer, "run", side_effect=evidence.Invalid("child_crash")), self.assertRaises(evidence.Invalid):
                producer.analyse(other, source, item, "hardware", "coreav", reg, args, spec())
            with mock.patch.object(producer, "run") as command, self.assertRaises(evidence.Invalid):
                producer.analyse(other, source, item, "hardware", "coreav", reg, args, spec())
            command.assert_not_called()

    def test_fake_commands_produce_replayable_dual_decode_and_metric_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source.yuv"
            source.write_bytes(smoke.source())
            item = producer.unit_item()
            reg = registered()
            reg["metric_tools"]["evidence_producer"] = evidence.file_hash(Path(producer.__file__))
            model = root / "model.json"
            model.write_bytes(b'{"fake_model":true}')
            reg["metric_tools"]["model"] = evidence.file_hash(model)
            directory = root / "capture"
            directory.mkdir()
            (directory / "video.h264").write_bytes(VIDEO)
            evidence.write(directory / "native.json", native_report(item))
            evidence.write(root / "registration.json", {})
            evidence.write(root / "timing.json", {})
            args = SimpleNamespace(oracle_bin=root, coreav_decoder=root / "decoder", model=model, resume=False,
                                   registration=root / "registration.json", capture=root / "timing.json")
            fake = FakeCommands(item, smoke.source())
            with mock.patch.object(producer, "run", side_effect=fake):
                result = producer.analyse(directory, source, item, "hardware", "coreav", reg, args, spec())
            producer.verify_output(result, directory, source, item, "hardware", "coreav", reg, spec(), args)
            args.resume = True
            with mock.patch.object(producer, "run", side_effect=AssertionError("completed work reran")):
                self.assertEqual(producer.analyse(directory, source, item, "hardware", "coreav", reg, args, spec()), result)
            changed = copy.deepcopy(reg)
            changed["metric_tools"]["model"] = h("changed-model")
            with mock.patch.object(producer, "run") as command, self.assertRaises(evidence.Invalid):
                producer.analyse(directory, source, item, "hardware", "coreav", changed, args, spec())
            command.assert_not_called()
            self.assertEqual(result["metrics"]["frames"][0]["squared_error"], 6144)
            self.assertEqual(result["metrics"]["frames"][0]["samples"], 6144)
            self.assertEqual(result["decoders"]["coreav"]["frame_sha256"], result["decoders"]["ffmpeg"]["frame_sha256"])
            commands = fake.commands
            self.assertTrue(any("compare" in command for command in commands))
            graph = next(command[command.index("-filter_complex") + 1] for command in commands if "-filter_complex" in command)
            self.assertIn("libvmaf=model=path=model.json", graph)
            self.assertIn("psnr=stats_file=psnr.log", graph)
            self.assertIn("ssim=stats_file=ssim.log", graph)
            self.assertNotIn("scale=", graph)
            wrong = copy.deepcopy(result)
            wrong["encoder_id"] = "supplied.unobserved.id"
            with self.assertRaises(evidence.Invalid):
                producer.verify_output(wrong, directory, source, item, "hardware", "coreav", reg, spec(), args)
            (directory / "analysis" / "coreav.yuv").write_bytes(b"wrong")
            with self.assertRaises(evidence.Invalid):
                producer.verify_output(result, directory, source, item, "hardware", "coreav", reg, spec(), args)

    def test_warnings_crashes_timeouts_and_missing_metadata_are_not_success(self):
        item = producer.unit_item()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for diagnostic in ("child_crash", "timeout", "process_infrastructure"):
                directory = root / diagnostic
                directory.mkdir()
                fake = FakeCommands(item, smoke.source())
                fake.fail = diagnostic
                with mock.patch.object(producer, "run", side_effect=fake), self.assertRaises(evidence.Invalid):
                    producer.decode(root / "fake.h264", item, directory, root / "ffmpeg", root / "ffprobe", root / "decoder")
            value = {"schema": "coreav.videotoolbox-decode/1", "width": 64, "height": 64, "frames": 4, "warnings": 1}
            with self.assertRaises(evidence.Invalid):
                producer.decoder_report(value, item)

    def test_preflight_ledger_is_computed_from_both_api_outputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            reg = registered()
            for side, filename in (("coreav", "decoder"), ("ffmpeg", "ffmpeg")):
                (root / filename).write_bytes(filename.encode())
                reg["executables"][side]["file"] = filename
                reg["executables"][side]["sha256"] = evidence.file_hash(root / filename)
            (root / "ffprobe").write_bytes(b"ffprobe")
            evidence.write(root / "build.json", {"observability": spec(), "ffprobe_sha256": evidence.file_hash(root / "ffprobe")})
            evidence.write(root / "registration.json", {})
            reg["metric_tools"]["evidence_producer"] = evidence.file_hash(Path(producer.__file__))
            args = SimpleNamespace(root=root, registration=root / "registration.json", output=root / "preflight",
                                   ffprobe=root / "ffprobe")
            fake = FakeCommands(producer.unit_item(), smoke.source(), smoke.source())
            fake.unavailable_software = True

            def native_capture(exporter, directory, mode, runner):
                directory.mkdir()
                value = native(mode, mode == "software")
                evidence.write(directory / "manifest.json", value)
                names = ["manifest.json"]
                if mode == "hardware":
                    (directory / "source.yuv").write_bytes(smoke.source())
                    (directory / "video.h264").write_bytes(VIDEO)
                    names += ["source.yuv", "video.h264"]
                evidence.write(directory / "capture.json", {"schema": "coreav.videotoolbox-capture/1",
                    "requested_mode": mode, "exporter_sha256": reg["executables"]["coreav"]["sha256"],
                    "runner_image": runner, "os_build": reg["host"]["os_build"], "machine": "arm64",
                    "files": evidence.bind(directory, names, SMALL)})

            SMALL = producer.SMALL
            with mock.patch.object(producer, "preflight", return_value=reg), \
                    mock.patch.object(timing, "host_state", return_value=observation()), \
                    mock.patch.object(smoke, "capture", side_effect=native_capture), \
                    mock.patch.object(producer, "run", side_effect=fake):
                producer.capture_preflights(args)
            result = timing.availability_file(args.output / "availability.json", reg, root)
            self.assertEqual(result["cells"]["hardware"]["ffmpeg"]["status"], "encoded")
            self.assertEqual(result["cells"]["software"]["coreav"]["status"], "unavailable")
            proof_path = args.output / "preflight.hardware.ffmpeg.json"
            value = evidence.load(proof_path)
            value["result"]["pattern_identity"] = False
            evidence.write(proof_path, value)
            with self.assertRaises(evidence.Invalid):
                timing.availability_file(args.output / "availability.json", reg, root)
            fake.drop_observation = "hardware"
            args.output = root / "missing-observation"
            with mock.patch.object(producer, "preflight", return_value=reg), \
                    mock.patch.object(timing, "host_state", return_value=observation()), \
                    mock.patch.object(smoke, "capture", side_effect=native_capture), \
                    mock.patch.object(producer, "run", side_effect=fake), self.assertRaises(evidence.Invalid):
                producer.capture_preflights(args)
            self.assertFalse((args.output / "availability.json").exists())
            failure = evidence.load(args.output / "failure.json")
            self.assertEqual(failure["diagnostic"], "ffmpeg_observability_missing")
            self.assertNotIn("/secret", (args.output / "failure.json").read_text())
            self.assertFalse(any(path.name == "stderr" for path in args.output.rglob("*")))


class MetricDefinitions(unittest.TestCase):
    def test_clip_psnr_pools_integer_errors_and_handles_perfect_asymmetry(self):
        reg = registered()
        item = reg["inputs"][0]
        value = proof(reg, item)
        frames = value["metrics"]["frames"]
        for index, frame in enumerate(frames):
            squared = 0 if index % 2 == 0 else 9 * frame["samples"]
            frame.update(squared_error=squared, psnr_status="perfect" if squared == 0 else "finite",
                         psnr=None if squared == 0 else 10 * math.log10(255**2 / 9))
        actual = judging.conformance(value, item, "hardware", "coreav", h("output"), reg)
        self.assertAlmostEqual(actual["psnr"], 10 * math.log10(255**2 / 4.5))
        for frame in frames:
            frame.update(squared_error=0, psnr=None, psnr_status="perfect")
        perfect = judging.conformance(value, item, "hardware", "coreav", h("output"), reg)
        self.assertIsNone(perfect["psnr"])
        self.assertEqual(judging.deltas(perfect, perfect)["psnr"], 0)
        self.assertEqual(judging.deltas(perfect, actual)["psnr"], math.inf)
        self.assertEqual(judging.deltas(actual, perfect)["psnr"], -math.inf)
        with self.assertRaises(evidence.Invalid):
            judging.quality({height: [judging.deltas(actual, perfect)] * 10 for height, _ in registration.CLASSES},
                            [judging.deltas(perfect, perfect)])

    def test_raw_extent_is_bounded_above_sixteen_gibibytes(self):
        maximum = {"width": 3840, "height": 2160, "frames": 1800}
        self.assertGreater(registration.raw_size(maximum), 16 * 1024**3)
        self.assertLess(registration.raw_size(maximum), 32 * 1024**3)
        with self.assertRaises(evidence.Invalid):
            registration.raw_size({"width": 3840, "height": 2160, "frames": 36000})


class GuardedEntry(unittest.TestCase):
    def test_interrupted_sealed_revalidation_preserves_every_hashed_record(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "evidence"
            output.mkdir()
            reg = registered()
            binding = {"immutable": h("campaign")}
            evidence.write(output / "campaign.json", {"schema": "coreav.videotoolbox-campaign/1",
                                                     "binding": binding})
            evidence.write(output / "evidence.json", {"sealed": True})
            evidence.write(output / "producer-status.json", {
                "schema": "coreav.videotoolbox-producer-status/1", "status": "complete",
                "diagnostic": "evidence_captured", "completed": [], "binding": binding,
                "history": [], "storage": {"budget_bytes": 1}})
            original = {path.name: path.read_bytes() for path in output.iterdir()}
            args = SimpleNamespace(root=root, registration=root / "registration.json", output=output,
                                   resume=True, capture=root / "times.json", artifacts=root / "artifacts")
            with mock.patch.object(producer, "trusted_analysis_event"), \
                    mock.patch.object(producer, "validate", return_value=reg), \
                    mock.patch.object(producer, "campaign_binding", return_value=binding), \
                    mock.patch.object(producer, "analysis_preflight", return_value=reg), \
                    mock.patch.object(producer, "run", side_effect=AssertionError("sealed media reran")):
                evidence.write(args.registration, {})
                with mock.patch.object(timing, "replay", side_effect=KeyboardInterrupt), \
                        self.assertRaises(KeyboardInterrupt):
                    producer.build_bundle(args)
                self.assertEqual({name: (output / name).read_bytes() for name in original}, original)
                with mock.patch.object(timing, "replay", return_value=False) as replay:
                    producer.build_bundle(args)
                replay.assert_called_once()
                self.assertEqual({name: (output / name).read_bytes() for name in original}, original)
                self.assertFalse(any(path.name.endswith(".tmp") for path in output.iterdir()))

    def test_resume_copies_are_verified_and_partial_writes_do_not_follow_links(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, target = root / "source.json", root / "target.json"
            source.write_bytes(b'{"immutable":true}')
            producer.copy_immutable(source, target, False)
            producer.copy_immutable(source, target, True)
            target.write_bytes(b"changed")
            with self.assertRaises(evidence.Invalid):
                producer.copy_immutable(source, target, True)
            self.assertEqual(target.read_bytes(), b"changed")
            partial = root / "target.json.tmp"
            partial.write_bytes(b'{"partial":')
            producer.remove_partial_write(target)
            self.assertFalse(partial.exists())
            partial.symlink_to(source)
            with self.assertRaises((evidence.Invalid, OSError)):
                producer.remove_partial_write(target)
            self.assertEqual(source.read_bytes(), b'{"immutable":true}')

    def test_analysis_storage_budget_is_explicit_and_does_not_allocate_capacity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            artifacts, output = root / "artifacts", root / "evidence"
            artifacts.mkdir()
            output.mkdir()
            args = SimpleNamespace(storage_root=root, artifacts=artifacts, output=output, storage_budget_bytes=1)
            with mock.patch.object(producer.shutil, "disk_usage") as usage, self.assertRaises(evidence.Invalid):
                producer.capacity(args, registered())
            usage.assert_not_called()
            self.assertEqual(list(artifacts.iterdir()), [])
            self.assertEqual(list(output.iterdir()), [])

    def test_trusted_main_analysis_does_not_bypass_merged_source_validation(self):
        for event in ("workflow_dispatch", "push"):
            environment = {"GITHUB_ACTIONS": "true", "GITHUB_EVENT_NAME": event, "GITHUB_REF": "refs/heads/main",
                           "GITHUB_REPOSITORY": "coreav-dev/coreav"}
            with mock.patch.dict(os.environ, environment, clear=True):
                producer.trusted_analysis_event()
                with mock.patch.object(producer.sys, "platform", "linux"), \
                        mock.patch.object(producer, "source_registration", side_effect=evidence.Invalid("unmerged")), \
                        mock.patch.object(producer, "run") as command, self.assertRaises(evidence.Invalid):
                    producer.analysis_preflight(SimpleNamespace(registration=Path("reg"), root=Path("root")))
                command.assert_not_called()

    def test_cache_observer_requires_complete_real_counts_and_immutable_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "observer"
            binary.write_bytes(b"fake observer, never executed")
            source = root / "source.yuv"
            source.write_bytes(smoke.source())
            item = producer.unit_item()
            reg = registered()
            reg["cache_observer"].update(file="observer", sha256=evidence.file_hash(binary))
            output = ["Files: 1\nDirectories: 0\nResident Pages: 2/2 32K/32K 100%\n"]

            def observe_command(argv, directory, timeout):
                self.assertEqual(argv[1:-1], ["-v", "-m", "32G"])
                (Path(directory) / "stdout").write_text("input /private/source.yuv\n" + output[0])
                (Path(directory) / "stderr").write_bytes(b"")
                return 1

            with mock.patch.object(cache.os, "sysconf", return_value=16384), \
                    mock.patch.object(cache, "run", side_effect=observe_command):
                value = cache.observe(reg, root, source, item)
                cache.validate(value, reg, item)
                self.assertNotIn("/private", value["stdout"])
                output[0] = "Files: 1\nDirectories: 0\nResident Pages: 1/2 16K/32K 50%\n"
                partial = cache.observe(reg, root, source, item)
                with self.assertRaises(evidence.Invalid):
                    cache.validate(partial, reg, item)
                for malformed in ("Files: 1\nDirectories: 0\nResident Pages: 2/3 32K/48K 100%\n",
                                  "Resident Pages: 2/2 32K/32K 100%\n"):
                    output[0] = malformed
                    with self.assertRaises(evidence.Invalid):
                        cache.observe(reg, root, source, item)
                binary.write_bytes(b"changed observer")
                with self.assertRaises(evidence.Invalid):
                    cache.observe(reg, root, source, item)
            reg["cache_observer"] = None
            registration.validate(reg)
            with self.assertRaises(evidence.Invalid):
                cache.preflight(reg, root)

    def test_documented_vmtouch_byte_summaries_are_checked_not_ignored(self):
        prefix = "Files: 1\nDirectories: 0\nResident Pages: "
        for body, resident, total in (("348/1307  1M/5M  26.6%", 348, 1307),
                                      ("0/42116  0/164M  0%", 0, 42116),
                                      ("42116/42116  164M/164M  100%", 42116, 42116)):
            self.assertEqual(cache.counters(prefix + body + "\n", total * 4096, 4096)[:2], (resident, total))
        for body in ("348/1307  1M/500M  26.6%", "0/42116  zero/164M  0%",
                     "348/1307  1M/5M  100%", "42116/42116 164M 100%"):
            with self.assertRaises(evidence.Invalid):
                cache.counters(prefix + body + "\n", (42116 if "42116" in body else 1307) * 4096, 4096)

    def test_identical_m1_specs_cannot_hide_a_different_observed_machine(self):
        first, second = ("-".join(character * width for width in (8, 4, 4, 4, 12))
                         for character in "12")
        identifiers = [first]
        values = {"hw.model": "Macmini9,1", "machdep.cpu.brand_string": "Apple M1", "hw.physicalcpu": "8",
                  "hw.memsize": "8589934592", "kern.hv_vmm_present": "0"}

        def command(argv, directory, timeout, stdout="stdout", **kwargs):
            path = Path(directory) / stdout
            if argv[0].endswith("ioreg"):
                path.write_bytes(plistlib.dumps([{"IOPlatformUUID": identifiers[0],
                                                  "IOPlatformSerialNumber": "private-serial-marker"}]))
            elif argv[0].endswith("sysctl"):
                path.write_text(values[argv[-1]])
            else:
                path.write_text("15.7" if argv[-1] == "-productVersion" else "24G-test")
            return 1

        with mock.patch.object(registration.sys, "platform", "darwin"), \
                mock.patch.object(registration, "run", side_effect=command):
            a = registration.host_observation()
            identifiers[0] = second
            b = registration.host_observation()
        self.assertEqual({k: v for k, v in a.items() if k != "hardware_sha256"},
                         {k: v for k, v in b.items() if k != "hardware_sha256"})
        self.assertNotEqual(a["hardware_sha256"], b["hardware_sha256"])
        reg = registered()
        reg["host"]["hardware_sha256"] = a["hardware_sha256"]
        with tempfile.TemporaryDirectory() as temporary, \
                mock.patch.dict(os.environ, {"COREAV_MAC_ALLOCATION_SHA256": reg["host"]["allocation_sha256"]}, clear=True), \
                mock.patch.object(registration.sys, "platform", "darwin"), \
                mock.patch.object(registration, "source_registration", return_value=reg), \
                mock.patch.object(registration, "host_observation", return_value=b), \
                mock.patch.object(registration, "run") as command, self.assertRaises(evidence.Invalid):
            registration.preflight(Path(temporary) / "registration.json", Path(temporary))
        command.assert_not_called()
        with mock.patch.object(timing.sys, "platform", "darwin"), \
                mock.patch.object(timing, "hardware_fingerprint", return_value=b["hardware_sha256"]), \
                mock.patch.object(timing, "run") as child, self.assertRaises(evidence.Invalid):
            timing.host_state(reg)
        child.assert_not_called()
        encoded = str(a) + str(b)
        self.assertNotIn(first, encoded)
        self.assertNotIn(second, encoded)
        self.assertNotIn("private-serial-marker", encoded)

    def test_ci_cannot_open_judged_references_or_launch_analysis_commands(self):
        for event in ("pull_request", "pull_request_target", "merge_group", "workflow_dispatch"):
            with mock.patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "GITHUB_EVENT_NAME": event}, clear=True), \
                    mock.patch.object(producer, "source_registration") as source, \
                    mock.patch.object(producer, "run") as command, self.assertRaises(evidence.Invalid):
                producer.analysis_preflight(SimpleNamespace())
            source.assert_not_called()
            command.assert_not_called()


if __name__ == "__main__":
    unittest.main()
