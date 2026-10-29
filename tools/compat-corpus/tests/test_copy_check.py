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
