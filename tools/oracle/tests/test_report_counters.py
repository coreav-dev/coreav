"""The fixture check counters of tools/stage3-report/run.py: each reads its
check's report, whatever line the check prints last (issue #178)."""
import importlib.util
import json
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools' / 'stage3-report'))
SPEC = importlib.util.spec_from_file_location('run', ROOT / 'tools' / 'stage3-report' / 'run.py')
run = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(run)


class ExtractCounter(unittest.TestCase):
    def test_the_count_comes_from_the_report_not_the_last_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            report = pathlib.Path(tmp) / 'extract.json'
            report.write_text(json.dumps({'passed': 40, 'direct': 39, 'total': 40, 'missing': [],
                                          'refused': ['edit_multi_audio.mp4 stream 0'], 'results': []}))
            stdout = ('40/40 extractions pass every oracle (39 directly, 1 through known divergences)\n'
                      'refused edit_multi_audio.mp4 stream 0: edit list outside the simple model\n')
            count, summary = run.count_extract(stdout, report)
            self.assertEqual(count, [40, 40])
            self.assertIn('1 refused as expected', summary)
            # The default, the first N/M of the last line, finds no count there.
            self.assertIsNone(run.first_count(stdout.strip().split('\n')[-1]))


class CompatCounter(unittest.TestCase):
    def test_an_unexplained_rejection_is_counted_not_a_crash(self):
        with tempfile.TemporaryDirectory() as tmp:
            report = pathlib.Path(tmp) / 'compat.json'
            report.write_text(json.dumps({'compared_files': 3, 'diffs': [{'file': 'a.mp4', 'key': 'format.duration'}],
                                          'via_known_divergence': [],
                                          'rejected_without_known_divergence': [
                                              {'file': 'b.mp4', 'ffprobe_exit': 0, 'coreav_exit': 1}]}))
            count, _ = run.count_compat('', report)
            self.assertEqual(count, [1, 3])


if __name__ == '__main__':
    unittest.main()
