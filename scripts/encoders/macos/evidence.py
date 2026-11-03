"""Closed diagnostics and bounded, hash-bound records for platform experiments."""

import hashlib
import json
import math
import os
from pathlib import Path
import re
import stat


class Invalid(Exception):
    def __init__(self, code):
        super().__init__(code)
        self.code = code


def require(condition, code):
    if not condition:
        raise Invalid(code)


def keys(value, names):
    require(type(value) is dict and set(value) == set(names.split()), "record_shape")
    return value


def integer(value, low, high):
    require(type(value) is int and low <= value <= high, "integer")
    return value


def number(value, low=0, high=1e30):
    require(type(value) in (int, float) and math.isfinite(value)
            and low <= value <= high, "number")
    return value


def digest(value):
    require(type(value) is str and re.fullmatch(r"[0-9a-f]{64}", value), "digest")
    return value


def token(value):
    require(type(value) is str and re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", value), "token")
    return value


def relative(value):
    require(type(value) is str and value and not value.startswith("/"), "path")
    require(all(re.fullmatch(r"[A-Za-z0-9_.-]+", part) and part not in (".", "..")
                for part in value.split("/")), "path")
    return value


def open_file(root, name):
    relative(name)
    root = Path(root)
    require(not root.is_symlink(), "symlink")
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        parts = name.split("/")
        for part in parts[:-1]:
            next_directory = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                     dir_fd=directory)
            os.close(directory)
            directory = next_directory
        return os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
    finally:
        os.close(directory)


def read(root, name, limit):
    fd = open_file(root, name)
    try:
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1
                and info.st_size <= limit, "file_bounds")
        chunks = []
        remaining = limit + 1
        while remaining:
            data = os.read(fd, min(remaining, 1024 * 1024))
            if not data:
                break
            chunks.append(data)
            remaining -= len(data)
        data = b"".join(chunks)
        require(len(data) <= limit and len(data) == info.st_size, "file_changed")
        after = os.fstat(fd)
        require((info.st_size, info.st_mtime_ns, info.st_ctime_ns) ==
                (after.st_size, after.st_mtime_ns, after.st_ctime_ns), "file_changed")
        return data
    finally:
        os.close(fd)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def file_hash(path, limit=16 * 1024**3, root=None):
    path = Path(path)
    fd = open_file(path.parent, path.name) if root is None else open_file(
        root, path.relative_to(root).as_posix())
    try:
        before = os.fstat(fd)
        require(stat.S_ISREG(before.st_mode) and before.st_size <= limit, "file_bounds")
        result = hashlib.sha256()
        size = 0
        while True:
            block = os.read(fd, 1024 * 1024)
            if not block:
                break
            size += len(block)
            require(size <= limit, "file_bounds")
            result.update(block)
        after = os.fstat(fd)
        require(size == before.st_size and
                (before.st_size, before.st_mtime_ns, before.st_ctime_ns) ==
                (after.st_size, after.st_mtime_ns, after.st_ctime_ns), "file_changed")
        return result.hexdigest()
    finally:
        os.close(fd)


def pairs(items):
    result = {}
    for key, value in items:
        require(key not in result, "duplicate_key")
        result[key] = value
    return result


def loads(data):
    try:
        return json.loads(data, object_pairs_hook=pairs,
                          parse_constant=lambda _: (_ for _ in ()).throw(Invalid("number")))
    except (ValueError, UnicodeError, RecursionError):
        raise Invalid("json") from None


def load(path, limit=4 * 1024**2):
    path = Path(path)
    return loads(read(path.parent, path.name, limit))


def write(path, value):
    data = (json.dumps(value, sort_keys=True, indent=2, allow_nan=False) + "\n").encode()
    write_bytes(path, data)


def write_bytes(path, data):
    path = Path(path)
    temporary = path.with_name(path.name + ".tmp")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        temporary.unlink(missing_ok=True)


def bind(root, names, limit):
    return {name: {"sha256": sha(read(root, name, limit)),
                   "bytes": (Path(root) / name).stat().st_size} for name in names}


def verify(root, records, expected, limit):
    require(type(records) is dict and set(records) == set(expected), "file_coverage")
    for name, record in records.items():
        keys(record, "sha256 bytes")
        digest(record["sha256"])
        integer(record["bytes"], 0, limit)
        data = read(root, name, limit)
        require(len(data) == record["bytes"] and sha(data) == record["sha256"], "file_hash")
