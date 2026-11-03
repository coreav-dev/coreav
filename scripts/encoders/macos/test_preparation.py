"""Synthetic bytes and mocked materialization only; no media or oracle runs."""

import copy
import hashlib
import json
import os
import signal
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import preparation as p
import preparation_process as process
from evidence import Invalid


def digest(value):
    return hashlib.sha256(value).hexdigest()


def item():
    frame = b"abcdef"
    return {
        "id": digest(b"source"),
        "class": 360,
        "source_sha256": digest(b"source"),
        "source_bytes": 6,
        "stream": 0,
        "width": 2,
        "height": 2,
        "frames": 240,
        "rate": {"num": 24, "den": 1},
        "colour": dict(p.COLOUR),
        "raw_sha256": digest(frame * 240),
        "source_frame_sha256": [digest(frame)] * 240,
        "source_tick": {"num": 1, "den": 24000},
        "source_start_ticks": 0,
        "audio_start_ticks": None,
        "preparation": {
            "source_pixel_format": "yuv420p",
            "source_colour": {
                "color_primaries": "bt709",
                "color_transfer": "bt709",
                "color_space": "bt709",
                "color_range": "tv",
            },
            "assumptions": [
                {
                    "field": field,
                    "assumed_value": value,
                    "observed": False,
                    "rule": "literal_yuvj_full_otherwise_limited"
                    if field == "color_range"
                    else "absent_tag_bt709",
                }
                for field, value in (
                    ("color_primaries", "bt709"),
                    ("color_transfer", "bt709"),
                    ("color_space", "bt709"),
                    ("color_range", "tv"),
                )
            ],
            "container_name": "mov",
            "field_order": "progressive",
            "oracle_sha256": {
                "ffmpeg": digest(b"fake ffmpeg"),
                "ffprobe": digest(b"fake ffprobe"),
            },
        },
    }


