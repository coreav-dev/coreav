"""scripts/opus_encode_check.py builds the command lines its bars judge
(docs/design/transcode.md, "Opus bars") as the note fixes them, and fails
O4 when the PCM it compares is missing. No media is made and no program
runs.

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
import pathlib
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import opus_encode_check  # noqa: E402


class Arms(unittest.TestCase):
    def test_the_default_rate_names_none(self):
        ours, theirs = opus_encode_check.arms('coreav', 'ffmpeg9', 'S1.mp4', 'o.mp4', 't.mp4', None)
        self.assertEqual(ours, ['coreav', 'transcode', 'S1.mp4', 'o.mp4', '--force', '--audio', 'opus',
                                '--encoder', 'ffmpeg', '--ffmpeg', 'ffmpeg9'])
        self.assertEqual(theirs, ['ffmpeg9', '-nostdin', '-v', 'error', '-y', '-i', 'S1.mp4',
                                  '-c:v', 'libx264', '-c:a', 'libopus', 't.mp4'])

    def test_a_rate_follows_each_option_whole(self):
        ours, theirs = opus_encode_check.arms('coreav', 'ffmpeg9', 'S1.mp4', 'o.mp4', 't.mp4', 32)
        self.assertEqual(ours, ['coreav', 'transcode', 'S1.mp4', 'o.mp4', '--force', '--audio', 'opus',
                                '--encoder', 'ffmpeg', '--ffmpeg', 'ffmpeg9', '--audio-bitrate', '32'])
        self.assertEqual(theirs, ['ffmpeg9', '-nostdin', '-v', 'error', '-y', '-i', 'S1.mp4',
                                  '-c:v', 'libx264', '-c:a', 'libopus', '-b:a', '32k', 't.mp4'])

    def test_every_option_of_every_input_keeps_its_value(self):
        for rate in opus_encode_check.RATES:
            ours, _ = opus_encode_check.arms('coreav', 'ffmpeg9', 'S1.mp4', 'o.mp4', 't.mp4', rate)
            options = ours[4:]
            pairs = dict(zip(options[1::2], options[2::2]))
            self.assertEqual(options[0], '--force')
            self.assertEqual(pairs['--audio'], 'opus', rate)
            self.assertEqual(pairs['--encoder'], 'ffmpeg', rate)
            self.assertEqual(pairs.get('--audio-bitrate'), rate and str(rate))


class Quality(unittest.TestCase):
    def test_a_missing_pcm_fails_o4(self):
        self.assertIsNone(opus_encode_check.samples(1, b'\0' * 8))
        self.assertIsNone(opus_encode_check.samples(0, b''))
        self.assertEqual(len(opus_encode_check.samples(0, b'\0' * 8)), 2)
        reference = opus_encode_check.samples(0, b'\0\0\x80\x3f' * 4)
        self.assertIsNone(opus_encode_check.quality(reference, None))
        self.assertIsNone(opus_encode_check.quality(reference, opus_encode_check.samples(0, b'')))
        self.assertIsNone(opus_encode_check.quality(None, reference))
        self.assertEqual(opus_encode_check.quality(reference, reference), float('inf'))
        self.assertFalse(opus_encode_check.o4(None, 20.0))
        self.assertFalse(opus_encode_check.o4(20.0, None))
        self.assertTrue(opus_encode_check.o4(19.0, 20.0))
        self.assertFalse(opus_encode_check.o4(18.9, 20.0))


if __name__ == '__main__':
    unittest.main()
