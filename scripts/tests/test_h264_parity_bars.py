"""scripts/h264_parity_bars.py on round 17's committed reports (docs/measurements/h264-rounds/round17/gating/).

Round 17's own analyzer (`analyze.py` in that directory) wrote `t8.summary.json` there: the shared analyzer, given
round 17's bars as a JSON file, must reproduce its medians, intervals and verdicts exactly, and must refuse the same
reports once a stream is missing or failed, which round 17's analyzer passed.

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
import contextlib
import io
import json
import pathlib
import shutil
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import h264_parity_bars as bars  # noqa: E402

ROUND17 = ROOT / 'docs' / 'measurements' / 'h264-rounds' / 'round17' / 'gating'
TRAINING = 'docs/measurements/h264-training/es.json'
CLASSES = ('sd', '720p', '1080p', '4k')
# Round 17's bars B-1 to B-3 (docs/design/h264.md, "Round 17"), as its analyze.py states them.
SPEC = {
    'new': 'after', 'old': 'before', 'seed': 20261024,
    'calls': {'t8': {'label': '8 threads, 32 training', 'streams': TRAINING},
              'g': {'label': '1 thread pinned, 32 training', 'streams': TRAINING}},
    'bars': [
        {'name': 'B-1', 'checks': [{'call': 't8', 'pairing': 'discard', 'median_max': 0.980, 'ci_high_below': 1.0}]},
        {'name': 'B-2', 'checks': [{'call': 't8', 'pairing': 'discard', 'groups': ['classes'], 'median_max': 1.005},
                                   {'call': 't8', 'pairing': 'write', 'median_max': 1.005}]},
        {'name': 'B-3', 'checks': [{'call': 'g', 'pairing': 'discard', 'groups': ['overall', 'classes'],
                                    'median_max': 1.005},
                                   {'call': 'g', 'pairing': 'write', 'median_max': 1.005}]},
    ],
}


class Round17(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name) / 'reports'
        self.dir.mkdir()
        for pre in ('t8', 'g'):
            for cls in CLASSES:
                shutil.copy(ROUND17 / f'{pre}.{cls}.json', self.dir)
        self.bars = pathlib.Path(self.tmp.name) / 'bars.json'
        self.write_spec(SPEC)

    def tearDown(self):
        self.tmp.cleanup()

    def write_spec(self, spec):
        self.bars.write_text(json.dumps(spec))

    def edit(self, name, change):
        path = self.dir / name
        report = json.loads(path.read_text())
        change(report)
        path.write_text(json.dumps(report))

    def run_main(self, *extra):
        out, err = io.StringIO(), io.StringIO()
        argv = sys.argv
        sys.argv = ['h264_parity_bars.py', str(self.bars), str(self.dir), *extra]
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                status = bars.main()
        finally:
            sys.argv = argv
        return status, out.getvalue(), err.getvalue()

    def test_it_reproduces_round_17s_own_analysis(self):
        summary = bars.analyze(bars.load_spec(str(self.bars)), str(self.dir))
        old = json.loads((ROUND17 / 't8.summary.json').read_text())
        for call, key in (('t8', 'threads8'), ('g', 'threads1')):
            for p in ('discard', 'write'):
                for group in ('overall',) + CLASSES:
                    new_cell, old_cell = summary['calls'][call][p][group], old[key][p][group]
                    self.assertEqual((new_cell['n'], new_cell['median'], new_cell['ci95'], new_cell['faster']),
                                     (old_cell['n'], old_cell['median'], old_cell['ci95'], old_cell['faster_after']),
                                     (call, p, group))
        self.assertEqual(summary['bars'], old['bars'])
        self.assertEqual(self.run_main()[0], 0)

    def test_a_missing_stream_is_refused_and_named(self):
        # the past mistake: the harness printed "FAILED, skipped" and left the stream out; round 17's analyzer
        # still evaluated B-1 to B-3 on the 31 left
        gone = []
        self.edit('t8.sd.json', lambda r: gone.append(r['streams'].pop(3)['stream']))
        status, out, err = self.run_main()
        self.assertEqual(status, bars.REFUSED)
        self.assertIn(f'no row for: {gone[0]}', err)
        self.assertNotIn('bars:', out)

    def test_a_failure_the_report_lists_is_refused(self):
        def fail(report):
            row = report['streams'].pop(0)
            report['requested'] = [x['stream'] for x in report['streams']] + [row['stream']]
            report['failed'] = [{'stream': row['stream'], 'class': 'sd', 'reason': 'after_discard exited with an error'}]
        self.edit('g.sd.json', fail)
        status, _, err = self.run_main()
        self.assertEqual(status, bars.REFUSED)
        self.assertIn('the harness failed on', err)
        self.assertIn('after_discard exited with an error', err)

    def test_a_call_not_asked_for_the_whole_set_is_refused(self):
        # a runner that passed the harness seven of the eight streams: the report is complete for what it
        # was asked, and still not the set
        def short(report):
            report['streams'].pop()
            report['requested'] = [x['stream'] for x in report['streams']]
            report['failed'] = []
        self.edit('t8.4k.json', short)
        status, _, err = self.run_main()
        self.assertEqual(status, bars.REFUSED)
        self.assertIn('t8.4k: the call was not asked for exactly the set; not asked for:', err)

    def test_a_missing_class_report_is_refused(self):
        (self.dir / 'g.1080p.json').unlink()
        status, _, err = self.run_main()
        self.assertEqual(status, bars.REFUSED)
        self.assertIn('g.1080p: no report; its 8 streams are missing', err)

    def test_a_row_without_both_builds_is_refused(self):
        self.edit('t8.720p.json', lambda r: r['streams'][0]['seconds'].pop('after_write'))
        status, _, err = self.run_main()
        self.assertEqual(status, bars.REFUSED)
        self.assertIn('has no after_write', err)

    def test_a_row_twice_is_refused(self):
        self.edit('t8.sd.json', lambda r: r['streams'].append(r['streams'][0]))
        self.assertIn('more than one row for', self.run_main()[2])

    def test_a_call_without_reports_is_pending(self):
        for cls in CLASSES:
            (self.dir / f'g.{cls}.json').unlink()
        status, out, _ = self.run_main()
        self.assertEqual(status, 1)
        self.assertIn('B-1 pass, B-2 pass, B-3 pending', out)

    def test_a_bar_that_fails_exits_1(self):
        spec = json.loads(json.dumps(SPEC))
        spec['bars'][0]['checks'][0]['median_max'] = 0.95
        self.write_spec(spec)
        status, out, _ = self.run_main()
        self.assertEqual(status, 1)
        self.assertIn('B-1 FAIL', out)

    def test_a_misspelt_key_is_refused(self):
        spec = json.loads(json.dumps(SPEC))
        spec['bars'][0]['checks'][0]['median_mx'] = spec['bars'][0]['checks'][0].pop('median_max')
        self.write_spec(spec)
        status, _, err = self.run_main()
        self.assertEqual(status, bars.REFUSED)
        self.assertIn("unknown keys ['median_mx']", err)

    def test_an_empty_group_list_is_refused(self):
        # an empty list would compare nothing and leave the bar passing: B-1 at 0.95 fails on these reports
        spec = json.loads(json.dumps(SPEC))
        spec['bars'][0]['checks'][0].update(median_max=0.95, groups=[])
        self.write_spec(spec)
        status, out, err = self.run_main()
        self.assertEqual(status, bars.REFUSED)
        self.assertIn('bar B-1: `groups` is empty', err)
        self.assertNotIn('bars:', out)

    def test_an_empty_bar_list_is_refused(self):
        for spec in (dict(SPEC, bars=[]), {'new': 'after', 'old': 'before', 'calls': {}, 'bars': []}):
            with self.subTest(spec=spec):
                self.write_spec(spec)
                status, out, err = self.run_main()
                self.assertEqual(status, bars.REFUSED, out)
                self.assertIn('`bars` must list at least one bar', err)

    def test_times_and_ratios_that_are_not_finite_positive_numbers_are_refused(self):
        # zero times passed every bar (a ratio new/old of 0), and a zero denominator raised ZeroDivisionError
        cases = [('seconds', 'after_discard', 0), ('seconds', 'after_write', -0.5),
                 ('seconds', 'before_discard', 0.0), ('seconds', 'after_discard', float('nan')),
                 ('seconds', 'before_write', float('inf')), ('seconds', 'after_discard', '0.9'),
                 ('seconds', 'after_discard', True), ('ratio', 'after:discard', float('nan')),
                 ('ratio', 'before:write', 0)]
        for table, key, value in cases:
            with self.subTest(table=table, key=key, value=value):
                self.setUp()
                stream = []

                def spoil(report):
                    stream.append(report['streams'][0]['stream'])
                    report['streams'][0][table][key] = value
                self.edit('t8.sd.json', spoil)
                status, out, err = self.run_main()
                self.assertEqual(status, bars.REFUSED, out)
                self.assertIn(f't8.sd: the row of {stream[0]} has values that are not finite positive numbers: '
                              f'{table}.{key} = {value!r}', err)
                self.assertNotIn('bars:', out)
                self.tearDown()
        self.setUp()

    def test_two_bars_of_one_name_are_refused(self):
        # the passing second B-1 would replace the failing first in the verdicts
        spec = json.loads(json.dumps(SPEC))
        spec['bars'][0]['checks'][0]['median_max'] = 0.95
        spec['bars'].append(dict(spec['bars'][1], name='B-1'))
        self.write_spec(spec)
        status, _, err = self.run_main()
        self.assertEqual(status, bars.REFUSED)
        self.assertIn('bars named more than once: B-1', err)

    def test_a_subset_outside_a_calls_set_is_left_out_of_that_call(self):
        # a training-only subset beside a held-out call: the held-out call has no such group, and a check that
        # asks for it there is refused
        heldout = 'docs/measurements/h264-n2-heldout/es.json'
        spec = json.loads(json.dumps(SPEC))
        spec['subsets'] = {'cavlc': ['5185721dda11', 'ec473c99c538']}
        spec['calls']['h'] = {'label': '1 thread pinned, 64 held-out', 'streams': heldout}
        manifest = json.loads((ROOT / heldout).read_text())
        for cls in CLASSES:
            want = sorted(n for n, e in manifest.items() if e['class'] == cls)
            report = json.loads((ROUND17 / f'g.{cls}.json').read_text())
            report['streams'] = [dict(report['streams'][i % len(report['streams'])], stream=n)
                                 for i, n in enumerate(want)]
            (self.dir / f'h.{cls}.json').write_text(json.dumps(report))
        spec['bars'].append({'name': 'H', 'checks': [{'call': 'h', 'pairing': 'discard', 'median_max': 2.0}]})
        self.write_spec(spec)
        status, out, err = self.run_main()
        self.assertEqual(status, 0, err)
        self.assertIn('H pass', out)
        spec['bars'][-1]['checks'][0]['groups'] = ['cavlc']
        self.write_spec(spec)
        status, _, err = self.run_main()
        self.assertEqual(status, bars.REFUSED)
        self.assertIn("subset cavlc has no stream in call h's set", err)

    def test_a_subset_named_per_stream_is_refused(self):
        # the summary keeps every stream's ratio under `per_stream` beside the groups: a subset of that name had
        # its statistics replaced by those ratios, and a check on it raised KeyError('median')
        spec = json.loads(json.dumps(SPEC))
        spec['subsets'] = {'per_stream': ['5185721dda11', 'ec473c99c538']}
        check = {'call': 'g', 'pairing': 'discard', 'groups': ['per_stream'], 'median_max': 2.0}
        for extra in ([], [{'name': 'S', 'checks': [check]}]):
            with self.subTest(extra=extra):
                self.write_spec(dict(spec, bars=SPEC['bars'] + extra))
                status, out, err = self.run_main()
                self.assertEqual(status, bars.REFUSED, out)
                self.assertIn('subset per_stream: a reserved name', err)
                self.assertNotIn('bars:', out)

    def test_subsets_and_the_summary_file(self):
        spec = json.loads(json.dumps(SPEC))
        spec['subsets'] = {'cavlc': ['5185721dda11', 'ec473c99c538']}
        spec['bars'].append({'name': 'S', 'checks': [{'call': 'g', 'pairing': 'discard', 'groups': ['cavlc'],
                                                      'median_max': 2.0}]})
        self.write_spec(spec)
        path = pathlib.Path(self.tmp.name) / 'summary.json'
        self.assertEqual(self.run_main('--summary', str(path))[0], 0)
        summary = json.loads(path.read_text())
        self.assertEqual(summary['calls']['g']['discard']['cavlc']['n'], 2)
        self.assertEqual(len(summary['calls']['t8']['write']['per_stream']), 32)


if __name__ == '__main__':
    unittest.main()
