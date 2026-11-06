import pathlib
import sys
import types
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import speed
sys.path.insert(0, str(speed.ROOT / 'scripts'))
import test_oracle


class OracleSelection(unittest.TestCase):
    def test_main_hands_the_owner_pair_to_the_executor_tools(self):
        pair = test_oracle.Oracle(pathlib.Path('/qualified/ffmpeg'), pathlib.Path('/qualified/ffprobe'))
        tools = {'ffmpeg': '/requested/ffmpeg', 'ffprobe': '/requested/ffprobe', 'other': '/candidate/tool'}
        expected = {'inputs_sha256': 'hash'}
        with mock.patch.object(speed, 'load_plan', return_value=({'exclusions_sha256': 'hash', 'round': 4,
                                                                 'provenance_commit': 'commit'}, {})), \
                mock.patch.object(speed.capacity, 'digest', return_value='hash'), \
                mock.patch.object(speed.capacity, 'canonical', return_value='binding'), \
                mock.patch.object(speed.capacity, 'load', side_effect=[expected, expected, {}, {}, tools]), \
                mock.patch.object(speed, 'input_records', return_value={}), \
                mock.patch.object(speed, 'runtime_matches'), \
                mock.patch.object(test_oracle, 'require', return_value=pair) as qualify, \
                mock.patch('builtins.open', mock.mock_open()), \
                mock.patch.object(speed.fcntl, 'flock'), \
                mock.patch.object(speed, 'Store'), \
                mock.patch.object(speed, 'run', return_value={'complete': True}) as run:
            args = [value for name in ('plan', 'roster', 'inputs', 'exclusions', 'tools', 'media', 'work', 'out-dir')
                    for value in ('--' + name, '/synthetic/' + name)]
            self.assertEqual(speed.main([*args, '--shard', '0']), 0)
        qualify.assert_called_once_with('/requested/ffmpeg', '/requested/ffprobe')
        retained = run.call_args.args[5]
        self.assertEqual(retained, {**tools, 'ffmpeg': str(pair.ffmpeg), 'ffprobe': str(pair.ffprobe)})
        execute = types.SimpleNamespace(tools=retained, invoke=mock.Mock(return_value=types.SimpleNamespace(stderr=b'')))
        with mock.patch.object(speed.capacity_run, 'wav_info', return_value={}), \
                mock.patch.object(speed, 'call_valid', return_value=True), \
                mock.patch.object(pathlib.Path, 'unlink'):
            speed.timed_call(execute, 'ffmpeg_cli', pathlib.Path('/synthetic/input'), 0,
                             pathlib.Path('/synthetic/work'), {'channels': 2})
        self.assertEqual(execute.invoke.call_args.args[2][0], str(pair.ffmpeg))
