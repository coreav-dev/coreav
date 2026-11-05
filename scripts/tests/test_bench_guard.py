import contextlib
import importlib.util
import io
import json
import math
import pathlib
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import bench_guard

SPEC = importlib.util.spec_from_file_location("release_set", ROOT / "tools/compat-corpus/release_set.py")
release_set = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release_set)


class InstructionTolerance(unittest.TestCase):
    def test_profile_capture_changes_only_the_destination_not_the_measurement(self):
        with tempfile.TemporaryDirectory() as directory:
            profile = pathlib.Path(directory) / "profiles" / "case.cg"
            result = SimpleNamespace(returncode=0, stderr="==42== I refs: 7,654\n")
            with patch.object(bench_guard.subprocess, "run", return_value=result) as run:
                plain = bench_guard.instructions("coreav", ["probe", "fixture.mp4"])
                captured = bench_guard.instructions("coreav", ["probe", "fixture.mp4"], profile=profile)
            self.assertEqual(plain, (7654, None))
            self.assertEqual(captured, plain)
            before, after = run.call_args_list
            self.assertEqual(before.kwargs, after.kwargs)
            expected = list(before.args[0])
            expected[3] = f"--cachegrind-out-file={profile}"
            self.assertEqual(after.args[0], expected)
            self.assertTrue(profile.parent.is_dir())

    def test_exact_boundary_and_adjacent_counts(self):
        for name in ("start-up", "decoder", "case"):
            for count, expected in ((94_999, "faster: the baseline can be lowered"),
                                    (95_000, "ok"), (95_001, "ok"),
                                    (104_999, "ok"), (105_000, "ok"),
                                    (105_001, "SLOWER")):
                with self.subTest(case=name, count=count):
                    failed = []
                    if name == "start-up":
                        _, status = bench_guard.compare(name, count, 100_000, 0.05, failed)
                    else:
                        before_startup = 20_000 if name == "case" else None
                        _, status, _ = bench_guard.compare_case(name, count, 100_000,
                                                               count + (before_startup or 0),
                                                               before_startup, 0.05, failed)
                    self.assertEqual(status, "SLOWER, and so is the whole command"
                                     if expected == "SLOWER" and name == "case" else expected)
                    self.assertEqual(failed, [name] if expected == "SLOWER" else [])


