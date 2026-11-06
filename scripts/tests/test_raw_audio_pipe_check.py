"""The raw-audio checker never normalizes or trims an output mismatch."""

import importlib.util
import io
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import wave


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

    def test_difference_diagnostic_has_a_closed_reason_and_bounded_sample_bytes(self):
        with self.assertRaises(AssertionError) as caught:
            CHECK.require_equal(b"\x01" * 1000, b"\x02" * 1000, "WAVE/rate")
        message = str(caught.exception)
        self.assertIn("WAVE/rate; offset 0, lengths 1000/1000; reason=raw-bytes", message)
        self.assertIn("actual=" + "01" * 16 + " expected=" + "02" * 16, message)
        self.assertNotIn("01" * 17, message)


class WaveInputs(unittest.TestCase):
    def test_pcm_headers_and_samples_are_independently_readable(self):
        for representation, width in [("u8", 1), ("s16", 2), ("s24", 3), ("s32", 4)]:
            for channels in (1, 2):
                for frames in (0, 1, 2, 33, 4097):
                    with self.subTest(representation=representation, channels=channels, frames=frames):
                        payload = CHECK.wave_samples(representation, channels, frames)
                        self.assertEqual(len(payload), width * channels * frames)
                        encoded = CHECK.wave_bytes(payload, representation, channels, 44100,
                                                   metadata=True)
                        with wave.open(io.BytesIO(encoded), "rb") as source:
                            self.assertEqual(source.getparams()[:4], (channels, width, 44100, frames))
                            self.assertEqual(source.readframes(frames + 1), payload)

    def test_short_inputs_do_not_grow_to_fit_extrema(self):
        for representation, width in CHECK.WAVE_WIDTHS.items():
            for frames in (0, 1, 2):
                self.assertEqual(len(CHECK.wave_samples(representation, 2, frames)), frames * 2 * width)
        self.assertEqual(CHECK.wave_samples("u8", 1, 6), b"\x00\xff\x7f\x80\x81\x83")
        self.assertEqual(CHECK.wave_samples("s24", 1, 2), b"\x00\x00\x80\xff\xff\x7f")

    def test_float_rounding_boundaries_include_adjacent_values_and_signed_zero(self):
        payload = CHECK.wave_samples("f32", 1, 33)
        words = struct.unpack("<33I", payload)
        self.assertEqual(words[:4], (0, 0x80000000, 1, 0x80000001))
        for boundary in (0x37800000, 0x38400000, 0x3f7fff00):
            for sign in (0, 0x80000000):
                for value in (boundary - 1, boundary, boundary + 1):
                    self.assertIn(value | sign, words)

    def test_rf64_and_bw64_substitute_complete_sizes(self):
        payload = b"\x00\xff\x80"
        for container in ("RF64", "BW64"):
            encoded = CHECK.wave_bytes(payload, "u8", container=container)
            self.assertEqual(encoded[:20], container.encode() + b"\xff" * 4 + b"WAVEds64\x1c\0\0\0")
            self.assertEqual(struct.unpack_from("<QQQI", encoded, 20),
                             (len(encoded) - 8, 3, 3, 0))
            self.assertEqual(encoded[72:], b"data" + b"\xff" * 4 + payload + b"\0")

    def test_extensible_uses_full_guid_precision_and_conventional_or_unspecified_masks(self):
        for representation, tag, bits in [("s32", 1, 32), ("f32", 3, 32)]:
            for channels, mask in [(1, 4), (2, 3), (2, 0)]:
                encoded = CHECK.wave_bytes(b"", representation, channels, extensible=True, mask=mask)
                self.assertEqual(struct.unpack_from("<HHI", encoded, 36), (22, bits, mask))
                self.assertEqual(encoded[44:60], bytes([tag, 0, 0, 0, 0, 0, 0x10, 0,
                                                      0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]))

    def test_odd_payload_excludes_padding_and_trailing_metadata(self):
        payload = b"\x00\x80\xff"
        encoded = CHECK.wave_bytes(payload, "u8", metadata=True)
        self.assertEqual(int.from_bytes(encoded[4:8], "little"), len(encoded) - 8)
        self.assertIn(b"JUNK\x03\0\0\0\x91\x82\x73\0", encoded)
        self.assertIn(b"data\x03\0\0\0" + payload + b"\0LIST", encoded)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "samples.wav"
            path.write_bytes(encoded)
            self.assertEqual(CHECK.wav_samples(path), payload)

    def test_source_identity_preserves_integer_precision_and_float_bits(self):
        self.assertEqual(CHECK.wave_identity(b"\0\xff\x80", "u8", "s16le"),
                         struct.pack("<hhh", -32768, 32512, 0))
        self.assertEqual(CHECK.wave_identity(b"\xff\xff\x7f\x01\x00\x80", "s24", "s16le"),
                         struct.pack("<hh", 32767, -32768))
        self.assertEqual(CHECK.wave_identity(b"\xff\xff\xff\x7f", "s32", "f32le"),
                         struct.pack("<f", 1.0))
        payload = struct.pack("<II", 0x80000000, 0x00000001)
        self.assertEqual(CHECK.wave_identity(payload, "f32", "f32le"), payload)
        with self.assertRaisesRegex(ValueError, "unregistered WAVE float packing"):
            CHECK.wave_identity(struct.pack("<f", 0.5 / 32768), "f32", "s16le")


