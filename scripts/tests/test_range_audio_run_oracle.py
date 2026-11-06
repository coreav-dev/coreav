import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('range_run', Path(__file__).resolve().parents[2] / 'bench/range-audio/run.py')
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


class OracleSelection(unittest.TestCase):
    def test_main_passes_the_owner_path_to_metadata_warmup_and_trials(self):
        pair = driver.test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = root / 'inputs.json'
            inputs.write_text(json.dumps({'files': [{'sha256': 'synthetic', 'key': 'input', 'ext': 'mp4', 'size': 100}]}))

            def download(r2, entry, path):
                path.touch()
                return 0

            with mock.patch.object(sys, 'argv', ['run.py', str(inputs), str(root / 'out'), '--trials', '2']), \
                    mock.patch.object(driver.test_oracle, 'require', return_value=pair), \
                    mock.patch.object(driver, 'R2'), \
                    mock.patch.object(driver, 'download', side_effect=download), \
                    mock.patch.object(driver, 'native_extract', return_value={'document': {'stream': {'stream_index': 0}}}), \
                    mock.patch.object(driver, 'cli_run', return_value={'seconds': 0}), \
                    mock.patch.object(driver, 'sha256_file', return_value='hash'), \
                    mock.patch.object(driver, 'machine', return_value={}), \
                    mock.patch.object(driver.subprocess, 'run', return_value=types.SimpleNamespace(stdout='', returncode=0)) as execute:
                driver.main()
            commands = [call.args[0] for call in execute.call_args_list]
            oracle = [command for command in commands if command[0] != 'git' and command[0] != 'node']
            self.assertEqual(len(oracle), 4)
            self.assertEqual([command[0] for command in oracle], [str(pair.ffmpeg)] * 4)
            self.assertEqual(oracle[0], [str(pair.ffmpeg), '-version'])
