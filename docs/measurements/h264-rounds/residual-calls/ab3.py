"""ab3.py ROUNDS STREAMS BIN...: CPU time (user+sys) of --discard at one thread, pinned to cpu 3, class frame count, the
builds in an order rotated each round; per stream each build's median over the first."""
import os, statistics, subprocess, sys
R = int(sys.argv[1]); ss = sys.argv[2].split(','); bins = sys.argv[3:]
ES = '/tmp/pmu/up/es/'; BI = '/tmp/rb/'
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
files = {s: next(f for f in os.listdir(ES) if f.startswith(s)) for s in ss}
t = {(s, b): [] for s in ss for b in bins}
for r in range(R):
    for s in ss:
        order = bins[r % len(bins):] + bins[:r % len(bins)]
        for b in order:
            p = subprocess.Popen(['taskset', '-c', '3', BI + b, '--frames', str(FR[files[s].split('.')[2]]), '--discard', ES + files[s]], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            _, _, ru = os.wait4(p.pid, 0); t[(s, b)].append(ru.ru_utime + ru.ru_stime)
med = {b: [] for b in bins}
for s in ss:
    base = statistics.median(t[(s, bins[0])])
    line = f'{files[s][:20]:20} {base:.3f}'
    for b in bins[1:]:
        q = statistics.median(t[(s, b)]) / base; med[b].append(q); line += f'  {b} {q:.4f}'
    print(line)
print('median', '  '.join(f'{b} {statistics.median(v):.4f}' for b, v in med.items() if v))