class RecipeTests(unittest.TestCase):
    def test_unconsumed_freeze_metadata_does_not_change_recipe(self):
        value = item()
        expected = p.command("source", value, "oracle")
        value.update(
            source_metadata={"observed": True}, capture={"sha256": digest(b"capture")}
        )
        self.assertEqual(p.command("source", value, "oracle"), expected)

    def test_exact_literal_conversion_and_command(self):
        value = item()
        before = copy.deepcopy(value)
        command = p.command("/local/source.bin", value, "/local/oracle")
        conversion = (
            "scale=w=iw:h=ih:flags=bilinear+accurate_rnd+bitexact:sws_dither=none:in_color_matrix=bt709:"
            "out_color_matrix=bt709:in_range=tv:out_range=tv:threads=1,format=pix_fmts=yuv444p16le,"
            "zscale=primariesin=bt709:transferin=bt709:matrixin=bt709:rangein=tv:primaries=bt709:"
            "transfer=bt709:matrix=bt709:range=limited:dither=ordered:filter=bilinear,format=pix_fmts=yuv420p"
        )
        self.assertEqual(
            command,
            [
                "/local/oracle/ffmpeg",
                "-hide_banner",
                "-nostdin",
                "-nostats",
                "-xerror",
                "-max_alloc",
                "268435456",
                "-max_pixels",
                "8294400",
                "-err_detect",
                "explode",
                "-ec",
                "0",
                "-noauto_conversion_filters",
                "-filter_threads",
                "1",
                "-filter_complex_threads",
                "1",
                "-threads",
                "1",
                "-probesize",
                "5242880",
                "-analyzeduration",
                "5000000",
                "-protocol_whitelist",
                "file,pipe",
                "-format_whitelist",
                "mov",
                "-enable_drefs",
                "0",
                "-use_absolute_path",
                "0",
                "-noautorotate",
                "-copyts",
                "-i",
                "/local/source.bin",
                "-map",
                "0:0",
                "-an",
                "-sn",
                "-dn",
                "-noautoscale",
                "-v",
                "error",
                "-vf",
                conversion,
                "-frames:v",
                "240",
                "-fps_mode",
                "passthrough",
                "-c:v",
                "rawvideo",
                "-threads:v",
                "1",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "rawvideo",
                "pipe:1",
            ],
        )
        self.assertEqual(value, before)

    def test_assumptions_remain_labeled_and_cannot_be_observations(self):
        value = item()
        p.validate_recipe(value)
        value["preparation"]["assumptions"][0]["observed"] = True
        with self.assertRaisesRegex(Invalid, "assumption_label"):
            p.validate_recipe(value)
        value = item()
        value["preparation"]["source_pixel_format"] = "yuvj420p"
        with self.assertRaisesRegex(Invalid, "assumption_label"):
            p.validate_recipe(value)
        value["preparation"]["source_colour"]["color_range"] = "pc"
        value["preparation"]["assumptions"][-1]["assumed_value"] = "pc"
        self.assertIn(
            ":in_range=pc:out_range=pc:",
            p.command("source", value, "oracle")[
                p.command("source", value, "oracle").index("-vf") + 1
            ],
        )

    def test_unsupported_scope_colour_geometry_prefix_and_missing_identity_refuse(self):
        changes = [("class", 2160), ("height", 2160), ("width", 3), ("frames", 241)]
        for field, replacement in changes:
            with self.subTest(field=field):
                value = item()
                value[field] = replacement
                with self.assertRaises(Invalid):
                    p.validate_recipe(value)
        for field, replacement in (
            ("field_order", "unknown"),
            ("source_pixel_format", "rgba"),
            ("oracle_sha256", {}),
        ):
            value = item()
            value["preparation"][field] = replacement
            with self.assertRaises(Invalid):
                p.validate_recipe(value)
        value = item()
        value["preparation"]["source_colour"]["color_transfer"] = "smpte2084"
        with self.assertRaisesRegex(Invalid, "source_colour"):
            p.validate_recipe(value)
        with (
            mock.patch.object(p, "MAX_PIXELS", 1),
            self.assertRaisesRegex(Invalid, "resource_constants"),
        ):
            p.validate_recipe(item())

    def test_fractional_clock_and_high_bit_depth_do_not_add_resampling(self):
        value = item()
        value.update(
            rate={"num": 30000, "den": 1001},
            frames=300,
            source_frame_sha256=[digest(b"abcdef")] * 300,
        )
        value["preparation"]["source_pixel_format"] = "yuv420p10le"
        value["preparation"]["container_name"] = "matroska"
        command = p.command("source", value, "oracle")
        self.assertNotIn("-r", command)
        self.assertNotIn("-enable_drefs", command)
        self.assertEqual(command[command.index("-frames:v") + 1], "300")
        self.assertEqual(value["rate"], {"num": 30000, "den": 1001})
        self.assertFalse(hasattr(p, "main"))


