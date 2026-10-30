"""scripts/compat_r4_gcp.py and tools/compat-corpus/round_files.py: the files of each set, the job the VM runs, the
limits the driver refuses to pass and the verdict it writes. Nothing leaves this machine and nothing is spent."""
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
sys.path.insert(0, str(ROOT / 'tools' / 'compat-corpus'))
import compat_r4_gcp  # noqa: E402
import round_files  # noqa: E402

MB = 1024 * 1024


def entry(n, ext, size=MB, **kw):
    sha = hashlib.sha256(f'{n}{ext}'.encode()).hexdigest()
    return {'sha256': sha, 'ext': ext, 'size': size, 'set': 'real', 'holdout': True, 'key': f'{sha}.{ext}', **kw}


class Sets(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = pathlib.Path(self.tmp.name)
        v1 = [entry(i, 'mp4') for i in range(30)] + [entry(i, 'webm') for i in range(5)]
        v1 += [entry('big', 'mov', 300 * MB), entry('edge', 'mp4', set='edge'), entry('dev', 'mkv', holdout=False),
               entry('ts-in-v1', 'ts'), entry('mp3', 'mp3')]
        for e in v1:
            e['key'] = 'corpus/v1/real/' + e['key']
        ts = [entry(i, 'ts') for i in range(3)] + [entry('aac', 'aac')]
        v2 = [entry(f'v2-{i}', 'mp4', draw=6, family='iso') for i in range(4)]
        v2 += [entry(f'v2-{i}', 'webm', draw=6, family='matroska') for i in range(2)]
        v2 += [entry('odd', 'mp4', draw=6, family='ts')]
        v2 += [entry(f'v2-{i}', 'm2ts', draw=5) for i in range(2)] + [entry('h', 'mp4', draw=1)]
        for e in v2:
            e['key'] = 'corpus/v2-heldout/' + e['key']
        for name, files in (('v1', v1), ('v1-ts-audio', ts), ('v2-heldout', v2)):
            (root / 'corpus' / name).mkdir(parents=True)
            (root / 'corpus' / name / 'manifest.json').write_text(json.dumps({'files': files}))
        self.root = str(root)

    def tearDown(self):
        self.tmp.cleanup()

    def families(self, files):
        return [e['family'] for e in files]

    def test_the_draw_is_draw_6_alone_where_the_extension_agrees_with_the_family(self):
        files = round_files.select('draw6', self.root)
        self.assertEqual(self.families(files), ['mp4-mov'] * 4 + ['matroska-webm'] * 2)
        self.assertTrue(all(e['key'].startswith('corpus/v2-heldout/') for e in files))

    def test_the_gate_takes_spent_held_out_real_files_in_the_r4_order(self):
        files = round_files.select('r4-gate', self.root)
        self.assertEqual(self.families(files), ['mp4-mov'] * 20 + ['matroska-webm'] * 5 + ['mpegts'] * 3)
        mp4 = [e for e in files if e['family'] == 'mp4-mov']
        order = [hashlib.sha256(('coreav-compat-r4:' + e['sha256']).encode()).hexdigest() for e in mp4]
        self.assertEqual(order, sorted(order))
        self.assertTrue(all(e['size'] <= 256 * MB for e in files))
        self.assertTrue(all(e['key'].startswith('corpus/v1-ts-audio/') for e in files if e['family'] == 'mpegts'))

    def test_the_r4_selection_and_its_cap(self):
        self.assertEqual(len(round_files.select('r4-draw6', self.root)), 6)
        self.assertEqual(self.families(round_files.select('r4-draw6', self.root, per_family=1)),
                         ['mp4-mov', 'matroska-webm'])

    def test_rehearsals_never_take_draw_6(self):
        draw6 = {e['sha256'] for e in round_files.select('draw6', self.root)}
        for name in ('rehearsal', 'r4-rehearsal'):
            files = round_files.select(name, self.root)
            self.assertTrue(files)
            self.assertFalse(draw6 & {e['sha256'] for e in files}, name)
        self.assertEqual(self.families(round_files.select('r4-rehearsal', self.root)), ['mp4-mov', 'mpegts', 'mpegts'])


def args(*extra):
    return [sys.executable, '-I', str(ROOT / 'scripts' / 'compat_r4_gcp.py'), '--out', '/nonexistent/out',
            '--coreav', 'c', '--ffmpeg', 'f', '--ffprobe', 'p', *extra]


class Limits(unittest.TestCase):
    def refused(self, *extra, env=None):
        done = subprocess.run(args(*extra), capture_output=True, text=True, timeout=60,
                              env={**os.environ, **(env or {k: 'x' for k in compat_r4_gcp.CREDENTIALS})})
        self.assertEqual(done.returncode, 2, done.stderr)
        return done.stderr

    def test_the_spend_cap(self):
        self.assertIn('at most 10', self.refused('--cap-usd', '11'))
        self.assertIn('above --cap-usd', self.refused('--max-hours', '9'))
        self.assertIn('--usd-per-hour is required', self.refused('--machine', 'c4-standard-4'))

    def test_the_store_credentials(self):
        env = {k: '' for k in compat_r4_gcp.CREDENTIALS}
        self.assertIn('must be set', self.refused(env=env))


class Job(unittest.TestCase):
    def test_the_gate_comes_before_the_first_read_of_the_draw(self):
        job = compat_r4_gcp.job_script(5, '4-7', rehearsal=False)
        self.assertLess(job.index('--files gate.json'), job.index('--arm "$arm"'))
        self.assertLess(job.index('echo "$gate" > out/gate.verdict'), job.index('--files draw.json'))
        self.assertIn('[ "$gate" = pass ] || [ false = true ] || exit 3', job)
        self.assertIn('sudo shutdown -P', job)
        self.assertIn('[ "$gate" = pass ] || [ true = true ]', compat_r4_gcp.job_script(5, '4-7', rehearsal=True))

    def test_only_a_failing_gate_report_fails_the_gate(self):
        for codes, verdict in (((0, 0), 'pass'), ((1, 0), 'fail'), ((0, 3), 'error'), ((1, 2), 'error'),
                               ((2, 1), 'error')):
            with tempfile.TemporaryDirectory() as tmp:
                d = pathlib.Path(tmp)
                job = compat_r4_gcp.job_script(1, '0', rehearsal=False)
                job = job.replace('python3 -I repo/tools/compat-corpus/speed.py r4-report', 'sh ./report.sh')
                job = job.replace('python3 -I repo/tools/compat-corpus/speed.py r4 ', 'true ')
                job = job.replace('python3 -I repo/tools/compat-corpus/round_files.py fetch', 'true')
                job = job.replace('chmod 700 bin/coreav bin/ffmpeg bin/ffprobe', 'true')
                job = job.replace('bin/ffmpeg -version', 'echo')
                (d / 'report.sh').write_text('n=$(cat n 2>/dev/null || echo 0); echo $((n + 1)) > n\n'
                                             f'[ "$n" = 0 ] && exit {codes[0]}; exit {codes[1]}\n')
                (d / 'r2.env').write_text('')
                subprocess.run(['bash', '-c', job.split('python3 -I - > out/host.json')[0]
                                + job.split('\nPY\n', 1)[1], 'job', '0'], cwd=d, capture_output=True, timeout=60)
                self.assertEqual((d / 'out' / 'gate.verdict').read_text().strip(), verdict, codes)

    def test_the_job_is_valid_bash(self):
        with tempfile.NamedTemporaryFile('w', suffix='.sh') as f:
            f.write(compat_r4_gcp.job_script(5, '4-7', rehearsal=False))
            f.flush()
            subprocess.run(['bash', '-n', f.name], check=True)


class Verdict(unittest.TestCase):
    def vm(self, files, run=None):
        self.tmp = tempfile.TemporaryDirectory()
        out = pathlib.Path(self.tmp.name)
        (out / 'vm').mkdir()
        for name, content in files.items():
            (out / 'vm' / name).write_text(content if isinstance(content, str) else json.dumps(content))
        run = {'machine': 'c4-standard-16', 'local': False, 'rehearsal': False, 'files': {}, **(run or {})}
        return out, run

    def tearDown(self):
        if hasattr(self, 'tmp'):
            self.tmp.cleanup()

    HOST = {'instance.json': {'machine_type': 'c4-standard-16', 'provisioning': 'STANDARD',
                              'advanced_machine_features': {'threadsPerCore': 1}},
            'host.json': {'smt_active': '0'}, 'job.status': '0'}
    REPORT = {'fallback_log': {'void': False}, 'failing': [], 'classes': [{'class': 'probe', 'family': 'all'}]}

    def test_r4(self):
        out, run = self.vm({**self.HOST, 'gate.verdict': 'pass', 'gate-aa.report.json': self.REPORT,
                            'gate-null.report.json': self.REPORT, 'r4.report.json': self.REPORT})
        self.assertEqual(compat_r4_gcp.judge(str(out), 0, run), 0)
        self.assertEqual(json.loads((out / 'verdict.json').read_text())['verdict'], 'r4')

    def test_a_failed_gate_is_a_host_fault(self):
        out, run = self.vm({**self.HOST, 'gate.verdict': 'fail', 'gate-aa.report.json': self.REPORT,
                            'gate-null.report.json': {**self.REPORT, 'failing': ['probe all']}})
        self.assertEqual(compat_r4_gcp.judge(str(out), 0, run), 3)

    def test_another_class_voids_the_run(self):
        host = {**self.HOST, 'host.json': {'smt_active': '1'}}
        out, run = self.vm({**host, 'gate.verdict': 'pass', 'gate-aa.report.json': self.REPORT,
                            'gate-null.report.json': self.REPORT, 'r4.report.json': self.REPORT})
        self.assertEqual(compat_r4_gcp.judge(str(out), 0, run), 5)
        self.assertIn('SMT is not off', json.loads((out / 'verdict.json').read_text())['reasons'])

    def test_a_missing_report_or_a_void_log_voids_the_run(self):
        out, run = self.vm({**self.HOST, 'gate.verdict': 'pass', 'gate-aa.report.json': self.REPORT,
                            'gate-null.report.json': self.REPORT})
        self.assertEqual(compat_r4_gcp.judge(str(out), 0, run), 5)
        out, run = self.vm({**self.HOST, 'gate.verdict': 'pass', 'gate-aa.report.json': self.REPORT,
                            'gate-null.report.json': self.REPORT,
                            'r4.report.json': {**self.REPORT, 'fallback_log': {'void': True}}})
        self.assertEqual(compat_r4_gcp.judge(str(out), 0, run), 5)

    def test_a_report_that_measured_nothing_voids_the_run(self):
        out, run = self.vm({**self.HOST, 'gate.verdict': 'pass', 'gate-aa.report.json': self.REPORT,
                            'gate-null.report.json': self.REPORT, 'r4.report.json': {**self.REPORT, 'classes': []}})
        self.assertEqual(compat_r4_gcp.judge(str(out), 0, run), 5)
        self.assertIn('r4 measured nothing', json.loads((out / 'verdict.json').read_text())['reasons'])
        out, run = self.vm({**self.HOST, 'gate.verdict': 'fail', 'gate-aa.report.json': self.REPORT,
                            'gate-null.report.json': {**self.REPORT, 'failing': ['clip all'],
                                                      'unmeasured': ['clip all']}})
        self.assertEqual(compat_r4_gcp.judge(str(out), 0, run), 5)
        self.assertIn('gate-null left clip all unmeasured', json.loads((out / 'verdict.json').read_text())['reasons'])
        out, run = self.vm({**self.HOST, 'gate.verdict': 'error', 'gate-aa.report.json': self.REPORT,
                            'gate-null.report.json': {**self.REPORT, 'classes': []}})
        self.assertEqual(compat_r4_gcp.judge(str(out), 0, run), 5)

    def test_nothing_came_back(self):
        out, run = self.vm({})
        self.assertEqual(compat_r4_gcp.judge(str(out), 1, run), 4)


if __name__ == '__main__':
    unittest.main()
