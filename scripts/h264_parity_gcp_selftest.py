#!/usr/bin/env python3
"""Self-test of scripts/h264_parity_gcp.py: the upload directory, the generated job.sh run locally against
sleeping stand-ins (no Compute Engine, no CPU to speak of), and the void verdict.

    python3 scripts/h264_parity_gcp_selftest.py
"""
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import h264_parity_gcp as gcp  # noqa: E402

FAKE = '#!/bin/sh\nsleep 0.2\n'


def fake(path):
    with open(path, 'w') as f:
        f.write(FAKE)
    os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR)


class Gcp(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        r = self.tmp.name
        self.es = os.path.join(r, 'es')
        os.makedirs(self.es)
        for n in ('a.0.sd.h264', 'b.0.sd.h264', 'c.0.720p.h264', 'd.0.720p.h264'):
            open(os.path.join(self.es, n), 'w').close()
        self.dec, self.ff, self.gate = (os.path.join(r, n) for n in ('dec', 'ff', 'gate.h264'))
        fake(self.dec)
        fake(self.ff)
        open(self.gate, 'w').close()
        self.up = os.path.join(r, 'up')
        self.out = os.path.join(r, 'out')

    def tearDown(self):
        self.tmp.cleanup()

    def build(self, *extra):
        return subprocess.run(
            [sys.executable, os.path.join(HERE, 'h264_parity_gcp.py'), '--out', self.out, '--es-dir', self.es,
             '--decoder', f'base={self.dec}', '--decoder', f'k={self.dec}', '--ffmpeg', self.ff,
             '--gate-stream', self.gate, '--null-arm', 'base', '--dry-run', '--upload-dir', self.up,
             '--limit-per-class', '1', '--classes', 'sd,720p', '--cpu-match', '', *extra],
            capture_output=True, text=True, check=True)

    def test_the_upload_and_the_job(self):
        proc = self.build('--rounds', '2', '--min-seconds', '0.5')
        self.assertIn('2 streams in 2 calls', proc.stdout)
        self.assertIn('spend bound 3.00 USD of 5.00', proc.stdout)
        self.assertIn('--machine c4-standard-16', proc.stdout)
        self.assertIn('--provisioning standard --threads-per-core 1', proc.stdout)
        self.assertEqual(sorted(os.listdir(os.path.join(self.up, 'es'))), ['a.0.sd.h264', 'c.0.720p.h264'])
        with open(os.path.join(self.up, 'job.sh')) as f:
            job = f.read()
        self.assertIn('--threads 8 --rounds 2 --min-seconds 0.5', job)
        self.assertIn('--gate-threads 8', job)
        self.assertNotIn('--gate-pin', job)
        self.assertIn('--null-bar 0.02 --spread-bar 1.25 --null-arm base', job)
        self.assertIn('--frames 1800', job)
        self.assertIn('--frames 900', job)
        self.assertNotIn('--require-cpu', job)
        self.assertEqual(subprocess.run(['bash', '-n', os.path.join(self.up, 'job.sh')]).returncode, 0)

    def test_another_thread_count(self):
        self.build('--threads', '1', '--pin', '3')
        with open(os.path.join(self.up, 'job.sh')) as f:
            job = f.read()
        self.assertIn('--threads 1 --rounds', job)
        self.assertNotIn('--gate-threads', job)
        self.assertIn('--null-arm base --pin 3', job)
        self.assertIn('--report out/t1.sd.json', job)
        self.assertNotIn('t8.', job)

    def test_the_cpu_check_and_the_cap(self):
        self.build('--cpu-match', '8581C')
        with open(os.path.join(self.up, 'job.sh')) as f:
            self.assertIn('--require-cpu 8581C', f.read())
        proc = subprocess.run(
            [sys.executable, os.path.join(HERE, 'gcp_vm_job.py'), '--upload', self.up, '--job', 'job.sh',
             '--download', self.out, '--machine', gcp.MACHINE, '--usd-per-hour', str(gcp.USD_PER_HOUR),
             '--max-hours', '9', '--cap-usd', '5'], capture_output=True, text=True)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn('above the cap', proc.stderr + proc.stdout)

    def test_the_job_runs_and_the_verdict_reads_its_files(self):
        self.build('--rounds', '2', '--min-seconds', '0.5', '--gate-frames', '300')
        job = os.path.join(self.up, 'job.sh')
        subprocess.run(['bash', 'job.sh'], cwd=self.up, check=True, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
        out = os.path.join(self.up, 'out')
        for name in ('host.txt', 'ref.json', 'progress.txt', 't8.sd.json', 't8.720p.json'):
            self.assertTrue(os.path.exists(os.path.join(out, name)), name)
        with open(os.path.join(out, 't8.sd.json')) as f:
            report = json.load(f)
        self.assertEqual(report['threads'], [8])
        self.assertEqual(report['method']['min_seconds'], 0.5)
        self.assertEqual(report['method']['null_deviation'], 0.02)
        self.assertEqual(report['method']['round_spread'], 1.25)
        with open(os.path.join(out, 'ref.json')) as f:
            ref = json.load(f)
        self.assertEqual(sorted(ref), ['after', 'before'])
        v = gcp.verdict(out, [('sd', ['es/a.0.sd.h264']), ('720p', ['es/c.0.720p.h264'])])
        self.assertEqual(v['unresolved'], {'sd': 0, '720p': 0})
        self.assertIn('SMT is not off', ' '.join(v['reasons']))      # this host is not the VM
        self.assertIn('reference_drift', v)

    CALLS = [('sd', ['a', 'b']), ('720p', ['c'])]

    def verdict_dir(self, before, after, streams, host=True):
        """A results directory: `streams` maps a class to (streams in its report, unresolved)."""
        self.n = getattr(self, 'n', 0) + 1
        d = os.path.join(self.tmp.name, f'v{self.n}')
        os.makedirs(d)
        with open(os.path.join(d, 'ref.json'), 'w') as f:
            json.dump({'before': {'median': before}, 'after': {'median': after}}, f)
        if host:
            with open(os.path.join(d, 'host.txt'), 'w') as f:
                f.write('CPU(s):                                  8\nsmt active: 0\n')
        for cls, (n, unresolved) in streams.items():
            with open(os.path.join(d, f't8.{cls}.json'), 'w') as f:
                json.dump({'method': {'unresolved': unresolved}, 'streams': [{}] * n}, f)
        return d

    def test_a_run_is_void_on_drift_unresolved_or_a_missing_report(self):
        ok = gcp.verdict(self.verdict_dir(1.0, 1.04, {'sd': (2, 2), '720p': (1, 0)}), self.CALLS)
        self.assertFalse(ok['void'], ok)
        for args, why in (((1.0, 1.06, {'sd': (2, 0), '720p': (1, 0)}), 'moved'),
                          ((1.0, 0.94, {'sd': (2, 0), '720p': (1, 0)}), 'moved'),
                          ((1.0, 1.0, {'sd': (2, 3), '720p': (1, 0)}), '3 unresolved'),
                          ((1.0, 1.0, {'sd': (2, 0)}), 'no report for t8.720p'),
                          ((1.0, 1.0, {'sd': (1, 0), '720p': (1, 0)}), '1 of 2 streams'),
                          ((0.0, 1.0, {'sd': (2, 0), '720p': (1, 0)}), 'no usable reference'),
                          ((1.0, float('nan'), {'sd': (2, 0), '720p': (1, 0)}), 'no usable reference')):
            v = gcp.verdict(self.verdict_dir(*args), self.CALLS)
            self.assertTrue(v['void'], args)
            self.assertIn(why, ' '.join(v['reasons']), args)

    def test_the_host_must_have_8_vcpus_and_smt_off(self):
        d = self.verdict_dir(1.0, 1.0, {'sd': (2, 0), '720p': (1, 0)})
        with open(os.path.join(d, 'host.txt'), 'w') as f:
            f.write('CPU(s):                                  16\nsmt active: 1\n')
        v = gcp.verdict(d, self.CALLS)
        self.assertTrue(v['void'])
        self.assertIn('16 vCPUs', ' '.join(v['reasons']))
        self.assertIn('SMT is not off', ' '.join(v['reasons']))
        none = gcp.verdict(self.verdict_dir(1.0, 1.0, {'sd': (2, 0), '720p': (1, 0)}, host=False), self.CALLS)
        self.assertIn('no host.txt', none['reasons'])

    def test_odd_files_make_a_void_verdict_not_a_crash(self):
        d = self.verdict_dir(1.0, 1.0, {'sd': (2, 0), '720p': (1, 0)})
        with open(os.path.join(d, 't8.sd.json'), 'w') as f:
            json.dump([1, 2], f)
        with open(os.path.join(d, 't8.720p.json'), 'w') as f:
            f.write('{not json')
        v = gcp.verdict(d, self.CALLS)
        self.assertTrue(v['void'])
        self.assertEqual(sum('no report' in r for r in v['reasons']), 2)

    def run_main(self, returncode, write_results):
        """main() with the VM job replaced: `write_results(out)` stands for what comes back."""
        def fake_run(cmd, *a, **k):
            if 'gcp_vm_job.py' in cmd[1]:
                write_results(cmd[cmd.index('--download') + 1])
                return mock.Mock(returncode=returncode)
            return subprocess.run(cmd, *a, **k)
        argv = ['h264_parity_gcp.py', '--out', self.out, '--es-dir', self.es, '--decoder', f'base={self.dec}',
                '--ffmpeg', self.ff, '--gate-stream', self.gate, '--null-arm', 'base', '--classes', 'sd',
                '--limit-per-class', '2']
        with mock.patch.object(sys, 'argv', argv), mock.patch.object(gcp.subprocess, 'run', fake_run):
            return gcp.main()

    def test_exit_statuses(self):
        def good(out):
            d = self.verdict_dir(1.0, 1.01, {'sd': (2, 0)})
            os.makedirs(out, exist_ok=True)
            for n in os.listdir(d):
                os.replace(os.path.join(d, n), os.path.join(out, n))
        self.assertEqual(self.run_main(0, good), 0)
        self.assertTrue(os.path.exists(os.path.join(self.out, 'verdict.json')))
        self.tearDown()
        self.setUp()

        def drifted(out):
            d = self.verdict_dir(1.0, 1.2, {'sd': (2, 0)})
            os.makedirs(out, exist_ok=True)
            for n in os.listdir(d):
                os.replace(os.path.join(d, n), os.path.join(out, n))
        self.assertEqual(self.run_main(0, drifted), 3)
        self.tearDown()
        self.setUp()
        # the VM job failed: status 4 and no verdict, whatever came back
        self.assertEqual(self.run_main(1, good), 4)
        self.assertFalse(os.path.exists(os.path.join(self.out, 'verdict.json')))

    def test_arguments_are_checked_before_a_VM_is_created(self):
        for bad in (['--decoder', 'a b=x'], ['--cap-usd', '11'], ['--max-hours', '9', '--cap-usd', '5'],
                    ['--rounds', '0'], ['--limit-per-class', '0'], ['--classes', 'bogus'],
                    ['--decoder', f'base={self.dec}'], ['--decoder', f'-x={self.dec}']):
            argv = [sys.executable, os.path.join(HERE, 'h264_parity_gcp.py'), '--out', self.out, '--es-dir', self.es,
                    '--decoder', f'base={self.dec}', '--ffmpeg', self.ff, '--gate-stream', self.gate,
                    '--null-arm', 'base', '--dry-run', *bad]
            proc = subprocess.run(argv, capture_output=True, text=True)
            self.assertEqual(proc.returncode, 2, (bad, proc.stderr))
        os.makedirs(self.out, exist_ok=True)
        open(os.path.join(self.out, 'old.json'), 'w').close()
        proc = subprocess.run(argv[:argv.index('--dry-run')], capture_output=True, text=True)
        self.assertEqual(proc.returncode, 2)
        self.assertIn('not empty', proc.stderr)

    # the one-thread class

    def build_1t(self, *extra):
        refs = []
        for n in ('refa.h264', 'refb.h264'):
            path = os.path.join(self.tmp.name, n)
            open(path, 'w').close()
            refs.append(f'{path}:300')
        argv = [sys.executable, os.path.join(HERE, 'h264_parity_gcp.py'), '--out', self.out, '--es-dir', self.es,
                '--decoder', f'base={self.dec}', '--ffmpeg', self.ff, '--gate-stream', self.gate, '--null-arm', 'base',
                '--dry-run', '--upload-dir', self.up, '--limit-per-class', '1', '--classes', 'sd,720p',
                '--cpu-match', '', '--one-thread-class', '--ref-stream', refs[0], '--ref-stream', refs[1], *extra]
        if '--turbo' not in extra:
            argv += ['--turbo', 'unrestricted']
        return subprocess.run(argv, capture_output=True, text=True, check=True)

    def test_one_thread_class_upload_and_job(self):
        proc = self.build_1t('--rounds', '2')
        self.assertIn('--machine c4-standard-8', proc.stdout)
        self.assertIn('spend bound 1.50 USD of 5.00', proc.stdout)
        self.assertEqual(sorted(os.listdir(os.path.join(self.up, 'refs'))), ['refa.h264', 'refb.h264', 'refs.json'])
        with open(os.path.join(self.up, 'job.sh')) as f:
            job = f.read()
        self.assertIn('--pin 2 --rounds 2', job)
        self.assertNotIn('--min-seconds', job)
        self.assertNotIn('--threads', job)
        self.assertIn('--null-bar 0.04 --spread-bar 1.5 --null-arm base', job)
        self.assertIn('--report out/g.sd.json', job)
        self.assertEqual(subprocess.run(['bash', '-n', os.path.join(self.up, 'job.sh')]).returncode, 0)
        self.assertTrue(os.path.exists(os.path.join(self.up, 'gate1t.py')))
        self.assertIn('python3 gate1t.py freq start', job)
        self.assertIn('python3 gate1t.py freq "after sd #$attempt"', job)
        self.assertIn('python3 gate1t.py freq end', job)
        self.assertNotIn('--turbo-mode', proc.stdout)

    def test_one_thread_class_at_all_core_max(self):
        proc = self.build_1t('--rounds', '2', '--turbo', 'all-core-max')
        self.assertIn('--turbo-mode ALL_CORE_MAX', proc.stdout)

    def test_one_thread_class_frequency_rules(self):
        out = self.run_1t_job()
        with open(os.path.join(out, 'freq.json')) as f:
            labels = [s['label'] for s in json.load(f)]
        self.assertEqual(labels, ['start', 'after sd #1', 'after 720p #1', 'end'])
        local = set(gcp.verdict(out, self.CALLS_1T, 1, True)['reasons'])     # this machine is not the VM
        self.assertEqual(set(gcp.verdict(out, self.CALLS_1T, 1, True, 'unrestricted')['reasons']), local)

        def d(features, samples):
            self.n = getattr(self, 'n', 0) + 1
            p = os.path.join(self.tmp.name, f'f{self.n}')
            shutil.copytree(out, p)
            if features is None:
                os.remove(os.path.join(p, 'instance.json')) if os.path.exists(os.path.join(p, 'instance.json')) else None
            else:
                with open(os.path.join(p, 'instance.json'), 'w') as f:
                    json.dump({'advanced_machine_features': features}, f)
            with open(os.path.join(p, 'freq.json'), 'w') as f:
                json.dump(samples, f)
            v = gcp.verdict(p, self.CALLS_1T, 1, True, 'all-core-max')
            return {'reasons': [r for r in v['reasons'] if r not in local]}
        acm = {'threadsPerCore': 1, 'turboMode': 'ALL_CORE_MAX'}
        good = [{'label': l, 'ghz': 3.09} for l in ('start', 'after sd #1', 'after 720p #1', 'end')]
        self.assertEqual(d(acm, good)['reasons'], [])
        self.assertIn('turboMode', ' '.join(d({'threadsPerCore': 1}, good)['reasons']))
        self.assertIn('not on record', ' '.join(d(None, good)['reasons']))
        self.assertIn("'after sd #1': 3.9", ' '.join(d(acm, good[:1] + [{'label': 'after sd #1', 'ghz': 3.9}] + good[2:])['reasons']))
        self.assertIn('CalledProcessError', ' '.join(d(acm, good[:3] + [{'label': 'end', 'error': 'CalledProcessError'}])['reasons']))
        self.assertIn('at the end', ' '.join(d(acm, good[:3])['reasons']))

    def run_1t_job(self, ff_script=None):
        self.build_1t('--rounds', '2')
        if ff_script:
            with open(os.path.join(self.up, 'ffmpeg'), 'w') as f:
                f.write(ff_script)
        env = dict(os.environ, REF_SLEEP='1', CHECK_SLEEP='1', PERF_INSTALL='0')
        subprocess.run(['bash', 'job.sh'], cwd=self.up, check=True, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, env=env)
        return os.path.join(self.up, 'out')

    CALLS_1T = [('sd', ['es/a.0.sd.h264']), ('720p', ['es/c.0.720p.h264'])]

    def test_one_thread_class_job_and_verdict(self):
        out = self.run_1t_job()
        for name in ('host.txt', 'progress.txt', 'gate-refs.json', 'gate-checks.log', 'g.sd.json', 'g.720p.json',
                     'g.sd.gate_ok', 'g.720p.gate_ok'):
            self.assertTrue(os.path.exists(os.path.join(out, name)), name)
        with open(os.path.join(out, 'g.sd.json')) as f:
            report = json.load(f)
        self.assertEqual(report['threads'], [1])
        self.assertEqual(report['pinned_core'], 2)
        self.assertEqual(report['method']['null_deviation'], 0.04)
        v = gcp.verdict(out, self.CALLS_1T, 1, True)
        self.assertEqual(v['unresolved'], {'sd': 0, '720p': 0})
        self.assertFalse([r for r in v['reasons'] if 'gate' in r or 'references' in r], v['reasons'])

    def test_one_thread_class_reruns_a_call_whose_after_check_fails(self):
        # gate calls: 3 references x 6 = 1-18; the first class's before check 19-24, its after check 25-30 (slowed)
        ff = ('#!/bin/sh\ncase "$*" in *refs/*) echo x >> gatecount; n=$(wc -l < gatecount); '
              'if [ "$n" -ge 25 ] && [ "$n" -le 30 ]; then sleep 0.6; else sleep 0.2; fi;; *) sleep 0.2;; esac\n')
        out = self.run_1t_job(ff)
        self.assertTrue(os.path.exists(os.path.join(out, 'g.sd.json.void1')))
        self.assertTrue(os.path.exists(os.path.join(out, 'g.sd.gate_ok')))
        with open(os.path.join(out, 'gate-checks.log')) as f:
            lines = f.read().split('\n')
        self.assertTrue(any(l.startswith('FAIL after sd #1') for l in lines))
        self.assertTrue(any(l.startswith('PASS after sd #2') for l in lines))

    def test_one_thread_class_verdict_rules(self):
        def d(refs, ok_classes, host_cpus=4):
            self.n = getattr(self, 'n', 0) + 1
            p = os.path.join(self.tmp.name, f'w{self.n}')
            os.makedirs(p)
            with open(os.path.join(p, 'gate-refs.json'), 'w') as f:
                json.dump(refs, f)
            with open(os.path.join(p, 'host.txt'), 'w') as f:
                f.write(f'CPU(s):                                  {host_cpus}\nsmt active: 0\n')
            for cls, _ in self.CALLS_1T:
                with open(os.path.join(p, f'g.{cls}.json'), 'w') as f:
                    json.dump({'method': {'unresolved': 0}, 'streams': [{}]}, f)
                if cls in ok_classes:
                    open(os.path.join(p, f'g.{cls}.gate_ok'), 'w').close()
            return p
        good = [{'label': 'r%d' % i, 's': 1.0 + 0.01 * i} for i in range(3)]
        self.assertFalse(gcp.verdict(d(good, ('sd', '720p')), self.CALLS_1T, 1, True)['void'])
        bad = gcp.verdict(d(good, ('sd',)), self.CALLS_1T, 1, True)
        self.assertTrue(bad['void'])
        self.assertIn('g.720p: no attempt passed its gate checks', bad['reasons'])
        far = [{'label': 'r%d' % i, 's': t} for i, t in enumerate((1.0, 1.0, 1.2))]
        self.assertIn('disagree', ' '.join(gcp.verdict(d(far, ('sd', '720p')), self.CALLS_1T, 1, True)['reasons']))
        self.assertIn('not 4', ' '.join(gcp.verdict(d(good, ('sd', '720p'), 8), self.CALLS_1T, 1, True)['reasons']))
        self.assertIn('three reference', ' '.join(gcp.verdict(d(good[:2], ('sd', '720p')), self.CALLS_1T, 1, True)['reasons']))

    def test_one_thread_class_does_not_start_a_call_on_a_host_that_fails_its_gate(self):
        # every gate call after the references (1-18) is slow: no call may start, in any attempt
        ff = ('#!/bin/sh\ncase "$*" in *refs/*) echo x >> gatecount; n=$(wc -l < gatecount); '
              'if [ "$n" -ge 19 ]; then sleep 0.6; else sleep 0.2; fi;; *) echo x >> callcount; sleep 0.2;; esac\n')
        out = self.run_1t_job(ff)
        self.assertFalse(os.path.exists(os.path.join(out, 'g.sd.json')))
        self.assertFalse(os.path.exists(os.path.join(out, 'g.sd.gate_ok')))
        self.assertFalse(os.path.exists(os.path.join(self.up, 'callcount')))
        with open(os.path.join(out, 'progress.txt')) as f:
            self.assertEqual(f.read().count('the host failed its gate, no call'), 6)     # 3 attempts, two classes
        v = gcp.verdict(out, self.CALLS_1T, 1, True)
        self.assertTrue(v['void'])

    def argv_1t(self, *bad):
        return [sys.executable, os.path.join(HERE, 'h264_parity_gcp.py'), '--out', self.out, '--es-dir', self.es,
                '--decoder', f'base={self.dec}', '--ffmpeg', self.ff, '--gate-stream', self.gate, '--null-arm', 'base',
                '--classes', 'sd', '--dry-run', *bad]

    def test_one_thread_class_arguments(self):
        refs = []
        for n in ('refa.h264', 'refb.h264'):
            path = os.path.join(self.tmp.name, n)
            open(path, 'w').close()
            refs += ['--ref-stream', f'{path}:300']
        refs_t = refs + ['--turbo', 'unrestricted']
        for bad in (['--ref-stream', 'x:1'], refs_t + ['--threads', '8'], refs_t + ['--min-seconds', '3'],
                    refs_t + ['--pin', '4'], refs[:2] + ['--ref-stream', 'nofile:\u00b2', '--turbo', 'unrestricted'],
                    refs, refs + ['--turbo', 'fixed']):
            self.assertEqual(subprocess.run(self.argv_1t('--one-thread-class', *bad), capture_output=True,
                                            text=True).returncode, 2, bad)
        self.assertEqual(subprocess.run(self.argv_1t('--one-thread-class', *refs_t), capture_output=True,
                                        text=True).returncode, 0)
        self.assertEqual(subprocess.run(self.argv_1t('--ref-stream', f'{self.gate}:5'), capture_output=True,
                                        text=True).returncode, 2)
        self.assertEqual(subprocess.run(self.argv_1t('--turbo', 'all-core-max'), capture_output=True,
                                        text=True).returncode, 2)

    def test_the_eight_thread_defaults_are_unchanged(self):
        self.build()
        with open(os.path.join(self.up, 'job.sh')) as f:
            job = f.read()
        self.assertIn('--threads 8 --rounds 10 --min-seconds 3 --shuffle-seed', job)
        self.assertIn('--null-bar 0.02 --spread-bar 1.25', job)
        self.assertIn('min_seconds=3)', job)


if __name__ == '__main__':
    unittest.main()
