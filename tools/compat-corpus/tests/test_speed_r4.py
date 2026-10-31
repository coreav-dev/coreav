"""speed.py's R4 runner and report (docs/design/compat-cli.md, "The 0.1.0
bars"), run against stand-in programs: no FFmpeg, no media, no timing
claims."""
import json
import os
import pathlib
import random
import shutil
import stat
import statistics
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import release_round  # noqa: E402
import speed  # noqa: E402

# Records every call; `ffprobe -show_streams` lists one video and one audio
# stream; an ffmpeg command writes its output and fails if the file is
# there already; `coreav` refuses a command that seeks (`-ss`).
FAKE = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
name, args = pathlib.Path(sys.argv[0]).name, sys.argv[1:]
assert os.environ['GCONV_PATH'] == '/nonexistent/gconv' and os.environ['COREAV_STRICT_SAFE'] == '1'
with open(os.environ['FAKE_CALLS'], 'a') as f:
    f.write(json.dumps([name] + args) + '\n')
if name == 'coreav' and '-ss' in args:
    with open(os.environ['COREAV_FALLBACK_LOG'], 'a') as f:
        f.write(json.dumps({'ran': None, 'not_run': 'strict_safe', 'tool': args[0],
                            'reason': "no video is presented in the clip's range"}) + '\n')
    sys.exit(4)
tool = args[0] if name == 'coreav' else name
if tool == 'ffprobe' and '-show_streams' in args and '-of' in args and 'json' in args:
    print(json.dumps({'streams': [{'index': 0, 'codec_type': 'video'}, {'index': 1, 'codec_type': 'audio'}]}))
if tool == 'ffmpeg':
    out = pathlib.Path(args[-1].replace('%03d', '000'))
    if out.exists():
        sys.exit(9)
    out.write_bytes(b'x')
