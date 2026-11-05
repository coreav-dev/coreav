import importlib.util
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / 'opus_pcm_eligibility.py'
SPEC = importlib.util.spec_from_file_location('opus_pcm_eligibility', SCRIPT)
eligibility = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(eligibility)


class OpusPcmEligibility(unittest.TestCase):
    def test_profile_requires_actual_family_zero_channels_and_signed_zero_gain(self):
        for channels in (1, 2):
            config = bytearray(struct.pack('>BBHIhB', 0, channels, 312, 48000, 0, 0))
            eligibility.profile(config, 0, channels)
            for input_rate in (0, 8000, 44100, 192000):
                changed = bytearray(config)
                struct.pack_into('>I', changed, 4, input_rate)
                eligibility.profile(changed, 0, channels)
            for gain in (-32768, -1, 1, 32767):
                changed = bytearray(config)
                struct.pack_into('>h', changed, 8, gain)
                with self.assertRaises(ValueError):
                    eligibility.profile(changed, 0, channels)
            for field, value in ((0, 1), (1, 3), (10, 1), (10, 2), (10, 255)):
                changed = bytearray(config)
                changed[field] = value
                with self.assertRaises(ValueError):
                    eligibility.profile(changed, 0, channels)
            for truncated in (b'', config[:-1]):
                with self.assertRaises(ValueError):
                    eligibility.profile(truncated, 0, channels)

    def test_complete_presentation_rejects_shift_truncation_and_terminal_corruption(self):
        raw = bytes(range(32))
        eligibility.presented(raw, raw[4:12], 1, 2, 1, 4)
        eligibility.presented(raw, b'', 8, 0, 1, 4)
        for actual in (raw[:8], raw[4:8], raw[4:12] + b'\0', raw[4:11] + b'\0'):
            with self.assertRaisesRegex(ValueError, 'terminal samples'):
                eligibility.presented(raw, actual, 1, 2, 1, 4)
        with self.assertRaisesRegex(ValueError, 'outside'):
            eligibility.presented(raw, b'', 7, 2, 1, 4)

    def test_quality_is_strict_finite_and_unambiguous(self):
        self.assertEqual(eligibility.quality('quality metric: 99.8'), 99.8)
        for text in ('', 'quality metric: 90', 'quality metric: 89.99',
                     'quality metric: nan', 'quality metric: inf',
                     'quality metric: 99 quality metric: 99'):
            with self.assertRaises(ValueError):
                eligibility.quality(text)

    def test_compare_never_aligns_or_truncates_reference_lengths(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            with mock.patch.object(eligibility.subprocess, 'run') as run:
                for reference, native in ((b'\0\0', b''), (b'', b''),
                                           (b'\0' * 4, b'\0' * 2), (b'\0', b'\0')):
                    with self.assertRaises(ValueError):
                        eligibility.compare(path / 'tool', path, 'case', reference, native, 1)
                run.assert_not_called()

    def test_oracle_diagnostics_are_consumed_before_fixture_or_tool_io(self):
        def diagnostics(_):
            yield 'wrong oracle'

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'absent'
            with mock.patch.object(eligibility, 'verify_bin', diagnostics), \
                    mock.patch.object(eligibility, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'wrong oracle'):
                    eligibility.check(path, path, path, path)
                run.assert_not_called()
                self.assertFalse(path.exists())

    def test_failed_cli_removes_a_stale_report_before_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report = root / 'report.json'
            report.write_text('{"passed": true}')
            result = subprocess.run([sys.executable, '-I', str(SCRIPT), '--coreav', str(root / 'missing'),
                '--oracle-dir', str(root / 'missing'), '--opus-compare', str(root / 'missing'),
                '--work', str(root / 'work'), '--out', str(report)],
                capture_output=True, timeout=30)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(report.exists())
            self.assertFalse((root / 'work').exists())

    def test_qualification_runs_beside_the_unchanged_standard_vector_gate(self):
        workflow = (SCRIPT.parents[1] / '.github/workflows/audio-conformance.yml').read_text()
        for path in ('scripts/opus_pcm_eligibility.py', 'scripts/tests/test_opus_pcm_eligibility.py',
                     'docs/design/opus-pcm-eligibility.md'):
            self.assertEqual(workflow.count('"' + path + '"'), 2)
        job = workflow.split('\n  opus:', 1)[1]
        self.assertIn('cargo test --locked --release -p coreav-opus -- --ignored --nocapture', job)
        self.assertIn('python3 -I scripts/opus_pcm_eligibility.py', job)
        self.assertIn('--opus-compare "$RUNNER_TEMP/opus-compare/opus_compare"', job)
        self.assertIn('${{ runner.temp }}/opus-pcm-eligibility.json', job)


if __name__ == '__main__':
    unittest.main()
