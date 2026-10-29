#!/usr/bin/env python3
"""Self-test of scripts/h264_parity.py's host-fault handling.

Stands in for FFmpeg and the decoder with shell scripts that sleep, so it
uses no CPU to speak of and can run beside a measurement. A script's delay
is 0.2 s; a file `burst` (lines of "FROM TO" call numbers) makes those calls
take 0.5 s, a file `every3` makes every third call take 0.5 s, a file `slow3rd`
makes the calls whose number is 3 modulo 4 take 0.3 s (with the harness's
command order, the decoder's third command of a round: its null copy), a file
`failfrom` (a call number) makes that call and every later one fail, and a
stream whose name contains `bad` makes the call fail.

    python3 scripts/h264_parity_selftest.py
"""
import argparse
from unittest import mock
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import h264_parity  # noqa: E402
import h264_parity_gcp  # noqa: E402
PARITY = os.path.join(HERE, 'h264_parity.py')

FAKE = '''#!/bin/sh
d=$(dirname "$0")
case "$*" in */bad.sd.264*) exit 1;; esac
echo x >> "$d/count"
n=$(wc -l < "$d/count")
if [ -f "$d/failfrom" ] && [ "$n" -ge "$(cat "$d/failfrom")" ]; then exit 1; fi
s=0.2
if [ -f "$d/burst" ]; then
  while read a b; do
    if [ "$n" -ge "$a" ] && [ "$n" -le "$b" ]; then s=0.5; fi
  done < "$d/burst"
fi
if [ -f "$d/every3" ] && [ $((n % 3)) -eq 0 ]; then s=0.5; fi
if [ -f "$d/slow3rd" ] && [ $((n % 4)) -eq 3 ]; then s=0.3; fi
sleep $s
'''


def fake(root, name):
    d = os.path.join(root, name)
    os.makedirs(d)
    path = os.path.join(d, 'run')
    with open(path, 'w') as f:
        f.write(FAKE)
    os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR)
    return d, path


