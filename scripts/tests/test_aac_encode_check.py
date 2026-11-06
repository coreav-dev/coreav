"""scripts/aac_encode_check.py builds the command lines its bars judge
(docs/design/transcode.md, "AAC bars") as the note fixes them, and judges
the bars as it fixes them. No media is made and no program runs.

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
import pathlib
import sys
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import aac_encode_check  # noqa: E402


class Arms(unittest.TestCase):
    def test_main_executes_the_owner_returned_pair(self):
        qualified = aac_encode_check.test_oracle.Oracle(pathlib.Path('/qualified/encoder'),
                                                       pathlib.Path('/qualified/probe'))
        argv = ['aac', '--coreav', 'coreav', '--ffmpeg', 'original-encoder', '--ffprobe', 'original-probe',
                '--work', 'work', '--out', 'report']
        commands = []

        def run(command, **kwargs):
            commands.append(command)
            return mock.Mock(returncode=0, stderr=b'', stdout=b'{"streams":[{"sample_rate":"48000"}]}')

        def timed(command):
            commands.append(command)
            return mock.Mock(returncode=0, stderr=b'', stdout=b'{"encoded_audio":{"samples":1}}'), 1.0

        with mock.patch.object(sys, 'argv', argv), \
                mock.patch.object(aac_encode_check.test_oracle, 'require', return_value=qualified) as require, \
                mock.patch.object(aac_encode_check.os, 'makedirs'), \
                mock.patch.object(aac_encode_check, 'TONAL', set()), \
                mock.patch.object(aac_encode_check, 'RATES', [None]), \
                mock.patch.object(aac_encode_check.opus, 'SOURCES', [('S4',)]), \
                mock.patch.object(aac_encode_check.opus, 'make_sources', return_value={'S4': 'source'}) as sources, \
                mock.patch.object(aac_encode_check.opus, 'timed', side_effect=timed), \
                mock.patch.object(aac_encode_check.opus, 'run', side_effect=run), \
                mock.patch('builtins.open', mock.mock_open()), mock.patch('builtins.print'):
            with self.assertRaises(SystemExit):
                aac_encode_check.main()
        require.assert_called_once_with('original-encoder', 'original-probe')
        sources.assert_called_once_with(str(qualified.ffmpeg), 'work')
        self.assertIn(str(qualified.ffmpeg), [c[0] for c in commands])
        self.assertIn(str(qualified.ffprobe), [c[0] for c in commands])
        self.assertTrue(all(c[0] in ('coreav', str(qualified.ffmpeg), str(qualified.ffprobe)) for c in commands))

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

    def test_a_missing_pcm_fails_a4(self):
        self.assertIsNone(aac_encode_check.opus.samples(1, b'\0' * 8))
        self.assertIsNone(aac_encode_check.opus.samples(0, b''))
        self.assertEqual(len(aac_encode_check.opus.samples(0, b'\0' * 8)), 2)
        reference = aac_encode_check.opus.samples(0, b'\0\0\x80\x3f' * 4)
        self.assertIsNone(aac_encode_check.opus.quality(reference, None))
        self.assertIsNone(aac_encode_check.opus.quality(reference, aac_encode_check.opus.samples(0, b'')))
        self.assertIsNone(aac_encode_check.opus.quality(None, reference))
        self.assertEqual(aac_encode_check.opus.quality(reference, reference), float('inf'))
        self.assertFalse(aac_encode_check.a4(None, 20.0))
        self.assertFalse(aac_encode_check.a4(20.0, None))
        self.assertTrue(aac_encode_check.a4(19.5, 20.0))
        self.assertFalse(aac_encode_check.a4(19.4, 20.0))

    def test_a_failed_input_misses_every_bar(self):
        failed = {'input': 'x', 'failed': 'exit 1', 'tonal': True}
        self.assertEqual(aac_encode_check.judge([row(), failed]),
                         {'A1': False, 'A2': False, 'A3': False, 'A4': False})


if __name__ == '__main__':
    unittest.main()
