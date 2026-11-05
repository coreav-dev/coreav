"""Exercise generated probe parity checks without installed media tools."""

import contextlib
import io
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import wave

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import standalone_audio_probe_check as check


class StandaloneAudioProbe(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def test_integer_wave_bytes_have_real_interleaved_samples(self):
        for bits in (8, 16, 24, 32):
            with self.subTest(bits=bits):
                data = check.wave(bits=bits)
                with wave.open(io.BytesIO(data)) as source:
                    self.assertEqual(source.getnchannels(), 2)
                    self.assertEqual(source.getsampwidth(), bits // 8)
                    self.assertEqual(source.getframerate(), 48000)
                    self.assertEqual(source.getnframes(), 480)
                    first = source.readframes(1)
                if bits == 8:
                    self.assertEqual(first, bytes((65, 96)))
                else:
                    self.assertEqual(int.from_bytes(first[:bits // 8], "little", signed=True),
                                     -63 * (1 << (bits - 8)))
                    self.assertEqual(int.from_bytes(first[bits // 8:], "little", signed=True),
                                     -32 * (1 << (bits - 8)))

    def test_special_wave_headers_store_sizes_endianness_and_padding(self):
        data = check.wave(endian=">")
        self.assertEqual(data[:4], b"RIFX")
        self.assertEqual(struct.unpack(">I", data[4:8])[0], len(data) - 8)
        self.assertEqual(struct.unpack(">HHIIHH", data[20:36]), (1, 2, 48000, 192000, 4, 16))
        self.assertEqual(data[44:48], struct.pack(">hh", -16128, -8192))
        for signature in (b"RF64", b"BW64"):
            data = check.wave(container=signature)
            self.assertEqual(data[:8], signature + b"\xff" * 4)
            self.assertEqual(data[12:16], b"ds64")
            self.assertEqual(struct.unpack("<QQQI", data[20:48]), (len(data) - 8, 1920, 480, 0))
            self.assertEqual(data[76:80], b"\xff" * 4)
        fixtures = dict(check.wave_fixtures())
        odd = fixtures["wav-u8-mono-odd-data"]
        self.assertEqual(struct.unpack("<I", odd[40:44])[0], 257)
        self.assertEqual(len(odd), 302)
        short = fixtures["wav-truncated-data"]
        self.assertEqual(struct.unpack("<I", short[40:44])[0], 1920)
        self.assertEqual(len(short) - 44, 960)

    def test_extensible_header_retains_valid_bits_mask_and_subformat(self):
        data = check.wave(tag=1, bits=32, valid_bits=24, mask=3)
        self.assertEqual(struct.unpack("<HH", data[20:24]), (0xFFFE, 2))
        self.assertEqual(struct.unpack("<HHI", data[36:44]), (22, 24, 3))
        self.assertEqual(data[44:60], bytes.fromhex("0100000000001000800000aa00389b71"))

    def test_flac_metadata_rewrite_preserves_streaminfo_and_frame_bytes(self):
        info, frames = bytes(range(34)), b"\xff\xf8frame-bytes"
        original = check.flac(info, frames, ((1, b"\0" * 7),))
        actual_info, actual_frames = check.split_flac(original)
        self.assertEqual(actual_info, info)
        self.assertEqual(actual_frames, frames)
        rewritten = check.flac(actual_info, actual_frames)
        self.assertEqual(rewritten, b"fLaC\x80\0\0\x22" + info + frames)
        for truncated in (original[:5], original[:41]):
            with self.subTest(truncated=truncated), self.assertRaises(check.CheckFailure):
                check.split_flac(truncated)

    def test_generation_is_rerunnable_and_does_not_reuse_encoded_metadata(self):
        commands = []

        def encoder(command, **kwargs):
            commands.append(command)
            if Path(command[0]).name == "ffmpeg":
                self.assertEqual(command[command.index("-i") - len(check.AUDIO_DECODE_INPUT):command.index("-i")],
                                 list(check.AUDIO_DECODE_INPUT))
                Path(command[-1]).write_bytes(check.flac(bytes(range(34)), b"\xff\xf8frame",
                                                       ((4, check.comments(("encoder=discard",))),)))
            return subprocess.CompletedProcess(command, 0, b"accepted", b"")

        (self.root / "unrelated.txt").write_text("keep")
        with patch.object(check, "run", side_effect=encoder):
            first = check.generate(Path("oracle"), self.root)
            contents = {path.name: path.read_bytes() for path in first}
            second = check.generate(Path("oracle"), self.root)
        self.assertEqual(len(first), 45)
        self.assertEqual(len(set(first)), len(first))
        self.assertEqual(contents, {path.name: path.read_bytes() for path in second})
        self.assertEqual((self.root / "unrelated.txt").read_text(), "keep")
        self.assertNotIn(b"encoder=discard", b"".join(contents.values()))
        self.assertTrue(contents["flac-empty-id3-prefix.flac"].startswith(b"ID3\x04\0\0\0\0\0\0fLaC"))
        self.assertEqual(len([command for command in commands if Path(command[0]).name == "ffmpeg"]), 10)

    def test_identical_stdout_uses_both_writers_same_filename_and_strict_safe(self):
        calls = []

        def command(args, **kwargs):
            calls.append((args, kwargs))
            return subprocess.CompletedProcess(args, 0, b"complete output\n", b"")

        source = self.root / "file.wav"
        with patch.object(check, "run", side_effect=command), patch.dict(os.environ, {
                "COREAV_STRICT_SAFE": "0", "COREAV_FALLBACK": "bad", "COREAV_FALLBACK_ACTIVE": "1"}):
            self.assertEqual(check.compare(Path("coreav"), Path("oracle"), [source]), 2)
        self.assertEqual([args[args.index("-of") + 1] for args, _ in calls], ["default", "default", "json", "json"])
        for oracle_call, native_call in ((calls[0], calls[1]), (calls[2], calls[3])):
            self.assertEqual(oracle_call[0][-1], native_call[0][-1])
            args, options = native_call
            self.assertEqual(args[:3], [Path("coreav"), "--strict-safe", "ffprobe"])
            self.assertEqual(options["env"]["COREAV_STRICT_SAFE"], "1")
            self.assertNotEqual(options["env"]["COREAV_FALLBACK"], "bad")
            self.assertNotIn("COREAV_FALLBACK_ACTIVE", options["env"])

    def test_default_and_json_whitespace_and_trailing_bytes_are_not_normalized(self):
        for writer, actual in (("default", b"same output"), ("json", b'{"a": 1}\n'),
                               ("json", b'{"a":1}\nextra')):
            with self.subTest(writer=writer, actual=actual):
                def output(args, **kwargs):
                    mode = args[args.index("-of") + 1]
                    expected = b"same output\n" if mode == "default" else b'{"a":1}\n'
                    return subprocess.CompletedProcess(args, 0,
                                                       actual if args[0] == Path("coreav") and mode == writer else expected, b"")

                with patch.object(check, "run", side_effect=output), self.assertRaisesRegex(
                        check.CheckFailure, f"file.wav \\({writer}\\): stdout differs"):
                    check.compare(Path("coreav"), Path("oracle"), [self.root / "file.wav"])

    def test_empty_successful_outputs_do_not_pass(self):
        with patch.object(check, "run", return_value=subprocess.CompletedProcess([], 0, b"", b"")), \
                self.assertRaises(check.CheckFailure):
            check.compare(Path("coreav"), Path("oracle"), [self.root / "file.wav"])

    def test_refusals_require_failure_empty_stdout_and_a_diagnostic(self):
        paths = [self.root / name for name in ("wav-rifx-s16.wav", "wav-rifx-f32.wav", "wav-partial-sample.wav")]
        for status, stdout, stderr in ((4, b"", b"unsupported"), (0, b"", b"unsupported"),
                                       (4, b"partial output", b"unsupported"), (4, b"", b"")):
            with self.subTest(status=status, stdout=stdout, stderr=stderr), patch.object(
                    check, "run", return_value=subprocess.CompletedProcess([], status, stdout, stderr)):
                if status == 4 and not stdout and stderr:
                    self.assertEqual(check.verify(Path("coreav"), Path("oracle"), paths),
                                     {"parity": 0, "refusals": 6, "divergences": 0})
                else:
                    with self.assertRaises(check.CheckFailure):
                        check.verify(Path("coreav"), Path("oracle"), paths)

    def test_known_divergence_checks_entire_reports_and_restores_the_fixture(self):
        source = self.root / "wav-extensible-s32-valid24.wav"
        original = check.wave(bits=32, mask=3, valid_bits=24)
        for fault in (None, "native-json-whitespace", "oracle-changed", "control-command"):
            with self.subTest(fault=fault):
                source.write_bytes(original)

                def output(args, **kwargs):
                    writer = args[args.index("-of") + 1]
                    control = source.read_bytes()[38:40] == b"\x20\0"
                    native = args[0] == Path("coreav")
                    if control and fault == "control-command":
                        raise check.CheckFailure("control command failed")
                    codec, fmt, duration = ("pcm_s32le", "s32", 480) if native or control else ("pcm_f24le", "flt", 640)
                    if fault == "oracle-changed" and not native and not control:
                        duration = 480
                    seconds = "0.010000" if duration == 480 else "0.013333"
                    data = (json.dumps({"streams": [{"codec_name": codec, "sample_fmt": fmt, "duration_ts": duration,
                                                     "duration": seconds, "bits_per_sample": 32,
                                                     "channels": 2, "sample_rate": "48000"}]})
                            if writer == "json" else f"codec_name={codec}\nsample_fmt={fmt}\nduration_ts={duration}\nduration={seconds}\n").encode()
                    if native and writer == "json" and fault == "native-json-whitespace":
                        data += b"\n"
                    return subprocess.CompletedProcess(args, 0, data, b"")

                with patch.object(check, "run", side_effect=output):
                    if fault is None:
                        self.assertEqual(check.verify(Path("coreav"), Path("oracle"), [source]),
                                         {"parity": 0, "refusals": 0, "divergences": 2})
                    else:
                        with self.assertRaises(check.CheckFailure):
                            check.verify(Path("coreav"), Path("oracle"), [source])
                self.assertEqual(source.read_bytes(), original)

    def test_command_failure_and_timeout_have_bounded_diagnostics(self):
        failure = subprocess.CompletedProcess([], 4, b"", b"too long " * 2000 + b"reason")
        with patch.object(subprocess, "run", return_value=failure), self.assertRaises(check.CheckFailure) as caught:
            check.run(["program"])
        self.assertIn("exit 4", str(caught.exception))
        self.assertTrue(str(caught.exception).endswith("reason"))
        self.assertLess(len(str(caught.exception)), 1700)
        with patch.object(subprocess, "run", side_effect=subprocess.TimeoutExpired("program", 30)), \
                self.assertRaisesRegex(check.CheckFailure, "timeout"):
            check.run(["program"])

    def test_pin_failure_prevents_fixture_generation(self):
        with patch.object(check.test_oracle, "verify_bin", return_value=("bad pin",)) as verify, \
                patch.object(check, "generate") as generate, contextlib.redirect_stderr(io.StringIO()) as stderr:
            result = check.main(["--coreav", "coreav", "--oracle-dir", "oracle", "--directory", str(self.root)])
        self.assertEqual(result, 1)
        self.assertIn("bad pin", stderr.getvalue())
        verify.assert_called_once_with(Path("oracle").resolve())
        generate.assert_not_called()


if __name__ == "__main__":
    unittest.main()
