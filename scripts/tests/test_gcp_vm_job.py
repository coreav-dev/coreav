"""scripts/gcp_vm_job.py and the --collect of scripts/h264_parity_gcp.py finish a job after the machine that runs them
restarts. Compute Engine and ssh are faked: no request leaves this machine and nothing is spent.

The fake cloud is a directory: vms.json holds its VMs, calls.log the requests, and homes/<vm> the home directory where
that VM's ssh commands run under bash. A run that is killed runs in a child process (this file with 'drive'), so that
it dies as a controller does, with no finally.

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from contextlib import ExitStack
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import gcp_vm_job  # noqa: E402
import h264_parity_gcp  # noqa: E402

REAL_RUN = subprocess.run
JOB = ('mkdir -p out\n'
       'echo "$1" > out/arg.txt\n'
       'for i in $(seq 400); do [ -e release ] && break; sleep 0.05; done\n'
       'echo done > out/result.txt\n'
       'exit 3\n')


def cloud():
    return pathlib.Path(os.environ['FAKE_CLOUD'])


def vms():
    p = cloud() / 'vms.json'
    return json.loads(p.read_text()) if p.exists() else {}


def home(ip):
    for name, vm in vms().items():
        if vm['ip'] == ip:
            return cloud() / 'homes' / name
    return None


class FakeCompute:
    def __init__(self, key):
        self.key = key

    def call(self, method, path, body=None, ok404=False):
        with open(cloud() / 'calls.log', 'a') as f:
            f.write(f'{method} {path}\n')
        if '/images/family/' in path:
            return {'selfLink': 'image'}
        all_ = vms()
        if method == 'POST':
            all_[body['name']] = {'ip': f'10.0.0.{len(all_) + 1}', 'body': body}
            (cloud() / 'homes' / body['name']).mkdir(parents=True)
            (cloud() / 'vms.json').write_text(json.dumps(all_))
            return {'status': 'DONE'}
        name = path.rsplit('/', 1)[-1]
        if name not in all_:
            if ok404:
                return None
            raise SystemExit(f'{method} {path}: HTTP 404: not found')
        if method == 'DELETE':
            del all_[name]
            (cloud() / 'vms.json').write_text(json.dumps(all_))
            return {'status': 'DONE'}
        return {'status': 'RUNNING', 'machineType': 'zones/z/machineTypes/c4-standard-8', 'zone': 'zones/us-central1-a',
                'scheduling': {'provisioningModel': 'STANDARD'},
                'networkInterfaces': [{'accessConfigs': [{'natIP': all_[name]['ip']}]}]}

    def wait(self, op, limit=600):
        return op


def local(spec):
    """A path of scp: coreav@IP:PATH in that VM's home, or a local path."""
    if spec.startswith('coreav@'):
        ip, _, rest = spec[len('coreav@'):].partition(':')
        return os.path.join(home(ip), rest)
    return spec


def fake_run(cmd, *args, **kw):
    if cmd[0] == 'ssh-keygen':
        key = cmd[cmd.index('-f') + 1]
        pathlib.Path(key).write_text('private')
        pathlib.Path(key + '.pub').write_text('ssh-ed25519 AAAA coreav')
        return subprocess.CompletedProcess(cmd, 0)
    if cmd[0] == 'ssh':
        host = next(c for c in cmd if c.startswith('coreav@'))
        h = home(host[len('coreav@'):])
        if h is None:
            return subprocess.CompletedProcess(cmd, 255, stdout='', stderr='')
        env = dict(os.environ, HOME=str(h), PATH=str(cloud() / 'bin') + os.pathsep + os.environ['PATH'])
        return REAL_RUN(['bash', '-c', ' '.join(cmd[cmd.index(host) + 1:])], *args, cwd=h, env=env, **kw)
    if cmd[0] == 'scp':
        shutil.copytree(local(cmd[-2]), local(cmd[-1]), dirs_exist_ok=True)
        return subprocess.CompletedProcess(cmd, 0)
    return REAL_RUN(cmd, *args, **kw)


def fakes():
    stack = ExitStack()
    stack.enter_context(mock.patch.object(gcp_vm_job, 'Compute', FakeCompute))
    stack.enter_context(mock.patch.object(gcp_vm_job.subprocess, 'run', fake_run))
    stack.enter_context(mock.patch.object(gcp_vm_job, 'POLL', 0.05))
    return stack


def vm_job(*argv):
    with fakes(), mock.patch.object(sys, 'argv', ['gcp_vm_job.py', *argv]):
        return gcp_vm_job.main()


