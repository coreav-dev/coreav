"""Registered compatibility checks execute only the shared owner's oracle pair."""
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
import hls_check
import mkv_remux_check
import oracle_kd_mkv

PAIR = types.SimpleNamespace(ffmpeg=Path('/verified/ffmpeg'), ffprobe=Path('/verified/ffprobe'))
CALLS = {
    'hls_check.ffprobe_json': 'hls_check.ffprobe_json(["-show_streams", "never-opened-media"])',
    'hls_check.frames': 'hls_check.frames("never-opened-media", 0)',
    'hls_check.source_composition_times': 'hls_check.source_composition_times("never-opened-media", 0)',
    'mkv_remux_check.ffprobe': 'mkv_remux_check.ffprobe("never-opened-media")',
    'mkv_remux_check.framemd5': 'mkv_remux_check.framemd5("never-opened-media", 0)',
    'oracle_kd_mkv._first_frame_size': 'oracle_kd_mkv._first_frame_size("never-opened-media", 0)',
}


class RegisteredSelection(unittest.TestCase):
    def test_fake_version_is_rejected_before_any_oracle_output_is_read(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            marker = root / 'called'
            for name in ('ffmpeg', 'ffprobe'):
                executable = root / name
                executable.write_text('#!/bin/sh\nif [ "$1" = "-version" ]; then\n'
                                      f'printf "%s\\n" "{name} version 0.0.0"\nelse\n'
                                      'printf "%s\\n" called >> "$CONTROL_CALLED"\n'
                                      'printf "%s\\n" \'{"streams":[],"frames":[]}\'\nfi\n')
                executable.chmod(0o700)
            env = dict(os.environ, CI='1', COREAV_FFMPEG=str(root / 'ffmpeg'),
                       COREAV_FFPROBE=str(root / 'ffprobe'), CONTROL_CALLED=str(marker),
                       PATH=str(root) + os.pathsep + os.environ['PATH'])
            for label, call in CALLS.items():
                with self.subTest(label):
                    code = (f'import sys; sys.path.insert(0, sys.argv[1]); import {label.split(".")[0]}; '
                            f'print({call})')
                    result = subprocess.run([sys.executable, '-I', '-B', '-c', code, str(ROOT / 'scripts')],
                                            env=env, capture_output=True, text=True, timeout=60)
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertIn('stage=oracle_qualification', result.stderr)
                    self.assertFalse(marker.exists())

    def test_hls_check_executes_the_returned_pair(self):
        commands = []

        def run(command, check=True):
            commands.append(command)
            output = b'{"streams":[]}' if 'json' in command else b''
            return types.SimpleNamespace(returncode=0, stdout=output, stderr=b'')

        with patch.object(hls_check.test_oracle, 'require', return_value=PAIR), \
                patch.object(hls_check, 'run', side_effect=run), \
                patch.object(hls_check.remux_check, 'last_skip_samples', side_effect=[(0, 5), None]) as skip:
            hls_check.ffprobe_json(['-show_streams', 'input.mp4'])
            hls_check.packets('input.mp4', 0)
            hls_check.frames('input.mp4', 0)
            hls_check.source_composition_times('input.mp4', 0)
            frame = [(0, 1, 'hash')]
            packet = [{'hash': 'hash', 'duration': 1}]
            self.assertTrue(hls_check.kd_source_last_cut('input.mp4', 0, 'whole.mp4', packet, packet, frame,
                                                         [(1, 'hash')], [1], 0))
            with tempfile.TemporaryDirectory() as work:
                hls_check.kd_fragment_reading('input.mp4', 0, frame, work)
        self.assertEqual([c[0] for c in commands],
                         [str(PAIR.ffprobe)] * 3 + [str(PAIR.ffmpeg), str(PAIR.ffprobe),
                                                    str(PAIR.ffmpeg), str(PAIR.ffmpeg)])
        self.assertEqual([c.args[0] for c in skip.call_args_list], [str(PAIR.ffprobe)] * 2)

    def test_fragment_reading_propagates_qualification_failure(self):
        failure = hls_check.test_oracle.QualificationError('stage=oracle_qualification')
        with patch.object(hls_check.test_oracle, 'require', side_effect=failure), \
                patch.object(hls_check, 'run') as run, tempfile.TemporaryDirectory() as work:
            with self.assertRaises(hls_check.test_oracle.QualificationError):
                hls_check.kd_fragment_reading('input.mp4', 0, [], work)
            run.assert_not_called()

    def test_mkv_remux_check_executes_the_returned_pair(self):
        with patch.object(mkv_remux_check.test_oracle, 'require', return_value=PAIR), \
                patch.object(mkv_remux_check, 'run', return_value=(0, b'{}')) as run:
            mkv_remux_check.ffprobe('input.mkv')
            mkv_remux_check.framemd5('input.mkv', 0)
        self.assertEqual([c.args[0][0] for c in run.call_args_list], [str(PAIR.ffprobe), str(PAIR.ffmpeg)])

    def test_mkv_overlay_probe_executes_the_returned_probe(self):
        with patch.object(oracle_kd_mkv.test_oracle, 'require', return_value=PAIR), \
                patch.object(oracle_kd_mkv.subprocess, 'run',
                             return_value=types.SimpleNamespace(stdout=b'{"frames":[{"width":12,"height":34}]}')) as run:
            self.assertEqual(oracle_kd_mkv._first_frame_size('input.mkv', 3), (12, 34))
        self.assertEqual(run.call_args.args[0][0], str(PAIR.ffprobe))
        failure = oracle_kd_mkv.test_oracle.QualificationError('stage=oracle_qualification')
        with patch.object(oracle_kd_mkv.test_oracle, 'require', side_effect=failure), \
                patch.object(oracle_kd_mkv.subprocess, 'run') as run:
            with self.assertRaises(oracle_kd_mkv.test_oracle.QualificationError):
                oracle_kd_mkv._first_frame_size('input.mkv', 3)
            run.assert_not_called()
