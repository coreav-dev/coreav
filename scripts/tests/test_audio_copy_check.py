import importlib.util
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / "audio_copy_check.py"
SPEC = importlib.util.spec_from_file_location("audio_copy_check", SCRIPT)
checker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(checker)


class ConcatRefusal(unittest.TestCase):
    def test_only_the_four_h264_concat_fixtures_require_refusal(self):
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary)
            for codec in ("ac3", "eac3", "flac", "alac", "other"):
                with self.subTest(codec=codec):
                    cases = list(checker.cases(out / f"av_{codec}.mp4", out))
                    self.assertEqual(next(c[3] for c in cases if c[0] == "join"), "twice")
                    self.assertEqual(cases[-1][3], "refused" if codec != "other" else "twice")
                    self.assertTrue(all(c[3] != "refused" for c in cases[:-1]))

    def test_unexpected_native_success_wrong_reason_and_published_output_fail(self):
        for code, stdout, stderr, published in [
            (0, b"", b"", False),
            (4, b"", b"another refusal", False),
            (4, b"unexpected", checker.CONCAT_REASON, False),
            (4, b"", checker.CONCAT_REASON, True),
        ]:
            with self.subTest(code=code, stdout=stdout, stderr=stderr, published=published):
                with tempfile.TemporaryDirectory() as temporary:
                    output = Path(temporary) / "output.mp4"
                    if published:
                        output.write_bytes(b"published")
                    problems = []
                    done = subprocess.CompletedProcess([], code, stdout, stderr)
                    with mock.patch.object(checker, "run") as run:
                        checker.check_concat_refusal(None, [], output, done, problems, checker.CONCAT_REASON)
                    self.assertTrue(problems)
                    run.assert_not_called()

    def exercise(self, fault=None, reason=checker.CONCAT_REASON):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "output.mp4"
            reference = root / "output.reference.mp4"
            reference.write_bytes(b"stale pinned output")
            args = ["ffmpeg", "-v", "error", "-f", "concat", "-safe", "0", "-i", "list.txt",
                    "-c", "copy", str(output)]
            tools = SimpleNamespace(coreav="controlled-coreav", ffmpeg="controlled-ffmpeg")
            calls = []

            def run(command, env=None):
                calls.append(command)
                if len(calls) == 1:
                    self.assertEqual(command, [tools.coreav, "--strict-safe", "ffmpeg", "-y", *args[1:]])
                    if fault == "destination":
                        output.write_bytes(b"changed destination")
                    if fault == "staging":
                        (root / "leaked-stage").write_bytes(b"staged")
                    return subprocess.CompletedProcess(command, 4, b"", reason)
                if len(calls) == 2:
                    self.assertFalse(reference.exists())
                    self.assertEqual(command, [tools.ffmpeg, "-y", *args[1:-1], str(reference)])
                    if fault == "reference-failure":
                        return subprocess.CompletedProcess(command, 1, b"", b"failed")
                    if fault == "reference-missing":
                        return subprocess.CompletedProcess(command, 0, b"", b"")
                    if fault == "reference-empty":
                        reference.write_bytes(b"")
                        return subprocess.CompletedProcess(command, 0, b"", b"")
                    payload = b"existing audio-copy output" if fault == "fallback-failure" else b"complete pinned output"
                    reference.write_bytes(payload)
                    return subprocess.CompletedProcess(command, 0, b"", b"")
                self.assertEqual(command, [tools.coreav, "ffmpeg", "-y", *args[1:]])
                self.assertNotIn("COREAV_STRICT_SAFE", env)
                self.assertNotIn("COREAV_FALLBACK_ACTIVE", env)
                self.assertNotIn("COREAV_FALLBACK_LOG", env)
                self.assertEqual(env["COREAV_FALLBACK"], str(Path("/pinned/ffmpeg").resolve()))
                if fault == "fallback-failure":
                    return subprocess.CompletedProcess(command, 1, b"", b"failed")
                if fault == "fallback-missing":
                    output.unlink()
                else:
                    output.write_bytes(b"different output" if fault == "fallback-bytes" else reference.read_bytes())
                return subprocess.CompletedProcess(command, 0, b"", b"fallback notice")

            problems = []
            done = subprocess.CompletedProcess([], 4, b"", reason)
            with mock.patch.object(checker, "run", run), mock.patch.object(checker.shutil, "which", return_value="/pinned/ffmpeg"):
                checker.check_concat_refusal(tools, args, output, done, problems, reason)
            return problems, len(calls)

    def test_exact_fallback_after_preserved_destination_passes(self):
        problems, calls = self.exercise()
        self.assertEqual(problems, [])
        self.assertEqual(calls, 3)

    def test_h264_refusal_uses_the_same_complete_fallback_contract(self):
        problems, calls = self.exercise(reason=checker.CONCAT_H264_REASON)
        self.assertEqual(problems, [])
        self.assertEqual(calls, 3)

    def test_each_fixture_requires_its_own_reason(self):
        self.assertEqual(checker.CONCAT_REFUSALS, {
            "av_ac3.mp4": checker.CONCAT_REASON,
            "av_eac3.mp4": checker.CONCAT_REASON,
            "av_flac.mp4": checker.CONCAT_H264_REASON,
            "av_alac.mp4": checker.CONCAT_H264_REASON,
        })
        for expected, observed in [(checker.CONCAT_REASON, checker.CONCAT_H264_REASON),
                                   (checker.CONCAT_H264_REASON, checker.CONCAT_REASON)]:
            with tempfile.TemporaryDirectory() as temporary:
                problems = []
                done = subprocess.CompletedProcess([], 4, b"", observed)
                with mock.patch.object(checker, "run") as run:
                    checker.check_concat_refusal(None, [], Path(temporary) / "output.mp4", done, problems, expected)
                self.assertTrue(problems)
                run.assert_not_called()

    def test_destination_and_staging_mutations_fail(self):
        for fault in ("destination", "staging"):
            with self.subTest(fault=fault):
                problems, calls = self.exercise(fault)
                self.assertTrue(problems)
                self.assertEqual(calls, 1)

    def test_failed_or_missing_reference_cannot_reuse_stale_bytes(self):
        for fault in ("reference-failure", "reference-missing", "reference-empty"):
            with self.subTest(fault=fault):
                problems, calls = self.exercise(fault)
                self.assertTrue(problems)
                self.assertEqual(calls, 2)

    def test_failed_missing_or_different_fallback_fails(self):
        for fault in ("fallback-failure", "fallback-missing", "fallback-bytes"):
            with self.subTest(fault=fault):
                problems, calls = self.exercise(fault)
                self.assertTrue(problems)
                self.assertEqual(calls, 3)


if __name__ == "__main__":
    unittest.main()
