"""The corpus compat check: one file's record
from compat_check.py's report, and the per-key bar over a family."""
import json
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools' / 'stage3-report'))
import corpus  # noqa: E402
import report  # noqa: E402


def compat_report(keys, via_kd=(), unexplained=()):
    return {'compared_files': 1, 'rejected_by_either': [], 'rejected_without_known_divergence': list(unexplained),
            'keys': {k: {'agreeing_files': int(ok), 'files': 1} for k, ok in keys.items()},
            'failing_keys': {}, 'diffs': [], 'via_known_divergence': [{'key': k} for k in via_kd]}


class Record(unittest.TestCase):
    def record(self, data, status=0):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'compat.json'
            if data is not None:
                path.write_text(json.dumps(data))
            return corpus.compat_record(path, status)

    def test_each_key_is_direct_through_a_divergence_or_different(self):
        r = self.record(compat_report({'format.duration': True, 'streams.duration': True, 'streams.width': False},
                                      via_kd=['streams[1].duration']))
        self.assertEqual(r['keys'], {'format.duration': 'direct', 'streams.duration': 'kd',
                                     'streams.width': 'differs'})
        self.assertTrue(r['compared'])
        self.assertFalse(r['rejected_unexplained'])

    def test_a_missing_report_is_an_error_not_agreement(self):
        self.assertEqual(self.record(None, 2), {'error': 'compat check failed (2)'})

    def test_an_unexplained_rejection_is_kept(self):
        data = compat_report({})
        data.update(compared_files=0, rejected_without_known_divergence=[{'file': 'x.mp4'}])
        r = self.record(data)
        self.assertFalse(r['compared'])
        self.assertTrue(r['rejected_unexplained'])


def summary(per_file):
    keys = {}
    for states in per_file:
        for key, state in states.items():
            t = keys.setdefault(key, {'files': 0, 'direct': 0, 'kd': 0, 'differs': 0})
            t['files'] += 1
            t[state] += 1
    return {'all': {'compat': {'keys': keys, 'compared': len(per_file), 'rejected_unexplained': 0, 'errors': 0,
                               'records': len(per_file)}}}


class Bar(unittest.TestCase):
    def test_every_key_must_reach_the_bar(self):
        # 49 of 50 files agree (98%): met; 48 of 50 (96%): not met.
        met = summary([{'format.duration': 'direct'}] * 49 + [{'format.duration': 'differs'}])
        self.assertTrue(report.compat_bars(met)['met'])
        missed = summary([{'format.duration': 'direct', 'streams.width': 'direct'}] * 48
                         + [{'format.duration': 'direct', 'streams.width': 'differs'}] * 2)
        b = report.compat_bars(missed)
        self.assertFalse(b['met'])
        self.assertEqual(b['worst'][0], 'streams.width')

    def test_agreement_through_a_divergence_counts_for_the_bar_and_not_directly(self):
        s = summary([{'streams.duration': 'kd'}] * 10)
        b = report.compat_bars(s)
        self.assertTrue(b['met'])
        self.assertEqual(b['worst_direct'][1], {'agree': 10, 'direct': 0, 'files': 10})

    def test_a_failed_or_missing_check_counts_against_every_key(self):
        # 49 of 50 agree, but one more file whose check failed: 49/51 < 98%.
        s = summary([{'format.duration': 'direct'}] * 49 + [{'format.duration': 'differs'}])
        s['all']['compat']['errors'] = 1
        b = report.compat_bars(s)
        self.assertEqual(b['keys']['format.duration'], {'agree': 49, 'direct': 49, 'files': 51})
        self.assertFalse(b['met'])
        s['all']['compat'].update(errors=0, not_run=1)
        self.assertFalse(report.compat_bars(s)['met'])

    def test_every_check_failed_is_not_met(self):
        s = summary([])
        s['all']['compat'].update(records=3, errors=3)
        b = report.compat_bars(s)
        self.assertFalse(b['met'])
        self.assertIsNone(b['worst'])

    def test_records_that_all_predate_the_check_are_not_met(self):
        s = summary([])
        s['all']['compat'].update(records=0, not_run=4)
        b = report.compat_bars(s)
        self.assertIsNotNone(b)
        self.assertFalse(b['met'])

    def test_no_compat_records_give_no_bar(self):
        self.assertIsNone(report.compat_bars({'all': {}}))
        self.assertIsNone(report.compat_bars(summary([])))


class Summarize(unittest.TestCase):
    """corpus.summarize() aggregates the records' compat states."""

    def test_states_errors_and_records_without_the_check(self):
        records = {'a' * 64: {'compat': {'compared': True, 'rejected_unexplained': False,
                                         'keys': {'format.duration': 'direct', 'streams.width': 'kd'}}},
                   'b' * 64: {'compat': {'compared': True, 'rejected_unexplained': True,
                                         'keys': {'format.duration': 'differs'}}},
                   'c' * 64: {'compat': {'error': 'compat check failed (2)'}},
                   'd' * 64: {}}
        saved = corpus.entries
        self.addCleanup(setattr, corpus, 'entries', saved)
        corpus.entries = lambda family: [{'sha256': sha, 'holdout': False, 'ext': 'mp4'} for sha in records]
        with tempfile.TemporaryDirectory() as tmp:
            (pathlib.Path(tmp) / 'files').mkdir()
            for sha, extra in records.items():
                (pathlib.Path(tmp) / 'files' / f'{sha}.json').write_text(json.dumps(
                    {'sha256': sha, 'ext': 'mp4', 'holdout': False, 'tools': None,
                     'levels': {'streams': {'items': 1, 'direct': 1, 'via_kd': 0, 'uncovered': 0, 'by_kd': {}}},
                     **extra}))
            c = corpus.summarize(tmp, 'isobmff')['all']['compat']
        self.assertEqual(c['records'], 3)
        self.assertEqual((c['compared'], c['errors'], c['not_run'], c['rejected_unexplained']), (2, 1, 1, 1))
        self.assertEqual(c['keys']['format.duration'], {'files': 2, 'direct': 1, 'kd': 0, 'differs': 1})
        self.assertEqual(c['keys']['streams.width'], {'files': 1, 'direct': 0, 'kd': 1, 'differs': 0})


if __name__ == '__main__':
    unittest.main()
