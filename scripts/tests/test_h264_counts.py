import contextlib
import io
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import h264_counts as counts


class Selection(unittest.TestCase):
    def test_both_executors_receive_the_owner_returned_program(self):
        oracle = counts.test_oracle.Oracle(Path('/qualified/encoder'), Path('/qualified/probe'))
        for original, program in (('/original/ffmpeg', oracle.ffmpeg), ('/original/ffprobe', oracle.ffprobe),
                                  ('/original/coreav', Path('/original/coreav'))):
            with self.subTest(original=original), \
                    mock.patch.object(sys, 'argv', ['counts', '--label', 'arm', '--', original, '-arg']), \
                    mock.patch.object(counts.test_oracle, 'require', return_value=oracle) as require, \
                    mock.patch.object(counts.subprocess, 'run', return_value=mock.Mock(
                        returncode=0, stderr='\n'.join(f'==1== {field}: 1' for field in counts.FIELDS))) as run, \
                    contextlib.redirect_stdout(io.StringIO()):
                counts.main()
            self.assertEqual([c.args[0] for c in run.call_args_list[:5]], [[str(program), '-arg']] * 5)
            self.assertEqual(run.call_args_list[5].args[0], [counts.VALGRIND, '--tool=cachegrind', '--branch-sim=yes',
                                                            '--cachegrind-out-file=/dev/null', str(program), '-arg'])
            if original.endswith('/coreav'):
                require.assert_not_called()
            elif original.endswith('/ffmpeg'):
                require.assert_called_once_with(original)
            else:
                require.assert_called_once_with(ffprobe=original)
