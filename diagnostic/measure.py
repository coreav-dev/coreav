import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys

import numpy as np
from scipy.interpolate import PchipInterpolator

ROOT = Path('/tmp/prune')
OUT = ROOT / 'results'
OUT.mkdir(exist_ok=True)
sys.path.insert(0, str(Path('scripts').resolve()))
import test_oracle

oracle = test_oracle.require(ROOT / 'bin/ffmpeg', ROOT / 'bin/ffprobe')
cpus = sorted(os.sched_getaffinity(0))
env = {k: v for k, v in os.environ.items() if not k.startswith(('R2_', 'AWS_', 'GH_TOKEN', 'GITHUB_TOKEN'))}
ARMS = ['main', *os.environ['ARMS'].split(',')]
INPUTS = [('A', 640, 480), ('B', 368, 800), ('C', 480, 416), ('D', 640, 368), ('E', 640, 368)]
CURVE = (22, 28, 34, 40)
TIMED = (22, 28)
stream, recon, decoded = ROOT / 'out.h264', ROOT / 'out.rec', ROOT / 'decoded.yuv'


def run(args, **kw):
    return subprocess.run(list(map(str, args)), check=True, capture_output=True, text=True, timeout=2400, **kw)


def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def cmd(arm, label, w, h, qp):
    return [ROOT / 'bin' / arm, w, h, qp, 60, ROOT / 'work' / f'{label}.yuv', stream, recon, 32]


def sse(label, w, h):
    size = w * h * 3 // 2
    a = np.fromfile(ROOT / 'work' / f'{label}.yuv', dtype=np.uint8).reshape(60, size)
    b = np.fromfile(recon, dtype=np.uint8).reshape(60, size)
    result, start = {}, 0
    for plane, samples in zip('yuv', (w * h, w * h // 4, w * h // 4)):
        d = a[:, start:start + samples].astype(np.int32) - b[:, start:start + samples].astype(np.int32)
        result[plane] = [int(np.sum(d * d, dtype=np.int64)), 60 * samples]
        start += samples
    return result


def bd_rate(candidate, baseline):
    def curve(points):
        points = sorted(points)
        x = np.array([p[0] for p in points], dtype=float)
        assert np.all(np.diff(x) > 0)
        return x, PchipInterpolator(x, np.log(np.array([p[1] for p in points], dtype=float)), extrapolate=False)
    a, ac = curve(candidate)
    b, bc = curve(baseline)
    low, high = max(a[0], b[0]), min(a[-1], b[-1])
    assert high > low
    return float(100 * math.expm1((ac.integrate(low, high) - bc.integrate(low, high)) / (high - low)))


record = dict(base=os.environ['BASE'], arms=ARMS, host=run(['lscpu']).stdout, rustc=run(['rustc', '-Vv']).stdout,
              bins={a: digest(ROOT / 'bin' / a) for a in ARMS}, curve={}, times={}, times4={}, counts={}, identity_checks=0, oracle_decodes=0)


def save():
    (OUT / 'pruning.json').write_text(json.dumps(record, indent=1))


for label, w, h in INPUTS:
    for qp in CURVE:
        for arm in ARMS:
            run(cmd(arm, label, w, h, qp), env=dict(env, THREADS='1'))
            hashes = [digest(stream), digest(recon)]
            assert recon.stat().st_size == w * h * 3 // 2 * 60
            run([oracle.ffmpeg, '-nostdin', '-v', 'error', '-xerror', '-y', '-i', stream, '-f', 'rawvideo', '-pix_fmt', 'yuv420p', decoded])
            assert digest(decoded) == hashes[1], (label, qp, arm, 'oracle')
            record['oracle_decodes'] += 1
            cell = dict(bytes=stream.stat().st_size, hashes=hashes, sse=sse(label, w, h))
            run(cmd(arm, label, w, h, qp), env=dict(env, THREADS='4'))
            assert [digest(stream), digest(recon)] == hashes, (label, qp, arm, 'threads')
            record['identity_checks'] += 1
            record['curve'][f'{label}-{qp}-{arm}'] = cell
        print('curve', label, qp, flush=True)
    save()

for label, w, h in INPUTS:
    for qp in TIMED:
        key = f'{label}-{qp}'
        for repeat in range(6):
            order = ARMS[repeat % len(ARMS):] + ARMS[:repeat % len(ARMS)]
            if repeat % 2:
                order.reverse()
            for threads in (1, 4):
                affinity = str(cpus[0]) if threads == 1 else ','.join(map(str, cpus[:4]))
                for arm in order:
                    result = run(['taskset', '-c', affinity, *cmd(arm, label, w, h, qp)], env=dict(env, THREADS=str(threads)))
                    assert [digest(stream), digest(recon)] == record['curve'][f'{key}-{arm}']['hashes']
                    record['identity_checks'] += 1
                    if repeat:
                        record['times' if threads == 1 else 'times4'].setdefault(f'{key}-{arm}', []).append(float(result.stdout))
        print('timed', key, flush=True)
    save()

for label, w, h in INPUTS:
    for qp in TIMED:
        key = f'{label}-{qp}'
        for arm in ARMS:
            cg = OUT / f'{key}-{arm}.cg'
            run(['taskset', '-c', cpus[0], 'valgrind', '--tool=callgrind', '--collect-atstart=no', '--toggle-collect=*measured_encode*',
                 '--cache-sim=no', '--branch-sim=no', f'--callgrind-out-file={cg}', *cmd(arm, label, w, h, qp)], env=dict(env, THREADS='1'))
            text = cg.read_text()
            assert 'coreav_h264_encoder' in text
            count = int(re.search(r'^summary: (\d+)', text, re.M)[1])
            assert count > 1000000
            record['counts'][f'{key}-{arm}'] = count
            assert [digest(stream), digest(recon)] == record['curve'][f'{key}-{arm}']['hashes']
            record['identity_checks'] += 1
            cg.unlink()
        print('counted', key, flush=True)
    save()

summary = {}
for arm in ARMS[1:]:
    rows = {}
    for label, w, h in INPUTS:
        row = {}
        for plane in 'yuv':
            curves = []
            for a in (arm, 'main'):
                points = []
                for qp in CURVE:
                    c = record['curve'][f'{label}-{qp}-{a}']
                    s, n = c['sse'][plane]
                    points.append((10 * math.log10(255 ** 2 * n / s), c['bytes'] * 8 / 2))
                curves.append(points)
            row[plane] = bd_rate(*curves)
        for qp in TIMED:
            key = f'{label}-{qp}'
            row[f'time{qp}'] = float(np.median(record['times'][f'{key}-{arm}']) / np.median(record['times'][f'{key}-main']))
            row[f'ir{qp}'] = record['counts'][f'{key}-{arm}'] / record['counts'][f'{key}-main']
            row[f'time4_{qp}'] = float(np.median(record['times4'][f'{key}-{arm}']) / np.median(record['times4'][f'{key}-main']))
        rows[label] = row
    summary[arm] = rows
record['summary'] = summary
save()
print(json.dumps(summary, indent=1))
