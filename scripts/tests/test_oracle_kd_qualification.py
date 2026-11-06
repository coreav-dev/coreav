"""Keep nested first-frame probes bound to the shared oracle owner."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_kd


class NestedProbe(unittest.TestCase):
    def test_fake_version_is_rejected_before_frame_json_enters_the_predicate(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            marker = root / 'frames-called'
            for name in ('ffmpeg', 'ffprobe'):
                executable = root / name
                executable.write_text('#!/bin/sh\nif [ "$1" = "-version" ]; then\n'
                                      f'printf "%s\\n" "{name} version 0.0.0"\nelse\n'
                                      'printf "%s\\n" frame >> "$CONTROL_FRAMES_CALLED"\n'
                                      'printf "%s\\n" \'{"frames":[{"width":12,"height":34}]}\'\nfi\n')
                executable.chmod(0o700)
            env = dict(os.environ, CI='1', COREAV_FFMPEG=str(root / 'ffmpeg'),
                       COREAV_FFPROBE=str(root / 'ffprobe'), CONTROL_FRAMES_CALLED=str(marker),
                       PATH=str(root) + os.pathsep + os.environ['PATH'])
            code = ('import sys; sys.path.insert(0, sys.argv[1]); import oracle_kd; '
                    'print(oracle_kd._first_frame_size("never-opened-media", 0))')
            result = subprocess.run([sys.executable, '-I', '-B', '-c', code, str(ROOT / 'scripts')],
                                    env=env, capture_output=True, text=True, timeout=30)
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn('stage=oracle_qualification', result.stderr)
            if sys.platform == 'darwin':
                self.assertIn('binary SHA-256 mismatch', result.stderr)
            self.assertFalse(marker.exists())

    def test_returned_probe_preserves_query_and_frame_results(self):
        pair = types.SimpleNamespace(ffmpeg=Path('/verified/ffmpeg'), ffprobe=Path('/verified/ffprobe'))
        cases = ((b'{"frames":[{"width":12,"height":34}]}', (12, 34)),
                 (b'{"frames":[{}]}', (None, None)), (b'{"frames":[]}', None),
                 (b'{}', None), (b'invalid JSON', None))
        for output, expected in cases:
            with self.subTest(output=output), \
                    patch.object(oracle_kd.test_oracle, 'require', return_value=pair) as qualify, \
                    patch.object(oracle_kd.subprocess, 'run', return_value=types.SimpleNamespace(stdout=output)) as run:
                self.assertEqual(oracle_kd._first_frame_size('input.mp4', 7), expected)
                qualify.assert_called_once_with()
                run.assert_called_once_with(
                    ['/verified/ffprobe', '-v', 'error', '-select_streams', '7', '-read_intervals', '%+#1',
                     '-show_entries', 'frame=width,height', '-of', 'json', 'input.mp4'],
                    capture_output=True, timeout=60, check=False)

    def test_probe_io_failures_stay_none_but_qualification_failure_propagates(self):
        pair = types.SimpleNamespace(ffmpeg=Path('/verified/ffmpeg'), ffprobe=Path('/verified/ffprobe'))
        for failure in (OSError('fixture'), subprocess.TimeoutExpired('/verified/ffprobe', 60)):
            with self.subTest(failure=type(failure).__name__), \
                    patch.object(oracle_kd.test_oracle, 'require', return_value=pair), \
                    patch.object(oracle_kd.subprocess, 'run', side_effect=failure):
                self.assertIsNone(oracle_kd._first_frame_size('input.mp4', 7))
        with patch.object(oracle_kd.test_oracle, 'require',
                          side_effect=oracle_kd.test_oracle.QualificationError('stage=oracle_qualification')), \
                patch.object(oracle_kd.subprocess, 'run') as run:
            with self.assertRaises(oracle_kd.test_oracle.QualificationError):
                oracle_kd._first_frame_size('input.mp4', 7)
            run.assert_not_called()
