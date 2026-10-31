"""Round-9 B4 protocol checks with synthetic times and stand-in processes."""
import copy
import hashlib
import json
import os
import pathlib
import random
import shutil
import statistics
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import speed


def result(family, file, ratio, command=0):
    return {'class': family, 'file': file, 'ratio': ratio, 'command': command}


class Intervals(unittest.TestCase):
    def test_empty_groups_have_no_interval(self):
        self.assertIsNone(speed.interval([]))

    def test_stratified_draws_carry_whole_files_with_multiplicity(self):
        rows = [result('isobmff', 'a', 0.1), result('isobmff', 'a', 0.2, 1),
                result('isobmff', 'b', 0.8), result('mpegts', 'c', 1.8),
                result('mpegts', 'c', 1.9, 1)]
        rng = random.Random(0)
        medians = []
        samples = []
        for _ in range(10000):
            files = rng.choices(['a', 'b'], k=2) + rng.choices(['c'], k=1)
            sample = [row['ratio'] for file in files for row in rows if row['file'] == file]
            samples.append(sample)
            medians.append(statistics.median(sample))
        with mock.patch.object(speed.statistics, 'median', wraps=statistics.median) as median:
            actual = speed.interval(rows)
        self.assertEqual([c.args[0] for c in median.call_args_list], samples)
        medians.sort()
        self.assertEqual(actual, [medians[249], medians[9749]])
        self.assertEqual(actual, speed.interval(rows))

    def test_calibration_resamples_files_without_container_strata(self):
        rows = [result(c, c, x) for c, x in [('isobmff', 0.8), ('matroska', 1.0), ('mpegts', 1.2)]]
        self.assertEqual(speed.interval(rows), [1.0, 1.0])
        self.assertEqual(speed.interval(rows, stratified=False), [0.8, 1.2])


class Pairs(unittest.TestCase):
    def pair(self, calls, refuse=True):
        with mock.patch.object(speed, 'timed', side_effect=calls):
            return speed.b4_pair(['ours'], ['theirs'], {}, refuse=refuse)

    def test_warmup_failures_record_both_statuses_without_a_ratio(self):
        for a, b in [(1, 0), (0, 1), (None, 0), (0, None), (4, 1)]:
            row = self.pair([(1, a), (1, b)])
            self.assertEqual(row['failed'], {'phase': 'warmup', 'exit': [a, b]})
            self.assertNotIn('ratio', row)

    def test_only_a_successful_reference_makes_exit_four_a_refusal(self):
        row = self.pair([(1, 4), (1, 0)])
        self.assertTrue(row['refused'])
        self.assertEqual(row['warmup_exit'], [4, 0])
        self.assertNotIn('ratio', row)
        self.assertIn('failed', self.pair([(1, 4), (1, 0)], refuse=False))

    def test_timed_failures_keep_partial_calls_but_no_ratio(self):
        for a, b in [(1, 0), (0, 1), (None, 0), (0, None), (4, 0)]:
            row = self.pair([(1, 0), (1, 0), (2, 0), (4, 0), (2, a), (4, b)])
            self.assertEqual(row['failed'], {'phase': 'timed', 'round': 1, 'exit': [a, b]})
            self.assertEqual(row['timed_exit'], [[0, 0], [a, b]])
            self.assertEqual(row['a'], [2, 2])
            self.assertNotIn('ratio', row)

    def test_success_uses_five_alternating_pairs_after_one_warmup(self):
        with mock.patch.object(speed, 'timed', side_effect=[(9, 0), (9, 0)] + [(2, 0), (4, 0)] * 5) as timed:
            row = speed.b4_pair(['ours'], ['theirs'], {'key': 'value'})
        self.assertEqual(row['ratio'], 0.5)
        self.assertEqual(row['a'], [2] * 5)
        self.assertEqual([call.args[0] for call in timed.call_args_list], [['ours'], ['theirs']] * 6)
        self.assertTrue(all(call.args[1] == {'key': 'value'} for call in timed.call_args_list))

    def test_timeout_and_launch_failure_are_not_success(self):
        with mock.patch.object(speed.subprocess, 'Popen', side_effect=OSError('missing')):
            _, actual = speed.timed(['stand-in'], {})
        self.assertEqual(actual, -1)
        if hasattr(os, 'pidfd_open'):
            with mock.patch.object(speed, 'B4_TIMEOUT', 0.01):
                _, actual = speed.timed([shutil.which('sleep'), '0.1'], {})
            self.assertIsNone(actual)

    @unittest.skipUnless(hasattr(os, 'pidfd_open'), 'B4 runs on Linux with process-exit descriptors')
    def test_a_real_nonzero_process_is_recorded(self):
        row = speed.b4_pair([sys.executable, '-c', 'raise SystemExit(9)'],
                            [sys.executable, '-c', 'pass'], {})
        self.assertEqual(row['failed'], {'phase': 'warmup', 'exit': [9, 0]})
        self.assertNotIn('ratio', row)


