"""Bounded process capture with exact raw extents and streamed frame hashes."""

import hashlib
import json
import os
import selectors
import signal
import subprocess
import time
from datetime import UTC, datetime
from pathlib import Path

from evidence import Invalid
from process import WATCHED, _SignalScope


class ProcessFailure(Invalid):
    pass


class InfrastructureFailure(RuntimeError):
    def __init__(
        self,
        reason,
        primary_failure=None,
        cleanup_failure=None,
        publication_failure=None,
        capture_record=None,
    ):
        super().__init__(reason)
        self.details = {
            "reason": reason,
            "primary_failure": primary_failure,
            "cleanup_failure": cleanup_failure,
            "publication_failure": publication_failure,
            "capture_record": capture_record,
        }


class CleanupFailure(InfrastructureFailure):
    def __init__(
        self,
        primary_failure,
        cleanup_failure,
        publication_failure=None,
        capture_record=None,
    ):
        super().__init__(
            "process_teardown_uncertain",
            primary_failure,
            cleanup_failure,
            publication_failure,
            capture_record,
        )


def diagnostic(error):
    if error is None:
        return None
    cause = error.__cause__
    return {
        "kind": type(error).__name__,
        "message": str(error),
        "cause_kind": type(cause).__name__ if cause else None,
        "cause_message": str(cause) if cause else None,
    }


