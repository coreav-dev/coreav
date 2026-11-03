"""Bind registered native-case media bytes to their producing build and case."""

from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import stat
import time


MAX_FILE_BYTES = 512 << 20
MAX_TARGET_BYTES = 2 << 30


def capture(directory, case, identity, deadline, remaining=MAX_TARGET_BYTES):
    directory = Path(directory)
    if (case.outcome != 'complete' or case.pictures <= 0 or case.capacity <= 0
            or directory.is_symlink() or not directory.is_dir()):
        raise ValueError('media capture requires a complete registered case directory')
    segments = (case.pictures + case.capacity - 1) // case.capacity
    names = {'wire.bin', 'combined.obu', 'samples.ndjson', 'configuration.av1c',
             'complete.json', 'layout.json'} | {f'segment-{number}.obu' for number in range(segments)}
    actual = set()
    for path in directory.iterdir():
        if path.name in {'input.bin', 'stdout.log', 'stderr.log'}:
            continue
        if path.name not in names:
            raise ValueError('native media directory contains an unregistered output file')
        actual.add(path.name)
    if actual != names:
        raise ValueError('native media file inventory differs from the registered case')
    files = {}
    total = 0
    for name in sorted(names):
        if time.monotonic() >= deadline:
            raise ValueError('native media capture deadline exhausted')
        path = directory / name
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, 'rb') as source:
            before = os.fstat(source.fileno())
            if (not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= MAX_FILE_BYTES
                    or before.st_size > remaining - total):
                raise ValueError('native media exceeds its file or target byte budget')
            digest = hashlib.sha256()
            count = 0
            while True:
                if time.monotonic() >= deadline:
                    raise ValueError('native media capture deadline exhausted')
                block = source.read(min(1 << 20, before.st_size - count + 1))
                if not block:
                    break
                count += len(block)
                if count > before.st_size:
                    raise ValueError('native media grew during capture')
                digest.update(block)
            after = os.fstat(source.fileno())
            current = path.lstat()
            fields = ('st_dev', 'st_ino', 'st_mode', 'st_size', 'st_mtime_ns', 'st_ctime_ns')
            if (count != before.st_size
                    or any(getattr(before, field) != getattr(after, field)
                           or getattr(before, field) != getattr(current, field) for field in fields)):
                raise ValueError('native media changed during capture')
        files[name] = {'bytes': count, 'sha256': digest.hexdigest()}
        total += count
    record = {'identity': dict(identity), 'case': json.loads(json.dumps(asdict(case))),
              'files': files, 'total_bytes': total}
    if time.monotonic() >= deadline:
        raise ValueError('native media capture deadline exhausted')
    return record


def verify(directory, case, identity, record, deadline, remaining=MAX_TARGET_BYTES):
    observed = capture(directory, case, identity, deadline, remaining)
    try:
        recorded = json.dumps(record, sort_keys=True, allow_nan=False)
    except (TypeError, ValueError):
        raise ValueError('invalid native media record') from None
    if recorded != json.dumps(observed, sort_keys=True, allow_nan=False):
        raise ValueError('native media bytes or producing identity changed')
    if time.monotonic() >= deadline:
        raise ValueError('native media verification deadline exhausted')
    return observed['total_bytes']
