"""ab.py ROUNDS BINDIR BIN...: CPU time (user+sys) of --discard at one thread, pinned to cpu 3, the class frame counts,
the 32 training streams, the builds in an order rotated each round; per stream the median over rounds of each build,
printed as seconds of the first build and ratios of the others to it (the method of ab3.py in cavlc-p-fused/)."""
import os, statistics, subprocess, sys
R = int(sys.argv[1]); BI = sys.argv[2]; bins = sys.argv[3:]
ES = '/tmp/pmu/up/es/'
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
files = sorted(f for f in os.listdir(ES) if f.endswith('.h264'))
t = {(s, b): [] for s in files for b in bins}
for r in range(R):
    for s in files:
        order = bins[r % len(bins):] + bins[:r % len(bins)]
        for b in order:
            p = subprocess.Popen(['taskset', '-c', '3', os.path.join(BI, b), '--frames', str(FR[s.split('.')[2]]), '--discard', ES + s],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            _, _, ru = os.wait4(p.pid, 0); t[(s, b)].append(ru.ru_utime + ru.ru_stime)
    print(f'round {r + 1} of {R} done', file=sys.stderr, flush=True)
for s in files:
    base = statistics.median(t[(s, bins[0])])
    print(f'{s[:20]:20} {base:.3f}' + ''.join(f'  {b} {statistics.median(t[(s, b)]) / base:.6f}' for b in bins[1:]))
