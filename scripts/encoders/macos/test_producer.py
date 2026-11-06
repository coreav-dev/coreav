"""Disjoint fake-command records; no native encoder, oracle or judged input."""

import copy
import math
import os
from pathlib import Path
import plistlib
import sys
import tempfile
import time
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
from test_oracle import Oracle
from test_tools import h, hosted_refusal, native, observation, proof, registered

SPS = b"\x67\x64\x00\x1f\x80"
PPS = b"\x68\x80"
PARAMETERS = b"\0\0\0\1" + SPS + b"\0\0\0\1" + PPS
VIDEO = PARAMETERS + b"".join(b"\0\0\0\1" + bytes([0x65 if n == 0 else 0x41, 0x80]) for n in range(4))
AVCC = list(bytes([1, 100, 0, 31, 255, 225]) + len(SPS).to_bytes(2, "big") + SPS + b"\1" + len(PPS).to_bytes(2, "big") + PPS)


def spec():
    return {"schema": "coreav.videotoolbox-observability/2",
            "encoder_id": {"prefix": "Init the encoder: ", "suffix": ""},
            "refusal_fields": {name: {"prefix": f"observed-{name}=", "suffix": ""}
                               for name in ("status", "submitted")}}


def observed(item, cell="hardware", bitrate=1000000):
    return {"encoder_id": "actual.fake.encoder"}


def trace_bytes(values):
    return ("\n".join("[h264_videotoolbox @ 0x1234] " +
                      (f"Init the encoder: {value}" if name == "encoder_id" else f"observed-{name}={value}")
                      for name, value in values.items()) + "\n").encode()