@unittest.skipUnless(sys.platform == "linux", "Linux process controls")
class MaterializationTests(unittest.TestCase):
    def run_fixture(
        self,
        change=None,
        capture_error=None,
        after_capture=None,
        publication_failure=False,
    ):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "input.bin"
            source.write_bytes(b"source")
            oracle = root / "oracle"
            oracle.mkdir()
            for name in ("ffmpeg", "ffprobe"):
                (oracle / name).write_bytes(("fake " + name).encode())
            value = item()
            if change:
                change(value, source, oracle)
            output = root / "result"
            publish = process.publish

            def published(path, result):
                if publication_failure and result.get("status") == "complete":
                    raise OSError("synthetic publication fault")
                return publish(path, result)

            def captured(argv, dest, phase, binding, deadline, raw=None, guard=None):
                self.assertLessEqual(deadline - time.time(), 60)
                self.assertEqual((dest / "source.bin").read_bytes(), b"source")
                self.assertEqual(
                    binding["oracle_sha256"], value["preparation"]["oracle_sha256"]
                )
                if phase == "oracle":
                    self.assertEqual(
                        argv[1:3], ["-I", str(p.ROOT / "scripts/oracle_pin.py")]
                    )
                    if after_capture:
                        after_capture(phase, source, oracle)
                    return {"status": "ok"}
                if capture_error:
                    raise capture_error
                data = b"abcdef" * 240
                (dest / "prepare.raw").write_bytes(data)
                raw.feed(data)
                result = {"status": "ok", "raw_hashes": raw.finish()}
                process.publish(dest / "prepare.process.json", result)
                if after_capture:
                    after_capture(phase, source, oracle)
                return result

            with (
                mock.patch.object(process, "capture", side_effect=captured) as capture,
                mock.patch.object(process, "publish", side_effect=published),
            ):
                try:
                    result = p.materialize_one(source, value, oracle, output)
                    error = None
                except BaseException as failure:
                    result, error = None, failure
                calls = capture.call_count
            record = json.loads((output / "materialization.json").read_bytes())
            remaining = {
                q.name: q.read_bytes() for q in output.iterdir() if q.is_file()
            }
            with mock.patch.object(process, "capture") as second:
                with self.assertRaises(FileExistsError):
                    p.materialize_one(source, value, oracle, output)
                second.assert_not_called()
            self.assertEqual(
                remaining,
                {q.name: q.read_bytes() for q in output.iterdir() if q.is_file()},
            )
            self.assertNotIn("source.bin", remaining)
            return result, error, record, remaining, calls

    def test_exact_raw_is_written_and_completed_without_overwrite(self):
        result, error, record, files, calls = self.run_fixture()
        self.assertIsNone(error)
        self.assertEqual(result["status"], record["status"])
        self.assertEqual(record["status"], "complete")
        self.assertEqual(files["reference.yuv"], b"abcdef" * 240)
        self.assertEqual(calls, 2)
        self.assertNotIn("prepare.raw", files)

    def test_source_hash_and_size_changes_refuse_before_any_process(self):
        for data in (b"change", b"short"):
            with self.subTest(data=data):
                _, error, record, files, calls = self.run_fixture(
                    lambda _v, source, _o, data=data: source.write_bytes(data)
                )
                self.assertIsInstance(error, Invalid)
                self.assertEqual(calls, 0)
                self.assertEqual(record["status"], "refused")
                self.assertNotIn("reference.yuv", files)

    def test_oracle_byte_change_refuses_before_shared_verifier(self):
        _, error, _, _, calls = self.run_fixture(
            lambda _v, _s, oracle: (oracle / "ffmpeg").write_bytes(
                b"changed executable"
            )
        )
        self.assertIsInstance(error, Invalid)
        self.assertEqual(calls, 0)

    def test_oracle_identity_is_unchanged_after_verifier_and_materialization(self):
        for changed_phase in ("oracle", "prepare"):

            def changed(phase, _source, oracle, changed_phase=changed_phase):
                if phase == changed_phase:
                    (oracle / "ffprobe").write_bytes(b"changed executable")

            _, error, _, files, calls = self.run_fixture(after_capture=changed)
            self.assertIsInstance(error, Invalid)
            self.assertEqual(calls, 1 if changed_phase == "oracle" else 2)
            self.assertNotIn("reference.yuv", files)

    def test_verified_spool_not_mutable_caller_path_is_consumed(self):
        def changed(phase, source, _oracle):
            if phase == "oracle":
                source.write_bytes(b"change")

        result, error, _, _, calls = self.run_fixture(after_capture=changed)
        self.assertIsNone(error)
        self.assertEqual(result["status"], "complete")
        self.assertEqual(calls, 2)

    def test_success_publication_fault_removes_raw_and_is_fatal(self):
        _, error, record, files, _ = self.run_fixture(publication_failure=True)
        self.assertIsInstance(error, process.InfrastructureFailure)
        self.assertEqual(record["status"], "fatal_infrastructure")
        self.assertNotIn("raw_hashes", record)
        self.assertNotIn("reference.yuv", files)

    def test_whole_or_individual_frame_hash_mismatch_refuses_and_retains_failure(self):
        for field in ("raw_sha256", "source_frame_sha256"):

            def change(value, _s, _o, field=field):
                value[field] = (
                    digest(b"wrong")
                    if field == "raw_sha256"
                    else [digest(b"wrong")] * 240
                )

            _, error, record, files, calls = self.run_fixture(change)
            self.assertIsInstance(error, Invalid)
            self.assertEqual(record["failure"]["message"], "preparation_raw_identity")
            self.assertEqual(calls, 2)
            self.assertNotIn("reference.yuv", files)
            self.assertNotIn("prepare.raw", files)

    def test_cleanup_uncertainty_is_terminal_and_preserves_both_diagnostics(self):
        failure = process.CleanupFailure(
            {"kind": "ProcessFailure", "message": "decode"},
            {"kind": "SyntheticDiscoveryFault", "message": "cleanup"},
        )
        _, error, record, files, calls = self.run_fixture(capture_error=failure)
        self.assertIs(error, failure)
        self.assertEqual(record["status"], "fatal_infrastructure")
        self.assertEqual(record["infrastructure_failure"], failure.details)
        self.assertNotIn("raw_hashes", record)
        self.assertNotIn("reference.yuv", files)
        self.assertEqual(calls, 2)

    def test_platform_refuses_before_source_or_process(self):
        with (
            mock.patch.object(p.sys, "platform", "darwin"),
            mock.patch.object(process, "capture") as capture,
        ):
            with self.assertRaisesRegex(Invalid, "linux_required"):
                p.materialize_one("absent", item(), "absent", "absent")
            capture.assert_not_called()


