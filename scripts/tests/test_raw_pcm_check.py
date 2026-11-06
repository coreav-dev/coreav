"""Native-only raw PCM comparisons reject sample and metadata drift."""

from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import raw_pcm_check as check


class RawPcm(unittest.TestCase):
    def test_roster_is_complete_and_unique(self):
        cases = list(check.cases())
        self.assertEqual(len(cases), 160)
        self.assertEqual(len(set(cases)), 160)
        self.assertEqual(sum(kind == 'raw' for *_, kind in cases), 60)
        self.assertEqual(sum((incoming, outgoing) == (19400, 1400) for incoming, outgoing, *_ in cases), 8)
        self.assertTrue(all(frames or kind == 'wav' for _, _, _, _, frames, kind in cases))

    def exercise(self, wrong_sample=False, wrong_fields=False):
        case = (44100, 48000, 1, 'constant', 25, 'raw')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)

            def run(args, *, env=None):
                native = args[0] == root / 'coreav' or args[0] == str(root / 'coreav')
                if native:
                    self.assertEqual(env['COREAV_STRICT_SAFE'], '1')
                    self.assertEqual(env['COREAV_FALLBACK'], str(root / 'absent-ffmpeg'))
                else:
                    at = args.index('-i')
                    self.assertEqual(args[at - len(check.AUDIO_DECODE_INPUT):at], list(check.AUDIO_DECODE_INPUT))
                    self.assertNotIn('-dither_method', args)
                Path(args[-1]).write_bytes(b'\x00\x20' if native or not wrong_sample else b'\x01\x20')
                return b''

            with patch.object(check, 'cases', return_value=[case]), patch.object(check, 'run', side_effect=run), \
                 patch.object(check, 'fields', side_effect=[{'channels': 1}, {'channels': 2 if wrong_fields else 1}]):
                return check.check(root / 'coreav', check.test_oracle.Oracle(
                    root / 'ffmpeg', root / 'ffprobe'), root)

    def test_equal_samples_pass_without_fallback(self):
        self.assertEqual(self.exercise()[0]['result'], 'equal')

    def test_one_changed_sample_fails(self):
        with self.assertRaisesRegex(AssertionError, 'samples differ'):
            self.exercise(wrong_sample=True)

    def test_one_changed_field_fails(self):
        with self.assertRaisesRegex(AssertionError, 'fields differ'):
            self.exercise(wrong_fields=True)



class OracleSelection(unittest.TestCase):
    def test_main_hands_on_the_owner_returned_pair(self):
        import contextlib
        import io
        import test_oracle
        pair = test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary) / 'report.json'
            argv = ['raw_pcm_check', '--coreav', 'coreav', '--oracle-dir', '/requested', '--out', str(out)]
            with patch.object(sys, 'argv', argv), \
                    patch.object(test_oracle, 'require', return_value=pair) as qualify, \
                    patch.object(check, 'check', return_value=[]) as selected, \
                    patch.object(check, 'digest', return_value='digest') as digest, \
                    patch.object(check.subprocess, 'check_output', return_value='head'), \
                    contextlib.redirect_stdout(io.StringIO()):
                check.main()
            qualify.assert_called_once_with(Path('/requested/ffmpeg'), Path('/requested/ffprobe'))
            self.assertIs(selected.call_args.args[1], pair)
            self.assertEqual([call.args[0] for call in digest.call_args_list][1:],
                             [pair.ffmpeg, pair.ffprobe])

    def test_pin_failure_precedes_output_and_work(self):
        import contextlib
        import io
        import test_oracle
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary) / 'absent' / 'report.json'
            work = Path(temporary) / 'work'
            argv = ['raw_pcm_check', '--coreav', 'coreav', '--oracle-dir', '/requested',
                    '--out', str(out), '--work-dir', str(work)]
            with patch.object(sys, 'argv', argv), \
                    patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('wrong pin')), \
                    patch.object(check, 'check') as selected, \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                check.main()
            self.assertEqual(error.exception.code, 2)
            selected.assert_not_called()
            self.assertFalse(out.parent.exists())
            self.assertFalse(work.exists())


if __name__ == '__main__':
    unittest.main()