class WaveCommands(unittest.TestCase):
    def drive_case(self, *, native_mutation=None, oracle_mutation=None, output_rate=None,
                   file_mutation=None, representation="s16", forms=("f32le", "s16le")):
        payload = b"\x00\xc0" if representation == "u8" else struct.pack("<hh", -32768, 16384)
        encoded = CHECK.wave_bytes(payload, representation, metadata=True)
        expected = {"s16le": struct.pack("<hh", -32768, 16384),
                    "f32le": struct.pack("<ff", -1.0, 0.5)}
        calls = []

        def execute(command, *, data=None, env=None):
            calls.append((command, data))
            form = command[command.index("-f") + 1]
            value = expected[form]
            if str(command[0]) == "oracle-ffmpeg":
                return oracle_mutation(value) if oracle_mutation else value
            if representation == "u8" and output_rate not in (None, 48000) and form == "s16le":
                self.assertEqual(env.get("COREAV_FALLBACK"), "oracle-ffmpeg")
                self.assertNotIn("COREAV_STRICT_SAFE", env)
            self.assertEqual(command[command.index("-threads") + 1], "1")
            self.assertEqual(data, encoded if command[command.index("-i") + 1] == "pipe:0" else None)
            value = native_mutation(value) if native_mutation else value
            if command[-1] == "pipe:1":
                return value
            if file_mutation:
                value = file_mutation(value)
            Path(command[-1]).write_bytes(value)
            return b""

        with tempfile.TemporaryDirectory() as directory, patch.object(CHECK, "run", side_effect=execute):
            count = CHECK.check_wave_case("coreav", "oracle-ffmpeg", Path(directory), {}, "unit",
                                          encoded, payload, representation, 1, 48000,
                                          output_rate=output_rate, forms=forms)
        return count, calls

    def test_files_stdin_explicit_and_implicit_codecs_use_complete_oracle_bytes(self):
        count, calls = self.drive_case()
        self.assertEqual(count, 12)
        native = [command for command, _ in calls if command[0] == "coreav"]
        self.assertTrue(any("-c:a" in command for command in native))
        self.assertTrue(any("-c:a" not in command for command in native))
        self.assertTrue(any(command[command.index("-i") + 1] == "pipe:0" for command in native))
        self.assertTrue(any(command[command.index("-i") + 1] != "pipe:0" for command in native))
        self.assertEqual(sum(command[-1] != "pipe:1" for command in native), 4)

    def test_one_changed_sample_or_tail_is_rejected_in_complete_command(self):
        for mutation in (lambda value: value[:-1], lambda value: value + b"\0",
                         lambda value: bytes([value[0] ^ 1]) + value[1:]):
            with self.subTest(mutation=mutation), self.assertRaisesRegex(AssertionError, "complete raw bytes differ"):
                self.drive_case(native_mutation=mutation)

    def test_stale_agreement_between_native_and_oracle_cannot_qualify_identity(self):
        mutation = lambda value: b"\0" * len(value)
        with self.assertRaisesRegex(AssertionError, "oracle/source identity"):
            self.drive_case(native_mutation=mutation, oracle_mutation=mutation)

    def test_file_bytes_are_compared_even_when_stdout_is_exact(self):
        with self.assertRaisesRegex(AssertionError, "WAVE file"):
            self.drive_case(file_mutation=lambda value: value + b"\0")

    def test_changed_rate_is_compared_to_whole_source_command_not_identity_recipe(self):
        with patch.object(CHECK, "wave_identity", side_effect=AssertionError("identity used for rate")):
            _, calls = self.drive_case(output_rate=16000)
        oracle = [command for command, _ in calls if str(command[0]) == "oracle-ffmpeg"]
        self.assertEqual(len(oracle), 2)
        for command in oracle:
            self.assertEqual(command[command.index("-ar") + 1], "16000")
            self.assertTrue(str(command[command.index("-i") + 1]).endswith(".wav"))
        with self.assertRaisesRegex(AssertionError, "complete raw bytes differ"):
            self.drive_case(output_rate=16000, native_mutation=lambda value: value[:-1])

    def test_linux_rate_roster_covers_every_representation_and_short_drain(self):
        cases = []
        def collect(*args, **kwargs):
            cases.append((args, kwargs))
            return 1
        with tempfile.TemporaryDirectory() as directory, patch.object(CHECK, "check_wave_case", side_effect=collect), \
                patch.object(CHECK, "wave_refusals", return_value=0), \
                patch.object(CHECK.sys, "platform", "linux"), patch.object(CHECK.platform, "machine", return_value="x86_64"):
            CHECK.wave_paths(Path("coreav"), CHECK.test_oracle.Oracle(
                Path("oracle/ffmpeg"), Path("oracle/ffprobe")), Path(directory))
        rate_cases = [(args, kwargs) for args, kwargs in cases if kwargs.get("output_rate") == 16000]
        self.assertFalse(any(args[7] == "f32" for args, _ in rate_cases))
        for representation in ("u8", "s16", "s24", "s32"):
            for rate in (44100, 48000):
                selected = [(args, kwargs) for args, kwargs in rate_cases
                            if args[7:10] == (representation, 1, rate)]
                self.assertEqual({len(args[6]) // CHECK.WAVE_WIDTHS[representation] for args, _ in selected},
                                 {0, 1, 2, 15, 16, 17, 31, 32, 33, 4097})
        stereo = [(args, kwargs) for args, kwargs in rate_cases if args[8] == 2]
        self.assertEqual(len(stereo), 20)
        for args, kwargs in stereo:
            self.assertEqual(args[7], "s16")
            self.assertEqual(kwargs["forms"], ("s16le",))

    def test_unsigned_s16_rate_requires_clean_refusal_and_complete_oracle_fallback(self):
        refused = []
        def refuse(command, env, data=None):
            refused.append(command)
        with patch.object(CHECK, "require_refusal", side_effect=refuse):
            count, _ = self.drive_case(representation="u8", output_rate=16000)
        self.assertEqual(count, 20)
        self.assertEqual(len(refused), 8)
        for command in refused:
            self.assertEqual(command[command.index("-f") + 1], "s16le")
        with patch.object(CHECK, "require_refusal", side_effect=refuse), \
                self.assertRaisesRegex(AssertionError, "complete raw bytes differ"):
            self.drive_case(representation="u8", output_rate=16000,
                            native_mutation=lambda value: value + b"\0", forms=("s16le",))

    def test_unsigned_rate_refusal_cannot_create_a_destination(self):
        def refuse(command, env, data=None):
            if command[-1] != "pipe:1":
                Path(command[-1]).write_bytes(b"partial")
        with patch.object(CHECK, "require_refusal", side_effect=refuse), \
                self.assertRaisesRegex(AssertionError, "rate created output"):
            self.drive_case(representation="u8", output_rate=16000)


class WaveRefusals(unittest.TestCase):
    def test_float_identity_roster_never_qualifies_another_packing(self):
        cases = []
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(CHECK, "check_wave_case", side_effect=lambda *args, **kwargs: cases.append((args, kwargs)) or 1), \
                patch.object(CHECK, "wave_refusals", return_value=0):
            CHECK.wave_paths(Path("coreav"), CHECK.test_oracle.Oracle(
                Path("oracle/ffmpeg"), Path("oracle/ffprobe")), Path(directory))
        floats = [(args, kwargs) for args, kwargs in cases if args[7] == "f32"]
        self.assertTrue(floats)
        for _, kwargs in floats:
            self.assertEqual(kwargs["forms"], ("f32le",))
            self.assertIsNone(kwargs.get("output_rate"))

    def test_refusal_requires_status_four_and_no_stdout(self):
        for status, output in [(0, b""), (1, b""), (4, b"partial audio")]:
            with self.subTest(status=status, output=output), patch.object(
                    CHECK.subprocess, "run", return_value=subprocess.CompletedProcess([], status, output, b"")):
                with self.assertRaisesRegex(AssertionError, "refuse cleanly"):
                    CHECK.require_refusal(["coreav"], {})

    def test_replay_rejects_modified_arguments_bytes_and_status(self):
        command = ["coreav", "ffmpeg", "-threads", "1", "-i", "pipe:0", "-f", "s16le", "pipe:1"]
        payload = CHECK.wave_bytes(b"\x80", "u8", metadata=True)
        expected = b"-hide_banner\n-threads\n1\n-i\npipe:0\n-f\ns16le\npipe:1\n" + payload
        for status, output, accepted in [(7, expected, True), (0, expected, False),
                                          (7, expected[:-1], False),
                                          (7, expected.removeprefix(b"-hide_banner\n"), False),
                                          (7, expected.replace(b"pipe:0", b"pipe:9"), False),
                                          (7, expected[:-1] + b"\xff", False)]:
            with self.subTest(status=status, output=output[-4:]), patch.object(
                    CHECK.subprocess, "run", return_value=subprocess.CompletedProcess(command, status, output, b"")):
                if accepted:
                    CHECK.require_replay(command, payload, {})
                else:
                    with self.assertRaisesRegex(AssertionError, "original-byte replay"):
                        CHECK.require_replay(command, payload, {})

    def test_refused_sources_never_create_or_replace_a_destination(self):
        def refuse(command, *, input=None, capture_output=None, env=None, timeout=None):
            if "COREAV_STRICT_SAFE" not in env:
                output = ("-hide_banner\n" + "\n".join(str(arg) for arg in command[2:]) + "\n").encode() + input
                return subprocess.CompletedProcess(command, 7, output, b"")
            return subprocess.CompletedProcess(command, 4, b"", b"unsupported")

        with tempfile.TemporaryDirectory() as directory, patch.object(CHECK.subprocess, "run", side_effect=refuse):
            root = Path(directory)
            self.assertGreater(CHECK.wave_refusals(Path("coreav"), root, {"COREAV_STRICT_SAFE": "1"}), 100)
            floats = (root / "wave-refused-nonfinite-7fc12345.wav").read_bytes()
            self.assertGreater(floats.index(struct.pack("<I", 0x7fc12345)), 65536)
            empty = (root / "wave-refused-empty-tail-u8-RIFF.wav").read_bytes()
            data = empty.index(b"data")
            self.assertEqual(empty[data + 4:data + 8], b"\0" * 4)
            self.assertGreater(len(empty), data + 8)
            self.assertEqual((root / "wave-refused-float-extrema.wav").read_bytes()[-8:],
                             struct.pack("<II", 0x7f7fffff, 0xff7fffff))

        def write_on_refusal(command, **kwargs):
            if command[-1] != "pipe:1":
                Path(command[-1]).write_bytes(b"partial output")
            return subprocess.CompletedProcess(command, 4, b"", b"unsupported")

        with tempfile.TemporaryDirectory() as directory, patch.object(CHECK.subprocess, "run", side_effect=write_on_refusal):
            with self.assertRaisesRegex(AssertionError, "created output"):
                CHECK.wave_refusals(Path("coreav"), Path(directory), {"COREAV_STRICT_SAFE": "1"})



class OracleSelection(unittest.TestCase):
    def test_main_hands_the_returned_pair_to_every_driver(self):
        import contextlib
        import io
        import test_oracle
        pair = test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        argv = ['raw_audio_pipe_check', '--coreav', 'coreav', '--oracle-bin', '/requested', '--smoke']
        with patch.object(CHECK.sys, 'argv', argv), \
                patch.object(test_oracle, 'require', return_value=pair) as qualify, \
                patch.object(CHECK, 'check', return_value=1) as check, \
                patch.object(CHECK, 'conformance_paths', return_value=2) as conformance, \
                patch.object(CHECK, 'wave_paths', return_value=3) as wave, \
                contextlib.redirect_stdout(io.StringIO()):
            CHECK.main()
        qualify.assert_called_once_with(Path('/requested/ffmpeg'), Path('/requested/ffprobe'))
        for driver in (check, conformance, wave):
            self.assertIs(driver.call_args.args[1], pair)

    def test_pin_failure_precedes_temporary_output_and_execution(self):
        import test_oracle
        argv = ['raw_audio_pipe_check', '--coreav', 'coreav', '--oracle-bin', '/requested']
        with patch.object(CHECK.sys, 'argv', argv), \
                patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('wrong pin')), \
                patch.object(CHECK.tempfile, 'TemporaryDirectory') as output, \
                patch.object(CHECK, 'run') as run, self.assertRaisesRegex(SystemExit, 'wrong pin'):
            CHECK.main()
        output.assert_not_called()
        run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
