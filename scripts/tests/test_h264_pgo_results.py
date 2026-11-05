"""Replay immutable public PGO work and point estimates without media execution."""
from collections import Counter
import gzip
import hashlib
import json
import math
from pathlib import Path
import statistics
import unittest

DATA = Path(__file__).resolve().parents[2] / 'docs/measurements/h264-pgo/heldout'


class PublishedPgoResults(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.record = json.loads((DATA / 'record.json').read_text())
        cls.summary = json.loads((DATA / 'summary.json').read_text())
        cls.events = [json.loads(line) for line in gzip.decompress((DATA / 'events.jsonl.gz').read_bytes()).splitlines()]
        cls.diagnostics = [json.loads(line) for line in gzip.decompress((DATA / 'diagnostics.jsonl.gz').read_bytes()).splitlines()]

    def test_retained_files_match_their_recorded_hashes(self):
        for name, expected in self.record['files'].items():
            with self.subTest(name=name):
                self.assertEqual(hashlib.sha256((DATA / name).read_bytes()).hexdigest(), expected)
        self.assertEqual(self.record['status'], 'complete')
        self.assertEqual(self.record['counts'], {'sd': 16, '720p': 16, '1080p': 16, '4k': 9})
        self.assertEqual(self.events[-1], {'kind': 'complete'})
        self.assertNotIn('abort', {row['kind'] for row in self.events})

    def test_every_registered_file_has_exact_qualification_work(self):
        rows = {r['label']: r for r in self.record['inputs']}
        self.assertEqual(len(rows), 57)
        calls = [e for e in self.events if e['kind'] == 'qualification']
        wanted = {(label, arm, t) for label in rows for arm in ('base', 'pgo', 'ffmpeg') for t in (1, 8)}
        self.assertEqual(len(calls), self.record['qualification_cells'])
        self.assertEqual({(e['label'], e['arm'], e['threads']) for e in calls}, wanted)
        for label, row in rows.items():
            values = [e for e in calls if e['label'] == label]
            pixels = row['width'] * row['height'] + 2 * ((row['width'] + 1) // 2) * ((row['height'] + 1) // 2)
            frames = min(row['source_frames'], self.record['method']['frames'][row['class']])
            self.assertEqual(row['frames'], frames)
            self.assertTrue(all(e['frames'] == frames and e['bytes'] == frames * pixels for e in values))
            self.assertEqual(len({e['sha256'] for e in values}), 1)

    def test_all_native_timed_cells_reproduce_the_published_point_estimates(self):
        rows = {r['label']: r for r in self.record['inputs']}
        timed = [e for e in self.events if e['kind'] == 'timed']
        rounds = self.record['method']['rounds']
        wanted = {(label, arm, t, n) for label in rows for arm in ('base', 'pgo', 'ffmpeg')
                  for t in (1, 8) for n in range(rounds)}
        self.assertEqual(len(timed), self.record['timed_cells'])
        self.assertEqual({(e['label'], e['arm'], e['threads'], e['round']) for e in timed}, wanted)
        self.assertTrue(all(math.isfinite(e['wall']) and e['wall'] > 0 for e in timed))
        for threads in ('1', '8'):
            medians = {}
            for label in rows:
                for arm in ('base', 'pgo', 'ffmpeg'):
                    values = [e['wall'] for e in timed if e['label'] == label and e['arm'] == arm
                              and e['threads'] == int(threads)]
                    medians[label, arm] = statistics.median(values)
                    self.assertEqual(self.summary[threads]['calls'][f'{label}/{arm}'],
                                     {'median': statistics.median(values), 'min': min(values),
                                      'max': max(values), 'range': max(values) - min(values)})
            for comparison, result in self.summary[threads]['comparisons'].items():
                new, old = comparison.split('/')
                ratios = {label: medians[label, new] / medians[label, old] for label in rows}
                self.assertEqual(result['per_stream'], ratios)
                for group, point in result['groups'].items():
                    members = [label for label, row in rows.items() if group == 'overall' or row['class'] == group]
                    self.assertEqual(point['n'], len(members))
                    self.assertEqual(point['median'], statistics.median(ratios[label] for label in members))
                    self.assertEqual(point['faster'], sum(ratios[label] < 1 for label in members))
                    self.assertTrue(all(math.isfinite(v) and v > 0 for v in point['ci95']))

    def test_untimed_preflight_keeps_the_registered_inputs_and_matches_full_qualification(self):
        pilot = json.loads((DATA / 'preflight.json').read_text())
        first = [next(r['label'] for r in self.record['inputs'] if r['class'] == c) for c in self.record['counts']]
        wanted = {(label, arm, t) for label in first for arm in ('base', 'pgo', 'ffmpeg') for t in (1, 8)}
        self.assertEqual(len(pilot), 24)
        self.assertEqual({(r['label'], r['arm'], r['threads']) for r in pilot}, wanted)
        qualified = {(r['label'], r['arm'], r['threads']): r for r in self.events if r['kind'] == 'qualification'}
        for row in pilot:
            full = qualified[row['label'], row['arm'], row['threads']]
            self.assertTrue(all(row[k] == full[k] for k in ('bytes', 'sha256', 'frames')))

    def test_the_original_verdict_is_a_completed_median_bar_miss(self):
        method = self.record['method']
        one = self.summary['1']['comparisons']['pgo/base']['groups']['overall']
        eight = self.summary['8']['comparisons']['pgo/base']['groups']
        self.assertGreater(one['median'], method['one_thread_median_max'])
        self.assertLess(one['ci95'][1], method['one_thread_ci_high_below'])
        self.assertTrue(all(eight[c]['ci95'][1] <= method['eight_thread_ci_high_max'] for c in self.record['counts']))
        self.assertFalse(self.summary['pass'])

    def test_diagnostic_calls_keep_their_separate_non_gating_result(self):
        self.assertEqual(Counter(e['kind'] for e in self.diagnostics),
                         {'qualification': 18, 'warmup': 12, 'timed': 108, 'complete': 1})
        self.assertFalse(self.summary['diagnostics']['gates_h264'])
        timed = [e for e in self.diagnostics if e['kind'] == 'timed']
        for case, threads in self.summary['diagnostics']['commands'].items():
            for thread, point in threads.items():
                self.assertFalse(point['guard'])
                for arm in ('base', 'pgo'):
                    values = [e['wall'] for e in sorted(timed, key=lambda e: e['round'])
                              if e['case'] == case and e['threads'] == int(thread) and e['arm'] == arm]
                    self.assertEqual(len(values), self.record['method']['rounds'])
                    self.assertEqual(point['calls'][arm]['values'], values)
                    self.assertEqual(point['calls'][arm]['median'], statistics.median(values))

    def test_acquisition_accounting_is_the_unchanged_separate_record(self):
        acquisition = json.loads((DATA / 'acquisition-report.json').read_text())
        self.assertEqual(self.summary['acquisition'], acquisition)
        self.assertEqual({c: row['selected'] for c, row in acquisition['per_class'].items()}, self.record['counts'])
        self.assertEqual(acquisition['inference_scope'], 'capture-clean-eligible-cohort')
        self.assertEqual(acquisition['failed_sources'], {'known': 0, 'unknown': 8})


if __name__ == '__main__':
    unittest.main()