'''


class Runner(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        d = pathlib.Path(cls.tmp.name)
        cls.bin = d / 'bin'
        cls.bin.mkdir()
        for name in ('coreav', 'ffmpeg', 'ffprobe'):
            (cls.bin / name).write_text(FAKE)
            (cls.bin / name).chmod(0o755 | stat.S_IEXEC)
        media = d / 'media'
        media.mkdir()
        for name in ('a.mp4', 'b.webm', 'c.ts'):
            (media / name).write_bytes(b'')
        cls.files = d / 'files.json'
        cls.files.write_text(json.dumps([{'family': 'mp4-mov', 'path': str(media / 'a.mp4')},
                                          {'family': 'matroska-webm', 'path': str(media / 'b.webm')},
                                          {'family': 'mpegts', 'path': str(media / 'c.ts')}]))
        cls.d = d
        cls.arms = {arm: cls.run_arm(arm) for arm in speed.ARMS}

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    @classmethod
    def run_arm(cls, arm):
        calls, out = cls.d / f'{arm}.calls', cls.d / f'{arm}.json'
        cpu = sorted(os.sched_getaffinity(0))[0]
        subprocess.run([sys.executable, '-I', str(HERE / 'speed.py'), 'r4', '--arm', arm,
                        '--coreav', str(cls.bin / 'coreav'), '--ffmpeg', str(cls.bin / 'ffmpeg'),
                        '--ffprobe', str(cls.bin / 'ffprobe'), '--files', str(cls.files),
                        '--scratch', str(cls.d / 'scratch'), '--out', str(out), '--rounds', '1',
                        '--cpus', str(cpu)],
                       env={**os.environ, 'FAKE_CALLS': str(calls)}, check=True, capture_output=True, timeout=600)
        lines = [json.loads(line) for line in calls.read_text().splitlines()]
        return json.loads(out.read_text()), [c for c in lines if '-show_streams' not in c or len(c) > 7]

    def arm(self, arm):
        return self.arms[arm]

    def test_each_arm_times_its_sides_in_turn(self):
        for arm, sides in (('r4', ('coreav', 'ffmpeg')), ('aa', ('ffmpeg', 'ffmpeg')), ('null', ('coreav', 'coreav'))):
            data, calls = self.arm(arm)
            self.assertEqual(data['sides'], list(sides))
            timed = [r for r in data['results'] if 'ratio' in r]
            self.assertTrue(timed)
            for r in timed:
                self.assertEqual((len(r['a']), len(r['b'])), (1, 1))
            # Each pair: one untimed run of each side, then the rounds in turn.
            first = [c[0] for c in calls[:4]]
            self.assertEqual(first, [sides[0], sides[1]] * 2, arm)
            self.assertEqual(calls[0][1:] if sides[0] == 'coreav' else calls[0], calls[1][1:] if sides[1] == 'coreav' else calls[1])
            self.assertFalse(any((self.d / 'scratch' / 'out').iterdir()))

    def test_a_logged_refusal_has_no_ratio_and_is_counted_without_voiding(self):
        data, _ = self.arm('r4')
        refused = [r for r in data['results'] if r.get('refused')]
        self.assertTrue(refused)
        self.assertTrue(all('ratio' not in r for r in refused))
        self.assertEqual(data['fallback_log'], {'lines': len(refused), 'refusal_lines': len(refused),
                                                'excluded_lines': len(refused), 'void': False})
        self.assertFalse(self.arm('aa')[0]['fallback_log']['void'])

    def test_no_pair_is_unmeasured(self):
        empty = self.d / 'empty.json'
        empty.write_text('[]')
        done = subprocess.run([sys.executable, '-I', str(HERE / 'speed.py'), 'r4', '--arm', 'r4',
                               '--coreav', str(self.bin / 'coreav'), '--ffmpeg', str(self.bin / 'ffmpeg'),
                               '--ffprobe', str(self.bin / 'ffprobe'), '--files', str(empty),
                               '--scratch', str(self.d / 'scratch'), '--out', str(self.d / 'none.json'),
                               '--cpus', str(sorted(os.sched_getaffinity(0))[0])],
                              env={**os.environ, 'FAKE_CALLS': str(self.d / 'none.calls')},
                              capture_output=True, text=True, timeout=60)
        self.assertEqual(done.returncode, release_round.UNMEASURED)
        self.assertIn('UNMEASURED', done.stdout)
        self.assertFalse((self.d / 'none.json').exists())

    def test_commands_run_on_their_families_and_name_no_file(self):
        data, _ = self.arm('aa')
        families = {r['family'] for r in data['results'] if r['class'] in ('clip', 'join', 'split')}
        self.assertEqual(families, {'mp4-mov'})
        self.assertNotIn('a.mp4', json.dumps(data))
        self.assertEqual({r['file'] for r in data['results']}, {'mp4-mov:0', 'matroska-webm:0', 'mpegts:0'})


@unittest.skipUnless(hasattr(os, 'pidfd_open'), 'R4 runs on Linux with process-exit descriptors')
class RealTiming(unittest.TestCase):
    def test_short_real_children_retain_their_slowdown_through_r4(self):
        sleeper = shutil.which('sleep')
        self.assertIsNotNone(sleeper)
        times = ([], [])
        with tempfile.TemporaryDirectory() as tmp:
            scratch = pathlib.Path(tmp)
            with mock.patch.object(speed.time, 'sleep', side_effect=AssertionError('R4 must not poll process exit')):
                for _ in range(5):
                    for side, delay in ((0, '0.06'), (1, '0.004')):
                        (scratch / 'output').write_text('temporary output')
                        elapsed, code = speed.timed_run([sleeper, delay], {}, scratch)
                        self.assertEqual(code, 0)
                        self.assertFalse(list(scratch.iterdir()))
                        times[side].append(elapsed)
        ratio = statistics.median(times[0]) / statistics.median(times[1])
        self.assertGreater(ratio, 1.1)
        summary = speed.r4_summary(results('r4', [('remux', 'mp4-mov', ratio)]), gate=False)
        self.assertEqual(summary['failing'], ['remux all', 'remux mp4-mov'])

    def test_the_shared_deadline_reaps_the_child_and_cleans_r4_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            scratch = pathlib.Path(tmp)
            (scratch / 'output').write_text('partial output')
            with mock.patch.object(speed, 'B4_TIMEOUT', 0.01), \
                    mock.patch.object(speed.os, 'pidfd_open', wraps=os.pidfd_open) as opened:
                _, code = speed.timed_run([shutil.which('sleep'), '0.1'], {}, scratch)
            self.assertIsNone(code)
            self.assertFalse(list(scratch.iterdir()))
            with self.assertRaises(ChildProcessError):
                os.waitpid(opened.call_args.args[0], os.WNOHANG)

    def test_descriptor_failure_reaps_the_child_and_cleans_r4_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            scratch = pathlib.Path(tmp)
            (scratch / 'output').write_text('partial output')
            with mock.patch.object(speed.os, 'pidfd_open', side_effect=OSError('descriptor unavailable')) as opened:
                with self.assertRaisesRegex(OSError, 'descriptor unavailable'):
                    speed.timed_run([shutil.which('sleep'), '0.1'], {}, scratch)
            self.assertFalse(list(scratch.iterdir()))
            with self.assertRaises(ChildProcessError):
                os.waitpid(opened.call_args.args[0], os.WNOHANG)

    def test_infrastructure_errors_abort_after_a_completed_pair(self):
        for stage, target, method in (('launch', speed.subprocess, 'Popen'),
                                      ('descriptor', speed.os, 'pidfd_open')):
            with self.subTest(stage=stage), tempfile.TemporaryDirectory() as tmp:
                root = pathlib.Path(tmp)
                out = root / 'results.json'
                opts = SimpleNamespace(scratch=str(root), out=str(out), cpus='0', arm='aa', rounds=1,
                                       ffmpeg=sys.executable, ffprobe=sys.executable, coreav=sys.executable)
                entry = {'id': 'C01', 'tool': 'ffmpeg', 'job': 'remux'}
                pairs = [(entry, i, {'args': ['-c', 'pass']}, 'mp4-mov:0', 'mp4-mov', root / 'input.mp4')
                         for i in range(2)]
                operation = getattr(target, method)
                calls = 0

                def fail_fifth(*args, **kwargs):
                    nonlocal calls
                    calls += 1
                    if calls == 5:
                        (root / 'out' / 'partial').write_text('partial output')
                        raise OSError('infrastructure unavailable')
                    return operation(*args, **kwargs)

                with mock.patch.object(speed.os, 'sched_setaffinity'), \
                        mock.patch.object(speed, 'r4_files', return_value=[]), \
                        mock.patch.object(speed, 'r4_pairs', return_value=pairs), \
                        mock.patch.object(target, method, side_effect=fail_fifth):
                    with self.assertRaisesRegex(OSError, 'infrastructure unavailable'):
                        speed.r4(opts)
                self.assertEqual(calls, 5)
                self.assertFalse(out.exists())
                self.assertFalse(list((root / 'out').iterdir()))

    def test_nonzero_child_exits_are_still_returned(self):
        for script, expected in (('import sys; sys.exit(7)', 7),
                                 ('import os, signal; os.kill(os.getpid(), signal.SIGHUP)', -1)):
            with self.subTest(expected=expected), tempfile.TemporaryDirectory() as tmp:
                scratch = pathlib.Path(tmp)
                _, code = speed.timed_run([sys.executable, '-c', script], {}, scratch)
                self.assertEqual(code, expected)
                self.assertFalse(list(scratch.iterdir()))


def results(arm, ratios):
    return {'arm': arm, 'sides': list(speed.ARMS[arm]), 'files': {}, 'fallback_log': {'void': False},
            'results': [{'class': c, 'family': f, 'id': 'C01', 'command': 0, 'ratio': x} for c, f, x in ratios]}


REFUSAL = {'ran': None, 'not_run': 'strict_safe', 'tool': 'ffmpeg',
           'reason': "no video is presented in the clip's range"}


@unittest.skipUnless(hasattr(os, 'pidfd_open'), 'R4 runs on Linux with process-exit descriptors')
class Invocation(unittest.TestCase):
    def call(self, code, raw, side='coreav'):
        script = ('import os,pathlib,sys; '
                  'raw=os.environ.get("RAW"); '
                  'raw is None or pathlib.Path(os.environ["COREAV_FALLBACK_LOG"]).write_text(raw); '
                  'sys.exit(int(os.environ["EXIT"]))')
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            scratch = root / 'out'
            scratch.mkdir()
            (scratch / 'output').write_text('temporary output')
            call_log, aggregate = root / 'call.log', root / 'fallback.log'
            call_log.write_text(json.dumps(REFUSAL) + '\n')
            env = {'EXIT': str(code), **({'RAW': raw} if raw is not None else {})}
            call = speed.r4_invocation([sys.executable, '-c', script], env, scratch,
                                       call_log, aggregate, side, 'ffmpeg')
            self.assertFalse(list(scratch.iterdir()))
            return call, aggregate.read_text() if aggregate.exists() else ''

    def test_only_a_corresponding_logged_strict_safe_exit_qualifies(self):
        valid = json.dumps(REFUSAL) + '\n'
        for code, raw, side, expected in (
                (4, valid, 'coreav', True), (4, valid * 2, 'coreav', True),
                (4, None, 'coreav', False), (1, valid, 'coreav', False),
                (0, valid, 'coreav', False), (4, valid, 'ffmpeg', False),
                (4, valid.rstrip(), 'coreav', False), (4, '{broken}\n', 'coreav', False),
                (4, '[]\n', 'coreav', False), (4, 'null\n', 'coreav', False),
                (4, json.dumps({**REFUSAL, 'tool': 'ffprobe'}) + '\n', 'coreav', False),
                (4, json.dumps({**REFUSAL, 'reason': ''}) + '\n', 'coreav', False),
                (4, json.dumps({**REFUSAL, 'ran': 'ffmpeg'}) + '\n', 'coreav', False),
                (4, json.dumps({**REFUSAL, 'not_run': 'no_program'}) + '\n', 'coreav', False)):
            with self.subTest(code=code, raw=raw, side=side):
                call, retained = self.call(code, raw, side)
                self.assertEqual(call['refusal'], expected)
                self.assertEqual(call['lines'], len(raw.splitlines()) if raw else 0)
                self.assertEqual(retained, raw or '')

    def test_an_unreadable_invocation_log_aborts(self):
        with mock.patch.object(pathlib.Path, 'read_bytes', side_effect=PermissionError('log unavailable')):
            with self.assertRaisesRegex(PermissionError, 'log unavailable'):
                self.call(4, json.dumps(REFUSAL) + '\n')


class Outcomes(unittest.TestCase):
    def run_pair(self, arm, second):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            opts = SimpleNamespace(scratch=str(root), out=str(root / 'results.json'), cpus='0',
                                   arm=arm, rounds=1, coreav='coreav', ffmpeg='ffmpeg', ffprobe='ffprobe')
            entry = {'id': 'C01', 'tool': 'ffmpeg', 'job': 'remux'}
            pairs = [(entry, i, {'args': ['INPUT', 'OUTPUT']}, 'mp4-mov:0', 'mp4-mov', root / 'in.mp4')
                     for i in range(2)]
            invocations = iter([(0, [])] * 4 + second)

            def timed(argv, env, scratch):
                code, records = next(invocations)
                if records:
                    pathlib.Path(env['COREAV_FALLBACK_LOG']).write_text(
                        ''.join(json.dumps(record) + '\n' for record in records))
                return 1.0, code

            with mock.patch.object(speed.os, 'sched_setaffinity'), \
                    mock.patch.object(speed, 'r4_files', return_value=[]), \
                    mock.patch.object(speed, 'r4_pairs', return_value=pairs), \
                    mock.patch.object(speed, 'timed_run', side_effect=timed):
                self.assertEqual(speed.r4(opts), 0)
            with self.assertRaises(StopIteration):
                next(invocations)
            return json.loads((root / 'results.json').read_text())

    def test_observed_refusals_are_excluded_and_counted_once(self):
        for arm, second in (('r4', [(4, [REFUSAL, REFUSAL]), (0, [])]),
                            ('null', [(4, [REFUSAL]), (4, [REFUSAL])])):
            with self.subTest(arm=arm):
                data = self.run_pair(arm, second)
                row = data['results'][1]
                self.assertTrue(row['refused'])
                self.assertNotIn('a', row)
                self.assertFalse(data['fallback_log']['void'])
                self.assertEqual(row['refusal_group'], 'layout')
                coverage = speed.r4_summary(data, gate=arm != 'r4')['coverage']
                self.assertEqual([coverage[k] for k in ('eligible', 'refused', 'timed', 'failed', 'oracle_failed')],
                                 [2, 1, 1, 0, 0])
                self.assertEqual(coverage['refusal_groups']['layout'], 1)
                self.assertEqual(coverage['timed_share'], 0.5)

    def test_mixed_null_outcomes_fail_without_refusal_double_counting(self):
        for other in (0, 1, 4, -11, None):
            for refused_side in (0, 1):
                with self.subTest(other=other, refused_side=refused_side):
                    second = [(other, []), (other, [])]
                    second[refused_side] = (4, [REFUSAL])
                    data = self.run_pair('null', second)
                    self.assertFalse(data['results'][1]['refused'])
                    self.assertTrue(data['fallback_log']['void'])
                    coverage = speed.r4_summary(data, gate=True)['coverage']
                    self.assertEqual((coverage['refused'], coverage['failed'], coverage['timed']), (0, 1, 1))
                    self.assertEqual(sum(coverage['refusal_groups'].values()), 0)

    def test_late_refusal_is_incomplete_not_a_new_exclusion(self):
        for arm in ('r4', 'null'):
            data = self.run_pair(arm, [(0, []), (0, []), (4, [REFUSAL]), (0, [])])
            summary = speed.r4_summary(data, gate=arm != 'r4')
            self.assertEqual(summary['incomplete'], ['C01.1 mp4-mov:0'])
            self.assertEqual((summary['coverage']['refused'], summary['coverage']['failed']), (0, 1))
            self.assertTrue(summary['fallback_log']['void'])

    def test_failures_and_oracle_failures_do_not_become_refusals(self):
        for left, right, failed, oracle, void in ((1, 0, 1, 0, False), (-11, 0, 1, 0, False),
                                                 (None, 0, 1, 0, False), (4, 0, 1, 0, False),
                                                 (0, 1, 0, 1, False), (4, 1, 0, 1, True)):
            with self.subTest(left=left, right=right):
                records = [REFUSAL] if left == 4 and right == 1 else []
                data = self.run_pair('r4', [(left, records), (right, [])])
                coverage = speed.r4_summary(data, gate=False)['coverage']
                self.assertEqual((coverage['refused'], coverage['failed'], coverage['oracle_failed']),
                                 (0, failed, oracle))
                self.assertEqual(data['fallback_log']['void'], void)

    def test_a_successful_exit_with_a_refusal_log_is_invalid_not_refused(self):
        data = self.run_pair('r4', [(0, [REFUSAL]), (0, [])])
        summary = speed.r4_summary(data, gate=False)
        self.assertEqual((summary['coverage']['refused'], summary['coverage']['failed']), (0, 1))
        self.assertTrue(summary['fallback_log']['void'])

    def test_reason_groups_are_exact_and_unknown_does_not_select_exclusions(self):
        for reasons, expected in ((['a codec that CoreAV does not copy'] * 2, 'codec_container'),
                                  (['-map_metadata 0:s:0'] * 2, 'option'),
                                  (['a new reason'] * 2, 'unknown'),
                                  (['-map_metadata 0:s:0', REFUSAL['reason']], 'unknown')):
            with self.subTest(reasons=reasons):
                data = self.run_pair('null', [(4, [{**REFUSAL, 'reason': reason}]) for reason in reasons])
                self.assertEqual(data['results'][1]['refusal_group'], expected)
                coverage = speed.r4_summary(data, gate=True)['coverage']
                self.assertEqual(coverage['refused'], 1)
                self.assertEqual(coverage['refusal_groups'][expected], 1)
                self.assertEqual(sum(coverage['refusal_groups'].values()), 1)


class Report(unittest.TestCase):
    def test_coverage_reconciles_beside_every_speed_number(self):
        data = results('r4', [('remux', 'mp4-mov', 0.8)] * 2)
        base = {'class': 'remux', 'family': 'mp4-mov', 'id': 'C01', 'command': 0, 'file': 'mp4-mov:0'}
        data['results'] += [{**base, 'exit': [4, 0], 'refused': True,
                             'refusal_group': 'layout', 'refusal_reasons': [REFUSAL['reason']] * 2},
                            {**base, 'exit': [1, 0]}, {**base, 'exit': [0, 1]},
                            {**base, 'a': [1], 'b': [1], 'exit': [1]}]
        data['commands'] = [{'id': 'C01', 'command': 0, 'class': 'remux'},
                            {'id': 'C02', 'command': 0, 'class': 'join'}]
        summary = speed.r4_summary(data, gate=False)
        coverage = summary['coverage']
        self.assertEqual([coverage[k] for k in ('eligible', 'refused', 'timed', 'failed', 'oracle_failed')],
                         [6, 1, 2, 2, 1])
        self.assertEqual(coverage['eligible'], sum(coverage[k] for k in ('refused', 'timed', 'failed', 'oracle_failed')))
        self.assertEqual(coverage['timed_share'], 1 / 3)
        self.assertEqual(coverage['refusal_groups']['layout'], 1)
        self.assertEqual(coverage['refusal_reasons'], {REFUSAL['reason']: 1})
        self.assertTrue(all(row['coverage'] == coverage for row in summary['classes']))
        self.assertEqual(summary['commands'][0]['coverage'], coverage)
        empty = summary['commands'][1]
        self.assertIsNone(empty['median'])
        self.assertEqual(empty['coverage']['eligible'], 0)
        self.assertIsNone(empty['coverage']['timed_share'])
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'report.json'
            path.write_text(json.dumps(data))
            done = subprocess.run([sys.executable, '-I', str(HERE / 'speed.py'), 'r4-report', '--results', str(path)],
                                  capture_output=True, text=True, timeout=60)
        self.assertEqual(done.returncode, 1, done.stderr)
        for line in done.stdout.splitlines():
            if 'median=' in line:
                for field in ('refused=', 'eligible=', 'timed=', 'timed_share='):
                    self.assertIn(field, line)

    def test_a_fully_refused_group_stays_unmeasured(self):
        data = results('null', [])
        data['results'] = [{'class': 'remux', 'family': 'mp4-mov', 'id': 'C01', 'command': 0,
                            'exit': [4, 4], 'refused': True, 'refusal_group': 'unknown',
                            'refusal_reasons': ['unrecognized reason']}]
        summary = speed.r4_summary(data, gate=True)
        self.assertEqual(summary['failing'], ['remux all'])
        self.assertEqual((summary['coverage']['refused'], summary['coverage']['timed']), (1, 0))
        self.assertEqual(summary['coverage']['timed_share'], 0)
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'report.json'
            path.write_text(json.dumps(data))
            done = subprocess.run([sys.executable, '-I', str(HERE / 'speed.py'), 'r4-report', '--gate',
                                   '--results', str(path)], capture_output=True, text=True, timeout=60)
        self.assertEqual(done.returncode, release_round.UNMEASURED, done.stderr)

    def test_historical_r4_ratios_remain_readable_without_a_method_version(self):
        data = results('r4', [('remux', 'mp4-mov', 0.8), ('remux', 'mp4-mov', 1.2)])
        self.assertNotIn('method', data)
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'historical.json'
            path.write_text(json.dumps(data))
            done = subprocess.run([sys.executable, '-I', str(HERE / 'speed.py'), 'r4-report', '--results', str(path)],
                                  capture_output=True, text=True, timeout=60)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn('remux', done.stdout)

    def test_r4_meets_at_or_below_one(self):
        rows = [('remux', 'mp4-mov', 0.9)] * 10 + [('remux', 'matroska-webm', 1.1)] * 10
        s = speed.r4_summary(results('r4', rows), gate=False)
        by = {(r['class'], r['family']): r for r in s['classes']}
        self.assertTrue(by[('remux', 'mp4-mov')]['meets'])
        self.assertTrue(by[('remux', 'mp4-mov')]['faster'])
        self.assertEqual(by[('remux', 'mp4-mov')]['faster_share'], 1.0)
        self.assertFalse(by[('remux', 'matroska-webm')]['meets'])
        self.assertEqual(s['failing'], ['remux matroska-webm'])

    def test_the_gate(self):
        rng = random.Random(1)
        steady = [('probe', 'mpegts', 1 + rng.uniform(-0.01, 0.01)) for _ in range(40)]
        self.assertEqual(speed.r4_summary(results('aa', steady), gate=True)['failing'], [])
        biased = [('probe', 'mpegts', 1.03 + rng.uniform(-0.01, 0.01)) for _ in range(40)]
        self.assertEqual(speed.r4_summary(results('null', biased), gate=True)['failing'], ['probe all'])
        # A median inside the band whose interval misses 1.00 fails too.
        tight = [('probe', 'mpegts', 1.015 + rng.uniform(-0.001, 0.001)) for _ in range(40)]
        self.assertEqual(speed.r4_summary(results('null', tight), gate=True)['failing'], ['probe all'])

    def test_a_group_with_pairs_and_no_ratio_fails(self):
        rows = [('remux', 'mp4-mov', 0.9)] * 10
        data = results('r4', rows)
        data['results'].append({'class': 'remux', 'family': 'mpegts', 'id': 'C01', 'command': 0, 'exit': [1, 0]})
        data['results'].append({'class': 'clip', 'family': 'mpegts', 'id': 'C05', 'command': 0, 'exit': [1, 0]})
        s = speed.r4_summary(data, gate=False)
        self.assertEqual(s['unmeasured'], ['clip all', 'clip mpegts', 'remux mpegts'])
        self.assertEqual(s['failing'], s['unmeasured'])
        steady = [('remux', f, 1.0) for f in ('mp4-mov', 'mpegts') for _ in range(20)]
        gate = results('aa', steady)
        gate['results'].append({'class': 'remux', 'family': 'matroska-webm', 'id': 'C01', 'command': 0, 'exit': [1, 0]})
        self.assertEqual(speed.r4_summary(gate, gate=True)['failing'], [])
        gate['results'].append({'class': 'clip', 'family': 'mpegts', 'id': 'C05', 'command': 0, 'exit': [1, 0]})
        self.assertEqual(speed.r4_summary(gate, gate=True)['failing'], ['clip all'])

    def test_a_timed_failure_fails_the_run_and_the_gate(self):
        for arm, gate, base in (('r4', [], 0.9), ('aa', ['--gate'], 1.0), ('null', ['--gate'], 1.0)):
            data = results(arm, [('probe', 'mp4-mov', base + 0.001 * (i % 3 - 1)) for i in range(21)])
            data['results'].append({'class': 'probe', 'family': 'mp4-mov', 'id': 'C04', 'command': 0,
                                    'file': 'mp4-mov:7', 'a': [0.1], 'b': [0.1], 'exit': [0, 1]})
            s = speed.r4_summary(data, gate=bool(gate))
            self.assertEqual((s['failing'], s['incomplete']), ([], ['C04.0 mp4-mov:7']))
            self.assertEqual(s['no_ratio'], {'timed run failed': 1})
            with tempfile.TemporaryDirectory() as tmp:
                path = pathlib.Path(tmp) / 'r.json'
                for extra, code in ((data['results'][-1:], 1), ([], 0)):
                    path.write_text(json.dumps({**data, 'results': data['results'][:-1] + extra}))
                    done = subprocess.run([sys.executable, '-I', str(HERE / 'speed.py'), 'r4-report',
                                           '--results', str(path)] + gate, capture_output=True, text=True, timeout=60)
                    self.assertEqual(done.returncode, code, (arm, done.stdout))
                    self.assertEqual('INCOMPLETE C04.0 mp4-mov:7' in done.stdout, bool(extra))

    def test_an_untimed_failure_is_counted_by_its_side(self):
        data = results('r4', [('probe', 'mp4-mov', 0.9)] * 5)
        data['results'] += [{'class': 'probe', 'family': 'mp4-mov', 'id': 'C04', 'command': 0, 'exit': [1, 0]},
                            {'class': 'probe', 'family': 'mp4-mov', 'id': 'C04', 'command': 1, 'exit': [0, 1]},
                            {'class': 'probe', 'family': 'mp4-mov', 'id': 'C04', 'command': 2, 'exit': [4, 0],
                             'refused': True}]
        s = speed.r4_summary(data, gate=False)
        self.assertEqual(s['no_ratio'], {'untimed run failed: coreav': 1, 'untimed run failed: ffmpeg': 1,
                                         'refused': 1})
        self.assertEqual(s['incomplete'], [])

    def test_no_ratio_is_unmeasured(self):
        with tempfile.TemporaryDirectory() as tmp:
            for arm, gate in (('r4', []), ('aa', ['--gate'])):
                data = results(arm, [])
                data['results'] = [{'class': 'remux', 'family': 'mpegts', 'id': 'C01', 'command': 0, 'exit': [1, 0]}]
                path = pathlib.Path(tmp) / f'{arm}.json'
                path.write_text(json.dumps(data))
                done = subprocess.run([sys.executable, '-I', str(HERE / 'speed.py'), 'r4-report',
                                       '--results', str(path)] + gate, capture_output=True, text=True, timeout=60)
                self.assertEqual(done.returncode, release_round.UNMEASURED, arm)
                self.assertIn('UNMEASURED', done.stdout)

    def test_cpu_lists(self):
        self.assertEqual(speed.cpu_list('4-7'), {4, 5, 6, 7})
        self.assertEqual(speed.cpu_list('0,2-3'), {0, 2, 3})


if __name__ == '__main__':
    unittest.main()
