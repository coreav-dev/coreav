import contextlib
import importlib.util
import io
import json
import pathlib
import tempfile
import types
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    'frames', pathlib.Path(__file__).resolve().parents[1] / 'frames.py')
frames = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(frames)


class OracleFailure(unittest.TestCase):
    def test_duplicate_fixture_names_are_refused_before_comparison(self):
        with tempfile.TemporaryDirectory() as tmp:
            dirs = [pathlib.Path(tmp) / name for name in ('one', 'two')]
            for directory in dirs:
                directory.mkdir()
                (directory / 'input.mp4').touch()
            args = ['frames.py', '--coreav', 'coreav', '--media', *map(str, dirs)]
            with patch.object(frames.sys, 'argv', args), \
                    patch.object(frames, 'judge', return_value={'result': 'agree'}) as judge, \
                    contextlib.redirect_stderr(io.StringIO()) as err:
                with self.assertRaises(SystemExit) as raised:
                    frames.main()
                self.assertEqual(raised.exception.code, 2)
                self.assertIn('duplicate fixture name: input.mp4', err.getvalue())
                judge.assert_not_called()

    def test_baseline_cannot_suppress_or_record_an_oracle_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            baseline = pathlib.Path(tmp) / 'baseline.json'
            saved = pathlib.Path(tmp) / 'saved.json'
            baseline.write_text(json.dumps({'failing': ['INPUT|input.mp4']}))
            args = ['frames.py', '--coreav', 'coreav', '--ffprobe', 'ffprobe',
                    '--media', tmp, '--baseline', str(baseline), '--write-baseline', str(saved)]
            for code in (None, -9):
                with self.subTest(code=code), patch.object(frames.sys, 'argv', args), \
                        patch.object(frames, 'COMMANDS', [['INPUT']]), \
                        patch.object(frames, 'media_files', return_value=[pathlib.Path('input.mp4')]), \
                        patch.object(frames, 'run', return_value=(code, b'', b'failed')), \
                        contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(frames.main(), 1)
                    self.assertEqual(json.loads(saved.read_text())['failing'], [])

    def test_timeout_and_crash_fail_instead_of_skipping_the_pair(self):
        opts = types.SimpleNamespace(ffprobe='ffprobe', coreav='coreav')
        for code in (None, -9):
            with self.subTest(code=code), patch.object(frames, 'run', return_value=(code, b'', b'failed')):
                result = frames.judge(opts, set(), ['INPUT'], pathlib.Path('input.mp4'))
                self.assertEqual(result['result'], 'fail')
                self.assertIn('oracle failed', result['stderr'])

    def test_media_rejected_by_the_oracle_is_skipped(self):
        opts = types.SimpleNamespace(ffprobe='ffprobe', coreav='coreav')
        with patch.object(frames, 'run', return_value=(1, b'', b'invalid media')):
            self.assertIsNone(frames.judge(opts, set(), ['INPUT'], pathlib.Path('input.mp4')))