@unittest.skipUnless(hasattr(os, 'pidfd_open'), 'B4 runs on Linux with process-exit descriptors')
class RealTiming(unittest.TestCase):
    def test_a_real_deadline_kills_and_reaps_the_child(self):
        with mock.patch.object(speed, 'B4_TIMEOUT', 0.01), \
                mock.patch.object(speed.os, 'pidfd_open', wraps=os.pidfd_open) as opened:
            elapsed, code = speed.timed([shutil.which('sleep'), '0.1'], {})
        self.assertIsNone(code)
        self.assertGreaterEqual(elapsed, 0.01)
        with self.assertRaises(ChildProcessError):
            os.waitpid(opened.call_args.args[0], os.WNOHANG)

    def test_a_descriptor_error_also_kills_and_reaps_the_child(self):
        with mock.patch.object(speed.os, 'pidfd_open', side_effect=OSError('no process descriptor')) as opened:
            _, code = speed.timed([shutil.which('sleep'), '0.1'], {})
        self.assertEqual(code, -1)
        with self.assertRaises(ChildProcessError):
            os.waitpid(opened.call_args.args[0], os.WNOHANG)

    def test_short_real_children_retain_their_slowdown_through_the_report(self):
        sleep = shutil.which('sleep')
        self.assertIsNotNone(sleep)
        with mock.patch.object(speed.time, 'sleep', side_effect=AssertionError('B4 must not poll process exit')):
            pair = speed.b4_pair([sleep, '0.06'], [sleep, '0.004'], {})
        self.assertNotIn('failed', pair)
        self.assertGreater(pair['ratio'], 1.1)
        data = {'method': speed.B4_METHOD, 'rounds': 5, 'commands': [['-show_streams', 'INPUT']],
                'selection': [{'class': 'isobmff', 'file': 'a'}],
                'calibration': {a: {'passes': True} for a in ('aa', 'null')},
                'fallback_log': {'void': False},
                'results': [{'command': 0, 'class': 'isobmff', 'file': 'a', **pair}]}
        summary = speed.b4_summary(data)
        self.assertFalse(summary['passes'])
        self.assertTrue(summary['missed'])


class Fallback(unittest.TestCase):
    def verdict(self, lines):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'fallback.jsonl'
            path.write_text('\n'.join(lines))
            return speed.b4_fallback(path)

    def test_strict_safe_records_are_not_executed_fallbacks(self):
        self.assertEqual(self.verdict([json.dumps({'ran': None, 'not_run': 'strict_safe'})]),
                         {'lines': 1, 'refused': 1, 'executed': 0, 'invalid': 0, 'void': False})

    def test_an_execution_or_unreadable_record_voids_the_arm(self):
        for line in ['{"ran":"ffprobe"}', '{"ran":"ffprobe","not_run":"strict_safe"}',
                     'bad json', 'null', '[]', '{"not_run":"missing"}']:
            self.assertTrue(self.verdict([line])['void'], line)
        self.assertFalse(self.verdict([])['void'])


