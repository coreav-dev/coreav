"""ab8.py ROUNDS OUT BIN...: wall and CPU of --discard at 8 threads, unpinned, class frame counts, all 32 streams,
builds rotated each round; per stream each build's median over the first; median per class and overall, bootstrap CI."""
import json, os, random, statistics as st, subprocess, sys, time
R = int(sys.argv[1]); OUT = sys.argv[2]; bins = sys.argv[3:]
ES = '$HOME/h264work/train/es/'; BI = '$HOME/h264work/bins/'
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
files = sorted(os.listdir(ES)); t = {f: {b: [] for b in bins} for f in files}
for r in range(R + 1):
    for f in files:
        order = bins[r % len(bins):] + bins[:r % len(bins)]
        for b in order:
            t0 = time.perf_counter()
            p = subprocess.Popen([BI + b, '--frames', str(FR[f.split('.')[2]]), '--threads', '8', '--discard', ES + f],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            _, s, ru = os.wait4(p.pid, 0); w = time.perf_counter() - t0
            if s: raise SystemExit(f'fail {b} {f}')
            if r: t[f][b].append((w, ru.ru_utime + ru.ru_stime))
    json.dump(t, open(OUT, 'w')); print('round', r, flush=True)
def ci(v):
    rng = random.Random(20261025); m = sorted(st.median(rng.choices(v, k=len(v))) for _ in range(2000)); return m[50], m[1949]
for i, name in ((0, 'wall'), (1, 'cpu')):
    for b in bins[1:]:
        q = {f: st.median(x[i] for x in t[f][b]) / st.median(x[i] for x in t[f][bins[0]]) for f in files}
        cls = {c: st.median(v for f, v in q.items() if f.split('.')[2] == c) for c in FR}
        lo, hi = ci(list(q.values()))
        print(f'{name} {b}: all {st.median(q.values()):.4f} [{lo:.4f}, {hi:.4f}] ({sum(v < 1 for v in q.values())}/32 faster) '
              + ' '.join(f'{c} {v:.4f}' for c, v in cls.items()))
