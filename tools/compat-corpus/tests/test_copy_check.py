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


if __name__ == "__main__":
    unittest.main()
