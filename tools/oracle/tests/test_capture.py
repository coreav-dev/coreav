"""Capture contract tests using public CLI-shaped outputs, no implementation source."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("oracle", ROOT / "scripts" / "oracle.py")
oracle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(oracle)


class CaptureTests(unittest.TestCase):
    def test_stream_duration_keeps_exact_ticks(self):
        stream = oracle.normalize_stream({"index": 0, "codec_type": "video",
            "time_base": "1/90000", "duration_ts": 3003, "duration": "0.033367"})
        self.assertEqual(stream["duration"], [1001, 30000])
        self.assertIsNone(stream["width"])
        self.assertEqual(oracle.rational("2.500000"), [5, 2])

    def test_clockwise_rotation_and_nonrotation(self):
        matrix = "\n00000000: 0 -65536 0\n00000001: 65536 0 0\n00000002: 0 0 1073741824\n"
        stream = {"side_data_list": [{"side_data_type": "Display Matrix", "rotation": 90, "displaymatrix": matrix}]}
        self.assertEqual(oracle.rotation(stream), 270)
        stream["side_data_list"][0]["displaymatrix"] = matrix.replace("65536", "131072")
        self.assertIsNone(oracle.rotation(stream))
        self.assertEqual(oracle.rotation({}), 0)

    def test_packets_pair_by_track_and_position_and_keep_signed_offsets(self):
        raw = [{"stream_index": 1, "pos": "48", "pts": 5, "dts": 3},
               {"stream_index": 0, "pos": "48", "pts": 10, "dts": 8}]
        shown = [{"stream_index": 0, "pos": "48", "pts": -2, "dts": -4, "size": "123", "flags": "KD"},
                 {"stream_index": 1, "pos": "48", "pts": 0, "dts": -2, "size": "45", "flags": "__"}]
        normalized = oracle.normalize_packets(shown, raw)
        self.assertEqual(normalized[0]["media_pts"], 10)
        self.assertEqual(normalized[0]["pts_offset"], -12)
        self.assertEqual(normalized[1]["pts_offset"], -5)
        self.assertTrue(normalized[0]["keyframe"])
        self.assertTrue(normalized[0]["discard"])
        self.assertEqual(normalized[1]["ordinal"], 0)

    def test_framemd5_keeps_source_track_and_time_base(self):
        output = b"#tb 0: 1/25\n0, 0, 2, 1, 115200, 0123456789abcdef0123456789abcdef\n"
        with patch.object(oracle, "invoke", return_value=subprocess.CompletedProcess([], 0, output, b"")) as run:
            frames, success = oracle.decode("ffmpeg", Path("sample.mp4"), [{"index": 4, "kind": "video"}], 10)
        self.assertTrue(success)
        self.assertEqual(frames[0]["stream_index"], 4)
        self.assertEqual(frames[0]["time_base"], [1, 25])
        self.assertIn("-noautorotate", run.call_args.args[0])
        self.assertEqual(oracle.decode("ffmpeg", Path("sample.mp4"), [], 10), ([], None))

    def test_manifest_rejects_missing_and_skipped_required_files(self):
        scratch = ROOT / "target" / "oracle"
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            folder = Path(directory)
            manifest = folder / "MANIFEST"
            manifest.write_text("generated absent.mp4\n")
            with self.assertRaisesRegex(RuntimeError, "missing fixture"):
                oracle.fixtures(folder)
            manifest.write_text("skipped vfr.mp4 unavailable\n")
            with self.assertRaisesRegex(RuntimeError, "required fixture skipped"):
                oracle.fixtures(folder)
            manifest.write_text("generated ../outside.mp4\n")
            with self.assertRaisesRegex(RuntimeError, "invalid or duplicate"):
                oracle.fixtures(folder)


class ShellCaptureTests(unittest.TestCase):
    def run_capture(self, folder):
        adapter = folder / "adapter.sh"
        adapter.write_text('#!/usr/bin/env bash\n'
                           'echo "$2" >> "${ORACLE_TEST_CALLS:?}"\n'
                           "printf '%s\\n' '{}'\n")
        adapter.chmod(0o755)
        return subprocess.run(
            [str(ROOT / "scripts" / "oracle-capture.sh"),
             "--media", str(folder / "media"), "--out", str(folder / "out"),
             "--adapter", str(adapter), "--levels", "packets"],
            env={**os.environ, "ORACLE_TEST_CALLS": str(folder / "calls")},
            capture_output=True, text=True, check=False,
        )

    def test_shell_manifest_rejects_unsafe_names_before_capture(self):
        scratch = ROOT / "target" / "oracle"
        scratch.mkdir(parents=True, exist_ok=True)
        invalid = ["", "..", "../outside.mp4", "nested/../outside.mp4",
                   "nested/..", "safe.mp4\ncontinuation.mp4", "safe.mp4\r"]
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            folder = Path(directory)
            media = folder / "media"
            media.mkdir()
            (media / "safe.mp4").touch()
            invalid.append(str(media / "safe.mp4"))
            for name in invalid:
                with self.subTest(name=name):
                    (media / "MANIFEST").write_text(f"generated {name}\n")
                    result = self.run_capture(folder)
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertIn("invalid manifest", result.stderr)
                    self.assertFalse((folder / "calls").exists())
                    self.assertEqual(list((folder / "out").rglob("*.json")), [])

    def test_shell_capture_rejects_newline_filename_without_manifest(self):
        scratch = ROOT / "target" / "oracle"
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            folder = Path(directory)
            media = folder / "media"
            media.mkdir()
            (media / "line\nbreak.mp4").touch()
            result = self.run_capture(folder)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn("without newlines", result.stderr)
            self.assertFalse((folder / "calls").exists())

    def test_shell_capture_accepts_relative_paths_and_unterminated_last_entry(self):
        scratch = ROOT / "target" / "oracle"
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            folder = Path(directory)
            media = folder / "media"
            (media / "nested").mkdir(parents=True)
            for name in ["nested/clip.mp4", "clip..mp4"]:
                (media / name).touch()
            (media / "MANIFEST").write_text(
                "# generated fixture list\n\ngenerated nested/clip.mp4\n"
                "skipped optional.mp4 unavailable\ngenerated clip..mp4")
            result = self.run_capture(folder)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(len((folder / "calls").read_text().splitlines()), 2)
            self.assertTrue((folder / "out" / "nested/clip.mp4.packets.json").is_file())
            self.assertTrue((folder / "out" / "clip..mp4.packets.json").is_file())


if __name__ == "__main__":
    unittest.main()