class Parity(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = self.tmp.name
        self.ff_dir, self.ff = fake(root, 'ff')
        self.dec_dir, self.dec = fake(root, 'dec')
        self.streams = []
        for n in ('a.sd.264', 'b.sd.264'):
            path = os.path.join(root, n)
            open(path, 'w').close()
            self.streams.append(path)
        self.report = os.path.join(root, 'report.json')

    def tearDown(self):
        self.tmp.cleanup()

    def run_parity(self, *extra, streams=None, status=0):
        proc = subprocess.run(
            [sys.executable, PARITY, '--decoder', f'before={self.dec}',
             '--ffmpeg', self.ff, '--rounds', '3', '--report', self.report,
             *extra, *(streams or self.streams)],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        self.assertEqual(proc.returncode, status, proc.stderr)
        self.stderr = proc.stderr
        with open(self.report) as f:
            return json.load(f)

    def calls(self, d):
        with open(os.path.join(d, 'count')) as f:
            return len(f.readlines())

    def touch(self, d, name, text=''):
        with open(os.path.join(d, name), 'w') as f:
            f.write(text)

    def gate_stream(self):
        gate = os.path.join(self.tmp.name, 'gate.264')
        open(gate, 'w').close()
        return gate

    def test_order_is_seeded_and_stored(self):
        one = self.streams[:1]
        first = self.run_parity('--shuffle-seed', '7', streams=one)
        second = self.run_parity('--shuffle-seed', '7', streams=one)
        other = self.run_parity('--shuffle-seed', '8', streams=one)
        orders = first['streams'][0]['order']
        self.assertEqual(orders, second['streams'][0]['order'])
        self.assertNotEqual(orders, other['streams'][0]['order'])
        self.assertEqual(first['method']['shuffle_seed'], 7)
        self.assertEqual(len(orders), 3)
        self.assertTrue(any(o != orders[0] for o in orders),
                        'the order never changed between rounds')
        self.assertEqual(len(first['streams'][0]['rounds']['before_discard']), 3)

    def test_a_call_without_the_new_options_times_what_it_did(self):
        # a one-round disturbance that the verdict would act on with
        # `--null-arm`: without it the stream is timed once, in the fixed order
        self.touch(self.dec_dir, 'burst', '4 4\n')
        report = self.run_parity(streams=self.streams[:1])
        row = report['streams'][0]
        self.assertEqual(row['attempts'], 1)
        self.assertFalse(row['unresolved'])
        self.assertEqual(self.calls(self.dec_dir), 2 + 3 * 2)    # warm-ups, rounds
        self.assertEqual(set(row['seconds']),
                         {'ff_null', 'ff_raw', 'before_discard', 'before_write'})
        self.assertEqual(len({tuple(o) for o in row['order']}), 1)
        self.assertIsNone(report['method']['shuffle_seed'])

    def test_null_arm_is_not_a_pairing(self):
        report = self.run_parity('--null-arm', 'before', '--threads', '1,8',
                                 streams=self.streams[:1])
        row = report['streams'][0]
        for table in (row['seconds'], row['ratio'], report['summary']):
            self.assertFalse([k for k in table if '~null' in k], table)
        self.assertEqual(sorted(row['null']),
                         ['before_discard@1', 'before_discard@8',
                          'before_write@1', 'before_write@8'])
        self.assertEqual(sorted(row['seconds']),
                         sorted(k for k in row['rounds'] if '~null' not in k))

    def test_a_disturbed_stream_is_timed_again(self):
        # decoder calls: 4 warm-ups, then 4 per round; call 7 is in round 1
        self.touch(self.dec_dir, 'burst', '7 7\n')
        report = self.run_parity('--null-arm', 'before', streams=self.streams[:1])
        row = report['streams'][0]
        self.assertGreaterEqual(row['attempts'], 2)
        self.assertFalse(row['unresolved'])
        self.assertTrue(row['history'][0]['faulty'])
        self.assertEqual(len(row['history'][0]['rounds']['before_discard']), 3)
        self.assertEqual(report['method']['retried'], 1)

    def test_a_null_arm_that_deviates_marks_the_stream(self):
        # the null copy is 50% slower in every round: no round spread, only
        # the deviation can mark the stream
        self.touch(self.dec_dir, 'slow3rd')
        report = self.run_parity('--null-arm', 'before', streams=self.streams[:1])
        row = report['streams'][0]
        first = row['history'][0]
        self.assertGreater(first['null']['before_discard'], 0.04)
        self.assertLess(max(first['spread'].values()), 1.50)
        self.assertTrue(first['faulty'])
        self.assertEqual(row['attempts'], 3)
        self.assertTrue(row['unresolved'])

    def test_a_persistent_fault_is_flagged_not_dropped(self):
        self.touch(self.dec_dir, 'every3')
        report = self.run_parity('--null-arm', 'before', '--shuffle-seed', '3',
                                 streams=self.streams[:1])
        row = report['streams'][0]
        self.assertEqual(row['attempts'], 3)
        self.assertTrue(row['unresolved'])
        self.assertEqual(report['method']['unresolved'], 1)
        orders = [tuple(map(tuple, h['order'])) for h in row['history']]
        self.assertEqual(len(set(orders)), 3, 'a retry replayed an earlier order')

    def test_a_failure_when_timed_again_after_a_drift_fails_the_call(self):
        # FFmpeg-script calls as in the drift test below: the check (21-23) and its confirmation (24-26) are
        # slowed, so the stream is timed again; the decoder's calls of the first timing are 2 warm-ups and 3
        # rounds x 2 (1-8), and from call 9, the retime's, it fails. A defect, not a host fault: the stream is
        # listed in `failed`, the call exits with status 3, and the runner voids it
        self.touch(self.ff_dir, 'burst', '21 26\n')
        self.touch(self.dec_dir, 'failfrom', '9')
        report = self.run_parity('--gate-stream', self.gate_stream(), '--gate-every', '1',
                                 streams=self.streams[:1], status=h264_parity.INCOMPLETE)
        self.assertEqual(report['failed'], [{
            'stream': 'a.sd.264', 'class': 'sd',
            'reason': 'timed again after a gate drift: before_discard exited with an error in round 1 of attempt 2'}])
        row = report['streams'][0]
        self.assertTrue(row['unresolved'])
        self.assertEqual(row['attempts'], 1)
        self.assertIn('INCOMPLETE: 1 of 1 streams failed', self.stderr)
        # a healthy VM and reference, one unresolved stream (within the runner's allowance): void for the failure
        out = os.path.join(self.tmp.name, 'out')
        os.makedirs(out)
        os.replace(self.report, os.path.join(out, 't8.sd.json'))
        with open(os.path.join(out, 'host.txt'), 'w') as f:
            f.write('CPU(s):                                  8\nsmt active: 0\n')
        with open(os.path.join(out, 'ref.json'), 'w') as f:
            json.dump({'before': {'median': 1.0}, 'after': {'median': 1.0}}, f)
        verdict = h264_parity_gcp.verdict(out, [('sd', ['es/a.sd.264'])])
        self.assertTrue(verdict['void'], verdict)
        self.assertIn('failed: a.sd.264 (timed again after a gate drift: before_discard exited with an error',
                      ' '.join(verdict['reasons']))

    def test_gate_drift_marks_and_retimes_streams(self):
        # FFmpeg-script calls: the warm gate sample 3 and the baseline 3 x 3,
        # stream a 2 warm-ups + 3 rounds x 2 (calls 13-20), then the check
        # (21-23) and its confirmation (24-26), both slowed
        self.touch(self.ff_dir, 'burst', '21 26\n')
        report = self.run_parity('--gate-stream', self.gate_stream(),
                                 '--gate-every', '1', streams=self.streams[:1])
        gates = report['method']['gates']
        row = report['streams'][0]
        self.assertEqual(report['method']['gate_drift_threshold'], 0.05)
        self.assertEqual([g['drifted'] for g in gates], [False, False, False, True, True, False])
        self.assertEqual([g['kind'] for g in gates], ['baseline'] * 3 + ['check', 'confirm', 'check'])
        self.assertTrue(row['gate_drift'])
        self.assertEqual(row['attempts'], 2)
        self.assertFalse(row['unresolved'])

    def test_the_gate_baseline_is_taken_after_a_warm_sample(self):
        # the first 3 FFmpeg calls (the warm gate sample) are slow, as a cold
        # run is; the baseline must not be that, or every later sample reads
        # as a drift of -60%
        self.touch(self.ff_dir, 'burst', '1 3\n')
        report = self.run_parity('--gate-stream', self.gate_stream(), '--gate-every', '1',
                                 streams=self.streams[:1])
        gates = report['method']['gates']
        self.assertEqual(len(gates), 4)         # the baseline's 3 and one check
        self.assertLess(gates[0]['seconds'], 0.35)
        self.assertFalse(any(g['drifted'] for g in gates))

    def test_the_gate_repeat_comes_from_a_warm_probe(self):
        # the first FFmpeg call (the cold probe of the thrown-away sample) is
        # slow: a K from it would be 2; the baseline's own probe is warm: 5
        self.touch(self.ff_dir, 'burst', '1 1\n')
        report = self.run_parity('--min-seconds', '1', '--rounds', '2', '--gate-stream', self.gate_stream(),
                                 '--gate-threads', '8', '--gate-every', '1', streams=self.streams[:1])
        self.assertEqual({g['repeat'] for g in report['method']['gates']}, {5})

    def test_one_slow_gate_sample_is_not_a_drift(self):
        # the check (calls 21-23, as in the drift test) is slowed, its
        # confirmation is not: an outlier, nothing is timed again
        self.touch(self.ff_dir, 'burst', '21 23\n')
        report = self.run_parity('--gate-stream', self.gate_stream(), '--gate-every', '1',
                                 streams=self.streams[:1])
        self.assertEqual([g['drifted'] for g in report['method']['gates']], [False, False, False, True, False])
        row = report['streams'][0]
        self.assertEqual(row['attempts'], 1)
        self.assertFalse(row.get('gate_drift'))

    def test_a_slow_first_baseline_sample_is_not_the_baseline(self):
        # the first baseline sample (calls 4-6) is slow: the last three do not
        # agree, a fourth is taken, and the baseline is not -60% of the rest
        self.touch(self.ff_dir, 'burst', '4 6\n')
        report = self.run_parity('--gate-stream', self.gate_stream(), '--gate-every', '1',
                                 streams=self.streams[:1])
        gates = report['method']['gates']
        self.assertEqual([g['kind'] for g in gates], ['baseline'] * 4 + ['check'])
        self.assertFalse(gates[4]['drifted'])
        self.assertTrue(report['method']['gate_baseline_stable'])
        self.assertEqual(report['streams'][0]['attempts'], 1)

    def test_two_slow_baseline_samples_are_not_the_baseline_either(self):
        # two slow samples (calls 4-9): a fourth and a fifth are taken
        self.touch(self.ff_dir, 'burst', '4 9\n')
        report = self.run_parity('--gate-stream', self.gate_stream(), '--gate-every', '1',
                                 streams=self.streams[:1])
        gates = report['method']['gates']
        self.assertEqual([g['kind'] for g in gates], ['baseline'] * 5 + ['check'])
        self.assertFalse(gates[5]['drifted'])
        self.assertAlmostEqual(report['method']['gate_base'], 0.2, delta=0.04)

    def test_a_gate_that_stays_drifted_flags_the_streams(self):
        self.touch(self.ff_dir, 'burst', '21 80\n')
        report = self.run_parity('--gate-stream', self.gate_stream(),
                                 '--gate-every', '1', streams=self.streams[:1])
        self.assertTrue(report['method']['gates'][-1]['drifted'])
        self.assertTrue(report['streams'][0]['unresolved'])
        self.assertEqual(report['method']['unresolved'], 1)

    def test_the_last_gate_sample_follows_a_failed_last_stream(self):
        bad = os.path.join(self.tmp.name, 'bad.sd.264')
        open(bad, 'w').close()
        report = self.run_parity('--gate-stream', self.gate_stream(),
                                 '--gate-every', '4',
                                 streams=[self.streams[0], bad], status=h264_parity.INCOMPLETE)
        self.assertEqual(len(report['streams']), 1)
        self.assertEqual([g['after_streams'] for g in report['method']['gates']],
                         [0, 0, 0, 1])

    def test_a_failed_stream_is_named_and_the_call_is_incomplete(self):
        # the past mistake: a stream whose decode failed was printed as "FAILED, skipped", left out of the
        # rows and out of the report, and the summary and every analyzer passed on the streams left
        bad = os.path.join(self.tmp.name, 'bad.sd.264')
        open(bad, 'w').close()
        report = self.run_parity(streams=[self.streams[0], bad, self.streams[1]],
                                 status=h264_parity.INCOMPLETE)
        self.assertEqual(report['requested'], ['a.sd.264', 'bad.sd.264', 'b.sd.264'])
        self.assertEqual([r['stream'] for r in report['streams']], ['a.sd.264', 'b.sd.264'])
        self.assertEqual(report['failed'], [{'stream': 'bad.sd.264', 'class': 'sd',
                                             'reason': 'ff_null exited with an error in round 1 of attempt 1'}])
        self.assertEqual(report['summary']['before:discard']['n'], 2)
        self.assertIn('INCOMPLETE: 1 of 3 streams failed', self.stderr)
        self.assertIn('bad.sd.264', self.stderr)
        # and the shared analyzer refuses the report, naming the stream
        manifest = os.path.join(self.tmp.name, 'es.json')
        with open(manifest, 'w') as f:
            json.dump({n: {'class': 'sd'} for n in report['requested']}, f)
        bars = os.path.join(self.tmp.name, 'bars.json')
        with open(bars, 'w') as f:
            json.dump({'new': 'before', 'old': 'before', 'calls': {'g': {'streams': manifest}},
                       'bars': [{'name': 'X', 'checks': [{'call': 'g', 'pairing': 'discard', 'median_max': 2}]}]}, f)
        os.replace(self.report, os.path.join(self.tmp.name, 'g.sd.json'))
        proc = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'h264_parity_bars.py'), bars, self.tmp.name],
                              capture_output=True, text=True)
        self.assertEqual(proc.returncode, 2, proc.stdout)
        self.assertIn('the harness failed on bad.sd.264', proc.stderr)
        self.assertIn('no row for: bad.sd.264', proc.stderr)

    def test_a_call_whose_streams_all_fail_still_writes_its_report(self):
        bad = os.path.join(self.tmp.name, 'bad.sd.264')
        open(bad, 'w').close()
        report = self.run_parity(streams=[bad], status=h264_parity.INCOMPLETE)
        self.assertEqual(report['streams'], [])
        self.assertEqual(report['summary']['before:discard'],
                         {'n': 0, 'median': None, 'ci_low': None, 'ci_high': None})

    def test_a_complete_call_names_what_it_was_asked(self):
        report = self.run_parity('--rounds', '2')
        self.assertEqual(report['requested'], ['a.sd.264', 'b.sd.264'])
        self.assertEqual(report['failed'], [])

    def test_gate_cadence_and_a_quiet_gate(self):
        extra = os.path.join(self.tmp.name, 'c.sd.264')
        open(extra, 'w').close()
        report = self.run_parity('--gate-stream', self.gate_stream(),
                                 '--gate-every', '2',
                                 streams=self.streams + [extra])
        self.assertEqual([g['after_streams'] for g in report['method']['gates']],
                         [0, 0, 0, 2, 3])
        self.assertFalse(any(g['drifted'] for g in report['method']['gates']))
        self.assertEqual(len(report['streams']), 3)

    def test_min_seconds_repeats_every_command_alike(self):
        # a single run lasts 0.2 s, so 1 s needs 5 runs of every command; the
        # decoder's second command (write) is slower in its warm-up (call 2:
        # 0.5 s), which would give 2 runs if K were taken per command, and
        # the minimum of the warm-ups gives K from the fastest
        self.touch(self.dec_dir, 'burst', '2 2\n')
        report = self.run_parity('--min-seconds', '1', '--rounds', '2',
                                 streams=self.streams[:1])
        row = report['streams'][0]
        self.assertEqual(row['repeat'], 5)
        self.assertEqual(report['method']['min_seconds'], 1.0)
        # warm-ups once, then rounds x commands x repeats, in each stand-in
        self.assertEqual(self.calls(self.dec_dir), 2 + 2 * 2 * 5)
        self.assertEqual(self.calls(self.ff_dir), 2 + 2 * 2 * 5)
        for name, times in row['rounds'].items():
            for t in times:      # a time is of one run
                self.assertTrue(0.15 < t < 0.35, (name, t))

    def test_a_retry_keeps_the_stream_repeat(self):
        # decoder calls with the null arm: 4 warm-ups, round 1 is calls 5-24
        # (discard x5, write x5, ...), round 2 is 25-44: slowing every run of
        # one command in round 1 marks attempt 1. Attempt 2's warm-ups are
        # decoder calls 45-48 and FFmpeg calls 23-24 (2 warm-ups and 2 rounds
        # x 2 commands x 5 repeats = 22 before them), all slowed to 0.5 s, so
        # that a K computed again would be 2, not 5
        self.touch(self.dec_dir, 'burst', '10 14\n45 48\n')
        self.touch(self.ff_dir, 'burst', '23 24\n')
        report = self.run_parity('--min-seconds', '1', '--rounds', '2',
                                 '--null-arm', 'before', streams=self.streams[:1])
        row = report['streams'][0]
        self.assertGreaterEqual(row['attempts'], 2)
        self.assertEqual({h['repeat'] for h in row['history']}, {5})

    def test_a_gate_retime_keeps_the_stream_repeat(self):
        # FFmpeg-script calls: the warm gate sample 3 and the baseline 3 x 3
        # (without --gate-threads the gate does not repeat); stream a: 2
        # warm-ups, 2 rounds x 2 commands x 5 repeats = 22 (calls 13-34); the
        # check is calls 35-37 and its confirmation 38-40 (slowed); the
        # retime's warm-ups are FFmpeg calls 41-42 and decoder calls 23-24,
        # slowed too, so that K computed again would be 2, not 5
        self.touch(self.ff_dir, 'burst', '35 42\n')
        self.touch(self.dec_dir, 'burst', '23 24\n')
        report = self.run_parity('--min-seconds', '1', '--rounds', '2',
                                 '--gate-stream', self.gate_stream(),
                                 '--gate-every', '1', streams=self.streams[:1])
        row = report['streams'][0]
        self.assertTrue(row['gate_drift'])
        self.assertEqual(row['attempts'], 2)
        self.assertEqual({h['repeat'] for h in row['history']}, {5})
        self.assertEqual(row['repeat'], 5)

    def test_min_seconds_is_validated(self):
        for bad in (['--min-seconds', '0'], ['--min-seconds', '-1'],
                    ['--min-seconds', '1', '--threads', '1,8']):
            proc = subprocess.run(
                [sys.executable, PARITY, '--decoder', f'before={self.dec}',
                 '--ffmpeg', self.ff, *bad, self.streams[0]],
                capture_output=True)
            self.assertNotEqual(proc.returncode, 0, bad)

    def test_a_call_without_min_seconds_runs_once(self):
        report = self.run_parity('--rounds', '2', streams=self.streams[:1])
        self.assertEqual(report['streams'][0]['repeat'], 1)
        self.assertIsNone(report['method']['min_seconds'])

    def test_the_gate_runs_on_its_own_core_with_gate_pin(self):
        # a taskset that records its arguments, then runs the command
        stub = os.path.join(self.tmp.name, 'bin')
        os.makedirs(stub)
        log = os.path.join(self.tmp.name, 'taskset.log')
        path = os.path.join(stub, 'taskset')
        with open(path, 'w') as f:
            f.write(f'#!/bin/sh\necho "$1 $2" >> {log}\nshift 2\nexec "$@"\n')
        os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR)
        env = dict(os.environ, PATH=stub + os.pathsep + os.environ['PATH'])
        subprocess.run(
            [sys.executable, PARITY, '--decoder', f'before={self.dec}', '--ffmpeg', self.ff, '--rounds', '2',
             '--report', self.report, '--gate-stream', self.gate_stream(), '--gate-pin', '3', '--gate-every', '1',
             self.streams[0]], check=True, stdout=subprocess.DEVNULL, env=env)
        with open(log) as f:
            lines = f.read().split('\n')
        self.assertEqual(lines.count('-c 3'), 15)     # five gate samples of 3 runs (the first thrown away)
        self.assertEqual([l for l in lines if l and l != '-c 3'], [])
        with open(self.report) as f:
            self.assertEqual(len(json.load(f)['method']['gates']), 4)

    def test_gate_threads_is_validated(self):
        for bad in (['--gate-threads', '8'], ['--gate-threads', '0', '--gate-stream', self.gate_stream()],
                    ['--gate-threads', '8', '--gate-stream', self.gate_stream(), '--pin', '2']):
            proc = subprocess.run([sys.executable, PARITY, '--decoder', f'before={self.dec}', '--ffmpeg', self.ff,
                                   *bad, self.streams[0]], capture_output=True)
            self.assertEqual(proc.returncode, 2, bad)

    def test_bars_given_to_a_call_replace_those_of_its_thread_count(self):
        report = self.run_parity('--null-arm', 'before', '--null-bar', '0.02', '--spread-bar', '1.25',
                                 streams=self.streams[:1])
        self.assertEqual(report['method']['null_deviation'], 0.02)
        self.assertEqual(report['method']['round_spread'], 1.25)
        args = argparse.Namespace(threads=[8], null_arm='before', null_bar=0.02, spread_bar=None)
        self.assertEqual(h264_parity.thresholds(args), (0.02, 1.50))
        args = argparse.Namespace(threads=[1], null_arm='before', null_bar=None, spread_bar=1.25)
        self.assertEqual(h264_parity.thresholds(args), (0.04, 1.25))
        # 3% null deviation is a fault under a 2% bar, and not under the eight-thread 8%
        times = {'before_discard': [1.0, 1.0, 1.0], 'before~null_discard': [1.03] * 3}
        faulty = h264_parity.judge(argparse.Namespace(threads=[8], null_arm='before', null_bar=0.02,
                                                      spread_bar=None), times)['faulty']
        quiet = h264_parity.judge(argparse.Namespace(threads=[8], null_arm='before'), times)['faulty']
        self.assertTrue(faulty)
        self.assertFalse(quiet)

    def test_the_gate_sample_follows_the_calls_threads_and_repeat(self):
        seen = []

        def fake_timed(argv, repeat=1):
            seen.append((argv, repeat))
            return 0.5

        ns = argparse.Namespace(ffmpeg='ff', gate_stream='g', gate_frames=9, gate_threads=8, gate_pin=None,
                                prefix=[], min_seconds=3)
        with mock.patch.object(h264_parity, 'timed', fake_timed):
            value, repeat = h264_parity.gate_sample(ns)
            self.assertEqual((value, repeat), (0.5, 6))
            self.assertEqual([r for _, r in seen], [1, 6, 6, 6])      # a probe, then 3 calls of 6 runs
            self.assertEqual(seen[0][0][seen[0][0].index('-threads') + 1], '8')
            seen.clear()
            self.assertEqual(h264_parity.gate_sample(ns, 4), (0.5, 4))    # K is kept after the first sample
            self.assertEqual([r for _, r in seen], [4, 4, 4])
            seen.clear()
            ns.gate_threads = None
            self.assertEqual(h264_parity.gate_sample(ns), (0.5, 1))       # without the option: 3 runs, no probe
            self.assertEqual(len(seen), 3)
            self.assertEqual(seen[0][0][seen[0][0].index('-threads') + 1], '1')


