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


class FfmpegGuard(unittest.TestCase):
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
            with self.subTest(case=name), tempfile.TemporaryDirectory() as directory:
                work = pathlib.Path(directory)
                fixture = work / case[0]
                fixture.write_bytes(b"fixture")
                startup = baseline["startup"]
                before = baseline["cases"].get(name, {"instructions": 100_000})["instructions"]
                expected = {name: {"instructions": before, "sha256": bench_guard.hashlib.sha256(fixture.read_bytes()).hexdigest()}}
                args = SimpleNamespace(coreav="coreav", decoder_coreav=None, fixtures=work, report_only=False, write=None)
                for increase, exit_code in ((0, 0), (0.10, 1)):
                    count = before + math.ceil(before * increase)
                    output, errors = io.StringIO(), io.StringIO()
                    with patch.multiple(bench_guard, CASES={name: case}, URL_CASES={}), \
                         patch.object(bench_guard, "derive"), patch.object(bench_guard, "generate", return_value={}), \
                         patch.object(bench_guard, "craft", return_value={}), \
                         patch.object(bench_guard, "inputs_of", return_value=[str(work / "inputs.ffconcat")]), \
                         patch.object(bench_guard, "instructions", side_effect=[(startup, None), (startup + count, None)]), \
                         contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
                        result = bench_guard.run_cases(args, baseline, baseline["tolerance"], expected, {}, work)
                    self.assertEqual(result, exit_code, output.getvalue() + errors.getvalue())
                    if increase:
                        row = next(line for line in output.getvalue().splitlines() if line.startswith(f"| {name} |"))
                        self.assertIn("SLOWER", row)
                        self.assertIn(name, errors.getvalue())
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
