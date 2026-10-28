"""ab8.py ROUNDS THREADS STREAMS BIN...: wall time of --discard at THREADS threads, class frame count, builds rotated each
round; per stream each build's median over the first's."""
import os, statistics, subprocess, sys, time
R = int(sys.argv[1]); T = sys.argv[2]; ss = sys.argv[3].split(','); bins = sys.argv[4:]
ES = '$HOME/h264work/train/es/'; BI = '$HOME/h264work/bins/'
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
if ss == ['all']: ss = sorted(f.split('.')[0] for f in os.listdir(ES))
files = {s: next(f for f in os.listdir(ES) if f.startswith(s)) for s in ss}
t = {(s, b): [] for s in ss for b in bins}
for r in range(R):
    for s in ss:
        order = bins[r % len(bins):] + bins[:r % len(bins)]
        for b in order:
            a = time.perf_counter()
            subprocess.run([BI + b, '--threads', T, '--frames', str(FR[files[s].split('.')[2]]), '--discard', ES + files[s]], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            t[(s, b)].append(time.perf_counter() - a)
med = {b: {} for b in bins}
for s in ss:
    base = statistics.median(t[(s, bins[0])])
    line = f'{files[s][:20]:20} {base:.3f}'
    for b in bins[1:]:
        q = statistics.median(t[(s, b)]) / base; med[b][s] = q; line += f'  {b} {q:.4f}'
    print(line)
for b, v in med.items():
    if not v: continue
    out = [f'{b} all {statistics.median(v.values()):.4f}']
    for c in FR:
        q = [x for s, x in v.items() if files[s].split('.')[2] == c]
        if q: out.append(f'{c} {statistics.median(q):.4f}')
    print('median', '  '.join(out))
