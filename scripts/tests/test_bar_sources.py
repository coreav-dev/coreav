"""The pinned FFmpeg makes every source of the Opus bars' set
(scripts/opus_encode_check.py; docs/design/transcode.md, "Opus bars"),
which the AAC bars share, at one second each, so a source its filter
parser refuses fails here and not in a judged run. The shared oracle
owner requires the pinned pair before any source is generated.

    COREAV_FFMPEG=FFMPEG python3 -I -m unittest discover -s scripts/tests -p 'test_bar_sources.py'
"""
import os
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import opus_encode_check  # noqa: E402
import test_oracle


class Sources(unittest.TestCase):
    def test_the_pinned_ffmpeg_makes_every_source(self):
        ffmpeg = test_oracle.require().ffmpeg
        with tempfile.TemporaryDirectory() as work:
            paths = opus_encode_check.make_sources(ffmpeg, work, duration=1)
            self.assertEqual(sorted(paths), [s[0] for s in opus_encode_check.SOURCES])
            for path in paths.values():
                self.assertGreater(os.path.getsize(path), 0, path)


if __name__ == '__main__':
    unittest.main()
