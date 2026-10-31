"""Tests of the packet-listing speed round's scripts: ts_speed.py (the timing), ts_speed_bars.py (the bars) and
ts_speed_gcp.py (the verdict of a VM's run).

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import ts_speed_bars  # noqa: E402
import ts_speed_gcp  # noqa: E402

MB = 10 ** 6
LINUX_ONLY = unittest.skipUnless(sys.platform == 'linux', 'ts_speed.py pins its runs to a core with taskset, which only Linux has')


def entry(name, size, coreav, lav, null=None, count=10, rounds=3):
    runs = lambda ns: [{'ok': True, 'ns': ns, 'count': count, 'bytes': 100} for _ in range(rounds)]
    return {'name': name, 'bytes': size,
            'warm': {'coreav': runs(coreav)[0], 'null': runs(null or coreav)[0], 'lav': runs(lav)[0]},
            'runs': {'coreav': runs(coreav), 'null': runs(null or coreav), 'lav': runs(lav)}}


def report(files):
    return {'core': 2, 'rounds': 3, 'arms': ['coreav', 'null', 'lav'], 'files': files,
            'freq': [{'label': 'start', 'ghz': 3.09}] + [{'label': f'after {f["name"]}', 'ghz': 3.1} for f in files]}


class Bars(unittest.TestCase):
    def test_classes_and_bars(self):
        files = ([entry(f's{i}', 1 * MB, 80, 100) for i in range(5)] + [entry(f'm{i}', 20 * MB, 90, 100) for i in range(5)]
                 + [entry(f'l{i}', 100 * MB, 120, 100) for i in range(3)])
        s = ts_speed_bars.analyze(report(files))
        self.assertEqual(s['classes']['small']['median'], 0.8)
        self.assertTrue(s['classes']['medium']['judged'])
        self.assertFalse(s['classes']['large']['judged'])
        self.assertEqual(set(s['bars']), {'overall', 'small', 'medium'})
        self.assertTrue(all(s['bars'].values()))
        self.assertEqual(s['overall']['faster'], 10)
        self.assertFalse(s['void'])

    def test_a_judged_class_above_the_bar_misses(self):
        files = [entry(f's{i}', MB, 80, 100) for i in range(20)] + [entry(f'l{i}', 70 * MB, 110, 100) for i in range(5)]
        s = ts_speed_bars.analyze(report(files))
        self.assertTrue(s['bars']['overall'])
        self.assertFalse(s['bars']['large'])

    def test_the_upper_bound_decides_as_well_as_the_median(self):
        ratios = [70, 75, 80, 85, 90, 95, 99, 104, 108, 112, 130]
        s = ts_speed_bars.analyze(report([entry(f'f{i}', MB, r, 100) for i, r in enumerate(ratios)]))
        self.assertLessEqual(s['overall']['median'], 1.0)
        self.assertGreater(s['overall']['high'], 1.0)
        self.assertFalse(s['bars']['overall'])

    def test_the_interval_is_seeded(self):
        values = [0.5 + i / 40 for i in range(40)]
        self.assertEqual(ts_speed_bars.interval(values), ts_speed_bars.interval(values))

    def test_gate(self):
        files = [entry(f's{i}', MB, 80, 100) for i in range(26)] + [entry(f'l{i}', 64 * MB, 96, 100) for i in range(11)]
        s = ts_speed_bars.analyze(report(files), gate=True)
        self.assertTrue(s['bars']['gate overall'])
        self.assertFalse(s['bars']['gate 64 MB or more'])

    def test_a_null_arm_off_its_band_voids_the_run(self):
        s = ts_speed_bars.analyze(report([entry(f'f{i}', MB, 100, 120, null=102) for i in range(5)]))
        self.assertTrue(s['void'])

    def test_refusals(self):
        failed = entry('f', MB, 100, 100)
        failed['runs']['lav'][1] = {'ok': False, 'ns': 5, 'error': 'x'}
        differ = entry('g', MB, 100, 100)
        differ['runs']['null'][0]['count'] = 11
        missing = entry('h', MB, 100, 100)
        del missing['runs']['null']
        for bad, words in ((failed, 'a run of lav failed'), (differ, 'different access units'),
                           (missing, 'no runs of null'), (entry('i', 201 * MB, 1, 1), 'over the 200 MB')):
            with self.assertRaisesRegex(ts_speed_bars.Refused, words):
                ts_speed_bars.analyze(report([bad]))


class Timing(unittest.TestCase):
    """ts_speed.py end to end, with stand-in engines that print fixed lines."""

    @LINUX_ONLY
    def test_a_run_records_every_arm_and_round(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            engine = d / 'engine'
            engine.write_text('#!/bin/sh\necho \'{"ok":true,"ns":1000,"count":3,"bytes":30}\'\n')
            engine.chmod(0o755)
            shutil.copy(engine, d / 'null')
            lav = d / 'lav'
            lav.write_text('#!/bin/sh\necho \'{"ok":true,"ns":2000,"count":3,"bytes":30,"error":""}\'\n')
            lav.chmod(0o755)
            (d / 'tmp').mkdir()
            files = []
            for n in ('a.ts', 'b.ts'):
                (d / n).write_bytes(b'\x47' + bytes(187))
                files.append(str(d / n))
            out = d / 'r.json'
            subprocess.run([sys.executable, '-I', str(SCRIPTS / 'ts_speed.py'), '--coreav', str(engine), '--null',
                            str(d / 'null'), '--lav', str(lav), '--core', str(min(os.sched_getaffinity(0))), '--rounds', '3',
                            '--tmp', str(d / 'tmp'),
                            '--report', str(out)] + files, check=True, capture_output=True)
            r = json.loads(out.read_text())
            self.assertEqual([f['name'] for f in r['files']], ['a.ts', 'b.ts'])
            self.assertEqual({arm: len(v) for arm, v in r['files'][0]['runs'].items()}, {'coreav': 3, 'null': 3, 'lav': 3})
            self.assertEqual([s['label'] for s in r['freq']], ['start', 'after a.ts', 'after b.ts'])
            self.assertEqual(ts_speed_bars.analyze(r)['overall']['median'], 0.5)

    @LINUX_ONLY
    def test_a_listing_that_exits_nonzero_failed_whatever_its_line_says(self):
        import ts_speed
        with tempfile.TemporaryDirectory() as d:
            engine = pathlib.Path(d) / 'engine'
            engine.write_text('#!/bin/sh\necho \'{"ok":true,"ns":1000,"count":3,"bytes":30}\'\nexit 9\n')
            engine.chmod(0o755)
            args = type('A', (), {'core': min(os.sched_getaffinity(0)), 'coreav': str(engine), 'null': str(engine), 'lav': str(engine)})()
            result = ts_speed.run_arm('coreav', args, str(engine))
        self.assertIs(result['ok'], False)
        self.assertIn('exit 9', result['error'])

    def test_the_frequency_is_sampled_over_a_busy_loop_not_a_file(self):
        import ts_speed
        seen = []

        def run(argv, **kwargs):
            seen.append(argv)
            return subprocess.CompletedProcess(argv, 0, '', '1550000000,,cycles:u,\n500.0,msec,task-clock:u,\n')

        real, ts_speed.subprocess.run = ts_speed.subprocess.run, run
        try:
            sample = ts_speed.frequency(type('A', (), {'core': 2})(), 'start')
        finally:
            ts_speed.subprocess.run = real
        self.assertAlmostEqual(sample['ghz'], 3.1)
        self.assertEqual(seen[0][-2:], ['-c', ts_speed.SPIN])
        self.assertIn('2', seen[0])

    def test_the_null_arm_must_be_the_same_binary(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            (d / 'a').write_text('#!/bin/sh\n')
            (d / 'b').write_text('#!/bin/sh\n\n')
            r = subprocess.run([sys.executable, '-I', str(SCRIPTS / 'ts_speed.py'), '--coreav', str(d / 'a'), '--null',
                                str(d / 'b'), '--lav', str(d / 'a'), '--report', str(d / 'r.json'), str(d / 'a')],
                               capture_output=True, text=True)
            self.assertEqual(r.returncode, 2)
            self.assertIn('byte-identical', r.stderr)


class Verdict(unittest.TestCase):
    HOST = 'CPU(s):                4\nModel name:            INTEL(R) XEON(R) PLATINUM 8581C CPU @ 2.30GHz\nsmt active: 0\n'

    def out(self, d, files, host=HOST, turbo='ALL_CORE_MAX', rc=0, rep=None):
        d = pathlib.Path(d)
        (d / 'instance.json').write_text(json.dumps({'advanced_machine_features': {'turboMode': turbo}}))
        (d / 'host.txt').write_text(host)
        (d / 'progress.txt').write_text(f'start 00:00:00\nrc={rc} 00:10:00\n')
        (d / 'report.json').write_text(json.dumps(rep or report(files)))
        return str(d)

    def test_a_clean_run_is_not_void(self):
        files = [entry(f'f{i}', MB, 90, 100) for i in range(5)]
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(ts_speed_gcp.problems(self.out(d, files), {'files': [f['name'] for f in files]}), [])

    def test_void_reasons(self):
        files = [entry(f'f{i}', MB, 90, 100) for i in range(5)]
        rep = report(files)
        rep['freq'][2]['ghz'] = 3.3
        del rep['freq'][3]
        with tempfile.TemporaryDirectory() as d:
            found = ts_speed_gcp.problems(
                self.out(d, files, host=self.HOST.replace('8581C', '8481C').replace(': 0', ': 1'), turbo=None, rc=1,
                         rep=rep), {'files': [f['name'] for f in files] + ['missing']})
        text = '\n'.join(found)
        for words in ('turboMode', 'not a Xeon 8581C', 'SMT is not off', 'status 0', '5 of the 6 files',
                      "no frequency sample 'after f2'", "'after f1': 3.3", "no frequency sample 'after missing'"):
            self.assertIn(words, text)

    def judge(self, d, files, rep, **host):
        out = self.out(d, files, rep=rep, **host)
        rc = ts_speed_gcp.judge(out, 0, {'files': [f['name'] for f in files]})
        return rc, json.loads((pathlib.Path(out) / 'verdict.json').read_text())

    def test_an_engine_refusal_fails_the_run_and_does_not_void_it(self):
        files = [entry(f'f{i}', MB, 90, 100) for i in range(5)]
        rep = report(files)
        rep['files'][1]['runs']['lav'][0] = {'ok': False, 'ns': None}
        with tempfile.TemporaryDirectory() as d:
            rc, verdict = self.judge(d, files, rep)
        self.assertEqual(rc, 5)
        self.assertEqual((verdict['failed'], verdict['void']), (True, False))
        self.assertIn('f1: a run of lav failed', verdict['failures'][0])

    def test_a_null_copy_that_lists_other_units_fails_the_run_even_beside_a_host_fault(self):
        files = [entry(f'f{i}', MB, 90, 100) for i in range(5)]
        rep = report(files)
        for run in rep['files'][2]['runs']['null']:
            run['count'] = 11
        with tempfile.TemporaryDirectory() as d:
            rc, verdict = self.judge(d, files, rep, turbo=None)
        self.assertEqual(rc, 5)
        self.assertEqual((verdict['failed'], verdict['void']), (True, False))
        self.assertIn('null arm list different access units', verdict['failures'][0])
        self.assertTrue(any('turboMode' in r for r in verdict['reasons']))

    def test_a_failed_warm_up_fails_the_run(self):
        files = [entry(f'f{i}', MB, 90, 100) for i in range(5)]
        rep = report(files)
        rep['files'][0]['warm']['coreav'] = {'ok': False, 'ns': None}
        with tempfile.TemporaryDirectory() as d:
            rc, verdict = self.judge(d, files, rep)
        self.assertEqual(rc, 5)
        self.assertIn('f0: the warm-up run of coreav failed', verdict['failures'][0])

    def test_a_warm_up_null_copy_that_lists_other_units_fails_the_run(self):
        files = [entry(f'f{i}', MB, 90, 100) for i in range(5)]
        rep = report(files)
        rep['files'][3]['warm']['null']['count'] = 99
        with tempfile.TemporaryDirectory() as d:
            rc, verdict = self.judge(d, files, rep)
        self.assertEqual(rc, 5)
        self.assertIn('f3: CoreAV and its null arm list different access units', verdict['failures'][0])

    def test_a_job_cut_short_before_any_file_is_void_not_failed(self):
        files = [entry('f0', MB, 90, 100)]
        rep = report([])
        with tempfile.TemporaryDirectory() as d:
            rc, verdict = self.judge(d, files, rep, rc=1)
        self.assertEqual(rc, 3)
        self.assertEqual((verdict['failed'], verdict['void']), (False, True))

    def launch(self, d, *extra):
        d = pathlib.Path(d)
        f = d / 'a.ts'
        f.write_bytes(b'\x47' + bytes(187))
        (d / 'lav').mkdir()
        return subprocess.run([sys.executable, '-I', str(SCRIPTS / 'ts_speed_gcp.py'), '--out', str(d / 'run'), '--coreav',
                               str(f), '--lav-dir', str(d / 'lav'), '--dry-run', *extra, str(f)],
                              capture_output=True, text=True), d / 'run'

    def test_a_cap_or_a_duration_that_is_not_a_finite_positive_number_refuses_to_start(self):
        for extra in (['--cap-usd', 'nan'], ['--cap-usd', 'inf'], ['--cap-usd', '-1'], ['--max-hours', 'nan'],
                      ['--max-hours', 'inf', '--cap-usd', '2'], ['--max-hours', '0'], ['--rounds', '0']):
            with self.subTest(extra=extra), tempfile.TemporaryDirectory() as d:
                r, out = self.launch(d, *extra)
                self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
                self.assertFalse(out.exists())
                self.assertNotIn('dry run', r.stdout)

    def test_a_valid_launch_reaches_the_dry_run(self):
        with tempfile.TemporaryDirectory() as d:
            r, out = self.launch(d, '--cap-usd', '2', '--max-hours', '1')
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertIn('dry run', r.stdout)

    def test_the_job_names_its_class(self):
        script = ts_speed_gcp.job_script({'files': ['a.ts'], 'rounds': 7, 'commands': False})
        self.assertIn('--core 2 --rounds 7 --tmp /dev/shm', script)
        self.assertIn('files/a.ts', script)
        self.assertIn('linux-perf', script)


if __name__ == '__main__':
    unittest.main()
