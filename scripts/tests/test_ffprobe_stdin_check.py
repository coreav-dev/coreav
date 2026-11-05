import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ffprobe_stdin_check as check


class FailureDiagnostics(unittest.TestCase):
    def test_oracle_failure_is_closed_and_precedes_any_fixture_work(self):
        with tempfile.TemporaryDirectory() as root, \
                patch.object(sys, 'argv', ['check', '--coreav', 'native', '--oracle-dir', 'oracle',
                                          '--directory', root]), \
                patch.object(check, 'Oracle', side_effect=check.OracleFailure('private-secret')), \
                patch.object(check, 'check') as work, contextlib.redirect_stderr(io.StringIO()) as error:
            self.assertEqual(check.main(), 1)
            work.assert_not_called()
            self.assertEqual(error.getvalue(), 'FAIL oracle-qualification\n')

    def test_unknown_failure_details_never_enter_output(self):
        with tempfile.TemporaryDirectory() as root, \
                patch.object(sys, 'argv', ['check', '--coreav', 'native', '--oracle-dir', 'oracle',
                                          '--directory', root]), patch.object(check, 'Oracle'), \
                patch.object(check, 'check', side_effect=check.CheckFailure('private-secret')), \
                contextlib.redirect_stderr(io.StringIO()) as error:
            self.assertEqual(check.main(), 1)
            self.assertEqual(error.getvalue(), 'FAIL structured-stdin-check\n')


if __name__ == '__main__':
    unittest.main()
