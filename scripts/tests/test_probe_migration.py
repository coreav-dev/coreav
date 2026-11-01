"""The fixture and hostile checks exercise the ffprobe command."""
import contextlib
import importlib.util
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[2]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


compat = load('compat_check')
hostile = load('cli_hostile_check')


class ProbeMigration(unittest.TestCase):
    def test_fixture_check_runs_ffprobe_and_compares_every_emitted_key(self):
        calls = []

        def run(argv, timeout):
            calls.append(argv)
            stream = {'index': 0, 'codec_name': 'aac', 'bit_rate': '100'}
            if argv[0] == 'coreav':
                stream['bit_rate'] = '101'
            return 0, {'streams': [stream], 'format': {'nb_streams': 1}}

        with tempfile.TemporaryDirectory() as tmp:
            directory = pathlib.Path(tmp) / 'media'
            directory.mkdir()
            (directory / 'input.mp4').touch()
            report = pathlib.Path(tmp) / 'report.json'
            args = ['compat_check.py', '--coreav', 'coreav', '--media', str(directory),
                    '--report', str(report)]
            with mock.patch.object(sys, 'argv', args), mock.patch.object(compat, 'run_json', run), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(compat.main(), 1)
            self.assertEqual(calls[1], ['coreav', 'ffprobe', '-v', 'error', '-of', 'json',
                                        '-show_format', '-show_streams', '-show_entries', compat.ENTRIES,
                                        str(directory / 'input.mp4')])
            result = json.loads(report.read_text())
            self.assertEqual(result['keys']['streams.codec_name'], {'agreeing_files': 1, 'files': 1})
            self.assertEqual(result['failing_keys']['streams.bit_rate']['agreeing_files'], 0)

    def test_selection_keeps_uncommon_legacy_side_data(self):
        sections = dict(part.split('=', 1) for part in compat.ENTRIES.split(':'))
        self.assertEqual(set(sections['stream_side_data'].split(',')), {
            'rotation', 'side_data_type', 'dv_version_major', 'dv_version_minor', 'dv_profile',
            'dv_level', 'rpu_present_flag', 'el_present_flag', 'bl_present_flag',
            'dv_bl_signal_compatibility_id'})

    def test_hostile_check_keeps_native_and_ffprobe_coverage(self):
        accepted = subprocess.CompletedProcess([], 0, b'{"streams": [], "format": {}}', b'')
        with mock.patch.object(hostile.subprocess, 'run', return_value=accepted) as run:
            self.assertIsNone(hostile.check('coreav', pathlib.Path('input'), 'bounded', False))
            self.assertEqual(run.call_args.args[0], ['coreav', 'probe', 'input'])
            self.assertIsNone(hostile.check('coreav', pathlib.Path('input'), 'bounded', True))
            self.assertEqual(run.call_args.args[0], ['coreav', 'ffprobe', '-of', 'json',
                                                    '-show_error', '-show_format', '-show_streams', 'input'])
            self.assertEqual(run.call_args.kwargs['env']['COREAV_STRICT_SAFE'], '1')

    def test_fixture_comparison_disables_ffmpeg_fallback(self):
        with mock.patch.object(compat.subprocess, 'run', return_value=
                               subprocess.CompletedProcess([], 0, b'{}', b'')) as run:
            self.assertEqual(compat.run_json(['coreav', 'ffprobe']), (0, {}))
        self.assertEqual(run.call_args.kwargs['env']['COREAV_STRICT_SAFE'], '1')

    def test_ffprobe_rejection_requires_a_structured_error(self):
        for output, valid in ((b'{"error":{"code":-1,"string":"invalid"}}', True),
                              (b'{}', False), (b'bad JSON', False)):
            with self.subTest(output=output):
                rejected = subprocess.CompletedProcess([], 1, output, b'coreav ffprobe: invalid')
                with mock.patch.object(hostile.subprocess, 'run', return_value=rejected):
                    result = hostile.check('coreav', pathlib.Path('input'), 'reject', True)
                self.assertEqual(result is None, valid)

    def test_ffprobe_acceptance_only_excuses_a_native_codec_rejection(self):
        accepted = subprocess.CompletedProcess([], 0, b'{"streams": [], "format": {}}', b'')
        for kind in ('codec_not_allowed', 'invalid_structure', 'limit'):
            with self.subTest(kind=kind):
                rejected = subprocess.CompletedProcess([], 1, b'', json.dumps({
                    'schema': 'coreav.error/1', 'kind': kind}).encode())
                with mock.patch.object(hostile.subprocess, 'run', side_effect=[accepted, rejected]):
                    result = hostile.check('coreav', pathlib.Path('input'), 'reject', True)
                self.assertEqual(result, None if kind == 'codec_not_allowed'
                                 else 'accepted a file that must be rejected')


if __name__ == '__main__':
    unittest.main()
