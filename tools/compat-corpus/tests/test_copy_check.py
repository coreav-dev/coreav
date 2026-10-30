"""copy_check.py's reading of ffmpeg's stream mapping and of the
summaries (docs/design/compat-cli.md, "coreav ffmpeg")."""
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

import copy_check  # noqa: E402


class Mapping(unittest.TestCase):
    def test_mapping_lines(self):
        stderr = ("Stream mapping:\n"
                  "  Stream #0:2 -> #0:1 (copy)\n"
                  "  Stream #0:0 -> #0:0 (copy)\n"
                  "Press [q] to stop\n")
        mapped = sorted((int(o), int(i)) for i, o in copy_check.MAPPING.findall(stderr))
        self.assertEqual(mapped, [(0, 0), (1, 2)])

    def test_selected_streams_of_each_summary(self):
        record = lambda detail: {"detail": detail}  # noqa: E731
        self.assertEqual(copy_check.selected(record({"stream": {"stream_index": 3}})), [3])
        self.assertEqual(copy_check.selected(record({"streams": [{"stream_index": 0}, {"stream_index": 2}]})),
                         [0, 2])
        segments = {"segments": [{"streams": [{"stream_index": 1}]},
                                 {"streams": [{"stream_index": 1}, {"stream_index": 0}]}]}
        self.assertEqual(copy_check.selected(record(segments)), [1, 0])


class Concat(unittest.TestCase):
    def test_microseconds_round_halves_away_from_zero(self):
        F = copy_check.fractions.Fraction
        self.assertEqual(copy_check.micros(F(1, 2_000_000)), 1)
        self.assertEqual(copy_check.micros(F(-1, 2_000_000)), -1)
        self.assertEqual(copy_check.micros(F(1, 3)), 333_333)

    def test_each_input_follows_the_durations_before_it(self):
        F = copy_check.fractions.Fraction
        # A 0.3 s video with 1 s of audio, listed twice: the demuxer places
        # the second file at 1 s, a join at the video's end at 0.3 s.
        formats = [(0, 1_000_000), (0, 1_000_000)]
        self.assertEqual(copy_check.placement([F(0), F(1)], formats), [])
        self.assertEqual(copy_check.placement([F(0), F(3, 10)], formats),
                         ["timing: input 1 placed at 300000 us, ffmpeg concat 1000000 us"])
        # Each file's own start time is moved to where the file goes.
        self.assertEqual(copy_check.placement([F(5), F(8)], [(500_000, 2_000_000), (-500_000, 1_000_000)]), [])

    def test_the_presentation_end_by_the_boxes(self):
        F = copy_check.fractions.Fraction

        def track(handler, timescale, durations, cto, base=0):
            return {"handler": handler, "timescale": timescale, "edits": None,
                    "samples": {"durations": durations, "cto": cto, "sizes": [1] * len(durations),
                                "base_dts": base, "gap": False}}
        # Two frames of 512 ticks presented from 1024, and 1 s of audio.
        video = track("vide", 12_800, [512, 512], [1024, 1024])
        audio = track("soun", 1000, [500, 500], [0, 0])
        movie = {"fragmented": True, "tracks": [video, audio]}
        self.assertEqual(copy_check.presentation(movie), (F(1), F(2, 25), 2))
        longer = dict(video, samples=dict(video["samples"], durations=[512, 12_800]))
        self.assertEqual(copy_check.presentation(dict(movie, tracks=[longer, audio])), (F(28, 25), F(2, 25), 2))
        self.assertIsNone(copy_check.presentation(dict(movie, fragmented=False)))
        self.assertIsNone(copy_check.presentation(dict(movie, tracks=[dict(video, edits=[(1, 1024, 1)]), audio])))
        self.assertIsNone(copy_check.presentation(dict(movie, tracks=[audio])))

    def test_only_an_excess_of_the_first_composition_time_is_set_aside(self):
        F = copy_check.fractions.Fraction
        found = [(F(502, 25), F(2, 25), 500)] * 2
        self.assertEqual(copy_check.composition_gaps([(0, 20_160_000)] * 2, found), [F(2, 25)] * 2)
        self.assertEqual(copy_check.composition_gaps([(0, 20_080_000)] * 2, found), [0, 0])
        self.assertEqual(copy_check.composition_gaps([(0, 20_200_000), (0, 20_160_000)], [None, found[1]]),
                         [0, F(2, 25)])
        times = [(F(i), F(i)) for i in range(4)]
        self.assertEqual(copy_check.without_gaps(times, [F(1, 2), F(1, 2)], [2, 2]),
                         [(F(0), F(0)), (F(1), F(1)), (F(3, 2), F(3, 2)), (F(5, 2), F(5, 2))])


class Range(unittest.TestCase):
    def test_times_in_ffmpeg_syntax(self):
        F = copy_check.fractions.Fraction
        self.assertEqual(copy_check.seconds("00:02:54.583"), F(174583, 1000))
        self.assertEqual(copy_check.seconds("03:10"), 190)
        self.assertEqual(copy_check.seconds("100ms"), F(1, 10))
        self.assertEqual(copy_check.seconds("1500us"), F(3, 2000))
        self.assertEqual(copy_check.seconds("2.5s"), F(5, 2))
        self.assertEqual(copy_check.seconds("120"), 120)

    def test_every_time_counts_from_the_input_start(self):
        # The pinned ffmpeg on an all-intra video at one frame a second
        # whose first frame is presented at 5 s.
        def at_5(command):
            return copy_check.requested(command.split(), 5)
        self.assertEqual(at_5("-ss 6 -i INPUT -t 1 -c copy OUTPUT"), (11, 12))
        self.assertEqual(at_5("-i INPUT -ss 6 -t 1 -c copy OUTPUT"), (11, 12))
        self.assertEqual(at_5("-ss 2 -i INPUT -ss 3 -t 1 -c copy OUTPUT"), (10, 11))
        self.assertEqual(at_5("-ss 6 -i INPUT -to 2 -c copy OUTPUT"), (11, 13))
        self.assertEqual(at_5("-ss 1 -i INPUT -to 3 -c copy OUTPUT"), (6, 9))
        self.assertEqual(at_5("-to 3 -i INPUT -c copy OUTPUT"), (5, 8))
        self.assertEqual(at_5("-i INPUT -ss 6 -c copy OUTPUT"), (11, None))


if __name__ == "__main__":
    unittest.main()
