"""Process-exit events, with no pipe-EOF or polling-sleep clock."""

import errno
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import threading
import time

from evidence import Invalid, require


WATCHED = (signal.SIGHUP, signal.SIGTERM, signal.SIGQUIT, signal.SIGINT)


class _Interrupted(BaseException):
    def __init__(self, signum, frame):
        super().__init__("interrupted")
        self.signum, self.frame = signum, frame


class _SignalScope:
    def __enter__(self):
        require(threading.current_thread() is threading.main_thread(), "signal_thread")
        self.pending = None
        self.launching = False
        self.mask = signal.pthread_sigmask(signal.SIG_BLOCK, WATCHED)
        self.handlers = {sig: signal.getsignal(sig) for sig in WATCHED}
        try:
            for sig, handler in self.handlers.items():
                if handler != signal.SIG_IGN:
                    signal.signal(sig, self.interrupt)
        finally:
            signal.pthread_sigmask(signal.SIG_SETMASK, self.mask)
        return self

    def interrupt(self, signum, frame):
        if self.pending is None:
            self.pending = _Interrupted(signum, frame)
        if not self.launching:
            raise self.pending

    def check(self):
        if self.pending is not None:
            raise self.pending

    def __exit__(self, kind, error, traceback):
        signal.pthread_sigmask(signal.SIG_BLOCK, WATCHED)
        for sig, handler in self.handlers.items():
            signal.signal(sig, handler)
        stopped = self.pending
        if stopped is not None and self.handlers[stopped.signum] == signal.SIG_DFL:
            signal.pthread_sigmask(signal.SIG_UNBLOCK, (stopped.signum,))
            os.kill(os.getpid(), stopped.signum)
            raise Invalid("interrupted")
        signal.pthread_sigmask(signal.SIG_SETMASK, self.mask)
        if stopped is not None:
            handler = self.handlers[stopped.signum]
            if callable(handler):
                handler(stopped.signum, stopped.frame)
            raise Invalid("interrupted")
        return False


def register(queue, child):
    event = select.kevent(child.pid, filter=select.KQ_FILTER_PROC,
                          flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                          fflags=select.KQ_NOTE_EXIT)
    try:
        queue.control([event], 0, 0)
        return True
    except OSError as error:
        if error.errno != errno.ESRCH:
            raise
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        pid, status = os.waitpid(child.pid, os.WNOHANG)
        require(pid == child.pid, "exit_registration")
        child.returncode = os.waitstatus_to_exitcode(status)
        return False


def run(command, directory, timeout, stdout="stdout", stderr="stderr", env=None, cwd=None):
    with _SignalScope() as guard:
        return _run(command, directory, timeout, stdout, stderr, env, cwd, guard)


def _run(command, directory, timeout, stdout, stderr, env, cwd, guard):
    require(0 < timeout <= 3600, "deadline")
    require(sys.platform in ("darwin", "linux"), "process_platform")
    directory = Path(directory)
    with (directory / stdout).open("xb") as out, (directory / stderr).open("xb") as err:
        queue = select.kqueue() if sys.platform == "darwin" else None
        pidfd = None
        child = None
        exited = False
        registered = False
        try:
            started = time.monotonic_ns()
            guard.launching = True
            try:
                child = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=out,
                                         stderr=err, start_new_session=True, env=env, cwd=cwd)
            finally:
                guard.launching = False
            guard.check()
            if queue is not None:
                registered = register(queue, child)
                exited = not registered
                if registered:
                    remaining = max(0, timeout - (time.monotonic_ns() - started) / 1e9)
                    events = queue.control([], 1, remaining)
                    require(not events or not events[0].flags & select.KQ_EV_ERROR,
                            "exit_registration")
                    exited = bool(events)
            else:
                pidfd = os.pidfd_open(child.pid)
                remaining = max(0, timeout - (time.monotonic_ns() - started) / 1e9)
                exited = bool(select.select([pidfd], [], [], remaining)[0])
            ended = time.monotonic_ns()
            if not exited:
                raise Invalid("timeout")
            if child.returncode is None:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            code = child.wait()
            require(code >= 0, "child_crash")
            require(code == 0, "child_failure")
            require(ended - started <= timeout * 1e9, "timeout")
            return ended - started
        except OSError:
            raise Invalid("process_infrastructure") from None
        finally:
            signal.pthread_sigmask(signal.SIG_BLOCK, WATCHED)
            try:
                if child is not None and child.returncode is None:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    if not exited:
                        if queue is not None:
                            if not registered:
                                registered = register(queue, child)
                                exited = not registered
                            if not exited:
                                events = queue.control([], 1, 10)
                                require(not events or not events[0].flags & select.KQ_EV_ERROR, "cleanup_failed")
                                exited = bool(events)
                        else:
                            if pidfd is None:
                                pidfd = os.pidfd_open(child.pid)
                            exited = bool(select.select([pidfd], [], [], 10)[0])
                    require(exited, "cleanup_failed")
                    child.wait()
            except OSError:
                raise Invalid("cleanup_failed") from None
            finally:
                if pidfd is not None:
                    os.close(pidfd)
                if queue is not None:
                    queue.close()
