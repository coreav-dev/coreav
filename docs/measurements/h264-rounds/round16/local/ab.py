"""ab.py ROUNDS BIN_A BIN_B STREAM...: CPU time (user+sys, wait4) of --discard at one thread, pinned to cpu 3, the class
frame count, alternating A and B; per stream the median ratio B/A and the per-round ratios."""
import os, statistics, subprocess, sys
R = int(sys.argv[1]); A, B = sys.argv[2:4]; ss = sys.argv[4:]
ES = os.environ['ES']; BI = os.environ['BINS']
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
def cpu(b, f):
    p = subprocess.Popen(['taskset', '-c', '3', BI + b, '--frames', str(FR[f.split('.')[2]]), '--discard', ES + f],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    _, _, ru = os.wait4(p.pid, 0); return ru.ru_utime + ru.ru_stime
files = {s: next(f for f in os.listdir(ES) if f.startswith(s)) for s in ss}
res = {s: ([], []) for s in ss}
for r in range(R):
    for s in ss:
        for b in ((A, B) if r % 2 == 0 else (B, A)):
            res[s][0 if b == A else 1].append(cpu(b, files[s]))
rs = []
for s in ss:
    a, b = res[s]; q = statistics.median(b) / statistics.median(a); rs.append(q)
    print(f"{files[s][:22]:22} {statistics.median(a):.3f} {statistics.median(b):.3f} {q:.4f}  " + ' '.join(f'{y/x:.3f}' for x, y in zip(a, b)))
print(f'median {statistics.median(rs):.4f}')
