"""scripts/thumbnail_oracle.py: the time FFmpeg seeks to for the frame CoreAV
chose comes from the packet that holds the frame's bytes, on FFmpeg's own
timeline, whatever start CoreAV counts from. scripts/vp9_speed.py seeks
with the same function. No media: ffprobe's listing is given.

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
import json
import pathlib
import sys
import types
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import thumbnail_oracle  # noqa: E402


def summary(seconds_x1000, offset):
    return {'stream_index': 0, 'packet': {'offset': offset, 'size': 100},
            'time': {'frame': {'num': seconds_x1000, 'den': 1000}}}


class FfmpegTime(unittest.TestCase):
    def listing(self, start, packets, base='1/1000'):
        doc = {'packets': packets, 'streams': [{'time_base': base}], 'format': {'start_time': start}}
        self.calls = []

        def run(cmd):
            self.calls.append(cmd)
            return types.SimpleNamespace(stdout=json.dumps(doc).encode())
        self.addCleanup(setattr, thumbnail_oracle, 'run', thumbnail_oracle.run)
        thumbnail_oracle.run = run

    def test_a_matroska_frame_maps_through_its_block(self):
        # FFmpeg 9.0.2 starts the file 65 ms later than CoreAV: CoreAV's
        # 10.065 s is FFmpeg's 10.000 s, the block holding the frame's bytes.
        self.listing('0.065000', [{'pos': '1000', 'pts': 10032, 'flags': 'K__'},
                                  {'pos': '2000', 'pts': 10065, 'flags': 'K__'},
                                  {'pos': '3000', 'pts': 10098, 'flags': '___'}])
        at = thumbnail_oracle.ffmpeg_time('ffprobe', pathlib.Path('a.webm'), summary(10065, 2012))
        self.assertEqual(at, '10.000000')
        self.assertIn('-select_streams', self.calls[0])

    def test_other_tracks_do_not_move_it(self):
        # The same frame, the file's start set by a track the thumbnail does
        # not read: only the frame's own packet and the file start count.
        self.listing('0.000000', [{'pos': '2000', 'pts': 10065, 'flags': 'K__'}])
        at = thumbnail_oracle.ffmpeg_time('ffprobe', pathlib.Path('a.mkv'), summary(10070, 2012))
        self.assertEqual(at, '10.065000')

    def test_an_mp4_frame_needs_its_exact_offset(self):
        self.listing('0.000000', [{'pos': '2000', 'pts': 512, 'flags': 'K__'}], base='1/512')
        self.assertEqual(thumbnail_oracle.ffmpeg_time('ffprobe', pathlib.Path('a.mp4'), summary(1000, 2000)),
                         '1.000000')
        self.assertEqual(thumbnail_oracle.ffmpeg_time('ffprobe', pathlib.Path('a.mp4'), summary(1500, 2012)),
                         '1.500000')

    def test_a_discarded_packet_has_no_reference(self):
        self.listing('0.000000', [{'pos': '2000', 'pts': 0, 'flags': 'KD_'}])
        self.assertIsNone(thumbnail_oracle.ffmpeg_time('ffprobe', pathlib.Path('a.mp4'), summary(0, 2000)))

    def test_no_packet_keeps_coreavs_time(self):
        self.listing('0.000000', [{'pos': 'N/A', 'pts': 10065, 'flags': 'K__'}])
        self.assertEqual(thumbnail_oracle.ffmpeg_time('ffprobe', pathlib.Path('a.webm'), summary(10065, 2012)),
                         '10.065000')


if __name__ == '__main__':
    unittest.main()
