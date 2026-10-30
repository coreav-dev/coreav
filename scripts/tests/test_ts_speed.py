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


def entry(name, size, coreav, lav, null=None, count=10, rounds=3):
    runs = lambda ns: [{'ok': True, 'ns': ns, 'count': count, 'bytes': 100} for _ in range(rounds)]
    return {'name': name, 'bytes': size,
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
                            str(d / 'null'), '--lav', str(lav), '--core', '0', '--rounds', '3', '--tmp', str(d / 'tmp'),
                            '--report', str(out)] + files, check=True, capture_output=True)
            r = json.loads(out.read_text())
            self.assertEqual([f['name'] for f in r['files']], ['a.ts', 'b.ts'])
            self.assertEqual({arm: len(v) for arm, v in r['files'][0]['runs'].items()}, {'coreav': 3, 'null': 3, 'lav': 3})
            self.assertEqual([s['label'] for s in r['freq']], ['start', 'after a.ts', 'after b.ts'])
            self.assertEqual(ts_speed_bars.analyze(r)['overall']['median'], 0.5)

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

    def test_the_job_names_its_class(self):
        script = ts_speed_gcp.job_script({'files': ['a.ts'], 'rounds': 7, 'commands': False})
        self.assertIn('--core 2 --rounds 7 --tmp /dev/shm', script)
        self.assertIn('files/a.ts', script)
        self.assertIn('linux-perf', script)


if __name__ == '__main__':
    unittest.main()
