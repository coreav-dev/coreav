"""Mocked owner-path execution regressions; no media or oracle processes."""

from contextlib import ExitStack
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

import baseline_smoke
import evidence
import preparation
import producer
import registration
import timing
from test_oracle import Oracle
from test_preparation import item
from test_tools import registered, observation


class OwnerPaths(unittest.TestCase):
    def test_timing_gate_executes_retained_encoder(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            reg = registered()
            oracle = Oracle(root / "owner-encoder", root / "owner-probe")
            evidence.write(root / "registration.json", {})
            registration_hash = evidence.file_hash(root / "registration.json")
            args = SimpleNamespace(report=root / "gate.json", registration=root / "registration.json", root=root,
                                   phase="before", availability=root / "availability.json", ram_output=root)
            with mock.patch.object(timing, "preflight", return_value=(reg, oracle)), \
                    mock.patch.object(timing.cache, "preflight"), \
                    mock.patch.object(timing, "availability_file", return_value={"registration_sha256": registration_hash}), \
                    mock.patch.object(timing, "ram_volume"), mock.patch.object(timing, "input_hashes"), \
                    mock.patch.object(timing, "host_state", return_value=observation()), \
                    mock.patch.object(timing, "warm"), \
                    mock.patch.object(timing.cache, "observe", return_value={}), \
                    mock.patch.object(timing.cache, "validate"), \
                    mock.patch.object(timing, "run", side_effect=evidence.Invalid("child_failure")) as execute, \
                    self.assertRaises(evidence.Invalid):
                timing.capture_gate(args)
            self.assertEqual(execute.call_count, 1)
            self.assertEqual(execute.call_args.args[0][0], str(oracle.ffmpeg))
            self.assertEqual(evidence.load(args.report)["status"], "failed")

    def test_baseline_smoke_executes_returned_encoder(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            selected = root / "selected"
            selected.mkdir()
            oracle = Oracle(root / "owner-encoder", root / "owner-probe")
            for path in (selected / "ffmpeg", selected / "ffprobe", oracle.ffmpeg, oracle.ffprobe, root / "exporter"):
                path.write_bytes(b"synthetic")
            fixture = producer.unit_item()
            calls = []

            def execute(argv, directory, _timeout, **kwargs):
                calls.append(argv)
                directory = Path(directory)
                if "--resolve" in argv:
                    (directory / kwargs["stdout"]).write_bytes(b"")
                    (directory / kwargs["stderr"]).write_bytes(b"")
                elif "identify" in argv:
                    evidence.write(directory / "identity.json", {"schema": "coreav.videotoolbox-encoder-identity/1",
                                   "encoder_id": "synthetic", "hardware": "hardware"})
                else:
                    (directory / "video.h264").write_bytes(b"synthetic")
                    (directory / "packets.stats").write_bytes(b"synthetic")
                    (directory / "observations.txt").write_bytes(b"synthetic")

            with ExitStack() as stack:
                owner = stack.enter_context(mock.patch("test_oracle.require", return_value=oracle))
                stack.enter_context(mock.patch.object(baseline_smoke.sys, "platform", "darwin"))
                stack.enter_context(mock.patch.object(sys, "argv", ["baseline", "--oracle-bin", str(selected),
                    "--exporter", str(root / "exporter"), "--output", str(root / "output")]))
                stack.enter_context(mock.patch.object(baseline_smoke, "fixtures", return_value={"synthetic": fixture}))
                stack.enter_context(mock.patch.object(baseline_smoke, "fixture_frames", return_value=iter([b"raw"] * 4)))
                stack.enter_context(mock.patch.object(baseline_smoke, "trace", return_value=({"encoder_id": "synthetic"}, None)))
                stack.enter_context(mock.patch.object(baseline_smoke, "packet_ledger", return_value=[{"bytes": 9}]))
                stack.enter_context(mock.patch.object(baseline_smoke, "run", side_effect=execute))
                baseline_smoke.main()
            owner.assert_called_once_with(selected / "ffmpeg", selected / "ffprobe")
            self.assertEqual(calls[1][0], str(oracle.ffmpeg))
            self.assertEqual(calls[0][-4:], ["--ffmpeg", str(oracle.ffmpeg), "--ffprobe", str(oracle.ffprobe)])
            self.assertTrue((root / "output/oracle.stdout").is_file())
            self.assertTrue((root / "output/oracle.stderr").is_file())

    def test_preparation_executes_returned_encoder(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            selected = root / "selected"
            selected.mkdir()
            oracle = Oracle(root / "owner-encoder", root / "owner-probe")
            for name, path in (("ffmpeg", oracle.ffmpeg), ("ffprobe", oracle.ffprobe)):
                data = ("fake " + name).encode()
                path.write_bytes(data)
                (selected / name).write_bytes(data)
            source = root / "source"
            source.write_bytes(b"source")
            calls = []

            def capture(argv, directory, phase, binding, deadline, raw=None, guard=None):
                calls.append(argv)
                if phase == "oracle":
                    return {"status": "ok"}
                data = b"abcdef" * 240
                (directory / "prepare.raw").write_bytes(data)
                raw.feed(data)
                return {"status": "ok", "raw_hashes": raw.finish()}

            with mock.patch("test_oracle.require", return_value=oracle) as owner, \
                    mock.patch.object(preparation.sys, "platform", "linux"), \
                    mock.patch.object(preparation.platform, "machine", return_value="x86_64"), \
                    mock.patch.object(preparation.os, "pidfd_open", create=True), \
                    mock.patch.object(preparation.process, "capture", side_effect=capture):
                result = preparation.materialize_one(source, item(), selected, root / "output")
            self.assertEqual(result["status"], "complete")
            owner.assert_called_once_with(selected / "ffmpeg", selected / "ffprobe")
            self.assertEqual(calls[-1][0], str(oracle.ffmpeg))

    def test_registration_executes_and_returns_qualified_pair(self):
        self.registration_fixture()

    def test_registration_qualifies_the_explicit_probe_without_a_new_path_guard(self):
        self.registration_fixture(explicit=True)

    def test_registration_keeps_the_pin_stderr_refusal(self):
        self.registration_fixture(pin_error=b"synthetic qualification diagnostic")

    def registration_fixture(self, explicit=False, pin_error=b""):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            reg = registered()
            for record in reg["executables"].values():
                path = root / record["file"]
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"synthetic")
            binary = root / reg["executables"]["ffmpeg"]["file"]
            (binary.parent / "ffprobe").write_bytes(b"synthetic")
            probe = root / "explicit-probe" if explicit else None
            oracle = Oracle(root / "owner-encoder", root / "owner-probe")
            help_bytes = b"synthetic help"
            build = {"ffprobe_file": str((binary.parent / "ffprobe").relative_to(root)),
                     "macos_build": "synthetic", "configuration": ["--synthetic"],
                     "encoder_help_sha256": evidence.sha(help_bytes)}
            calls = []

            def execute(argv, directory, timeout, stdout, stderr):
                calls.append(argv)
                (Path(directory) / stderr).write_bytes(pin_error if stdout == "pin" else b"")
                data = b"ffmpeg version synthetic\nconfiguration: --synthetic\n" if "-version" in argv else help_bytes
                (Path(directory) / stdout).write_bytes(data)

            with mock.patch("test_oracle.require", return_value=oracle) as owner, \
                    mock.patch.object(registration.sys, "platform", "darwin"), \
                    mock.patch.dict(registration.os.environ, {"COREAV_MAC_ALLOCATION_SHA256": reg["host"]["allocation_sha256"]}, clear=True), \
                    mock.patch.object(registration, "source_registration", return_value=reg), \
                    mock.patch.object(registration, "host_observation", return_value=reg["host"]), \
                    mock.patch.object(registration, "file_hash", side_effect=lambda path, **kwargs:
                        next(record["sha256"] for record in reg["executables"].values() if root / record["file"] == path)), \
                    mock.patch.object(registration, "load", return_value=build), \
                    mock.patch.object(registration, "run", side_effect=execute):
                if pin_error:
                    with self.assertRaisesRegex(evidence.Invalid, "baseline_pin"):
                        registration.preflight(root / "registration.json", root, probe)
                else:
                    result = registration.preflight(root / "registration.json", root, probe)
            owner.assert_called_once_with(binary, probe if explicit else binary.parent / "ffprobe")
            if pin_error:
                self.assertEqual(len(calls), 1)
                return
            self.assertEqual(result, (reg, oracle))
            self.assertEqual([argv[0] for argv in calls[1:]], [str(oracle.ffmpeg)] * 2)
            self.assertEqual(calls[0][-4:], ["--ffmpeg", str(oracle.ffmpeg), "--ffprobe", str(oracle.ffprobe)])

    def test_analysis_preflight_executes_returned_filter_tool(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            reg = registered()
            oracle = Oracle(root / "owner-encoder", root / "owner-probe")
            reg["metric_tools"]["metrics"] = reg["metric_tools"]["ffmpeg"]
            args = SimpleNamespace(root=root, registration=root / "registration.json", oracle_bin=root / "selected",
                                   coreav_decoder=root / "decoder", model=root / "model", install_oracle=False)
            hashes = {oracle.ffmpeg: reg["metric_tools"]["ffmpeg"], oracle.ffprobe: reg["metric_tools"]["ffprobe"],
                      args.coreav_decoder: reg["metric_tools"]["coreav_decoder"], args.model: reg["metric_tools"]["model"],
                      Path(producer.__file__).resolve(): reg["metric_tools"]["evidence_producer"]}
            hashes.update({args.oracle_bin / name: reg["metric_tools"][name] for name in ("ffmpeg", "ffprobe")})
            calls = []

            def execute(argv, directory, timeout, **kwargs):
                calls.append(argv)
                (Path(directory) / "stdout").write_text(" T libvmaf\n T psnr\n T ssim\n")

            with mock.patch("test_oracle.require", return_value=oracle) as owner, \
                    mock.patch.object(producer.sys, "platform", "linux"), \
                    mock.patch.dict(producer.os.environ, {}, clear=True), \
                    mock.patch.object(producer, "source_registration", return_value=reg), \
                    mock.patch.object(producer, "file_hash", side_effect=lambda path, *args: hashes[path]), \
                    mock.patch.object(producer, "run", side_effect=execute):
                result = producer.analysis_preflight(args)
            owner.assert_called_once_with(args.oracle_bin / "ffmpeg", args.oracle_bin / "ffprobe")
            self.assertEqual(result, (reg, oracle))
            self.assertEqual(calls, [[str(oracle.ffmpeg), "-hide_banner", "-filters"]])


if __name__ == "__main__":
    unittest.main()