class Calibration(unittest.TestCase):
    def test_one_failed_case_fails_an_otherwise_steady_calibration_arm(self):
        selection = [{'class': c, 'sha256': c, 'ext': 'dummy', 'file': c} for c in speed.CLASSES]
        with tempfile.TemporaryDirectory() as tmp:
            pairs = [{'failed': {'phase': 'warmup', 'exit': [0, 1]}}] + [{'ratio': 1.0, 'a': [1] * 5, 'b': [1] * 5}] * 11
            with mock.patch.object(speed, 'b4_pair', side_effect=pairs):
                arms = speed.calibrate(selection, pathlib.Path(tmp), 'coreav', 'ffprobe', {},
                                       pathlib.Path(tmp) / 'log')
        self.assertFalse(arms['aa']['passes'])
        self.assertTrue(arms['null']['passes'])

    def test_both_the_band_and_the_interval_are_required(self):
        selection = [{'class': c, 'sha256': c, 'ext': 'dummy', 'file': c} for c in speed.CLASSES]
        with tempfile.TemporaryDirectory() as tmp:
            for ratio, ci, expected in [(1.0, [0.99, 1.01], True), (1.03, [0.99, 1.04], False),
                                        (1.015, [1.014, 1.016], False)]:
                with mock.patch.object(speed, 'b4_pair', return_value={'ratio': ratio, 'a': [ratio] * 5, 'b': [1] * 5}), \
                        mock.patch.object(speed, 'interval', return_value=ci) as interval:
                    arms = speed.calibrate(selection, pathlib.Path(tmp), 'coreav', 'ffprobe', {},
                                           pathlib.Path(tmp) / 'log')
                self.assertTrue(all(a['passes'] == expected for a in arms.values()))
                self.assertTrue(all(c.kwargs == {'stratified': False} for c in interval.call_args_list))

    def test_calibration_keeps_the_median_over_thirty_call_ratios(self):
        selection = [{'class': c, 'sha256': c, 'ext': 'dummy', 'file': c} for c in speed.CLASSES]
        pair = {'ratio': 1.5, 'a': [1, 1, 3, 3, 3], 'b': [1, 1, 2, 2, 4]}
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(speed, 'b4_pair', return_value=pair), \
                    mock.patch.object(speed, 'interval', wraps=speed.interval) as interval:
                arms = speed.calibrate(selection, pathlib.Path(tmp), 'coreav', 'ffprobe', {},
                                       pathlib.Path(tmp) / 'log')
        self.assertTrue(all(a['median'] == 1.0 and a['passes'] for a in arms.values()))
        self.assertEqual([len(c.args[0]) for c in interval.call_args_list], [30, 30])


