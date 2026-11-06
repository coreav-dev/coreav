"""PCM output selection retains the shared owner's executable pair."""

from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import pcm_output_check as check


class OracleSelection(unittest.TestCase):
    def test_main_hands_on_the_owner_returned_pair(self):
        import contextlib
        import io
        import sys
        import test_oracle
        pair = test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary) / 'report.json'
            argv = ['pcm_output_check', '--coreav', 'coreav', '--oracle-dir', '/requested', '--out', str(out)]
            with patch.object(sys, 'argv', argv), \
                    patch.object(test_oracle, 'require', return_value=pair) as qualify, \
                    patch.object(check, 'check', return_value=[]) as selected, \
                    contextlib.redirect_stdout(io.StringIO()):
                check.main()
            qualify.assert_called_once_with(Path('/requested/ffmpeg'), Path('/requested/ffprobe'))
            self.assertIs(selected.call_args.args[1], pair)

    def test_pin_failure_precedes_output(self):
        import contextlib
        import io
        import sys
        import test_oracle
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary) / 'absent' / 'report.json'
            argv = ['pcm_output_check', '--coreav', 'coreav', '--oracle-dir', '/requested', '--out', str(out)]
            with patch.object(sys, 'argv', argv), \
                    patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('wrong pin')), \
                    patch.object(check, 'check') as selected, \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                check.main()
            self.assertEqual(error.exception.code, 2)
            selected.assert_not_called()
            self.assertFalse(out.parent.exists())


if __name__ == '__main__':
    unittest.main()
