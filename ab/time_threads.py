import hashlib
import json
import os
from pathlib import Path
import platform
import resource
import statistics
import subprocess
import sys

WORK, BINS, OUT = map(Path, sys.argv[1:4])
THREADS = [int(n) for n in os.environ.get('THREADS', '1,2,4,8,16').split(',')]
CPUS = len(os.sched_getaffinity(0))
assert CPUS >= max(THREADS), 'not enough vCPUs for the requested timing'
EXPECTED = json.loads(Path('ab/timing-expected.json').read_text())
SIZES = {'A': (640, 480), 'B': (368, 800), 'C': (480, 416), 'D': (640, 368), 'E': (640, 368)}
ARMS = [('main', 1)] + [('cand', n) for n in THREADS]
REPEATS = 7


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def call(label, arm, threads):
    w, h = SIZES[label]
    stream, recon = WORK / 'timed.h264', WORK / 'timed.rec'
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    result = subprocess.run([str(BINS / f'{arm}-abt'), str(w), str(h), '27', '60', str(WORK / f'{label}.yuv'), str(stream), str(recon), '32', str(threads)], check=True, capture_output=True, text=True, timeout=600)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    assert [digest(stream), digest(recon)] == EXPECTED[label][arm]
    return {'encode_seconds': float(result.stdout.strip()), 'process_cpu_seconds': after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime}


record = {
    'available_vcpus': CPUS,
    'affinity': sorted(os.sched_getaffinity(0)),
    'uname': platform.uname()._asdict(),
    'cpuinfo': Path('/proc/cpuinfo').read_text(),
    'build': json.loads((BINS / 'build.json').read_text()),
    'repeats': REPEATS,
    'inputs': {},
}
for label in SIZES:
    for arm, threads in ARMS:
        call(label, arm, threads)
    calls = {f'{arm}{threads}': [] for arm, threads in ARMS}
    for repeat in range(REPEATS):
        order = ARMS[repeat % len(ARMS):] + ARMS[:repeat % len(ARMS)]
        for arm, threads in order:
            calls[f'{arm}{threads}'].append(call(label, arm, threads))
    record['inputs'][label] = calls
    OUT.write_text(json.dumps(record, indent=2))
    medians = {key: statistics.median(c['encode_seconds'] for c in values) for key, values in calls.items()}
    print(label, json.dumps(medians), flush=True)
