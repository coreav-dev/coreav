import importlib.util
from pathlib import Path
import types
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('benchlib', Path(__file__).resolve().parents[2] / 'bench/shared/benchlib.py')
benchlib = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchlib)


class OracleSelection(unittest.TestCase):
    def test_version_query_executes_the_supplied_qualified_program(self):
        with mock.patch.object(benchlib.subprocess, 'run', return_value=types.SimpleNamespace(stdout='version\n')) as execute:
            self.assertEqual(benchlib.tool_versions('/qualified/ffmpeg'), {'ffmpeg': 'version', 'node': 'version'})
        self.assertEqual([call.args[0] for call in execute.call_args_list],
                         [['/qualified/ffmpeg', '-version'], ['node', '--version']])