def probe_document(item):
    extradata = "\n" + "\n".join(f"{at:08x}: {PARAMETERS[at:at + 16].hex()}  text"
                                   for at in range(0, len(PARAMETERS), 16)) + "\n"
    return {"streams": [{"codec_name": "h264", "profile": "High", "level": 31, "has_b_frames": 0,
                         "width": item["width"], "height": item["height"],
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
                         "expected_frame_rate_hint": registration.cadence_hint(item),
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
        elif executable == "decoder" and command[1] == "identify":
            evidence.write(Path(command[command.index("--report") + 1]), {
                "schema": "coreav.videotoolbox-encoder-identity/1",
                "encoder_id": command[command.index("--encoder-id") + 1], "hardware": "hardware"})
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
                f"{n} 1/30 {n} {6 + (len(PARAMETERS) if n == 0 else 0)}\n".encode()
                for n in range(self.item["frames"])))
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
    def test_encoder_ids_are_bounded_opaque_utf8_not_path_tokens(self):
        for identifier in ("opaque encoder / Main (é)", "a" * 4096, "é" * 2048):
            parsed, _ = observations.trace(trace_bytes({"encoder_id": identifier}), spec())
            self.assertEqual(parsed["encoder_id"], identifier)
        for identifier in ("", "bad\0id", "bad\nid", "bad\rid", "a" * 4097, "é" * 2049, "\ud800"):
            with self.assertRaises(evidence.Invalid):
                observations.encoder_identifier(identifier)

    def test_unobserved_refusal_fields_never_establish_unavailability(self):
        value = spec()
        value["refusal_fields"] = None
        good = b"[h264_videotoolbox @ 0x1234] Init the encoder: actual.encoder\n"
        observed, retained = observations.trace(good, value)
        self.assertEqual(observed, {"encoder_id": "actual.encoder"})
        self.assertEqual(retained, good)
        for data in (b"", good, b"[h264_videotoolbox @ 1234] status=no_encoder submitted=0\n"):
            with self.assertRaises(evidence.Invalid):
                observations.trace(data, value, failed=True)

    def test_query_binds_the_actual_id_trace_output_binary_host_and_process(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = root / "capture"
            directory.mkdir()
            reg = registered()
            item = producer.unit_item()
            for side, name in (("coreav", "decoder"), ("ffmpeg", "ffmpeg")):
                (root / name).write_bytes(name.encode())
                reg["executables"][side].update(file=name, sha256=evidence.file_hash(root / name))
            command = timing.commands(reg, {"raw_file": "unused", **item}, "hardware", root,
                                      directory / "video.h264", directory / "packets.stats")["ffmpeg"]
            command[command.index("-i") + 1] = str(directory / "source.yuv")
            command[command.index("-b:v") + 1] = "2000000"
            fake = FakeCommands(item, smoke.source())
            fake(command, directory, 120)
            ended = time.time_ns()
            with mock.patch.object(observations, "host_observation", return_value=reg["host"]), \
                    mock.patch.dict(os.environ, {"COREAV_MAC_ALLOCATION_SHA256": reg["host"]["allocation_sha256"]}), \
                    mock.patch.object(observations, "run", side_effect=fake):
                result = observations.capture_identity(directory, reg, root, spec(), ended, command, item, "hardware")
            self.assertEqual(result["hardware"], "hardware")
            queries = [call for call in fake.commands if "identify" in call]
            self.assertEqual(len(queries), 1)
            self.assertEqual(queries[0][2:4], ["--encoder-id", "actual.fake.encoder"])
            value = evidence.load(directory / "identity-query.json")
            self.assertGreaterEqual(value["process"]["started_ns"], ended)
            self.assertEqual(value["trace_sha256"], evidence.file_hash(directory / "stderr"))
            original = {name: (directory / name).read_bytes() for name in observations.BASELINE_FILES}
            mutations = [
                ("identity.json", lambda r: r.pop("hardware")),
                ("identity.json", lambda r: r.update(hardware=None)),
                ("identity.json", lambda r: r.update(hardware=True)),
                ("identity.json", lambda r: r.update(hardware="unknown")),
                ("identity.json", lambda r: r.update(hardware="software")),
                ("identity.json", lambda r: r.update(encoder_id="unmatched.encoder")),
                ("identity.json", lambda r: r.update(extra=True)),
                ("identity-query.json", lambda r: r.update(binary_sha256=h("other-binary"))),
                ("identity-query.json", lambda r: r["host"].update(hardware_sha256=h("other-host"))),
                ("identity-query.json", lambda r: r["host"].update(os_build="other-build")),
                ("identity-query.json", lambda r: r["host"].update(allocation_sha256=h("other-allocation"))),
                ("identity-query.json", lambda r: r.update(trace_sha256=h("other-trace"))),
                ("identity-query.json", lambda r: r.update(output_sha256=h("other-output"))),
                ("identity-query.json", lambda r: r["process"].update(started_ns=ended - 1)),
                ("identity-query.json", lambda r: r["process"].update(status="child_failure", exit_code=1)),
                ("identity-query.json", lambda r: r["process"].pop("report_sha256")),
                ("identity-query.json", lambda r: r["process"]["arguments"].__setitem__(3, "other.id"))]
            for filename, mutate in mutations:
                changed = evidence.loads(original[filename])
                mutate(changed)
                evidence.write(directory / filename, changed)
                if filename == "identity.json":
                    query = evidence.loads(original["identity-query.json"])
                    query["process"]["report_sha256"] = evidence.file_hash(directory / filename)
                    evidence.write(directory / "identity-query.json", query)
                with self.subTest(filename=filename, changed=changed), self.assertRaises(evidence.Invalid):
                    observations.identity(directory, reg, "hardware", spec(), evidence.file_hash(directory / "video.h264"), ended)
                for name, data in original.items():
                    (directory / name).write_bytes(data)
            duplicate = b'{"schema":"coreav.videotoolbox-encoder-identity/1","encoder_id":"actual.fake.encoder",' \
                        b'"hardware":"hardware","hardware":"software"}'
            (directory / "identity.json").write_bytes(duplicate)
            with self.assertRaises(evidence.Invalid):
                observations.identity(directory, reg, "hardware", spec(), evidence.file_hash(directory / "video.h264"))

    def test_failed_query_is_retained_and_never_an_unavailable_encode(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            reg, item = registered(), producer.unit_item()
            (root / "decoder").write_bytes(b"decoder")
            reg["executables"]["coreav"].update(file="decoder", sha256=evidence.file_hash(root / "decoder"))
            (root / "stderr").write_bytes(trace_bytes(observed(item)))
            (root / "video.h264").write_bytes(VIDEO)
            command = timing.commands(reg, {"raw_file": "unused", **item}, "hardware", root,
                                      root / "video.h264", root / "packets.stats")["ffmpeg"]
            command[command.index("-i") + 1] = str(root / "source.yuv")
            command[command.index("-b:v") + 1] = "2000000"
            with mock.patch.object(observations, "host_observation", return_value=reg["host"]), \
                    mock.patch.dict(os.environ, {"COREAV_MAC_ALLOCATION_SHA256": reg["host"]["allocation_sha256"]}), \
                    mock.patch.object(observations, "run", side_effect=evidence.Invalid("child_failure")) as query, \
                    self.assertRaises(evidence.Invalid):
                observations.capture_identity(root, reg, root, spec(), time.time_ns(), command, item, "hardware")
            query.assert_called_once()
            self.assertEqual(evidence.load(root / "identity-query.json")["process"]["status"], "failed")
            self.assertTrue((root / "observations.txt").is_file())
            self.assertFalse((root / "availability.json").exists())

    def test_integer_hint_is_requested_only_and_native_readback_stays_exact(self):
        for num, den, hint in ((30000, 1001, 30), (24000, 1001, 24), (60000, 1001, 60),
                               (3, 2, 2), (5, 2, 3), (7, 3, 2), (1, 1, 1), (479, 2, 240)):
            item = producer.unit_item()
            item["rate"] = {"num": num, "den": den}
            self.assertEqual(registration.cadence_hint(item), hint)
            reg = registered()
            arguments = timing.commands(reg, {"raw_file": "unused", **item}, "hardware", Path("/root"),
                                        Path("/ram/video.h264"), Path("/ram/packets"))["ffmpeg"]
            arguments[arguments.index("-i") + 1] = "/ram/source.yuv"
            arguments[arguments.index("-b:v") + 1] = "2000000"
            value = observations.requested(arguments, item)
            observations.validate_requested(value, item, reg, "hardware")
            self.assertEqual(value["nominal_frame_rate_hint"]["value"], hint)
            self.assertEqual(value["nominal_frame_rate_hint"]["observation"], "unobserved")
            self.assertIsNone(value["realtime"]["-realtime"])
            self.assertIsNone(value["rate_control"]["-constant_bit_rate"])
            for mutate in (lambda r: r.update(observation="observed"),
                           lambda r: r["nominal_frame_rate_hint"].update(observation="observed"),
                           lambda r: r["nominal_frame_rate_hint"].update(provenance="session_property"),
                           lambda r: r["nominal_frame_rate_hint"].update(value=hint + 1),
                           lambda r: r["realtime"].update({"-realtime": "0"}),
                           lambda r: r["rate_control"].update({"-constant_bit_rate": "0"}),
                           lambda r: r["arguments"].extend(["-realtime", "0"])):
                changed = copy.deepcopy(value)
                mutate(changed)
                with self.assertRaises(evidence.Invalid):
                    observations.validate_requested(changed, item, reg, "hardware")
        item = producer.unit_item()
        value = native_report(item)
        producer.native_facts(value, item, "hardware")
        value["settings"]["expected_frame_rate_hint"] = 29
        with self.assertRaises(evidence.Invalid):
            producer.native_facts(value, item, "hardware")

    def test_demux_clock_and_advertised_rate_are_diagnostic_only(self):
        for rate, ticks in ((144, 8333), (225, 5333)):
            item = producer.unit_item()
            item["rate"] = {"num": rate, "den": 1}
            item["source_tick"] = {"num": 1, "den": 90000}
            value = probe_document(item)
            value["streams"][0].update(time_base="1/1200000", r_frame_rate=f"{rate}/1")
            for frame in value["frames"]:
                frame["duration"] = ticks
            _, _, observed_clock = producer.probe_facts(value, item)
            self.assertEqual(observed_clock["demux_time_base_diagnostic"], {"num": 1, "den": 1200000})
            value["frames"][0]["duration"] = ticks + 8
            producer.probe_facts(value, item)
            value["frames"][0]["duration"] = ticks
            value["streams"][0]["r_frame_rate"] = "30/1"
            self.assertEqual(producer.probe_facts(value, item)[2]["advertised_rate_diagnostic"], {"num": 30, "den": 1})
            ledger = b"".join(f"{n} 1/{rate} {n} 6\n".encode() for n in range(item["frames"]))
            producer.packet_ledger(ledger, item, item["frames"] * 6)
            with self.assertRaises(evidence.Invalid):
                producer.packet_ledger(ledger.replace(f"1/{rate}".encode(), b"1/30"), item, item["frames"] * 6)

    def test_empty_oracle_wrappers_keep_producer_metadata_and_clock_checks(self):
        item = producer.unit_item()
        value = probe_document(item)
        expected = producer.probe_facts(value, item)
        value.update(programs=[], stream_groups=[])
        self.assertEqual(producer.probe_facts(value, item), expected)
        for mutate in (lambda r: r["streams"][0].update(color_range="pc"),
                       lambda r: r["streams"][0].update(level=42),
                       lambda r: r["streams"][0].update(has_b_frames=1),
                       lambda r: r["frames"][1].update(pict_type="B"),
                       lambda r: r["frames"][0].update(duration=0),
                       lambda r: r["frames"].pop()):
            changed = copy.deepcopy(value)
            mutate(changed)
            with self.assertRaises(evidence.Invalid):
                producer.probe_facts(changed, item)

    def test_only_one_pinned_encoder_id_log_is_identity_evidence(self):
        item = producer.unit_item()
        good = observed(item)
        parsed, retained = observations.trace(trace_bytes(good) + b"input /secret/path\n", spec())
        self.assertEqual(parsed, {"encoder_id": "actual.fake.encoder"})
        self.assertIn(b"input /secret/path", retained)
        for changed in (b"-require_sw 0\n", b"Init the encoder: actual.fake.encoder\n",
                        b"observed-encoder_id=actual.fake.encoder\n", trace_bytes({}),
                        trace_bytes(good) * 2,
                        trace_bytes(good) + trace_bytes({"encoder_id": "other.encoder"}),
                        trace_bytes({"encoder_id": "bad\0id"})):
            with self.assertRaises(evidence.Invalid):
                observations.trace(changed, spec())
        refusal = trace_bytes({"status": "no_encoder", "submitted": 0})
        self.assertEqual(observations.trace(refusal, spec(), failed=True)[0]["status"], "no_encoder")
        for changed in (refusal, trace_bytes({"status": "no_encoder"}),
                        trace_bytes({"status": "no_encoder", "submitted": 1}),
                        refusal + trace_bytes(good)):
            with self.assertRaises(evidence.Invalid):
                observations.trace(changed, spec())
        for changed in (trace_bytes({"status": "no_encoder"}),
                        trace_bytes({"status": "no_encoder", "submitted": 1}), refusal + trace_bytes(good)):
            with self.assertRaises(evidence.Invalid):
                observations.trace(changed, spec(), failed=True)

    def test_exact_rational_clocks_and_complete_packet_coverage(self):
        item = producer.unit_item()
        good = b"".join(f"{n} 1/30 {n} 6\n".encode() for n in range(4))
        producer.packet_ledger(good, item, 24)
        producer.packet_ledger(good.replace(b"1/30", b"1/29"), item, 24)
        for changed in (good.replace(b"1/30", b"1/15"), good.replace(b"1 1/30 1", b"0 1/30 1"),
                        good.splitlines(keepends=True)[0], good + good[:13], good.replace(b"6", b"0")):
            with self.assertRaises(evidence.Invalid):
                producer.packet_ledger(changed, item, 24)
        for changed in (good.replace(b"6", b"7"), good.replace(b"1/30 1", b"1/30 0")):
            with self.assertRaises(evidence.Invalid):
                producer.packet_ledger(changed, item, 24)
        item["rate"] = {"num": 30000, "den": 1001}
        item["source_tick"] = {"num": 1, "den": 90000}
        fractional = b"".join(f"{n * 6006} 1/180000 {n * 6006} 6\n".encode() for n in range(4))
        producer.packet_ledger(fractional, item, 24)
        boundary = fractional.replace(b"6006 1/180000 6006", b"6007 1/180000 6007")
        producer.packet_ledger(boundary, item, 24)
        with self.assertRaises(evidence.Invalid):
            producer.packet_ledger(boundary.replace(b"6007", b"6008"), item, 24)


class RealEvidencePaths(unittest.TestCase):
    def test_uncovered_2160_is_refused_before_media_or_identity_execution(self):
        reg = registered()
        item = {**producer.unit_item(), "class": 2160, "height": 1200}
        with mock.patch.object(producer, "run") as media, \
                mock.patch.object(observations, "run") as query:
            with self.assertRaises(evidence.Invalid):
                producer.decode(Path("missing"), item, Path("missing"), Path("missing"), Path("missing"), Path("missing"))
            with self.assertRaises(evidence.Invalid):
                observations.capture_identity(Path("missing"), reg, Path("missing"), spec(), 1, [], item, "hardware")
            with self.assertRaises(evidence.Invalid):
                producer.packet_ledger(b"", item, 0)
            with self.assertRaises(evidence.Invalid):
                producer.output_limits(item, "ffmpeg")
        media.assert_not_called()
        query.assert_not_called()

    def test_baseline_capture_analysis_and_replay_bind_all_new_records(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, model = root / "source.yuv", root / "model.json"
            source.write_bytes(smoke.source())
            model.write_bytes(b'{"fake_model":true}')
            reg, item = registered(), producer.unit_item()
            for side, name in (("coreav", "decoder"), ("ffmpeg", "ffmpeg")):
                (root / name).write_bytes(name.encode())
                reg["executables"][side].update(file=name, sha256=evidence.file_hash(root / name))
            reg["metric_tools"]["model"] = evidence.file_hash(model)
            evidence.write(root / "registration.json", {})
            evidence.write(root / "timing.json", {})
            directory = root / "capture"
            directory.mkdir()
            fake = FakeCommands(item, smoke.source())
            command = timing.commands(reg, {"raw_file": "unused", **item}, "hardware", root,
                                      directory / "video.h264", directory / "packets.stats")["ffmpeg"]
            command[command.index("-i") + 1] = str(directory / "source.yuv")
            command[command.index("-b:v") + 1] = "2000000"
            fake(command, directory, 120)
            with mock.patch.object(observations, "host_observation", return_value=reg["host"]), \
                    mock.patch.dict(os.environ, {"COREAV_MAC_ALLOCATION_SHA256": reg["host"]["allocation_sha256"]}), \
                    mock.patch.object(observations, "run", side_effect=fake):
                observations.capture_identity(directory, reg, root, spec(), time.time_ns(), command, item, "hardware")
            for name in ("stdout", "stderr"):
                (directory / name).unlink()
            args = SimpleNamespace(oracle_bin=root, coreav_decoder=root / "decoder", model=model, resume=False,
                                   registration=root / "registration.json", capture=root / "timing.json")
            with mock.patch.object(producer, "run", side_effect=fake):
                result = producer.analyse(directory, source, item, "hardware", "ffmpeg", reg, args, spec(), Oracle(root / "ffmpeg", root / "ffprobe"))
            self.assertEqual(result["baseline"]["identity"]["encoder_id"], "actual.fake.encoder")
            self.assertEqual(result["requested_settings"]["observation"], "unobserved")
            self.assertEqual(sum(row["bytes"] for row in result["packet_ledger"]), len(VIDEO))
            with mock.patch.object(producer, "run", side_effect=AssertionError("replay executed media")), \
                    mock.patch.object(observations, "run", side_effect=AssertionError("replay queried identity")):
                producer.verify_output(result, directory, source, item, "hardware", "ffmpeg", reg, spec(), args)
            for mutate in (lambda r: r["baseline"]["identity"].update(hardware="unknown"),
                           lambda r: r["baseline"]["query"].pop("host"),
                           lambda r: r["requested_settings"].update(observation="observed"),
                           lambda r: r["packet_ledger"][0].update(bytes=1),
                           lambda r: (r["packet_ledger"][0].update(bytes=r["packet_ledger"][0]["bytes"] - 1),
                                      r["packet_ledger"][1].update(bytes=r["packet_ledger"][1]["bytes"] + 1)),
                           lambda r: r.update(packet_text=r["packet_text"].replace("1/30", "1/29")),
                           lambda r: r["packet_ledger"][1].update(dts_value=0),
                           lambda r: r["frames"][1].update(pts={"value": 0, "scale": 30})):
                changed = copy.deepcopy(result)
                mutate(changed)
                with self.assertRaises(evidence.Invalid):
                    judging.conformance(changed, item, "hardware", "ffmpeg", result["output_sha256"], reg)
            for filename in observations.BASELINE_FILES:
                original = (directory / filename).read_bytes()
                (directory / filename).write_bytes(original + b" ")
                with self.subTest(filename=filename), self.assertRaises(evidence.Invalid):
                    producer.verify_output(result, directory, source, item, "hardware", "ffmpeg", reg, spec(), args)
                (directory / filename).write_bytes(original)

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
                producer.analyse(directory, source, item, "hardware", "coreav", reg, args, spec(), Oracle(root / "ffmpeg", root / "ffprobe"))
            self.assertEqual(evidence.load(directory / "analysis-owner.json")["status"], "incomplete")
            (directory / "analysis-owner.json.tmp").write_bytes(b'{"partial":')
            (directory / "analysis" / "unrelated.txt").write_bytes(b"keep")
            args.resume = True
            with mock.patch.object(producer, "run") as command, self.assertRaises(evidence.Invalid):
                producer.analyse(directory, source, item, "hardware", "coreav", reg, args, spec(), Oracle(root / "ffmpeg", root / "ffprobe"))
            command.assert_not_called()
            self.assertEqual((directory / "analysis" / "unrelated.txt").read_bytes(), b"keep")
            (directory / "analysis" / "unrelated.txt").unlink()
            fake = FakeCommands(item, smoke.source())
            with mock.patch.object(producer, "run", side_effect=fake):
                result = producer.analyse(directory, source, item, "hardware", "coreav", reg, args, spec(), Oracle(root / "ffmpeg", root / "ffprobe"))
            self.assertTrue((directory / "failure.json").exists())
            self.assertFalse((directory / "analysis-owner.json.tmp").exists())
            producer.verify_output(result, directory, source, item, "hardware", "coreav", reg, spec(), args)
            self.assertEqual(len(evidence.load(directory / "analysis-owner.json")["history"]), 2)
            other = root / "failed"
            other.mkdir()
            (other / "video.h264").write_bytes(VIDEO)
            evidence.write(other / "native.json", native_report(item))
            with mock.patch.object(producer, "run", side_effect=evidence.Invalid("child_crash")), self.assertRaises(evidence.Invalid):
                producer.analyse(other, source, item, "hardware", "coreav", reg, args, spec(), Oracle(root / "ffmpeg", root / "ffprobe"))
            with mock.patch.object(producer, "run") as command, self.assertRaises(evidence.Invalid):
                producer.analyse(other, source, item, "hardware", "coreav", reg, args, spec(), Oracle(root / "ffmpeg", root / "ffprobe"))
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
            selected = Oracle(root / "owner" / "ffmpeg", root / "owner" / "ffprobe")
            with mock.patch.object(producer, "run", side_effect=fake):
                result = producer.analyse(directory, source, item, "hardware", "coreav", reg, args, spec(), selected)
            producer.verify_output(result, directory, source, item, "hardware", "coreav", reg, spec(), args)
            args.resume = True
            with mock.patch.object(producer, "run", side_effect=AssertionError("completed work reran")):
                self.assertEqual(producer.analyse(directory, source, item, "hardware", "coreav", reg, args, spec(), selected), result)
            changed = copy.deepcopy(reg)
            changed["metric_tools"]["model"] = h("changed-model")
            with mock.patch.object(producer, "run") as command, self.assertRaises(evidence.Invalid):
                producer.analyse(directory, source, item, "hardware", "coreav", changed, args, spec(), selected)
            command.assert_not_called()
            self.assertEqual(result["metrics"]["frames"][0]["squared_error"], 6144)
            self.assertEqual(result["metrics"]["frames"][0]["samples"], 6144)
            self.assertEqual(result["decoders"]["coreav"]["frame_sha256"], result["decoders"]["ffmpeg"]["frame_sha256"])
            commands = fake.commands
            self.assertEqual([command[0] for command in commands if Path(command[0]).name in ("ffmpeg", "ffprobe")],
                             [str(selected.ffmpeg), str(selected.ffprobe), str(selected.ffmpeg)])
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
            refused_software = False

            def native_capture(exporter, directory, mode, runner):
                directory.mkdir()
                value = hosted_refusal() if mode == "software" and refused_software else native(mode, mode == "software")
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
            (root / "owner").mkdir()
            (root / "owner/ffprobe").write_bytes(b"ffprobe")
            selected = Oracle(root / "owner/ffmpeg", root / "owner/ffprobe")
            with mock.patch.object(producer, "preflight", return_value=(reg, selected)) as admission, \
                    mock.patch.object(timing, "host_state", return_value=observation()), \
                    mock.patch.object(observations, "host_observation", return_value=reg["host"]), \
                    mock.patch.dict(os.environ, {"COREAV_MAC_ALLOCATION_SHA256": reg["host"]["allocation_sha256"]}), \
                    mock.patch.object(observations, "run", side_effect=fake), \
                    mock.patch.object(smoke, "capture", side_effect=native_capture), \
                    mock.patch.object(producer, "run", side_effect=fake):
                producer.capture_preflights(args)
            self.assertEqual(admission.call_args_list[0], mock.call(args.registration, args.root, args.ffprobe))
            for name in ("ffmpeg", "ffprobe"):
                commands = [command for command in fake.commands if Path(command[0]).name == name]
                self.assertTrue(commands)
                self.assertTrue(all(command[0] == str(getattr(selected, name)) for command in commands))
            result = timing.availability_file(args.output / "availability.json", reg, root)
            self.assertEqual(result["cells"]["hardware"]["ffmpeg"]["status"], "encoded")
            self.assertEqual(result["cells"]["software"]["coreav"]["status"], "unavailable")
            software = args.output / "preflight.software.coreav.artifacts"
            original = (software / "manifest.json").read_bytes()
            evidence.write(software / "manifest.json", hosted_refusal())
            with self.assertRaises(evidence.Invalid) as refused:
                producer.preflight_facts(software, "software", "coreav", reg, spec())
            self.assertEqual(refused.exception.code, "hosted_refusal_is_not_availability")
            with self.assertRaises(evidence.Invalid):
                timing.availability_file(args.output / "availability.json", reg, root)
            (software / "manifest.json").write_bytes(original)
            refused_software = True
            refused_args = SimpleNamespace(**{**vars(args), "output": root / "hosted-refusal"})
            with mock.patch.object(producer, "preflight", return_value=(reg, Oracle(root / "ffmpeg", (root / "ffprobe").resolve()))), \
                    mock.patch.object(timing, "host_state", return_value=observation()), \
                    mock.patch.object(observations, "host_observation", return_value=reg["host"]), \
                    mock.patch.dict(os.environ, {"COREAV_MAC_ALLOCATION_SHA256": reg["host"]["allocation_sha256"]}), \
                    mock.patch.object(observations, "run", side_effect=fake), \
                    mock.patch.object(smoke, "capture", side_effect=native_capture), \
                    mock.patch.object(producer, "run", side_effect=fake), self.assertRaises(evidence.Invalid) as refused:
                producer.capture_preflights(refused_args)
            self.assertEqual(refused.exception.code, "hosted_refusal_is_not_availability")
            self.assertFalse((refused_args.output / "availability.json").exists())
            self.assertEqual(evidence.load(refused_args.output / "failure.json")["diagnostic"], "hosted_refusal_is_not_availability")
            refused_software = False
            proof_path = args.output / "preflight.hardware.ffmpeg.json"
            value = evidence.load(proof_path)
            value["result"]["pattern_identity"] = False
            evidence.write(proof_path, value)
            with self.assertRaises(evidence.Invalid):
                timing.availability_file(args.output / "availability.json", reg, root)
            fake.drop_observation = "encoder_id"
            args.output = root / "missing-observation"
            with mock.patch.object(producer, "preflight", return_value=(reg, Oracle(root / "ffmpeg", (root / "ffprobe").resolve()))), \
                    mock.patch.object(timing, "host_state", return_value=observation()), \
                    mock.patch.object(observations, "host_observation", return_value=reg["host"]), \
                    mock.patch.dict(os.environ, {"COREAV_MAC_ALLOCATION_SHA256": reg["host"]["allocation_sha256"]}), \
                    mock.patch.object(observations, "run", side_effect=fake), \
                    mock.patch.object(smoke, "capture", side_effect=native_capture), \
                    mock.patch.object(producer, "run", side_effect=fake), self.assertRaises(evidence.Invalid):
                producer.capture_preflights(args)
            self.assertFalse((args.output / "availability.json").exists())
            failure = evidence.load(args.output / "failure.json")
            self.assertEqual(failure["diagnostic"], "ffmpeg_observability_missing")
            self.assertNotIn("/secret", (args.output / "failure.json").read_text())
            self.assertFalse(any(path.name == "stderr" for path in args.output.rglob("*")))


class BaselineBuild(unittest.TestCase):
    def test_prebuilt_identity_accepts_reviewed_gpl_configuration_and_rejects_mutations(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "scripts").mkdir()
            reg = registered()
            pin = {"MACOS_BUILD": "test-vendor", "MACOS_SIGNING_TEAM": "TESTTEAM"}
            for name in ("ffmpeg", "ffprobe"):
                pin.update({f"MACOS_{name.upper()}_URL": f"https://example.invalid/{name}.zip",
                            f"MACOS_{name.upper()}_SHA256": h(f"zip-{name}"),
                            f"MACOS_{name.upper()}_BINARY_SHA256": h(name)})
            (root / "scripts/oracle-ffmpeg.env").write_text("VERSION=test\nBUILD=unrelated-linux-build\n")
            (root / "scripts/oracle-ffmpeg-macos.env").write_text(
                "".join(f"{key}={value}\n" for key, value in pin.items()))
            configuration = ["--enable-gpl", "--enable-videotoolbox"]
            configuration_hash = evidence.sha("\0".join(configuration).encode())
            reg["executables"]["ffmpeg"]["configuration_sha256"] = configuration_hash
            record = {"schema": "coreav.videotoolbox-ffmpeg-build/2", "version": "test",
                      "macos_build": pin["MACOS_BUILD"],
                      "macos_pin_sha256": evidence.file_hash(root / "scripts/oracle-ffmpeg-macos.env"),
                      "binary_sha256": h("ffmpeg"), "ffprobe_file": "bin/ffprobe", "ffprobe_sha256": h("ffprobe"),
                      "configuration": configuration, "configuration_sha256": configuration_hash,
                      "encoder_help_sha256": h("help"),
                      "release_review": {"role": "release-owner", "binary_sha256": h("ffmpeg")},
                      "source_removed": True, "examples_removed": True, "source_acquired": False,
                      "binary_redistributed": False, "observability": spec(),
                      "architectures": {name: "arm64" for name in ("ffmpeg", "ffprobe")},
                      "signatures": {name: {"verified": True, "strict": True, "anchor": "apple generic",
                                            "team": pin["MACOS_SIGNING_TEAM"]} for name in ("ffmpeg", "ffprobe")},
                      "acquisition": {"kind": "pinned-prebuilt-zip", **{
                          name: {"url": pin[f"MACOS_{name.upper()}_URL"],
                                 "archive_sha256": pin[f"MACOS_{name.upper()}_SHA256"],
                                 "binary_sha256": pin[f"MACOS_{name.upper()}_BINARY_SHA256"]}
                          for name in ("ffmpeg", "ffprobe")}}}
            registration.baseline_build(record, reg, root)
            mutations = [lambda r: r.update(schema="coreav.videotoolbox-ffmpeg-build/1"),
                         lambda r: r.update(version="other"), lambda r: r.update(**{"macos_build": "other"}),
                         lambda r: r.update(macos_pin_sha256=h("other")),
                         lambda r: r.update(binary_sha256=h("other")),
                         lambda r: r.update(ffprobe_sha256=h("other")),
                         lambda r: r.update(ffprobe_file="other/ffprobe"),
                         lambda r: r["configuration"].append("--different"),
                         lambda r: r["release_review"].update(role="caller"),
                         lambda r: r["release_review"].update(binary_sha256=h("other")),
                         lambda r: r.update(source_removed=False), lambda r: r.update(examples_removed=False),
                         lambda r: r.update(source_acquired=True), lambda r: r.update(binary_redistributed=True),
                         lambda r: r["acquisition"]["ffmpeg"].update(url="https://example.invalid/other.zip"),
                         lambda r: r["acquisition"]["ffprobe"].update(archive_sha256=h("other")),
                         lambda r: r["acquisition"]["ffmpeg"].update(binary_sha256=h("other")),
                         lambda r: r["architectures"].update(ffprobe="x86_64"),
                         lambda r: r["signatures"]["ffmpeg"].update(team="OTHERTEAM"),
                         lambda r: r["signatures"]["ffprobe"].update(verified=False),
                         lambda r: r["signatures"]["ffmpeg"].update(strict=False),
                         lambda r: r["observability"]["encoder_id"].update(prefix="requested-encoder="),
                         lambda r: r.pop("observability"), lambda r: r.pop("release_review")]
            for mutate in mutations:
                changed = copy.deepcopy(record)
                mutate(changed)
                with self.assertRaises(evidence.Invalid):
                    registration.baseline_build(changed, reg, root)
            (root / "scripts/oracle-ffmpeg-macos.env").unlink()
            with self.assertRaises(OSError):
                registration.baseline_build(record, reg, root)


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
            judging.quality({height: [judging.deltas(actual, perfect)] * 10 for height in registration.COVERED_CLASSES},
                            [judging.deltas(perfect, perfect)], reg)

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
                    mock.patch.object(producer, "analysis_preflight", return_value=(reg, Oracle(root / "ffmpeg", root / "ffprobe"))), \
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
        reg = registered()
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
                path.write_text(reg["host"]["os_version"] if argv[-1] == "-productVersion" else reg["host"]["os_build"])
            return 1

        with mock.patch.object(registration.sys, "platform", "darwin"), \
                mock.patch.object(registration, "run", side_effect=command):
            a = registration.host_observation()
            identifiers[0] = second
            b = registration.host_observation()
        self.assertEqual({k: v for k, v in a.items() if k != "hardware_sha256"},
                         {k: v for k, v in b.items() if k != "hardware_sha256"})
        self.assertNotEqual(a["hardware_sha256"], b["hardware_sha256"])
        self.assertEqual({k: v for k, v in a.items() if k != "hardware_sha256"},
                         {k: reg["host"][k] for k in a if k != "hardware_sha256"})
        reg["host"]["hardware_sha256"] = a["hardware_sha256"]
        with tempfile.TemporaryDirectory() as temporary, \
                mock.patch.dict(os.environ, {"COREAV_MAC_ALLOCATION_SHA256": reg["host"]["allocation_sha256"]}, clear=True), \
                mock.patch.object(registration.sys, "platform", "darwin"), \
                mock.patch.object(registration, "source_registration", return_value=reg), \
                mock.patch.object(registration, "host_observation", return_value=b), \
                mock.patch.object(registration, "file_hash", side_effect=AssertionError("post-host file check reached")) as binary, \
                mock.patch.object(registration, "run") as command, \
                self.assertRaisesRegex(evidence.Invalid, "^host_mismatch$"):
            registration.preflight(Path(temporary) / "registration.json", Path(temporary))
        binary.assert_not_called()
        command.assert_not_called()
        with tempfile.TemporaryDirectory() as temporary, \
                mock.patch.dict(os.environ, {"COREAV_MAC_ALLOCATION_SHA256": reg["host"]["allocation_sha256"]}, clear=True), \
                mock.patch.object(registration.sys, "platform", "darwin"), \
                mock.patch.object(registration, "source_registration", return_value=reg), \
                mock.patch.object(registration, "host_observation", return_value=a), \
                mock.patch.object(registration, "file_hash", side_effect=AssertionError("post-host file check reached")) as binary, \
                mock.patch.object(registration, "run") as command, \
                self.assertRaisesRegex(AssertionError, "^post-host file check reached$"):
            registration.preflight(Path(temporary) / "registration.json", Path(temporary))
        binary.assert_called_once()
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

    def test_fingerprint_regression_detects_neutralized_hardware_comparison(self):
        original = registration.preflight

        def neutralized(path, root):
            reg = registration.source_registration(path, root)
            observed = registration.host_observation()
            with mock.patch.object(registration, "host_observation", return_value={
                    **observed, "hardware_sha256": reg["host"]["hardware_sha256"]}):
                return original(path, root)

        case = type(self)("test_identical_m1_specs_cannot_hide_a_different_observed_machine")
        result = unittest.TestResult()
        with mock.patch.object(registration, "preflight", side_effect=neutralized):
            case.run(result)
        self.assertEqual(result.testsRun, 1)
        self.assertEqual(result.errors, [])
        self.assertEqual(len(result.failures), 1)
        self.assertIn("post-host file check reached", result.failures[0][1])

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
