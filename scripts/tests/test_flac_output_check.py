import copy
import hashlib
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "flac_output_check.py"
spec = importlib.util.spec_from_file_location("flac_output_check", SCRIPT)
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class FlacCorrectness(unittest.TestCase):
    def test_tiny_reference_rule_excludes_empty_and_normal_outputs(self):
        for count in range(1, 16):
            self.assertEqual(check.tiny_reference_options(count), ["-frame_size", "4096"])
        for count in (0, 16, 17, 4096, 8243):
            self.assertEqual(check.tiny_reference_options(count), [])

    def test_only_the_specific_default_refusal_allows_a_tiny_reference(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "reference.flac"
            refusal = subprocess.CompletedProcess([], 234, b"", b"[flac] invalid block size: 1\n")
            with patch.object(check.subprocess, "run", return_value=refusal) as default, \
                    patch.object(check, "run", return_value=b"") as reference:
                output.write_bytes(b"partial")
                self.assertEqual(check.encode_reference("ffmpeg", "source", output, 1, [], []),
                                 ["-frame_size", "4096"])
                self.assertNotIn("-frame_size", default.call_args.args[0])
                self.assertIn("-frame_size", reference.call_args.args[0])
                self.assertFalse(output.exists())
            for code, diagnostic in [(0, b""), (1, refusal.stderr), (-11, refusal.stderr),
                                     (234, b"invalid block size: 10\n"), (234, b"other error\n")]:
                with self.subTest(code=code, diagnostic=diagnostic), \
                        patch.object(check.subprocess, "run", return_value=
                                     subprocess.CompletedProcess([], code, b"", diagnostic)), \
                        patch.object(check, "run") as reference:
                    with self.assertRaises(AssertionError):
                        check.encode_reference("ffmpeg", "source", output, 1, [], [])
                    reference.assert_not_called()
            with patch.object(check.subprocess, "run", side_effect=subprocess.TimeoutExpired("ffmpeg", 60)), \
                    patch.object(check, "run") as reference:
                with self.assertRaises(subprocess.TimeoutExpired):
                    check.encode_reference("ffmpeg", "source", output, 1, [], [])
                reference.assert_not_called()
            for count in (0, 16, 17, 4096):
                with patch.object(check.subprocess, "run") as default, \
                        patch.object(check, "run", return_value=b"") as reference:
                    self.assertEqual(check.encode_reference("ffmpeg", "source", output, count, [], []), [])
                    default.assert_not_called()
                    self.assertNotIn("-frame_size", reference.call_args.args[0])

    def test_native_gate_precedes_reference_encoding(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(check, "make_source", return_value=Path(directory) / "source"), \
                patch.object(check, "run", return_value=b""), \
                patch.object(check, "verify_native", side_effect=AssertionError("wrong native samples")), \
                patch.object(check, "encode_reference") as reference:
            with self.assertRaisesRegex(AssertionError, "wrong native samples"):
                check.check(Path("coreav"), Path("oracle"), Path(directory))
            reference.assert_not_called()

    def test_source_gate_refuses_sample_header_checksum_and_probe_mutations(self):
        samples = b"\0\x80"
        header = bytearray(b"fLaC\x80\0\0\x22" + bytes(34))
        header[8:12] = bytes.fromhex("10001000")
        header[18:26] = ((48000 << 44) | (15 << 36) | 1).to_bytes(8, "big")
        header[26:42] = hashlib.md5(samples).digest()
        encoded = header + bytearray.fromhex("fff87a0800000000000000")
        stream = {"codec_name": "flac", "sample_fmt": "s16", "sample_rate": "48000",
                  "channels": 1, "channel_layout": "mono", "bits_per_sample": 0,
                  "bits_per_raw_sample": "16", "time_base": "1/48000",
                  "duration_ts": 1, "duration": "0.000021"}
        fields = {"stream": stream, "tags": {}}
        source = copy.deepcopy(fields)
        source["stream"].update(time_base="1/1000", duration_ts=None, duration=None)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "native.flac"

            def verify(data=encoded, decoded=samples, actual=fields):
                output.write_bytes(data)
                with patch.object(check, "run", return_value=decoded), \
                        patch.object(check, "probe", side_effect=[copy.deepcopy(actual), copy.deepcopy(source)]):
                    return check.verify_native("ffmpeg", "ffprobe", "source", output, samples, 16, 1, [])

            self.assertEqual(verify(), (fields, 1))
            chained = bytearray(encoded[:42])
            chained[4] = 0
            chained.extend(b"\x84\0\0\x08" + bytes(8) + encoded[42:])
            self.assertEqual(verify(data=chained), (fields, 1))
            mutations = []
            for offset, value in [(0, 0), (8, 0), (10, 0), (25, 2), (26, 0),
                                  (42, 0), (43, 0xF9), (44, 0), (46, 1), (48, 1)]:
                bad = bytearray(encoded)
                bad[offset] = value
                mutations.append(bad)
            mutations.append(encoded[:42])
            mutations.append(chained[:45])
            bad = bytearray(chained)
            bad[45] = 255
            mutations.append(bad)
            for data in mutations:
                with self.subTest(data=data.hex()), self.assertRaises(AssertionError):
                    verify(data=data)
            with self.assertRaises(AssertionError):
                verify(decoded=b"\0\0")
            for key in check.FIELDS:
                bad = copy.deepcopy(fields)
                bad["stream"][key] = "wrong"
                with self.subTest(field=key), self.assertRaises(AssertionError):
                    verify(actual=bad)
            bad = copy.deepcopy(fields)
            bad["tags"]["title"] = "unexpected"
            with self.assertRaises(AssertionError):
                verify(actual=bad)


if __name__ == "__main__":
    unittest.main()
