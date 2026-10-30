"""scripts/aac_encode_check.py builds the command lines its bars judge
(docs/design/transcode.md, "AAC bars") as the note fixes them, and judges
the bars as it fixes them. No media is made and no program runs.

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
import pathlib
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import aac_encode_check  # noqa: E402


class Arms(unittest.TestCase):
    def test_the_default_rate_names_none(self):
        ours, theirs = aac_encode_check.arms('coreav', 'ffmpeg9', 'S1.mp4', 'o.mp4', 't.mp4', None)
        self.assertEqual(ours, ['coreav', 'transcode', 'S1.mp4', 'o.mp4', '--force', '--audio', 'aac',
                                '--encoder', 'ffmpeg', '--ffmpeg', 'ffmpeg9'])
        self.assertEqual(theirs, ['ffmpeg9', '-nostdin', '-v', 'error', '-y', '-i', 'S1.mp4',
                                  '-c:v', 'libx264', '-c:a', 'aac', 't.mp4'])

    def test_a_rate_follows_each_option_whole(self):
        ours, theirs = aac_encode_check.arms('coreav', 'ffmpeg9', 'S1.mp4', 'o.mp4', 't.mp4', 192)
        self.assertEqual(ours[-2:], ['--audio-bitrate', '192'])
        self.assertEqual(theirs[-3:], ['-b:a', '192k', 't.mp4'])

    def test_the_set(self):
        self.assertEqual(aac_encode_check.RATES, [None, 64, 192])
        names = [s[0] for s in aac_encode_check.opus.SOURCES]
        self.assertEqual(names, ['S1', 'S2', 'S3', 'S4', 'S5', 'S6'])
        self.assertEqual(aac_encode_check.TONAL, {'S1', 'S2', 'S3', 'S6'})


def row(ratio=1.0, tonal=True, **bars):
    return {'input': 'x', 'failed': None, 'tonal': tonal, 'bit_rate_ratio': ratio,
            'a1': bars.get('a1', True), 'a2': bars.get('a2', True), 'a4': bars.get('a4', True)}


class Bars(unittest.TestCase):
    def test_the_timeline(self):
        ours = {'codec_name': 'aac', 'profile': 'LC', 'sample_rate': '44100', 'start_pts': 0,
                'duration_ts': 441000}
        summary = {'samples': 441000}
        self.assertTrue(aac_encode_check.timeline(ours, {'duration_ts': 441000}, summary, '44100'))
        self.assertFalse(aac_encode_check.timeline(ours, {'duration_ts': 441001}, summary, '44100'))
        self.assertFalse(aac_encode_check.timeline(ours, {'duration_ts': 441000}, summary, '48000'))
        self.assertFalse(aac_encode_check.timeline({**ours, 'profile': 'HE-AAC'}, {'duration_ts': 441000},
                                                   summary, '44100'))

    def test_all_met(self):
        self.assertTrue(all(aac_encode_check.judge([row(0.99), row(1.01), row(1.0, tonal=False)]).values()))

    def test_the_rate_bounds(self):
        self.assertFalse(aac_encode_check.judge([row(0.94), row(1.0), row(1.0)])['A3'])
        self.assertFalse(aac_encode_check.judge([row(1.03), row(1.03), row(1.0)])['A3'])
        self.assertTrue(aac_encode_check.judge([row(1.05), row(0.95), row(1.0)])['A3'])

    def test_quality_is_judged_on_tonal_inputs_only(self):
        self.assertTrue(aac_encode_check.judge([row(tonal=False, a4=False)])['A4'])
        self.assertFalse(aac_encode_check.judge([row(a4=False)])['A4'])

    def test_a_failed_input_misses_every_bar(self):
        failed = {'input': 'x', 'failed': 'exit 1', 'tonal': True}
        self.assertEqual(aac_encode_check.judge([row(), failed]),
                         {'A1': False, 'A2': False, 'A3': False, 'A4': False})


if __name__ == '__main__':
    unittest.main()
