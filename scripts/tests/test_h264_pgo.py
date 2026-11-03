"""Media-free tests of the registered PGO evidence boundary."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import h264_parity_bars as bars
import h264_pgo_round as round_tool


class PgoRound(unittest.TestCase):
    def setUp(self):
        self.plan = json.loads(round_tool.PLAN.read_text())

    def test_interval_bar_is_inclusive_but_significance_is_strict(self):
        one = {'median': 0.97, 'ci95': [0.94, 0.999]}
        eight = {c: {'ci95': [0.98, 1.01]} for c in bars.CLASSES}
        self.assertTrue(round_tool.verdict(one, eight, self.plan))
        eight['1080p']['ci95'][1] = 1.010001
        self.assertFalse(round_tool.verdict(one, eight, self.plan))
        eight['1080p']['ci95'][1] = 1.01
        one['ci95'][1] = 1
        self.assertFalse(round_tool.verdict(one, eight, self.plan))
        one['ci95'][1] = 0.999
        one['median'] = 0.970001
        self.assertFalse(round_tool.verdict(one, eight, self.plan))

    def test_paired_round_bootstrap_preserves_round_pairing(self):
        streams = {c: c for c in bars.CLASSES}
        times = {(s, a): {i: (i + 1) * (0.95 if a == 'pgo' else 1) for i in range(3)}
                 for s in streams for a in ('base', 'pgo')}
        value = bars.paired_round_summary(times, streams, 'pgo', 'base', 3, 42, 100)
        for cell in value['groups'].values():
            self.assertAlmostEqual(cell['median'], 0.95)
            for bound in cell['ci95']:
                self.assertAlmostEqual(bound, 0.95)
        del times['4k', 'pgo'][2]
        with self.assertRaises(bars.Refused):
            bars.paired_round_summary(times, streams, 'pgo', 'base', 3, 42, 100)

    def test_duplicate_source_and_registry_owner_omissions_are_refused(self):
        registry = {c: {'commitment': 'a' * 64, 'hashes': ['a' * 64], 'identities': ['old'], 'groups': ['old-group']}
                    for c in round_tool.COVERAGE}
        candidate = {'identity': 'new', 'groups': ['new-group']}
        self.assertEqual(round_tool.allocate([candidate], registry, self.plan), [candidate])
        self.assertEqual(round_tool.allocate([candidate | {'groups': ['old-group']}], registry, self.plan), [])
        self.assertEqual(round_tool.allocate([candidate | {'sha256': 'a' * 64}], registry, self.plan), [])
        with self.assertRaises(ValueError):
            round_tool.allocate([candidate, candidate], registry, self.plan)
        del registry['encoder-sets']
        with self.assertRaises(ValueError):
            round_tool.allocate([candidate], registry, self.plan)

    def test_schedule_has_every_cell_once_and_is_deterministic(self):
        rows = [{'label': f'h{i:03}'} for i in range(64)]
        cells = round_tool.schedule(rows, self.plan)
        self.assertEqual(cells, round_tool.schedule(rows, self.plan))
        keys = {(r['label'], r['arm'], r['threads'], r['round']) for r in cells}
        self.assertEqual(len(keys), 64 * 3 * 2 * self.plan['rounds'])
        self.assertEqual(len(cells), len(keys))

    def test_process_failures_and_raw_overflow_cannot_be_fast_samples(self):
        with self.assertRaises(ValueError):
            round_tool.execute([sys.executable, '-c', 'raise SystemExit(1)'])
        with self.assertRaises(ValueError):
            round_tool.execute([sys.executable, '-c', 'print("unexpected", file=__import__("sys").stderr)'])
        with self.assertRaises(ValueError):
            round_tool.execute([sys.executable, '-c', 'print("x" * 1000)'], maximum=10)
        result = round_tool.execute([sys.executable, '-c', 'print("abc", end="")'], maximum=3)
        self.assertEqual(result['bytes'], 3)
        self.assertGreater(result['wall'], 0)

    def test_registry_indexes_hashes_sources_and_groups_from_actual_files(self):
        with tempfile.TemporaryDirectory() as name:
            p = Path(name) / 'inventory.json'
            p.write_text(json.dumps({'sha256': 'b' * 64, 'url': 'https://example.test/new', 'groups': ['family']}))
            owners = {owner: [str(p)] for owner in round_tool.COVERAGE}
            result = round_tool.exclusion_registry(owners)
            self.assertIn('b' * 64, result['training']['hashes'])
            self.assertIn('https://example.test/new', result['encoder-sets']['identities'])
            self.assertIn('family', result['other-draws']['groups'])
            previous = result['training']['commitment']
            p.write_text(p.read_text() + '\n')
            self.assertNotEqual(previous, round_tool.exclusion_registry(owners)['training']['commitment'])


if __name__ == '__main__':
    unittest.main()
