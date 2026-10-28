"""ab1t.py ROUNDS OUT BIN...: CPU time (user + sys) of `--discard` at one thread pinned to CPU 3, the class frame
counts, the 32 training streams; the builds in an order rotated each round, one warm-up round not kept. Per stream,
each build's median over the first build's; the median per class and over the 32 with a 95% bootstrap interval."""
import json, os, random, statistics as st, subprocess, sys
R = int(sys.argv[1]); OUT = sys.argv[2]; bins = sys.argv[3:]
ES = '$HOME/tr/es/'; BI = '$HOME/c8/'
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
files = sorted(f for f in os.listdir(ES) if f.endswith('.h264'))
t = {f: {b: [] for b in bins} for f in files}
for r in range(R + 1):
    for f in files:
        for b in bins[r % len(bins):] + bins[:r % len(bins)]:
            p = subprocess.Popen(['taskset', '-c', '3', BI + b, '--frames', str(FR[f.split('.')[2]]), '--discard', ES + f],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            _, s, ru = os.wait4(p.pid, 0)
            if s: raise SystemExit(f'fail {b} {f}')
            if r: t[f][b].append(ru.ru_utime + ru.ru_stime)
    json.dump(t, open(OUT, 'w')); print('round', r, flush=True)
def ci(v):
    rng = random.Random(20261026); m = sorted(st.median(rng.choices(v, k=len(v))) for _ in range(2000)); return m[50], m[1949]
for b in bins[1:]:
    q = {f: st.median(t[f][b]) / st.median(t[f][bins[0]]) for f in files}
    lo, hi = ci(list(q.values()))
    cls = {c: st.median(v for f, v in q.items() if f.split('.')[2] == c) for c in FR}
    print(f'{b}: all {st.median(q.values()):.4f} [{lo:.4f}, {hi:.4f}] ({sum(v < 1 for v in q.values())}/32 faster) '
          + ' '.join(f'{c} {v:.4f}' for c, v in cls.items()))
