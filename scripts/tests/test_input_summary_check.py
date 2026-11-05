import contextlib
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import input_summary_check as check


SUMMARY = (
    b"Input #0, mov,mp4, from 'generated.mp4':\n"
    b"  Metadata:\n"
    b"    title           : first line\n"
    b"                    : second line\n"
    b"  Duration: 00:00:00.40, start: 0.000000, bitrate: 123 kb/s\n"
    b"  Chapters:\n"
    b"    Chapter #0:0: start 0.000000, end 0.200000\n"
    b"      Metadata:\n"
    b"        title           : Opening\n"
    b"  Stream #0:0: Video: h264, yuv420p, 64x48, 10 fps\n"
    b"    Metadata:\n"
    b"      title           : Last stream\n"
    b"    Side data:\n"
    b"      displaymatrix: rotation of 90.00 degrees\n"
)
TERMINAL = b"At least one output file must be specified\n"


class ParameterSetMutations(unittest.TestCase):
    def test_pps_mutations_preserve_sps_suffix_samples_and_box_extents(self):
        sps = b"\x67\x64\x00\x0a\x80"
        pps = b"\x68\xce\x3c\x80"
        suffix = b"\xfd\xf8\xf8\x00"
        config = b"\x01\x64\x00\x0a\xff\xe1" + len(sps).to_bytes(2, "big") + sps
        prefix = config + b"\x01"
        config = prefix + len(pps).to_bytes(2, "big") + pps + suffix
        box = check.stsd_craft.box
        sample = box(b"mdat", b"unchanged encoded samples")
        entry = box(b"avc1", bytes(78) + box(b"avcC", config))
        track = box(b"stsd", b"\x00\x00\x00\x00\x00\x00\x00\x01" + entry)
        for kind in (b"stbl", b"minf", b"mdia", b"trak", b"moov"):
            track = box(kind, track)
        source = sample + track
        for change in (lambda _: b"\x68", check.unused_pps):
            mutated = check.rewrite_avcc(source, lambda config: check.avcc_pps(config, change))
            self.assertEqual(mutated[:len(sample)], sample)
            self.assertEqual(int.from_bytes(mutated[len(sample):len(sample) + 4], "big"),
                             len(mutated) - len(sample))
            self.assertIn(prefix, mutated)
            self.assertTrue(mutated.endswith(suffix))
            self.assertIn(change(pps), mutated)
        self.assertEqual(check.unused_pps(pps), b"\x68\x53\x8f\x20")
        self.assertEqual(check.avcc_pps(config, lambda pps: pps, suffix=False), config[:-4])


def result(stderr=SUMMARY, *, code=0, stdout=b""):
    return subprocess.CompletedProcess([], code, stdout, stderr)