class Thresholds(unittest.TestCase):
    """The verdict's thresholds, on made-up times: no process is run."""

    def verdict(self, threads, null_ratio, spread):
        args = argparse.Namespace(threads=threads, null_arm='before')
        times = {'before_discard': [1.0, 1.0, spread],
                 'before~null_discard': [null_ratio] * 3}
        return h264_parity.judge(args, times)

    def test_one_thread_uses_the_pinned_thresholds(self):
        self.assertEqual(h264_parity.thresholds(argparse.Namespace(threads=[1])), (0.04, 1.50))
        self.assertTrue(self.verdict([1], 1.0, 1.60)['faulty'])
        self.assertTrue(self.verdict([1], 1.05, 1.0)['faulty'])
        self.assertFalse(self.verdict([1], 1.03, 1.40)['faulty'])

    def test_several_threads_use_the_calibrated_thresholds(self):
        self.assertEqual(h264_parity.thresholds(argparse.Namespace(threads=[8])), (0.08, 1.50))
        self.assertFalse(self.verdict([8], 1.05, 1.43)['faulty'])      # the calibration's maximum
        self.assertTrue(self.verdict([8], 1.10, 1.0)['faulty'])
        self.assertTrue(self.verdict([8], 1.0, 1.60)['faulty'])


if __name__ == '__main__':
    unittest.main()
