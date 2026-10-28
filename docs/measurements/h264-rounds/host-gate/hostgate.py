#!/usr/bin/env python3
"""Host-health gate for H.264 A/B timing: FFmpeg 9.0.2's time, pinned to core 2, on two fixed reference streams
must be within +-5% of the recorded reference of the host's class. Run before and after every timing call.

    python3 -I hostgate.py LABEL        # prints PASS|FAIL, exits 0|1

The command timed is scripts/h264_parity.py's `ff_null` for the stream:
    taskset -c 2 ffmpeg -nostdin -v error -threads 1 -i ES -frames:v F -f null -
three times, median. Streams are the training set's elementary streams (docs/measurements/h264-training/es.json), F as the
class's frame count. Class of the host: Intel Xeon family 6 model 106 (2.90 GHz) or model 85 (2.50 GHz), from /proc/cpuinfo.
Environment overrides: FFMPEG (default ~/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg), ES_DIR (default ~/tr/es).
"""
import os, re, statistics, subprocess, sys, time

FF = os.environ.get('FFMPEG', os.path.expanduser('~/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg'))
E = os.environ.get('ES_DIR', os.path.expanduser('~/tr/es'))
STREAMS = {'36501d448eaa.0.720p': 900, 'a6af290d9053.0.1080p': 300}
# Reference seconds per host class. 2.90 GHz model 106: median over five gating runs (round 11, round 12, motion-12b, intra,
# deblock-pair; spread at most 3.5% on these streams). 2.50 GHz model 85: mean of two runs (prefetch-skip A/B 1.520 and 5.225,
# chroma-MC info run 1.537 and 5.448; spread 1.1% and 4.2%), provisional until a third run exists.
REF = {
    106: {'36501d448eaa.0.720p': 1.295, 'a6af290d9053.0.1080p': 4.388},
    85: {'36501d448eaa.0.720p': 1.529, 'a6af290d9053.0.1080p': 5.337},
}
TOL = 0.05


def model():
    m = re.search(r'^model\s*:\s*(\d+)', open('/proc/cpuinfo').read(), re.M)
    return int(m.group(1)) if m else -1


mod = model()
if mod not in REF:
    print(f'FAIL no reference for cpu model {mod}'); sys.exit(2)
ok = True; out = []
for name, frames in STREAMS.items():
    ts = []
    for _ in range(3):
        t = time.perf_counter()
        subprocess.run(['taskset', '-c', '2', FF, '-nostdin', '-v', 'error', '-threads', '1', '-i', f'{E}/{name}.h264',
                        '-frames:v', str(frames), '-f', 'null', '-'], check=True, stderr=subprocess.DEVNULL)
        ts.append(time.perf_counter() - t)
    ref = REF[mod][name]; r = statistics.median(ts) / ref
    out.append(f'{name} {statistics.median(ts):.3f}s (ref {ref}) ratio {r:.3f}')
    ok = ok and abs(r - 1) <= TOL
print(('PASS' if ok else 'FAIL'), f'model{mod}', sys.argv[1] if len(sys.argv) > 1 else '', time.strftime('%H:%MZ', time.gmtime()), '; '.join(out), flush=True)
sys.exit(0 if ok else 1)
