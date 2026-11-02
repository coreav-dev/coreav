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
        self.assertEqual(len(cases), 140)
        self.assertEqual(len(set(cases)), 140)
        self.assertEqual(sum(kind == 'raw' for *_, kind in cases), 56)
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
                return check.check(root / 'coreav', root, root)

    def test_equal_samples_pass_without_fallback(self):
        self.assertEqual(self.exercise()[0]['result'], 'equal')

    def test_one_changed_sample_fails(self):
        with self.assertRaisesRegex(AssertionError, 'samples differ'):
            self.exercise(wrong_sample=True)

    def test_one_changed_field_fails(self):
        with self.assertRaisesRegex(AssertionError, 'fields differ'):
            self.exercise(wrong_fields=True)


if __name__ == '__main__':
    unittest.main()
