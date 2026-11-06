import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('range_gaps', Path(__file__).resolve().parents[1] / 'gaps.py')
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


class OracleSelection(unittest.TestCase):
    def test_packet_query_executes_the_owner_probe(self):
        pair = driver.test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = root / 'inputs.json'
            inputs.write_text(json.dumps({'files': [{'sha256': 'synthetic', 'key': 'input', 'size': 100}]}))
            with mock.patch.object(sys, 'argv', ['gaps.py', str(inputs), str(root / 'out.json')]), \
                    mock.patch.object(driver.test_oracle, 'require', return_value=pair), \
                    mock.patch.object(driver, 'R2') as r2, \
                    mock.patch.object(driver.subprocess, 'run', return_value=types.SimpleNamespace(stdout='pos=0|size=10\n')) as execute:
                r2.return_value.presign.return_value = 'synthetic-url'
                driver.main()
            self.assertEqual(execute.call_args.args[0][0], str(pair.ffprobe))
