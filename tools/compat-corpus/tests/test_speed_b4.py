import json
import pathlib
import sys
import tempfile
import types
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import speed


class Failures(unittest.TestCase):
    def result(self, calls):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / 'selection.json').write_text(json.dumps([{'sha256': 'sample', 'ext': 'mp4', 'class': 'isobmff'}]))
            opts = types.SimpleNamespace(coreav='coreav', dir=tmp, out=str(root / 'result.json'), rounds=1)
            with mock.patch.object(speed, 'native_commands', return_value=[['-show_streams', 'INPUT']]), mock.patch.object(speed, 'timed', side_effect=calls):
                code = speed.run(opts)
            return code, json.loads((root / 'result.json').read_text())['results'][0]

    def test_a_failed_warmup_has_no_speed_ratio(self):
        code, row = self.result([(1, 0), (1, 1)])
        self.assertEqual(code, 1)
        self.assertEqual(row['failed']['phase'], 'warmup')
        self.assertNotIn('ratio', row)

    def test_a_timed_failure_or_timeout_has_no_speed_ratio(self):
        for failure in (1, None):
            code, row = self.result([(1, 0), (1, 0), (0.01, failure), (1, 0)])
            self.assertEqual(code, 1)
            self.assertEqual(row['failed']['phase'], 'timed')
            self.assertNotIn('ratio', row)

    def test_successful_pairs_keep_their_ratio(self):
        code, row = self.result([(1, 0), (1, 0), (2, 0), (4, 0)])
        self.assertEqual(code, 0)
        self.assertEqual(row['ratio'], 0.5)


if __name__ == '__main__':
    unittest.main()
