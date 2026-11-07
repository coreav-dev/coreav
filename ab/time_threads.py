import hashlib
import json
import os
from pathlib import Path
import platform
import re
import resource
import statistics
import subprocess
import sys
import time

WORK, BINS, OUT = map(Path, sys.argv[1:4])
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import test_oracle
ORACLE = test_oracle.require(BINS / 'ffmpeg', BINS / 'ffprobe')
FFMPEG = str(ORACLE.ffmpeg)
SIZES = {'A': (640, 480), 'B': (368, 800), 'C': (480, 416), 'D': (640, 368), 'E': (640, 368)}
QPS = [22, 27, 32, 37, 40]
THREADS = sorted({1, len(os.sched_getaffinity(0))})
ARMS = [(arm, n) for n in THREADS for arm in ['coreav', 'matched', 'medium']]

def run(args):
    return subprocess.run(args, check=True, capture_output=True, text=True, timeout=600)

def call(label, arm, threads, qp):
    w, h = SIZES[label]
    source = WORK / f'{label}.yuv'
    stream, recon, decoded = [WORK / name for name in ['timed.h264', 'timed.rec', 'decoded.yuv']]
    if arm == 'coreav':
        args = [str(BINS / 'cand-abt'), str(w), str(h), str(qp), '60', str(source), str(stream), str(recon), '32', str(threads)]
    else:
        args = [FFMPEG, '-nostdin', '-v', 'error', '-y', '-f', 'rawvideo', '-pix_fmt', 'yuv420p', '-s', f'{w}x{h}', '-r', '30', '-i', str(source), '-frames:v', '60', '-an', '-c:v', 'libx264', '-preset', 'medium', '-qp', str(qp), '-threads', str(threads)]
        if arm == 'matched':
            args += ['-profile:v', 'baseline', '-tune', 'psnr', '-x264-params', 'cabac=0:bframes=0:ref=1:keyint=32:min-keyint=32:scenecut=0:8x8dct=0']
        args += ['-f', 'h264', str(stream)]
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter()
    result = run(args)
    wall = time.perf_counter() - start
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    run([FFMPEG, '-nostdin', '-v', 'error', '-xerror', '-y', '-i', str(stream), '-pix_fmt', 'yuv420p', '-f', 'rawvideo', str(decoded)])
    assert decoded.stat().st_size == source.stat().st_size == w * h * 3 // 2 * 60
    if arm == 'coreav':
        assert hashlib.sha256(decoded.read_bytes()).digest() == hashlib.sha256(recon.read_bytes()).digest()
    metric = run([FFMPEG, '-nostdin', '-v', 'info', '-f', 'rawvideo', '-pix_fmt', 'yuv420p', '-s', f'{w}x{h}', '-r', '30', '-i', str(source), '-f', 'rawvideo', '-pix_fmt', 'yuv420p', '-s', f'{w}x{h}', '-r', '30', '-i', str(decoded), '-lavfi', 'psnr', '-f', 'null', '-'])
    match = re.search(r'PSNR y:([\d.]+) u:([\d.]+) v:([\d.]+)', metric.stderr)
    assert match, metric.stderr
    y, u, v = map(float, match.groups())
    return {'wall': wall, 'cpu': after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime, 'encode_internal': float(result.stdout) if arm == 'coreav' else None, 'bytes': stream.stat().st_size, 'sha256': hashlib.sha256(stream.read_bytes()).hexdigest(), 'psnr': [y, u, v, (6*y+u+v)/8]}

record = {'available_vcpus': len(os.sched_getaffinity(0)), 'affinity': sorted(os.sched_getaffinity(0)), 'uname': platform.uname()._asdict(), 'lscpu': run(['lscpu', '-J']).stdout, 'cpuinfo': Path('/proc/cpuinfo').read_text(), 'smt_active': Path('/sys/devices/system/cpu/smt/active').read_text().strip(), 'build': json.loads((BINS / 'build.json').read_text()), 'ffmpeg_version': run([FFMPEG, '-version']).stdout, 'uptime_before': run(['uptime']).stdout, 'repeats': 3, 'inputs': {}}
for label in SIZES:
    cases = {}
    for qp in QPS:
        for arm, threads in ARMS:
            call(label, arm, threads, qp)
        calls = {f'{arm}-{threads}': [] for arm, threads in ARMS}
        for repeat in range(3):
            order = ARMS[repeat % len(ARMS):] + ARMS[:repeat % len(ARMS)]
            for arm, threads in order:
                calls[f'{arm}-{threads}'].append(call(label, arm, threads, qp))
        cases[str(qp)] = calls
        record['inputs'][label] = cases
        OUT.write_text(json.dumps(record, indent=2))
        print(f'completed {label} qp {qp}', flush=True)
record['uptime_after'] = run(['uptime']).stdout
OUT.write_text(json.dumps(record, indent=2))
