"""scripts/codec_coverage_gcp.py: the host record, the run manifest's checks, the rerun job and its verdict, and the
limits the driver refuses to pass. Nothing leaves this machine and nothing is spent."""
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
import codec_coverage_gcp as ccg  # noqa: E402

SCRIPT = str(ROOT / 'scripts' / 'codec_coverage_gcp.py')
BUILD_ID = ccg.pin()['BUILD']


def stub(path, text):
    path.write_text('#!/bin/sh\n' + text)
    path.chmod(0o755)
    return path


def sha(data):
    return hashlib.sha256(data).hexdigest()


class Binaries:
    """A decode-oracle, ffmpeg and ffprobe that print a version line, and the manifest that names them."""

    def make(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.bin = self.dir / 'bin'
        self.bin.mkdir()
        for name in ccg.BINARIES:
            stub(self.bin / name, f'echo "{name} version {BUILD_ID} Copyright"\n')
        digest = {n: ccg.sha256(self.bin / n) for n in ccg.BINARIES}
        self.manifest = {'decode-oracle': {'sha256': digest['decode-oracle']},
                         'ffmpeg': {n: {'sha256': digest[n]} for n in ('ffmpeg', 'ffprobe')}}
        (self.dir / 'manifest.json').write_text(json.dumps(self.manifest))


class Host(Binaries, unittest.TestCase):
    def setUp(self):
        self.make()

    def tearDown(self):
        self.tmp.cleanup()

    def test_the_host_names_the_binaries_and_the_fixtures(self):
        fixtures = self.dir / 'fixtures'
        fixtures.mkdir()
        for name in ccg.FIXTURES:
            (fixtures / name).write_bytes(name.encode())
        h = ccg.host('v2-v3', self.bin, fixtures, self.bin)
        self.assertEqual(h['ffmpeg']['ffprobe']['version'], f'ffprobe version {BUILD_ID} Copyright')
        self.assertEqual(h['fixtures']['basic.mp4'], sha(b'basic.mp4'))
        self.assertEqual(h['decode-oracle'], self.manifest['decode-oracle']['sha256'])
        self.assertEqual(h['cores'], os.cpu_count())


class Manifest(unittest.TestCase):
    KEY = 'ab' * 32

    def good(self):
        build = {'commit': 'c' * 40,
                 'coreav': {'sha256': '1' * 64, 'features': 'release-full', 'profile': 'dist',
                            'libraries': ccg.codec_versions()},
                 'decode-oracle': {'sha256': '2' * 64, 'features': 'native-h264,dav1d,libvpx', 'profile': 'dist',
                                   'target': 'x86_64-unknown-linux-musl'},
                 'x264': {'sha256': '3' * 64, 'commit': ccg.pin()['X264_COMMIT']},
                 'ffmpeg-harness': {'sha256': '4' * 64, 'features': 'release-full'},
                 'evidence_key_sha256': self.KEY}
        ff = {n: {'sha256': '5' * 64, 'version': f'{n} version {BUILD_ID}'} for n in ('ffmpeg', 'ffprobe')}
        hosts = [{'job': 'v1-0', 'ffmpeg': ff}, {'job': 'v1-1', 'ffmpeg': ff},
                 {'job': 'v2-v3', 'ffmpeg': ff, 'fixtures': {n: '6' * 64 for n in ccg.FIXTURES}}]
        counts = [{'shard': 0, 'files': 2, 'records': 2, 'pending': 0}, {'shard': 1, 'files': 2, 'records': 1,
                                                                           'pending': 1}]
        return build, hosts, counts

    def problems(self, build, hosts, counts, set_name='v1', shards=2):
        return ccg.manifest_problems(build, hosts, set_name, shards, counts, self.KEY)

    def test_the_plan_s_manifest_passes(self):
        self.assertEqual(self.problems(*self.good()), [])

    def test_each_item_that_differs_is_named(self):
        cases = [
            (lambda b, h, c: b['coreav'].update(features='release-default'), 'not the full build'),
            (lambda b, h, c: b['coreav']['libraries'].update(dav1d='0'), 'versions codecs.sh pins'),
            (lambda b, h, c: b['decode-oracle'].update(features='native-h264'), 'decode-oracle is not built'),
            (lambda b, h, c: b['ffmpeg-harness'].update(features='cli'), 'test executable'),
            (lambda b, h, c: b['x264'].update(commit='0' * 40), 'x264'),
            (lambda b, h, c: b.update(evidence_key_sha256='cd' * 32), 'evidence key'),
            (lambda b, h, c: b['coreav'].update(sha256=None), 'coreav has no SHA-256'),
            (lambda b, h, c: h[1].update(ffmpeg={**h[1]['ffmpeg'], 'ffmpeg': {'sha256': '7' * 64,
                                                                            'version': BUILD_ID}}),
             'different ffmpeg'),
            (lambda b, h, c: [x['ffmpeg'].update({n: {'sha256': '5' * 64, 'version': 'ffmpeg version 7.0.2'}
                                                  for n in ('ffmpeg', 'ffprobe')}) for x in h], 'pinned build'),
            (lambda b, h, c: h.pop(), 'five fixtures'),
            (lambda b, h, c: c.pop(), '1 of 2 V1 shards'),
        ]
        for change, words in cases:
            with self.subTest(words=words):
                build, hosts, counts = json.loads(json.dumps(self.good()))
                change(build, hosts, counts)
                self.assertTrue(any(words in p for p in self.problems(build, hosts, counts)),
                                self.problems(build, hosts, counts))

    def test_a_pull_request_has_no_shards(self):
        build, hosts, _ = self.good()
        self.assertEqual(self.problems(build, hosts, [], 'fixtures', 0), [])

    def test_the_command_writes_the_manifest_and_fails_on_a_problem(self):
        build, hosts, counts = self.good()
        with tempfile.TemporaryDirectory() as tmp:
            d = pathlib.Path(tmp)
            (d / 'build.json').write_text(json.dumps(build))
            for i, h in enumerate(hosts):
                (d / f'host-{i}.json').write_text(json.dumps(h))
            for c in counts:
                (d / f"v1-{c['shard']}.json").write_text(json.dumps(c))
            args = [sys.executable, '-I', SCRIPT, 'manifest', '--build', str(d / 'build.json'), '--hosts',
                    *(str(d / f'host-{i}.json') for i in range(len(hosts))), '--set', 'v1', '--shards', '2',
                    '--shard-counts', str(d / 'v1-0.json'), str(d / 'v1-1.json'), '--evidence-key', self.KEY,
                    '--out', str(d / 'manifest.json')]
            done = subprocess.run(args, capture_output=True, text=True, timeout=60)
            self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
            self.assertIn('4 files, 3 records, 1 left for the rerun host', done.stdout)
            written = json.loads((d / 'manifest.json').read_text())
            self.assertEqual(written['fixtures'], hosts[2]['fixtures'])
            self.assertEqual([h['job'] for h in written['hosts']], ['v1-0', 'v1-1', 'v2-v3'])
            args[args.index('--shards') + 1] = '3'
            self.assertEqual(subprocess.run(args, capture_output=True, timeout=60).returncode, 1)


PENDING = [{'sha256': sha(str(i).encode()), 'key': f'corpus/v1/real/{i}.mp4', 'ext': 'mp4', 'size': 10,
            'family': 'mp4-mov'} for i in range(3)]


class Limits(Binaries, unittest.TestCase):
    def setUp(self):
        self.make()
        (self.dir / 'pending.json').write_text(json.dumps(PENDING))

    def tearDown(self):
        self.tmp.cleanup()

    def run_(self, *extra, env=None, pending=None):
        if pending is not None:
            (self.dir / 'pending.json').write_text(json.dumps(pending))
        args = [sys.executable, '-I', SCRIPT, 'rerun', '--pending', str(self.dir / 'pending.json'), '--manifest',
                str(self.dir / 'manifest.json'), '--bin', str(self.bin), '--out', str(self.dir / 'out'), *extra]
        creds = {k: 'x' for k in ccg.CREDENTIALS}
        return subprocess.run(args, capture_output=True, text=True, timeout=60,
                              env={**os.environ, **(env if env is not None else creds)})

    def refused(self, *extra, **kw):
        done = self.run_(*extra, **kw)
        self.assertEqual(done.returncode, 2, done.stderr)
        return done.stderr

    def test_the_spend_cap(self):
        self.assertIn('at most 10', self.refused('--cap-usd', '11'))
        self.assertIn('above --cap-usd', self.refused('--max-hours', '9'))
        self.assertIn('--usd-per-hour is required', self.refused('--machine', 'c4-standard-8'))
        for flag, value in (('--cap-usd', 'nan'), ('--max-hours', 'inf'), ('--max-hours', '-1')):
            self.assertIn('must be a finite number above 0', self.refused(flag, value))

    def test_six_hours_a_side(self):
        self.assertIn('less than 360 minutes a side', self.refused('--max-hours', '6'))

    def test_the_inputs(self):
        self.assertIn('must be set', self.refused(env={k: '' for k in ccg.CREDENTIALS}))
        self.assertIn('nothing to rerun', self.refused(pending=[]))
        self.assertIn('need sha256, key, ext and size', self.refused(pending=[{'sha256': 'x'}]))

    def test_the_binaries_must_be_the_run_s(self):
        stub(self.bin / 'ffprobe', 'echo another\n')
        self.assertIn("ffprobe is not the run's", self.refused('--dry-run'))

    def test_a_dry_run_leaves_no_credentials_behind(self):
        done = self.run_('--dry-run')
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn('--machine c4-standard-16', done.stdout)
        self.assertIn('--provisioning standard', done.stdout)
        up = self.dir / 'out' / 'upload'
        self.assertFalse((up / 'r2.env').exists())
        self.assertEqual(json.loads((up / 'pending.json').read_text()), PENDING)
        for rel in ccg.UPLOADED:
            self.assertTrue((up / 'repo' / rel).is_file(), rel)


class Job(unittest.TestCase):
    def test_the_job_is_valid_bash(self):
        with tempfile.NamedTemporaryFile('w', suffix='.sh') as f:
            f.write(ccg.job_script(4))
            f.flush()
            subprocess.run(['bash', '-n', f.name], check=True)

    def test_the_runner_gets_six_hours_a_side_and_a_deadline_before_the_power_off(self):
        job = ccg.job_script(4)
        self.assertIn('--limit-minutes 360 --rerun-host', job)
        self.assertIn('($1 - 10) * 60', job)
        self.assertIn('--fetch data', job)
        self.assertLess(job.index('trap finish EXIT'), job.index('. ./r2.env'))

    def run_job(self, records):
        """job.sh with decode_breadth.py replaced by a stub that writes RECORDS, then the verdict."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        out = pathlib.Path(tmp.name)
        up = out / 'upload'
        (up / 'bin').mkdir(parents=True)
        for name in ccg.BINARIES:
            stub(up / 'bin' / name, 'echo version\n')
        (up / 'pending.json').write_text(json.dumps(PENDING))
        (up / 'r2.env').write_text('R2_BUCKET=x\n')
        fake = stub(out / 'breadth.sh', 'printf "%s" "$RECORDS" > out/rerun.jsonl\nprintf "%s\\n" "$@" > out/args\n')
        job = ccg.job_script(2).replace('python3 -I repo/tools/compat-corpus/decode_breadth.py run', str(fake))
        job = job.replace('python3 -I repo/scripts/codec_coverage_gcp.py host', 'echo')
        (up / 'job.sh').write_text(job)
        done = subprocess.run(['bash', 'job.sh', '0'], cwd=up, capture_output=True, text=True, timeout=60,
                              env={**os.environ, 'RECORDS': ''.join(json.dumps(r) + '\n' for r in records)})
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertFalse((up / 'r2.env').exists())
        import shutil
        shutil.copytree(up / 'out', out / 'vm')
        (out / 'vm' / 'job.status').write_text('0\n')
        code = ccg.judge(out, 0, {'machine': 'c4-standard-16', 'local': True})
        return code, json.loads((out / 'verdict.json').read_text()), (out / 'vm' / 'args').read_text().split('\n')

    def test_every_file_with_a_result_completes_the_rerun(self):
        code, verdict, args = self.run_job([{'sha256': e['sha256'], 'code': 'md5'} for e in PENDING])
        self.assertEqual((code, verdict['verdict']), (0, 'complete'))
        self.assertNotIn('--deadline', args)

    def test_a_file_without_a_result_leaves_the_rerun_incomplete(self):
        records = [{'sha256': PENDING[0]['sha256'], 'code': 'ok'},
                   {'sha256': PENDING[1]['sha256'], 'host_limit': 'job_end'}]
        code, verdict, _ = self.run_job(records)
        self.assertEqual(code, 5)
        self.assertEqual(verdict['reasons'], ['2 of 3 files have no result'])

    def test_a_vm_of_another_class_is_named(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = pathlib.Path(tmp)
            (out / 'upload').mkdir()
            (out / 'vm').mkdir()
            (out / 'upload' / 'pending.json').write_text(json.dumps(PENDING[:1]))
            (out / 'vm' / 'job.status').write_text('0\n')
            (out / 'vm' / 'instance.json').write_text(json.dumps({'machine_type': 'c4-standard-8'}))
            (out / 'vm' / 'rerun.jsonl').write_text(json.dumps({'sha256': PENDING[0]['sha256']}) + '\n')
            self.assertEqual(ccg.judge(out, 0, {'machine': 'c4-standard-16', 'local': False}), 5)
            self.assertIn('the VM is c4-standard-8, not c4-standard-16',
                          json.loads((out / 'verdict.json').read_text())['reasons'])

    def test_nothing_came_back(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(ccg.judge(tmp, 1, {'machine': 'c4-standard-16', 'local': False}), 4)


if __name__ == '__main__':
    unittest.main()
