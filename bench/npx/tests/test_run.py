import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('npx_run', Path(__file__).resolve().parents[1] / 'run.py')
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


class OracleSelection(unittest.TestCase):
    def test_main_uses_the_owner_pair_for_versions_and_all_reference_operations(self):
        pair = driver.test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'bin').mkdir()
            (root / 'bin/coreav').write_bytes(b'synthetic executable identity')
            source = root / 'cache/input.mp4'
            source.parent.mkdir()
            source.write_bytes(b'not media')
            inputs = root / 'inputs.json'
            inputs.write_text(json.dumps({'files': [{'sha256': 'input', 'ext': 'mp4', 'size': 9}],
                                         'source': 'synthetic', 'manifest_sha256': '', 'count': 1, 'bytes': 9}))
            programs = []

            def measure(native, launcher, op, scratch):
                name, _, _, build = op
                programs.append((name, build('out', {'streams': []})[0]))
                return {'status': 'timed', 'equal': True}

            with mock.patch.object(sys, 'argv', ['run.py', str(inputs), str(root / 'out'), '--cache', str(source.parent)]), \
                    mock.patch.dict(os.environ, COREAV_BIN='/candidate/coreav'), \
                    mock.patch.object(driver.test_oracle, 'require', return_value=pair), \
                    mock.patch.object(driver, 'install', return_value=root), \
                    mock.patch.object(driver, 'NATIVE_PACKAGE', root), \
                    mock.patch.object(driver, 'sha256_file', return_value='hash'), \
                    mock.patch.object(driver, 'write_json'), \
                    mock.patch.object(driver, 'launcher_costs', return_value={}), \
                    mock.patch.object(driver, 'run', return_value={'code': 0, 'stdout': b'{"format":{"duration":"20"}}'}), \
                    mock.patch.object(driver, 'measure_op', side_effect=measure), \
                    mock.patch.object(driver, 'append_jsonl'), \
                    mock.patch.object(driver.subprocess, 'run', return_value=types.SimpleNamespace(stdout='version\n')) as execute, \
                    mock.patch.object(driver, 'MODULE', source):
                (root / 'out').mkdir()
                (root / 'out/results.jsonl').write_text('')
                driver.main()
            self.assertIn([str(pair.ffmpeg), '-version'], [call.args[0] for call in execute.call_args_list])
            self.assertEqual(len(programs), 10)
            for name, program in programs:
                self.assertEqual(program, str(pair.ffprobe if name in ('probe', 'check', 'chapters') else pair.ffmpeg))
