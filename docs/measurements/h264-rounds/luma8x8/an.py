# an.py FILE A B: per-stream median CPU time ratio B/A over rounds; median over streams, 95% bootstrap, per class.
import sys, json, statistics as st, random, collections
rows = [json.loads(l) for l in open(sys.argv[1])]; A, B = sys.argv[2], sys.argv[3]
t = collections.defaultdict(lambda: collections.defaultdict(list))
for r in rows: t[r['stream']][r['arm']].append(r['s'])
rat = {s: st.median(v[B]) / st.median(v[A]) for s, v in t.items() if v[A] and v[B]}
def ci(xs):
    rng = random.Random(7); m = sorted(st.median(rng.choices(xs, k=len(xs))) for _ in range(2000))
    return m[50], m[1949]
def line(name, xs):
    lo, hi = ci(xs); print(f'{name:8s} n={len(xs):2d} {st.median(xs):.4f} [{lo:.4f},{hi:.4f}] faster {sum(x < 1 for x in xs)}/{len(xs)}')
print(f'{B}/{A}, rounds {max(r["round"] for r in rows) + 1}')
line('all', list(rat.values()))
for c in ['sd', '720p', '1080p', '4k']: line(c, [v for s, v in rat.items() if s.split('.')[2] == c])
import os
sub = os.environ.get('SUBSET')
if sub: line('subset', [v for s, v in rat.items() if s.split('.')[0] in sub.split(',')])
for s in sorted(rat): print(f'  {s} {rat[s]:.4f}')
