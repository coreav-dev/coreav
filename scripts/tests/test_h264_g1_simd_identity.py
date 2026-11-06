import importlib.util
from pathlib import Path
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location("simd_identity", Path(__file__).parents[1] / "h264_g1_simd_identity.py")
IDENTITY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(IDENTITY)


class IdentityTests(unittest.TestCase):
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
