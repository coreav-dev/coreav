import copy
import gzip
import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
NAMES = ['a084bd45168c', '70eec00bc7d2', '584a16b906bb', 'bb5d39d47357']
FAKE = '''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
packets = int(args[args.index('--packets') + 1])
threads = int(os.environ['COREAV_DIAG_THREADS'])
threads = int(os.environ.get('FAKE_THREADS', 4 if threads == 0 else threads))
print(f"DIAG_CONFIG threads={threads} row_mt={os.environ['COREAV_DIAG_ROW_MT']}", file=sys.stderr)
for line in os.environ.get('FAKE_PERF', '').split('|'):
    if line:
        print(line, file=sys.stderr)
frames = 0 if packets == 0 else int(os.environ.get('FAKE_FRAMES', 3))
fed = int(os.environ.get('FAKE_FED', 0 if packets == 0 else frames))
print(json.dumps(dict(decoder=os.environ.get('FAKE_DECODER', 'libvpx 1.14.1'), fed=fed,
                      frames=[dict(md5='d%d' % n) for n in range(frames)])))
'''


class AttributionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        (self.root / 'mimalloc-arena-reserve-kib.txt').write_text('131072\n')
        files = [dict(sha256=name + '0' * 52, ext='webm', maps=dict(video=0)) for name in NAMES]
        (self.root / 'maps.json').write_text(json.dumps(dict(files=files)))
        variants = ['musl-plain', 'musl-chunk', 'musl-count', 'gnu-pinned-plain',
                    'gnu-pinned-count', 'gnu-debian-plain', 'gnu-debian-count']
        binaries = set(variants) | {'ffmpeg'} | {variant + '-hash' for variant in variants
                                                 if not variant.endswith('-count')}
        self.builds = dict(variants={variant: dict(vpx_version='1.15.0' if 'debian' in variant else '1.14.1')
                                     for variant in variants},
                           binaries={name: '0' * 64 for name in sorted(binaries)})
        (self.root / 'builds.json').write_text(json.dumps(self.builds))
        (self.root / 'out').mkdir()
        (self.root / 'bin').mkdir()
        fake = self.root / 'bin' / 'musl-plain'
        fake.write_text(FAKE)
        fake.chmod(0o755)
        spec = importlib.util.spec_from_file_location('vp9_release_attribution', SCRIPTS / 'vp9_release_attribution.py')
        self.runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.runner)
        self.runner.load(self.root, self.root / 'out')
        self.out = self.runner.OUT
        self.units = {name: 2 for name in NAMES}
        self.counts = {name: 3 for name in NAMES}
        self.rows = self.complete_rows()

    def complete_rows(self):
        runner = self.runner
        rows = []
        for name in NAMES:
            for variant, threads, row_mt in runner.CELLS:
                cell = dict(file=name, variant=variant, threads_requested=threads, row_mt_requested=row_mt, rc=0)
                for r in range(5):
                    rows.append(dict(cell, kind='nondecode', round=r, frames=0, fed=0, cpu=0.01, wall=0.01))
                    for n in range(2):
                        rows.append(dict(cell, kind='timed', round=r, repetition=n, frames=3, cpu=1.0, wall=0.5))
                rows.append(dict(cell, kind='counter', frames=3, instructions=10, cycles=20,
                                 counter_events=['instructions', 'cycles']))
                if not variant.endswith(('-count', '-aa')):
                    rows.append(dict(cell, kind='pixel-identity', frames=3, matches=True))
        for variant, threads, row_mt in runner.PROFILE_CELLS:
            for n in range(runner.PROFILE_REPETITIONS):
                rows.append(dict(kind='profile-work', file=runner.PROFILE_FILE, variant=variant,
                                 threads_requested=threads, row_mt_requested=row_mt, repetition=n, frames=3, rc=0))
            (self.out / f'{variant}-t{threads}-r{row_mt}.profile.txt').write_text('cycles\n')
        (self.out / 'gate.json').write_text(json.dumps(dict(before=1.0, after=1.0, ratio=1.0)))
        return rows

    def analyse(self, rows):
        (self.out / 'calls.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in rows))
        return self.runner.analyse(self.units, self.counts)

    def test_a_complete_report_summarises_every_cell_without_a_verdict(self):
        report = self.analyse(self.rows)
        self.assertTrue(report['diagnostic_only'])
        self.assertNotIn('passed', report)
        self.assertTrue(report['host_valid'])
        cell = report['files']['584a16b906bb']['gnu-pinned-plain:t1:r0']
        self.assertEqual(cell['median'], 1.0)
        self.assertEqual(cell['wall']['median'], 0.5)
        self.assertEqual(cell['nondecode_cpu']['median'], 0.01)
        self.assertEqual((cell['instructions'], cell['cycles']), (10, 20))
        self.assertEqual(len(report['files']['a084bd45168c']), 25)

    def test_each_roster_refuses_a_missing_or_duplicate_call(self):
        kinds = ['timed', 'nondecode', 'counter', 'pixel-identity', 'profile-work']
        for kind in kinds:
            for mutate in ['missing', 'duplicate']:
                with self.subTest(kind=kind, mutate=mutate):
                    rows = copy.deepcopy(self.rows)
                    row = next(row for row in rows if row['kind'] == kind)
                    if mutate == 'missing':
                        rows.remove(row)
                    else:
                        rows.append(row)
                    with self.assertRaises(RuntimeError):
                        self.analyse(rows)

    def test_work_events_and_profiles_are_checked(self):
        mutations = [('nondecode', 'fed', 1), ('nondecode', 'frames', 1), ('timed', 'frames', 2),
                     ('counter', 'cycles', 0), ('counter', 'counter_events', ['instructions:u', 'cycles:u']),
                     ('pixel-identity', 'matches', False), ('profile-work', 'frames', 2), ('timed', 'rc', 1)]
        for kind, field, value in mutations:
            with self.subTest(kind=kind, field=field):
                rows = copy.deepcopy(self.rows)
                next(row for row in rows if row['kind'] == kind)[field] = value
                with self.assertRaises(RuntimeError):
                    self.analyse(rows)
        (self.out / 'musl-count-t1-r1.profile.txt').write_text('')
        with self.assertRaises(RuntimeError):
            self.analyse(self.rows)

    def test_host_controls_mark_the_diagnostic_invalid(self):
        rows = copy.deepcopy(self.rows)
        for row in rows:
            if row['kind'] == 'timed' and row['variant'] == 'musl-plain-aa':
                row['cpu'] = 1.03
        self.assertFalse(self.analyse(rows)['host_valid'])
        (self.out / 'gate.json').write_text(json.dumps(dict(before=1.0, after=1.06, ratio=1.06)))
        self.assertFalse(self.analyse(self.rows)['host_valid'])

    def test_the_build_manifest_names_every_variant_and_libvpx_version(self):
        self.runner.check_manifest()
        del self.runner.BUILDS['variants']['gnu-pinned-count']
        with self.assertRaisesRegex(RuntimeError, 'variant roster'):
            self.runner.verify()

    def replay(self, rows, **files):
        evidence = self.out
        for name in ['maps.json', 'builds.json']:
            (evidence / name).write_bytes((self.root / name).read_bytes())
        warm = [dict(kind='warm', file=name, variant='musl-plain', threads_requested=0, row_mt_requested=1,
                     rc=0, frames=3, cpu=0.9, wall=0.5) for name in NAMES]
        with gzip.open(evidence / 'calls.jsonl.gz', 'wt') as stream:
            stream.write(''.join(json.dumps(row) + '\n' for row in warm + rows))
        (evidence / 'completed.json').write_text(json.dumps(dict(files=4, cells=25, rounds=5, units=self.units)))
        for name, text in files.items():
            (evidence / name).write_text(text)
        return subprocess.run([sys.executable, '-I', str(SCRIPTS / 'vp9_release_attribution.py'),
                               '--analyse', str(evidence)], capture_output=True, text=True,
                              cwd=tempfile.gettempdir())

    def test_published_evidence_replays_without_binaries_media_or_job_directory(self):
        expected = self.analyse(self.rows)
        (self.out / 'calls.jsonl').unlink()
        (self.out / 'analysis.json').unlink()
        result = self.replay(self.rows)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads((self.out / 'analysis.json').read_text()), expected)
        self.assertEqual(self.replay(self.rows, **{'gate.json': json.dumps(dict(ratio=1.06))}).returncode, 3)

    def test_replay_refuses_aborted_incomplete_or_unsupported_evidence(self):
        cases = [(self.rows, {'aborted.json': '{}'}),
                 (self.rows, {'completed.json': json.dumps(dict(files=4, cells=25, rounds=5,
                                                                units=dict(self.units, a084bd45168c=3)))}),
                 (self.rows[1:], {}),
                 (self.rows + [dict(kind='warm', file='a084bd45168c', variant='musl-plain', threads_requested=0,
                                    row_mt_requested=1, rc=0, frames=3, wall=0.5)], {})]
        for rows, files in cases:
            with self.subTest(files=sorted(files), rows=len(rows)):
                (self.out / 'aborted.json').unlink(missing_ok=True)
                result = self.replay(rows, **files)
                self.assertNotIn(result.returncode, (0, 3), result.stdout)
                self.assertIn('RuntimeError', result.stderr)

    def decode(self, cell, kind, expected, **environment):
        file = next(file for file in self.runner.FILES if file['sha256'].startswith('584a16'))
        with mock.patch.dict(os.environ, environment):
            return self.runner.decode(cell, file, kind, expected=expected)

    def last_call(self):
        return json.loads((self.out / 'calls.jsonl').read_text().splitlines()[-1])

    def test_decode_records_the_configured_decoder_and_real_frames(self):
        row = self.decode(('musl-plain', 0, 1), 'timed', 3)
        self.assertEqual((row['configured_threads'], row['row_mt'], row['frames'], row['fed']), (4, 1, 3, 3))
        self.assertEqual(self.decode(('musl-plain-aa', 2, 0), 'nondecode', 0)['frames'], 0)

    def test_decode_refuses_wrong_configuration_work_or_library(self):
        failures = [(('musl-plain', 0, 1), 'timed', 3, dict(FAKE_THREADS='2')),
                    (('musl-plain', 1, 0), 'timed', 3, dict(FAKE_FRAMES='2')),
                    (('musl-plain', 1, 1), 'timed', None, dict(FAKE_FRAMES='0')),
                    (('musl-plain', 0, 1), 'nondecode', 0, dict(FAKE_FED='1')),
                    (('musl-plain', 0, 1), 'timed', 3, dict(FAKE_DECODER='libvpx 1.15.0'))]
        for cell, kind, expected, environment in failures:
            with self.subTest(environment=environment):
                with self.assertRaises(RuntimeError):
                    self.decode(cell, kind, expected, **environment)
                self.assertIn('error', self.last_call())

    def test_counters_parse_the_event_scope_perf_reports(self):
        measured = self.runner.measurement.measured

        def without_perf(cmd, label, read_output=None):
            return measured(cmd[cmd.index('--') + 1:], label, read_output)

        file = next(file for file in self.runner.FILES if file['sha256'].startswith('584a16'))
        with mock.patch.object(self.runner.measurement, 'measured', without_perf):
            with mock.patch.dict(os.environ, FAKE_PERF='120;;instructions:u;1;100.00|240;;cycles:u;1;100.00'):
                row = self.runner.decode(('musl-plain', 0, 1), file, 'counter', expected=3, counter=True)
            self.assertEqual((row['instructions'], row['cycles']), (120, 240))
            self.assertEqual(row['counter_events'], ['instructions:u', 'cycles:u'])
            with mock.patch.dict(os.environ, FAKE_PERF='<not counted>;;instructions;0;0|240;;cycles;1;100.00'):
                with self.assertRaises(ValueError):
                    self.runner.decode(('musl-plain', 0, 1), file, 'counter', expected=3, counter=True)


if __name__ == '__main__':
    unittest.main()
