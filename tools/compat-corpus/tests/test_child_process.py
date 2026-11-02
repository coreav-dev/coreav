"""Launcher capture and ownership regressions with generated Python children."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
import r3  # noqa: E402
import encode_breadth as eb  # noqa: E402


class Capture(unittest.TestCase):
    def test_capture_drains_both_pipes_and_bounds_a_busy_writer(self):
        code = ('import os\n'
                'for _ in range(64):\n'
                '    os.write(1, b"a" * 4096)\n'
                '    os.write(2, b"b" * 4096)\n')
        done, timed_out = r3.capture([sys.executable, '-c', code], dict(os.environ), 10)
        self.assertFalse(timed_out)
        self.assertEqual(done.returncode, 0)
        self.assertEqual(done.stdout, b'a' * (64 * 4096))
        self.assertEqual(done.stderr, b'b' * (64 * 4096))
        busy = 'import os, time\nwhile True:\n    os.write(1, b"x" * 4096)\n    time.sleep(0.001)\n'
        done, timed_out = r3.capture([sys.executable, '-c', busy], dict(os.environ), 0.3)
        self.assertTrue(timed_out)
        self.assertNotEqual(done.returncode, 0)
        self.assertTrue(done.stdout)

    def test_callers_preserve_status_streams_and_output_files(self):
        env = dict(os.environ)
        self.assertEqual(r3.run([sys.executable, '-c', 'raise SystemExit(17)'], env), 17)
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            probe = root / 'probe'
            probe.write_text(f'#!{sys.executable}\nprint(\'{json.dumps({"streams": [{"index": 7}]})}\')\n')
            probe.chmod(0o755)
            self.assertEqual(r3.streams(str(probe), root / 'unused', env), [{'index': 7}])
            ran = eb.execute([sys.executable, '-c', 'import sys; print("output"); print("detail", file=sys.stderr)'],
                             root, env, 10, pipe=True)
            self.assertEqual((ran.code, ran.timed_out, ran.stderr), (0, False, 'detail\n'))
            self.assertEqual((root / eb.STDOUT).read_text(), 'output\n')

    @unittest.skipUnless(sys.platform == 'linux', 'Linux adopted-child ownership')
    def test_timeouts_reap_each_launchers_adopted_descendants(self):
        for launcher in ('run', 'streams', 'execute'):
            with self.subTest(launcher=launcher):
                self.scenario(launcher, False, False)

    @unittest.skipUnless(sys.platform == 'linux', 'Linux pipe-held timeout')
    def test_a_reaped_leader_cannot_lose_ownership_to_a_pipe_timeout(self):
        self.scenario('run', True, False)

    @unittest.skipUnless(sys.platform == 'linux', 'Linux concurrent ownership')
    def test_overlapping_workers_do_not_reap_each_others_children(self):
        self.scenario('run', False, True)

    def scenario(self, launcher, pipe_held, overlap):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tree = root / 'tree'
            grandchild = 'import os, time; print(os.getpid(), flush=True); time.sleep(60)'
            child = ('import os, subprocess, sys, time\n'
                     f'p = subprocess.Popen([sys.executable, "-c", {grandchild!r}], stdout=subprocess.PIPE)\n'
                     'print(os.getpid(), int(p.stdout.readline()), flush=True)\n'
                     'time.sleep(60)\n')
            tree.write_text(f'''#!{sys.executable}
import os, pathlib, subprocess, sys, time
tag = os.environ['TAG']
root = pathlib.Path({str(root)!r})
p = subprocess.Popen([sys.executable, '-c', {child!r}], stdout=subprocess.PIPE,
                     stderr=None if {pipe_held!r} else subprocess.DEVNULL)
pids = [os.getpid(), *map(int, p.stdout.readline().split())]
(root / tag).write_text(' '.join(map(str, pids)))
if {overlap!r}:
    while not (root / 'release').exists():
        time.sleep(0.01)
    raise SystemExit(int(tag))
if not {pipe_held!r}:
    time.sleep(60)
''')
            tree.chmod(0o755)
            script = f'''
import concurrent.futures, contextlib, ctypes, os, pathlib, signal, subprocess, sys, time
sys.path.insert(0, {str(HERE)!r})
import r3, encode_breadth as eb
libc = ctypes.CDLL(None, use_errno=True)
assert libc.prctl(36, 1, 0, 0, 0) == 0
root = pathlib.Path({str(root)!r})
tree = {str(tree)!r}
r3.TIMEOUT = 2
def call(tag):
    env = {{**os.environ, 'TAG': tag}}
    if {launcher!r} == 'run':
        return r3.run([tree], env, timeout=10 if {overlap!r} else 2)
    if {launcher!r} == 'streams':
        return r3.streams(tree, 'unused', env)
    result = eb.execute([tree], root, env, 2)
    assert result.timed_out, result
    return None
tags = ['17', '23'] if {overlap!r} else ['one']
other = subprocess.Popen([sys.executable, '-c', 'raise SystemExit(31)'])
try:
    if {overlap!r}:
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            futures = [pool.submit(call, tag) for tag in tags]
            end = time.monotonic() + 5
            while not all((root / tag).exists() for tag in tags):
                assert time.monotonic() < end, 'children did not become ready'
                time.sleep(0.01)
            (root / 'release').touch()
            assert [f.result() for f in futures] == [17, 23]
    else:
        assert call('one') is None
    assert other.wait() == 31
    for tag in tags:
        pids = list(map(int, (root / tag).read_text().split()))
        assert len(pids) == 3
        for pid in pids:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError(f'process {{pid}} remains alive or unreaped')
finally:
    for tag in tags:
        if (root / tag).exists():
            for pid in map(int, (root / tag).read_text().split()):
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, signal.SIGKILL)
                with contextlib.suppress(ChildProcessError):
                    os.waitpid(pid, 0)
    other.wait()
'''
            done = subprocess.run([sys.executable, '-I', '-c', script], capture_output=True, timeout=20)
            self.assertEqual(done.returncode, 0, done.stderr.decode())
