import copy
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import hevc_conformance as harness
import hevc_synthetic as runner


class SyntheticAdmission(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        self.registered = json.loads(runner.fixture.REGISTRATION.read_text())

    def test_identity_queries_execute_the_owner_returned_pair(self):
        pair = runner.test_oracle.Oracle(pathlib.Path('/qualified/ffmpeg'), pathlib.Path('/qualified/ffprobe'))
        harness.write_json(self.root / 'reference.json', self.registered)
        def capture(command):
            name = pathlib.Path(command[0]).name
            return f'{name} version {runner.fixture.oracle_pin.load()["BUILD"]}\n'.encode(), ''
        with mock.patch.object(runner.test_oracle, 'require', return_value=pair), \
                mock.patch.object(sys, 'argv', ['hevc_synthetic.py', '--data', str(self.root),
                    '--out', str(self.root / 'out'), '--parser', str(self.root / 'parser'),
                    '--oracle-bin', str(self.root / 'oracle')]), \
                mock.patch.object(runner.fixture, 'capture', side_effect=capture) as executed, \
                mock.patch.object(runner.fixture, 'file_sha256', return_value='a' * 64):
            with self.assertRaisesRegex(ValueError, 'oracle identity differs'):
                runner.main()
        self.assertEqual([call.args[0][0] for call in executed.call_args_list],
                         [str(pair.ffmpeg), str(pair.ffprobe)])

    def check(self):
        return runner.inspect(self.root, self.root / 'out', self.root / 'parser',
                              runner.test_oracle.Oracle(self.root / 'oracle/ffmpeg', self.root / 'oracle/ffprobe'), harness)

    def test_modified_preparation_cannot_supply_native_candidates(self):
        modified = copy.deepcopy(self.registered)
        modified['controls']['crossed_component_hashes_rejected'] = False
        harness.write_json(self.root / 'reference.json', modified)
        with self.assertRaisesRegex(ValueError, 'preparation differs'):
            self.check()
        self.assertFalse((self.root / 'out/census.json').exists())

    def test_changed_oracle_cannot_supply_native_candidates(self):
        harness.write_json(self.root / 'reference.json', self.registered)
        with mock.patch.object(runner.fixture, 'binary_identity', return_value={}):
            with self.assertRaisesRegex(ValueError, 'oracle identity differs'):
                self.check()
        self.assertFalse((self.root / 'out/census.json').exists())

    def test_fixture_membership_is_checked_before_opening_media(self):
        modified = copy.deepcopy(self.registered)
        modified['fixtures'][0]['id'] = '../outside'
        path = self.root / 'registered.json'
        harness.write_json(path, modified)
        harness.write_json(self.root / 'reference.json', modified)
        with mock.patch.object(runner.fixture, 'REGISTRATION', path), \
                mock.patch.object(runner.fixture, 'binary_identity', return_value=modified['oracle']), \
                mock.patch.object(runner.fixture, 'bounded_bytes') as media:
            with self.assertRaisesRegex(ValueError, 'membership differs'):
                self.check()
            media.assert_not_called()

    def test_cli_entry_points_work_in_isolated_python(self):
        scripts = pathlib.Path(__file__).resolve().parents[1]
        for name in ('hevc_fixture.py', 'hevc_synthetic.py'):
            with self.subTest(name=name):
                result = subprocess.run([sys.executable, '-I', str(scripts / name), '--help'],
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