class SummaryComparison(unittest.TestCase):
    def test_extract_keeps_all_metadata_and_side_data_bytes(self):
        self.assertEqual(check.extract_summary(b"demuxer warning\n" + SUMMARY, "fixture"), SUMMARY)
        self.assertEqual(check.extract_summary(SUMMARY + TERMINAL, "fixture", ffmpeg=True), SUMMARY)

    def test_missing_duplicate_truncated_or_error_tailed_summaries_fail(self):
        for output in (b"", b"noise\n", SUMMARY + SUMMARY, SUMMARY[:-1],
                       b"Input #0, mov:\n  Duration: N/A\n"):
            with self.subTest(output=output), self.assertRaises(check.CheckFailure):
                check.extract_summary(output, "fixture")

    def test_terminal_message_is_removed_only_from_the_exact_end(self):
        for ending in (b"", TERMINAL[:-1], b"At least one output must be specified\n",
                       TERMINAL + b"extra\n", TERMINAL + TERMINAL):
            with self.subTest(ending=ending), self.assertRaises(check.CheckFailure):
                check.extract_summary(SUMMARY + ending, "fixture", ffmpeg=True)

    def test_same_summary_passes_with_correct_exit_status(self):
        check.compare_results(result(), result(), "probe")
        check.compare_results(result(), result(b"oracle warning\n" + SUMMARY), "probe")
        check.compare_results(result(SUMMARY + TERMINAL, code=1),
                              result(SUMMARY + TERMINAL, code=1), "ffmpeg", ffmpeg=True)

    def test_trailing_oracle_diagnostics_are_outside_summary_but_still_exact(self):
        warning = b"Unsupported codec with id 98314 for input stream 2\n"
        self.assertEqual(check.extract_summary(SUMMARY + warning, "chapters"), SUMMARY)
        check.compare_results(result(SUMMARY + warning), result(SUMMARY + warning), "chapters")
        with self.assertRaises(check.CheckFailure):
            check.compare_results(result(), result(SUMMARY + warning), "chapters")

    def test_every_byte_matters_including_last_metadata_and_side_data(self):
        for native in (SUMMARY.replace(b"123 kb/s", b"124 kb/s"),
                       SUMMARY.replace(b"first line", b"first  line"),
                       SUMMARY.replace(b"second line", b"second line\r"),
                       SUMMARY.replace(b"Last stream", b"Last Stream"),
                       SUMMARY.replace(b"90.00", b"89.99"),
                       SUMMARY.replace(b"\n", b"\r\n"),
                       SUMMARY + b"\n", SUMMARY + b"unexpected error\n",
                       SUMMARY.split(b"    Side data:")[0]):
            with self.subTest(native=native), self.assertRaises(check.CheckFailure):
                check.compare_results(result(native), result(), "fixture")

    def test_native_banner_is_not_allowed_outside_the_summary(self):
        for prefix in (b"ffprobe version fake\n", b"ffmpeg version fake\n",
                       b"configuration: fake\n", b"  libavcodec 1.0\n"):
            with self.subTest(prefix=prefix), self.assertRaises(check.CheckFailure):
                check.compare_results(result(prefix + SUMMARY), result(), "fixture")

    def test_diagnostics_before_input_are_outside_the_summary(self):
        check.compare_results(result(b"native warning\n" + SUMMARY), result(), "fixture")

    def test_stdout_and_wrong_exit_fail_on_either_side(self):
        for broken in (result(code=4), result(stdout=b"media-content")):
            for native, oracle in ((broken, result()), (result(), broken)):
                with self.subTest(native=native), self.assertRaises(check.CheckFailure):
                    check.compare_results(native, oracle, "fixture")
        for code in (0, 2, 4):
            with self.subTest(code=code), self.assertRaises(check.CheckFailure):
                check.compare_results(result(SUMMARY + TERMINAL, code=code),
                                      result(SUMMARY + TERMINAL, code=1), "fixture", ffmpeg=True)

    def test_suppressed_log_levels_have_exact_stderr_and_status(self):
        for level in ("warning", "error", "quiet"):
            check.compare_results(result(b""), result(b""), "probe", level=level)
            expected = b"" if level == "quiet" else TERMINAL
            check.compare_results(result(expected, code=1), result(expected, code=1),
                                  "ffmpeg", ffmpeg=True, level=level)
            for output in (SUMMARY, b"warning\n", expected + b"\n"):
                with self.subTest(level=level, output=output), self.assertRaises(check.CheckFailure):
                    check.compare_results(result(output, code=1), result(expected, code=1),
                                          "ffmpeg", ffmpeg=True, level=level)

    def test_warning_diagnostics_match_exactly_without_a_summary(self):
        check.compare_results(result(b"warning\n"), result(b"warning\n"), "probe", level="warning")
        with self.assertRaises(check.CheckFailure):
            check.compare_results(result(b""), result(b"warning\n"), "probe", level="warning")
        with self.assertRaises(check.CheckFailure):
            check.compare_results(result(), result(), "probe", level="warning")

    def test_refusal_requires_exit_four_empty_stdout_and_no_partial_input(self):
        check.check_refusal(result(b"unsupported\n", code=4), "unsupported-debug")
        for broken in (result(b"unsupported\n", code=0), result(b"", code=1),
                       result(b"", code=4, stdout=b"payload"),
                       result(b"Input #0", code=4), result(b"prefix\nInput #0, mov:\n", code=4)):
            with self.subTest(broken=broken), self.assertRaises(check.CheckFailure):
                check.check_refusal(broken, "unsupported-debug")


