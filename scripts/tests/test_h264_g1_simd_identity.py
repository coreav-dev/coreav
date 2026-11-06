import importlib.util
import json
from pathlib import Path
import statistics
import sys
import tempfile
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location("simd_identity", Path(__file__).parents[1] / "h264_g1_simd_identity.py")
IDENTITY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(IDENTITY)


class IdentityTests(unittest.TestCase):
    def test_noop_driver_cannot_reuse_outputs_from_an_earlier_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            case = Path(tmp) / "32x32-ramp-10-false-1"
            case.mkdir()
            for name in ("auto", "scalar"):
                for suffix in ("h264", "recon", "decoded"):
                    (case / name).with_suffix("." + suffix).write_bytes(b"x" * 6144)
            args = ["identity", "--auto-driver", sys.executable, "--scalar-driver", sys.executable, "--output", tmp]
            noop = IDENTITY.subprocess.CompletedProcess([], 0, stdout=b"", stderr=b"")
            with mock.patch.object(sys, "argv", args), mock.patch.object(IDENTITY.subprocess, "run", return_value=noop):
                with self.assertRaises(FileNotFoundError):
                    IDENTITY.main()
            self.assertFalse((case / "auto.h264").exists())
            self.assertFalse((case / "scalar.h264").exists())

    def test_reference_retains_all_pairs_and_prior_stream_identities(self):
        design = Path(__file__).parents[2] / "docs/design"
        record = json.loads((design / "h264-encoder-simd-reference.json").read_text())
        prior = json.loads((design / "h264-encoder-efficiency-reference.json").read_text())
        self.assertEqual(len(record["calls"]), 60)
        seen = set()
        for row in record["calls"]:
            key = (row["clip"], row["phase"], row["arm"], row["repeat"])
            self.assertNotIn(key, seen)
            seen.add(key)
            self.assertAlmostEqual(row["fps"], 60 / row["wall_seconds"])
            self.assertGreater(row["cpu_seconds"], 0)
            old = next(r for r in prior["calls"] if r["clip"] == row["clip"] and r["arm"] == "final" and r["qp"] == 28)
            self.assertEqual(row["stream_sha256"], old["stream_sha256"])
            self.assertEqual(row["reconstruction_sha256"], old["decoded_sha256"])
            self.assertEqual(record["inputs"][row["clip"]]["sha256"], old["binding"]["input_sha256"])
        expected = {(clip, phase, arm, repeat) for clip in "abc" for phase, arms in (("baseline-pair", ("baseline", "final")), ("scalar-control", ("scalar", "final"))) for arm in arms for repeat in range(5)}
        self.assertEqual(seen, expected)
        for clip, phases in record["summary"].items():
            for phase, arms in phases.items():
                for arm, summary in arms.items():
                    values = [r["fps"] for r in record["calls"] if r["clip"] == clip and r["phase"] == phase and r["arm"] == arm]
                    self.assertEqual(summary, {"median_fps": statistics.median(values), "min_fps": min(values), "max_fps": max(values)})
        self.assertEqual(record["binaries"]["scalar"]["features"], ["full", "scalar"])
        self.assertEqual(record["host"]["compiler_source"], "ELF .comment for each executable")
        self.assertEqual({r["rustc"] for r in record["binaries"].values()}, {record["host"]["rustc"]})

    def test_comparison_rejects_changed_stream_reconstruction_or_decoder(self):
        with tempfile.TemporaryDirectory() as tmp:
            auto, scalar = Path(tmp) / "auto", Path(tmp) / "scalar"
            for suffix in ("h264", "recon", "decoded"):
                for prefix in (auto, scalar):
                    prefix.with_suffix("." + suffix).write_bytes(b"abcd")
            self.assertEqual(len(IDENTITY.compare_outputs(auto, scalar, 4)), 3)
            for suffix in ("h264", "recon", "decoded"):
                scalar.with_suffix("." + suffix).write_bytes(b"abce")
                with self.assertRaisesRegex(ValueError, "outputs differ"):
                    IDENTITY.compare_outputs(auto, scalar, 4)
                scalar.with_suffix("." + suffix).write_bytes(b"abcd")
            with self.assertRaisesRegex(ValueError, "incomplete"):
                IDENTITY.compare_outputs(auto, scalar, 5)
            for prefix in (auto, scalar):
                prefix.with_suffix(".decoded").write_bytes(b"abce")
            with self.assertRaisesRegex(ValueError, "native decoder"):
                IDENTITY.compare_outputs(auto, scalar, 4)

    def test_generated_pictures_have_complete_planes_and_real_motion(self):
        for width, height in ((32, 32), (80, 48)):
            for pattern in ("ramp", "edge"):
                raw = IDENTITY.pictures(width, height, 4, pattern)
                size = width * height * 3 // 2
                self.assertEqual(len(raw), size * 4)
                self.assertNotEqual(raw[:size], raw[size:2 * size])
