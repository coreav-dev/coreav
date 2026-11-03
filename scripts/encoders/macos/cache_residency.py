"""Full-file residency observations from a separately registered vmtouch binary."""

import json
from fractions import Fraction
import os
from pathlib import Path
import re
import tempfile
import time

from evidence import digest, file_hash, integer, keys, read, require, sha
from process import run
from registration import raw_size


def declaration(value):
    if value is None:
        return
    keys(value, "file sha256 kind page_size arguments")
    from evidence import relative
    relative(value["file"])
    digest(value["sha256"])
    require(value["kind"] == "vmtouch-verbose/1", "cache_observer_unsupported")
    require(value["arguments"] == ["-v", "-m", "32G"], "cache_observer_unsupported")
    size = integer(value["page_size"], 1024, 65536)
    require(size & (size - 1) == 0, "cache_observer_unsupported")


def counters(data, extent, page_size):
    require(type(data) is str and len(data) <= 4096, "cache_observer_output")
    files = re.findall(r"^\s*Files:\s*([0-9]+)\s*$", data, re.MULTILINE)
    directories = re.findall(r"^\s*Directories:\s*([0-9]+)\s*$", data, re.MULTILINE)
    pages = re.findall(r"^\s*Resident Pages:\s*([0-9]+)/([0-9]+)\s+([0-9]+(?:\.[0-9]+)?[KMGT]?)/"
                       r"([0-9]+(?:\.[0-9]+)?[KMGT]?)\s+([0-9]+(?:\.[0-9]+)?)%\s*$",
                       data, re.MULTILINE)
    require(len(files) == len(directories) == len(pages) == 1 and int(files[0]) == 1 and
            int(directories[0]) == 0, "cache_observer_output")
    resident, total, resident_size, total_size, percentage = pages[0]
    resident, total = int(resident), int(total)
    require(total == (extent + page_size - 1) // page_size and 0 <= resident <= total and
            0 <= float(percentage) <= 100, "cache_observer_output")
    padding = total * page_size - extent
    for token, count in ((resident_size, resident), (total_size, total)):
        match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)([KMGT]?)", token)
        scale = 1024 ** ("KMGT".index(match[2]) + 1) if match[2] else 1
        amount = Fraction(match[1]) * scale
        low, high = max(0, count * page_size - padding), count * page_size
        if not match[2]:
            require(amount in (low, high), "cache_observer_output")
        else:
            step = Fraction(scale, 10 ** len(match[1].partition(".")[2]))
            require((count == 0 and amount == 0) or (count > 0 and amount > 0 and
                    high >= amount - step / 2 and low < amount + step), "cache_observer_output")
    step = Fraction(1, 10 ** len(percentage.partition(".")[2]))
    require(abs(Fraction(100 * resident, total) - Fraction(percentage)) <= step, "cache_observer_output")
    return resident, total, float(percentage)


def validate(value, registration, item):
    observer = registration.get("cache_observer")
    require(observer is not None, "cache_observer_missing")
    declaration(observer)
    keys(value, "observer_sha256 raw_sha256 bytes page_size file_identity_sha256 stdout started_ns ended_ns")
    require(value["observer_sha256"] == observer["sha256"] and value["raw_sha256"] == item["raw_sha256"] and
            type(value["bytes"]) is int and value["bytes"] == raw_size(item) and
            type(value["page_size"]) is int and value["page_size"] == observer["page_size"], "cache_binding")
    digest(value["file_identity_sha256"])
    integer(value["started_ns"], 1, 2**63 - 1)
    integer(value["ended_ns"], value["started_ns"], 2**63 - 1)
    resident, total, percent = counters(value["stdout"], raw_size(item), observer["page_size"])
    require(resident == total and percent == 100, "cache_not_resident")


def pair(before, after, started, ended, registration, item):
    validate(before, registration, item)
    validate(after, registration, item)
    require(before["ended_ns"] <= started < ended <= after["started_ns"] and
            before["file_identity_sha256"] == after["file_identity_sha256"], "cache_binding")


def observe(registration, root, source, item):
    observer = registration.get("cache_observer")
    require(observer is not None, "cache_observer_missing")
    declaration(observer)
    binary = Path(root) / observer["file"]
    require(file_hash(binary, root=Path(root)) == observer["sha256"], "cache_observer_changed")
    require(os.sysconf("SC_PAGESIZE") == observer["page_size"], "cache_observer_unsupported")
    source = Path(source)
    require(not source.is_symlink(), "symlink")
    before = source.stat()
    require(before.st_size == raw_size(item), "input_hash")
    started = time.time_ns()
    with tempfile.TemporaryDirectory() as temporary:
        run([str(binary), *observer["arguments"], str(source)], temporary, 30)
        require(not read(temporary, "stderr", 1024**2), "cache_observer_output")
        text = read(temporary, "stdout", 1024**2).decode("ascii")
    ended = time.time_ns()
    safe = "\n".join(line.strip() for line in text.splitlines()
                     if re.match(r"^\s*(?:Files|Directories|Resident Pages):", line)) + "\n"
    counters(safe, raw_size(item), observer["page_size"])
    after = source.stat()
    identity = lambda s: (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)
    require(identity(before) == identity(after), "input_changed")
    require(file_hash(binary, root=Path(root)) == observer["sha256"], "cache_observer_changed")
    return {"observer_sha256": observer["sha256"], "raw_sha256": item["raw_sha256"], "bytes": before.st_size,
            "page_size": observer["page_size"], "file_identity_sha256": sha(json.dumps(identity(before)).encode()),
            "stdout": safe, "started_ns": started, "ended_ns": ended}


def preflight(registration, root):
    observer = registration.get("cache_observer")
    require(observer is not None, "cache_observer_missing")
    declaration(observer)
    with tempfile.TemporaryDirectory() as temporary:
        source = Path(temporary) / "probe"
        data = b"0" * 24576
        source.write_bytes(data)
        require(source.read_bytes() == data, "cache_observer_output")
        item = {"width": 128, "height": 128, "frames": 1, "raw_sha256": sha(data)}
        value = observe(registration, root, source, item)
        validate(value, registration, item)