def utc():
    return datetime.now(UTC).isoformat()


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def publish(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    data = (
        json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + "\n"
    ).encode()
    temporary = path.with_suffix(path.suffix + ".tmp")
    with open(temporary, "wb") as out:
        out.write(data)
        out.flush()
        os.fsync(out.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


class RawHashes:
    def __init__(self, frame_bytes, count):
        if (
            type(frame_bytes) is not int
            or frame_bytes <= 0
            or type(count) is not int
            or not 0 < count <= 1800
        ):
            raise ValueError("invalid_raw_hash_extent")
        self.frame_bytes, self.count = frame_bytes, count
        self.whole, self.frame, self.frame_fill, self.received = (
            hashlib.sha256(),
            hashlib.sha256(),
            0,
            0,
        )
        self.frames = []

    def feed(self, data):
        if self.received + len(data) > self.frame_bytes * self.count:
            raise ProcessFailure("raw_output_exceeds_exact_extent")
        self.whole.update(data)
        self.received += len(data)
        view = memoryview(data)
        while view:
            size = min(len(view), self.frame_bytes - self.frame_fill)
            self.frame.update(view[:size])
            self.frame_fill += size
            view = view[size:]
            if self.frame_fill == self.frame_bytes:
                self.frames.append(self.frame.hexdigest())
                self.frame, self.frame_fill = hashlib.sha256(), 0

    def finish(self):
        if (
            self.received != self.frame_bytes * self.count
            or self.frame_fill
            or len(self.frames) != self.count
        ):
            raise ProcessFailure("raw_output_shorter_than_exact_prefix")
        return {
            "whole_sha256": self.whole.hexdigest(),
            "frame_sha256": self.frames,
            "frame_bytes": self.frame_bytes,
            "frames": self.count,
            "bytes": self.received,
        }


def terminate_group(child, deadline):
    descriptors = []
    discovery_error = None
    try:
        try:
            os.killpg(child.pid, 0)
        except ProcessLookupError:
            if child.poll() is None:
                child.kill()
            child.wait()
            return
        except PermissionError:
            discovery_error = ProcessFailure("group_permission_is_not_absence")
        except BaseException as error:
            discovery_error = error
        try:
            for stat in Path("/proc").glob("[0-9]*/stat"):
                try:
                    fields = stat.read_text().rsplit(")", 1)[1].split()
                    if int(fields[2]) == child.pid:
                        descriptors.append(os.pidfd_open(int(stat.parent.name)))
                except (FileNotFoundError, ProcessLookupError):
                    continue
        except BaseException as error:
            discovery_error = error
        finally:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except OSError as error:
                discovery_error = error
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait()
        with selectors.DefaultSelector() as selector:
            for descriptor in descriptors:
                selector.register(descriptor, selectors.EVENT_READ)
            while selector.get_map():
                events = selector.select(max(0, deadline - time.monotonic()))
                if not events:
                    raise ProcessFailure(
                        "process_group_exit_not_observed_before_deadline"
                    )
                for key, _ in events:
                    selector.unregister(key.fileobj)
        if discovery_error is not None:
            raise ProcessFailure(
                "process_group_discovery_or_kill_error"
            ) from discovery_error
    finally:
        for descriptor in descriptors:
            os.close(descriptor)


def capture(argv, directory, phase, binding, deadline, raw=None, guard=None):
    if guard is None:
        with _SignalScope() as scope:
            return capture(argv, directory, phase, binding, deadline, raw, scope)
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    outpath = directory / (phase + (".raw" if raw else ".stdout"))
    errpath = directory / (phase + ".stderr")
    processpath = directory / (phase + ".process.json")
    record = {
        "argv": list(argv),
        "binding": binding,
        "started_utc": utc(),
        "status": "starting",
        "stderr_policy": "error",
        "limits": {
            "command_seconds": 60,
            "metadata_bytes": 16 << 20,
            "exact_raw_bytes": raw.frame_bytes * raw.count if raw else None,
        },
    }
    publish(processpath, record)
    child, status = None, "ok"
    primary_error, cleanup_error, publication_error = None, None, None
    counts = {"stdout": 0, "stderr": 0}
    stream_hashes = {"stdout": hashlib.sha256(), "stderr": hashlib.sha256()}
    end = time.monotonic() + min(60, max(0, deadline - time.time()))
    try:
        with open(outpath, "wb") as stdout, open(errpath, "wb") as stderr:
            guard.check()
            guard.launching = True
            try:
                child = subprocess.Popen(
                    argv,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    start_new_session=True,
                )
            finally:
                guard.launching = False
            guard.check()
            record["pid"] = child.pid
            publish(processpath, record)
            with selectors.DefaultSelector() as selector:
                selector.register(
                    child.stdout, selectors.EVENT_READ, ("stdout", stdout)
                )
                selector.register(
                    child.stderr, selectors.EVENT_READ, ("stderr", stderr)
                )
                while selector.get_map():
                    remaining = end - time.monotonic()
                    if remaining <= 0:
                        raise ProcessFailure("command_deadline")
                    events = selector.select(remaining)
                    if not events:
                        raise ProcessFailure("command_deadline")
                    for key, _ in events:
                        name, sink = key.data
                        available = (
                            raw.frame_bytes * raw.count - raw.received
                            if raw and name == "stdout"
                            else (16 << 20)
                            - counts["stderr"]
                            - (0 if raw else counts["stdout"])
                        )
                        data = os.read(
                            key.fileobj.fileno(), min(65536, max(1, available + 1))
                        )
                        if not data:
                            selector.unregister(key.fileobj)
                            continue
                        stream_hashes[name].update(data)
                        counts[name] += len(data)
                        if raw and name == "stdout":
                            raw.feed(data)
                        elif (
                            counts["stderr"] + (0 if raw else counts["stdout"])
                            > 16 << 20
                        ):
                            raise ProcessFailure("metadata_output_limit")
                        sink.write(data)
                        if name == "stderr":
                            sink.flush()
                            os.fsync(sink.fileno())
                            raise ProcessFailure("nonempty_error_level_stderr")
            child.wait(timeout=max(0.001, end - time.monotonic()))
            if child.returncode:
                raise ProcessFailure("nonzero_command_exit")
            if raw:
                record["raw_hashes"] = raw.finish()
            for sink in (stdout, stderr):
                sink.flush()
                os.fsync(sink.fileno())
    except BaseException as error:
        primary_error = error
        status = (
            type(error).__name__ + ":" + str(error)
            if isinstance(error, ProcessFailure)
            else "exception:" + type(error).__name__
        )
    finally:
        mask = signal.pthread_sigmask(signal.SIG_BLOCK, WATCHED)
        try:
            if child is not None:
                try:
                    terminate_group(child, end)
                    record["cleanup"] = "group_exit_observed_and_direct_child_reaped"
                except BaseException as error:
                    cleanup_error = error
                    record["cleanup"] = "unconfirmed:" + type(error).__name__
                try:
                    child.stdout.close()
                    child.stderr.close()
                except BaseException as error:
                    cleanup_error = cleanup_error or error
            if cleanup_error is not None:
                status = "fatal_cleanup_uncertain"
                if "raw_hashes" in record:
                    record["unaccepted_raw_hashes"] = record.pop("raw_hashes")
            record.update(
                status=status,
                finished_utc=utc(),
                returncode=child.returncode if child else None,
                output_bytes=counts,
                primary_failure=diagnostic(primary_error),
                cleanup_failure=diagnostic(cleanup_error),
            )
            try:
                record["outputs"] = {
                    q.name: sha(q)
                    for q in (errpath, outpath)
                    if q.exists() and (not raw or q != outpath)
                }
                record["retained_output_bytes"] = {
                    q.name: q.stat().st_size for q in (outpath, errpath) if q.exists()
                }
                record["observed_stream_sha256"] = {
                    name: hasher.hexdigest() for name, hasher in stream_hashes.items()
                }
                publish(processpath, record)
            except BaseException as error:
                publication_error = error
        finally:
            signal.pthread_sigmask(signal.SIG_SETMASK, mask)
    if cleanup_error is not None:
        raise CleanupFailure(
            diagnostic(primary_error),
            diagnostic(cleanup_error),
            diagnostic(publication_error),
            record,
        ) from primary_error
    if publication_error is not None:
        raise InfrastructureFailure(
            "capture_record_publication_failed",
            diagnostic(primary_error),
            None,
            diagnostic(publication_error),
            record,
        ) from publication_error
    if primary_error is not None:
        if isinstance(primary_error, subprocess.TimeoutExpired):
            raise ProcessFailure("command_deadline") from primary_error
        if isinstance(primary_error, (OSError, TypeError, subprocess.SubprocessError)):
            raise InfrastructureFailure(
                "process_capture_infrastructure_failure",
                diagnostic(primary_error),
                capture_record=record,
            ) from primary_error
        raise primary_error
    return record