@unittest.skipUnless(sys.platform == "linux", "Linux process controls")
class CaptureTests(unittest.TestCase):
    def test_exact_count_is_streamed_while_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = process.capture(
                [sys.executable, "-c", "import os;os.write(1,b'abcdef')"],
                root,
                "prepare",
                {},
                time.time() + 3,
                process.RawHashes(3, 2),
            )
            self.assertEqual(
                result["raw_hashes"]["frame_sha256"], [digest(b"abc"), digest(b"def")]
            )
            self.assertEqual((root / "prepare.raw").read_bytes(), b"abcdef")
            self.assertEqual(
                result["cleanup"], "group_exit_observed_and_direct_child_reaped"
            )

    def test_short_excess_and_exit_zero_decoder_error_cannot_pass(self):
        for program in (
            "import os;os.write(1,b'abc')",
            "import os;os.write(1,b'abcdefg')",
            "import os;os.write(1,b'abcdef');os.write(2,b'decoder error\\n')",
        ):
            with (
                self.subTest(program=program),
                tempfile.TemporaryDirectory() as directory,
            ):
                with self.assertRaises(process.ProcessFailure):
                    process.capture(
                        [sys.executable, "-c", program],
                        Path(directory),
                        "prepare",
                        {},
                        time.time() + 3,
                        process.RawHashes(3, 2),
                    )
                record = json.loads(
                    (Path(directory) / "prepare.process.json").read_bytes()
                )
                self.assertNotEqual(record["status"], "ok")

    def test_cleanup_precedes_primary_failure_and_withholds_success_hashes(self):
        real_popen = process.subprocess.Popen

        def completed(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            child.wait(timeout=2)
            return child

        for primary_fails in (False, True):
            with (
                self.subTest(primary_fails=primary_fails),
                tempfile.TemporaryDirectory() as directory,
            ):
                program = "import os;os.write(1,b'abcdef')" + (
                    ";os.write(2,b'error')" if primary_fails else ""
                )
                with (
                    mock.patch.object(
                        process.subprocess, "Popen", side_effect=completed
                    ),
                    mock.patch.object(
                        process,
                        "terminate_group",
                        side_effect=OSError("synthetic cleanup fault"),
                    ),
                    self.assertRaises(process.CleanupFailure),
                ):
                    process.capture(
                        [sys.executable, "-c", program],
                        Path(directory),
                        "prepare",
                        {},
                        time.time() + 3,
                        process.RawHashes(3, 2),
                    )
                record = json.loads(
                    (Path(directory) / "prepare.process.json").read_bytes()
                )
                self.assertEqual(record["status"], "fatal_cleanup_uncertain")
                self.assertNotIn("raw_hashes", record)
                self.assertEqual(
                    record["primary_failure"]["kind"]
                    if primary_fails
                    else record["primary_failure"],
                    "ProcessFailure" if primary_fails else None,
                )

    def test_group_discovery_fault_still_kills_and_reaps_direct_child(self):
        real_popen = process.subprocess.Popen
        children = []

        def launched(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            children.append(child)
            return child

        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(process.subprocess, "Popen", side_effect=launched),
            mock.patch.object(
                process.Path, "glob", side_effect=OSError("synthetic discovery fault")
            ),
            self.assertRaises(process.CleanupFailure),
        ):
            process.capture(
                [
                    sys.executable,
                    "-c",
                    "import os,time;os.write(1,b'too many');time.sleep(30)",
                ],
                Path(directory),
                "prepare",
                {},
                time.time() + 3,
                process.RawHashes(1, 1),
            )
        self.assertEqual(len(children), 1)
        self.assertIsNotNone(children[0].returncode)
        with self.assertRaises(ChildProcessError):
            os.waitpid(children[0].pid, os.WNOHANG)

    def test_signal_during_launch_is_retained_and_child_is_reaped(self):
        real_popen = process.subprocess.Popen
        children, signals = [], []
        previous = signal.signal(
            signal.SIGTERM, lambda sig, _frame: signals.append(sig)
        )

        def launched(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            children.append(child)
            os.kill(os.getpid(), signal.SIGTERM)
            return child

        try:
            with (
                tempfile.TemporaryDirectory() as directory,
                mock.patch.object(process.subprocess, "Popen", side_effect=launched),
            ):
                with self.assertRaisesRegex(Invalid, "interrupted"):
                    process.capture(
                        [sys.executable, "-c", "import time;time.sleep(30)"],
                        Path(directory),
                        "prepare",
                        {},
                        time.time() + 3,
                        process.RawHashes(1, 1),
                    )
                record = json.loads(
                    (Path(directory) / "prepare.process.json").read_bytes()
                )
                self.assertEqual(record["primary_failure"]["kind"], "_Interrupted")
                self.assertEqual(
                    record["cleanup"], "group_exit_observed_and_direct_child_reaped"
                )
            self.assertEqual(signals, [signal.SIGTERM])
            self.assertIsNotNone(children[0].returncode)
            with self.assertRaises(ChildProcessError):
                os.waitpid(children[0].pid, os.WNOHANG)
        finally:
            signal.signal(signal.SIGTERM, previous)

    def test_timeout_and_metadata_limit_refuse_with_retained_records(self):
        cases = [
            (
                "import os,time;os.close(1);os.close(2);time.sleep(30)",
                0.1,
                "command_deadline",
            ),
            ("import os;os.write(1,b'x'*(16777216+1))", 3, "metadata_output_limit"),
        ]
        for program, timeout, reason in cases:
            with (
                self.subTest(reason=reason),
                tempfile.TemporaryDirectory() as directory,
            ):
                with self.assertRaisesRegex(process.ProcessFailure, reason):
                    process.capture(
                        [sys.executable, "-c", program],
                        Path(directory),
                        "oracle",
                        {},
                        time.time() + timeout,
                    )
                record = json.loads(
                    (Path(directory) / "oracle.process.json").read_bytes()
                )
                self.assertEqual(
                    record["cleanup"], "group_exit_observed_and_direct_child_reaped"
                )

    def test_process_record_publication_fault_is_fatal(self):
        publish = process.publish

        def failed(path, value):
            if value.get("finished_utc"):
                raise OSError("synthetic publication fault")
            return publish(path, value)

        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(process, "publish", side_effect=failed),
        ):
            with self.assertRaises(process.InfrastructureFailure) as raised:
                process.capture(
                    [sys.executable, "-c", "pass"],
                    Path(directory),
                    "oracle",
                    {},
                    time.time() + 3,
                )
            self.assertEqual(
                raised.exception.details["publication_failure"]["kind"], "OSError"
            )


if __name__ == "__main__":
    unittest.main()