class Restart(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        r = pathlib.Path(self.tmp.name)
        (r / 'cloud' / 'bin').mkdir(parents=True)
        shim = r / 'cloud' / 'bin' / 'timeout'
        shim.write_text('#!/bin/sh\nshift\nexec "$@"\n')
        shim.chmod(0o755)
        self.env = mock.patch.dict(os.environ, {
            'FAKE_CLOUD': str(r / 'cloud'), 'COREAV_GCP_PROJECT': 'fake-project',
            'COREAV_GCP_SA_KEY': json.dumps({'client_email': 'e', 'private_key': 'k', 'token_uri': 't'})})
        self.env.start()
        self.up = r / 'up'
        self.up.mkdir()
        (self.up / 'job.sh').write_text(JOB)
        self.out = r / 'out'
        self.state = str(self.out) + '.gcpjob'
        self.argv = ['--upload', str(self.up), '--job', 'job.sh hello', '--download', str(self.out),
                     '--provisioning', 'standard']

    def tearDown(self):
        for job in (cloud() / 'homes').glob('*/job'):
            (job / 'release').touch()
        self.env.stop()
        time.sleep(0.2)
        self.tmp.cleanup()

    def killed_run(self):
        """A run killed while its job runs: the VM and the job are left, as after a restart."""
        with open(cloud() / 'controller.log', 'w') as log:
            p = subprocess.Popen([sys.executable, '-I', __file__, 'drive', *self.argv], stdout=log, stderr=log)
        try:
            for _ in range(400):
                pids = list((cloud() / 'homes').glob('*/job/job.pid'))
                if pids:
                    break
                time.sleep(0.05)
            else:
                self.fail('the job never started: ' + (cloud() / 'controller.log').read_text())
        finally:
            p.kill()
            p.wait()
        self.assertEqual(len(vms()), 1)
        self.assertFalse((self.out / 'result.txt').exists())
        return pids[0].parent

    def test_a_run_brings_the_output_back_and_deletes_the_vm(self):
        (self.up / 'release').touch()
        vm_job(*self.argv)
        self.assertEqual((self.out / 'result.txt').read_text(), 'done\n')
        self.assertEqual((self.out / 'arg.txt').read_text(), 'hello\n')
        self.assertEqual((self.out / 'job.status').read_text(), '3\n')
        self.assertEqual(json.loads((self.out / 'spend.json').read_text())['status'], 'job status 3')
        self.assertEqual(vms(), {})
        self.assertFalse(os.path.exists(os.path.join(self.state, 'id')), 'the ssh key goes with the VM')
        with self.assertRaises(SystemExit) as e:
            vm_job(*self.argv)
        self.assertIn('--collect', str(e.exception))

    def test_collect_finishes_a_run_whose_controller_was_killed(self):
        job = self.killed_run()
        (job / 'release').touch()
        vm_job('--collect', self.state)
        self.assertEqual((self.out / 'result.txt').read_text(), 'done\n')
        self.assertEqual((self.out / 'job.status').read_text(), '3\n')
        spend = (self.out / 'spend.json').read_text()
        self.assertEqual(json.loads(spend)['status'], 'job status 3')
        self.assertEqual(vms(), {})
        self.assertFalse(os.path.exists(os.path.join(self.state, 'id')))
        calls = (cloud() / 'calls.log').read_text()
        vm_job('--collect', self.state)
        self.assertEqual((cloud() / 'calls.log').read_text(), calls, 'a second collect calls Compute Engine again')
        self.assertEqual((self.out / 'spend.json').read_text(), spend)

    def test_collect_past_the_time_limit_deletes_the_vm_without_waiting(self):
        self.killed_run()
        later = time.time() + 10 * 3600
        with mock.patch.object(gcp_vm_job.time, 'time', return_value=later), self.assertRaises(SystemExit) as e:
            vm_job('--collect', self.state)
        self.assertIn('time limit', str(e.exception))
        self.assertEqual(vms(), {})
        self.assertFalse((self.out / 'result.txt').exists())
        self.assertEqual(json.loads((self.out / 'spend.json').read_text())['usd_bound'], 2 * 0.60,
                         'the spend bound counts at most --max-hours')
        with self.assertRaises(SystemExit):
            vm_job('--collect', self.state)
        self.assertEqual(vms(), {})


class WrapperCollect(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.r = pathlib.Path(self.tmp.name)
        self.out = self.r / 'out'

    def tearDown(self):
        self.tmp.cleanup()

    def main(self, *argv, rc=0):
        cmds = []

        def run(cmd, *a, **k):
            cmds.append(cmd)
            return subprocess.CompletedProcess(cmd, rc)
        with mock.patch.object(sys, 'argv', ['h264_parity_gcp.py', *argv]), \
                mock.patch.object(h264_parity_gcp.subprocess, 'run', run):
            return h264_parity_gcp.main(), cmds

    def test_a_run_writes_what_collect_needs(self):
        es = self.r / 'es'
        es.mkdir()
        (es / 'a.0.sd.h264').touch()
        for n in ('dec', 'ff', 'gate.h264'):
            (self.r / n).touch()
        status, cmds = self.main('--out', str(self.out), '--es-dir', str(es), '--decoder', f'base={self.r / "dec"}',
                                 '--ffmpeg', str(self.r / 'ff'), '--gate-stream', str(self.r / 'gate.h264'),
                                 '--null-arm', 'base', '--classes', 'sd', '--cpu-match', '', rc=1)
        self.assertEqual(status, 4)
        cmd = cmds[0]
        self.assertEqual(cmd[cmd.index('--state') + 1], str(self.out) + '.gcpjob')
        run = json.loads((self.out / 'run.json').read_text())
        self.assertEqual(run['calls'], [['sd', ['es/a.0.sd.h264']]])

    def test_collect_finishes_the_vm_job_and_writes_the_verdict(self):
        self.out.mkdir()
        (self.out / 'run.json').write_text(json.dumps(
            {'calls': [['sd', ['es/a.0.sd.h264']]], 'threads': 8, 'one_thread_class': False, 'turbo': None}))
        (self.out / 'host.txt').write_text('CPU(s):                                  8\nsmt active: 0\n')
        (self.out / 'ref.json').write_text(json.dumps({'before': {'median': 1.0}, 'after': {'median': 1.01}}))
        (self.out / 't8.sd.json').write_text(json.dumps({'method': {'unresolved': 0}, 'streams': [{}]}))
        status, cmds = self.main('--collect', str(self.out))
        self.assertEqual(status, 0)
        self.assertEqual(cmds[0][-2:], ['--collect', str(self.out) + '.gcpjob'])
        self.assertFalse(json.loads((self.out / 'verdict.json').read_text())['void'])
        self.assertEqual(self.main('--collect', str(self.out), rc=1)[0], 4)


if __name__ == '__main__' and sys.argv[1:2] == ['drive']:
    vm_job(*sys.argv[2:])
