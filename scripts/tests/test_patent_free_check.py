import contextlib
import importlib.util
import io
import json
import pathlib
import unittest
from unittest import mock


SCRIPT = pathlib.Path(__file__).resolve().parents[1] / 'patent_free_check.py'
SPEC = importlib.util.spec_from_file_location('patent_free_check', SCRIPT)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)
HEVC = {'coreav-hevc', 'coreav-kernels-hevc', 'coreav-backend-hevc'}


class PatentFreeCheckTests(unittest.TestCase):
    def run_check(self, linked, missing=None):
        names = (set(CHECK.PATENTED) | {'coreav-cli'}) - {missing}
        metadata = json.dumps({'packages': [{'name': name} for name in names]})
        output = io.StringIO()
        with mock.patch.object(CHECK, 'cargo', return_value=metadata), \
             mock.patch.object(CHECK, 'tree', side_effect=linked), \
             mock.patch('sys.argv', [str(SCRIPT)]), contextlib.redirect_stdout(output):
            result = CHECK.main()
        return result, output.getvalue()

    def test_every_native_hevc_crate_is_forbidden_by_default(self):
        self.assertLessEqual(HEVC, set(CHECK.PATENTED))
        for name in HEVC:
            with self.subTest(name=name):
                result, output = self.run_check(
                    lambda root, args: {name} if args == ['-p', 'coreav-cli'] else set())
                self.assertEqual(result, 1)
                self.assertIn(f'coreav-cli, default features: links {name} (HEVC)', output)

    def test_release_full_refuses_each_hevc_crate(self):
        for name in HEVC:
            with self.subTest(name=name):
                result, output = self.run_check(
                    lambda root, args: {name} if args[-1] == 'release-full' else set())
                self.assertEqual(result, 1)
                self.assertIn(f'release-full without HEVC', output)
                self.assertIn(f'links {name} (HEVC)', output)

    def test_release_full_keeps_existing_h264_and_aac(self):
        result, output = self.run_check(
            lambda root, args: {'coreav-h264', 'coreav-aac'}
            if args[-1] == 'release-full' else set())
        self.assertEqual(result, 0)
        self.assertIn('0 problems', output)

    def test_missing_hevc_member_cannot_make_the_check_vacuous(self):
        for name in HEVC:
            with self.subTest(name=name):
                result, output = self.run_check(lambda root, args: set(), missing=name)
                self.assertEqual(result, 1)
                self.assertIn(f'{name} is in PATENTED but is no workspace member', output)


if __name__ == '__main__':
    unittest.main()
