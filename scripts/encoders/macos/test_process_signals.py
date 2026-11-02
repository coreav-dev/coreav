"""Hosted-macOS signal cleanup using only disjoint Python child programs."""

import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import process


@unittest.skipUnless(sys.platform == "darwin", "kernel process-exit checks run on hosted macOS")
class Signals(unittest.TestCase):
    def test_signal_during_child_publication_still_reaps_child(self):
        children = []
        original = subprocess.Popen
        previous = signal.getsignal(signal.SIGINT)
        mask = signal.pthread_sigmask(signal.SIG_UNBLOCK, (signal.SIGINT,))

        def launch(*args, **kwargs):
            child = original(*args, **kwargs)
            children.append(child)
            signal.raise_signal(signal.SIGINT)
            return child

        signal.signal(signal.SIGINT, signal.default_int_handler)
        try:
            with tempfile.TemporaryDirectory() as directory, \
                    mock.patch.object(process.subprocess, "Popen", side_effect=launch), \
                    self.assertRaises(KeyboardInterrupt):
                process.run([sys.executable, "-c", "import time; time.sleep(60)"], directory, 10)
            self.assertEqual(len(children), 1)
            self.assertEqual(children[0].returncode, -signal.SIGKILL)
            with self.assertRaises(ChildProcessError):
                os.waitpid(children[0].pid, os.WNOHANG)
            self.assertIs(signal.getsignal(signal.SIGINT), signal.default_int_handler)
        finally:
            signal.signal(signal.SIGINT, previous)
            signal.pthread_sigmask(signal.SIG_SETMASK, mask)

    def test_term_and_hup_clean_the_group_before_original_termination(self):
        for signum in (signal.SIGTERM, signal.SIGHUP, signal.SIGQUIT, signal.SIGINT):
            with self.subTest(signal=signum), tempfile.TemporaryDirectory() as temporary, select.kqueue() as queue:
                root = Path(temporary)
                child = root / "child.py"
                child.write_text(
                    "import os,signal,time\nfrom pathlib import Path\n"
                    "signal.signal(signal.SIGTERM,signal.SIG_IGN)\n"
                    "signal.signal(signal.SIGHUP,signal.SIG_IGN)\n"
                    "descendant=os.fork()\n"
                    "if descendant == 0:\n time.sleep(60)\n"
                    "else:\n"
                    " Path('ready.tmp').write_text(str(os.getpid())+' '+str(descendant))\n"
                    " os.replace('ready.tmp','ready')\n time.sleep(60)\n")
                driver = root / "driver.py"
                driver.write_text(
                    "import json,signal,sys\nfrom pathlib import Path\n"
                    f"sys.path.insert(0,{str(Path(process.__file__).parent)!r})\nimport process\n"
                    "signal.signal(signal.SIGHUP,signal.SIG_DFL)\n"
                    "signal.signal(signal.SIGTERM,signal.SIG_DFL)\n"
                    "signal.signal(signal.SIGQUIT,signal.SIG_DFL)\n"
                    "signal.signal(signal.SIGINT,signal.default_int_handler)\n"
                    "signal.pthread_sigmask(signal.SIG_UNBLOCK,process.WATCHED)\n"
                    "Path('result.json').write_text(json.dumps({'status':'aborted'}))\n"
                    "try:\n process.run([sys.executable,'child.py'],'.',45)\n"
                    "except KeyboardInterrupt:\n"
                    " Path('result.json').write_text(json.dumps({'status':'aborted','restored':"
                    "signal.getsignal(signal.SIGINT) is signal.default_int_handler}))\n sys.exit(130)\n"
                    "Path('result.json').write_text(json.dumps({'status':'pass'}))\n")
                directory = os.open(root, os.O_RDONLY)
                running = None
                pids = []
                observed = set()
                try:
                    queue.control([select.kevent(directory, filter=select.KQ_FILTER_VNODE,
                                                  flags=select.KQ_EV_ADD | select.KQ_EV_CLEAR,
                                                  fflags=select.KQ_NOTE_WRITE)], 0, 0)
                    with (root / "driver-out").open("wb") as out, (root / "driver-error").open("wb") as err:
                        running = subprocess.Popen([sys.executable, str(driver)], cwd=root, stdout=out, stderr=err,
                                                   start_new_session=True)
                    queue.control([select.kevent(running.pid, filter=select.KQ_FILTER_PROC,
                                                  flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                                                  fflags=select.KQ_NOTE_EXIT)], 0, 0)
                    deadline = time.monotonic() + 15
                    while not (root / "ready").exists():
                        remaining = deadline - time.monotonic()
                        self.assertGreater(remaining, 0)
                        self.assertTrue(queue.control([], 4, remaining))
                    pids = [int(value) for value in (root / "ready").read_text().split()]
                    self.assertEqual(len(pids), 2)
                    for pid in pids:
                        queue.control([select.kevent(pid, filter=select.KQ_FILTER_PROC,
                                                      flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                                                      fflags=select.KQ_NOTE_EXIT)], 0, 0)
                    os.kill(running.pid, signum)
                    expected = {running.pid, *pids}
                    deadline = time.monotonic() + 15
                    while observed != expected:
                        remaining = deadline - time.monotonic()
                        self.assertGreater(remaining, 0)
                        events = queue.control([], 4, remaining)
                        self.assertTrue(events)
                        observed.update(event.ident for event in events if event.filter == select.KQ_FILTER_PROC)
                    result = running.wait()
                    self.assertEqual(result, 130 if signum == signal.SIGINT else -signum)
                    report = json.loads((root / "result.json").read_text())
                    self.assertEqual(report["status"], "aborted")
                    if signum == signal.SIGINT:
                        self.assertTrue(report["restored"])
                finally:
                    if running is not None and running.returncode is None:
                        for event in queue.control([], 4, 0):
                            if event.filter == select.KQ_FILTER_PROC:
                                observed.add(event.ident)
                        if pids and pids[0] not in observed:
                            try:
                                os.killpg(pids[0], signal.SIGKILL)
                            except ProcessLookupError:
                                pass
                        if running.pid not in observed:
                            running.kill()
                        running.wait()
                    os.close(directory)

    def test_normal_and_failed_children_restore_handlers_and_mask(self):
        handlers = {sig: signal.getsignal(sig) for sig in process.WATCHED}
        mask = signal.pthread_sigmask(signal.SIG_BLOCK, ())
        for code in (0, 1):
            with tempfile.TemporaryDirectory() as directory:
                if code:
                    with self.assertRaisesRegex(process.Invalid, "child_failure"):
                        process.run([sys.executable, "-c", f"raise SystemExit({code})"], directory, 10)
                else:
                    process.run([sys.executable, "-c", f"raise SystemExit({code})"], directory, 10)
            self.assertEqual({sig: signal.getsignal(sig) for sig in process.WATCHED}, handlers)
            self.assertEqual(signal.pthread_sigmask(signal.SIG_BLOCK, ()), mask)


if __name__ == "__main__":
    unittest.main()
