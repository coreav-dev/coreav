import importlib.util
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('stage3_run', Path(__file__).resolve().parents[2] / 'tools/stage3-report/run.py')
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


class OracleSelection(unittest.TestCase):
    def test_build_only_metadata_executes_the_owner_pair(self):
        pair = driver.test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as temporary:
            with mock.patch.object(sys, 'argv', ['run.py', '--work', temporary, '--steps', 'build']), \
                    mock.patch.object(driver.test_oracle, 'require', return_value=pair) as qualify, \
                    mock.patch.object(driver, 'run', return_value=(0, '', '')), \
                    mock.patch.object(driver, 'sha256', return_value='hash'), \
                    mock.patch.object(driver, 'save'), \
                    mock.patch.object(driver, 'extra_hostile', None), \
                    mock.patch.object(driver.subprocess, 'run', return_value=types.SimpleNamespace(stdout='version\n')) as execute:
                self.assertEqual(driver.main(), 0)
            qualify.assert_called_once_with()
            self.assertEqual([call.args[0] for call in execute.call_args_list],
                             [[str(pair.ffmpeg), '-version'], [str(pair.ffprobe), '-version'], ['mediainfo', '--Version']])

    def test_hostile_generator_receives_the_retained_path(self):
        pair = driver.test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            (work / 'fixtures').mkdir()
            (work / 'fixtures/MANIFEST').touch()
            with mock.patch.object(driver, 'run', side_effect=RuntimeError('captured')) as execute:
                with self.assertRaisesRegex(RuntimeError, 'captured'):
                    driver.step_hostile(work, types.SimpleNamespace(oracle=pair))
            self.assertEqual(execute.call_args.args[0][-1], str(pair.ffmpeg))

    def test_fixture_checkers_receive_the_retained_pair_without_changing_protected_children(self):
        pair = driver.test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)

            def run(command, **extra):
                if command[0] == 'bash':
                    command[-1].mkdir()
                    (command[-1] / 'MANIFEST').write_text('generated synthetic\n')
                return 0, '', ''

            with mock.patch.object(driver, 'run', side_effect=run), \
                    mock.patch.object(driver, 'checker', return_value={}) as checker, \
                    mock.patch.object(driver, 'hls_expected_refused', return_value=''), \
                    mock.patch.object(driver, 'probe_oracle', return_value={}), \
                    mock.patch.object(driver, 'three_way', return_value={}), \
                    mock.patch.object(driver, 'save'), \
                    mock.patch.object(driver.shutil, 'which', return_value='/other/mediainfo'):
                driver.step_fixtures(work, types.SimpleNamespace(oracle=pair))
            for call in checker.call_args_list:
                command = call.args[1]
                script = Path(command[2]).name
                if script in ('hls_check.py', 'mkv_remux_check.py'):
                    self.assertNotIn('--ffmpeg', command)
                    self.assertNotIn('--ffprobe', command)
                    continue
                self.assertEqual(command[command.index('--ffprobe') + 1], str(pair.ffprobe))
                if script not in ('compat_check.py', 'policy_check.py'):
                    self.assertEqual(command[command.index('--ffmpeg') + 1], str(pair.ffmpeg))

    def test_corpus_children_probe_with_the_retained_path(self):
        pair = driver.test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as temporary, \
                mock.patch.object(driver, 'corpus_credentials', return_value={}), \
                mock.patch.object(driver, 'run', side_effect=RuntimeError('captured')) as execute:
            with self.assertRaisesRegex(RuntimeError, 'captured'):
                driver.step_corpus(Path(temporary), types.SimpleNamespace(oracle=pair, workers=1, recheck=None))
        self.assertEqual(execute.call_args.kwargs['FFPROBE'], str(pair.ffprobe))
        self.assertEqual(execute.call_args.kwargs['FFMPEG'], str(pair.ffmpeg))

    def test_fixture_captures_override_an_inherited_pair(self):
        pair = driver.test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as temporary, \
                mock.patch.dict(driver.os.environ, {'FFMPEG': '/inherited/ffmpeg', 'FFPROBE': '/inherited/ffprobe'}), \
                mock.patch.object(driver.subprocess, 'run', return_value=types.SimpleNamespace(
                    returncode=1, stdout='', stderr='stop')) as execute:
            result = driver.probe_oracle(Path(temporary), Path(temporary), 'isobmff-adapter', 'oracle_kd.py',
                                         'oracle-isobmff', pair)
        self.assertFalse(result['passed'])
        environment = execute.call_args.kwargs['env']
        self.assertEqual((environment['FFMPEG'], environment['FFPROBE']), (str(pair.ffmpeg), str(pair.ffprobe)))
