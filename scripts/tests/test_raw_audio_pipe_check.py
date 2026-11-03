"""The raw-audio checker never normalizes or trims an output mismatch."""

import importlib.util
from pathlib import Path
import unittest


SPEC = importlib.util.spec_from_file_location(
    "raw_audio_pipe_check", Path(__file__).resolve().parents[1] / "raw_audio_pipe_check.py")
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class ExactOutput(unittest.TestCase):
    def test_identical_bytes_pass(self):
        CHECK.require_equal(b"\0\x80\xff\x7f", b"\0\x80\xff\x7f", "identity")

    def test_one_step_difference_fails(self):
        with self.assertRaisesRegex(AssertionError, "complete raw bytes differ"):
            CHECK.require_equal(b"\x01\0", b"\0\0", "one S16 step")

    def test_missing_or_extra_tail_fails(self):
        for actual in [b"\0\0", b"\0\0\0\0\0\0"]:
            with self.assertRaises(AssertionError):
                CHECK.require_equal(actual, b"\0\0\0\0", "complete length")

    def test_float_signed_zero_is_not_normalized(self):
        with self.assertRaises(AssertionError):
            CHECK.require_equal(b"\0\0\0\x80", b"\0\0\0\0", "float bits")


if __name__ == "__main__":
    unittest.main()