class Commands(unittest.TestCase):
    def oracle(self):
        with patch.object(check.test_oracle, "verify_bin", return_value=[]), \
                patch.object(check.test_oracle, "verify_bin", return_value=[]):
            return check.Oracle(Path("oracle"))

    def test_platform_pin_is_verified_on_construction(self):
        for platform, module in (("darwin", check.test_oracle), ("linux", check.test_oracle)):
            with self.subTest(platform=platform), patch.object(check.sys, "platform", platform), \
                    patch.object(module, "verify_bin", return_value=[]) as verify:
                oracle = check.Oracle(Path("oracle"))
                verify.assert_called_once_with(Path("oracle").resolve())
                self.assertEqual(oracle.ffprobe, Path("oracle").resolve() / "ffprobe")

    def test_pin_failure_stops_before_generation_and_hides_verifier_content(self):
        for failure in (["/private/path arbitrary content"], OSError("/secret/path"), SystemExit("secret")):
            options = {"side_effect": failure} if isinstance(failure, BaseException) else {"return_value": failure}
            output = io.StringIO()
            with self.subTest(failure=failure), patch.object(check.sys, "platform", "darwin"), \
                    patch.object(check.test_oracle, "verify_bin", **options), \
                    patch.object(check, "generate_fixtures") as generate, \
                    contextlib.redirect_stderr(output):
                self.assertEqual(check.main(["--coreav", "coreav", "--oracle-dir", "oracle",
                                             "--directory", "fixtures"]), 1)
                generate.assert_not_called()
                self.assertEqual(output.getvalue(), "FAIL oracle-pin\n")

    def test_file_urls_reach_every_spelling_without_becoming_filesystem_paths(self):
        calls = []
        def child(command, **kwargs):
            calls.append(command)
            level = command[command.index("-loglevel") + 1] if "-loglevel" in command else None
            ffmpeg = "ffmpeg" in command or Path(command[0]).name == "ffmpeg"
            stderr = SUMMARY if level in (None, "info") else b""
            if ffmpeg and level != "quiet":
                stderr += TERMINAL
            return result(stderr, code=int(ffmpeg))
        with patch.object(check.subprocess, "run", side_effect=child):
            self.assertEqual(check.compare_file("native", self.oracle(), Path("literal%20name.mp4"),
                                               "file-url", file_url=True), [])
        self.assertEqual(len(calls), 30)
        expected = "file:" + str(Path("literal%20name.mp4").resolve())
        self.assertTrue(all(command[-1] == expected for command in calls))

    def test_unqualified_fixtures_require_both_atomic_refusal_and_exact_fallback(self):
        calls = []
        def child(command, **kwargs):
            calls.append((command, kwargs))
            if command[0] == "native" and kwargs["env"].get("COREAV_STRICT_SAFE") == "1":
                return result(b"unsupported\n", code=4)
            ffmpeg = "ffmpeg" in command or Path(command[0]).name == "ffmpeg"
            return result(SUMMARY + (TERMINAL if ffmpeg else b""), code=int(ffmpeg))
        with patch.object(check.subprocess, "run", side_effect=child), \
                patch.dict(check.os.environ, {"COREAV_STRICT_SAFE": "1"}):
            self.assertEqual(check.compare_unsupported("native", self.oracle(), Path("large.mkv"),
                                                       "large-title"), [])
        native = [(command, kwargs["env"]) for command, kwargs in calls if command[0] == "native"]
        self.assertEqual(len(native), 6)
        fallback = [(command, env) for command, env in native if "COREAV_STRICT_SAFE" not in env]
        self.assertEqual(len(fallback), 3)
        for _, env in fallback:
            self.assertEqual(env["COREAV_FALLBACK"], str(Path("oracle").resolve()))

    def test_compare_existing_file_checks_all_spellings_levels_and_strict_safe(self):
        calls = []

        def child(command, **kwargs):
            calls.append((command, kwargs))
            level = command[command.index("-loglevel") + 1] if "-loglevel" in command else None
            ffmpeg = "ffmpeg" in command or Path(command[0]).name == "ffmpeg"
            stderr = SUMMARY if level in (None, "info") else b""
            if ffmpeg and level != "quiet":
                stderr += TERMINAL
            return result(stderr, code=int(ffmpeg))

        oracle = self.oracle()
        with patch.object(check.subprocess, "run", side_effect=child), \
                patch.object(check, "generate_fixtures") as generate:
            self.assertEqual(check.compare_file(Path("native"), oracle, Path("existing.mp4"), "dev"), [])
            generate.assert_not_called()
        native = [(args, kwargs) for args, kwargs in calls if args[0] == "native"]
        self.assertEqual(len(native), 15)
        self.assertTrue(any(args == ["native", "ffprobe", str(Path("existing.mp4").resolve())]
                            for args, _ in native))
        self.assertTrue(any(args == ["native", "ffprobe", "-i", str(Path("existing.mp4").resolve())]
                            for args, _ in native))
        for args, kwargs in native:
            self.assertEqual(kwargs["env"]["COREAV_STRICT_SAFE"], "1")
            self.assertNotIn("-hide_banner", args)
            self.assertTrue(kwargs["capture_output"])
        for args, _ in calls:
            if args[0] != "native":
                self.assertIn("-hide_banner", args)

    def test_refusal_commands_are_named_and_run_strict_safe(self):
        with patch.object(check.subprocess, "run", return_value=result(b"unsupported\n", code=4)) as child:
            self.assertEqual(check.compare_refusals("native", Path("generated.mp4"), "mp4-av"), [])
        self.assertEqual(child.call_count, 6)
        self.assertEqual({call.args[0][3] for call in child.call_args_list}, {"debug", "trace"})
        for call in child.call_args_list:
            self.assertEqual(call.kwargs["env"]["COREAV_STRICT_SAFE"], "1")

    def test_failures_do_not_echo_media_paths_bytes_or_exceptions(self):
        oracle = self.oracle()
        for failure in (OSError("/private/media-secret"),
                        subprocess.TimeoutExpired(["/private/media-secret"], 30, stderr=b"secret")):
            with self.subTest(failure=failure), patch.object(check.subprocess, "run", side_effect=failure):
                failures = check.compare_file("native", oracle, Path("/private/media-secret"), "safe\ncase")
            self.assertEqual(len(failures), 15)
            for name in failures:
                self.assertRegex(name, r"\Acase-[a-z-]+\Z")
                self.assertNotIn("private", name)
                self.assertNotIn("secret", name)

    def test_generate_only_does_not_invoke_native_comparison(self):
        with patch.object(check, "Oracle"), \
                patch.object(check, "generate_fixtures", return_value=[check.Fixture("mp4-av", Path("a.mp4"))]), \
                patch.object(check, "compare_file") as compare, \
                patch.object(check, "compare_refusals") as refuse, \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertFalse(check.main(["--coreav", "native", "--oracle-dir", "oracle",
                                         "--directory", "fixtures", "--generate-only"]))
            compare.assert_not_called()
            refuse.assert_not_called()

    def test_workflow_tracks_both_imported_oracle_pin_verifiers(self):
        workflow = Path(__file__).resolve().parents[2] / '.github/workflows/input-summary.yml'
        text = workflow.read_text()
        for name in ('oracle_pin.py', 'oracle_pin_macos.py', 'stsd_craft.py'):
            self.assertIn('      - scripts/' + name + '\n', text,
                          'Restore the imported oracle verifier to the workflow path filter')

    def test_generation_is_rerunnable_and_preserves_unrelated_files(self):
        oracle = self.oracle()

        def child(command, **kwargs):
            if Path(command[0]).name == "ffprobe":
                return result(SUMMARY + b"  No Program\n      Display Matrix: rotation of 90.00 degrees\n"
                              b"      Mastering display metadata: generated\n"
                              b"      Content light level metadata: generated\n")
            Path(command[-1]).write_bytes(b"generated")
            return result(b"")

        with tempfile.TemporaryDirectory() as directory, patch.object(check.subprocess, "run", side_effect=child), \
                patch.object(check, "matroska_without_default_duration", side_effect=lambda data: data), \
                patch.object(check, "hevc_sample_types", return_value={0, 1, 20, 21, 32, 33, 34, 35, 40}), \
                patch.object(check, "rewrite_avcc", side_effect=lambda data, change: data):
            root = Path(directory)
            (root / "unrelated").write_bytes(b"leave alone")
            first = check.generate_fixtures(oracle, root, audio=True)
            second = check.generate_fixtures(oracle, root, audio=True)
            self.assertEqual(first, second)
            self.assertEqual(len(first), 50)
            by_name = {fixture.name: fixture for fixture in first}
            for name in ("mov-pcm", "ts-av", "ts-audio", "ts-programs", "ts-mp2-unassigned", "mp4-fractional-rotation", "ts-offset", "mp4-split-utf8",
                         "mp4-fragmented", "mp4-irregular-clock", "mkv-large-track-title", "mkv-analysis-packet-cap",
                         "mp4-unspecified-aspect-header-only-pps", "mp4-unspecified-aspect-truncated-pps",
                         "mp4-unspecified-aspect-unused-pps", "mp4-hevc-hev1-inband",
                         "mp4-hevc-hev1-aud", "mp4-hevc-hev1-hash", "mp4-hevc-hev1-with-audio"):
                self.assertFalse(by_name[name].native, name)
            for name in ("mov-aac", "ts-mp2", "ts-mp2-offset", "ts-mp2-programs",
                         "mp4-chapters", "mp4-hdr", "mp4-hdr-rotation", "mp4-long-metadata",
                         "mp4-unspecified-aspect-without-config-suffix", "mp4-hevc-hev1",
                         "mp4-hevc-hev1-cra", "mp4-hevc-main10", "mp4-hevc-metadata", "mp4-hevc-hvc1"):
                self.assertTrue(by_name[name].native, name)
            self.assertEqual([fixture.name for fixture in first if fixture.structured_side_data],
                             ["mp4-hdr-rotation"])
            self.assertEqual((root / "unrelated").read_bytes(), b"leave alone")
            self.assertFalse(list(root.glob(".input-summary-*")))
            for fixture in second:
                self.assertEqual(fixture.path.read_bytes(), b"generated")

    def test_hevc_fixture_types_are_read_from_all_framed_sample_units(self):
        def sample(kind):
            return b"\x00\x00\x00\x03" + bytes((kind << 1, 1, 0x80))

        for payloads, expected in (([sample(20), sample(21)], {20, 21}),
                                   ([sample(35) + sample(20), sample(40)], {20, 35, 40})):
            with patch.object(check.stsd_craft, "read_source",
                              return_value=(b"", 0, [(value, 1, 0, True) for value in payloads], (0, 0))):
                self.assertEqual(check.hevc_sample_types("generated.mp4"), expected)
        for payload in (b"\x00", b"\x00\x00\x00\x00", b"\x00\x00\x00\x09\x28\x01\x80"):
            with patch.object(check.stsd_craft, "read_source", return_value=(b"", 0, [(payload, 1, 0, True)], (0, 0))):
                with self.assertRaises(check.CheckFailure):
                    check.hevc_sample_types("generated.mp4")

    def test_structured_control_compares_complete_output_status_and_stderr(self):
        oracle = self.oracle()
        good = subprocess.CompletedProcess([], 0, b'{"streams":[{"width":64}]}\n', b'')
        with patch.object(check.subprocess, "run", side_effect=[good, good]):
            self.assertEqual(check.compare_structured("native", oracle, Path("generated.mp4"), "hevc"), [])
        for bad in (subprocess.CompletedProcess([], 0, b'{"streams":[{"width":65}]}\n', b''),
                    subprocess.CompletedProcess([], 1, good.stdout, b''),
                    subprocess.CompletedProcess([], 0, good.stdout, b'warning'),
                    subprocess.CompletedProcess([], 0, b'', b'')):
            with patch.object(check.subprocess, "run", side_effect=[bad, good]):
                self.assertEqual(check.compare_structured("native", oracle, Path("generated.mp4"), "hevc"),
                                 ["hevc-structured"])

    def test_matroska_rate_fixture_keeps_packets_but_omits_stale_indexes_and_defaults(self):
        craft = check.mkv_craft
        track = craft.element(craft.TRACK_NUMBER, b"\x01") + craft.element(craft.DEFAULT_DURATION, b"\x01")
        tracks = craft.element(craft.TRACKS, craft.element(craft.TRACK_ENTRY, track))
        cluster = craft.element(craft.CLUSTER, craft.element(craft.TIMESTAMP, b"\x00")
                                + craft.element(craft.SIMPLE_BLOCK, b"\x81\x00\x00\x80frame"))
        indexes = b''.join(craft.element(kind, b"unused") for kind in (craft.SEEK_HEAD, craft.CUES, craft.TAGS))
        data = craft.element(craft.SEGMENT, indexes + tracks + cluster)
        expected = craft.element(craft.SEGMENT, craft.element(craft.TRACKS,
            craft.element(craft.TRACK_ENTRY, craft.element(craft.TRACK_NUMBER, b"\x01"))) + cluster)
        self.assertEqual(check.matroska_without_default_duration(data), expected)

    def test_structured_side_data_rejects_reordering_status_and_diagnostics(self):
        expected = b'{"streams":[{"side_data_list":["matrix","mastering","light"]}]}\n'
        reference = result(b"", stdout=expected)
        for native, accepted in [
            (reference, True),
            (result(b"", stdout=expected.replace(b'"matrix","mastering"', b'"mastering","matrix"')), False),
            (result(b"", stdout=expected, code=4), False),
            (result(b"warning", stdout=expected), False),
            (result(b""), False),
        ]:
            with self.subTest(native=native), patch.object(check, "run", side_effect=[reference, native]) as run:
                failures = check.compare_structured_side_data(Path("native"), self.oracle(),
                                                               Path("fixture.mp4"), "combined")
                self.assertEqual(failures, [] if accepted else ["combined-structured-side-data"])
                self.assertIn("stream_side_data=side_data_type", run.call_args.args[0])

    def test_audio_matrix_requires_exact_masks_and_preserves_unsupported_cases(self):
        def generate(oracle, root):
            paths = []
            for name, data in (
                ("wav-extensible-stereo.wav", check.audio_probe.wave(mask=3)),
                ("wav-s16-stereo.wav", check.audio_probe.wave()),
                ("flac-s16-2ch-48000-minimal.flac", check.audio_probe.flac(bytes(34), b"frames")),
                ("flac-s16-2ch-32000-minimal.flac", check.audio_probe.flac(bytes(34), b"frames")),
                ("flac-s24-6ch-48000-minimal.flac", check.audio_probe.flac(bytes(34), b"frames")),
            ):
                path = root / name
                path.write_bytes(data)
                paths.append(path)
            return paths
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(check.audio_probe, "generate", side_effect=generate):
            root = Path(directory)
            (root / "unrelated").write_bytes(b"keep")
            first = check.generate_audio_fixtures(self.oracle(), root)
            self.assertEqual(first, check.generate_audio_fixtures(self.oracle(), root))
            by_name = {fixture.name: fixture for fixture in first}
            for name in ("wav-extensible-stereo", "flac-s16-2ch-48000-minimal",
                         "flac-explicit-back", "flac-explicit-side", "flac-fixed-one",
                         "flac-fixed-two", "flac-fixed-short-tail", "flac-variable-two", "flac-20bit-valid"):
                self.assertTrue(by_name[name].native, name)
            for name in ("wav-s16-stereo", "wav-empty", "wav-fact", "wav-language",
                         "flac-language", "flac-alias-collision", "flac-invalid-layout",
                         "flac-duplicate-layout", "flac-s24-6ch-48000-minimal",
                         "flac-zero-payload", "flac-single-zero-payload", "flac-bad-frame-crc",
                         "flac-truncated-frame", "flac-trailing-bytes", "flac-bad-md5", "flac-fixed-offset",
                         "flac-variable-offset", "flac-duplicate-number", "flac-number-gap",
                         "flac-mixed-strategy", "flac-reserved-header", "flac-overlong-number", "flac-20bit-padding"):
                self.assertFalse(by_name[name].native, name)
            self.assertFalse(list(root.glob(".input-summary-*")))
            self.assertEqual((root / "unrelated").read_bytes(), b"keep")

    def test_audio_generation_errors_use_closed_labels(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(check.audio_probe, "generate", side_effect=check.audio_probe.CheckFailure("private marker")):
            with self.assertRaisesRegex(check.CheckFailure, "^generate-standalone-audio$"):
                check.generate_audio_fixtures(self.oracle(), Path(directory))

    def test_workflow_checks_audio_and_tracks_its_generator(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/input-summary.yml").read_text()
        commands = [line.strip() for line in workflow.splitlines() if line.strip().startswith("run:")]
        self.assertTrue(any("scripts/input_summary_check.py" in line and "--audio" in line for line in commands))
        for path in ("crates/coreav-audio/**", "scripts/standalone_audio_probe_check.py", "scripts/mkv_craft.py", "scripts/oracle_audio.py", "scripts/oracle-audio-args.txt"):
            self.assertIn(f"      - {path}\n", workflow)

    def test_generation_failure_never_reuses_a_stale_fixture(self):
        oracle = self.oracle()
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(check.subprocess, "run", return_value=result(b"secret", code=1)):
            root = Path(directory)
            (root / "mp4-av.mp4").write_bytes(b"old")
            with self.assertRaisesRegex(check.CheckFailure, "^generate-mp4-av$"):
                check.generate_fixtures(oracle, root)
            self.assertEqual((root / "mp4-av.mp4").read_bytes(), b"old")
            self.assertFalse(list(root.glob(".input-summary-*")))


if __name__ == "__main__":
    unittest.main()
