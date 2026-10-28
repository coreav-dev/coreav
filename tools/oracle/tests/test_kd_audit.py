"""tools/oracle/kd_audit.py: the verdict of one bug-shaped change."""
import importlib.util
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('kd_audit', ROOT / 'tools' / 'oracle' / 'kd_audit.py')
audit = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(audit)


class Verdicts(unittest.TestCase):
    # ({level: (items, direct, uncovered, by_kd)}, gating differences)
    counts = {'streams': (1, 0, 0, {'KD-PCM-END': 1}), 'packets': (10, 8, 0, {'KD-CTTS': 2})}
    diffs = frozenset({'a', 'b', 'c'})
    base = (counts, diffs)

    def test_a_new_uncovered_item_is_caught(self):
        result = (dict(self.counts, packets=(10, 8, 1, {'KD-CTTS': 1})), self.diffs | {'d'})
        self.assertEqual(audit.verdict_of(self.base, result), 'caught')

    def test_a_known_divergence_that_covers_more_is_a_loophole(self):
        result = (dict(self.counts, packets=(10, 7, 0, {'KD-CTTS': 3})), self.diffs | {'d'})
        self.assertTrue(audit.verdict_of(self.base, result).startswith('COVERED'))

    def test_a_new_difference_absorbed_by_an_entry_is_a_loophole(self):
        # Same counts, but the comparison shows a difference it did not.
        result = (self.counts, (self.diffs - {'c'}) | {'c2'})
        self.assertTrue(audit.verdict_of(self.base, result).startswith('COVERED absorbed'))

    def test_a_lost_packet_the_reference_does_not_list(self):
        result = (dict(self.counts, packets=(9, 8, 0, {'KD-CTTS': 1})), self.diffs - {'c'})
        self.assertEqual(audit.verdict_of(self.base, result), 'unlisted')

    def test_no_change_and_direct_agreement(self):
        self.assertEqual(audit.verdict_of(self.base, (dict(self.counts), self.diffs)), 'no effect')
        result = (dict(self.counts, streams=(1, 1, 0, {})), self.diffs - {'a'})
        self.assertEqual(audit.verdict_of(self.base, result), 'agrees')


if __name__ == '__main__':
    unittest.main()