class Runner(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        self.selection = []
        for family, ext in [('isobmff', 'mp4'), ('matroska', 'webm'), ('mpegts', 'ts')]:
            contents = family.encode()
            sha = hashlib.sha256(contents).hexdigest()
            (self.root / f'{sha}.{ext}').write_bytes(contents)
            self.selection.append({'class': family, 'ext': ext, 'sha256': sha, 'size': len(contents)})
        (self.root / 'selection.json').write_text(json.dumps(self.selection))
        self.opts = types.SimpleNamespace(coreav='coreav', oracle_bin=str(self.root), cpus='0',
                                          dir=str(self.root), out=str(self.root / 'results.json'), rounds=5)

    def run_arm(self, timing=None, commands=None):
        calls = []

        def timed(argv, env):
            calls.append(argv)
            self.assertEqual(env['COREAV_STRICT_SAFE'], '1')
            self.assertEqual(env['GCONV_PATH'], '/nonexistent/gconv')
            self.assertEqual(env['COREAV_FALLBACK_LOG'], str(self.root / 'results.fallback.jsonl'))
            return timing(argv, env) if timing else (1, 0)

        with mock.patch.object(speed.os, 'sched_setaffinity', create=True), mock.patch.object(speed.subprocess, 'run') as pin, \
                mock.patch.object(speed, 'native_commands', return_value=commands or [['-f', 'mpegts', 'INPUT']]), \
                mock.patch.object(speed, 'timed', side_effect=timed):
            code = speed.run(self.opts)
        self.assertIn('--verify-bin', pin.call_args.args[0])
        return code, json.loads(pathlib.Path(self.opts.out).read_text()), calls

    def test_forced_format_pairs_are_ineligible_before_either_tool_runs(self):
        code, data, calls = self.run_arm()
        self.assertEqual(code, 0)
        self.assertEqual([r['class'] for r in data['results'] if r.get('ineligible')], ['isobmff', 'matroska'])
        forced = [c for c in calls if '-f' in c]
        self.assertEqual(len(forced), 12)
        self.assertTrue(all(c[-1].endswith('.ts') for c in forced))
        self.assertEqual(data['results'][2]['ratio'], 1)

    def test_failed_calibration_prevents_the_b4_arm(self):
        code, data, calls = self.run_arm(lambda a, e: (1, 1))
        self.assertEqual(code, 1)
        self.assertFalse(data['calibration']['aa']['passes'])
        self.assertFalse(data['calibration']['null']['passes'])
        self.assertEqual(data['results'], [])
        self.assertFalse(any('-f' in c for c in calls))

    def test_warmup_and_timed_b4_failures_make_the_run_fail(self):
        for phase, failed_call in [('warmup', 1), ('timed', 4)]:
            n = 0

            def timing(argv, env):
                nonlocal n
                if '-f' in argv:
                    n += 1
                    if n == failed_call:
                        return 1, None
                return 1, 0

            code, data, _ = self.run_arm(timing)
            self.assertEqual(code, 1)
            self.assertEqual(data['results'][2]['failed']['phase'], phase)
            self.assertNotIn('ratio', data['results'][2])

    def test_logs_are_run_local_and_executed_fallbacks_void_the_arm(self):
        log = self.root / 'results.fallback.jsonl'
        log.write_text('{"ran":"stale"}\n')

        def timing(argv, env):
            if '-f' in argv:
                log.write_text('{"ran":"ffprobe"}\n')
            return 1, 0

        code, data, _ = self.run_arm(timing)
        self.assertEqual(code, 1)
        self.assertEqual(data['fallback_log']['executed'], 1)
        self.assertTrue(data['fallback_log']['void'])
        code, data, _ = self.run_arm()
        self.assertEqual(code, 0)
        self.assertEqual(data['fallback_log']['lines'], 0)

    def test_refused_pairs_are_counted_without_becoming_fallbacks_or_speed_wins(self):
        def timing(argv, env):
            if argv[0] == 'coreav' and '-f' in argv:
                pathlib.Path(env['COREAV_FALLBACK_LOG']).write_text('{"ran":null,"not_run":"strict_safe"}\n')
                return 1, 4
            return 1, 0

        code, data, _ = self.run_arm(timing)
        self.assertEqual(code, 0)
        self.assertFalse(data['fallback_log']['void'])
        summary = speed.b4_summary(data)
        self.assertFalse(summary['passes'])
        self.assertEqual(summary['coverage'], {'total': 3, 'eligible': 1, 'ineligible': 2,
                                              'refused': 1, 'failed': 0, 'timed': 0, 'missing': 0})

    def test_file_bytes_are_checked_before_calibration(self):
        entry = self.selection[0]
        (self.root / f"{entry['sha256']}.{entry['ext']}").write_bytes(b'changed')
        with mock.patch.object(speed, 'timed') as timed:
            with self.assertRaisesRegex(ValueError, 'frozen selection'):
                speed.verify_selection(self.selection, self.root)
        timed.assert_not_called()

    def test_unforced_commands_keep_every_family_and_the_command_set_is_unchanged(self):
        code, data, _ = self.run_arm(commands=[['-show_streams', 'INPUT']])
        self.assertEqual(code, 0)
        self.assertTrue(all('ratio' in r for r in data['results']))
        self.assertEqual(len(speed.native_commands()), 93)
        forced = [c for c in speed.native_commands() if '-f' in c]
        self.assertEqual(len(forced), 1)
        self.assertEqual(forced[0][forced[0].index('-f') + 1], 'mpegts')

    def test_the_registered_matrix_keeps_all_files_and_excludes_only_the_forty_forced_pairs(self):
        families = ['isobmff'] * 20 + ['matroska'] * 20 + ['mpegts'] * 17
        pairs = [(command, family) for command in speed.native_commands() for family in families]
        self.assertEqual(len(pairs), 5301)
        self.assertEqual(sum(not speed.eligible(c, f) for c, f in pairs), 40)

    def test_other_cpu_or_repeat_counts_cannot_start_the_runner(self):
        for cpu, rounds in [('0,1', 5), ('0', 1)]:
            self.opts.cpus, self.opts.rounds = cpu, rounds
            with mock.patch.object(speed.subprocess, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'one pinned CPU and five'):
                    speed.run(self.opts)
            run.assert_not_called()


class Report(unittest.TestCase):
    def data(self):
        return {'method': speed.B4_METHOD, 'rounds': 5, 'commands': [['-show_streams', 'INPUT']],
                'selection': [{'class': 'isobmff', 'file': 'a'}, {'class': 'mpegts', 'file': 'b'}],
                'calibration': {a: {'passes': True} for a in ('aa', 'null')},
                'fallback_log': {'void': False},
                'results': [result('isobmff', 'a', 0.5), result('mpegts', 'b', 0.8)]}

    def test_every_group_has_an_interval_and_keeps_the_pair_weighting(self):
        data = self.data()
        data['commands'].append(['-show_format', 'INPUT'])
        data['results'] += [result('isobmff', 'a', 0.6, 1), result('mpegts', 'b', 0.9, 1)]
        summary = speed.b4_summary(data)
        self.assertTrue(summary['passes'])
        self.assertTrue(all(r['ci95'] is not None for r in summary['rows']))
        overall = next(r for r in summary['rows'] if r['group'] == 'all')
        self.assertEqual((overall['n'], overall['files'], overall['median']), (4, 2, 0.7))
        self.assertEqual(summary, speed.b4_summary(data))
        self.assertEqual(summary['coverage'], {'total': 4, 'eligible': 4, 'ineligible': 0,
                                              'refused': 0, 'failed': 0, 'timed': 4, 'missing': 0})

    def test_refusals_and_failures_are_not_ratios_or_unmeasured_passes(self):
        for outcome in [{'refused': True}, {'failed': {'phase': 'warmup', 'exit': [1, 0]}},
                        {'failed': {'phase': 'timed', 'exit': [0, None]}}]:
            data = self.data()
            data['results'][1] = {'class': 'mpegts', 'file': 'b', 'command': 0, **outcome}
            summary = speed.b4_summary(data)
            self.assertFalse(summary['passes'])
            self.assertTrue(summary['unmeasured'])
            row = next(r for r in summary['rows'] if r['group'] == 'class' and r['name'] == 'mpegts')
            self.assertIsNone(row['median'])
            self.assertIsNone(row['ci95'])

    def test_incomplete_coverage_and_slow_commands_cannot_pass(self):
        for change in ('missing', 'slow', 'fallback', 'calibration'):
            data = self.data()
            if change == 'missing':
                data['results'].pop()
            elif change == 'slow':
                data['results'][1]['ratio'] = 1.1
            elif change == 'fallback':
                data['fallback_log']['void'] = True
            else:
                data['calibration']['aa']['passes'] = False
            self.assertFalse(speed.b4_summary(data)['passes'], change)

    def test_duplicate_pairs_or_wrong_eligibility_are_invalid(self):
        data = self.data()
        data['results'].append(copy.deepcopy(data['results'][0]))
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            speed.b4_summary(data)
        data = self.data()
        data['results'][0]['ineligible'] = True
        with self.assertRaisesRegex(ValueError, 'eligibility'):
            speed.b4_summary(data)

    def test_historical_outputs_cannot_be_rescored(self):
        data = self.data()
        del data['method']
        with self.assertRaisesRegex(ValueError, 'historical B4'):
            speed.b4_summary(data)

    def test_report_writes_the_coverage_and_returns_the_verdict(self):
        with tempfile.TemporaryDirectory() as tmp:
            opts = types.SimpleNamespace(results=str(pathlib.Path(tmp) / 'data.json'),
                                         out=str(pathlib.Path(tmp) / 'summary.json'))
            for ratio, code in [(0.8, 0), (1.1, 1)]:
                data = self.data()
                data['results'][1]['ratio'] = ratio
                pathlib.Path(opts.results).write_text(json.dumps(data))
                self.assertEqual(speed.report(opts), code)
                self.assertEqual(json.loads(pathlib.Path(opts.out).read_text())['passes'], code == 0)


if __name__ == '__main__':
    unittest.main()