class FfmpegGuard(unittest.TestCase):
    def test_required_refusals_are_only_the_two_approved_commands(self):
        self.assertEqual(bench_guard.REQUIRED_REFUSALS,
                         {"ffmpeg-C02-join", "ffmpeg-C18-clip-input", "ffmpeg-C19-clip-audio-input"})
        self.assertEqual(bench_guard.CASES["ffmpeg-C02-join"],
                         ("video.mp4", ["ffmpeg", "-f", "concat", "-i", "{inputs:concat}",
                                        "-c", "copy", "{out}/out.mp4"]))
        self.assertEqual(bench_guard.CASES["ffmpeg-C18-clip-input"],
                         ("basic.mp4", ["ffmpeg", "-ss", "0.5", "-i", "{}", "-to", "1.5",
                                        "-c", "copy", "{out}/out.mp4"]))
        self.assertEqual(bench_guard.CASES["ffmpeg-C19-clip-audio-input"],
                         ("aac.m4a", ["ffmpeg", "-ss", "0.5", "-i", "{}", "-t", "1",
                                      "-c:a", "copy", "{out}/out.mp4"]))

    def test_required_refusal_proves_exit_preservation_and_real_fallback(self):
        for fault in (None, "native", "stdout", "destination", "staging", "pin", "status", "bytes"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as directory:
                output = pathlib.Path(directory) / "out.mp4"
                command = ["ffmpeg", "-ss", "0.5", "-i", "in.mp4", "-to", "1.5", "-c", "copy", str(output)]
                strict_calls = []

                def run(argv, **kwargs):
                    self.assertNotIn("COREAV_STRICT_SAFE", kwargs["env"])
                    self.assertNotIn("COREAV_FALLBACK_ACTIVE", kwargs["env"])
                    self.assertIn("-y", argv)
                    self.assertEqual(kwargs["timeout"], 120)
                    if "--strict-safe" in argv:
                        strict_calls.append(output.exists())
                        if fault == "destination" and output.exists():
                            output.write_bytes(b"changed")
                        if fault == "staging":
                            output.with_name("stage").write_bytes(b"partial")
                        return SimpleNamespace(returncode=0 if fault == "native" else 4,
                                               stdout=b"unexpected" if fault == "stdout" else b"")
                    if argv[0] == "/pin/ffmpeg":
                        self.assertEqual(argv, ["/pin/ffmpeg", "-nostdin", "-hide_banner", "-y", *command[1:]])
                        output.write_bytes(b"exact pinned output")
                        return SimpleNamespace(returncode=1 if fault == "pin" else 0, stdout=b"")
                    self.assertEqual(argv, ["coreav", "ffmpeg", "-y", *command[1:]])
                    self.assertEqual(kwargs["env"]["COREAV_FALLBACK"], "/pin/ffmpeg")
                    output.write_bytes(b"wrong" if fault == "bytes" else b"exact pinned output")
                    return SimpleNamespace(returncode=1 if fault == "status" else 0, stdout=b"")

                with patch.object(bench_guard.test_oracle, "require", return_value=SimpleNamespace(ffmpeg=pathlib.Path('/pin/ffmpeg'))), \
                     patch.object(bench_guard.subprocess, "run", side_effect=run):
                    error = bench_guard.required_refusal("coreav", command)
                if fault is None:
                    self.assertIsNone(error)
                    self.assertEqual(strict_calls, [False, True])
                else:
                    self.assertIsNotNone(error)
                self.assertFalse(output.exists())
                self.assertFalse(output.with_name("reference.mp4").exists())

    def test_required_refusals_are_not_native_instruction_counts(self):
        for error in (None, "wrong fallback"):
            with self.subTest(error=error), tempfile.TemporaryDirectory() as directory:
                work = pathlib.Path(directory)
                cases = {name: bench_guard.CASES[name] for name in bench_guard.REQUIRED_REFUSALS}
                for fixture, _ in cases.values():
                    (work / fixture).write_bytes(b"fixture")
                args = SimpleNamespace(coreav="coreav", decoder_coreav=None, fixtures=work,
                                       report_only=False, write=work / "counts.json")
                output, errors = io.StringIO(), io.StringIO()
                with patch.multiple(bench_guard, CASES=cases, URL_CASES={}), \
                     patch.object(bench_guard, "derive"), patch.object(bench_guard, "generate", return_value={}), \
                     patch.object(bench_guard, "craft", return_value={}), \
                     patch.object(bench_guard, "inputs_of", return_value=[str(work / "inputs.ffconcat")]), \
                     patch.object(bench_guard, "required_refusal", return_value=error) as checked, \
                     patch.object(bench_guard, "instructions", return_value=(100, None)) as counted, \
                     contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
                    result = bench_guard.run_cases(args, {"startup": 100}, 0.05,
                                                   {name: {} for name in cases}, {}, work)
                self.assertEqual(result, 1 if error else 0)
                self.assertEqual(counted.call_count, 1)
                self.assertEqual(checked.call_count, len(cases))
                report = json.loads(args.write.read_text())
                self.assertEqual(report["cases"], {})
                self.assertEqual(set(report["required_refusals"]), set() if error else set(cases))
                self.assertIn("required refusal failed" if error else "not native or measured", output.getvalue())

    def test_required_refusal_cannot_disappear_with_its_fixture_or_old_count(self):
        with tempfile.TemporaryDirectory() as directory:
            work = pathlib.Path(directory)
            name = "ffmpeg-C18-clip-input"
            args = SimpleNamespace(coreav="coreav", decoder_coreav=None, fixtures=work,
                                   report_only=False, write=None)
            with patch.multiple(bench_guard, CASES={name: bench_guard.CASES[name]}, URL_CASES={}), \
                 patch.object(bench_guard, "derive"), patch.object(bench_guard, "generate", return_value={}), \
                 patch.object(bench_guard, "craft", return_value={}), \
                 patch.object(bench_guard, "instructions", return_value=(100, None)), \
                 contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(bench_guard.run_cases(args, {"startup": 100}, 0.05, {}, {}, work), 1)

    def test_every_selected_ffmpeg_shape_has_its_own_case(self):
        entries = {e["id"]: e for e in json.loads(release_set.OUT.read_text())["entries"] if e["tool"] == "ffmpeg"}
        found = {}
        for name, (_, command) in bench_guard.CASES.items():
            if command[0] != "ffmpeg":
                continue
            identifier = name.split("-")[1]
            self.assertNotIn(identifier, found)
            shape, _, refusal, _ = release_set.ffmpeg_shape(command[1:])
            self.assertIsNone(refusal)
            self.assertEqual(list(shape), entries[identifier]["shape"], name)
            self.assertEqual(release_set.job_of("ffmpeg", shape)[0], entries[identifier]["job"], name)
            found[identifier] = name
        self.assertEqual(set(found), set(entries))

    def test_each_ffmpeg_case_rejects_ten_percent_more_instructions(self):
        baseline = json.loads((ROOT / "bench/guard/baseline.json").read_text())
        cases = {name: case for name, case in bench_guard.CASES.items() if case[1][0] == "ffmpeg"}
        for name, case in cases.items():
            if name in bench_guard.REQUIRED_REFUSALS:
                continue
            with self.subTest(case=name), tempfile.TemporaryDirectory() as directory:
                work = pathlib.Path(directory)
                fixture = work / case[0]
                fixture.write_bytes(b"fixture")
                startup = baseline["startup"]
                before = baseline["cases"][name]["instructions"]
                expected = {name: {"instructions": before, "sha256": bench_guard.hashlib.sha256(fixture.read_bytes()).hexdigest()}}
                args = SimpleNamespace(coreav="coreav", decoder_coreav=None, fixtures=work,
                                       report_only=False, write=None, profile_dir=work / "profiles")
                for count, exit_code in ((before + math.floor(before * baseline["tolerance"]) - 1, 0),
                                         (before + math.ceil(before * 0.10), 1)):
                    output, errors = io.StringIO(), io.StringIO()
                    with patch.multiple(bench_guard, CASES={name: case}, URL_CASES={}), \
                         patch.object(bench_guard, "derive"), patch.object(bench_guard, "generate", return_value={}), \
                         patch.object(bench_guard, "craft", return_value={}), \
                         patch.object(bench_guard, "inputs_of", return_value=[str(work / "inputs.ffconcat")]), \
                         patch.object(bench_guard, "instructions", side_effect=[(startup, None), (startup + count, None)]) as measured, \
                         contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
                        result = bench_guard.run_cases(args, baseline, baseline["tolerance"], expected, {}, work)
                    self.assertEqual(result, exit_code, output.getvalue() + errors.getvalue())
                    self.assertEqual(measured.call_args_list[0].kwargs["profile"], work / "profiles/startup.cg")
                    self.assertEqual(measured.call_args.kwargs["profile"], work / "profiles" / f"{name}.cg")
                    self.assertTrue(all("{" not in arg for arg in measured.call_args.args[1]))
                    if exit_code:
                        row = next(line for line in output.getvalue().splitlines() if line.startswith(f"| {name} |"))
                        self.assertIn("SLOWER", row)
                        self.assertIn(f"beyond the baseline's tolerance: {name}", errors.getvalue())
                        self.assertNotIn("did not run", errors.getvalue())
                        print(row)

    def test_concat_list_names_the_prepared_segments_in_numeric_order(self):
        with tempfile.TemporaryDirectory() as directory:
            work = pathlib.Path(directory)

            def split(command, **_):
                pattern = command[-1]
                for number in (10, 2, 1):
                    pathlib.Path(pattern.replace("%d", str(number))).write_bytes(b"segment")
                return SimpleNamespace(returncode=0)

            with patch.object(bench_guard.subprocess, "run", side_effect=split):
                listing, = bench_guard.inputs_of("coreav", work / "basic.mp4", "concat", work)
            listing = pathlib.Path(listing)
            self.assertEqual(listing.read_text(), "ffconcat version 1.0\nfile 'seg-1.mp4'\nfile 'seg-2.mp4'\nfile 'seg-10.mp4'\n")
            for name in ("seg-1.mp4", "seg-2.mp4", "seg-10.mp4"):
                self.assertTrue((listing.parent / name).is_file())
