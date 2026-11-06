import argparse
import json
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import h264_parity_gcp as driver
import h264_parity


class Selection(unittest.TestCase):
    def test_generated_gates_execute_the_owner_returned_program(self):
        oracle = driver.test_oracle.Oracle(Path('/qualified/encoder'), Path('/qualified/probe'))
        for mode in ('ref', 'freq'):
            commands = []

            def run(command, **kwargs):
                commands.append(command)
                return mock.Mock(stderr='100,,cycles\n10,,task-clock\n')

            with self.subTest(mode=mode), \
                    mock.patch.object(driver.test_oracle, 'require', return_value=oracle) as require, \
                    mock.patch('builtins.open', mock.mock_open(read_data=json.dumps(
                        {'streams': {'reference': 1}, 'core': 0}))), \
                    mock.patch.object(driver.subprocess, 'run', side_effect=run), \
                    mock.patch.object(sys, 'argv', ['gate', mode, 'label']), \
                    mock.patch('builtins.print'):
                try:
                    exec(compile(driver.GATE1T, '<gate1t>', 'exec'), {})
                except SystemExit as error:
                    self.assertEqual(error.code, 0)
            require.assert_called_once_with('./ffmpeg', './ffprobe')
            self.assertTrue(commands)
            self.assertTrue(all(str(oracle.ffmpeg) in c and './ffmpeg' not in c for c in commands))

        args = argparse.Namespace(decoders=[('base', 'decoder')], one_thread_class=False,
                                  gate_frames=1, min_seconds=0.1)
        script = driver.job_script(args, [])
        reference = script.split("<<'PY'\n", 1)[1].split('\nPY\n', 1)[0]
        with mock.patch.object(driver.test_oracle, 'require', return_value=oracle), \
                mock.patch.object(h264_parity, 'timed', return_value=1.0) as timed, \
                mock.patch.object(sys, 'argv', ['gate', 'before']), \
                mock.patch('builtins.open', mock.mock_open(read_data='{}')):
            exec(compile(reference, '<gate>', 'exec'), {})
        self.assertTrue(timed.call_args_list)
        self.assertTrue(all(c.args[0][0] == str(oracle.ffmpeg) for c in timed.call_args_list))
